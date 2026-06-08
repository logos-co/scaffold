use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::bail;

use crate::process::{run_capture, run_forwarded};
use crate::DynResult;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum SourceMismatchPolicy {
    #[default]
    Fail,
    AutoRecloneIfClean,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RepoSyncOptions {
    pub(crate) source_mismatch: SourceMismatchPolicy,
}

impl RepoSyncOptions {
    pub(crate) fn fail_on_source_mismatch() -> Self {
        Self {
            source_mismatch: SourceMismatchPolicy::Fail,
        }
    }

    pub(crate) fn auto_reclone_cache_repo() -> Self {
        Self {
            source_mismatch: SourceMismatchPolicy::AutoRecloneIfClean,
        }
    }
}

pub(crate) fn sync_repo_to_pin_at_path_with_opts(
    path: &Path,
    source: &str,
    pin: &str,
    label: &str,
    opts: RepoSyncOptions,
) -> DynResult<()> {
    ensure_repo_present(path, source, label, opts)?;

    let _ = run_forwarded(
        Command::new("git")
            .current_dir(path)
            .arg("fetch")
            .arg("--all")
            .arg("--tags"),
        &format!("git fetch ({label})"),
    );

    let resolved_pin = ensure_pin_exists(path, source, pin, label)?;

    run_forwarded(
        Command::new("git")
            .current_dir(path)
            .arg("checkout")
            .arg(pin),
        &format!("git checkout pin ({label})"),
    )?;

    let head = git_head_sha(path)?;
    if head != resolved_pin {
        // Comparison is against the resolved SHA, not the raw `pin` string —
        // otherwise tag/branch pins (e.g. `pin = "v0.2.0"`) always trip this
        // assertion even though the checkout succeeded.
        bail!(
            "{label} pin mismatch after checkout (expected {resolved_pin} resolved from {pin}, got {head})"
        );
    }

    Ok(())
}

/// Verify that `pin` exists in the repo at `path` and return its resolved
/// commit SHA. Accepts any revision spec `git rev-parse` understands (SHA,
/// short SHA, tag, branch); the returned value is always a 40-hex SHA so
/// callers can compare it byte-for-byte against `HEAD`.
pub(crate) fn ensure_pin_exists(
    path: &Path,
    source: &str,
    pin: &str,
    label: &str,
) -> DynResult<String> {
    let rev = format!("{pin}^{{commit}}");
    match run_capture(
        Command::new("git")
            .current_dir(path)
            .arg("rev-parse")
            .arg("--verify")
            .arg(&rev),
        &format!("verify pin ({label})"),
    ) {
        Ok(captured) => Ok(captured.stdout.trim().to_string()),
        Err(_) => bail!(
            "configured {label} pin {pin} is not available in {} from source `{source}`. Ensure the repo source contains this commit (try `--lez-path` pointing to a repo that has it).",
            path.display(),
        ),
    }
}

pub(crate) fn ensure_repo_present(
    path: &Path,
    source: &str,
    label: &str,
    opts: RepoSyncOptions,
) -> DynResult<()> {
    if path.exists() {
        if path.join(".git").exists() {
            reconcile_repo_source(path, source, label, opts)?;
            return Ok(());
        }
        bail!("{} exists but is not a git repo: {}", label, path.display());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    run_forwarded(
        Command::new("git")
            .arg("clone")
            .arg("--no-hardlinks")
            .arg("--")
            .arg(source)
            .arg(path),
        &format!("clone {label}"),
    )
}

fn reconcile_repo_source(
    path: &Path,
    source: &str,
    label: &str,
    opts: RepoSyncOptions,
) -> DynResult<()> {
    let Some(origin) = git_origin_url(path)? else {
        return Ok(());
    };

    if source_matches(path, source, &origin) {
        return Ok(());
    }

    match opts.source_mismatch {
        SourceMismatchPolicy::Fail => bail!(
            "{label} repository at {} uses origin `{origin}`, which does not match requested source `{source}`. Refusing to reuse this repo. Use `--lez-path` with a matching repo, choose a different `--cache-root`, or remove the stale cache repo and retry.",
            path.display(),
        ),
        SourceMismatchPolicy::AutoRecloneIfClean => {
            if !git_clean(path)? {
                bail!(
                    "{label} repository at {} has origin `{origin}` (expected `{source}`) and has local changes. Refusing to auto-refresh this cache repo; clean/remove it manually and retry.",
                    path.display(),
                );
            }

            // Validate the new source is reachable BEFORE removing the
            // cache. A typo on `--lez-path /nonexistent` used to wipe the
            // populated cache and force a fresh GitHub clone on the next
            // valid invocation. Now we bail with the cache intact.
            if !source_is_reachable(source) {
                bail!(
                    "{label} source `{source}` is not reachable (path does not exist, is not a git repo, or remote refused HEAD probe). \
                     Refusing to discard the existing cache at {} — fix the path/URL and retry.",
                    path.display(),
                );
            }

            fs::remove_dir_all(path)?;
            run_forwarded(
                Command::new("git")
                    .arg("clone")
                    .arg("--no-hardlinks")
                    .arg("--")
                    .arg(source)
                    .arg(path),
                &format!("refresh clone {label}"),
            )?;
        }
    }

    Ok(())
}

fn git_origin_url(repo: &Path) -> DynResult<Option<String>> {
    let output = Command::new("git")
        .current_dir(repo)
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .output()?;

    if !output.status.success() {
        return Ok(None);
    }

    let origin = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if origin.is_empty() {
        Ok(None)
    } else {
        Ok(Some(origin))
    }
}

fn source_matches(repo_path: &Path, expected_source: &str, existing_origin: &str) -> bool {
    let expected = expected_source.trim();
    let origin = existing_origin.trim();

    let expected_is_url = looks_like_url(expected);
    let origin_is_url = looks_like_url(origin);
    if expected_is_url || origin_is_url {
        if expected_is_url && origin_is_url {
            return normalize_url(expected) == normalize_url(origin);
        }
        return false;
    }

    if expected == origin {
        return true;
    }

    let origin_path = normalize_path_source(repo_path, origin);
    let mut expected_paths = vec![normalize_path_source(repo_path, expected)];
    if let Ok(cwd) = env::current_dir() {
        expected_paths.push(normalize_path_source(&cwd, expected));
    }

    expected_paths.into_iter().any(|path| path == origin_path)
}

fn normalize_path_source(base: &Path, source: &str) -> PathBuf {
    let raw = PathBuf::from(source.trim());
    let resolved = if raw.is_absolute() {
        raw
    } else {
        base.join(raw)
    };
    resolved.canonicalize().unwrap_or(resolved)
}

fn looks_like_url(source: &str) -> bool {
    source.contains("://") || source.starts_with("git@")
}

/// Cheap probe: does this source point at something git can talk to?
/// For URL sources, `git ls-remote --exit-code <source> HEAD` works
/// without cloning. We disable credential prompts (HTTPS via
/// `GIT_TERMINAL_PROMPT`/`GIT_ASKPASS`/`GCM_INTERACTIVE`; SSH via
/// `SSH_ASKPASS*` plus `GIT_SSH_COMMAND="ssh -o BatchMode=yes
/// -o ConnectTimeout=10"`) and bound HTTPS DNS/transport stalls
/// (`GIT_HTTP_LOW_SPEED_*`) so a typo on a private or unreachable URL
/// doesn't hang at a credential prompt or on DNS lookup.
/// For local paths, check the directory looks like a git repo (worktree
/// `.git`, bare repo `HEAD` + `objects`).
fn source_is_reachable(source: &str) -> bool {
    if looks_like_url(source) {
        // Intentionally bypass run_* helpers: this silent probe should collapse
        // auth, DNS, and missing-remote failures into `false`.
        std::process::Command::new("git")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "/bin/false")
            .env("GCM_INTERACTIVE", "Never")
            // Block SSH askpass fallbacks (gnome-ssh-askpass etc. that
            // OpenSSH spawns when $DISPLAY is set) and force batch + a
            // short connect timeout so an SSH typo can't hang the CLI.
            .env("SSH_ASKPASS", "/bin/false")
            .env("SSH_ASKPASS_REQUIRE", "never")
            .env(
                "GIT_SSH_COMMAND",
                "ssh -o BatchMode=yes -o ConnectTimeout=10",
            )
            // Bound HTTPS stalls: abort if the transfer is below 1 B/s for
            // more than 10 s. Together with `GIT_TERMINAL_PROMPT=0` this
            // covers unreachable hosts and silently-dropped connections.
            .env("GIT_HTTP_LOW_SPEED_LIMIT", "1")
            .env("GIT_HTTP_LOW_SPEED_TIME", "10")
            .arg("ls-remote")
            .arg("--exit-code")
            .arg(source)
            .arg("HEAD")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    } else {
        let p = Path::new(source);
        p.exists()
            && (p.join(".git").exists() || p.join("HEAD").exists() || p.join("objects").exists())
    }
}

fn normalize_url(source: &str) -> String {
    let without_trailing = source.trim_end_matches('/');
    without_trailing
        .strip_suffix(".git")
        .unwrap_or(without_trailing)
        .to_ascii_lowercase()
}

pub(crate) fn git_head_sha(repo: &Path) -> DynResult<String> {
    let out = run_capture(
        Command::new("git")
            .current_dir(repo)
            .arg("rev-parse")
            .arg("HEAD"),
        "git rev-parse HEAD",
    )?;
    Ok(out.stdout.trim().to_string())
}

pub(crate) fn git_clean(repo: &Path) -> DynResult<bool> {
    let out = run_capture(
        Command::new("git")
            .current_dir(repo)
            .arg("status")
            .arg("--porcelain"),
        "git status --porcelain",
    )?;
    Ok(out.stdout.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use tempfile::tempdir;

    use super::{
        git_head_sha, sync_repo_to_pin_at_path_with_opts, RepoSyncOptions, SourceMismatchPolicy,
    };

    #[test]
    fn source_mismatch_reclones_cache_repo_when_clean() {
        let temp = tempdir().expect("tempdir");
        let source_a = temp.path().join("source-a");
        let source_b = temp.path().join("source-b");
        let cache_repo = temp.path().join("cache/repo");

        let _pin_a = init_repo_with_commit(&source_a, "a.txt", "a");
        let pin_b = init_repo_with_commit(&source_b, "b.txt", "b");
        git_clone(&source_a, &cache_repo);

        sync_repo_to_pin_at_path_with_opts(
            &cache_repo,
            &source_b.display().to_string(),
            &pin_b,
            "lez",
            RepoSyncOptions::auto_reclone_cache_repo(),
        )
        .expect("sync success");

        let head = git_head_sha(&cache_repo).expect("head");
        assert_eq!(head, pin_b);
    }

    #[test]
    fn source_mismatch_fails_for_non_cache_policy() {
        let temp = tempdir().expect("tempdir");
        let source_a = temp.path().join("source-a");
        let source_b = temp.path().join("source-b");
        let repo = temp.path().join("repo");

        let pin_a = init_repo_with_commit(&source_a, "a.txt", "a");
        let _pin_b = init_repo_with_commit(&source_b, "b.txt", "b");
        git_clone(&source_a, &repo);

        let err = sync_repo_to_pin_at_path_with_opts(
            &repo,
            &source_b.display().to_string(),
            &pin_a,
            "lez",
            RepoSyncOptions::fail_on_source_mismatch(),
        )
        .expect_err("must fail");

        let msg = format!("{err:#}");
        assert!(msg.contains("does not match requested source"));
        assert!(msg.contains("Refusing to reuse this repo"));
    }

    #[test]
    fn source_mismatch_reclone_refuses_when_new_source_unreachable() {
        // R-E2: a typo in --lez-path used to wipe the populated cache
        // because reconcile_repo_source called fs::remove_dir_all(path)
        // BEFORE git clone <new_source>. Now we probe the new source first
        // and bail if it's unreachable, leaving the cache intact.
        let temp = tempdir().expect("tempdir");
        let source_a = temp.path().join("source-a");
        let cache_repo = temp.path().join("cache/repo");

        let pin_a = init_repo_with_commit(&source_a, "a.txt", "a");
        git_clone(&source_a, &cache_repo);

        let bogus_source = temp.path().join("nonexistent-source");

        let err = sync_repo_to_pin_at_path_with_opts(
            &cache_repo,
            &bogus_source.display().to_string(),
            &pin_a,
            "lez",
            RepoSyncOptions::auto_reclone_cache_repo(),
        )
        .expect_err("must fail");

        let msg = format!("{err:#}");
        assert!(msg.contains("not reachable"), "got: {msg}");
        assert!(msg.contains("Refusing to discard"), "got: {msg}");

        // The cache must still be present and look like a git repo.
        assert!(cache_repo.exists(), "cache must survive");
        assert!(cache_repo.join(".git").exists(), "cache .git must survive");
    }

    #[test]
    fn pin_as_tag_succeeds_with_resolved_sha_check() {
        // Regression: when `[repos.*].pin` is a tag (or any non-SHA ref), the
        // post-checkout assertion used to compare the raw `pin` string against
        // the 40-hex HEAD SHA and always fail. Now we compare against the
        // SHA resolved by `git rev-parse <pin>^{commit}`.
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("source");
        let cache_repo = temp.path().join("cache/repo");

        let sha = init_repo_with_commit(&source, "a.txt", "a");
        run_git(&source, &["tag", "v0.2.0"]);
        git_clone(&source, &cache_repo);

        sync_repo_to_pin_at_path_with_opts(
            &cache_repo,
            &source.display().to_string(),
            "v0.2.0",
            "lez",
            RepoSyncOptions::fail_on_source_mismatch(),
        )
        .expect("tag-as-pin sync must succeed");

        let head = git_head_sha(&cache_repo).expect("head");
        assert_eq!(head, sha, "HEAD must resolve to the tagged commit");
    }

    #[test]
    fn source_mismatch_with_dirty_cache_repo_refuses_reclone() {
        let temp = tempdir().expect("tempdir");
        let source_a = temp.path().join("source-a");
        let source_b = temp.path().join("source-b");
        let cache_repo = temp.path().join("cache/repo");

        let pin_a = init_repo_with_commit(&source_a, "a.txt", "a");
        let _pin_b = init_repo_with_commit(&source_b, "b.txt", "b");
        git_clone(&source_a, &cache_repo);
        fs::write(cache_repo.join("dirty.txt"), "dirty").expect("write dirty file");

        let err = sync_repo_to_pin_at_path_with_opts(
            &cache_repo,
            &source_b.display().to_string(),
            &pin_a,
            "lez",
            RepoSyncOptions {
                source_mismatch: SourceMismatchPolicy::AutoRecloneIfClean,
            },
        )
        .expect_err("must fail");

        let msg = format!("{err:#}");
        assert!(msg.contains("has local changes"));
        assert!(cache_repo.join("dirty.txt").exists());
    }

    fn init_repo_with_commit(path: &std::path::Path, file: &str, contents: &str) -> String {
        fs::create_dir_all(path).expect("create repo");
        run_git(path, &["init"]);
        run_git(path, &["config", "user.email", "test@example.com"]);
        run_git(path, &["config", "user.name", "Test User"]);
        run_git(path, &["config", "commit.gpgsign", "false"]);
        run_git(path, &["config", "tag.gpgsign", "false"]);
        fs::write(path.join(file), contents).expect("write file");
        run_git(path, &["add", "."]);
        run_git(path, &["commit", "-m", "init"]);
        git_rev_parse(path, "HEAD")
    }

    fn git_clone(source: &std::path::Path, target: &std::path::Path) {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        let status = Command::new("git")
            .arg("clone")
            .arg("--no-hardlinks")
            .arg(source)
            .arg(target)
            .status()
            .expect("run git clone");
        assert!(status.success(), "git clone should succeed");
    }

    fn run_git(path: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(path)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {:?} failed", args);
    }

    fn git_rev_parse(path: &std::path::Path, rev: &str) -> String {
        let output = Command::new("git")
            .current_dir(path)
            .arg("rev-parse")
            .arg(rev)
            .output()
            .expect("run rev-parse");
        assert!(output.status.success(), "rev-parse should succeed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }
}
