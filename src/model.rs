use std::path::PathBuf;

use serde::Serialize;

use crate::constants::{
    DEFAULT_SEQUENCER_BIN_NAME, DEFAULT_SEQUENCER_CONFIG_REL_PATH, SEQUENCER_TARGET_REL_DIR,
};

/// A pinned external git dependency.
///
/// `source` is a clone URL or, for `RepoBuild::NixFlake`, a flake-style ref
/// (e.g. `github:owner/repo`). `pin` is the git SHA. `attr` is the flake
/// output attribute when `build == NixFlake`, otherwise empty (cargo build
/// targets are decided code-side, not data-driven).
///
/// `path` is an optional override for the on-disk clone location:
/// - `Some` (non-empty after construction): authoritative — used literally
///   if absolute, joined to `project.root` if relative. Set by
///   `--vendor-deps`, hand-edited overrides, or pre-portability scaffold.toml
///   files in the wild (back-compat).
/// - `None` / empty: derive `<cache_root>/repos/<name>/<pin>` at runtime.
#[derive(Clone, Debug, Default)]
pub(crate) struct RepoRef {
    pub(crate) source: String,
    pub(crate) pin: String,
    pub(crate) build: RepoBuild,
    pub(crate) attr: String,
    pub(crate) path: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RepoBuild {
    #[default]
    Cargo,
    NixFlake,
}

impl RepoBuild {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::NixFlake => "nix-flake",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "cargo" => Some(Self::Cargo),
            "nix-flake" => Some(Self::NixFlake),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LocalnetConfig {
    pub(crate) port: u16,
    pub(crate) risc0_dev_mode: bool,
    /// Cargo package name for the LEZ sequencer. Used both at build time
    /// (`cargo build -p <sequencer_binary>`) and at runtime
    /// (`<lez>/target/release/<sequencer_binary>`). See
    /// `DEFAULT_SEQUENCER_BIN_NAME`.
    pub(crate) sequencer_binary: String,
    /// Path to the sequencer's JSON config file. Resolved relative to the
    /// LEZ checkout root if not absolute. Mutated in place by
    /// `patch_sequencer_port` in `commands::localnet` to honour
    /// `[localnet].port`, so it must point at a regular file the scaffold
    /// has write access to.
    pub(crate) sequencer_config_path: String,
}

impl LocalnetConfig {
    /// Resolved on-disk path to the sequencer binary inside the LEZ
    /// checkout. Mirrors `Path::join`'s semantics: an absolute
    /// `sequencer_binary` replaces `<lez>/target/release/`, but the
    /// validator in `config.rs` rejects absolute / path-separator values
    /// so we never reach that branch in normal flow.
    pub(crate) fn sequencer_bin_path(&self, lez: &std::path::Path) -> PathBuf {
        lez.join(SEQUENCER_TARGET_REL_DIR)
            .join(&self.sequencer_binary)
    }

    /// Resolved on-disk path to the sequencer config file. Absolute paths
    /// are taken as-is; relative paths are resolved against the LEZ
    /// checkout root so the same value works on every machine.
    pub(crate) fn sequencer_config_resolved_path(&self, lez: &std::path::Path) -> PathBuf {
        let p = PathBuf::from(&self.sequencer_config_path);
        if p.is_absolute() {
            p
        } else {
            lez.join(p)
        }
    }
}

impl Default for LocalnetConfig {
    fn default() -> Self {
        Self {
            port: 3040,
            risc0_dev_mode: true,
            sequencer_binary: DEFAULT_SEQUENCER_BIN_NAME.to_string(),
            sequencer_config_path: DEFAULT_SEQUENCER_CONFIG_REL_PATH.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub(crate) version: String,
    pub(crate) cache_root: String,
    pub(crate) lez: RepoRef,
    pub(crate) spel: RepoRef,
    /// `[repos.basecamp]`. Optional — projects that don't use basecamp can
    /// omit the section. When `None`, basecamp commands fail with a hint
    /// pointing at `init`.
    pub(crate) basecamp_repo: Option<RepoRef>,
    /// `[repos.lgpm]`. Optional, same reasoning as `basecamp_repo`.
    pub(crate) lgpm_repo: Option<RepoRef>,
    pub(crate) wallet_home_dir: String,
    pub(crate) framework: FrameworkConfig,
    pub(crate) localnet: LocalnetConfig,
    /// `[modules.<name>]` — top-level Logos module catalog (was
    /// `[basecamp.modules.<name>]` pre-consolidation).
    pub(crate) modules: std::collections::BTreeMap<String, ModuleEntry>,
    /// `[basecamp]` — runtime config only (port_base, port_stride). Pin and
    /// source moved to `[repos.basecamp]`; modules moved to `[modules.*]`.
    pub(crate) basecamp: Option<BasecampConfig>,
    /// `[run]` — `lgs run` pipeline config (post-deploy hooks).
    pub(crate) run: RunConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModuleRole {
    /// Module the developer is building and shipping. `build-portable` operates
    /// only on these (attr-swap to `#lgx-portable`).
    Project,
    /// Runtime companion resolved from another source's `metadata.json`
    /// `dependencies` array or declared explicitly by the developer.
    /// `build-portable` skips these — the target AppImage provides its own.
    Dependency,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModuleEntry {
    pub(crate) flake: String,
    pub(crate) role: ModuleRole,
}

/// `[basecamp]` runtime config only. Pin and source moved to
/// `[repos.basecamp]`; lgpm moved to `[repos.lgpm]`; modules moved to
/// `[modules.*]`.
#[derive(Clone, Debug)]
pub(crate) struct BasecampConfig {
    pub(crate) port_base: u16,
    pub(crate) port_stride: u16,
}

impl Default for BasecampConfig {
    fn default() -> Self {
        Self {
            port_base: 60000,
            port_stride: 10,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Project {
    pub(crate) root: PathBuf,
    pub(crate) config: Config,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LocalnetState {
    pub(crate) sequencer_pid: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum BasecampSource {
    Path(String),
    Flake(String),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BasecampState {
    pub(crate) pin: String,
    pub(crate) basecamp_bin: String,
    pub(crate) lgpm_bin: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CheckRow {
    pub(crate) status: CheckStatus,
    pub(crate) name: String,
    pub(crate) detail: String,
    pub(crate) remediation: Option<String>,
}

pub(crate) struct Captured {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalnetOwnership {
    Managed,
    Foreign,
    StaleState,
    ManagedNotReady,
    Stopped,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct LocalnetStatusReport {
    pub(crate) tracked_pid: Option<u32>,
    pub(crate) tracked_running: bool,
    pub(crate) listener_present: bool,
    pub(crate) listener_pid: Option<u32>,
    pub(crate) ownership: LocalnetOwnership,
    pub(crate) ready: bool,
    pub(crate) log_path: String,
    pub(crate) remediation: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DoctorSummary {
    pub(crate) pass: usize,
    pub(crate) warn: usize,
    pub(crate) fail: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DoctorReport {
    pub(crate) status: String,
    pub(crate) summary: DoctorSummary,
    pub(crate) checks: Vec<CheckRow>,
    pub(crate) next_steps: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CollectedItem {
    pub(crate) path: String,
    pub(crate) source: String,
    pub(crate) notes: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SkippedItem {
    pub(crate) path: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct RedactionSummary {
    pub(crate) files_redacted: usize,
    pub(crate) replacements: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ToolCommandResult {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) status: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReportManifest {
    pub(crate) generated_at_unix: u64,
    pub(crate) project_root: String,
    pub(crate) output_archive: String,
    pub(crate) include_count: usize,
    pub(crate) skip_count: usize,
    pub(crate) redaction: RedactionSummary,
    pub(crate) collected: Vec<CollectedItem>,
    pub(crate) skipped: Vec<SkippedItem>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct FrameworkConfig {
    pub(crate) kind: String,
    pub(crate) version: String,
    pub(crate) idl: FrameworkIdlConfig,
}

#[derive(Clone, Debug)]
pub(crate) struct FrameworkIdlConfig {
    pub(crate) spec: String,
    pub(crate) path: String,
}

/// `[run]` — config for the `lgs run` pipeline. Branch-1 surface is the
/// minimal inline `post_deploy` hook(s); profile/reset support arrives in
/// later branches of the run-command stack.
#[derive(Clone, Debug, Default)]
pub(crate) struct RunConfig {
    /// Inline `[run].post_deploy` — string (single hook) or array (multiple).
    /// Empty when not configured.
    pub(crate) post_deploy: Vec<String>,
}
