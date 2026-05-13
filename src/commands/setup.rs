use std::path::Path;
use std::process::Command;

use crate::model::RepoRef;
use crate::process::run_checked;
use crate::project::{ensure_dir_exists, load_project, resolve_repo_path};
use crate::repo::{sync_repo_to_pin_at_path_with_opts, RepoSyncOptions};
use crate::state::prepare_wallet_home;
use crate::DynResult;

use super::wallet_support::{
    first_public_wallet_address, read_default_wallet_address, wallet_state_path,
    write_default_wallet_address,
};

pub(crate) fn cmd_setup() -> DynResult<()> {
    let project = load_project()?;
    let lez = resolve_repo_path(&project, &project.config.lez, "lez")?;
    let spel = resolve_repo_path(&project, &project.config.spel, "spel")?;

    sync_pinned_repo(&project.config.lez, &lez, "lez")?;
    ensure_dir_exists(&lez, "lez")?;

    // The binary name in [localnet].sequencer_binary doubles as the cargo
    // package name (`-p`) and the produced binary's filename under
    // `target/release/` — see `model::LocalnetConfig::sequencer_binary`.
    let sequencer_binary = &project.config.localnet.sequencer_binary;
    let build_label = format!("build {sequencer_binary} (standalone)");
    run_checked(
        Command::new("cargo")
            .current_dir(&lez)
            .arg("build")
            .arg("--release")
            .arg("--features")
            .arg("standalone")
            .arg("-p")
            .arg(sequencer_binary),
        &build_label,
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
