use clap::{Parser, Subcommand};
use example_program_deployment_methods::LEZ_COUNTER_ELF;
use nssa::{
    AccountId, ProgramId, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use sequencer_service_rpc::RpcClient as _;
use serde::Serialize;
use wallet::WalletCore;

// `lib.rs` also carries the host-side program the IDL is extracted from;
// the runner only needs `runner_support`, so the rest would warn as unused.
#[allow(dead_code)]
#[path = "../lib.rs"]
mod scaffold_lib;
use scaffold_lib::runner_support::{load_program, parse_account_id};

#[derive(Parser, Debug)]
struct Cli {
    #[arg(long)]
    program_path: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create the program's counter account (a PDA) and set it to zero.
    Init {
        /// Self-owned public account that signs the transaction.
        #[arg(long)]
        authority: String,
    },
    /// Add `--amount` to the counter.
    Increment {
        /// Self-owned public account that signs the transaction.
        #[arg(long)]
        authority: String,
        #[arg(long)]
        amount: u64,
    },
    /// Print the counter account and its current value.
    Show,
}

/// Wire form of the program's instructions. Variant order and field types
/// must match the `#[instruction]` functions in `methods/guest` (the IDL in
/// `idl/lez_counter.json` lists them in the same order).
#[derive(Serialize)]
enum Instruction {
    Initialize,
    Increment { amount: u64 },
}

/// The counter is not a user account: both instructions derive it as the
/// PDA `[program_id, "counter"]`, so there is one counter per deployment.
fn counter_account(program_id: &ProgramId) -> AccountId {
    let seed = spel_framework_core::pda::seed_from_str("counter");
    spel_framework_core::pda::compute_pda(program_id, &[&seed])
}

async fn submit(
    wallet_core: &WalletCore,
    program_id: ProgramId,
    counter: AccountId,
    authority: AccountId,
    instruction: Instruction,
) {
    let signing_key = wallet_core
        .storage()
        .user_data
        .get_pub_account_signing_key(authority)
        .unwrap_or_else(|| panic!("authority {authority} must be a self-owned public account"));
    let nonces = wallet_core
        .get_accounts_nonces(vec![authority])
        .await
        .expect("failed to query authority nonce from sequencer");
    let message = Message::try_new(program_id, vec![counter, authority], nonces, instruction)
        .expect("failed to build transaction message");
    let witness_set = WitnessSet::for_message(&message, &[signing_key]);
    let tx = PublicTransaction::new(message, witness_set);
    let response = wallet_core
        .sequencer_client
        .send_transaction(tx.into())
        .await
        .expect("failed to submit transaction to localnet");

    println!("submitted transaction: tx_hash={}", hex::encode(response.0));
    println!("verification hint: wallet account get --account-id Public/{counter}");
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let wallet_core = WalletCore::from_env().expect("wallet should initialize from environment");
    let program = load_program(cli.program_path.as_deref(), LEZ_COUNTER_ELF, "lez_counter");
    let program_id = program.id();
    let counter = counter_account(&program_id);

    match cli.command {
        Command::Init { authority } => {
            let authority = parse_account_id(&authority);
            println!("Init counter at account {counter} (authority: {authority})");
            submit(&wallet_core, program_id, counter, authority, Instruction::Initialize).await;
        }
        Command::Increment { authority, amount } => {
            let authority = parse_account_id(&authority);
            println!("Increment counter {counter} by {amount} (authority: {authority})");
            let instruction = Instruction::Increment { amount };
            submit(&wallet_core, program_id, counter, authority, instruction).await;
        }
        Command::Show => {
            let account = wallet_core
                .sequencer_client
                .get_account(counter)
                .await
                .expect("failed to read counter account from sequencer");
            let value = <[u8; 8]>::try_from(&account.data[..]).map(u64::from_le_bytes);
            match value {
                Ok(value) => println!("counter {counter} = {value}"),
                Err(_) if account.data.is_empty() => {
                    println!("counter {counter} is not initialized; run `init` first")
                }
                Err(_) => println!(
                    "counter {counter} holds {} bytes, expected an 8-byte little-endian u64",
                    account.data.len()
                ),
            }
        }
    }
}
