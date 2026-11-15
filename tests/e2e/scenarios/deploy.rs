//! `.dogfooding/features/deploy.md`
//!
//! The suite builds guests with the host toolchain, so the artefacts `deploy`
//! ships are under `target/riscv-guest/…/release/`.

use std::fs;

use crate::fixtures;
use crate::harness::{check, Ctx, MovedAway, BLACKHOLE, R};
use crate::scenarios::build::{guest_bins, programs};
use crate::scenarios::localnet::PORT;
use crate::scenarios::wallet::set_sequencer_addr;

/// DEPLOY-001 — Deploy all discovered programs
pub fn deploy_all(cx: &Ctx) -> R {
    let (p, out) = fixtures::main_deployed(cx, true)?;
    let programs = programs(&p);
    check!(programs.len() > 1, "the template has only {programs:?}");
    for program in &programs {
        out.has(&format!("{program} submitted"))?;
    }
    let ids = out.lines_with("program_id:").len();
    check!(
        ids >= programs.len(),
        "{ids} program_id lines for {} programs",
        programs.len()
    );
    out.has("Waiting for a new block past")?
        .has(&format!("Succeeded: {}", programs.len()))?
        .has("Failed: 0")?;
    // The last build was a local one (BUILD-005): its release artefacts ship.
    let echoed = out.lines_with("deploy-program");
    check!(
        echoed.len() >= programs.len()
            && echoed
                .iter()
                .all(|l| l.contains("/target/riscv-guest/") && l.contains("/release/")),
        "deploy did not ship the local build's release artefacts: {echoed:?}"
    );
    check!(
        cx.localnet_status(&p)?["ready"] == true,
        "localnet is not ready after the deploy"
    );
    Ok(())
}

/// DEPLOY-002 — Deploy one program by name or path, with JSON output
pub fn deploy_one(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_deployed(cx, false)?;
    let by_name = cx
        .lgs(&p.root, &["deploy", "hello_world"])
        .timeout(600)
        .run();
    by_name
        .ok()?
        .has("hello_world submitted")?
        .has("program_id:")?
        .has("Succeeded: 1")?
        .has("Failed: 0")?;
    check!(
        by_name
            .lines_with(" submitted")
            .iter()
            .filter(|l| l.contains("OK"))
            .count()
            == 1,
        "`deploy hello_world` submitted more than one program:\n{}",
        by_name.stdout
    );

    let entry_ok = |e: &serde_json::Value| {
        e["status"] == "submitted"
            && e["program"] == "hello_world"
            && e["program_id"].as_str().is_some_and(|id| id.len() == 64)
            && e.get("tx").is_none()
    };
    let v = cx
        .lgs(&p.root, &["deploy", "hello_world", "--json"])
        .timeout(600)
        .run()
        .json()?;
    check!(
        v["deploys"]
            .as_array()
            .is_some_and(|a| a.len() == 1 && entry_ok(&a[0])),
        "`deploy hello_world --json`: {v}"
    );

    let bins = guest_bins(&p.path("target"), "hello_world.bin");
    let Some(bin) = bins.iter().find(|b| b.ends_with("hello_world.bin")) else {
        crate::harness::blocked!("no hello_world.bin under target/");
    };
    let bin = bin.to_string_lossy().into_owned();
    cx.lgs(&p.root, &["deploy", "--program-path", &bin])
        .timeout(600)
        .run()
        .ok()?
        .has("OK")?
        .has(&bin)?;
    let flat = cx
        .lgs(&p.root, &["deploy", "--program-path", &bin, "--json"])
        .timeout(600)
        .run()
        .json()?;
    check!(
        entry_ok(&flat),
        "`deploy --program-path --json` is not one flat entry: {flat}"
    );
    Ok(())
}

/// DEPLOY-005 — Unknown program or missing binaries give an actionable error
pub fn unknown_or_missing(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_deployed(cx, false)?;
    cx.lgs(&p.root, &["deploy", "nonexistent_program"])
        .run()
        .fails()?
        .has("unknown program")?
        .has("hello_world")?;

    // Move the artefacts away (and back): nothing has to be rebuilt.
    let _docker = MovedAway::new(&p.path("target/riscv-guest-docker"));
    let Some(_local) = MovedAway::new(&p.path("target/riscv-guest")) else {
        crate::harness::blocked!("no target/riscv-guest to move aside");
    };
    let out = cx.lgs(&p.root, &["deploy"]).timeout(120).run();
    out.fails()?
        .has("missing binary `hello_world.bin` (searched: ")?
        .has("logos-scaffold build")?
        .lacks("Submission confirmed")?;
    // The four search roots, in ranking order.
    let line = out
        .lines_with("missing binary `hello_world.bin`")
        .first()
        .cloned()
        .unwrap_or_default();
    let root = p.root.to_string_lossy().into_owned();
    let expected = [
        "target/riscv-guest-docker",
        "target/riscv-guest",
        "methods/target",
        "methods/guest/target",
    ]
    .map(|r| format!("{root}/{r}"))
    .join(", ");
    check!(
        line.contains(&format!("(searched: {expected})")),
        "the error does not list the search roots in order ({expected}): {line}"
    );
    Ok(())
}

/// DEPLOY-006 — Unreachable sequencer fails fast with a hint
pub fn unreachable_sequencer(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_deployed(cx, false)?;
    // Step 1: nothing listening.
    cx.localnet_down();
    cx.lgs(&p.root, &["localnet", "stop"]).run();
    let refused = cx.lgs(&p.root, &["deploy"]).timeout(60).run();
    refused
        .fails()?
        .within(15.0)?
        .has("cannot deploy programs:")?
        .has(&format!(
            "sequencer appears unavailable at http://127.0.0.1:{PORT}"
        ))?
        .has("logos-scaffold localnet start")?
        .lacks("reachability probe failed")?;

    // Steps 2-3: an address that never answers.
    if cx.caps.blackhole {
        let addr = format!("http://{BLACKHOLE}:{PORT}");
        let _guard = set_sequencer_addr(&p, &addr)?;
        let hung = cx.lgs(&p.root, &["deploy"]).timeout(120).run();
        hung.fails()?
            .within(30.0)?
            .has(&format!("sequencer appears unavailable at {addr}"))?
            .lacks("reachability probe failed")?;
    } else {
        cx.note(format!(
            "step 3 not run: {BLACKHOLE} is not a black hole on this host"
        ));
    }
    cx.localnet_up(&p)
}

/// DEPLOY-007 — Deploy ranks only the owned docker artefact first
pub fn artefact_ranking(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_deployed(cx, false)?;
    let release = guest_bins(&p.path("target/riscv-guest"), "/release/");
    let find = |name: &str| release.iter().find(|b| b.ends_with(name)).cloned();
    let (Some(hello), Some(other)) = (find("hello_world.bin"), find("simple_tail_call.bin")) else {
        crate::harness::blocked!("the local build's release artefacts are missing");
    };

    // A docker artefact in a root this project kind does not own is ignored.
    let planted_dir = p.path("methods/guest/target/riscv32im-risc0-zkvm-elf/docker");
    fs::create_dir_all(&planted_dir).expect("mkdir");
    fs::copy(&other, planted_dir.join("hello_world.bin")).expect("plant");
    let out = cx
        .lgs(&p.root, &["deploy", "hello_world"])
        .timeout(600)
        .run();
    let _ = fs::remove_dir_all(p.path("methods/guest/target"));
    out.ok()?.has("note: ignoring")?;
    let echoed = out.lines_with("deploy-program").join("\n");
    check!(
        echoed.contains(&*hello.to_string_lossy()) && !echoed.contains("methods/guest/target"),
        "deploy did not ship the project's own artefact: {echoed}"
    );
    check!(
        out.lines_with("note: ignoring")
            .iter()
            .any(|l| l.contains("methods/guest/target") && l.contains("hello_world.bin")),
        "the `note: ignoring` line does not name the planted file"
    );

    // A docker artefact in the owned root outranks the release one.
    let owned_dir = p.path("target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker");
    fs::create_dir_all(&owned_dir).expect("mkdir");
    fs::copy(&hello, owned_dir.join("hello_world.bin")).expect("copy");
    let out = cx
        .lgs(&p.root, &["deploy", "hello_world"])
        .timeout(600)
        .run();
    let _ = fs::remove_dir_all(p.path("target/riscv-guest-docker"));
    out.ok()?;
    let echoed = out.lines_with("deploy-program").join("\n");
    check!(
        echoed.contains("target/riscv-guest-docker") && echoed.contains("/docker/hello_world.bin"),
        "an owned docker artefact did not outrank the release one: {echoed}"
    );
    Ok(())
}
