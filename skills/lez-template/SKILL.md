---
name: lez-template
description: Use when working inside a project scaffolded with the bare LEZ template (`lgs new` default — raw Rust + risc0 guest programs, no framework macros). Identify by scaffold.toml without `framework = "lez-framework"`, presence of `methods/guest/src/bin/*.rs`, and absence of `idl/` + `crates/lez-client-gen/`.
---

# Bare LEZ Template (`lgs new` Default)

This skill activates when the agent is working *inside* a project scaffolded with `lgs new <name>` (or `lgs new <name> --template default`). It is the bare LEZ standalone template: a raw Rust workspace plus a risc0 guest crate, no macros. For driving the `lgs` CLI itself, use the `lgs-cli` skill.

## When to Use

Identify a default-template project by **all** of:

- `scaffold.toml` exists at the project root and **does not** contain `framework = "lez-framework"`.
- `methods/guest/src/bin/*.rs` exists — one file per risc0 guest program.
- The project does **not** have `crates/lez-client-gen/` or `idl/` directories (those are lez-framework-specific).
- `src/lib.rs` defines a `runner_support` module with `parse_account_id` / `load_program` helpers (see `templates/default/src/lib.rs`).

If the project has `crates/lez-client-gen/` and `idl/`, switch to `lez-framework-template` instead.

## What This Template Produces

File-tree highlights from `templates/default/` (paths relative to the project root):

```
<project>/
├── Cargo.toml                 # workspace; usually excludes methods/ from members
├── rust-toolchain.toml
├── .env.local                 # local env defaults
├── scaffold.toml              # written by `lgs new` / `lgs init`
├── .scaffold/                 # state, logs, wallet/, repos/, reports/
│   └── commands.md            # canned reference (verbatim below)
├── src/
│   ├── lib.rs                 # `runner_support` helpers
│   └── bin/                   # example runners (host-side clients)
│       ├── run_hello_world.rs
│       ├── run_hello_world_private.rs
│       ├── run_hello_world_with_authorization.rs
│       ├── run_hello_world_with_move_function.rs
│       ├── run_hello_world_through_tail_call.rs
│       ├── run_hello_world_through_tail_call_private.rs
│       └── run_hello_world_with_authorization_through_tail_call_with_pda.rs
└── methods/                   # risc0 guest crate (excluded from main workspace)
    └── guest/
        └── src/bin/<program>.rs
```

The parent `Cargo.toml` typically excludes `methods/` from workspace members so the guest crate can build with its own toolchain. `lgs build` knows about this and auto-compiles `methods/Cargo.toml` when present.

## Risc0 Guest Discovery Convention

`lgs deploy` (and the deploy auto-discovery code path) walks `methods/guest/src/bin/*.rs` to enumerate programs. The basename of each `.rs` file is the program name. For each, the corresponding compiled binary lives at:

```
target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release/<program>.bin
```

The `EXAMPLE_PROGRAMS_BUILD_DIR` env var conventionally captures this absolute path so example runners can pass `--program-path "$EXAMPLE_PROGRAMS_BUILD_DIR/<program>.bin"` for non-default builds.

**One file = one program.** Adding a new program is `methods/guest/src/bin/<name>.rs` + a host-side runner in `src/bin/run_<name>.rs`.

## Build Pipeline

```bash
lgs build              # runs `setup` first, then cargo build --workspace
                       # auto-compiles methods/Cargo.toml if present
lgs build my-project   # explicit project path
```

Key behavior (FURPS Functionality #5): `build` auto-compiles `methods/Cargo.toml` even when the parent workspace excludes the guest crate. No manual `cargo build --manifest-path methods/Cargo.toml` needed.

Success criteria: `methods/target/.../release/<program>.bin` artefacts exist for every guest in `methods/guest/src/bin/`.

## Deploy Pipeline

```bash
lgs deploy                              # auto-discover all guest programs
lgs deploy hello_world                  # deploy a single program by name
lgs deploy --program-path "<path>"      # explicit path; works without methods/
lgs deploy --program-path "<path>" --json
```

`lgs deploy` prints `program_id: <hex>` (the risc0 image ID, computed locally from the submitted ELF) on every successful submission.

JSON output shapes (FURPS Functionality #9):

- `--program-path … --json` — bare object: `{"status":"submitted","program":...,"tx"?:...,"program_id"?:...}`. Absent values are omitted, not `null`.
- Auto-discovery `--json` — silently accepted but ignored. (Use `lgs deploy <name> --program-path` if you need JSON for a single program.)

Failure modes:

- Unknown program name → lists discovered programs.
- Missing binary → points back at `lgs build`.
- Localnet unreachable → sequencer-unavailable hint, not a vague wallet error.

## Example Runners

The template ships seven host-side runners under `src/bin/`. Each uses `runner_support::parse_account_id` and `runner_support::load_program` from `src/lib.rs`. They expect `NSSA_WALLET_HOME_DIR` to be set when invoked directly via `cargo run` (scaffold `wallet --` passthrough sets it automatically; direct `cargo run` does not).

```bash
export NSSA_WALLET_HOME_DIR="$(pwd)/.scaffold/wallet"

cargo run --bin run_hello_world -- <public_account_id>
cargo run --bin run_hello_world_private -- <private_account_id>
lgs wallet -- account sync-private    # after private-account writes

cargo run --bin run_hello_world_with_authorization -- <public_account_id>

cargo run --bin run_hello_world_with_move_function -- write-public  <public_account_id> "<text>"
cargo run --bin run_hello_world_with_move_function -- write-private <private_account_id> "<text>"
cargo run --bin run_hello_world_with_move_function -- move-data-public-to-private <public> <private>

cargo run --bin run_hello_world_through_tail_call -- <public_account_id>
cargo run --bin run_hello_world_through_tail_call_private -- <private_account_id>

cargo run --bin run_hello_world_with_authorization_through_tail_call_with_pda
```

Each runner prints, on success:

```
submitted transaction: status=<...> tx_hash=<...>
verification hint: lgs wallet -- account get --account-id <id>
```

Optional path overrides for custom builds:

```bash
export EXAMPLE_PROGRAMS_BUILD_DIR="$(pwd)/target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release"

cargo run --bin run_hello_world -- \
  --program-path "$EXAMPLE_PROGRAMS_BUILD_DIR/hello_world.bin" \
  <public_account_id>

cargo run --bin run_hello_world_through_tail_call_private -- \
  --simple-tail-call-path "$EXAMPLE_PROGRAMS_BUILD_DIR/simple_tail_call.bin" \
  --hello-world-path     "$EXAMPLE_PROGRAMS_BUILD_DIR/hello_world.bin" \
  <private_account_id>
```

## Iteration Loop

1. Edit guest at `methods/guest/src/bin/<program>.rs`.
2. `lgs build` (auto-compiles guest crate).
3. `lgs deploy [program]` to push the new ELF; capture the printed `program_id`.
4. Edit (or add) the host runner at `src/bin/run_<program>.rs`.
5. `cargo run --bin run_<program> -- <args>` to invoke against the running localnet.
6. `lgs wallet -- account get --account-id <id>` to verify state mutation.

If you hit the wallet `from_env()` panic on direct `cargo run`, you forgot `export NSSA_WALLET_HOME_DIR="$(pwd)/.scaffold/wallet"`.

## Account Creation

```bash
lgs wallet -- account new public      # → "Public/<base58>"
lgs wallet -- account new private     # → "Private/<base58>"
lgs wallet -- account list
lgs wallet -- account get --account-id <id>
lgs wallet -- account sync-private    # after private-account writes
```

Runners take the **base58 portion** of the account ID as the positional arg, not the `Public/` or `Private/` prefix. `runner_support::parse_account_id` strips the prefix automatically, so passing the full `Public/<base58>` form also works.

## `.scaffold/commands.md` Quick Reference

Verbatim from `templates/default/.scaffold/commands.md` (shipped into every default-template project):

```markdown
# Command References

- standalone sequencer: `RUST_LOG=info target/release/sequencer_service sequencer/service/configs/debug/sequencer_config.json`
- lez standalone docs: `https://github.com/logos-blockchain/logos-execution-zone/tree/main?tab=readme-ov-file#standalone-mode`
- wallet commands: `logos-scaffold wallet -- <args>`
- localnet json status: `logos-scaffold localnet status --json`
- doctor json status: `logos-scaffold doctor --json`
- diagnostics bundle for issue reports: `logos-scaffold report --tail 500`
```

## When to Switch Templates

If you find yourself writing repetitive instruction-dispatch + account-derivation boilerplate by hand, consider the LEZ Framework template (`lgs new <name> --template lez-framework`), which adds Anchor-style `#[lez_program]` / `#[instruction]` / `#[account(…)]` macros and auto-generates IDL JSON. See the `lez-framework-template` skill.

Drop down to this `default` template when you need primitives the framework hasn't surfaced or when guest-binary size is critical.

## Key Rules

- **One guest program per `methods/guest/src/bin/<name>.rs`.** The basename is the program name `lgs deploy` recognises.
- **One host runner per program, named `src/bin/run_<name>.rs`.** Reuse `runner_support::parse_account_id` and `runner_support::load_program`.
- **`NSSA_WALLET_HOME_DIR` must be set for direct `cargo run`.** Use `export NSSA_WALLET_HOME_DIR="$(pwd)/.scaffold/wallet"` once per shell.
- **Account IDs are passed as CLI args**, never hardcoded. Use `lgs wallet -- account new {public,private}` to create fresh ones.
- **Don't add Qt / UI / QML deps.** This template is for zk programs; UI work belongs in a separate Logos module project (different repo, different toolchain).
- **Parent `Cargo.toml` should keep `methods/` excluded** from workspace members; `lgs build` handles its compilation separately.
- **Don't hand-edit `target/` or `.scaffold/`**. Treat them as generated output.
- **JSON deploys require `--program-path`.** Discovery-path `--json` is silently accepted; if you need structured output, deploy one program at a time.