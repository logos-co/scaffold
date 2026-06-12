//! JSON-RPC client for sequencer test nodes with structured transaction
//! outcomes.
//!
//! The core purpose of a local-sequencer test path is divergence detection:
//! a test must know whether the sequencer **committed** a transaction,
//! **rejected** it (and in which phase), could not decide in time
//! (**timeout**), or could not even be reached (**transport error**). This
//! module never collapses those cases.
//!
//! Wire facts this client is built on (pinned LEZ sequencer RPC):
//!
//! - `sendTransaction` takes one positional param — the transaction borsh
//!   bytes, base64 encoded — and returns the transaction hash as a hex
//!   string. Stateless check failures (e.g. `InvalidSignature`) surface as a
//!   JSON-RPC error before the transaction enters the mempool.
//! - `getTransaction` returns the borsh+base64 echo of a transaction once it
//!   is part of a committed block, `null` before that.
//! - `getBlock` returns the whole block as borsh+base64. The header prefix
//!   is fixed-layout: `block_id: u64` at bytes 0..8 and `timestamp: u64` at
//!   bytes 72..80 (after two 32-byte hashes). Because borsh serialization is
//!   deterministic and the block body embeds each transaction's bytes
//!   verbatim, a committed transaction's bytes appear as a contiguous
//!   subsequence of its block's bytes — which lets the client locate the
//!   containing block without a full borsh decoder.
//!
//! Observation rule for rejection vs timeout: a transaction that the
//! sequencer accepted into its mempool is expected in one of the next
//! blocks. `WaitOptions::rejection_blocks` (default 3) sets how many new
//! blocks past the submission boundary the client must observe **without**
//! the transaction before reporting a stateful rejection. If the overall
//! timeout elapses before that many blocks were produced, the outcome is
//! `Timeout` — the node could not decide in time, and the test should say
//! so rather than guess.

use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};

/// Block id and timestamp a transaction executed against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct BlockContext {
    pub block_id: u64,
    pub timestamp: u64,
}

/// Which validation phase rejected the transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionPhase {
    /// Rejected synchronously at submission (signature/size/decode checks),
    /// before entering the mempool.
    Stateless,
    /// Accepted into the mempool but not included after the observation
    /// window — dropped during stateful validation/execution.
    Stateful,
}

/// Terminal outcome of submitting (or waiting for) a transaction.
///
/// Serializes to the documented `--json` shapes, e.g.
/// `{"status":"committed","tx_hash":"…","block_id":72,"timestamp":…}`.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TransactionOutcome {
    /// The transaction is part of a committed block.
    Committed {
        tx_hash: String,
        #[serde(flatten)]
        block: BlockContext,
    },
    /// The sequencer rejected the transaction.
    Rejected {
        #[serde(skip_serializing_if = "Option::is_none")]
        tx_hash: Option<String>,
        phase: RejectionPhase,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Head block id at the moment the stateful rejection was concluded.
        #[serde(skip_serializing_if = "Option::is_none")]
        observed_after_block_id: Option<u64>,
    },
    /// The node could not decide within the deadline.
    Timeout {
        tx_hash: String,
        last_observed_block_id: u64,
    },
    /// The node could not be reached (or answered malformed data); distinct
    /// from any business-level rejection.
    TransportError { operation: String, message: String },
    /// The node echoed transaction bytes that differ from what was
    /// submitted.
    WireMismatch {
        #[serde(skip_serializing_if = "Option::is_none")]
        submitted_hash: Option<String>,
        returned_hash: String,
        /// Base64 of the submitted borsh bytes.
        submitted_tx: String,
        /// Base64 of the borsh bytes the node echoed back.
        echoed_tx: String,
    },
}

impl TransactionOutcome {
    /// `true` only for [`TransactionOutcome::Committed`].
    pub fn is_committed(&self) -> bool {
        matches!(self, Self::Committed { .. })
    }
}

/// Transaction bytes plus their wire encoding.
#[derive(Clone, Debug)]
pub struct TransactionBytes {
    bytes: Vec<u8>,
}

impl TransactionBytes {
    /// From raw borsh bytes.
    pub fn borsh(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// From a base64 string of borsh bytes (the wire encoding).
    pub fn borsh_base64(encoded: &str) -> Result<Self, RpcError> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|err| RpcError::Other {
                operation: "decodeTransaction".to_string(),
                message: format!("invalid base64 transaction encoding: {err}"),
            })?;
        Ok(Self { bytes })
    }

    /// The wire encoding (base64 of the borsh bytes).
    pub fn to_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.bytes)
    }

    /// The raw borsh bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Options for waiting on a transaction.
#[derive(Clone, Debug)]
pub struct WaitOptions {
    /// Only blocks **after** this id are considered when locating the
    /// transaction and counting the rejection window. Defaults to the head
    /// observed at submission/wait start.
    pub after_block: Option<u64>,
    /// Overall deadline for a terminal outcome.
    pub timeout: Duration,
    /// How many new blocks past `after_block` must be observed without the
    /// transaction before concluding a stateful rejection.
    pub rejection_blocks: u64,
    /// Poll interval.
    pub poll_interval: Duration,
}

impl Default for WaitOptions {
    fn default() -> Self {
        Self {
            after_block: None,
            timeout: Duration::from_secs(60),
            rejection_blocks: 3,
            poll_interval: Duration::from_millis(500),
        }
    }
}

/// Error from a single RPC operation.
#[derive(Clone, Debug)]
pub enum RpcError {
    /// Could not reach the node or read its response.
    Transport { operation: String, message: String },
    /// The node answered with a JSON-RPC error object.
    JsonRpc {
        operation: String,
        code: Option<i64>,
        message: String,
    },
    /// The node answered 200 but with an unusable payload, or local
    /// encoding/decoding failed.
    Other { operation: String, message: String },
}

impl RpcError {
    pub fn operation(&self) -> &str {
        match self {
            Self::Transport { operation, .. }
            | Self::JsonRpc { operation, .. }
            | Self::Other { operation, .. } => operation,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Transport { message, .. } | Self::Other { message, .. } => message.clone(),
            Self::JsonRpc { code, message, .. } => match code {
                Some(code) => format!("JSON-RPC error {code}: {message}"),
                None => format!("JSON-RPC error: {message}"),
            },
        }
    }

    fn into_transport_outcome(self) -> TransactionOutcome {
        TransactionOutcome::TransportError {
            operation: self.operation().to_string(),
            message: self.message(),
        }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed: {}", self.operation(), self.message())
    }
}

impl std::error::Error for RpcError {}

/// Result of a bare `submit` (no waiting).
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SubmitOutcome {
    /// The node accepted the transaction into its mempool.
    Submitted { tx_hash: String },
    /// Synchronous stateless rejection.
    Rejected {
        phase: RejectionPhase,
        reason: String,
    },
    /// The node could not be reached.
    TransportError { operation: String, message: String },
}

/// Minimal JSON-RPC client for one sequencer node.
#[derive(Clone, Debug)]
pub struct TestNodeClient {
    rpc_url: String,
    agent: ureq::Agent,
}

impl TestNodeClient {
    pub fn new(rpc_url: impl Into<String>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        Self {
            rpc_url: rpc_url.into(),
            agent,
        }
    }

    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// Raw JSON-RPC call with positional params.
    pub(crate) fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1_u64,
            "method": method,
            "params": params,
        });

        let response = self
            .agent
            .post(&self.rpc_url)
            .set("content-type", "application/json")
            .send_json(payload)
            .map_err(|err| match err {
                ureq::Error::Transport(transport) => RpcError::Transport {
                    operation: method.to_string(),
                    message: transport.to_string(),
                },
                ureq::Error::Status(code, response) => {
                    // jsonrpsee answers HTTP 4xx/5xx for some protocol-level
                    // errors with a JSON-RPC error body; surface that body
                    // when present.
                    let body = response.into_string().unwrap_or_default();
                    match serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|value| extract_jsonrpc_error(method, &value))
                    {
                        Some(error) => error,
                        None => RpcError::Transport {
                            operation: method.to_string(),
                            message: format!("HTTP {code}: {}", one_line(&body)),
                        },
                    }
                }
            })?;

        let body: Value = response.into_json().map_err(|err| RpcError::Other {
            operation: method.to_string(),
            message: format!("failed to decode JSON-RPC response: {err}"),
        })?;

        if let Some(error) = extract_jsonrpc_error(method, &body) {
            return Err(error);
        }

        body.get("result").cloned().ok_or_else(|| RpcError::Other {
            operation: method.to_string(),
            message: format!("response has no `result`: {}", one_line(&body.to_string())),
        })
    }

    /// `getLastBlockId` — current head block id.
    pub fn last_block_id(&self) -> Result<u64, RpcError> {
        let result = self.call("getLastBlockId", json!([]))?;
        result.as_u64().ok_or_else(|| RpcError::Other {
            operation: "getLastBlockId".to_string(),
            message: format!("non-numeric result: {result}"),
        })
    }

    /// `getTransaction` — base64 borsh echo of a committed transaction, or
    /// `None` when the node does not (yet) have it in a block.
    pub fn transaction(&self, tx_hash: &str) -> Result<Option<String>, RpcError> {
        let result = self.call("getTransaction", json!([tx_hash]))?;
        match result {
            Value::Null => Ok(None),
            Value::String(encoded) => Ok(Some(encoded)),
            other => Err(RpcError::Other {
                operation: "getTransaction".to_string(),
                message: format!("unexpected result shape: {}", one_line(&other.to_string())),
            }),
        }
    }

    /// `getBlock` — raw borsh bytes of a block, or `None` for an unknown id.
    pub fn block_bytes(&self, block_id: u64) -> Result<Option<Vec<u8>>, RpcError> {
        let result = self.call("getBlock", json!([block_id]))?;
        match result {
            Value::Null => Ok(None),
            Value::String(encoded) => base64::engine::general_purpose::STANDARD
                .decode(encoded.as_bytes())
                .map(Some)
                .map_err(|err| RpcError::Other {
                    operation: "getBlock".to_string(),
                    message: format!("invalid base64 block payload: {err}"),
                }),
            other => Err(RpcError::Other {
                operation: "getBlock".to_string(),
                message: format!("unexpected result shape: {}", one_line(&other.to_string())),
            }),
        }
    }

    /// Submit a transaction. Returns the node-assigned hash, a structured
    /// stateless rejection, or a transport error — never a panic or a
    /// string to parse.
    pub fn submit(&self, tx: &TransactionBytes) -> SubmitOutcome {
        match self.call("sendTransaction", json!([tx.to_base64()])) {
            Ok(Value::String(tx_hash)) => SubmitOutcome::Submitted { tx_hash },
            Ok(other) => SubmitOutcome::TransportError {
                operation: "sendTransaction".to_string(),
                message: format!("unexpected result shape: {}", one_line(&other.to_string())),
            },
            Err(RpcError::JsonRpc { message, .. }) => SubmitOutcome::Rejected {
                phase: RejectionPhase::Stateless,
                reason: message,
            },
            Err(err) => SubmitOutcome::TransportError {
                operation: err.operation().to_string(),
                message: err.message(),
            },
        }
    }

    /// Wait for a terminal outcome of `tx_hash`, optionally verifying the
    /// node's byte echo against `submitted` (wire-mismatch detection).
    pub fn wait(
        &self,
        tx_hash: &str,
        submitted: Option<&TransactionBytes>,
        options: &WaitOptions,
    ) -> TransactionOutcome {
        let deadline = Instant::now() + options.timeout;

        // Establish the observation boundary.
        let after_block = match options.after_block {
            Some(block) => block,
            None => match self.last_block_id() {
                Ok(head) => head,
                Err(err) => return err.into_transport_outcome(),
            },
        };

        let mut last_observed_head = after_block;
        let mut last_error: Option<RpcError> = None;
        let mut any_successful_poll = false;

        loop {
            match self.transaction(tx_hash) {
                Ok(Some(echoed)) => {
                    if let Some(submitted) = submitted {
                        if echoed != submitted.to_base64() {
                            return TransactionOutcome::WireMismatch {
                                submitted_hash: None,
                                returned_hash: tx_hash.to_string(),
                                submitted_tx: submitted.to_base64(),
                                echoed_tx: echoed,
                            };
                        }
                    }
                    return self.locate_committed(tx_hash, &echoed, after_block);
                }
                Ok(None) => {
                    any_successful_poll = true;
                    match self.last_block_id() {
                        Ok(head) => {
                            last_observed_head = head;
                            if head.saturating_sub(after_block) >= options.rejection_blocks {
                                // Observation window exhausted: the node
                                // produced enough blocks to have included a
                                // valid transaction.
                                return TransactionOutcome::Rejected {
                                    tx_hash: Some(tx_hash.to_string()),
                                    phase: RejectionPhase::Stateful,
                                    reason: None,
                                    observed_after_block_id: Some(head),
                                };
                            }
                        }
                        Err(err) => last_error = Some(err),
                    }
                }
                Err(err) => last_error = Some(err),
            }

            if Instant::now() >= deadline {
                // Distinguish "node reachable but undecided" from "node
                // unreachable the whole time".
                if !any_successful_poll {
                    if let Some(err) = last_error {
                        return err.into_transport_outcome();
                    }
                }
                return TransactionOutcome::Timeout {
                    tx_hash: tx_hash.to_string(),
                    last_observed_block_id: last_observed_head,
                };
            }
            std::thread::sleep(options.poll_interval);
        }
    }

    /// Submit and wait for exactly one terminal outcome.
    pub fn submit_and_wait(
        &self,
        tx: &TransactionBytes,
        options: &WaitOptions,
    ) -> TransactionOutcome {
        // Pin the observation boundary before submission so the containing
        // block can't slip under it.
        let after_block = match options.after_block {
            Some(block) => Some(block),
            None => match self.last_block_id() {
                Ok(head) => Some(head),
                Err(err) => return err.into_transport_outcome(),
            },
        };

        let tx_hash = match self.submit(tx) {
            SubmitOutcome::Submitted { tx_hash } => tx_hash,
            SubmitOutcome::Rejected { phase, reason } => {
                return TransactionOutcome::Rejected {
                    tx_hash: None,
                    phase,
                    reason: Some(reason),
                    observed_after_block_id: None,
                }
            }
            SubmitOutcome::TransportError { operation, message } => {
                return TransactionOutcome::TransportError { operation, message }
            }
        };

        let wait_options = WaitOptions {
            after_block,
            ..options.clone()
        };
        self.wait(&tx_hash, Some(tx), &wait_options)
    }

    /// The transaction is committed; find its containing block after
    /// `after_block` and return the block context.
    fn locate_committed(
        &self,
        tx_hash: &str,
        echoed_base64: &str,
        after_block: u64,
    ) -> TransactionOutcome {
        let tx_bytes =
            match base64::engine::general_purpose::STANDARD.decode(echoed_base64.as_bytes()) {
                Ok(bytes) => bytes,
                Err(err) => {
                    return TransactionOutcome::TransportError {
                        operation: "getTransaction".to_string(),
                        message: format!("node echoed invalid base64: {err}"),
                    }
                }
            };

        let head = match self.last_block_id() {
            Ok(head) => head,
            Err(err) => return err.into_transport_outcome(),
        };

        // Scan the post-boundary range first (the overwhelmingly common
        // case), then fall back to the rest of the chain — the caller may
        // have passed an `after_block` that postdates the commit.
        let post_boundary = (after_block + 1)..=head;
        let pre_boundary = 0..=after_block;
        for block_id in post_boundary.chain(pre_boundary) {
            match self.block_bytes(block_id) {
                Ok(Some(block_bytes)) => {
                    if contains_subsequence(&block_bytes, &tx_bytes) {
                        match parse_block_prefix(&block_bytes) {
                            Some(context) => {
                                return TransactionOutcome::Committed {
                                    tx_hash: tx_hash.to_string(),
                                    block: context,
                                }
                            }
                            None => {
                                return TransactionOutcome::TransportError {
                                    operation: "getBlock".to_string(),
                                    message: format!(
                                        "block {block_id} payload too short to parse header"
                                    ),
                                }
                            }
                        }
                    }
                }
                Ok(None) => {}
                Err(err) => return err.into_transport_outcome(),
            }
        }

        TransactionOutcome::TransportError {
            operation: "locateCommittedBlock".to_string(),
            message: format!(
                "transaction {tx_hash} is reported committed but no block in 0..={head} \
                 contains its bytes"
            ),
        }
    }
}

/// Borsh block layout prefix: `header.block_id: u64` at 0..8, then two
/// 32-byte hashes, then `header.timestamp: u64` at 72..80.
pub(crate) fn parse_block_prefix(block_bytes: &[u8]) -> Option<BlockContext> {
    let block_id = u64::from_le_bytes(block_bytes.get(0..8)?.try_into().ok()?);
    let timestamp = u64::from_le_bytes(block_bytes.get(72..80)?.try_into().ok()?);
    Some(BlockContext {
        block_id,
        timestamp,
    })
}

pub(crate) fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn extract_jsonrpc_error(operation: &str, body: &Value) -> Option<RpcError> {
    let error = body.get("error")?;
    Some(RpcError::JsonRpc {
        operation: operation.to_string(),
        code: error.get("code").and_then(Value::as_i64),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| one_line(&error.to_string())),
    })
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use super::*;

    /// Tiny scripted JSON-RPC server: dispatches on `method` via the
    /// provided handler until dropped.
    struct FakeNode {
        url: String,
        shutdown: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl FakeNode {
        fn start<F>(handler: F) -> Self
        where
            F: Fn(&str, &Value) -> Value + Send + Sync + 'static,
        {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.set_nonblocking(true).expect("nonblocking");
            let addr = listener.local_addr().expect("addr");
            let shutdown = Arc::new(AtomicBool::new(false));
            let shutdown_flag = shutdown.clone();

            let handle = std::thread::spawn(move || {
                while !shutdown_flag.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_nonblocking(false).expect("blocking stream");
                            let Some(request) = read_http_request(&mut stream) else {
                                continue;
                            };
                            let parsed: Value =
                                serde_json::from_str(&request).unwrap_or(Value::Null);
                            let method = parsed
                                .get("method")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let params = parsed.get("params").cloned().unwrap_or(Value::Null);
                            let result = handler(&method, &params);
                            // Handler returns either a full {"error": ...}
                            // envelope marker or a plain result value.
                            let body = if result.get("__jsonrpc_error").is_some() {
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": 1,
                                    "error": result["__jsonrpc_error"],
                                })
                                .to_string()
                            } else {
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": 1,
                                    "result": result,
                                })
                                .to_string()
                            };
                            let response = format!(
                                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = stream.write_all(response.as_bytes());
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self {
                url: format!("http://{addr}"),
                shutdown,
                handle: Some(handle),
            }
        }
    }

    impl Drop for FakeNode {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn read_http_request(stream: &mut TcpStream) -> Option<String> {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut data = Vec::new();
        let mut buf = [0_u8; 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    data.extend_from_slice(&buf[..n]);
                    if let Some(header_end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers =
                            String::from_utf8_lossy(&data[..header_end]).to_ascii_lowercase();
                        let content_len = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if data.len() >= header_end + 4 + content_len {
                            return Some(
                                String::from_utf8_lossy(&data[header_end + 4..]).into_owned(),
                            );
                        }
                    }
                }
                Err(_) => break,
            }
        }
        None
    }

    /// Build fake block bytes with the documented prefix layout and the
    /// given embedded transaction bytes.
    fn fake_block_bytes(block_id: u64, timestamp: u64, embedded_tx: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&block_id.to_le_bytes());
        bytes.extend_from_slice(&[0xAA; 32]); // prev_block_hash
        bytes.extend_from_slice(&[0xBB; 32]); // hash
        bytes.extend_from_slice(&timestamp.to_le_bytes());
        bytes.extend_from_slice(&[0xCC; 64]); // signature-ish filler
        bytes.extend_from_slice(&1_u32.to_le_bytes()); // tx count
        bytes.extend_from_slice(embedded_tx);
        bytes
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    const TX_HASH: &str = "ec8991c2b2c73a451068a6e136c7cb7e2bae9169f534b158b13979502fce570d";

    fn fast_wait() -> WaitOptions {
        WaitOptions {
            after_block: None,
            timeout: Duration::from_secs(5),
            rejection_blocks: 3,
            poll_interval: Duration::from_millis(20),
        }
    }

    #[test]
    fn submit_and_wait_reports_committed_with_block_context() {
        let tx_bytes: Vec<u8> = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let block = fake_block_bytes(7, 1_710_000_000_000, &tx_bytes);
        let tx_echo = b64(&tx_bytes);
        let block_b64 = b64(&block);

        let node = FakeNode::start(move |method, params| match method {
            "getLastBlockId" => json!(7),
            "sendTransaction" => json!(TX_HASH),
            "getTransaction" => json!(tx_echo.clone()),
            "getBlock" => {
                let id = params[0].as_u64().unwrap();
                if id == 7 {
                    json!(block_b64.clone())
                } else {
                    Value::Null
                }
            }
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let outcome = client.submit_and_wait(
            &TransactionBytes::borsh(tx_bytes.clone()),
            &WaitOptions {
                // Boundary below the committed block so the post-boundary
                // scan finds it.
                after_block: Some(6),
                ..fast_wait()
            },
        );

        match outcome {
            TransactionOutcome::Committed { tx_hash, block } => {
                assert_eq!(tx_hash, TX_HASH);
                assert_eq!(block.block_id, 7);
                assert_eq!(block.timestamp, 1_710_000_000_000);
            }
            other => panic!("expected Committed, got {other:?}"),
        }
    }

    #[test]
    fn committed_json_shape_matches_contract() {
        let outcome = TransactionOutcome::Committed {
            tx_hash: TX_HASH.to_string(),
            block: BlockContext {
                block_id: 72,
                timestamp: 1_710_000_000_000,
            },
        };
        let value = serde_json::to_value(&outcome).unwrap();
        assert_eq!(
            value,
            json!({
                "status": "committed",
                "tx_hash": TX_HASH,
                "block_id": 72,
                "timestamp": 1_710_000_000_000_u64,
            })
        );
    }

    #[test]
    fn stateless_rejection_surfaces_reason() {
        let node = FakeNode::start(|method, _| match method {
            "getLastBlockId" => json!(1),
            "sendTransaction" => json!({
                "__jsonrpc_error": { "code": -32602, "message": "InvalidSignature" }
            }),
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let outcome = client.submit_and_wait(&TransactionBytes::borsh(vec![1, 2, 3]), &fast_wait());

        match outcome {
            TransactionOutcome::Rejected {
                phase,
                reason,
                tx_hash,
                ..
            } => {
                assert_eq!(phase, RejectionPhase::Stateless);
                assert_eq!(reason.as_deref(), Some("InvalidSignature"));
                assert!(tx_hash.is_none());
            }
            other => panic!("expected stateless rejection, got {other:?}"),
        }
    }

    #[test]
    fn stateful_rejection_after_observation_window() {
        use std::sync::atomic::AtomicU64;
        let head = Arc::new(AtomicU64::new(10));
        let head_for_node = head.clone();

        let node = FakeNode::start(move |method, _| match method {
            // Head advances one block per query, simulating block production.
            "getLastBlockId" => json!(head_for_node.fetch_add(1, Ordering::Relaxed)),
            "sendTransaction" => json!(TX_HASH),
            "getTransaction" => Value::Null,
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let outcome = client.submit_and_wait(&TransactionBytes::borsh(vec![9, 9, 9]), &fast_wait());

        match outcome {
            TransactionOutcome::Rejected {
                phase,
                observed_after_block_id,
                tx_hash,
                ..
            } => {
                assert_eq!(phase, RejectionPhase::Stateful);
                assert_eq!(tx_hash.as_deref(), Some(TX_HASH));
                assert!(observed_after_block_id.unwrap() >= 13);
            }
            other => panic!("expected stateful rejection, got {other:?}"),
        }
    }

    #[test]
    fn timeout_when_node_cannot_decide() {
        let node = FakeNode::start(|method, _| match method {
            // Head never advances: no rejection window, no commit.
            "getLastBlockId" => json!(5),
            "sendTransaction" => json!(TX_HASH),
            "getTransaction" => Value::Null,
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let outcome = client.submit_and_wait(
            &TransactionBytes::borsh(vec![1]),
            &WaitOptions {
                timeout: Duration::from_millis(300),
                poll_interval: Duration::from_millis(20),
                ..fast_wait()
            },
        );

        match outcome {
            TransactionOutcome::Timeout {
                tx_hash,
                last_observed_block_id,
            } => {
                assert_eq!(tx_hash, TX_HASH);
                assert_eq!(last_observed_block_id, 5);
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn transport_error_when_node_unreachable() {
        let client = TestNodeClient::new("http://127.0.0.1:1");
        let outcome = client.submit_and_wait(&TransactionBytes::borsh(vec![1]), &fast_wait());
        match outcome {
            TransactionOutcome::TransportError { operation, .. } => {
                assert_eq!(operation, "getLastBlockId");
            }
            other => panic!("expected TransportError, got {other:?}"),
        }
    }

    #[test]
    fn wire_mismatch_when_echo_differs() {
        let node = FakeNode::start(|method, _| match method {
            "getLastBlockId" => json!(3),
            "sendTransaction" => json!(TX_HASH),
            // Echo different bytes than submitted.
            "getTransaction" => json!(b64(&[42, 42, 42])),
            other => panic!("unexpected method {other}"),
        });

        let client = TestNodeClient::new(&node.url);
        let submitted = TransactionBytes::borsh(vec![1, 2, 3]);
        let outcome = client.submit_and_wait(&submitted, &fast_wait());

        match outcome {
            TransactionOutcome::WireMismatch {
                returned_hash,
                submitted_tx,
                echoed_tx,
                ..
            } => {
                assert_eq!(returned_hash, TX_HASH);
                assert_eq!(submitted_tx, submitted.to_base64());
                assert_eq!(echoed_tx, b64(&[42, 42, 42]));
            }
            other => panic!("expected WireMismatch, got {other:?}"),
        }
    }

    #[test]
    fn parse_block_prefix_reads_id_and_timestamp() {
        let bytes = fake_block_bytes(42, 1_700_000_000_123, &[1, 2, 3]);
        let context = parse_block_prefix(&bytes).unwrap();
        assert_eq!(context.block_id, 42);
        assert_eq!(context.timestamp, 1_700_000_000_123);

        assert!(parse_block_prefix(&[0_u8; 10]).is_none());
    }

    #[test]
    fn contains_subsequence_basics() {
        assert!(contains_subsequence(&[1, 2, 3, 4], &[2, 3]));
        assert!(!contains_subsequence(&[1, 2, 3, 4], &[3, 2]));
        assert!(!contains_subsequence(&[1, 2], &[1, 2, 3]));
        assert!(!contains_subsequence(&[1, 2], &[]));
    }

    #[test]
    fn transaction_bytes_round_trip_base64() {
        let tx = TransactionBytes::borsh_base64("AQIDBA==").unwrap();
        assert_eq!(tx.as_bytes(), &[1, 2, 3, 4]);
        assert_eq!(tx.to_base64(), "AQIDBA==");

        assert!(TransactionBytes::borsh_base64("not base64!!").is_err());
    }

    #[test]
    fn rejected_json_shapes_match_contract() {
        let stateless = TransactionOutcome::Rejected {
            tx_hash: None,
            phase: RejectionPhase::Stateless,
            reason: Some("InvalidSignature".to_string()),
            observed_after_block_id: None,
        };
        assert_eq!(
            serde_json::to_value(&stateless).unwrap(),
            json!({ "status": "rejected", "phase": "stateless", "reason": "InvalidSignature" })
        );

        let stateful = TransactionOutcome::Rejected {
            tx_hash: Some(TX_HASH.to_string()),
            phase: RejectionPhase::Stateful,
            reason: None,
            observed_after_block_id: Some(74),
        };
        assert_eq!(
            serde_json::to_value(&stateful).unwrap(),
            json!({
                "status": "rejected",
                "tx_hash": TX_HASH,
                "phase": "stateful",
                "observed_after_block_id": 74,
            })
        );
    }
}
