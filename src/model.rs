use std::path::PathBuf;

use serde::Serialize;

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
    /// `[repos.<name>.attr]` per-platform map (nix system triple -> flake
    /// output attribute), e.g. `attr.aarch64-darwin = "bin-macos-app"`. Empty
    /// when `attr` is given in scalar form; `effective_attr` prefers a matching
    /// entry here and falls back to the scalar `attr`.
    pub(crate) attr_platform: std::collections::BTreeMap<String, String>,
    pub(crate) path: String,
}

impl RepoRef {
    /// Resolve the flake output attribute for `system` (a nix system triple).
    /// Prefers a per-platform `attr_platform` entry; falls back to the scalar
    /// `attr` when the platform isn't mapped (or no map was given).
    pub(crate) fn effective_attr(&self, system: &str) -> &str {
        self.attr_platform
            .get(system)
            .map(String::as_str)
            .unwrap_or(self.attr.as_str())
    }
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
}

impl Default for LocalnetConfig {
    fn default() -> Self {
        Self {
            port: 3040,
            risc0_dev_mode: true,
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
    /// `[modules.<name>].standalone_app` — optional flake app attr that
    /// `basecamp run --host standalone` invokes via `nix run <flake>#<attr>`.
    /// `None` runs the flake's default app (`apps.<system>.default`).
    pub(crate) standalone_app: Option<String>,
}

/// `[basecamp]` runtime config only. Pin and source moved to
/// `[repos.basecamp]`; lgpm moved to `[repos.lgpm]`; modules moved to
/// `[modules.*]`.
#[derive(Clone, Debug)]
pub(crate) struct BasecampConfig {
    pub(crate) port_base: u16,
    pub(crate) port_stride: u16,
    /// `[basecamp.env]` — plain env vars injected into every profile's launch
    /// (replace semantics).
    pub(crate) env: std::collections::BTreeMap<String, String>,
    /// `[basecamp.env_append]` — path-style env. Each list is `:`-joined and
    /// appended onto the value `lgs` inherited at launch time (so basecamp's
    /// own paths aren't clobbered). Applied before `env`.
    pub(crate) env_append: std::collections::BTreeMap<String, Vec<String>>,
    /// `[basecamp.profiles.<name>]` — per-profile launch config, keyed by an
    /// arbitrary profile name (no alice/bob gate).
    pub(crate) profiles: std::collections::BTreeMap<String, BasecampProfile>,
}

impl Default for BasecampConfig {
    fn default() -> Self {
        Self {
            port_base: 60000,
            port_stride: 10,
            env: std::collections::BTreeMap::new(),
            env_append: std::collections::BTreeMap::new(),
            profiles: std::collections::BTreeMap::new(),
        }
    }
}

/// `[basecamp.profiles.<name>]` — per-profile launch configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BasecampProfile {
    /// `.env` — per-profile plain env (replace semantics). Wins over the
    /// global `[basecamp.env]` for the launched profile.
    pub(crate) env: std::collections::BTreeMap<String, String>,
    /// `.env_file` — path (project-relative or absolute) to a dotenv-style
    /// `KEY=VALUE` file sourced before the `env` layers override it.
    pub(crate) env_file: Option<String>,
    /// `.runtime_dir` — short `TMPDIR`/`XDG_RUNTIME_DIR` root, to dodge the
    /// macOS `sun_path == 104` Unix-socket path limit. Project-relative or
    /// absolute; defaults to `/tmp/lgs-<profile>` on macOS when unset.
    pub(crate) runtime_dir: Option<String>,
    /// `.log_file` — tee basecamp's stdout/stderr to this file on launch
    /// (project-relative or absolute). Overridden by `launch --log-file`.
    pub(crate) log_file: Option<String>,
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
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Clone, Debug, Serialize)]
pub struct CheckRow {
    pub status: CheckStatus,
    pub name: String,
    pub detail: String,
    pub remediation: Option<String>,
}

pub(crate) struct Captured {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalnetOwnership {
    Managed,
    Foreign,
    StaleState,
    ManagedNotReady,
    Stopped,
}

#[derive(Clone, Debug, Serialize)]
pub struct LocalnetStatusReport {
    pub tracked_pid: Option<u32>,
    pub tracked_running: bool,
    pub listener_present: bool,
    pub listener_pid: Option<u32>,
    pub ownership: LocalnetOwnership,
    pub ready: bool,
    pub log_path: String,
    pub remediation: Vec<String>,
}

/// Machine-readable shape for `localnet logs --json`. Mirrors the human
/// output: `lines` holds the last `tail` lines of the sequencer log (empty
/// when the log is missing or empty), and `exists` lets consumers tell an
/// absent log file apart from an empty one without parsing prose.
#[derive(Clone, Debug, Serialize)]
pub struct LocalnetLogsReport {
    pub log_path: String,
    pub exists: bool,
    pub tail: usize,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorSummary {
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    pub status: String,
    pub summary: DoctorSummary,
    pub checks: Vec<CheckRow>,
    pub next_steps: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CollectedItem {
    pub path: String,
    pub source: String,
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SkippedItem {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RedactionSummary {
    pub files_redacted: usize,
    pub replacements: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolCommandResult {
    pub name: String,
    pub command: String,
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReportManifest {
    pub generated_at_unix: u64,
    pub project_root: String,
    pub output_archive: String,
    pub include_count: usize,
    pub skip_count: usize,
    pub redaction: RedactionSummary,
    pub collected: Vec<CollectedItem>,
    pub skipped: Vec<SkippedItem>,
    pub warnings: Vec<String>,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RunProfile {
    /// Wipe rocksdb + wallet, restart sequencer, re-seed default wallet.
    /// Broader than `lgs localnet reset`: re-establishes the documented
    /// fresh-project state for the full deploy cycle. Suitable both as
    /// a manual recovery and as the per-run default for fixture-based
    /// deterministic test suites where PDA-key collisions force a wipe.
    pub(crate) reset: bool,
    pub(crate) post_deploy: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RunConfig {
    /// Name of a profile in `profiles` to use when no `--profile` flag is
    /// passed. If `Some` and the named profile exists, it shadows
    /// `inline`.
    pub(crate) default_profile: Option<String>,
    /// Inline `[run]` keys parsed flat (not under a `[run.profiles.*]`
    /// section) — the unnamed/legacy profile used when no `--profile`
    /// flag and no `default_profile` resolves.
    pub(crate) inline: RunProfile,
    /// Named profiles parsed from `[run.profiles.<name>]` sub-sections.
    pub(crate) profiles: std::collections::BTreeMap<String, RunProfile>,
    /// `[run.watch]` — filters and debounce for `lgs run --watch`.
    pub(crate) watch: WatchConfig,
}

/// `[run.watch]` — controls which filesystem changes trigger a `--watch`
/// re-run, and how long to coalesce a burst of saves.
///
/// Resolution for a changed path: it triggers a re-run iff it matches at
/// least one `include` glob (or `include` is empty, meaning "any path") AND
/// matches zero `exclude` globs. `exclude` always wins. Globs are
/// project-relative, gitignore-style (`**` spans path segments, `*`/`?`
/// match within a segment; a slash-less pattern matches at any depth).
/// Built-in ignores (`.scaffold`, `target`, `.git`, the IDL output dir)
/// always apply regardless of these filters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WatchConfig {
    pub(crate) include: Vec<String>,
    pub(crate) exclude: Vec<String>,
    /// Debounce window in milliseconds. `None` → built-in default (500ms).
    /// Overridden per-invocation by `--watch-debounce-ms`.
    pub(crate) debounce_ms: Option<u64>,
}

impl RunConfig {
    /// Resolve the effective `RunProfile` for a given `--profile` selector.
    /// `Some(name)` errors if the profile is absent. `None` falls back to
    /// `default_profile` if set, else the inline values.
    pub(crate) fn resolve_profile(&self, selector: Option<&str>) -> anyhow::Result<RunProfile> {
        match selector {
            Some(name) => self.profiles.get(name).cloned().ok_or_else(|| {
                let known: Vec<&str> = self.profiles.keys().map(String::as_str).collect();
                anyhow::anyhow!(
                    "scaffold.toml has no [run.profiles.{name}] section. Known profiles: [{}]",
                    known.join(", ")
                )
            }),
            None => match self.default_profile.as_deref() {
                Some(name) => self.profiles.get(name).cloned().ok_or_else(|| {
                    anyhow::anyhow!(
                        "scaffold.toml `[run].default_profile = \"{name}\"` but no matching [run.profiles.{name}] section"
                    )
                }),
                None => Ok(self.inline.clone()),
            },
        }
    }
}
