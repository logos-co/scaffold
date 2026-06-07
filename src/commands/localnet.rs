use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use serde_json::Value;

use crate::circuits::ensure_circuits_for_subprocess;
use crate::constants::{SEQUENCER_BIN_REL_PATH, SEQUENCER_CONFIG_REL_PATH};
use crate::error::{LocalnetError, ResetError};
use crate::model::{
    LocalnetLogsReport, LocalnetOwnership, LocalnetState, LocalnetStatusReport, Project,
};
use crate::process::{listener_pid, pid_alive, pid_command, pid_running, port_open, spawn_to_log};
use crate::project::{
    ensure_dir_exists, find_project_root, load_project, resolve_cache_root, resolve_repo_path,
};
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
        json: bool,
    },
    Reset {
        dry_run: bool,
        yes: bool,
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

    // The standalone `sequencer_service` binary calls into the
    // `logos-blockchain-zksign` runtime, which loads circuit witness
    // generators from `LOGOS_BLOCKCHAIN_CIRCUITS` (or `~/.logos-blockchain-circuits`)
    // and panics if neither exists. Materialise the release if absent and
    // export the env var so any subprocess we spawn here inherits it.
    if matches!(
        action,
        LocalnetAction::Start { .. } | LocalnetAction::Reset { .. }
    ) {
        let (cache_root, _) = resolve_cache_root(project)?;
        ensure_circuits_for_subprocess(&cache_root)?;
    }

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
        LocalnetAction::Logs { tail, json } => cmd_localnet_logs(&log_path, tail, json),
        LocalnetAction::Reset {
            dry_run,
            yes,
            reset_wallet,
            verify_timeout_sec,
        } => cmd_localnet_reset(
            project,
            &lez,
            &state_path,
            &log_path,
            &localnet_addr,
            dry_run,
            yes,
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

    let state_dir = state_path.parent().ok_or_else(|| {
        anyhow!(
            "localnet state path has no parent directory: {}",
            state_path.display()
        )
    })?;
    let patched_config_path = prepare_sequencer_config(lez, state_dir, localnet_port)?;

    // Use a path relative to lez (the child's cwd), not relative to the
    // parent's cwd.  `current_dir(lez)` applies before exec, so a parent-
    // relative path like `.scaffold/cache/repos/lez/target/release/…`
    // would be resolved inside lez and fail with ENOENT. The patched config
    // path is absolute (under the project's `.scaffold/state/`), so it is
    // unaffected by the cwd switch.
    let mut sequencer_cmd = Command::new(format!("./{SEQUENCER_BIN_REL_PATH}"));
    sequencer_cmd
        .current_dir(lez)
        .arg(&patched_config_path)
        .env("RUST_LOG", "info")
        .env("RISC0_DEV_MODE", if risc0_dev_mode { "1" } else { "0" });

    // Auto-detect r0vm path from rzup installation if RISC0_SERVER_PATH is not set.
    // On macOS, rzup installs r0vm under ~/.risc0/extensions/<version>/r0vm but does
    // not add it to PATH. The sequencer needs RISC0_SERVER_PATH to locate it.
    // We derive the exact risc0-zkvm version from the LEZ Cargo.lock so we never
    // hand the sequencer a mismatched r0vm binary.
    if std::env::var("RISC0_SERVER_PATH").is_err() {
        if let Some(r0vm_path) = find_r0vm_path_for_lez(lez) {
            sequencer_cmd.env("RISC0_SERVER_PATH", &r0vm_path);
        } else {
            eprintln!(
                "warning: RISC0_SERVER_PATH is not set and r0vm was not found in the \
                 rzup extensions directory for the LEZ-pinned risc0 version. \
                 If transaction execution fails, set RISC0_SERVER_PATH to the r0vm \
                 binary installed by rzup."
            );
        }
    }

    let sequencer_pid = spawn_to_log(&mut sequencer_cmd, log_path)?;

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
            let kill_output = Command::new("kill")
                .arg(pid.to_string())
                .output()
                .context("failed to spawn `kill`")?;
            let kill_succeeded = kill_output.status.success();

            // Wait for the process to actually exit, regardless of `kill`'s
            // exit code: TERM may have been delivered even if `kill` returned
            // non-zero (race with reaping). If the process is still alive
            // after the deadline, do not remove the state file — we still
            // legitimately track this pid.
            let pid_timeout = Duration::from_secs(5);
            if !wait_for_pid_exit(pid, pid_timeout) {
                if !kill_succeeded {
                    let stderr = String::from_utf8_lossy(&kill_output.stderr)
                        .trim()
                        .to_string();
                    return Err(LocalnetError::StopKillFailed { pid, stderr }.into());
                }
                return Err(LocalnetError::StopTimeout {
                    pid,
                    timeout_sec: pid_timeout.as_secs(),
                }
                .into());
            }

            // Process exited; wait briefly for the bound socket to be released
            // so an immediate `localnet start` doesn't race the still-bound
            // port. A timeout here is not fatal — the port may be held by a
            // foreign listener that survived our sequencer, and `localnet
            // start` will surface that with a more specific error.
            let _ = wait_for_port_free(&localnet_addr, Duration::from_secs(5));
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

    let sequencer_url = format!("http://{localnet_addr}");
    if report.listener_present {
        let pid_text = report
            .listener_pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!("listener {sequencer_url}: reachable (pid={pid_text})");
    } else {
        println!("listener {sequencer_url}: not reachable");
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

fn cmd_localnet_logs(log_path: &Path, tail: usize, json: bool) -> DynResult<()> {
    if !log_path.exists() {
        if json {
            print_logs_json(log_path, false, tail, Vec::new())?;
        } else {
            println!("log file does not exist yet: {}", log_path.display());
        }
        return Ok(());
    }

    let content = fs::read_to_string(log_path)
        .with_context(|| format!("failed to read log file {}", log_path.display()))?;

    // Treat a whitespace-only log as empty in BOTH modes. Without this, a log
    // containing only newlines yields `content.lines() == [""]`, so JSON would
    // report a non-empty `lines` array — contradicting the LocalnetLogsReport
    // contract (empty when the log is empty) and the plain-text branch below.
    if content.trim().is_empty() {
        if json {
            return print_logs_json(log_path, true, tail, Vec::new());
        }
        println!("log file is empty: {}", log_path.display());
        return Ok(());
    }

    let all_lines: Vec<&str> = content.lines().collect();
    let start = all_lines.len().saturating_sub(tail);
    let tail_lines = &all_lines[start..];

    if json {
        let lines = tail_lines.iter().map(|l| l.to_string()).collect();
        return print_logs_json(log_path, true, tail, lines);
    }

    for line in tail_lines {
        println!("{line}");
    }

    Ok(())
}

fn print_logs_json(
    log_path: &Path,
    exists: bool,
    tail: usize,
    lines: Vec<String>,
) -> DynResult<()> {
    let report = LocalnetLogsReport {
        log_path: log_path.display().to_string(),
        exists,
        tail,
        lines,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
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

/// Produce a sequencer config patched for scaffold's localnet — port set to
/// the project's configured port and `max_block_size` widened so the bundled
/// deploy flow fits in a single block — and return the absolute path to the
/// patched file. The pinned LEZ version does not accept `--port` as a CLI flag
/// — it reads everything from the config file passed as its first argument.
///
/// The patched copy is written under `dest_dir` (the project's
/// `.scaffold/state/`), **not** back into the vendored LEZ checkout. Writing
/// into the vendored repo would silently break three invariants the rest of
/// scaffold relies on:
///   1. `git_clean(lez)` would always report dirty, disabling the
///      `AutoRecloneIfClean` safety net in `reconcile_repo_source` for cache
///      repos.
///   2. A `git checkout` during a pin bump would discard the patch, leaving
///      the next `localnet start` running against unmodified upstream config
///      until scaffold re-patches.
///   3. `git status` in the LEZ tree would be permanently dirty after the
///      first `localnet start`, hiding genuine local edits.
///
/// Block-size bump: the upstream debug config caps `max_block_size` at 1 MiB,
/// which a `lgs deploy` of the default template (5 risc0 guest ELFs ≈ 360 KiB
/// each) overflows on the second block — and the pinned sequencer crashes
/// rather than carrying the deferred tx forward. Until LEZ stops aborting on
/// deferral, scaffold widens the limit so the documented first-success path
/// fits in a single block.
fn prepare_sequencer_config(lez: &Path, dest_dir: &Path, port: u16) -> DynResult<PathBuf> {
    let src_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
    let text = fs::read_to_string(&src_path)
        .with_context(|| format!("failed to read {}", src_path.display()))?;
    let mut doc: Value =
        serde_json::from_str(&text).context("failed to parse sequencer_config.json")?;

    let Some(obj) = doc.as_object_mut() else {
        bail!(
            "sequencer_config.json is not a JSON object: {}",
            src_path.display()
        );
    };
    obj.insert("port".to_string(), Value::Number(port.into()));
    obj.insert(
        "max_block_size".to_string(),
        Value::String("8 MiB".to_string()),
    );

    fs::create_dir_all(dest_dir)
        .with_context(|| format!("failed to create {}", dest_dir.display()))?;
    let dest_path = dest_dir.join("sequencer_config.json");
    let updated = serde_json::to_string_pretty(&doc).context("failed to serialize config")?;
    fs::write(&dest_path, format!("{updated}\n"))
        .with_context(|| format!("failed to write {}", dest_path.display()))?;
    Ok(dest_path)
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

#[allow(clippy::too_many_arguments)]
fn cmd_localnet_reset_dry_run(
    project: &Project,
    lez: &Path,
    state_path: &Path,
    log_path: &Path,
    localnet_addr: &str,
    localnet_port: u16,
    reset_wallet: bool,
    verify_timeout_sec: u64,
    sequencer_bin: &Path,
) -> DynResult<()> {
    // `lez` and `sequencer_bin` come out of the cache-root resolver, which can
    // return either an absolute path (env override, explicit absolute
    // `[repos.lez].path`, or scaffold default cache layer) or a path relative
    // to `project.root` (the portable default for vendored / new projects).
    // State / wallet paths are already joined onto `project.root`. Pass
    // every lez-derived path through `abs` so the dry-run output stays
    // copy-pasteable into a shell regardless of which case applied.
    let abs = |p: &Path| -> PathBuf {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            project.root.join(p)
        }
    };
    let rocksdb_path = abs(&lez.join("rocksdb"));
    let sequencer_bin_abs = abs(sequencer_bin);

    println!("dry-run: localnet reset (no changes made)");
    println!(
        "planned: stop sequencer if tracked (state file: {})",
        state_path.display()
    );
    println!(
        "planned: delete sequencer DB at {} (exists: {})",
        rocksdb_path.display(),
        rocksdb_path.exists()
    );
    if reset_wallet {
        let wallet_path = project.root.join(&project.config.wallet_home_dir);
        let wallet_state = wallet_state_path(&project.root);
        println!(
            "planned: delete wallet home at {} (exists: {})",
            wallet_path.display(),
            wallet_path.exists()
        );
        println!(
            "planned: delete wallet state at {} (exists: {})",
            wallet_state.display(),
            wallet_state.exists()
        );
    } else {
        println!("planned: preserve wallet home (pass --reset-wallet to also remove wallet)");
    }
    println!(
        "planned: delete localnet state file {} (exists: {})",
        state_path.display(),
        state_path.exists()
    );
    println!(
        "planned: start sequencer (log: {}), then verify block production (timeout {}s)",
        log_path.display(),
        verify_timeout_sec
    );
    if !sequencer_bin_abs.exists() {
        println!(
            "warning: missing sequencer binary at {}; a real reset would fail before any destructive step",
            sequencer_bin_abs.display()
        );
    } else {
        println!(
            "prerequisite ok: sequencer binary exists at {}",
            sequencer_bin_abs.display()
        );
    }
    let state = read_localnet_state(state_path).unwrap_or_default();
    if let Some(pid) = state.sequencer_pid {
        println!(
            "current tracked sequencer pid: {pid} (running: {})",
            pid_running(pid)
        );
    } else {
        println!("current tracked sequencer pid: none");
    }
    if port_open(localnet_addr) {
        // Foreign-listener case: nothing tracked but the port is held. The
        // real reset stops nothing, then `wait_for_port_free` times out and
        // returns `ResetError::ForeignListener` before any cleanup. Surface
        // that here so the dry-run doesn't silently imply a clean run.
        if state.sequencer_pid.is_none() {
            println!(
                "warning: foreign listener on {localnet_addr} (port {localnet_port}); a real reset would abort with foreign-listener error before any destructive step"
            );
        } else {
            println!("note: listener currently on {localnet_addr} (port {localnet_port})");
        }
    } else {
        println!("note: no listener on {localnet_addr} currently");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_localnet_reset(
    project: &Project,
    lez: &Path,
    state_path: &Path,
    log_path: &Path,
    localnet_addr: &str,
    dry_run: bool,
    yes: bool,
    reset_wallet: bool,
    verify_timeout_sec: u64,
) -> DynResult<()> {
    let localnet_port = project.config.localnet.port;

    let sequencer_bin = lez.join(SEQUENCER_BIN_REL_PATH);
    if dry_run {
        return cmd_localnet_reset_dry_run(
            project,
            lez,
            state_path,
            log_path,
            localnet_addr,
            localnet_port,
            reset_wallet,
            verify_timeout_sec,
            &sequencer_bin,
        );
    }

    if !yes {
        let wallet_hint = if reset_wallet {
            " (with --reset-wallet, also deletes wallet keypairs irrecoverably)"
        } else {
            ""
        };
        bail!(
            "localnet reset is destructive: it wipes the sequencer chain DB{wallet_hint}.\n\
             Pass --yes to confirm, or --dry-run to preview the plan first.\n\
             Examples:\n  \
             logos-scaffold localnet reset --dry-run\n  \
             logos-scaffold localnet reset --yes\n  \
             logos-scaffold localnet reset --reset-wallet --yes"
        );
    }

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

/// Poll `pid` until it is no longer running (exited or zombie), or `timeout`
/// elapses. Returns `true` if the process exited within the deadline.
fn wait_for_pid_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !pid_running(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
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

/// Resolve the r0vm binary path from the rzup installation that matches the LEZ risc0 version.
///
/// Reads the risc0-zkvm version from the LEZ Cargo.lock, then looks for an exact-version
/// rzup-managed extension dir: ~/.risc0/extensions/v<version>-cargo-risczero-<arch>-<os>/r0vm.
/// Returns None (without guessing) if the version cannot be determined or the exact path
/// does not exist. The caller should error with a clear diagnostic if None is returned.
fn find_r0vm_path_for_lez(lez: &std::path::Path) -> Option<std::path::PathBuf> {
    // Read risc0-zkvm version from LEZ Cargo.lock
    let lockfile = lez.join("Cargo.lock");
    let lock_content = std::fs::read_to_string(&lockfile).ok()?;
    let risc0_version = parse_risc0_version(&lock_content)?;

    // Determine platform triple (rzup naming convention)
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "aarch64"
    };
    let os = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else {
        "unknown-linux-gnu"
    };
    let ext_name = format!("v{risc0_version}-cargo-risczero-{arch}-{os}");

    let home = std::env::var("HOME").ok()?;
    let r0vm = std::path::Path::new(&home)
        .join(".risc0")
        .join("extensions")
        .join(&ext_name)
        .join("r0vm");

    if r0vm.exists() {
        Some(r0vm)
    } else {
        None
    }
}

/// Extract risc0-zkvm version from Cargo.lock content.
fn parse_risc0_version(lock_content: &str) -> Option<String> {
    let mut in_risc0_zkvm = false;
    for line in lock_content.lines() {
        let line = line.trim();
        if line == r#"name = "risc0-zkvm""# {
            in_risc0_zkvm = true;
            continue;
        }
        if in_risc0_zkvm {
            if let Some(version) = line
                .strip_prefix("version = \"")
                .and_then(|s| s.strip_suffix('"'))
            {
                return Some(version.to_string());
            }
            if line.starts_with("name =") {
                break;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use std::net::TcpListener;
    use std::time::Duration;

    use super::{
        prepare_sequencer_config, reset_cleanup, verify_block_production, wait_for_pid_exit,
        wait_for_port_free,
    };
    use crate::commands::wallet_support::wallet_state_path;
    use crate::constants::SEQUENCER_CONFIG_REL_PATH;
    use crate::error::ResetError;
    use crate::model::{
        Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, Project, RepoRef, RunConfig,
    };
    use crate::repo::git_clean;
    use std::process::Command;

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

    /// Regression test for #114. `prepare_sequencer_config` must write the
    /// patched copy under `dest_dir` (project-owned state) and leave the
    /// vendored LEZ checkout byte-identical — otherwise `git_clean(lez)`
    /// reports dirty and the `AutoRecloneIfClean` safety net in
    /// `reconcile_repo_source` is silently disabled for cache repos.
    #[test]
    fn prepare_sequencer_config_does_not_dirty_vendored_lez_repo() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let state_dir = temp.path().join(".scaffold/state");

        // Build a real git repo under `lez` with the sequencer config committed
        // at its real relative path, so `git status --porcelain` is initially
        // empty and we can see any write-back to that file as a dirty diff.
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, "{\n  \"port\": 3040\n}\n").unwrap();

        let git_init = Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .current_dir(&lez)
            .status()
            .unwrap();
        assert!(git_init.success(), "git init failed");
        // Local identity so `git commit` doesn't require system-level config.
        // Disable GPG signing locally so an environment with `commit.gpgsign
        // = true` + an unavailable signing key (sandboxed CI containers,
        // etc.) doesn't block the commit and mask this regression test.
        for (k, v) in [
            ("user.email", "t@example.com"),
            ("user.name", "test"),
            ("commit.gpgsign", "false"),
            ("tag.gpgsign", "false"),
        ] {
            Command::new("git")
                .args(["config", k, v])
                .current_dir(&lez)
                .status()
                .unwrap();
        }
        Command::new("git")
            .args(["add", "."])
            .current_dir(&lez)
            .status()
            .unwrap();
        let commit = Command::new("git")
            .args(["commit", "--quiet", "-m", "seed"])
            .current_dir(&lez)
            .status()
            .unwrap();
        assert!(commit.success(), "git commit failed");

        assert!(
            git_clean(&lez).unwrap(),
            "test precondition: seeded lez tree should be clean"
        );

        let dest = prepare_sequencer_config(&lez, &state_dir, 4040).unwrap();

        assert_eq!(
            dest,
            state_dir.join("sequencer_config.json"),
            "patched config must land under the project state dir, not the lez repo"
        );

        let dest_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
        assert_eq!(dest_json["port"], serde_json::json!(4040));
        assert_eq!(
            dest_json["max_block_size"],
            serde_json::json!("8 MiB"),
            "patched copy should carry the widened max_block_size override"
        );

        let src_after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(
            src_after["port"],
            serde_json::json!(3040),
            "vendored sequencer_config.json must be left untouched"
        );
        assert!(
            src_after.get("max_block_size").is_none(),
            "vendored sequencer_config.json must be left untouched (no max_block_size injected)"
        );

        assert!(
            git_clean(&lez).unwrap(),
            "lez tree must remain clean after prepare_sequencer_config"
        );
    }

    #[test]
    fn wait_for_pid_exit_returns_true_after_process_dies() {
        // Spawn a long sleep, then SIGKILL it. wait_for_pid_exit must observe
        // the exit within the timeout — this is the success path that
        // `cmd_localnet_stop` relies on before deleting the state file.
        let mut child = std::process::Command::new("sleep")
            .arg("10")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        child.kill().expect("kill sleep");
        let exited = wait_for_pid_exit(pid, Duration::from_secs(2));
        let _ = child.wait();
        assert!(exited, "expected pid {pid} to exit within 2s after SIGKILL");
    }

    #[test]
    fn wait_for_pid_exit_returns_false_when_pid_stays_alive() {
        // While the child is still running, wait_for_pid_exit must time out
        // rather than report success — this is the failure path that
        // `cmd_localnet_stop` relies on to preserve the state file.
        let mut child = std::process::Command::new("sleep")
            .arg("10")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        let exited = wait_for_pid_exit(pid, Duration::from_millis(200));
        let _ = child.kill();
        let _ = child.wait();
        assert!(!exited, "expected timeout while pid {pid} was still alive");
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
