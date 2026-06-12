use std::path::PathBuf;

use anyhow::Context;

use crate::model::Project;
use crate::project::{load_project, load_project_at, resolve_cache_root};
use crate::testnode::{
    acquire_run_slot, prepare_for_project, resolve_node_dir, run_with_test_node, stop_node_in_dir,
    PortSelection, TestNode, TestNodeConfig,
};
use crate::DynResult;

#[derive(Debug, Clone)]
pub(crate) enum TestNodeAction {
    Prepare {
        project: Option<PathBuf>,
        json: bool,
    },
    Start {
        project: Option<PathBuf>,
        state: Option<PathBuf>,
        port: u16,
        work_dir: Option<PathBuf>,
        preserve_work_dir: bool,
        timeout_sec: u64,
        json: bool,
    },
    Status {
        project: Option<PathBuf>,
        node: String,
        json: bool,
    },
    Stop {
        project: Option<PathBuf>,
        node: String,
        preserve_work_dir: bool,
    },
    Run {
        project: Option<PathBuf>,
        state: Option<PathBuf>,
        serial: bool,
        parallel: Option<usize>,
        timeout_sec: u64,
        command: Vec<String>,
    },
}

pub(crate) fn cmd_test_node(action: TestNodeAction) -> DynResult<()> {
    match action {
        TestNodeAction::Prepare { project, json } => {
            let project = load_selected_project(project.as_deref())?;
            let prepared = prepare_for_project(&project)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&prepared)?);
            } else {
                println!("test-node prerequisites ready");
                println!("  lez: {}", prepared.lez.display());
                println!(
                    "  sequencer binary: {}",
                    prepared.sequencer_binary.display()
                );
                println!("  circuits: {}", prepared.circuits_dir.display());
            }
            Ok(())
        }
        TestNodeAction::Start {
            project,
            state,
            port,
            work_dir,
            preserve_work_dir,
            timeout_sec,
            json,
        } => {
            let project = load_selected_project(project.as_deref())?;
            let config = TestNodeConfig {
                state,
                port: if port == 0 {
                    PortSelection::Auto
                } else {
                    PortSelection::Fixed(port)
                },
                work_dir,
                preserve_work_dir,
                timeout_sec,
            };
            // Detach: the CLI exits but the node keeps running until
            // `test-node stop`.
            let node = TestNode::start(&project, &config)?;
            let info = node.detach();
            if json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                let id = info
                    .state_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| info.state_dir.display().to_string());
                println!("test node ready");
                println!("  node: {id}");
                println!("  pid: {}", info.pid);
                println!("  rpc_url: {}", info.rpc_url);
                println!("  state_dir: {}", info.state_dir.display());
                println!("  log: {}", info.log_path.display());
                println!("  genesis_block_id: {}", info.genesis_block_id);
                println!("  block_height: {}", info.block_height);
                println!("Stop with: lgs test-node stop --node {id}");
            }
            Ok(())
        }
        TestNodeAction::Status {
            project,
            node,
            json,
        } => {
            let project_root = selected_project_root(project.as_deref())?;
            let node_dir = resolve_node_dir(&project_root, &node)?;
            let handle = TestNode::from_state_dir(&node_dir)?;
            let status = handle.status();
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("healthy: {}", status.healthy);
                println!("running: {}", status.running);
                println!("rpc_url: {}", status.rpc_url);
                println!("pid: {}", status.pid);
                match status.block_height {
                    Some(height) => println!("block_height: {height}"),
                    None => println!("block_height: unreachable"),
                }
                println!("state_dir: {}", status.state_dir.display());
                println!("log: {}", status.log_path.display());
            }
            if !status.healthy {
                anyhow::bail!("test node is not healthy");
            }
            Ok(())
        }
        TestNodeAction::Stop {
            project,
            node,
            preserve_work_dir,
        } => {
            let project_root = selected_project_root(project.as_deref())?;
            let node_dir = resolve_node_dir(&project_root, &node)?;
            let preserve_override = preserve_work_dir.then_some(true);
            stop_node_in_dir(&node_dir, preserve_override)?;
            println!("test node stopped");
            if preserve_work_dir {
                println!("  state preserved at {}", node_dir.display());
            }
            Ok(())
        }
        TestNodeAction::Run {
            project,
            state,
            serial,
            parallel,
            timeout_sec,
            command,
        } => {
            let project = load_selected_project(project.as_deref())?;

            // --serial caps cross-process node creation at 1; --parallel <N>
            // at N. Held for the whole child run so the node count is the
            // resource being limited, not just startup.
            let max_parallel = if serial { Some(1) } else { parallel };
            let _slot = match max_parallel {
                Some(max) if max == 0 => anyhow::bail!("--parallel must be at least 1"),
                Some(max) => {
                    let (cache_root, _) = resolve_cache_root(&project)?;
                    Some(acquire_run_slot(&cache_root, max)?)
                }
                None => None,
            };

            let config = TestNodeConfig {
                state,
                port: PortSelection::Auto,
                work_dir: None,
                preserve_work_dir: false,
                timeout_sec,
            };
            let status = run_with_test_node(&project, &config, &command)?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
            Ok(())
        }
    }
}

fn load_selected_project(project: Option<&std::path::Path>) -> DynResult<Project> {
    match project {
        Some(root) => load_project_at(root),
        None => load_project(),
    }
}

fn selected_project_root(project: Option<&std::path::Path>) -> DynResult<PathBuf> {
    Ok(load_selected_project(project)
        .context("test-node commands need a project (pass --project <root> or run inside one)")?
        .root)
}
