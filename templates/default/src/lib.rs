use lee::AccountId;
use program_loader_core::MAX_SEGMENT_DATA_LEN;
use wallet::{WalletCore, program_facades::program_loader::ProgramLoader};

/// Deploys `bytecode` through `program_loader`, returning the header's `AccountId`.
///
/// `payer` must be an existing, funded account — a freshly-claimed account can't pay for its own
/// claim.
pub async fn deploy_program(
    wallet_core: &mut WalletCore,
    bytecode: Vec<u8>,
    payer: AccountId,
) -> anyhow::Result<AccountId> {
    let segment_count = bytecode.len().div_ceil(MAX_SEGMENT_DATA_LEN);
    let header = wallet_core.create_new_account_public(None).0;
    let segments: Vec<AccountId> =
        std::iter::repeat_with(|| wallet_core.create_new_account_public(None).0)
            .take(segment_count)
            .collect();
    wallet_core.store_persistent_data()?;

    ProgramLoader(wallet_core)
        .deploy(header, &segments, bytecode, true, Some(payer))
        .await
}

// ---------------------------------------------------------------------------
// Added by logos-scaffold. LEZ v0.3.1's example runners build their public
// transactions with `Message::try_new`, i.e. fee-exempt, and a LEZ 0.3
// sequencer rejects a fee-exempt user transaction with `Incorrect fee`. The
// runners scaffold ships call this helper instead of submitting the message
// as-is; it is the same fee declaration and co-signing the wallet's own
// `send_pub_tx_paid_by` applies.
// ---------------------------------------------------------------------------

/// Re-issue `message` (built fee-exempt, e.g. with `Message::try_new`) with a
/// fee declaration paid by `payer`, sign it with `signing_keys` plus the
/// payer's key when the payer is not already one of the signers, submit it,
/// and wait until it is included in a block. `payer` must be a funded public
/// account whose key the wallet holds.
pub async fn send_paid_public_transaction(
    wallet_core: &WalletCore,
    message: lee::public_transaction::Message,
    signing_keys: &[&lee::PrivateKey],
    payer: AccountId,
) -> anyhow::Result<common::HashType> {
    use sequencer_service_rpc::RpcClient as _;

    let lee::public_transaction::Message {
        program_account_id,
        shard_selectors,
        mut nonces,
        instruction_data,
        fee: _,
    } = message;

    let mut keys: Vec<&lee::PrivateKey> = signing_keys.to_vec();
    let payer_signs = signing_keys
        .iter()
        .any(|key| AccountId::from(&lee::PublicKey::new_from_private_key(key)) == payer);
    if !payer_signs {
        let payer_key = wallet_core
            .storage()
            .key_chain()
            .pub_account_signing_key(payer)
            .ok_or_else(|| anyhow::anyhow!("the wallet does not hold the payer's signing key"))?;
        let payer_nonce = wallet_core
            .get_accounts_nonces(&[payer])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no nonce returned for the payer"))?;
        nonces.push(payer_nonce);
        keys.push(payer_key);
    }

    let gas_limit = wallet_core.config().gas_limit;
    let message = lee::public_transaction::Message::new_preserialized(
        program_account_id,
        shard_selectors,
        nonces,
        instruction_data,
        Some(lee::FeeDeclaration::new(
            payer,
            gas_limit,
            0,
            wallet::max_fee_for(gas_limit),
        )),
    );
    let witness_set = lee::public_transaction::WitnessSet::for_message(&message, &keys);
    let tx = lee::PublicTransaction::new(message, witness_set);
    let tx_hash = wallet_core
        .client_owned()
        .send_transaction(common::transaction::LeeTransaction::Public(tx))
        .await?;
    // Wait for inclusion, like the wallet's own commands: the payer's nonce
    // only advances once the transaction is in a block, so a follow-up
    // transaction from the same payer would otherwise be rejected.
    let (_, block_id) = wallet_core.poll_transaction(tx_hash).await?;
    println!("Transaction {tx_hash} is included in block {block_id}");
    Ok(tx_hash)
}
