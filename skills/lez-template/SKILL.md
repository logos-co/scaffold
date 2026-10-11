---
name: lez-template
description: Use when working inside a project scaffolded with `lgs new` (the default LEZ 0.3 template — LEZ's examples/program_deployment: raw Rust risc0 guest programs plus host runners). Identify by scaffold.toml with `[framework] kind = "default"`, `methods/guest/src/bin/*.rs`, and runners under `src/bin/run_*.rs` that call `program_deployment::deploy_program`.
---

# Default LEZ 0.3 Template (`lgs new`)

This skill activates when the agent is working *inside* a project scaffolded with `lgs new <name>`. The project is LEZ v0.3's own `examples/program_deployment`, copied from the pinned release, with a scaffold workspace `Cargo.toml` on top. For driving the `lgs` CLI itself, use the `lgs-cli` skill.

Scaffold supports the **LEZ 0.3** release line only. Anything you remember about LEZ 0.1/0.2 (`nssa`/`nssa_core` crates, `wallet deploy-program`, the Piñata faucet, `program_owner` accounts, image-ID program ids) is wrong here.

## When to Use

Identify a default-template project by **all** of:

- `scaffold.toml` at the project root with `[scaffold] version = "0.3.0"` and `[framework] kind = "default"`.
- `methods/guest/src/bin/*.rs` — one file per risc0 guest program.
- `src/lib.rs` exporting `deploy_program` (crate name `program_deployment`), used by every `src/bin/run_*.rs`.

## What This Template Produces

```
<project>/
├── Cargo.toml          # workspace (., methods, methods/guest); lee/lee_core/wallet/common/
│                       # sequencer_service_rpc/program_loader_core git deps at the LEZ pin;
│                       # [lib] name = "program_deployment"; keccak [patch] copied from LEZ
├── Cargo.lock          # seeded from the pinned LEZ release's lockfile — commit it
├── rust-toolchain.toml # 1.98.1 (LEZ 0.3's toolchain)
├── .env.local          # RUST_LOG=info, RISC0_DEV_MODE=1
├── scaffold.toml
├── .scaffold/          # state/, logs/, wallet/, localnet/ (chain), commands.md
├── src/
│   ├── lib.rs          # deploy_program(): program-loader deployment helper
│   └── bin/run_*.rs    # LEZ's example runners (deploy + call)
└── methods/
    ├── build.rs        # risc0_build::embed_methods()
    └── guest/src/bin/  # hello_world, hello_world_with_authorization,
                        # hello_world_with_move_function, simple_tail_call, tail_call_with_pda
```

## The LEZ 0.3 Program Model

Guest programs use `lee_core::program`:

```rust
use lee_core::{account::ShardData, program::{Plan, PlanInput, run_program}};

fn main() { run_program(plan, apply) }

fn plan(input: &PlanInput, instruction: Vec<u8>) -> Plan {
    let mut plan = Plan::new(input);
    let [account] = input.accounts.as_slice() else { panic!("one account") };
    plan.effect(account, &instruction);   // propose an effect on this program's shard
    plan
}

fn apply(greeting: Vec<u8>, pre_data: &ShardData) -> Option<Vec<u8>> {
    let mut bytes = pre_data.clone().into_inner();
    bytes.extend(greeting);
    Some(bytes)                            // None keeps the shard unchanged
}
```

- Accounts have a nonce and **shards**: one byte blob per program, keyed by the program's account id. A program writes only its own shard; the native balance is the shard of the native token program (`111…1`).
- Programs are addressed by **program account id** (the `program_loader` header account), not by risc0 image id. Tail calls name the callee's account id at run time (`ChainedCall::new(callee_account_id, selectors, &data)`), so `simple_tail_call` takes its callee as instruction data.
- Public transactions pay **fees**: the payer must hold the fee reserve (about 16,003,000 at the default gas limit) plus anything it sends. `lgs wallet topup` funds the default account with 1,000,000,000.

## Build

```bash
lgs build                  # runs setup, cargo build --workspace, then the guest build
lgs build --guest local    # host risc0 toolchain (rzup install rust); no Docker
```

Guest binaries:

- default (docker): `target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker/<program>.bin`
- `--guest local`: `target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release/<program>.bin`

Each `lgs build` clears the other mode's tree, so the last build is what `lgs deploy` ships. The docker build is reproducible (same bytes, same image id on every machine); `deploy --json` reports `elf_sha256`.

## Deploy

```bash
lgs wallet topup                         # the default account pays deployment fees
lgs deploy                               # every program in methods/guest/src/bin
lgs deploy hello_world --json
lgs deploy --program-path <path.bin> --json
```

For each program scaffold creates a fresh header account plus one segment account per 96 KiB of the user ELF and runs `wallet program-loader deploy --elf … --header … --segments … --payer <default>`. The wallet waits for every transaction, so `"status": "deployed"` is confirmed. The header id is the `program_id`; it is recorded in `.scaffold/state/deployments.json`. Every deploy creates a new header — re-deploying changes the id.

JSON: `--program-path … --json` prints one object (`status`, `program`, `program_id`, `tx`, `elf_sha256`); discovery prints `{"deploys":[…]}` with `error` instead of `program_id` for failures. Absent values are omitted. `Incorrect fee` means the payer is unfunded — run `lgs wallet topup`.

## Example Runners

The runners are LEZ's own and **deploy their own copy** of the program before calling it. Arguments are the guest `.bin` path(s), plain base58 account ids (no `Public/`), and a funded payer:

```bash
export LEE_WALLET_HOME_DIR="$(pwd)/.scaffold/wallet"     # the only variable the 0.3 wallet reads
export BIN_DIR="$(pwd)/target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker"
export PAYER=$(sed -n 's/^default_address=Public\///p' .scaffold/state/wallet.state)
lgs wallet -- account new public                         # → Public/<id>; use <id>

cargo run --bin run_hello_world -- "$BIN_DIR/hello_world.bin" <id> "$PAYER"
cargo run --bin run_hello_world_with_authorization -- "$BIN_DIR/hello_world_with_authorization.bin" <id> "$PAYER"
cargo run --bin run_hello_world_through_tail_call -- "$BIN_DIR/simple_tail_call.bin" "$BIN_DIR/hello_world.bin" <id> "$PAYER"
cargo run --bin run_hello_world_with_move_function -- "$BIN_DIR/hello_world_with_move_function.bin" "$PAYER" write-public <id> Hola
cargo run --bin run_hello_world_with_authorization_through_tail_call_with_pda -- "$BIN_DIR/tail_call_with_pda.bin" "$BIN_DIR/hello_world_with_authorization.bin" "$PAYER"
lgs wallet -- account get --account-id Public/<id>      # shows the program's shard
```

The `_private` runners prove locally; keep `RISC0_DEV_MODE=1` unless you have a machine that can run the real prover.

## Iteration Loop

1. Edit `methods/guest/src/bin/<program>.rs`.
2. `lgs build`.
3. `lgs deploy <program>` and take `program_id` from the output or `deployments.json` — or let a runner deploy its own copy.
4. Send transactions (runner, or your own client using `lee::public_transaction::Message::try_new(program_account_id, selectors, nonces, instruction)` with a fee declaration via the wallet).
5. `lgs wallet -- account get --account-id Public/<id>` to verify.

## Key Rules

- **One guest program per `methods/guest/src/bin/<name>.rs`.** The basename is the program name `lgs deploy` recognises.
- **Export `LEE_WALLET_HOME_DIR`** before `cargo run` of a runner; without it the wallet silently uses `~/.lee/wallet`.
- **Fund before you deploy or call.** Fees are mandatory for public transactions.
- **Program ids are header accounts.** Never derive one from the ELF; read it from deploy output or `.scaffold/state/deployments.json`.
- **Keep `max_block_size` at or below 1,835,008 bytes** if you edit sequencer configs; LEZ 0.3 refuses to start above Bedrock's inscription limit.
- **Don't hand-edit `target/` or `.scaffold/`.** `lgs localnet reset --yes` wipes the chain (`.scaffold/localnet`) and the recorded program ids.
