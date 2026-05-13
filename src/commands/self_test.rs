//! Hidden CLI entry points used by the integration test suite. Not part of
//! the user-facing surface. Keeps the binary end-to-end verifiable without
//! requiring test harnesses to invoke nix or other heavy external tools.

use std::path::Path;
use std::process::Command;

use crate::process::{run_logged, set_print_output};
use crate::DynResult;

/// Drive `run_logged` against `true` (or `false` when `fail` is set) and exit.
/// Lets CLI integration tests pin the visible output shape of the logged /
/// `--print-output` paths without invoking nix.
pub(crate) fn cmd_self_test_run_logged(
    log_path: &Path,
    step: &str,
    fail: bool,
    print_output: bool,
) -> DynResult<()> {
    if print_output {
        set_print_output(true);
    }
    let binary = if fail { "false" } else { "true" };
    let mut cmd = Command::new(binary);
    run_logged(&mut cmd, step, log_path)
}
