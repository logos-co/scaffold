use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context};
use serde_json::Value;

use crate::constants::{DEFAULT_WALLET_PASSWORD, WALLET_BIN_REL_PATH};
use crate::model::Project;
use crate::project::resolve_repo_path;
use crate::state::write_text;
use crate::DynResult;

pub(crate) const WALLET_CONFIG_PRIMARY: &str = "wallet_config.json";
pub(crate) const WALLET_CONFIG_FALLBACK: &str = "config.json";

pub(crate) struct WalletRuntimeContext {
    pub(crate) wallet_home: PathBuf,
    pub(crate) wallet_binary: PathBuf,
    pub(crate) sequencer_addr: Option<String>,
}

/// When `wallet_config.json` omits `sequencer_addr`, RPC calls should target the same host/port
/// as `logos-scaffold localnet` (`[localnet] port` in `scaffold.toml`, default 3040).
pub(crate) fn default_sequencer_http_url_for_project(project: &Project) -> String {
    format!("http://127.0.0.1:{}", project.config.localnet.port)
}

pub(crate) fn load_wallet_runtime(project: &Project) -> DynResult<WalletRuntimeContext> {
    let lez = resolve_repo_path(project, &project.config.lez, "lez")?;
    let wallet_binary = lez.join(WALLET_BIN_REL_PATH);
    if !wallet_binary.exists() {
        bail!(
            "missing wallet binary at {}. Run `logos-scaffold setup`.",
            wallet_binary.display()
        );
    }

    let wallet_home = project.root.join(&project.config.wallet_home_dir);
    if !wallet_home.exists() {
        bail!(
            "missing wallet home at {}. Run `logos-scaffold setup` first.",
            wallet_home.display()
        );
    }

    let (_, wallet_config) = read_wallet_config(&wallet_home)?;
    let sequencer_addr = wallet_config
        .get("sequencer_addr")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(WalletRuntimeContext {
        wallet_home,
        wallet_binary,
        sequencer_addr,
    })
}

fn read_wallet_config(wallet_home: &Path) -> DynResult<(PathBuf, Value)> {
    let primary = wallet_home.join(WALLET_CONFIG_PRIMARY);
    let fallback = wallet_home.join(WALLET_CONFIG_FALLBACK);

    let path = if primary.exists() {
        primary
    } else if fallback.exists() {
        // Legacy: older `setup` runs wrote "config.json" instead of
        // "wallet_config.json". Re-run `logos-scaffold setup` to migrate.
        eprintln!(
            "warning: found legacy wallet config '{}';              re-run `logos-scaffold setup` to migrate to '{}'.",
            WALLET_CONFIG_FALLBACK,
            WALLET_CONFIG_PRIMARY,
        );
        fallback
    } else {
        return Err(anyhow::anyhow!(
            "missing wallet config at \'{}\'. Run `logos-scaffold setup`.",
            wallet_home.join(WALLET_CONFIG_PRIMARY).display()
        ));
    };

    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read wallet config at {}", path.display()))?;
    let value = serde_json::from_str::<Value>(&text)
        .with_context(|| format!("failed to parse wallet config JSON at {}", path.display()))?;

    Ok((path, value))
}

pub(crate) fn first_public_wallet_address(wallet_home: &Path) -> DynResult<Option<String>> {
    let (_, wallet_config) = read_wallet_config(wallet_home)?;
    let Some(accounts) = wallet_config
        .get("initial_accounts")
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };

    for account in accounts {
        let Some(account_id) = account
            .get("Public")
            .and_then(|public| public.get("account_id"))
            .and_then(Value::as_str)
        else {
            continue;
        };

        let candidate = format!("Public/{account_id}");
        if let Ok(normalized) = normalize_address_ref(&candidate) {
            return Ok(Some(normalized));
        }
    }

    Ok(None)
}

pub(crate) fn wallet_state_path(project_root: &Path) -> PathBuf {
    project_root.join(".scaffold/state/wallet.state")
}

pub(crate) fn write_default_wallet_address(
    project_root: &Path,
    address: &str,
) -> DynResult<String> {
    let normalized_address = normalize_address_ref(address)?;
    write_text(
        &wallet_state_path(project_root),
        &format!("default_address={normalized_address}\n"),
    )?;
    Ok(normalized_address)
}

pub(crate) fn wallet_password() -> String {
    match env::var("LOGOS_SCAFFOLD_WALLET_PASSWORD") {
        Ok(password) if !password.trim().is_empty() => password,
        _ => DEFAULT_WALLET_PASSWORD.to_string(),
    }
}

pub(crate) fn read_default_wallet_address(project_root: &Path) -> DynResult<Option<String>> {
    let state_path = wallet_state_path(project_root);
    if !state_path.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(&state_path)
        .with_context(|| format!("failed to read {}", state_path.display()))?;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("default_address=") {
            let value = rest.trim();
            if value.is_empty() {
                bail!(
                    "default wallet at {} is empty. Run `logos-scaffold wallet default set <address>`.",
                    state_path.display()
                );
            }
            return Ok(Some(value.to_string()));
        }
    }

    if text.trim().is_empty() {
        return Ok(None);
    }

    bail!(
        "wallet state at {} is malformed. Expected `default_address=<address>`. Run `logos-scaffold wallet default set <address>`.",
        state_path.display()
    )
}

pub(crate) fn resolve_wallet_address(
    explicit: Option<&str>,
    default_from_state: Option<&str>,
) -> DynResult<String> {
    if let Some(explicit) = explicit {
        return normalize_address_ref(explicit);
    }

    if let Some(default_address) = default_from_state {
        return normalize_address_ref(default_address);
    }

    bail!(
        "wallet topup requires a destination address.\nNext step: run `logos-scaffold wallet list` to inspect available wallets, then run `logos-scaffold wallet default set <address>` or pass `--address <address>`."
    )
}

pub(crate) fn normalize_address_ref(raw: &str) -> DynResult<String> {
    let input = raw.trim();
    if input.is_empty() {
        bail!(invalid_address_message(raw));
    }

    let (prefix, account_id) = if let Some(rest) = input.strip_prefix("Public/") {
        ("Public", rest)
    } else if let Some(rest) = input.strip_prefix("Private/") {
        ("Private", rest)
    } else {
        ("Public", input)
    };

    validate_base58_account_id(account_id)
        .map_err(|_| anyhow::anyhow!(invalid_address_message(raw)))?;

    Ok(format!("{prefix}/{account_id}"))
}

fn validate_base58_account_id(account_id: &str) -> DynResult<()> {
    let decoded = bs58::decode(account_id)
        .into_vec()
        .map_err(|_| anyhow::anyhow!("invalid base58 account id"))?;

    if decoded.len() != 32 {
        bail!("account id must decode to exactly 32 bytes");
    }

    Ok(())
}

fn invalid_address_message(raw: &str) -> String {
    format!(
        "invalid address format `{raw}`\nAccepted formats:\n- Public/<base58-account-id>\n- Private/<base58-account-id>\n- <base58-account-id> (treated as Public/<...>)\nExamples:\n- Public/6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV\n- Private/2ECgkFTaXzwjJBXR7ZKmXYQtpHbvTTHK9Auma4NL9AUo\n- 6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV"
    )
}

pub(crate) fn is_connectivity_failure(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "connection refused",
        "connecterror",
        "failed to connect",
        "tcp connect error",
        "network is unreachable",
        "error sending request",
        "http error",
        "127.0.0.1:3040",
        "localhost:3040",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub(crate) fn is_uninitialized_account_output(text: &str) -> bool {
    text.to_lowercase().contains("account is uninitialized")
}

pub(crate) fn is_already_initialized_failure(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "already initialized",
        "account must be uninitialized",
        "account is already initialized",
        "cannot claim an initialized account",
        "only uninitialized accounts can be initialized",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub(crate) fn is_confirmation_timeout_failure(text: &str) -> bool {
    text.to_lowercase()
        .contains("transaction not found in preconfigured amount of blocks")
}

pub(crate) fn summarize_command_failure(stdout: &str, stderr: &str) -> String {
    let stderr_line = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.to_string());
    if let Some(line) = stderr_line {
        return line;
    }

    let stdout_line = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.to_string());
    if let Some(line) = stdout_line {
        return line;
    }

    "command failed without stderr output".to_string()
}

pub(crate) fn extract_tx_identifier(stdout: &str, stderr: &str) -> Option<String> {
    let combined = format!("{stdout}\n{stderr}");

    if let Some(hex_hash) = extract_tx_hash_from_hash_type_bytes(&combined) {
        return Some(hex_hash);
    }

    for raw_line in combined.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.split("tx_hash=").nth(1) {
            return Some(rest.trim().to_string());
        }
        if line.contains("tx_hash:") {
            return Some(line.to_string());
        }
        if line.contains("\"tx_hash\"") {
            return Some(line.to_string());
        }
    }

    None
}

fn extract_tx_hash_from_hash_type_bytes(text: &str) -> Option<String> {
    let mut offset = 0;
    while let Some(found) = text[offset..].find("tx_hash:") {
        let field_start = offset + found + "tx_hash:".len();
        let after_field = &text[field_start..];
        let after_whitespace = after_field.trim_start();

        // Only parse HashType byte-array output. `tx_hash=<id>` is handled by fallback parsing.
        if !after_whitespace.starts_with("HashType(") {
            offset = field_start;
            continue;
        }

        let after_hash_type = &after_whitespace["HashType(".len()..];
        let Some(open_bracket) = after_hash_type.find('[') else {
            offset = field_start;
            continue;
        };

        if !after_hash_type[..open_bracket].trim().is_empty() {
            offset = field_start;
            continue;
        }

        let after_open = &after_hash_type[open_bracket + 1..];
        let Some(close_bracket) = after_open.find(']') else {
            offset = field_start;
            continue;
        };
        let inside = &after_open[..close_bracket];

        let mut bytes = Vec::new();
        let mut parse_failed = false;
        for chunk in inside.split(|ch: char| ch == ',' || ch.is_whitespace()) {
            if chunk.is_empty() {
                continue;
            }
            match chunk.parse::<u8>() {
                Ok(value) => bytes.push(value),
                Err(_) => {
                    parse_failed = true;
                    break;
                }
            }
        }

        if parse_failed || bytes.is_empty() {
            offset = field_start;
            continue;
        }

        let mut hex_hash = String::from("0x");
        for byte in bytes {
            use std::fmt::Write as _;
            let _ = write!(&mut hex_hash, "{byte:02x}");
        }
        return Some(hex_hash);
    }

    None
}

#[derive(Debug, Clone)]
pub(crate) enum RpcReachabilityError {
    Connectivity(String),
    Other(String),
}

impl std::fmt::Display for RpcReachabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcReachabilityError::Connectivity(msg) => write!(f, "{msg}"),
            RpcReachabilityError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for RpcReachabilityError {}

pub(crate) fn rpc_get_last_block_id(sequencer_addr: &str) -> Result<u64, RpcReachabilityError> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1_u64,
        "method": "getLastBlockId",
        "params": {}
    });

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(1))
        .timeout_read(Duration::from_secs(2))
        .timeout_write(Duration::from_secs(2))
        .build();

    let response = agent
        .post(sequencer_addr)
        .set("content-type", "application/json")
        .send_json(payload)
        .map_err(map_ureq_error)?;

    let body: Value = response.into_json().map_err(|err| {
        RpcReachabilityError::Other(format!(
            "failed to decode getLastBlockId response from {sequencer_addr}: {err}"
        ))
    })?;

    if let Some(err_obj) = body.get("error") {
        let code = err_obj.get("code").and_then(Value::as_i64);
        let message = err_obj.get("message").and_then(Value::as_str).unwrap_or("");
        let formatted = match code {
            Some(c) => format!("getLastBlockId RPC error {c}: {message}"),
            None => format!(
                "getLastBlockId RPC error: {}",
                one_line(&err_obj.to_string())
            ),
        };
        return Err(RpcReachabilityError::Other(formatted));
    }

    body.get("result").and_then(Value::as_u64).ok_or_else(|| {
        RpcReachabilityError::Other(format!(
            "getLastBlockId response missing numeric `result`: {}",
            one_line(&body.to_string())
        ))
    })
}

fn map_ureq_error(err: ureq::Error) -> RpcReachabilityError {
    match err {
        ureq::Error::Transport(transport) => {
            let msg = transport.to_string();
            if is_connectivity_failure(&msg) {
                RpcReachabilityError::Connectivity(msg)
            } else {
                RpcReachabilityError::Other(msg)
            }
        }
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            RpcReachabilityError::Other(format!("HTTP {code}: {}", one_line(&body)))
        }
    }
}

pub(crate) fn sequencer_unreachable_hint(sequencer_addr: &str) -> String {
    format!(
        "sequencer appears unavailable at {sequencer_addr}\nRun `logos-scaffold localnet start`.\nAnother project's sequencer may already be running and may not match this project."
    )
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::WALLET_CONFIG_PRIMARY;
    use std::fs;

    use tempfile::tempdir;

    use super::{
        extract_tx_identifier, first_public_wallet_address, is_already_initialized_failure,
        is_uninitialized_account_output, normalize_address_ref, read_default_wallet_address,
        resolve_wallet_address, wallet_state_path, write_default_wallet_address,
    };

    const ACCOUNT_ID: &str = "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV";

    #[test]
    fn normalize_accepts_raw_account_id() {
        let normalized = normalize_address_ref(ACCOUNT_ID).expect("normalize");
        assert_eq!(normalized, format!("Public/{ACCOUNT_ID}"));
    }

    #[test]
    fn normalize_accepts_private_prefix() {
        let normalized =
            normalize_address_ref(&format!("Private/{ACCOUNT_ID}")).expect("normalize");
        assert_eq!(normalized, format!("Private/{ACCOUNT_ID}"));
    }

    #[test]
    fn normalize_rejects_invalid_address() {
        let err = normalize_address_ref("abc").expect_err("must reject invalid address");
        assert!(err.to_string().contains("invalid address format"));
    }

    #[test]
    fn read_default_wallet_address_returns_none_for_missing_state() {
        let temp = tempdir().expect("tempdir");
        let value = read_default_wallet_address(temp.path()).expect("read default");
        assert!(value.is_none());
    }

    #[test]
    fn read_default_wallet_address_parses_state_file() {
        let temp = tempdir().expect("tempdir");
        let state_path = temp.path().join(".scaffold/state/wallet.state");
        fs::create_dir_all(state_path.parent().expect("parent")).expect("mkdir");
        fs::write(
            &state_path,
            format!("default_address=Public/{ACCOUNT_ID}\n"),
        )
        .expect("write");

        let value = read_default_wallet_address(temp.path()).expect("read default");
        let expected = format!("Public/{ACCOUNT_ID}");
        assert_eq!(value.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn resolve_wallet_address_prefers_explicit_input() {
        let value = resolve_wallet_address(
            Some(ACCOUNT_ID),
            Some("Private/2ECgkFTaXzwjJBXR7ZKmXYQtpHbvTTHK9Auma4NL9AUo"),
        )
        .expect("resolve");
        assert_eq!(value, format!("Public/{ACCOUNT_ID}"));
    }

    #[test]
    fn resolve_wallet_address_uses_default_when_explicit_missing() {
        let value =
            resolve_wallet_address(None, Some(&format!("Public/{ACCOUNT_ID}"))).expect("resolve");
        assert_eq!(value, format!("Public/{ACCOUNT_ID}"));
    }

    #[test]
    fn resolve_wallet_address_errors_when_both_missing() {
        let err = resolve_wallet_address(None, None).expect_err("must fail");
        assert!(err
            .to_string()
            .contains("wallet topup requires a destination address"));
    }

    #[test]
    fn first_public_wallet_address_parses_wallet_config() {
        let temp = tempdir().expect("tempdir");
        let wallet_home = temp.path().join(".scaffold/wallet");
        fs::create_dir_all(&wallet_home).expect("mkdir wallet home");
        fs::write(
            wallet_home.join(WALLET_CONFIG_PRIMARY),
            r#"{
  "initial_accounts": [
    { "Private": { "account_id": "2ECgkFTaXzwjJBXR7ZKmXYQtpHbvTTHK9Auma4NL9AUo" } },
    { "Public": { "account_id": "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV" } }
  ]
}"#,
        )
        .expect("write wallet config");

        let value = first_public_wallet_address(&wallet_home).expect("first public");
        let expected = format!("Public/{ACCOUNT_ID}");
        assert_eq!(value.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn first_public_wallet_address_returns_none_without_public_accounts() {
        let temp = tempdir().expect("tempdir");
        let wallet_home = temp.path().join(".scaffold/wallet");
        fs::create_dir_all(&wallet_home).expect("mkdir wallet home");
        fs::write(
            wallet_home.join(WALLET_CONFIG_PRIMARY),
            r#"{
  "initial_accounts": [
    { "Private": { "account_id": "2ECgkFTaXzwjJBXR7ZKmXYQtpHbvTTHK9Auma4NL9AUo" } }
  ]
}"#,
        )
        .expect("write wallet config");

        let value = first_public_wallet_address(&wallet_home).expect("first public");
        assert!(value.is_none());
    }

    #[test]
    fn write_default_wallet_address_persists_normalized_address() {
        let temp = tempdir().expect("tempdir");
        let normalized = write_default_wallet_address(temp.path(), ACCOUNT_ID).expect("write");
        assert_eq!(normalized, format!("Public/{ACCOUNT_ID}"));

        let state = fs::read_to_string(wallet_state_path(temp.path())).expect("read wallet.state");
        assert_eq!(state, format!("default_address=Public/{ACCOUNT_ID}\n"));
    }

    #[test]
    fn extract_tx_identifier_finds_tx_hash_key() {
        let tx = extract_tx_identifier("ok tx_hash=abc123", "");
        assert_eq!(tx.as_deref(), Some("abc123"));
    }

    #[test]
    fn extract_tx_identifier_parses_multiline_hash_type_bytes() {
        let stdout = r#"Results of tx send are SendTxResponse {
    status: "Transaction submitted",
    tx_hash: HashType(
        [
            236,
            137,
            145,
            194,
            178,
            199,
            58,
            69,
            16,
            104,
            166,
            225,
            54,
            199,
            203,
            126,
            43,
            174,
            145,
            105,
            245,
            52,
            177,
            88,
            177,
            57,
            121,
            80,
            47,
            206,
            87,
            13,
        ],
    ),
}"#;

        let tx = extract_tx_identifier(stdout, "");
        assert_eq!(
            tx.as_deref(),
            Some("0xec8991c2b2c73a451068a6e136c7cb7e2bae9169f534b158b13979502fce570d")
        );
    }

    #[test]
    fn extract_tx_identifier_prefers_plain_tx_hash_over_unrelated_byte_array() {
        let stdout = r#"
status: ok
tx_hash=plain-id-123
debug payload: [1, 2, 3]
"#;

        let tx = extract_tx_identifier(stdout, "");
        assert_eq!(tx.as_deref(), Some("plain-id-123"));
    }

    #[test]
    fn extract_tx_identifier_does_not_parse_bytes_without_hash_type_marker() {
        let stdout = r#"
status: pending
tx_hash: plain-id-789
details: [1, 2, 3]
"#;

        let tx = extract_tx_identifier(stdout, "");
        assert_eq!(tx.as_deref(), Some("tx_hash: plain-id-789"));
    }

    #[test]
    fn rpc_get_last_block_id_parses_valid_response() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://{addr}");

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf);

            let body = r#"{"jsonrpc":"2.0","result":42,"id":1}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write");
            stream.flush().expect("flush");
        });

        let block =
            super::rpc_get_last_block_id(&url).expect("rpc_get_last_block_id should succeed");
        assert_eq!(block, 42);
        handle.join().expect("server thread");
    }

    #[test]
    fn rpc_get_last_block_id_returns_connectivity_error_when_unreachable() {
        let result = super::rpc_get_last_block_id("http://127.0.0.1:1");
        assert!(result.is_err());
        match result.unwrap_err() {
            super::RpcReachabilityError::Connectivity(_) => {}
            other => panic!("expected Connectivity error, got: {other}"),
        }
    }

    #[test]
    fn rpc_get_last_block_id_returns_error_on_malformed_response() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://{addr}");

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf);

            // Response with non-numeric `result`
            let body = r#"{"jsonrpc":"2.0","result":{},"id":1}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write");
            stream.flush().expect("flush");
        });

        let result = super::rpc_get_last_block_id(&url);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("missing numeric `result`"),
            "expected missing numeric result error, got: {err_msg}"
        );
        handle.join().expect("server thread");
    }

    #[test]
    fn rpc_get_last_block_id_returns_error_on_method_not_found() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://{addr}");

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf);

            let body =
                r#"{"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found"},"id":1}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write");
            stream.flush().expect("flush");
        });

        let result = super::rpc_get_last_block_id(&url);
        let err_msg = result
            .expect_err("method-not-found should surface as error")
            .to_string();
        assert!(
            err_msg.contains("-32601") && err_msg.contains("Method not found"),
            "expected JSON-RPC error code and message to surface, got: {err_msg}"
        );
        assert!(
            !err_msg.contains("missing numeric"),
            "JSON-RPC error should surface structurally, not fall through to the \
             generic missing-result branch; got: {err_msg}"
        );
        handle.join().expect("server thread");
    }

    #[test]
    fn detects_uninitialized_account_output() {
        let combined = "some output\nAccount is Uninitialized\nmore output";
        assert!(is_uninitialized_account_output(combined));
    }

    #[test]
    fn detects_already_initialized_failure_output() {
        let combined = "Error: Account must be uninitialized";
        assert!(is_already_initialized_failure(combined));
    }
}
