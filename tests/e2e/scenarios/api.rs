//! `.dogfooding/features/rust-api.md`: the library, called directly.

use std::time::Duration;

use logos_scaffold::api::testnode::{BlockTimingOverrides, TestNode, TestNodeConfig};
use logos_scaffold::api::{Error, Project};

use crate::fixtures;
use crate::harness::{check, pids_named, Ctx, R};
use crate::scenarios::tnode::tn;

/// The library resolves the cache root from the process environment, and
/// echoes the commands it runs. Point it at the suite's cache and keep the
/// report readable while `f` runs.
fn with_env<T>(cx: &Ctx, f: impl FnOnce() -> T) -> T {
    const VARS: [&str; 2] = ["LOGOS_SCAFFOLD_CACHE_ROOT", "LOGOS_SCAFFOLD_QUIET"];
    let saved: Vec<_> = VARS.iter().map(std::env::var_os).collect();
    std::env::set_var(VARS[0], &cx.cache);
    std::env::set_var(VARS[1], "1");
    let result = f();
    for (name, value) in VARS.iter().zip(saved) {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
    result
}

fn api<T>(what: &str, r: Result<T, Error>) -> R<T> {
    match r {
        Ok(v) => Ok(v),
        Err(e) => crate::harness::fail!("{what} failed: {e}"),
    }
}

/// API-001 — Open a project and read its paths and status
pub fn open_and_paths(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_created(cx, false)?;
    let root = p.root.to_string_lossy().into_owned();
    let cli = tn(cx, &["pins", "--project", &root, "--json"])
        .run()
        .json()?;
    let cli_ready = cx.localnet_status(&p)?["ready"].clone();
    with_env(cx, || -> R {
        let project = api("Project::open", Project::open(&p.root))?;
        check!(
            project.root() == p.root,
            "Project::open gave root {}",
            project.root().display()
        );
        let paths = api("Project::paths", project.paths())?;
        check!(
            Some(&*paths.sequencer_binary.to_string_lossy()) == cli["sequencer_binary"].as_str(),
            "API sequencer binary {} differs from the CLI's {}",
            paths.sequencer_binary.display(),
            cli["sequencer_binary"]
        );
        check!(
            paths.cache_root == cx.cache
                && paths
                    .cache_root_source
                    .contains("LOGOS_SCAFFOLD_CACHE_ROOT"),
            "API cache root {} (from {})",
            paths.cache_root.display(),
            paths.cache_root_source
        );
        check!(
            paths.wallet_home == p.path(".scaffold/wallet")
                && paths.scaffold_toml == p.path("scaffold.toml"),
            "API paths are not under the project: {paths:?}"
        );
        check!(
            project.localnet_rpc_url() == format!("http://127.0.0.1:{}", super::localnet::PORT),
            "API localnet URL {}",
            project.localnet_rpc_url()
        );
        check!(
            Some(project.localnet_status().ready) == cli_ready.as_bool(),
            "API readiness differs from `localnet status --json`"
        );
        let found = api("Project::discover", Project::discover(p.path("src/bin")))?;
        check!(
            found.root() == project.root(),
            "discover from a subdirectory found another root"
        );
        let empty = cx.empty_dir("api-empty");
        match Project::discover(&empty) {
            Err(Error::Config { message }) => check!(
                message.contains("no scaffold.toml found in")
                    && message.contains("or any parent directory"),
                "discover's Config message: {message}"
            ),
            other => crate::harness::fail!(
                "discover in an empty dir returned {:?}",
                other.map(|p| p.root().to_path_buf())
            ),
        }
        Ok(())
    })
}

/// API-003 — Errors are categorized
pub fn error_categories(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_setup(cx, false)?;
    with_env(cx, || -> R {
        let empty = cx.empty_dir("api-empty");
        match Project::open(&empty) {
            Err(Error::Config { message }) => {
                check!(
                    message.contains("no scaffold.toml found"),
                    "open's Config message: {message}"
                )
            }
            other => crate::harness::fail!(
                "open of a non-project returned {:?}",
                other.map(|p| p.root().to_path_buf())
            ),
        }
        let project = api("Project::open", Project::open(&p.root))?;
        match project.wallet_passthrough(&["no-such-subcommand".to_string()]) {
            Err(Error::Command(f)) => check!(
                f.command.ends_with("wallet no-such-subcommand")
                    && f.exit_code.is_some_and(|c| c != 0)
                    && f.label == "wallet passthrough command",
                "CommandFailed fields: command={:?} exit={:?} label={:?}",
                f.command,
                f.exit_code,
                f.label
            ),
            other => crate::harness::fail!("a failing wallet command returned {other:?}"),
        }
        let before = pids_named("sequencer_servi");
        match TestNode::start_with_block_timing(
            &project,
            &TestNodeConfig::default(),
            BlockTimingOverrides::new().with_block_create_timeout_ms(0),
        ) {
            Err(Error::BlockTimingValidation { source }) => check!(
                source.field() == "block_create_timeout_ms",
                "BlockTimingValidation names {}",
                source.field()
            ),
            other => {
                crate::harness::fail!("a zero block timing returned {:?}", other.map(|n| n.info()))
            }
        }
        check!(
            pids_named("sequencer_servi") == before,
            "a rejected timing still started a node"
        );
        Ok(())
    })
}

/// API-004 — Start, query and stop a test node from the library
pub fn test_node_from_library(cx: &Ctx) -> R {
    let (p, _) = fixtures::tn_prepared(cx, false)?;
    let before = pids_named("sequencer_servi");
    with_env(cx, || -> R {
        let project = api("Project::open", Project::open(&p.root))?;
        let node = api(
            "TestNode::start",
            TestNode::start(&project, &TestNodeConfig::default()),
        )?;
        let dir = node.state_dir().to_path_buf();
        let url = node.rpc_url().to_string();
        let status = node.status();
        let dir_existed = dir.is_dir();
        let head = node.client().last_block_id();
        let healthy = node.wait_healthy(Duration::from_secs(30));
        let stopped = node.stop();
        check!(
            url.starts_with("http://127.0.0.1:") && url != project.localnet_rpc_url(),
            "the library node's URL is {url}"
        );
        check!(
            status.healthy && status.running && dir_existed,
            "status of a fresh node: {status:?} (runtime dir existed: {dir_existed})"
        );
        check!(
            matches!(head, Ok(h) if h >= 1),
            "client().last_block_id() returned {head:?}"
        );
        api("wait_healthy", healthy)?;
        api("TestNode::stop", stopped)?;
        check!(!dir.exists(), "stop() left the node's runtime dir behind");
        // Dropping a handle cleans up too.
        let dropped = api(
            "TestNode::start",
            TestNode::start(&project, &TestNodeConfig::default()),
        )?;
        let dir = dropped.state_dir().to_path_buf();
        drop(dropped);
        check!(
            !dir.exists(),
            "dropping the handle left the node's runtime dir behind"
        );
        Ok(())
    })?;
    std::thread::sleep(Duration::from_millis(500));
    // A stopped node is this process's child: it stays in the process table
    // as a zombie until this process exits, which `pids_named` leaves out.
    let after = pids_named("sequencer_servi");
    check!(
        after.iter().all(|pid| before.contains(pid)),
        "the library left a sequencer running: before {before:?}, after {after:?}"
    );
    Ok(())
}
