//! `.dogfooding/features/localnet.md`

use std::fs;
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::time::Duration;

use crate::fixtures;
use crate::harness::{check, pid_alive, port_free, Ctx, Listener, Restore, R};
use crate::scenarios::tnode::tn;

pub const PORT: u16 = 3040;

/// A non-loopback address of this host, if it has one.
pub fn non_loopback_ip() -> Option<std::net::IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("10.255.255.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// LOCALNET-001 — Start localnet and wait until ready
pub fn start_ready(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_setup(cx, false)?;
    cx.localnet_down();
    cx.lgs(&p.root, &["localnet", "stop"]).run();
    if !port_free(PORT) {
        crate::harness::blocked!("port {PORT} is held by a process this suite did not start");
    }
    let start = cx.lgs(&p.root, &["localnet", "start"]).timeout(180).run();
    if start.code == Some(0) {
        cx.localnet_mark(&p, true);
    }
    start
        .ok()?
        .has("localnet ready (sequencer pid=")?
        .has("--listen-address 127.0.0.1")?;
    check!(
        !port_free(PORT),
        "`localnet start` returned but nothing answers on 127.0.0.1:{PORT}"
    );
    // Loopback only: the RPC has no caller authentication.
    match non_loopback_ip() {
        Some(ip) => check!(
            TcpStream::connect_timeout(&SocketAddr::new(ip, PORT), Duration::from_secs(2)).is_err(),
            "the sequencer RPC is reachable on {ip}:{PORT}, not only on loopback"
        ),
        None => cx.note(
            "loopback-only bind checked through the command line only: host has no other address",
        ),
    }
    Ok(())
}

/// LOCALNET-002 — Status and logs in human and JSON form
pub fn status_and_logs(cx: &Ctx) -> R {
    let p = fixtures::main_localnet(cx)?;
    let v = cx.localnet_status(&p)?;
    check!(
        v["tracked_pid"].is_u64()
            && v["listener_present"] == true
            && v["ownership"] == "managed"
            && v["ready"] == true,
        "`localnet status --json` of a running localnet: {v}"
    );
    let human = cx.lgs(&p.root, &["localnet", "status"]).run();
    human
        .ok()?
        .has(&format!("pid={}", v["tracked_pid"]))?
        .has("ownership: managed")?
        .has("ready: true")?;
    let logs = cx
        .lgs(&p.root, &["-q", "localnet", "logs", "--tail", "200"])
        .run();
    logs.ok()?;
    check!(
        !logs.stdout.trim().is_empty(),
        "`localnet logs` printed nothing for a running sequencer"
    );
    let j = cx
        .lgs(
            &p.root,
            &["-q", "localnet", "logs", "--tail", "20", "--json"],
        )
        .run()
        .json()?;
    let n = j["lines"].as_array().map(Vec::len).unwrap_or(usize::MAX);
    check!(
        (1..=20).contains(&n),
        "`logs --tail 20 --json` holds {n} lines: {j}"
    );
    Ok(())
}

/// LOCALNET-004 — Stop localnet, and a started one survives terminal closure
pub fn stop_and_survive(cx: &Ctx) -> R {
    let p = fixtures::main_localnet(cx)?;
    let stop = cx.lgs(&p.root, &["localnet", "stop"]).timeout(60).run();
    cx.localnet_mark(&p, false);
    stop.ok()?.has("localnet stopped")?;
    check!(
        cx.localnet_status(&p)?["ready"] == false,
        "status still says ready after stop"
    );
    check!(
        port_free(PORT),
        "a listener remains on {PORT} after `localnet stop`"
    );

    // Start it from a shell in its own process group, then hang that group up,
    // as a closing terminal does. The sequencer is daemonized and must stay.
    let mut shell = cx
        .sh(&p.root, "\"$LGS\" localnet start; sleep 120")
        .spawn(&cx.out.join("logs/LOCALNET-004-shell.log"))?;
    let ready = cx.poll(180, 500, || {
        (cx.localnet_status(&p).ok()?["ready"] == true).then_some(())
    });
    check!(
        ready.is_some(),
        "localnet did not become ready:\n{}",
        shell.log_text()
    );
    cx.localnet_mark(&p, true);
    let pid = cx.localnet_status(&p)?["tracked_pid"].as_u64().unwrap_or(0) as u32;
    shell.signal(libc::SIGHUP);
    check!(
        shell.wait(10).is_some(),
        "the starting shell did not exit on SIGHUP"
    );
    drop(shell);
    std::thread::sleep(Duration::from_secs(1));
    check!(
        pid_alive(pid) && cx.localnet_status(&p)?["ready"] == true,
        "the sequencer (pid {pid}) died with the shell that started it"
    );
    Ok(())
}

/// LOCALNET-006 — Reset chain state
pub fn reset(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_topped_up(cx, false)?;
    let url = format!("http://127.0.0.1:{PORT}");
    let addr = p.default_address().unwrap_or_default();
    let state = || -> R<String> {
        let v = tn(
            cx,
            &[
                "account",
                "get",
                "--url",
                &url,
                "--account-id",
                &addr,
                "--json",
            ],
        )
        .run()
        .json()?;
        Ok(v["state"].as_str().unwrap_or_default().to_string())
    };
    check!(
        state()? == "present",
        "the default account is not on chain before the reset"
    );

    // Step 1: destructive, so it asks.
    cx.lgs(&p.root, &["localnet", "reset"])
        .run()
        .fails()?
        .has("--yes")?
        .has("--dry-run")?;
    // Step 2: the dry run changes nothing.
    let pid = cx.localnet_status(&p)?["tracked_pid"].clone();
    cx.lgs(&p.root, &["localnet", "reset", "--dry-run"])
        .run()
        .ok()?
        .has("rocksdb")?;
    check!(
        cx.localnet_status(&p)?["tracked_pid"] == pid && state()? == "present",
        "`reset --dry-run` changed something"
    );
    // Step 3: the real thing. Everything deployed or funded so far is gone,
    // so the fixtures that put it there must run again.
    cx.forget("main: wallet topup");
    cx.forget("main: deploy");
    let wallet_files = crate::scenarios::ls(&p.path(".scaffold/wallet"));
    cx.lgs(
        &p.root,
        &["localnet", "reset", "--yes", "--verify-timeout-sec", "120"],
    )
    .timeout(300)
    .run()
    .ok()?
    .has("localnet reset complete; sequencer producing blocks")?;
    // Step 4.
    let head = tn(cx, &["blocks", "head", "--url", &url, "--json"])
        .run()
        .json()?;
    check!(
        head["block_id"].as_u64().is_some_and(|b| b <= 3),
        "the chain did not restart from genesis: {head}"
    );
    check!(
        state()? == "missing",
        "the default account survived the reset"
    );
    check!(
        crate::scenarios::ls(&p.path(".scaffold/wallet")) == wallet_files,
        "the reset touched the wallet files without --reset-wallet"
    );
    Ok(())
}

/// LOCALNET-007 — Start failure points at the next diagnostic step
pub fn start_failures(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_built(cx, false)?;
    cx.localnet_down();
    cx.lgs(&p.root, &["localnet", "stop"]).run();
    check!(
        port_free(PORT),
        "port {PORT} is still busy after stopping localnet"
    );
    let me = std::process::id();
    {
        // Step 1: a foreign process (this one) holds the port.
        let _foreign = Listener::bind(PORT, true)?;
        let start = cx.lgs(&p.root, &["localnet", "start"]).timeout(60).run();
        start
            .fails()?
            .has(&format!(
                "cannot start localnet: port {PORT} is already in use (pid={me})"
            ))?
            .has(&format!("Try: kill {me}"))?;
        let status = cx.lgs(&p.root, &["localnet", "status"]).run();
        status
            .ok()?
            .has("ownership: foreign")?
            .has(&format!("pid={me}"))?;
        // Step 2: `run` stops at its localnet step the same way.
        let run = cx
            .lgs(
                &p.root,
                &["run", "--localnet-timeout", "5", "--no-post-deploy"],
            )
            .timeout(fixtures::BUILD_TIMEOUT)
            .run();
        run.fails()?
            .has("Ensuring localnet")?
            .has(&format!(
                "localnet port is in use by another process (pid={me})"
            ))?
            .has(&format!("kill {me}"))?
            .lacks("Deploying programs")?;
    }
    check!(
        port_free(PORT),
        "the test listener did not release port {PORT}"
    );

    // Step 3: the sequencer dies at startup (a corrupt database).
    let root = p.root.to_string_lossy().into_owned();
    let pins = tn(cx, &["pins", "--project", &root, "--json"])
        .run()
        .json()?;
    let current = std::path::Path::new(pins["lez_checkout"].as_str().unwrap_or_default())
        .join("rocksdb/CURRENT");
    check!(
        current.is_file(),
        "no sequencer database at {}",
        current.display()
    );
    {
        let _restore = Restore::of(&current);
        fs::write(&current, "junk\n").expect("corrupt CURRENT");
        let start = cx.lgs(&p.root, &["localnet", "start"]).timeout(180).run();
        start
            .fails()?
            .has("sequencer process exited before becoming ready")?
            .has("logos-scaffold localnet status")?
            .has("logos-scaffold localnet logs --tail 200")?;
    }
    // The restored database starts again.
    cx.localnet_up(&p)
}
