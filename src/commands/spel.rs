use std::process::Command;

use anyhow::bail;

use crate::commands::wallet_support::set_wallet_home_env;
use crate::constants::SPEL_BIN_REL_PATH;
use crate::project::{load_project, resolve_repo_path};
use crate::DynResult;

/// Proxy `lgs spel -- <args...>` to the project-vendored `spel` binary so any
/// spel subcommand (`inspect`, `pda`, `generate-idl`, …) runs against the
/// project's pinned version. Mirrors the existing `wallet --` passthrough.
/// The vendored binary is built by `cmd_setup`; if it isn't present, point the
/// user at `setup` rather than failing with a raw exec error.
pub(crate) fn cmd_spel(args: Vec<String>) -> DynResult<()> {
    let project = load_project()?;
    let status = spel_passthrough_for_project(&project, &args)?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Run the project-vendored `spel` binary with `args`, forwarding its output,
/// and return the exit status. The API path uses the returned status instead
/// of exiting the process.
pub(crate) fn spel_passthrough_for_project(
    project: &crate::model::Project,
    args: &[String],
) -> DynResult<std::process::ExitStatus> {
    let spel_bin =
        resolve_repo_path(project, &project.config.spel, "spel")?.join(SPEL_BIN_REL_PATH);
    if !spel_bin.exists() {
        bail!(
            "vendored spel binary not found at `{}`\nNext step: run `logos-scaffold setup` to build it.",
            spel_bin.display()
        );
    }
    Ok(spel_command(project, &spel_bin, args).status()?)
}

/// spel's IDL-driven instruction commands submit transactions through the
/// LEZ wallet, which reads its home from the environment. Without the
/// project's wallet home, `lgs spel -- --idl … <instruction>` signed against
/// `~/.nssa/wallet` (or `~/.lee/wallet`) and failed with `Failed to read
/// persistent storage`, even though `lgs wallet …` worked in the same project.
fn spel_command(
    project: &crate::model::Project,
    spel_bin: &std::path::Path,
    args: &[String],
) -> Command {
    let mut cmd = Command::new(spel_bin);
    cmd.args(args);
    set_wallet_home_env(&mut cmd, project.root.join(&project.config.wallet_home_dir));
    cmd
}
