//! Stable account and proof snapshots for parity assertions.
//!
//! Local-sequencer integration tests do not only need transactions to
//! commit — they need to assert that final account state and proofs match
//! the reference state transition. Because the sequencer keeps producing
//! clock blocks in the background, naive reads can compare state from
//! different block boundaries. Every read in this module is **block
//! scoped**: the head is checked before and after the read; when it moved,
//! the read is retried, and persistent movement surfaces as a structured
//! retryable error instead of silently inconsistent data.
//!
//! The pinned sequencer serves only the *latest* state (`getAccount` /
//! `getProofForCommitment` have no historical parameter). `ReadAt::Block`
//! therefore validates that the node's head **is** the requested block and
//! fails with a targeted error otherwise — which still gives tests an exact
//! block-scoped read right after `blocks wait` / `tx submit-and-wait`
//! pinned the boundary.

use std::time::Duration;

use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};

use super::client::{RpcError, TestNodeClient};

/// Which block boundary a read must be scoped to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadAt {
    /// The current head, whatever it is — but consistent: the head must not
    /// move during the read.
    Latest,
    /// Exactly this block id; fails when the node's head is elsewhere.
    Block(u64),
}

/// An account's value at the read boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AccountValue {
    /// The account exists (has been written at least once).
    Present {
        /// Canonical borsh encoding of the account
        /// (`program_owner [u32;8]`, `balance u128`, `data Vec<u8>`,
        /// `nonce u128`), base64 — lossless for byte-level comparisons.
        encoded: String,
        /// Program owner as hex of the 32 little-endian bytes.
        program_owner: String,
        balance: u128,
        nonce: u128,
        /// Account data bytes, base64 (lossless).
        data: String,
        data_len: usize,
    },
    /// The account has never been written: the node returned the default
    /// account (zero balance, zero nonce, empty data, default owner).
    Missing,
    /// The node returned a payload this client could not decode. The raw
    /// JSON is preserved for inspection.
    DecodeError { message: String, raw: Value },
}

/// One account read, with the block boundary it was scoped to.
#[derive(Clone, Debug, Serialize)]
pub struct AccountRead {
    pub account_id: String,
    /// Block id the read was performed at (head was identical before and
    /// after the read).
    pub block_id: u64,
    #[serde(flatten)]
    pub value: AccountValue,
}

/// A batch of account reads sharing one consistent block boundary.
#[derive(Clone, Debug, Serialize)]
pub struct BatchAccountRead {
    /// The single block id every entry was read at.
    pub block_id: u64,
    pub accounts: Vec<BatchAccountEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BatchAccountEntry {
    pub account_id: String,
    #[serde(flatten)]
    pub value: AccountValue,
}

/// A membership-proof read for one commitment.
#[derive(Clone, Debug, Serialize)]
pub struct ProofRead {
    /// The queried commitment, hex.
    pub commitment: String,
    /// Block id the read was scoped to.
    pub block_id: u64,
    /// `None` when the node does not know the commitment.
    pub proof: Option<ProofValue>,
}

/// A membership proof (`(leaf_index, sibling_path)` on the wire).
#[derive(Clone, Debug, Serialize)]
pub struct ProofValue {
    pub leaf_index: u64,
    /// Sibling hashes, hex, leaf to root.
    pub path: Vec<String>,
}

impl TestNodeClient {
    /// Read one account at a stable block boundary.
    pub fn account(&self, account_id: &str, at: ReadAt) -> Result<AccountRead, RpcError> {
        let (block_id, value) =
            self.read_at_boundary(at, |client| client.fetch_account_value(account_id))?;
        Ok(AccountRead {
            account_id: account_id.to_string(),
            block_id,
            value,
        })
    }

    /// Read several accounts at ONE consistent block boundary — the head is
    /// identical before and after all reads, so every entry reflects the
    /// same block.
    pub fn accounts(
        &self,
        account_ids: &[String],
        at: ReadAt,
    ) -> Result<BatchAccountRead, RpcError> {
        let (block_id, accounts) = self.read_at_boundary(at, |client| {
            account_ids
                .iter()
                .map(|account_id| {
                    client
                        .fetch_account_value(account_id)
                        .map(|value| BatchAccountEntry {
                            account_id: account_id.clone(),
                            value,
                        })
                })
                .collect::<Result<Vec<_>, RpcError>>()
        })?;
        Ok(BatchAccountRead { block_id, accounts })
    }

    /// Read the membership proof for a commitment (hex or base58 of 32
    /// bytes) at a stable block boundary. Distinguishes: invalid commitment
    /// (local error, no RPC), missing commitment (`proof: None`), and
    /// transport failures.
    pub fn proof(&self, commitment: &str, at: ReadAt) -> Result<ProofRead, RpcError> {
        let bytes = parse_commitment(commitment)?;
        let commitment_hex = hex_string(&bytes);
        let param: Vec<u64> = bytes.iter().map(|byte| u64::from(*byte)).collect();

        let (block_id, proof) = self.read_at_boundary(at, |client| {
            let result = client.call("getProofForCommitment", json!([param]))?;
            match result {
                Value::Null => Ok(None),
                other => parse_proof(&other)
                    .map(Some)
                    .map_err(|message| RpcError::Other {
                        operation: "getProofForCommitment".to_string(),
                        message,
                    }),
            }
        })?;

        Ok(ProofRead {
            commitment: commitment_hex,
            block_id,
            proof,
        })
    }

    fn fetch_account_value(&self, account_id: &str) -> Result<AccountValue, RpcError> {
        let result = self.call("getAccount", json!([account_id]))?;
        Ok(parse_account_value(&result))
    }

    /// Run `reads` scoped to a block boundary: the head must be identical
    /// before and after the reads. `Latest` retries a bounded number of
    /// times when a clock block lands mid-read; `Block(n)` additionally
    /// requires the head to be exactly `n`.
    fn read_at_boundary<T>(
        &self,
        at: ReadAt,
        reads: impl Fn(&Self) -> Result<T, RpcError>,
    ) -> Result<(u64, T), RpcError> {
        const ATTEMPTS: u32 = 5;
        let mut last_heads = (0, 0);
        for _ in 0..ATTEMPTS {
            let head_before = self.last_block_id()?;
            if let ReadAt::Block(wanted) = at {
                if head_before != wanted {
                    return Err(RpcError::Other {
                        operation: "readAtBlock".to_string(),
                        message: format!(
                            "node head is {head_before}, requested block {wanted}; this \
                             sequencer serves only latest state. Pin the boundary right after \
                             `blocks wait` / a committed transaction, or use --at latest."
                        ),
                    });
                }
            }
            let value = reads(self)?;
            let head_after = self.last_block_id()?;
            if head_before == head_after {
                return Ok((head_after, value));
            }
            last_heads = (head_before, head_after);
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(RpcError::Other {
            operation: "readAtBoundary".to_string(),
            message: format!(
                "head kept advancing during reads ({} -> {} on the last attempt); \
                 retry — the node is producing blocks faster than the read loop",
                last_heads.0, last_heads.1
            ),
        })
    }
}

fn parse_account_value(value: &Value) -> AccountValue {
    let Some(object) = value.as_object() else {
        return AccountValue::DecodeError {
            message: "account payload is not a JSON object".to_string(),
            raw: value.clone(),
        };
    };

    let balance = match read_u128_field(object.get("balance")) {
        Some(balance) => balance,
        None => {
            return AccountValue::DecodeError {
                message: "missing or non-numeric `balance`".to_string(),
                raw: value.clone(),
            }
        }
    };
    let nonce = match read_u128_field(object.get("nonce")) {
        Some(nonce) => nonce,
        None => {
            return AccountValue::DecodeError {
                message: "missing or non-numeric `nonce`".to_string(),
                raw: value.clone(),
            }
        }
    };
    let program_owner: Option<Vec<u32>> = object
        .get("program_owner")
        .and_then(Value::as_array)
        .map(|words| {
            words
                .iter()
                .map(|word| word.as_u64().and_then(|w| u32::try_from(w).ok()))
                .collect::<Option<Vec<u32>>>()
        })
        .unwrap_or_default();
    let Some(program_owner) = program_owner.filter(|words| words.len() == 8) else {
        return AccountValue::DecodeError {
            message: "missing or malformed `program_owner` (expected 8 u32 words)".to_string(),
            raw: value.clone(),
        };
    };
    let data: Option<Vec<u8>> = object
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_u64().and_then(|byte| u8::try_from(byte).ok()))
                .collect::<Option<Vec<u8>>>()
        })
        .unwrap_or(Some(Vec::new()));
    let Some(data) = data else {
        return AccountValue::DecodeError {
            message: "`data` is not a byte array".to_string(),
            raw: value.clone(),
        };
    };

    let is_default = balance == 0
        && nonce == 0
        && data.is_empty()
        && program_owner.iter().all(|word| *word == 0);
    if is_default {
        return AccountValue::Missing;
    }

    // Canonical borsh of the account: program_owner words LE, balance u128
    // LE, Vec<u8> data (u32 len + bytes), nonce u128 LE.
    let mut encoded = Vec::with_capacity(32 + 16 + 4 + data.len() + 16);
    for word in &program_owner {
        encoded.extend_from_slice(&word.to_le_bytes());
    }
    encoded.extend_from_slice(&balance.to_le_bytes());
    encoded.extend_from_slice(&(data.len() as u32).to_le_bytes());
    encoded.extend_from_slice(&data);
    encoded.extend_from_slice(&nonce.to_le_bytes());

    let owner_bytes: Vec<u8> = program_owner
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();

    AccountValue::Present {
        encoded: base64::engine::general_purpose::STANDARD.encode(&encoded),
        program_owner: hex_string(&owner_bytes),
        balance,
        nonce,
        data: base64::engine::general_purpose::STANDARD.encode(&data),
        data_len: data.len(),
    }
}

fn read_u128_field(value: Option<&Value>) -> Option<u128> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return Some(u128::from(number));
    }
    serde_json::from_value::<u128>(value.clone()).ok()
}

/// `MembershipProof = (usize, Vec<[u8; 32]>)` on the wire: `[index, [path…]]`.
fn parse_proof(value: &Value) -> Result<ProofValue, String> {
    let pair = value
        .as_array()
        .filter(|items| items.len() == 2)
        .ok_or("proof payload is not a [leaf_index, path] pair")?;
    let leaf_index = pair[0].as_u64().ok_or("proof leaf index is not a number")?;
    let path = pair[1]
        .as_array()
        .ok_or("proof path is not an array")?
        .iter()
        .map(|node| {
            let bytes: Option<Vec<u8>> = node.as_array().map(|items| {
                items
                    .iter()
                    .map(|item| item.as_u64().and_then(|byte| u8::try_from(byte).ok()))
                    .collect::<Option<Vec<u8>>>()
            })?;
            bytes
                .filter(|bytes| bytes.len() == 32)
                .map(|b| hex_string(&b))
        })
        .collect::<Option<Vec<String>>>()
        .ok_or("proof path nodes are not 32-byte arrays")?;
    Ok(ProofValue { leaf_index, path })
}

/// Accept a commitment as 64 hex chars or base58 of 32 bytes. Anything else
/// is an *invalid commitment* — a local error, reported before any RPC.
fn parse_commitment(input: &str) -> Result<[u8; 32], RpcError> {
    let trimmed = input.trim();
    let invalid = |detail: String| RpcError::Other {
        operation: "parseCommitment".to_string(),
        message: format!("invalid commitment `{trimmed}`: {detail}"),
    };

    if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        let mut bytes = [0_u8; 32];
        for (i, chunk) in trimmed.as_bytes().chunks(2).enumerate() {
            let hex_pair =
                std::str::from_utf8(chunk).map_err(|_| invalid("non-UTF8 hex".to_string()))?;
            bytes[i] = u8::from_str_radix(hex_pair, 16)
                .map_err(|err| invalid(format!("bad hex byte: {err}")))?;
        }
        return Ok(bytes);
    }

    match bs58::decode(trimmed).into_vec() {
        Ok(decoded) if decoded.len() == 32 => {
            let mut bytes = [0_u8; 32];
            bytes.copy_from_slice(&decoded);
            Ok(bytes)
        }
        Ok(decoded) => Err(invalid(format!(
            "base58 decodes to {} bytes, expected 32",
            decoded.len()
        ))),
        Err(_) => Err(invalid(
            "expected 64 hex characters or base58 of 32 bytes".to_string(),
        )),
    }
}

fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testnode::test_support::FakeNode;

    fn present_account() -> Value {
        json!({
            "program_owner": [1, 0, 0, 0, 0, 0, 0, 0],
            "balance": 10_000,
            "data": [1, 2, 3],
            "nonce": 7,
        })
    }

    fn default_account() -> Value {
        json!({
            "program_owner": [0, 0, 0, 0, 0, 0, 0, 0],
            "balance": 0,
            "data": [],
            "nonce": 0,
        })
    }

    #[test]
    fn account_read_includes_block_id_and_lossless_bytes() {
        let node = FakeNode::start(|method, _| match method {
            "getLastBlockId" => json!(42),
            "getAccount" => present_account(),
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let read = client.account("someid", ReadAt::Latest).unwrap();
        assert_eq!(read.block_id, 42);
        match read.value {
            AccountValue::Present {
                balance,
                nonce,
                data,
                data_len,
                encoded,
                program_owner,
            } => {
                assert_eq!(balance, 10_000);
                assert_eq!(nonce, 7);
                assert_eq!(data_len, 3);
                assert_eq!(
                    base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .unwrap(),
                    vec![1, 2, 3]
                );
                assert!(program_owner.starts_with("01000000"));
                // Lossless canonical borsh: owner(32) + balance(16) +
                // len(4) + data(3) + nonce(16).
                let encoded_bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap();
                assert_eq!(encoded_bytes.len(), 32 + 16 + 4 + 3 + 16);
                assert_eq!(encoded_bytes[0], 1); // first owner word LE
                assert_eq!(encoded_bytes[32], 0x10); // balance 10000 = 0x2710 LE
                assert_eq!(encoded_bytes[33], 0x27);
            }
            other => panic!("expected Present, got {other:?}"),
        }
    }

    #[test]
    fn default_account_reports_missing() {
        let node = FakeNode::start(|method, _| match method {
            "getLastBlockId" => json!(9),
            "getAccount" => default_account(),
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let read = client.account("someid", ReadAt::Latest).unwrap();
        assert_eq!(read.value, AccountValue::Missing);
    }

    #[test]
    fn malformed_account_reports_decode_error_with_raw_payload() {
        let node = FakeNode::start(|method, _| match method {
            "getLastBlockId" => json!(9),
            "getAccount" => json!({ "balance": "not-a-number" }),
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let read = client.account("someid", ReadAt::Latest).unwrap();
        match read.value {
            AccountValue::DecodeError { message, raw } => {
                assert!(message.contains("balance"), "{message}");
                assert_eq!(raw["balance"], json!("not-a-number"));
            }
            other => panic!("expected DecodeError, got {other:?}"),
        }
    }

    #[test]
    fn batch_read_uses_one_boundary() {
        let node = FakeNode::start(|method, params| match method {
            "getLastBlockId" => json!(50),
            "getAccount" => {
                if params[0].as_str() == Some("a") {
                    present_account()
                } else {
                    default_account()
                }
            }
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let batch = client
            .accounts(&["a".to_string(), "b".to_string()], ReadAt::Latest)
            .unwrap();
        assert_eq!(batch.block_id, 50);
        assert_eq!(batch.accounts.len(), 2);
        assert!(matches!(
            batch.accounts[0].value,
            AccountValue::Present { .. }
        ));
        assert_eq!(batch.accounts[1].value, AccountValue::Missing);
    }

    #[test]
    fn read_at_block_rejects_wrong_head() {
        let node = FakeNode::start(|method, _| match method {
            "getLastBlockId" => json!(10),
            "getAccount" => default_account(),
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let err = client.account("someid", ReadAt::Block(7)).unwrap_err();
        assert!(err.message().contains("head is 10"), "{err}");

        // Matching head succeeds and reports the requested block.
        let read = client.account("someid", ReadAt::Block(10)).unwrap();
        assert_eq!(read.block_id, 10);
    }

    #[test]
    fn racing_head_is_a_structured_retryable_error() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        let counter = Arc::new(AtomicU64::new(0));
        let counter_for_node = counter.clone();

        let node = FakeNode::start(move |method, _| match method {
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
        let err = client.account("someid", ReadAt::Latest).unwrap_err();
        assert!(err.message().contains("head kept advancing"), "{err}");
    }

    #[test]
    fn proof_read_distinguishes_missing_and_present() {
        let commitment_hex = "ab".repeat(32);
        let node = FakeNode::start(|method, params| match method {
            "getLastBlockId" => json!(20),
            "getProofForCommitment" => {
                // Commitment arrives as an array of 32 numbers.
                let bytes = params[0].as_array().unwrap();
                assert_eq!(bytes.len(), 32);
                if bytes[0].as_u64() == Some(0xAB) {
                    json!([3, [vec![1_u8; 32], vec![2_u8; 32]]])
                } else {
                    Value::Null
                }
            }
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);

        let read = client.proof(&commitment_hex, ReadAt::Latest).unwrap();
        assert_eq!(read.block_id, 20);
        let proof = read.proof.expect("proof present");
        assert_eq!(proof.leaf_index, 3);
        assert_eq!(proof.path.len(), 2);
        assert_eq!(proof.path[0], "01".repeat(32));

        let missing = client.proof(&"cd".repeat(32), ReadAt::Latest).unwrap();
        assert!(missing.proof.is_none());
    }

    #[test]
    fn invalid_commitment_is_a_local_error() {
        let client = TestNodeClient::new("http://127.0.0.1:1");
        let err = client
            .proof("zz-not-a-commitment", ReadAt::Latest)
            .unwrap_err();
        assert!(err.message().contains("invalid commitment"), "{err}");
        assert_eq!(err.operation(), "parseCommitment");
    }

    #[test]
    fn commitment_accepts_hex_and_base58() {
        let bytes = parse_commitment(&"ab".repeat(32)).unwrap();
        assert_eq!(bytes[0], 0xAB);

        let base58 = bs58::encode([7_u8; 32]).into_string();
        let bytes = parse_commitment(&base58).unwrap();
        assert_eq!(bytes, [7_u8; 32]);

        assert!(parse_commitment("abc").is_err());
    }
}
