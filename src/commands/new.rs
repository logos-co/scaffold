use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

use crate::config::{
    default_basecamp_repo, default_lez_repo, default_lgpm_repo, lez_framework_unsupported_message,
    serialize_config,
};
use crate::constants::{
    DEFAULT_BASECAMP_PIN, DEFAULT_FRAMEWORK_VERSION, DEFAULT_LEZ, DEFAULT_LGPM_PIN,
    FRAMEWORK_KIND_DEFAULT, FRAMEWORK_KIND_LEZ_FRAMEWORK, LEZ_SOURCE, SCAFFOLD_TOML_SCHEMA_VERSION,
};
use crate::model::{Config, FrameworkConfig, LocalnetConfig, RunConfig};
use crate::project::bootstrap_cache_root;
use crate::repo::{sync_repo_to_pin_at_path_with_opts, RepoSyncOptions};
use crate::state::write_text;
use crate::template::copy::copy_dir_contents;
use crate::template::project::{apply_overlay, OverlayRenderContext};
use crate::template::skills::apply_skills;
use crate::DynResult;

#[derive(Debug)]
pub(crate) struct NewCommand {
    pub(crate) name: String,
    pub(crate) template: String,
    pub(crate) vendor_deps: bool,
    pub(crate) lez_path: Option<PathBuf>,
    pub(crate) cache_root: Option<PathBuf>,
}

pub(crate) fn cmd_new(cmd: NewCommand) -> DynResult<()> {
    let cwd = env::current_dir()?;
    create_project_in(&cwd, cmd)?;
    Ok(())
}

/// Create a new project at `base_dir/<cmd.name>` and return the project root.
/// Used by the CLI (with the current directory) and the public API (with an
/// explicit parent directory).
pub(crate) fn create_project_in(base_dir: &Path, cmd: NewCommand) -> DynResult<PathBuf> {
    let template_variant = match cmd.template.as_str() {
        FRAMEWORK_KIND_DEFAULT => cmd.template.clone(),
        FRAMEWORK_KIND_LEZ_FRAMEWORK => bail!("{}", lez_framework_unsupported_message()),
        other => {
            bail!("unsupported template `{other}`. Expected `default`.")
        }
    };

    let target = base_dir.join(&cmd.name);

    if target.exists() {
        bail!("target exists: {}", target.display());
    }

    // Run the rest in an inner function so we can clean up `target` on
    // failure. Without this, a sync/template error (e.g. a typo on
    // `--lez-path`) leaves a half-built project directory behind. The
    // `target.exists()` guard above guarantees we only delete a directory
    // we created ourselves in this run.
    let result = cmd_new_inner(&cmd, &target, &template_variant);
    if result.is_err() {
        match fs::remove_dir_all(&target) {
            Ok(()) => {}
            // Ignore NotFound: inner may have failed before creating target.
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => eprintln!(
                "warning: failed to clean up incomplete project directory {}: {err}",
                target.display()
            ),
        }
    }
    result.map(|()| target)
}

fn cmd_new_inner(cmd: &NewCommand, target: &Path, template_variant: &str) -> DynResult<()> {
    let crate_name = {
        let fallback = "app";
        let file_name = target
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(fallback);
        to_cargo_crate_name(file_name)
    };

    fs::create_dir_all(target.join(".scaffold/state"))?;
    fs::create_dir_all(target.join(".scaffold/logs"))?;

    let bootstrap_cache = bootstrap_cache_root(cmd.cache_root.as_deref())?;
    fs::create_dir_all(bootstrap_cache.join("repos"))?;
    fs::create_dir_all(bootstrap_cache.join("state"))?;
    fs::create_dir_all(bootstrap_cache.join("logs"))?;
    fs::create_dir_all(bootstrap_cache.join("builds"))?;

    let lez_source = cmd
        .lez_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| LEZ_SOURCE.to_string());

    // First-run noise reduction: scaffold normally echoes every git
    // subprocess (`$ git clone ...`, `$ git fetch --all --tags`,
    // `$ git checkout <pin>`). For a fresh `lgs new` that's three
    // shell-prefixed lines before any human-friendly status. Suppress
    // the echo for the LEZ sync only — `setup`, `localnet`, etc. keep
    // their existing echo behavior. `git clone --no-hardlinks` still
    // prints its own progress to stderr, which is reassuring during a
    // slow first clone.
    println!(
        "Cloning lez at pin {} from {} (this may take a minute the first time)...",
        DEFAULT_LEZ.sha, lez_source
    );
    let lez_repo_path = {
        let _echo_guard = crate::process::EchoGuard::suppress();
        if cmd.vendor_deps {
            let root = target.join(".scaffold/repos");
            fs::create_dir_all(&root)?;
            let lez_vendor = root.join("lez");
            sync_repo_to_pin_at_path_with_opts(
                &lez_vendor,
                &lez_source,
                DEFAULT_LEZ.sha,
                "lez",
                RepoSyncOptions::fail_on_source_mismatch(),
            )?;
            lez_vendor
        } else {
            let lez_cached = bootstrap_cache.join("repos/lez").join(DEFAULT_LEZ.sha);
            sync_repo_to_pin_at_path_with_opts(
                &lez_cached,
                &lez_source,
                DEFAULT_LEZ.sha,
                "lez",
                RepoSyncOptions::auto_reclone_cache_repo(),
            )?;
            lez_cached
        }
    };

    // Persist `path` only for vendored projects (relative, project-local).
    // Cache-managed projects leave it empty so scaffold.toml stays portable;
    // `resolve_repo_path` derives the on-disk location from cache_root + pin
    // at runtime.
    let lez_persisted_path = if cmd.vendor_deps {
        ".scaffold/repos/lez".to_string()
    } else {
        String::new()
    };

    let mut lez = default_lez_repo(DEFAULT_LEZ.sha);
    lez.source = lez_source;
    lez.path = lez_persisted_path;

    let persisted_cache_root = match &cmd.cache_root {
        Some(p) => p.display().to_string(),
        None => String::new(),
    };

    let cfg = Config {
        version: SCAFFOLD_TOML_SCHEMA_VERSION.to_string(),
        cache_root: persisted_cache_root,
        lez,
        basecamp_repo: Some(default_basecamp_repo(DEFAULT_BASECAMP_PIN)),
        lgpm_repo: Some(default_lgpm_repo(DEFAULT_LGPM_PIN)),
        wallet_home_dir: ".scaffold/wallet".to_string(),
        framework: FrameworkConfig {
            kind: template_variant.to_string(),
            version: DEFAULT_FRAMEWORK_VERSION.to_string(),
        },
        build: crate::model::BuildConfig::default(),
        localnet: LocalnetConfig::default(),
        modules: std::collections::BTreeMap::new(),
        basecamp: None,
        run: RunConfig::default(),
    };

    let template_root = lez_repo_path.join("examples/program_deployment");
    if !template_root.exists() {
        bail!("template not found at {}", template_root.display());
    }

    copy_dir_contents(&template_root, target).context("failed to copy scaffold template")?;
    seed_lockfile(&lez_repo_path, target)?;
    let overlay_ctx = OverlayRenderContext {
        crate_name: &crate_name,
        lez_pin: &cfg.lez.pin,
    };
    apply_overlay(target, template_variant, &overlay_ctx)?;
    write_text(&target.join("scaffold.toml"), &serialize_config(&cfg)?)?;
    apply_skills(&target)?;

    let old_getting_started = target.join("GETTING_STARTED.md");
    if old_getting_started.exists() {
        fs::remove_file(old_getting_started)?;
    }

    println!(
        "Created logos-scaffold project from template {} at {}",
        template_root.display(),
        target.display()
    );
    println!("Pinned lez: {}", cfg.lez.pin);
    println!("Template variant: {}", cfg.framework.kind);
    println!("AI skills installed under .claude/skills/, .cursor/rules/, and AGENTS.md.");

    Ok(())
}

/// Start the project from the dependency versions the pinned LEZ release
/// was tested with. The example crates are path dependencies inside LEZ and
/// git dependencies here, so cargo re-resolves those entries, but every
/// crates.io package keeps its locked version instead of floating to the
/// newest semver-compatible one (a fresh resolve has pulled in crates whose
/// MSRV the risc0 guest toolchain does not meet).
fn seed_lockfile(lez_repo_path: &Path, target: &Path) -> DynResult<()> {
    let lock = lez_repo_path.join("Cargo.lock");
    if lock.is_file() {
        fs::copy(&lock, target.join("Cargo.lock"))
            .with_context(|| format!("failed to copy {}", lock.display()))?;
    }
    Ok(())
}

pub(crate) fn to_cargo_crate_name(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in input.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };

        if mapped == '-' {
            if !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        } else {
            out.push(mapped);
            prev_dash = false;
        }
    }

    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "program_deployment".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::to_cargo_crate_name;

    #[test]
    fn simple_name_is_lowercased() {
        assert_eq!(to_cargo_crate_name("MyApp"), "myapp");
    }

    #[test]
    fn spaces_become_dashes() {
        assert_eq!(to_cargo_crate_name("my app"), "my-app");
    }

    #[test]
    fn special_chars_become_single_dash() {
        assert_eq!(to_cargo_crate_name("my--app"), "my-app");
        assert_eq!(to_cargo_crate_name("my___app"), "my-app");
    }

    #[test]
    fn leading_and_trailing_dashes_are_trimmed() {
        assert_eq!(to_cargo_crate_name("--myapp--"), "myapp");
        assert_eq!(to_cargo_crate_name("_myapp_"), "myapp");
    }

    #[test]
    fn empty_string_returns_default() {
        assert_eq!(to_cargo_crate_name(""), "program_deployment");
    }

    #[test]
    fn only_special_chars_returns_default() {
        assert_eq!(to_cargo_crate_name("---"), "program_deployment");
        assert_eq!(to_cargo_crate_name("!!!"), "program_deployment");
    }

    #[test]
    fn alphanumeric_preserved() {
        assert_eq!(to_cargo_crate_name("my-app-123"), "my-app-123");
    }

    #[test]
    fn unicode_becomes_dash() {
        assert_eq!(to_cargo_crate_name("héllo"), "h-llo");
    }
}
