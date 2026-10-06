use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, bail, Context};

use crate::commands::wallet_support::WALLET_CONFIG_PRIMARY;
use crate::constants::WALLET_CONFIG_REL_PATHS;
use crate::lez_layout::first_existing_lez_path;
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

/// Seed a fresh wallet home from the vendored LEZ debug wallet config, if
/// one doesn't already exist there.
///
/// `sequencer_addr` overrides the vendored config's hardcoded value (the
/// LEZ debug fixture ships `http://127.0.0.1:3040`) with the address this
/// project's sequencer actually binds to, so the wallet follows
/// `[localnet].port` instead of silently pointing at 3040 regardless of
/// what the project is configured to use. Pass
/// `default_sequencer_http_url_for_project(project)`.
pub(crate) fn prepare_wallet_home(
    lez_repo: &Path,
    wallet_home: &Path,
    sequencer_addr: &str,
) -> DynResult<()> {
    fs::create_dir_all(wallet_home)?;
    let cfg_dst = wallet_home.join(WALLET_CONFIG_PRIMARY);
    if !cfg_dst.exists() {
        let cfg_src =
            first_existing_lez_path(lez_repo, WALLET_CONFIG_REL_PATHS, "wallet debug config")?;
        let text = fs::read_to_string(&cfg_src)
            .with_context(|| format!("failed to read {}", cfg_src.display()))?;
        let mut doc: serde_json::Value =
            serde_json::from_str(&text).context("failed to parse wallet debug config")?;
        let Some(obj) = doc.as_object_mut() else {
            bail!(
                "wallet debug config is not a JSON object: {}",
                cfg_src.display()
            );
        };
        obj.insert(
            "sequencer_addr".to_string(),
            serde_json::Value::String(sequencer_addr.to_string()),
        );
        fs::write(&cfg_dst, serde_json::to_string_pretty(&doc)?)?;
    }
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
    fn prepare_wallet_home_accepts_nested_lez_layout() {
        let tmp = tempdir().expect("tempdir");
        let lez = tmp.path().join("lez");
        let wallet_home = tmp.path().join("wallet-home");

        // Newer LEZ pins moved the repository payload under a `lez/` prefix.
        // Setup/reset reseeding should copy the wallet debug config from that
        // layout when the old flat path is absent.
        let cfg_src = lez.join("lez/wallet/configs/debug/wallet_config.json");
        fs::create_dir_all(cfg_src.parent().unwrap()).expect("create nested wallet config dir");
        fs::write(&cfg_src, "{ \"network\": \"debug\" }\n").expect("write wallet config");

        prepare_wallet_home(&lez, &wallet_home, "http://127.0.0.1:5050")
            .expect("prepare wallet home");

        let copied = fs::read_to_string(wallet_home.join(WALLET_CONFIG_PRIMARY))
            .expect("read copied wallet config");
        let copied: serde_json::Value = serde_json::from_str(&copied).expect("parse copied");
        assert_eq!(copied["network"], "debug");
        assert_eq!(copied["sequencer_addr"], "http://127.0.0.1:5050");
    }

    /// Regression for #263: the vendored LEZ debug wallet config hardcodes
    /// `sequencer_addr: http://127.0.0.1:3040`. Without overriding it on
    /// seeding, a project configured for a different `[localnet].port`
    /// would silently get a wallet that talks to the wrong (or, when 3040
    /// is occupied by another project, a nonexistent) sequencer.
    #[test]
    fn prepare_wallet_home_overrides_vendored_sequencer_addr() {
        let tmp = tempdir().expect("tempdir");
        let lez = tmp.path().join("lez");
        let wallet_home = tmp.path().join("wallet-home");

        let cfg_src = lez.join("wallet/configs/debug/wallet_config.json");
        fs::create_dir_all(cfg_src.parent().unwrap()).expect("create wallet config dir");
        fs::write(
            &cfg_src,
            r#"{ "sequencer_addr": "http://127.0.0.1:3040", "seq_poll_timeout": "30s" }"#,
        )
        .expect("write vendored wallet config");

        prepare_wallet_home(&lez, &wallet_home, "http://127.0.0.1:4141")
            .expect("prepare wallet home");

        let copied = fs::read_to_string(wallet_home.join(WALLET_CONFIG_PRIMARY))
            .expect("read copied wallet config");
        let copied: serde_json::Value = serde_json::from_str(&copied).expect("parse copied");
        assert_eq!(
            copied["sequencer_addr"], "http://127.0.0.1:4141",
            "expected the project's configured sequencer address to replace the vendored default, got: {copied}"
        );
        // Unrelated fields must survive untouched.
        assert_eq!(copied["seq_poll_timeout"], "30s");
    }

    /// An already-seeded wallet home (e.g. a project whose wallet was set up
    /// before, and whose localnet port later changed) must not be silently
    /// rewritten — `prepare_wallet_home` only seeds a fresh wallet home.
    #[test]
    fn prepare_wallet_home_does_not_touch_existing_wallet_config() {
        let tmp = tempdir().expect("tempdir");
        let lez = tmp.path().join("lez");
        let wallet_home = tmp.path().join("wallet-home");

        let cfg_src = lez.join("wallet/configs/debug/wallet_config.json");
        fs::create_dir_all(cfg_src.parent().unwrap()).expect("create wallet config dir");
        fs::write(&cfg_src, r#"{ "sequencer_addr": "http://127.0.0.1:3040" }"#)
            .expect("write vendored wallet config");

        fs::create_dir_all(&wallet_home).expect("create wallet home");
        let existing = wallet_home.join(WALLET_CONFIG_PRIMARY);
        fs::write(
            &existing,
            r#"{ "sequencer_addr": "http://127.0.0.1:9999" }"#,
        )
        .expect("write pre-existing wallet config");

        prepare_wallet_home(&lez, &wallet_home, "http://127.0.0.1:4141")
            .expect("prepare wallet home");

        let unchanged = fs::read_to_string(&existing).expect("read wallet config");
        assert_eq!(
            unchanged,
            r#"{ "sequencer_addr": "http://127.0.0.1:9999" }"#
        );
    }
}
