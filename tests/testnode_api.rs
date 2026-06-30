#![cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(target_os = "macos", target_arch = "aarch64")
))]

#[allow(dead_code)]
mod common;

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use common::test_node::setup_test_node_project;
use logos_scaffold::api::testnode::{
    run_with_test_node_with_block_timing, BlockTimingOverrides, TestNode, TestNodeConfig,
};
use logos_scaffold::api::Project;
use tempfile::tempdir;

const CIRCUITS_ENV: &str = "LOGOS_BLOCKCHAIN_CIRCUITS";
static CIRCUITS_ENV_LOCK: Mutex<()> = Mutex::new(());

struct CircuitsEnvGuard<'a> {
    _lock: MutexGuard<'a, ()>,
    original: Option<OsString>,
}

impl CircuitsEnvGuard<'_> {
    fn set(path: &Path) -> Self {
        let lock = CIRCUITS_ENV_LOCK.lock().expect("lock circuits env");
        let original = std::env::var_os(CIRCUITS_ENV);
        std::env::set_var(CIRCUITS_ENV, path);
        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for CircuitsEnvGuard<'_> {
    fn drop(&mut self) {
        match &self.original {
            Some(value) => std::env::set_var(CIRCUITS_ENV, value),
            None => std::env::remove_var(CIRCUITS_ENV),
        }
    }
}

#[test]
fn api_start_with_block_timing_patches_runtime_config() {
    let temp = tempdir().expect("tempdir");
    let fixtures = setup_test_node_project(temp.path());
    let _circuits_env = CircuitsEnvGuard::set(&fixtures.circuits_path);
    let project = Project::open(temp.path()).expect("open project");
    let work_dir = temp.path().join("node-work");
    let config = TestNodeConfig {
        work_dir: Some(work_dir.clone()),
        timeout_sec: 5,
        ..TestNodeConfig::default()
    };
    let block_timing = BlockTimingOverrides::new()
        .with_block_create_timeout_ms(100)
        .with_retry_pending_blocks_timeout_ms(250);

    let node = TestNode::start_with_block_timing(&project, &config, block_timing)
        .expect("start test node");
    let runtime_config =
        fs::read_to_string(work_dir.join("sequencer_config.json")).expect("read runtime config");
    node.stop().expect("stop test node");
    let runtime_json: serde_json::Value =
        serde_json::from_str(&runtime_config).expect("parse runtime config");

    assert_eq!(
        runtime_json["block_create_timeout"],
        serde_json::json!("100ms")
    );
    assert_eq!(
        runtime_json["retry_pending_blocks_timeout"],
        serde_json::json!("250ms")
    );
}

#[test]
fn api_run_with_block_timing_patches_runtime_config_for_child() {
    let temp = tempdir().expect("tempdir");
    let fixtures = setup_test_node_project(temp.path());
    let _circuits_env = CircuitsEnvGuard::set(&fixtures.circuits_path);
    let project = Project::open(temp.path()).expect("open project");
    let observed_path = temp.path().join("observed-config.json");
    let config = TestNodeConfig {
        timeout_sec: 5,
        ..TestNodeConfig::default()
    };
    let block_timing = BlockTimingOverrides::new()
        .with_block_create_timeout_ms(101)
        .with_retry_pending_blocks_timeout_ms(251);
    let command = vec![
        fixtures.python_path.display().to_string(),
        "-c".to_string(),
        "import json, os, sys\n\
         cfg = json.load(open(os.environ['LGS_TEST_NODE_CONFIG_PATH'], encoding='utf-8'))\n\
         json.dump({\n\
             'block_create_timeout': cfg.get('block_create_timeout'),\n\
             'retry_pending_blocks_timeout': cfg.get('retry_pending_blocks_timeout'),\n\
         }, open(sys.argv[1], 'w', encoding='utf-8'))\n"
            .to_string(),
        observed_path.display().to_string(),
    ];

    let status = run_with_test_node_with_block_timing(&project, &config, block_timing, &command)
        .expect("run command with test node");
    assert!(status.success(), "child command failed: {status}");
    let observed = fs::read_to_string(&observed_path).expect("read observed config");
    let observed_json: serde_json::Value =
        serde_json::from_str(&observed).expect("parse observed config");

    assert_eq!(
        observed_json["block_create_timeout"],
        serde_json::json!("101ms")
    );
    assert_eq!(
        observed_json["retry_pending_blocks_timeout"],
        serde_json::json!("251ms")
    );
}

#[test]
fn api_run_with_block_timing_returns_child_exit_status() {
    let temp = tempdir().expect("tempdir");
    let fixtures = setup_test_node_project(temp.path());
    let _circuits_env = CircuitsEnvGuard::set(&fixtures.circuits_path);
    let project = Project::open(temp.path()).expect("open project");
    let config = TestNodeConfig {
        timeout_sec: 5,
        ..TestNodeConfig::default()
    };
    let block_timing = BlockTimingOverrides::new()
        .with_block_create_timeout_ms(100)
        .with_retry_pending_blocks_timeout_ms(100);
    let command = vec![
        fixtures.python_path.display().to_string(),
        "-c".to_string(),
        "import sys; sys.exit(7)".to_string(),
    ];

    let status = run_with_test_node_with_block_timing(&project, &config, block_timing, &command)
        .expect("run command with test node");

    assert_eq!(status.code(), Some(7));
}
