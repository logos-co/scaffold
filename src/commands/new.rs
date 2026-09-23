use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

use crate::config::{
    default_basecamp_repo, default_lez_repo, default_lgpm_repo, default_spel_repo, serialize_config,
};
use crate::constants::{
    DEFAULT_BASECAMP_PIN, DEFAULT_FRAMEWORK_IDL_PATH, DEFAULT_FRAMEWORK_IDL_SPEC,
    DEFAULT_FRAMEWORK_VERSION, DEFAULT_LEZ, DEFAULT_LGPM_PIN, DEFAULT_SPEL, FRAMEWORK_KIND_DEFAULT,
    FRAMEWORK_KIND_LEZ_FRAMEWORK, FRAMEWORK_KIND_SPEL, LEZ_SOURCE, SCAFFOLD_TOML_SCHEMA_VERSION,
    SPEL_BIN_REL_PATH, SPEL_SOURCE,
};
use crate::model::{Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, RunConfig};
use crate::process::{apply_host_cc_overrides, run_checked};
use crate::project::bootstrap_cache_root;
use crate::repo::{sync_repo_to_pin_at_path_with_opts, RepoSyncOptions};
use crate::state::write_text;
use crate::template::copy::{copy_dir_contents, patch_simple_tail_call_program_id};
use crate::template::project::{apply_overlay, ensure_scaffold_in_gitignore, OverlayRenderContext};
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
        FRAMEWORK_KIND_SPEL => cmd.template.clone(),
        FRAMEWORK_KIND_LEZ_FRAMEWORK => {
            eprintln!(
                "warning: template `lez-framework` is deprecated; use `--template spel` instead."
            );
            FRAMEWORK_KIND_SPEL.to_string()
        }
        other => {
            bail!(
                "unsupported template `{other}`. \
                 Expected `default` or `spel` (or the deprecated alias `lez-framework`)."
            )
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
    // NB: the project directory is deliberately NOT created here. `spel init`
    // refuses to write into a directory that already exists, so creating
    // `target/.scaffold/...` up front would break `--template spel` outright.
    // Each path creates the project directory itself: `spel init` for spel,
    // the template overlay for default.
    let bootstrap_cache = bootstrap_cache_root(cmd.cache_root.as_deref())?;
    fs::create_dir_all(bootstrap_cache.join("repos"))?;
    fs::create_dir_all(bootstrap_cache.join("state"))?;
    fs::create_dir_all(bootstrap_cache.join("logs"))?;
    fs::create_dir_all(bootstrap_cache.join("builds"))?;

    if template_variant == FRAMEWORK_KIND_SPEL {
        cmd_new_spel(cmd, target, &bootstrap_cache)
    } else {
        cmd_new_default(cmd, target, template_variant, &bootstrap_cache)
    }
}

/// Scaffold a `spel` project by delegating to the `spel init` CLI, then
/// layering scaffold.toml and AI skills on top.
///
/// The CLI is bootstrapped from `DEFAULT_SPEL`, exactly the way the `default`
/// template bootstraps LEZ: clone the pinned commit into the scaffold cache
/// (or into the project with `--vendor-deps`) and build it there. Scaffold
/// deliberately ignores any `spel` that happens to be on PATH — the pin
/// recorded in scaffold.toml is the only version that matters, and `setup`
/// reuses this same checkout, so nothing is built twice.
fn cmd_new_spel(
    cmd: &NewCommand,
    target: &Path,
    bootstrap_cache: &std::path::Path,
) -> DynResult<()> {
    if cmd.lez_path.is_some() {
        anyhow::bail!(
            "`--lez-path` is not supported with `--template spel`.\n\
             `spel init` fetches LEZ via `--lez-tag`; a local path override is not forwarded.\n\
             Use `--template default` if you need a local LEZ checkout."
        );
    }

    println!(
        "Cloning spel at pin {} from {} (this may take a minute the first time)...",
        DEFAULT_SPEL.sha, SPEL_SOURCE
    );
    let spel_repo = {
        let _echo_guard = crate::process::EchoGuard::suppress();
        if cmd.vendor_deps {
            let root = target.join(".scaffold/repos");
            fs::create_dir_all(&root)?;
            let vendored = root.join("spel");
            sync_repo_to_pin_at_path_with_opts(
                &vendored,
                SPEL_SOURCE,
                DEFAULT_SPEL.sha,
                "spel",
                RepoSyncOptions::fail_on_source_mismatch(),
            )?;
            vendored
        } else {
            let cached = bootstrap_cache.join("repos/spel").join(DEFAULT_SPEL.sha);
            sync_repo_to_pin_at_path_with_opts(
                &cached,
                SPEL_SOURCE,
                DEFAULT_SPEL.sha,
                "spel",
                RepoSyncOptions::auto_reclone_cache_repo(),
            )?;
            cached
        }
    };

    let spel_bin = build_spel_cli(&spel_repo)?;

    println!(
        "Running `spel init {}` (LEZ tag: {}, spel tag: {})...",
        cmd.name, DEFAULT_LEZ.tag, DEFAULT_SPEL.tag
    );
    let cwd = env::current_dir()?;
    // Flags MUST precede the project name. `spel init`'s parser walks
    // arguments until the first non-flag, treats it as the name, and silently
    // ignores everything after it — `spel init foo --spel-tag v0.7.0` exits 0
    // having pinned nothing.
    let status = std::process::Command::new(&spel_bin)
        .arg("init")
        .arg("--lez-tag")
        .arg(DEFAULT_LEZ.tag)
        // `spel init` defaults the framework to branch `main`; pin it to the
        // same tag scaffold pins so the generated project is reproducible.
        .arg("--spel-tag")
        .arg(DEFAULT_SPEL.tag)
        .arg(&cmd.name)
        .current_dir(&cwd)
        .status()
        .context("failed to launch spel init")?;
    if !status.success() {
        anyhow::bail!("spel init failed");
    }

    // `spel init` created the project directory; layer scaffold state on top.
    fs::create_dir_all(target.join(".scaffold/state"))?;
    fs::create_dir_all(target.join(".scaffold/logs"))?;

    let cfg = build_scaffold_config(cmd, FRAMEWORK_KIND_SPEL, bootstrap_cache);
    write_text(&target.join("scaffold.toml"), &serialize_config(&cfg)?)?;
    ensure_scaffold_in_gitignore(target)?;
    apply_skills(target)?;

    println!("Created spel project at {}", target.display());
    println!("Pinned LEZ: {}  ({})", DEFAULT_LEZ.tag, DEFAULT_LEZ.sha);
    println!("Pinned spel: {}  ({})", DEFAULT_SPEL.tag, DEFAULT_SPEL.sha);
    println!("AI skills installed under .claude/skills/, .cursor/rules/, and AGENTS.md.");
    println!();
    println!("Next steps:");
    println!("  cd {}", cmd.name);
    println!("  lgs setup       # clone LEZ, build sequencer + wallet + spel CLI");
    println!("  lgs run         # build, start localnet, top up wallet, deploy");

    Ok(())
}

/// Build the `spel` CLI from a checkout already synced to its pin, returning
/// the binary path. `setup` builds the same target in the same checkout, so a
/// later `lgs setup` is a no-op rather than a rebuild.
fn build_spel_cli(spel_repo: &Path) -> DynResult<PathBuf> {
    let spel_bin = spel_repo.join(SPEL_BIN_REL_PATH);
    if spel_bin.is_file() {
        return Ok(spel_bin);
    }
    println!("Building the spel CLI (first run only)...");
    let mut build = std::process::Command::new("cargo");
    build
        .current_dir(spel_repo)
        .arg("build")
        .arg("--release")
        .arg("-p")
        .arg("spel");
    apply_host_cc_overrides(&mut build);
    run_checked(&mut build, "build spel CLI")?;
    if !spel_bin.is_file() {
        anyhow::bail!(
            "spel CLI was built but no binary appeared at {}",
            spel_bin.display()
        );
    }
    Ok(spel_bin)
}

/// Scaffold a `default` (bare LEZ) project by copying the LEZ example template
/// and applying scaffold's overlay files.
fn cmd_new_default(
    cmd: &NewCommand,
    target: &Path,
    template_variant: &str,
    bootstrap_cache: &std::path::Path,
) -> DynResult<()> {
    fs::create_dir_all(target.join(".scaffold/state"))?;
    fs::create_dir_all(target.join(".scaffold/logs"))?;

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

    let lez_source = cmd
        .lez_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| LEZ_SOURCE.to_string());

    // First-run noise reduction: scaffold normally echoes every git
    // subprocess. Suppress the echo for the LEZ sync only.
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

    // spel is recorded in scaffold.toml here but actually cloned + built by
    // `setup`. Persist `path` only for vendored projects (relative,
    // project-local). Cache-managed projects leave it empty so scaffold.toml
    // stays portable; `resolve_repo_path` derives the on-disk location from
    // cache_root + pin at runtime.
    let (lez_persisted_path, spel_persisted_path) = if cmd.vendor_deps {
        (
            ".scaffold/repos/lez".to_string(),
            ".scaffold/repos/spel".to_string(),
        )
    } else {
        (String::new(), String::new())
    };

    let mut lez = default_lez_repo(DEFAULT_LEZ.sha);
    lez.source = lez_source;
    lez.path = lez_persisted_path;
    let mut spel = default_spel_repo(DEFAULT_SPEL.sha);
    spel.path = spel_persisted_path;

    let persisted_cache_root = match &cmd.cache_root {
        Some(p) => p.display().to_string(),
        None => String::new(),
    };

    let cfg = Config {
        version: SCAFFOLD_TOML_SCHEMA_VERSION.to_string(),
        cache_root: persisted_cache_root,
        lez,
        spel,
        // Default scaffolded projects don't pin basecamp/lgpm — only
        // projects building Logos modules need them. `lgs basecamp setup`
        // is the entry point that backfills those sections, mirroring how
        // `lgs init` backfills `[repos.spel]` for pre-spel projects.
        basecamp_repo: Some(default_basecamp_repo(DEFAULT_BASECAMP_PIN)),
        lgpm_repo: Some(default_lgpm_repo(DEFAULT_LGPM_PIN)),
        wallet_home_dir: ".scaffold/wallet".to_string(),
        circuits: crate::model::CircuitsConfig::default(),
        framework: FrameworkConfig {
            kind: template_variant.to_string(),
            version: DEFAULT_FRAMEWORK_VERSION.to_string(),
            idl: FrameworkIdlConfig {
                spec: DEFAULT_FRAMEWORK_IDL_SPEC.to_string(),
                path: DEFAULT_FRAMEWORK_IDL_PATH.to_string(),
            },
        },
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
    if template_variant == FRAMEWORK_KIND_DEFAULT {
        patch_simple_tail_call_program_id(target)?;
    }
    let overlay_ctx = OverlayRenderContext {
        crate_name: &crate_name,
        lez_pin: &cfg.lez.pin,
        lez_tag: DEFAULT_LEZ.tag,
        spel_pin: &cfg.spel.pin,
    };
    apply_overlay(target, template_variant, &overlay_ctx)?;

    write_text(&target.join("scaffold.toml"), &serialize_config(&cfg)?)?;
    apply_skills(target)?;

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

fn build_scaffold_config(
    cmd: &NewCommand,
    framework_kind: &str,
    bootstrap_cache: &std::path::Path,
) -> Config {
    let (lez_persisted_path, spel_persisted_path) = if cmd.vendor_deps {
        (
            ".scaffold/repos/lez".to_string(),
            ".scaffold/repos/spel".to_string(),
        )
    } else {
        (String::new(), String::new())
    };

    let lez_source = cmd
        .lez_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| LEZ_SOURCE.to_string());

    let mut lez = default_lez_repo(DEFAULT_LEZ.sha);
    lez.source = lez_source;
    lez.path = lez_persisted_path;
    let mut spel = default_spel_repo(DEFAULT_SPEL.sha);
    spel.path = spel_persisted_path;

    let persisted_cache_root = match &cmd.cache_root {
        Some(p) => p.display().to_string(),
        None => bootstrap_cache.display().to_string(),
    };

    Config {
        version: SCAFFOLD_TOML_SCHEMA_VERSION.to_string(),
        cache_root: persisted_cache_root,
        lez,
        spel,
        basecamp_repo: Some(default_basecamp_repo(DEFAULT_BASECAMP_PIN)),
        lgpm_repo: Some(default_lgpm_repo(DEFAULT_LGPM_PIN)),
        wallet_home_dir: ".scaffold/wallet".to_string(),
        circuits: crate::model::CircuitsConfig::default(),
        framework: FrameworkConfig {
            kind: framework_kind.to_string(),
            version: DEFAULT_FRAMEWORK_VERSION.to_string(),
            idl: FrameworkIdlConfig {
                spec: DEFAULT_FRAMEWORK_IDL_SPEC.to_string(),
                path: DEFAULT_FRAMEWORK_IDL_PATH.to_string(),
            },
        },
        localnet: LocalnetConfig::default(),
        modules: std::collections::BTreeMap::new(),
        basecamp: None,
        run: RunConfig::default(),
    }
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
