use std::collections::HashMap;

use anyhow::Context;
use clap::Parser;
use example_program_deployment_methods::{HELLO_WORLD_ELF, SIMPLE_TAIL_CALL_ELF};
use nssa::{
    ProgramId, privacy_preserving_transaction::circuit::ProgramWithDependencies, program::Program,
};
use wallet::{PrivacyPreservingAccount, WalletCore};

#[path = "../lib.rs"]
mod scaffold_lib;
use scaffold_lib::runner_support::{load_program, parse_account_id};

#[derive(Parser, Debug)]
struct Cli {
    #[arg(long)]
    simple_tail_call_path: Option<String>,
    #[arg(long)]
    hello_world_path: Option<String>,
    account_id: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let wallet_core = WalletCore::from_env().context("failed to initialize wallet from environment")?;

    let simple_tail_call = load_program(
        cli.simple_tail_call_path.as_deref(),
        SIMPLE_TAIL_CALL_ELF,
        "simple_tail_call",
    )?;
    let hello_world = load_program(
        cli.hello_world_path.as_deref(),
        HELLO_WORLD_ELF,
        "hello_world",
    )?;

    let dependencies: HashMap<ProgramId, Program> =
        [(hello_world.id(), hello_world)].into_iter().collect();
    let program_with_dependencies = ProgramWithDependencies::new(simple_tail_call, dependencies);
    let account_id = parse_account_id(&cli.account_id)?;
    let accounts = vec![PrivacyPreservingAccount::PrivateOwned(account_id)];

    let (response, _) = wallet_core
        .send_privacy_preserving_tx(
            accounts,
            Program::serialize_instruction(())
                .context("failed to serialize private instruction payload")?,
            &program_with_dependencies,
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
