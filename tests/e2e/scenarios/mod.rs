//! One module per feature file in `.dogfooding/features/`. Each public
//! function is one scenario; its doc comment names the ID.

pub mod api;
pub mod basecamp;
pub mod build;
pub mod cli;
pub mod create;
pub mod deploy;
pub mod diag;
pub mod localnet;
pub mod run;
pub mod runner;
pub mod skills;
pub mod spel;
pub mod tnode;
pub mod trpc;
pub mod tui;
pub mod wallet;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// The body of a `[section]` of a TOML document: the lines up to the next
/// table header.
pub fn toml_section(text: &str, header: &str) -> String {
    let mut body = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line.trim() == header {
            inside = true;
            continue;
        }
        if inside {
            if line.starts_with('[') {
                break;
            }
            body.push_str(line);
            body.push('\n');
        }
    }
    body
}

/// A `key = "value"` string value inside a TOML section body.
pub fn toml_value(section: &str, key: &str) -> Option<String> {
    section.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
    })
}

/// Relative paths of every file under `root`, skipping directories named in
/// `prune` at the top level.
pub fn files_under(root: &Path, prune: &[&str]) -> BTreeSet<String> {
    fn walk(dir: &Path, root: &Path, prune: &[&str], out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            if prune.contains(&rel.as_str()) {
                continue;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                walk(&path, root, prune, out);
            } else {
                out.insert(rel);
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, root, prune, &mut out);
    out
}

/// Names directly inside `dir` (like `ls -A`).
pub fn ls(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
