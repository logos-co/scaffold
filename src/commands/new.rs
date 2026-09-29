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
/// template bootstraps LEZ: clone the pinned commit into the scaffold cache and
/// build it there. Scaffold deliberately ignores any `spel` that happens to be
/// on PATH — the pin recorded in scaffold.toml is the only version that matters.
///
/// Without `--vendor-deps`, `setup` resolves to this same cache checkout, so it
/// reuses the build rather than repeating it. With `--vendor-deps`, scaffold.toml
/// points `setup` at the project-local copy instead, which is a fresh clone with
/// no build in it, so `setup` builds the CLI there once more. That is deliberate:
/// the cache's `target/` is ~1.7 GB, too much to copy into every vendored project.
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

    // The CLI always comes from the scaffold cache, never from inside `target`
    // — even with `--vendor-deps`. `spel init` refuses to write into a
    // directory that already exists, so anything created under `target` before
    // the delegation makes the whole command fail. Vendoring therefore happens
    // *after* `spel init` has created the project.
    println!(
        "Cloning spel at pin {} from {} (this may take a minute the first time)...",
        DEFAULT_SPEL.sha, SPEL_SOURCE
    );
    let cached_spel = bootstrap_cache.join("repos/spel").join(DEFAULT_SPEL.sha);
    {
        let _echo_guard = crate::process::EchoGuard::suppress();
        sync_repo_to_pin_at_path_with_opts(
            &cached_spel,
            SPEL_SOURCE,
            DEFAULT_SPEL.sha,
            "spel",
            RepoSyncOptions::auto_reclone_cache_repo(),
        )?;
    }

    let spel_bin = build_spel_cli(&cached_spel)?;
    finish_spel_project(cmd, target, &spel_bin)
}

/// Everything after the spel CLI is resolved: delegate to `spel init`, then
/// layer scaffold's state, config and skills on top of what it generated.
///
/// Split out from `cmd_new_spel` so it can be driven by a stub `spel` in tests
/// without a network clone or a CLI build. The ordering it encodes has been
/// wrong twice, both times silently: **nothing** may exist at `target` when
/// `spel init` runs, because it refuses to write into an existing directory.
fn finish_spel_project(cmd: &NewCommand, target: &Path, spel_bin: &Path) -> DynResult<()> {
    run_spel_init(spel_bin, target)?;

    // `spel init` created the project directory; layer scaffold state on top.
    fs::create_dir_all(target.join(".scaffold/state"))?;
    fs::create_dir_all(target.join(".scaffold/logs"))?;

    // Now that the project exists, `--vendor-deps` can place a project-local
    // copy of the repo at the path `scaffold.toml` records.
    if cmd.vendor_deps {
        let vendored = target.join(".scaffold/repos/spel");
        println!("Vendoring spel into {}...", vendored.display());
        let _echo_guard = crate::process::EchoGuard::suppress();
        sync_repo_to_pin_at_path_with_opts(
            &vendored,
            SPEL_SOURCE,
            DEFAULT_SPEL.sha,
            "spel",
            RepoSyncOptions::fail_on_source_mismatch(),
        )?;
    }

    let cfg = build_scaffold_config(cmd, FRAMEWORK_KIND_SPEL);
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

/// Run `spel init` so that it creates exactly `target`.
///
/// `spel init` resolves the name it is given against its *own* working
/// directory, so the invocation is derived from `target` alone — its parent as
/// the working directory, its final component as the name — never from the
/// name the user typed. Using the process cwd broke `api::create_project`,
/// which takes an explicit parent; passing the typed name broke names with a
/// path separator (`nested/sub-app`), where the prefix was applied twice. Both
/// left scaffold's overlay in one directory and the spel project in another,
/// while reporting success.
fn run_spel_init(spel_bin: &Path, target: &Path) -> DynResult<()> {
    let name = target
        .file_name()
        .with_context(|| format!("project path `{}` has no final component", target.display()))?;
    let parent = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => env::current_dir()?,
    };
    fs::create_dir_all(&parent)?;

    println!(
        "Running `spel init {}` in {} (LEZ tag: {}, spel tag: {})...",
        name.to_string_lossy(),
        parent.display(),
        DEFAULT_LEZ.tag,
        DEFAULT_SPEL.tag
    );
    // Flags MUST precede the project name. `spel init`'s parser walks
    // arguments until the first non-flag, treats it as the name, and silently
    // ignores everything after it — `spel init foo --spel-tag v0.7.0` exits 0
    // having pinned nothing.
    let status = std::process::Command::new(spel_bin)
        .arg("init")
        .arg("--lez-tag")
        .arg(DEFAULT_LEZ.tag)
        // `spel init` defaults the framework to branch `main`; pin it to the
        // same tag scaffold pins so the generated project is reproducible.
        .arg("--spel-tag")
        .arg(DEFAULT_SPEL.tag)
        .arg(name)
        .current_dir(&parent)
        .status()
        .context("failed to launch spel init")?;
    if !status.success() {
        anyhow::bail!("spel init failed");
    }
    Ok(())
}

/// Build the `spel` CLI from a checkout already synced to its pin, returning
/// the binary path. For cache-managed projects `setup` builds the same target
/// in the same checkout, so a later `lgs setup` reuses it rather than rebuilding.
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

fn build_scaffold_config(cmd: &NewCommand, framework_kind: &str) -> Config {
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

    // Persist only an explicitly requested cache root. Recording the derived
    // bootstrap path here would bake a machine-specific absolute path
    // (`/home/<user>/.cache/logos-scaffold`) into every committed
    // scaffold.toml; left empty, `resolve_cache_root` derives it at runtime
    // and the file stays portable. `cmd_new_default` does the same.
    let persisted_cache_root = match &cmd.cache_root {
        Some(p) => p.display().to_string(),
        None => String::new(),
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
    use super::{build_scaffold_config, to_cargo_crate_name, NewCommand};
    use crate::constants::{DEFAULT_LEZ, DEFAULT_SPEL, FRAMEWORK_KIND_SPEL};

    /// A stand-in for `spel init` with the two properties the real one has and
    /// scaffold has to respect: it resolves the project name against its *own*
    /// working directory, and it refuses a directory that already exists. It
    /// also refuses to run anywhere outside `sandbox`, so a regression that
    /// sends it to the process cwd fails the test instead of writing a project
    /// into the repo. Each invocation is appended to `log` as `<cwd>|<args>`.
    #[cfg(unix)]
    fn stub_spel(sandbox: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let bin = sandbox.join("stub-spel");
        let log = sandbox.join("stub-spel.log");
        let script = format!(
            r#"#!/bin/sh
cwd=$(pwd -P)
case "$cwd" in
  "{sandbox}"*) ;;
  *) echo "stub spel ran outside the test sandbox: $cwd" >&2; exit 97 ;;
esac
for name; do :; done
if [ -e "$name" ]; then
  echo "Directory '$name' already exists" >&2
  exit 1
fi
mkdir "$name"
: > "$name/spel.toml"
: > "$name/Cargo.toml"
printf '%s|%s\n' "$cwd" "$*" >> "{log}"
"#,
            sandbox = sandbox.display(),
            log = log.display(),
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, log)
    }

    #[cfg(unix)]
    fn sandbox() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        // Canonical, so the stub's `pwd -P` comparison holds where the temp dir
        // sits behind a symlink (macOS `/tmp` → `/private/tmp`).
        let root = tmp.path().canonicalize().unwrap();
        (tmp, root)
    }

    /// `lgs new nested/sub-app --template spel` used to pass the typed name to
    /// `spel init` *and* run it in the target's parent, applying `nested/`
    /// twice: scaffold's overlay landed in `nested/sub-app` and the spel
    /// project in `nested/nested/sub-app`, with exit 0 and a success line.
    ///
    /// Because the stub refuses an existing directory, this also holds the
    /// ordering invariant in place: anything created at `target` before the
    /// delegation makes this test fail.
    #[cfg(unix)]
    #[test]
    fn spel_project_lands_in_one_directory_for_a_nested_name() {
        let (_tmp, root) = sandbox();
        let (spel, log) = stub_spel(&root);
        let target = root.join("nested/sub-app");
        let mut cmd = spel_cmd();
        cmd.name = "nested/sub-app".to_string();

        super::finish_spel_project(&cmd, &target, &spel).expect("scaffold spel project");

        // One directory holds both what `spel init` generated and scaffold's overlay.
        assert!(
            target.join("spel.toml").is_file(),
            "spel project missing from target"
        );
        assert!(
            target.join("scaffold.toml").is_file(),
            "overlay missing from target"
        );
        assert!(
            !root.join("nested/nested").exists(),
            "the `nested/` prefix must not be applied twice"
        );

        let calls = std::fs::read_to_string(&log).unwrap();
        let calls: Vec<&str> = calls.lines().collect();
        assert_eq!(calls.len(), 1, "spel init must run exactly once: {calls:?}");
        let (cwd, args) = calls[0].split_once('|').unwrap();
        assert_eq!(cwd, root.join("nested").to_str().unwrap());
        assert!(
            args.ends_with(" sub-app"),
            "spel init must be given the final component, got: {args}"
        );
    }

    /// `api::create_project` passes an explicit parent that is not the process
    /// cwd. `spel init` must run there, or the spel project and scaffold's
    /// overlay end up in two different directories. The stub's sandbox check
    /// is what catches a regression to the process cwd.
    #[cfg(unix)]
    #[test]
    fn spel_init_runs_in_the_target_parent_not_the_process_cwd() {
        let (_tmp, root) = sandbox();
        let (spel, log) = stub_spel(&root);
        let target = root.join("api-app");
        let mut cmd = spel_cmd();
        cmd.name = "api-app".to_string();

        super::finish_spel_project(&cmd, &target, &spel).expect("scaffold spel project");

        assert!(target.join("spel.toml").is_file());
        assert!(target.join("scaffold.toml").is_file());
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(
            calls.starts_with(&format!("{}|", root.display())),
            "spel init ran in the wrong directory: {calls}"
        );
    }

    /// The pins must reach `spel init` as flags *before* the name — its parser
    /// silently ignores anything after the first positional argument.
    #[cfg(unix)]
    #[test]
    fn spel_init_receives_both_pins_before_the_name() {
        let (_tmp, root) = sandbox();
        let (spel, log) = stub_spel(&root);
        let target = root.join("pinned");

        super::run_spel_init(&spel, &target).expect("run spel init");

        let calls = std::fs::read_to_string(&log).unwrap();
        let (_, args) = calls.trim_end().split_once('|').unwrap();
        assert_eq!(
            args,
            format!(
                "init --lez-tag {} --spel-tag {} pinned",
                DEFAULT_LEZ.tag, DEFAULT_SPEL.tag
            )
        );
    }

    fn spel_cmd() -> NewCommand {
        NewCommand {
            name: "demo".to_string(),
            template: FRAMEWORK_KIND_SPEL.to_string(),
            vendor_deps: false,
            lez_path: None,
            cache_root: None,
        }
    }

    /// Recording the derived bootstrap path would bake a machine-specific
    /// absolute path into every committed scaffold.toml. Left empty, it is
    /// resolved at runtime.
    #[test]
    fn scaffold_config_leaves_cache_root_empty_without_an_explicit_flag() {
        let cfg = build_scaffold_config(&spel_cmd(), FRAMEWORK_KIND_SPEL);
        assert_eq!(cfg.cache_root, "");
    }

    #[test]
    fn scaffold_config_persists_an_explicit_cache_root() {
        let mut cmd = spel_cmd();
        cmd.cache_root = Some(std::path::PathBuf::from("/custom/cache"));
        let cfg = build_scaffold_config(&cmd, FRAMEWORK_KIND_SPEL);
        assert_eq!(cfg.cache_root, "/custom/cache");
    }

    /// Cache-managed projects leave `path` empty so scaffold.toml stays
    /// portable; `--vendor-deps` records project-local relative paths.
    #[test]
    fn scaffold_config_records_vendored_repo_paths_only_when_asked() {
        let cfg = build_scaffold_config(&spel_cmd(), FRAMEWORK_KIND_SPEL);
        assert_eq!(cfg.lez.path, "");
        assert_eq!(cfg.spel.path, "");

        let mut vendored = spel_cmd();
        vendored.vendor_deps = true;
        let cfg = build_scaffold_config(&vendored, FRAMEWORK_KIND_SPEL);
        assert_eq!(cfg.lez.path, ".scaffold/repos/lez");
        assert_eq!(cfg.spel.path, ".scaffold/repos/spel");
    }

    /// The pins scaffold records must be the ones it passes to `spel init`,
    /// or the project tracks one version while scaffold.toml claims another.
    #[test]
    fn scaffold_config_records_the_default_pins() {
        let cfg = build_scaffold_config(&spel_cmd(), FRAMEWORK_KIND_SPEL);
        assert_eq!(cfg.lez.pin, DEFAULT_LEZ.sha);
        assert_eq!(cfg.spel.pin, DEFAULT_SPEL.sha);
        assert_eq!(cfg.framework.kind, FRAMEWORK_KIND_SPEL);
    }

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
