use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context};
use walkdir::WalkDir;

use crate::constants::SPEL_BIN_REL_PATH;
use crate::process::{run_with_stdin, EchoGuard};
use crate::project::{load_project, resolve_repo_path};
use crate::DynResult;

use super::wallet_support::{
    extract_tx_identifier, is_connectivity_failure, load_wallet_runtime, rpc_get_last_block_id,
    sequencer_unreachable_hint, summarize_command_failure, wallet_password, RpcReachabilityError,
};

/// Roots searched (in order) for guest `.bin` artefacts. Both layouts exist in
/// the wild: risc0's default workspace layout emits to `target/riscv-guest/...`
/// (used by the scaffold template), while sub-crate builds can land in
/// `methods/target/...`. Discovery walks both so renamed projects work
/// regardless of which layout cargo/risc0 chose. The `methods/...` half of
/// this constant is the same project-relative directory that `build.rs`
/// compiles via `crate::constants::METHODS_DIR`; keep them in sync.
const GUEST_BIN_SEARCH_ROOTS: &[&str] = &["target/riscv-guest", "methods/target"];
const DEFAULT_SEQUENCER_ADDR: &str = "http://127.0.0.1:3040";

/// `spel inspect` line prefix that carries the risc0 image ID — the value the
/// sequencer uses as the on-chain program ID. Format is whitespace-tolerant:
/// `   ImageID (hex bytes): <64 hex chars>`.
const SPEL_IMAGE_ID_PREFIX: &str = "ImageID (hex bytes):";

pub(crate) fn cmd_deploy(
    program_name: Option<String>,
    program_path: Option<PathBuf>,
    json: bool,
) -> DynResult<()> {
    let project = load_project().context(
        "This command must be run inside a logos-scaffold project.\nNext step: cd into your scaffolded project directory and retry.",
    )?;
    let wallet = load_wallet_runtime(&project)?;
    let spel_bin =
        resolve_repo_path(&project, &project.config.spel, "spel")?.join(SPEL_BIN_REL_PATH);

    let sequencer_addr = wallet
        .sequencer_addr
        .clone()
        .unwrap_or_else(|| DEFAULT_SEQUENCER_ADDR.to_string());

    // --program-path: deploy a single custom ELF directly, skip auto-discovery
    if let Some(custom_path) = program_path {
        if !custom_path.exists() {
            bail!("program binary not found at `{}`", custom_path.display());
        }
        let program_name = custom_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        return deploy_single_program(
            &wallet,
            &program_name,
            &custom_path,
            &sequencer_addr,
            &spel_bin,
            json,
        );
    }

    let available_programs = discover_deployable_programs(&project.root)?;
    if available_programs.is_empty() {
        bail!(
            "no deployable programs found in `{}`",
            project.root.join("methods/guest/src/bin").display()
        );
    }

    let selected_programs = resolve_selected_programs(program_name, &available_programs)?;
    let discovered = discover_program_binaries(&project.root, &selected_programs);

    preflight_sequencer_reachability(&sequencer_addr)?;

    // Suppress the per-subprocess `$ <cmd>` echoes while `--json` is in
    // effect so stdout stays a single JSON object. RAII guard restores echo
    // state on scope exit even if a `?` or panic interrupts the loop.
    let _echo_guard = json.then(EchoGuard::suppress);

    let mut results = Vec::new();
    for program in selected_programs {
        let Some(binary_path) = discovered.get(&program).cloned() else {
            if !json {
                let searched = GUEST_BIN_SEARCH_ROOTS
                    .iter()
                    .map(|r| project.root.join(r).display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("FAIL {program} deployment failed");
                println!("  Error: missing binary `{program}.bin` (searched: {searched})");
                println!("  Hint: run `logos-scaffold build` first.");
            }
            results.push(DeployResult {
                program,
                status: DeployStatus::Failed,
                detail: "missing program binary".to_string(),
                tx: None,
                program_id: None,
            });
            continue;
        };

        let mut command = Command::new(&wallet.wallet_binary);
        command
            .env(
                "NSSA_WALLET_HOME_DIR",
                wallet.wallet_home.as_os_str().to_string_lossy().to_string(),
            )
            .arg("deploy-program")
            .arg(&binary_path);

        let output = match run_with_stdin(command, format!("{}\n", wallet_password())) {
            Ok(output) => output,
            Err(err) => {
                if !json {
                    println!("FAIL {program} deployment failed");
                    println!("  Error: failed to execute wallet command: {err}");
                }
                results.push(DeployResult {
                    program,
                    status: DeployStatus::Failed,
                    detail: format!("wallet command invocation failed: {err}"),
                    tx: None,
                    program_id: None,
                });
                continue;
            }
        };

        let tx = extract_tx_identifier(&output.stdout, &output.stderr);

        if !output.status.success() {
            let summary = summarize_command_failure(&output.stdout, &output.stderr);
            let combined = format!("{}\n{}", output.stdout, output.stderr);
            let connectivity_failure = is_connectivity_failure(&combined);
            if !json {
                println!("FAIL {program} deployment failed");
                println!("  Error: {summary}");
                if connectivity_failure {
                    println!("  Hint: {}", sequencer_unreachable_hint(&sequencer_addr));
                } else {
                    println!("  Hint: inspect sequencer logs and retry.");
                }
            }
            let detail = if connectivity_failure {
                format!("{summary}; sequencer connectivity failure")
            } else {
                summary
            };
            results.push(DeployResult {
                program,
                status: DeployStatus::Failed,
                detail,
                tx,
                program_id: None,
            });
            continue;
        }

        let program_id = extract_program_id(&spel_bin, &binary_path);

        if !json {
            println!("OK  {program} submitted");
            if let Some(tx) = &tx {
                println!("  tx: {tx}");
            }
            print_program_id_line(&program_id);
        }
        results.push(DeployResult {
            program,
            status: DeployStatus::Submitted,
            detail: "wallet submission command exited successfully".to_string(),
            tx,
            program_id,
        });
    }

    let success_count = results
        .iter()
        .filter(|result| matches!(result.status, DeployStatus::Submitted))
        .count();
    let failed_count = results
        .iter()
        .filter(|result| matches!(result.status, DeployStatus::Failed))
        .count();

    if json {
        // Single-line JSON object on stdout. One entry per attempted
        // program; absent fields (tx, program_id) are omitted, not nulled,
        // matching the single-program --program-path contract. Failed
        // entries carry `error` instead of `program_id`.
        let entries: Vec<serde_json::Value> =
            results.iter().map(render_deploy_result_json).collect();
        let value = serde_json::json!({ "deploys": entries });
        println!("{}", serde_json::to_string(&value)?);
    } else {
        println!("Note: Submission confirmed by wallet exit status; deploy inclusion receipt is not currently exposed by LEZ wallet/RPC for scaffold. Program ID is computed locally from the submitted ELF.");
        println!("Summary:");
        println!("  Succeeded: {success_count}");
        println!("  Failed: {failed_count}");
        println!("  Results:");
        for result in &results {
            // Per-program details (program_id, tx) are printed once in the OK
            // block above. Summary only carries the status label + detail to
            // stay terse and avoid grep ambiguity ("which line is canonical?").
            println!("    {}: {}", result.program, result.status.label());
            println!("      {}", result.detail);
        }
    }

    if failed_count > 0 {
        bail!("deploy completed with {failed_count} failed program(s)");
    }

    Ok(())
}

pub(crate) fn render_deploy_result_json(result: &DeployResult) -> serde_json::Value {
    // serde_json handles every escape RFC 8259 mandates (control chars, \u
    // sequences, embedded quotes, ANSI escapes). Hand-rolling the JSON here
    // would re-introduce the `summarize_command_failure`-passes-tabs-through
    // bug class.
    let mut obj = serde_json::Map::new();
    obj.insert(
        "status".to_string(),
        serde_json::Value::String(result.status.label().to_string()),
    );
    obj.insert(
        "program".to_string(),
        serde_json::Value::String(result.program.clone()),
    );
    if let Some(tx) = &result.tx {
        obj.insert("tx".to_string(), serde_json::Value::String(tx.clone()));
    }
    match result.status {
        DeployStatus::Submitted => {
            if let Some(id) = &result.program_id {
                obj.insert(
                    "program_id".to_string(),
                    serde_json::Value::String(id.clone()),
                );
            }
        }
        DeployStatus::Failed => {
            obj.insert(
                "error".to_string(),
                serde_json::Value::String(result.detail.clone()),
            );
        }
    }
    serde_json::Value::Object(obj)
}

fn preflight_sequencer_reachability(sequencer_addr: &str) -> DynResult<()> {
    match rpc_get_last_block_id(sequencer_addr) {
        Ok(_) => Ok(()),
        Err(RpcReachabilityError::Connectivity(err)) => {
            bail!(
                "cannot deploy programs: {err}\n{}",
                sequencer_unreachable_hint(sequencer_addr)
            )
        }
        Err(err) => {
            println!(
                "warning: sequencer reachability probe failed ({err}); continuing with wallet submission mode"
            );
            Ok(())
        }
    }
}

pub(crate) fn discover_deployable_programs(project_root: &Path) -> DynResult<Vec<String>> {
    let programs_dir = project_root.join("methods/guest/src/bin");
    if !programs_dir.exists() {
        bail!(
            "missing deployable program directory at {}",
            programs_dir.display()
        );
    }

    let mut programs = Vec::new();
    for entry in fs::read_dir(&programs_dir)
        .with_context(|| format!("failed to read {}", programs_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        programs.push(stem.to_string());
    }

    programs.sort();
    Ok(programs)
}

fn resolve_selected_programs(
    requested_program: Option<String>,
    available_programs: &[String],
) -> DynResult<Vec<String>> {
    if requested_program.is_none() {
        return Ok(available_programs.to_vec());
    }

    let raw = requested_program.unwrap_or_default();
    let candidate = raw.trim().trim_end_matches(".bin").to_string();
    if candidate.is_empty() {
        bail!("program name cannot be empty");
    }

    if available_programs
        .iter()
        .any(|program| program == &candidate)
    {
        return Ok(vec![candidate]);
    }

    bail!(
        "unknown program `{candidate}`. Available programs: {}",
        available_programs.join(", ")
    )
}

fn deploy_single_program(
    wallet: &super::wallet_support::WalletRuntimeContext,
    program_name: &str,
    binary_path: &Path,
    sequencer_addr: &str,
    spel_bin: &Path,
    json: bool,
) -> DynResult<()> {
    preflight_sequencer_reachability(sequencer_addr)?;

    let mut command = std::process::Command::new(&wallet.wallet_binary);
    command
        .env(
            "NSSA_WALLET_HOME_DIR",
            wallet.wallet_home.as_os_str().to_string_lossy().to_string(),
        )
        .arg("deploy-program")
        .arg(binary_path);

    // Suppress the `$ <cmd>` echo on stdout for --json so the output is a
    // pure JSON object that pipes cleanly into `jq`. RAII guard restores echo
    // state on scope exit so `?` propagation below is safe.
    let _echo_guard = json.then(EchoGuard::suppress);
    let output = run_with_stdin(command, format!("{}\n", wallet_password()))
        .context("failed to execute wallet deploy-program command")?;

    let tx = extract_tx_identifier(&output.stdout, &output.stderr);

    if !output.status.success() {
        let summary = summarize_command_failure(&output.stdout, &output.stderr);
        if json {
            let value = serde_json::json!({
                "status": "failed",
                "program": program_name,
                "error": summary,
            });
            eprintln!("{}", serde_json::to_string(&value)?);
        } else {
            println!("FAIL {program_name} deployment failed");
            println!("  Error: {summary}");
        }
        bail!("deploy failed: {summary}");
    }

    let program_id = extract_program_id(spel_bin, binary_path);

    if json {
        // Omit absent fields entirely rather than emitting `null`. Presence
        // implies a real value; consumers test `has("tx")` / `has("program_id")`
        // instead of branching on null. (LEZ doesn't surface tx receipts yet,
        // so today `tx` is always absent — keeping a guaranteed-null key would
        // train scripts to depend on it.)
        let mut obj = serde_json::Map::new();
        obj.insert(
            "status".to_string(),
            serde_json::Value::String("submitted".to_string()),
        );
        obj.insert(
            "program".to_string(),
            serde_json::Value::String(program_name.to_string()),
        );
        if let Some(tx) = &tx {
            obj.insert("tx".to_string(), serde_json::Value::String(tx.clone()));
        }
        if let Some(id) = &program_id {
            obj.insert(
                "program_id".to_string(),
                serde_json::Value::String(id.clone()),
            );
        }
        let value = serde_json::Value::Object(obj);
        println!("{}", serde_json::to_string(&value)?);
    } else {
        println!("OK  {program_name} submitted");
        println!("  Binary: {}", binary_path.display());
        if let Some(tx) = &tx {
            println!("  tx: {tx}");
        }
        print_program_id_line(&program_id);
        println!(
            "  Note: Program ID is computed locally; on-chain inclusion is not yet verifiable."
        );
    }

    Ok(())
}

/// Wall-clock cap for `spel inspect`. The CLI typically returns in
/// milliseconds; a hung binary should not block the deploy summary.
/// Override with `LOGOS_SCAFFOLD_SPEL_INSPECT_TIMEOUT_MS` if needed.
const SPEL_INSPECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Run the project-vendored `spel inspect <binary>` and return the risc0
/// image ID parsed from its output. Returns `None` on any failure (binary
/// missing, non-zero exit, output unparseable, timeout). Callers print an
/// "unavailable" hint instead of failing the deploy — the deploy itself has
/// already succeeded by the time this runs.
pub(crate) fn extract_program_id(spel_bin: &Path, binary_path: &Path) -> Option<String> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Instant;

    let timeout = std::env::var("LOGOS_SCAFFOLD_SPEL_INSPECT_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
        .unwrap_or(SPEL_INSPECT_TIMEOUT);

    let mut child = Command::new(spel_bin)
        .arg("inspect")
        .arg(binary_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut stdout = String::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_string(&mut stdout);
                }
                for line in stdout.lines() {
                    if let Some((_, after)) = line.split_once(SPEL_IMAGE_ID_PREFIX) {
                        let hex = after.trim();
                        if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                            return Some(hex.to_string());
                        }
                    }
                }
                return None;
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

fn print_program_id_line(program_id: &Option<String>) {
    // Lowercase, snake_case key with 2-space indent so the same awk/grep
    // pattern matches in single-program and multi-program plain output and
    // mirrors the JSON key. Single canonical line per deployed program.
    match program_id {
        Some(id) => println!("  program_id: {id}"),
        None => println!(
            "  program_id: unavailable (run `logos-scaffold setup` to build the vendored spel)"
        ),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DeployResult {
    pub(crate) program: String,
    pub(crate) status: DeployStatus,
    pub(crate) detail: String,
    pub(crate) tx: Option<String>,
    pub(crate) program_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum DeployStatus {
    Submitted,
    Failed,
}

impl DeployStatus {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            DeployStatus::Submitted => "submitted",
            DeployStatus::Failed => "failed",
        }
    }
}

fn is_valid_program_name(program: &str) -> bool {
    !program.is_empty()
        && program.len() <= 128
        && !program.contains('/')
        && !program.contains('\\')
        && !program.contains("..")
}

/// Walk every `GUEST_BIN_SEARCH_ROOTS` once and return a `program -> binary_path`
/// map. Only paths whose components include both a `riscv32im*` target triple
/// and a `release` directory match (debug builds are ignored as a fallback).
/// When multiple matches exist for the same program, the shallowest path wins
/// (preferring the canonical risc0 layout over nested workspace duplicates).
pub(crate) fn discover_program_binaries(
    project_root: &Path,
    programs: &[String],
) -> HashMap<String, PathBuf> {
    let wanted: HashMap<String, &str> = programs
        .iter()
        .filter(|p| is_valid_program_name(p))
        .map(|p| (format!("{p}.bin"), p.as_str()))
        .collect();
    if wanted.is_empty() {
        return HashMap::new();
    }

    let mut release: HashMap<String, (usize, PathBuf)> = HashMap::new();
    let mut debug_fallback: HashMap<String, (usize, PathBuf)> = HashMap::new();

    for root in GUEST_BIN_SEARCH_ROOTS {
        let search_dir = project_root.join(root);
        if !search_dir.exists() {
            continue;
        }
        for entry in WalkDir::new(&search_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            let Some(filename) = path.file_name().and_then(|f| f.to_str()) else {
                continue;
            };
            let Some(&program) = wanted.get(filename) else {
                continue;
            };

            let mut has_riscv32im = false;
            let mut has_release = false;
            let mut depth = 0usize;
            for component in path.components() {
                if let std::path::Component::Normal(name) = component {
                    depth += 1;
                    if let Some(name) = name.to_str() {
                        if name.starts_with("riscv32im") {
                            has_riscv32im = true;
                        }
                        if name == "release" {
                            has_release = true;
                        }
                    }
                }
            }
            if !has_riscv32im {
                continue;
            }

            let bucket = if has_release {
                &mut release
            } else {
                &mut debug_fallback
            };
            match bucket.get(program) {
                Some((existing_depth, _)) if *existing_depth <= depth => {}
                _ => {
                    bucket.insert(program.to_string(), (depth, path.to_path_buf()));
                }
            }
        }
    }

    let mut out = HashMap::new();
    for (program, (_, path)) in release {
        out.insert(program, path);
    }
    for (program, (_, path)) in debug_fallback {
        out.entry(program).or_insert(path);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn lookup(root: &Path, program: &str) -> Option<PathBuf> {
        discover_program_binaries(root, &[program.to_string()]).remove(program)
    }

    #[test]
    fn finds_binary_in_methods_target_layout() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp
            .path()
            .join("methods/target/some_crate/riscv32im-risc0-zkvm-elf/release");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("my_program.bin"), b"fake").unwrap();

        let result = lookup(tmp.path(), "my_program").unwrap();
        assert!(result.ends_with("my_program.bin"));
    }

    /// Regression test for issue #59: a project named anything other than
    /// the scaffold template (`example_program_deployment`) places its guest
    /// binaries under `target/riscv-guest/<project>_methods/<project>_programs/...`.
    /// Before this PR, deploy hardcoded the template name and could never
    /// find these binaries.
    #[test]
    fn finds_binary_for_renamed_project_in_riscv_guest_layout() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp.path().join(
            "target/riscv-guest/my_app_methods/my_app_programs/riscv32im-risc0-zkvm-elf/release",
        );
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("foo.bin"), b"fake").unwrap();

        let result = lookup(tmp.path(), "foo").unwrap();
        assert!(result.ends_with("foo.bin"));
        assert!(result
            .components()
            .any(|c| c.as_os_str() == "my_app_methods"));
    }

    #[test]
    fn returns_none_when_no_search_roots_exist() {
        let tmp = TempDir::new().unwrap();
        assert!(lookup(tmp.path(), "my_program").is_none());
    }

    #[test]
    fn returns_none_when_no_matching_bin() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp
            .path()
            .join("methods/target/some_crate/riscv32im-risc0-zkvm-elf/release");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("other_program.bin"), b"fake").unwrap();

        assert!(lookup(tmp.path(), "my_program").is_none());
    }

    #[test]
    fn ignores_non_riscv32im_paths() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp
            .path()
            .join("methods/target/some_crate/x86_64-unknown-linux/release");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("my_program.bin"), b"fake").unwrap();

        assert!(lookup(tmp.path(), "my_program").is_none());
    }

    #[test]
    fn rejects_path_traversal_in_program_name() {
        let tmp = TempDir::new().unwrap();
        assert!(lookup(tmp.path(), "../etc/passwd").is_none());
        assert!(lookup(tmp.path(), "foo/../bar").is_none());
    }

    #[test]
    fn rejects_overlong_program_name() {
        let tmp = TempDir::new().unwrap();
        let long_name = "a".repeat(200);
        assert!(lookup(tmp.path(), &long_name).is_none());
    }

    #[test]
    fn prefers_release_over_debug() {
        let tmp = TempDir::new().unwrap();
        let debug_dir = tmp
            .path()
            .join("methods/target/some_crate/riscv32im-risc0-zkvm-elf/debug");
        let release_dir = tmp
            .path()
            .join("methods/target/some_crate/riscv32im-risc0-zkvm-elf/release");
        fs::create_dir_all(&debug_dir).unwrap();
        fs::create_dir_all(&release_dir).unwrap();
        fs::write(debug_dir.join("my_program.bin"), b"debug").unwrap();
        fs::write(release_dir.join("my_program.bin"), b"release").unwrap();

        let result = lookup(tmp.path(), "my_program").unwrap();
        assert!(result.components().any(|c| c.as_os_str() == "release"));
    }

    #[test]
    fn falls_back_to_debug_when_only_debug_exists() {
        let tmp = TempDir::new().unwrap();
        let debug_dir = tmp
            .path()
            .join("methods/target/some_crate/riscv32im-risc0-zkvm-elf/debug");
        fs::create_dir_all(&debug_dir).unwrap();
        fs::write(debug_dir.join("my_program.bin"), b"debug").unwrap();

        let result = lookup(tmp.path(), "my_program").unwrap();
        assert!(result.components().any(|c| c.as_os_str() == "debug"));
    }

    #[test]
    fn rejects_substring_only_riscv32im_components() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp.path().join("methods/target/not-riscv32im-foo/release");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("my_program.bin"), b"fake").unwrap();

        assert!(lookup(tmp.path(), "my_program").is_none());
    }

    #[test]
    fn discover_handles_multiple_programs_in_one_walk() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(
            "target/riscv-guest/my_app_methods/my_app_programs/riscv32im-risc0-zkvm-elf/release",
        );
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("foo.bin"), b"a").unwrap();
        fs::write(dir.join("bar.bin"), b"b").unwrap();

        let map = discover_program_binaries(
            tmp.path(),
            &["foo".to_string(), "bar".to_string(), "missing".to_string()],
        );
        assert!(map.get("foo").unwrap().ends_with("foo.bin"));
        assert!(map.get("bar").unwrap().ends_with("bar.bin"));
        assert!(!map.contains_key("missing"));
    }

    /// `summarize_command_failure` only strips trailing whitespace; raw
    /// wallet stderr can carry tabs, embedded newlines, ANSI color
    /// sequences, and other control bytes. The hand-rolled JSON encoder
    /// previously embedded those verbatim, producing invalid JSON per
    /// RFC 8259 (control chars must be `\uXXXX`-escaped). Going through
    /// `serde_json` here is a contract: the renderer's output must always
    /// round-trip through `serde_json::from_str`.
    #[test]
    fn render_deploy_result_json_escapes_control_chars_and_ansi() {
        let nasty = "wallet error\tline2\nbacktrace:\x1b[31m  at \x00 fn\x1b[0m  ".to_string();
        let result = DeployResult {
            program: "alpha".to_string(),
            status: DeployStatus::Failed,
            detail: nasty.clone(),
            tx: None,
            program_id: None,
        };
        let value = render_deploy_result_json(&result);
        let serialized = serde_json::to_string(&value).expect("serialize");
        // The serialized form must parse back as valid JSON…
        let parsed: serde_json::Value =
            serde_json::from_str(&serialized).expect("must round-trip as valid JSON");
        // …with the original raw bytes preserved in the `error` string
        // (serde_json escapes them on the wire and decodes back to the
        // original on parse).
        assert_eq!(
            parsed
                .get("error")
                .and_then(|v| v.as_str())
                .expect("error field"),
            nasty
        );
        assert_eq!(
            parsed.get("status").and_then(|v| v.as_str()),
            Some("failed")
        );
        assert_eq!(
            parsed.get("program").and_then(|v| v.as_str()),
            Some("alpha")
        );
    }
}
