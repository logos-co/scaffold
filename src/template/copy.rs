use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail};

use crate::constants::TAIL_CALL_TARGETS;
use crate::state::write_text;
use crate::DynResult;

/// Write each tail-call caller's callee id (see [`TAIL_CALL_TARGETS`]) as
/// the deterministic build produces it.
pub(crate) fn patch_tail_call_program_ids(project_root: &Path) -> DynResult<()> {
    for target in TAIL_CALL_TARGETS {
        let path = guest_bin_dir(project_root).join(target.caller_file);
        set_hex_const(&path, target.const_name, target.docker_program_id)?;
    }
    Ok(())
}

pub(crate) fn guest_bin_dir(project_root: &Path) -> std::path::PathBuf {
    project_root.join("methods/guest/src/bin")
}

/// Set `const <name>: &str = "<hex>";` in the Rust file at `path`. Returns
/// whether the file changed; a missing file or const is left alone.
pub(crate) fn set_hex_const(path: &Path, const_name: &str, hex: &str) -> DynResult<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(path)?;
    let marker = format!("const {const_name}: &str =");
    let Some(marker_pos) = content.find(&marker) else {
        return Ok(false);
    };

    let from_marker = &content[marker_pos..];
    let open_quote_rel = from_marker
        .find('"')
        .ok_or_else(|| anyhow!("failed to locate opening quote for {const_name}"))?;
    let open_quote = marker_pos + open_quote_rel + 1;

    let after_open = &content[open_quote..];
    let close_quote_rel = after_open
        .find('"')
        .ok_or_else(|| anyhow!("failed to locate closing quote for {const_name}"))?;
    let close_quote = open_quote + close_quote_rel;

    if content[open_quote..close_quote].eq_ignore_ascii_case(hex) {
        return Ok(false);
    }

    let mut patched = String::with_capacity(content.len());
    patched.push_str(&content[..open_quote]);
    patched.push_str(hex);
    patched.push_str(&content[close_quote..]);

    write_text(path, &patched)?;
    Ok(true)
}

pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> DynResult<()> {
    if !src.exists() {
        bail!("copy source does not exist: {}", src.display());
    }
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str =
        "/// id\nconst HELLO_WORLD_PROGRAM_ID_HEX: &str =\n    \"aaaa\";\nfn main() {}\n";

    #[test]
    fn set_hex_const_rewrites_only_the_named_const() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("simple_tail_call.rs");
        fs::write(&path, SRC).unwrap();

        assert!(set_hex_const(&path, "HELLO_WORLD_PROGRAM_ID_HEX", "bbbb").unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            SRC.replace("\"aaaa\"", "\"bbbb\"")
        );
        // Same value again: unchanged, so `build` does not rebuild.
        assert!(!set_hex_const(&path, "HELLO_WORLD_PROGRAM_ID_HEX", "BBBB").unwrap());
        // Unknown const or missing file: left alone.
        assert!(!set_hex_const(&path, "OTHER_PROGRAM_ID_HEX", "cccc").unwrap());
        assert!(!set_hex_const(&dir.path().join("absent.rs"), "X", "cccc").unwrap());
    }

    #[test]
    fn new_writes_docker_ids_into_both_tail_call_callers() {
        let dir = tempfile::tempdir().unwrap();
        let bin = guest_bin_dir(dir.path());
        fs::create_dir_all(&bin).unwrap();
        for target in TAIL_CALL_TARGETS {
            fs::write(
                bin.join(target.caller_file),
                format!("const {}: &str =\n    \"stale\";\n", target.const_name),
            )
            .unwrap();
        }

        patch_tail_call_program_ids(dir.path()).unwrap();

        for target in TAIL_CALL_TARGETS {
            let text = fs::read_to_string(bin.join(target.caller_file)).unwrap();
            assert!(text.contains(target.docker_program_id), "{text}");
        }
    }
}
