use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::circuits::ensure_circuits_for_subprocess;
use crate::constants::FRAMEWORK_KIND_LEZ_FRAMEWORK;
use crate::model::Project;
use crate::process::run_capture;
use crate::project::{load_project, resolve_cache_root, run_in_project_dir};
use crate::state::write_text;
use crate::DynResult;

const IDL_BEGIN_PREFIX: &str = "--- LSSA IDL BEGIN ";
const IDL_END_PREFIX: &str = "--- LSSA IDL END ";
const IDL_MARKER_SUFFIX: &str = " ---";

/// Cache file recording the fingerprint of the inputs that produced the IDL,
/// alongside the list of output `.json` files written. Lives in the same
/// `.scaffold/state/` neighbourhood as `run_deploy.json`.
pub(crate) const IDL_STATE_REL: &str = ".scaffold/state/idl_build.json";

/// Persisted IDL-build fingerprint. `input_hash` covers every project `.rs`
/// source and `Cargo.toml` manifest, `Cargo.lock` (which pins the IDL-emitting
/// proc-macro), and the `[framework.idl]` config; `outputs` are the file names
/// written so a cache hit can verify they still exist before skipping the
/// rebuild.
#[derive(Debug, Default, Serialize, Deserialize)]
struct IdlBuildState {
    input_hash: String,
    outputs: Vec<String>,
}

pub(crate) fn cmd_idl(args: &[String]) -> DynResult<()> {
    if args.is_empty() {
        bail!("usage: logos-scaffold build idl [project-path]");
    }

    match args[0].as_str() {
        "build" => {
            let project_dir = parse_optional_project_path(&args[1..], "logos-scaffold build idl")?;
            // Explicit `build idl` regenerates unconditionally (force = true):
            // it's the documented "regenerate the IDL" escape hatch, so it
            // must not silently short-circuit on the cache.
            run_in_project_dir(project_dir.as_deref(), || build_idl_inner(true))
        }
        other => Err(anyhow!("unknown idl command: {other}")),
    }
}

/// Cache-aware IDL build used by the `build` / `run` pipelines. Short-circuits
/// when inputs are unchanged; see `build_idl_inner`.
pub(crate) fn build_idl_for_current_project() -> DynResult<()> {
    build_idl_inner(false)
}

fn build_idl_inner(force: bool) -> DynResult<()> {
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

    // Hash-based short-circuit: the `cargo test __lssa_idl_print` invocation
    // below dominates `lgs run` wall time (~30s) even when it re-emits an IDL
    // whose inputs are byte-identical. When the fingerprint matches the prior
    // build AND every output file is still on disk, skip the rebuild.
    let input_hash = compute_idl_input_hash(&project)?;
    if !force {
        if let Some(state) = load_idl_state(&project) {
            if state.input_hash == input_hash && outputs_present(&idl_dir, &state.outputs) {
                println!(
                    "IDL up-to-date (skipped); inputs unchanged. Force a rebuild with `lgs build idl` or `lgs run --reset`."
                );
                return Ok(());
            }
        }
    }

    fs::create_dir_all(&idl_dir)?;
    clear_existing_json_files(&idl_dir)?;

    // Same rationale as `setup`: the workspace test build pulls in the
    // logos-blockchain crates that need a populated circuits release.
    let (cache_root, _) = resolve_cache_root(&project)?;
    ensure_circuits_for_subprocess(&cache_root)?;

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
    let mut outputs = Vec::new();
    for (name, json_text) in blocks {
        let canonical = canonical_json(&json_text)?;
        let file_name = format!("{}.json", sanitize_file_stem(&name));
        let path = idl_dir.join(&file_name);
        write_text(&path, &canonical)?;
        println!("Wrote IDL {}", path.display());
        outputs.push(file_name);
    }

    // Record the fingerprint last, after every output landed. A crash
    // mid-write leaves no/old state, so the next build re-runs rather than
    // trusting a half-written IDL set.
    save_idl_state(
        &project,
        &IdlBuildState {
            input_hash,
            outputs,
        },
    )?;

    Ok(())
}

/// SHA-256 over the inputs that determine the IDL output: every `.rs` source
/// and every `Cargo.toml` manifest in the project (target / hidden / vendor
/// dirs excluded), `Cargo.lock` (so a `cargo update` that bumps the
/// IDL-emitting proc-macro busts the cache even when guest sources are
/// byte-identical), and the `[framework.idl]` config. Manifests are included
/// because a `Cargo.toml` feature-flag / dependency / proc-macro change can
/// alter what `cargo test __lssa_idl_print` emits without touching any `.rs`
/// file or `Cargo.lock`.
fn compute_idl_input_hash(project: &Project) -> DynResult<String> {
    let root = &project.root;
    // Collect just the input paths (no file bytes), sort for a stable digest,
    // then stream each file into the hasher one at a time — peak memory is a
    // single file rather than the whole source tree.
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| keep_for_idl_hash(e, root))
    {
        let entry = entry.with_context(|| format!("walk {}", root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let is_rs = path.extension().is_some_and(|ext| ext == "rs");
        let is_manifest = path.file_name().is_some_and(|n| n == "Cargo.toml");
        if is_rs || is_manifest {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            files.push((rel, path.to_path_buf()));
        }
    }
    // Deterministic order so the digest is stable regardless of FS traversal.
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (rel, abs) in &files {
        hasher.update(rel.as_bytes());
        hasher.update(b"\x00");
        let bytes = fs::read(abs).with_context(|| format!("read {}", abs.display()))?;
        hasher.update(&bytes);
        hasher.update(b"\x00");
    }
    // Cargo.lock is part of the cache key. A missing lock is fine (absent), but
    // any other read error (permissions, I/O) must surface — silently ignoring
    // it would fold an empty value into the digest and risk a false cache hit
    // that skips a needed rebuild.
    let lock_path = root.join("Cargo.lock");
    match fs::read(&lock_path) {
        Ok(lock) => {
            hasher.update(b"cargo.lock\x00");
            hasher.update(&lock);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| format!("read {}", lock_path.display()));
        }
    }
    hasher.update(b"idlcfg\x00");
    hasher.update(project.config.framework.idl.spec.as_bytes());
    hasher.update(b"\x00");
    hasher.update(project.config.framework.idl.path.as_bytes());

    Ok(hex_encode(&hasher.finalize()))
}

/// `filter_entry` predicate: keep the root, every file, and any directory that
/// isn't a build/vendor/hidden dir. Pruning here (rather than post-filtering)
/// means `walkdir` never descends into `target/`, `node_modules/`, `result/`,
/// `.scaffold/`, or `.git/`.
fn keep_for_idl_hash(entry: &walkdir::DirEntry, root: &Path) -> bool {
    if entry.path() == root {
        return true;
    }
    if entry.file_type().is_dir() {
        let name = entry.file_name().to_string_lossy();
        if name.starts_with('.')
            || matches!(
                name.as_ref(),
                "target" | "node_modules" | "result" | "vendor"
            )
        {
            return false;
        }
    }
    true
}

fn outputs_present(idl_dir: &Path, outputs: &[String]) -> bool {
    // `is_file()` (not `exists()`): a directory named `<stem>.json` must not
    // count as a valid cached output and short-circuit the rebuild.
    !outputs.is_empty() && outputs.iter().all(|name| idl_dir.join(name).is_file())
}

fn load_idl_state(project: &Project) -> Option<IdlBuildState> {
    let path = project.root.join(IDL_STATE_REL);
    let bytes = fs::read(&path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(state) => Some(state),
        Err(err) => {
            eprintln!(
                "warning: ignoring malformed IDL cache at {} ({err}); IDL will rebuild",
                path.display()
            );
            None
        }
    }
}

fn save_idl_state(project: &Project, state: &IdlBuildState) -> DynResult<()> {
    let path = project.root.join(IDL_STATE_REL);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(state).context("serialize idl_build.json")?;
    // Atomic write: stage to a sibling tempfile, then rename. A SIGINT
    // between write and rename leaves the prior cache intact rather than
    // truncating it. Same parent dir keeps rename(2) atomic.
    let tmp = path.with_file_name("idl_build.json.tmp");
    fs::write(&tmp, &text).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub(crate) fn parse_optional_project_path(
    args: &[String],
    usage_label: &str,
) -> DynResult<Option<PathBuf>> {
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

pub(crate) fn sanitize_file_stem(name: &str) -> String {
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
    use super::*;
    use std::collections::BTreeMap;

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

    fn make_test_project(root: std::path::PathBuf) -> Project {
        use crate::model::{
            Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, RepoRef, RunConfig,
        };
        Project {
            root,
            config: Config {
                version: "0.2.0".to_string(),
                cache_root: ".scaffold/cache".to_string(),
                lez: RepoRef::default(),
                spel: RepoRef::default(),
                basecamp_repo: None,
                lgpm_repo: None,
                wallet_home_dir: ".scaffold/wallet".to_string(),
                framework: FrameworkConfig {
                    kind: FRAMEWORK_KIND_LEZ_FRAMEWORK.to_string(),
                    version: "0.1.0".to_string(),
                    idl: FrameworkIdlConfig {
                        spec: "lssa-idl/0.1.0".to_string(),
                        path: "idl".to_string(),
                    },
                },
                localnet: LocalnetConfig {
                    port: 3040,
                    risc0_dev_mode: true,
                },
                modules: BTreeMap::new(),
                run: RunConfig::default(),
                basecamp: None,
            },
        }
    }

    fn write(root: &Path, rel: &str, contents: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn idl_hash_is_stable_when_nothing_changes() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            temp.path(),
            "methods/guest/src/bin/counter.rs",
            b"fn main() {}\n",
        );
        let project = make_test_project(temp.path().to_path_buf());
        let a = compute_idl_input_hash(&project).expect("hash a");
        let b = compute_idl_input_hash(&project).expect("hash b");
        assert_eq!(a, b);
    }

    #[test]
    fn idl_hash_changes_when_a_source_file_changes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let rel = "methods/guest/src/bin/counter.rs";
        write(temp.path(), rel, b"fn main() {}\n");
        let project = make_test_project(temp.path().to_path_buf());
        let before = compute_idl_input_hash(&project).expect("before");
        // Edit a guest source that feeds the IDL.
        write(temp.path(), rel, b"fn main() { /* changed */ }\n");
        let after = compute_idl_input_hash(&project).expect("after");
        assert_ne!(before, after, "editing a guest source must bust the cache");
    }

    #[test]
    fn idl_hash_changes_when_cargo_lock_changes() {
        // Acceptance: bumping the IDL-emitting macro's pin (which lands in
        // Cargo.lock) re-runs the IDL even if guest sources are identical.
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            temp.path(),
            "methods/guest/src/bin/counter.rs",
            b"fn main() {}\n",
        );
        write(temp.path(), "Cargo.lock", b"# v1\n");
        let project = make_test_project(temp.path().to_path_buf());
        let before = compute_idl_input_hash(&project).expect("before");
        write(temp.path(), "Cargo.lock", b"# v2 (macro pin bumped)\n");
        let after = compute_idl_input_hash(&project).expect("after");
        assert_ne!(before, after, "Cargo.lock change must bust the cache");
    }

    #[test]
    fn idl_hash_changes_when_cargo_toml_changes() {
        // A Cargo.toml feature/dep/proc-macro change can alter the emitted IDL
        // without touching any .rs file or Cargo.lock — it must bust the cache.
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            temp.path(),
            "methods/guest/src/bin/counter.rs",
            b"fn main() {}\n",
        );
        write(temp.path(), "Cargo.toml", b"[package]\nname='x'\n");
        let project = make_test_project(temp.path().to_path_buf());
        let before = compute_idl_input_hash(&project).expect("before");
        write(
            temp.path(),
            "Cargo.toml",
            b"[package]\nname='x'\n\n[features]\nidl-extra=[]\n",
        );
        let after = compute_idl_input_hash(&project).expect("after");
        assert_ne!(before, after, "Cargo.toml change must bust the cache");
    }

    #[test]
    fn idl_hash_ignores_target_hidden_and_vendor_dirs() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            temp.path(),
            "methods/guest/src/bin/counter.rs",
            b"fn main() {}\n",
        );
        let project = make_test_project(temp.path().to_path_buf());
        let before = compute_idl_input_hash(&project).expect("before");
        // Build artifacts, scaffold state, and vendored sources must not affect
        // the fingerprint (vendor/ can be large + is not project source).
        write(temp.path(), "target/debug/build/gen.rs", b"// generated\n");
        write(temp.path(), "methods/target/x/riscv.rs", b"// generated\n");
        write(temp.path(), ".scaffold/state/note.rs", b"// state\n");
        write(temp.path(), "vendor/somecrate/src/lib.rs", b"// vendored\n");
        write(temp.path(), "vendor/somecrate/Cargo.toml", b"[package]\n");
        let after = compute_idl_input_hash(&project).expect("after");
        assert_eq!(
            before, after,
            "target/.scaffold/vendor must be excluded from the fingerprint"
        );
    }

    #[test]
    fn idl_state_round_trips() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());
        let state = IdlBuildState {
            input_hash: "deadbeef".to_string(),
            outputs: vec!["counter.json".to_string()],
        };
        save_idl_state(&project, &state).expect("save");
        let loaded = load_idl_state(&project).expect("load");
        assert_eq!(loaded.input_hash, "deadbeef");
        assert_eq!(loaded.outputs, vec!["counter.json".to_string()]);
        // No leftover tempfile.
        assert!(!temp
            .path()
            .join(".scaffold/state/idl_build.json.tmp")
            .exists());
    }

    #[test]
    fn outputs_present_requires_every_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let idl_dir = temp.path().join("idl");
        fs::create_dir_all(&idl_dir).unwrap();
        fs::write(idl_dir.join("counter.json"), b"{}").unwrap();
        let outputs = vec!["counter.json".to_string()];
        assert!(outputs_present(&idl_dir, &outputs));
        // A second expected file that's missing flips it to false.
        let two = vec!["counter.json".to_string(), "token.json".to_string()];
        assert!(!outputs_present(&idl_dir, &two));
        // Empty outputs never count as a fresh cache.
        assert!(!outputs_present(&idl_dir, &[]));
        // A *directory* named like an output must not count as present.
        fs::create_dir_all(idl_dir.join("dir.json")).unwrap();
        assert!(!outputs_present(&idl_dir, &["dir.json".to_string()]));
    }

    #[test]
    fn malformed_idl_cache_is_ignored() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());
        write(temp.path(), ".scaffold/state/idl_build.json", b"{not json");
        assert!(load_idl_state(&project).is_none());
    }
}
