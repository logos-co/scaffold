use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail};

use crate::constants::FRAMEWORK_KIND_LEZ_FRAMEWORK;
use crate::process::run_capture;
use crate::project::{load_project, run_in_project_dir};
use crate::state::write_text;
use crate::DynResult;

const IDL_BEGIN_PREFIX: &str = "--- LSSA IDL BEGIN ";
const IDL_END_PREFIX: &str = "--- LSSA IDL END ";
const IDL_MARKER_SUFFIX: &str = " ---";

pub(crate) fn cmd_idl(args: &[String]) -> DynResult<()> {
    if args.is_empty() {
        bail!("usage: logos-scaffold build idl [project-path]");
    }

    match args[0].as_str() {
        "build" => {
            let project_dir = parse_optional_project_path(&args[1..], "logos-scaffold build idl")?;
            run_in_project_dir(project_dir.as_deref(), build_idl_for_current_project)
        }
        other => Err(anyhow!("unknown idl command: {other}")),
    }
}

pub(crate) fn build_idl_for_current_project() -> DynResult<()> {
    let project = load_project()?;
    if project.config.framework.kind != FRAMEWORK_KIND_LEZ_FRAMEWORK {
        // Explicit `build idl` only applies to lez-framework projects. The
        // `lgs build` shortcut already gates on framework kind and won't
        // call this for `default` projects, so reaching this branch means
        // the user typed `build idl` against an incompatible framework.
        // Fail loudly instead of silently no-op'ing — agents that piped
        // `lgs build idl && next-step` would otherwise carry on with no IDL.
        bail!(
            "`build idl` is only supported for `lez-framework` projects (current framework.kind = `{}`).\n\
             Use `logos-scaffold build` for the framework-agnostic build, \
             or set `framework.kind = \"lez-framework\"` in scaffold.toml.",
            project.config.framework.kind
        );
    }

    let idl_dir = project.root.join(&project.config.framework.idl.path);
    fs::create_dir_all(&idl_dir)?;
    clear_existing_json_files(&idl_dir)?;

    let out = run_capture(
        Command::new("cargo")
            .current_dir(&project.root)
            .arg("test")
            .arg("--workspace")
            .arg("__lssa_idl_print")
            .arg("--")
            .arg("--show-output")
            .arg("--quiet"),
        "cargo test __lssa_idl_print",
    )?;

    let mut blocks = parse_idl_blocks(&out.stdout)?;
    if blocks.is_empty() {
        bail!("no IDL blocks were printed. Ensure hidden test `__lssa_idl_print` is configured.");
    }

    blocks.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, json_text) in blocks {
        let canonical = canonical_json(&json_text)?;
        let file_name = format!("{}.json", sanitize_file_stem(&name));
        let path = idl_dir.join(file_name);
        write_text(&path, &canonical)?;
        println!("Wrote IDL {}", path.display());
    }

    Ok(())
}

fn parse_optional_project_path(args: &[String], usage_label: &str) -> DynResult<Option<PathBuf>> {
    let mut project_dir: Option<PathBuf> = None;

    for arg in args {
        if arg.starts_with("--") {
            bail!("unknown flag for `{usage_label}`: {arg}");
        }
        if project_dir.is_none() {
            project_dir = Some(PathBuf::from(arg));
        } else {
            bail!("unexpected argument `{arg}` for `{usage_label}`");
        }
    }

    Ok(project_dir)
}

fn clear_existing_json_files(dir: &std::path::Path) -> DynResult<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            fs::remove_file(path)?;
        }
    }

    Ok(())
}

fn canonical_json(text: &str) -> DynResult<String> {
    let value: serde_json::Value = serde_json::from_str(text.trim())?;
    let pretty = serde_json::to_string_pretty(&value)?;
    Ok(format!("{pretty}\n"))
}

fn sanitize_file_stem(name: &str) -> String {
    let mut out = String::new();
    let mut prev_sep = false;

    for ch in name.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '_'
        };

        if mapped == '_' {
            if !prev_sep {
                out.push('_');
                prev_sep = true;
            }
        } else {
            out.push(mapped);
            prev_sep = false;
        }
    }

    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "program".to_string()
    } else {
        out
    }
}

fn parse_idl_blocks(output: &str) -> DynResult<Vec<(String, String)>> {
    let mut blocks = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for raw_line in output.lines() {
        if let Some(name) = parse_marker(raw_line, IDL_BEGIN_PREFIX) {
            if current_name.is_some() {
                bail!("found nested IDL begin marker");
            }
            current_name = Some(name.to_string());
            current_lines.clear();
            continue;
        }

        if let Some(name) = parse_marker(raw_line, IDL_END_PREFIX) {
            let Some(open_name) = current_name.take() else {
                bail!("found IDL end marker without begin");
            };
            if open_name != name {
                bail!("IDL marker mismatch: begin `{open_name}` end `{name}`");
            }
            blocks.push((open_name, current_lines.join("\n")));
            current_lines.clear();
            continue;
        }

        if current_name.is_some() {
            current_lines.push(raw_line.to_string());
        }
    }

    if let Some(open_name) = current_name {
        bail!("missing IDL end marker for `{open_name}`");
    }

    Ok(blocks)
}

fn parse_marker<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let tail = line.strip_prefix(prefix)?;
    tail.strip_suffix(IDL_MARKER_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::parse_idl_blocks;

    #[test]
    fn parses_idl_blocks() {
        let text = r#"
ignored
--- LSSA IDL BEGIN token_program ---
{"a":1}
--- LSSA IDL END token_program ---
"#;
        let blocks = parse_idl_blocks(text).expect("blocks should parse");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, "token_program");
        assert_eq!(blocks[0].1, r#"{"a":1}"#);
    }
}
