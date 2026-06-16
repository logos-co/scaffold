//! Public Rust API for logos-scaffold.
//!
//! Everything the `lgs` / `logos-scaffold` CLI can do is available here as a
//! typed library surface, so Rust tests and dev tooling can drive
//! scaffold-managed projects without shelling out and parsing text.
//!
//! Design rules:
//!
//! - Every operation takes an **explicit project root** (via
//!   [`Project::open`] / [`Project::discover`]); nothing depends on the
//!   process working directory except where noted below.
//! - Every operation returns **typed results** ([`LocalnetStatusReport`],
//!   [`DoctorReport`], [`DeployResult`], …) — the same models that back the
//!   CLI's `--json` output — and **categorized errors** ([`Error`]).
//! - Operations that start long-lived processes return handles
//!   ([`LocalnetHandle`]) exposing status and stop/cleanup.
//! - Failures of external commands surface as [`CommandFailed`] with the
//!   rendered command, exit code, and captured diagnostics.
//!
//! Long-running build-style operations ([`Project::setup`],
//! [`Project::build`], [`Project::run`], [`Project::basecamp`]) stream their
//! progress to stdout/stderr exactly like the CLI; their *results* are still
//! typed. [`Project::build`] and [`Project::run`] temporarily change the
//! process working directory (restored afterwards), so do not run them
//! concurrently from multiple threads.
//!
//! # Examples
//!
//! Set up a project and manage its localnet:
//!
//! ```no_run
//! use logos_scaffold::api::{LocalnetStartOptions, Project, SetupOptions};
//!
//! fn main() -> logos_scaffold::api::Result<()> {
//!     let project = Project::open("/path/to/my-app")?;
//!     project.setup(&SetupOptions::default())?;
//!
//!     let node = project.localnet_start(&LocalnetStartOptions::default())?;
//!     println!("sequencer pid={} rpc={}", node.pid(), node.rpc_url());
//!     assert!(node.status().ready);
//!
//!     node.stop()?;
//!     Ok(())
//! }
//! ```
//!
//! Top up the default wallet and deploy:
//!
//! ```no_run
//! use logos_scaffold::api::{DeployOptions, Project, TopupOptions, TopupOutcome};
//!
//! fn main() -> logos_scaffold::api::Result<()> {
//!     let project = Project::open("/path/to/my-app")?;
//!
//!     match project.wallet_topup(&TopupOptions::default())? {
//!         TopupOutcome::Success => {}
//!         TopupOutcome::ConfirmationTimeout { message } => {
//!             eprintln!("funding uncertain: {message}");
//!         }
//!     }
//!
//!     for result in project.deploy(&DeployOptions::default())? {
//!         println!("{}: {:?} (tx={:?})", result.program, result.status, result.tx);
//!     }
//!     Ok(())
//! }
//! ```
//!
//! Run diagnostics and collect a report bundle:
//!
//! ```no_run
//! use logos_scaffold::api::{Project, ReportOptions};
//!
//! fn main() -> logos_scaffold::api::Result<()> {
//!     let project = Project::open("/path/to/my-app")?;
//!
//!     let doctor = project.doctor()?;
//!     println!(
//!         "doctor: {} ({} pass / {} warn / {} fail)",
//!         doctor.status, doctor.summary.pass, doctor.summary.warn, doctor.summary.fail
//!     );
//!
//!     let report = project.report(&ReportOptions::default())?;
//!     println!("archive at {}", report.archive.display());
//!     Ok(())
//! }
//! ```

mod error;
pub mod testnode;

use std::path::{Path, PathBuf};

pub use error::{CommandFailed, Error, Result};

pub use crate::commands::deploy::{DeployResult, DeployStatus};
pub use crate::commands::wallet::{TopupOutcome, WalletListReport};
pub use crate::model::{
    CheckRow, CheckStatus, CollectedItem, DoctorReport, DoctorSummary, LocalnetLogsReport,
    LocalnetOwnership, LocalnetStatusReport, RedactionSummary, ReportManifest, SkippedItem,
    ToolCommandResult,
};

use crate::commands::basecamp::{basecamp_for_project, BasecampAction};
use crate::commands::build::cmd_build_shortcut;
use crate::commands::client::generate_clients_from_current_idl;
use crate::commands::deploy::deploy_for_project;
use crate::commands::doctor::build_doctor_report;
use crate::commands::idl::build_idl_for_current_project;
use crate::commands::init::cmd_init_at;
pub use crate::commands::localnet::LocalnetStopOutcome;

use crate::commands::localnet::{
    build_localnet_status_for_project, localnet_logs_for_project, localnet_reset_for_project,
    localnet_start_for_project, localnet_stop_for_project,
};
use crate::commands::new::{create_project_in, NewCommand};
use crate::commands::report::report_for_project;
use crate::commands::run::{run_for_project, RunInvocation};
use crate::commands::setup::setup_for_project;
use crate::commands::spel::spel_passthrough_for_project;
use crate::commands::wallet::{
    cmd_wallet_proxy, cmd_wallet_topup_inner, wallet_default_set_for_project,
    wallet_list_for_project,
};
use crate::constants::{
    DEFAULT_RUN_LOCALNET_TIMEOUT_SEC, SEQUENCER_BIN_REL_PATH, SPEL_BIN_REL_PATH,
    WALLET_BIN_REL_PATH,
};
use crate::project::{find_project_root, load_project_at, resolve_cache_root, resolve_repo_path};

/// A scaffold-managed project, addressed by an explicit root directory.
///
/// This is the entry point of the API: open (or discover) a project once,
/// then call operations on it. The handle is cheap to clone.
#[derive(Clone, Debug)]
pub struct Project {
    inner: crate::model::Project,
}

impl Project {
    /// Open the project whose root directory is `root` (the directory that
    /// contains `scaffold.toml`). No upward discovery happens; pass the exact
    /// root. Configuration problems surface as [`Error::Config`].
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let inner = load_project_at(root.as_ref()).map_err(|err| Error::Config {
            message: format!("{err:#}"),
        })?;
        Ok(Self { inner })
    }

    /// Discover a project by walking upward from `start_dir` until a
    /// directory containing `scaffold.toml` is found, then open it.
    pub fn discover(start_dir: impl AsRef<Path>) -> Result<Self> {
        let start = start_dir.as_ref().to_path_buf();
        let root = find_project_root(start.clone()).ok_or_else(|| Error::Config {
            message: format!(
                "no scaffold.toml found in {} or any parent directory",
                start.display()
            ),
        })?;
        Self::open(root)
    }

    /// The project root directory.
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// The localnet RPC URL this project is configured for
    /// (`http://127.0.0.1:<localnet.port>`).
    pub fn localnet_rpc_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.inner.config.localnet.port)
    }

    /// Resolve every path scaffold derives for this project: cache root,
    /// pinned repo checkouts, vendored binaries, wallet home, localnet state
    /// and log files, and the circuits directory. Nothing is created or
    /// downloaded; paths are reported whether or not they exist yet.
    pub fn paths(&self) -> Result<ProjectPaths> {
        let (cache_root, source) = resolve_cache_root(&self.inner).map_err(error::classify)?;
        let lez_repo = resolve_repo_path(&self.inner, &self.inner.config.lez, "lez")
            .map_err(error::classify)?;
        let spel_repo = resolve_repo_path(&self.inner, &self.inner.config.spel, "spel")
            .map_err(error::classify)?;
        let circuits_dir =
            crate::circuits::circuits_dir_for_cache_root(&cache_root).map_err(error::classify)?;
        Ok(ProjectPaths {
            root: self.inner.root.clone(),
            scaffold_toml: self.inner.root.join("scaffold.toml"),
            cache_root,
            cache_root_source: source.label().to_string(),
            sequencer_binary: lez_repo.join(SEQUENCER_BIN_REL_PATH),
            wallet_binary: lez_repo.join(WALLET_BIN_REL_PATH),
            spel_binary: spel_repo.join(SPEL_BIN_REL_PATH),
            lez_repo,
            spel_repo,
            wallet_home: self.inner.root.join(&self.inner.config.wallet_home_dir),
            localnet_state: self.inner.root.join(".scaffold/state/localnet.state"),
            sequencer_log: self.inner.root.join(".scaffold/logs/sequencer.log"),
            circuits_dir,
        })
    }

    // ── setup / build / deploy / run ───────────────────────────────────────

    /// Sync pinned repos and build the project-local binaries (sequencer,
    /// wallet, spel). Streams build progress to stdout; external-command
    /// failures surface as [`Error::Command`].
    pub fn setup(&self, options: &SetupOptions) -> Result<()> {
        setup_for_project(&self.inner, options.prebuilt).map_err(error::classify)
    }

    /// Build the project workspace and guest programs (chains `setup`
    /// internally, mirroring `lgs build`). Temporarily changes the process
    /// working directory to the project root.
    pub fn build(&self, options: &BuildOptions) -> Result<()> {
        cmd_build_shortcut(Some(self.inner.root.clone()), options.prebuilt).map_err(error::classify)
    }

    /// Build IDL files for the current project (framework projects only).
    /// Temporarily changes the process working directory to the project root.
    pub fn build_idl(&self) -> Result<()> {
        crate::project::run_in_project_dir(Some(&self.inner.root), || {
            build_idl_for_current_project()
        })
        .map_err(error::classify)
    }

    /// Generate client code from the current IDL files (framework projects
    /// only). Temporarily changes the process working directory to the
    /// project root.
    pub fn build_client(&self) -> Result<()> {
        crate::project::run_in_project_dir(Some(&self.inner.root), || {
            generate_clients_from_current_idl()
        })
        .map_err(error::classify)
    }

    /// Deploy guest programs to the running localnet. Returns one
    /// [`DeployResult`] per attempted program; inspect `status` to decide
    /// whether failures are fatal — partial failure is **not** an `Err`.
    pub fn deploy(&self, options: &DeployOptions) -> Result<Vec<DeployResult>> {
        deploy_for_project(
            &self.inner,
            options.program.clone(),
            options.program_path.clone(),
            false,
        )
        .map_err(error::classify)
    }

    /// Execute the full run pipeline: build → IDL → localnet → wallet topup →
    /// deploy → post-deploy hooks. Streams step progress to stdout and
    /// temporarily changes the process working directory to the project root.
    /// Watch mode is CLI-only.
    pub fn run(&self, options: &RunOptions) -> Result<()> {
        run_for_project(
            &self.inner,
            RunInvocation {
                profile: options.profile.clone(),
                reset: options.reset,
                post_deploy_override: options.post_deploy_override.clone(),
                localnet_timeout_sec: Some(
                    options
                        .localnet_timeout_sec
                        .unwrap_or(DEFAULT_RUN_LOCALNET_TIMEOUT_SEC),
                ),
                watch: false,
                watch_debounce_ms: None,
            },
        )
        .map_err(error::classify)
    }

    // ── localnet lifecycle ─────────────────────────────────────────────────

    /// Start (or reuse) the project's localnet sequencer and wait until it is
    /// ready. Returns a [`LocalnetHandle`] exposing the pid, RPC URL, state
    /// and log paths, plus status/stop operations. The sequencer keeps
    /// running when the handle is dropped — stop it explicitly.
    pub fn localnet_start(&self, options: &LocalnetStartOptions) -> Result<LocalnetHandle> {
        let outcome = localnet_start_for_project(&self.inner, options.timeout_sec)
            .map_err(error::classify)?;
        Ok(LocalnetHandle {
            project: self.inner.clone(),
            pid: outcome.pid,
            reused: outcome.reused,
            rpc_url: outcome.rpc_url,
            state_path: outcome.state_path,
            log_path: outcome.log_path,
        })
    }

    /// Stop the tracked localnet sequencer, if any. Never touches unmanaged
    /// (foreign) listeners; see [`LocalnetStopOutcome`].
    pub fn localnet_stop(&self) -> Result<LocalnetStopOutcome> {
        localnet_stop_for_project(&self.inner).map_err(error::classify)
    }

    /// Current localnet status (tracked pid, listener, ownership, readiness,
    /// remediation hints). Same model as `lgs localnet status --json`.
    pub fn localnet_status(&self) -> LocalnetStatusReport {
        build_localnet_status_for_project(&self.inner)
    }

    /// Tail of the sequencer log. Same model as `lgs localnet logs --json`.
    pub fn localnet_logs(&self, tail: usize) -> Result<LocalnetLogsReport> {
        localnet_logs_for_project(&self.inner, tail).map_err(error::classify)
    }

    /// Reset the localnet: stop the sequencer, wipe its chain database
    /// (and optionally the wallet), restart, and verify block production.
    /// **Destructive** — equivalent to `lgs localnet reset --yes`.
    pub fn localnet_reset(&self, options: &LocalnetResetOptions) -> Result<()> {
        localnet_reset_for_project(
            &self.inner,
            options.reset_wallet,
            options.verify_timeout_sec,
        )
        .map_err(error::classify)
    }

    // ── wallet ─────────────────────────────────────────────────────────────

    /// List wallet accounts via the vendored wallet binary, returning the
    /// captured output. `long` mirrors `wallet list --long`.
    pub fn wallet_list(&self, long: bool) -> Result<WalletListReport> {
        wallet_list_for_project(&self.inner, long).map_err(error::classify)
    }

    /// Top up a wallet from the localnet faucet. With `address: None`, the
    /// project's default wallet is used. A [`TopupOutcome::ConfirmationTimeout`]
    /// means the submission reached the sequencer but confirmation timed out —
    /// funding is uncertain, not failed.
    pub fn wallet_topup(&self, options: &TopupOptions) -> Result<TopupOutcome> {
        cmd_wallet_topup_inner(&self.inner, options.address.clone(), false, false)
            .map_err(error::classify)
    }

    /// Set the project's default wallet address; returns the normalized form
    /// (`Public/<base58>` or `Private/<base58>`).
    pub fn wallet_set_default(&self, address: &str) -> Result<String> {
        wallet_default_set_for_project(&self.inner, address).map_err(error::classify)
    }

    /// Forward raw arguments to the vendored wallet binary with the project's
    /// wallet environment (`NSSA_WALLET_HOME_DIR`), streaming its output.
    /// Non-zero exit surfaces as [`Error::Command`].
    pub fn wallet_passthrough(&self, args: &[String]) -> Result<()> {
        cmd_wallet_proxy(&self.inner, args).map_err(error::classify)
    }

    // ── passthrough / basecamp / diagnostics ──────────────────────────────

    /// Forward arguments to the project-vendored `spel` binary, streaming its
    /// output, and return its exit status (the API never exits the process).
    pub fn spel(&self, args: &[String]) -> Result<std::process::ExitStatus> {
        spel_passthrough_for_project(&self.inner, args).map_err(error::classify)
    }

    /// Execute a basecamp flow (setup, modules, install, launch, develop,
    /// build-portable, doctor). Streams progress to stdout like the CLI.
    pub fn basecamp(&self, command: BasecampCommand) -> Result<()> {
        basecamp_for_project(self.inner.clone(), command.into_action()).map_err(error::classify)
    }

    /// Run all health checks and return the structured report. Same model as
    /// `lgs doctor --json`; a report with failing checks is still `Ok` — read
    /// `summary.fail`.
    pub fn doctor(&self) -> Result<DoctorReport> {
        // Suppress `$ <cmd>` echoes from the probe subprocesses; the report
        // is the product here, not the console transcript.
        let _echo = crate::process::EchoGuard::suppress();
        build_doctor_report(&self.inner).map_err(error::classify)
    }

    /// Collect a sanitized diagnostics archive. Returns the archive path and
    /// the manifest of collected/skipped items.
    pub fn report(&self, options: &ReportOptions) -> Result<ReportOutcome> {
        let outcome = report_for_project(&self.inner, options.out.clone(), options.tail)
            .map_err(error::classify)?;
        Ok(ReportOutcome {
            archive: outcome.archive,
            manifest: outcome.manifest,
        })
    }
}

/// Create a new scaffold project at `parent_dir/<options.name>` and return a
/// handle to it. Equivalent to `lgs new` run from `parent_dir`.
pub fn create_project(
    parent_dir: impl AsRef<Path>,
    options: CreateProjectOptions,
) -> Result<Project> {
    let root = create_project_in(
        parent_dir.as_ref(),
        NewCommand {
            name: options.name,
            template: options.template,
            vendor_deps: options.vendor_deps,
            lez_path: options.lez_path,
            cache_root: options.cache_root,
        },
    )
    .map_err(error::classify)?;
    Project::open(root)
}

/// Initialize (or migrate) `scaffold.toml` in an existing directory and
/// return a handle to the project. Equivalent to `lgs init`.
pub fn init_project(dir: impl AsRef<Path>, options: InitProjectOptions) -> Result<Project> {
    cmd_init_at(dir.as_ref(), "logos-scaffold", false, options.no_backup)
        .map_err(error::classify)?;
    Project::open(dir.as_ref())
}

/// Handle to a started localnet sequencer.
///
/// Dropping the handle does **not** stop the sequencer — localnet is a
/// long-lived development service. Call [`LocalnetHandle::stop`] for
/// deterministic teardown.
#[derive(Clone, Debug)]
pub struct LocalnetHandle {
    project: crate::model::Project,
    pid: u32,
    reused: bool,
    rpc_url: String,
    state_path: PathBuf,
    log_path: PathBuf,
}

impl LocalnetHandle {
    /// Pid of the sequencer process.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// `true` when an already-running tracked sequencer was reused instead of
    /// spawning a new process.
    pub fn reused(&self) -> bool {
        self.reused
    }

    /// JSON-RPC endpoint of the sequencer (`http://127.0.0.1:<port>`).
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// Path of the localnet state file tracking the sequencer pid.
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// Path of the sequencer log file.
    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// Current status (tracked pid, listener, ownership, readiness).
    pub fn status(&self) -> LocalnetStatusReport {
        build_localnet_status_for_project(&self.project)
    }

    /// Tail of the sequencer log.
    pub fn logs(&self, tail: usize) -> Result<LocalnetLogsReport> {
        localnet_logs_for_project(&self.project, tail).map_err(error::classify)
    }

    /// Stop the sequencer and clean up the state file.
    pub fn stop(&self) -> Result<LocalnetStopOutcome> {
        localnet_stop_for_project(&self.project).map_err(error::classify)
    }
}

/// Every path scaffold derives for a project. Reported as configured/derived;
/// existence is not guaranteed (run [`Project::setup`] to materialise
/// binaries, repos, and circuits).
#[derive(Clone, Debug)]
pub struct ProjectPaths {
    pub root: PathBuf,
    pub scaffold_toml: PathBuf,
    /// Resolved cache root (env override → scaffold.toml → platform default).
    pub cache_root: PathBuf,
    /// Which layer supplied `cache_root` (e.g. `LOGOS_SCAFFOLD_CACHE_ROOT`,
    /// `scaffold.toml [scaffold].cache_root`, `$XDG_CACHE_HOME`).
    pub cache_root_source: String,
    /// Pinned LEZ checkout directory.
    pub lez_repo: PathBuf,
    /// Pinned spel checkout directory.
    pub spel_repo: PathBuf,
    /// Standalone sequencer binary built by `setup`.
    pub sequencer_binary: PathBuf,
    /// Wallet binary built by `setup`.
    pub wallet_binary: PathBuf,
    /// Vendored spel binary built by `setup`.
    pub spel_binary: PathBuf,
    /// Project wallet home directory.
    pub wallet_home: PathBuf,
    /// Localnet state file tracking the sequencer pid.
    pub localnet_state: PathBuf,
    /// Sequencer log file.
    pub sequencer_log: PathBuf,
    /// Circuits release directory (env override or cache location).
    pub circuits_dir: PathBuf,
}

/// Options for [`Project::setup`].
#[derive(Clone, Debug, Default)]
pub struct SetupOptions {
    /// Download prebuilt binaries instead of compiling from source, falling
    /// back to a source build when no prebuilt exists for the pinned commit.
    pub prebuilt: bool,
}

/// Options for [`Project::build`].
#[derive(Clone, Debug, Default)]
pub struct BuildOptions {
    /// See [`SetupOptions::prebuilt`].
    pub prebuilt: bool,
}

/// Options for [`Project::deploy`].
#[derive(Clone, Debug, Default)]
pub struct DeployOptions {
    /// Deploy only this discovered program (default: all).
    pub program: Option<String>,
    /// Deploy a single ELF directly, skipping discovery.
    pub program_path: Option<PathBuf>,
}

/// Options for [`Project::run`].
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Named `[run.profiles.<name>]` profile to use.
    pub profile: Option<String>,
    /// Override the profile's `reset` flag.
    pub reset: Option<bool>,
    /// Override the post-deploy hooks.
    pub post_deploy_override: Option<Vec<String>>,
    /// Seconds to wait for the sequencer when `run` starts localnet itself
    /// (default: 120).
    pub localnet_timeout_sec: Option<u64>,
}

/// Options for [`Project::localnet_start`].
#[derive(Clone, Debug)]
pub struct LocalnetStartOptions {
    /// Seconds to wait for the sequencer to become ready (default: 20).
    pub timeout_sec: u64,
}

impl Default for LocalnetStartOptions {
    fn default() -> Self {
        Self { timeout_sec: 20 }
    }
}

/// Options for [`Project::localnet_reset`].
#[derive(Clone, Debug)]
pub struct LocalnetResetOptions {
    /// Also delete the wallet home and wallet state (irrecoverable).
    pub reset_wallet: bool,
    /// Seconds to wait for post-reset block production (default: 30).
    pub verify_timeout_sec: u64,
}

impl Default for LocalnetResetOptions {
    fn default() -> Self {
        Self {
            reset_wallet: false,
            verify_timeout_sec: 30,
        }
    }
}

/// Options for [`Project::wallet_topup`].
#[derive(Clone, Debug, Default)]
pub struct TopupOptions {
    /// Destination address; `None` uses the project default wallet.
    pub address: Option<String>,
}

/// Options for [`Project::report`].
#[derive(Clone, Debug)]
pub struct ReportOptions {
    /// Output archive path (default: `.scaffold/reports/<timestamp>.tar.gz`).
    pub out: Option<PathBuf>,
    /// How many log lines to include per collected log (default: 500).
    pub tail: usize,
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self {
            out: None,
            tail: 500,
        }
    }
}

/// Result of [`Project::report`]: the written archive and its manifest.
#[derive(Clone, Debug)]
pub struct ReportOutcome {
    pub archive: PathBuf,
    pub manifest: ReportManifest,
}

/// Options for [`init_project`].
#[derive(Clone, Debug, Default)]
pub struct InitProjectOptions {
    /// Skip writing `scaffold.toml.bak` when migrating an existing config.
    pub no_backup: bool,
}

/// Options for [`create_project`].
#[derive(Clone, Debug)]
pub struct CreateProjectOptions {
    /// Project (directory) name.
    pub name: String,
    /// Template name (`default` or `lez-framework`).
    pub template: String,
    /// Vendor pinned repos into the project instead of the shared cache.
    pub vendor_deps: bool,
    /// Use an existing local LEZ checkout instead of cloning.
    pub lez_path: Option<PathBuf>,
    /// Override the cache root recorded in `scaffold.toml`.
    pub cache_root: Option<PathBuf>,
}

impl CreateProjectOptions {
    /// Options for a `default`-template project named `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            template: "default".to_string(),
            vendor_deps: false,
            lez_path: None,
            cache_root: None,
        }
    }
}

/// Basecamp flows, mirroring `lgs basecamp <subcommand>`.
#[derive(Clone, Debug)]
pub enum BasecampCommand {
    /// Build/install the pinned basecamp binary and seed profiles.
    Setup,
    /// Capture project modules (and dependencies) into `scaffold.toml`.
    Modules {
        paths: Vec<PathBuf>,
        flakes: Vec<String>,
        /// Only print the captured state; change nothing.
        show: bool,
    },
    /// Build and install everything captured in state.
    Install,
    /// Launch a seeded profile.
    Launch { profile: String },
    /// Enter a module's Nix dev shell.
    Develop {
        module: String,
        dev_shell: Option<String>,
    },
    /// Attr-swap replay producing portable module builds.
    BuildPortable,
    /// Basecamp-specific health checks.
    Doctor,
}

impl BasecampCommand {
    fn into_action(self) -> BasecampAction {
        match self {
            Self::Setup => BasecampAction::Setup,
            Self::Modules {
                paths,
                flakes,
                show,
            } => BasecampAction::Modules {
                paths,
                flakes,
                show,
            },
            Self::Install => BasecampAction::Install {
                print_output: false,
            },
            // The public API doesn't surface `--log-file`; default to no log
            // (falls back to `[basecamp.profiles.<name>].log_file` if set).
            Self::Launch { profile } => BasecampAction::Launch {
                profile,
                log_file: None,
            },
            Self::Develop { module, dev_shell } => BasecampAction::Develop { module, dev_shell },
            Self::BuildPortable => BasecampAction::BuildPortable,
            Self::Doctor => BasecampAction::Doctor { json: false },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    /// Minimal valid scaffold.toml for fixture projects, mirroring what
    /// `lgs new` writes (schema 0.2.0).
    fn write_fixture_config(root: &Path) {
        fs::write(
            root.join("scaffold.toml"),
            r#"[scaffold]
version = "0.2.0"

[repos.lez]
source = "https://github.com/logos-blockchain/logos-execution-zone.git"
pin = "cf3639d8252040d13b3d4e933feb19b42c76e14a"
build = "cargo"

[repos.spel]
source = "https://github.com/logos-co/spel.git"
pin = "73fc462eb8f0a4d00f1a846437c627ec2e523f83"
build = "cargo"

[wallet]
home_dir = ".scaffold/wallet"

[framework]
kind = "default"
version = "0.1.0"

[localnet]
port = 3040
"#,
        )
        .expect("write scaffold.toml");
    }

    #[test]
    fn open_loads_project_from_explicit_root() {
        let temp = tempdir().expect("tempdir");
        write_fixture_config(temp.path());

        let project = Project::open(temp.path()).expect("open project");
        assert_eq!(project.root(), temp.path());
        assert_eq!(project.localnet_rpc_url(), "http://127.0.0.1:3040");
    }

    #[test]
    fn open_rejects_directory_without_scaffold_toml() {
        let temp = tempdir().expect("tempdir");
        let err = Project::open(temp.path()).expect_err("must fail");
        match err {
            Error::Config { message } => {
                assert!(message.contains("scaffold.toml"), "{message}")
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn discover_walks_up_to_project_root() {
        let temp = tempdir().expect("tempdir");
        write_fixture_config(temp.path());
        let nested = temp.path().join("src/deep/dir");
        fs::create_dir_all(&nested).expect("mkdir nested");

        let project = Project::discover(&nested).expect("discover project");
        assert_eq!(project.root(), temp.path());
    }

    #[test]
    fn paths_reports_project_owned_locations() {
        let temp = tempdir().expect("tempdir");
        write_fixture_config(temp.path());

        let project = Project::open(temp.path()).expect("open project");
        let paths = project.paths().expect("paths");

        assert_eq!(paths.root, temp.path());
        assert_eq!(paths.scaffold_toml, temp.path().join("scaffold.toml"));
        assert_eq!(paths.wallet_home, temp.path().join(".scaffold/wallet"));
        assert_eq!(
            paths.localnet_state,
            temp.path().join(".scaffold/state/localnet.state")
        );
        assert_eq!(
            paths.sequencer_log,
            temp.path().join(".scaffold/logs/sequencer.log")
        );
        assert!(paths
            .sequencer_binary
            .ends_with("target/release/sequencer_service"));
        assert!(paths.wallet_binary.ends_with("target/release/wallet"));
        assert!(paths.spel_binary.ends_with("target/release/spel"));
    }

    #[test]
    fn localnet_status_on_fresh_project_is_stopped() {
        let temp = tempdir().expect("tempdir");
        write_fixture_config(temp.path());

        let project = Project::open(temp.path()).expect("open project");
        let status = project.localnet_status();
        assert!(!status.ready);
        assert!(status.tracked_pid.is_none());
    }

    #[test]
    fn localnet_logs_reports_missing_log_file() {
        let temp = tempdir().expect("tempdir");
        write_fixture_config(temp.path());

        let project = Project::open(temp.path()).expect("open project");
        let logs = project.localnet_logs(50).expect("logs report");
        assert!(!logs.exists);
        assert!(logs.lines.is_empty());
    }
}
