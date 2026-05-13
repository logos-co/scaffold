use std::path::Path;
use std::process::Command;

use anyhow::bail;

use crate::circuits::ensure_circuits_for_subprocess;
use crate::doctor_checks::check_logos_blockchain_circuits;
use crate::model::{CheckStatus, RepoRef};
use crate::process::run_checked;
use crate::project::{ensure_dir_exists, load_project, resolve_cache_root, resolve_repo_path};
use crate::repo::{sync_repo_to_pin_at_path_with_opts, RepoSyncOptions};
use crate::state::prepare_wallet_home;
use crate::DynResult;

use super::wallet_support::{
    first_public_wallet_address, read_default_wallet_address, wallet_state_path,
    write_default_wallet_address,
};

pub(crate) fn cmd_setup() -> DynResult<()> {
    // Load project first so an outdated scaffold.toml gets the canonical
    // "run `lgs init`" hint before we surface unrelated environment
    // gripes (circuits artifact, etc.). Tests assert the migration hint
    // wins on pre-v0.2.0 configs.
    let project = load_project()?;
    ensure_logos_blockchain_circuits_present()?;
    let lez = resolve_repo_path(&project, &project.config.lez, "lez")?;
    let spel = resolve_repo_path(&project, &project.config.spel, "spel")?;

    // Both the LEZ standalone-sequencer build below and (downstream) the
    // user-project workspace build pull in `logos-blockchain-{pol,poc,poq,zksign}`
    // build scripts, which panic when their circuits release isn't visible.
    // Materialise it once before any cargo invocation; the export propagates
    // to every subprocess for the rest of the process.
    let (cache_root, _) = resolve_cache_root(&project)?;
    ensure_circuits_for_subprocess(&cache_root)?;

    sync_pinned_repo(&project.config.lez, &lez, "lez")?;
    ensure_dir_exists(&lez, "lez")?;

    run_checked(
        Command::new("cargo")
            .current_dir(&lez)
            .arg("build")
            .arg("--release")
            .arg("--features")
            .arg("standalone")
            .arg("-p")
            .arg("sequencer_service"),
        "build sequencer_service (standalone)",
    )?;

    run_checked(
        Command::new("cargo")
            .current_dir(&lez)
            .arg("build")
            .arg("--release")
            .arg("-p")
            .arg("wallet"),
        "build wallet",
    )?;

    sync_pinned_repo(&project.config.spel, &spel, "spel")?;
    ensure_dir_exists(&spel, "spel")?;
    run_checked(
        Command::new("cargo")
            .current_dir(&spel)
            .arg("build")
            .arg("--release")
            .arg("-p")
            .arg("spel"),
        "build spel",
    )?;

    let wallet_home = project.root.join(&project.config.wallet_home_dir);
    prepare_wallet_home(&lez, &wallet_home)?;
    ensure_default_wallet_seeded(&project.root, &wallet_home)?;

    println!("setup complete");

    Ok(())
}

/// Bail before any cargo work if the `logos-blockchain-circuits` artifact
/// the LEZ build chain depends on isn't reachable. Without this the build
/// fails deep inside `logos-blockchain-pol`'s build script with a raw
/// panic backtrace; the user has no signal that the missing piece is a
/// scaffold prerequisite.
fn ensure_logos_blockchain_circuits_present() -> DynResult<()> {
    let row = check_logos_blockchain_circuits();
    if matches!(row.status, CheckStatus::Fail) {
        let remediation = row.remediation.as_deref().unwrap_or("");
        bail!(
            "{}. {} Run `logos-scaffold doctor` for the full prerequisite list.",
            row.detail,
            remediation
        );
    }
    Ok(())
}

/// Sync the cloned repo to its pinned commit at `path`.
///
/// Sync mode is decided by `repo.path`: empty → cache-managed (auto-reclone
/// when origin drifts, since the directory is scaffold-owned); non-empty →
/// vendored or user-overridden, where we refuse to silently rewrite the
/// developer's checkout on origin mismatch.
fn sync_pinned_repo(repo: &RepoRef, path: &Path, label: &str) -> DynResult<()> {
    let opts = if repo.path.is_empty() {
        RepoSyncOptions::auto_reclone_cache_repo()
    } else {
        RepoSyncOptions::fail_on_source_mismatch()
    };
    sync_repo_to_pin_at_path_with_opts(path, &repo.source, &repo.pin, label, opts)
}

fn ensure_default_wallet_seeded(project_root: &Path, wallet_home: &Path) -> DynResult<()> {
    let should_seed = match read_default_wallet_address(project_root) {
        Ok(Some(existing)) => {
            println!("default wallet already configured: {existing}");
            false
        }
        Ok(None) => true,
        Err(err) => {
            println!(
                "warning: wallet default state is malformed; attempting deterministic reseed: {err}"
            );
            true
        }
    };

    if !should_seed {
        return Ok(());
    }

    match first_public_wallet_address(wallet_home) {
        Ok(Some(address)) => {
            let normalized = write_default_wallet_address(project_root, &address)?;
            let state_path = wallet_state_path(project_root);
            println!("default wallet seeded from preconfigured account");
            println!("  Address: {normalized}");
            println!("  State file: {}", state_path.display());
        }
        Ok(None) => {
            println!(
                "warning: could not seed default wallet automatically (no preconfigured public account found)"
            );
        }
        Err(err) => {
            println!("warning: could not seed default wallet automatically: {err}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::wallet_support::WALLET_CONFIG_PRIMARY;
    use std::fs;

    use tempfile::tempdir;

    use super::ensure_default_wallet_seeded;
    use crate::commands::wallet_support::wallet_state_path;

    const PUBLIC_ACCOUNT_ID: &str = "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV";
    const PRIVATE_ACCOUNT_ID: &str = "2ECgkFTaXzwjJBXR7ZKmXYQtpHbvTTHK9Auma4NL9AUo";

    #[test]
    fn ensure_default_wallet_seeded_writes_first_public_account() {
        let temp = tempdir().expect("tempdir");
        let wallet_home = temp.path().join(".scaffold/wallet");
        fs::create_dir_all(&wallet_home).expect("mkdir wallet home");
        fs::write(
            wallet_home.join(WALLET_CONFIG_PRIMARY),
            format!(
                r#"{{
  "initial_accounts": [
    {{ "Private": {{ "account_id": "{PRIVATE_ACCOUNT_ID}" }} }},
    {{ "Public": {{ "account_id": "{PUBLIC_ACCOUNT_ID}" }} }}
  ]
}}"#
            ),
        )
        .expect("write wallet config");

        ensure_default_wallet_seeded(temp.path(), &wallet_home).expect("seed default wallet");

        let state = fs::read_to_string(wallet_state_path(temp.path())).expect("read wallet.state");
        assert_eq!(
            state,
            format!("default_address=Public/{PUBLIC_ACCOUNT_ID}\n")
        );
    }

    #[test]
    fn ensure_default_wallet_seeded_does_not_overwrite_existing_default() {
        let temp = tempdir().expect("tempdir");
        let state_path = wallet_state_path(temp.path());
        fs::create_dir_all(state_path.parent().expect("parent")).expect("mkdir state parent");
        fs::write(
            &state_path,
            "default_address=Public/8zxWNm1qh6FLsJpVBuDxdxcTm55qHPgFEdqJpPVu1fuy\n",
        )
        .expect("write wallet.state");

        let wallet_home = temp.path().join(".scaffold/wallet");
        fs::create_dir_all(&wallet_home).expect("mkdir wallet home");
        fs::write(
            wallet_home.join(WALLET_CONFIG_PRIMARY),
            format!(
                r#"{{
  "initial_accounts": [
    {{ "Public": {{ "account_id": "{PUBLIC_ACCOUNT_ID}" }} }}
  ]
}}"#
            ),
        )
        .expect("write wallet config");

        ensure_default_wallet_seeded(temp.path(), &wallet_home).expect("seed default wallet");

        let state = fs::read_to_string(state_path).expect("read wallet.state");
        assert_eq!(
            state,
            "default_address=Public/8zxWNm1qh6FLsJpVBuDxdxcTm55qHPgFEdqJpPVu1fuy\n"
        );
    }
}
