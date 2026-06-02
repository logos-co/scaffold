use std::fmt::Write;
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

pub(crate) fn cmd_setup(prebuilt: bool) -> DynResult<()> {
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

    let built_from_prebuilt = if prebuilt {
        try_download_prebuilt(&lez, &project.config.lez.pin)?
    } else {
        false
    };

    if !built_from_prebuilt {
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
    }

    // wallet is always built from source — prebuilt download only covers sequencer_service
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

pub(crate) fn ensure_default_wallet_seeded(
    project_root: &Path,
    wallet_home: &Path,
) -> DynResult<()> {
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

    #[test]
    fn prebuilt_tag_format() {
        let pin = "35d8df0d031315219f94d1546ceb862b0e5b208f";
        let commit = &pin[..8];
        let tag = format!("lssa-prebuilt-{commit}-x86_64-linux");
        assert_eq!(tag, "lssa-prebuilt-35d8df0d-x86_64-linux");
    }

    #[test]
    fn prebuilt_tag_short_pin() {
        let pin = "abc123";
        let commit = &pin[..8.min(pin.len())];
        assert_eq!(commit, "abc123");
    }

    #[test]
    fn prebuilt_url_contains_tag_and_binary() {
        let tag = "lssa-prebuilt-35d8df0d-x86_64-linux";
        let url = format!(
            "https://github.com/logos-co/logos-scaffold/releases/download/{tag}/sequencer_service"
        );
        assert!(url.contains("lssa-prebuilt-35d8df0d"));
        assert!(url.contains("sequencer_service"));
    }
}

fn try_download_prebuilt(lez: &Path, pin: &str) -> crate::DynResult<bool> {
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        eprintln!(
            "warning: --prebuilt not supported on this architecture, falling back to source build"
        );
        return Ok(false);
    };
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        eprintln!("warning: --prebuilt not supported on this OS, falling back to source build");
        return Ok(false);
    };
    let commit = &pin[..8.min(pin.len())];
    let tag = format!("lssa-prebuilt-{commit}-{arch}-{os}");
    println!("Checking for prebuilt binaries (tag: {tag})...");
    let url = format!(
        "https://github.com/logos-co/logos-scaffold/releases/download/{tag}/sequencer_service"
    );
    let bin_dir = lez.join("target/release");
    std::fs::create_dir_all(&bin_dir)?;
    let dest = bin_dir.join("sequencer_service");
    match ureq::get(&url).call() {
        Ok(resp) => {
            let mut reader = resp.into_reader();
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut reader, &mut bytes)?;

            // Verify SHA256 integrity if a checksum file is published alongside the binary
            let sha_url = format!("{url}.sha256");
            if let Ok(sha_resp) = ureq::get(&sha_url).call() {
                let mut sha_reader = sha_resp.into_reader();
                let mut sha_bytes = Vec::new();
                std::io::Read::read_to_end(&mut sha_reader, &mut sha_bytes)?;
                let expected = String::from_utf8_lossy(&sha_bytes)
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !expected.is_empty() {
                    let actual = sha256_hex(&bytes);
                    if actual != expected {
                        anyhow::bail!(
                            "SHA256 mismatch for prebuilt sequencer_service: expected {expected}, got {actual}"
                        );
                    }
                    println!("SHA256 verified: {actual}");
                }
            }

            std::fs::write(&dest, &bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
            }
            println!("prebuilt sequencer_service downloaded successfully");
            Ok(true)
        }
        Err(ureq::Error::Status(404, _)) => {
            println!("no prebuilt published yet for tag {tag}, falling back to source build");
            Ok(false)
        }
        Err(e) => {
            eprintln!("warning: --prebuilt download failed ({e}), falling back to source build");
            Ok(false)
        }
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(data);
    let mut s = String::new();
    for byte in hash {
        write!(s, "{byte:02x}").unwrap();
    }
    s
}
