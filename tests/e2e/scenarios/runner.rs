//! `.dogfooding/features/example-runners.md`

use crate::fixtures::{self, BUILD_TIMEOUT};
use crate::harness::{check, Ctx, Out, Project, R};
use crate::scenarios::wallet::new_account;

/// `cargo run --bin <bin> -- <args…>` in the project, with the project wallet
/// home under both names (a v0.2.x wallet given only one silently uses
/// `~/.lee/wallet`).
fn runner(cx: &Ctx, p: &Project, bin: &str, args: &[&str]) -> Out {
    let wallet = p.path(".scaffold/wallet");
    let mut argv = vec!["run", "--quiet", "--bin", bin, "--"];
    argv.extend_from_slice(args);
    cx.cmd(&p.root, "cargo", &argv)
        .env("NSSA_WALLET_HOME_DIR", wallet.clone())
        .env("LEE_WALLET_HOME_DIR", wallet)
        .env("RISC0_DEV_MODE", "1")
        .timeout(BUILD_TIMEOUT)
        .run()
}

fn account(cx: &Ctx, p: &Project, full_id: &str) -> Out {
    cx.lgs(
        &p.root,
        &[
            "-q",
            "wallet",
            "--",
            "account",
            "get",
            "--account-id",
            full_id,
        ],
    )
    .run()
}

/// The account's `data` decoded as text, once it has any.
fn data(out: &Out) -> Option<String> {
    let hex = out.stdout.split("\"data\":\"").nth(1)?.split('"').next()?;
    if hex.is_empty() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .and_then(|b| u8::from_str_radix(b, 16).ok())
        })
        .collect();
    Some(String::from_utf8_lossy(&bytes?).into_owned())
}

/// Poll until the account's data is `expected`. Reads lag block production.
fn expect_data(cx: &Ctx, p: &Project, full_id: &str, expected: &str, sync_private: bool) -> R<Out> {
    let mut last = Out::default();
    let seen = cx.poll(180, 5000, || {
        if sync_private {
            cx.lgs(&p.root, &["-q", "wallet", "--", "account", "sync-private"])
                .timeout(120)
                .run();
        }
        last = account(cx, p, full_id);
        (data(&last).as_deref() == Some(expected)).then_some(())
    });
    check!(
        seen.is_some(),
        "{full_id} never held {expected:?} (a runner can exit 0 while execution is rejected; see the sequencer log). Last read:\n{}",
        last.stdout
    );
    Ok(last)
}

fn owner_set(out: &Out) -> bool {
    out.stdout
        .split("\"program_owner\":")
        .nth(1)
        .is_some_and(|rest| !rest.trim_start().starts_with("null"))
}

/// RUNNER-001 — Public runners write their data on chain
pub fn public_runners(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_deployed(cx, false)?;
    let a = new_account(cx, &p, "public")?;
    let b = new_account(cx, &p, "public")?;
    let hello = runner(cx, &p, "run_hello_world", &[&a]);
    hello
        .ok()?
        .has("submitted transaction: tx_hash=")?
        .has("verification hint:")?
        .has("account get")?;
    let moved = runner(
        cx,
        &p,
        "run_hello_world_with_move_function",
        &["write-public", &b, "dogfood-test-message"],
    );
    moved
        .ok()?
        .has("submitted transaction: tx_hash=")?
        .has("verification hint:")?;
    let read = expect_data(cx, &p, &format!("Public/{a}"), "Hola mundo!", false)?;
    check!(
        owner_set(&read),
        "account A has no program_owner after the write"
    );
    expect_data(
        cx,
        &p,
        &format!("Public/{b}"),
        "dogfood-test-message",
        false,
    )?;
    Ok(())
}

/// RUNNER-003 — Bad runner input fails with a clear error
pub fn bad_input(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_deployed(cx, false)?;
    let bad = runner(cx, &p, "run_hello_world", &["not-base58!"]);
    bad.fails()?
        .has("invalid account_id")?
        .lacks("panicked at")?
        .lacks("unwrap()")?;
    let a = new_account(cx, &p, "public")?;
    cx.localnet_down();
    cx.lgs(&p.root, &["localnet", "stop"]).run();
    let down = runner(cx, &p, "run_hello_world", &[&a]);
    let restart = cx.localnet_up(&p);
    down.fails()?.lacks("panicked at")?;
    check!(
        down.all().contains("Connection refused") || down.all().to_lowercase().contains("connect"),
        "with localnet down the runner should report a connection error: {}",
        down.all()
    );
    restart
}

/// RUNNER-004 — A private runner's write is visible after sync-private
pub fn private_runner(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_deployed(cx, false)?;
    // The host risc0-zkvm must be the provisioned r0vm's version: the
    // template pins it exactly, and the lockfile must follow.
    let pinned = p
        .read("Cargo.toml")
        .lines()
        .find(|l| l.trim_start().starts_with("risc0-zkvm"))
        .and_then(|l| l.split("\"=").nth(1))
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_default()
        .to_string();
    let lock = p.read("Cargo.lock");
    let locked = lock
        .split("name = \"risc0-zkvm\"\n")
        .nth(1)
        .and_then(|rest| rest.lines().next())
        .unwrap_or_default()
        .to_string();
    check!(
        !pinned.is_empty() && locked == format!("version = \"{pinned}\""),
        "Cargo.lock resolves risc0-zkvm to {locked:?} but the template pins ={pinned}"
    );
    let id = new_account(cx, &p, "private")?;
    let out = runner(cx, &p, "run_hello_world_private", &[&id]);
    out.ok()?
        .has("tx_hash=")?
        .has("account sync-private")?
        .has(&format!("Private/{id}"))?;
    let read = expect_data(cx, &p, &format!("Private/{id}"), "Hola mundo!", true)?;
    check!(
        owner_set(&read),
        "the private account has no program_owner after the write"
    );
    Ok(())
}

/// RUNNER-005 — Tail-call runner lands its greeting with build-synced callee ids
pub fn tail_call(cx: &Ctx) -> R {
    let (p, first_build) = fixtures::main_built(cx, false)?;
    fixtures::main_deployed(cx, false)?;
    // The callee ids embedded in the tail-call programs match the build: an
    // unchanged rebuild rewrites nothing.
    let rebuild = cx.lgs(&p.root, &["build"]).timeout(BUILD_TIMEOUT).run();
    rebuild.ok()?.lacks("program id")?;
    for (file, constant) in [
        ("simple_tail_call.rs", "HELLO_WORLD_PROGRAM_ID_HEX"),
        (
            "tail_call_with_pda.rs",
            "HELLO_WORLD_WITH_AUTHORIZATION_PROGRAM_ID_HEX",
        ),
    ] {
        check!(
            p.read(&format!("methods/guest/src/bin/{file}"))
                .contains(constant),
            "{file} no longer embeds {constant}"
        );
    }
    if !first_build.all().contains("Updated ") {
        cx.note("the first build did not need to rewrite the callee ids (already in sync)");
    }
    let log = p.path(".scaffold/logs/sequencer.log");
    let rejections = |text: &str| {
        text.matches("Unknown program").count() + text.matches("ClaimedUnauthorizedAccount").count()
    };
    let before = rejections(&std::fs::read_to_string(&log).unwrap_or_default());
    let id = new_account(cx, &p, "public")?;
    runner(cx, &p, "run_hello_world_through_tail_call", &[&id])
        .ok()?
        .has("tx_hash=")?;
    expect_data(
        cx,
        &p,
        &format!("Public/{id}"),
        "Hello from tail call",
        false,
    )?;
    let after = rejections(&std::fs::read_to_string(&log).unwrap_or_default());
    check!(
        after == before,
        "the sequencer logged {} `Unknown program`/`ClaimedUnauthorizedAccount` rejection(s) for the tail call",
        after - before
    );
    Ok(())
}
