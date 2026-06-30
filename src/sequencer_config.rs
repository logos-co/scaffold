use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde_json::{Map, Value};

use crate::constants::SEQUENCER_CONFIG_REL_PATH;
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

pub(crate) fn patch_runtime_sequencer_config<F, R>(
    lez: &Path,
    dest_dir: &Path,
    patch: F,
) -> DynResult<(PathBuf, R)>
where
    F: FnOnce(&mut Map<String, Value>) -> DynResult<R>,
{
    let src_path = lez.join(SEQUENCER_CONFIG_REL_PATH);
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
        fs::write(&config_path, r#"{"home": ".", "genesis_id": 1}"#).expect("write source config");
        lez
    }

    #[test]
    fn patch_runtime_sequencer_config_allows_same_process_parallel_writes() {
        let temp = tempdir().expect("tempdir");
        let lez = write_source_config(temp.path());
        let dest_dir = temp.path().join("runtime");
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
