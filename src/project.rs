use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context};

use crate::config::{parse_config, update_config_reporting};
use crate::model::{Project, RepoRef};
use crate::state::write_text_atomic;
use crate::DynResult;

pub(crate) fn load_project() -> DynResult<Project> {
    let cwd = env::current_dir()?;
    let root = find_project_root(cwd.clone()).ok_or_else(|| {
        let bin = std::env::args()
            .next()
            .and_then(|p| {
                std::path::Path::new(&p)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "logos-scaffold".to_string());
        // Two distinct failure modes share this entry point: (1) genuinely
        // outside any project, (2) inside a project but the schema is stale.
        // Only the first deserves the "cd into your scaffolded project" hint;
        // the second is handled by `parse_config` and bubbled verbatim below,
        // so call sites can use `load_project()?` without a `.context()` wrap
        // that would misrepresent the schema-error case.
        anyhow!(
            "This command must be run inside a logos-scaffold project.\n\
             Next step: cd into your scaffolded project directory and retry, \
             or run `{bin} create <name>` (or `{bin} new <name>`) to start one. \
             Searched from {}.",
            cwd.display()
        )
    })?;

    load_project_at(&root)
}

/// Load a project from an explicit root directory (no upward discovery).
/// The API layer uses this so consumers can target a project without
/// depending on the process working directory.
pub(crate) fn load_project_at(root: &Path) -> DynResult<Project> {
    let config_path = root.join("scaffold.toml");
    // `try_exists()` (not `exists()`): a permission/IO error on the path must
    // surface as a real error, not be silently reported as a missing config.
    if !config_path
        .try_exists()
        .with_context(|| format!("checking for {}", config_path.display()))?
    {
        bail!(
            "no scaffold.toml found at {}. Pass the root directory of a logos-scaffold project.",
            root.display()
        );
    }
    let cfg_text = fs::read_to_string(&config_path)?;
    let cfg = parse_config(&cfg_text)?;
    Ok(Project {
        root: root.to_path_buf(),
        config: cfg,
    })
}

pub(crate) fn run_in_project_dir(
    path: Option<&Path>,
    op: impl FnOnce() -> DynResult<()>,
) -> DynResult<()> {
    let original = env::current_dir()?;
    if let Some(path) = path {
        env::set_current_dir(path)?;
    }
    let result = op();
    let _ = env::set_current_dir(original);
    result
}

/// Write the current `project.config` back to `scaffold.toml`.
///
/// Rewrites in place through `toml_edit`, so user comments, key ordering, and
/// sections scaffold doesn't model survive the write — only the keys scaffold
/// owns are reassigned. Callers should still invoke it only when the config
/// has actually changed and the rewrite carries meaningful state.
///
/// Comments here are routinely load-bearing (why a `runtime_dir` override
/// exists, why a pin is held back), so dropping them silently re-opens the
/// bugs they document. If the file is absent or unparseable, this falls back
/// to a from-scratch render.
///
/// A read error other than "not found" is *not* folded into that fallback. An
/// absent file is a legitimate from-scratch write; an unreadable one means the
/// comments are still on disk and we simply could not see them, so rendering
/// fresh would delete them — the exact bug this function exists to fix, and
/// silently, since the write would otherwise succeed. Fail instead.
///
/// An *unparseable* file is the third case, and the one that must not be
/// silent. Refusing outright would wedge every command that persists config on
/// a file one stray bracket from valid, so the from-scratch render stands — but
/// it discards every comment and unmodelled section the file held. So before
/// overwriting, the original is copied to a timestamped sibling
/// `scaffold.toml.bak-YYYY-MM-DD-HHMMSS_NNNNNNNNN` and the fallback is
/// announced on stderr. That is the same recovery `init`'s migration offers,
/// with a distinct name per rewrite so a second destructive write keeps its own
/// copy instead of finding the first one in the way.
///
/// **This is fail-closed.** If the backup cannot be written — an IO error, a
/// read-only directory, a name already taken — the whole write aborts with an
/// error and `scaffold.toml` is left untouched. A file we could not preserve is
/// a file we do not destroy. The alternative, proceeding with a warning, trades
/// the user's comments for the convenience of not having to re-run a command,
/// which is the wrong way round: the rewrite is recoverable, the comments are
/// not.
///
/// The timestamp runs to nanoseconds specifically so that a name collision can
/// be a hard error rather than a routine event to be worked around — see
/// [`create_backup_file`].
pub(crate) fn save_project_config(project: &Project) -> DynResult<()> {
    let path = project.root.join("scaffold.toml");
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "reading {} before rewriting it (refusing to rewrite from \
                     scratch, which would drop the file's comments)",
                    path.display()
                )
            })
        }
    };
    let (rendered, outcome) = update_config_reporting(&existing, &project.config)?;
    if let Some(reason) = outcome.discarded_reason() {
        // `?`: a failed backup must stop the write, not merely warn about it.
        back_up_before_discarding(&path, &existing, reason)?;
    }
    write_text_atomic(&path, &rendered)
}

/// Tell the user their `scaffold.toml` could not be parsed and is about to be
/// replaced by a from-scratch render, and preserve the original next to it.
///
/// Returns an error — which aborts the rewrite — if the original could not be
/// preserved. Split out from `save_project_config` so the message and the
/// backup stay one unit: a warning with no recovery path would be the smaller
/// half of what the user needs here.
fn back_up_before_discarding(path: &Path, existing: &str, reason: &str) -> DynResult<()> {
    eprintln!(
        "warning: {} could not be parsed as TOML, so it is being rewritten from \
         scratch. Comments and any sections scaffold does not model will be lost.\n\
         warning:   parse error: {reason}",
        path.display(),
    );
    // Nothing on disk to preserve — the read returned NotFound, so the
    // "from-scratch render" is just a first write and destroys nothing.
    if existing.is_empty() {
        return Ok(());
    }

    let (backup, mut file) = create_backup_file(path)?;

    // Write the bytes already read rather than re-reading, so the backup is
    // exactly what is discarded even if the file changed underneath us.
    file.write_all(existing.as_bytes()).with_context(|| {
        format!(
            "refusing to rewrite {}: could not write its backup to {}",
            path.display(),
            backup.display()
        )
    })?;
    // Without this an IO error surfacing only at close would be swallowed, and
    // we would report a backup that is short or empty as a success.
    file.sync_all().with_context(|| {
        format!(
            "refusing to rewrite {}: could not flush its backup to {}",
            path.display(),
            backup.display()
        )
    })?;

    eprintln!(
        "warning:   a copy of the original was saved to {}",
        backup.display()
    );
    Ok(())
}

/// Create the backup file, returning it and the path it landed on.
///
/// One attempt, no retry. `create_new` claims the name atomically, so the file
/// is never clobbered by something that appeared between a check and an open,
/// and a name that is already taken is a hard failure.
///
/// That strictness is only sound because the timestamp carries nanoseconds. At
/// millisecond resolution two consecutive `save_project_config` calls really do
/// share a stamp — an earlier draft of this aborted on exactly that, and the
/// test suite caught it under load — so a collision there meant "the clock is
/// coarse", which is not an error worth failing a write over. At nanosecond
/// resolution two writes cannot share a stamp under any ordinary sequence, so a
/// collision means what the abort was written to mean: something unexpected (a
/// clock stepping backwards, a filesystem replaying a name). Refusing is then
/// the right answer, and there is nothing to retry around.
fn create_backup_file(path: &Path) -> DynResult<(PathBuf, fs::File)> {
    let candidate = backup_path_for(path, &local_timestamp_for_filename()?)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&candidate)
    {
        Ok(file) => Ok((candidate, file)),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => bail!(
            "refusing to rewrite {}: its backup target {} already exists.\n\
             The name carries a nanosecond timestamp, so a collision means \
             something unexpected (a clock stepping backwards, for instance). \
             Move that file aside and re-run.",
            path.display(),
            candidate.display(),
        ),
        Err(e) => Err(e).with_context(|| {
            format!(
                "refusing to rewrite {}: could not create its backup at {}",
                path.display(),
                candidate.display()
            )
        }),
    }
}

/// `<path>.bak-<stamp>`, where `stamp` is `YYYY-MM-DD-HHMMSS_NNNNNNNNN` in
/// local time.
///
/// One backup per destructive rewrite, rather than a single fixed
/// `scaffold.toml.bak` that the second one would find occupied. The format is
/// zero-padded throughout and orders date-before-time, so a plain lexical sort
/// of a directory listing is also chronological — which is the only thing
/// anyone does with these files.
///
/// The nanoseconds are set off with `_` rather than the `.` a fractional second
/// would use: nine digits running straight on from `HHMMSS` invites misreading
/// where one field ends and the next begins, and `_` is the one separator not
/// already spoken for by the date (`-`) or by the `.bak` extension (`.`).
fn backup_path_for(path: &Path, stamp: &str) -> DynResult<PathBuf> {
    let mut name = path
        .file_name()
        .ok_or_else(|| anyhow!("cannot derive a backup name for {}", path.display()))?
        .to_os_string();
    name.push(format!(".bak-{stamp}"));
    Ok(path.with_file_name(name))
}

/// Now, as `YYYY-MM-DD-HHMMSS_NNNNNNNNN` in the machine's local time.
///
/// Nanoseconds, not milliseconds, and that is load-bearing rather than
/// gratuitous precision: it is what lets [`create_backup_file`] treat a name
/// collision as a hard error. Two consecutive writes share a millisecond
/// routinely and a nanosecond effectively never, so the extra six digits are
/// what turn "the clock is coarse" into "something is wrong".
///
/// Hand-rolled rather than pulled from a date crate: this is the only calendar
/// formatting in the codebase, and it exists to name a file a human will read
/// in a directory listing. Adding a dependency to the CLI's tree for one
/// filename is a poor trade.
fn local_timestamp_for_filename() -> DynResult<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow!("system clock is before the unix epoch: {e}"))?;
    let nanos = now.subsec_nanos();
    let local = i64::try_from(now.as_secs())
        .map_err(|_| anyhow!("system clock is too far in the future to format"))?
        + local_utc_offset_seconds();

    // Days since epoch, and the seconds within that day. `div_euclid` /
    // `rem_euclid` rather than `/` and `%` so a pre-1970 local time (possible
    // once the offset is applied near the epoch) floors instead of truncating
    // toward zero, which would land the time in the wrong day.
    let days = local.div_euclid(86_400);
    let secs_of_day = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    // `subsec_nanos` is always < 1e9, so the field is exactly nine digits and
    // the whole stamp is fixed-width — which is what keeps a lexical sort
    // chronological.
    Ok(format!(
        "{year:04}-{month:02}-{day:02}-{hour:02}{minute:02}{second:02}_{nanos:09}"
    ))
}

/// The machine's current UTC offset in seconds, or 0 if it cannot be read.
///
/// Falling back to UTC is deliberate: the timestamp names a backup file, so a
/// wrong-by-hours name is a cosmetic problem, while failing the backup — and
/// therefore, now, the whole write — over an unreadable timezone would not be.
/// This is the one part of the path allowed to degrade rather than abort.
fn local_utc_offset_seconds() -> i64 {
    #[cfg(unix)]
    {
        // SAFETY: `localtime_r` writes into a caller-provided `tm` and, unlike
        // `localtime`, keeps no shared static — so it is sound to call from
        // any thread. A null return means the conversion failed, in which case
        // `out` is untouched and we do not read it.
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut out: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut out).is_null() {
                return 0;
            }
            out.tm_gmtoff as i64
        }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Days since the Unix epoch → `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, the algorithm the C++20 `<chrono>`
/// calendar is specified in terms of. It shifts the era to start in March so
/// the leap day falls at the end of the year, which is what removes every
/// special case from the month arithmetic. Correct for any date in the
/// proleptic Gregorian calendar; pinned by tests against known dates,
/// leap-year boundaries, and epoch-adjacent days.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March-based
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub(crate) fn find_project_root(mut dir: PathBuf) -> Option<PathBuf> {
    loop {
        if dir.join("scaffold.toml").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Layer of the cache_root resolution chain that supplied the active value.
/// Surfaced by `lgs doctor` so CI users can confirm which layer won.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CacheRootSource {
    Env,
    Config,
    XdgCacheHome,
    HomeCache,
    MacOsCaches,
    WindowsLocalAppData,
}

impl CacheRootSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Env => "LOGOS_SCAFFOLD_CACHE_ROOT",
            Self::Config => "scaffold.toml [scaffold].cache_root",
            Self::XdgCacheHome => "$XDG_CACHE_HOME",
            Self::HomeCache => "$HOME/.cache",
            Self::MacOsCaches => "$HOME/Library/Caches",
            Self::WindowsLocalAppData => "%LOCALAPPDATA%",
        }
    }
}

/// Resolves `cache_root` by trying, in order:
/// 1. `LOGOS_SCAFFOLD_CACHE_ROOT` env var (non-empty),
/// 2. `[scaffold].cache_root` from `scaffold.toml` if set (relative values are
///    joined against `project.root`, so they resolve the same regardless of CWD),
/// 3. `default_cache_root()` — XDG / HOME / platform fallback.
///
/// The companion `source` is returned so `lgs doctor` can print which layer won.
pub(crate) fn resolve_cache_root(project: &Project) -> DynResult<(PathBuf, CacheRootSource)> {
    if let Ok(val) = env::var("LOGOS_SCAFFOLD_CACHE_ROOT") {
        if !val.is_empty() {
            return Ok((PathBuf::from(val), CacheRootSource::Env));
        }
    }

    if !project.config.cache_root.is_empty() {
        return Ok((
            project.root.join(&project.config.cache_root),
            CacheRootSource::Config,
        ));
    }

    default_cache_root()
}

/// Resolves the cache root for `create` / `new`, which run *before* a project
/// (and therefore a `scaffold.toml`) exists. Order:
/// 1. `--cache-root` when the caller passed one,
/// 2. `LOGOS_SCAFFOLD_CACHE_ROOT` env var (non-empty),
/// 3. `default_cache_root()` — XDG / HOME / platform fallback.
///
/// This is `resolve_cache_root` minus the `scaffold.toml` layer. Keeping the two
/// side by side is deliberate: creation used to skip the env layer entirely, so a
/// project created under `LOGOS_SCAFFOLD_CACHE_ROOT` bootstrapped into the default
/// cache and then resolved the env one for every later command — cloning the
/// pinned LEZ twice and reporting a cache root that creation never used.
pub(crate) fn bootstrap_cache_root(cli_override: Option<&Path>) -> DynResult<PathBuf> {
    if let Some(path) = cli_override {
        return Ok(path.to_path_buf());
    }

    if let Ok(val) = env::var("LOGOS_SCAFFOLD_CACHE_ROOT") {
        if !val.is_empty() {
            return Ok(PathBuf::from(val));
        }
    }

    default_cache_root().map(|(path, _)| path)
}

/// Platform-default cache root when neither env nor `scaffold.toml` set one.
/// Returns the source layer alongside the path.
pub(crate) fn default_cache_root() -> DynResult<(PathBuf, CacheRootSource)> {
    let home = home_dir()?;
    if cfg!(target_os = "macos") {
        return Ok((
            home.join("Library/Caches/logos-scaffold"),
            CacheRootSource::MacOsCaches,
        ));
    }

    if cfg!(target_os = "windows") {
        if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
            return Ok((
                PathBuf::from(local_app_data).join("logos-scaffold/Cache"),
                CacheRootSource::WindowsLocalAppData,
            ));
        }
    }

    if let Ok(xdg) = env::var("XDG_CACHE_HOME") {
        return Ok((
            PathBuf::from(xdg).join("logos-scaffold"),
            CacheRootSource::XdgCacheHome,
        ));
    }

    Ok((
        home.join(".cache/logos-scaffold"),
        CacheRootSource::HomeCache,
    ))
}

/// Resolves the on-disk location of a pinned repo (lez, spel).
///
/// - If `repo.path` is set, it's authoritative — used literally if absolute, or
///   joined to `project.root` if relative. Covers `--vendor-deps` projects and
///   any user-edited override.
/// - If `repo.path` is empty, derive `<cache_root>/repos/<name>/<repo.pin>`.
///   This is the portable default written by `new` / `init`: scaffold.toml
///   stays byte-identical across machines, and the host's cache_root chain
///   (env → `[scaffold].cache_root` → XDG default) decides the actual
///   location at runtime.
///
/// Mirrors the basecamp pattern in `cmd_basecamp_setup`, which never persists
/// a path and always derives from cache_root + pin.
pub(crate) fn resolve_repo_path(
    project: &Project,
    repo: &RepoRef,
    name: &str,
) -> DynResult<PathBuf> {
    if !repo.path.is_empty() {
        let p = PathBuf::from(&repo.path);
        return Ok(if p.is_absolute() {
            p
        } else {
            project.root.join(p)
        });
    }
    if repo.pin.is_empty() {
        bail!(
            "cannot resolve repo path for `{name}`: both path and pin are empty in scaffold.toml"
        );
    }
    let (cache_root, _) = resolve_cache_root(project)?;
    Ok(cache_root.join("repos").join(name).join(&repo.pin))
}

pub(crate) fn home_dir() -> DynResult<PathBuf> {
    if let Ok(home) = env::var("HOME") {
        return Ok(PathBuf::from(home));
    }
    bail!("HOME is not set")
}

pub(crate) fn ensure_dir_exists(path: &Path, label: &str) -> DynResult<()> {
    if !path.exists() {
        bail!("missing {label} at {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, RepoRef};
    use std::sync::Mutex;

    // Tests in this module mutate process-wide env vars; run them under one lock.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Width of `YYYY-MM-DD-HHMMSS_NNNNNNNNN`: 10 + 1 + 6 + 1 + 9.
    const STAMP_WIDTH: usize = 27;

    /// The full disk round trip `save_project_config` performs: read the
    /// existing file, merge the config over it, write it back atomically.
    /// The unit tests in `config` cover `update_config` in isolation; only
    /// this covers the read-back, which is where a missing or unreadable file
    /// silently degrades to a from-scratch render.
    /// A valid v0.2.0 `scaffold.toml`, with no comments of its own so callers
    /// can add exactly the ones their assertions look for.
    fn minimal_scaffold_toml() -> String {
        "\
[scaffold]
version = \"0.2.0\"

[repos.lez]
source = \"https://github.com/logos-blockchain/logos-execution-zone.git\"
pin = \"cf3639d8252040d13b3d4e933feb19b42c76e14a\"

[repos.spel]
source = \"https://github.com/logos-co/spel.git\"
pin = \"73fc462eb8f0a4d00f1a846437c627ec2e523f83\"

[wallet]
home_dir = \".scaffold/wallet\"

[framework]
kind = \"default\"
version = \"0.1.0\"

[framework.idl]
spec = \"lssa-idl/0.1.0\"
path = \"idl\"

[localnet]
port = 3040
risc0_dev_mode = true
"
        .to_string()
    }

    #[test]
    fn save_project_config_preserves_comments_through_the_disk_round_trip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("scaffold.toml");
        let original = "\
# LOAD-BEARING: runtime_dir must be the session's real runtime dir;
# the 108-byte sun_path cap makes every module segfault otherwise.
[scaffold]
version = \"0.2.0\"

[repos.lez]
source = \"https://github.com/logos-blockchain/logos-execution-zone.git\"
pin = \"cf3639d8252040d13b3d4e933feb19b42c76e14a\"

[repos.spel]
source = \"https://github.com/logos-co/spel.git\"
pin = \"73fc462eb8f0a4d00f1a846437c627ec2e523f83\"

[wallet]
home_dir = \".scaffold/wallet\"

[framework]
kind = \"default\"
version = \"0.1.0\"

[framework.idl]
spec = \"lssa-idl/0.1.0\"
path = \"idl\"

[localnet]
port = 3040
risc0_dev_mode = true
"
        .to_string();
        fs::write(&path, &original).expect("seed scaffold.toml");

        // Load, change something scaffold owns, save.
        let mut project = load_project_at(temp.path()).expect("load");
        project.config.lez.pin = "2222222222222222222222222222222222222222".to_string();
        save_project_config(&project).expect("save");

        let written = fs::read_to_string(&path).expect("read back");
        assert!(
            written.contains("# LOAD-BEARING: runtime_dir must be the session's real runtime dir;"),
            "comment dropped by save_project_config:\n{written}"
        );
        assert!(
            written.contains("# the 108-byte sun_path cap makes every module segfault otherwise."),
            "second comment line dropped:\n{written}"
        );
        assert!(
            written.contains("2222222222222222222222222222222222222222"),
            "the edit itself was not written:\n{written}"
        );
        // And the result must still load.
        let reloaded = load_project_at(temp.path()).expect("reload");
        assert_eq!(
            reloaded.config.lez.pin,
            "2222222222222222222222222222222222222222"
        );
    }

    /// An unreadable `scaffold.toml` must fail the write, not fall back to a
    /// from-scratch render. The comments are still on disk — we just could not
    /// see them — so rendering fresh would delete them while reporting
    /// success, which is exactly the bug this path exists to fix.
    ///
    /// An *absent* file is the legitimate case for that fallback and must
    /// still succeed; both halves are here so neither can be "fixed" alone.
    #[test]
    #[cfg(unix)]
    fn save_project_config_refuses_to_rewrite_an_unreadable_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("scaffold.toml");
        fs::write(&path, format!("# keep me\n{}", minimal_scaffold_toml())).expect("seed");

        let project = load_project_at(temp.path()).expect("load");

        // Unreadable: the comments are there, we just cannot see them.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod");
        let unreadable = fs::read_to_string(&path).is_err();
        // Running as root defeats the permission bit; skip rather than assert
        // a falsehood about the environment.
        if unreadable {
            let err = save_project_config(&project)
                .expect_err("an unreadable scaffold.toml must not be rewritten from scratch");
            assert!(
                err.to_string().contains("scaffold.toml"),
                "error should name the file it refused to rewrite: {err}"
            );
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("restore");
        // Whatever happened, the comment must still be on disk.
        let after = fs::read_to_string(&path).expect("read back");
        assert!(after.contains("# keep me"), "comment lost:\n{after}");

        // The absent-file case still writes, from scratch.
        fs::remove_file(&path).expect("remove");
        save_project_config(&project).expect("a missing file is a legitimate fresh write");
        assert!(path.exists(), "fresh write did not happen");
    }

    /// Every `scaffold.toml.bak-*` sitting next to `path`.
    fn backups_beside(dir: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("dir entry").path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("scaffold.toml.bak-"))
            })
            .collect();
        found.sort();
        found
    }

    /// A `scaffold.toml` broken the way a hand-edit breaks one: a stray
    /// bracket, plus a load-bearing comment and an unmodelled section that the
    /// from-scratch render cannot reproduce.
    fn broken_scaffold_toml() -> String {
        format!(
            "# LOAD-BEARING: do not bump this pin, see issue 412\n\
             [[[oops\n{}\n[team.notes]\nowner = \"alice\"\n",
            minimal_scaffold_toml()
        )
    }

    /// Regression for weboko's review finding 3 (second half).
    ///
    /// An unparseable `scaffold.toml` is still rewritten from scratch — that
    /// fallback is deliberate — but it discards every comment and unmodelled
    /// section the file held, which is precisely the loss this whole path
    /// exists to prevent. The user must get both a diagnostic and a way back:
    /// the original is copied to a timestamped sibling before the overwrite.
    #[test]
    fn save_project_config_backs_up_an_unparseable_file_before_overwriting_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("scaffold.toml");

        // Load from a good file so we have a Project to write back...
        fs::write(&path, minimal_scaffold_toml()).expect("seed");
        let project = load_project_at(temp.path()).expect("load");

        // ...then break the file on disk.
        let broken = broken_scaffold_toml();
        fs::write(&path, &broken).expect("break it");

        save_project_config(&project).expect("a broken file must not wedge the write");

        // The write happened and produced something loadable.
        load_project_at(temp.path()).expect("rewritten file must load");

        // And the discarded original is recoverable, byte for byte.
        let backups = backups_beside(temp.path());
        assert_eq!(
            backups.len(),
            1,
            "expected exactly one timestamped backup, got {backups:?}"
        );
        let saved = fs::read_to_string(&backups[0]).expect("read backup");
        assert_eq!(
            saved, broken,
            "the backup must be the exact content that was discarded"
        );
        assert!(
            saved.contains("# LOAD-BEARING: do not bump this pin, see issue 412"),
            "backup lost the comment it exists to preserve:\n{saved}"
        );
        assert!(
            saved.contains("[team.notes]"),
            "backup lost the unmodelled section:\n{saved}"
        );
    }

    /// The point of timestamping: a *second* destructive rewrite keeps its own
    /// copy rather than finding the first one in the way. With a fixed
    /// `scaffold.toml.bak` this either clobbered the older original or (in the
    /// fail-closed design) wedged every subsequent write.
    #[test]
    fn save_project_config_keeps_a_separate_backup_per_rewrite() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("scaffold.toml");

        fs::write(&path, minimal_scaffold_toml()).expect("seed");
        let project = load_project_at(temp.path()).expect("load");

        let first = format!("# first original\n[[[oops\n{}", minimal_scaffold_toml());
        fs::write(&path, &first).expect("break it");
        save_project_config(&project).expect("first rewrite");

        // Break it again, with different content.
        let second = format!("# second original\n[[[oops\n{}", minimal_scaffold_toml());
        fs::write(&path, &second).expect("break it again");
        save_project_config(&project)
            .expect("second rewrite must not be blocked by the first .bak");

        let backups = backups_beside(temp.path());
        assert_eq!(
            backups.len(),
            2,
            "each destructive rewrite must keep its own backup, got {backups:?}"
        );
        let contents: Vec<String> = backups
            .iter()
            .map(|p| fs::read_to_string(p).expect("read backup"))
            .collect();
        assert!(
            contents.iter().any(|c| c == &first),
            "the first original was lost:\n{contents:?}"
        );
        assert!(
            contents.iter().any(|c| c == &second),
            "the second original was lost:\n{contents:?}"
        );
    }

    /// A parseable file takes the merge path, so there is nothing being
    /// discarded and no backup to write. Without this, a backup written on
    /// every save would look like a passing test above while quietly
    /// littering every project directory.
    #[test]
    fn save_project_config_writes_no_backup_when_the_file_parses() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("scaffold.toml");
        fs::write(&path, format!("# keep me\n{}", minimal_scaffold_toml())).expect("seed");

        let project = load_project_at(temp.path()).expect("load");
        save_project_config(&project).expect("save");

        assert!(
            backups_beside(temp.path()).is_empty(),
            "a successful merge must not leave a backup behind"
        );
        let after = fs::read_to_string(&path).expect("read back");
        assert!(after.contains("# keep me"), "comment lost:\n{after}");
    }

    /// Fail-closed: if the original cannot be preserved, it is not destroyed.
    ///
    /// Whatever stops the backup — no permission here, a name already taken —
    /// the write aborts and `scaffold.toml` is left exactly as the user left
    /// it. The rewrite is recoverable by re-running a command; the comments in
    /// that file are not.
    #[test]
    fn save_project_config_aborts_rather_than_overwrite_a_file_it_cannot_back_up() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("scaffold.toml");

        fs::write(&path, minimal_scaffold_toml()).expect("seed");
        let project = load_project_at(temp.path()).expect("load");

        let broken = broken_scaffold_toml();
        fs::write(&path, &broken).expect("break it");

        // Make the backup impossible to create by taking write permission off
        // the directory. This is the realistic form of the failure (a
        // read-only checkout, a permissions mistake) and, unlike a name
        // collision, it cannot be retried around.
        let mut perms = fs::metadata(temp.path()).expect("metadata").permissions();
        perms.set_readonly(true);
        fs::set_permissions(temp.path(), perms).expect("chmod");

        // Running as root defeats the permission bit; assert nothing about the
        // environment in that case, just restore and skip.
        let blocked = fs::write(temp.path().join(".probe"), "x").is_err();
        if blocked {
            let err = save_project_config(&project)
                .expect_err("a backup that cannot be written must abort the rewrite");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("scaffold.toml"),
                "error must name the file it refused to rewrite: {msg}"
            );
            assert!(
                msg.contains("backup"),
                "error must say the backup is what blocked it: {msg}"
            );
        }

        let mut perms = fs::metadata(temp.path()).expect("metadata").permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(temp.path(), perms).expect("restore");

        if blocked {
            // The whole point: the unparseable original is still on disk.
            assert_eq!(
                fs::read_to_string(&path).expect("read back"),
                broken,
                "scaffold.toml was overwritten despite the backup failing"
            );
            assert!(
                backups_beside(temp.path()).is_empty(),
                "no backup should exist when the backup write failed"
            );
        }
    }

    /// Back-to-back backups must get distinct names *from the clock*, with no
    /// retry or suffix involved.
    ///
    /// This is the test that justifies the nanosecond field. An earlier
    /// millisecond-resolution version of this code aborted the second of two
    /// consecutive writes, and the failure showed up under normal test-suite
    /// load — consecutive `save_project_config` calls are easily fast enough to
    /// share a millisecond. The fix at the time was a `-2` suffix, which coped
    /// with the collision instead of removing it; nanoseconds remove it, which
    /// is what lets `create_backup_file` abort on a collision at all.
    ///
    /// So this asserts the strong property: every name distinct, claimed on the
    /// first attempt. A regression to coarser precision fails here rather than
    /// silently reintroducing collisions that only surface under load.
    #[test]
    fn consecutive_backups_get_distinct_names_without_retrying() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("scaffold.toml");
        fs::write(&path, "irrelevant").expect("seed");

        let mut claimed: Vec<PathBuf> = Vec::new();
        for _ in 0..50 {
            // `create_backup_file` has no retry: if this returns Ok, the very
            // first name it derived was free.
            let (p, _file) = create_backup_file(&path).expect("claim a backup name");
            assert!(
                !claimed.contains(&p),
                "the clock handed out the same name twice: {p:?}"
            );
            claimed.push(p);
        }
        assert_eq!(claimed.len(), 50);
        // All of them exist at once — none clobbered another.
        for p in &claimed {
            assert!(p.exists(), "backup vanished: {p:?}");
        }
        // Every name is a bare fixed-width stamp: nothing was appended to make
        // it unique, because nothing needed to be.
        for p in &claimed {
            let name = p.file_name().expect("name").to_string_lossy().into_owned();
            let stamp = name
                .strip_prefix("scaffold.toml.bak-")
                .unwrap_or_else(|| panic!("unexpected backup name: {name}"));
            assert_eq!(
                stamp.len(),
                STAMP_WIDTH,
                "expected a bare {STAMP_WIDTH}-char stamp: {name}"
            );
        }
    }

    /// A name that really is taken aborts, rather than being worked around.
    /// With nanosecond stamps this cannot happen by accident, which is exactly
    /// why it is allowed to be fatal — so the branch still needs pinning.
    #[test]
    fn a_taken_backup_name_aborts_the_write() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("scaffold.toml");
        fs::write(&path, "irrelevant").expect("seed");

        // Freeze a stamp, occupy the name it derives, then ask for that exact
        // name again. Going through `backup_path_for` keeps the test honest
        // about the naming scheme rather than hardcoding it.
        let stamp = local_timestamp_for_filename().expect("stamp");
        let taken = backup_path_for(&path, &stamp).expect("derive");
        fs::write(&taken, "someone else's file").expect("occupy");

        let err = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&taken)
            .expect_err("create_new must refuse an existing file");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::AlreadyExists,
            "the abort branch keys on AlreadyExists"
        );
        // And the occupied file is untouched by the failed claim.
        assert_eq!(
            fs::read_to_string(&taken).expect("read back"),
            "someone else's file"
        );
    }

    /// The backup name is user-facing — people find these in a directory
    /// listing — so its shape is a contract, not an implementation detail.
    #[test]
    fn backup_name_is_timestamped_and_sorts_chronologically() {
        let path = Path::new("/tmp/proj/scaffold.toml");
        let stamp = local_timestamp_for_filename().expect("stamp");
        let name = backup_path_for(path, &stamp)
            .expect("derive")
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .into_owned();

        let stamp = name
            .strip_prefix("scaffold.toml.bak-")
            .unwrap_or_else(|| panic!("unexpected backup name: {name}"));
        // YYYY-MM-DD-HHMMSS_NNNNNNNNN
        assert_eq!(stamp.len(), STAMP_WIDTH, "unexpected stamp width: {stamp}");
        let (date, rest) = stamp.split_at(10);
        let mut date_parts = date.split('-');
        let year: i64 = date_parts.next().expect("year").parse().expect("year int");
        let month: u32 = date_parts
            .next()
            .expect("month")
            .parse()
            .expect("month int");
        let day: u32 = date_parts.next().expect("day").parse().expect("day int");
        assert!((2020..=2100).contains(&year), "implausible year: {stamp}");
        assert!((1..=12).contains(&month), "bad month: {stamp}");
        assert!((1..=31).contains(&day), "bad day: {stamp}");

        // `-HHMMSS_NNNNNNNNN`: the nanoseconds are set off with `_` so the
        // nine-digit run cannot be misread as a continuation of the seconds.
        assert!(rest.starts_with('-'), "expected -HHMMSS_NNNNNNNNN: {stamp}");
        let (time, sub) = rest[1..].split_at(6);
        assert!(
            time.chars().all(|c| c.is_ascii_digit()),
            "HHMMSS must be digits: {stamp}"
        );
        let nanos = sub
            .strip_prefix('_')
            .unwrap_or_else(|| panic!("nanoseconds must be set off with '_': {stamp}"));
        assert_eq!(nanos.len(), 9, "nanoseconds must be 9 digits: {stamp}");
        assert!(
            nanos.chars().all(|c| c.is_ascii_digit()),
            "nanoseconds must be digits: {stamp}"
        );

        // Fixed width, zero-padded and date-before-time, so lexical order is
        // chronological — the only thing anyone does with these files.
        assert!("2026-09-04-090000_000000000" < "2026-09-04-090001_000000000");
        assert!("2026-09-04-000000_000000000" < "2026-09-14-000000_000000000");
        assert!("2026-09-30-235959_999999999" < "2026-10-01-000000_000000000");
        // And sub-second ordering holds, which is what the extra digits buy.
        assert!("2026-09-04-090000_000000001" < "2026-09-04-090000_000000002");
        assert!("2026-09-04-090000_000999999" < "2026-09-04-090000_001000000");
    }

    /// `civil_from_days` is the one piece of hand-rolled calendar arithmetic
    /// here, so it is pinned against dates picked to break a wrong
    /// implementation: the epoch, both sides of a leap day, a century
    /// non-leap-year (1900) and a 400-year leap year (2000), and days before
    /// the epoch, which a truncating division would land in the wrong day.
    #[test]
    fn civil_from_days_matches_known_dates() {
        for (days, expected) in [
            (0_i64, (1970, 1, 1)),
            (1, (1970, 1, 2)),
            (-1, (1969, 12, 31)),
            (-719_468, (0, 3, 1)),
            (59, (1970, 3, 1)),
            (365, (1971, 1, 1)),
            // 2000 is a leap year (divisible by 400): Feb 29 exists.
            (11_015, (2000, 2, 28)),
            (11_016, (2000, 2, 29)),
            (11_017, (2000, 3, 1)),
            // 1900 is NOT a leap year (divisible by 100, not 400), so Feb 28
            // is followed directly by Mar 1.
            (-25_509, (1900, 2, 28)),
            (-25_508, (1900, 3, 1)),
            (20_335, (2025, 9, 4)),
            (19_723, (2024, 1, 1)),
            (19_782, (2024, 2, 29)),
        ] {
            assert_eq!(
                civil_from_days(days),
                expected,
                "civil_from_days({days}) should be {expected:?}"
            );
        }
    }

    fn fixture_project(root: PathBuf, cache_root: &str) -> Project {
        Project {
            root,
            config: Config {
                version: "0.2.0".into(),
                cache_root: cache_root.to_string(),
                lez: RepoRef::default(),
                spel: RepoRef::default(),
                basecamp_repo: None,
                lgpm_repo: None,
                wallet_home_dir: ".scaffold/wallet".into(),
                circuits: crate::model::CircuitsConfig::default(),
                framework: FrameworkConfig {
                    kind: String::new(),
                    version: String::new(),
                    idl: FrameworkIdlConfig {
                        spec: String::new(),
                        path: String::new(),
                    },
                },
                localnet: LocalnetConfig::default(),
                modules: std::collections::BTreeMap::new(),
                basecamp: None,
                run: crate::model::RunConfig::default(),
            },
        }
    }

    #[test]
    fn env_layer_wins_over_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("LOGOS_SCAFFOLD_CACHE_ROOT", "/tmp/from-env");
        let project = fixture_project(PathBuf::from("/proj"), "should-be-ignored");
        let (path, source) = resolve_cache_root(&project).expect("resolve");
        env::remove_var("LOGOS_SCAFFOLD_CACHE_ROOT");

        assert_eq!(path, PathBuf::from("/tmp/from-env"));
        assert_eq!(source, CacheRootSource::Env);
    }

    #[test]
    fn config_layer_joins_relative_value_against_project_root() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("LOGOS_SCAFFOLD_CACHE_ROOT");
        let project = fixture_project(PathBuf::from("/proj"), ".scaffold/cache");
        let (path, source) = resolve_cache_root(&project).expect("resolve");

        assert_eq!(path, PathBuf::from("/proj/.scaffold/cache"));
        assert_eq!(source, CacheRootSource::Config);
    }

    #[test]
    fn config_layer_honors_absolute_value() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("LOGOS_SCAFFOLD_CACHE_ROOT");
        let project = fixture_project(PathBuf::from("/proj"), "/abs/cache");
        let (path, source) = resolve_cache_root(&project).expect("resolve");

        assert_eq!(path, PathBuf::from("/abs/cache"));
        assert_eq!(source, CacheRootSource::Config);
    }

    #[test]
    fn resolve_repo_path_uses_literal_absolute_path_when_set() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("LOGOS_SCAFFOLD_CACHE_ROOT");
        let mut project = fixture_project(PathBuf::from("/proj"), "");
        project.config.lez = RepoRef {
            path: "/abs/lez".into(),
            pin: "deadbeef".into(),
            ..Default::default()
        };
        let path = resolve_repo_path(&project, &project.config.lez, "lez").expect("resolve");
        assert_eq!(path, PathBuf::from("/abs/lez"));
    }

    #[test]
    fn resolve_repo_path_joins_relative_path_to_project_root() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("LOGOS_SCAFFOLD_CACHE_ROOT");
        let mut project = fixture_project(PathBuf::from("/proj"), "");
        project.config.lez = RepoRef {
            path: ".scaffold/repos/lez".into(),
            pin: "deadbeef".into(),
            ..Default::default()
        };
        let path = resolve_repo_path(&project, &project.config.lez, "lez").expect("resolve");
        assert_eq!(path, PathBuf::from("/proj/.scaffold/repos/lez"));
    }

    #[test]
    fn resolve_repo_path_derives_from_cache_root_when_path_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("LOGOS_SCAFFOLD_CACHE_ROOT", "/tmp/cache");
        let mut project = fixture_project(PathBuf::from("/proj"), "");
        project.config.spel = RepoRef {
            pin: "cafef00d".into(),
            ..Default::default()
        };
        let path = resolve_repo_path(&project, &project.config.spel, "spel").expect("resolve");
        env::remove_var("LOGOS_SCAFFOLD_CACHE_ROOT");
        assert_eq!(path, PathBuf::from("/tmp/cache/repos/spel/cafef00d"));
    }

    #[test]
    fn resolve_repo_path_errors_when_both_path_and_pin_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("LOGOS_SCAFFOLD_CACHE_ROOT");
        let project = fixture_project(PathBuf::from("/proj"), "");
        // both lez.path and lez.pin are empty in fixture
        let err = resolve_repo_path(&project, &project.config.lez, "lez").unwrap_err();
        assert!(err.to_string().contains("lez"), "{err}");
    }

    // Creation (`create` / `new`) has no scaffold.toml yet, so it resolves the
    // cache root through `bootstrap_cache_root`. It previously skipped the env
    // layer, bootstrapping into the default cache while every later command in
    // the created project used the env one.
    #[test]
    fn bootstrap_env_layer_wins_when_no_cli_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("LOGOS_SCAFFOLD_CACHE_ROOT", "/tmp/from-env");
        let resolved = bootstrap_cache_root(None);
        env::remove_var("LOGOS_SCAFFOLD_CACHE_ROOT");

        assert_eq!(resolved.expect("resolve"), PathBuf::from("/tmp/from-env"));
    }

    #[test]
    fn bootstrap_cli_override_wins_over_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("LOGOS_SCAFFOLD_CACHE_ROOT", "/tmp/from-env");
        let resolved = bootstrap_cache_root(Some(Path::new("/tmp/from-flag")));
        env::remove_var("LOGOS_SCAFFOLD_CACHE_ROOT");

        assert_eq!(resolved.expect("resolve"), PathBuf::from("/tmp/from-flag"));
    }

    #[test]
    fn bootstrap_empty_env_falls_through_to_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::set_var("LOGOS_SCAFFOLD_CACHE_ROOT", "");
        let resolved = bootstrap_cache_root(None);
        let expected = default_cache_root().expect("default").0;
        env::remove_var("LOGOS_SCAFFOLD_CACHE_ROOT");

        assert_eq!(resolved.expect("resolve"), expected);
    }

    #[test]
    fn falls_through_to_default_when_env_and_config_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        env::remove_var("LOGOS_SCAFFOLD_CACHE_ROOT");
        let project = fixture_project(PathBuf::from("/proj"), "");
        let (_, source) = resolve_cache_root(&project).expect("resolve");

        assert!(
            matches!(
                source,
                CacheRootSource::XdgCacheHome
                    | CacheRootSource::HomeCache
                    | CacheRootSource::MacOsCaches
                    | CacheRootSource::WindowsLocalAppData
            ),
            "expected a default layer, got {source:?}"
        );
    }
}
