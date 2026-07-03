//! Isolated, short-lived sequencer test nodes.
//!
//! Unlike `localnet` — one long-lived developer sequencer per project, on a
//! fixed port, with state inside the vendored LEZ checkout — a test node is
//! an independent sequencer instance with its own RPC port, patched config,
//! database, log file, and runtime directory. Test nodes are designed for
//! integration tests: repeatable startup, isolated state, dynamic ports,
//! machine-readable output, and clean teardown.
//!
//! Each node lives in a runtime directory (default
//! `<project>/.scaffold/test-nodes/<id>`) containing:
//!
//! - `sequencer_config.json` — patched copy of the pinned LEZ debug config
//!   with `home` pointed at the runtime directory;
//! - `rocksdb/` — the node's own chain database (created by the sequencer,
//!   or pre-seeded from a caller-provided state directory);
//! - `sequencer.log` — captured stdout/stderr;
//! - `node.json` — node metadata used by `status` / `stop` and by handles
//!   reconnecting from another process.

pub mod accounts;
pub mod blocks;
pub mod client;
pub mod pins;
pub mod state;
#[cfg(test)]
pub(crate) mod test_support;

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::circuits::ensure_circuits_for_subprocess;
use crate::commands::localnet::find_r0vm_path_for_lez;
use crate::commands::wallet_support::{rpc_get_last_block_id, RpcReachabilityError};
use crate::constants::SEQUENCER_BIN_REL_PATH;
use crate::error::BlockTimingValidationError;
use crate::model::Project;
use crate::process::{pid_running, port_open, spawn_to_log};
use crate::project::{resolve_cache_root, resolve_repo_path};
use crate::sequencer_config::{apply_common_runtime_overrides, patch_runtime_sequencer_config};
use crate::DynResult;

/// Project-relative directory holding test-node runtime directories.
pub(crate) const TEST_NODES_REL_DIR: &str = ".scaffold/test-nodes";

/// Metadata file written into every node runtime directory.
const NODE_META_FILE: &str = "node.json";

/// Default seconds to wait for a test node to become healthy.
pub const DEFAULT_TEST_NODE_TIMEOUT_SEC: u64 = 60;

/// Minimum accepted block timing override, in milliseconds.
pub const MIN_BLOCK_TIMING_TIMEOUT_MS: u64 = 1;

/// Maximum accepted block timing override, in milliseconds.
pub const MAX_BLOCK_TIMING_TIMEOUT_MS: u64 = 3_600_000;

/// How the RPC port for a test node is chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PortSelection {
    /// Pick an unused localhost port automatically (reported in
    /// [`TestNodeInfo::rpc_url`]).
    #[default]
    Auto,
    /// Bind exactly this port; fails fast if it is already in use.
    Fixed(u16),
}

/// Configuration for starting a test node.
#[derive(Clone, Debug, Default)]
pub struct TestNodeConfig {
    /// Pre-seeded state directory (containing a `rocksdb/` database) to copy
    /// into the node's runtime directory before startup. `None` starts from
    /// the pinned genesis state.
    pub state: Option<PathBuf>,
    /// RPC port selection (default: auto).
    pub port: PortSelection,
    /// Runtime directory override. Default: a fresh directory under
    /// `<project>/.scaffold/test-nodes/`.
    pub work_dir: Option<PathBuf>,
    /// Keep the runtime directory on stop/cleanup instead of deleting it.
    pub preserve_work_dir: bool,
    /// Seconds to wait for the node to become healthy during start
    /// (0 → default of [`DEFAULT_TEST_NODE_TIMEOUT_SEC`]).
    pub timeout_sec: u64,
}

/// Optional sequencer block timing overrides for a test node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct BlockTimingOverrides {
    /// Optional `block_create_timeout` override, in milliseconds.
    /// Must be between [`MIN_BLOCK_TIMING_TIMEOUT_MS`] and
    /// [`MAX_BLOCK_TIMING_TIMEOUT_MS`] when present.
    pub block_create_timeout_ms: Option<u64>,
    /// Optional `retry_pending_blocks_timeout` override, in milliseconds.
    /// Must be between [`MIN_BLOCK_TIMING_TIMEOUT_MS`] and
    /// [`MAX_BLOCK_TIMING_TIMEOUT_MS`] when present.
    pub retry_pending_blocks_timeout_ms: Option<u64>,
}

impl BlockTimingOverrides {
    /// Create an empty override set that preserves all pinned config timing
    /// values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `block_create_timeout`, in milliseconds.
    #[must_use]
    pub fn with_block_create_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.block_create_timeout_ms = Some(timeout_ms);
        self
    }

    /// Set `retry_pending_blocks_timeout`, in milliseconds.
    #[must_use]
    pub fn with_retry_pending_blocks_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.retry_pending_blocks_timeout_ms = Some(timeout_ms);
        self
    }

    /// Validate all present override values against the accepted millisecond range.
    ///
    /// # Errors
    ///
    /// Returns [`BlockTimingValidationError`] when any present value is outside
    /// [`MIN_BLOCK_TIMING_TIMEOUT_MS`]..=[`MAX_BLOCK_TIMING_TIMEOUT_MS`].
    pub fn validate(self) -> Result<(), BlockTimingValidationError> {
        validate_block_timing_timeout_ms("block_create_timeout_ms", self.block_create_timeout_ms)?;
        validate_block_timing_timeout_ms(
            "retry_pending_blocks_timeout_ms",
            self.retry_pending_blocks_timeout_ms,
        )?;
        Ok(())
    }
}

fn validate_block_timing_timeout_ms(
    name: &'static str,
    timeout_ms: Option<u64>,
) -> Result<(), BlockTimingValidationError> {
    if let Some(timeout_ms) = timeout_ms {
        if !(MIN_BLOCK_TIMING_TIMEOUT_MS..=MAX_BLOCK_TIMING_TIMEOUT_MS).contains(&timeout_ms) {
            return Err(BlockTimingValidationError::new(
                name,
                MIN_BLOCK_TIMING_TIMEOUT_MS,
                MAX_BLOCK_TIMING_TIMEOUT_MS,
            ));
        }
    }
    Ok(())
}

/// Static facts about a running (or started) test node.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestNodeInfo {
    /// JSON-RPC endpoint, e.g. `http://127.0.0.1:39041`.
    pub rpc_url: String,
    /// Sequencer process id.
    pub pid: u32,
    /// Runtime directory owning the node's database, config, and logs.
    pub state_dir: PathBuf,
    /// Patched sequencer config the node was started with.
    pub config_path: PathBuf,
    /// Captured stdout/stderr of the sequencer process.
    pub log_path: PathBuf,
    /// Genesis block id from the node's config.
    pub genesis_block_id: u64,
    /// Block height observed when this info was produced.
    pub block_height: u64,
}

/// Live status of a test node.
#[derive(Clone, Debug, Serialize)]
pub struct TestNodeStatus {
    /// `true` when the process is running and the RPC endpoint answers.
    pub healthy: bool,
    /// Whether the sequencer process is running.
    pub running: bool,
    /// JSON-RPC endpoint the node serves.
    pub rpc_url: String,
    pub pid: u32,
    /// Last block id reported over RPC, when reachable.
    pub block_height: Option<u64>,
    pub state_dir: PathBuf,
    pub log_path: PathBuf,
}

/// Persisted node metadata (`node.json`). Superset of [`TestNodeInfo`] —
/// carries the preserve flag and project root so `stop` from another process
/// honors the caller's cleanup intent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct NodeMeta {
    pub(crate) pid: u32,
    pub(crate) port: u16,
    pub(crate) rpc_url: String,
    pub(crate) state_dir: PathBuf,
    pub(crate) config_path: PathBuf,
    pub(crate) log_path: PathBuf,
    pub(crate) genesis_block_id: u64,
    pub(crate) project_root: PathBuf,
    pub(crate) preserve_work_dir: bool,
}

/// A handle that owns a running test-node sequencer process and its runtime
/// directory.
///
/// Dropping the handle stops the process and removes the runtime directory
/// (unless `preserve_work_dir` was requested or [`TestNode::detach`] was
/// called). Prefer explicit [`TestNode::stop`] in tests so teardown errors
/// are observable.
#[derive(Debug)]
pub struct TestNode {
    meta: NodeMeta,
    /// When `true`, `Drop` stops the process and cleans the runtime dir.
    owned: bool,
}

impl TestNode {
    /// Start an isolated sequencer test node for `project`.
    pub fn start(project: &Project, config: &TestNodeConfig) -> DynResult<Self> {
        Self::start_with_block_timing(project, config, BlockTimingOverrides::default())
    }

    /// Start an isolated sequencer test node with optional block timing
    /// overrides in milliseconds.
    pub fn start_with_block_timing(
        project: &Project,
        config: &TestNodeConfig,
        block_timing: BlockTimingOverrides,
    ) -> DynResult<Self> {
        block_timing.validate()?;
        let prepared = verify_prepared(project)?;
        let timeout_sec = if config.timeout_sec == 0 {
            DEFAULT_TEST_NODE_TIMEOUT_SEC
        } else {
            config.timeout_sec
        };

        let work_dir = match &config.work_dir {
            Some(dir) => {
                fs::create_dir_all(dir)
                    .with_context(|| format!("create work dir {}", dir.display()))?;
                ensure_dir_empty_enough(dir)?;
                dir.clone()
            }
            None => allocate_work_dir(&project.root)?,
        };

        let start = StartGuard::new(&work_dir, config.preserve_work_dir);

        // Seed the database before the sequencer first opens it.
        if let Some(state_src) = &config.state {
            seed_state_dir(state_src, &work_dir)?;
        }

        let port = match config.port {
            PortSelection::Auto => pick_unused_port()?,
            PortSelection::Fixed(port) => {
                let addr = format!("127.0.0.1:{port}");
                if port_open(&addr) {
                    bail!(
                        "cannot start test node: port {port} is already in use. \
                         Pass --port 0 to choose a free port automatically."
                    );
                }
                port
            }
        };

        let (config_path, genesis_block_id) =
            write_node_config(&prepared.lez, &work_dir, port, block_timing)?;
        let log_path = work_dir.join("sequencer.log");
        let rpc_url = format!("http://127.0.0.1:{port}");

        let mut cmd = Command::new(&prepared.sequencer_binary);
        cmd.current_dir(&work_dir)
            .arg(&config_path)
            .arg("--port")
            .arg(port.to_string())
            .env("RUST_LOG", "info")
            .env(
                "RISC0_DEV_MODE",
                if project.config.localnet.risc0_dev_mode {
                    "1"
                } else {
                    "0"
                },
            );
        if std::env::var("RISC0_SERVER_PATH").is_err() {
            if let Some(r0vm) = find_r0vm_path_for_lez(&prepared.lez) {
                cmd.env("RISC0_SERVER_PATH", &r0vm);
            }
        }

        let pid = spawn_to_log(&mut cmd, &log_path)?;

        let meta = NodeMeta {
            pid,
            port,
            rpc_url,
            state_dir: work_dir.clone(),
            config_path,
            log_path,
            genesis_block_id,
            project_root: project.root.clone(),
            preserve_work_dir: config.preserve_work_dir,
        };
        write_node_meta(&meta)?;

        let node = Self { meta, owned: true };
        if let Err(err) = node.wait_healthy(Duration::from_secs(timeout_sec)) {
            // Best-effort teardown; the start error is the interesting one.
            let _ = node.kill_process(Duration::from_secs(5));
            drop(start); // removes the work dir unless preservation was asked
            std::mem::forget(node); // teardown already handled above
            return Err(err);
        }

        start.disarm();
        Ok(node)
    }

    /// Reconnect to a node started earlier (possibly by another process) from
    /// its runtime directory. The returned handle is detached: dropping it
    /// does not stop the node.
    pub fn from_state_dir(state_dir: &Path) -> DynResult<Self> {
        let meta = read_node_meta(state_dir)?;
        Ok(Self { meta, owned: false })
    }

    /// Static facts about the node, refreshing the observed block height.
    pub fn info(&self) -> TestNodeInfo {
        let block_height = rpc_get_last_block_id(&self.meta.rpc_url).unwrap_or(0);
        TestNodeInfo {
            rpc_url: self.meta.rpc_url.clone(),
            pid: self.meta.pid,
            state_dir: self.meta.state_dir.clone(),
            config_path: self.meta.config_path.clone(),
            log_path: self.meta.log_path.clone(),
            genesis_block_id: self.meta.genesis_block_id,
            block_height,
        }
    }

    /// The node's JSON-RPC endpoint.
    pub fn rpc_url(&self) -> &str {
        &self.meta.rpc_url
    }

    /// The node's runtime directory.
    pub fn state_dir(&self) -> &Path {
        &self.meta.state_dir
    }

    /// Live status: process running, RPC reachable, current block height.
    pub fn status(&self) -> TestNodeStatus {
        let running = pid_running(self.meta.pid);
        let block_height = rpc_get_last_block_id(&self.meta.rpc_url).ok();
        TestNodeStatus {
            healthy: running && block_height.is_some(),
            running,
            rpc_url: self.meta.rpc_url.clone(),
            pid: self.meta.pid,
            block_height,
            state_dir: self.meta.state_dir.clone(),
            log_path: self.meta.log_path.clone(),
        }
    }

    /// Block until the node answers RPC (or fail with a diagnostic carrying
    /// the log tail).
    pub fn wait_healthy(&self, timeout: Duration) -> DynResult<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if !pid_running(self.meta.pid) {
                bail!(
                    "test node exited before becoming ready (pid={})\nlast logs:\n{}",
                    self.meta.pid,
                    log_tail(&self.meta.log_path, 60)
                );
            }
            match rpc_get_last_block_id(&self.meta.rpc_url) {
                Ok(_) => return Ok(()),
                Err(RpcReachabilityError::Connectivity(_)) => {}
                // The server is up but answered oddly (e.g. still
                // initializing); keep polling until the deadline.
                Err(_) => {}
            }
            if Instant::now() >= deadline {
                bail!(
                    "test node did not become healthy within {}s (pid={}, rpc={})\nlast logs:\n{}",
                    timeout.as_secs(),
                    self.meta.pid,
                    self.meta.rpc_url,
                    log_tail(&self.meta.log_path, 60)
                );
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    /// Stop the node and remove its runtime directory (unless the node was
    /// started with `preserve_work_dir`). Consumes the handle.
    pub fn stop(mut self) -> DynResult<()> {
        self.owned = false; // teardown handled here; Drop must not repeat it
        stop_node_at(&self.meta, None)
    }

    /// Detach the handle: the node keeps running after the handle is dropped.
    pub fn detach(mut self) -> TestNodeInfo {
        self.owned = false;
        self.info()
    }

    fn kill_process(&self, wait: Duration) -> DynResult<()> {
        terminate_pid(self.meta.pid, wait)
    }
}

impl Drop for TestNode {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        let _ = terminate_pid(self.meta.pid, Duration::from_secs(5));
        if !self.meta.preserve_work_dir {
            let _ = fs::remove_dir_all(&self.meta.state_dir);
        }
    }
}

/// Prerequisites a node start needs: the LEZ checkout (for the sequencer
/// config template and r0vm pinning) and the built sequencer binary.
struct StartPrereqs {
    lez: PathBuf,
    sequencer_binary: PathBuf,
}

fn verify_prepared(project: &Project) -> DynResult<StartPrereqs> {
    let lez = resolve_repo_path(project, &project.config.lez, "lez")?;
    let sequencer_binary = lez.join(SEQUENCER_BIN_REL_PATH);
    if !sequencer_binary.exists() {
        bail!(
            "missing standalone sequencer binary at {}.\n\
             Next step: run `lgs test-node prepare` (or `lgs setup`) to build it.",
            sequencer_binary.display()
        );
    }
    let (cache_root, _) = resolve_cache_root(project)?;
    ensure_circuits_for_subprocess(&cache_root)?;
    Ok(StartPrereqs {
        lez,
        sequencer_binary,
    })
}

/// Start a node, export its connection details to `command`'s environment,
/// forward the child's exit status, and stop the node when the child exits.
///
/// Exported environment: `LGS_TEST_NODE_RPC_URL`, `LGS_TEST_NODE_PORT`,
/// `LGS_TEST_NODE_PID`, `LGS_TEST_NODE_STATE_DIR`, `LGS_TEST_NODE_CONFIG_PATH`,
/// `LGS_TEST_NODE_LOG_PATH`, `LGS_TEST_NODE_GENESIS_BLOCK_ID`.
pub fn run_with_test_node(
    project: &Project,
    config: &TestNodeConfig,
    command: &[String],
) -> DynResult<std::process::ExitStatus> {
    run_with_test_node_with_block_timing(project, config, BlockTimingOverrides::default(), command)
}

/// Start a node with optional block timing overrides, export its connection
/// details to `command`'s environment, forward the child's exit status, and
/// stop the node when the child exits.
pub fn run_with_test_node_with_block_timing(
    project: &Project,
    config: &TestNodeConfig,
    block_timing: BlockTimingOverrides,
    command: &[String],
) -> DynResult<std::process::ExitStatus> {
    let Some((program, args)) = command.split_first() else {
        bail!("test-node run requires a command after `--`");
    };

    let node = TestNode::start_with_block_timing(project, config, block_timing)?;
    let info = node.info();

    let status = Command::new(program)
        .args(args)
        .env("LGS_TEST_NODE_RPC_URL", &info.rpc_url)
        .env("LGS_TEST_NODE_PORT", node.meta.port.to_string())
        .env("LGS_TEST_NODE_PID", info.pid.to_string())
        .env("LGS_TEST_NODE_STATE_DIR", &info.state_dir)
        .env("LGS_TEST_NODE_CONFIG_PATH", &info.config_path)
        .env("LGS_TEST_NODE_LOG_PATH", &info.log_path)
        .env(
            "LGS_TEST_NODE_GENESIS_BLOCK_ID",
            info.genesis_block_id.to_string(),
        )
        .status()
        .with_context(|| format!("failed to spawn `{program}`"));

    let stop_result = node.stop();
    let status = status?;
    stop_result?;
    Ok(status)
}

/// Resolve a `--node` selector to a runtime directory: an existing directory
/// path is used as-is; otherwise it is treated as a node id under the
/// project's `.scaffold/test-nodes/`.
pub fn resolve_node_dir(project_root: &Path, selector: &str) -> DynResult<PathBuf> {
    let as_path = PathBuf::from(selector);
    if as_path.is_dir() {
        return Ok(as_path);
    }
    let candidate = project_root.join(TEST_NODES_REL_DIR).join(selector);
    if candidate.is_dir() {
        return Ok(candidate);
    }
    bail!(
        "unknown test node `{selector}`: not a directory and {} does not exist",
        candidate.display()
    )
}

/// Stop the node whose runtime directory is `state_dir`. `preserve_override`
/// forces keeping/removing the directory regardless of what the node was
/// started with.
pub fn stop_node_in_dir(state_dir: &Path, preserve_override: Option<bool>) -> DynResult<()> {
    let meta = read_node_meta(state_dir)?;
    stop_node_at(&meta, preserve_override)
}

fn stop_node_at(meta: &NodeMeta, preserve_override: Option<bool>) -> DynResult<()> {
    terminate_pid(meta.pid, Duration::from_secs(10))?;
    let preserve = preserve_override.unwrap_or(meta.preserve_work_dir);
    if !preserve {
        fs::remove_dir_all(&meta.state_dir).with_context(|| {
            format!(
                "failed to remove test-node runtime dir {}",
                meta.state_dir.display()
            )
        })?;
    }
    Ok(())
}

/// TERM the pid, wait for exit, escalate to KILL after half the deadline.
/// A pid that is already gone is success (idempotent stop).
fn terminate_pid(pid: u32, wait: Duration) -> DynResult<()> {
    if !pid_running(pid) {
        return Ok(());
    }
    let _ = Command::new("kill").arg(pid.to_string()).status();
    let half = wait / 2;
    if wait_for_pid_exit(pid, half) {
        return Ok(());
    }
    let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
    if wait_for_pid_exit(pid, half) {
        return Ok(());
    }
    bail!("test node process {pid} did not exit after TERM and KILL");
}

fn wait_for_pid_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !pid_running(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Bind port 0 to learn a free port, then release it. There is a small race
/// window before the sequencer rebinds; callers get a clear port-in-use error
/// if it is lost.
fn pick_unused_port() -> DynResult<u16> {
    let listener =
        TcpListener::bind("127.0.0.1:0").context("failed to probe for an unused port")?;
    let port = listener
        .local_addr()
        .context("failed to read probed port")?
        .port();
    Ok(port)
}

/// Allocate a fresh runtime directory under `.scaffold/test-nodes/`.
fn allocate_work_dir(project_root: &Path) -> DynResult<PathBuf> {
    let base = project_root.join(TEST_NODES_REL_DIR);
    fs::create_dir_all(&base).with_context(|| format!("create {}", base.display()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let pid = std::process::id();
    for attempt in 0u32..100 {
        let id = format!("tn-{}-{:05}-{attempt:02}", now.as_secs(), pid % 100_000);
        let dir = base.join(&id);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("create node dir {}", dir.display()))
            }
        }
    }
    bail!(
        "could not allocate a unique test-node directory under {}",
        base.display()
    )
}

/// A caller-supplied work dir must not already contain another node's state.
fn ensure_dir_empty_enough(dir: &Path) -> DynResult<()> {
    if dir.join(NODE_META_FILE).exists() {
        bail!(
            "work dir {} already contains a test node (found {}). \
             Stop it first or pass a fresh directory.",
            dir.display(),
            NODE_META_FILE
        );
    }
    Ok(())
}

/// Materialise a caller-provided state into the node work dir before the
/// sequencer first opens it. Accepts:
/// - a directory containing `rocksdb/` (or a bare database directory):
///   copied verbatim — the node resumes from that exact state;
/// - a `state seed`-produced directory containing `seed.json`: the seed is
///   copied and later injected into the node's sequencer config as genesis
///   `initial_public_accounts` / `initial_private_accounts`, so the node
///   starts from exactly those accounts and nothing else.
fn seed_state_dir(state_src: &Path, work_dir: &Path) -> DynResult<()> {
    if !state_src.is_dir() {
        bail!("state directory not found at {}", state_src.display());
    }

    let seed_file = state_src.join(state::SEED_FILE);
    let has_db = state_src.join("rocksdb").is_dir() || state_src.join("CURRENT").exists();

    if has_db {
        let src_db = if state_src.join("rocksdb").is_dir() {
            state_src.join("rocksdb")
        } else {
            state_src.to_path_buf()
        };
        let dest_db = work_dir.join("rocksdb");
        return copy_dir_recursive(&src_db, &dest_db).with_context(|| {
            format!(
                "failed to seed state from {} into {}",
                src_db.display(),
                dest_db.display()
            )
        });
    }

    if seed_file.exists() {
        // Validate now so a malformed seed fails before the node spawns.
        state::load_seed_from_state_dir(state_src)?;
        fs::copy(&seed_file, work_dir.join(state::SEED_FILE))
            .with_context(|| format!("failed to copy seed file from {}", seed_file.display()))?;
        return Ok(());
    }

    bail!(
        "{} is neither a database state directory (rocksdb/) nor a seeded state directory \
         ({}). Produce one with `lgs test-node state seed`.",
        state_src.display(),
        state::SEED_FILE
    )
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

/// Produce the node's sequencer config: the pinned LEZ debug config with
/// `home` pointed at the node's runtime directory, the port recorded, and
/// `max_block_size` widened (same rationale as localnet). Returns the config
/// path and the genesis block id.
fn write_node_config(
    lez: &Path,
    work_dir: &Path,
    port: u16,
    block_timing: BlockTimingOverrides,
) -> DynResult<(PathBuf, u64)> {
    let (dest_path, genesis_block_id) = patch_runtime_sequencer_config(lez, work_dir, |obj| {
        obj.insert(
            "home".to_string(),
            Value::String(work_dir.display().to_string()),
        );
        // Recorded for tooling; the pinned sequencer takes the port via --port.
        apply_common_runtime_overrides(obj, port);
        if let Some(timeout_ms) = block_timing.block_create_timeout_ms {
            obj.insert(
                "block_create_timeout".to_string(),
                Value::String(format!("{timeout_ms}ms")),
            );
        }
        if let Some(timeout_ms) = block_timing.retry_pending_blocks_timeout_ms {
            obj.insert(
                "retry_pending_blocks_timeout".to_string(),
                Value::String(format!("{timeout_ms}ms")),
            );
        }

        // Genesis-config seeding: a seed.json placed in the work dir (by
        // `--state <seeded dir>`) becomes the node's initial accounts. The
        // sequencer builds its genesis state from exactly these values when no
        // database exists — no testnet default accounts are added.
        if let Some(snapshot) = state::load_seed_from_state_dir(work_dir)? {
            let (public, private) = snapshot.to_config_values();
            obj.insert("initial_public_accounts".to_string(), public);
            obj.insert("initial_private_accounts".to_string(), private);
        }

        obj.get("genesis_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("sequencer_config.json has no numeric `genesis_id`"))
    })?;

    Ok((dest_path, genesis_block_id))
}

fn write_node_meta(meta: &NodeMeta) -> DynResult<()> {
    let path = meta.state_dir.join(NODE_META_FILE);
    let text = serde_json::to_string_pretty(meta).context("serialize node metadata")?;
    fs::write(&path, format!("{text}\n"))
        .with_context(|| format!("failed to write {}", path.display()))
}

pub(crate) fn read_node_meta(state_dir: &Path) -> DynResult<NodeMeta> {
    let path = state_dir.join(NODE_META_FILE);
    if !path.exists() {
        bail!(
            "{} is not a test-node runtime directory (missing {})",
            state_dir.display(),
            NODE_META_FILE
        );
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("failed to parse node metadata at {}", path.display()))
}

fn log_tail(log_path: &Path, tail: usize) -> String {
    let Ok(content) = fs::read_to_string(log_path) else {
        return format!("<log file missing: {}>", log_path.display());
    };
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return "<no log output yet>".to_string();
    }
    let start = lines.len().saturating_sub(tail);
    lines[start..].join("\n")
}

/// Removes the work dir on early start failure (unless preservation was
/// requested); disarmed once the node is healthy.
struct StartGuard {
    dir: PathBuf,
    preserve: bool,
    armed: std::cell::Cell<bool>,
}

impl StartGuard {
    fn new(dir: &Path, preserve: bool) -> Self {
        Self {
            dir: dir.to_path_buf(),
            preserve,
            armed: std::cell::Cell::new(true),
        }
    }

    fn disarm(&self) {
        self.armed.set(false);
    }
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        if self.armed.get() && !self.preserve {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

// ─── cross-process run slots (--serial / --parallel) ────────────────────────

/// Guard for one concurrency slot acquired via `acquire_run_slot`. Releases
/// the slot file on drop.
pub struct RunSlot {
    path: PathBuf,
}

impl Drop for RunSlot {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Cap concurrent test-node creation across processes by acquiring one of
/// `max_parallel` slot files under the cache root. `--serial` is
/// `max_parallel == 1`. Slots held by dead processes are reclaimed.
pub fn acquire_run_slot(cache_root: &Path, max_parallel: usize) -> DynResult<RunSlot> {
    let slots_dir = cache_root.join("test-node-slots");
    fs::create_dir_all(&slots_dir)
        .with_context(|| format!("create slot dir {}", slots_dir.display()))?;
    let my_pid = std::process::id().to_string();

    loop {
        for slot in 0..max_parallel {
            let path = slots_dir.join(format!("slot-{slot}.lock"));

            // Reclaim slots whose owning process is gone.
            if let Ok(existing) = fs::read_to_string(&path) {
                if let Ok(owner) = existing.trim().parse::<u32>() {
                    if !pid_running(owner) {
                        let _ = fs::remove_file(&path);
                    }
                } else {
                    let _ = fs::remove_file(&path);
                }
            }

            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    use std::io::Write;
                    let _ = file.write_all(my_pid.as_bytes());
                    return Ok(RunSlot { path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    return Err(err).with_context(|| format!("acquire slot {}", path.display()))
                }
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::constants::{SEQUENCER_CONFIG_NESTED_REL_PATH, SEQUENCER_CONFIG_REL_PATH};
    use tempfile::tempdir;

    use super::*;

    fn runtime_config_path(work: &Path) -> PathBuf {
        work.join(crate::sequencer_config::RUNTIME_SEQUENCER_CONFIG_FILE)
    }

    #[test]
    fn write_node_config_patches_home_port_and_block_size() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            r#"{ "home": ".", "genesis_id": 1, "port": 3040, "block_create_timeout": "2s", "retry_pending_blocks_timeout": "3s" }"#,
        )
        .unwrap();

        let (dest, genesis) =
            write_node_config(&lez, &work, 41234, BlockTimingOverrides::default()).unwrap();
        assert_eq!(genesis, 1);
        assert_eq!(dest, work.join("sequencer_config.json"));

        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
        assert_eq!(doc["home"], serde_json::json!(work.display().to_string()));
        assert_eq!(doc["port"], serde_json::json!(41234));
        assert_eq!(doc["max_block_size"], serde_json::json!("8 MiB"));
        assert_eq!(doc["block_create_timeout"], serde_json::json!("2s"));
        assert_eq!(doc["retry_pending_blocks_timeout"], serde_json::json!("3s"));

        // The vendored config must be untouched.
        let src: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(src["home"], serde_json::json!("."));
        assert_eq!(src["port"], serde_json::json!(3040));
    }

    #[test]
    fn write_node_config_accepts_nested_lez_layout() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();

        // Newer LEZ pins moved the repository payload under a `lez/` prefix.
        // Test-node startup should use the same config probing as localnet.
        let config_path = lez.join(SEQUENCER_CONFIG_NESTED_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            r#"{ "home": ".", "genesis_id": 7, "port": 3040 }"#,
        )
        .unwrap();

        let (dest, genesis) =
            write_node_config(&lez, &work, 41235, BlockTimingOverrides::default()).unwrap();
        assert_eq!(genesis, 7);

        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
        assert_eq!(doc["home"], serde_json::json!(work.display().to_string()));
        assert_eq!(doc["port"], serde_json::json!(41235));
        assert_eq!(doc["max_block_size"], serde_json::json!("8 MiB"));
    }

    #[test]
    fn write_node_config_leaves_absent_block_timing_keys_absent_by_default() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, r#"{ "home": ".", "genesis_id": 1 }"#).unwrap();

        let (dest, genesis) =
            write_node_config(&lez, &work, 41234, BlockTimingOverrides::default()).unwrap();
        assert_eq!(genesis, 1);

        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
        assert!(doc.get("block_create_timeout").is_none());
        assert!(doc.get("retry_pending_blocks_timeout").is_none());
    }

    #[test]
    fn write_node_config_patches_block_timing_overrides_as_milliseconds() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            r#"{ "home": ".", "genesis_id": 1, "block_create_timeout": "2s", "retry_pending_blocks_timeout": "3s" }"#,
        )
        .unwrap();

        let block_timing = BlockTimingOverrides {
            block_create_timeout_ms: Some(100),
            retry_pending_blocks_timeout_ms: Some(250),
        };

        let (dest, genesis) = write_node_config(&lez, &work, 41234, block_timing).unwrap();
        assert_eq!(genesis, 1);

        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
        assert_eq!(doc["block_create_timeout"], serde_json::json!("100ms"));
        assert_eq!(
            doc["retry_pending_blocks_timeout"],
            serde_json::json!("250ms")
        );

        let src: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(src["block_create_timeout"], serde_json::json!("2s"));
        assert_eq!(src["retry_pending_blocks_timeout"], serde_json::json!("3s"));
    }

    #[test]
    fn write_node_config_patches_block_timing_min_max_values_as_milliseconds() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, r#"{ "home": ".", "genesis_id": 1 }"#).unwrap();

        let block_timing = BlockTimingOverrides {
            block_create_timeout_ms: Some(MIN_BLOCK_TIMING_TIMEOUT_MS),
            retry_pending_blocks_timeout_ms: Some(MAX_BLOCK_TIMING_TIMEOUT_MS),
        };

        let (dest, genesis) = write_node_config(&lez, &work, 41234, block_timing).unwrap();
        assert_eq!(genesis, 1);

        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
        assert_eq!(doc["block_create_timeout"], serde_json::json!("1ms"));
        assert_eq!(
            doc["retry_pending_blocks_timeout"],
            serde_json::json!("3600000ms")
        );
    }

    #[test]
    fn write_node_config_replaces_existing_runtime_config_after_success() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, r#"{ "home": ".", "genesis_id": 7 }"#).unwrap();
        fs::write(runtime_config_path(&work), "old config").unwrap();

        let (dest, genesis) =
            write_node_config(&lez, &work, 41234, BlockTimingOverrides::default()).unwrap();
        assert_eq!(genesis, 7);
        assert_eq!(dest, runtime_config_path(&work));

        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
        assert_eq!(doc["genesis_id"], serde_json::json!(7));
    }

    #[test]
    fn block_timing_overrides_reject_out_of_range_values() {
        let zero_err = BlockTimingOverrides {
            block_create_timeout_ms: Some(0),
            retry_pending_blocks_timeout_ms: None,
        }
        .validate()
        .unwrap_err();
        assert!(
            zero_err.to_string().contains("block_create_timeout_ms"),
            "{zero_err}"
        );

        let too_large_err = BlockTimingOverrides {
            block_create_timeout_ms: None,
            retry_pending_blocks_timeout_ms: Some(MAX_BLOCK_TIMING_TIMEOUT_MS + 1),
        }
        .validate()
        .unwrap_err();
        assert!(
            too_large_err
                .to_string()
                .contains("retry_pending_blocks_timeout_ms"),
            "{too_large_err}"
        );
    }

    #[test]
    fn write_node_config_injects_seeded_initial_accounts() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, r#"{ "home": ".", "genesis_id": 1 }"#).unwrap();

        // Place a seed.json as `--state <seeded dir>` would.
        fs::write(
            work.join("seed.json"),
            serde_json::json!({
                "format": "lgs-state-snapshot/1",
                "public_accounts": [
                    { "account_id": "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV", "balance": 42 }
                ],
                "private_accounts": [],
            })
            .to_string(),
        )
        .unwrap();

        let (dest, _) =
            write_node_config(&lez, &work, 41234, BlockTimingOverrides::default()).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
        assert_eq!(
            doc["initial_public_accounts"][0]["account_id"],
            serde_json::json!("6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV")
        );
        assert_eq!(
            doc["initial_public_accounts"][0]["balance"],
            serde_json::json!(42)
        );
        assert_eq!(
            doc["initial_private_accounts"],
            serde_json::json!([]),
            "private accounts key must be present (empty) so the sequencer skips the \
             testnet default state"
        );
    }

    #[test]
    fn write_node_config_requires_genesis_id() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, r#"{ "home": "." }"#).unwrap();

        let err =
            write_node_config(&lez, &work, 41234, BlockTimingOverrides::default()).unwrap_err();
        assert!(err.to_string().contains("genesis_id"), "{err}");
        assert!(
            !runtime_config_path(&work).exists(),
            "runtime config must not be written when genesis_id is invalid"
        );
    }

    #[test]
    fn write_node_config_preserves_existing_runtime_config_on_validation_failure() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, r#"{ "home": "." }"#).unwrap();
        let existing = r#"{"existing":true}"#;
        fs::write(runtime_config_path(&work), existing).unwrap();

        let err =
            write_node_config(&lez, &work, 41234, BlockTimingOverrides::default()).unwrap_err();
        assert!(err.to_string().contains("genesis_id"), "{err}");
        assert_eq!(
            fs::read_to_string(runtime_config_path(&work)).unwrap(),
            existing
        );
    }

    #[test]
    fn write_node_config_rejects_non_numeric_genesis_id_without_writing_runtime_config() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, r#"{ "home": ".", "genesis_id": "1" }"#).unwrap();

        let err =
            write_node_config(&lez, &work, 41234, BlockTimingOverrides::default()).unwrap_err();
        assert!(err.to_string().contains("genesis_id"), "{err}");
        assert!(
            !runtime_config_path(&work).exists(),
            "runtime config must not be written when genesis_id is invalid"
        );
    }

    #[test]
    fn write_node_config_rejects_malformed_json() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, "{ not json").unwrap();

        let err =
            write_node_config(&lez, &work, 41234, BlockTimingOverrides::default()).unwrap_err();
        assert!(
            err.to_string()
                .contains("failed to parse sequencer_config.json"),
            "{err}"
        );
        assert!(
            !runtime_config_path(&work).exists(),
            "runtime config must not be written when source JSON is malformed"
        );
    }

    #[test]
    fn write_node_config_rejects_non_object_json() {
        let temp = tempdir().unwrap();
        let lez = temp.path().join("lez");
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(&config_path, "[]").unwrap();

        let err =
            write_node_config(&lez, &work, 41234, BlockTimingOverrides::default()).unwrap_err();
        assert!(err.to_string().contains("is not a JSON object"), "{err}");
        assert!(
            !runtime_config_path(&work).exists(),
            "runtime config must not be written when source JSON is not an object"
        );
    }

    #[test]
    fn node_meta_round_trips_through_state_dir() {
        let temp = tempdir().unwrap();
        let meta = NodeMeta {
            pid: 4242,
            port: 39000,
            rpc_url: "http://127.0.0.1:39000".to_string(),
            state_dir: temp.path().to_path_buf(),
            config_path: temp.path().join("sequencer_config.json"),
            log_path: temp.path().join("sequencer.log"),
            genesis_block_id: 1,
            project_root: temp.path().join("project"),
            preserve_work_dir: true,
        };
        write_node_meta(&meta).unwrap();

        let loaded = read_node_meta(temp.path()).unwrap();
        assert_eq!(loaded.pid, 4242);
        assert_eq!(loaded.port, 39000);
        assert_eq!(loaded.rpc_url, "http://127.0.0.1:39000");
        assert!(loaded.preserve_work_dir);
    }

    #[test]
    fn read_node_meta_rejects_non_node_dir() {
        let temp = tempdir().unwrap();
        let err = read_node_meta(temp.path()).unwrap_err();
        assert!(err.to_string().contains("node.json"), "{err}");
    }

    #[test]
    fn resolve_node_dir_accepts_path_and_id() {
        let temp = tempdir().unwrap();
        let project_root = temp.path();
        let nodes = project_root.join(TEST_NODES_REL_DIR);
        let node_dir = nodes.join("tn-test-1");
        fs::create_dir_all(&node_dir).unwrap();

        let by_id = resolve_node_dir(project_root, "tn-test-1").unwrap();
        assert_eq!(by_id, node_dir);

        let by_path = resolve_node_dir(project_root, node_dir.to_str().unwrap()).unwrap();
        assert_eq!(by_path, node_dir);

        let err = resolve_node_dir(project_root, "tn-missing").unwrap_err();
        assert!(err.to_string().contains("tn-missing"), "{err}");
    }

    #[test]
    fn allocate_work_dir_creates_unique_dirs() {
        let temp = tempdir().unwrap();
        let a = allocate_work_dir(temp.path()).unwrap();
        let b = allocate_work_dir(temp.path()).unwrap();
        assert_ne!(a, b);
        assert!(a.is_dir());
        assert!(b.is_dir());
        assert!(a.starts_with(temp.path().join(TEST_NODES_REL_DIR)));
    }

    #[test]
    fn seed_state_dir_copies_rocksdb_layouts() {
        let temp = tempdir().unwrap();

        // Layout A: state dir containing rocksdb/
        let src_a = temp.path().join("state-a");
        fs::create_dir_all(src_a.join("rocksdb/sub")).unwrap();
        fs::write(src_a.join("rocksdb/CURRENT"), "manifest").unwrap();
        fs::write(src_a.join("rocksdb/sub/file.sst"), "data").unwrap();
        let work_a = temp.path().join("node-a");
        fs::create_dir_all(&work_a).unwrap();
        seed_state_dir(&src_a, &work_a).unwrap();
        assert_eq!(
            fs::read_to_string(work_a.join("rocksdb/CURRENT")).unwrap(),
            "manifest"
        );
        assert_eq!(
            fs::read_to_string(work_a.join("rocksdb/sub/file.sst")).unwrap(),
            "data"
        );

        // Layout B: bare database directory
        let src_b = temp.path().join("state-b");
        fs::create_dir_all(&src_b).unwrap();
        fs::write(src_b.join("CURRENT"), "manifest-b").unwrap();
        let work_b = temp.path().join("node-b");
        fs::create_dir_all(&work_b).unwrap();
        seed_state_dir(&src_b, &work_b).unwrap();
        assert_eq!(
            fs::read_to_string(work_b.join("rocksdb/CURRENT")).unwrap(),
            "manifest-b"
        );
    }

    #[test]
    fn seed_state_dir_rejects_missing_source() {
        let temp = tempdir().unwrap();
        let work = temp.path().join("node");
        fs::create_dir_all(&work).unwrap();
        let err = seed_state_dir(&temp.path().join("nope"), &work).unwrap_err();
        assert!(err.to_string().contains("state directory"), "{err}");
    }

    #[test]
    fn run_slot_reclaims_stale_and_blocks_live() {
        let temp = tempdir().unwrap();

        // A stale slot (dead pid) must be reclaimed.
        let slots = temp.path().join("test-node-slots");
        fs::create_dir_all(&slots).unwrap();
        fs::write(slots.join("slot-0.lock"), "999999999").unwrap();
        let slot = acquire_run_slot(temp.path(), 1).unwrap();
        let content = fs::read_to_string(slots.join("slot-0.lock")).unwrap();
        assert_eq!(content, std::process::id().to_string());

        // Releasing the guard frees the slot for the next acquire.
        drop(slot);
        assert!(!slots.join("slot-0.lock").exists());
        let _slot2 = acquire_run_slot(temp.path(), 1).unwrap();
    }

    #[test]
    fn ensure_dir_empty_enough_rejects_existing_node() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join(NODE_META_FILE), "{}").unwrap();
        let err = ensure_dir_empty_enough(temp.path()).unwrap_err();
        assert!(err.to_string().contains("already contains"), "{err}");
    }
}
