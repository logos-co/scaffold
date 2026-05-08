use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use serde_json::Value;

use crate::constants::{SEQUENCER_BIN_REL_PATH, SEQUENCER_CONFIG_REL_PATH};
use crate::error::{LocalnetError, ResetError};
use crate::model::{LocalnetOwnership, LocalnetState, LocalnetStatusReport, Project};
use crate::process::{listener_pid, pid_alive, pid_command, pid_running, port_open, spawn_to_log};
use crate::project::{ensure_dir_exists, find_project_root, load_project, resolve_repo_path};
use crate::state::{read_localnet_state, write_localnet_state};
use crate::DynResult;

use super::wallet_support::{rpc_get_last_block_id, wallet_state_path, RpcReachabilityError};

// LOCALNET_ADDR is now read from project config (localnet.port)

#[derive(Debug, Clone, Copy)]
pub(crate) enum LocalnetAction {
    Start {
        timeout_sec: u64,
    },
    Stop,
    Status {
        json: bool,
    },
    Logs {
        tail: usize,
    },
    Reset {
        reset_wallet: bool,
        verify_timeout_sec: u64,
    },
}

pub(crate) fn cmd_localnet(action: LocalnetAction) -> DynResult<()> {
    match action {
        LocalnetAction::Stop => {
            let cwd = env::current_dir()?;
            if find_project_root(cwd).is_some() {
                let project = load_project()?;
                cmd_localnet_in_project(&project, action)
            } else {
                cmd_localnet_stop_outside_project()
            }
        }
        _ => {
            let project = load_project()?;
            cmd_localnet_in_project(&project, action)
        }
    }
}

pub(crate) fn build_localnet_status_for_project(project: &Project) -> LocalnetStatusReport {
    let state_path = project.root.join(".scaffold/state/localnet.state");
    let log_path = project.root.join(".scaffold/logs/sequencer.log");
    build_status_report(
        &state_path,
        &log_path,
        &format!("127.0.0.1:{}", project.config.localnet.port),
        project.config.localnet.port,
    )
}

fn cmd_localnet_in_project(project: &Project, action: LocalnetAction) -> DynResult<()> {
    let localnet_port = project.config.localnet.port;
    let risc0_dev_mode = project.config.localnet.risc0_dev_mode;
    let localnet_addr = format!("127.0.0.1:{localnet_port}");
    let lez = resolve_repo_path(project, &project.config.lez, "lez")?;
    let state_path = project.root.join(".scaffold/state/localnet.state");
    let logs_dir = project.root.join(".scaffold/logs");
    let log_path = logs_dir.join("sequencer.log");
    fs::create_dir_all(&logs_dir)?;

    match action {
        LocalnetAction::Start { timeout_sec } => cmd_localnet_start(
            &lez,
            &state_path,
            &log_path,
            timeout_sec,
            localnet_port,
            risc0_dev_mode,
            &localnet_addr,
        ),
        LocalnetAction::Stop => cmd_localnet_stop(&state_path, localnet_port),
        LocalnetAction::Status { json } => {
            cmd_localnet_status(&state_path, &log_path, json, &localnet_addr, localnet_port)
        }
        LocalnetAction::Logs { tail } => cmd_localnet_logs(&log_path, tail),
        LocalnetAction::Reset {
            reset_wallet,
            verify_timeout_sec,
        } => cmd_localnet_reset(
            project,
            &lez,
            &state_path,
            &log_path,
            &localnet_addr,
            reset_wallet,
            verify_timeout_sec,
        ),
    }
}

fn cmd_localnet_stop_outside_project() -> DynResult<()> {
    let default_addr = "127.0.0.1:3040";
    let default_port: u16 = 3040;
    if !port_open(default_addr) {
        println!("localnet not running (no listener on {default_addr})");
        return Ok(());
    }

    let listener_pid = listener_pid(default_port);
    let pid_text = listener_pid
        .map(|pid| pid.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    println!("listener detected on {default_addr} (pid={pid_text})");
    println!(
        "This command is running outside a logos-scaffold project; it will not stop unmanaged processes automatically."
    );
    println!(
        "This may be a sequencer started from another project and may not match your current workspace."
    );

    if let Some(pid) = listener_pid {
        if let Some(command) = pid_command(pid) {
            println!("listener process: {command}");
        }
        println!("Try: kill {pid}");
    } else {
        println!("Try: lsof -nP -iTCP:{default_port} -sTCP:LISTEN");
    }

    Ok(())
}

fn cmd_localnet_start(
    lez: &Path,
    state_path: &Path,
    log_path: &Path,
    timeout_sec: u64,
    localnet_port: u16,
    risc0_dev_mode: bool,
    localnet_addr: &str,
) -> DynResult<()> {
    ensure_dir_exists(lez, "lez")?;
    let sequencer_bin = lez.join(SEQUENCER_BIN_REL_PATH);
    if !sequencer_bin.exists() {
        return Err(LocalnetError::MissingSequencerBinary {
            path: sequencer_bin.display().to_string(),
        }
        .into());
    }

    let mut state = read_localnet_state(state_path).unwrap_or_default();
    if let Some(pid) = state.sequencer_pid {
        if pid_running(pid) {
            wait_for_readiness(pid, timeout_sec, log_path, localnet_addr)?;
            println!("localnet ready (sequencer pid={pid})");
            return Ok(());
        }

        if state_path.exists() {
            fs::remove_file(state_path)?;
        }
        state = LocalnetState::default();
    }

    let existing_listener_pid = listener_pid(localnet_port);
    if port_open(localnet_addr) {
        let mut message = match existing_listener_pid {
            Some(pid) => {
                format!("cannot start localnet: port {localnet_port} is already in use (pid={pid})")
            }
            None => format!(
                "cannot start localnet: port {localnet_port} is already in use (pid=unknown)"
            ),
        };
        message.push_str(
            "\nThis may be a sequencer started from another project and may not work with the current project.",
        );
        message.push_str("\nStop that process and retry `logos-scaffold localnet start`.");
        if let Some(pid) = existing_listener_pid {
            message.push_str(&format!("\nTry: kill {pid}"));
        }
        bail!("{message}");
    }

    patch_sequencer_port(lez, localnet_port)?;

    // Use a path relative to lez (the child's cwd), not relative to the
    // parent's cwd.  `current_dir(lez)` applies before exec, so a parent-
    // relative path like `.scaffold/cache/repos/lez/target/release/…`
    // would be resolved inside lez and fail with ENOENT.
    let sequencer_pid = spawn_to_log(
        Command::new(format!("./{SEQUENCER_BIN_REL_PATH}"))
            .current_dir(lez)
            .arg(SEQUENCER_CONFIG_REL_PATH)
            .env("RUST_LOG", "info")
            .env("RISC0_DEV_MODE", if risc0_dev_mode { "1" } else { "0" }),
        log_path,
    )?;

    state.sequencer_pid = Some(sequencer_pid);
    write_localnet_state(state_path, &state)?;

    if let Err(err) = wait_for_readiness(sequencer_pid, timeout_sec, log_path, localnet_addr) {
        if pid_alive(sequencer_pid) {
            let _ = Command::new("kill").arg(sequencer_pid.to_string()).status();
        }
        if state_path.exists() {
            let _ = fs::remove_file(state_path);
        }
        return Err(err);
    }

    println!("localnet ready (sequencer pid={sequencer_pid})");
    Ok(())
}

fn wait_for_readiness(
    pid: u32,
    timeout_sec: u64,
    log_path: &Path,
    localnet_addr: &str,
) -> DynResult<()> {
    let deadline = Instant::now() + Duration::from_secs(timeout_sec.max(1));

    loop {
        let running = pid_running(pid);
        let ready = running && port_open(localnet_addr);
        if ready {
            return Ok(());
        }

        if !running {
            return Err(LocalnetError::ExitedBeforeReady {
                pid,
                log_tail: read_log_tail(log_path, 60),
            }
            .into());
        }

        if Instant::now() >= deadline {
            return Err(LocalnetError::StartTimeout {
                timeout_sec,
                pid,
                log_tail: read_log_tail(log_path, 60),
            }
            .into());
        }

        thread::sleep(Duration::from_millis(200));
    }
}

fn cmd_localnet_stop(state_path: &Path, localnet_port: u16) -> DynResult<()> {
    let localnet_addr = format!("127.0.0.1:{localnet_port}");
    let report = build_status_report(
        state_path,
        Path::new(".scaffold/logs/sequencer.log"),
        &localnet_addr,
        localnet_port,
    );
    if let Some(pid) = report.tracked_pid {
        if report.tracked_running {
            println!("$ kill {pid} # sequencer");
            let _ = Command::new("kill").arg(pid.to_string()).status();
        } else {
            println!("sequencer state is stale (pid={pid} not running)");
        }

        if state_path.exists() {
            fs::remove_file(state_path)?;
        }
        println!("localnet stopped");
        return Ok(());
    }

    if report.listener_present {
        let pid_text = report
            .listener_pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "foreign listener detected on {localnet_addr} (pid={pid_text}); not stopping unmanaged process"
        );
        return Ok(());
    }

    println!("localnet not running");
    Ok(())
}

fn cmd_localnet_status(
    state_path: &Path,
    log_path: &Path,
    as_json: bool,
    localnet_addr: &str,
    localnet_port: u16,
) -> DynResult<()> {
    let report = build_status_report(state_path, log_path, localnet_addr, localnet_port);

    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    if let Some(pid) = report.tracked_pid {
        println!(
            "tracked sequencer: pid={pid} running={}",
            report.tracked_running
        );
    } else {
        println!("tracked sequencer: not tracked");
    }

    if report.listener_present {
        let pid_text = report
            .listener_pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!("listener {localnet_addr}: reachable (pid={pid_text})");
    } else {
        println!("listener {localnet_addr}: not reachable");
    }

    println!("ownership: {}", ownership_label(report.ownership));
    println!("ready: {}", report.ready);
    if !report.remediation.is_empty() {
        println!("next steps:");
        for step in &report.remediation {
            println!("- {step}");
        }
    }

    Ok(())
}

fn ownership_label(ownership: LocalnetOwnership) -> &'static str {
    match ownership {
        LocalnetOwnership::Managed => "managed",
        LocalnetOwnership::Foreign => "foreign",
        LocalnetOwnership::StaleState => "stale_state",
        LocalnetOwnership::ManagedNotReady => "managed_not_ready",
        LocalnetOwnership::Stopped => "stopped",
    }
}

fn cmd_localnet_logs(log_path: &Path, tail: usize) -> DynResult<()> {
    if !log_path.exists() {
        println!("log file does not exist yet: {}", log_path.display());
        return Ok(());
    }

    let content = fs::read_to_string(log_path)
        .with_context(|| format!("failed to read log file {}", log_path.display()))?;

    if content.trim().is_empty() {
        println!("log file is empty: {}", log_path.display());
        return Ok(());
    }

    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(tail);
    for line in &lines[start..] {
        println!("{line}");
    }

    Ok(())
}

fn build_status_report(
    state_path: &Path,
    log_path: &Path,
    localnet_addr: &str,
    localnet_port: u16,
) -> LocalnetStatusReport {
    let state = read_localnet_state(state_path).unwrap_or_default();
    let tracked_pid = state.sequencer_pid;
    let tracked_running = tracked_pid.map(pid_running).unwrap_or(false);
    let listener_present = port_open(localnet_addr);
    let listener_pid = if listener_present {
        listener_pid(localnet_port)
    } else {
        None
    };

    let ownership = match (tracked_pid, tracked_running, listener_present) {
        (Some(pid), true, true) => match listener_pid {
            Some(listener) if listener == pid => LocalnetOwnership::Managed,
            Some(_) => LocalnetOwnership::Foreign,
            None => LocalnetOwnership::ManagedNotReady,
        },
        (Some(_), true, false) => LocalnetOwnership::ManagedNotReady,
        (Some(_), false, _) => LocalnetOwnership::StaleState,
        (None, _, true) => LocalnetOwnership::Foreign,
        (None, _, false) => LocalnetOwnership::Stopped,
    };

    let ready = tracked_running && listener_present;
    let remediation = match ownership {
        LocalnetOwnership::Managed if ready => vec![],
        LocalnetOwnership::Managed => {
            vec!["Wait a moment and re-run `logos-scaffold localnet status`".to_string()]
        }
        LocalnetOwnership::ManagedNotReady => vec![
            "Run `logos-scaffold localnet logs --tail 200` to inspect startup issues".to_string(),
            "Run `logos-scaffold localnet stop` then `logos-scaffold localnet start`".to_string(),
        ],
        LocalnetOwnership::StaleState => vec![
            "Run `logos-scaffold localnet stop` to clean stale state".to_string(),
            "Run `logos-scaffold localnet start` to restart localnet".to_string(),
        ],
        LocalnetOwnership::Foreign => vec![
            format!("Stop the external listener on {localnet_addr} or choose a clean environment"),
            "Then run `logos-scaffold localnet start`".to_string(),
        ],
        LocalnetOwnership::Stopped => vec!["Run `logos-scaffold localnet start`".to_string()],
    };

    LocalnetStatusReport {
        tracked_pid,
        tracked_running,
        listener_present,
        listener_pid,
        ownership,
        ready,
        log_path: log_path.display().to_string(),
        remediation,
    }
}

/// Update the port in `sequencer_config.json` so the sequencer listens on the
/// configured port.  The pinned LEZ version does not accept `--port` as a CLI
/// flag — it reads the port from this file.
fn patch_sequencer_port(lez: &Path, port: u16) -> DynResult<()> {
    let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
    let text = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let mut doc: Value =
        serde_json::from_str(&text).context("failed to parse sequencer_config.json")?;

    if let Some(obj) = doc.as_object_mut() {
        obj.insert("port".to_string(), Value::Number(port.into()));
    } else {
        bail!(
            "sequencer_config.json is not a JSON object: {}",
            config_path.display()
        );
    }

    let updated = serde_json::to_string_pretty(&doc).context("failed to serialize config")?;
    fs::write(&config_path, format!("{updated}\n"))
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    Ok(())
}

fn read_log_tail(log_path: &Path, tail: usize) -> String {
    let Ok(content) = fs::read_to_string(log_path) else {
        return format!("<log file missing: {}>", log_path.display());
    };

    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return "<no log output yet>".to_string();
    }

    let start = lines.len().saturating_sub(tail);
    lines[start..].join("\n")
}

// ─── reset ───────────────────────────────────────────────────────────────────

pub(crate) fn cmd_localnet_reset(
    project: &Project,
    lez: &Path,
    state_path: &Path,
    log_path: &Path,
    localnet_addr: &str,
    reset_wallet: bool,
    verify_timeout_sec: u64,
) -> DynResult<()> {
    let localnet_port = project.config.localnet.port;

    // Prerequisite: the sequencer binary must already be built. If not, setup
    // would fail later and we'd have already deleted data with no way to start.
    let sequencer_bin = lez.join(SEQUENCER_BIN_REL_PATH);
    if !sequencer_bin.exists() {
        return Err(LocalnetError::MissingSequencerBinary {
            path: sequencer_bin.display().to_string(),
        }
        .into());
    }

    println!("stopping sequencer…");
    cmd_localnet_stop(state_path, localnet_port)?;

    // `cmd_localnet_stop` sends SIGTERM without waiting, so the port may still
    // be held by our own sequencer for a short window. Poll briefly for it to
    // free. If it stays open past the deadline, something foreign owns it and
    // we refuse to delete data (restart would fail anyway).
    wait_for_port_free(localnet_addr, Duration::from_secs(5)).map_err(|_| {
        ResetError::ForeignListener {
            addr: localnet_addr.to_string(),
            pid: listener_pid(localnet_port),
        }
    })?;

    reset_cleanup(project, lez, state_path, reset_wallet)?;

    println!("starting sequencer…");
    cmd_localnet_start(
        lez,
        state_path,
        log_path,
        20,
        localnet_port,
        project.config.localnet.risc0_dev_mode,
        localnet_addr,
    )?;

    println!("waiting for block production…");
    verify_block_production(localnet_addr, verify_timeout_sec)
}

/// Deletes on-disk state so the next start begins with a fresh chain.
/// Extracted from `cmd_localnet_reset` so it can be unit-tested without
/// invoking setup or starting a real sequencer.
fn reset_cleanup(
    project: &Project,
    lez: &Path,
    state_path: &Path,
    reset_wallet: bool,
) -> DynResult<()> {
    let rocksdb_path = lez.join("rocksdb");
    remove_dir_if_exists(&rocksdb_path, "sequencer DB")?;

    if reset_wallet {
        let wallet_path = project.root.join(&project.config.wallet_home_dir);
        remove_dir_if_exists(&wallet_path, "wallet")?;

        let wallet_state = wallet_state_path(&project.root);
        remove_file_if_exists(&wallet_state, "wallet state")?;
    } else {
        println!("preserving wallet (pass --reset-wallet to delete)");
    }

    remove_file_if_exists(state_path, "localnet state")?;
    Ok(())
}

fn remove_dir_if_exists(path: &Path, label: &str) -> DynResult<()> {
    if path.exists() {
        fs::remove_dir_all(path)
            .with_context(|| format!("failed to delete {label} at {}", path.display()))?;
        println!("deleted {label} at {}", path.display());
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path, label: &str) -> DynResult<()> {
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("failed to delete {label} at {}", path.display()))?;
        println!("deleted {label} at {}", path.display());
    }
    Ok(())
}

/// Poll `localnet_addr` until no listener is accepting, or `timeout` elapses.
fn wait_for_port_free(localnet_addr: &str, timeout: Duration) -> Result<(), ()> {
    let deadline = Instant::now() + timeout;
    loop {
        if !port_open(localnet_addr) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn verify_block_production(localnet_addr: &str, timeout_sec: u64) -> DynResult<()> {
    // `rpc_get_last_block_id` needs a full URL; `localnet_addr` is `host:port`.
    let rpc_url = format!("http://{localnet_addr}");
    let deadline = Instant::now() + Duration::from_secs(timeout_sec.max(1));
    loop {
        if Instant::now() >= deadline {
            return Err(ResetError::BlocksNotProduced { timeout_sec }.into());
        }

        match rpc_get_last_block_id(&rpc_url) {
            Ok(block_height) if block_height > 0 => {
                println!(
                    "localnet reset complete; sequencer producing blocks (block_height={block_height})"
                );
                return Ok(());
            }
            Ok(_) => {
                // block_height == 0 — sequencer is up but no block yet; keep polling
            }
            Err(RpcReachabilityError::Connectivity(_)) => {
                // port may still be coming up; keep polling
            }
            Err(e) => {
                return Err(ResetError::VerificationPollFailed(e.to_string()).into());
            }
        }

        thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use std::net::TcpListener;
    use std::time::Duration;

    use super::{reset_cleanup, verify_block_production, wait_for_port_free};
    use crate::commands::wallet_support::wallet_state_path;
    use crate::error::ResetError;
    use crate::model::{
        Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, Project, RepoRef, RunConfig,
    };

    fn make_test_project(temp: &tempfile::TempDir) -> (Project, PathBuf) {
        let lez_dir = temp.path().join(".scaffold/cache/repos/lez");
        let wallet_dir = temp.path().join(".scaffold/wallet");
        let state_dir = temp.path().join(".scaffold/state");
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&lez_dir).unwrap();
        fs::create_dir_all(&wallet_dir).unwrap();

        let config = Config {
            version: "1.0.0".to_string(),
            cache_root: temp.path().join(".scaffold/cache").display().to_string(),
            lez: RepoRef {
                source: String::new(),
                pin: String::new(),
                build: crate::model::RepoBuild::Cargo,
                attr: String::new(),
                path: lez_dir.display().to_string(),
            },
            spel: RepoRef::default(),
            basecamp_repo: None,
            lgpm_repo: None,
            wallet_home_dir: ".scaffold/wallet".to_string(),
            framework: FrameworkConfig {
                kind: String::new(),
                version: String::new(),
                idl: FrameworkIdlConfig {
                    spec: String::new(),
                    path: String::new(),
                },
            },
            localnet: LocalnetConfig {
                port: 3040,
                risc0_dev_mode: false,
            },
            modules: std::collections::BTreeMap::new(),
            basecamp: None,
            run: RunConfig::default(),
        };

        let project = Project {
            root: temp.path().to_path_buf(),
            config,
        };
        (project, lez_dir)
    }

    #[test]
    fn cleanup_preserves_wallet_by_default() {
        let temp = tempdir().unwrap();
        let (project, lez) = make_test_project(&temp);

        let wallet_dir = project.root.join(&project.config.wallet_home_dir);
        let marker = wallet_dir.join("keys.json");
        fs::write(&marker, "{}").unwrap();
        let wallet_state = wallet_state_path(&project.root);
        fs::write(&wallet_state, "default_address=Public/demo\n").unwrap();
        let state_path = project.root.join(".scaffold/state/localnet.state");
        fs::write(&state_path, "sequencer_pid=123\n").unwrap();
        let rocksdb = lez.join("rocksdb");
        fs::create_dir_all(&rocksdb).unwrap();

        reset_cleanup(&project, &lez, &state_path, false).unwrap();

        assert!(!rocksdb.exists(), "rocksdb should be deleted");
        assert!(!state_path.exists(), "localnet state should be deleted");
        assert!(
            marker.exists(),
            "wallet keypairs must survive default reset"
        );
        assert!(
            wallet_state.exists(),
            "wallet state must survive default reset"
        );
    }

    #[test]
    fn cleanup_deletes_wallet_when_reset_wallet_true() {
        let temp = tempdir().unwrap();
        let (project, lez) = make_test_project(&temp);

        let wallet_dir = project.root.join(&project.config.wallet_home_dir);
        fs::write(wallet_dir.join("keys.json"), "{}").unwrap();
        let wallet_state = wallet_state_path(&project.root);
        fs::write(&wallet_state, "default_address=Public/demo\n").unwrap();
        let state_path = project.root.join(".scaffold/state/localnet.state");

        reset_cleanup(&project, &lez, &state_path, true).unwrap();

        assert!(
            !wallet_dir.exists(),
            "wallet must be deleted with --reset-wallet"
        );
        assert!(
            !wallet_state.exists(),
            "wallet state must be deleted with --reset-wallet"
        );
    }

    #[test]
    fn cleanup_is_idempotent_when_nothing_exists() {
        let temp = tempdir().unwrap();
        let (project, lez) = make_test_project(&temp);

        let wallet_dir = project.root.join(&project.config.wallet_home_dir);
        fs::remove_dir_all(&wallet_dir).unwrap();
        let state_path = project.root.join(".scaffold/state/localnet.state");

        // No rocksdb, no wallet, no state file — cleanup should succeed silently.
        reset_cleanup(&project, &lez, &state_path, true).unwrap();
    }

    #[test]
    fn wait_for_port_free_returns_ok_when_nothing_listening() {
        // Port 1 is privileged and unbound on a user account.
        wait_for_port_free("127.0.0.1:1", Duration::from_millis(200)).unwrap();
    }

    #[test]
    fn wait_for_port_free_times_out_when_listener_stays_open() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let err = wait_for_port_free(&addr, Duration::from_millis(300));
        drop(listener);
        assert!(err.is_err(), "expected timeout while listener was open");
    }

    #[test]
    fn verify_block_production_times_out_with_bounded_timeout() {
        // Poll a port nothing is listening on; verification should exit after
        // timeout_sec with BlocksNotProduced rather than hang.
        let err = verify_block_production("127.0.0.1:1", 1).unwrap_err();
        let reset_err = err
            .downcast_ref::<ResetError>()
            .expect("ResetError variant");
        match reset_err {
            ResetError::BlocksNotProduced { timeout_sec } => assert_eq!(*timeout_sec, 1),
            other => panic!("expected BlocksNotProduced, got {other:?}"),
        }
    }
}
