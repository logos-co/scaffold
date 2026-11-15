//! The basecamp scenarios that need neither Nix nor a basecamp build
//! (`.dogfooding/features/basecamp-setup.md`, `basecamp-launch.md`).

use std::fs;

use crate::fixtures;
use crate::harness::{check, Ctx, R};
use crate::scenarios::files_under;

/// BCSETUP-006 — Without Nix or before setup - help, docs and one-line hints
pub fn without_nix_or_setup(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_created(cx, false)?;
    let help = cx.lgs(&p.root, &["basecamp", "--help"]).run();
    help.ok()?;
    for sub in [
        "setup",
        "modules",
        "install",
        "launch",
        "develop",
        "build",
        "build-portable",
        "run",
        "doctor",
        "paths",
        "docs",
    ] {
        check!(
            help.stdout
                .lines()
                .any(|l| l.trim_start().starts_with(&format!("{sub} "))),
            "`basecamp --help` does not list `{sub}`"
        );
    }
    let docs = cx.lgs(&p.root, &["basecamp", "docs"]).run();
    docs.ok()?;
    let repo_docs = fs::read_to_string(cx.repo.join("docs/basecamp-module-requirements.md"))
        .unwrap_or_default();
    check!(
        docs.stdout.trim_end() == repo_docs.trim_end(),
        "`basecamp docs` differs from docs/basecamp-module-requirements.md"
    );

    // Step 2: a PATH without nix. Drop every directory that has one.
    let path = std::env::var("PATH").unwrap_or_default();
    let no_nix: Vec<&str> = path
        .split(':')
        .filter(|d| !d.is_empty() && !std::path::Path::new(d).join("nix").exists())
        .collect();
    let setup = cx
        .lgs(&p.root, &["basecamp", "setup"])
        .env("PATH", no_nix.join(":"))
        .run();
    setup
        .fails()?
        .has("this command requires Nix (with flakes enabled) on PATH")?
        .lacks("os error 2")?;

    // Step 3: needs the built app, before setup.
    cx.lgs(&p.root, &["basecamp", "launch", "alice"])
        .run()
        .fails()?
        .has("basecamp not set up yet; run: logos-scaffold basecamp setup")?;
    Ok(())
}

/// BCLAUNCH-005 — paths resolves profile paths without side effects
pub fn paths_pure(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_created(cx, false)?;
    let before = files_under(&p.root, &["target"]);
    let v = cx
        .lgs(&p.root, &["basecamp", "paths", "alice", "--json"])
        .run()
        .json()?;
    let tmpdir = v["tmpdir"].as_str().unwrap_or_default();
    check!(
        !tmpdir.is_empty()
            && v["tmpdir"] == v["xdg_runtime_dir"]
            && tmpdir.starts_with("/tmp/lgs-"),
        "runtime dir is not `/tmp/lgs-<hash>-<profile>` under both names: {v}"
    );
    check!(
        tmpdir.ends_with("-alice"),
        "runtime dir does not end in the profile name: {tmpdir}"
    );
    for key in [
        "module_data_dir",
        "app_logs_dir",
        "modules_dir",
        "plugins_dir",
        "home",
        "launch_state",
    ] {
        check!(
            v[key].is_string(),
            "`basecamp paths --json` has no `{key}`: {v}"
        );
    }
    cx.lgs(&p.root, &["basecamp", "paths", "alice"])
        .run()
        .ok()?;
    check!(
        files_under(&p.root, &["target"]) == before,
        "`basecamp paths` created or removed files"
    );
    cx.lgs(&p.root, &["basecamp", "paths", "../escape"])
        .run()
        .fails()?
        .has("invalid profile name")?
        .has("must be a single path component")?;
    Ok(())
}
