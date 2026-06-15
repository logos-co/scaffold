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

pub use crate::testnode::{
    PortSelection, PreparedTestNode, TestNodeConfig, TestNodeInfo, TestNodeStatus,
    DEFAULT_TEST_NODE_TIMEOUT_SEC,
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

/// Verify (and materialise where possible) the test-node prerequisites for
/// `project`: the standalone sequencer binary and the circuits release.
pub fn prepare(project: &Project) -> Result<PreparedTestNode> {
    crate::testnode::prepare_for_project(&project.inner).map_err(classify)
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
