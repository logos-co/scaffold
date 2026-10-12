use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, bail};

use crate::commands::wallet_support::WALLET_CONFIG_PRIMARY;
use crate::constants::WALLET_CONFIG_REL_PATH;
use crate::lez_layout::lez_file;
use crate::model::{BasecampState, LocalnetState};
use crate::DynResult;

pub(crate) fn write_text(path: &Path, text: &str) -> DynResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)?;
    Ok(())
}

/// Atomic replacement of `path`'s contents with `text`. Writes to a
/// sibling temp file in the same directory (so the rename stays on one
/// filesystem) then `rename`s into place. Readers see either the old
/// contents or the new contents — never a partial write.
pub(crate) fn write_text_atomic(path: &Path, text: &str) -> DynResult<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(text.as_bytes())?;
    tmp.as_file_mut().sync_all()?;
    tmp.persist(path)
        .map_err(|e| anyhow!("persist {}: {e}", path.display()))?;
    Ok(())
}

pub(crate) fn write_localnet_state(path: &Path, state: &LocalnetState) -> DynResult<()> {
    let mut content = String::new();
    if let Some(pid) = state.sequencer_pid {
        content.push_str(&format!("sequencer_pid={pid}\n"));
    }
    write_text(path, &content)
}

pub(crate) fn read_localnet_state(path: &Path) -> DynResult<LocalnetState> {
    let mut text = String::new();
    File::open(path)?.read_to_string(&mut text)?;

    let mut state = LocalnetState::default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("sequencer_pid=") {
            let pid: u32 = rest.parse().map_err(|_| anyhow!("invalid sequencer pid"))?;
            state.sequencer_pid = Some(pid);
        }
    }

    Ok(state)
}

pub(crate) fn write_basecamp_state(path: &Path, state: &BasecampState) -> DynResult<()> {
    // The state file is a line-oriented key=value format. A newline or CR embedded
    // in a value would split the record and silently corrupt state on the next read.
    check_state_value("pin", &state.pin)?;
    check_state_value("basecamp_bin", &state.basecamp_bin)?;
    check_state_value("lgpm_bin", &state.lgpm_bin)?;

    // Source lines are no longer part of the state file — the captured module
    // set lives in `[modules.*]` in scaffold.toml (schema v0.2.0). Any
    // residual `project_sources` / `dependencies` values on the struct are
    // intentionally ignored here; the fields are removed in Phase 3.
    let mut content = String::new();
    if !state.pin.is_empty() {
        content.push_str(&format!("pin={}\n", state.pin));
    }
    if !state.basecamp_bin.is_empty() {
        content.push_str(&format!("basecamp_bin={}\n", state.basecamp_bin));
    }
    if !state.lgpm_bin.is_empty() {
        content.push_str(&format!("lgpm_bin={}\n", state.lgpm_bin));
    }
    write_text(path, &content)
}

fn check_state_value(key: &str, value: &str) -> DynResult<()> {
    if value.contains(['\n', '\r']) {
        bail!(
            "basecamp state value for `{key}` contains a newline/CR which would corrupt \
             the line-oriented state file: {value:?}"
        );
    }
    Ok(())
}

pub(crate) fn read_basecamp_state(path: &Path) -> DynResult<BasecampState> {
    let mut text = String::new();
    File::open(path)?.read_to_string(&mut text)?;

    let mut state = BasecampState::default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("pin=") {
            state.pin = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("basecamp_bin=") {
            state.basecamp_bin = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("lgpm_bin=") {
            state.lgpm_bin = rest.to_string();
        }
        // Any other key (legacy `project:*`, `dep:*`, `source:*` lines from
        // in-PR iterations) is silently ignored. The captured module set is
        // now sourced from scaffold.toml's `[modules.*]` section.
    }

    Ok(state)
}

/// Create the project wallet home and point its config at the project's
/// localnet. The config is seeded from the pinned LEZ debug wallet config on
/// first use; on every call `sequencer.sequencer_addr` is rewritten to
/// `http://127.0.0.1:<port>` so a changed `[localnet].port` reaches the
/// wallet the next time `setup` (or `localnet reset`) runs, and the
/// confirmation polling is tuned for a local chain: the wallet sleeps
/// `seq_poll_timeout` between inclusion checks (30s in LEZ's debug config,
/// i.e. ~30s per transaction), so scaffold polls every second for up to two
/// minutes instead. Other keys the user or the wallet added are preserved.
/// Wallet confirmation polling for scaffold-managed wallets (see
/// [`prepare_wallet_home`]).
const WALLET_POLL_INTERVAL: &str = "1s";
const WALLET_POLL_ATTEMPTS: u64 = 120;

pub(crate) fn prepare_wallet_home(lez_repo: &Path, wallet_home: &Path, port: u16) -> DynResult<()> {
    fs::create_dir_all(wallet_home)?;
    let cfg_dst = wallet_home.join(WALLET_CONFIG_PRIMARY);
    if !cfg_dst.exists() {
        let cfg_src = lez_file(lez_repo, WALLET_CONFIG_REL_PATH, "wallet debug config")?;
        fs::copy(cfg_src, &cfg_dst)?;
    }
    let text = fs::read_to_string(&cfg_dst)?;
    let mut config: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
        anyhow::anyhow!(
            "failed to parse wallet config at {}: {err}",
            cfg_dst.display()
        )
    })?;
    let root = config.as_object_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "wallet config at {} is not a JSON object",
            cfg_dst.display()
        )
    })?;
    let sequencer = root
        .entry("sequencer")
        .or_insert_with(|| serde_json::json!({}));
    let sequencer = sequencer.as_object_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "wallet config at {}: `sequencer` is not a JSON object",
            cfg_dst.display()
        )
    })?;
    sequencer.insert(
        "sequencer_addr".to_string(),
        serde_json::Value::String(format!("http://127.0.0.1:{port}")),
    );
    root.insert(
        "seq_poll_timeout".to_string(),
        serde_json::Value::String(WALLET_POLL_INTERVAL.to_string()),
    );
    root.insert(
        "seq_tx_poll_max_blocks".to_string(),
        serde_json::Value::from(WALLET_POLL_ATTEMPTS),
    );
    write_text(&cfg_dst, &(serde_json::to_string_pretty(&config)? + "\n"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn basecamp_state_roundtrips_pin_artifacts() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("basecamp.state");

        let state = BasecampState {
            pin: "deadbeef".to_string(),
            basecamp_bin: "/nix/store/abc/bin/basecamp".to_string(),
            lgpm_bin: "/nix/store/def/bin/lgpm".to_string(),
        };

        write_basecamp_state(&path, &state).expect("write");
        let loaded = read_basecamp_state(&path).expect("read");

        assert_eq!(loaded.pin, state.pin);
        assert_eq!(loaded.basecamp_bin, state.basecamp_bin);
        assert_eq!(loaded.lgpm_bin, state.lgpm_bin);
    }

    #[test]
    fn basecamp_state_empty_writes_expected_minimum() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("basecamp.state");

        let state = BasecampState {
            pin: "sha1".to_string(),
            basecamp_bin: String::new(),
            lgpm_bin: String::new(),
        };

        write_basecamp_state(&path, &state).expect("write");
        let content = fs::read_to_string(&path).expect("read raw");
        assert_eq!(content, "pin=sha1\n");

        let loaded = read_basecamp_state(&path).expect("read");
        assert_eq!(loaded.pin, "sha1");
    }

    #[test]
    fn basecamp_state_reader_ignores_legacy_source_lines() {
        // State files written by earlier in-PR iterations carried
        // `project:flake=` / `dep:flake=` / `source:flake=` lines. Reader
        // must tolerate (ignore) them rather than error out, so an in-flight
        // working copy upgrading past this commit doesn't see a crash.
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("basecamp.state");
        fs::write(
            &path,
            "pin=abc\nproject:flake=path:/p#lgx\ndep:flake=github:x/y/z#lgx\nsource:path=/m.lgx\n",
        )
        .unwrap();
        let loaded = read_basecamp_state(&path).expect("read legacy");
        assert_eq!(loaded.pin, "abc");
    }

    #[test]
    fn prepare_wallet_home_seeds_config_and_points_it_at_the_localnet_port() {
        let tmp = tempdir().expect("tempdir");
        let lez = tmp.path().join("lez");
        let wallet_home = tmp.path().join("wallet-home");

        let cfg_src = lez.join("lez/wallet/configs/debug/wallet_config.json");
        fs::create_dir_all(cfg_src.parent().unwrap()).expect("create wallet config dir");
        fs::write(
            &cfg_src,
            "{ \"sequencer\": { \"sequencer_addr\": \"http://127.0.0.1:3040\" }, \"seq_poll_timeout\": \"30s\" }\n",
        )
        .expect("write wallet config");

        prepare_wallet_home(&lez, &wallet_home, 3055).expect("prepare wallet home");
        let copied: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(wallet_home.join(WALLET_CONFIG_PRIMARY)).expect("read"),
        )
        .expect("json");
        assert_eq!(
            copied["sequencer"]["sequencer_addr"],
            "http://127.0.0.1:3055"
        );
        assert_eq!(copied["seq_poll_timeout"], "1s");
        assert_eq!(copied["seq_tx_poll_max_blocks"], 120);

        // A later port change is applied to the existing config.
        prepare_wallet_home(&lez, &wallet_home, 3077).expect("prepare again");
        let updated: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(wallet_home.join(WALLET_CONFIG_PRIMARY)).expect("read"),
        )
        .expect("json");
        assert_eq!(
            updated["sequencer"]["sequencer_addr"],
            "http://127.0.0.1:3077"
        );
    }

    #[test]
    fn prepare_wallet_home_names_the_pin_when_the_checkout_is_not_lez_0_3() {
        let tmp = tempdir().expect("tempdir");
        let lez = tmp.path().join("lez");
        // Pre-0.2 flat layout only.
        let flat = lez.join("wallet/configs/debug/wallet_config.json");
        fs::create_dir_all(flat.parent().unwrap()).expect("mkdir");
        fs::write(&flat, "{}\n").expect("write");
        let err = prepare_wallet_home(&lez, &tmp.path().join("w"), 3040).unwrap_err();
        assert!(err.to_string().contains("LEZ 0.3"), "{err}");
    }
}
