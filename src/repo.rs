use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::bail;

use crate::process::{run_capture, run_checked};
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

    let _ = run_checked(
        Command::new("git")
            .current_dir(path)
            .arg("fetch")
            .arg("--all")
            .arg("--tags"),
        &format!("git fetch ({label})"),
    );

    ensure_pin_exists(path, source, pin, label)?;

    run_checked(
        Command::new("git")
            .current_dir(path)
            .arg("checkout")
            .arg(pin),
        &format!("git checkout pin ({label})"),
    )?;

    let head = git_head_sha(path)?;
    if head != pin {
        bail!(
            "{label} pin mismatch after checkout (expected {}, got {})",
            pin,
            head
        );
    }

    Ok(())
}

pub(crate) fn ensure_pin_exists(
    path: &Path,
    source: &str,
    pin: &str,
    label: &str,
) -> DynResult<()> {
    let rev = format!("{pin}^{{commit}}");
    if run_capture(
        Command::new("git")
            .current_dir(path)
            .arg("rev-parse")
            .arg("--verify")
            .arg(&rev),
        &format!("verify pin ({label})"),
    )
    .is_err()
    {
        bail!(
            "configured {label} pin {pin} is not available in {} from source `{source}`. Ensure the repo source contains this commit (try `--lez-path` pointing to a repo that has it).",
            path.display(),
        );
    }

    Ok(())
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

    run_checked(
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

            fs::remove_dir_all(path)?;
            run_checked(
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
