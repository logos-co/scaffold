use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};

use crate::config::{parse_config, update_config};
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
    write_text_atomic(&path, &update_config(&existing, &project.config)?)
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
