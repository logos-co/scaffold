use anyhow::Context;
use clap::{Parser, Subcommand};
use example_program_deployment_methods::HELLO_WORLD_WITH_MOVE_FUNCTION_ELF;
use nssa::{PublicTransaction, program::Program, public_transaction};
use sequencer_service_rpc::RpcClient as _;
use wallet::{PrivacyPreservingAccount, WalletCore};

#[path = "../lib.rs"]
mod scaffold_lib;
use scaffold_lib::runner_support::{load_program, parse_account_id};

type Instruction = (u8, Vec<u8>);
const WRITE_FUNCTION_ID: u8 = 0;
const MOVE_DATA_FUNCTION_ID: u8 = 1;

#[derive(Parser, Debug)]
struct Cli {
    #[arg(long)]
    program_path: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    WritePublic {
        account_id: String,
        greeting: String,
    },
    WritePrivate {
        account_id: String,
        greeting: String,
    },
    MoveDataPublicToPublic {
        from: String,
        to: String,
    },
    MoveDataPublicToPrivate {
        from: String,
        to: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let program = load_program(
        cli.program_path.as_deref(),
        HELLO_WORLD_WITH_MOVE_FUNCTION_ELF,
        "hello_world_with_move_function",
    )?;
    let wallet_core = WalletCore::from_env().context("failed to initialize wallet from environment")?;

    match cli.command {
        Command::WritePublic {
            account_id,
            greeting,
        } => {
            let instruction: Instruction = (WRITE_FUNCTION_ID, greeting.into_bytes());
            let account_id = parse_account_id(&account_id)?;
            let message = public_transaction::Message::try_new(
                program.id(),
                vec![account_id],
                vec![],
                instruction,
            )
            .context("failed to build write-public message")?;
            let witness_set = public_transaction::WitnessSet::for_message(&message, &[]);
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
            println!("verification hint: wallet account get --account-id {account_id}");
        }
        Command::WritePrivate {
            account_id,
            greeting,
        } => {
            let instruction: Instruction = (WRITE_FUNCTION_ID, greeting.into_bytes());
            let account_id = parse_account_id(&account_id)?;
            let accounts = vec![PrivacyPreservingAccount::PrivateOwned(account_id)];
            let (response, _) = wallet_core
                .send_privacy_preserving_tx(
                    accounts,
                    Program::serialize_instruction(instruction)
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
        }
        Command::MoveDataPublicToPublic { from, to } => {
            let instruction: Instruction = (MOVE_DATA_FUNCTION_ID, vec![]);
            let from = parse_account_id(&from)?;
            let to = parse_account_id(&to)?;
            let message = public_transaction::Message::try_new(
                program.id(),
                vec![from, to],
                vec![],
                instruction,
            )
            .context("failed to build move-data-public-to-public message")?;
            let witness_set = public_transaction::WitnessSet::for_message(&message, &[]);
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
            println!("verification hint: wallet account get --account-id {from}");
            println!("verification hint: wallet account get --account-id {to}");
        }
        Command::MoveDataPublicToPrivate { from, to } => {
            let instruction: Instruction = (MOVE_DATA_FUNCTION_ID, vec![]);
            let from = parse_account_id(&from)?;
            let to = parse_account_id(&to)?;
            let accounts = vec![
                PrivacyPreservingAccount::Public(from),
                PrivacyPreservingAccount::PrivateOwned(to),
            ];
            let (response, _) = wallet_core
                .send_privacy_preserving_tx(
                    accounts,
                    Program::serialize_instruction(instruction)
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
        }
    };

    Ok(())
}
