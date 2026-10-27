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
    let mut command = Command::new(&spel_bin);
    command.args(args);
    let wallet_home = project.root.join(&project.config.wallet_home_dir);
    point_spel_at_project_wallet(&mut command, &wallet_home);
    Ok(command.status()?)
}

/// spel loads its wallet from `LEE_WALLET_HOME_DIR` (falling back to
/// `~/.lee/wallet`), so without this every transaction-sending `lgs spel --`
/// call fails with "Storage not found" even though scaffold manages the
/// project wallet. Like every other wallet passthrough (`lgs wallet --`,
/// `deploy`, `run`, `doctor`, `setup`) this overrides whatever the shell
/// exports, so one project never has two commands on two different wallets.
/// It is set even before `lgs setup` has created the directory: failing
/// against the project wallet beats silently writing into `~/.lee/wallet`.
fn point_spel_at_project_wallet(command: &mut Command, wallet_home: &std::path::Path) {
    set_wallet_home_env(command, wallet_home);
}

#[cfg(test)]
mod tests {
    use super::point_spel_at_project_wallet;
    use std::ffi::{OsStr, OsString};
    use std::process::Command;

    fn env_of(command: &Command, name: &str) -> Option<OsString> {
        command
            .get_envs()
            .find(|(k, _)| *k == OsStr::new(name))
            .and_then(|(_, v)| v.map(OsStr::to_os_string))
    }

    /// `Command::env` overrides the inherited value, so an ambient
    /// `NSSA_WALLET_HOME_DIR`/`LEE_WALLET_HOME_DIR` pointing elsewhere (DOGFOODING
    /// D6 tells dogfooders to export both) can no longer leave spel on another
    /// project's wallet — or, with only the legacy name set, on `~/.lee/wallet`.
    #[test]
    fn spel_always_gets_the_project_wallet() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".scaffold/wallet");
        let mut command = Command::new("spel");
        command.env(
            "NSSA_WALLET_HOME_DIR",
            "/some/other/project/.scaffold/wallet",
        );
        point_spel_at_project_wallet(&mut command, &home);
        let expected = home.into_os_string();
        assert_eq!(
            env_of(&command, "LEE_WALLET_HOME_DIR"),
            Some(expected.clone())
        );
        assert_eq!(env_of(&command, "NSSA_WALLET_HOME_DIR"), Some(expected));
    }
}
