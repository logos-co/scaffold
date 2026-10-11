use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde_json::{Map, Value};

use crate::constants::{
    DEV_FUNDER_ACCOUNT_ID, DEV_FUNDER_GENESIS_BALANCE, SEQUENCER_CONFIG_REL_PATH,
};
use crate::lez_layout::lez_file;
use crate::DynResult;

pub(crate) const RUNTIME_SEQUENCER_CONFIG_FILE: &str = "sequencer_config.json";

/// Override every scaffold-started sequencer (localnet and test-node)
/// applies to LEZ's debug config: the dev funder ([`DEV_FUNDER_ACCOUNT_ID`])
/// is guaranteed a genesis `supply_account`. LEZ's debug config already funds
/// it today; enforcing it here keeps `wallet topup` working if upstream
/// reshuffles its debug accounts.
///
/// `max_block_size` is deliberately left at the upstream value: LEZ 0.3
/// refuses to start above Bedrock's inscription limit (1,835,008 bytes), and
/// `program_loader` deployments upload one 96 KiB segment per transaction, so
/// the old "fit every guest ELF in one block" widening is neither possible
/// nor needed.
pub(crate) fn apply_common_runtime_overrides(obj: &mut Map<String, Value>) {
    ensure_genesis_supply(obj, DEV_FUNDER_ACCOUNT_ID, DEV_FUNDER_GENESIS_BALANCE);
}

/// Add a `{"supply_account": {account_id, balance}}` genesis action for
/// `account_id` unless the config already supplies it.
pub(crate) fn ensure_genesis_supply(obj: &mut Map<String, Value>, account_id: &str, balance: u64) {
    let genesis = obj
        .entry("genesis")
        .or_insert_with(|| Value::Array(Vec::new()));
    if !genesis.is_array() {
        *genesis = Value::Array(Vec::new());
    }
    let actions = genesis.as_array_mut().expect("genesis is an array");
    let already_supplied = actions.iter().any(|action| {
        action
            .get("supply_account")
            .and_then(|supply| supply.get("account_id"))
            .and_then(Value::as_str)
            == Some(account_id)
    });
    if !already_supplied {
        actions.push(serde_json::json!({
            "supply_account": { "account_id": account_id, "balance": balance }
        }));
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
    let src_path = lez_file(lez, SEQUENCER_CONFIG_REL_PATH, "sequencer debug config")?;
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
    let mut tmp = tempfile::NamedTempFile::new_in(dest_dir)
        .with_context(|| format!("failed to create temp file in {}", dest_dir.display()))?;
    let tmp_path = tmp.path().to_path_buf();
    tmp.write_all(format!("{updated}\n").as_bytes())
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    tmp.persist(&dest_path)
        .map(|_| ())
        .map_err(|err| err.error)
        .with_context(|| {
            format!(
                "failed to replace {} with {}",
                dest_path.display(),
                tmp_path.display()
            )
        })?;
    Ok((dest_path, result))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;
    use tempfile::tempdir;

    fn write_source_config(root: &Path) -> PathBuf {
        let lez = root.join("lez");
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        fs::create_dir_all(config_path.parent().expect("parent")).expect("create config dir");
        fs::write(&config_path, r#"{"home": ".", "max_num_tx_in_block": 1}"#)
            .expect("write source config");
        lez
    }

    #[test]
    fn patch_runtime_sequencer_config_reads_lez_0_3_layout() {
        let temp = tempdir().expect("tempdir");
        let lez = temp.path().join("lez");
        let config_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
        let dest_dir = temp.path().join("runtime");
        fs::create_dir_all(config_path.parent().expect("parent")).expect("create config dir");
        fs::write(&config_path, r#"{"home": ".", "max_num_tx_in_block": 7}"#)
            .expect("write source config");

        let (dest_path, genesis_id) = patch_runtime_sequencer_config(&lez, &dest_dir, |obj| {
            obj.insert("port".to_string(), Value::Number(30_400.into()));
            Ok(obj["max_num_tx_in_block"]
                .as_u64()
                .expect("max_num_tx_in_block"))
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
        assert_eq!(final_config["max_num_tx_in_block"], serde_json::json!(1));
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

    #[test]
    fn common_overrides_fund_the_dev_funder_once_and_keep_block_size() {
        let mut obj: Map<String, Value> = serde_json::from_str(
            r#"{"max_block_size": "1 MiB", "genesis": [
                {"supply_account": {"account_id": "other", "balance": 5}}
            ]}"#,
        )
        .expect("json");
        apply_common_runtime_overrides(&mut obj);
        apply_common_runtime_overrides(&mut obj);
        assert_eq!(obj["max_block_size"], "1 MiB");
        let genesis = obj["genesis"].as_array().expect("genesis");
        assert_eq!(genesis.len(), 2, "{genesis:?}");
        assert_eq!(
            genesis[1]["supply_account"]["account_id"],
            DEV_FUNDER_ACCOUNT_ID
        );
    }
}
