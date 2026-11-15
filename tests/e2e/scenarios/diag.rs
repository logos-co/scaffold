//! `.dogfooding/features/diagnostics.md`

use std::fs;

use crate::fixtures;
use crate::harness::{check, Ctx, Listener, Out, Project, Restore, BLACKHOLE, R};
use crate::scenarios::localnet::PORT;
use crate::scenarios::wallet::set_sequencer_addr;

/// DIAG-005 — Shell completions
pub fn completions(cx: &Ctx) -> R {
    let dir = cx.empty_dir("diag-005");
    let zsh = cx.lgs(&dir, &["completions", "zsh"]).run();
    zsh.ok()?;
    check!(
        zsh.stdout.starts_with("#compdef lgs logos-scaffold"),
        "zsh completions do not start with `#compdef lgs logos-scaffold`: {:?}",
        zsh.stdout.lines().next()
    );
    let bash = cx.lgs(&dir, &["completions", "bash"]).run();
    bash.ok()?;
    for name in ["lgs", "logos-scaffold"] {
        check!(
            bash.stdout
                .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
                .any(|w| w == name),
            "bash completions never mention `{name}`"
        );
    }
    for shell in ["bash", "zsh"] {
        let help = cx.lgs(&dir, &["completions", shell, "--help"]).run();
        help.ok()?.has(&format!("lgs completions {shell} >"))?;
    }
    Ok(())
}

/// One row of `doctor`'s table with the remediation printed under it.
#[derive(Debug, Default, Clone)]
pub struct Row {
    pub status: String,
    pub name: String,
    pub detail: String,
    pub remediation: String,
}

pub fn rows(out: &Out) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for line in out.stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, " | ").collect();
        if parts.len() == 3 && ["PASS", "WARN", "FAIL"].contains(&parts[0]) {
            rows.push(Row {
                status: parts[0].into(),
                name: parts[1].trim().into(),
                detail: parts[2].into(),
                remediation: String::new(),
            });
        } else if let (Some(text), Some(last)) = (
            line.trim_start().strip_prefix("remediation:"),
            rows.last_mut(),
        ) {
            last.remediation = text.trim().into();
        }
    }
    rows
}

pub fn row(out: &Out, name: &str) -> Row {
    rows(out)
        .into_iter()
        .find(|r| r.name == name)
        .unwrap_or_default()
}

fn doctor(cx: &Ctx, p: &Project) -> Out {
    cx.lgs(&p.root, &["-q", "doctor"]).timeout(300).run()
}

/// DIAG-001 — doctor gives actionable checks, also as JSON
pub fn doctor_checks(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_built(cx, false)?;
    // Step 1: every row that is not a pass says what to do.
    let out = doctor(cx, &p);
    let all = rows(&out);
    check!(
        all.len() > 10,
        "doctor printed only {} rows:\n{}",
        all.len(),
        out.stdout
    );
    for r in all.iter().filter(|r| r.status != "PASS") {
        check!(
            !r.remediation.is_empty(),
            "doctor row without a next step: {r:?}"
        );
    }
    let guest = row(&out, "guest build");
    check!(
        guest.detail.starts_with("local"),
        "`guest build` does not name the mode: {guest:?}"
    );
    for name in ["logos-blockchain-circuits", "cache root"] {
        check!(
            !row(&out, name).status.is_empty(),
            "doctor has no `{name}` row"
        );
    }
    let v = cx
        .lgs(&p.root, &["doctor", "--json"])
        .timeout(300)
        .run()
        .json()?;
    for key in ["status", "summary", "checks", "next_steps"] {
        check!(!v[key].is_null(), "`doctor --json` has no `{key}`");
    }

    // Steps 2-3: a circuits install that does not match the config.
    {
        let _restore = Restore::of(&p.path("scaffold.toml"));
        let toml = p.toml();
        let version = crate::scenarios::toml_value(
            &crate::scenarios::toml_section(&toml, "[circuits]"),
            "version",
        )
        .unwrap_or_default();
        let wrong = toml.replacen(
            &format!("[circuits]\nversion = \"{version}\""),
            "[circuits]\nversion = \"0.0.1\"",
            1,
        );
        check!(
            wrong != toml,
            "could not edit [circuits].version in scaffold.toml"
        );
        fs::write(p.path("scaffold.toml"), wrong).expect("write scaffold.toml");
        let out = doctor(cx, &p);
        out.fails()?;
        let r = row(&out, "logos-blockchain-circuits");
        check!(
            r.status == "FAIL"
                && r.detail.contains("configured [circuits].version=0.0.1")
                && r.detail.contains(".scaffold/circuits")
                && r.remediation.contains("logos-scaffold setup"),
            "circuits version mismatch row: {r:?}"
        );

        let empty = cx.empty_dir("empty-circuits");
        let moved = toml.replacen(
            "[circuits]\n",
            &format!("[circuits]\ninstall_dir = \"{}\"\n", empty.display()),
            1,
        );
        fs::write(p.path("scaffold.toml"), moved).expect("write scaffold.toml");
        let out = doctor(cx, &p);
        let r = row(&out, "logos-blockchain-circuits");
        check!(
            r.status == "FAIL"
                && r.detail.contains(&format!(
                    "missing configured circuits release at {}",
                    empty.display()
                ))
                && r.remediation.contains("logos-scaffold setup"),
            "missing circuits install row: {r:?}"
        );
    }

    // Step 4: a container runtime that never answers costs one bounded probe.
    if cx.sh(&p.root, "command -v docker").run().code != Some(0) {
        cx.note("step 4 (hanging DOCKER_HOST) not run: no docker CLI on PATH");
        return Ok(());
    }
    let (docker_mode, created) = fixtures::create(cx, "diag-docker", &[]);
    created.ok()?;
    let hang = Listener::bind(0, false)?;
    let out = cx
        .lgs(&docker_mode.root, &["-q", "doctor"])
        .env("DOCKER_HOST", format!("tcp://127.0.0.1:{}", hang.port))
        .timeout(120)
        .run();
    out.within(40.0)?;
    let runtime = row(&out, "container runtime");
    let guest = row(&out, "guest build");
    for (r, status) in [(&runtime, "WARN"), (&guest, "FAIL")] {
        check!(
            r.status == status
                && r.detail.contains("`docker info` did not answer within 10s")
                && r.remediation.contains("DOCKER_HOST")
                && !r.remediation.contains("127.0.0.1"),
            "with a hanging DOCKER_HOST, expected {status} naming the variable but not its value: {r:?}"
        );
    }
    Ok(())
}

/// DIAG-004 — report produces a support bundle without wallet secrets
pub fn report_bundle(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_built(cx, false)?;
    let out = cx
        .lgs(&p.root, &["report", "--tail", "200"])
        .timeout(300)
        .run();
    out.ok()?.has(".scaffold/reports/")?.has("nspect")?;
    check!(
        crate::scenarios::ls(&p.path(".scaffold/reports"))
            .iter()
            .any(|f| f.ends_with(".tar.gz")),
        "`report` left no archive in .scaffold/reports"
    );
    let archive = p.path("artifacts/support-report.tar.gz");
    let archive_s = archive.to_string_lossy().into_owned();
    cx.lgs(&p.root, &["report", "--out", &archive_s])
        .timeout(300)
        .run()
        .ok()?;
    check!(archive.is_file(), "`report --out` wrote no archive");
    let listing = cx.cmd(&p.root, "tar", &["-tzf", &archive_s]).run();
    listing.ok()?;
    for required in [
        "README.txt",
        "manifest.json",
        "diagnostics/doctor.json",
        "diagnostics/localnet-status.json",
        "summaries/build-evidence.json",
    ] {
        check!(
            listing.stdout.lines().any(|l| l.ends_with(required)),
            "the archive has no {required}:\n{}",
            listing.stdout
        );
    }
    check!(
        !listing
            .stdout
            .lines()
            .any(|l| l.contains("wallet/") || l.contains("storage.json")),
        "the archive contains wallet files:\n{}",
        listing.stdout
    );
    // No absolute local paths: neither the home directory nor the project root.
    let content = cx.cmd(&p.root, "tar", &["-xzf", &archive_s, "-O"]).run();
    let mut leaks = vec![p.root.to_string_lossy().into_owned()];
    if let Some(home) = std::env::var_os("HOME").filter(|h| h.len() > 1) {
        leaks.push(home.to_string_lossy().into_owned());
    }
    for leak in leaks {
        check!(
            !content.stdout.contains(&leak),
            "the archive contains the local path {leak}"
        );
    }

    // `report` embeds doctor, so it inherits the bounded runtime probe.
    if cx.sh(&p.root, "command -v docker").run().code == Some(0) {
        let hang = Listener::bind(0, false)?;
        let bounded = cx
            .work
            .join("report-bounded.tar.gz")
            .to_string_lossy()
            .into_owned();
        cx.lgs(&p.root, &["report", "--out", &bounded])
            .env("DOCKER_HOST", format!("tcp://127.0.0.1:{}", hang.port))
            .timeout(120)
            .run()
            .ok()?
            .within(40.0)?;
    } else {
        cx.note("bounded-probe step not run: no docker CLI on PATH");
    }
    Ok(())
}

/// DIAG-006 — doctor checks the wallet's sequencer address in either config shape
pub fn wallet_sequencer_address(cx: &Ctx) -> R {
    let p = fixtures::main_localnet(cx)?;
    let config = p.wallet_config();
    let net = |out: &Out| row(out, "wallet network config");
    let local = format!("http://127.0.0.1:{PORT}");

    let out = doctor(cx, &p);
    check!(
        net(&out).status == "PASS",
        "a fresh setup should pass: {:?}",
        net(&out)
    );

    // A wrong port is not a prefix match.
    {
        let _g = set_sequencer_addr(&p, &format!("http://127.0.0.1:{PORT}0"))?;
        let r = net(&doctor(cx, &p));
        check!(
            r.status == "WARN" && r.detail.contains(&format!("127.0.0.1:{PORT}0")),
            "port {PORT}0 must not pass for {PORT}: {r:?}"
        );
    }
    // An address that never answers: seconds, not minutes.
    if cx.caps.blackhole {
        let far = format!("http://{BLACKHOLE}:{PORT}");
        let _g = set_sequencer_addr(&p, &far)?;
        let out = doctor(cx, &p);
        out.within(60.0)?;
        let r = net(&out);
        check!(
            r.status == "WARN"
                && r.detail.contains(&far)
                && r.remediation.contains("sequencers[0].sequencer_addr")
                && r.remediation
                    .contains(".scaffold/wallet/wallet_config.json"),
            "`wallet network config` for an unreachable address: {r:?}"
        );
        let usable = row(&out, "wallet usability");
        check!(
            usable.status == "WARN"
                && usable.detail.contains(&format!(
                    "wallet cannot reach its configured sequencer at {far}"
                )),
            "`wallet usability` for an unreachable address: {usable:?}"
        );
        // A top-level key next to `sequencers` is never read by the v0.2.4 wallet.
        let mut v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&config).unwrap_or_default())
                .unwrap_or_default();
        if v["sequencers"].is_array() {
            v["sequencer_addr"] = serde_json::Value::String(local.clone());
            fs::write(&config, v.to_string()).expect("write wallet config");
            let r = net(&doctor(cx, &p));
            check!(
                r.status == "WARN" && r.detail.contains(&far),
                "an unread top-level sequencer_addr changed the verdict: {r:?}"
            );
        }
    } else {
        cx.note(format!(
            "unreachable-address steps not run: {BLACKHOLE} is not a black hole on this host"
        ));
    }
    // The pre-v0.2.4 shape, and a file that does not parse.
    let _restore = Restore::of(&config);
    fs::write(&config, "{\"sequencer_addr\": \"http://10.0.0.1:3040\"}").expect("write");
    let r = net(&doctor(cx, &p));
    check!(
        r.status == "WARN"
            && r.detail.contains("http://10.0.0.1:3040")
            && r.remediation.contains("sequencer_addr")
            && !r.remediation.contains("sequencers[0]"),
        "old-shape config: {r:?}"
    );
    fs::write(&config, "{ \"sequencers\": [ ").expect("write");
    let r = net(&doctor(cx, &p));
    check!(
        r.status == "WARN"
            && r.detail.contains("could not")
            && r.remediation.contains("logos-scaffold setup"),
        "unparseable config: {r:?}"
    );
    Ok(())
}
