//! `.dogfooding/features/wallet.md`

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::Value;

use crate::fixtures;
use crate::harness::{check, Ctx, MovedAway, Project, Restore, R};
use crate::scenarios::tnode::tn;

/// The wallet binary the project uses.
pub fn wallet_bin(cx: &Ctx, p: &Project) -> R<PathBuf> {
    let root = p.root.to_string_lossy().into_owned();
    let pins = tn(cx, &["pins", "--project", &root, "--json"])
        .run()
        .json()?;
    Ok(
        PathBuf::from(pins["lez_checkout"].as_str().unwrap_or_default())
            .join("target/release/wallet"),
    )
}

/// Create an account and return its id without the `Public/` or `Private/` prefix.
pub fn new_account(cx: &Ctx, p: &Project, kind: &str) -> R<String> {
    let out = cx
        .lgs(&p.root, &["-q", "wallet", "--", "account", "new", kind])
        .timeout(120)
        .run();
    out.ok()?;
    let id = out
        .stdout
        .split("account_id ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|full| full.split('/').nth(1))
        .unwrap_or_default()
        .to_string();
    check!(
        !id.is_empty(),
        "could not read the new account id from: {}",
        out.stdout
    );
    Ok(id)
}

/// Point the wallet at another sequencer address until the guard drops.
pub fn set_sequencer_addr(p: &Project, addr: &str) -> R<Restore> {
    let path = p.wallet_config();
    let guard = Restore::of(&path);
    let mut v: Value = match serde_json::from_str(&guard.original_text()) {
        Ok(v) => v,
        Err(e) => crate::harness::blocked!("cannot parse {}: {e}", path.display()),
    };
    if v["sequencers"][0]["sequencer_addr"].is_string() {
        v["sequencers"][0]["sequencer_addr"] = Value::String(addr.into());
    } else {
        v["sequencer_addr"] = Value::String(addr.into());
    }
    fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default())
        .expect("write wallet config");
    Ok(guard)
}

/// WALLET-001 — Top up the default wallet without an address
pub fn topup_default(cx: &Ctx) -> R {
    let (p, out) = fixtures::main_topped_up(cx, true)?;
    out.lacks("no destination configured")?
        .has("pinata claim")?;
    let Some(addr) = p.default_address() else {
        crate::harness::fail!("wallet.state has no default_address after topup");
    };
    let list = cx
        .lgs(&p.root, &["-q", "wallet", "--", "account", "list"])
        .run();
    list.ok()?;
    check!(
        list.stdout
            .lines()
            .any(|l| l.starts_with("/ ") && l.contains(&addr)),
        "the default address {addr} is not a stored (`/ `) account:\n{}",
        list.stdout
    );
    Ok(())
}

/// WALLET-002 — List accounts and pass raw commands through to the project wallet
pub fn list_and_passthrough(cx: &Ctx) -> R {
    let p = fixtures::main_localnet(cx)?;
    let addr = p.default_address().unwrap_or_default();
    cx.lgs(&p.root, &["wallet", "list"])
        .run()
        .ok()?
        .has(&format!("Public/{addr}"))?;
    cx.lgs(&p.root, &["wallet", "list", "--long"])
        .run()
        .ok()?
        .has(&format!("Public/{addr}"))?;
    let v = cx
        .lgs(&p.root, &["wallet", "list", "--json"])
        .run()
        .json()?;
    let accounts: Vec<String> = v["accounts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    check!(
        accounts.iter().any(|a| a.contains(&addr))
            && accounts
                .iter()
                .all(|a| a.starts_with('/') && (a.contains("Public/") || a.contains("Private/"))),
        "`wallet list --json` accounts are not exactly the account lines: {accounts:?}"
    );

    // Passthrough runs the project wallet on the project wallet home.
    cx.lgs(&p.root, &["wallet", "--", "check-health"])
        .run()
        .ok()?;
    cx.lgs(&p.root, &["-q", "wallet", "--", "account", "list"])
        .run()
        .ok()?
        .has(&addr)?;
    check!(
        p.path(".scaffold/wallet/storage.json").is_file(),
        ".scaffold/wallet holds no account storage: the wallet used another home"
    );
    cx.lgs(&p.root, &["wallet", "account", "list"])
        .run()
        .fails()?
        .has("unrecognized subcommand")?;

    // No wallet binary: a scaffold error naming it, with PATH irrelevant.
    let bin = wallet_bin(cx, &p)?;
    let Some(_moved) = MovedAway::new(&bin) else {
        crate::harness::blocked!("cannot move {} aside", bin.display());
    };
    cx.lgs(&p.root, &["wallet", "list"])
        .run()
        .fails()?
        .has(&format!("missing wallet binary at {}", bin.display()))?
        .has("logos-scaffold setup")?;
    Ok(())
}

/// WALLET-003 — Set the project default address and reject bad addresses
pub fn default_address(cx: &Ctx) -> R {
    let p = fixtures::main_localnet(cx)?;
    let state = p.path(".scaffold/state/wallet.state");
    let _restore = Restore::of(&state);
    let original = p.default_address().unwrap_or_default();
    let fresh = new_account(cx, &p, "public")?;
    cx.lgs(
        &p.root,
        &["wallet", "default", "set", &format!("Public/{fresh}")],
    )
    .run()
    .ok()?;
    check!(
        p.default_address().as_deref() == Some(&fresh),
        "positional `default set` did not persist"
    );
    cx.lgs(
        &p.root,
        &[
            "wallet",
            "default",
            "set",
            "--address",
            &format!("Public/{original}"),
        ],
    )
    .run()
    .ok()?;
    check!(
        p.default_address().as_deref() == Some(&original),
        "`default set --address` did not persist"
    );
    cx.lgs(&p.root, &["wallet", "default", "set", &fresh])
        .run()
        .ok()?;
    check!(
        p.read(".scaffold/state/wallet.state")
            .contains(&format!("default_address=Public/{fresh}")),
        "a bare base58 id was not normalized to Public/<id>"
    );
    cx.lgs(&p.root, &["wallet", "default", "set", "not-an-address"])
        .run()
        .fails()?
        .has("Accepted formats")?;
    cx.lgs(
        &p.root,
        &["wallet", "topup", "Public/abc", "--address", "Public/def"],
    )
    .run()
    .fails()?
    .has("not both")?;
    Ok(())
}

/// WALLET-004 — Topup dry run, and topup follows the wallet's configured sequencer
pub fn dry_run_and_target(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_topped_up(cx, false)?;
    let dry = cx.lgs(&p.root, &["wallet", "topup", "--dry-run"]).run();
    dry.ok()?.has("pinata claim")?.has(&format!(
        "planned network: local sequencer (http://127.0.0.1:{})",
        super::localnet::PORT
    ))?;
    let v = cx
        .lgs(&p.root, &["wallet", "topup", "--dry-run", "--json"])
        .run()
        .json()?;
    check!(
        v["status"] == "dry_run" && v["address"].is_string() && v["network"].is_string(),
        "`topup --dry-run --json`: {v}"
    );

    // A closed port: bind one to learn a free number, then release it.
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(3999);
    let addr = format!("http://127.0.0.1:{closed}");
    let _guard = set_sequencer_addr(&p, &addr)?;
    cx.lgs(&p.root, &["wallet", "topup", "--dry-run"])
        .run()
        .ok()?
        .has(&format!("({addr})"))?;
    cx.lgs(&p.root, &["wallet", "topup"])
        .timeout(120)
        .run()
        .fails()?
        .has(&format!("sequencer appears unavailable at {addr}"))?;
    Ok(())
}

/// WALLET-007 — Wallet password override
pub fn password_override(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_topped_up(cx, false)?;
    let bin = wallet_bin(cx, &p)?;
    let record = cx.out.join("logs/WALLET-007-stdin.log");
    let _ = fs::remove_file(&record);
    let Some(moved) = MovedAway::new(&bin) else {
        crate::harness::blocked!("cannot move {} aside", bin.display());
    };
    // A wrapper that records what the wallet is sent on stdin, then runs it.
    fs::write(
        &bin,
        format!(
            "#!/bin/bash\nIN=$(cat)\necho \"args=$* stdin=[$IN]\" >> '{}'\necho \"$IN\" | exec '{}' \"$@\"\n",
            record.display(),
            moved.moved_to().display()
        ),
    )
    .expect("write wallet wrapper");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod wrapper");

    cx.lgs(&p.root, &["doctor"])
        .env("LOGOS_SCAFFOLD_WALLET_PASSWORD", "custom-pw")
        .timeout(300)
        .run();
    let custom = fs::read_to_string(&record).unwrap_or_default();
    let _ = fs::remove_file(&record);
    cx.lgs(&p.root, &["doctor"]).timeout(300).run();
    let default = fs::read_to_string(&record).unwrap_or_default();
    drop(moved);

    check!(
        custom.contains("args=check-health stdin=[custom-pw]"),
        "doctor's wallet health check did not send LOGOS_SCAFFOLD_WALLET_PASSWORD:\n{custom}"
    );
    let default_line = default
        .lines()
        .find(|l| l.starts_with("args=check-health"))
        .unwrap_or("");
    check!(
        !default_line.is_empty()
            && !default_line.contains("custom-pw")
            && !default_line.ends_with("stdin=[]"),
        "without the variable the wallet should get the built-in password: {default_line:?}"
    );
    Ok(())
}
