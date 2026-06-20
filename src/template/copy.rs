use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail};

use crate::constants::DEFAULT_HELLO_WORLD_IMAGE_ID_HEX;
use crate::state::write_text;
use crate::DynResult;

pub(crate) fn patch_simple_tail_call_program_id(project_root: &Path) -> DynResult<()> {
    let path = project_root.join("methods/guest/src/bin/simple_tail_call.rs");
    if !path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&path)?;
    let marker = "const HELLO_WORLD_PROGRAM_ID_HEX: &str =";
    let Some(marker_pos) = content.find(marker) else {
        return Ok(());
    };

    let from_marker = &content[marker_pos..];
    let open_quote_rel = from_marker
        .find('"')
        .ok_or_else(|| anyhow!("failed to locate opening quote for HELLO_WORLD_PROGRAM_ID_HEX"))?;
    let open_quote = marker_pos + open_quote_rel + 1;

    let after_open = &content[open_quote..];
    let close_quote_rel = after_open
        .find('"')
        .ok_or_else(|| anyhow!("failed to locate closing quote for HELLO_WORLD_PROGRAM_ID_HEX"))?;
    let close_quote = open_quote + close_quote_rel;

    if &content[open_quote..close_quote] == DEFAULT_HELLO_WORLD_IMAGE_ID_HEX {
        return Ok(());
    }

    let mut patched = String::with_capacity(content.len());
    patched.push_str(&content[..open_quote]);
    patched.push_str(DEFAULT_HELLO_WORLD_IMAGE_ID_HEX);
    patched.push_str(&content[close_quote..]);

    write_text(&path, &patched)?;
    Ok(())
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
