//! Isolated sequencer test nodes for integration tests.
//!
//! A test node is an independent standalone sequencer with its own RPC port,
//! config, database, log file, and runtime directory — designed for
//! repeatable test startup, isolated state, and clean teardown. See the
//! `lgs test-node` command group for the CLI equivalents.
//!
//! ```no_run
//! use logos_scaffold::api::testnode::{TestNode, TestNodeConfig};
//! use logos_scaffold::api::Project;
//! use std::time::Duration;
//!
//! fn main() -> logos_scaffold::api::Result<()> {
//!     let project = Project::open("/path/to/my-app")?;
//!
//!     let node = TestNode::start(&project, &TestNodeConfig::default())?;
//!     node.wait_healthy(Duration::from_secs(30))?;
//!
//!     let info = node.info();
//!     println!("rpc={} state={}", info.rpc_url, info.state_dir.display());
//!
//!     node.stop()?;
//!     Ok(())
//! }
//! ```

use std::path::Path;
use std::time::Duration;

pub use crate::testnode::blocks::{
    clock_account_ids, BlockInfo, BlockRange, ClockAccount, ClockReadMode, ClockSnapshot, TxKind,
    TxSummary,
};
pub use crate::testnode::client::{
    BlockContext, RejectionPhase, RpcError, SubmitOutcome, TestNodeClient, TransactionBytes,
    TransactionOutcome, WaitOptions,
};
pub use crate::testnode::pins::{
    CheckoutOwnership, PinOrigin, PinOverrides, PreparedTestNode, TestNodeCheck,
    TestNodeCheckCategory, TestNodeDoctorReport, TestNodePins,
};
pub use crate::testnode::{
    PortSelection, TestNodeConfig, TestNodeInfo, TestNodeStatus, DEFAULT_TEST_NODE_TIMEOUT_SEC,
};

use super::error::{classify, Result};
use super::Project;

/// Owning handle to a running test-node sequencer.
///
/// The node and its runtime directory are cleaned up when the handle is
/// dropped (unless `preserve_work_dir` was set or [`TestNode::detach`] was
/// called). Prefer explicit [`TestNode::stop`] in tests so teardown errors
/// are observable.
#[derive(Debug)]
pub struct TestNode {
    inner: crate::testnode::TestNode,
}

impl TestNode {
    /// Start an isolated sequencer test node for `project` and wait until it
    /// is healthy.
    pub fn start(project: &Project, config: &TestNodeConfig) -> Result<Self> {
        crate::testnode::TestNode::start(&project.inner, config)
            .map(|inner| Self { inner })
            .map_err(classify)
    }

    /// Reconnect to a node started earlier (possibly by another process)
    /// from its runtime directory. The returned handle is detached: dropping
    /// it does not stop the node.
    pub fn from_state_dir(state_dir: impl AsRef<Path>) -> Result<Self> {
        crate::testnode::TestNode::from_state_dir(state_dir.as_ref())
            .map(|inner| Self { inner })
            .map_err(classify)
    }

    /// Static facts about the node (RPC URL, pid, paths, genesis block id,
    /// current block height).
    pub fn info(&self) -> TestNodeInfo {
        self.inner.info()
    }

    /// The node's JSON-RPC endpoint.
    pub fn rpc_url(&self) -> &str {
        self.inner.rpc_url()
    }

    /// A [`TestNodeClient`] for this node — typed transaction submission and
    /// terminal-outcome observation.
    ///
    /// ```no_run
    /// # use logos_scaffold::api::testnode::{TestNode, TestNodeConfig, TransactionBytes, WaitOptions};
    /// # use logos_scaffold::api::Project;
    /// # fn main() -> logos_scaffold::api::Result<()> {
    /// # let project = Project::open("/path/to/my-app")?;
    /// let node = TestNode::start(&project, &TestNodeConfig::default())?;
    /// let client = node.client();
    /// let tx = TransactionBytes::borsh_base64("…").unwrap();
    /// let outcome = client.submit_and_wait(&tx, &WaitOptions::default());
    /// assert!(outcome.is_committed());
    /// # Ok(())
    /// # }
    /// ```
    pub fn client(&self) -> TestNodeClient {
        TestNodeClient::new(self.inner.rpc_url())
    }

    /// The node's runtime directory.
    pub fn state_dir(&self) -> &Path {
        self.inner.state_dir()
    }

    /// Live status: process running, RPC reachable, current block height.
    pub fn status(&self) -> TestNodeStatus {
        self.inner.status()
    }

    /// Block until the node answers RPC, or fail with the log tail.
    pub fn wait_healthy(&self, timeout: Duration) -> Result<()> {
        self.inner.wait_healthy(timeout).map_err(classify)
    }

    /// Stop the node and remove its runtime directory (unless preservation
    /// was requested). Consumes the handle.
    pub fn stop(self) -> Result<()> {
        self.inner.stop().map_err(classify)
    }

    /// Detach: the node keeps running after the handle is dropped. Returns
    /// the node's info so callers can reconnect later via
    /// [`TestNode::from_state_dir`].
    pub fn detach(self) -> TestNodeInfo {
        self.inner.detach()
    }
}

/// Resolve the LEZ and circuits pins test-node commands will use for
/// `project`: overrides win, then the project's `scaffold.toml`, then
/// scaffold defaults. Read-only — nothing is cloned, built, or downloaded.
pub fn resolve_test_node_pins(project: &Project, overrides: &PinOverrides) -> Result<TestNodePins> {
    crate::testnode::pins::resolve_test_node_pins(&project.inner, overrides).map_err(classify)
}

/// Resolve pins, materialise the LEZ checkout and circuits release, and
/// build the standalone sequencer for those pins. Managed cache checkouts
/// may be cloned/fetched/checked out; caller-provided checkouts are only
/// validated (clean worktree at the requested commit) and never modified.
pub fn prepare_test_node(
    project: &Project,
    overrides: &PinOverrides,
    cache_root: Option<&Path>,
) -> Result<PreparedTestNode> {
    crate::testnode::pins::prepare_test_node(&project.inner, overrides, cache_root)
        .map_err(classify)
}

/// Run the test-node health checks: pin drift, checkout state (missing,
/// dirty, mismatched commit), sequencer binary, circuits release, platform
/// support — each reported as a separate categorized check.
pub fn doctor_test_node(project: &Project) -> Result<TestNodeDoctorReport> {
    crate::testnode::pins::doctor_test_node(&project.inner).map_err(classify)
}

/// Start a node, run `command` with the node's connection details exported
/// (`LGS_TEST_NODE_RPC_URL`, `LGS_TEST_NODE_PORT`, `LGS_TEST_NODE_STATE_DIR`,
/// …), forward the child's exit status, and stop the node when the child
/// exits.
pub fn run_with_test_node(
    project: &Project,
    config: &TestNodeConfig,
    command: &[String],
) -> Result<std::process::ExitStatus> {
    crate::testnode::run_with_test_node(&project.inner, config, command).map_err(classify)
}
