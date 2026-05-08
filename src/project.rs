use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};

use crate::config::{parse_config, serialize_config};
use crate::model::{Project, RepoRef};
use crate::state::write_text_atomic;
use crate::DynResult;

pub(crate) fn load_project() -> DynResult<Project> {
    let cwd = env::current_dir()?;
    let root = find_project_root(cwd.clone()).ok_or_else(|| {
        anyhow!(
            "Not a logos-scaffold project at {}. Run `logos-scaffold create <name>` (or `logos-scaffold new <name>`) first.",
            cwd.display()
        )
    })?;

    let config_path = root.join("scaffold.toml");
    let cfg_text = fs::read_to_string(&config_path)?;
    let cfg = parse_config(&cfg_text)?;
    Ok(Project { root, config: cfg })
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

/// Rewrite `scaffold.toml` from scratch using the current `project.config`.
///
/// This is a destructive serialization: user comments, key ordering, and any
/// hand-formatting are lost. Callers should only invoke it when the config
/// has actually changed and the rewrite carries meaningful state. The
/// comment-preserving path is `init`'s in-place `toml_edit` migration —
/// not this function.
pub(crate) fn save_project_config(project: &Project) -> DynResult<()> {
    write_text_atomic(
        &project.root.join("scaffold.toml"),
        &serialize_config(&project.config)?,
    )
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
