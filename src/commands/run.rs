use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{bail, Context};
use notify::{RecursiveMode, Watcher};

use crate::commands::build::cmd_build_shortcut;
use crate::commands::deploy::{
    cmd_deploy, discover_deployable_programs, discover_program_binaries, extract_program_id,
};
use crate::commands::idl::build_idl_for_current_project;
use crate::commands::localnet::{
    build_localnet_status_for_project, cmd_localnet, cmd_localnet_reset, LocalnetAction,
};
use crate::commands::run_state::{
    compute_program_hashes, current_localnet_pid, deploy_can_be_skipped, load_state, save_state,
    RunDeployState,
};
use crate::commands::setup::ensure_default_wallet_seeded;
use crate::commands::wallet::{cmd_wallet_topup_inner, TopupOutcome};
use crate::constants::{DEFAULT_RUN_LOCALNET_TIMEOUT_SEC, SPEL_BIN_REL_PATH};
use crate::model::{LocalnetOwnership, Project, RunProfile};
use crate::project::{load_project, resolve_repo_path, run_in_project_dir};
use crate::state::prepare_wallet_home;
use crate::DynResult;

/// Debounce window for the watch loop. Filesystem events from a single
/// editor save typically arrive in a flurry; sleep this long after the
/// first event before re-running so we coalesce them.
const WATCH_DEBOUNCE_MS: u64 = 500;

/// All knobs that control a `lgs run` invocation. Built by `cli.rs` from
/// the parsed `RunArgs` (with conflicting-flag resolution into `Option<bool>`)
/// and consumed by `cmd_run`. Grouping the fields together prevents the
/// positional-swap class of bug.
#[derive(Clone, Debug, Default)]
pub(crate) struct RunInvocation {
    pub(crate) profile: Option<String>,
    pub(crate) reset: Option<bool>,
    pub(crate) post_deploy_override: Option<Vec<String>>,
    pub(crate) localnet_timeout_sec: Option<u64>,
    pub(crate) watch: bool,
}

pub(crate) fn cmd_run(inv: RunInvocation) -> DynResult<()> {
    let project = load_project()?;
    let resolved = project.config.run.resolve_profile(inv.profile.as_deref())?;
    if let Some(name) = inv.profile.as_deref() {
        println!("Using [run.profiles.{name}]");
    } else if let Some(name) = project.config.run.default_profile.as_deref() {
        println!("Using [run.profiles.{name}] (default_profile)");
    }
    let hooks = inv
        .post_deploy_override
        .unwrap_or_else(|| resolved.post_deploy.clone());
    let localnet_timeout_sec = inv
        .localnet_timeout_sec
        .unwrap_or(DEFAULT_RUN_LOCALNET_TIMEOUT_SEC);

    let mut params = PipelineParams {
        resolved: resolved.clone(),
        hooks,
        reset_override: inv.reset,
        localnet_timeout_sec,
    };

    // Anchor the pipeline at the discovered project root. Otherwise commands
    // that resolve paths relative to cwd (`cmd_build_shortcut`,
    // `build_idl_for_current_project`, etc.) would build/deploy from whichever
    // subdirectory the user invoked `lgs run` in.
    run_in_project_dir(Some(&project.root), || {
        run_pipeline_once(&project, &params)?;

        if inv.watch {
            // Subsequent iterations share the same hook/profile selection but
            // never reset the localnet again — that would clobber the state
            // hook code is verifying.
            params.reset_override = Some(false);
            watch_loop(&project, &params)?;
        }

        Ok(())
    })
}

#[derive(Clone)]
struct PipelineParams {
    resolved: RunProfile,
    hooks: Vec<String>,
    reset_override: Option<bool>,
    localnet_timeout_sec: u64,
}

fn run_pipeline_once(project: &Project, params: &PipelineParams) -> DynResult<()> {
    let has_hooks = !params.hooks.is_empty();
    // Steps: build, build idl, localnet, topup, deploy, [+1 if hooks]
    let total_steps: u32 = if has_hooks { 6 } else { 5 };
    let effective_reset = params.reset_override.unwrap_or(params.resolved.reset);

    // Surface destructive intent up front when reset comes from config
    // (not from the CLI flag). With --reset the user already typed the
    // word, so re-stating it is noise; with `reset = true` in scaffold.toml
    // they may not realize step 3 will wipe rocksdb + wallet, so warn
    // before step 1 instead of after the build has run.
    if effective_reset && params.reset_override.is_none() {
        eprintln!(
            "warning: scaffold.toml requested reset = true; step 3 will wipe sequencer state + wallet. Pass --no-reset to override."
        );
    }

    // Step 1: Build (chains setup internally)
    println!("[1/{total_steps}] Building...");
    cmd_build_shortcut(None, false)?;

    // Step 2: Build IDL (no-op for non-lez-framework projects)
    println!("[2/{total_steps}] Building IDL...");
    build_idl_for_current_project()?;

    // Step 3: Reset OR ensure localnet.
    if effective_reset {
        println!("[3/{total_steps}] Resetting localnet (wipes sequencer + wallet)...");
        reset_for_run(project, params.localnet_timeout_sec)?;
        // A reset wipes on-chain state, so any prior deploy is gone:
        // the next deploy must run regardless of hash equality. Tolerate
        // NotFound (no prior run); surface anything else.
        let state_file = project.root.join(".scaffold/state/run_deploy.json");
        match std::fs::remove_file(&state_file) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("clear stale deploy state at {}", state_file.display())
                });
            }
        }
    } else {
        println!("[3/{total_steps}] Ensuring localnet...");
        ensure_localnet(project, params.localnet_timeout_sec)?;
    }

    // Step 4: Wallet topup
    println!("[4/{total_steps}] Topping up wallet...");
    let outcome = cmd_wallet_topup_inner(project, None, false)?;
    if let TopupOutcome::ConfirmationTimeout { message } = outcome {
        bail!(
            "{message}\n\
             Run aborted before deploy to avoid deploying with uncertain funding.\n\
             Hint: retry `logos-scaffold run` or run `logos-scaffold wallet topup` manually."
        );
    }

    // Step 5: Deploy (idempotent: skip when guest .bin + IDL + deploy
    // config hashes match the prior deploy AND the sequencer is the same
    // instance that received it. A `lgs localnet stop && start` cycle
    // changes the sequencer PID and wipes on-chain state, so PID equality
    // is the gate that prevents stale-deploy false positives. To force a
    // re-deploy without restarting localnet, use `--reset` (which also
    // clears the cache) or delete `.scaffold/state/run_deploy.json`
    // manually.
    let current_hashes = compute_program_hashes(project)?;
    let current_pid = current_localnet_pid(project);
    let prior = load_state(project);
    let deploy_skipped = if deploy_can_be_skipped(&current_hashes, current_pid, &prior) {
        println!(
            "[5/{total_steps}] Deploy skipped (guest binaries + IDL + config + sequencer unchanged; pass `--reset` to wipe and re-deploy, or delete `.scaffold/state/run_deploy.json` to force a re-deploy without a wipe)"
        );
        true
    } else {
        println!("[5/{total_steps}] Deploying programs...");
        cmd_deploy(None, None, false)?;
        save_state(
            project,
            &RunDeployState {
                program_hashes: current_hashes,
                localnet_pid: current_pid,
            },
        )?;
        false
    };

    // Collect deployed-program metadata for hook env injection regardless
    // of whether deploy ran or was skipped — hooks address programs by
    // name and shouldn't have to care about cache state.
    // `extract_program_id` shells out to `spel inspect` once per program
    // here so the per-hook loop doesn't multiply latency by hook count.
    let deployed = collect_deployed_programs(project, deploy_skipped)?;

    // Step 6: Post-deploy hooks (or summary)
    if has_hooks {
        let n = params.hooks.len();
        println!("[6/{total_steps}] Running {n} post-deploy hook(s)...");
        check_env_var_suffix_collisions(&deployed.programs)?;
        warn_on_rewritten_program_names(&deployed.programs);
        for (i, hook) in params.hooks.iter().enumerate() {
            println!("===> post_deploy[{}/{n}]: {hook}", i + 1);
            run_post_deploy_hook(project, hook, &deployed)?;
            println!("<=== post_deploy[{}/{n}] OK", i + 1);
        }
    } else {
        print_deploy_summary(project)?;
    }

    Ok(())
}

fn reset_for_run(project: &Project, verify_timeout_sec: u64) -> DynResult<()> {
    let lez = resolve_repo_path(project, &project.config.lez, "lez")?;
    let state_path = project.root.join(".scaffold/state/localnet.state");
    let log_path = project.root.join(".scaffold/logs/sequencer.log");
    let localnet_addr = format!("127.0.0.1:{}", project.config.localnet.port);
    cmd_localnet_reset(
        project,
        &lez,
        &state_path,
        &log_path,
        &localnet_addr,
        false, // dry_run — actually perform the reset
        true,  // yes — non-interactive; run is the user-initiated action that authorizes it
        true,  // reset_wallet — full wipe; reseed_after_wipe re-seeds below
        verify_timeout_sec,
    )?;
    // The wipe deleted the wallet directory and state file; the next pipeline
    // step (topup) would fail without a re-seed. Recover by calling the same
    // primitives `cmd_setup` invokes when the wallet is absent. If the
    // re-seed itself fails, stop the sequencer we just started so we don't
    // strand the project in a half-wiped state with a running daemon.
    if let Err(err) = reseed_after_wipe(project) {
        let _ = cmd_localnet(LocalnetAction::Stop);
        return Err(err.context(
            "post-reset wallet re-seed failed; sequencer was stopped to avoid leaving a half-wiped project",
        ));
    }
    Ok(())
}

/// Re-seed the project's default wallet after `cmd_localnet_reset` wiped
/// it. Reuses the same primitives `cmd_setup` calls so the resulting
/// `wallet.state` is byte-equivalent to a fresh `lgs setup`. Extracted as
/// its own helper so the byte-equivalence test can drive it directly
/// without booting a real sequencer.
fn watch_loop(project: &Project, params: &PipelineParams) -> DynResult<()> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .context("create filesystem watcher")?;
    watcher
        .watch(&project.root, RecursiveMode::Recursive)
        .context("watch project root")?;

    // The IDL build writes JSON files into framework.idl.path on every
    // iteration. Without ignoring that directory, those writes fire
    // their own notify events → infinite loop. Resolve once before
    // entering the loop. Use the project-relative form so we match
    // both canonical and non-canonical event paths.
    let idl_rel = PathBuf::from(&project.config.framework.idl.path);
    let watch_ctx = WatchIgnore { idl_rel };

    println!();
    println!(
        "===> watching {} for changes (Ctrl-C to exit)",
        project.root.display()
    );

    loop {
        let event = match rx.recv() {
            Ok(Ok(ev)) => ev,
            Ok(Err(err)) => {
                eprintln!("watch error: {err}");
                continue;
            }
            Err(_) => {
                eprintln!("===> watcher channel disconnected; exiting watch loop");
                break;
            }
        };
        if !is_watched_event(project, &watch_ctx, &event) {
            continue;
        }
        // Debounce: sleep then drain the rest of the burst.
        std::thread::sleep(Duration::from_millis(WATCH_DEBOUNCE_MS));
        while rx.try_recv().is_ok() {}
        println!();
        println!("===> change detected, re-running pipeline");
        if let Err(err) = run_pipeline_once(project, params) {
            eprintln!("pipeline failed: {err:#}");
            eprintln!("===> waiting for next change");
        }
    }

    Ok(())
}

struct WatchIgnore {
    idl_rel: PathBuf,
}

fn is_watched_event(project: &Project, ctx: &WatchIgnore, event: &notify::Event) -> bool {
    for path in &event.paths {
        if !is_ignored_path(&project.root, ctx, path) {
            return true;
        }
    }
    false
}

fn is_ignored_path(project_root: &Path, ctx: &WatchIgnore, path: &Path) -> bool {
    // `notify` may emit canonical (symlinks resolved) or non-canonical paths
    // depending on the platform and how the project root was registered.
    // Try both forms so we never silently *ignore* a project edit just
    // because the OS canonicalized one side and not the other. Fail-open:
    // a path we can't classify is treated as "watched" so the worst case
    // is a spurious re-run, never a missed edit.
    let canonical_root = project_root.canonicalize().ok();
    let rel = path.strip_prefix(project_root).ok().or_else(|| {
        canonical_root
            .as_deref()
            .and_then(|r| path.strip_prefix(r).ok())
    });
    let Some(rel) = rel else {
        // Path doesn't belong to the project tree under either form. Ignore
        // — this is a notify event from outside the watched directory.
        return true;
    };
    for component in rel.components() {
        let s = component.as_os_str().to_string_lossy();
        if matches!(s.as_ref(), ".scaffold" | "target" | ".git") {
            return true;
        }
    }
    if rel.starts_with(&ctx.idl_rel) {
        return true;
    }
    false
}

fn reseed_after_wipe(project: &Project) -> DynResult<()> {
    let lez = resolve_repo_path(project, &project.config.lez, "lez")?;
    let wallet_home = project.root.join(&project.config.wallet_home_dir);
    prepare_wallet_home(&lez, &wallet_home)?;
    ensure_default_wallet_seeded(&project.root, &wallet_home)
}

/// Per-program metadata exposed to post-deploy hooks via env vars.
/// `program_id` may be `None` when `spel inspect` fails (missing vendored
/// binary, unreadable ELF).
#[derive(Clone, Debug)]
pub(crate) struct DeployedProgram {
    pub(crate) name: String,
    pub(crate) program_id: Option<String>,
    pub(crate) binary_path: PathBuf,
}

/// Run-level outcome of step 5 plus per-program metadata. `skipped` is
/// run-level (the cache either short-circuited the whole deploy or not),
/// so it lives here rather than on each `DeployedProgram`.
#[derive(Clone, Debug, Default)]
pub(crate) struct DeployedPrograms {
    pub(crate) skipped: bool,
    pub(crate) programs: Vec<DeployedProgram>,
}

/// Errors from `discover_deployable_programs` or `resolve_repo_path` are
/// propagated rather than swallowed: an unreadable bin dir or a
/// misconfigured `[repos.spel]` would otherwise silently strip
/// `SCAFFOLD_PROGRAMS` and all indexed env vars from hooks. The
/// missing-bin-dir case stays a successful empty result, because step 1
/// (build) is the layer that validates project layout.
fn collect_deployed_programs(project: &Project, skipped: bool) -> DynResult<DeployedPrograms> {
    let programs_dir = project.root.join("methods/guest/src/bin");
    if !programs_dir.exists() {
        return Ok(DeployedPrograms {
            skipped,
            programs: Vec::new(),
        });
    }
    let programs = discover_deployable_programs(&project.root)
        .context("failed to discover deployable programs for run post-deploy env")?;
    let binaries = discover_program_binaries(&project.root, &programs);
    let spel_repo = resolve_repo_path(project, &project.config.spel, "spel")
        .context("failed to resolve spel repo path for run post-deploy env")?;
    let spel_bin = spel_repo.join(SPEL_BIN_REL_PATH);

    // Warn early when the vendored spel binary has not been built yet.
    // extract_program_id() spawns spel_bin and returns None on any failure,
    // so a missing binary silently leaves SCAFFOLD_PROGRAM_ID unset — which
    // causes post-deploy hooks to fail in confusing ways (issue #160).
    if !spel_bin.is_file() {
        eprintln!(
            "warning: vendored spel binary not found at {}.              SCAFFOLD_PROGRAM_ID will be unset for all programs.              Run `lgs setup` to build spel.",
            spel_bin.display()
        );
    }

    let mut out = Vec::new();
    for stem in programs {
        let Some(bin_path) = binaries.get(&stem).cloned() else {
            continue;
        };
        let program_id = extract_program_id(&spel_bin, &bin_path);
        out.push(DeployedProgram {
            name: stem,
            program_id,
            binary_path: bin_path,
        });
    }
    Ok(DeployedPrograms {
        skipped,
        programs: out,
    })
}

/// Bail when two raw program names sanitize to the same env-var suffix
/// (e.g. `my-program.rs` and `my_program.rs` both map to `my_program`).
/// Without this, the second `cmd.env()` would silently shadow the first
/// and hooks would see the wrong program_id/binary_path for one of them.
fn check_env_var_suffix_collisions(programs: &[DeployedProgram]) -> DynResult<()> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for p in programs {
        groups
            .entry(env_var_suffix(&p.name))
            .or_default()
            .push(p.name.as_str());
    }
    let collisions: Vec<(String, Vec<&str>)> = groups
        .into_iter()
        .filter(|(_, raws)| raws.len() > 1)
        .collect();
    if collisions.is_empty() {
        return Ok(());
    }
    let mut msg = String::from(
        "post-deploy env vars: program names collide after sanitization to [A-Za-z0-9_]:",
    );
    for (suffix, raws) in collisions {
        msg.push_str(&format!("\n  {} -> {}", raws.join(", "), suffix));
    }
    msg.push_str(
        "\nRename one of the program source files in methods/guest/src/bin/ to disambiguate.",
    );
    bail!("{msg}")
}

/// Replace any character that isn't `[A-Za-z0-9_]` with `_` so the result
/// is a legal POSIX env var name suffix. Program names from
/// `methods/guest/src/bin/*.rs` are typically already snake_case, but
/// nothing prevents `my-program.rs` from existing — sanitize defensively.
fn env_var_suffix(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// When a program filename contains characters that get rewritten in env
/// var names (anything outside `[A-Za-z0-9_]`), the indexed forms
/// `SCAFFOLD_PROGRAM_ID_<name>` use the rewritten suffix while
/// `$SCAFFOLD_PROGRAMS` round-trips the raw filename. A hook that interpolates
/// `$SCAFFOLD_PROGRAM_ID_my-program` would parse as
/// `${SCAFFOLD_PROGRAM_ID_my}-program` and silently produce wrong output.
/// Print one line per rewritten name so the user sees the actual var name to
/// reference before the hook runs.
fn warn_on_rewritten_program_names(deployed: &[DeployedProgram]) {
    let rewrites: Vec<(&str, String)> = deployed
        .iter()
        .filter_map(|d| {
            let suffix = env_var_suffix(&d.name);
            if suffix == d.name {
                None
            } else {
                Some((d.name.as_str(), suffix))
            }
        })
        .collect();
    if rewrites.is_empty() {
        return;
    }
    println!(
        "      note: program name(s) rewritten for env-var legality (any char outside [A-Za-z0-9_] becomes _):"
    );
    for (raw, suffix) in rewrites {
        println!("        {raw} -> SCAFFOLD_PROGRAM_ID_{suffix} (and SCAFFOLD_GUEST_BIN_{suffix}, SCAFFOLD_DEPLOY_SKIPPED_{suffix})");
    }
}

fn ensure_localnet(project: &Project, timeout_sec: u64) -> DynResult<()> {
    let status = build_localnet_status_for_project(project);
    match status.ownership {
        LocalnetOwnership::Managed if status.ready => {
            let pid_display = status
                .tracked_pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            println!("      localnet already running (sequencer pid={pid_display})");
            Ok(())
        }
        LocalnetOwnership::Foreign => {
            let pid_display = status
                .listener_pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            bail!(
                "localnet port is in use by another process (pid={pid_display}).\n\
                 This may be a sequencer from another project.\n\
                 Stop it first with `logos-scaffold localnet stop` (or `kill {pid_display}`)."
            );
        }
        _ => cmd_localnet(LocalnetAction::Start { timeout_sec }),
    }
}

fn print_deploy_summary(project: &Project) -> DynResult<()> {
    let programs_dir = project.root.join("methods/guest/src/bin");
    if !programs_dir.exists() {
        return Ok(());
    }

    let programs = discover_deployable_programs(&project.root)?;
    if programs.is_empty() {
        println!();
        println!("No deployable programs found in {}", programs_dir.display());
        return Ok(());
    }
    let binaries = discover_program_binaries(&project.root, &programs);

    println!();
    println!("Deployed programs:");
    for stem in &programs {
        if let Some(binary_path) = binaries.get(stem) {
            println!("  {stem}");
            println!("    Binary: {}", binary_path.display());
        }
    }

    // Use the same URL construction as build_hook_command and wallet_support
    // so the summary always reflects the address the wallet actually targets,
    // rather than the raw localnet.port value which may differ when
    // wallet_config.json overrides sequencer_addr (issue #161).
    let sequencer_url =
        crate::commands::wallet_support::default_sequencer_http_url_for_project(project);
    println!();
    println!("Sequencer: {sequencer_url}");

    Ok(())
}

fn build_hook_command(
    project: &Project,
    hook_command: &str,
    deployed: &DeployedPrograms,
) -> Command {
    let sequencer_url =
        crate::commands::wallet_support::default_sequencer_http_url_for_project(project);
    let wallet_home = project
        .root
        .join(&project.config.wallet_home_dir)
        .canonicalize()
        .unwrap_or_else(|_| project.root.join(&project.config.wallet_home_dir));
    let project_root = project
        .root
        .canonicalize()
        .unwrap_or_else(|_| project.root.clone());
    let idl_dir = project
        .root
        .join(&project.config.framework.idl.path)
        .canonicalize()
        .unwrap_or_else(|_| project.root.join(&project.config.framework.idl.path));

    let run_deploy_skipped = deployed.skipped;

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(hook_command)
        .env("SEQUENCER_URL", &sequencer_url)
        .env("NSSA_WALLET_HOME_DIR", &wallet_home)
        .env("SCAFFOLD_PROJECT_ROOT", &project_root)
        .env("SCAFFOLD_IDL_DIR", &idl_dir)
        // Always-on: deploy-skip state is run-level (it's the same for
        // every program in this invocation), so multi-program hooks need
        // it just as much as single-program ones.
        .env(
            "SCAFFOLD_DEPLOY_SKIPPED",
            if run_deploy_skipped { "1" } else { "0" },
        )
        .current_dir(&project.root);

    // Per-program metadata: `SCAFFOLD_PROGRAMS` holds the space-separated
    // list of names, with parallel `SCAFFOLD_PROGRAM_ID_<name>`,
    // `SCAFFOLD_GUEST_BIN_<name>`, `SCAFFOLD_DEPLOY_SKIPPED_<name>` per
    // entry. Names are sanitized for env-var-suffix legality.
    let names: Vec<&str> = deployed.programs.iter().map(|d| d.name.as_str()).collect();
    cmd.env("SCAFFOLD_PROGRAMS", names.join(" "));
    for d in &deployed.programs {
        let suffix = env_var_suffix(&d.name);
        if let Some(id) = &d.program_id {
            cmd.env(format!("SCAFFOLD_PROGRAM_ID_{suffix}"), id);
        }
        cmd.env(format!("SCAFFOLD_GUEST_BIN_{suffix}"), &d.binary_path);
        cmd.env(
            format!("SCAFFOLD_DEPLOY_SKIPPED_{suffix}"),
            if run_deploy_skipped { "1" } else { "0" },
        );
    }
    // Single-program shortcut: only set when there's exactly one program.
    // Hooks that handle multi-program projects must use the indexed forms.
    // `SCAFFOLD_DEPLOY_SKIPPED` is set unconditionally above (run-level),
    // so it's not duplicated here.
    if let [single] = deployed.programs.as_slice() {
        cmd.env("SCAFFOLD_PROGRAM_NAME", &single.name);
        if let Some(id) = &single.program_id {
            cmd.env("SCAFFOLD_PROGRAM_ID", id);
        }
        cmd.env("SCAFFOLD_GUEST_BIN", &single.binary_path);
    }
    cmd
}

fn run_post_deploy_hook(
    project: &Project,
    hook_command: &str,
    deployed: &DeployedPrograms,
) -> DynResult<()> {
    let status = build_hook_command(project, hook_command, deployed)
        .status()
        .context("failed to execute post-deploy hook")?;

    if !status.success() {
        let code = status.code().unwrap_or(1);
        bail!("post-deploy hook exited with status {code}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Config, FrameworkConfig, FrameworkIdlConfig, LocalnetConfig, Project, RepoRef, RunConfig,
    };
    use std::path::PathBuf;

    fn make_test_project(root: PathBuf) -> Project {
        Project {
            root,
            config: Config {
                version: "0.2.0".to_string(),
                cache_root: ".scaffold/cache".to_string(),
                lez: RepoRef {
                    source: "lez".to_string(),
                    path: "lez".to_string(),
                    pin: "abc123".to_string(),
                    ..Default::default()
                },
                spel: RepoRef {
                    source: "spel".to_string(),
                    path: "spel".to_string(),
                    pin: "def456".to_string(),
                    ..Default::default()
                },
                basecamp_repo: None,
                lgpm_repo: None,
                wallet_home_dir: ".scaffold/wallet".to_string(),
                framework: FrameworkConfig {
                    kind: "default".to_string(),
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
                modules: std::collections::BTreeMap::new(),
                run: RunConfig::default(),
                basecamp: None,
            },
        }
    }

    #[test]
    fn hook_receives_sequencer_url_env() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let hook = format!("echo \"$SEQUENCER_URL\" > '{}'", env_file.display());
        run_post_deploy_hook(&project, &hook, &DeployedPrograms::default())
            .expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "http://127.0.0.1:3040");
    }

    #[test]
    fn hook_receives_wallet_home_dir_env() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let hook = format!("echo \"$NSSA_WALLET_HOME_DIR\" > '{}'", env_file.display());
        run_post_deploy_hook(&project, &hook, &DeployedPrograms::default())
            .expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert!(
            content.trim().ends_with(".scaffold/wallet"),
            "expected wallet home to end with .scaffold/wallet, got: {content}"
        );
    }

    #[test]
    fn hook_receives_project_root_env() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let hook = format!("echo \"$SCAFFOLD_PROJECT_ROOT\" > '{}'", env_file.display());
        run_post_deploy_hook(&project, &hook, &DeployedPrograms::default())
            .expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        let canonical = temp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp.path().to_path_buf());
        assert_eq!(content.trim(), canonical.display().to_string());
    }

    #[test]
    fn hook_receives_idl_dir_env() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let hook = format!("echo \"$SCAFFOLD_IDL_DIR\" > '{}'", env_file.display());
        run_post_deploy_hook(&project, &hook, &DeployedPrograms::default())
            .expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert!(
            content.trim().ends_with("/idl"),
            "expected IDL dir to end with /idl, got: {content}"
        );
    }

    #[test]
    fn hook_uses_custom_port() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let mut project = make_test_project(temp.path().to_path_buf());
        project.config.localnet.port = 9999;

        let hook = format!("echo \"$SEQUENCER_URL\" > '{}'", env_file.display());
        run_post_deploy_hook(&project, &hook, &DeployedPrograms::default())
            .expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "http://127.0.0.1:9999");
    }

    #[test]
    fn hook_failure_propagates_as_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());

        let result = run_post_deploy_hook(&project, "exit 42", &DeployedPrograms::default());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("42"),
            "expected exit code 42 in error, got: {msg}"
        );
    }

    #[test]
    fn hook_runs_in_project_root_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pwd_file = temp.path().join("pwd_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let hook = format!("pwd > '{}'", pwd_file.display());
        run_post_deploy_hook(&project, &hook, &DeployedPrograms::default())
            .expect("hook should succeed");

        let content = std::fs::read_to_string(&pwd_file).expect("read pwd output");
        let canonical = temp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp.path().to_path_buf());
        assert_eq!(content.trim(), canonical.display().to_string());
    }

    #[test]
    fn print_deploy_summary_shows_programs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());

        let programs_dir = temp.path().join("methods/guest/src/bin");
        std::fs::create_dir_all(&programs_dir).expect("create programs dir");
        std::fs::write(programs_dir.join("counter.rs"), "fn main() {}").expect("write source");

        // Mirror the layout `discover_program_binaries` walks for: a
        // `riscv32im*/release/` segment under one of the search roots.
        let binary_dir = temp
            .path()
            .join("target/riscv-guest/methods/programs/riscv32im-risc0-zkvm-elf/release");
        std::fs::create_dir_all(&binary_dir).expect("create binary dir");
        std::fs::write(binary_dir.join("counter.bin"), b"fake binary").expect("write binary");

        print_deploy_summary(&project).expect("should succeed");
    }

    #[test]
    fn print_deploy_summary_skips_non_rs_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());

        let programs_dir = temp.path().join("methods/guest/src/bin");
        std::fs::create_dir_all(&programs_dir).expect("create programs dir");
        std::fs::write(programs_dir.join("README.md"), "# readme").expect("write non-rs file");

        print_deploy_summary(&project).expect("should succeed with no .rs files");
    }

    #[test]
    fn print_deploy_summary_returns_ok_when_no_programs_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());

        print_deploy_summary(&project).expect("should succeed with missing dir");
    }

    #[test]
    fn hook_receives_full_env_contract_in_one_invocation() {
        // Integration-style assertion: every documented always-on env var
        // reaches the hook in a single shell invocation, in the same form
        // `cmd_run` would produce.
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let hook = format!(
            "{{ \
                echo \"SEQUENCER_URL=$SEQUENCER_URL\"; \
                echo \"NSSA_WALLET_HOME_DIR=$NSSA_WALLET_HOME_DIR\"; \
                echo \"SCAFFOLD_PROJECT_ROOT=$SCAFFOLD_PROJECT_ROOT\"; \
                echo \"SCAFFOLD_IDL_DIR=$SCAFFOLD_IDL_DIR\"; \
            }} > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, &DeployedPrograms::default())
            .expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        let canonical = temp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp.path().to_path_buf());
        let lines: Vec<&str> = content.lines().collect();

        assert_eq!(lines[0], "SEQUENCER_URL=http://127.0.0.1:3040");
        assert!(
            lines[1].starts_with("NSSA_WALLET_HOME_DIR=") && lines[1].ends_with(".scaffold/wallet"),
            "wallet home line was: {}",
            lines[1]
        );
        assert_eq!(
            lines[2],
            format!("SCAFFOLD_PROJECT_ROOT={}", canonical.display())
        );
        assert!(
            lines[3].starts_with("SCAFFOLD_IDL_DIR=") && lines[3].ends_with("/idl"),
            "idl dir line was: {}",
            lines[3]
        );
    }

    fn fake_deployed(name: &str, id: Option<&str>) -> DeployedProgram {
        DeployedProgram {
            name: name.to_string(),
            program_id: id.map(str::to_string),
            binary_path: PathBuf::from(format!("/fake/{name}.bin")),
        }
    }

    fn programs(progs: Vec<DeployedProgram>, skipped: bool) -> DeployedPrograms {
        DeployedPrograms {
            skipped,
            programs: progs,
        }
    }

    #[test]
    fn hook_receives_single_program_env_when_provided() {
        // When a single `DeployedProgram` is passed, `SCAFFOLD_PROGRAM_ID`
        // and `SCAFFOLD_GUEST_BIN` reach the hook environment.
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let deployed = programs(
            vec![DeployedProgram {
                name: "counter".to_string(),
                program_id: Some("deadbeef".to_string()),
                binary_path: temp.path().join("counter.bin"),
            }],
            false,
        );

        let hook = format!(
            "echo \"$SCAFFOLD_PROGRAM_ID|$SCAFFOLD_GUEST_BIN\" > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, &deployed).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        let expected_bin = temp.path().join("counter.bin");
        assert_eq!(
            content.trim(),
            format!("deadbeef|{}", expected_bin.display())
        );
    }

    #[test]
    fn hook_omits_program_id_env_when_extraction_failed() {
        // When `extract_program_id` returns None, the env var must be unset
        // rather than set to an empty string — a hook that tests `[ -z
        // "$SCAFFOLD_PROGRAM_ID" ]` should see it as unset.
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let deployed = programs(
            vec![DeployedProgram {
                name: "counter".to_string(),
                program_id: None,
                binary_path: temp.path().join("counter.bin"),
            }],
            false,
        );

        let hook = format!(
            "if [ -z \"${{SCAFFOLD_PROGRAM_ID+set}}\" ]; then echo unset; else echo set; fi > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, &deployed).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "unset");
    }

    #[test]
    fn hook_omits_single_program_env_when_metadata_absent() {
        // No-program projects pass `&[]`, and the single-program shortcut
        // env vars must not be set.
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let hook = format!(
            "echo \"id=${{SCAFFOLD_PROGRAM_ID+set}}|bin=${{SCAFFOLD_GUEST_BIN+set}}\" > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, &DeployedPrograms::default())
            .expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "id=|bin=");
    }

    #[test]
    fn hook_receives_deploy_skipped_env_for_multiprogram_run() {
        // `SCAFFOLD_DEPLOY_SKIPPED` is run-level state and must reach
        // multi-program hooks too — the env var is set unconditionally
        // (driven by `deployed[0].skipped`), not gated on the
        // single-program shortcut block.
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());
        let deployed = programs(
            vec![
                fake_deployed("a", Some("h1")),
                fake_deployed("b", Some("h2")),
            ],
            true,
        );

        let hook = format!(
            "echo \"$SCAFFOLD_DEPLOY_SKIPPED\" > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, &deployed).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "1");
    }

    #[test]
    fn hook_receives_deploy_skipped_zero_when_no_programs() {
        // Empty `deployed` (no programs at all) still sets the env var,
        // surfacing "0" rather than leaving it unset — hooks shouldn't
        // need to disambiguate "deploy ran" from "no programs".
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let hook = format!(
            "echo \"$SCAFFOLD_DEPLOY_SKIPPED\" > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, &DeployedPrograms::default())
            .expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "0");
    }

    #[test]
    fn hook_receives_program_id_indexed_by_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());
        let deployed = programs(vec![fake_deployed("counter", Some("deadbeef"))], false);

        let hook = format!(
            "echo \"$SCAFFOLD_PROGRAM_ID_counter\" > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, &deployed).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "deadbeef");
    }

    #[test]
    fn hook_receives_single_program_shortcut_when_one_program() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());
        let deployed = programs(vec![fake_deployed("counter", Some("abc123"))], false);

        let hook = format!(
            "printf '%s|%s|%s' \"$SCAFFOLD_PROGRAM_NAME\" \"$SCAFFOLD_PROGRAM_ID\" \"$SCAFFOLD_DEPLOY_SKIPPED\" > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, &deployed).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content, "counter|abc123|0");
    }

    #[test]
    fn hook_omits_single_program_shortcut_when_multiple() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());
        let deployed = programs(
            vec![
                fake_deployed("counter", Some("h1")),
                fake_deployed("greeter", Some("h2")),
            ],
            false,
        );

        let hook = format!(
            "echo \"[${{SCAFFOLD_PROGRAM_NAME:-unset}}]\" > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, &deployed).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "[unset]");
    }

    #[test]
    fn hook_receives_programs_list_and_skipped_flag() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());
        let deployed = programs(
            vec![fake_deployed("a", None), fake_deployed("b", Some("h"))],
            true,
        );

        let hook = format!(
            "printf '%s|%s|%s' \"$SCAFFOLD_PROGRAMS\" \"$SCAFFOLD_DEPLOY_SKIPPED_a\" \"$SCAFFOLD_DEPLOY_SKIPPED_b\" > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, &deployed).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content, "a b|1|1");
    }

    #[test]
    fn hook_program_id_unset_when_extraction_failed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());
        let deployed = programs(vec![fake_deployed("noid", None)], false);

        let hook = format!(
            "echo \"[${{SCAFFOLD_PROGRAM_ID_noid:-unset}}]\" > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, &deployed).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "[unset]");
    }

    #[test]
    fn env_var_suffix_sanitizes_unsafe_characters() {
        assert_eq!(env_var_suffix("plain"), "plain");
        assert_eq!(env_var_suffix("with-dash"), "with_dash");
        assert_eq!(env_var_suffix("dot.name"), "dot_name");
        assert_eq!(env_var_suffix("a/b"), "a_b");
    }

    #[test]
    fn collect_deployed_programs_returns_empty_ok_when_bin_dir_missing() {
        // No `methods/guest/src/bin` directory at all: this is a valid
        // state for non-LEZ projects, so it's Ok(empty) — not an error.
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());

        let got = collect_deployed_programs(&project, false).expect("should be Ok");
        assert!(got.programs.is_empty());
        assert!(!got.skipped);
    }

    #[test]
    fn collect_deployed_programs_propagates_spel_resolution_error() {
        // Misconfigured `[repos.spel]` (both path and pin empty) must
        // bubble up as a hard error rather than silently producing
        // an empty `SCAFFOLD_PROGRAMS` for hooks.
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("methods/guest/src/bin")).expect("create bin dir");
        let mut project = make_test_project(temp.path().to_path_buf());
        project.config.spel.path = String::new();
        project.config.spel.pin = String::new();

        let err = collect_deployed_programs(&project, false).expect_err("should be Err");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("spel"),
            "expected error to mention spel, got: {msg}"
        );
    }

    #[test]
    fn check_env_var_suffix_collisions_bails_on_collision() {
        let progs = vec![
            DeployedProgram {
                name: "my-program".to_string(),
                program_id: None,
                binary_path: PathBuf::from("/dev/null"),
            },
            DeployedProgram {
                name: "my_program".to_string(),
                program_id: None,
                binary_path: PathBuf::from("/dev/null"),
            },
        ];
        let err = check_env_var_suffix_collisions(&progs).expect_err("should be Err");
        let msg = format!("{err:#}");
        assert!(msg.contains("my-program"), "msg was: {msg}");
        assert!(msg.contains("my_program"), "msg was: {msg}");
    }

    #[test]
    fn check_env_var_suffix_collisions_passes_when_unique() {
        let progs = vec![
            DeployedProgram {
                name: "counter".to_string(),
                program_id: None,
                binary_path: PathBuf::from("/dev/null"),
            },
            DeployedProgram {
                name: "greeter".to_string(),
                program_id: None,
                binary_path: PathBuf::from("/dev/null"),
            },
        ];
        check_env_var_suffix_collisions(&progs).expect("no collisions");
    }

    #[test]
    fn warn_on_rewritten_program_names_only_lists_rewrites() {
        // Just exercise the function on a mixed list; the println!s go to
        // stdout (captured by `cargo test`) but we mainly want to confirm
        // it doesn't panic and the rewrite-detection logic stays consistent
        // with `env_var_suffix`.
        let deployed = vec![
            DeployedProgram {
                name: "alphanumeric".to_string(),
                program_id: None,
                binary_path: PathBuf::from("/dev/null"),
            },
            DeployedProgram {
                name: "with-dash".to_string(),
                program_id: None,
                binary_path: PathBuf::from("/dev/null"),
            },
        ];
        warn_on_rewritten_program_names(&deployed);
        // No assertion: the contract is "doesn't panic, prints only for
        // rewritten names". The print_deploy_summary integration tests
        // already lock in the user-visible output shape.
    }

    /// Asserts that `reseed_after_wipe` produces a `wallet.state` byte-equivalent
    /// to a fresh setup. The test does not exercise `reset_for_run` itself —
    /// `cmd_localnet_reset` requires a real sequencer and isn't reachable from
    /// unit-test scope. What this *does* lock down: if `reseed_after_wipe`
    /// drifts away from calling the same primitives `cmd_setup` uses
    /// (`prepare_wallet_home` + `ensure_default_wallet_seeded`), the byte
    /// comparison breaks. `reset_for_run` itself must keep calling
    /// `reseed_after_wipe` (not inline a parallel seed) — that contract is
    /// enforced by code review, not by this test.
    #[test]
    fn reseed_after_wipe_matches_setup_baseline() {
        use crate::commands::setup::ensure_default_wallet_seeded;
        use crate::commands::wallet_support::{wallet_state_path, WALLET_CONFIG_PRIMARY};
        use crate::state::prepare_wallet_home;

        // Two parallel project trees with identical fake LEZ wallet configs.
        // Both start from the same state a freshly-wiped project would: LEZ
        // bundled wallet config exists on disk, but no `.scaffold/wallet/`.
        let baseline = tempfile::tempdir().expect("baseline tempdir");
        let post_reset = tempfile::tempdir().expect("post_reset tempdir");

        for root in [baseline.path(), post_reset.path()] {
            let lez_cfg_dir = root.join("lez/wallet/configs/debug");
            std::fs::create_dir_all(&lez_cfg_dir).expect("create lez cfg dir");
            std::fs::write(
                lez_cfg_dir.join("wallet_config.json"),
                r#"{
  "initial_accounts": [
    { "Public": { "account_id": "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV" } }
  ]
}"#,
            )
            .expect("write lez wallet config");
        }

        // Baseline: drive `cmd_setup`'s seed primitives directly.
        {
            let lez = baseline.path().join("lez");
            let wallet_home = baseline.path().join(".scaffold/wallet");
            prepare_wallet_home(&lez, &wallet_home).expect("baseline prepare");
            ensure_default_wallet_seeded(baseline.path(), &wallet_home).expect("baseline seed");
        }

        // Post-reset: drive `reseed_after_wipe` directly.
        // The fixture sets `lez.path = "lez"` (relative); make it absolute
        // so the helper doesn't depend on cwd (tests run in parallel and
        // can't share cwd).
        let mut project = make_test_project(post_reset.path().to_path_buf());
        project.config.lez.path = post_reset.path().join("lez").to_string_lossy().to_string();
        reseed_after_wipe(&project).expect("post-reset reseed");

        let baseline_state =
            std::fs::read(wallet_state_path(baseline.path())).expect("read baseline state");
        let post_reset_state =
            std::fs::read(wallet_state_path(post_reset.path())).expect("read post-reset state");
        assert_eq!(
            baseline_state, post_reset_state,
            "post-reset wallet.state must be byte-equivalent to clean-setup baseline"
        );

        let baseline_cfg = std::fs::read(
            baseline
                .path()
                .join(".scaffold/wallet")
                .join(WALLET_CONFIG_PRIMARY),
        )
        .expect("read baseline cfg");
        let post_reset_cfg = std::fs::read(
            post_reset
                .path()
                .join(".scaffold/wallet")
                .join(WALLET_CONFIG_PRIMARY),
        )
        .expect("read post-reset cfg");
        assert_eq!(baseline_cfg, post_reset_cfg);
    }
}
