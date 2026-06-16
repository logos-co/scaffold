//! Deterministic block and clock context for test-node replay.
//!
//! Sequencer execution uses real block ids and wall-clock timestamps.
//! Clock-sensitive programs need the exact sequencer context to compare
//! state correctly, so tests must be able to observe blocks (including
//! empty post-genesis blocks, which still advance clock state via the
//! mandatory clock transaction) and read the clock accounts at a stable
//! boundary.
//!
//! Wire facts this module is built on (pinned LEZ sequencer):
//!
//! - A block is borsh: header (`block_id: u64`, two 32-byte hashes,
//!   `timestamp: u64`, 64-byte signature — 144 bytes total), then
//!   `Vec<NSSATransaction>` (u32 count + transactions), then a 1-byte
//!   bedrock status and a 32-byte parent id.
//! - `NSSATransaction` is a borsh enum: tag 0 = `Public`, 1 =
//!   `PrivacyPreserving`, 2 = `ProgramDeployment`. The `Public` and
//!   `ProgramDeployment` variants have fully fixed-shape layouts
//!   (length-prefixed vectors of fixed-size elements), so their byte spans
//!   — and therefore their hashes (`sha256` of the variant bytes) — can be
//!   recovered without the upstream types. `PrivacyPreserving` transactions
//!   carry nested proof structures; the walker stops there and reports the
//!   block as partially parsed rather than guessing.
//! - The genesis block is constructed with **zero** transactions; every
//!   later block carries the mandatory sequencer clock transaction plus any
//!   user transactions. The clock transaction is a `Public` transaction
//!   whose account set is exactly the three `/LEZ/ClockProgramAccount/…`
//!   accounts.
//! - Each clock account's `data` is borsh `ClockAccountData { block_id:
//!   u64, timestamp: u64 }`.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

use super::client::{parse_block_prefix, BlockContext, RpcError, TestNodeClient};

/// Borsh size of the block header (`u64` + 32 + 32 + `u64` + 64-byte
/// signature).
const BLOCK_HEADER_LEN: usize = 8 + 32 + 32 + 8 + 64;

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
    ProgramDeployment,
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
}

/// Observable facts about one block.
#[derive(Clone, Debug, Serialize)]
pub struct BlockInfo {
    pub block_id: u64,
    pub timestamp: u64,
    /// Total number of transactions in the block body.
    pub transaction_count: u32,
    /// `true` for the genesis block — the only block constructed with zero
    /// transactions (no clock tick happens at genesis; tests must not
    /// replay one).
    pub is_genesis: bool,
    /// `true` when the block carries the mandatory sequencer clock
    /// transaction (every post-genesis block; empty blocks still advance
    /// clock state through it).
    pub has_clock_transaction: bool,
    /// `true` when the block carries at least one user transaction beyond
    /// the clock transaction.
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
    pub balance: u128,
    pub nonce: u128,
    /// `ClockAccountData.block_id` parsed from the account data — the block
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
    let balance = read_u128(value.get("balance")).ok_or("missing numeric `balance`")?;
    let nonce = read_u128(value.get("nonce")).ok_or("missing numeric `nonce`")?;
    let data: Vec<u8> = value
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_u64().map(|byte| byte as u8))
                .collect::<Option<Vec<u8>>>()
        })
        .unwrap_or(Some(Vec::new()))
        .ok_or("`data` is not a byte array")?;

    // ClockAccountData { block_id: u64, timestamp: u64 } — borsh, 16 bytes.
    // An unticked clock account has empty data.
    let (block_id, timestamp) = if data.len() >= 16 {
        (
            Some(u64::from_le_bytes(data[0..8].try_into().unwrap())),
            Some(u64::from_le_bytes(data[8..16].try_into().unwrap())),
        )
    } else {
        (None, None)
    };

    Ok(ClockAccount {
        account_id: account_id.to_string(),
        balance,
        nonce,
        block_id,
        timestamp,
    })
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

    let is_genesis = transaction_count == 0;
    let has_clock_transaction = !is_genesis;
    let has_user_transactions = transaction_count >= 2;

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

/// Parse one `NSSATransaction` starting at `offset`. Returns the summary and
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
        // Public: program_id [u32;8], Vec<AccountId{32}>, Vec<Nonce{16}>,
        // Vec<u32> instruction data, Vec<(Signature{64}, PublicKey{32})>.
        0 => {
            let mut cursor = body_start;
            cursor = skip_fixed(bytes, cursor, 32, "program_id")?;
            let (account_ids_start, account_count) =
                (cursor + 4, read_vec_len(bytes, cursor, "account_ids")?);
            cursor = skip_vec(bytes, cursor, 32, "account_ids")?;
            cursor = skip_vec(bytes, cursor, 16, "nonces")?;
            cursor = skip_vec(bytes, cursor, 4, "instruction_data")?;
            cursor = skip_vec(bytes, cursor, 96, "witness_set")?;

            let is_clock = is_clock_account_set(bytes, account_ids_start, account_count);
            Ok(Some((
                TxSummary {
                    hash: sha256_hex(&bytes[body_start..cursor]),
                    kind: TxKind::Public,
                    is_clock,
                },
                cursor,
            )))
        }
        1 => Ok(None),
        // ProgramDeployment: Vec<u8> bytecode.
        2 => {
            let cursor = skip_vec(bytes, body_start, 1, "bytecode")?;
            Ok(Some((
                TxSummary {
                    hash: sha256_hex(&bytes[body_start..cursor]),
                    kind: TxKind::ProgramDeployment,
                    is_clock: false,
                },
                cursor,
            )))
        }
        other => Err(format!("unknown transaction tag {other}")),
    }
}

/// `true` when the account id vector is exactly the three clock accounts in
/// canonical order — the shape of the sequencer's clock transaction.
fn is_clock_account_set(bytes: &[u8], start: usize, count: u32) -> bool {
    if count != 3 {
        return false;
    }
    CLOCK_ACCOUNT_RAW_IDS.iter().enumerate().all(|(i, raw)| {
        bytes
            .get(start + i * 32..start + (i + 1) * 32)
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

    /// Borsh-encode a synthetic Public transaction with the given account
    /// ids; returns the full enum encoding (tag + body).
    fn fake_public_tx(account_ids: &[[u8; 32]]) -> Vec<u8> {
        let mut tx = vec![0_u8]; // tag: Public
        tx.extend_from_slice(&[0x11; 32]); // program_id
        tx.extend_from_slice(&(account_ids.len() as u32).to_le_bytes());
        for id in account_ids {
            tx.extend_from_slice(id);
        }
        tx.extend_from_slice(&1_u32.to_le_bytes()); // one nonce
        tx.extend_from_slice(&[0; 16]);
        tx.extend_from_slice(&2_u32.to_le_bytes()); // two instruction words
        tx.extend_from_slice(&[0; 8]);
        tx.extend_from_slice(&1_u32.to_le_bytes()); // one (sig, pk) pair
        tx.extend_from_slice(&[0x22; 96]);
        tx
    }

    fn fake_clock_tx() -> Vec<u8> {
        let ids: Vec<[u8; 32]> = CLOCK_ACCOUNT_RAW_IDS.iter().map(|raw| **raw).collect();
        fake_public_tx(&ids)
    }

    fn fake_deploy_tx(bytecode: &[u8]) -> Vec<u8> {
        let mut tx = vec![2_u8];
        tx.extend_from_slice(&(bytecode.len() as u32).to_le_bytes());
        tx.extend_from_slice(bytecode);
        tx
    }

    fn fake_block(block_id: u64, timestamp: u64, txs: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&block_id.to_le_bytes());
        bytes.extend_from_slice(&[0xAA; 32]);
        bytes.extend_from_slice(&[0xBB; 32]);
        bytes.extend_from_slice(&timestamp.to_le_bytes());
        bytes.extend_from_slice(&[0xCC; 64]); // signature
        bytes.extend_from_slice(&(txs.len() as u32).to_le_bytes());
        for tx in txs {
            bytes.extend_from_slice(tx);
        }
        bytes.push(0); // bedrock status
        bytes.extend_from_slice(&[0; 32]); // bedrock parent id
        bytes
    }

    #[test]
    fn genesis_block_is_explicit() {
        let info = parse_block_info(&fake_block(1, 1_700_000_000_000, &[])).unwrap();
        assert!(info.is_genesis);
        assert!(!info.has_clock_transaction);
        assert!(!info.has_user_transactions);
        assert_eq!(info.transaction_count, 0);
        assert!(info.fully_parsed);
    }

    #[test]
    fn empty_post_genesis_block_reports_clock_only() {
        let info = parse_block_info(&fake_block(5, 1_700_000_000_500, &[fake_clock_tx()])).unwrap();
        assert!(!info.is_genesis);
        assert!(info.has_clock_transaction);
        assert!(!info.has_user_transactions);
        assert_eq!(info.transactions.len(), 1);
        assert!(info.transactions[0].is_clock);
        assert_eq!(info.transactions[0].kind, TxKind::Public);
        assert!(info.fully_parsed);
    }

    #[test]
    fn user_transactions_are_distinguished_from_clock() {
        let user_tx = fake_public_tx(&[[0x55; 32], [0x66; 32]]);
        let deploy_tx = fake_deploy_tx(&[1, 2, 3, 4, 5]);
        let info = parse_block_info(&fake_block(
            7,
            1_700_000_001_000,
            &[fake_clock_tx(), user_tx.clone(), deploy_tx.clone()],
        ))
        .unwrap();

        assert!(info.has_user_transactions);
        assert_eq!(info.transactions.len(), 3);
        assert!(info.transactions[0].is_clock);
        assert!(!info.transactions[1].is_clock);
        assert_eq!(info.transactions[2].kind, TxKind::ProgramDeployment);

        // Hashes are sha256 over the variant bytes (without the enum tag).
        assert_eq!(info.transactions[1].hash, sha256_hex(&user_tx[1..]));
        assert_eq!(info.transactions[2].hash, sha256_hex(&deploy_tx[1..]));
    }

    #[test]
    fn privacy_preserving_tx_marks_block_partially_parsed() {
        let pp_tx = vec![1_u8, 0xDE, 0xAD]; // tag 1 + opaque bytes
        let info =
            parse_block_info(&fake_block(9, 1_700_000_002_000, &[fake_clock_tx(), pp_tx])).unwrap();
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
                    "program_owner": [0,0,0,0,0,0,0,0],
                    "balance": 0,
                    "data": data,
                    "nonce": 4,
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
            "getAccount" => json!({
                "program_owner": [0,0,0,0,0,0,0,0],
                "balance": 0,
                "data": [],
                "nonce": 0,
            }),
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
