use anyhow::{Context, anyhow};
use clap::Parser;
use example_program_deployment_methods::HELLO_WORLD_ELF;
use nssa::{
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
    let wallet_core = WalletCore::from_env().context("failed to initialize wallet from environment")?;

    let program = load_program(cli.program_path.as_deref(), HELLO_WORLD_ELF, "hello_world")?;
    let account_id = parse_account_id(&cli.account_id)?;

    // The hello_world program claims an uninitialized input account with
    // `Claim::Authorized`, so the first transaction against a fresh account
    // must be authorized by that account's key. Submitting it unsigned (empty
    // witness set / nonces) makes the sequencer reject the claim with
    // `InvalidProgramBehaviour`. Sign with the account's own signing key and
    // its current nonce, the same way the authorized example does.
    let signing_key = wallet_core
        .storage()
        .user_data
        .get_pub_account_signing_key(account_id)
        .ok_or_else(|| anyhow!("input account must be a self-owned public account"))?;

    let greeting: Vec<u8> = vec![72, 111, 108, 97, 32, 109, 117, 110, 100, 111, 33];
    let nonces = wallet_core
        .get_accounts_nonces(vec![account_id])
        .await
        .context("failed to query account nonce from sequencer")?;
    let message = Message::try_new(program.id(), vec![account_id], nonces, greeting)
        .context("failed to build hello_world transaction message")?;
    let witness_set = WitnessSet::for_message(&message, &[signing_key]);
    let tx = PublicTransaction::new(message, witness_set);

    let response = wallet_core
        .sequencer_client
        .send_transaction(tx.into())
        .await
        .context("failed to submit public transaction to localnet")?;

    println!(
        "submitted transaction: tx_hash={}",
        hex::encode(response.0)
    );
    println!("verification hint: wallet account get --account-id {}", cli.account_id);

    Ok(())
}
