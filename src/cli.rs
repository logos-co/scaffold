use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::anyhow;
use clap::{CommandFactory, Parser, Subcommand};

use crate::cli_help::{
    EXAMPLES_BUILD, EXAMPLES_COMPLETIONS, EXAMPLES_CREATE, EXAMPLES_DEPLOY, EXAMPLES_DOCTOR,
    EXAMPLES_INIT, EXAMPLES_LOCALNET_LOGS, EXAMPLES_LOCALNET_RESET, EXAMPLES_LOCALNET_START,
    EXAMPLES_LOCALNET_STATUS, EXAMPLES_LOCALNET_STOP, EXAMPLES_REPORT, EXAMPLES_ROOT,
    EXAMPLES_SETUP, EXAMPLES_WALLET, EXAMPLES_WALLET_DEFAULT_SET, EXAMPLES_WALLET_LIST,
    EXAMPLES_WALLET_TOPUP,
};
use crate::commands::basecamp::{cmd_basecamp, BasecampAction};
use crate::commands::build::cmd_build_shortcut;
use crate::commands::client::cmd_client;
use crate::commands::completions::cmd_completions;
use crate::commands::deploy::cmd_deploy;
use crate::commands::doctor::cmd_doctor;
use crate::commands::idl::cmd_idl;
use crate::commands::init::cmd_init;
use crate::commands::localnet::{cmd_localnet, LocalnetAction};
use crate::commands::new::{cmd_new, NewCommand};
use crate::commands::report::cmd_report;
use crate::commands::run::{cmd_run, RunInvocation};
use crate::commands::setup::cmd_setup;
use crate::commands::spel::cmd_spel;
use crate::commands::wallet::{cmd_wallet, WalletAction};
use crate::constants::{DEFAULT_RUN_LOCALNET_TIMEOUT_SEC, VERSION};
use crate::process::set_command_echo;
use crate::template::project::available_templates;
use crate::DynResult;

static TEMPLATE_HELP: LazyLock<String> = LazyLock::new(|| {
    let templates = available_templates().join(", ");
    format!("Template to use (available: {templates})")
});

static CREATE_ABOUT: LazyLock<String> = LazyLock::new(|| {
    let templates = available_templates().join(", ");
    format!("Create a new logos-scaffold project (templates: {templates})")
});

static NEW_ABOUT: LazyLock<String> = LazyLock::new(|| {
    let templates = available_templates().join(", ");
    format!("Alias for `create` (templates: {templates})")
});

static RUN_LOCALNET_TIMEOUT_HELP: LazyLock<String> = LazyLock::new(|| {
    format!(
        "Seconds to wait for the sequencer to become ready when `run` has to \
         start localnet itself (default: {DEFAULT_RUN_LOCALNET_TIMEOUT_SEC}). \
         Bump this if a cold first run (fresh clone, cold caches) overshoots \
         the default."
    )
});

#[derive(Debug, Parser)]
#[command(
    name = "logos-scaffold",
    version = VERSION,
    disable_help_subcommand = true,
    after_long_help = EXAMPLES_ROOT
)]
struct Cli {
    #[arg(
        short,
        long,
        global = true,
        help = "Suppress echoed external commands (lines starting with `$`). Same effect as LOGOS_SCAFFOLD_QUIET=1."
    )]
    quiet: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "Create a new logos-scaffold project")]
    #[command(before_long_help = CREATE_ABOUT.as_str())]
    Create(NewArgs),
    #[command(about = "Alias for `create`")]
    #[command(before_long_help = NEW_ABOUT.as_str())]
    New(NewArgs),
    Setup(SetupArgs),
    Build(BuildArgs),
    Deploy(DeployArgs),
    Localnet(LocalnetArgs),
    Wallet(WalletArgs),
    #[command(about = "Manage pre-seeded basecamp profiles for p2p dogfooding")]
    Basecamp(BasecampArgs),
    Doctor(DoctorArgs),
    #[command(about = "Build, start localnet, top up wallet, deploy, and run post-deploy hook")]
    Run(RunArgs),
    #[command(about = "Collect a sanitized diagnostics archive for issue reporting")]
    Report(ReportArgs),
    #[command(
        about = "Print a shell completion script to stdout",
        long_about = "Print a shell completion script to stdout.\n\n\
                      Run `lgs completions <shell> --help` for per-shell install instructions."
    )]
    #[command(after_long_help = EXAMPLES_COMPLETIONS)]
    Completions(CompletionsArgs),
    #[command(about = "Initialize scaffold.toml in the current directory")]
    #[command(after_long_help = EXAMPLES_INIT)]
    Init(InitArgs),
    #[command(hide = true)]
    Help,
    /// Test-only hooks — hidden from `--help` output. Keeps the binary
    /// verifiable end-to-end without polluting the user-visible CLI surface.
    #[command(hide = true)]
    SelfTest(SelfTestArgs),
}

#[derive(Debug, clap::Args)]
struct SelfTestArgs {
    #[command(subcommand)]
    command: SelfTestSubcommand,
}

#[derive(Debug, Subcommand)]
enum SelfTestSubcommand {
    /// Drive `run_logged` against a trivial subprocess. Used by the CLI
    /// integration suite to pin the logged / `--print-output` output
    /// shapes against regressions.
    RunLogged(SelfTestRunLoggedArgs),
}

#[derive(Debug, clap::Args)]
struct SelfTestRunLoggedArgs {
    /// Absolute path to write the captured log to.
    #[arg(long, value_name = "PATH")]
    log: PathBuf,
    /// Step label passed to `run_logged`. Appears in progress / failure lines.
    #[arg(long, default_value = "self-test step")]
    step: String,
    /// Run `false` instead of `true` — exercises the failure bail.
    #[arg(long)]
    fail: bool,
    /// Set `LOGOS_SCAFFOLD_PRINT_OUTPUT=1` for this call — exercises the
    /// streamed shape instead of the captured one.
    #[arg(long)]
    print_output: bool,
}

#[derive(Debug, clap::Args)]
struct CompletionsArgs {
    #[command(subcommand)]
    shell: CompletionsShell,
}

#[derive(Debug, Subcommand)]
enum CompletionsShell {
    #[command(
        about = "Print bash completion script to stdout",
        long_about = "Print bash completion script to stdout.\n\n\
                      The generated script completes both `lgs` and `logos-scaffold`.\n\n\
                      Install:\n\n    \
                      lgs completions bash > ~/.local/share/bash-completion/completions/lgs\n\n\
                      Then reload your shell (or `source` the file) to pick up completions."
    )]
    Bash,
    #[command(
        about = "Print zsh completion script to stdout",
        long_about = "Print zsh completion script to stdout.\n\n\
                      The generated script completes both `lgs` and `logos-scaffold`.\n\n\
                      Install (plain zsh):\n\n    \
                      mkdir -p ~/.zfunc\n    \
                      lgs completions zsh > ~/.zfunc/_lgs\n\n\
                      Then ensure ~/.zshrc contains:\n\n    \
                      fpath=(~/.zfunc $fpath)\n    \
                      autoload -Uz compinit && compinit\n\n\
                      Install (oh-my-zsh, as a custom plugin):\n\n    \
                      mkdir -p ~/.oh-my-zsh/custom/plugins/lgs\n    \
                      lgs completions zsh > ~/.oh-my-zsh/custom/plugins/lgs/_lgs\n\n\
                      Then add `lgs` to the `plugins=(...)` array in ~/.zshrc and reload the shell."
    )]
    Zsh,
}

#[derive(Debug, clap::Args)]
#[command(after_long_help = EXAMPLES_CREATE)]
struct NewArgs {
    name: String,
    #[arg(long)]
    vendor_deps: bool,
    #[arg(long, alias = "lssa-path")]
    lez_path: Option<PathBuf>,
    #[arg(long, default_value = "default", help = TEMPLATE_HELP.as_str())]
    template: String,
}

#[derive(Debug, clap::Args)]
#[command(after_long_help = EXAMPLES_SETUP)]
struct SetupArgs {}

#[derive(Debug, clap::Args)]
#[command(after_long_help = EXAMPLES_BUILD)]
struct BuildArgs {
    #[command(subcommand)]
    subcommand: Option<BuildSubcommand>,
    project_path: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum BuildSubcommand {
    #[command(about = "Build IDL files from the current project")]
    Idl(BuildSubArgs),
    #[command(about = "Build client code from IDL files")]
    Client(BuildSubArgs),
}

#[derive(Debug, clap::Args)]
struct BuildSubArgs {
    project_path: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
#[command(after_long_help = EXAMPLES_DEPLOY)]
struct DeployArgs {
    program_name: Option<String>,
    /// Path to a custom ELF binary to deploy directly (bypasses auto-discovery)
    #[arg(long, value_name = "PATH")]
    program_path: Option<PathBuf>,
    #[arg(
        long,
        help = "Emit deploy results as JSON on stdout (recommended for automation)."
    )]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct InitArgs {
    #[arg(
        long,
        help = "Print what `init` would create or migrate, without writing scaffold.toml or creating .scaffold/."
    )]
    dry_run: bool,
    #[arg(
        long,
        help = "Skip writing scaffold.toml.bak before migrating an existing config (default: a backup is written next to scaffold.toml)."
    )]
    no_backup: bool,
}

#[derive(Debug, clap::Args)]
#[command(after_long_help = EXAMPLES_DOCTOR)]
struct DoctorArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
#[command(after_long_help = EXAMPLES_REPORT)]
struct ReportArgs {
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long, default_value_t = 500)]
    tail: usize,
}

#[derive(Debug, clap::Args)]
struct RunArgs {
    /// Skip post-deploy hooks even if scaffold.toml configures them
    #[arg(long)]
    no_post_deploy: bool,
    /// Override post-deploy hooks (repeatable). Replaces config-defined hooks
    /// for this invocation. Conflicts with --no-post-deploy.
    #[arg(long, value_name = "CMD", conflicts_with = "no_post_deploy")]
    post_deploy: Vec<String>,
    #[arg(long, value_name = "SECS", help = RUN_LOCALNET_TIMEOUT_HELP.as_str())]
    localnet_timeout: Option<u64>,
}

#[derive(Debug, clap::Args)]
struct LocalnetArgs {
    #[command(subcommand)]
    command: LocalnetSubcommand,
}

#[derive(Debug, Subcommand)]
enum LocalnetSubcommand {
    #[command(after_long_help = EXAMPLES_LOCALNET_START)]
    Start(LocalnetStartArgs),
    #[command(after_long_help = EXAMPLES_LOCALNET_STOP)]
    Stop,
    #[command(after_long_help = EXAMPLES_LOCALNET_STATUS)]
    Status(LocalnetStatusArgs),
    #[command(after_long_help = EXAMPLES_LOCALNET_LOGS)]
    Logs(LocalnetLogsArgs),
    #[command(after_long_help = EXAMPLES_LOCALNET_RESET)]
    Reset(LocalnetResetArgs),
}

#[derive(Debug, clap::Args)]
struct LocalnetStartArgs {
    #[arg(long, default_value_t = 20)]
    timeout_sec: u64,
}

#[derive(Debug, clap::Args)]
struct LocalnetStatusArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct LocalnetLogsArgs {
    #[arg(long, default_value_t = 200)]
    tail: usize,
}

/// Reset localnet to a clean state: stop the sequencer, delete the sequencer
/// database, restart the sequencer, and verify block production.
///
/// The wallet is preserved by default. Pass `--reset-wallet` to additionally
/// delete wallet keypairs and wallet state.
#[derive(Debug, clap::Args)]
struct LocalnetResetArgs {
    #[arg(
        long,
        help = "Print planned reset steps and paths without stopping, deleting, or restarting."
    )]
    dry_run: bool,
    #[arg(
        long,
        help = "Confirm the destructive reset (always wipes the sequencer DB; with --reset-wallet also deletes wallet keypairs). Required unless --dry-run is passed."
    )]
    yes: bool,
    /// Also delete the wallet home directory and wallet state. Destructive:
    /// keypairs are not recoverable after this.
    #[arg(long)]
    reset_wallet: bool,

    /// Seconds to wait for the restarted sequencer to produce a block.
    #[arg(long, default_value_t = 30)]
    verify_timeout_sec: u64,
}

#[derive(Debug, clap::Args)]
#[command(after_long_help = EXAMPLES_WALLET)]
struct WalletArgs {
    #[command(subcommand)]
    command: WalletSubcommand,
}

#[derive(Debug, Subcommand)]
enum WalletSubcommand {
    #[command(about = "List wallet accounts (same as `wallet account list`)")]
    #[command(after_long_help = EXAMPLES_WALLET_LIST)]
    List(WalletListArgs),
    #[command(about = "Top up wallet using pinata faucet claim")]
    #[command(after_long_help = EXAMPLES_WALLET_TOPUP)]
    Topup(WalletTopupArgs),
    #[command(about = "Manage project default wallet")]
    Default(WalletDefaultArgs),
}

#[derive(Debug, clap::Args)]
struct WalletListArgs {
    #[arg(long)]
    long: bool,
    /// Output as JSON for programmatic consumption
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct WalletTopupArgs {
    #[arg(value_name = "ADDRESS")]
    address: Option<String>,
    #[arg(long = "address", value_name = "ADDRESS")]
    address_flag: Option<String>,
    #[arg(long)]
    dry_run: bool,
    /// Output result as JSON for programmatic consumption
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct WalletDefaultArgs {
    #[command(subcommand)]
    command: WalletDefaultSubcommand,
}

#[derive(Debug, Subcommand)]
enum WalletDefaultSubcommand {
    #[command(after_long_help = EXAMPLES_WALLET_DEFAULT_SET)]
    Set(WalletDefaultSetArgs),
}

#[derive(Debug, clap::Args)]
struct WalletDefaultSetArgs {
    #[arg(value_name = "ADDRESS")]
    address: Option<String>,
    #[arg(long = "address", value_name = "ADDRESS")]
    address_flag: Option<String>,
}

#[derive(Debug, clap::Args)]
struct BasecampArgs {
    #[command(subcommand)]
    command: BasecampSubcommand,
}

#[derive(Debug, Subcommand)]
enum BasecampSubcommand {
    #[command(
        about = "Fetch, build, and seed pinned basecamp + lgpm + alice/bob profiles. See `basecamp docs` for project requirements."
    )]
    Setup,
    #[command(
        about = "Capture the set of modules + runtime dependencies to install; auto-discovers or takes explicit --flake/--path. See `basecamp docs` for project requirements."
    )]
    Modules(BasecampModulesArgs),
    #[command(
        about = "Build the project's .lgx and install it into basecamp profile(s). See `basecamp docs` for project requirements."
    )]
    Install(BasecampInstallArgs),
    #[command(
        about = "Launch basecamp for a named profile with clean-slate semantics. Destructive by default (scrubs xdg-data/xdg-cache); requires --yes, --no-clean, or --dry-run. See `basecamp docs` for project requirements."
    )]
    Launch(BasecampLaunchArgs),
    #[command(
        name = "build-portable",
        about = "Build the project's .#lgx-portable artefacts for hand-loading into a basecamp AppImage. See `basecamp docs` for project requirements."
    )]
    BuildPortable(BasecampBuildPortableArgs),
    #[command(
        about = "Basecamp-specific doctor: captured modules, manifest variants, and state drift. See `basecamp docs` for project requirements."
    )]
    Doctor(BasecampDoctorArgs),
    #[command(
        about = "Print the canonical project-compatibility rules (embedded copy of docs/basecamp-module-requirements.md)"
    )]
    Docs,
}

#[derive(Debug, clap::Args)]
struct BasecampDoctorArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct BasecampModulesArgs {
    /// Path to a pre-built .lgx file to capture as a project source (repeatable)
    #[arg(long, value_name = "PATH")]
    path: Vec<PathBuf>,
    /// Flake reference producing .lgx to capture as a project source, e.g. `./sub#lgx` (repeatable)
    #[arg(long, value_name = "REF")]
    flake: Vec<String>,
    /// Print the currently captured set and exit without mutating state
    #[arg(long)]
    show: bool,
}

#[derive(Debug, clap::Args)]
struct BasecampBuildPortableArgs {
    // `build-portable` takes no CLI source flags: it attr-swaps
    // `state.project_sources` (`#lgx` → `#lgx-portable`) and builds that.
    // `state.dependencies` are ignored — the target AppImage provides them.
}

#[derive(Debug, clap::Args)]
struct BasecampInstallArgs {
    // `install` takes no source-set flags: source set lives in `basecamp.state`
    // and is managed by `basecamp modules`. If state is empty on the first
    // `install`, it transparently invokes `modules` in auto-discover mode.
    /// Stream nix output directly to the terminal instead of logging to
    /// `.scaffold/logs/<ts>-install.log` and printing a one-line status.
    /// Equivalent to `LOGOS_SCAFFOLD_PRINT_OUTPUT=1`. Useful for CI.
    #[arg(long)]
    print_output: bool,
}

#[derive(Debug, clap::Args)]
struct BasecampLaunchArgs {
    #[arg(value_name = "PROFILE")]
    profile: String,
    /// Skip the clean-slate scrub and reinstall step
    #[arg(long)]
    no_clean: bool,
    #[arg(
        long,
        help = "Confirm the clean-slate scrub of the profile's xdg-data and xdg-cache. Required for the destructive default launch unless --no-clean or --dry-run is passed."
    )]
    yes: bool,
    #[arg(
        long,
        help = "Print what would be scrubbed and reinstalled without touching the profile or launching basecamp."
    )]
    dry_run: bool,
}

pub(crate) fn run(args: Vec<String>) -> DynResult<()> {
    apply_quiet_from_env();
    let passthrough_start = leading_global_flags_end(&args);
    if passthrough_start > 1 {
        set_command_echo(false);
    }
    if let Some(action) = wallet_passthrough_action(&args, passthrough_start)? {
        return cmd_wallet(action);
    }
    if let Some(spel_args) = spel_passthrough_args(&args, passthrough_start)? {
        return cmd_spel(spel_args);
    }

    let bin_name = args
        .first()
        .and_then(|s| std::path::Path::new(s).file_name())
        .and_then(|f| f.to_str())
        .unwrap_or("logos-scaffold")
        .to_string();

    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(err) => match err.kind() {
            clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                print!("{err}");
                return Ok(());
            }
            _ => {
                // clap's Display already starts with "error: "; strip it to avoid
                // "error: error: ..." when entry_main adds its own prefix.
                let msg = err.to_string();
                let msg = msg.strip_prefix("error: ").unwrap_or(&msg);
                return Err(anyhow!("{}", msg));
            }
        },
    };

    if cli.quiet {
        set_command_echo(false);
    }

    match cli.command {
        Some(Commands::Create(args)) | Some(Commands::New(args)) => cmd_new(NewCommand {
            name: args.name,
            vendor_deps: args.vendor_deps,
            lez_path: args.lez_path,
            template: args.template,
        }),
        Some(Commands::Setup(_)) => cmd_setup(),
        Some(Commands::Build(args)) => match args.subcommand {
            Some(BuildSubcommand::Idl(sub)) => cmd_idl(
                &sub.project_path
                    .map(|p| vec!["build".to_string(), p.to_string_lossy().to_string()])
                    .unwrap_or_else(|| vec!["build".to_string()]),
            ),
            Some(BuildSubcommand::Client(sub)) => cmd_client(
                &sub.project_path
                    .map(|p| vec!["build".to_string(), p.to_string_lossy().to_string()])
                    .unwrap_or_else(|| vec!["build".to_string()]),
            ),
            None => cmd_build_shortcut(args.project_path),
        },
        Some(Commands::Deploy(args)) => cmd_deploy(args.program_name, args.program_path, args.json),
        Some(Commands::Localnet(localnet)) => {
            let action = match localnet.command {
                LocalnetSubcommand::Start(args) => LocalnetAction::Start {
                    timeout_sec: args.timeout_sec,
                },
                LocalnetSubcommand::Stop => LocalnetAction::Stop,
                LocalnetSubcommand::Status(args) => LocalnetAction::Status { json: args.json },
                LocalnetSubcommand::Logs(args) => LocalnetAction::Logs { tail: args.tail },
                LocalnetSubcommand::Reset(args) => LocalnetAction::Reset {
                    dry_run: args.dry_run,
                    yes: args.yes,
                    reset_wallet: args.reset_wallet,
                    verify_timeout_sec: args.verify_timeout_sec,
                },
            };
            cmd_localnet(action)
        }
        Some(Commands::Wallet(args)) => {
            let action = match args.command {
                WalletSubcommand::List(args) => WalletAction::List {
                    long: args.long,
                    json: args.json,
                },
                WalletSubcommand::Topup(args) => WalletAction::Topup {
                    address: merge_optional_address(
                        args.address,
                        args.address_flag,
                        "wallet topup",
                    )?,
                    dry_run: args.dry_run,
                    json: args.json,
                },
                WalletSubcommand::Default(args) => match args.command {
                    WalletDefaultSubcommand::Set(set) => WalletAction::DefaultSet {
                        address: require_address(
                            set.address,
                            set.address_flag,
                            "wallet default set",
                        )?,
                    },
                },
            };
            cmd_wallet(action)
        }
        Some(Commands::Basecamp(args)) => {
            let action = match args.command {
                BasecampSubcommand::Setup => BasecampAction::Setup,
                BasecampSubcommand::Modules(args) => BasecampAction::Modules {
                    paths: args.path,
                    flakes: args.flake,
                    show: args.show,
                },
                BasecampSubcommand::Install(args) => BasecampAction::Install {
                    print_output: args.print_output,
                },
                BasecampSubcommand::Launch(args) => BasecampAction::Launch {
                    profile: args.profile,
                    no_clean: args.no_clean,
                    yes: args.yes,
                    dry_run: args.dry_run,
                },
                BasecampSubcommand::BuildPortable(_) => BasecampAction::BuildPortable,
                BasecampSubcommand::Doctor(args) => BasecampAction::Doctor { json: args.json },
                BasecampSubcommand::Docs => BasecampAction::Docs,
            };
            cmd_basecamp(action)
        }
        Some(Commands::Doctor(args)) => cmd_doctor(args.json),
        Some(Commands::Run(args)) => {
            let post_deploy = if args.no_post_deploy {
                Some(Vec::new())
            } else if !args.post_deploy.is_empty() {
                Some(args.post_deploy)
            } else {
                None
            };
            cmd_run(RunInvocation {
                post_deploy_override: post_deploy,
                localnet_timeout_sec: args.localnet_timeout,
            })
        }
        Some(Commands::Report(args)) => cmd_report(args.out, args.tail),
        Some(Commands::Completions(args)) => {
            let shell = match args.shell {
                CompletionsShell::Bash => clap_complete::Shell::Bash,
                CompletionsShell::Zsh => clap_complete::Shell::Zsh,
            };
            cmd_completions(shell)
        }
        Some(Commands::Init(args)) => cmd_init(&bin_name, args.dry_run, args.no_backup),
        Some(Commands::Help) => print_help(&bin_name),
        Some(Commands::SelfTest(args)) => match args.command {
            SelfTestSubcommand::RunLogged(a) => {
                crate::commands::self_test::cmd_self_test_run_logged(
                    &a.log,
                    &a.step,
                    a.fail,
                    a.print_output,
                )
            }
        },
        None => print_help(&bin_name),
    }
}

pub(crate) fn cli_command() -> clap::Command {
    Cli::command()
}

pub(crate) fn print_help(bin_name: &str) -> DynResult<()> {
    let mut cmd = Cli::command().bin_name(bin_name);
    cmd.print_help()?;
    println!();
    Ok(())
}

fn apply_quiet_from_env() {
    if std::env::var("LOGOS_SCAFFOLD_QUIET")
        .map(|v| {
            v == "1"
                || v.eq_ignore_ascii_case("true")
                || v.eq_ignore_ascii_case("yes")
                || v.eq_ignore_ascii_case("on")
        })
        .unwrap_or(false)
    {
        set_command_echo(false);
    }
}

fn leading_global_flags_end(args: &[String]) -> usize {
    let mut i = 1;
    while i < args.len() && (args[i] == "--quiet" || args[i] == "-q") {
        i += 1;
    }
    i
}

/// Advance past any consecutive `-q`/`--quiet` tokens in `args` starting
/// at `from`. Returns the new index and whether at least one was consumed.
/// Used inside the passthrough sniffers so `lgs wallet -q -- <args...>` /
/// `lgs spel -q -- <args...>` are recognized, matching clap's `global = true`
/// semantics on the `--quiet` flag. Caller is responsible for calling
/// `set_command_echo(false)` if the second tuple element is `true`.
fn skip_inline_quiet(args: &[String], from: usize) -> (usize, bool) {
    let mut i = from;
    let mut seen = false;
    while i < args.len() && (args[i] == "-q" || args[i] == "--quiet") {
        seen = true;
        i += 1;
    }
    (i, seen)
}

/// Forward `lgs spel -- <args...>` to the project-vendored `spel` binary.
/// Mirrors `wallet_passthrough_action` so the same `--` convention applies
/// across passthroughs. When `spel` is invoked without `--`, intercept early
/// and surface a hint pointing at the right form — clap's "unknown
/// subcommand" message would otherwise leave the user guessing. `start` is
/// the index after any leading global flags (e.g. `-q`), matching the
/// signature of `wallet_passthrough_action` so `lgs -q spel -- <args...>`
/// composes the same way. `lgs spel -q -- <args...>` (the in-between form)
/// is also accepted via `skip_inline_quiet`.
fn spel_passthrough_args(args: &[String], start: usize) -> DynResult<Option<Vec<String>>> {
    if args.len() <= start || args[start] != "spel" {
        return Ok(None);
    }
    let (sep_idx, quiet_seen) = skip_inline_quiet(args, start + 1);
    if args.len() <= sep_idx {
        return Err(anyhow!(
            "`spel` requires arguments. Use the passthrough form, e.g. `logos-scaffold spel -- inspect <bin>`."
        ));
    }
    if args[sep_idx] != "--" {
        return Err(anyhow!(
            "`spel {0} ...` is not a scaffold subcommand. Did you mean `logos-scaffold spel -- {0} ...`? \
             The `--` separator forwards every following argument to the project-vendored `spel` binary.",
            args[sep_idx]
        ));
    }
    if args.len() == sep_idx + 1 {
        return Err(anyhow!(
            "spel passthrough requires at least one argument after `--`. Example: `logos-scaffold spel -- inspect <bin>`"
        ));
    }
    if quiet_seen {
        set_command_echo(false);
    }
    Ok(Some(args[sep_idx + 1..].to_vec()))
}

fn wallet_passthrough_action(args: &[String], start: usize) -> DynResult<Option<WalletAction>> {
    if args.len() <= start || args[start] != "wallet" {
        return Ok(None);
    }
    let (sep_idx, quiet_seen) = skip_inline_quiet(args, start + 1);
    // Only intercept as passthrough when the next token is `--`; otherwise
    // it's `wallet list`, `wallet topup`, etc. — let clap parse it. This
    // also means a stray `lgs wallet -q` (no `--`) falls through to clap,
    // which surfaces a normal "missing subcommand" error rather than us
    // hijacking it.
    if sep_idx >= args.len() || args[sep_idx] != "--" {
        return Ok(None);
    }
    if args.len() == sep_idx + 1 {
        return Err(anyhow!(
            "wallet passthrough requires at least one argument after `--`. Example: `logos-scaffold wallet -- account list`. Discover inner flags with: `logos-scaffold wallet -- --help` (from a project directory)."
        ));
    }
    if quiet_seen {
        set_command_echo(false);
    }
    Ok(Some(WalletAction::Proxy {
        args: args[sep_idx + 1..].to_vec(),
    }))
}

fn merge_optional_address(
    positional: Option<String>,
    flagged: Option<String>,
    context: &str,
) -> DynResult<Option<String>> {
    if positional.is_some() && flagged.is_some() {
        return Err(anyhow!(
            "{context}: provide address either as positional argument or `--address`, not both."
        ));
    }

    Ok(positional.or(flagged))
}

fn require_address(
    positional: Option<String>,
    flagged: Option<String>,
    context: &str,
) -> DynResult<String> {
    let merged = merge_optional_address(positional, flagged, context)?;
    merged.ok_or_else(|| {
        anyhow!(
            "{context} requires an address. Examples: `logos-scaffold wallet default set <address>` or `logos-scaffold wallet default set --address <address>`."
        )
    })
}
