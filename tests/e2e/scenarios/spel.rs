//! `.dogfooding/features/spel-template.md`
//!
//! The spel guest builds only in Docker, so SPEL-002 and SPEL-005 are skipped
//! on a host without a running daemon.

use std::fs;

use crate::fixtures::{self, BUILD_TIMEOUT};
use crate::harness::{check, skip, Ctx, R};
use crate::scenarios::{files_under, ls};

fn need_docker(cx: &Ctx) -> R {
    if !(cx.caps.docker && cx.caps.cargo_risczero) {
        skip!("the spel guest builds only in Docker: needs a running daemon and cargo-risczero");
    }
    Ok(())
}

/// SPEL-001 — Create a spel project with pinned versions
pub fn create(cx: &Ctx) -> R {
    let (p, out) = fixtures::spel_created(cx, true)?;
    out.has("Pinned LEZ:")?
        .has("Pinned spel:")?
        .lacks("already exists")?;
    for rel in [
        "spel.toml",
        "Makefile",
        "spel_core",
        "spel_ffi",
        "methods/guest/src/bin/spel.rs",
    ] {
        check!(p.path(rel).exists(), "the spel project has no {rel}");
    }
    let manifest = p.read("methods/guest/Cargo.toml");
    let dep = manifest
        .lines()
        .find(|l| l.contains("spel-framework"))
        .unwrap_or("");
    check!(
        dep.contains("tag =") && !dep.contains("branch"),
        "spel-framework is not pinned to a tag: {dep:?}"
    );
    Ok(())
}

/// SPEL-002 — Set up and build a spel project
pub fn setup_and_build(cx: &Ctx) -> R {
    need_docker(cx)?;
    let (p, _) = fixtures::spel_created(cx, false)?;
    cx.lgs(&p.root, &["setup"])
        .timeout(BUILD_TIMEOUT)
        .run()
        .ok()?;
    cx.localnet_up(&p)?;
    let doctor = cx
        .lgs(&p.root, &["doctor", "--json"])
        .timeout(300)
        .run()
        .json()?;
    check!(
        doctor["summary"]["fail"] == 0,
        "doctor reports FAIL rows in a spel project: {}",
        doctor["summary"]
    );
    cx.lgs(&p.root, &["build"])
        .timeout(BUILD_TIMEOUT)
        .run()
        .ok()?;
    check!(
        ls(&p.root).iter().any(|f| f.ends_with("-idl.json")),
        "`build` left no <name>-idl.json at the project root"
    );
    check!(
        files_under(&p.path("methods/guest/target"), &[])
            .iter()
            .any(|f| f.contains("/docker/") && f.ends_with(".bin")),
        "`build` left no docker guest binary under methods/guest/target"
    );
    // `--guest local` does not apply; the positional path works from outside.
    cx.lgs(
        &cx.work,
        &["build", "--guest", "local", &p.root.to_string_lossy()],
    )
    .timeout(BUILD_TIMEOUT)
    .run()
    .ok()?
    .has("--guest local does not apply to spel projects")?;
    Ok(())
}

/// SPEL-003 — Regenerate the IDL and the client bindings
pub fn idl_and_client(cx: &Ctx) -> R {
    let (p, _) = fixtures::spel_created(cx, false)?;
    // `setup` builds the vendored spel-client-gen.
    cx.once("spel: setup", false, || {
        let out = cx.lgs(&p.root, &["setup"]).timeout(BUILD_TIMEOUT).run();
        out.ok()?;
        Ok(out)
    })?;
    let idl_files = || -> Vec<String> {
        ls(&p.root)
            .into_iter()
            .filter(|f| f.ends_with("-idl.json"))
            .collect()
    };
    let generated = || files_under(&p.path("spel_ffi/generated"), &[]);

    let idl = cx
        .lgs(&p.root, &["build", "idl"])
        .timeout(BUILD_TIMEOUT)
        .run();
    idl.ok()?
        .has("Generating IDL via `make idl` (spel project)...")?
        .has("IDL written to")?;
    check!(
        idl_files().len() == 1,
        "expected one <name>-idl.json at the project root: {:?}",
        idl_files()
    );
    let file = p.path(&idl_files()[0]);
    check!(
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&file).unwrap_or_default())
            .is_ok(),
        "{} is not valid JSON",
        file.display()
    );
    check!(
        !p.path("idl").exists(),
        "`build idl` created an idl/ directory in a spel project"
    );
    check!(
        !idl.stdout.lines().any(|l| l.starts_with('{')),
        "`build idl` printed the IDL to the terminal instead of the file"
    );

    let client = cx
        .lgs(&p.root, &["build", "client"])
        .timeout(BUILD_TIMEOUT)
        .run();
    client.ok()?;
    let text = client.all();
    let (Some(gen_at), Some(make_at)) =
        (text.find("Generating IDL via"), text.find("$ make ffi-gen"))
    else {
        crate::harness::fail!(
            "`build client` did not regenerate the IDL and run `make ffi-gen`:\n{text}"
        );
    };
    check!(
        gen_at < make_at,
        "`build client` ran make ffi-gen before regenerating the IDL"
    );
    let vendored = format!("{}/repos/spel/", cx.cache.display());
    check!(
        text.lines()
            .any(|l| l.contains("spel-client-gen") && l.contains(&vendored)),
        "the generator `make ffi-gen` ran is not the vendored one under {vendored}"
    );
    let expected: Vec<&str> = vec!["spel.h", "spel_client.rs", "spel_ffi.rs"];
    check!(
        expected.iter().all(|f| generated().contains(*f)),
        "generated bindings: {:?}",
        generated()
    );

    // Both take a project path and work from outside.
    let _ = fs::remove_file(&file);
    let _ = fs::remove_dir_all(p.path("spel_ffi/generated"));
    cx.lgs(&cx.work, &["build", "client", &p.root.to_string_lossy()])
        .timeout(BUILD_TIMEOUT)
        .run()
        .ok()?;
    check!(
        idl_files().len() == 1 && expected.iter().all(|f| generated().contains(*f)),
        "`build client <path>` from outside did not recreate the IDL and bindings"
    );
    cx.lgs(&p.root, &["build", "--guest", "docker", "idl"])
        .run()
        .fails()?
        .has("`--guest` applies to `logos-scaffold build`")?;
    Ok(())
}

/// SPEL-005 — Deploy and invoke instructions through spel
pub fn deploy_and_invoke(cx: &Ctx) -> R {
    need_docker(cx)?;
    let (p, _) = fixtures::spel_created(cx, false)?;
    if !files_under(&p.path("methods/guest/target"), &[])
        .iter()
        .any(|f| f.contains("/docker/") && f.ends_with(".bin"))
    {
        crate::harness::blocked!("the spel guest is not built (SPEL-002)");
    }
    cx.localnet_up(&p)?;
    cx.lgs(&p.root, &["wallet", "--", "check-health"])
        .run()
        .ok()?;
    cx.lgs(&p.root, &["deploy"])
        .timeout(1800)
        .run()
        .ok()?
        .has("program_id:")?;
    let id = super::wallet::new_account(cx, &p, "public")?;
    let owner = format!("Public/{id}");
    // Before the account signs anything: an account claimed by a spel program
    // can never send tokens.
    cx.lgs(
        &p.root,
        &[
            "wallet",
            "--",
            "auth-transfer",
            "init",
            "--account-id",
            &owner,
        ],
    )
    .timeout(300)
    .run()
    .ok()?;
    let init = cx
        .lgs(&p.root, &["spel", "--", "initialize", "--owner", &owner])
        .timeout(600)
        .run();
    init.ok()?.lacks("Storage not found")?;
    let state = cx
        .lgs(&p.root, &["spel", "--", "pda", "state"])
        .timeout(300)
        .run();
    state.ok()?;
    let pda = state
        .stdout
        .split_whitespace()
        .rev()
        .find(|w| w.len() >= 32 && w.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_default()
        .to_string();
    check!(
        !pda.is_empty(),
        "`spel -- pda state` printed no address: {}",
        state.stdout
    );
    let inspected = cx.poll(180, 5000, || {
        let o = cx
            .lgs(
                &p.root,
                &["spel", "--", "inspect", &pda, "--type", "ProgramState"],
            )
            .timeout(120)
            .run();
        (o.code == Some(0)).then_some(o)
    });
    check!(
        inspected.is_some(),
        "`spel -- inspect {pda} --type ProgramState` never succeeded"
    );

    // With localnet down the passthroughs forward the tool's own error.
    cx.localnet_down();
    cx.lgs(&p.root, &["localnet", "stop"]).run();
    let health = cx
        .lgs(&p.root, &["wallet", "--", "check-health"])
        .timeout(120)
        .run();
    let invoke = cx
        .lgs(&p.root, &["spel", "--", "initialize", "--owner", &owner])
        .timeout(120)
        .run();
    let restart = cx.localnet_up(&p);
    health
        .fails()?
        .has("wallet passthrough command failed")?
        .lacks("sequencer appears unavailable")?;
    invoke.fails()?.lacks("sequencer appears unavailable")?;
    restart
}
