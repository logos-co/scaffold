use std::path::{Path, PathBuf};

use anyhow::bail;

use crate::DynResult;

/// Resolve `rel` (a path inside LEZ 0.3's nested `lez/...` layout) in the
/// pinned checkout, or fail with a message that names the likely cause: a
/// checkout that is not a LEZ 0.3 release.
pub(crate) fn lez_file(lez: &Path, rel: &str, label: &str) -> DynResult<PathBuf> {
    let candidate = lez.join(rel);
    if candidate.exists() {
        return Ok(candidate);
    }
    bail!(
        "missing {label} at {}; is [repos.lez] pinned to a LEZ 0.3 release? \
         Run `logos-scaffold init` to migrate the pin, then `logos-scaffold setup`.",
        candidate.display()
    );
}
