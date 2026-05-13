//! Persisted run state used by `cmd_run` for deploy idempotence.
//!
//! The state file at `.scaffold/state/run_deploy.json` records the SHA-256
//! of every guest `.bin` (folded together with its IDL JSON and a small
//! deploy-config digest) that was last deployed, plus the sequencer PID
//! that received the deploy. Before re-deploying, `cmd_run` compares the
//! current hashes against the stored ones and skips the deploy step when
//! they match AND the sequencer is the same instance. A `lgs localnet
//! stop && start` cycle changes the PID, which invalidates the cache so
//! the next run actually re-deploys against the empty chain. To force a
//! fresh deploy without restarting localnet, use `lgs run --reset` (which
//! clears this file as a side effect of the wipe) or delete the file
//! manually.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::commands::deploy::{discover_deployable_programs, discover_program_binaries};
use crate::commands::wallet_support::load_wallet_runtime;
use crate::constants::FRAMEWORK_KIND_LEZ_FRAMEWORK;
use crate::model::Project;
use crate::state::read_localnet_state;
use crate::DynResult;

/// Fallback sequencer address when the wallet config doesn't pin one and
/// the wallet runtime isn't loadable yet. Mirrors `cmd_deploy`'s default
/// so the cache key matches the address `cmd_deploy` would actually use.
const DEFAULT_SEQUENCER_ADDR: &str = "http://127.0.0.1:3040";

const RUN_DEPLOY_STATE_REL: &str = ".scaffold/state/run_deploy.json";
const LOCALNET_STATE_REL: &str = ".scaffold/state/localnet.state";

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct RunDeployState {
    pub(crate) program_hashes: BTreeMap<String, String>,
    /// Sequencer PID at deploy time. A stop+start cycle changes the PID
    /// (and wipes on-chain state along with it), so a mismatch here means
    /// the cached deploy is stale even when the binaries haven't changed.
    /// `None` for legacy state files written before this field existed —
    /// treated as unknown and forces a re-deploy.
    #[serde(default)]
    pub(crate) localnet_pid: Option<u32>,
}

/// Read the current sequencer PID from `.scaffold/state/localnet.state`.
/// `None` when the state file is absent or unparseable — callers should
/// treat that as "no localnet identity available", which forces a deploy.
pub(crate) fn current_localnet_pid(project: &Project) -> Option<u32> {
    let path = project.root.join(LOCALNET_STATE_REL);
    read_localnet_state(&path)
        .ok()
        .and_then(|s| s.sequencer_pid)
}

pub(crate) fn compute_program_hashes(project: &Project) -> DynResult<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    // `discover_deployable_programs` returns an empty Vec when the bin dir
    // is missing, so we don't need to pre-check. Surfacing its errors
    // (unreadable dir, etc.) is preferable to silently treating them as
    // "no programs to hash".
    let programs = discover_deployable_programs(&project.root)?;
    if programs.is_empty() {
        return Ok(out);
    }
    let binaries = discover_program_binaries(&project.root, &programs);
    // If a discovered guest source has no compiled `.bin` yet, the cache
    // would otherwise hash only the subset that *is* built and could
    // declare a hit against an old cache that predates the new source.
    // Bail loudly so the user sees the missing-binary error here instead
    // of `cmd_deploy` reporting it later — and so a cache hit can never
    // skip a deploy that would have failed.
    if binaries.len() != programs.len() {
        let missing: Vec<&str> = programs
            .iter()
            .filter(|p| !binaries.contains_key(*p))
            .map(String::as_str)
            .collect();
        anyhow::bail!(
            "guest source(s) without a compiled binary: {}.\n\
             Run `lgs build` to produce the missing `.bin` file(s) before re-running.",
            missing.join(", ")
        );
    }
    let idl_dir = project.root.join(&project.config.framework.idl.path);
    let cfg_digest = config_digest(project);
    let is_lez_framework = project.config.framework.kind == FRAMEWORK_KIND_LEZ_FRAMEWORK;

    for (stem, bin_path) in binaries {
        let mut hasher = Sha256::new();
        let bin_bytes = std::fs::read(&bin_path)
            .with_context(|| format!("read {} for hashing", bin_path.display()))?;
        hasher.update(&bin_bytes);
        // Fold a small canonical digest of deploy-affecting config so the
        // cache invalidates when the user switches sequencer port, wallet
        // home, or IDL path without doing a `--reset`. Without this, a
        // run pointed at a different localnet would silently skip deploy
        // and leave the new target empty.
        hasher.update(b"\x00cfg\x00");
        hasher.update(cfg_digest.as_bytes());
        // Fold the corresponding IDL JSON (if any) into the program's
        // hash so that an ABI-only edit invalidates the cached deploy
        // even when the compiled binary is byte-identical.
        let idl_path = idl_dir.join(format!("{stem}.json"));
        if idl_path.exists() {
            let idl_bytes = std::fs::read(&idl_path)
                .with_context(|| format!("read {} for hashing", idl_path.display()))?;
            hasher.update(b"\x00idl\x00");
            hasher.update(&idl_bytes);
        } else if is_lez_framework {
            // For lez-framework projects, the IDL file is a documented
            // build artifact (`<stem>.json` produced by `build idl`).
            // Missing it would mean we cache a partial digest and silently
            // skip deploys after later ABI-only edits. Bail loudly.
            anyhow::bail!(
                "expected IDL file {} for program `{stem}` is missing; \
                 run `lgs build idl` first or delete {} to bypass the deploy cache",
                idl_path.display(),
                project.root.join(RUN_DEPLOY_STATE_REL).display()
            );
        }
        out.insert(stem, hex_encode(&hasher.finalize()));
    }
    Ok(out)
}

/// Canonical, stable digest of the deploy-affecting bits of scaffold.toml
/// plus the resolved deploy target. Pinned to a small explicit set rather
/// than serializing the whole config so unrelated edits don't bust the
/// cache. The sequencer address comes from the wallet config when
/// available (this is the address `cmd_deploy` actually targets) — that
/// way `wallet_config.json` repointing at a different sequencer
/// invalidates the cache even when `scaffold.toml` is unchanged.
fn config_digest(project: &Project) -> String {
    let cfg = &project.config;
    let sequencer = resolved_sequencer_addr(project);
    format!(
        "port={}|wallet={}|idl={}|sequencer={}",
        cfg.localnet.port, cfg.wallet_home_dir, cfg.framework.idl.path, sequencer
    )
}

/// Resolve the sequencer address `cmd_deploy` would use for this project.
/// Falls back to `DEFAULT_SEQUENCER_ADDR` when the wallet runtime isn't
/// loadable (e.g. on a fresh project before `lgs setup` ran). The
/// fallback matches `cmd_deploy`'s own default, so cache and deploy stay
/// in lockstep.
fn resolved_sequencer_addr(project: &Project) -> String {
    load_wallet_runtime(project)
        .ok()
        .and_then(|w| w.sequencer_addr)
        .unwrap_or_else(|| DEFAULT_SEQUENCER_ADDR.to_string())
}

pub(crate) fn load_state(project: &Project) -> RunDeployState {
    let path = state_path(&project.root);
    let Ok(bytes) = std::fs::read(&path) else {
        return RunDeployState::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(state) => state,
        Err(err) => {
            // Surface cache corruption rather than silently re-deploying. A
            // truncated `run_deploy.json` from a SIGINT mid-write would
            // otherwise force every subsequent run to do a real deploy
            // with no signal that the cache was discarded.
            eprintln!(
                "warning: ignoring malformed deploy cache at {} ({err}); next deploy will re-run",
                path.display()
            );
            RunDeployState::default()
        }
    }
}

pub(crate) fn save_state(project: &Project, state: &RunDeployState) -> DynResult<()> {
    let path = state_path(&project.root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(state).context("serialize run_deploy.json")?;
    // Atomic write: stage to a sibling tempfile, then rename. A SIGINT
    // between write and rename leaves the prior cache intact rather than
    // truncating it. Same parent dir so rename(2) stays atomic across the
    // same filesystem.
    let tmp_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => format!("{n}.tmp"),
        None => return Err(anyhow::anyhow!("invalid state path: {}", path.display())),
    };
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

/// `true` → all known programs hashed, hashes match prior, AND the
/// sequencer PID matches the one that received the prior deploy. A `false`
/// here can mean any of: nothing built, prior cache absent, hash mismatch,
/// or sequencer was restarted (which wiped on-chain state).
pub(crate) fn deploy_can_be_skipped(
    current: &BTreeMap<String, String>,
    current_pid: Option<u32>,
    prior: &RunDeployState,
) -> bool {
    if current.is_empty() || prior.program_hashes.is_empty() {
        return false;
    }
    if current != &prior.program_hashes {
        return false;
    }
    // Both PIDs must be present and equal — `None` on either side means
    // we can't prove the sequencer is the same instance, so re-deploy.
    match (current_pid, prior.localnet_pid) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn state_path(project_root: &Path) -> std::path::PathBuf {
    project_root.join(RUN_DEPLOY_STATE_REL)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(hashes: &[(&str, &str)], pid: Option<u32>) -> RunDeployState {
        RunDeployState {
            program_hashes: hashes
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            localnet_pid: pid,
        }
    }

    #[test]
    fn deploy_skipped_when_hashes_and_pid_match() {
        let current: BTreeMap<String, String> =
            [("hello".to_string(), "deadbeef".to_string())].into();
        let prior = state_with(&[("hello", "deadbeef")], Some(42));
        assert!(deploy_can_be_skipped(&current, Some(42), &prior));
    }

    #[test]
    fn deploy_not_skipped_when_pid_differs() {
        let current: BTreeMap<String, String> =
            [("hello".to_string(), "deadbeef".to_string())].into();
        let prior = state_with(&[("hello", "deadbeef")], Some(42));
        assert!(!deploy_can_be_skipped(&current, Some(99), &prior));
    }

    #[test]
    fn deploy_not_skipped_when_pid_unknown_on_either_side() {
        let current: BTreeMap<String, String> =
            [("hello".to_string(), "deadbeef".to_string())].into();
        let with_pid = state_with(&[("hello", "deadbeef")], Some(42));
        let no_pid = state_with(&[("hello", "deadbeef")], None);
        assert!(!deploy_can_be_skipped(&current, None, &with_pid));
        assert!(!deploy_can_be_skipped(&current, Some(42), &no_pid));
    }

    #[test]
    fn deploy_not_skipped_when_hash_differs() {
        let current: BTreeMap<String, String> =
            [("hello".to_string(), "deadbeef".to_string())].into();
        let prior = state_with(&[("hello", "cafebabe")], Some(42));
        assert!(!deploy_can_be_skipped(&current, Some(42), &prior));
    }

    #[test]
    fn deploy_not_skipped_when_new_program_added() {
        let current: BTreeMap<String, String> = [
            ("hello".to_string(), "h1".to_string()),
            ("counter".to_string(), "h2".to_string()),
        ]
        .into();
        let prior = state_with(&[("hello", "h1")], Some(42));
        assert!(!deploy_can_be_skipped(&current, Some(42), &prior));
    }

    #[test]
    fn deploy_not_skipped_when_either_empty() {
        let empty: BTreeMap<String, String> = BTreeMap::new();
        let nonempty: BTreeMap<String, String> = [("hello".to_string(), "h1".to_string())].into();
        let prior_with = state_with(&[("hello", "h1")], Some(42));
        let prior_empty = state_with(&[], Some(42));
        assert!(!deploy_can_be_skipped(&empty, Some(42), &prior_with));
        assert!(!deploy_can_be_skipped(&nonempty, Some(42), &prior_empty));
    }

    #[test]
    fn hex_encode_is_lowercase_padded() {
        assert_eq!(hex_encode(&[0x0a, 0xff, 0x00]), "0aff00");
    }

    #[test]
    fn save_then_load_round_trips_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());
        let state = state_with(&[("hello", "deadbeef")], Some(1234));
        save_state(&project, &state).expect("save");
        let loaded = load_state(&project);
        assert_eq!(loaded.program_hashes, state.program_hashes);
        assert_eq!(loaded.localnet_pid, state.localnet_pid);
    }

    #[test]
    fn save_atomically_replaces_prior_cache() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());
        save_state(&project, &state_with(&[("a", "h1")], Some(1))).expect("save 1");
        save_state(&project, &state_with(&[("b", "h2")], Some(2))).expect("save 2");
        let loaded = load_state(&project);
        assert_eq!(loaded.localnet_pid, Some(2));
        assert!(loaded.program_hashes.contains_key("b"));
        assert!(!loaded.program_hashes.contains_key("a"));
        // The temp file must not be left behind.
        let tmp = temp.path().join(".scaffold/state/run_deploy.json.tmp");
        assert!(!tmp.exists(), "temp file leaked: {}", tmp.display());
    }

    #[test]
    fn malformed_cache_returns_default_with_warning() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());
        let path = temp.path().join(".scaffold/state/run_deploy.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not valid json").unwrap();
        let loaded = load_state(&project);
        assert!(loaded.program_hashes.is_empty());
        assert_eq!(loaded.localnet_pid, None);
    }

    #[test]
    fn legacy_cache_without_localnet_pid_loads_as_none() {
        // Cache files written before the localnet_pid field existed must
        // still parse — they just force a re-deploy.
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());
        let path = temp.path().join(".scaffold/state/run_deploy.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"program_hashes":{"hello":"deadbeef"}}"#).unwrap();
        let loaded = load_state(&project);
        assert_eq!(loaded.localnet_pid, None);
        assert_eq!(
            loaded.program_hashes.get("hello").map(String::as_str),
            Some("deadbeef")
        );
    }

    fn make_test_project(root: std::path::PathBuf) -> Project {
        use crate::model::{
            Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, RepoRef, RunConfig,
        };
        Project {
            root,
            config: Config {
                version: "0.2.0".to_string(),
                cache_root: ".scaffold/cache".to_string(),
                lez: RepoRef::default(),
                spel: RepoRef::default(),
                basecamp_repo: None,
                lgpm_repo: None,
                wallet_home_dir: ".scaffold/wallet".to_string(),
                framework: FrameworkConfig {
                    kind: "default".to_string(),
                    version: "0.1.0".to_string(),
                    idl: FrameworkIdlConfig {
                        spec: "lssa-idl/0.1.0".to_string(),
                        path: "idl".to_string(),
                    },
                },
                localnet: LocalnetConfig {
                    port: 3040,
                    risc0_dev_mode: true,
                },
                modules: BTreeMap::new(),
                run: RunConfig::default(),
                basecamp: None,
            },
        }
    }

    /// Stage a guest source under `methods/guest/src/bin/<stem>.rs` so
    /// `discover_deployable_programs` will pick it up. Optionally stages a
    /// matching `.bin` under a riscv32im release path so
    /// `discover_program_binaries` finds it; pass `bin = false` to leave
    /// the binary missing.
    fn stage_guest_program(root: &Path, stem: &str, bin: bool) {
        let src = root
            .join("methods/guest/src/bin")
            .join(format!("{stem}.rs"));
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"// fixture\n").unwrap();
        if bin {
            // discover_program_binaries requires a `riscv32im*` segment
            // and prefers paths containing `release`.
            let bin_dir = root.join(format!(
                "target/riscv-guest/riscv32im-risc0-zkvm-elf/release"
            ));
            std::fs::create_dir_all(&bin_dir).unwrap();
            std::fs::write(bin_dir.join(format!("{stem}.bin")), b"\x7fELF...").unwrap();
        }
    }

    #[test]
    fn compute_hashes_bails_when_a_program_is_missing_its_bin() {
        // Two guest sources, only one compiled. Without the missing-binary
        // bail, `compute_program_hashes` would silently return a one-entry
        // map matching whatever the prior cache stored for that single
        // program, and `lgs run` would skip a deploy that `cmd_deploy`
        // would have failed.
        let temp = tempfile::tempdir().expect("tempdir");
        stage_guest_program(temp.path(), "alpha", true);
        stage_guest_program(temp.path(), "beta", false);
        let project = make_test_project(temp.path().to_path_buf());

        let err = compute_program_hashes(&project).expect_err("must bail");
        let msg = format!("{err}");
        assert!(
            msg.contains("beta") && msg.contains("lgs build"),
            "error must name the missing program and the build hint: {msg}"
        );
    }

    #[test]
    fn compute_hashes_succeeds_when_every_program_has_a_bin() {
        let temp = tempfile::tempdir().expect("tempdir");
        stage_guest_program(temp.path(), "alpha", true);
        stage_guest_program(temp.path(), "beta", true);
        let project = make_test_project(temp.path().to_path_buf());

        let hashes = compute_program_hashes(&project).expect("succeeds");
        assert_eq!(hashes.len(), 2);
        assert!(hashes.contains_key("alpha"));
        assert!(hashes.contains_key("beta"));
    }

    #[test]
    fn compute_hashes_includes_sequencer_addr_in_digest() {
        // Two project trees identical apart from `wallet_config.json`'s
        // sequencer_addr: hashes must differ. Without the sequencer in the
        // digest, repointing the wallet at a different sequencer would
        // skip deploy and silently leave the new target empty.
        let temp_a = tempfile::tempdir().expect("tempdir a");
        let temp_b = tempfile::tempdir().expect("tempdir b");
        for root in [temp_a.path(), temp_b.path()] {
            stage_guest_program(root, "alpha", true);
            // Stage a wallet runtime so `load_wallet_runtime` succeeds.
            // It needs the lez wallet binary plus a `wallet_config.json`.
            let lez = root.join("lez");
            std::fs::create_dir_all(lez.join("target/release")).unwrap();
            std::fs::write(lez.join("target/release/wallet"), b"#!/bin/sh\n").unwrap();
            let wallet_home = root.join(".scaffold/wallet");
            std::fs::create_dir_all(&wallet_home).unwrap();
        }
        std::fs::write(
            temp_a.path().join(".scaffold/wallet/wallet_config.json"),
            br#"{"sequencer_addr":"http://127.0.0.1:3040"}"#,
        )
        .unwrap();
        std::fs::write(
            temp_b.path().join(".scaffold/wallet/wallet_config.json"),
            br#"{"sequencer_addr":"http://10.0.0.1:9999"}"#,
        )
        .unwrap();

        // RepoRef::default() leaves both `path` and `pin` empty; without
        // a path `resolve_repo_path` bails and `load_wallet_runtime`
        // falls through. Pin lez to the staged tree so the wallet
        // runtime resolves and the wallet config is actually read.
        let mut project_a = make_test_project(temp_a.path().to_path_buf());
        project_a.config.lez.path = temp_a.path().join("lez").to_string_lossy().to_string();
        let mut project_b = make_test_project(temp_b.path().to_path_buf());
        project_b.config.lez.path = temp_b.path().join("lez").to_string_lossy().to_string();

        let hashes_a = compute_program_hashes(&project_a).expect("a");
        let hashes_b = compute_program_hashes(&project_b).expect("b");
        assert_ne!(
            hashes_a.get("alpha"),
            hashes_b.get("alpha"),
            "wallet sequencer_addr drift must invalidate the cache"
        );
    }
}
