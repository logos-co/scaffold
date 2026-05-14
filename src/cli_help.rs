pub(crate) const EXAMPLES_ROOT: &str = r"Examples:
  logos-scaffold create my-app
  logos-scaffold init
  logos-scaffold setup
  logos-scaffold build
  logos-scaffold deploy --json
  logos-scaffold localnet start
  logos-scaffold localnet status --json
  logos-scaffold doctor --json

Environment:
  LOGOS_SCAFFOLD_QUIET   If set to 1, true, yes, or on, suppresses echoed external commands (same as --quiet).
  LOGOS_SCAFFOLD_WALLET_PASSWORD   Optional password for wallet subprocess stdin (see wallet and deploy help).";

pub(crate) const EXAMPLES_CREATE: &str = r"Examples:
  logos-scaffold create my-app
  logos-scaffold new my-app --template default
  logos-scaffold create my-app --vendor-deps --lez-path /abs/path/to/lez
  logos-scaffold create my-app --cache-root ~/.cache/logos-scaffold";

pub(crate) const EXAMPLES_INIT: &str = r"Examples:
  logos-scaffold init
  logos-scaffold init --dry-run
  logos-scaffold init --no-backup
  cd my-project && logos-scaffold init

Migration safety:
  When scaffold.toml exists at an older schema, `init` rewrites it in place.
  By default a sibling scaffold.toml.bak is written first; pass --no-backup
  to skip. Use --dry-run to preview the change without touching disk.";

pub(crate) const EXAMPLES_SETUP: &str = r"Examples:
  logos-scaffold setup

Run inside a project directory that contains scaffold.toml (after create or init).";

pub(crate) const EXAMPLES_BUILD: &str = r"Examples:
  logos-scaffold build
  logos-scaffold build ./my-project
  logos-scaffold build idl
  logos-scaffold build idl ./my-project
  logos-scaffold build client
  logos-scaffold build client ./my-project";

pub(crate) const EXAMPLES_DEPLOY: &str = r"Examples:
  logos-scaffold deploy
  logos-scaffold deploy hello_world
  logos-scaffold deploy --program-path /path/to/program.bin
  logos-scaffold deploy --json
  logos-scaffold deploy hello_world --json

Machine-readable output:
  Use --json for stable parsing (recommended for automation).

Environment:
  LOGOS_SCAFFOLD_WALLET_PASSWORD   Password sent to the wallet binary on stdin; do not log this value.";

pub(crate) const EXAMPLES_LOCALNET_START: &str = r"Examples:
  logos-scaffold localnet start
  logos-scaffold localnet start --timeout-sec 45";

pub(crate) const EXAMPLES_LOCALNET_STOP: &str = r"Examples:
  logos-scaffold localnet stop";

pub(crate) const EXAMPLES_LOCALNET_STATUS: &str = r"Examples:
  logos-scaffold localnet status
  logos-scaffold localnet status --json";

pub(crate) const EXAMPLES_LOCALNET_LOGS: &str = r"Examples:
  logos-scaffold localnet logs
  logos-scaffold localnet logs --tail 500";

pub(crate) const EXAMPLES_LOCALNET_RESET: &str = r"Examples:
  logos-scaffold localnet reset --dry-run
  logos-scaffold localnet reset --yes
  logos-scaffold localnet reset --reset-wallet --yes
  logos-scaffold localnet reset --reset-wallet --dry-run --verify-timeout-sec 60

Safety:
  Destructive (wipes sequencer DB; --reset-wallet also deletes keypairs irrecoverably).
  --yes is required unless --dry-run is passed.";

pub(crate) const EXAMPLES_WALLET: &str = r"Examples:
  logos-scaffold wallet list
  logos-scaffold wallet topup --dry-run
  logos-scaffold wallet -- account list
  logos-scaffold wallet -- --help

Environment:
  LOGOS_SCAFFOLD_WALLET_PASSWORD   Optional; local dev default if unset.";

pub(crate) const EXAMPLES_WALLET_LIST: &str = r"Examples:
  logos-scaffold wallet list
  logos-scaffold wallet list --long

Passthrough (inner wallet CLI):
  logos-scaffold wallet -- --help
  logos-scaffold wallet -- account list

Environment:
  LOGOS_SCAFFOLD_WALLET_PASSWORD   Optional; local dev default if unset.";

pub(crate) const EXAMPLES_WALLET_TOPUP: &str = r"Examples:
  logos-scaffold wallet topup
  logos-scaffold wallet topup Public/6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV
  logos-scaffold wallet topup --address Public/6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV
  logos-scaffold wallet topup --dry-run

Exit codes:
  0  topup confirmed.
  non-zero  any other outcome, including `submitted but confirmation timed out`
            (status: pending — the topup may still land; retry or check balance).

Environment:
  LOGOS_SCAFFOLD_WALLET_PASSWORD   Optional; local dev default if unset.";

pub(crate) const EXAMPLES_WALLET_DEFAULT_SET: &str = r"Examples:
  logos-scaffold wallet default set Public/6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV
  logos-scaffold wallet default set --address Public/6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV

Environment:
  LOGOS_SCAFFOLD_WALLET_PASSWORD   Optional; local dev default if unset.";

pub(crate) const EXAMPLES_DOCTOR: &str = r"Examples:
  logos-scaffold doctor
  logos-scaffold doctor --json";

pub(crate) const EXAMPLES_REPORT: &str = r"Examples:
  logos-scaffold report
  logos-scaffold report --out /tmp/diag.tar.gz
  logos-scaffold report --tail 1000";

pub(crate) const EXAMPLES_COMPLETIONS: &str = r"Examples:
  logos-scaffold completions bash
  logos-scaffold completions zsh
  lgs completions bash > ~/.local/share/bash-completion/completions/lgs";
