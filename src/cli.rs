use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::anyhow;
use clap::{CommandFactory, Parser, Subcommand};

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
    disable_help_subcommand = true
)]
struct Cli {
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
    Completions(CompletionsArgs),
    #[command(about = "Initialize scaffold.toml in the current directory")]
    Init,
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
    /// Run `/bin/false` instead of `/bin/true` — exercises the failure bail.
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
struct SetupArgs {}

#[derive(Debug, clap::Args)]
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
struct DeployArgs {
    program_name: Option<String>,
    /// Path to a custom ELF binary to deploy directly (bypasses auto-discovery)
    #[arg(long, value_name = "PATH")]
    program_path: Option<PathBuf>,
    /// Output result as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct DoctorArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
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
    Start(LocalnetStartArgs),
    Stop,
    Status(LocalnetStatusArgs),
    Logs(LocalnetLogsArgs),
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
    /// Also delete the wallet home directory and wallet state. Destructive:
    /// keypairs are not recoverable after this.
    #[arg(long)]
    reset_wallet: bool,

    /// Seconds to wait for the restarted sequencer to produce a block.
    #[arg(long, default_value_t = 30)]
    verify_timeout_sec: u64,
}

#[derive(Debug, clap::Args)]
struct WalletArgs {
    #[command(subcommand)]
    command: WalletSubcommand,
}

#[derive(Debug, Subcommand)]
enum WalletSubcommand {
    #[command(about = "List wallet accounts (same as `wallet account list`)")]
    List(WalletListArgs),
    #[command(about = "Top up wallet using pinata faucet claim")]
    Topup(WalletTopupArgs),
    #[command(about = "Manage project default wallet")]
    Default(WalletDefaultArgs),
}

#[derive(Debug, clap::Args)]
struct WalletListArgs {
    #[arg(long)]
    long: bool,
}

#[derive(Debug, clap::Args)]
struct WalletTopupArgs {
    #[arg(value_name = "ADDRESS")]
    address: Option<String>,
    #[arg(long = "address", value_name = "ADDRESS")]
    address_flag: Option<String>,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, clap::Args)]
struct WalletDefaultArgs {
    #[command(subcommand)]
    command: WalletDefaultSubcommand,
}

#[derive(Debug, Subcommand)]
enum WalletDefaultSubcommand {
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
        about = "Launch basecamp for a named profile with clean-slate semantics. See `basecamp docs` for project requirements."
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
}

pub(crate) fn run(args: Vec<String>) -> DynResult<()> {
    if let Some(action) = wallet_passthrough_action(&args)? {
        return cmd_wallet(action);
    }
    if let Some(spel_args) = spel_passthrough_args(&args)? {
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
            _ => return Err(anyhow!(err.to_string())),
        },
    };

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
                    reset_wallet: args.reset_wallet,
                    verify_timeout_sec: args.verify_timeout_sec,
                },
            };
            cmd_localnet(action)
        }
        Some(Commands::Wallet(args)) => {
            let action = match args.command {
                WalletSubcommand::List(args) => WalletAction::List { long: args.long },
                WalletSubcommand::Topup(args) => WalletAction::Topup {
                    address: merge_optional_address(
                        args.address,
                        args.address_flag,
                        "wallet topup",
                    )?,
                    dry_run: args.dry_run,
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
        Some(Commands::Init) => cmd_init(&bin_name),
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

/// Forward `lgs spel -- <args...>` to the project-vendored `spel` binary.
/// Mirrors `wallet_passthrough_action` so the same `--` convention applies
/// across passthroughs. When `spel` is invoked without `--`, intercept early
/// and surface a hint pointing at the right form — clap's "unknown
/// subcommand" message would otherwise leave the user guessing.
fn spel_passthrough_args(args: &[String]) -> DynResult<Option<Vec<String>>> {
    if args.len() < 2 || args[1] != "spel" {
        return Ok(None);
    }
    if args.len() < 3 {
        return Err(anyhow!(
            "`spel` requires arguments. Use the passthrough form, e.g. `logos-scaffold spel -- inspect <bin>`."
        ));
    }
    if args[2] != "--" {
        return Err(anyhow!(
            "`spel {0} ...` is not a scaffold subcommand. Did you mean `logos-scaffold spel -- {0} ...`? \
             The `--` separator forwards every following argument to the project-vendored `spel` binary.",
            args[2]
        ));
    }
    if args.len() == 3 {
        return Err(anyhow!(
            "spel passthrough requires at least one argument after `--`. Example: `logos-scaffold spel -- inspect <bin>`"
        ));
    }
    Ok(Some(args[3..].to_vec()))
}

fn wallet_passthrough_action(args: &[String]) -> DynResult<Option<WalletAction>> {
    if args.len() < 3 {
        return Ok(None);
    }

    if args[1] == "wallet" && args[2] == "--" {
        if args.len() == 3 {
            return Err(anyhow!(
                "wallet passthrough requires at least one argument after `--`. Example: `logos-scaffold wallet -- account list`"
            ));
        }

        return Ok(Some(WalletAction::Proxy {
            args: args[3..].to_vec(),
        }));
    }

    Ok(None)
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
