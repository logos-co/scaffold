use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail};

use crate::commands::idl::build_idl_for_current_project;
use crate::constants::{
    FRAMEWORK_KIND_LEZ_FRAMEWORK, FRAMEWORK_KIND_SPEL, SPEL_CLIENT_GEN_BIN_REL_PATH,
};
use crate::model::Project;
use crate::process::{apply_host_cc_overrides, run_checked};
use crate::project::{load_project, resolve_repo_path, run_in_project_dir};
use crate::DynResult;

pub(crate) fn cmd_client(args: &[String]) -> DynResult<()> {
    if args.is_empty() {
        bail!("usage: logos-scaffold build client [project-path]");
    }

    match args[0].as_str() {
        "build" => {
            let project_dir =
                parse_optional_project_path(&args[1..], "logos-scaffold build client")?;
            run_in_project_dir(project_dir.as_deref(), build_clients_for_current_project)
        }
        other => Err(anyhow!("unknown client command: {other}")),
    }
}

pub(crate) fn build_clients_for_current_project() -> DynResult<()> {
    let project = load_project()?;
    if project.config.framework.kind == FRAMEWORK_KIND_SPEL {
        // spel projects: delegate IDL regen + FFI gen entirely to spel's own tooling.
        build_idl_for_current_project()?;
        return run_spel_ffi_gen(&project);
    }
    let project = require_lez_framework_project(project)?;
    // Always regenerate IDL in direct `build client` flows to prevent stale IDL drift.
    println!("[client] Regenerating IDL to ensure it is fresh...");
    build_idl_for_current_project()?;
    generate_clients_from_project_idl(&project)
}

pub(crate) fn generate_clients_from_current_idl() -> DynResult<()> {
    let project = load_project()?;
    if project.config.framework.kind == FRAMEWORK_KIND_SPEL {
        return run_spel_ffi_gen(&project);
    }
    let project = require_lez_framework_project(project)?;
    generate_clients_from_project_idl(&project)
}

/// Run FFI/client generation for a spel project.
///
/// There is no `spel ffi-gen` subcommand: generation is done by the separate
/// `spel-client-gen` binary, and the `Makefile` written by `spel init` is what
/// knows this project's IDL file and FFI output directory. So drive `make
/// ffi-gen` rather than restating that layout here, and point it at the
/// vendored generator — the Makefile declares `SPEL_CLIENT_GEN ?= ...`, which
/// the environment overrides, so this works even when only `spel` itself is on
/// PATH.
fn run_spel_ffi_gen(project: &Project) -> DynResult<()> {
    let generator = resolve_repo_path(project, &project.config.spel, "spel")?
        .join(SPEL_CLIENT_GEN_BIN_REL_PATH);
    if !generator.exists() {
        bail!(
            "vendored spel-client-gen binary not found at `{}`\nNext step: run `logos-scaffold setup` to build it.",
            generator.display()
        );
    }
    let mut cmd = Command::new("make");
    cmd.arg("ffi-gen")
        .current_dir(&project.root)
        .env("SPEL_CLIENT_GEN", &generator);
    apply_host_cc_overrides(&mut cmd);
    run_checked(&mut cmd, "generate spel FFI client")
}

fn require_lez_framework_project(project: Project) -> DynResult<Project> {
    if project.config.framework.kind == FRAMEWORK_KIND_LEZ_FRAMEWORK {
        return Ok(project);
    }
    bail!(
        "`build client` is only supported for `spel` and `lez-framework` projects \
         (current framework.kind = `{}`).\n\
         Use `logos-scaffold build` for the framework-agnostic build, \
         or set `framework.kind = \"spel\"` in scaffold.toml.",
        project.config.framework.kind
    )
}

fn generate_clients_from_project_idl(project: &Project) -> DynResult<()> {
    let idl_dir = project.root.join(&project.config.framework.idl.path);
    let out_dir = project.root.join("src/generated");
    fs::create_dir_all(&out_dir)?;

    let generator_manifest = project.root.join("crates/lez-client-gen/Cargo.toml");
    if !generator_manifest.exists() {
        bail!(
            "missing client generator crate at {}",
            generator_manifest.display()
        );
    }

    let mut cmd = Command::new("cargo");
    cmd.current_dir(&project.root)
        .arg("run")
        .arg("--manifest-path")
        .arg(&generator_manifest)
        .arg("--")
        .arg("--idl-dir")
        .arg(&idl_dir)
        .arg("--out-dir")
        .arg(&out_dir);
    // The generator shares the project workspace target dir; keep its build
    // env consistent with `build`/IDL so the guest embed is not refingerprinted
    // and host-target C deps stay on the system compiler.
    apply_host_cc_overrides(&mut cmd);
    run_checked(&mut cmd, "run lez client generator")?;

    Ok(())
}

fn parse_optional_project_path(args: &[String], usage_label: &str) -> DynResult<Option<PathBuf>> {
    let mut project_dir: Option<PathBuf> = None;

    for arg in args {
        if arg.starts_with("--") {
            bail!("unknown flag for `{usage_label}`: {arg}");
        }
        if project_dir.is_none() {
            project_dir = Some(PathBuf::from(arg));
        } else {
            bail!("unexpected argument `{arg}` for `{usage_label}`");
        }
    }

    Ok(project_dir)
}
