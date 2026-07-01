use std::path::{Path, PathBuf};

use anyhow::bail;

use crate::DynResult;

/// Return the first existing LEZ-relative path from `rel_paths`.
///
/// LEZ has existed in both a flat repository layout (`sequencer/...`,
/// `wallet/...`) and a nested layout (`lez/sequencer/...`, `lez/wallet/...`).
/// Callers use this helper at the boundary where scaffold consumes files from a
/// checked-out LEZ repo so both layouts stay supported without mutating the LEZ
/// checkout or requiring symlinks.
pub(crate) fn first_existing_lez_path(
    lez: &Path,
    rel_paths: &[&str],
    label: &str,
) -> DynResult<PathBuf> {
    for rel in rel_paths {
        let candidate = lez.join(rel);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    let tried = rel_paths
        .iter()
        .map(|rel| lez.join(rel).display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    bail!("missing {label} in lez repo; tried {tried}");
}
