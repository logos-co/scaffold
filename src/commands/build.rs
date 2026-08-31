use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Once;

use anyhow::{anyhow, bail, Context};

use crate::circuits::ensure_circuits_for_project;
use crate::commands::client::generate_clients_from_current_idl;
use crate::commands::idl::build_idl_for_current_project;
use crate::commands::setup::cmd_setup;
use crate::constants::{
    DEFAULT_RISC0_METHODS_ENTRY, FRAMEWORK_KIND_DEFAULT, FRAMEWORK_KIND_LEZ_FRAMEWORK,
    GUEST_DOCKER_TARGET_DIR, METHODS_DIR,
};
use crate::model::GuestBuildMode;
use crate::process::{apply_host_cc_overrides, run_checked, which};
use crate::project::{load_project, run_in_project_dir};
use crate::DynResult;

pub(crate) fn cmd_build_shortcut(
    project_dir: Option<PathBuf>,
    prebuilt: bool,
    guest_override: Option<GuestBuildMode>,
) -> DynResult<()> {
    run_in_project_dir(project_dir.as_deref(), || {
        let cwd = env::current_dir()?;
        // Check the deterministic toolchain before `setup`, not at the guest
        // step: `setup` can compile the LEZ sequencer from source for
        // minutes, and telling someone their Docker daemon is down only
        // after that is a bad trade for one `which` and one `docker info`.
        let early_mode = guest_override.unwrap_or(load_project()?.config.build.guest);
        if early_mode.is_deterministic() && cwd.join(METHODS_DIR).join("Cargo.toml").is_file() {
            preflight_deterministic_toolchain()?;
        }

        cmd_setup(prebuilt)?;

        // Re-read after `setup`, which is allowed to touch project state.
        let project = load_project()?;
        ensure_circuits_for_project(&project)?;
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
        let mode = guest_override.unwrap_or(project.config.build.guest);
        build_methods_guests(&cwd, mode, &project.config.build.risc0_docker_tag)?;

        Ok(())
    })
}

fn build_workspace_for_current_project(cwd: &Path) -> DynResult<()> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(cwd).arg("build").arg("--workspace");
    apply_host_cc_overrides(&mut cmd);
    run_checked(&mut cmd, "cargo build --workspace (project)")
}

/// Detect and build Risc0 guest binaries in the `methods/` directory.
///
/// Risc0 guest crates are intentionally excluded from the main workspace
/// because they target `riscv32im-risc0-zkvm-elf`. This function detects
/// whether a `methods/` package exists and compiles it as part of the
/// standard build pipeline, using the strategy `mode` selects.
///
/// Both modes can leave a `<program>.bin` on disk, with different bytes and
/// therefore different `program_id`s. Two rules keep "the last `lgs build`
/// decides what `lgs deploy` ships" true (scaffold#259):
///
/// - deploy-side discovery ranks a `docker` path component above `release`
///   (see `deploy::discover_program_binaries`), which settles docker mode —
///   the workspace build re-runs `embed_methods()` for the `methods` crate
///   either way, so the `release/` artefacts cannot simply be removed;
/// - local mode deletes the deterministic tree outright, because nothing
///   rebuilds it and it would otherwise outrank the fresh local artefacts
///   forever.
fn build_methods_guests(cwd: &Path, mode: GuestBuildMode, docker_tag: &str) -> DynResult<()> {
    let methods_manifest = cwd.join(METHODS_DIR).join("Cargo.toml");
    if !methods_manifest.is_file() {
        return Ok(());
    }
    match mode {
        GuestBuildMode::Local => {
            clear_docker_guest_artifacts(cwd)?;
            build_methods_guests_local(cwd, &methods_manifest)?;
            warn_local_guest_build_is_not_reproducible();
        }
        GuestBuildMode::Docker => build_methods_guests_docker(cwd, &methods_manifest, docker_tag)?,
    }
    Ok(())
}

/// Host-toolchain guest build: `cargo build` on `methods/`, whose `build.rs`
/// runs `risc0_build::embed_methods()`. Fast and Docker-free, but the ELF it
/// emits depends on the local Rust/clang, so `program_id` is not portable.
fn build_methods_guests_local(cwd: &Path, methods_manifest: &Path) -> DynResult<()> {
    println!("Building guest methods...");
    // `--release` is required: deploy-side discovery (`deploy.rs`,
    // `GUEST_BIN_SEARCH_ROOTS`) only matches `.bin` files whose path
    // contains a `release/` component, so a debug build here would
    // produce artefacts the deploy step cannot find.
    let mut cmd = Command::new("cargo");
    cmd.current_dir(cwd)
        .arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(methods_manifest);
    apply_host_cc_overrides(&mut cmd);
    run_checked(
        &mut cmd,
        "cargo build --release --manifest-path methods/Cargo.toml",
    )
}

/// Print the non-reproducibility note once per process. `lgs run --watch`
/// rebuilds in-process on every file change; repeating this on each rebuild
/// would train people to ignore it.
fn warn_local_guest_build_is_not_reproducible() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        println!(
            "Note: guest ELFs were built with the local risc0 toolchain, so their\n\
             `program_id` can differ on another machine, OS, or Rust/clang version.\n\
             For reproducible artefacts add this to scaffold.toml (needs Docker):\n\
             \n    [build]\n    guest = \"docker\"\n"
        );
    });
}

/// Deterministic guest build: `cargo risczero build` compiles the guest inside
/// the pinned `risczero/risc0-guest-builder:<tag>` container, so the ELF bytes
/// — and the `program_id` derived from them — depend only on the source and
/// the tag. This is the strategy `lssa` uses for the program artefacts it
/// ships and verifies in CI.
fn build_methods_guests_docker(
    cwd: &Path,
    methods_manifest: &Path,
    docker_tag: &str,
) -> DynResult<()> {
    let guests = risc0_guest_manifests(cwd, methods_manifest)?;
    preflight_deterministic_toolchain()?;

    // One `CARGO_TARGET_DIR` is shared by every guest package, so clear it
    // once up front rather than per package — otherwise the second package's
    // build would delete the first's output. Clearing at all matters because
    // `--output` only overwrites the files this build produces: a program
    // deleted from the source tree would otherwise keep a deployable `.bin`.
    let target_dir = cwd.join(GUEST_DOCKER_TARGET_DIR);
    remove_dir_if_present(&target_dir)?;

    println!("Building guest methods (deterministic, risc0-guest-builder:{docker_tag})...");
    for guest_manifest in &guests {
        let mut cmd = deterministic_guest_command(cwd, guest_manifest, &target_dir, docker_tag);
        run_checked(
            &mut cmd,
            &format!(
                "cargo risczero build --manifest-path {}",
                guest_manifest
                    .strip_prefix(cwd)
                    .unwrap_or(guest_manifest)
                    .display()
            ),
        )?;
    }

    println!(
        "Guest ELFs (deterministic) in {}",
        target_dir
            .join("riscv32im-risc0-zkvm-elf")
            .join("docker")
            .display()
    );
    Ok(())
}

/// Build the `cargo risczero build` invocation for one guest package.
///
/// Split out so the contract is unit-testable without Docker. Three things
/// matter and all three are asserted in tests:
///
/// - `RISC0_DOCKER_CONTAINER_TAG` is set explicitly. risc0 falls back to
///   whatever tag the installed `cargo-risczero` was compiled against, which
///   would make the ELF depend on the developer's tooling version — the exact
///   property this mode exists to remove.
/// - `CARGO_TARGET_DIR` points at scaffold's own tree. `cargo risczero build`
///   emits to `<CARGO_TARGET_DIR>/riscv32im-risc0-zkvm-elf/docker/<bin>.bin`,
///   so this is what keeps the deterministic output out of the
///   `target/riscv-guest/` tree `embed_methods()` owns.
/// - the manifest is the *guest* crate, not `methods/` — `cargo risczero
///   build` compiles guest packages directly and never runs `methods/build.rs`.
///
/// `cwd` is the project root, which is also the Docker build context risc0
/// uses (it defaults to the current directory).
fn deterministic_guest_command(
    cwd: &Path,
    guest_manifest: &Path,
    target_dir: &Path,
    docker_tag: &str,
) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(cwd)
        .arg("risczero")
        .arg("build")
        .arg("--manifest-path")
        .arg(guest_manifest)
        .env("RISC0_DOCKER_CONTAINER_TAG", docker_tag)
        .env("CARGO_TARGET_DIR", target_dir);
    cmd
}

/// Resolve the guest packages `risc0_build::embed_methods()` would compile:
/// the `[package.metadata.risc0].methods` list in `methods/Cargo.toml`, which
/// holds paths relative to that manifest's directory. Defaults to `["guest"]`,
/// the entry both scaffold templates (and the upstream LEZ example they derive
/// from) declare.
fn risc0_guest_manifests(cwd: &Path, methods_manifest: &Path) -> DynResult<Vec<PathBuf>> {
    let text = std::fs::read_to_string(methods_manifest)
        .with_context(|| format!("read {}", methods_manifest.display()))?;
    let doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("parse {}", methods_manifest.display()))?;
    let declared = doc
        .get("package")
        .and_then(toml_edit::Item::as_table)
        .and_then(|t| t.get("metadata"))
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|t| t.get("risc0"))
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|t| t.get("methods").cloned());

    let entries: Vec<String> = match declared {
        Some(item) => item
            .as_array()
            .ok_or_else(|| {
                anyhow!(
                    "{}: [package.metadata.risc0].methods must be an array of paths",
                    methods_manifest.display()
                )
            })?
            .iter()
            .map(|v| {
                v.as_str().map(str::to_string).ok_or_else(|| {
                    anyhow!(
                        "{}: [package.metadata.risc0].methods entries must be strings",
                        methods_manifest.display()
                    )
                })
            })
            .collect::<DynResult<Vec<_>>>()?,
        None => vec![DEFAULT_RISC0_METHODS_ENTRY.to_string()],
    };

    let methods_dir = cwd.join(METHODS_DIR);
    let mut manifests = Vec::with_capacity(entries.len());
    for entry in entries {
        // The entry is attacker-controllable only by whoever already owns the
        // manifest, but it is joined onto the project root and handed to
        // cargo, so keep it inside the project the same way `[circuits]`
        // paths are checked.
        let path = Path::new(&entry);
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            bail!(
                "{}: [package.metadata.risc0].methods entry {entry:?} must be a relative path \
                 inside the project (no `..`)",
                methods_manifest.display()
            );
        }
        let manifest = methods_dir.join(path).join("Cargo.toml");
        if !manifest.is_file() {
            bail!(
                "guest manifest not found at `{}` (declared by [package.metadata.risc0].methods \
                 in {})",
                manifest.display(),
                methods_manifest.display()
            );
        }
        manifests.push(manifest);
    }
    Ok(manifests)
}

/// Fail before the first (slow) container pull if the deterministic path
/// cannot possibly work, with the fix rather than a raw docker/cargo error.
fn preflight_deterministic_toolchain() -> DynResult<()> {
    if which("cargo-risczero").is_none() {
        bail!(
            "[build].guest = \"docker\" needs `cargo-risczero` on PATH, which was not found.\n\
             Install it with `cargo install cargo-risczero` (or `rzup install cargo-risczero`), \
             or set [build].guest = \"local\" in scaffold.toml to build guests with the host \
             toolchain (non-reproducible `program_id`)."
        );
    }
    if which("docker").is_none() {
        bail!(
            "[build].guest = \"docker\" needs `docker` on PATH, which was not found.\n\
             Install Docker, or set [build].guest = \"local\" in scaffold.toml to build guests \
             with the host toolchain (non-reproducible `program_id`)."
        );
    }
    let daemon_ok = Command::new("docker")
        .arg("info")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !daemon_ok {
        bail!(
            "[build].guest = \"docker\" needs a running Docker daemon; `docker info` failed.\n\
             Start Docker and retry, or set [build].guest = \"local\" in scaffold.toml to build \
             guests with the host toolchain (non-reproducible `program_id`)."
        );
    }
    Ok(())
}

/// Drop the deterministic artefact tree so a later `lgs deploy` cannot ship
/// ELFs from a build mode that is no longer in effect. Only touches the
/// scaffold-owned `target/riscv-guest-docker` directory.
fn clear_docker_guest_artifacts(cwd: &Path) -> DynResult<()> {
    let dir = cwd.join(GUEST_DOCKER_TARGET_DIR);
    if !dir.exists() {
        return Ok(());
    }
    println!(
        "Clearing deterministic guest artefacts in {} (this build uses the local toolchain)",
        dir.display()
    );
    remove_dir_if_present(&dir)
}

fn remove_dir_if_present(dir: &Path) -> DynResult<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(anyhow!("failed to remove {}: {err}", dir.display()).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const DOCKER_TAG: &str = "r0.0.0.0";

    fn write_methods_manifest(root: &Path, body: &str) -> PathBuf {
        let methods = root.join(METHODS_DIR);
        fs::create_dir_all(&methods).expect("mkdir methods");
        let manifest = methods.join("Cargo.toml");
        fs::write(&manifest, body).expect("write methods/Cargo.toml");
        manifest
    }

    #[test]
    fn build_methods_guests_is_noop_when_methods_dir_absent() {
        let tmp = tempdir().expect("create temp dir");
        build_methods_guests(tmp.path(), GuestBuildMode::Local, DOCKER_TAG)
            .expect("no methods/ -> Ok");
    }

    #[test]
    fn build_methods_guests_is_noop_when_methods_dir_lacks_cargo_toml() {
        let tmp = tempdir().expect("create temp dir");
        fs::create_dir(tmp.path().join("methods")).expect("mkdir methods");
        build_methods_guests(tmp.path(), GuestBuildMode::Local, DOCKER_TAG)
            .expect("methods/ without Cargo.toml -> Ok");
    }

    #[test]
    fn build_methods_guests_invokes_cargo_when_manifest_present() {
        let tmp = tempdir().expect("create temp dir");
        // Intentionally invalid manifest content so cargo errors out fast and
        // we can assert that the cargo invocation was actually attempted (vs.
        // silently no-op'd by our own gate).
        write_methods_manifest(tmp.path(), "this is not valid toml");
        let err = build_methods_guests(tmp.path(), GuestBuildMode::Local, DOCKER_TAG)
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

    /// A local-mode build must not leave a previous deterministic build's
    /// `.bin` files on disk: deploy prefers them, so they would keep being
    /// shipped long after the source moved on (scaffold#259).
    #[test]
    fn local_build_clears_stale_deterministic_artifacts() {
        let tmp = tempdir().expect("create temp dir");
        write_methods_manifest(tmp.path(), "this is not valid toml");
        let docker_dir = tmp
            .path()
            .join(GUEST_DOCKER_TARGET_DIR)
            .join("riscv32im-risc0-zkvm-elf")
            .join("docker");
        fs::create_dir_all(&docker_dir).expect("mkdir docker artefacts");
        fs::write(docker_dir.join("counter.bin"), b"stale").expect("write stale artefact");

        // The cargo step fails (invalid manifest), but the clear happens first
        // and is what this asserts.
        let _ = build_methods_guests(tmp.path(), GuestBuildMode::Local, DOCKER_TAG);
        assert!(
            !tmp.path().join(GUEST_DOCKER_TARGET_DIR).exists(),
            "local build should have removed {GUEST_DOCKER_TARGET_DIR}"
        );
    }

    #[test]
    fn deterministic_guest_command_pins_the_tag_and_target_dir() {
        let cwd = Path::new("/proj");
        let manifest = Path::new("/proj/methods/guest/Cargo.toml");
        let target_dir = Path::new("/proj").join(GUEST_DOCKER_TARGET_DIR);
        let cmd = deterministic_guest_command(cwd, manifest, &target_dir, "r0.1.91.1");

        assert_eq!(cmd.get_program(), "cargo");
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            args,
            vec![
                "risczero".to_string(),
                "build".to_string(),
                "--manifest-path".to_string(),
                manifest.display().to_string(),
            ]
        );

        let envs: Vec<(String, String)> = cmd
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();
        assert!(
            envs.contains(&(
                "RISC0_DOCKER_CONTAINER_TAG".to_string(),
                "r0.1.91.1".to_string()
            )),
            "the pinned tag must not be left to cargo-risczero's default; got: {envs:?}"
        );
        assert!(
            envs.contains(&(
                "CARGO_TARGET_DIR".to_string(),
                target_dir.display().to_string()
            )),
            "deterministic output must land in scaffold's own tree; got: {envs:?}"
        );
        assert_eq!(cmd.get_current_dir(), Some(cwd));
    }

    #[test]
    fn guest_manifests_default_to_the_guest_subcrate() {
        let tmp = tempdir().expect("create temp dir");
        let manifest = write_methods_manifest(tmp.path(), "[package]\nname = \"m\"\n");
        let guest = tmp.path().join("methods/guest");
        fs::create_dir_all(&guest).expect("mkdir guest");
        fs::write(guest.join("Cargo.toml"), "[package]\nname = \"g\"\n").expect("write guest");

        let found = risc0_guest_manifests(tmp.path(), &manifest).expect("resolve guests");
        assert_eq!(found, vec![guest.join("Cargo.toml")]);
    }

    #[test]
    fn guest_manifests_follow_declared_risc0_metadata() {
        let tmp = tempdir().expect("create temp dir");
        let manifest = write_methods_manifest(
            tmp.path(),
            "[package]\nname = \"m\"\n\n[package.metadata.risc0]\nmethods = [\"a\", \"b\"]\n",
        );
        for name in ["a", "b"] {
            let dir = tmp.path().join("methods").join(name);
            fs::create_dir_all(&dir).expect("mkdir guest");
            fs::write(dir.join("Cargo.toml"), "[package]\nname = \"g\"\n").expect("write guest");
        }

        let found = risc0_guest_manifests(tmp.path(), &manifest).expect("resolve guests");
        assert_eq!(
            found,
            vec![
                tmp.path().join("methods/a/Cargo.toml"),
                tmp.path().join("methods/b/Cargo.toml"),
            ]
        );
    }

    #[test]
    fn guest_manifests_reject_paths_escaping_the_project() {
        let tmp = tempdir().expect("create temp dir");
        let manifest = write_methods_manifest(
            tmp.path(),
            "[package]\nname = \"m\"\n\n[package.metadata.risc0]\nmethods = [\"../../etc\"]\n",
        );
        let err = risc0_guest_manifests(tmp.path(), &manifest).expect_err("`..` must be rejected");
        assert!(
            format!("{err:#}").contains("no `..`"),
            "expected a traversal error; got: {err:#}"
        );
    }

    #[test]
    fn guest_manifests_report_a_missing_declared_guest() {
        let tmp = tempdir().expect("create temp dir");
        let manifest = write_methods_manifest(
            tmp.path(),
            "[package]\nname = \"m\"\n\n[package.metadata.risc0]\nmethods = [\"nope\"]\n",
        );
        let err = risc0_guest_manifests(tmp.path(), &manifest).expect_err("missing guest crate");
        assert!(
            format!("{err:#}").contains("guest manifest not found"),
            "expected a missing-manifest error; got: {err:#}"
        );
    }
}
