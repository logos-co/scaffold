use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use crate::commands::client::generate_clients_from_current_idl;
use crate::commands::idl::build_idl_for_current_project;
use crate::commands::setup::cmd_setup;
use crate::constants::{FRAMEWORK_KIND_DEFAULT, FRAMEWORK_KIND_LEZ_FRAMEWORK, METHODS_DIR};
use crate::process::run_checked;
use crate::project::{load_project, run_in_project_dir};
use crate::DynResult;

pub(crate) fn cmd_build_shortcut(project_dir: Option<PathBuf>) -> DynResult<()> {
    run_in_project_dir(project_dir.as_deref(), || {
        cmd_setup()?;
        let cwd = env::current_dir()?;

        let project = load_project()?;
        build_workspace_for_current_project(&cwd)?;
        match project.config.framework.kind.as_str() {
            FRAMEWORK_KIND_DEFAULT => {}
            FRAMEWORK_KIND_LEZ_FRAMEWORK => {
                build_idl_for_current_project()?;
                generate_clients_from_current_idl()?;
            }
            other => {
                println!(
                    "Skipping IDL/client generation for framework kind `{}`",
                    other
                );
            }
        }
        // Guest building is intentionally framework-agnostic: any project with
        // a `methods/Cargo.toml` (Risc0 guest crate excluded from the parent
        // workspace) gets it compiled, regardless of `framework.kind`.
        build_methods_guests(&cwd)?;

        Ok(())
    })
}

fn build_workspace_for_current_project(cwd: &Path) -> DynResult<()> {
    run_checked(
        Command::new("cargo")
            .current_dir(cwd)
            .arg("build")
            .arg("--workspace"),
        "cargo build --workspace (project)",
    )
}

/// Detect and build Risc0 guest binaries in the `methods/` directory.
///
/// Risc0 guest crates are intentionally excluded from the main workspace
/// because they target `riscv32im-risc0-zkvm-elf`. This function detects
/// whether a `methods/` package exists and compiles it as part of the
/// standard build pipeline.
fn build_methods_guests(cwd: &Path) -> DynResult<()> {
    let methods_manifest = cwd.join(METHODS_DIR).join("Cargo.toml");
    if methods_manifest.is_file() {
        println!("Building guest methods...");
        // `--release` is required: deploy-side discovery (`deploy.rs`,
        // `GUEST_BIN_SEARCH_ROOTS`) only matches `.bin` files whose path
        // contains a `release/` component, so a debug build here would
        // produce artefacts the deploy step cannot find.
        run_checked(
            Command::new("cargo")
                .current_dir(cwd)
                .arg("build")
                .arg("--release")
                .arg("--manifest-path")
                .arg(&methods_manifest),
            "cargo build --release --manifest-path methods/Cargo.toml",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::build_methods_guests;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn build_methods_guests_is_noop_when_methods_dir_absent() {
        let tmp = tempdir().expect("create temp dir");
        build_methods_guests(tmp.path()).expect("no methods/ -> Ok");
    }

    #[test]
    fn build_methods_guests_is_noop_when_methods_dir_lacks_cargo_toml() {
        let tmp = tempdir().expect("create temp dir");
        fs::create_dir(tmp.path().join("methods")).expect("mkdir methods");
        build_methods_guests(tmp.path()).expect("methods/ without Cargo.toml -> Ok");
    }

    #[test]
    fn build_methods_guests_invokes_cargo_when_manifest_present() {
        let tmp = tempdir().expect("create temp dir");
        let methods = tmp.path().join("methods");
        fs::create_dir(&methods).expect("mkdir methods");
        // Intentionally invalid manifest content so cargo errors out fast and
        // we can assert that the cargo invocation was actually attempted (vs.
        // silently no-op'd by our own gate).
        fs::write(methods.join("Cargo.toml"), "this is not valid toml")
            .expect("write methods/Cargo.toml");
        let err = build_methods_guests(tmp.path())
            .expect_err("invalid manifest -> cargo should fail and propagate");
        let msg = format!("{err:#}");
        // Match the substring we control (the cargo flags) rather than the
        // full label string, so this test does not break if `run_checked`'s
        // error format is reworded.
        assert!(
            msg.contains("cargo build --release"),
            "expected error to mention the cargo invocation; got: {msg}"
        );
    }
}
