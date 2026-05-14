use std::process::Command;

use anyhow::{bail, Context};

use crate::process::{render_command, run_forwarded, run_with_stdin, set_command_echo};
use crate::project::load_project;
use crate::DynResult;

use super::wallet_support::{
    extract_tx_identifier, is_already_initialized_failure, is_confirmation_timeout_failure,
    is_connectivity_failure, is_uninitialized_account_output, load_wallet_runtime,
    read_default_wallet_address, resolve_wallet_address, sequencer_unreachable_hint,
    summarize_command_failure, wallet_password, wallet_state_path, write_default_wallet_address,
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
    let project = load_project().context(
        "This command must be run inside a logos-scaffold project.\nNext step: cd into your scaffolded project directory and retry.",
    )?;

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

    if long || json {
        command.arg("--long");
    }

    if json {
        // Capture output and format as JSON
        let output = command
            .output()
            .context("failed to execute wallet account list")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("{}", serde_json::json!({ "error": stderr.trim() }));
            bail!("wallet account list failed");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let accounts: Vec<serde_json::Value> = stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| serde_json::json!({ "account": line.trim() }))
            .collect();

        println!("{}", serde_json::to_string_pretty(&accounts)?);
    } else {
        run_forwarded(&mut command, "wallet account list")
            .context("failed to execute wallet list command")?;
    }

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

/// In JSON mode, progress messages go to stderr so stdout stays valid JSON.
macro_rules! progress {
    ($json:expr, $($arg:tt)*) => {
        if $json {
            eprintln!($($arg)*);
        } else {
            println!($($arg)*);
        }
    };
}

fn cmd_wallet_topup(
    project: &crate::model::Project,
    address: Option<String>,
    dry_run: bool,
    json: bool,
) -> DynResult<()> {
    if json {
        set_command_echo(false);
    }
    match cmd_wallet_topup_inner(project, address, dry_run, json)? {
        TopupOutcome::Success => Ok(()),
        TopupOutcome::ConfirmationTimeout { message } => bail!("{message}"),
    }
}

pub(crate) fn cmd_wallet_topup_inner(
    project: &crate::model::Project,
    address: Option<String>,
    dry_run: bool,
    json: bool,
) -> DynResult<TopupOutcome> {
    let wallet = load_wallet_runtime(project)?;
    let default_address = read_default_wallet_address(&project.root)?;
    let resolved_to = resolve_wallet_address(address.as_deref(), default_address.as_deref())?;
    let sequencer_addr = wallet
        .sequencer_addr
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:3040".to_string());
    let wallet_home = wallet.wallet_home.as_os_str().to_string_lossy().to_string();
    let password_input = format!("{}\n", wallet_password());

    let mut preflight_command = Command::new(&wallet.wallet_binary);
    preflight_command
        .env("NSSA_WALLET_HOME_DIR", wallet_home.clone())
        .arg("account")
        .arg("get")
        .arg("--account-id")
        .arg(&resolved_to);

    let mut init_command = Command::new(&wallet.wallet_binary);
    init_command
        .env("NSSA_WALLET_HOME_DIR", wallet_home.clone())
        .arg("auth-transfer")
        .arg("init")
        .arg("--account-id")
        .arg(&resolved_to);

    let mut pinata_command = Command::new(&wallet.wallet_binary);
    pinata_command
        .env("NSSA_WALLET_HOME_DIR", wallet_home.clone())
        .arg("pinata")
        .arg("claim")
        .arg("--to")
        .arg(&resolved_to);

    if dry_run {
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
            bail!(
                "wallet topup failed during account preflight: {summary}\n{}",
                sequencer_unreachable_hint(&sequencer_addr)
            );
        }

        bail!(
            "wallet topup failed while checking account initialization: {summary}\nHint: verify the destination with `logos-scaffold wallet -- account get --account-id {resolved_to}`."
        );
    }

    let preflight_combined = format!("{}\n{}", preflight_output.stdout, preflight_output.stderr);
    if is_uninitialized_account_output(&preflight_combined) {
        println!(
            "wallet topup preflight: destination is uninitialized; running auth-transfer init"
        );
        let init_output = run_with_stdin(init_command, password_input.clone())
            .context("failed to execute wallet topup init command")?;

        if !init_output.status.success() {
            let summary = summarize_command_failure(&init_output.stdout, &init_output.stderr);
            let combined = format!("{}\n{}", init_output.stdout, init_output.stderr);
            if is_connectivity_failure(&combined) {
                bail!(
                    "wallet topup failed during account initialization: {summary}\n{}",
                    sequencer_unreachable_hint(&sequencer_addr)
                );
            }
            if is_already_initialized_failure(&combined) {
                progress!(
                    json,
                    "wallet topup preflight: destination already initialized; continuing"
                );
            } else {
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
            bail!(
                "wallet topup failed: {summary}\n{}",
                sequencer_unreachable_hint(&sequencer_addr)
            );
        }
        if is_confirmation_timeout_failure(&combined) {
            let message = confirmation_timeout_message(
                &resolved_to,
                &sequencer_addr,
                &output.stdout,
                &output.stderr,
            );
            return Ok(TopupOutcome::ConfirmationTimeout { message });
        }
        bail!(
            "wallet topup failed: {summary}\nHint: run `logos-scaffold wallet list` to inspect addresses, then retry with `--address` or set a default wallet."
        );
    }

    if json {
        let tx = extract_tx_identifier(&output.stdout, &output.stderr);
        let json_out = serde_json::json!({
            "status": "ok",
            "address": resolved_to,
            "method": "pinata",
            "tx": tx,
        });
        println!("{}", serde_json::to_string(&json_out)?);
    } else {
        println!("wallet topup complete");
        println!("  Address: {resolved_to}");
        println!("  Method: pinata faucet claim");
        println!("  Network: local sequencer ({sequencer_addr})");
        if let Some(tx) = extract_tx_identifier(&output.stdout, &output.stderr) {
            println!("  Tx: {tx}");
        }
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
