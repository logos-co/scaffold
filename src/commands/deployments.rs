//! `.scaffold/state/deployments.json` — the program ids `deploy` produced.
//!
//! In LEZ 0.3 a program's id is the `program_loader` header account its
//! deployment created, so it cannot be recomputed from the binary the way a
//! risc0 image id can. `deploy` records each successful deployment here
//! (keyed by program name; a re-deploy replaces the entry), `run` reads it to
//! hand `SCAFFOLD_PROGRAM_ID_*` to post-deploy hooks even when the deploy step
//! was skipped, and `localnet reset` deletes it together with the chain the ids
//! pointed into.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::state::write_text;
use crate::DynResult;

const DEPLOYMENTS_FILE: &str = ".scaffold/state/deployments.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DeploymentRecord {
    pub(crate) program: String,
    /// Header account id (base58) — the on-chain program id.
    pub(crate) program_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) elf_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) payer: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DeploymentsFile {
    #[serde(default)]
    programs: BTreeMap<String, DeploymentRecord>,
}

pub(crate) fn deployments_path(project_root: &Path) -> PathBuf {
    project_root.join(DEPLOYMENTS_FILE)
}

/// Every recorded deployment, keyed by program name. A missing or unreadable
/// file reads as empty: the record is a convenience, and a deploy rewrites it.
pub(crate) fn read_deployments(project_root: &Path) -> BTreeMap<String, DeploymentRecord> {
    fs::read_to_string(deployments_path(project_root))
        .ok()
        .and_then(|text| serde_json::from_str::<DeploymentsFile>(&text).ok())
        .map(|file| file.programs)
        .unwrap_or_default()
}

pub(crate) fn record_deployments(
    project_root: &Path,
    records: &[DeploymentRecord],
) -> DynResult<()> {
    let mut programs = read_deployments(project_root);
    for record in records {
        programs.insert(record.program.clone(), record.clone());
    }
    let path = deployments_path(project_root);
    let text = serde_json::to_string_pretty(&DeploymentsFile { programs })
        .context("failed to serialize deployments")?;
    write_text(&path, &(text + "\n")).with_context(|| format!("failed to write {}", path.display()))
}

/// Forget every deployment (the chain they lived on is gone).
pub(crate) fn clear_deployments(project_root: &Path) -> DynResult<()> {
    let path = deployments_path(project_root);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn record(program: &str, id: &str) -> DeploymentRecord {
        DeploymentRecord {
            program: program.to_string(),
            program_id: id.to_string(),
            elf_sha256: Some("ab".repeat(32)),
            payer: None,
        }
    }

    #[test]
    fn records_merge_by_program_and_clear() {
        let tmp = tempdir().expect("tempdir");
        assert!(read_deployments(tmp.path()).is_empty());

        record_deployments(tmp.path(), &[record("a", "id-a1"), record("b", "id-b")])
            .expect("record");
        record_deployments(tmp.path(), &[record("a", "id-a2")]).expect("record again");
        let all = read_deployments(tmp.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all["a"].program_id, "id-a2");
        assert_eq!(all["b"].program_id, "id-b");

        clear_deployments(tmp.path()).expect("clear");
        assert!(read_deployments(tmp.path()).is_empty());
        clear_deployments(tmp.path()).expect("clear is idempotent");
    }
}
