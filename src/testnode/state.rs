//! Caller-provided state seeding for test nodes.
//!
//! Integration tests usually construct a precise pre-state (token mints,
//! vaults, program-owned accounts) and need the sequencer to start from
//! that same world. The pinned sequencer offers exactly two supported ways
//! to begin from caller-defined state, and this module wraps both — with
//! validation up front so incompatible snapshots fail with a targeted error
//! instead of a confusing runtime failure deep inside the node:
//!
//! 1. **Genesis-config seeding.** When the sequencer starts with no
//!    database, its config may carry `initial_public_accounts`
//!    (`{account_id, balance}` pairs) and `initial_private_accounts`
//!    (`{npk, account}` commitments); the genesis state is then built from
//!    exactly those accounts — no implicit wallets, sample programs, or
//!    testnet default accounts. Limitations of this path at the pinned
//!    revision: public accounts can seed **balance only** (no data, nonce,
//!    or program owner), while private accounts seed a full account behind
//!    a commitment. Snapshots that need unsupported public-account fields
//!    are rejected during validation as a storage-schema mismatch.
//! 2. **Database seeding.** A state directory containing a `rocksdb/`
//!    database (exported from a previous node) is copied verbatim; the
//!    sequencer resumes from it exactly.
//!
//! `lgs test-node state seed` validates a snapshot and produces a state
//! directory; `lgs test-node start --state <dir>` consumes either kind.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

use crate::model::Project;
use crate::DynResult;

use super::pins::{resolve_test_node_pins, PinOverrides};

/// On-disk marker of a config-seeded state directory.
pub(crate) const SEED_FILE: &str = "seed.json";

/// Snapshot format identifier written/accepted by this revision.
pub const STATE_SNAPSHOT_FORMAT: &str = "lgs-state-snapshot/1";
/// Account snapshot format produced by `test-node snapshot accounts`.
pub const ACCOUNT_SNAPSHOT_FORMAT: &str = "lgs-account-snapshot/1";
/// The NSSA state layout the pinned LEZ revision uses.
pub const STATE_FORMAT_VERSION: &str = "nssa-v03";

/// Validation failures, distinguished per class so harnesses can branch.
#[derive(Debug, Error)]
pub enum SeedError {
    /// The input file's `format` marker is unknown or missing.
    #[error("snapshot format mismatch: {0}")]
    FormatMismatch(String),
    /// The snapshot needs state the pinned sequencer cannot seed (e.g.
    /// public-account data/nonce/owner via genesis config).
    #[error("storage schema mismatch: {0}")]
    StorageSchemaMismatch(String),
    /// The snapshot was produced against a different LEZ pin.
    #[error("lez pin mismatch: {0}")]
    PinMismatch(String),
    /// An account inside the snapshot could not be decoded.
    #[error("account decode error: {0}")]
    AccountDecode(String),
}

/// What the current project pins accept as seedable state.
#[derive(Clone, Debug, Serialize)]
pub struct StateSchema {
    /// NSSA state layout of the pinned sequencer.
    pub state_format_version: String,
    /// LEZ ref the project pins.
    pub lez_ref: String,
    /// Resolved LEZ commit when the checkout is materialised.
    pub lez_commit: Option<String>,
    /// Snapshot formats `state seed` accepts.
    pub accepted_inputs: Vec<String>,
    /// Public-account fields seedable through the genesis config.
    pub public_account_fields: Vec<String>,
    /// Private accounts are seeded as full accounts behind commitments.
    pub private_account_fields: Vec<String>,
    /// Sequencer config keys the seed is applied through.
    pub config_keys: Vec<String>,
}

impl StateSchema {
    /// Identify the exact snapshot shapes accepted by `project`'s pins.
    /// Internal entry point; the public path is
    /// [`crate::api::testnode::state_schema`], which takes an `api::Project`.
    pub(crate) fn for_project(project: &Project) -> DynResult<Self> {
        let pins = resolve_test_node_pins(project, &PinOverrides::default())?;
        Ok(Self {
            state_format_version: STATE_FORMAT_VERSION.to_string(),
            lez_ref: pins.lez_ref,
            lez_commit: pins.lez_resolved_commit,
            accepted_inputs: vec![
                format!("{STATE_SNAPSHOT_FORMAT} (JSON)"),
                format!("{ACCOUNT_SNAPSHOT_FORMAT} (JSON, public balances only)"),
                "state directory containing a rocksdb/ database".to_string(),
            ],
            public_account_fields: vec!["account_id".to_string(), "balance".to_string()],
            private_account_fields: vec![
                "npk".to_string(),
                "account.program_owner".to_string(),
                "account.balance".to_string(),
                "account.data".to_string(),
                "account.nonce".to_string(),
            ],
            config_keys: vec![
                "initial_public_accounts".to_string(),
                "initial_private_accounts".to_string(),
            ],
        })
    }
}

/// One public account to seed (`{account_id, balance}` — the only public
/// fields the pinned genesis config supports).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublicSeedAccount {
    /// Base58 account id.
    pub account_id: String,
    pub balance: u128,
}

/// A private account to seed: full account state behind a commitment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrivateSeedAccount {
    /// Nullifier public key, 32 bytes.
    pub npk: Vec<u8>,
    pub account: SeedAccountState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeedAccountState {
    #[serde(default)]
    pub program_owner: Vec<u32>,
    pub balance: u128,
    #[serde(default)]
    pub data: Vec<u8>,
    #[serde(default)]
    pub nonce: u128,
}

/// A validated, typed in-memory state snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub format: String,
    /// LEZ commit the snapshot was produced against, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lez_commit: Option<String>,
    #[serde(default)]
    pub public_accounts: Vec<PublicSeedAccount>,
    #[serde(default)]
    pub private_accounts: Vec<PrivateSeedAccount>,
}

impl StateSnapshot {
    /// Build a snapshot from typed in-memory accounts.
    pub fn new(
        public_accounts: Vec<PublicSeedAccount>,
        private_accounts: Vec<PrivateSeedAccount>,
    ) -> Self {
        Self {
            format: STATE_SNAPSHOT_FORMAT.to_string(),
            lez_commit: None,
            public_accounts,
            private_accounts,
        }
    }

    /// Load and validate a snapshot file. Accepts `lgs-state-snapshot/1`
    /// directly and converts `lgs-account-snapshot/1` (from `test-node
    /// snapshot accounts`), rejecting account-snapshot entries that carry
    /// state the genesis config cannot seed.
    pub fn from_file(path: &Path) -> Result<Self, SeedError> {
        let text = fs::read_to_string(path).map_err(|err| {
            SeedError::FormatMismatch(format!("cannot read {}: {err}", path.display()))
        })?;
        let value: Value = serde_json::from_str(&text).map_err(|err| {
            SeedError::FormatMismatch(format!("{} is not valid JSON: {err}", path.display()))
        })?;
        Self::from_json(&value)
    }

    /// Validate a snapshot from parsed JSON.
    pub fn from_json(value: &Value) -> Result<Self, SeedError> {
        let format = value.get("format").and_then(Value::as_str).ok_or_else(|| {
            SeedError::FormatMismatch(
                "missing `format` field; expected lgs-state-snapshot/1 or \
                     lgs-account-snapshot/1"
                    .to_string(),
            )
        })?;

        match format {
            STATE_SNAPSHOT_FORMAT => {
                let snapshot: Self = serde_json::from_value(value.clone())
                    .map_err(|err| SeedError::AccountDecode(err.to_string()))?;
                snapshot.validate()?;
                Ok(snapshot)
            }
            ACCOUNT_SNAPSHOT_FORMAT => Self::from_account_snapshot(value),
            other => Err(SeedError::FormatMismatch(format!(
                "unsupported snapshot format `{other}`; this scaffold revision accepts \
                 {STATE_SNAPSHOT_FORMAT} and {ACCOUNT_SNAPSHOT_FORMAT}"
            ))),
        }
    }

    /// Convert an account snapshot (`test-node snapshot accounts`) into a
    /// seedable state snapshot. Only balance-bearing public accounts can be
    /// carried over; entries with data/nonce are a storage-schema mismatch.
    fn from_account_snapshot(value: &Value) -> Result<Self, SeedError> {
        let entries = value
            .get("accounts")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                SeedError::FormatMismatch("account snapshot has no `accounts` array".to_string())
            })?;

        let mut public_accounts = Vec::new();
        for entry in entries {
            let account_id = entry
                .get("account_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    SeedError::AccountDecode("entry missing `account_id`".to_string())
                })?;
            match entry.get("state").and_then(Value::as_str) {
                Some("missing") => continue,
                Some("present") => {}
                other => {
                    return Err(SeedError::AccountDecode(format!(
                        "account {account_id}: unsupported entry state {other:?}"
                    )))
                }
            }
            let data_len = entry.get("data_len").and_then(Value::as_u64).unwrap_or(0);
            let nonce = entry
                .get("nonce")
                .and_then(|nonce| serde_json::from_value::<u128>(nonce.clone()).ok())
                .unwrap_or(0);
            if data_len > 0 || nonce > 0 {
                return Err(SeedError::StorageSchemaMismatch(format!(
                    "account {account_id} carries data ({data_len} bytes) or a non-zero nonce \
                     ({nonce}); the pinned sequencer's genesis config can seed public accounts \
                     with balance only. Export the node's database directory instead for \
                     full-fidelity seeding."
                )));
            }
            let balance = entry
                .get("balance")
                .and_then(|balance| serde_json::from_value::<u128>(balance.clone()).ok())
                .ok_or_else(|| {
                    SeedError::AccountDecode(format!(
                        "account {account_id}: missing numeric `balance`"
                    ))
                })?;
            public_accounts.push(PublicSeedAccount {
                account_id: account_id.to_string(),
                balance,
            });
        }

        let snapshot = Self {
            format: STATE_SNAPSHOT_FORMAT.to_string(),
            lez_commit: None,
            public_accounts,
            private_accounts: Vec::new(),
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// Validate account encodings: base58 ids of 32 bytes, 32-byte npks,
    /// 8-word program owners.
    pub fn validate(&self) -> Result<(), SeedError> {
        for account in &self.public_accounts {
            let decoded = bs58::decode(&account.account_id)
                .into_vec()
                .map_err(|err| {
                    SeedError::AccountDecode(format!(
                        "public account id `{}` is not base58: {err}",
                        account.account_id
                    ))
                })?;
            if decoded.len() != 32 {
                return Err(SeedError::AccountDecode(format!(
                    "public account id `{}` decodes to {} bytes, expected 32",
                    account.account_id,
                    decoded.len()
                )));
            }
        }
        for (index, private) in self.private_accounts.iter().enumerate() {
            if private.npk.len() != 32 {
                return Err(SeedError::AccountDecode(format!(
                    "private account #{index}: npk has {} bytes, expected 32",
                    private.npk.len()
                )));
            }
            if !private.account.program_owner.is_empty() && private.account.program_owner.len() != 8
            {
                return Err(SeedError::AccountDecode(format!(
                    "private account #{index}: program_owner has {} words, expected 8 (or empty \
                     for the default owner)",
                    private.account.program_owner.len()
                )));
            }
        }
        Ok(())
    }

    /// The `initial_public_accounts` / `initial_private_accounts` values to
    /// inject into the sequencer config.
    pub(crate) fn to_config_values(&self) -> (Value, Value) {
        let public: Vec<Value> = self
            .public_accounts
            .iter()
            .map(|account| {
                json!({
                    "account_id": account.account_id,
                    "balance": account.balance,
                })
            })
            .collect();
        let private: Vec<Value> = self
            .private_accounts
            .iter()
            .map(|private| {
                let owner = if private.account.program_owner.is_empty() {
                    vec![0_u32; 8]
                } else {
                    private.account.program_owner.clone()
                };
                json!({
                    "npk": private.npk,
                    "account": {
                        "program_owner": owner,
                        "balance": private.account.balance,
                        "data": private.account.data,
                        "nonce": private.account.nonce,
                    },
                })
            })
            .collect();
        (Value::Array(public), Value::Array(private))
    }
}

/// Result of `state seed`: a directory `test-node start --state` accepts,
/// plus the metadata tests need to verify the seeded state.
#[derive(Clone, Debug, Serialize)]
pub struct SeededState {
    /// The produced state directory.
    pub state_dir: PathBuf,
    /// LEZ commit the seed targets (the project's resolved pin), when the
    /// checkout is materialised.
    pub lez_commit: Option<String>,
    pub state_format_version: String,
    pub public_account_count: usize,
    pub private_account_count: usize,
    /// `config` (genesis-config seeding) or `database` (rocksdb copy).
    pub seed_kind: String,
    pub warnings: Vec<String>,
}

/// Validate `input` (snapshot file or rocksdb state directory) and produce
/// a state directory under `output_dir` (or a fresh directory next to the
/// project's test nodes).
pub fn seed_state(
    project: &Project,
    input: &Path,
    output_dir: Option<&Path>,
) -> DynResult<SeededState> {
    let pins = resolve_test_node_pins(project, &PinOverrides::default())?;
    let mut warnings = Vec::new();

    let state_dir = match output_dir {
        Some(dir) => {
            fs::create_dir_all(dir)?;
            dir.to_path_buf()
        }
        None => {
            let base = project.root.join(super::TEST_NODES_REL_DIR).join("seeds");
            fs::create_dir_all(&base)?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let dir = base.join(format!("seed-{stamp}-{}", std::process::id()));
            fs::create_dir_all(&dir)?;
            dir
        }
    };

    // Database-directory input: validate and copy verbatim.
    if input.is_dir() {
        let db_dir = if input.join("rocksdb").is_dir() {
            input.join("rocksdb")
        } else {
            input.to_path_buf()
        };
        if !db_dir.join("CURRENT").exists() {
            return Err(SeedError::FormatMismatch(format!(
                "{} is neither a snapshot file nor a rocksdb state directory (no CURRENT file)",
                input.display()
            ))
            .into());
        }
        copy_dir_recursive(&db_dir, &state_dir.join("rocksdb"))?;
        return Ok(SeededState {
            state_dir,
            lez_commit: pins.lez_resolved_commit,
            state_format_version: STATE_FORMAT_VERSION.to_string(),
            public_account_count: 0,
            private_account_count: 0,
            seed_kind: "database".to_string(),
            warnings: vec![
                "database seeds are copied verbatim; account counts are not inspected".to_string(),
            ],
        });
    }

    // Snapshot-file input: validate, check pins, and write seed.json.
    let snapshot = StateSnapshot::from_file(input)?;

    if let (Some(snapshot_commit), Some(project_commit)) =
        (&snapshot.lez_commit, &pins.lez_resolved_commit)
    {
        if snapshot_commit != project_commit {
            return Err(SeedError::PinMismatch(format!(
                "snapshot was produced against LEZ {snapshot_commit}, but the project pins \
                 {project_commit}. Re-export the snapshot against the project pin, or update \
                 the snapshot's `lez_commit`."
            ))
            .into());
        }
    } else if snapshot.lez_commit.is_none() {
        warnings.push(
            "snapshot carries no `lez_commit`; pin compatibility was not verified".to_string(),
        );
    }

    let normalized = StateSnapshot {
        lez_commit: pins.lez_resolved_commit.clone().or(snapshot.lez_commit),
        ..snapshot
    };
    let seed_path = state_dir.join(SEED_FILE);
    fs::write(
        &seed_path,
        format!("{}\n", serde_json::to_string_pretty(&normalized)?),
    )?;

    Ok(SeededState {
        state_dir,
        lez_commit: normalized.lez_commit.clone(),
        state_format_version: STATE_FORMAT_VERSION.to_string(),
        public_account_count: normalized.public_accounts.len(),
        private_account_count: normalized.private_accounts.len(),
        seed_kind: "config".to_string(),
        warnings,
    })
}

/// Export public accounts from a running node into a
/// `lgs-state-snapshot/1` file. The pinned RPC has no account enumeration
/// and no private-state export, so the caller names the accounts; for
/// full-fidelity state (including account data and private commitments),
/// export the stopped node's database directory instead.
pub fn export_state_snapshot(
    rpc_url: &str,
    account_ids: &[String],
    lez_commit: Option<String>,
    output: &Path,
) -> DynResult<StateSnapshot> {
    use super::accounts::{AccountValue, ReadAt};
    use super::client::TestNodeClient;

    let client = TestNodeClient::new(rpc_url);
    let batch = client.accounts(account_ids, ReadAt::Latest)?;

    let mut public_accounts = Vec::new();
    for entry in batch.accounts {
        match entry.value {
            AccountValue::Present {
                balance,
                nonce,
                data_len,
                ..
            } => {
                if data_len > 0 || nonce > 0 {
                    return Err(SeedError::StorageSchemaMismatch(format!(
                        "account {} carries data ({data_len} bytes) or nonce ({nonce}); the \
                         genesis config can seed public balances only. Export the node's \
                         database directory for full-fidelity seeding.",
                        entry.account_id
                    ))
                    .into());
                }
                public_accounts.push(PublicSeedAccount {
                    account_id: entry.account_id,
                    balance,
                });
            }
            AccountValue::Missing => {}
            AccountValue::DecodeError { message, .. } => {
                return Err(SeedError::AccountDecode(format!(
                    "account {}: {message}",
                    entry.account_id
                ))
                .into())
            }
        }
    }

    let snapshot = StateSnapshot {
        format: STATE_SNAPSHOT_FORMAT.to_string(),
        lez_commit,
        public_accounts,
        private_accounts: Vec::new(),
    };
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        output,
        format!("{}\n", serde_json::to_string_pretty(&snapshot)?),
    )?;
    Ok(snapshot)
}

/// Load the seed snapshot from a state directory, when present.
pub(crate) fn load_seed_from_state_dir(state_dir: &Path) -> DynResult<Option<StateSnapshot>> {
    let path = state_dir.join(SEED_FILE);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(StateSnapshot::from_file(&path)?))
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> DynResult<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::model::{
        Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, RepoRef, RunConfig,
    };

    const ACCOUNT_ID: &str = "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV";

    fn fixture_project(root: &Path) -> Project {
        Project {
            root: root.to_path_buf(),
            config: Config {
                version: "0.2.0".into(),
                cache_root: ".scaffold/cache".into(),
                lez: RepoRef::default(),
                spel: RepoRef::default(),
                basecamp_repo: None,
                lgpm_repo: None,
                wallet_home_dir: ".scaffold/wallet".into(),
                framework: FrameworkConfig {
                    kind: "default".into(),
                    version: "0.1.0".into(),
                    idl: FrameworkIdlConfig {
                        spec: String::new(),
                        path: String::new(),
                    },
                },
                localnet: LocalnetConfig::default(),
                modules: std::collections::BTreeMap::new(),
                basecamp: None,
                run: RunConfig::default(),
            },
        }
    }

    #[test]
    fn schema_reports_pinned_capabilities() {
        let temp = tempdir().unwrap();
        let project = fixture_project(temp.path());
        let schema = StateSchema::for_project(&project).unwrap();
        assert_eq!(schema.state_format_version, "nssa-v03");
        assert!(schema
            .config_keys
            .contains(&"initial_public_accounts".to_string()));
        assert_eq!(schema.public_account_fields, ["account_id", "balance"]);
    }

    #[test]
    fn snapshot_round_trips_and_validates() {
        let snapshot = StateSnapshot::new(
            vec![PublicSeedAccount {
                account_id: ACCOUNT_ID.to_string(),
                balance: 12_345,
            }],
            vec![PrivateSeedAccount {
                npk: vec![7; 32],
                account: SeedAccountState {
                    program_owner: vec![],
                    balance: 999,
                    data: vec![1, 2],
                    nonce: 0,
                },
            }],
        );
        snapshot.validate().unwrap();

        let value = serde_json::to_value(&snapshot).unwrap();
        let parsed = StateSnapshot::from_json(&value).unwrap();
        assert_eq!(parsed.public_accounts.len(), 1);
        assert_eq!(parsed.private_accounts.len(), 1);

        let (public, private) = parsed.to_config_values();
        assert_eq!(public[0]["account_id"], serde_json::json!(ACCOUNT_ID));
        assert_eq!(public[0]["balance"], serde_json::json!(12_345));
        // Empty owner normalizes to the default 8-word owner.
        assert_eq!(
            private[0]["account"]["program_owner"],
            serde_json::json!([0, 0, 0, 0, 0, 0, 0, 0])
        );
    }

    #[test]
    fn unknown_format_is_format_mismatch() {
        let err =
            StateSnapshot::from_json(&serde_json::json!({ "format": "bogus/9" })).unwrap_err();
        assert!(matches!(err, SeedError::FormatMismatch(_)), "{err}");
    }

    #[test]
    fn bad_account_id_is_decode_error() {
        let snapshot = StateSnapshot::new(
            vec![PublicSeedAccount {
                account_id: "not-base58!!".to_string(),
                balance: 1,
            }],
            vec![],
        );
        let err = snapshot.validate().unwrap_err();
        assert!(matches!(err, SeedError::AccountDecode(_)), "{err}");
    }

    #[test]
    fn bad_npk_is_decode_error() {
        let snapshot = StateSnapshot::new(
            vec![],
            vec![PrivateSeedAccount {
                npk: vec![1; 16],
                account: SeedAccountState {
                    program_owner: vec![],
                    balance: 0,
                    data: vec![],
                    nonce: 0,
                },
            }],
        );
        let err = snapshot.validate().unwrap_err();
        assert!(err.to_string().contains("npk has 16 bytes"), "{err}");
    }

    #[test]
    fn account_snapshot_converts_balances_and_rejects_data() {
        let ok = serde_json::json!({
            "format": ACCOUNT_SNAPSHOT_FORMAT,
            "block_id": 9,
            "accounts": [
                { "account_id": ACCOUNT_ID, "state": "present", "balance": 500, "nonce": 0, "data_len": 0 },
                { "account_id": "missing-one", "state": "missing" },
            ],
        });
        let snapshot = StateSnapshot::from_json(&ok).unwrap();
        assert_eq!(snapshot.public_accounts.len(), 1);
        assert_eq!(snapshot.public_accounts[0].balance, 500);

        let with_data = serde_json::json!({
            "format": ACCOUNT_SNAPSHOT_FORMAT,
            "accounts": [
                { "account_id": ACCOUNT_ID, "state": "present", "balance": 500, "nonce": 0, "data_len": 64 },
            ],
        });
        let err = StateSnapshot::from_json(&with_data).unwrap_err();
        assert!(matches!(err, SeedError::StorageSchemaMismatch(_)), "{err}");
    }

    #[test]
    fn seed_state_writes_seed_file_with_metadata() {
        let temp = tempdir().unwrap();
        let project = fixture_project(temp.path());

        let snapshot = StateSnapshot::new(
            vec![PublicSeedAccount {
                account_id: ACCOUNT_ID.to_string(),
                balance: 777,
            }],
            vec![],
        );
        let input = temp.path().join("snapshot.json");
        fs::write(&input, serde_json::to_string(&snapshot).unwrap()).unwrap();

        let out = temp.path().join("seeded");
        let seeded = seed_state(&project, &input, Some(&out)).unwrap();
        assert_eq!(seeded.seed_kind, "config");
        assert_eq!(seeded.public_account_count, 1);
        assert_eq!(seeded.state_format_version, "nssa-v03");
        assert!(out.join(SEED_FILE).exists());

        let loaded = load_seed_from_state_dir(&out).unwrap().unwrap();
        assert_eq!(loaded.public_accounts[0].balance, 777);
    }

    #[test]
    fn seed_state_rejects_pin_mismatch() {
        let temp = tempdir().unwrap();
        let checkout = temp.path().join("lez");
        // Real git repo so the project pin resolves to a commit.
        let head = {
            use std::process::Command;
            fs::create_dir_all(&checkout).unwrap();
            for args in [
                vec!["init", "--quiet", "--initial-branch=main"],
                vec!["config", "user.email", "t@example.com"],
                vec!["config", "user.name", "test"],
                vec!["config", "commit.gpgsign", "false"],
            ] {
                assert!(Command::new("git")
                    .args(&args)
                    .current_dir(&checkout)
                    .status()
                    .unwrap()
                    .success());
            }
            fs::write(checkout.join("x"), "x").unwrap();
            assert!(Command::new("git")
                .args(["add", "."])
                .current_dir(&checkout)
                .status()
                .unwrap()
                .success());
            assert!(Command::new("git")
                .args(["commit", "--quiet", "-m", "seed"])
                .current_dir(&checkout)
                .status()
                .unwrap()
                .success());
            let out = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&checkout)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        let mut project = fixture_project(temp.path());
        project.config.lez = RepoRef {
            pin: head,
            path: checkout.display().to_string(),
            ..Default::default()
        };

        let mut snapshot = StateSnapshot::new(vec![], vec![]);
        snapshot.lez_commit = Some("0".repeat(40));
        let input = temp.path().join("snapshot.json");
        fs::write(&input, serde_json::to_string(&snapshot).unwrap()).unwrap();

        let err = seed_state(&project, &input, Some(&temp.path().join("out"))).unwrap_err();
        let seed_err = err.downcast_ref::<SeedError>().expect("SeedError");
        assert!(matches!(seed_err, SeedError::PinMismatch(_)), "{seed_err}");
    }

    #[test]
    fn seed_state_accepts_database_directory() {
        let temp = tempdir().unwrap();
        let project = fixture_project(temp.path());

        let db = temp.path().join("exported/rocksdb");
        fs::create_dir_all(&db).unwrap();
        fs::write(db.join("CURRENT"), "MANIFEST-000001").unwrap();
        fs::write(db.join("000001.sst"), "data").unwrap();

        let out = temp.path().join("seeded");
        let seeded = seed_state(&project, &temp.path().join("exported"), Some(&out)).unwrap();
        assert_eq!(seeded.seed_kind, "database");
        assert_eq!(
            fs::read_to_string(out.join("rocksdb/CURRENT")).unwrap(),
            "MANIFEST-000001"
        );

        // A directory that is not a database is a format mismatch.
        let bogus = temp.path().join("bogus");
        fs::create_dir_all(&bogus).unwrap();
        let err = seed_state(&project, &bogus, Some(&temp.path().join("out2"))).unwrap_err();
        let seed_err = err.downcast_ref::<SeedError>().expect("SeedError");
        assert!(
            matches!(seed_err, SeedError::FormatMismatch(_)),
            "{seed_err}"
        );
    }
}
