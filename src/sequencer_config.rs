use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context};
use serde_json::{Map, Value};

use crate::constants::SEQUENCER_CONFIG_REL_PATHS;
use crate::lez_layout::first_existing_lez_path;
use crate::DynResult;

pub(crate) const RUNTIME_SEQUENCER_CONFIG_FILE: &str = "sequencer_config.json";
pub(crate) const WIDENED_MAX_BLOCK_SIZE: &str = "8 MiB";

pub(crate) fn apply_common_runtime_overrides(obj: &mut Map<String, Value>, port: u16) {
    obj.insert("port".to_string(), Value::Number(port.into()));
    obj.insert(
        "max_block_size".to_string(),
        Value::String(WIDENED_MAX_BLOCK_SIZE.to_string()),
    );
}

/// CLI args that keep a developer sequencer's RPC on loopback.
///
/// The LEZ sequencer binds `0.0.0.0` unless told otherwise, and its own `--help` warns that
/// the RPC has no caller auth (anyone who can reach the port can submit transactions).
/// Localnet and test nodes are only ever reached through `127.0.0.1`, so ask for loopback.
/// Older pinned LEZ revisions have no `--listen-address`; passing it there would make the
/// sequencer exit on an unknown argument, so only pass it when `--help` advertises it.
pub(crate) fn loopback_listen_args(sequencer_bin: &Path) -> Vec<&'static str> {
    let help = Command::new(sequencer_bin)
        .arg("--help")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match help {
        Ok(out) if String::from_utf8_lossy(&out.stdout).contains("--listen-address") => {
            vec!["--listen-address", "127.0.0.1"]
        }
        _ => Vec::new(),
    }
}

pub(crate) fn patch_runtime_sequencer_config<F, R>(
    lez: &Path,
    dest_dir: &Path,
    patch: F,
) -> DynResult<(PathBuf, R)>
where
    F: FnOnce(&mut Map<String, Value>) -> DynResult<R>,
{
    let src_path =
        first_existing_lez_path(lez, SEQUENCER_CONFIG_REL_PATHS, "sequencer debug config")?;
    let text = fs::read_to_string(&src_path)
        .with_context(|| format!("failed to read {}", src_path.display()))?;
    let mut doc: Value =
        serde_json::from_str(&text).context("failed to parse sequencer_config.json")?;
    let Some(obj) = doc.as_object_mut() else {
        bail!(
            "sequencer_config.json is not a JSON object: {}",
            src_path.display()
        );
    };

    let result = patch(obj)?;

    fs::create_dir_all(dest_dir)
        .with_context(|| format!("failed to create {}", dest_dir.display()))?;
    let dest_path = dest_dir.join(RUNTIME_SEQUENCER_CONFIG_FILE);
    let updated = serde_json::to_string_pretty(&doc).context("failed to serialize config")?;
    crate::state::write_text_atomic(&dest_path, &format!("{updated}\n"))
        .with_context(|| format!("failed to write {}", dest_path.display()))?;
    Ok((dest_path, result))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use crate::constants::{SEQUENCER_CONFIG_NESTED_REL_PATH, SEQUENCER_CONFIG_REL_PATH};

    use super::*;
    use tempfile::tempdir;

    fn write_source_config(root: &Path) -> PathBuf {
        let lez = root.join("lez");
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().expect("parent")).expect("create config dir");
        fs::write(&config_path, r#"{"home": ".", "genesis_id": 1}"#).expect("write source config");
        lez
    }

    #[cfg(unix)]
    fn fake_sequencer(dir: &Path, help: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("sequencer_service");
        fs::write(&bin, format!("#!/bin/sh\ncat <<'EOF'\n{help}\nEOF\n")).expect("write fake");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
        bin
    }

    #[cfg(unix)]
    #[test]
    fn loopback_listen_args_follow_what_the_sequencer_advertises() {
        let temp = tempdir().expect("tempdir");
        let new = fake_sequencer(
            temp.path(),
            "Options:\n  -p, --port <PORT>\n      --listen-address <LISTEN_ADDRESS>",
        );
        assert_eq!(
            loopback_listen_args(&new),
            vec!["--listen-address", "127.0.0.1"]
        );

        let old = fake_sequencer(temp.path(), "Options:\n  -p, --port <PORT>");
        assert!(
            loopback_listen_args(&old).is_empty(),
            "older pins have no flag"
        );

        assert!(
            loopback_listen_args(&temp.path().join("missing")).is_empty(),
            "an unrunnable binary must not add args (the spawn reports the real error)"
        );
    }

    #[test]
    fn patch_runtime_sequencer_config_accepts_nested_lez_layout() {
        let temp = tempdir().expect("tempdir");
        let lez = temp.path().join("lez");
        let config_path = lez.join(SEQUENCER_CONFIG_NESTED_REL_PATH);
        let dest_dir = temp.path().join("runtime");
        fs::create_dir_all(config_path.parent().expect("parent")).expect("create config dir");
        fs::write(&config_path, r#"{"home": ".", "genesis_id": 7}"#).expect("write source config");

        let (dest_path, genesis_id) = patch_runtime_sequencer_config(&lez, &dest_dir, |obj| {
            obj.insert("port".to_string(), Value::Number(30_400.into()));
            Ok(obj["genesis_id"].as_u64().expect("genesis_id"))
        })
        .expect("patch config");

        assert_eq!(genesis_id, 7);
        assert_eq!(dest_path, dest_dir.join(RUNTIME_SEQUENCER_CONFIG_FILE));
        let doc =
            fs::read_to_string(dest_dir.join(RUNTIME_SEQUENCER_CONFIG_FILE)).expect("read config");
        let doc: Value = serde_json::from_str(&doc).expect("parse config");
        assert_eq!(doc["port"], serde_json::json!(30_400));
    }

    #[test]
    fn patch_runtime_sequencer_config_produces_valid_json_under_concurrent_writes() {
        let temp = tempdir().expect("tempdir");
        let lez = write_source_config(temp.path());
        let dest_dir = temp.path().join("runtime");
        // Concurrent node starts must target distinct --work-dir values; this only
        // verifies atomic replacement leaves a valid final config.
        let barrier = Arc::new(Barrier::new(8));

        let handles: Vec<_> = (0..8)
            .map(|index| {
                let lez = lez.clone();
                let dest_dir = dest_dir.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    patch_runtime_sequencer_config(&lez, &dest_dir, |obj| {
                        obj.insert("port".to_string(), Value::Number((40_000 + index).into()));
                        Ok(index)
                    })
                })
            })
            .collect();

        for (index, handle) in handles.into_iter().enumerate() {
            let (_path, result) = handle.join().expect("writer thread").expect("patch config");
            assert_eq!(result, index);
        }

        let final_config =
            fs::read_to_string(dest_dir.join(RUNTIME_SEQUENCER_CONFIG_FILE)).expect("read config");
        let final_config: Value = serde_json::from_str(&final_config).expect("parse config");
        assert_eq!(final_config["genesis_id"], serde_json::json!(1));
        assert!(final_config["port"].as_u64().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn patch_runtime_sequencer_config_does_not_follow_predictable_tmp_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().expect("tempdir");
        let lez = write_source_config(temp.path());
        let dest_dir = temp.path().join("runtime");
        fs::create_dir_all(&dest_dir).expect("create runtime dir");
        let victim = temp.path().join("victim.txt");
        fs::write(&victim, "keep").expect("write victim");
        let predictable_tmp = dest_dir.join(format!(
            ".{RUNTIME_SEQUENCER_CONFIG_FILE}.{}.tmp",
            std::process::id()
        ));
        symlink(&victim, &predictable_tmp).expect("create predictable tmp symlink");

        patch_runtime_sequencer_config(&lez, &dest_dir, |obj| {
            obj.insert("port".to_string(), Value::Number(30_400.into()));
            Ok(())
        })
        .expect("patch config");

        assert_eq!(fs::read_to_string(&victim).expect("read victim"), "keep");
        assert!(fs::symlink_metadata(&predictable_tmp)
            .expect("tmp symlink metadata")
            .file_type()
            .is_symlink());
    }
}
