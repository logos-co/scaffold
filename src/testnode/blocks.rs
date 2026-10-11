//! Deterministic block and clock context for test-node replay.
//!
//! Sequencer execution uses real block ids and wall-clock timestamps.
//! Clock-sensitive programs need the exact sequencer context to compare
//! state correctly, so tests must be able to observe blocks (including
//! empty post-genesis blocks, which still advance clock state via the
//! mandatory clock transaction) and read the clock accounts at a stable
//! boundary.
//!
//! Wire facts this module is built on (LEZ 0.3 sequencer):
//!
//! - A block is borsh: header (`block_id: u64`, two 32-byte hashes,
//!   `timestamp: u64` — 80 bytes total), then `Vec<LeeTransaction>` (u32
//!   count + transactions), then a 1-byte bedrock status.
//! - `LeeTransaction` is a borsh enum: tag 0 = `Public`, 1 =
//!   `PrivacyPreserving`. A `Public` transaction is a message
//!   (`program_account_id`, `Vec<ProgramShardSelector>` of 64-byte
//!   `(account_id, program_account_id)` pairs, `Vec<Nonce>` of u128,
//!   `Vec<u8>` instruction data, `Option<FeeDeclaration>` of 64 bytes) and a
//!   witness set (`Vec<(Signature{64}, PublicKey{32})>`), so its byte span —
//!   and therefore its hash (`sha256` of the variant bytes) — can be recovered
//!   without the upstream types. `PrivacyPreserving` transactions carry
//!   nested proof structures; the walker stops there and reports the block
//!   as partially parsed rather than guessing.
//! - Every block, genesis (block id 1) included, ends with two fee-exempt
//!   system transactions: the fee distribution and the clock tick, whose
//!   shard selectors are exactly the three `/LEZ/ClockProgramAccount/…`
//!   accounts. Wallet-submitted transactions always declare a fee, which is
//!   how user transactions are told apart from system ones. Genesis also
//!   carries the chain's setup transactions (channel params, supplies).
//! - Each clock account holds `ClockAccountData { block_id: u64, timestamp:
//!   u64 }` (borsh) in the clock program's shard.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

use super::client::{parse_block_prefix, BlockContext, RpcError, TestNodeClient};

/// Borsh size of the block header (`u64` + 32 + 32 + `u64`).
const BLOCK_HEADER_LEN: usize = 8 + 32 + 32 + 8;
/// Block id LEZ gives the genesis block (`lee_core::GENESIS_BLOCK_ID`).
const GENESIS_BLOCK_ID: u64 = 1;

/// The three clock program account ids (raw 32-byte values; base58 on the
/// RPC wire).
pub const CLOCK_ACCOUNT_RAW_IDS: [&[u8; 32]; 3] = [
    b"/LEZ/ClockProgramAccount/0000001",
    b"/LEZ/ClockProgramAccount/0000010",
    b"/LEZ/ClockProgramAccount/0000050",
];

/// Base58 account ids of the clock accounts, as used by `getAccount`.
pub fn clock_account_ids() -> Vec<String> {
    CLOCK_ACCOUNT_RAW_IDS
        .iter()
        .map(|raw| bs58::encode(raw).into_string())
        .collect()
}

/// Kind of a transaction inside a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TxKind {
    Public,
    PrivacyPreserving,
}

/// One parsed transaction inside a block.
#[derive(Clone, Debug, Serialize)]
pub struct TxSummary {
    /// `sha256` of the transaction's borsh variant bytes — the same value
    /// the sequencer returns from `sendTransaction`.
    pub hash: String,
    pub kind: TxKind,
    /// `true` when this is the sequencer's mandatory clock transaction
    /// (a `Public` transaction over exactly the three clock accounts).
    pub is_clock: bool,
    /// `true` for a fee-exempt sequencer transaction (clock tick, fee
    /// distribution, genesis setup) — anything a wallet did not submit.
    pub is_system: bool,
}

/// Observable facts about one block.
#[derive(Clone, Debug, Serialize)]
pub struct BlockInfo {
    pub block_id: u64,
    pub timestamp: u64,
    /// Total number of transactions in the block body.
    pub transaction_count: u32,
    /// `true` for the genesis block (block id 1), which carries the chain's
    /// setup transactions rather than user transactions.
    pub is_genesis: bool,
    /// `true` when the block carries the mandatory sequencer clock
    /// transaction (every block; empty blocks still advance clock state
    /// through it).
    pub has_clock_transaction: bool,
    /// `true` when the block carries at least one user (fee-paying or
    /// privacy-preserving) transaction.
    pub has_user_transactions: bool,
    /// Per-transaction summaries, in block order. May be shorter than
    /// `transaction_count` when `fully_parsed` is `false`.
    pub transactions: Vec<TxSummary>,
    /// `false` when the block contains a privacy-preserving transaction,
    /// whose nested proof layout this client intentionally does not walk;
    /// `transactions` then covers only the prefix before it.
    pub fully_parsed: bool,
}

/// Block range request (inclusive).
#[derive(Clone, Copy, Debug)]
pub struct BlockRange {
    pub from: u64,
    pub to: u64,
}

/// One clock account's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ClockAccount {
    /// Base58 account id.
    pub account_id: String,
    pub nonce: u128,
    /// `ClockAccountData.block_id` parsed from the clock shard — the block
    /// the clock last ticked at (granularity depends on the account).
    pub block_id: Option<u64>,
    /// `ClockAccountData.timestamp` parsed from the account data.
    pub timestamp: Option<u64>,
}

/// All clock accounts read at one consistent block boundary.
#[derive(Clone, Debug, Serialize)]
pub struct ClockSnapshot {
    /// Head block id the snapshot was read at (identical before and after
    /// the reads for stable snapshots).
    pub read_block_id: u64,
    pub accounts: Vec<ClockAccount>,
}

/// How to read the clock.
#[derive(Clone, Copy, Debug)]
pub enum ClockReadMode {
    /// One read at the current head; the head may tick mid-read.
    Latest,
    /// Keep sampling until `samples` consecutive reads observe the same
    /// head and identical clock accounts, or `timeout` elapses (returned as
    /// a retryable error).
    Stable { samples: u32, timeout: Duration },
}

impl TestNodeClient {
    /// Current head block with parsed transaction summaries.
    pub fn block_head(&self) -> Result<BlockInfo, RpcError> {
        let head = self.last_block_id()?;
        self.block_info(head)?.ok_or_else(|| RpcError::Other {
            operation: "getBlock".to_string(),
            message: format!("head block {head} reported by getLastBlockId is not available"),
        })
    }

    /// One block's info; `None` for an unknown id.
    pub fn block_info(&self, block_id: u64) -> Result<Option<BlockInfo>, RpcError> {
        let Some(bytes) = self.block_bytes(block_id)? else {
            return Ok(None);
        };
        parse_block_info(&bytes)
            .map(Some)
            .map_err(|message| RpcError::Other {
                operation: "getBlock".to_string(),
                message: format!("block {block_id}: {message}"),
            })
    }

    /// Inclusive block range. Fails with a targeted error when a block in
    /// the range is not available.
    pub fn blocks(&self, range: BlockRange) -> Result<Vec<BlockInfo>, RpcError> {
        if range.to < range.from {
            return Err(RpcError::Other {
                operation: "getBlockRange".to_string(),
                message: format!("invalid range: from={} > to={}", range.from, range.to),
            });
        }
        let mut blocks = Vec::new();
        for block_id in range.from..=range.to {
            match self.block_info(block_id)? {
                Some(info) => blocks.push(info),
                None => {
                    return Err(RpcError::Other {
                        operation: "getBlock".to_string(),
                        message: format!("block {block_id} not found on the node"),
                    })
                }
            }
        }
        Ok(blocks)
    }

    /// Wait until `count` blocks exist after `after_block` and return them.
    /// Lets tests wait for a known number of blocks past a submission
    /// boundary.
    pub fn wait_blocks(
        &self,
        after_block: u64,
        count: u64,
        timeout: Duration,
    ) -> Result<Vec<BlockInfo>, RpcError> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let deadline = Instant::now() + timeout;
        let target = after_block + count;
        loop {
            let head = self.last_block_id()?;
            if head >= target {
                return self.blocks(BlockRange {
                    from: after_block + 1,
                    to: target,
                });
            }
            if Instant::now() >= deadline {
                return Err(RpcError::Other {
                    operation: "waitBlocks".to_string(),
                    message: format!(
                        "timed out after {}s waiting for block {target} (head={head})",
                        timeout.as_secs()
                    ),
                });
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Read all clock accounts. `Latest` reads once at the current head;
    /// `Stable` provides a read barrier against the always-ticking
    /// sequencer clock by requiring consecutive identical samples.
    pub fn clock_snapshot(&self, mode: ClockReadMode) -> Result<ClockSnapshot, RpcError> {
        match mode {
            ClockReadMode::Latest => self.read_clock_once(),
            ClockReadMode::Stable { samples, timeout } => {
                let needed = samples.max(2);
                let deadline = Instant::now() + timeout;
                let mut previous: Option<ClockSnapshot> = None;
                let mut consecutive = 0_u32;
                loop {
                    let snapshot = self.read_clock_once()?;
                    match &previous {
                        Some(prev)
                            if prev.read_block_id == snapshot.read_block_id
                                && prev.accounts == snapshot.accounts =>
                        {
                            consecutive += 1;
                            if consecutive + 1 >= needed {
                                return Ok(snapshot);
                            }
                        }
                        _ => {
                            consecutive = 0;
                            previous = Some(snapshot);
                        }
                    }
                    if Instant::now() >= deadline {
                        return Err(RpcError::Other {
                            operation: "clockWaitStable".to_string(),
                            message: format!(
                                "no stable clock snapshot within {}s ({needed} identical \
                                 consecutive samples required); retry with a longer timeout",
                                timeout.as_secs()
                            ),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }

    /// One clock read with a consistency check: the head must be identical
    /// before and after the account reads, otherwise the read is retried
    /// (bounded number of attempts).
    fn read_clock_once(&self) -> Result<ClockSnapshot, RpcError> {
        for _ in 0..5 {
            let head_before = self.last_block_id()?;
            let mut accounts = Vec::new();
            for account_id in clock_account_ids() {
                accounts.push(self.clock_account(&account_id)?);
            }
            let head_after = self.last_block_id()?;
            if head_before == head_after {
                return Ok(ClockSnapshot {
                    read_block_id: head_after,
                    accounts,
                });
            }
        }
        Err(RpcError::Other {
            operation: "clockRead".to_string(),
            message: "head kept advancing during clock reads; retry (the node is producing \
                      blocks faster than the read loop)"
                .to_string(),
        })
    }

    fn clock_account(&self, account_id: &str) -> Result<ClockAccount, RpcError> {
        let result = self.call("getAccount", json!([account_id]))?;
        parse_clock_account(account_id, &result).map_err(|message| RpcError::Other {
            operation: "getAccount".to_string(),
            message: format!("clock account {account_id}: {message}"),
        })
    }
}

fn parse_clock_account(account_id: &str, value: &Value) -> Result<ClockAccount, String> {
    let nonce = read_u128(value.get("nonce")).ok_or("missing numeric `nonce`")?;
    // LEZ 0.3: `{"nonce", "data": {"shards": {<program>: [bytes]}}}`. A clock
    // account carries a single shard, the clock program's; an unticked one
    // carries none.
    let shards = value
        .get("data")
        .and_then(|data| data.get("shards"))
        .and_then(Value::as_object);
    let clock_data = match shards {
        Some(shards) => shards
            .values()
            .map(shard_bytes)
            .collect::<Option<Vec<Vec<u8>>>>()
            .ok_or("a shard is not a byte array")?
            .into_iter()
            .find(|bytes| bytes.len() >= 16)
            .unwrap_or_default(),
        None => Vec::new(),
    };

    // ClockAccountData { block_id: u64, timestamp: u64 } — borsh, 16 bytes.
    let (block_id, timestamp) = if clock_data.len() >= 16 {
        (
            Some(u64::from_le_bytes(clock_data[0..8].try_into().unwrap())),
            Some(u64::from_le_bytes(clock_data[8..16].try_into().unwrap())),
        )
    } else {
        (None, None)
    };

    Ok(ClockAccount {
        account_id: account_id.to_string(),
        nonce,
        block_id,
        timestamp,
    })
}

/// A shard as served over JSON: an array of byte values.
pub(crate) fn shard_bytes(value: &Value) -> Option<Vec<u8>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_u64().and_then(|byte| u8::try_from(byte).ok()))
        .collect()
}

fn read_u128(value: Option<&Value>) -> Option<u128> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return Some(u128::from(number));
    }
    // serde_json supports 128-bit integers natively; route through
    // serde to cover balances beyond u64.
    serde_json::from_value::<u128>(value.clone()).ok()
}

/// Parse a borsh-encoded block into [`BlockInfo`].
pub(crate) fn parse_block_info(block_bytes: &[u8]) -> Result<BlockInfo, String> {
    let BlockContext {
        block_id,
        timestamp,
    } = parse_block_prefix(block_bytes).ok_or("payload too short for the block header")?;

    let count_bytes = block_bytes
        .get(BLOCK_HEADER_LEN..BLOCK_HEADER_LEN + 4)
        .ok_or("payload too short for the transaction count")?;
    let transaction_count = u32::from_le_bytes(count_bytes.try_into().unwrap());

    let mut transactions = Vec::new();
    let mut fully_parsed = true;
    let mut offset = BLOCK_HEADER_LEN + 4;
    for _ in 0..transaction_count {
        match parse_transaction_span(block_bytes, offset)? {
            Some((summary, next_offset)) => {
                transactions.push(summary);
                offset = next_offset;
            }
            None => {
                // Privacy-preserving transaction: nested proof layout is
                // intentionally not walked.
                fully_parsed = false;
                break;
            }
        }
    }

    let is_genesis = block_id == GENESIS_BLOCK_ID;
    let has_clock_transaction = transactions.iter().any(|tx| tx.is_clock);
    // An unparsed (privacy-preserving) transaction is always a user one.
    let has_user_transactions = !fully_parsed || transactions.iter().any(|tx| !tx.is_system);

    Ok(BlockInfo {
        block_id,
        timestamp,
        transaction_count,
        is_genesis,
        has_clock_transaction,
        has_user_transactions,
        transactions,
        fully_parsed,
    })
}

/// Parse one `LeeTransaction` starting at `offset`. Returns the summary and
/// the offset right after the transaction, or `None` for a
/// privacy-preserving transaction (unparsed by design).
fn parse_transaction_span(
    bytes: &[u8],
    offset: usize,
) -> Result<Option<(TxSummary, usize)>, String> {
    let tag = *bytes
        .get(offset)
        .ok_or("payload too short for a transaction tag")?;
    let body_start = offset + 1;

    match tag {
        0 => {
            let mut cursor = body_start;
            cursor = skip_fixed(bytes, cursor, 32, "program_account_id")?;
            let (selectors_start, selector_count) =
                (cursor + 4, read_vec_len(bytes, cursor, "shard_selectors")?);
            cursor = skip_vec(bytes, cursor, 64, "shard_selectors")?;
            cursor = skip_vec(bytes, cursor, 16, "nonces")?;
            cursor = skip_vec(bytes, cursor, 1, "instruction_data")?;
            let fee_tag = *bytes
                .get(cursor)
                .ok_or("payload too short for the fee declaration tag")?;
            cursor += 1;
            let is_system = match fee_tag {
                0 => true,
                1 => {
                    // payer 32 + gas_limit u64 + tip u64 + max_fee u128.
                    cursor = skip_fixed(bytes, cursor, 32 + 8 + 8 + 16, "fee")?;
                    false
                }
                other => return Err(format!("invalid fee declaration tag {other}")),
            };
            cursor = skip_vec(bytes, cursor, 96, "witness_set")?;

            let is_clock = is_clock_selector_set(bytes, selectors_start, selector_count);
            Ok(Some((
                TxSummary {
                    hash: sha256_hex(&bytes[body_start..cursor]),
                    kind: TxKind::Public,
                    is_clock,
                    is_system,
                },
                cursor,
            )))
        }
        1 => Ok(None),
        other => Err(format!("unknown transaction tag {other}")),
    }
}

/// `true` when the shard selectors address exactly the three clock accounts
/// in canonical order — the shape of the sequencer's clock transaction. Each
/// selector is `(account_id, program_account_id)`; only the account half is
/// compared.
fn is_clock_selector_set(bytes: &[u8], start: usize, count: u32) -> bool {
    if count != 3 {
        return false;
    }
    CLOCK_ACCOUNT_RAW_IDS.iter().enumerate().all(|(i, raw)| {
        bytes
            .get(start + i * 64..start + i * 64 + 32)
            .map(|span| span == *raw)
            .unwrap_or(false)
    })
}

fn read_vec_len(bytes: &[u8], offset: usize, field: &str) -> Result<u32, String> {
    let span = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| format!("payload too short for `{field}` length"))?;
    Ok(u32::from_le_bytes(span.try_into().unwrap()))
}

fn skip_vec(
    bytes: &[u8],
    offset: usize,
    element_size: usize,
    field: &str,
) -> Result<usize, String> {
    let len = read_vec_len(bytes, offset, field)? as usize;
    let end = offset + 4 + len * element_size;
    if bytes.len() < end {
        return Err(format!("payload too short for `{field}` elements"));
    }
    Ok(end)
}

fn skip_fixed(bytes: &[u8], offset: usize, size: usize, field: &str) -> Result<usize, String> {
    let end = offset + size;
    if bytes.len() < end {
        return Err(format!("payload too short for `{field}`"));
    }
    Ok(end)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use base64::Engine as _;
    use serde_json::json;

    use super::*;
    use crate::testnode::test_support::FakeNode;

    /// Borsh-encode a synthetic LEZ 0.3 Public transaction whose shard
    /// selectors address `account_ids`; `fee_paying` adds a fee declaration
    /// (what every wallet-submitted transaction carries). Returns the full
    /// enum encoding (tag + body).
    fn fake_public_tx(account_ids: &[[u8; 32]], fee_paying: bool) -> Vec<u8> {
        let mut tx = vec![0_u8]; // tag: Public
        tx.extend_from_slice(&[0x11; 32]); // program_account_id
        tx.extend_from_slice(&(account_ids.len() as u32).to_le_bytes());
        for id in account_ids {
            tx.extend_from_slice(id); // selector account
            tx.extend_from_slice(&[0x33; 32]); // selector program
        }
        tx.extend_from_slice(&1_u32.to_le_bytes()); // one nonce
        tx.extend_from_slice(&[0; 16]);
        tx.extend_from_slice(&3_u32.to_le_bytes()); // three instruction bytes
        tx.extend_from_slice(&[7, 8, 9]);
        if fee_paying {
            tx.push(1);
            tx.extend_from_slice(&[0x44; 64]);
        } else {
            tx.push(0);
        }
        tx.extend_from_slice(&1_u32.to_le_bytes()); // one (sig, pk) pair
        tx.extend_from_slice(&[0x22; 96]);
        tx
    }

    fn fake_clock_tx() -> Vec<u8> {
        let ids: Vec<[u8; 32]> = CLOCK_ACCOUNT_RAW_IDS.iter().map(|raw| **raw).collect();
        fake_public_tx(&ids, false)
    }

    fn fake_fee_tx() -> Vec<u8> {
        fake_public_tx(&[[0x01; 32], [0x02; 32], [0x03; 32], [0x04; 32]], false)
    }

    fn fake_block(block_id: u64, timestamp: u64, txs: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&block_id.to_le_bytes());
        bytes.extend_from_slice(&[0xAA; 32]);
        bytes.extend_from_slice(&[0xBB; 32]);
        bytes.extend_from_slice(&timestamp.to_le_bytes());
        bytes.extend_from_slice(&(txs.len() as u32).to_le_bytes());
        for tx in txs {
            bytes.extend_from_slice(tx);
        }
        bytes.push(0); // bedrock status
        bytes
    }

    #[test]
    fn genesis_block_is_explicit() {
        let info = parse_block_info(&fake_block(
            1,
            0,
            &[fake_fee_tx(), fake_fee_tx(), fake_clock_tx()],
        ))
        .unwrap();
        assert!(info.is_genesis);
        assert!(info.has_clock_transaction);
        assert!(!info.has_user_transactions);
        assert_eq!(info.transaction_count, 3);
        assert!(info.fully_parsed);
    }

    #[test]
    fn empty_post_genesis_block_reports_system_transactions_only() {
        let info = parse_block_info(&fake_block(
            5,
            1_700_000_000_500,
            &[fake_fee_tx(), fake_clock_tx()],
        ))
        .unwrap();
        assert!(!info.is_genesis);
        assert!(info.has_clock_transaction);
        assert!(!info.has_user_transactions);
        assert_eq!(info.transactions.len(), 2);
        assert!(info.transactions[1].is_clock);
        assert!(info.transactions.iter().all(|tx| tx.is_system));
        assert_eq!(info.transactions[1].kind, TxKind::Public);
        assert!(info.fully_parsed);
    }

    #[test]
    fn user_transactions_are_distinguished_from_system_ones() {
        let user_tx = fake_public_tx(&[[0x55; 32], [0x66; 32]], true);
        let info = parse_block_info(&fake_block(
            7,
            1_700_000_001_000,
            &[user_tx.clone(), fake_fee_tx(), fake_clock_tx()],
        ))
        .unwrap();

        assert!(info.has_user_transactions);
        assert_eq!(info.transactions.len(), 3);
        assert!(!info.transactions[0].is_system);
        assert!(!info.transactions[0].is_clock);
        assert!(info.transactions[1].is_system);
        assert!(info.transactions[2].is_clock);

        // Hashes are sha256 over the variant bytes (without the enum tag).
        assert_eq!(info.transactions[0].hash, sha256_hex(&user_tx[1..]));
    }

    #[test]
    fn privacy_preserving_tx_marks_block_partially_parsed() {
        let pp_tx = vec![1_u8, 0xDE, 0xAD]; // tag 1 + opaque bytes
        let info = parse_block_info(&fake_block(
            9,
            1_700_000_002_000,
            &[fake_fee_tx(), pp_tx],
        ))
        .unwrap();
        assert!(!info.fully_parsed);
        assert_eq!(info.transaction_count, 2);
        assert_eq!(info.transactions.len(), 1);
        assert!(info.has_user_transactions);
    }

    #[test]
    fn clock_account_ids_are_base58_of_raw_constants() {
        let ids = clock_account_ids();
        assert_eq!(ids.len(), 3);
        for (id, raw) in ids.iter().zip(CLOCK_ACCOUNT_RAW_IDS) {
            assert_eq!(bs58::decode(id).into_vec().unwrap(), raw.to_vec());
        }
    }

    #[test]
    fn blocks_range_and_head_round_trip_over_rpc() {
        let block5 = fake_block(5, 500, &[fake_clock_tx()]);
        let block6 = fake_block(6, 600, &[fake_clock_tx()]);
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let block5_b64 = b64(&block5);
        let block6_b64 = b64(&block6);

        let node = FakeNode::start(move |method, params| match method {
            "getLastBlockId" => json!(6),
            "getBlock" => match params[0].as_u64().unwrap() {
                5 => json!(block5_b64.clone()),
                6 => json!(block6_b64.clone()),
                _ => serde_json::Value::Null,
            },
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let head = client.block_head().unwrap();
        assert_eq!(head.block_id, 6);
        assert_eq!(head.timestamp, 600);

        let blocks = client.blocks(BlockRange { from: 5, to: 6 }).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block_id, 5);

        let err = client.blocks(BlockRange { from: 6, to: 7 }).unwrap_err();
        assert!(err.message().contains("block 7 not found"), "{err}");
    }

    #[test]
    fn wait_blocks_returns_blocks_after_boundary() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        let head = Arc::new(AtomicU64::new(3));
        let head_for_node = head.clone();

        let block4 = fake_block(4, 400, &[fake_clock_tx()]);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&block4);

        let node = FakeNode::start(move |method, params| match method {
            "getLastBlockId" => json!(head_for_node.fetch_add(1, Ordering::Relaxed)),
            "getBlock" => match params[0].as_u64().unwrap() {
                4 => json!(b64.clone()),
                _ => serde_json::Value::Null,
            },
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let blocks = client.wait_blocks(3, 1, Duration::from_secs(5)).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block_id, 4);
    }

    #[test]
    fn clock_snapshot_reads_all_clock_accounts() {
        let node = FakeNode::start(|method, params| match method {
            "getLastBlockId" => json!(12),
            "getAccount" => {
                let account_id = params[0].as_str().unwrap();
                assert!(clock_account_ids().contains(&account_id.to_string()));
                // ClockAccountData { block_id: 11, timestamp: 999 }
                let mut data = Vec::new();
                data.extend_from_slice(&11_u64.to_le_bytes());
                data.extend_from_slice(&999_u64.to_le_bytes());
                json!({
                    "nonce": 4,
                    "data": { "shards": { "ClockProgram1111111111111111111111111111111": data } },
                })
            }
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let snapshot = client.clock_snapshot(ClockReadMode::Latest).unwrap();
        assert_eq!(snapshot.read_block_id, 12);
        assert_eq!(snapshot.accounts.len(), 3);
        for account in &snapshot.accounts {
            assert_eq!(account.block_id, Some(11));
            assert_eq!(account.timestamp, Some(999));
            assert_eq!(account.nonce, 4);
        }
    }

    #[test]
    fn stable_clock_read_times_out_when_head_keeps_ticking() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        let counter = Arc::new(AtomicU64::new(0));
        let counter_for_node = counter.clone();

        let node = FakeNode::start(move |method, _| match method {
            // Head changes on every read — never stable.
            "getLastBlockId" => json!(counter_for_node.fetch_add(1, Ordering::Relaxed)),
            "getAccount" => json!({ "nonce": 0, "data": { "shards": {} } }),
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let err = client
            .clock_snapshot(ClockReadMode::Stable {
                samples: 2,
                timeout: Duration::from_millis(300),
            })
            .unwrap_err();
        let message = err.message();
        assert!(
            message.contains("head kept advancing") || message.contains("no stable clock"),
            "{message}"
        );
    }
}
