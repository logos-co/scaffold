use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

use crate::config::{
    default_basecamp_repo, default_lez_repo, default_lgpm_repo, default_spel_repo, serialize_config,
};
use crate::constants::{
    DEFAULT_BASECAMP_PIN, DEFAULT_FRAMEWORK_IDL_PATH, DEFAULT_FRAMEWORK_IDL_SPEC,
    DEFAULT_FRAMEWORK_VERSION, DEFAULT_LEZ, DEFAULT_LGPM_PIN, DEFAULT_SPEL, FRAMEWORK_KIND_DEFAULT,
    FRAMEWORK_KIND_LEZ_FRAMEWORK, LEZ_SOURCE, SCAFFOLD_TOML_SCHEMA_VERSION,
};
use crate::model::{Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, RunConfig};
use crate::project::default_cache_root;
use crate::repo::{sync_repo_to_pin_at_path_with_opts, RepoSyncOptions};
use crate::state::write_text;
use crate::template::copy::{copy_dir_contents, patch_simple_tail_call_program_id};
use crate::template::project::{apply_overlay, OverlayRenderContext};
use crate::DynResult;

#[derive(Debug)]
pub(crate) struct NewCommand {
    pub(crate) name: String,
    pub(crate) template: String,
    pub(crate) vendor_deps: bool,
    pub(crate) lez_path: Option<PathBuf>,
}

pub(crate) fn cmd_new(cmd: NewCommand) -> DynResult<()> {
    let template_variant = match cmd.template.as_str() {
        FRAMEWORK_KIND_DEFAULT | FRAMEWORK_KIND_LEZ_FRAMEWORK => cmd.template.clone(),
        other => {
            bail!("unsupported template `{other}`. Expected `default` or `lez-framework`.")
        }
    };

    let cwd = env::current_dir()?;
    let target = cwd.join(&cmd.name);
    let crate_name = {
        let fallback = "app";
        let file_name = target
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(fallback);
        to_cargo_crate_name(file_name)
    };

    if target.exists() {
        bail!("target exists: {}", target.display());
    }

    fs::create_dir_all(target.join(".scaffold/state"))?;
    fs::create_dir_all(target.join(".scaffold/logs"))?;

    let (bootstrap_cache, _) = default_cache_root()?;
    fs::create_dir_all(bootstrap_cache.join("repos"))?;
    fs::create_dir_all(bootstrap_cache.join("state"))?;
    fs::create_dir_all(bootstrap_cache.join("logs"))?;
    fs::create_dir_all(bootstrap_cache.join("builds"))?;

    let lez_source = cmd
        .lez_path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| LEZ_SOURCE.to_string());

    let lez_repo_path = if cmd.vendor_deps {
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

    let cfg = Config {
        version: SCAFFOLD_TOML_SCHEMA_VERSION.to_string(),
        cache_root: String::new(),
        lez,
        spel,
        // Default scaffolded projects don't pin basecamp/lgpm — only
        // projects building Logos modules need them. `lgs basecamp setup`
        // is the entry point that backfills those sections, mirroring how
        // `lgs init` backfills `[repos.spel]` for pre-spel projects.
        basecamp_repo: Some(default_basecamp_repo(DEFAULT_BASECAMP_PIN)),
        lgpm_repo: Some(default_lgpm_repo(DEFAULT_LGPM_PIN)),
        wallet_home_dir: ".scaffold/wallet".to_string(),
        framework: FrameworkConfig {
            kind: template_variant.clone(),
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

    copy_dir_contents(&template_root, &target).context("failed to copy scaffold template")?;
    if template_variant == FRAMEWORK_KIND_DEFAULT {
        patch_simple_tail_call_program_id(&target)?;
    }
    let overlay_ctx = OverlayRenderContext {
        crate_name: &crate_name,
        lez_pin: &cfg.lez.pin,
        spel_tag: DEFAULT_SPEL.tag,
    };
    apply_overlay(&target, &template_variant, &overlay_ctx)?;
    if template_variant == FRAMEWORK_KIND_LEZ_FRAMEWORK {
        cleanup_lez_hello_artifacts(&target)?;
    }
    write_text(&target.join("scaffold.toml"), &serialize_config(&cfg)?)?;

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

    Ok(())
}

fn cleanup_lez_hello_artifacts(project_root: &Path) -> DynResult<()> {
    const RUNNER_FILES: &[&str] = &[
        "src/bin/run_hello_world.rs",
        "src/bin/run_hello_world_private.rs",
        "src/bin/run_hello_world_with_authorization.rs",
        "src/bin/run_hello_world_with_move_function.rs",
        "src/bin/run_hello_world_through_tail_call.rs",
        "src/bin/run_hello_world_through_tail_call_private.rs",
        "src/bin/run_hello_world_with_authorization_through_tail_call_with_pda.rs",
    ];
    const GUEST_METHOD_FILES: &[&str] = &[
        "methods/guest/src/bin/hello_world.rs",
        "methods/guest/src/bin/hello_world_with_authorization.rs",
        "methods/guest/src/bin/hello_world_with_move_function.rs",
        "methods/guest/src/bin/simple_tail_call.rs",
        "methods/guest/src/bin/tail_call_with_pda.rs",
    ];

    for rel_path in RUNNER_FILES.iter().chain(GUEST_METHOD_FILES) {
        let path = project_root.join(rel_path);
        if path.exists() {
            fs::remove_file(path)?;
        }
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
