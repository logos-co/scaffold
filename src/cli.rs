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
use crate::commands::testnode::{cmd_test_node, TestNodeAction};
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
    #[command(
        about = "Sync dependencies and build project-local binaries (sequencer, wallet, spel)"
    )]
    Setup(SetupArgs),
    #[command(about = "Build the project workspace and guest programs")]
    Build(BuildArgs),
    #[command(about = "Deploy guest programs to the running localnet")]
    Deploy(DeployArgs),
    #[command(about = "Manage the local sequencer (start, stop, status, logs, reset)")]
    Localnet(LocalnetArgs),
    #[command(
        name = "test-node",
        about = "Manage isolated, short-lived sequencer test nodes for integration tests"
    )]
    TestNode(TestNodeArgs),
    #[command(about = "Manage project wallet accounts and faucet top-ups")]
    Wallet(WalletArgs),
    /// `spel` is dispatched via the early `spel_passthrough_args` intercept
    /// before clap parses argv (the user types `lgs spel -- <args>`). The
    /// variant exists so clap lists `spel` in `--help` output and renders
    /// the `long_about` for `lgs spel --help`. The match arm below is
    /// unreachable at runtime.
    #[command(
        about = "Forward arguments to the project-vendored `spel` binary",
        long_about = "Forward arguments to the project-vendored `spel` binary.\n\n\
                      Use the form: `logos-scaffold spel -- <spel-subcommand...>`\n\n\
                      Examples:\n  \
                      logos-scaffold spel -- inspect path/to/program.bin\n  \
                      logos-scaffold spel -- generate-idl"
    )]
    Spel(SpelArgs),
    #[command(about = "Manage pre-seeded basecamp profiles for p2p dogfooding")]
    Basecamp(BasecampArgs),
    #[command(about = "Check project health and report actionable next steps")]
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
    /// Override the cache root used for non-vendored dependencies. Written to
    /// scaffold.toml so subsequent commands reuse the same location.
    #[arg(long, value_name = "PATH")]
    cache_root: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
#[command(after_long_help = EXAMPLES_SETUP)]
struct SetupArgs {
    /// Download prebuilt binaries instead of compiling from source.
    /// Falls back to source build if no prebuilt exists for the pinned commit.
    #[arg(long, default_value_t = false)]
    prebuilt: bool,
}

#[derive(Debug, clap::Args)]
#[command(after_long_help = EXAMPLES_BUILD)]
struct BuildArgs {
    /// Download prebuilt binaries instead of compiling from source.
    /// Falls back to source build if no prebuilt exists for the pinned commit.
    #[arg(long, default_value_t = false)]
    prebuilt: bool,
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
    /// Select a named profile from `[run.profiles.<name>]`
    #[arg(long, value_name = "NAME")]
    profile: Option<String>,
    /// Wipe rocksdb + wallet, restart sequencer, re-seed default wallet
    /// (overrides scaffold.toml). Broader than `lgs localnet reset`: this
    /// re-establishes the documented fresh-project state for the full
    /// deploy cycle.
    #[arg(long)]
    reset: bool,
    /// Skip the run-level reset even if scaffold.toml says true
    #[arg(long, conflicts_with = "reset")]
    no_reset: bool,
    /// Skip post-deploy hooks even if the resolved profile defines them
    #[arg(long)]
    no_post_deploy: bool,
    /// Override post-deploy hooks (repeatable). Replaces config-defined hooks
    /// for this invocation. Conflicts with --no-post-deploy.
    #[arg(long, value_name = "CMD", conflicts_with = "no_post_deploy")]
    post_deploy: Vec<String>,
    #[arg(long, value_name = "SECS", help = RUN_LOCALNET_TIMEOUT_HELP.as_str())]
    localnet_timeout: Option<u64>,
    /// After the initial run, watch the project for file changes and
    /// re-run the pipeline (build + idl + deploy + hooks) on each change.
    /// Localnet is reused; reset is skipped on re-runs.
    #[arg(long)]
    watch: bool,
    /// Override the `--watch` debounce window (milliseconds) for this
    /// invocation. Defaults to `[run.watch].debounce_ms`, else 500.
    /// Only meaningful with `--watch`, so it requires it.
    #[arg(long, value_name = "MS", requires = "watch")]
    watch_debounce_ms: Option<u64>,
}

#[derive(Debug, clap::Args)]
struct LocalnetArgs {
    #[command(subcommand)]
    command: LocalnetSubcommand,
}

#[derive(Debug, clap::Args)]
struct TestNodeArgs {
    #[command(subcommand)]
    command: TestNodeSubcommand,
}

#[derive(Debug, Subcommand)]
enum TestNodeSubcommand {
    #[command(
        about = "Report the LEZ and circuits pins test-node commands will use for this project"
    )]
    Pins(TestNodePinsArgs),
    #[command(
        about = "Resolve the project's LEZ/circuits pins, build the standalone sequencer for them, and fetch circuits"
    )]
    Prepare(TestNodePrepareArgs),
    #[command(
        about = "Check test-node prerequisites: pins, checkout state, sequencer binary, circuits, platform"
    )]
    Doctor(TestNodeDoctorArgs),
    #[command(about = "Start an isolated sequencer test node with its own port, state, and logs")]
    Start(TestNodeStartArgs),
    #[command(about = "Report whether a test node is healthy and which RPC URL it serves")]
    Status(TestNodeStatusArgs),
    #[command(about = "Stop a test node and remove its runtime state")]
    Stop(TestNodeStopArgs),
    #[command(
        about = "Start a node, run a command with its connection exported, then stop the node",
        long_about = "Start a test node, wait until it is healthy, run the given command with the \
                      node's connection details exported (LGS_TEST_NODE_RPC_URL, \
                      LGS_TEST_NODE_PORT, LGS_TEST_NODE_STATE_DIR, ...), forward the command's \
                      exit status, and stop the node when the command exits.\n\n\
                      Example:\n  lgs test-node run --serial -- cargo test --test sequencer_it"
    )]
    Run(TestNodeRunArgs),
    #[command(
        about = "Submit transactions and observe definitive committed/rejected/timeout outcomes"
    )]
    Tx(TestNodeTxArgs),
    #[command(about = "Inspect blocks: head, ranges, and waiting for block production")]
    Blocks(TestNodeBlocksArgs),
    #[command(about = "Read the sequencer clock accounts at a stable boundary")]
    Clock(TestNodeClockArgs),
    #[command(about = "Block-scoped account reads for parity assertions")]
    Account(TestNodeAccountArgs),
    #[command(about = "Block-scoped membership-proof reads")]
    Proof(TestNodeProofArgs),
    #[command(about = "Write account snapshots for later comparison or seeding")]
    Snapshot(TestNodeSnapshotArgs),
    #[command(about = "Seed test nodes from caller-provided state snapshots")]
    State(TestNodeStateArgs),
}

#[derive(Debug, clap::Args)]
struct TestNodeStateArgs {
    #[command(subcommand)]
    command: TestNodeStateSubcommand,
}

#[derive(Debug, Subcommand)]
enum TestNodeStateSubcommand {
    #[command(about = "Identify the exact state snapshot formats the current project pins accept")]
    Schema(TestNodeStateSchemaArgs),
    #[command(
        about = "Export named public accounts from a node into a state snapshot file",
        long_about = "Export named public accounts from a running node into an \
                      lgs-state-snapshot/1 JSON file usable by `state seed`.\n\n\
                      The pinned sequencer RPC exposes public account balances only (no \
                      enumeration, no private state). For a complete, full-fidelity state \
                      snapshot, stop a node with --preserve-work-dir and pass its state \
                      directory to `state seed` instead."
    )]
    Export(TestNodeStateExportArgs),
    #[command(
        about = "Validate a snapshot and produce a state directory for `test-node start --state`"
    )]
    Seed(TestNodeStateSeedArgs),
}

#[derive(Debug, clap::Args)]
struct TestNodeStateSchemaArgs {
    /// Project root (default: discover from the current directory).
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeStateExportArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    /// Base58 account ids to export (repeat the flag per account).
    #[arg(long = "account-id", value_name = "ID", required = true)]
    account_ids: Vec<String>,
    /// Output snapshot file (JSON, lgs-state-snapshot/1).
    #[arg(long, value_name = "PATH")]
    output: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeStateSeedArgs {
    /// Project root (default: discover from the current directory).
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
    /// Snapshot file (lgs-state-snapshot/1 or lgs-account-snapshot/1) or a
    /// state directory containing a rocksdb database.
    #[arg(long, value_name = "PATH")]
    input: PathBuf,
    /// Output state directory (default: .scaffold/test-nodes/seeds/<id>).
    #[arg(long, value_name = "DIR")]
    output: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeAccountArgs {
    #[command(subcommand)]
    command: TestNodeAccountSubcommand,
}

#[derive(Debug, Subcommand)]
enum TestNodeAccountSubcommand {
    #[command(about = "Read one account at a stable block boundary")]
    Get(TestNodeAccountGetArgs),
    #[command(
        name = "batch-get",
        about = "Read several accounts at ONE consistent block boundary"
    )]
    BatchGet(TestNodeAccountBatchGetArgs),
}

#[derive(Debug, clap::Args)]
struct TestNodeAccountGetArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    /// Base58 account id.
    #[arg(long, value_name = "ID")]
    account_id: String,
    /// Require the read to happen exactly at this block (default: latest,
    /// with a head-stability barrier).
    #[arg(long, value_name = "BLOCK")]
    at_block: Option<u64>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeAccountBatchGetArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    /// Base58 account ids (repeat the flag per account).
    #[arg(long = "account-id", value_name = "ID", required = true)]
    account_ids: Vec<String>,
    /// Require the reads to happen exactly at this block (default: latest,
    /// with a head-stability barrier).
    #[arg(long, value_name = "BLOCK")]
    at_block: Option<u64>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeProofArgs {
    #[command(subcommand)]
    command: TestNodeProofSubcommand,
}

#[derive(Debug, Subcommand)]
enum TestNodeProofSubcommand {
    #[command(about = "Read the membership proof for a commitment at a stable block boundary")]
    Get(TestNodeProofGetArgs),
}

#[derive(Debug, clap::Args)]
struct TestNodeProofGetArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    /// Commitment (64 hex chars or base58 of 32 bytes).
    #[arg(long, value_name = "COMMITMENT")]
    commitment: String,
    /// Require the read to happen exactly at this block (default: latest,
    /// with a head-stability barrier).
    #[arg(long, value_name = "BLOCK")]
    at_block: Option<u64>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeSnapshotArgs {
    #[command(subcommand)]
    command: TestNodeSnapshotSubcommand,
}

#[derive(Debug, Subcommand)]
enum TestNodeSnapshotSubcommand {
    #[command(about = "Write a block-consistent account snapshot to a JSON file")]
    Accounts(TestNodeSnapshotAccountsArgs),
}

#[derive(Debug, clap::Args)]
struct TestNodeSnapshotAccountsArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    /// Base58 account ids (repeat the flag per account).
    #[arg(long = "account-id", value_name = "ID", required = true)]
    account_ids: Vec<String>,
    /// Output JSON file.
    #[arg(long, value_name = "PATH")]
    output: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeBlocksArgs {
    #[command(subcommand)]
    command: TestNodeBlocksSubcommand,
}

#[derive(Debug, Subcommand)]
enum TestNodeBlocksSubcommand {
    #[command(about = "Current head block: id, timestamp, and transaction summaries")]
    Head(TestNodeBlocksHeadArgs),
    #[command(
        about = "Inclusive block range with per-block clock/user transaction classification"
    )]
    Range(TestNodeBlocksRangeArgs),
    #[command(about = "Wait for a number of blocks after a known boundary, then print them")]
    Wait(TestNodeBlocksWaitArgs),
}

#[derive(Debug, clap::Args)]
struct TestNodeBlocksHeadArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeBlocksRangeArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    #[arg(long, value_name = "BLOCK")]
    from: u64,
    #[arg(long, value_name = "BLOCK")]
    to: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeBlocksWaitArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    /// Block boundary; only blocks after this id are returned.
    #[arg(long, value_name = "BLOCK")]
    after: u64,
    /// How many blocks after the boundary to wait for.
    #[arg(long, default_value_t = 1)]
    count: u64,
    /// Seconds to wait for the blocks to be produced.
    #[arg(long, default_value_t = 60)]
    timeout_sec: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeClockArgs {
    #[command(subcommand)]
    command: TestNodeClockSubcommand,
}

#[derive(Debug, Subcommand)]
enum TestNodeClockSubcommand {
    #[command(about = "Read all clock accounts at the current head")]
    Read(TestNodeClockReadArgs),
    #[command(
        name = "wait-stable",
        about = "Read the clock accounts behind a stability barrier (consecutive identical samples)"
    )]
    WaitStable(TestNodeClockWaitStableArgs),
}

#[derive(Debug, clap::Args)]
struct TestNodeClockReadArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeClockWaitStableArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    /// Consecutive identical samples required for a stable snapshot.
    #[arg(long, default_value_t = 2)]
    samples: u32,
    /// Seconds before giving up with a retryable error.
    #[arg(long, default_value_t = 30)]
    timeout_sec: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeTxArgs {
    #[command(subcommand)]
    command: TestNodeTxSubcommand,
}

#[derive(Debug, Subcommand)]
enum TestNodeTxSubcommand {
    #[command(
        about = "Submit a transaction; prints the tx hash or a structured stateless rejection"
    )]
    Submit(TestNodeTxSubmitArgs),
    #[command(
        about = "Wait for a definitive outcome of a submitted transaction (committed, rejected, timeout)"
    )]
    Wait(TestNodeTxWaitArgs),
    #[command(
        name = "submit-and-wait",
        about = "Submit a transaction and wait for exactly one terminal outcome"
    )]
    SubmitAndWait(TestNodeTxSubmitAndWaitArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum TxEncodingArg {
    /// File contains base64 text of the transaction's borsh bytes.
    BorshBase64,
    /// File contains the raw borsh bytes.
    Borsh,
}

impl TxEncodingArg {
    fn into_encoding(self) -> crate::commands::testnode::TxEncoding {
        match self {
            Self::BorshBase64 => crate::commands::testnode::TxEncoding::BorshBase64,
            Self::Borsh => crate::commands::testnode::TxEncoding::Borsh,
        }
    }
}

#[derive(Debug, clap::Args)]
struct TestNodeTxSubmitArgs {
    /// Sequencer JSON-RPC URL (e.g. from `test-node start --json`).
    #[arg(long, value_name = "URL")]
    url: String,
    /// Transaction file.
    #[arg(long, value_name = "PATH")]
    file: PathBuf,
    /// Encoding of the transaction file.
    #[arg(long, value_enum, default_value_t = TxEncodingArg::BorshBase64)]
    encoding: TxEncodingArg,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeTxWaitArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    /// Transaction hash returned by submit.
    #[arg(long, value_name = "HASH")]
    hash: String,
    /// Only observe the transaction after this block boundary.
    #[arg(long, value_name = "BLOCK")]
    after_block: Option<u64>,
    /// Seconds to wait for a terminal outcome.
    #[arg(long, default_value_t = 60)]
    timeout_sec: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeTxSubmitAndWaitArgs {
    /// Sequencer JSON-RPC URL.
    #[arg(long, value_name = "URL")]
    url: String,
    /// Transaction file.
    #[arg(long, value_name = "PATH")]
    file: PathBuf,
    /// Encoding of the transaction file.
    #[arg(long, value_enum, default_value_t = TxEncodingArg::BorshBase64)]
    encoding: TxEncodingArg,
    /// Seconds to wait for a terminal outcome.
    #[arg(long, default_value_t = 60)]
    timeout_sec: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodePinsArgs {
    /// Project root (default: discover from the current directory).
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
    /// Override the LEZ source (clone URL or local checkout directory).
    #[arg(long, value_name = "URL|DIR")]
    lez_source: Option<String>,
    /// Override the LEZ ref (SHA, tag, or branch).
    #[arg(long, value_name = "REF")]
    lez_ref: Option<String>,
    /// Override the circuits release version.
    #[arg(long, value_name = "VER")]
    circuits_version: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodePrepareArgs {
    /// Project root (default: discover from the current directory).
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
    /// Override the cache root used for managed checkouts and circuits.
    #[arg(long, value_name = "DIR")]
    cache_root: Option<PathBuf>,
    /// Override the LEZ source (clone URL or local checkout directory).
    /// Local checkouts are validated, never modified.
    #[arg(long, value_name = "URL|DIR")]
    lez_source: Option<String>,
    /// Override the LEZ ref (SHA, tag, or branch).
    #[arg(long, value_name = "REF")]
    lez_ref: Option<String>,
    /// Override the circuits release version.
    #[arg(long, value_name = "VER")]
    circuits_version: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeDoctorArgs {
    /// Project root (default: discover from the current directory).
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeStartArgs {
    /// Project root (default: discover from the current directory).
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
    /// Pre-seeded state directory (containing a rocksdb database) to start from.
    #[arg(long, value_name = "DIR")]
    state: Option<PathBuf>,
    /// RPC port. 0 (the default) picks an unused localhost port.
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Runtime directory for this node (default: .scaffold/test-nodes/<id>).
    #[arg(long, value_name = "DIR")]
    work_dir: Option<PathBuf>,
    /// Keep the runtime directory when the node is stopped.
    #[arg(long)]
    preserve_work_dir: bool,
    /// Seconds to wait for the node to become healthy.
    #[arg(long, default_value_t = crate::testnode::DEFAULT_TEST_NODE_TIMEOUT_SEC)]
    timeout_sec: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeStatusArgs {
    /// Node id (directory name under .scaffold/test-nodes/) or runtime dir path.
    #[arg(long, value_name = "ID|DIR")]
    node: String,
    /// Project root (default: discover from the current directory).
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeStopArgs {
    /// Node id (directory name under .scaffold/test-nodes/) or runtime dir path.
    #[arg(long, value_name = "ID|DIR")]
    node: String,
    /// Project root (default: discover from the current directory).
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
    /// Keep the runtime directory instead of removing it.
    #[arg(long)]
    preserve_work_dir: bool,
}

#[derive(Debug, clap::Args)]
struct TestNodeRunArgs {
    /// Project root (default: discover from the current directory).
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
    /// Pre-seeded state directory (containing a rocksdb database) to start from.
    #[arg(long, value_name = "DIR")]
    state: Option<PathBuf>,
    /// Low-resource CI path: at most one test node at a time (machine-wide).
    #[arg(long, conflicts_with = "parallel")]
    serial: bool,
    /// Cap concurrent test-node creation at N (machine-wide).
    #[arg(long, value_name = "N")]
    parallel: Option<usize>,
    /// Seconds to wait for the node to become healthy before running the command.
    #[arg(long, default_value_t = crate::testnode::DEFAULT_TEST_NODE_TIMEOUT_SEC)]
    timeout_sec: u64,
    /// Command (after `--`) to run with the node's connection exported.
    #[arg(last = true, required = true)]
    command: Vec<String>,
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
    /// Emit the tailed log lines as a JSON object instead of plain text.
    #[arg(long)]
    json: bool,
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

#[derive(Debug, clap::Args)]
struct SpelArgs {
    /// Trailing args forwarded to the project-vendored `spel` binary.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
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
    /// Emit accounts as a JSON object instead of forwarding the wallet's text.
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
    /// Emit the topup outcome as a JSON object instead of human-readable text.
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
        about = "Launch basecamp for a named profile with clean-slate semantics. Scrubs the profile's xdg-data/xdg-cache and replays the captured module set on every invocation. See `basecamp docs` for project requirements."
    )]
    Launch(BasecampLaunchArgs),
    #[command(
        about = "Enter a module's Nix dev shell (`nix develop`) resolved from [modules.<name>]. Verb-set symmetry so contributors stop reaching for raw `nix`."
    )]
    Develop(BasecampDevelopArgs),
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
}

#[derive(Debug, clap::Args)]
struct BasecampDevelopArgs {
    /// Name of the module to enter, keyed in `[modules.<name>]`.
    #[arg(value_name = "MODULE")]
    module: String,
    /// Dev-shell attribute to select (`nix develop <flake>#<attr>`). Defaults
    /// to the flake's default dev shell.
    #[arg(long, value_name = "ATTR")]
    dev_shell: Option<String>,
}

/// Passthrough args for `build idl` / `build client`: the `build` verb
/// followed by an optional project path.
fn build_args(project_path: Option<PathBuf>) -> Vec<String> {
    let mut args = vec!["build".to_string()];
    args.extend(project_path.map(|p| p.to_string_lossy().into_owned()));
    args
}

pub(crate) fn run(args: Vec<String>) -> DynResult<()> {
    apply_quiet_from_env();
    let passthrough_start = leading_global_flags_end(&args);
    if passthrough_start > 1 {
        set_command_echo(false);
    }

    let bin_name = args
        .first()
        .and_then(|s| std::path::Path::new(s).file_name())
        .and_then(|f| f.to_str())
        .unwrap_or("logos-scaffold")
        .to_string();

    if is_spel_help_request(&args, passthrough_start) {
        return print_spel_help(&bin_name);
    }
    if let Some(action) = wallet_passthrough_action(&args, passthrough_start)? {
        return cmd_wallet(action);
    }
    if let Some(spel_args) = spel_passthrough_args(&args, passthrough_start)? {
        return cmd_spel(&spel_args);
    }

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
                return Err(anyhow!("{msg}"));
            }
        },
    };

    if cli.quiet {
        set_command_echo(false);
    }

    match cli.command {
        Some(Commands::Create(args) | Commands::New(args)) => cmd_new(NewCommand {
            name: args.name,
            vendor_deps: args.vendor_deps,
            lez_path: args.lez_path,
            template: args.template,
            cache_root: args.cache_root,
        }),
        Some(Commands::Setup(args)) => cmd_setup(args.prebuilt),
        Some(Commands::Build(args)) => match args.subcommand {
            Some(BuildSubcommand::Idl(sub)) => cmd_idl(&build_args(sub.project_path)),
            Some(BuildSubcommand::Client(sub)) => cmd_client(&build_args(sub.project_path)),
            None => cmd_build_shortcut(args.project_path, args.prebuilt),
        },
        Some(Commands::Deploy(args)) => cmd_deploy(args.program_name, args.program_path, args.json),
        Some(Commands::Localnet(localnet)) => {
            let action = match localnet.command {
                LocalnetSubcommand::Start(args) => LocalnetAction::Start {
                    timeout_sec: args.timeout_sec,
                },
                LocalnetSubcommand::Stop => LocalnetAction::Stop,
                LocalnetSubcommand::Status(args) => LocalnetAction::Status { json: args.json },
                LocalnetSubcommand::Logs(args) => LocalnetAction::Logs {
                    tail: args.tail,
                    json: args.json,
                },
                LocalnetSubcommand::Reset(args) => LocalnetAction::Reset {
                    dry_run: args.dry_run,
                    yes: args.yes,
                    reset_wallet: args.reset_wallet,
                    verify_timeout_sec: args.verify_timeout_sec,
                },
            };
            cmd_localnet(action)
        }
        Some(Commands::TestNode(test_node)) => {
            let action = match test_node.command {
                TestNodeSubcommand::Pins(args) => TestNodeAction::Pins {
                    project: args.project,
                    overrides: crate::testnode::pins::PinOverrides {
                        lez_source: args.lez_source,
                        lez_ref: args.lez_ref,
                        circuits_version: args.circuits_version,
                    },
                    json: args.json,
                },
                TestNodeSubcommand::Prepare(args) => TestNodeAction::Prepare {
                    project: args.project,
                    overrides: crate::testnode::pins::PinOverrides {
                        lez_source: args.lez_source,
                        lez_ref: args.lez_ref,
                        circuits_version: args.circuits_version,
                    },
                    cache_root: args.cache_root,
                    json: args.json,
                },
                TestNodeSubcommand::Doctor(args) => TestNodeAction::Doctor {
                    project: args.project,
                    json: args.json,
                },
                TestNodeSubcommand::Start(args) => TestNodeAction::Start {
                    project: args.project,
                    state: args.state,
                    port: args.port,
                    work_dir: args.work_dir,
                    preserve_work_dir: args.preserve_work_dir,
                    timeout_sec: args.timeout_sec,
                    json: args.json,
                },
                TestNodeSubcommand::Status(args) => TestNodeAction::Status {
                    project: args.project,
                    node: args.node,
                    json: args.json,
                },
                TestNodeSubcommand::Stop(args) => TestNodeAction::Stop {
                    project: args.project,
                    node: args.node,
                    preserve_work_dir: args.preserve_work_dir,
                },
                TestNodeSubcommand::Run(args) => TestNodeAction::Run {
                    project: args.project,
                    state: args.state,
                    serial: args.serial,
                    parallel: args.parallel,
                    timeout_sec: args.timeout_sec,
                    command: args.command,
                },
                TestNodeSubcommand::Tx(tx) => match tx.command {
                    TestNodeTxSubcommand::Submit(args) => TestNodeAction::TxSubmit {
                        url: args.url,
                        file: args.file,
                        encoding: args.encoding.into_encoding(),
                        json: args.json,
                    },
                    TestNodeTxSubcommand::Wait(args) => TestNodeAction::TxWait {
                        url: args.url,
                        hash: args.hash,
                        after_block: args.after_block,
                        timeout_sec: args.timeout_sec,
                        json: args.json,
                    },
                    TestNodeTxSubcommand::SubmitAndWait(args) => TestNodeAction::TxSubmitAndWait {
                        url: args.url,
                        file: args.file,
                        encoding: args.encoding.into_encoding(),
                        timeout_sec: args.timeout_sec,
                        json: args.json,
                    },
                },
                TestNodeSubcommand::Blocks(blocks) => match blocks.command {
                    TestNodeBlocksSubcommand::Head(args) => TestNodeAction::BlocksHead {
                        url: args.url,
                        json: args.json,
                    },
                    TestNodeBlocksSubcommand::Range(args) => TestNodeAction::BlocksRange {
                        url: args.url,
                        from: args.from,
                        to: args.to,
                        json: args.json,
                    },
                    TestNodeBlocksSubcommand::Wait(args) => TestNodeAction::BlocksWait {
                        url: args.url,
                        after: args.after,
                        count: args.count,
                        timeout_sec: args.timeout_sec,
                        json: args.json,
                    },
                },
                TestNodeSubcommand::Clock(clock) => match clock.command {
                    TestNodeClockSubcommand::Read(args) => TestNodeAction::ClockRead {
                        url: args.url,
                        json: args.json,
                    },
                    TestNodeClockSubcommand::WaitStable(args) => TestNodeAction::ClockWaitStable {
                        url: args.url,
                        samples: args.samples,
                        timeout_sec: args.timeout_sec,
                        json: args.json,
                    },
                },
                TestNodeSubcommand::Account(account) => match account.command {
                    TestNodeAccountSubcommand::Get(args) => TestNodeAction::AccountGet {
                        url: args.url,
                        account_id: args.account_id,
                        at_block: args.at_block,
                        json: args.json,
                    },
                    TestNodeAccountSubcommand::BatchGet(args) => TestNodeAction::AccountBatchGet {
                        url: args.url,
                        account_ids: args.account_ids,
                        at_block: args.at_block,
                        json: args.json,
                    },
                },
                TestNodeSubcommand::Proof(proof) => match proof.command {
                    TestNodeProofSubcommand::Get(args) => TestNodeAction::ProofGet {
                        url: args.url,
                        commitment: args.commitment,
                        at_block: args.at_block,
                        json: args.json,
                    },
                },
                TestNodeSubcommand::Snapshot(snapshot) => match snapshot.command {
                    TestNodeSnapshotSubcommand::Accounts(args) => {
                        TestNodeAction::SnapshotAccounts {
                            url: args.url,
                            account_ids: args.account_ids,
                            output: args.output,
                            json: args.json,
                        }
                    }
                },
                TestNodeSubcommand::State(state) => match state.command {
                    TestNodeStateSubcommand::Schema(args) => TestNodeAction::StateSchema {
                        project: args.project,
                        json: args.json,
                    },
                    TestNodeStateSubcommand::Export(args) => TestNodeAction::StateExport {
                        url: args.url,
                        account_ids: args.account_ids,
                        output: args.output,
                        json: args.json,
                    },
                    TestNodeStateSubcommand::Seed(args) => TestNodeAction::StateSeed {
                        project: args.project,
                        input: args.input,
                        output: args.output,
                        json: args.json,
                    },
                },
            };
            cmd_test_node(action)
        }
        Some(Commands::Spel(_)) => {
            // The early `spel_passthrough_args` intercept above always
            // wins for the runtime `spel -- <args>` form. The clap variant
            // exists only so `--help` lists `spel` and renders its
            // `long_about`; reaching this arm means clap parsed `spel`
            // without `--`, which the intercept would already have
            // rejected with a hint. Kept as `unreachable!` so a future
            // refactor of the intercept surfaces immediately rather than
            // silently no-op'ing.
            unreachable!("spel is intercepted by spel_passthrough_args before clap parses argv")
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
                },
                BasecampSubcommand::Develop(args) => BasecampAction::Develop {
                    module: args.module,
                    dev_shell: args.dev_shell,
                },
                BasecampSubcommand::BuildPortable(_) => BasecampAction::BuildPortable,
                BasecampSubcommand::Doctor(args) => BasecampAction::Doctor { json: args.json },
                BasecampSubcommand::Docs => BasecampAction::Docs,
            };
            cmd_basecamp(action)
        }
        Some(Commands::Doctor(args)) => cmd_doctor(args.json),
        Some(Commands::Run(args)) => {
            let reset = if args.reset {
                Some(true)
            } else if args.no_reset {
                Some(false)
            } else {
                None
            };
            let post_deploy = if args.no_post_deploy {
                Some(Vec::new())
            } else if !args.post_deploy.is_empty() {
                Some(args.post_deploy)
            } else {
                None
            };
            cmd_run(RunInvocation {
                profile: args.profile,
                reset,
                post_deploy_override: post_deploy,
                localnet_timeout_sec: args.localnet_timeout,
                watch: args.watch,
                watch_debounce_ms: args.watch_debounce_ms,
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

fn print_spel_help(bin_name: &str) -> DynResult<()> {
    let mut cmd = Cli::command().bin_name(bin_name);
    if let Some(spel) = cmd.find_subcommand_mut("spel") {
        spel.print_long_help()?;
        println!();
        Ok(())
    } else {
        print_help(bin_name)
    }
}

fn is_spel_help_request(args: &[String], start: usize) -> bool {
    args.len() > start + 1 && args[start] == "spel" && is_help_token(&args[start + 1])
}

fn is_help_token(token: &str) -> bool {
    matches!(token, "--help" | "-h" | "help" | "-?")
}

fn apply_quiet_from_env() {
    if std::env::var("LOGOS_SCAFFOLD_QUIET").is_ok_and(|v| {
        v == "1"
            || v.eq_ignore_ascii_case("true")
            || v.eq_ignore_ascii_case("yes")
            || v.eq_ignore_ascii_case("on")
    }) {
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
    // Help spellings are handled before this function so the user sees the
    // variant's `long_about` instead of the passthrough hint.
    if args.len() > start + 1 && is_help_token(&args[start + 1]) {
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
