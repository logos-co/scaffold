use anyhow::Context;
use clap::Parser;
use common::transaction::LeeTransaction;
use example_program_deployment_methods::TAIL_CALL_WITH_PDA_ELF;
use lee::{
    AccountId, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use lee_core::program::PdaSeed;
use sequencer_service_rpc::RpcClient as _;
use wallet::WalletCore;

#[path = "../lib.rs"]
mod scaffold_lib;
use scaffold_lib::runner_support::load_program;

const PDA_SEED: PdaSeed = PdaSeed::new([37; 32]);

#[derive(Parser, Debug)]
struct Cli {
    #[arg(long)]
    program_path: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let wallet_core = WalletCore::from_env()
        .await
        .context("failed to initialize wallet from environment")?;

    let program = load_program(
        cli.program_path.as_deref(),
        TAIL_CALL_WITH_PDA_ELF,
        "tail_call_with_pda",
    )?;

    let pda = AccountId::for_public_pda(&program.id(), &PDA_SEED);
    let message = Message::try_new(program.id(), vec![pda], vec![], ())
        .context("failed to build pda transaction message")?;
    let witness_set = WitnessSet::for_message(&message, &[]);
    let tx = PublicTransaction::new(message, witness_set);

    let response = wallet_core
        .helm_owned()
        .send_transaction(LeeTransaction::Public(tx))
        .await
        .context("failed to submit public transaction to localnet")?;

    println!(
        "submitted transaction: tx_hash={}",
        hex::encode(response.0)
    );
    println!("the program derived account id is: {pda}");

    Ok(())
}
