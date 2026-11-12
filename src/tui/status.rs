//! What the dashboard shows, read in process from the same functions the
//! CLI uses, on a background thread so a slow `lsof` or RPC never stalls
//! the screen.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use crate::commands::deploy::{discover_deployable_programs, discover_project_binaries};
use crate::commands::localnet::{build_localnet_status_for_project, localnet_logs_for_project};
use crate::commands::run_state::{compute_program_hashes, current_localnet_pid, load_state};
use crate::commands::wallet_support::{
    default_sequencer_http_url_for_project, read_default_wallet_address, rpc_get_last_block_id,
};
use crate::model::{LocalnetStatusReport, Project};
use crate::project::load_project_at;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const SEQUENCER_LOG_TAIL: usize = 500;

pub(crate) struct Snapshot {
    pub(crate) localnet: LocalnetStatusReport,
    pub(crate) block: Option<u64>,
    pub(crate) wallet: Option<String>,
    /// `Err` holds why programs could not be listed (e.g. no guest dir).
    pub(crate) programs: Result<Vec<ProgramRow>, String>,
    pub(crate) sequencer_log: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ProgramRow {
    pub(crate) name: String,
    /// `docker`, `release` or `debug`: which build `deploy` would ship.
    /// `None` when the program has no `.bin` yet.
    pub(crate) build: Option<&'static str>,
    /// The last `lgs run` deployed exactly this build to the running localnet.
    pub(crate) deployed: bool,
}

/// Poll the project forever, sending a snapshot per tick. Stops when the
/// receiver is gone.
pub(crate) fn spawn_poller(root: PathBuf, tx: Sender<Snapshot>) {
    thread::spawn(move || loop {
        let Ok(project) = load_project_at(&root) else {
            thread::sleep(POLL_INTERVAL);
            continue;
        };
        if tx.send(collect(&project)).is_err() {
            return;
        }
        thread::sleep(POLL_INTERVAL);
    });
}

fn collect(project: &Project) -> Snapshot {
    let localnet = build_localnet_status_for_project(project);
    let block = localnet
        .listener_present
        .then(|| rpc_get_last_block_id(&default_sequencer_http_url_for_project(project)).ok())
        .flatten();
    Snapshot {
        block,
        wallet: read_default_wallet_address(&project.root).ok().flatten(),
        programs: programs(project).map_err(|err| format!("{err:#}")),
        sequencer_log: localnet_logs_for_project(project, SEQUENCER_LOG_TAIL)
            .map(|report| clean_log(&report.lines))
            .unwrap_or_default(),
        localnet,
    }
}

/// Log lines as a terminal would show them. risc0's tracing layer writes ANSI
/// colour codes into `sequencer.log`; drawn raw, the escape bytes take a cell
/// each in ratatui's buffer but none on the real terminal, which then drifts
/// out of step with what ratatui thinks is on screen (shifted, spliced lines).
fn clean_log(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| super::app::clean_line(line.as_bytes()))
        .collect()
}

fn programs(project: &Project) -> crate::DynResult<Vec<ProgramRow>> {
    let names = discover_deployable_programs(&project.root)?;
    let binaries = discover_project_binaries(project, &names);
    // Same check `lgs run` makes before skipping a deploy: every build hash
    // matches the recorded one and the sequencer is the one deployed to.
    let state = load_state(project);
    let same_sequencer =
        state.localnet_pid.is_some() && state.localnet_pid == current_localnet_pid(project);
    let hashes = compute_program_hashes(project).unwrap_or_default();
    Ok(names
        .into_iter()
        .map(|name| ProgramRow {
            build: binaries.get(&name).map(|path| build_kind(path)),
            deployed: same_sequencer
                && hashes.contains_key(&name)
                && hashes.get(&name) == state.program_hashes.get(&name),
            name,
        })
        .collect())
}

/// Name the discovery rank a binary won, as `lgs build --guest` calls it.
fn build_kind(path: &Path) -> &'static str {
    let names: Vec<_> = path.components().map(|c| c.as_os_str()).collect();
    if names.iter().any(|n| *n == "docker") {
        "docker"
    } else if names.iter().any(|n| *n == "release") {
        "release"
    } else {
        "debug"
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sequencer_log_loses_its_ansi_colour_codes() {
        let raw = "\x1b[2m2026-11-10T03:33:12.873453Z\x1b[0m \x1b[32m INFO\x1b[0m \x1b[2mrisc0_zkvm::host::server::exec::executor\x1b[0m\x1b[2m:\x1b[0m execution time: 9.886085ms";
        assert_eq!(
            super::clean_log(&[raw.to_string()]),
            ["2026-11-10T03:33:12.873453Z  INFO risc0_zkvm::host::server::exec::executor: execution time: 9.886085ms"]
        );
    }

    use super::*;

    #[test]
    fn build_kind_names_the_rank() {
        let kind = |p: &str| build_kind(Path::new(p));
        assert_eq!(
            kind("target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker/hello.bin"),
            "docker"
        );
        assert_eq!(
            kind("target/riscv-guest/m/g/riscv32im-risc0-zkvm-elf/release/hello.bin"),
            "release"
        );
        assert_eq!(
            kind("target/riscv-guest/m/g/riscv32im-risc0-zkvm-elf/debug/hello.bin"),
            "debug"
        );
    }
}
