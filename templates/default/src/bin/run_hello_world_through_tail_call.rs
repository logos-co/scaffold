use anyhow::{Context, anyhow};
use clap::Parser;
use common::transaction::LeeTransaction;
use example_program_deployment_methods::SIMPLE_TAIL_CALL_ELF;
use lee::{
    PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use sequencer_service_rpc::RpcClient as _;
use wallet::WalletCore;

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
    let wallet_core = WalletCore::from_env()
        .await
        .context("failed to initialize wallet from environment")?;

    let program = load_program(
        cli.program_path.as_deref(),
        SIMPLE_TAIL_CALL_ELF,
        "simple_tail_call",
    )?;
    let account_id = parse_account_id(&cli.account_id)?;

    // simple_tail_call hands the account to hello_world, which claims an
    // uninitialized account with `Claim::Authorized`. The authorization
    // travels with the tail call, so this transaction must carry the
    // account's signature and nonce; unsigned, the sequencer rejects it with
    // `ClaimedUnauthorizedAccount`.
    let signing_key = wallet_core
        .get_account_public_signing_key(account_id)
        .ok_or_else(|| anyhow!("input account must be a self-owned public account"))?;
    let nonces = wallet_core
        .get_accounts_nonces(&[account_id])
        .await
        .context("failed to query account nonce from sequencer")?;
    let message = Message::try_new(program.id(), vec![account_id], nonces, ())
        .context("failed to build tail-call transaction message")?;
    let witness_set = WitnessSet::for_message(&message, &[signing_key]);
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
    println!("verification hint: wallet account get --account-id Public/{account_id}");

    Ok(())
}
