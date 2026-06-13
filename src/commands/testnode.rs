use std::path::PathBuf;

use anyhow::Context;

use crate::model::{CheckStatus, Project};
use crate::process::EchoGuard;
use crate::project::{load_project, load_project_at, resolve_cache_root};
use crate::testnode::blocks::{BlockInfo, BlockRange, ClockReadMode, ClockSnapshot};
use crate::testnode::client::{
    SubmitOutcome, TestNodeClient, TransactionBytes, TransactionOutcome, WaitOptions,
};
use crate::testnode::pins::{
    doctor_test_node, prepare_test_node, resolve_test_node_pins, PinOverrides,
};
use crate::testnode::{
    acquire_run_slot, resolve_node_dir, run_with_test_node, stop_node_in_dir, PortSelection,
    TestNode, TestNodeConfig,
};
use crate::DynResult;

#[derive(Debug, Clone)]
pub(crate) enum TestNodeAction {
    Pins {
        project: Option<PathBuf>,
        overrides: PinOverrides,
        json: bool,
    },
    Prepare {
        project: Option<PathBuf>,
        overrides: PinOverrides,
        cache_root: Option<PathBuf>,
        json: bool,
    },
    Doctor {
        project: Option<PathBuf>,
        json: bool,
    },
    Start {
        project: Option<PathBuf>,
        state: Option<PathBuf>,
        port: u16,
        work_dir: Option<PathBuf>,
        preserve_work_dir: bool,
        timeout_sec: u64,
        json: bool,
    },
    Status {
        project: Option<PathBuf>,
        node: String,
        json: bool,
    },
    Stop {
        project: Option<PathBuf>,
        node: String,
        preserve_work_dir: bool,
    },
    Run {
        project: Option<PathBuf>,
        state: Option<PathBuf>,
        serial: bool,
        parallel: Option<usize>,
        timeout_sec: u64,
        command: Vec<String>,
    },
    TxSubmit {
        url: String,
        file: PathBuf,
        encoding: TxEncoding,
        json: bool,
    },
    TxWait {
        url: String,
        hash: String,
        after_block: Option<u64>,
        timeout_sec: u64,
        json: bool,
    },
    TxSubmitAndWait {
        url: String,
        file: PathBuf,
        encoding: TxEncoding,
        timeout_sec: u64,
        json: bool,
    },
    BlocksHead {
        url: String,
        json: bool,
    },
    BlocksRange {
        url: String,
        from: u64,
        to: u64,
        json: bool,
    },
    BlocksWait {
        url: String,
        after: u64,
        count: u64,
        timeout_sec: u64,
        json: bool,
    },
    ClockRead {
        url: String,
        json: bool,
    },
    ClockWaitStable {
        url: String,
        samples: u32,
        timeout_sec: u64,
        json: bool,
    },
}

fn print_block_info(info: &BlockInfo) {
    println!("block {}", info.block_id);
    println!("  timestamp: {}", info.timestamp);
    println!("  transactions: {}", info.transaction_count);
    println!("  is_genesis: {}", info.is_genesis);
    println!("  has_clock_transaction: {}", info.has_clock_transaction);
    println!("  has_user_transactions: {}", info.has_user_transactions);
    for tx in &info.transactions {
        println!(
            "  tx {} ({:?}{})",
            tx.hash,
            tx.kind,
            if tx.is_clock { ", clock" } else { "" }
        );
    }
    if !info.fully_parsed {
        println!(
            "  note: block contains privacy-preserving transaction(s); per-tx list is partial"
        );
    }
}

fn print_clock_snapshot(snapshot: &ClockSnapshot) {
    println!("read_block_id: {}", snapshot.read_block_id);
    for account in &snapshot.accounts {
        println!("clock account {}", account.account_id);
        println!("  balance: {}", account.balance);
        println!("  nonce: {}", account.nonce);
        match (account.block_id, account.timestamp) {
            (Some(block_id), Some(timestamp)) => {
                println!("  block_id: {block_id}");
                println!("  timestamp: {timestamp}");
            }
            _ => println!("  (clock not ticked yet — empty account data)"),
        }
    }
}

/// On-disk encoding of a transaction file passed to `tx submit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TxEncoding {
    /// File contains base64 text of the borsh bytes (the wire encoding).
    BorshBase64,
    /// File contains raw borsh bytes.
    Borsh,
}

fn read_transaction_file(file: &PathBuf, encoding: TxEncoding) -> DynResult<TransactionBytes> {
    match encoding {
        TxEncoding::BorshBase64 => {
            let text = std::fs::read_to_string(file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            Ok(TransactionBytes::borsh_base64(&text)?)
        }
        TxEncoding::Borsh => {
            let bytes = std::fs::read(file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            Ok(TransactionBytes::borsh(bytes))
        }
    }
}

/// Print a terminal outcome (JSON or human form) and convert non-committed
/// outcomes into a non-zero exit.
fn finish_tx_outcome(outcome: &TransactionOutcome, json: bool) -> DynResult<()> {
    if json {
        println!("{}", serde_json::to_string(outcome)?);
    } else {
        match outcome {
            TransactionOutcome::Committed { tx_hash, block } => {
                println!("committed");
                println!("  tx_hash: {tx_hash}");
                println!("  block_id: {}", block.block_id);
                println!("  timestamp: {}", block.timestamp);
            }
            TransactionOutcome::Rejected {
                tx_hash,
                phase,
                reason,
                observed_after_block_id,
            } => {
                println!("rejected ({phase:?})");
                if let Some(tx_hash) = tx_hash {
                    println!("  tx_hash: {tx_hash}");
                }
                if let Some(reason) = reason {
                    println!("  reason: {reason}");
                }
                if let Some(block) = observed_after_block_id {
                    println!("  observed_after_block_id: {block}");
                }
            }
            TransactionOutcome::Timeout {
                tx_hash,
                last_observed_block_id,
            } => {
                println!("timeout");
                println!("  tx_hash: {tx_hash}");
                println!("  last_observed_block_id: {last_observed_block_id}");
            }
            TransactionOutcome::TransportError { operation, message } => {
                println!("transport_error");
                println!("  operation: {operation}");
                println!("  message: {message}");
            }
            TransactionOutcome::WireMismatch {
                returned_hash,
                submitted_tx,
                echoed_tx,
                ..
            } => {
                println!("wire_mismatch");
                println!("  returned_hash: {returned_hash}");
                println!("  submitted_tx: {submitted_tx}");
                println!("  echoed_tx: {echoed_tx}");
            }
        }
    }

    if !outcome.is_committed() {
        // The JSON object on stdout is the structured contract; the process
        // exit code is the pass/fail signal for harnesses.
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn cmd_test_node(action: TestNodeAction) -> DynResult<()> {
    match action {
        TestNodeAction::Pins {
            project,
            overrides,
            json,
        } => {
            // Keep `--json` stdout a single JSON object: suppress the `$ git …`
            // echoes the pin resolver emits while shelling out.
            let _echo = json.then(EchoGuard::suppress);
            let project = load_selected_project(project.as_deref())?;
            let pins = resolve_test_node_pins(&project, &overrides)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&pins)?);
            } else {
                println!(
                    "lez source: {} ({:?})",
                    pins.lez_source, pins.lez_source_origin
                );
                println!("lez ref: {} ({:?})", pins.lez_ref, pins.lez_ref_origin);
                match &pins.lez_resolved_commit {
                    Some(commit) => println!("lez resolved commit: {commit}"),
                    None => println!("lez resolved commit: <checkout not materialised>"),
                }
                println!(
                    "lez checkout: {} ({:?})",
                    pins.lez_checkout.display(),
                    pins.checkout_ownership
                );
                println!("sequencer binary: {}", pins.sequencer_binary.display());
                println!(
                    "circuits version: {} ({:?})",
                    pins.circuits_version, pins.circuits_version_origin
                );
                println!("circuits path: {}", pins.circuits_path.display());
            }
            Ok(())
        }
        TestNodeAction::Prepare {
            project,
            overrides,
            cache_root,
            json,
        } => {
            // `--json`: drop the `$ …` echoes (cargo still streams build
            // progress to stderr, so stdout stays the JSON object).
            let _echo = json.then(EchoGuard::suppress);
            let project = load_selected_project(project.as_deref())?;
            let prepared = prepare_test_node(&project, &overrides, cache_root.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&prepared)?);
            } else {
                println!("test-node prerequisites ready");
                println!(
                    "  lez checkout: {} ({:?})",
                    prepared.checkout.display(),
                    prepared.checkout_ownership
                );
                println!("  lez commit: {}", prepared.lez_commit);
                println!(
                    "  sequencer binary: {}",
                    prepared.sequencer_binary.display()
                );
                println!(
                    "  circuits: v{} at {}",
                    prepared.circuits_version,
                    prepared.circuits_path.display()
                );
            }
            Ok(())
        }
        TestNodeAction::Doctor { project, json } => {
            let _echo = json.then(EchoGuard::suppress);
            let project = load_selected_project(project.as_deref())?;
            let report = doctor_test_node(&project)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                for check in &report.checks {
                    let label = match check.status {
                        CheckStatus::Pass => "PASS",
                        CheckStatus::Warn => "WARN",
                        CheckStatus::Fail => "FAIL",
                    };
                    println!("{label} {} — {}", check.name, check.detail);
                    if let Some(remediation) = &check.remediation {
                        println!("     fix: {remediation}");
                    }
                }
                println!(
                    "test-node doctor: {}",
                    if report.ok { "ok" } else { "failing" }
                );
            }
            if !report.ok {
                anyhow::bail!("test-node doctor reported failing checks");
            }
            Ok(())
        }
        TestNodeAction::Start {
            project,
            state,
            port,
            work_dir,
            preserve_work_dir,
            timeout_sec,
            json,
        } => {
            // `--json`: suppress the `$ ./sequencer_service …` spawn echo so
            // stdout is the node's JSON connection record only.
            let _echo = json.then(EchoGuard::suppress);
            let project = load_selected_project(project.as_deref())?;
            let config = TestNodeConfig {
                state,
                port: if port == 0 {
                    PortSelection::Auto
                } else {
                    PortSelection::Fixed(port)
                },
                work_dir,
                preserve_work_dir,
                timeout_sec,
            };
            // Detach: the CLI exits but the node keeps running until
            // `test-node stop`.
            let node = TestNode::start(&project, &config)?;
            let info = node.detach();
            if json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                let id = info
                    .state_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| info.state_dir.display().to_string());
                println!("test node ready");
                println!("  node: {id}");
                println!("  pid: {}", info.pid);
                println!("  rpc_url: {}", info.rpc_url);
                println!("  state_dir: {}", info.state_dir.display());
                println!("  log: {}", info.log_path.display());
                println!("  genesis_block_id: {}", info.genesis_block_id);
                println!("  block_height: {}", info.block_height);
                println!("Stop with: lgs test-node stop --node {id}");
            }
            Ok(())
        }
        TestNodeAction::Status {
            project,
            node,
            json,
        } => {
            let project_root = selected_project_root(project.as_deref())?;
            let node_dir = resolve_node_dir(&project_root, &node)?;
            let handle = TestNode::from_state_dir(&node_dir)?;
            let status = handle.status();
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("healthy: {}", status.healthy);
                println!("running: {}", status.running);
                println!("rpc_url: {}", status.rpc_url);
                println!("pid: {}", status.pid);
                match status.block_height {
                    Some(height) => println!("block_height: {height}"),
                    None => println!("block_height: unreachable"),
                }
                println!("state_dir: {}", status.state_dir.display());
                println!("log: {}", status.log_path.display());
            }
            if !status.healthy {
                anyhow::bail!("test node is not healthy");
            }
            Ok(())
        }
        TestNodeAction::Stop {
            project,
            node,
            preserve_work_dir,
        } => {
            let project_root = selected_project_root(project.as_deref())?;
            let node_dir = resolve_node_dir(&project_root, &node)?;
            let preserve_override = preserve_work_dir.then_some(true);
            stop_node_in_dir(&node_dir, preserve_override)?;
            println!("test node stopped");
            if preserve_work_dir {
                println!("  state preserved at {}", node_dir.display());
            }
            Ok(())
        }
        TestNodeAction::Run {
            project,
            state,
            serial,
            parallel,
            timeout_sec,
            command,
        } => {
            let project = load_selected_project(project.as_deref())?;

            // --serial caps cross-process node creation at 1; --parallel <N>
            // at N. Held for the whole child run so the node count is the
            // resource being limited, not just startup.
            let max_parallel = if serial { Some(1) } else { parallel };
            let _slot = match max_parallel {
                Some(max) if max == 0 => anyhow::bail!("--parallel must be at least 1"),
                Some(max) => {
                    let (cache_root, _) = resolve_cache_root(&project)?;
                    Some(acquire_run_slot(&cache_root, max)?)
                }
                None => None,
            };

            let config = TestNodeConfig {
                state,
                port: PortSelection::Auto,
                work_dir: None,
                preserve_work_dir: false,
                timeout_sec,
            };
            let status = run_with_test_node(&project, &config, &command)?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
            Ok(())
        }
        TestNodeAction::TxSubmit {
            url,
            file,
            encoding,
            json,
        } => {
            let tx = read_transaction_file(&file, encoding)?;
            let client = TestNodeClient::new(url);
            let outcome = client.submit(&tx);
            if json {
                println!("{}", serde_json::to_string(&outcome)?);
            } else {
                match &outcome {
                    SubmitOutcome::Submitted { tx_hash } => {
                        println!("submitted");
                        println!("  tx_hash: {tx_hash}");
                    }
                    SubmitOutcome::Rejected { phase, reason } => {
                        println!("rejected ({phase:?})");
                        println!("  reason: {reason}");
                    }
                    SubmitOutcome::TransportError { operation, message } => {
                        println!("transport_error");
                        println!("  operation: {operation}");
                        println!("  message: {message}");
                    }
                }
            }
            if !matches!(outcome, SubmitOutcome::Submitted { .. }) {
                std::process::exit(1);
            }
            Ok(())
        }
        TestNodeAction::TxWait {
            url,
            hash,
            after_block,
            timeout_sec,
            json,
        } => {
            let client = TestNodeClient::new(url);
            let options = WaitOptions {
                after_block,
                timeout: std::time::Duration::from_secs(timeout_sec),
                ..WaitOptions::default()
            };
            let outcome = client.wait(&hash, None, &options);
            finish_tx_outcome(&outcome, json)
        }
        TestNodeAction::TxSubmitAndWait {
            url,
            file,
            encoding,
            timeout_sec,
            json,
        } => {
            let tx = read_transaction_file(&file, encoding)?;
            let client = TestNodeClient::new(url);
            let options = WaitOptions {
                timeout: std::time::Duration::from_secs(timeout_sec),
                ..WaitOptions::default()
            };
            let outcome = client.submit_and_wait(&tx, &options);
            finish_tx_outcome(&outcome, json)
        }
        TestNodeAction::BlocksHead { url, json } => {
            let client = TestNodeClient::new(url);
            let head = client.block_head()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&head)?);
            } else {
                print_block_info(&head);
            }
            Ok(())
        }
        TestNodeAction::BlocksRange {
            url,
            from,
            to,
            json,
        } => {
            let client = TestNodeClient::new(url);
            let blocks = client.blocks(BlockRange { from, to })?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "blocks": blocks }))?
                );
            } else {
                for info in &blocks {
                    print_block_info(info);
                }
            }
            Ok(())
        }
        TestNodeAction::BlocksWait {
            url,
            after,
            count,
            timeout_sec,
            json,
        } => {
            let client = TestNodeClient::new(url);
            let blocks =
                client.wait_blocks(after, count, std::time::Duration::from_secs(timeout_sec))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "blocks": blocks }))?
                );
            } else {
                for info in &blocks {
                    print_block_info(info);
                }
            }
            Ok(())
        }
        TestNodeAction::ClockRead { url, json } => {
            let client = TestNodeClient::new(url);
            let snapshot = client.clock_snapshot(ClockReadMode::Latest)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                print_clock_snapshot(&snapshot);
            }
            Ok(())
        }
        TestNodeAction::ClockWaitStable {
            url,
            samples,
            timeout_sec,
            json,
        } => {
            let client = TestNodeClient::new(url);
            let snapshot = client.clock_snapshot(ClockReadMode::Stable {
                samples,
                timeout: std::time::Duration::from_secs(timeout_sec),
            })?;
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                print_clock_snapshot(&snapshot);
            }
            Ok(())
        }
    }
}

fn load_selected_project(project: Option<&std::path::Path>) -> DynResult<Project> {
    match project {
        Some(root) => load_project_at(root),
        None => load_project(),
    }
}

fn selected_project_root(project: Option<&std::path::Path>) -> DynResult<PathBuf> {
    Ok(load_selected_project(project)
        .context("test-node commands need a project (pass --project <root> or run inside one)")?
        .root)
}
