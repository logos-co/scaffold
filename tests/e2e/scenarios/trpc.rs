//! `.dogfooding/features/test-node-rpc.md`

use std::fs;

use serde_json::Value;

use crate::fixtures;
use crate::harness::{check, port_free, Ctx, Out, R};
use crate::scenarios::localnet::PORT;
use crate::scenarios::tnode::{tn, Node};

/// Debug-config sample accounts and their vault PDAs at the default LEZ pin.
const SEEDED: &str = "CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r";
const SEEDED_VAULT: &str = "G1u9gmu1Gb8PG59zHZYNL5WdJNNtBKEN4U5qUdx2y6sG";
const LEFT_OUT_VAULT: &str = "BAX2jjbhmT3CgDBi9MCFMgFsCeKECtaiyj1XDwXfZGrR";
/// An id nobody wrote to.
const NOBODY: &str = "11111111111111111111111111111111";

/// Short blocks, so waiting for a few of them takes seconds.
const FAST: &[&str] = &["--block-create-timeout-ms", "2000"];

fn blocks(v: &Value) -> Vec<Value> {
    v["blocks"].as_array().cloned().unwrap_or_default()
}

/// TRPC-001 — Read and classify blocks and clock state
pub fn blocks_and_clock(cx: &Ctx) -> R {
    let (p, _) = fixtures::tn_prepared(cx, false)?;
    let node = Node::start(cx, &p, FAST)?;
    let waited = node
        .rpc(&["blocks", "wait", "--after", "1", "--count", "2", "--json"])
        .timeout(120)
        .run()
        .json()?;
    let ids: Vec<u64> = blocks(&waited)
        .iter()
        .filter_map(|b| b["block_id"].as_u64())
        .collect();
    check!(
        ids == [2, 3],
        "`blocks wait --after 1 --count 2` returned blocks {ids:?}"
    );

    let range = node
        .rpc(&["blocks", "range", "--from", "1", "--to", "3", "--json"])
        .run()
        .json()?;
    let bs = blocks(&range);
    check!(
        bs.len() == 3,
        "`blocks range --from 1 --to 3` returned {} blocks",
        bs.len()
    );
    let genesis = &bs[0];
    check!(
        genesis["block_id"] == 1
            && genesis["is_genesis"] == true
            && genesis["has_clock_transaction"] == false
            && genesis["has_user_transactions"] == false,
        "block 1 is not classified as genesis: {genesis}"
    );
    for b in &bs[1..] {
        let txs = b["transactions"].as_array().cloned().unwrap_or_default();
        check!(
            b["is_genesis"] == false
                && b["transaction_count"] == 1
                && b["has_clock_transaction"] == true
                && b["has_user_transactions"] == false
                && b["fully_parsed"] == true
                && b["timestamp"].as_u64().is_some_and(|t| t > 0)
                && txs.len() == 1
                && txs[0]["is_clock"] == true
                && txs[0]["kind"] == "public",
            "an idle post-genesis block is not clock-only: {b}"
        );
    }

    let head = node.rpc(&["blocks", "head", "--json"]).run().json()?;
    let clock = node.rpc(&["clock", "read", "--json"]).run().json()?;
    let accounts = clock["accounts"].as_array().cloned().unwrap_or_default();
    check!(
        accounts.len() == 3,
        "`clock read` decoded {} accounts: {clock}",
        accounts.len()
    );
    let first = accounts[0]["block_id"].as_u64().unwrap_or(0);
    let head_id = head["block_id"].as_u64().unwrap_or(u64::MAX);
    check!(
        first + 1 >= head_id && first <= head_id + 1,
        "the first clock account ({first}) does not track the head ({head_id})"
    );
    check!(
        accounts[1..]
            .iter()
            .all(|a| a["block_id"].as_u64().is_some_and(|b| b < first)),
        "the slower clock accounts do not lag the first: {clock}"
    );
    node.rpc(&["clock", "wait-stable", "--samples", "2", "--json"])
        .timeout(120)
        .run()
        .ok()?
        .json()?;
    Ok(())
}

/// TRPC-002 — Transactions end in one definitive outcome
pub fn tx_outcomes(cx: &Ctx) -> R {
    let (p, _) = fixtures::tn_prepared(cx, false)?;
    let node = Node::start(cx, &p, FAST)?;
    let dir = cx.empty_dir("trpc-002");
    let b64 = dir.join("bad.b64");
    let bin = dir.join("bad.bin");
    let junk = dir.join("notb64.txt");
    fs::write(&b64, "AAAA\n").expect("write");
    fs::write(&bin, [0u8, 0, 0]).expect("write");
    fs::write(&junk, "!!!\n").expect("write");
    let s = |p: &std::path::Path| p.to_string_lossy().into_owned();

    let terminal = |out: &Out| -> R<Value> {
        out.fails()?;
        let v = out.json()?;
        check!(v["status"].is_string(), "not one terminal object: {v}");
        Ok(v)
    };
    let text = terminal(
        &node
            .rpc(&["tx", "submit-and-wait", "--file", &s(&b64), "--json"])
            .timeout(180)
            .run(),
    )?;
    check!(
        (text["status"] == "rejected" && text["phase"] == "stateless")
            || text["status"] == "wire_mismatch",
        "garbage bytes should be rejected statelessly: {text}"
    );
    let raw = terminal(
        &node
            .rpc(&[
                "tx",
                "submit-and-wait",
                "--file",
                &s(&bin),
                "--encoding",
                "borsh",
                "--json",
            ])
            .timeout(180)
            .run(),
    )?;
    check!(
        raw == text,
        "the same bytes gave {raw} as raw borsh but {text} as base64"
    );

    // A transport failure is never reported as a rejection.
    let dead = tn(
        cx,
        &[
            "tx",
            "submit-and-wait",
            "--url",
            "http://127.0.0.1:1",
            "--file",
            &s(&b64),
            "--json",
        ],
    )
    .timeout(180)
    .run();
    let v = terminal(&dead)?;
    check!(
        v["status"] == "transport_error",
        "an unreachable node gave {v}"
    );

    let submit = terminal(
        &node
            .rpc(&[
                "tx",
                "submit",
                "--file",
                &s(&bin),
                "--encoding",
                "borsh",
                "--json",
            ])
            .run(),
    )?;
    check!(
        submit["status"] == "rejected",
        "`tx submit` of garbage: {submit}"
    );

    let not_b64 = node
        .rpc(&["tx", "submit", "--file", &s(&junk), "--json"])
        .run();
    not_b64
        .fails()?
        .has("invalid base64 transaction encoding")?;
    check!(
        not_b64.stdout.trim().is_empty(),
        "a local decode error still printed JSON: {}",
        not_b64.stdout
    );
    Ok(())
}

/// TRPC-003 — Block-scoped account and proof reads, and account snapshots
pub fn account_and_proof_reads(cx: &Ctx) -> R {
    let (p, _) = fixtures::tn_prepared(cx, false)?;
    let node = Node::start(cx, &p, FAST)?;
    // The first clock account always exists.
    let clock = node.rpc(&["clock", "read", "--json"]).run().json()?;
    let Some(id) = clock["accounts"][0]["account_id"]
        .as_str()
        .map(str::to_string)
    else {
        crate::harness::fail!("`clock read` names no account: {clock}");
    };
    let one = node
        .rpc(&["account", "get", "--account-id", &id, "--json"])
        .run()
        .json()?;
    check!(
        one["state"] == "present"
            && one["encoded"].as_str().is_some_and(|e| !e.is_empty())
            && one["program_owner"].is_string()
            && one["balance"].is_u64()
            && one["nonce"].is_u64()
            && one["data"].is_string()
            && one["block_id"].is_u64(),
        "`account get` of an existing account: {one}"
    );
    let batch = node
        .rpc(&[
            "account",
            "batch-get",
            "--account-id",
            &id,
            "--account-id",
            NOBODY,
            "--json",
        ])
        .run()
        .json()?;
    let entries = batch["accounts"].as_array().cloned().unwrap_or_default();
    check!(
        batch["block_id"].is_u64()
            && entries.len() == 2
            && entries[0]["state"] == "present"
            && entries[1]["state"] == "missing",
        "`account batch-get` at one block boundary: {batch}"
    );
    let pinned = node
        .rpc(&[
            "account",
            "get",
            "--account-id",
            &id,
            "--at-block",
            "1",
            "--json",
        ])
        .run()
        .json()?;
    check!(
        pinned["block_id"] == 1,
        "`--at-block 1` read at {}",
        pinned["block_id"]
    );

    let invalid = node
        .rpc(&["proof", "get", "--commitment", "00", "--json"])
        .run();
    invalid
        .fails()?
        .has("expected 64 hex characters or base58 of 32 bytes")?;
    let zeros = "0".repeat(64);
    let missing = node
        .rpc(&["proof", "get", "--commitment", &zeros, "--json"])
        .run()
        .json()?;
    check!(
        missing["proof"].is_null() && missing["root"].is_string(),
        "a commitment nobody made should give `proof: null` with a root: {missing}"
    );

    let snap = cx.work.join("trpc-003-accounts.json");
    let _ = fs::remove_file(&snap);
    let written = node
        .rpc(&[
            "snapshot",
            "accounts",
            "--account-id",
            &id,
            "--output",
            &snap.to_string_lossy(),
            "--json",
        ])
        .run()
        .json()?;
    check!(
        written["account_count"] == 1 && written["block_id"].is_u64(),
        "`snapshot accounts` reported {written}"
    );
    let file: Value =
        serde_json::from_str(&fs::read_to_string(&snap).unwrap_or_default()).unwrap_or_default();
    check!(
        file["format"] == "lgs-account-snapshot/1",
        "snapshot file format: {}",
        file["format"]
    );
    Ok(())
}

fn account_state(node: &Node, id: &str) -> R<Value> {
    node.rpc(&["account", "get", "--account-id", id, "--json"])
        .run()
        .json()
}

/// TRPC-005 — Start a node from exactly a seeded state
pub fn seeded_state(cx: &Ctx) -> R {
    let (p, _) = fixtures::tn_prepared(cx, false)?;
    let root = p.root.to_string_lossy().into_owned();
    // The control: on an unseeded node the left-out sample account exists.
    {
        let plain = Node::start(cx, &p, &[])?;
        let v = account_state(&plain, LEFT_OUT_VAULT)?;
        check!(
            v["state"] == "present" && v["balance"].as_u64().is_some_and(|b| b > 0),
            "the sample vault {LEFT_OUT_VAULT} is not funded on an unseeded node: {v}"
        );
    }
    let schema = tn(cx, &["state", "schema", "--project", &root, "--json"])
        .run()
        .json()?;
    let inputs = schema["accepted_inputs"].to_string();
    check!(
        schema["state_format_version"] == "nssa-v03"
            && inputs.contains("lgs-state-snapshot/1")
            && inputs.contains("lgs-account-snapshot/1")
            && inputs.contains("rocksdb"),
        "`state schema`: {schema}"
    );

    let dir = cx.empty_dir("trpc-005");
    let input = dir.join("seed.json");
    let seeded_dir = dir.join("seeded");
    fs::write(
        &input,
        format!(
            "{{\"format\":\"lgs-state-snapshot/1\",\"public_accounts\":[{{\"account_id\":\"{SEEDED}\",\"balance\":4242}}],\"private_accounts\":[]}}"
        ),
    )
    .expect("write seed");
    let seed = tn(
        cx,
        &[
            "state",
            "seed",
            "--project",
            &root,
            "--input",
            &input.to_string_lossy(),
            "--output",
            &seeded_dir.to_string_lossy(),
            "--json",
        ],
    )
    .run();
    seed.ok()?;
    let sj = seed.json()?;
    check!(
        sj["public_account_count"] == 1
            && sj["state_format_version"] == "nssa-v03"
            && sj["lez_commit"].is_string()
            && sj["warnings"].to_string().contains("vault PDA"),
        "`state seed` reported {sj}"
    );

    let node = Node::start(cx, &p, &["--state", &seeded_dir.to_string_lossy()])?;
    let vault = account_state(&node, SEEDED_VAULT)?;
    check!(
        vault["state"] == "present" && vault["balance"] == 4242,
        "the seeded balance is not on the account's vault PDA: {vault}"
    );
    check!(
        account_state(&node, SEEDED)?["state"] == "missing",
        "on LEZ v0.2.4 the seeded account itself should read missing"
    );
    check!(
        account_state(&node, LEFT_OUT_VAULT)?["state"] == "missing",
        "a sample account the seed left out exists: the seed did not replace the debug state"
    );
    let export = dir.join("export.json");
    node.rpc(&[
        "state",
        "export",
        "--account-id",
        SEEDED_VAULT,
        "--output",
        &export.to_string_lossy(),
        "--json",
    ])
    .run()
    .ok()?;
    let ev: Value =
        serde_json::from_str(&fs::read_to_string(&export).unwrap_or_default()).unwrap_or_default();
    check!(
        ev["public_accounts"][0]["account_id"] == SEEDED_VAULT
            && ev["public_accounts"][0]["balance"] == 4242,
        "`state export` wrote {ev}"
    );
    Ok(())
}

/// TRPC-006 — Bad snapshots are rejected up front
pub fn bad_snapshots(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_created(cx, false)?;
    let root = p.root.to_string_lossy().into_owned();
    let dir = cx.empty_dir("trpc-006");
    let rejected = dir.join("rejected");
    let seeds_before = crate::scenarios::ls(&p.path(".scaffold/test-nodes/seeds"));
    let h = "\"format\":\"lgs-state-snapshot/1\"";
    let npk: Vec<String> = (1..=32).map(|n| n.to_string()).collect();
    let cases = [
        ("unknown format", "{\"format\":\"bogus/1\"}".to_string(), vec!["format mismatch"]),
        (
            "extra public field",
            format!("{{{h},\"public_accounts\":[{{\"account_id\":\"{SEEDED}\",\"balance\":1,\"nonce\":3}}]}}"),
            vec!["storage schema mismatch", "nonce"],
        ),
        (
            "private accounts",
            format!(
                "{{{h},\"public_accounts\":[],\"private_accounts\":[{{\"npk\":[{}],\"account\":{{\"balance\":5}}}}]}}",
                npk.join(",")
            ),
            vec!["storage schema mismatch", "private account"],
        ),
        (
            "foreign pin",
            format!("{{{h},\"lez_commit\":\"deadbeef\",\"public_accounts\":[{{\"account_id\":\"{SEEDED}\",\"balance\":1}}]}}"),
            vec!["lez pin mismatch", "deadbeef"],
        ),
        (
            "undecodable id",
            format!("{{{h},\"public_accounts\":[{{\"account_id\":\"not-base58!!\",\"balance\":1}}]}}"),
            vec!["account decode error", "is not base58"],
        ),
    ];
    for (name, body, expected) in cases {
        let input = dir.join("bad.json");
        fs::write(&input, body).expect("write");
        let out = tn(
            cx,
            &[
                "state",
                "seed",
                "--project",
                &root,
                "--input",
                &input.to_string_lossy(),
                "--output",
                &rejected.to_string_lossy(),
            ],
        )
        .run();
        out.fails()?;
        for needle in expected {
            check!(
                out.all().contains(needle),
                "{name}: expected {needle:?} in:\n{}",
                out.all()
            );
        }
    }
    check!(
        !rejected.exists(),
        "a rejected seed still created its output dir"
    );
    check!(
        crate::scenarios::ls(&p.path(".scaffold/test-nodes/seeds")) == seeds_before,
        "a rejected seed left a seeds/seed-* dir"
    );
    Ok(())
}

/// TRPC-007 — A real wallet transaction is observed as committed, also after a database restart
pub fn real_transaction(cx: &Ctx) -> R {
    let (p, _) = fixtures::tn_prepared(cx, false)?;
    // The wallet must exist and have a default account: that needs one
    // localnet session. Then the port is handed to a test node.
    fixtures::main_localnet(cx)?;
    cx.localnet_down();
    cx.lgs(&p.root, &["localnet", "stop"]).run();
    check!(
        port_free(PORT),
        "port {PORT} is busy; the wallet's node cannot start there"
    );

    let port = PORT.to_string();
    let node = Node::start(cx, &p, &["--port", &port, "--preserve-work-dir"])?;
    let head0 = node.rpc(&["blocks", "head", "--json"]).run().json()?["block_id"]
        .as_u64()
        .unwrap_or(0);
    let after = head0.to_string();
    cx.lgs(&p.root, &["wallet", "topup"])
        .timeout(600)
        .run()
        .ok()?;
    let seen = node
        .rpc(&[
            "blocks",
            "wait",
            "--after",
            &after,
            "--count",
            "6",
            "--timeout-sec",
            "180",
            "--json",
        ])
        .timeout(240)
        .run()
        .json()?;
    let bs = blocks(&seen);
    check!(
        bs.iter().all(|b| b["fully_parsed"] == true),
        "a block of real transactions was not fully parsed (wire-format regression): {seen}"
    );
    let user_tx = bs
        .iter()
        .filter(|b| b["has_user_transactions"] == true && b["transaction_count"] == 2)
        .flat_map(|b| b["transactions"].as_array().cloned().unwrap_or_default())
        .find(|t| t["is_clock"] == false && t["kind"] == "public");
    let Some(tx) = user_tx else {
        crate::harness::fail!(
            "no block with a user transaction after a wallet topup (r0vm missing or wrong version?): {seen}"
        );
    };
    let hash = tx["hash"].as_str().unwrap_or_default().to_string();
    let committed = node
        .rpc(&[
            "tx",
            "wait",
            "--hash",
            &hash,
            "--after-block",
            &after,
            "--json",
        ])
        .timeout(180)
        .run();
    committed.ok()?;
    let cv = committed.json()?;
    check!(
        cv["status"] == "committed" && cv["block_id"].is_u64() && cv["timestamp"].is_u64(),
        "`tx wait` of a committed transaction: {cv}"
    );

    // Stop, keep the database, resume from it: the transaction is still found,
    // even when asked to look only after a later block.
    let dir = node.dir.clone();
    node.stop().ok()?;
    check!(
        std::path::Path::new(&dir).is_dir(),
        "`--preserve-work-dir` did not keep {dir}"
    );
    let resumed = Node::start(cx, &p, &["--state", &dir]);
    let result = (|| -> R {
        let resumed = resumed?;
        let later = resumed.rpc(&["blocks", "head", "--json"]).run().json()?["block_id"]
            .as_u64()
            .unwrap_or(0);
        check!(
            later >= cv["block_id"].as_u64().unwrap_or(u64::MAX),
            "the resumed node restarted from genesis"
        );
        let again = resumed
            .rpc(&[
                "tx",
                "wait",
                "--hash",
                &hash,
                "--after-block",
                &later.to_string(),
                "--json",
            ])
            .timeout(180)
            .run();
        again.ok()?;
        let av = again.json()?;
        check!(
            av["status"] == "committed" && av["block_id"] == cv["block_id"],
            "after a database restart `tx wait` gave {av}, expected the original block {}",
            cv["block_id"]
        );
        Ok(())
    })();
    let _ = fs::remove_dir_all(&dir);
    result
}
