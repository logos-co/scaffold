# logos-scaffold

`logos-scaffold` is a Rust CLI for bootstrapping LEZ (Logos Execution Zone) `program_deployment` projects in standalone mode.

## Documentation

- [FURPS+](FURPS.md) — Functional and non-functional requirements
- [ADR](ADR.md) — Architecture Decision Records

## Using scaffold as a Rust library

Everything the CLI does is also exposed as a typed Rust API under
`logos_scaffold::api`, so tests and dev tooling can drive scaffold-managed
projects (setup, localnet lifecycle, wallet top-ups, deploys, diagnostics)
without shelling out to `lgs` and parsing text:

```rust
use logos_scaffold::api::{LocalnetStartOptions, Project};

let project = Project::open("/path/to/my-app")?;
let node = project.localnet_start(&LocalnetStartOptions::default())?;
println!("sequencer pid={} rpc={}", node.pid(), node.rpc_url());
node.stop()?;
```

See the `api` module rustdoc for the full surface, typed result models, and
categorized errors.

## Platform

The CLI is currently Unix-only.
Localnet and process/port detection rely on Unix tools (lsof, ps, kill).

## Scope

- Single external dependency: [LEZ](https://github.com/logos-blockchain/logos-execution-zone/)
- Standalone sequencer flow only
- No `logos-blockchain` dependency

## Prerequisites

- `git`, `rustc`, `cargo`
- `curl` for fetching the `logos-blockchain-circuits` release on first `setup`
- Unix process helpers: `lsof`, `ps`, `kill`
- Container runtime for guest builds: Docker or Podman
- `logos-blockchain-circuits` release on disk: either set
  `LOGOS_BLOCKCHAIN_CIRCUITS=<path>` or place the release at
  `~/.logos-blockchain-circuits/`. Required by the LEZ standalone build
  chain that `setup` invokes.
- `nix` (with flakes enabled) — only required for `basecamp` subcommands.

## Install

```bash
cargo install --path .
```

This installs two binaries on your PATH: `logos-scaffold` and the shorter
alias `lgs`. They are functionally identical; use either.

### Shell completions

`lgs completions <shell>` prints a completion script to stdout. The script
completes both `lgs` and `logos-scaffold`.

Per-shell install instructions live in the CLI help itself:

```bash
lgs completions bash --help
lgs completions zsh --help
```

## DOGFOODING

Canonical dogfooding scenarios live in [DOGFOODING.md](./DOGFOODING.md).
Keep that runbook updated whenever first-class commands, templates, or supported workflows change.

## CLI

All commands below also work under the `lgs` alias (e.g. `lgs setup`).

```bash
logos-scaffold create <name> [--vendor-deps] [--lez-path PATH]
logos-scaffold new <name> [--vendor-deps] [--lez-path PATH]
logos-scaffold init [--dry-run] [--no-backup]
logos-scaffold setup
logos-scaffold build [project-path]
logos-scaffold deploy [program-name]
logos-scaffold localnet start [--timeout-sec N]
logos-scaffold localnet stop
logos-scaffold localnet status [--json]
logos-scaffold localnet logs [--tail N]
logos-scaffold localnet reset (--yes | --dry-run) [--reset-wallet] [--verify-timeout-sec N]
logos-scaffold test-node pins [--project DIR] [--lez-source URL|DIR] [--lez-ref REF] [--circuits-version VER] [--json]
logos-scaffold test-node prepare [--project DIR] [--cache-root DIR] [--lez-source URL|DIR] [--lez-ref REF] [--circuits-version VER] [--json]
logos-scaffold test-node doctor [--project DIR] [--json]
logos-scaffold test-node start [--project DIR] [--state DIR] [--port N] [--work-dir DIR] [--preserve-work-dir] [--json]
logos-scaffold test-node status --node <id|dir> [--json]
logos-scaffold test-node stop --node <id|dir> [--preserve-work-dir]
logos-scaffold test-node run [--project DIR] [--state DIR] [--serial | --parallel N] -- <command...>
logos-scaffold test-node tx submit --url URL --file PATH [--encoding borsh-base64|borsh] [--json]
logos-scaffold test-node tx wait --url URL --hash HASH [--after-block N] [--timeout-sec N] [--json]
logos-scaffold test-node tx submit-and-wait --url URL --file PATH [--encoding borsh-base64|borsh] [--timeout-sec N] [--json]
logos-scaffold test-node blocks head --url URL [--json]
logos-scaffold test-node blocks range --url URL --from N --to N [--json]
logos-scaffold test-node blocks wait --url URL --after N [--count N] [--timeout-sec N] [--json]
logos-scaffold test-node clock read --url URL [--json]
logos-scaffold test-node clock wait-stable --url URL [--samples N] [--timeout-sec N] [--json]
logos-scaffold build idl [project-path]
logos-scaffold build client [project-path]
logos-scaffold wallet list [--long]
logos-scaffold wallet topup [<address> | --address <address-ref>] [--dry-run]
logos-scaffold wallet default set <address-ref>
logos-scaffold wallet default set --address <address-ref>
logos-scaffold wallet -- <wallet-command...>
logos-scaffold run [--profile NAME] [--reset | --no-reset] [--post-deploy <cmd>...] [--no-post-deploy] [--watch] [--localnet-timeout N]
logos-scaffold spel -- <spel-command...>
logos-scaffold basecamp setup
logos-scaffold basecamp modules [--path PATH]... [--flake REF]... [--show]
logos-scaffold basecamp install [--print-output]
logos-scaffold basecamp launch <profile>
logos-scaffold basecamp develop <module> [--dev-shell ATTR]
logos-scaffold basecamp build-portable
logos-scaffold basecamp doctor [--json]
logos-scaffold doctor [--json]
logos-scaffold report [--out PATH] [--tail N]
logos-scaffold completions <bash|zsh>
logos-scaffold help
```

Each subcommand documents copy-paste examples under `--help`. Global `-q` / `--quiet` (or `LOGOS_SCAFFOLD_QUIET=1`) suppresses echoed external commands.

## Command Semantics

- `create` and `new` are aliases.
- `init` writes `scaffold.toml` (schema v0.2.0) with defaults into the current directory so an existing project can use the scaffold workflow. It creates `.scaffold/{state,logs}` and appends `.scaffold` to `.gitignore`. When `scaffold.toml` already exists at an older schema, `init` migrates it in place via `toml_edit` so comments, key ordering, and unrelated sections survive the rewrite — old `[basecamp].pin` / `.source` / `.lgpm_flake` move to `[repos.basecamp]` / `[repos.lgpm]`; old `[basecamp.modules.*]` move to top-level `[modules.*]`; legacy `url` fields on `[repos.{lez,spel}]` are dropped. Migrations write a `scaffold.toml.bak` next to the original by default (skip with `--no-backup`); preview either form with `--dry-run`. Already-current configs succeed, leave `scaffold.toml` unchanged, and refresh the shipped AI skills. Run `setup` next after a fresh init or migration.
- `setup` syncs LEZ and `spel` to their pinned commits (read from `[repos.lez]` / `[repos.spel]`), builds the standalone `sequencer_service`, `wallet`, and `spel` binaries locally, and seeds a deterministic default wallet from preconfigured public accounts when none is set. All binaries are project-local and are not installed to PATH — use `logos-scaffold wallet ...` / `logos-scaffold spel -- ...` to interact with them. By default `[repos.lez].path` / `[repos.spel].path` are empty in `scaffold.toml`; the on-disk location is resolved at runtime from `<cache_root>/repos/<name>/<pin>`, so the file is portable across machines and CI. `--vendor-deps` projects keep relative `.scaffold/repos/{lez,spel}` literals; an explicit absolute `path` set in `scaffold.toml` is honored as-is.
- `build [project-path]` runs `setup` and then `cargo build --workspace`.
- `deploy [program-name]` deploys one or all guest programs discovered in `methods/guest/src/bin/*.rs` using prebuilt `.bin` artifacts. After each successful submission it prints `program_id: <hex>` (the risc0 image ID, computed locally from the submitted ELF) and includes it in `--program-path … --json` output. Use `--json` for machine-readable output (recommended for automation).
- `build idl [project-path]` regenerates the IDL from the project source using the vendored `spel` binary.
- `build client [project-path]` regenerates client bindings from the current IDL using the vendored `spel` binary.
- `localnet start` waits until localnet is actually ready (`pid alive` + `127.0.0.1:3040` reachable), otherwise fails with diagnostics.
- `localnet status` distinguishes managed process, stale state, and foreign listeners.
- `localnet reset` stops the sequencer, clears sequencer chain state, restarts, and verifies blocks. Destructive: `--yes` is required unless `--dry-run` is passed (`--dry-run` prints the plan without changing anything). `--reset-wallet` also deletes the project wallet home and default-address state (irrecoverable).
- `test-node` manages isolated, short-lived sequencer instances for integration tests — unlike `localnet` (one long-lived developer sequencer per project on a fixed port), each test node gets its own RPC port, config, database, log, and runtime directory under `.scaffold/test-nodes/<id>`. Test-node commands follow the **caller project's pins**: `pins` reports the LEZ source/ref, resolved commit, checkout location and ownership, sequencer binary path, and circuits version/path that test-node commands will use — each value annotated with where it came from (CLI override → `scaffold.toml` → scaffold default). `prepare` resolves those pins, materialises the LEZ checkout and circuits release, and builds the standalone sequencer for them; managed cache checkouts may be cloned/re-synced, while caller-provided checkouts (`[repos.lez].path`, or a local directory passed via `--lez-source`) are only validated — clean worktree at the requested commit, any origin URL form — and never reset or force-checked-out. `doctor` reports pin drift, missing/dirty/mismatched checkouts, missing binaries, missing circuits, and unsupported platforms as separate categorized checks. `start` spawns a node (`--port 0`/default picks a free port; `--state DIR` seeds the database from a caller-provided state directory) and prints connection details — machine-readable with `--json` (`rpc_url`, `pid`, `state_dir`, `config_path`, `log_path`, `genesis_block_id`, `block_height`). `status --node <id>` reports health and the served RPC URL. `stop --node <id>` terminates only that node and removes its runtime state unless `--preserve-work-dir` is passed. `run -- <cmd>` starts a node, waits for health, runs the command with `LGS_TEST_NODE_RPC_URL` / `LGS_TEST_NODE_PORT` / `LGS_TEST_NODE_STATE_DIR` (and friends) exported, forwards the command's exit status, and stops the node; `--serial` caps machine-wide node concurrency at one for low-resource CI, `--parallel N` at N. The same surface is available in Rust via `logos_scaffold::api::testnode::TestNode`. `tx submit` / `tx wait` / `tx submit-and-wait` give test harnesses definitive transaction outcomes: `submit` returns the tx hash or a structured stateless rejection; `submit-and-wait --json` prints exactly one terminal outcome object — `committed` (with the actual sequencer `block_id` and `timestamp`), `rejected` (`phase: stateless|stateful`, with reason or `observed_after_block_id`), `timeout` (`last_observed_block_id`), `transport_error`, or `wire_mismatch` — and exits non-zero for anything but `committed`. Stateful rejection follows an explicit observation rule (a configurable number of new blocks past the submission boundary without inclusion), never a single sleep; transport failures are never reported as business rejections. In Rust: `node.client().submit_and_wait(&TransactionBytes::borsh_base64(..)?, &WaitOptions::default())`. `blocks head|range|wait` expose deterministic block context for replay: per block they report the real sequencer `block_id` and `timestamp`, transaction count, and explicit classification — genesis (the only zero-transaction block; no clock tick to replay), clock-only (empty post-genesis blocks still advance clock state via the mandatory clock transaction), and user transactions — plus per-transaction hashes for public/deployment transactions. `clock read` returns all three `/LEZ/ClockProgramAccount/…` accounts with their decoded `block_id`/`timestamp` data; `clock wait-stable` is a read barrier that requires consecutive identical samples (head + clock state) before returning, so tests comparing local expected state against an always-ticking sequencer get a consistent snapshot or a retryable timeout. In Rust: `client.block_head()`, `client.blocks(BlockRange { from, to })`, `client.wait_blocks(after, count, timeout)`, `client.clock_snapshot(ClockReadMode::Stable { samples, timeout })`.
- `wallet list` shows known wallet accounts (`wallet account list`).
- `wallet topup` checks account state first (`wallet account get --account-id ...`), runs `wallet auth-transfer init --account-id ...` only when the destination is uninitialized, then performs Piñata faucet claim (`wallet pinata claim --to ...`). If address is omitted, scaffold uses project default wallet from `.scaffold/state/wallet.state`.
- `wallet default set` stores a project-scoped default wallet address in `.scaffold/state/wallet.state`.
- `wallet -- ...` forwards raw wallet CLI arguments to the project-local wallet binary while preserving project wallet environment.
- `run` combines build (which chains `setup`), IDL build, localnet start, wallet topup, and deploy into a single command — the inner loop for day-to-day development. It works with no configuration. If a `[run]` section with `post_deploy` is present in `scaffold.toml`, each hook is executed after deploy via `sh -c` (cwd = project root) with `SEQUENCER_URL`, `NSSA_WALLET_HOME_DIR`, `SCAFFOLD_PROJECT_ROOT`, and `SCAFFOLD_IDL_DIR` env vars; when the project has exactly one deployable program, `SCAFFOLD_PROGRAM_ID` and `SCAFFOLD_GUEST_BIN` are also set. If a localnet is already running it is reused; otherwise it is started, and deploy is skipped when the guest binaries + IDL + config and the sequencer instance are unchanged. `--profile NAME` selects a named pipeline from `[run.profiles.<name>]`; `--reset` wipes sequencer state + wallet and re-seeds before the run (`--no-reset` overrides a config-set default); `--post-deploy <cmd>` (repeatable) overrides the configured hooks and `--no-post-deploy` skips them entirely; `--watch` re-runs the pipeline on file changes. `run` covers the deploy loop only — it does not run `wallet -- check-health` or any `basecamp` command.
- `spel -- ...` forwards raw spel CLI arguments to the project-vendored `spel` binary so any spel subcommand (`inspect`, `pda`, `generate-idl`, …) runs against the project's pinned version without a global install.
- `basecamp setup` pins basecamp + `lgpm` (read from `[repos.basecamp]` / `[repos.lgpm]` — both `build = "nix-flake"`), builds both (logged to `.scaffold/logs/<timestamp>-setup-*.log`), and seeds per-profile XDG directories for `alice` and `bob` under `.scaffold/basecamp/profiles/`. Runtime config (`port_base`, `port_stride`) is in `[basecamp]`.
- `basecamp modules` is the sole writer of the captured module set, which lives in top-level `[modules.<name>]` sections (each with `flake` and `role = "project" | "dependency"`). Modules aren't basecamp's property — they're the project's Logos modules, which basecamp happens to be one consumer of. Zero-arg runs auto-discovery: walks project flakes (root `.#lgx` first, else immediate sub-flakes), derives a `module_name` per source (from `metadata.json.name` for local paths; heuristic from the github repo slug for remote refs, with a one-line assumption note you can correct in `scaffold.toml`), then resolves each declared dep name by: (1) already keyed in `[modules]`, (2) basecamp preinstall list, (3) the source's own `flake.lock`, (4) scaffold-default pin. Unresolved deps **fail fast** — no silent skip. `--flake <ref>` / `--path <file>` capture explicit project sources; `--show` prints the current set without mutating. Re-runs are idempotent: existing `[modules]` entries are preserved so hand-edits survive. Project contract: see [docs/basecamp-module-requirements.md](./docs/basecamp-module-requirements.md).
- `basecamp install` is pure replay: builds every captured source (dependencies first, then project modules — fail-fast on a broken companion pin) and installs them into both `alice` and `bob` via `lgpm`. No source-set flags. If the state is empty on first call it transparently invokes `basecamp modules` in auto-discover mode, prints what was captured, and proceeds. Each nix build logs to `.scaffold/logs/<timestamp>-install.log` with a one-line progress status (duration on both success and failure); `--print-output` (or `LOGOS_SCAFFOLD_PRINT_OUTPUT=1`) opts back into streaming nix output directly for CI.
- `basecamp launch <profile>` scrubs the profile's data/cache under `.scaffold/basecamp/profiles/<profile>/`, replays captured modules, assigns per-profile ports, and execs `basecamp` with the profile's XDG environment. Before exec, prints a one-line variant-check summary of installed modules so the freeze-on-first-click case (upstream manifest variant mismatch) is visible. The scrub is scoped to the project's own profiles directory and is the whole point of the command — clean-slate semantics on every launch. Custom launch env is declarative via `scaffold.toml`: `[basecamp.env]` sets plain vars on every profile, `[basecamp.env_append]` `:`-joins path lists (e.g. `QT_PLUGIN_PATH`, `LD_LIBRARY_PATH`) onto the value `lgs` inherited so basecamp's own paths aren't clobbered, and `[basecamp.profiles.<name>.env]` sets per-profile vars that win over the global `[basecamp.env]` (e.g. distinct `LOGOS_STORAGE_API_PORT` for `alice` vs `bob`).
- `basecamp develop <module>` resolves the module's flake from `[modules.<module>]`, strips its `#lgx` output fragment, and execs `nix develop <flake>` from the project root — so an in-shell `lgs` resolves this project via its normal cwd-upward search (the dev shell starts in the project root). It also exports `SCAFFOLD_PROJECT_ROOT` / `LOGOS_PROFILE` as context for scripts in the shell (project discovery itself doesn't read them). `--dev-shell <attr>` selects a non-default dev shell (`nix develop <flake>#<attr>`). An unknown module name fails fast with the captured-module list before any `nix` invocation. This is the verb-set-symmetry wrapper so contributors stop reaching for raw `cd <module> && nix develop`.
- `basecamp build-portable` rebuilds every `role = "project"` entry in `[modules]` with attr-swapped `#lgx-portable` for hand-loading into a basecamp AppImage. Zero-arg: sources come from scaffold.toml (managed via `basecamp modules`). `role = "dependency"` entries are intentionally skipped — the target AppImage provides its own release companion modules via its Package Manager catalog. Output is ordered topologically by `metadata.json` dependencies (leaves first, so basecamp's AppImage can resolve each module's deps before loading it), and symlinked into `.scaffold/basecamp/portable/` as `<NN>-<module_name>.lgx` so the AppImage's "install lgx" file picker has browsable, human-named files in the right order. The directory is wiped and recreated per run.
- `basecamp doctor` emits a basecamp-specific health report: captured modules summary (each entry's flake ref, parsed tag/commit annotation for github refs, and any API headers already installed in alice's profile), manifest variant check per seeded profile (flags modules whose `main` is missing the current-platform `-dev` key — the freeze-on-first-click failure mode), dep-pin drift (captured `role = "dependency"` rev vs. scaffold default), and auto-discovery drift (project sources discoverable today but absent from the captured set). `--json` for machine-readable output.
- `doctor` prints actionable checks and next steps; `--json` is for CI/machine parsing.
- `report` creates a `.tar.gz` diagnostics bundle for GitHub issues using strict allowlist collection with redaction and explicit skip reporting.
- `completions <shell>` prints a shell completion script to stdout. Supported shells: `bash`, `zsh`. The generated script covers both `lgs` and `logos-scaffold`.
- Wallet-facing commands accept `LOGOS_SCAFFOLD_WALLET_PASSWORD` for password override (fallback: local dev default).

## First Success Path

`lgs run` runs setup (via build), build, IDL, localnet, topup, and deploy in one
pipeline. Use it as your daily inner loop:

```bash
lgs new my-app
cd my-app
lgs run
lgs wallet -- check-health   # confirm wallet + localnet after the first pipeline
```

The first `run` seeds the default wallet and starts localnet; later runs reuse a
running localnet and skip deploy when nothing changed.

### Step-by-step (optional)

Use these when debugging a single phase or learning what `run` orchestrates:

```bash
lgs setup
lgs localnet start
lgs build
lgs deploy
lgs wallet topup
```

### Adopt scaffold in an existing project

If you already have a Rust/LEZ project, add scaffold to it without regenerating:

```bash
cd my-existing-project
lgs init
lgs setup
```

`init` only writes `scaffold.toml` and creates `.scaffold/` directories.
It does not touch your `Cargo.toml` or `src/`. Edit `scaffold.toml` if you
need non-default framework settings (e.g. `lez-framework`).

### Migrate an older scaffolded project

If `scaffold.toml` predates a section scaffold now requires (e.g.
`[repos.spel]`), commands fail with an error pointing at `init`. Migrate with:

```bash
cd my-existing-project
lgs init    # appends the missing section to scaffold.toml in place
lgs setup   # picks up the new section
```

Existing fields are preserved verbatim.

### Configuring `lgs run`

`lgs run` works with no configuration (setup, via build, → IDL → localnet → topup → deploy).
To run one or more post-deploy hooks automatically (e.g. submit a transaction
with [spel](https://github.com/logos-co/spel)), add a `[run]` section to
`scaffold.toml`. `post_deploy` is a list of shell commands executed in order;
the run aborts at the first non-zero exit:

```toml
[run]
post_deploy = [
  "lgs spel -- --idl $SCAFFOLD_IDL_DIR/counter.json -p $SCAFFOLD_GUEST_BIN init",
  "lgs spel -- --idl $SCAFFOLD_IDL_DIR/counter.json -p $SCAFFOLD_GUEST_BIN increment --by 5",
]
```

The `lgs spel --` passthrough invokes the project-vendored `spel` binary
so hooks pick up the same pinned version `deploy` used.

A single command may also be written as a plain string for brevity:
`post_deploy = "echo done"`.

Each hook runs via `sh -c` with cwd set to the project root and these
environment variables pre-set:

| Variable | Value |
|---|---|
| `SEQUENCER_URL` | `http://127.0.0.1:<port>` (from `scaffold.toml`) |
| `NSSA_WALLET_HOME_DIR` | Absolute path to project wallet directory |
| `SCAFFOLD_PROJECT_ROOT` | Absolute path to project root |
| `SCAFFOLD_IDL_DIR` | Absolute path to IDL output directory |
| `SCAFFOLD_PROGRAM_ID` | risc0 image ID (hex) of the deployed program. Set only when the project has exactly one deployable program; unset if `spel inspect` cannot extract the ID |
| `SCAFFOLD_GUEST_BIN` | Absolute path to the guest `.bin`. Set only when the project has exactly one deployable program |

`SCAFFOLD_PROGRAM_ID` and `SCAFFOLD_GUEST_BIN` are unset for
multi-program projects so hooks fail loudly rather than silently
picking up the wrong program.

#### One-off override / skip

To run a different hook without editing `scaffold.toml`:

```bash
lgs run --post-deploy "scripts/smoke.sh"
lgs run --post-deploy "step-a" --post-deploy "step-b"   # repeatable
lgs run --no-post-deploy                                 # skip all hooks
```

`--post-deploy` and `--no-post-deploy` conflict with each other and
both override whatever `[run].post_deploy` defines.

#### Watch mode

`lgs run --watch` re-runs the pipeline on each filesystem change (localnet
is reused; reset is skipped on re-runs). Scope what counts as a change with
`[run.watch]` and tune the coalescing window:

```toml
[run.watch]
include = ["programs/**/guest/**", "contracts/**/*.sol"]
exclude = ["**/*.md", "Cargo.lock"]
debounce_ms = 1500
```

A changed path triggers a re-run **iff** it matches at least one `include`
glob (or `include` is unset, meaning "any path") **and** matches zero
`exclude` globs — `exclude` always wins. Globs are project-relative,
gitignore-style: `**` spans path segments, `*`/`?` match within a segment,
and a slash-less pattern (`Cargo.lock`) matches at any depth. `.scaffold`,
`target`, `.git`, and the IDL output dir are always ignored regardless of
these filters. Override the debounce per invocation with
`lgs run --watch --watch-debounce-ms 1500` (CLI wins over
`[run.watch].debounce_ms`, which wins over the 500ms default).

Checkpoint commands:

```bash
logos-scaffold localnet status
logos-scaffold doctor
```

## LEZ Framework

To use the [LEZ Framework](https://github.com/jimmy-claw/lez-framework) for an
ergonomic developer experience similar to Anchor on Solana:

```
logos-scaffold new <name> --template lez-framework
```

See [LEZ Framework Template](./templates/lez-framework/README.md) for details.

## Troubleshooting

- If `localnet start` fails, inspect:

```bash
logos-scaffold localnet logs --tail 200
```

- If status reports `ownership: foreign`, stop external listeners on `127.0.0.1:3040` before starting scaffold localnet.
- If status reports stale state, run:

```bash
logos-scaffold localnet stop
logos-scaffold localnet start
```

- JSON status for tooling:

```bash
logos-scaffold localnet status --json
logos-scaffold doctor --json
logos-scaffold report --tail 500
```

## Example Runs

Run examples directly without passing `.bin` paths:

```bash
cargo run --bin run_hello_world -- <public_account_id>
cargo run --bin run_hello_world_private -- <private_account_id>
cargo run --bin run_hello_world_with_authorization -- <public_account_id>
cargo run --bin run_hello_world_with_move_function -- write-public <public_account_id> <text>
cargo run --bin run_hello_world_through_tail_call -- <public_account_id>
cargo run --bin run_hello_world_through_tail_call_private -- <private_account_id>
cargo run --bin run_hello_world_with_authorization_through_tail_call_with_pda
```

Optional overrides for custom binaries:

```bash
export EXAMPLE_PROGRAMS_BUILD_DIR=$(pwd)/target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release
cargo run --bin run_hello_world -- --program-path "$EXAMPLE_PROGRAMS_BUILD_DIR/hello_world.bin" <public_account_id>
cargo run --bin run_hello_world_through_tail_call_private -- --simple-tail-call-path "$EXAMPLE_PROGRAMS_BUILD_DIR/simple_tail_call.bin" --hello-world-path "$EXAMPLE_PROGRAMS_BUILD_DIR/hello_world.bin" <private_account_id>
```
