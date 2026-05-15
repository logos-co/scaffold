use anyhow::Context;
use clap::Parser;
use example_program_deployment_methods::HELLO_WORLD_ELF;
use nssa::program::Program;
use wallet::{PrivacyPreservingAccount, WalletCore};

#[path = "../lib.rs"]
mod scaffold_lib;
use scaffold_lib::runner_support::{load_program, parse_account_id};

#[derive(Parser, Debug)]
struct Cli {
    #[arg(long)]
    program_path: Option<String>,
    account_id: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let wallet_core = WalletCore::from_env().context("failed to initialize wallet from environment")?;

    let program = load_program(cli.program_path.as_deref(), HELLO_WORLD_ELF, "hello_world")?;
    let account_id = parse_account_id(&cli.account_id)?;

    let greeting: Vec<u8> = vec![72, 111, 108, 97, 32, 109, 117, 110, 100, 111, 33];
    let accounts = vec![PrivacyPreservingAccount::PrivateOwned(account_id)];

    let (response, _) = wallet_core
        .send_privacy_preserving_tx(
            accounts,
            Program::serialize_instruction(greeting)
                .context("failed to serialize private instruction payload")?,
            &program.into(),
        )
        .await
        .map_err(|err| anyhow::anyhow!("failed to submit private transaction: {err}"))?;

    println!(
        "submitted transaction: tx_hash={}",
        hex::encode(response.0)
    );
    println!("verification hint: wallet account sync-private");

    Ok(())
}
