use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context};
use walkdir::WalkDir;

use crate::process::{run_with_stdin, EchoGuard};
use crate::project::load_project;
use crate::DynResult;

use super::deployments::{record_deployments, DeploymentRecord};
use super::wallet_support::{
    default_sequencer_http_url_for_project, extract_tx_identifier, load_wallet_runtime,
    probe_sequencer_reachable, read_default_wallet_address, rpc_get_last_block_id,
    sequencer_connectivity_failure_with_probe, sequencer_unreachable_hint, set_wallet_home_env,
    summarize_command_failure, wallet_password, RpcReachabilityError, WalletRuntimeContext,
};

/// Bytes of a guest's user ELF that fit in one `program_loader` segment
/// account (`program_loader_core::MAX_SEGMENT_DATA_LEN` at LEZ 0.3).
const MAX_SEGMENT_DATA_LEN: usize = 96 * 1024;

/// Roots searched for guest `.bin` artefacts. Three layouts exist: risc0's
/// default workspace layout emits to `target/riscv-guest/...` (what
/// `embed_methods()` produces for the scaffold template), sub-crate builds can
/// land in `methods/target/...`, and the deterministic Docker build
/// (`[build].guest = "docker"`) emits to
/// `target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker/`. Discovery
/// walks all three so renamed projects work regardless of which layout
/// cargo/risc0 chose. The `methods/...` entry is the same project-relative
/// directory that `build.rs` compiles via `crate::constants::METHODS_DIR`;
/// keep them in sync.
const GUEST_BIN_SEARCH_ROOTS: &[&str] = &[
    crate::constants::GUEST_DOCKER_TARGET_DIR,
    "target/riscv-guest",
    "methods/target",
];

pub(crate) fn cmd_deploy(
    program_name: Option<String>,
    program_path: Option<PathBuf>,
    json: bool,
) -> DynResult<()> {
    let project = load_project()?;
    let results = deploy_for_project(&project, program_name, program_path, json)?;

    let failed_count = results
        .iter()
        .filter(|result| matches!(result.status, DeployStatus::Failed))
        .count();
    if failed_count > 0 {
        if results.len() == 1 {
            bail!("deploy failed: {}", results[0].detail);
        }
        bail!("deploy completed with {failed_count} failed program(s)");
    }

    Ok(())
}

/// Deploy guest programs for `project`. Returns one `DeployResult` per
/// attempted program; the caller decides whether failures are fatal. With
/// `json = true` the CLI JSON object is printed and `$ <cmd>` echoes are
/// suppressed; with `json = false` per-program progress lines stream to
/// stdout (API callers should read the returned results, not the output).
///
/// LEZ 0.3 deploys through the `program_loader` builtin: the guest's user
/// ELF is split into segment accounts and a header account points at the
/// chain; the header's account id *is* the program id every later
/// transaction targets. Scaffold creates fresh wallet accounts for the header
/// and segments, runs `wallet program-loader deploy`, and records the result
/// in `.scaffold/state/deployments.json`. The wallet waits for every segment
/// and header transaction to be included before it returns, so a successful
/// deploy is a confirmed one.
pub(crate) fn deploy_for_project(
    project: &crate::model::Project,
    program_name: Option<String>,
    program_path: Option<PathBuf>,
    json: bool,
) -> DynResult<Vec<DeployResult>> {
    let wallet = load_wallet_runtime(project)?;
    let sequencer_addr = wallet
        .sequencer_addr
        .clone()
        .unwrap_or_else(|| default_sequencer_http_url_for_project(project));
    let payer = read_default_wallet_address(&project.root)?.ok_or_else(|| {
        anyhow::anyhow!(
            "deploy needs a funded default wallet to pay the deployment fees.\n\
             Next step: run `logos-scaffold setup` (seeds the default wallet), then \
             `logos-scaffold wallet topup`."
        )
    })?;

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
        preflight_sequencer_reachability(&sequencer_addr)?;
        let _echo_guard = json.then(EchoGuard::suppress);
        let result = deploy_one(
            &wallet,
            &payer,
            &program_name,
            &custom_path,
            &sequencer_addr,
            &|addr| probe_sequencer_reachable(addr),
        );
        record_successes(project, std::slice::from_ref(&result))?;
        if json {
            println!(
                "{}",
                serde_json::to_string(&render_deploy_result_json(&result))?
            );
        } else {
            print_result_block(&result, Some(&custom_path));
        }
        return Ok(vec![result]);
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

    // The reachability probe asks a network-level question whose answer is the
    // same for every program in this run, so cache it: if the sequencer dies
    // mid-deploy and several programs fail in a row, they issue at most one
    // probe between them and cannot disagree with each other.
    let cached_reachable: std::cell::Cell<Option<bool>> = std::cell::Cell::new(None);
    let probe_once = |addr: &str| match cached_reachable.get() {
        Some(reachable) => reachable,
        None => {
            let reachable = probe_sequencer_reachable(addr);
            cached_reachable.set(Some(reachable));
            reachable
        }
    };

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
            results.push(DeployResult::failed(
                program,
                "missing program binary".to_string(),
            ));
            continue;
        };

        if !json {
            println!("Deploying {program} ...");
        }
        let result = deploy_one(
            &wallet,
            &payer,
            &program,
            &binary_path,
            &sequencer_addr,
            &probe_once,
        );
        if !json {
            print_result_block(&result, None);
        }
        results.push(result);
    }

    record_successes(project, &results)?;

    let success_count = results
        .iter()
        .filter(|result| matches!(result.status, DeployStatus::Deployed))
        .count();
    let failed_count = results
        .iter()
        .filter(|result| matches!(result.status, DeployStatus::Failed))
        .count();

    if json {
        // Single-line JSON object on stdout. One entry per attempted
        // program; absent fields are omitted, not nulled, matching the
        // single-program --program-path contract. Failed entries carry
        // `error` instead of `program_id`.
        let entries: Vec<serde_json::Value> =
            results.iter().map(render_deploy_result_json).collect();
        let value = serde_json::json!({ "deploys": entries });
        println!("{}", serde_json::to_string(&value)?);
    } else {
        println!("Summary:");
        println!("  Deployed: {success_count}");
        println!("  Failed: {failed_count}");
        println!("  Results:");
        for result in &results {
            println!("    {}: {}", result.program, result.status.label());
            println!("      {}", result.detail);
        }
        if success_count > 0 {
            println!(
                "Program ids are recorded in {}",
                super::deployments::deployments_path(&project.root).display()
            );
        }
    }

    Ok(results)
}

fn record_successes(project: &crate::model::Project, results: &[DeployResult]) -> DynResult<()> {
    let records: Vec<DeploymentRecord> = results
        .iter()
        .filter_map(|result| {
            Some(DeploymentRecord {
                program: result.program.clone(),
                program_id: result.program_id.clone()?,
                elf_sha256: result.elf_sha256.clone(),
                payer: result.payer.clone(),
            })
        })
        .collect();
    if records.is_empty() {
        return Ok(());
    }
    record_deployments(&project.root, &records)
}

fn print_result_block(result: &DeployResult, binary_path: Option<&Path>) {
    match result.status {
        DeployStatus::Deployed => {
            println!("OK  {} deployed", result.program);
            if let Some(path) = binary_path {
                println!("  Binary: {}", path.display());
            }
            if let Some(id) = &result.program_id {
                println!("  program_id: {id}");
            }
            if let Some(tx) = &result.tx {
                println!("  tx: {tx}");
            }
        }
        DeployStatus::Failed => {
            println!("FAIL {} deployment failed", result.program);
            println!("  Error: {}", result.detail);
        }
    }
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
    if let Some(sha) = &result.elf_sha256 {
        obj.insert(
            "elf_sha256".to_string(),
            serde_json::Value::String(sha.clone()),
        );
    }
    match result.status {
        DeployStatus::Deployed => {
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

/// Deploy one guest binary through `wallet program-loader deploy`. Never
/// returns `Err`: every failure becomes a `Failed` result so a multi-program
/// deploy reports each program.
fn deploy_one(
    wallet: &WalletRuntimeContext,
    payer: &str,
    program: &str,
    binary_path: &Path,
    sequencer_addr: &str,
    probe: &dyn Fn(&str) -> bool,
) -> DeployResult {
    let bytes = match fs::read(binary_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return DeployResult::failed(
                program.to_string(),
                format!("failed to read {}: {err}", binary_path.display()),
            )
        }
    };
    let elf_sha256 = Some(sha256_hex(&bytes));
    let segment_count = match program_segment_count(&bytes) {
        Ok(count) => count,
        Err(err) => {
            let mut result = DeployResult::failed(
                program.to_string(),
                format!(
                    "{} is not a risc0 program binary: {err}",
                    binary_path.display()
                ),
            );
            result.elf_sha256 = elf_sha256;
            return result;
        }
    };

    // One header plus one account per segment, all fresh: the loader only
    // writes into accounts whose `program_loader` shard is still empty.
    let mut accounts = Vec::with_capacity(segment_count + 1);
    for _ in 0..=segment_count {
        match new_public_account(wallet) {
            Ok(account) => accounts.push(account),
            Err(err) => {
                let mut result = DeployResult::failed(
                    program.to_string(),
                    format!("failed to create program accounts: {err}"),
                );
                result.elf_sha256 = elf_sha256;
                return result;
            }
        }
    }
    let header = accounts.remove(0);

    let mut command = Command::new(&wallet.wallet_binary);
    set_wallet_home_env(&mut command, &wallet.wallet_home);
    command
        .arg("program-loader")
        .arg("deploy")
        .arg("--elf")
        .arg(binary_path)
        .arg("--header")
        .arg(&header)
        .arg("--segments")
        .args(&accounts)
        .arg("--payer")
        .arg(payer);

    let output = match run_with_stdin(command, format!("{}\n", wallet_password())) {
        Ok(output) => output,
        Err(err) => {
            let mut result = DeployResult::failed(
                program.to_string(),
                format!("wallet command invocation failed: {err}"),
            );
            result.elf_sha256 = elf_sha256;
            return result;
        }
    };
    let tx = extract_last_tx_identifier(&output.stdout, &output.stderr);

    if !output.status.success() {
        let summary = summarize_command_failure(&output.stdout, &output.stderr);
        let combined = format!("{}\n{}", output.stdout, output.stderr);
        let detail = if sequencer_connectivity_failure_with_probe(&combined, sequencer_addr, probe)
        {
            format!(
                "{summary}; sequencer connectivity failure. {}",
                sequencer_unreachable_hint(sequencer_addr)
            )
        } else if combined.contains("Incorrect fee") {
            format!(
                "{summary}. The payer {payer} cannot cover the deployment fees; run \
                 `logos-scaffold wallet topup` and retry."
            )
        } else {
            summary
        };
        let mut result = DeployResult::failed(program.to_string(), detail);
        result.tx = tx;
        result.elf_sha256 = elf_sha256;
        return result;
    }

    let program_id = parse_deployed_header(&output.stdout)
        .or_else(|| header.strip_prefix("Public/").map(ToString::to_string));
    DeployResult {
        program: program.to_string(),
        status: DeployStatus::Deployed,
        detail: format!(
            "deployed through program_loader ({segment_count} segment{}), paid by {payer}",
            if segment_count == 1 { "" } else { "s" }
        ),
        tx,
        program_id,
        elf_sha256,
        payer: Some(payer.to_string()),
    }
}

/// Number of `program_loader` segments `wallet program-loader deploy` will
/// split `binary` into. Parses the risc0 `ProgramBinary` container (`R0BF`
/// magic, format version 1, length-prefixed header, then the length-prefixed
/// user ELF) — only the user ELF is uploaded, the kernel is the protocol's.
pub(crate) fn program_segment_count(binary: &[u8]) -> DynResult<usize> {
    fn read_u32(bytes: &[u8], at: usize) -> DynResult<u32> {
        let slice = bytes
            .get(at..at + 4)
            .ok_or_else(|| anyhow::anyhow!("truncated at byte {at}"))?;
        Ok(u32::from_le_bytes(slice.try_into().expect("4 bytes")))
    }
    if binary.get(..4) != Some(b"R0BF".as_slice()) {
        bail!("missing R0BF magic");
    }
    let version = read_u32(binary, 4)?;
    if version != 1 {
        bail!("unsupported ProgramBinary format version {version}");
    }
    let header_len = read_u32(binary, 8)? as usize;
    let user_len_at = 12usize
        .checked_add(header_len)
        .ok_or_else(|| anyhow::anyhow!("header length overflows"))?;
    let user_len = read_u32(binary, user_len_at)? as usize;
    if user_len == 0 || binary.len() < user_len_at + 4 + user_len {
        bail!("user ELF length {user_len} does not fit the file");
    }
    Ok(user_len.div_ceil(MAX_SEGMENT_DATA_LEN))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `wallet account new public` → the new `Public/<id>`.
fn new_public_account(wallet: &WalletRuntimeContext) -> DynResult<String> {
    let mut command = Command::new(&wallet.wallet_binary);
    set_wallet_home_env(&mut command, &wallet.wallet_home);
    command.arg("account").arg("new").arg("public");
    let _quiet = EchoGuard::suppress();
    let output = run_with_stdin(command, format!("{}\n", wallet_password()))?;
    if !output.status.success() {
        bail!(
            "{}",
            summarize_command_failure(&output.stdout, &output.stderr)
        );
    }
    parse_generated_public_account(&output.stdout).ok_or_else(|| {
        anyhow::anyhow!(
            "could not find the new account id in wallet output: {}",
            summarize_command_failure(&output.stdout, &output.stderr)
        )
    })
}

/// Parse `Generated new account with account_id Public/<id> at path …`.
pub(crate) fn parse_generated_public_account(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let rest = line.split("account_id ").nth(1)?;
        let token = rest.split_whitespace().next()?;
        token.starts_with("Public/").then(|| token.to_string())
    })
}

/// Parse `Program deployed. Header account id: <id>`.
pub(crate) fn parse_deployed_header(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let rest = line.split("Header account id:").nth(1)?;
        let id = rest.trim();
        (!id.is_empty()).then(|| id.to_string())
    })
}

/// A deploy is several transactions (segments, then the header); report the
/// last one — the header, whose inclusion completes the deployment.
fn extract_last_tx_identifier(stdout: &str, stderr: &str) -> Option<String> {
    let mut last = None;
    for line in stdout.lines().chain(stderr.lines()) {
        if let Some(rest) = line.trim().strip_prefix("Transaction hash is ") {
            last = Some(rest.trim().to_string());
        }
    }
    last.or_else(|| extract_tx_identifier(stdout, stderr))
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
            // stderr, not stdout: a `println!` here would interleave a
            // non-JSON warning into a `--json` stdout stream and break `jq`.
            eprintln!(
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

/// Outcome of one program deployment attempt.
#[derive(Clone, Debug, serde::Serialize)]
pub struct DeployResult {
    pub program: String,
    pub status: DeployStatus,
    pub detail: String,
    /// Hash of the last transaction the wallet reported (the header write
    /// for a successful deploy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx: Option<String>,
    /// The program's on-chain id: the `program_loader` header account (base58)
    /// that transactions name as their program.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program_id: Option<String>,
    /// SHA-256 of the deployed `.bin`, for reproducibility checks: the header
    /// account is fresh on every deploy, the binary hash is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elf_sha256: Option<String>,
    /// Account that paid the deployment fees.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
}

impl DeployResult {
    fn failed(program: String, detail: String) -> Self {
        Self {
            program,
            status: DeployStatus::Failed,
            detail,
            tx: None,
            program_id: None,
            elf_sha256: None,
            payer: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeployStatus {
    /// Every segment and the header transaction were included in a block.
    Deployed,
    Failed,
}

impl DeployStatus {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            DeployStatus::Deployed => "deployed",
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
/// map. Only paths whose components include a `riscv32im*` target triple match.
///
/// Ranking, highest first:
///
/// 1. a `docker` component *directly under* the `riscv32im*` one — the
///    deterministic `cargo risczero build` output, whose layout is always
///    `<target-dir>/riscv32im-risc0-zkvm-elf/docker/`;
/// 2. a `release` component — the host-toolchain `embed_methods()` output;
/// 3. anything else (a `debug` build) as a last-resort fallback.
///
/// Docker outranks release because a `docker/` artefact only exists when a
/// deterministic build produced it, and `lgs build` deletes that tree whenever
/// it runs in `local` mode — so if both are present, docker is the one the
/// last build meant (scaffold#259). The adjacency requirement keeps a guest
/// crate (or any other directory) that happens to be named `docker` from
/// being promoted: `embed_methods()` puts package names *above* the target
/// triple, never below it. Within a rank the shallowest path wins, preferring
/// the canonical risc0 layout over nested workspace duplicates.
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

    let mut docker: HashMap<String, (usize, PathBuf)> = HashMap::new();
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
            let mut has_docker = false;
            let mut depth = 0usize;
            let mut prev_was_riscv32im = false;
            for component in path.components() {
                if let std::path::Component::Normal(name) = component {
                    depth += 1;
                    let name = name.to_str().unwrap_or_default();
                    let is_riscv32im = name.starts_with("riscv32im");
                    has_riscv32im |= is_riscv32im;
                    has_release |= name == "release";
                    // Only the segment immediately below the target triple —
                    // see the ranking notes on this function.
                    has_docker |= name == "docker" && prev_was_riscv32im;
                    prev_was_riscv32im = is_riscv32im;
                }
            }
            if !has_riscv32im {
                continue;
            }

            let bucket = if has_docker {
                &mut docker
            } else if has_release {
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
    for (program, (_, path)) in docker {
        out.insert(program, path);
    }
    for (program, (_, path)) in release {
        out.entry(program).or_insert(path);
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

    /// A minimal risc0 `ProgramBinary`: magic, version, a header, the user
    /// ELF of `user_len` bytes, then a kernel blob.
    fn fake_program_binary(user_len: usize) -> Vec<u8> {
        let mut bytes = b"R0BF".to_vec();
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        let header = [9_u8; 5];
        bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&(user_len as u32).to_le_bytes());
        bytes.extend(std::iter::repeat_n(0x7f_u8, user_len));
        bytes.extend_from_slice(b"kernel");
        bytes
    }

    #[test]
    fn segment_count_follows_the_user_elf_in_96_kib_chunks() {
        assert_eq!(program_segment_count(&fake_program_binary(1)).unwrap(), 1);
        assert_eq!(
            program_segment_count(&fake_program_binary(MAX_SEGMENT_DATA_LEN)).unwrap(),
            1
        );
        assert_eq!(
            program_segment_count(&fake_program_binary(MAX_SEGMENT_DATA_LEN + 1)).unwrap(),
            2
        );
        assert_eq!(
            program_segment_count(&fake_program_binary(3 * MAX_SEGMENT_DATA_LEN)).unwrap(),
            3
        );
    }

    #[test]
    fn segment_count_rejects_non_program_binaries() {
        assert!(program_segment_count(b"\x7fELF plain elf").is_err());
        let mut truncated = fake_program_binary(100);
        truncated.truncate(30);
        assert!(program_segment_count(&truncated).is_err());
        let mut wrong_version = fake_program_binary(10);
        wrong_version[4] = 2;
        assert!(program_segment_count(&wrong_version).is_err());
    }

    #[test]
    fn parses_wallet_account_and_deploy_output() {
        let generated = "Generated new account with account_id \
                         Public/BzdBoL4JRa5M873cuWb9rbYgASr1pXyaAZ1YW9ertWH9 at path /3\n\
                         With pk abcd\n";
        assert_eq!(
            parse_generated_public_account(generated).as_deref(),
            Some("Public/BzdBoL4JRa5M873cuWb9rbYgASr1pXyaAZ1YW9ertWH9")
        );
        assert_eq!(parse_generated_public_account("nothing here"), None);

        let deployed = "Transaction hash is aa11\nTransaction is included in block 4\n\
                        Transaction hash is bb22\nTransaction is included in block 5\n\
                        Program deployed. Header account id: 6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV\n";
        assert_eq!(
            parse_deployed_header(deployed).as_deref(),
            Some("6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV")
        );
        assert_eq!(
            extract_last_tx_identifier(deployed, "").as_deref(),
            Some("bb22")
        );
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

    /// The deterministic build's artefact must win over a host-toolchain
    /// `release/` one: they have different bytes, and therefore different
    /// `program_id`s, so silently picking either would make deploy
    /// unpredictable (scaffold#259).
    #[test]
    fn prefers_deterministic_docker_output_over_local_release() {
        let tmp = TempDir::new().unwrap();
        let release_dir = tmp.path().join(
            "target/riscv-guest/my_app_methods/my_app_programs/riscv32im-risc0-zkvm-elf/release",
        );
        let docker_dir = tmp
            .path()
            .join(crate::constants::GUEST_DOCKER_TARGET_DIR)
            .join("riscv32im-risc0-zkvm-elf/docker");
        fs::create_dir_all(&release_dir).unwrap();
        fs::create_dir_all(&docker_dir).unwrap();
        fs::write(release_dir.join("my_program.bin"), b"local").unwrap();
        fs::write(docker_dir.join("my_program.bin"), b"deterministic").unwrap();

        let result = lookup(tmp.path(), "my_program").unwrap();
        assert!(
            result.components().any(|c| c.as_os_str() == "docker"),
            "expected the docker artefact, got {}",
            result.display()
        );
    }

    /// A guest package named `docker` lives *above* the target triple in the
    /// `embed_methods()` layout, so it must not be mistaken for deterministic
    /// output and promoted over a real one.
    #[test]
    fn docker_named_crate_is_not_mistaken_for_deterministic_output() {
        let tmp = TempDir::new().unwrap();
        let release_dir = tmp
            .path()
            .join("target/riscv-guest/docker/docker/riscv32im-risc0-zkvm-elf/release");
        fs::create_dir_all(&release_dir).unwrap();
        fs::write(release_dir.join("my_program.bin"), b"local").unwrap();

        let docker_dir = tmp
            .path()
            .join(crate::constants::GUEST_DOCKER_TARGET_DIR)
            .join("riscv32im-risc0-zkvm-elf/docker");
        fs::create_dir_all(&docker_dir).unwrap();
        fs::write(docker_dir.join("my_program.bin"), b"deterministic").unwrap();

        let result = lookup(tmp.path(), "my_program").unwrap();
        assert_eq!(
            fs::read(&result).unwrap(),
            b"deterministic",
            "a crate named `docker` must not outrank the real deterministic \
             artefact; got {}",
            result.display()
        );
    }

    #[test]
    fn finds_binary_in_deterministic_docker_layout() {
        let tmp = TempDir::new().unwrap();
        let docker_dir = tmp
            .path()
            .join(crate::constants::GUEST_DOCKER_TARGET_DIR)
            .join("riscv32im-risc0-zkvm-elf/docker");
        fs::create_dir_all(&docker_dir).unwrap();
        fs::write(docker_dir.join("my_program.bin"), b"deterministic").unwrap();

        let result = lookup(tmp.path(), "my_program").unwrap();
        assert!(result.ends_with("my_program.bin"));
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
            elf_sha256: None,
            payer: None,
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
