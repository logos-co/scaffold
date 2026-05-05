//! Persisted run state used by `cmd_run` for deploy idempotence.
//!
//! The state file at `.scaffold/state/run_deploy.json` records the SHA-256
//! of every guest `.bin` (folded together with its IDL JSON) that was
//! last deployed. Before re-deploying, `cmd_run` compares the current
//! hashes against the stored ones and skips the deploy step when they
//! match. To force a fresh deploy, use `lgs run --reset` (which clears
//! this file as a side effect of the wipe) or delete the file manually.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::commands::deploy::{discover_deployable_programs, discover_program_binaries};
use crate::model::Project;
use crate::DynResult;

const RUN_DEPLOY_STATE_REL: &str = ".scaffold/state/run_deploy.json";

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct RunDeployState {
    pub(crate) program_hashes: BTreeMap<String, String>,
}

pub(crate) fn compute_program_hashes(project: &Project) -> DynResult<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let programs_dir = project.root.join("methods/guest/src/bin");
    if !programs_dir.exists() {
        return Ok(out);
    }
    let programs = discover_deployable_programs(&project.root)?;
    let binaries = discover_program_binaries(&project.root, &programs);
    let idl_dir = project.root.join(&project.config.framework.idl.path);
    for (stem, bin_path) in binaries {
        let mut hasher = Sha256::new();
        let bin_bytes = std::fs::read(&bin_path)
            .with_context(|| format!("read {} for hashing", bin_path.display()))?;
        hasher.update(&bin_bytes);
        // Fold the corresponding IDL JSON (if any) into the program's
        // hash so that an ABI-only edit invalidates the cached deploy
        // even when the compiled binary is byte-identical. Missing IDL
        // is hashed as the empty string — consistent across runs for
        // non-lez-framework projects.
        let idl_path = idl_dir.join(format!("{stem}.json"));
        if idl_path.exists() {
            let idl_bytes = std::fs::read(&idl_path)
                .with_context(|| format!("read {} for hashing", idl_path.display()))?;
            hasher.update(b"\x00idl\x00");
            hasher.update(&idl_bytes);
        }
        out.insert(stem, hex_encode(&hasher.finalize()));
    }
    Ok(out)
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
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

/// `Some(true)` → all known programs hashed and match prior; safe to skip.
/// `Some(false)` → at least one differs / new / removed; must deploy.
/// Empty `current` is treated as "must deploy" (nothing built yet).
pub(crate) fn deploy_can_be_skipped(
    current: &BTreeMap<String, String>,
    prior: &BTreeMap<String, String>,
) -> bool {
    if current.is_empty() || prior.is_empty() {
        return false;
    }
    current == prior
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

    #[test]
    fn deploy_skipped_when_hashes_match() {
        let mut a = BTreeMap::new();
        a.insert("hello".to_string(), "deadbeef".to_string());
        let b = a.clone();
        assert!(deploy_can_be_skipped(&a, &b));
    }

    #[test]
    fn deploy_not_skipped_when_hash_differs() {
        let mut a = BTreeMap::new();
        a.insert("hello".to_string(), "deadbeef".to_string());
        let mut b = BTreeMap::new();
        b.insert("hello".to_string(), "cafebabe".to_string());
        assert!(!deploy_can_be_skipped(&a, &b));
    }

    #[test]
    fn deploy_not_skipped_when_new_program_added() {
        let mut a = BTreeMap::new();
        a.insert("hello".to_string(), "h1".to_string());
        a.insert("counter".to_string(), "h2".to_string());
        let mut b = BTreeMap::new();
        b.insert("hello".to_string(), "h1".to_string());
        assert!(!deploy_can_be_skipped(&a, &b));
    }

    #[test]
    fn deploy_not_skipped_when_either_empty() {
        let a: BTreeMap<String, String> = BTreeMap::new();
        let mut b = BTreeMap::new();
        b.insert("hello".to_string(), "h1".to_string());
        assert!(!deploy_can_be_skipped(&a, &b));
        assert!(!deploy_can_be_skipped(&b, &a));
    }

    #[test]
    fn hex_encode_is_lowercase_padded() {
        assert_eq!(hex_encode(&[0x0a, 0xff, 0x00]), "0aff00");
    }
}
