use std::process::Command;

use anyhow::{anyhow, bail, Context};
use serde_json::json;

use crate::process::{render_command, run_forwarded, run_with_stdin, EchoGuard};
use crate::project::load_project;
use crate::DynResult;

use super::wallet_support::{
    default_sequencer_http_url_for_project, extract_tx_identifier, is_already_initialized_failure,
    is_confirmation_timeout_failure, is_connectivity_failure, is_uninitialized_account_output,
    load_wallet_runtime, read_default_wallet_address, resolve_wallet_address,
    sequencer_unreachable_hint, summarize_command_failure, wallet_password, wallet_state_path,
    write_default_wallet_address,
};

/// Result of a wallet topup attempt. `cmd_run` distinguishes the
/// confirmation-timeout case so the pipeline can bail before deploy
/// rather than continue with uncertain funding. Standalone `wallet topup`
/// treats both as success (matching prior behavior).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TopupOutcome {
    Success,
    ConfirmationTimeout { message: String },
}

#[derive(Debug, Clone)]
pub(crate) enum WalletAction {
    List {
        long: bool,
        json: bool,
    },
    Proxy {
        args: Vec<String>,
    },
    Topup {
        address: Option<String>,
        dry_run: bool,
        json: bool,
    },
    DefaultSet {
        address: String,
    },
}

pub(crate) fn cmd_wallet(action: WalletAction) -> DynResult<()> {
    let project = load_project()?;

    match action {
        WalletAction::List { long, json } => cmd_wallet_list(&project, long, json),
        WalletAction::Proxy { args } => cmd_wallet_proxy(&project, &args),
        WalletAction::Topup {
            address,
            dry_run,
            json,
        } => cmd_wallet_topup(&project, address, dry_run, json),
        WalletAction::DefaultSet { address } => cmd_wallet_default_set(&project, &address),
    }
}

fn cmd_wallet_list(project: &crate::model::Project, long: bool, json: bool) -> DynResult<()> {
    let wallet = load_wallet_runtime(project)?;

    let mut command = Command::new(&wallet.wallet_binary);
    command
        .env(
            "NSSA_WALLET_HOME_DIR",
            wallet.wallet_home.as_os_str().to_string_lossy().to_string(),
        )
        .arg("account")
        .arg("list");

    if long {
        command.arg("--long");
    }

    if json {
        // Capture the inner wallet output and re-emit a structured envelope so
        // consumers can read `exit_code` instead of guessing success from text.
        // `accounts` is the best-effort line split; `stdout`/`stderr` keep the
        // raw output for callers that need the wallet's own formatting.
        // `command` is the actual rendered invocation (binary + args, so it
        // reflects `--long`) rather than a hard-coded label.
        let rendered_command = render_command(&command);
        let output = command
            .output()
            .context("failed to execute wallet list command")?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        // Always emit a number: a wallet killed by a signal has no exit code,
        // so map that to the shell convention `128 + signal` (else -1) rather
        // than letting `exit_code` serialize as `null`.
        let exit_code = output.status.code().unwrap_or_else(|| {
            use std::os::unix::process::ExitStatusExt;
            output.status.signal().map(|s| 128 + s).unwrap_or(-1)
        });
        let accounts: Vec<&str> = stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let report = json!({
            "command": rendered_command,
            "exit_code": exit_code,
            "accounts": accounts,
            "stdout": stdout,
            "stderr": stderr,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        if !output.status.success() {
            bail!("wallet account list failed");
        }
        return Ok(());
    }

    run_forwarded(&mut command, "wallet account list")
        .context("failed to execute wallet list command")?;

    Ok(())
}

fn cmd_wallet_proxy(project: &crate::model::Project, args: &[String]) -> DynResult<()> {
    if args.is_empty() {
        bail!("wallet passthrough requires at least one argument after `--`. Example: `logos-scaffold wallet -- account list`");
    }

    let wallet = load_wallet_runtime(project)?;

    let mut command = Command::new(&wallet.wallet_binary);
    command.env(
        "NSSA_WALLET_HOME_DIR",
        wallet.wallet_home.as_os_str().to_string_lossy().to_string(),
    );
    for arg in args {
        command.arg(arg);
    }

    run_forwarded(&mut command, "wallet passthrough command")
        .context("wallet passthrough command failed")?;

    Ok(())
}

fn cmd_wallet_topup(
    project: &crate::model::Project,
    address: Option<String>,
    dry_run: bool,
    json: bool,
) -> DynResult<()> {
    match cmd_wallet_topup_inner(project, address, dry_run, json)? {
        TopupOutcome::Success => Ok(()),
        TopupOutcome::ConfirmationTimeout { message } => bail!("{message}"),
    }
}

/// Print the structured error object to stdout. Used in `--json` mode right
/// before a `bail!` so machine consumers get a categorized `reason` instead of
/// substring-matching the wallet's stderr — and the same `address` / `method`
/// / `network` context the success / dry_run / pending objects carry, so error
/// handling isn't missing what was attempted.
fn emit_topup_error_json(reason: &str, message: &str, address: &str, network: &str) {
    let report = json!({
        "status": "error",
        "reason": reason,
        "address": address,
        "method": "pinata faucet claim",
        "network": network,
        // Always present (null here) so the object shape is stable across
        // every `status` and consumers don't need per-status conditional keys.
        "tx": serde_json::Value::Null,
        "message": message,
    });
    if let Ok(text) = serde_json::to_string_pretty(&report) {
        println!("{text}");
    }
}

/// Build the error for a topup step that failed because the sequencer was
/// unreachable. In JSON mode it first emits the structured `connectivity`
/// error; either way the returned error carries `message` plus the
/// sequencer-unreachable hint. Used by all three topup steps (preflight, init,
/// pinata claim), which differ only in `message`.
fn connectivity_error(
    json: bool,
    message: String,
    address: &str,
    sequencer_addr: &str,
) -> anyhow::Error {
    if json {
        emit_topup_error_json("connectivity", &message, address, sequencer_addr);
    }
    anyhow!("{message}\n{}", sequencer_unreachable_hint(sequencer_addr))
}

pub(crate) fn cmd_wallet_topup_inner(
    project: &crate::model::Project,
    address: Option<String>,
    dry_run: bool,
    json: bool,
) -> DynResult<TopupOutcome> {
    // In JSON mode, keep stdout clean: suppress the `$ <cmd>` echoes so the
    // only thing on stdout is the structured object we emit at the end.
    let _echo_guard = json.then(EchoGuard::suppress);
    let wallet = load_wallet_runtime(project)?;
    let default_address = read_default_wallet_address(&project.root)?;
    let resolved_to = resolve_wallet_address(address.as_deref(), default_address.as_deref())?;
    let sequencer_addr = wallet
        .sequencer_addr
        .unwrap_or_else(|| default_sequencer_http_url_for_project(project));
    let wallet_home = wallet.wallet_home.as_os_str().to_string_lossy().to_string();
    let password_input = format!("{}\n", wallet_password());

    let mut preflight_command = Command::new(&wallet.wallet_binary);
    preflight_command
        .env("NSSA_WALLET_HOME_DIR", &wallet_home)
        .arg("account")
        .arg("get")
        .arg("--account-id")
        .arg(&resolved_to);

    let mut init_command = Command::new(&wallet.wallet_binary);
    init_command
        .env("NSSA_WALLET_HOME_DIR", &wallet_home)
        .arg("auth-transfer")
        .arg("init")
        .arg("--account-id")
        .arg(&resolved_to);

    let mut pinata_command = Command::new(&wallet.wallet_binary);
    pinata_command
        .env("NSSA_WALLET_HOME_DIR", &wallet_home)
        .arg("pinata")
        .arg("claim")
        .arg("--to")
        .arg(&resolved_to);

    if dry_run {
        if json {
            let report = json!({
                "status": "dry_run",
                "address": resolved_to,
                "method": "pinata faucet claim",
                "network": sequencer_addr,
                // Stable shape: no tx exists for a dry run, but keep the key.
                "tx": serde_json::Value::Null,
                "wallet_home": wallet_home,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(TopupOutcome::Success);
        }
        println!("dry-run: wallet topup command will not be executed");
        println!("NSSA_WALLET_HOME_DIR={wallet_home}");
        println!("$ {}", render_command(&preflight_command));
        println!("planned preflight: check destination wallet initialization");
        println!(
            "planned conditional step: run only if uninitialized -> {}",
            render_command(&init_command)
        );
        println!("$ {}", render_command(&pinata_command));
        println!("planned wallet: {resolved_to}");
        println!("planned method: pinata faucet claim");
        println!("planned network: local sequencer ({sequencer_addr})");
        return Ok(TopupOutcome::Success);
    }

    let preflight_output = run_with_stdin(preflight_command, password_input.clone())
        .context("failed to execute wallet topup preflight command")?;
    if !preflight_output.status.success() {
        let summary = summarize_command_failure(&preflight_output.stdout, &preflight_output.stderr);
        let combined = format!("{}\n{}", preflight_output.stdout, preflight_output.stderr);
        if is_connectivity_failure(&combined) {
            return Err(connectivity_error(
                json,
                format!("wallet topup failed during account preflight: {summary}"),
                &resolved_to,
                &sequencer_addr,
            ));
        }

        if json {
            emit_topup_error_json(
                "preflight",
                &format!("wallet topup failed while checking account initialization: {summary}"),
                &resolved_to,
                &sequencer_addr,
            );
        }
        bail!(
            "wallet topup failed while checking account initialization: {summary}\nHint: verify the destination with `logos-scaffold wallet -- account get --account-id {resolved_to}`."
        );
    }

    let preflight_combined = format!("{}\n{}", preflight_output.stdout, preflight_output.stderr);
    if is_uninitialized_account_output(&preflight_combined) {
        if !json {
            println!(
                "wallet topup preflight: destination is uninitialized; running auth-transfer init"
            );
        }
        let init_output = run_with_stdin(init_command, password_input.clone())
            .context("failed to execute wallet topup init command")?;

        if !init_output.status.success() {
            let summary = summarize_command_failure(&init_output.stdout, &init_output.stderr);
            let combined = format!("{}\n{}", init_output.stdout, init_output.stderr);
            if is_connectivity_failure(&combined) {
                return Err(connectivity_error(
                    json,
                    format!("wallet topup failed during account initialization: {summary}"),
                    &resolved_to,
                    &sequencer_addr,
                ));
            }
            if is_already_initialized_failure(&combined) {
                if !json {
                    println!("wallet topup preflight: destination already initialized; continuing");
                }
            } else {
                if json {
                    emit_topup_error_json(
                        "init",
                        &format!(
                            "wallet topup failed while initializing destination wallet: {summary}"
                        ),
                        &resolved_to,
                        &sequencer_addr,
                    );
                }
                bail!("wallet topup failed while initializing destination wallet: {summary}");
            }
        }
    }

    let output = run_with_stdin(pinata_command, password_input)
        .context("failed to execute wallet topup command")?;

    if !output.status.success() {
        let summary = summarize_command_failure(&output.stdout, &output.stderr);
        let combined = format!("{}\n{}", output.stdout, output.stderr);
        if is_connectivity_failure(&combined) {
            return Err(connectivity_error(
                json,
                format!("wallet topup failed: {summary}"),
                &resolved_to,
                &sequencer_addr,
            ));
        }
        if is_confirmation_timeout_failure(&combined) {
            let message = confirmation_timeout_message(
                &resolved_to,
                &sequencer_addr,
                &output.stdout,
                &output.stderr,
            );
            if json {
                let report = json!({
                    "status": "pending",
                    "address": resolved_to,
                    "method": "pinata faucet claim",
                    "network": sequencer_addr,
                    "tx": extract_tx_identifier(&output.stdout, &output.stderr),
                    "message": message,
                });
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            return Ok(TopupOutcome::ConfirmationTimeout { message });
        }
        if json {
            emit_topup_error_json(
                "failed",
                &format!("wallet topup failed: {summary}"),
                &resolved_to,
                &sequencer_addr,
            );
        }
        bail!(
            "wallet topup failed: {summary}\nHint: run `logos-scaffold wallet list` to inspect addresses, then retry with `--address` or set a default wallet."
        );
    }

    let tx = extract_tx_identifier(&output.stdout, &output.stderr);
    if json {
        let report = json!({
            "status": "success",
            "address": resolved_to,
            "method": "pinata faucet claim",
            "network": sequencer_addr,
            "tx": tx,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(TopupOutcome::Success);
    }

    println!("wallet topup complete");
    println!("  Address: {resolved_to}");
    println!("  Method: pinata faucet claim");
    println!("  Network: local sequencer ({sequencer_addr})");
    if let Some(tx) = tx {
        println!("  Tx: {tx}");
    }

    Ok(TopupOutcome::Success)
}

fn confirmation_timeout_message(
    resolved_to: &str,
    sequencer_addr: &str,
    stdout: &str,
    stderr: &str,
) -> String {
    // Submission reached the sequencer but confirmation didn't arrive before
    // the wallet binary's timeout. We genuinely don't know whether the topup
    // landed, so callers must treat this as uncertain funding.
    let tx_line = match extract_tx_identifier(stdout, stderr) {
        Some(tx) => format!("\n  Tx: {tx}"),
        None => String::new(),
    };
    format!(
        "wallet topup submitted, but confirmation timed out (status: pending — topup may still land)\n  \
         Address: {resolved_to}\n  \
         Method: pinata faucet claim\n  \
         Network: local sequencer ({sequencer_addr}){tx_line}\n  \
         Hint: verify balance with `logos-scaffold wallet -- account list`, or retry with `logos-scaffold wallet topup`."
    )
}

fn cmd_wallet_default_set(project: &crate::model::Project, address: &str) -> DynResult<()> {
    let normalized_address = write_default_wallet_address(&project.root, address)?;
    let state_path = wallet_state_path(&project.root);

    println!("default wallet updated");
    println!("  Address: {normalized_address}");
    println!("  State file: {}", state_path.display());

    Ok(())
}
