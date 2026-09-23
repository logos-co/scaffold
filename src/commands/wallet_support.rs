use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context};
use serde_json::Value;

use crate::constants::{DEFAULT_WALLET_PASSWORD, WALLET_BIN_REL_PATH, WALLET_HOME_ENV_VARS};
use crate::model::Project;
use crate::project::resolve_repo_path;
use crate::state::write_text;
use crate::DynResult;

pub(crate) const WALLET_CONFIG_PRIMARY: &str = "wallet_config.json";
pub(crate) const WALLET_CONFIG_FALLBACK: &str = "config.json";

/// Point a wallet subprocess at `wallet_home` under every env var name the
/// vendored wallet binary may read (see [`WALLET_HOME_ENV_VARS`]). The path is
/// passed as an `OsStr` (not via `to_string_lossy`) so a non-UTF8 wallet-home
/// path reaches the child verbatim instead of being corrupted.
pub(crate) fn set_wallet_home_env(command: &mut Command, wallet_home: impl AsRef<OsStr>) {
    for name in WALLET_HOME_ENV_VARS {
        command.env(name, wallet_home.as_ref());
    }
}

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

/// Extract the first `Public/<base58-account-id>` account from wallet
/// `account list` output.
///
/// In the output shapes captured in this repo (unit tests here and the fake
/// wallet in `tests/cli.rs`) listing entries render as `/ Public/<account-id>`,
/// so `/ `-prefixed lines are preferred: the address this returns becomes the
/// project's default topup destination, and base58 validation alone cannot
/// tell a listing entry from a real address printed on some other line.
///
/// Those other lines are not hypothetical. A real `wallet account list` run
/// prints the wallet's own built-in `Preconfigured Public/<id>` /
/// `Preconfigured Private/<id>` entries *above* the stored accounts — and it
/// prints them even when the pinned debug config ships no `initial_accounts`,
/// which is exactly the case that reaches this function. Without the `/ `
/// tier the scan would adopt the first of those instead of the account the
/// wallet just created in its own persistent storage.
///
/// When the output contains no usable `/ `-prefixed entry, the scan falls back
/// to any whitespace-separated `Public/<account-id>` token anywhere in the
/// output. The marker is not a documented contract of the wallet CLI, and
/// returning `None` here leaves `.scaffold/state/wallet.state` without a
/// `default_address=` line — the failure scaffold#240 was about — so an
/// unmarked candidate is better than none.
///
/// Either way the account id is validated through [`normalize_address_ref`] —
/// it must decode to exactly 32 bytes.
pub(crate) fn first_public_address_in_listing(output: &str) -> Option<String> {
    let marked = output.lines().find_map(|line| {
        let entry = line.trim().strip_prefix("/ ")?;
        public_address_at_start(entry.trim_start())
    });
    if marked.is_some() {
        return marked;
    }

    output.split_whitespace().find_map(public_address_at_start)
}

/// Parse a `Public/<base58-account-id>` reference at the start of `text`,
/// trimming anything after the base58 run (e.g. trailing punctuation).
fn public_address_at_start(text: &str) -> Option<String> {
    let rest = text.strip_prefix("Public/")?;
    let account_id: String = rest
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    normalize_address_ref(&format!("Public/{account_id}")).ok()
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
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Like [`is_connectivity_failure`], but also treats a wallet-reported
/// timeout as connectivity, corroborated via [`rpc_get_last_block_id`]
/// against `sequencer_addr` rather than enumerating wallet wording. See
/// [`sequencer_connectivity_failure_with_probe`] for the two-tier match and
/// [`probe_sequencer_reachable`] for why the corroboration is RPC-level, not
/// a raw socket check.
///
/// Diverges intentionally from `doctor`'s `is_localnet_connectivity_failure`,
/// which is text-only and requires an address plus a transport token: that
/// check runs during `doctor`, where there is no live wallet call to
/// corroborate against, so text is the only signal available. This function
/// runs right after a real wallet command just failed, so a live RPC probe
/// is available and is the stronger signal.
pub(crate) fn sequencer_connectivity_failure(text: &str, sequencer_addr: &str) -> bool {
    sequencer_connectivity_failure_with_probe(text, sequencer_addr, probe_sequencer_reachable)
}

/// Same as [`sequencer_connectivity_failure`], but takes the reachability
/// probe as a parameter so tests can supply a fake instead of hitting a real
/// endpoint.
///
/// This does *not* exclude a confirmation timeout. A caller that also submits
/// transactions — so its subprocess can emit "transaction not found in
/// preconfigured amount of blocks" — must check
/// [`is_confirmation_timeout_failure`] *before* this, so a tx that reached the
/// sequencer but did not settle is reported as pending (with its id) rather
/// than as connectivity. Folding that exclusion in here only honoured the one
/// caller that has a confirmation-timeout branch and silently dropped the case
/// for the callers that submit transactions without one (see the ordering at
/// `wallet.rs`'s pinata-claim site).
///
/// - Fast path: the message names our sequencer (port-matched under the
///   `127.0.0.1`/`localhost` aliases, boundary-anchored) and carries an
///   unambiguous connect-specific phrase — no probe needed. This is deliberate,
///   not just an optimization: text that explicitly names the connection and
///   our address is stronger evidence than a generic timeout is, so it is
///   trusted without a live RPC round-trip. It is gated on the configured host
///   itself being a loopback alias, because the aliases it recognizes are
///   hardcoded loopback: for a *remote* sequencer a message naming
///   `localhost:<port>` is a different machine, so it falls through to the
///   probe (the strictly better signal) instead.
/// - Fallback: any other message that merely says "timeout" is corroborated
///   against `sequencer_addr` via `probe`, since real wallets don't reliably
///   name the connection or the address.
pub(crate) fn sequencer_connectivity_failure_with_probe(
    text: &str,
    sequencer_addr: &str,
    probe: impl Fn(&str) -> bool,
) -> bool {
    if is_connectivity_failure(text) {
        return true;
    }
    let lower = text.to_lowercase();

    if sequencer_host_is_loopback(sequencer_addr) {
        if let Some(port) = sequencer_port(sequencer_addr) {
            let mentions_sequencer = [format!("127.0.0.1:{port}"), format!("localhost:{port}")]
                .iter()
                .any(|needle| contains_on_port_boundary(&lower, needle));
            if mentions_sequencer {
                const CONNECT_TIMEOUT_PHRASES: &[&str] = &[
                    "connection timed out",
                    "connect timed out",
                    "connection timeout",
                    "connect timeout",
                ];
                if CONNECT_TIMEOUT_PHRASES
                    .iter()
                    .any(|needle| lower.contains(needle))
                {
                    return true;
                }
            }
        }
    }

    let mentions_timeout = ["timeout", "timed out"]
        .iter()
        .any(|needle| lower.contains(needle));
    if !mentions_timeout {
        return false;
    }
    !probe(sequencer_addr)
}

/// Corroborates a wallet-reported timeout using [`rpc_get_last_block_id`] /
/// [`RpcReachabilityError`], the same primitives `deploy`'s
/// `preflight_sequencer_reachability` already trusts for this exact
/// question, so there is no bespoke socket-handling code here.
///
/// `Connectivity` errors (refused, timed out, DNS failure — see
/// [`map_ureq_error`]) are treated as unreachable. Anything else, including
/// a non-connectivity transport error, a bad HTTP status, or a malformed
/// response, is treated as reachable: this asks "did the connection reach
/// *something*", not "did it reach our sequencer specifically" — a foreign
/// process squatting on the configured port still reads as reachable here,
/// same as it does for `preflight_sequencer_reachability`.
pub(crate) fn probe_sequencer_reachable(sequencer_addr: &str) -> bool {
    match rpc_get_last_block_id(sequencer_addr) {
        Ok(_) => true,
        Err(RpcReachabilityError::Connectivity(_)) => false,
        Err(RpcReachabilityError::Other(_)) => true,
    }
}

/// Strip the scheme and any path/query/fragment from a resolved sequencer URL,
/// leaving the `[userinfo@]host:port` authority. Wallet output naming only the
/// authority never carries the path, so keeping it would poison downstream
/// needle matches (`http://127.0.0.1:3040/rpc` → `127.0.0.1:3040`).
fn sequencer_authority(sequencer_addr: &str) -> &str {
    let after_scheme = sequencer_addr
        .split_once("://")
        .map_or(sequencer_addr, |(_, rest)| rest);
    after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme)
}

/// Extract the port from a resolved sequencer URL so `http://127.0.0.1:3040/rpc`
/// yields `3040`.
fn sequencer_port(sequencer_addr: &str) -> Option<String> {
    let (_, port) = sequencer_authority(sequencer_addr).rsplit_once(':')?;
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(port.to_string())
}

/// Whether the resolved sequencer URL's host is a loopback alias (`127.0.0.1`,
/// `localhost`, or the IPv6 `::1`). The zero-probe fast path in
/// [`sequencer_connectivity_failure_with_probe`] recognizes the sequencer only
/// under the hardcoded `127.0.0.1`/`localhost` aliases, so it is only sound
/// when the configured host is itself one of those — a remote sequencer that
/// merely shares the port must fall through to the probe instead.
fn sequencer_host_is_loopback(sequencer_addr: &str) -> bool {
    let authority = sequencer_authority(sequencer_addr);
    // Drop any `userinfo@`, then the `:port`, leaving the bare host.
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, rest)| rest);
    let host = host_port
        .rsplit_once(':')
        .map_or(host_port, |(host, _)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// Substring match that rejects a trailing-digit extension, so the needle
/// `127.0.0.1:3040` matches `…:3040` and `…:3040/rpc` but not `…:30401`.
/// Only the right edge is checked: a needle preceded by other characters
/// (`foo127.0.0.1:3040`) still matches. That's deliberate — the left side
/// only ever gets scheme/whitespace prefixes in practice, where a false
/// match is harmless, so guarding the noisier right edge (arbitrary path or
/// port suffixes) is what actually matters here.
fn contains_on_port_boundary(haystack: &str, needle: &str) -> bool {
    let mut start = 0;
    while let Some(offset) = haystack[start..].find(needle) {
        let end = start + offset + needle.len();
        if !haystack[end..].starts_with(|ch: char| ch.is_ascii_digit()) {
            return true;
        }
        start = end;
    }
    false
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
            // ureq reports both "connection refused" and a connect *timeout*
            // (e.g. a blackholed/SYN-dropped address) as `ConnectionFailed`;
            // its prose for the latter ("Connect error: connection timed
            // out") matches none of `is_connectivity_failure`'s needles, and
            // a blackholed sequencer is exactly the case this connectivity
            // hint exists for. Classify on the structured `ErrorKind`
            // instead of scraping prose.
            match transport.kind() {
                ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::Dns => {
                    RpcReachabilityError::Connectivity(msg)
                }
                // `Io` covers a read timeout against a listener that *did*
                // accept the connection, so it stays "something answered"
                // and a slow-but-reachable sequencer is not misreported as
                // unreachable — text-matched here as a fallback, same as
                // before this change.
                _ if is_connectivity_failure(&msg) => RpcReachabilityError::Connectivity(msg),
                _ => RpcReachabilityError::Other(msg),
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
    use std::fs;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    use tempfile::tempdir;

    use super::{
        extract_tx_identifier, first_public_address_in_listing, first_public_wallet_address,
        is_already_initialized_failure, is_uninitialized_account_output, normalize_address_ref,
        read_default_wallet_address, resolve_wallet_address, set_wallet_home_env,
        wallet_state_path, write_default_wallet_address, WALLET_CONFIG_PRIMARY,
    };
    use crate::commands::test_support::drain_http_request;
    use crate::constants::WALLET_HOME_ENV_VARS;

    use super::{sequencer_connectivity_failure, sequencer_connectivity_failure_with_probe};

    /// The original classification test, ported to a fake probe so it never
    /// touches real sockets. `3040`/`3050` are only ever used here as text —
    /// no port on the test-running machine needs to be free (crucially,
    /// `3040` is this project's *default* `[localnet] port`, so a developer
    /// with a localnet running in another terminal must not change these
    /// results).
    #[test]
    fn sequencer_connectivity_failure_classification() {
        let default_addr = "http://127.0.0.1:3040";
        let custom_addr = "http://127.0.0.1:3050";

        let unreachable = |_addr: &str| false;
        let reachable = |_addr: &str| true;

        // False positive guard: a logical rejection that merely echoes the
        // sequencer URL is NOT a connectivity failure.
        let rejection =
            "Error: transaction rejected: invalid signature (sequencer at http://127.0.0.1:3040)";
        assert!(
            !sequencer_connectivity_failure_with_probe(rejection, default_addr, unreachable),
            "logical rejection echoing the URL must not be connectivity"
        );

        // False negative guard: a real connect timeout against a sequencer on a
        // custom [localnet] port is a connectivity failure (the old hardcoded
        // :3040 needle missed this), corroborated by an unreachable probe.
        let timeout =
            "Error: request to http://127.0.0.1:3050 failed: Connection timed out (os error 110)";
        assert!(
            sequencer_connectivity_failure_with_probe(timeout, custom_addr, unreachable),
            "connect timeout naming the configured sequencer, corroborated as \
             unreachable, must be connectivity"
        );

        // Corroboration matters: a message that mentions a *different* port
        // than the one we're configured for (so it skips the fast path) but
        // still carries generic timeout wording falls to the probe. If the
        // probe reports our real sequencer as unreachable, that IS a genuine
        // connectivity failure regardless of which port the message text
        // happened to name — this is precisely the case the pinned wallet's
        // bare "Error: Request timeout" represents (see the dedicated probe
        // tests below for the exact real-world wording).
        let other_port = "Error: connection timed out contacting http://127.0.0.1:30401";
        assert!(
            sequencer_connectivity_failure_with_probe(other_port, default_addr, unreachable),
            "generic timeout wording, corroborated as unreachable against our real \
             sequencer, must be connectivity even if the message names a different port"
        );

        // Same message, but the probe reports our sequencer as reachable:
        // must NOT be connectivity. This is the case the old text-only
        // matcher could not distinguish from the one above, and pins the
        // probe result as load-bearing, not decorative.
        assert!(
            !sequencer_connectivity_failure_with_probe(other_port, default_addr, reachable),
            "generic timeout wording, corroborated as reachable, must not be \
             classified as connectivity"
        );

        // Control: an explicit transport error is connectivity on its own,
        // regardless of the probe (short-circuits before it is consulted).
        let refused =
            "error sending request for url (http://127.0.0.1:3040/): Connection refused (os error 111)";
        assert!(
            sequencer_connectivity_failure_with_probe(refused, default_addr, reachable),
            "transport error must be connectivity even if the probe claims reachable"
        );

        // Regression: a transport token with no URL still classifies (matches
        // the pre-change behaviour, so no true positive is lost).
        assert!(
            sequencer_connectivity_failure_with_probe(
                "tcp connect error: connection refused",
                default_addr,
                reachable
            ),
            "bare transport token must still classify regardless of the probe"
        );

        // Confirmation-timeout ordering is now owned by the callers that submit
        // transactions (see `wallet.rs`'s pinata-claim site), not by this
        // helper. A both-tokens message — the confirmation line plus a transport
        // token, which is what a sequencer dying mid-operation looks like once
        // `combined` is stdout+stderr — therefore classifies as connectivity
        // here; the caller must check `is_confirmation_timeout_failure` first if
        // it wants the pending outcome. (Folding the guard into this helper only
        // honoured the one caller that has a confirmation-timeout branch and
        // silently dropped the case for the others that submit txs without one.)
        assert!(
            sequencer_connectivity_failure_with_probe(
                "transaction not found in preconfigured amount of blocks \
                 (http error contacting http://127.0.0.1:3040)",
                default_addr,
                unreachable
            ),
            "with the helper no longer owning the confirmation-timeout guard, a \
             transport token classifies as connectivity; ordering is the caller's job"
        );

        // Fast-path aliases are hardcoded loopback, so the zero-probe fast path
        // must only fire when the configured host is itself loopback. A remote
        // sequencer whose failure text happens to name `localhost:<port>` is a
        // different machine: the message alone must not classify — it must fall
        // through to the probe. Here the probe reports reachable, so the result
        // is NOT connectivity, which it could not be if the fast path had fired.
        assert!(
            !sequencer_connectivity_failure_with_probe(
                "Error: connection timed out contacting localhost:3040",
                "http://10.0.0.5:3040",
                reachable
            ),
            "a remote sequencer must not take the loopback fast path on a \
             localhost-named message; the probe decides"
        );

        // Alias: config names 127.0.0.1 but the wallet's output names localhost.
        // The port is matched under both aliases, so this is still connectivity.
        assert!(
            sequencer_connectivity_failure_with_probe(
                "Error: connection timed out contacting localhost:3040",
                default_addr,
                unreachable
            ),
            "connect timeout naming the localhost alias of the configured port must be connectivity"
        );

        // Boundary matching still governs the FAST path: a configured port
        // must not match a longer port that merely has it as a digit prefix
        // (`:3040` vs `:30401`), so this does not take the zero-network fast
        // path — it falls to the probe instead (exercised above as
        // `other_port`), which is the fix for exactly this ambiguity.

        // Path in the configured address must not poison the needle: wallet
        // output naming only the authority still classifies.
        assert!(
            sequencer_connectivity_failure_with_probe(
                "Error: connection timed out contacting http://127.0.0.1:3040",
                "http://127.0.0.1:3040/rpc",
                unreachable
            ),
            "a path on the configured address must be stripped before matching"
        );

        // A success line that merely names the sequencer is not a failure
        // (no "timeout" wording, so the probe is never consulted).
        assert!(
            !sequencer_connectivity_failure_with_probe(
                "connected to sequencer http://127.0.0.1:3040",
                default_addr,
                unreachable
            ),
            "success line naming the URL must not be connectivity"
        );
    }

    // Port 1 is a privileged port with nothing listening in this test
    // environment (matches the existing convention in
    // `rpc_get_last_block_id_returns_connectivity_error_when_unreachable`
    // below) — a connection to it is refused immediately, so this is
    // deterministic and fast, no need to actually wait out a timeout. Unlike
    // the classification test above, this one exercises the REAL probe
    // end-to-end via the public `sequencer_connectivity_failure`.
    // Control case: port 1 gives an immediate "connection refused", which was
    // already classified as connectivity before this PR via the generic
    // transport needles in `is_connectivity_failure` — this path never
    // needed the probe. Kept alongside the blackhole test below because they
    // fail differently: refused is a fast, unambiguous rejection, while a
    // blackholed address (see below) exercises the actual connect-*timeout*
    // path the probe exists for.
    #[test]
    fn sequencer_connectivity_failure_corroborates_bare_timeout_with_rpc_probe() {
        let addr = "http://127.0.0.1:1";
        // This is the pinned LEZ wallet's actual, verbatim timeout wording —
        // note it names neither "connection" nor the sequencer's
        // address/port at all, so this deliberately does NOT interpolate
        // `addr` into the message. That is the exact property that defeats
        // any text-only matcher: `mentions_sequencer` would be false for
        // this message regardless of wording, so gating the check behind it
        // would miss this case entirely. The probe corroborates
        // unreachability using the `sequencer_addr` parameter directly,
        // independent of what (if anything) the message text names.
        let real_wallet_wording = "Error: Request timeout";
        assert!(
            sequencer_connectivity_failure(real_wallet_wording, addr),
            "a generic timeout that names no address at all must still be \
             classified as connectivity when the real sequencer is unreachable — \
             gating the probe behind a sequencer-mention check would miss this"
        );
    }

    // The actual case the PR exists for: a blackholed sequencer whose SYNs
    // are dropped rather than refused (firewalled, wrong host, host down).
    // 10.255.255.1 is RFC 1918 and unrouted here, so the connection attempt
    // times out rather than failing immediately — bounded by ureq's
    // configured connect timeout (~1s), not the OS default. Unlike a refused
    // connection, this exercises `map_ureq_error`'s `ErrorKind::ConnectionFailed`
    // classification of a real connect timeout, which text-matching on
    // "Connect error: connection timed out" cannot see (it matches none of
    // `is_connectivity_failure`'s needles).
    //
    // On a runner inside a 10/8 VPC the kernel may return "refused" or
    // "no route" instead of timing out. Both still map to
    // `ErrorKind::ConnectionFailed`, so the test stays green either way — it
    // just stops exercising the *timeout* path it is named for. That is
    // acceptable: the timeout-specific behaviour is a property of the OS
    // network stack, not of our classification code, and what the test
    // actually asserts is that the probe reports unreachable.
    #[test]
    fn sequencer_connectivity_failure_corroborates_bare_timeout_when_blackholed() {
        let addr = "http://10.255.255.1:3040";
        let real_wallet_wording = "Error: Request timeout";
        assert!(
            sequencer_connectivity_failure(real_wallet_wording, addr),
            "a blackholed sequencer (SYN dropped, not refused) must still be \
             classified as connectivity — this is the exact scenario the PR \
             sets out to fix"
        );
    }

    // A raw TCP accept is not enough to prove OUR sequencer is up — some
    // other process could hold the port. So the probe must go through RPC: a
    // listener that actually answers `getLastBlockId` is unambiguously our
    // sequencer, even though the wallet's own message (captured before this
    // response arrived) still says "timed out".
    #[test]
    fn sequencer_connectivity_failure_does_not_misclassify_a_reachable_but_slow_sequencer() {
        let (addr, handle) = spawn_json_rpc_server(r#"{"jsonrpc":"2.0","result":42,"id":1}"#);

        let slow_but_reachable = format!("Error: request to {addr} timed out after 30s");
        assert!(
            !sequencer_connectivity_failure(&slow_but_reachable, &addr),
            "a sequencer that actually answers RPC must not be classified as \
             connectivity, even when the wallet's own message says it timed out"
        );
        handle.join().expect("server thread");
    }

    const ACCOUNT_ID: &str = "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV";

    fn spawn_json_rpc_server(body: &'static str) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://{addr}");

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            drain_http_request(&mut stream);

            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write");
            stream.flush().expect("flush");
        });

        (url, handle)
    }

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
    fn set_wallet_home_env_sets_every_wallet_home_var() {
        let mut command = std::process::Command::new("true");
        set_wallet_home_env(&mut command, "/project/.scaffold/wallet");

        let envs: std::collections::HashMap<_, _> = command
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|v| v.to_os_string())))
            .collect();
        for name in WALLET_HOME_ENV_VARS {
            assert_eq!(
                envs.get(std::ffi::OsStr::new(name)),
                Some(&Some("/project/.scaffold/wallet".into())),
                "missing wallet home env var {name}"
            );
        }
    }

    #[test]
    fn first_public_address_in_listing_parses_wallet_list_output() {
        // Shape of v0.2.0 `wallet account list` output right after the CLI
        // initializes its storage on first use.
        let output = format!(
            "Persistent storage not found, need to execute setup\n\
             Input password: \n\
             Stored persistent accounts at /tmp/wallet-home/storage.json\n\
             / Public/{ACCOUNT_ID}\n\
             / Private/2ECgkFTaXzwjJBXR7ZKmXYQtpHbvTTHK9Auma4NL9AUo\n"
        );

        let value = first_public_address_in_listing(&output);
        assert_eq!(
            value.as_deref(),
            Some(format!("Public/{ACCOUNT_ID}").as_str())
        );
    }

    #[test]
    fn first_public_address_in_listing_skips_invalid_account_ids() {
        let output = format!(
            "note: Public/not-base58 is not an address\n\
             / Public/abc\n\
             / Public/{ACCOUNT_ID}\n"
        );

        let value = first_public_address_in_listing(&output);
        assert_eq!(
            value.as_deref(),
            Some(format!("Public/{ACCOUNT_ID}").as_str())
        );
    }

    #[test]
    fn first_public_address_in_listing_returns_none_without_public_entries() {
        let output = "/ Private/2ECgkFTaXzwjJBXR7ZKmXYQtpHbvTTHK9Auma4NL9AUo\n";
        assert!(first_public_address_in_listing(output).is_none());
    }

    /// A wallet build that prints a real, well-formed address in a banner or
    /// status line before the listing must not have that address adopted as
    /// the project's default topup destination while the listing itself
    /// carries `/ `-prefixed entries. Validating base58 alone is not enough,
    /// because banner addresses are just as valid as entry ones.
    #[test]
    fn first_public_address_in_listing_prefers_listing_entries_over_banner_addresses() {
        const BANNER_ID: &str = "2ECgkFTaXzwjJBXR7ZKmXYQtpHbvTTHK9Auma4NL9AUo";

        let banner_then_listing = format!(
            "wallet ready; active account Public/{BANNER_ID}\n\
             Accounts:\n\
             / Public/{ACCOUNT_ID}\n\
             / Private/{BANNER_ID}\n"
        );
        assert_eq!(
            first_public_address_in_listing(&banner_then_listing).as_deref(),
            Some(format!("Public/{ACCOUNT_ID}").as_str()),
            "the first listing entry wins over any earlier banner address"
        );
    }

    /// The `/ ` entry marker is not a documented contract of the wallet CLI.
    /// A build that prints its accounts without the marker must still yield
    /// an address: returning `None` here is what leaves `wallet.state` with
    /// no `default_address=` line (scaffold#240).
    #[test]
    fn first_public_address_in_listing_falls_back_when_no_entry_marker_is_present() {
        let unmarked = format!("Accounts:\n  Public/{ACCOUNT_ID}\n");
        assert_eq!(
            first_public_address_in_listing(&unmarked).as_deref(),
            Some(format!("Public/{ACCOUNT_ID}").as_str()),
            "an unmarked candidate is better than no default address at all"
        );
    }

    /// Pins which account wins on the shape the repo's own fake wallet in
    /// `tests/cli.rs` prints: an unmarked `Preconfigured Public/<id>` line
    /// followed by a marked `/ Public/<id>` entry. The marked entry is the
    /// listing entry, so it is the one adopted.
    #[test]
    fn first_public_address_in_listing_prefers_marked_entry_over_earlier_unmarked_line() {
        const MARKED_ID: &str = "8zxWNm1qh6FLsJpVBuDxdxcTm55qHPgFEdqJpPVu1fuy";

        let output = format!(
            "Preconfigured Public/{ACCOUNT_ID}\n\
             / Public/{MARKED_ID}\n"
        );
        assert_eq!(
            first_public_address_in_listing(&output).as_deref(),
            Some(format!("Public/{MARKED_ID}").as_str()),
            "a `/ `-marked entry wins over an earlier unmarked Public/ mention"
        );
    }

    /// The listing shape a **real** wallet prints on the path this function
    /// exists for: a debug config with `initial_accounts` removed. The wallet
    /// still emits its own built-in `Preconfigured …` entries above the
    /// stored accounts, so "the config ships no preconfigured account" does
    /// not imply "the listing contains none". Captured from a live run
    /// against the pinned LEZ during review of scaffold#246.
    ///
    /// This is what makes the `/ ` tier load-bearing rather than defensive
    /// garnish: a first-`Public/`-token scan adopts `6iArKUXx…`, four lines
    /// above the account the wallet actually created in its own storage.
    #[test]
    fn first_public_address_in_listing_skips_preconfigured_entries_on_real_output() {
        const STORED_ID: &str = "EkA2CTdiiPw6b2syzMxBZcZ33n4W5RCbWH3DDsonWy6w";

        let output = "\
            Preconfigured Public/6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV\n\
            Preconfigured Public/7wHg9sbJwc6h3NP1S9bekfAzB8CHifEcxKswCKUt3YQo\n\
            Preconfigured Private/5ya25h4Xc9GAmrGB2WrTEnEWtQKJwRwQx3Xfo2tucNcE\n\
            Preconfigured Private/E8HwiTyQe4H9HK7icTvn95HQMnzx49mP9A2ddtMLpNaN\n\
            / Public/EkA2CTdiiPw6b2syzMxBZcZ33n4W5RCbWH3DDsonWy6w\n\
            / Private/4vgGxgZ5vkP99trgfVDLaF7o5NdHjnJUh26BGRUqCmRD\n";

        assert_eq!(
            first_public_address_in_listing(output).as_deref(),
            Some(format!("Public/{STORED_ID}").as_str()),
            "the stored account must win over the wallet's Preconfigured entries"
        );
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
        let (url, handle) = spawn_json_rpc_server(r#"{"jsonrpc":"2.0","result":42,"id":1}"#);

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
        // Response with non-numeric `result`
        let (url, handle) = spawn_json_rpc_server(r#"{"jsonrpc":"2.0","result":{},"id":1}"#);

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
        let (url, handle) = spawn_json_rpc_server(
            r#"{"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found"},"id":1}"#,
        );

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
