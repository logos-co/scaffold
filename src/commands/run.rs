use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context};

use crate::commands::build::cmd_build_shortcut;
use crate::commands::deploy::{
    cmd_deploy, discover_deployable_programs, discover_program_binaries, extract_program_id,
};
use crate::commands::idl::build_idl_for_current_project;
use crate::commands::localnet::{build_localnet_status_for_project, cmd_localnet, LocalnetAction};
use crate::commands::wallet::{cmd_wallet_topup_inner, TopupOutcome};
use crate::constants::{DEFAULT_RUN_LOCALNET_TIMEOUT_SEC, SPEL_BIN_REL_PATH};
use crate::model::{LocalnetOwnership, Project};
use crate::project::{load_project, resolve_repo_path, run_in_project_dir};
use crate::DynResult;

/// All knobs that control a `lgs run` invocation. Built by `cli.rs` from
/// the parsed `RunArgs` (with conflicting-flag resolution into `Option<Vec<_>>`)
/// and consumed by `cmd_run`. Grouping the fields together prevents the
/// positional-swap class of bug.
#[derive(Clone, Debug, Default)]
pub(crate) struct RunInvocation {
    pub(crate) post_deploy_override: Option<Vec<String>>,
    pub(crate) localnet_timeout_sec: Option<u64>,
}

pub(crate) fn cmd_run(inv: RunInvocation) -> DynResult<()> {
    let project = load_project()?;
    let hooks = inv
        .post_deploy_override
        .unwrap_or_else(|| project.config.run.post_deploy.clone());
    let localnet_timeout_sec = inv
        .localnet_timeout_sec
        .unwrap_or(DEFAULT_RUN_LOCALNET_TIMEOUT_SEC);

    // Anchor the pipeline at the discovered project root. Otherwise commands
    // that resolve paths relative to cwd (`cmd_build_shortcut`,
    // `build_idl_for_current_project`, etc.) would build/deploy from whichever
    // subdirectory the user invoked `lgs run` in.
    run_in_project_dir(Some(&project.root), || {
        run_pipeline_once(&project, &hooks, localnet_timeout_sec)
    })
}

fn run_pipeline_once(
    project: &Project,
    hooks: &[String],
    localnet_timeout_sec: u64,
) -> DynResult<()> {
    let has_hooks = !hooks.is_empty();
    // Steps: build, build idl, localnet, topup, deploy, [+1 if hooks]
    let total_steps: u32 = if has_hooks { 6 } else { 5 };

    // Step 1: Build (chains setup internally)
    println!("[1/{total_steps}] Building...");
    cmd_build_shortcut(None)?;

    // Step 2: Build IDL (no-op for non-lez-framework projects)
    println!("[2/{total_steps}] Building IDL...");
    build_idl_for_current_project()?;

    // Step 3: Ensure localnet is running.
    println!("[3/{total_steps}] Ensuring localnet...");
    ensure_localnet(project, localnet_timeout_sec)?;

    // Step 4: Wallet topup
    println!("[4/{total_steps}] Topping up wallet...");
    let outcome = cmd_wallet_topup_inner(project, None, false)?;
    if outcome == TopupOutcome::ConfirmationTimeout {
        bail!("wallet topup confirmation timed out; aborting run to avoid deploying with uncertain funding.\nHint: retry `logos-scaffold run` or run `logos-scaffold wallet topup` manually.");
    }

    // Step 5: Deploy
    println!("[5/{total_steps}] Deploying programs...");
    cmd_deploy(None, None, false)?;

    // Step 6: Post-deploy hooks (or summary)
    if has_hooks {
        let n = hooks.len();
        println!("[6/{total_steps}] Running {n} post-deploy hook(s)...");
        // Resolve the single-program shortcut metadata once: `extract_program_id`
        // shells out to `spel inspect` with a per-call timeout, so doing it
        // inside the loop would multiply latency by the hook count.
        let single_program = resolve_single_program_metadata(project)?;
        for (i, hook) in hooks.iter().enumerate() {
            println!("===> post_deploy[{}/{n}]: {hook}", i + 1);
            run_post_deploy_hook(project, hook, single_program.as_ref())?;
            println!("<=== post_deploy[{}/{n}] OK", i + 1);
        }
    } else {
        print_deploy_summary(project)?;
    }

    Ok(())
}

/// Single-program shortcut metadata exposed to post-deploy hooks via env vars.
/// Resolved once per `run` invocation and reused across hooks.
struct SingleProgram {
    binary_path: PathBuf,
    program_id: Option<String>,
}

fn resolve_single_program_metadata(project: &Project) -> DynResult<Option<SingleProgram>> {
    let Some(binary_path) = single_program_binary(project)? else {
        return Ok(None);
    };
    let program_id =
        resolve_spel_bin(project).and_then(|spel_bin| extract_program_id(&spel_bin, &binary_path));
    Ok(Some(SingleProgram {
        binary_path,
        program_id,
    }))
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

    let port = project.config.localnet.port;
    println!();
    println!("Sequencer: http://127.0.0.1:{port}");

    Ok(())
}

fn build_hook_command(
    project: &Project,
    hook_command: &str,
    single_program: Option<&SingleProgram>,
) -> Command {
    let port = project.config.localnet.port;
    let sequencer_url = format!("http://127.0.0.1:{port}");
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

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(hook_command)
        .env("SEQUENCER_URL", &sequencer_url)
        .env("NSSA_WALLET_HOME_DIR", &wallet_home)
        .env("SCAFFOLD_PROJECT_ROOT", &project_root)
        .env("SCAFFOLD_IDL_DIR", &idl_dir)
        .current_dir(&project.root);

    // Single-program shortcut: when there's exactly one deployable program,
    // expose its program-id and guest-binary path as env vars so simple
    // hooks can call `spel` or the dogfood client without parsing the
    // deploy summary.
    if let Some(sp) = single_program {
        if let Some(id) = &sp.program_id {
            cmd.env("SCAFFOLD_PROGRAM_ID", id);
        }
        cmd.env("SCAFFOLD_GUEST_BIN", &sp.binary_path);
    }
    cmd
}

fn single_program_binary(project: &Project) -> DynResult<Option<PathBuf>> {
    let programs_dir = project.root.join("methods/guest/src/bin");
    if !programs_dir.exists() {
        return Ok(None);
    }
    // Propagate I/O failures rather than treating them as "no programs":
    // an unreadable bin dir is a real error, and silently dropping it
    // strips the SCAFFOLD_GUEST_BIN env var from post-deploy hooks
    // without ever surfacing the cause.
    let programs = discover_deployable_programs(&project.root)
        .context("failed to discover deployable programs for run")?;
    if programs.len() != 1 {
        return Ok(None);
    }
    let binaries = discover_program_binaries(&project.root, &programs);
    Ok(binaries.get(&programs[0]).cloned())
}

fn resolve_spel_bin(project: &Project) -> Option<PathBuf> {
    let spel = resolve_repo_path(project, &project.config.spel, "spel").ok()?;
    Some(spel.join(SPEL_BIN_REL_PATH))
}

fn run_post_deploy_hook(
    project: &Project,
    hook_command: &str,
    single_program: Option<&SingleProgram>,
) -> DynResult<()> {
    let status = build_hook_command(project, hook_command, single_program)
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
        run_post_deploy_hook(&project, &hook, None).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "http://127.0.0.1:3040");
    }

    #[test]
    fn hook_receives_wallet_home_dir_env() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let hook = format!("echo \"$NSSA_WALLET_HOME_DIR\" > '{}'", env_file.display());
        run_post_deploy_hook(&project, &hook, None).expect("hook should succeed");

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
        run_post_deploy_hook(&project, &hook, None).expect("hook should succeed");

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
        run_post_deploy_hook(&project, &hook, None).expect("hook should succeed");

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
        run_post_deploy_hook(&project, &hook, None).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "http://127.0.0.1:9999");
    }

    #[test]
    fn hook_failure_propagates_as_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = make_test_project(temp.path().to_path_buf());

        let result = run_post_deploy_hook(&project, "exit 42", None);
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
        run_post_deploy_hook(&project, &hook, None).expect("hook should succeed");

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
        run_post_deploy_hook(&project, &hook, None).expect("hook should succeed");

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

    #[test]
    fn hook_receives_single_program_env_when_provided() {
        // When `SingleProgram` is passed, `SCAFFOLD_PROGRAM_ID` and
        // `SCAFFOLD_GUEST_BIN` reach the hook environment.
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let single = SingleProgram {
            binary_path: temp.path().join("counter.bin"),
            program_id: Some("deadbeef".to_string()),
        };

        let hook = format!(
            "echo \"$SCAFFOLD_PROGRAM_ID|$SCAFFOLD_GUEST_BIN\" > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, Some(&single)).expect("hook should succeed");

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

        let single = SingleProgram {
            binary_path: temp.path().join("counter.bin"),
            program_id: None,
        };

        let hook = format!(
            "if [ -z \"${{SCAFFOLD_PROGRAM_ID+set}}\" ]; then echo unset; else echo set; fi > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, Some(&single)).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "unset");
    }

    #[test]
    fn hook_omits_single_program_env_when_metadata_absent() {
        // Multi-program (or no-program) projects pass `None`, and the
        // single-program shortcut env vars must not be set.
        let temp = tempfile::tempdir().expect("tempdir");
        let env_file = temp.path().join("env_out.txt");
        let project = make_test_project(temp.path().to_path_buf());

        let hook = format!(
            "echo \"id=${{SCAFFOLD_PROGRAM_ID+set}}|bin=${{SCAFFOLD_GUEST_BIN+set}}\" > '{}'",
            env_file.display()
        );
        run_post_deploy_hook(&project, &hook, None).expect("hook should succeed");

        let content = std::fs::read_to_string(&env_file).expect("read env output");
        assert_eq!(content.trim(), "id=|bin=");
    }
}
