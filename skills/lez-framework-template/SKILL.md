---
name: lez-framework-template
description: Use when working inside a project scaffolded with scaffold's lez-framework template — Anchor-on-Solana parallel for LEZ with #[lez_program] / #[instruction] / #[account(...)] macros and auto-generated IDL. Identify by scaffold.toml framework = "lez-framework", crates/lez-client-gen/, idl/, and #[lez_program] in the source.
---

# LEZ-Framework Template Development

This skill activates when the agent is working *inside* a project scaffolded with `lgs new <name> --template lez-framework`. The template uses [LEZ Framework](https://github.com/jimmy-claw/lez-framework) for an ergonomic developer experience similar to Anchor on Solana. For driving the `lgs` CLI itself, use the `lgs-cli` skill.

## When to Use

Identify a lez-framework project by **all** of:

- `scaffold.toml` contains `framework = "lez-framework"`.
- `crates/lez-client-gen/` exists (host-side client generator crate).
- `idl/` directory exists with one `<program>.json` per program (e.g. `idl/lez_counter.json`).
- `methods/guest/src/bin/<program>.rs` and `src/bin/run_<program>.rs` exist (same convention as the default template).
- `src/lib.rs` declares `#[lez_program] mod <program> { ... }` with `#[instruction]` handlers.

If any of these are missing (especially `framework = "lez-framework"` in `scaffold.toml`), switch to `lez-template` instead.

## Why This Template Exists

The `default` template is the bare LEZ standalone surface: you write everything (instruction dispatch, account derivation, IDL by hand, client stubs by hand). The `lez-framework` template is a declarative wrapper that:

- Eliminates instruction-dispatch boilerplate (`#[lez_program]` / `#[instruction]`).
- Annotates account constraints and PDA derivation declaratively (`#[account(…)]`).
- Generates IDL JSON at compile time (exposed as `PROGRAM_IDL_JSON` and persisted under `idl/`).
- Generates host-side client bindings under `src/generated/` from the IDL.

It's the right choice when you'd otherwise be writing repetitive boilerplate. Drop down to `default` only for primitives the framework hasn't surfaced or for extreme guest-binary size constraints.

## What This Template Produces

File-tree highlights from `templates/lez-framework/`:

```
<project>/
├── Cargo.toml                          # main workspace; includes crates/
├── rust-toolchain.toml
├── scaffold.toml                       # framework = "lez-framework"
├── .scaffold/
├── src/
│   ├── lib.rs                          # #[lez_program] mod ... + runner_support
│   ├── generated/                      # client bindings (output of `lgs build client`)
│   └── bin/
│       └── run_lez_counter.rs          # host-side runner with init/increment subcommands
├── crates/
│   └── lez-client-gen/                 # host crate that turns idl/*.json → src/generated/
├── idl/
│   └── lez_counter.json                # auto-generated IDL (don't hand-edit)
└── methods/
    └── guest/src/bin/lez_counter.rs    # risc0 guest binary
```

## Macro Vocabulary

From `templates/lez-framework/src/lib.rs` — the `lez_counter` reference example:

```rust
use lez_framework::prelude::*;
use lez_framework::error::{LezError, LezResult};
use lez_framework_core::types::LezOutput;
use nssa_core::program::AccountPostState;
use nssa_core::account::AccountWithMetadata;

#[lez_program]
mod lez_counter {
    #[allow(unused_imports)]
    use super::*;

    #[instruction]
    pub fn initialize(
        #[account(init, pda = literal("counter"))]
        counter: AccountWithMetadata,
        #[account(signer)]
        authority: AccountWithMetadata,
    ) -> LezResult {
        Ok(LezOutput::states_only(vec![
            AccountPostState::new_claimed(counter.account.clone()),
            AccountPostState::new(authority.account.clone()),
        ]))
    }

    #[instruction]
    pub fn increment(
        #[account(mut, pda = literal("counter"))]
        counter: AccountWithMetadata,
        #[account(signer)]
        authority: AccountWithMetadata,
        amount: u64,
    ) -> LezResult {
        let mut counter_post = counter.account.clone();
        counter_post.balance += amount as u128;

        Ok(LezOutput::states_only(vec![
            AccountPostState::new(counter_post),
            AccountPostState::new(authority.account.clone()),
        ]))
    }
}
```

| Annotation | Generates |
|---|---|
| `#[lez_program] mod <name> { … }` | Program-level scaffolding: instruction enum, dispatch, IDL constant `PROGRAM_IDL_JSON`. The mod name **is** the program name (must match `methods/guest/src/bin/<name>.rs`). |
| `#[instruction] pub fn <handler>(…)` | Instruction enum variant with PascalCase name (`initialize` → `Initialize`); discriminator computed from the name; argument schema (non-account params) is the variant payload. |
| `#[account(init, pda = literal("<seed>"))]` | Account claimed (zero-balance, default visibility) at the literal PDA; `claim_if_default = true`. |
| `#[account(mut, pda = literal("<seed>"))]` | Existing PDA account; mutable; `claim_if_default = false`. |
| `#[account(signer)]` | Authority account; `auth = true`. |
| `LezResult` / `Ok(LezOutput::states_only(vec![…]))` | Return type for instruction handlers. Use `AccountPostState::new(…)` for unchanged authorities and `AccountPostState::new_claimed(…)` when claiming a freshly-derived account. |

Non-account function parameters (e.g. `amount: u64` on `increment`) become args in the IDL.

## IDL Pipeline

The IDL is the contract between the program and any client. It's written to `idl/<program>.json` at compile time (visible in tests via `PROGRAM_IDL_JSON`). Schema (from `idl/lez_counter.json`):

```json
{
  "spec": "lssa-idl/0.1.0",
  "metadata": { "name": "lez_counter", "version": "0.1.0" },
  "program":  { "name": "lez_counter" },
  "instructions": [
    {
      "name": "initialize",
      "variant": "Initialize",
      "discriminator": [220, 59, 207, 236, 108, 250, 47, 100],
      "accounts": [
        { "name": "counter",   "ty": "AccountWithMetadata",
          "auth": false, "claim_if_default": true,  "mutable": false, "visibility": ["public"] },
        { "name": "authority", "ty": "AccountWithMetadata",
          "auth": true,  "claim_if_default": false, "mutable": false, "visibility": ["public"] }
      ],
      "args": [],
      "execution": { "private_owned": false, "public": true }
    }
  ],
  "errors": [],
  "types": []
}
```

Regenerate the IDL whenever the program surface changes:

```bash
lgs build idl       # writes idl/<program>.json from the macro-extracted PROGRAM_IDL_JSON
```

Look for `Wrote IDL ...` lines in the command output to confirm regeneration. Missing markers or empty IDL is a regression.

## Client Generation

```bash
lgs build client    # regenerates IDL first, then runs crates/lez-client-gen
                    # to produce host-side bindings under src/generated/
```

Client artefacts under `src/generated/` reflect the current contents of `idl/`. Don't hand-edit them — treat the macro layer as the source of truth and regenerate.

## Build / Deploy

For the LEZ template, `lgs build` automatically runs IDL regeneration and client generation as part of the pipeline (per DOGFOODING scenario L1). You usually don't need to call `build idl` / `build client` manually except when iterating on IDL alone.

```bash
lgs setup
lgs localnet start
lgs build           # cargo build + IDL regen + client gen
lgs deploy          # auto-discovers methods/guest/src/bin/lez_counter.rs
```

## Reference Example: Running `lez_counter`

The runner at `src/bin/run_lez_counter.rs` exposes `init` and `increment` subcommands:

```bash
export NSSA_WALLET_HOME_DIR="$(pwd)/.scaffold/wallet"
lgs wallet -- account new public            # capture the base58 account id

cargo run --bin run_lez_counter -- init      --to <account-id>
cargo run --bin run_lez_counter -- increment --counter <account-id> --authority <account-id> --amount 5
```

> **Caveat (per DOGFOODING scenario L4):** as of writing, the `run_lez_counter` runner contains `TODO` placeholders for actual transaction submission. Don't be surprised if subcommands accept input but only print diagnostic messages without submitting. When transaction submission is implemented, follow the same `verification hint:` pattern as default-template runners (`lgs wallet -- account get --account-id <id>`).

## Differences vs. `default` Template

| Concern | `default` | `lez-framework` |
|---|---|---|
| Instruction dispatch | hand-written | generated by `#[lez_program]` |
| Account derivation | hand-written | declarative `#[account(…)]` |
| IDL | none | auto-generated under `idl/` |
| Client bindings | hand-written runners | generated under `src/generated/` from IDL |
| Workspace | excludes `methods/` | includes `crates/`; `methods/` still its own crate |
| `lgs build` | cargo build + auto-build `methods/` | adds IDL regen + client gen |
| Recommended for | low-level / size-critical zk programs | most projects (less boilerplate, type-safe clients) |

## Common Gotchas

- **Mod name must match the guest binary.** `#[lez_program] mod lez_counter` requires `methods/guest/src/bin/lez_counter.rs`. Renaming one without the other breaks `lgs deploy` discovery.
- **Don't hand-edit `idl/*.json`** — the next `lgs build` (or `lgs build idl`) overwrites it from the macro-derived `PROGRAM_IDL_JSON`. Edit the source instead and regenerate.
- **Don't hand-edit `src/generated/`** — regenerated by `lgs build client`.
- **Account derivation order matters.** `#[account(init, pda = …)]` accounts that depend on others should appear after their inputs in the handler signature.
- **Visibility defaults to `public`.** If you need private execution, configure `execution.private_owned = true` (currently driven by macro inputs not shown in the counter example).
- **`crates/` workspace members.** If `Cargo.toml` doesn't include `crates/lez-client-gen`, `lgs build client` will not find the generator. Check the workspace `members` array.

## Adding a New Instruction

1. Add a `#[instruction] pub fn <name>(…) -> LezResult { … }` inside the `#[lez_program]` mod in `src/lib.rs`. Mirror the account-annotation patterns from `initialize` / `increment`.
2. `lgs build idl` to regenerate `idl/<program>.json`. Verify the new instruction appears with the expected `accounts` / `args` / `discriminator`.
3. `lgs build client` to regenerate host-side bindings.
4. Extend `src/bin/run_<program>.rs` with a new subcommand that uses the regenerated client.
5. Test: `lgs build && lgs deploy && cargo run --bin run_<program> -- <new-subcommand> …`.

## Adding a New Program

A project can host multiple `#[lez_program]` modules, each one its own program with its own guest binary:

1. New `methods/guest/src/bin/<new_program>.rs`.
2. New `#[lez_program] mod <new_program> { … }` in `src/lib.rs` (or a sibling `.rs` file pulled in via `mod`).
3. New `src/bin/run_<new_program>.rs`.
4. `lgs build` regenerates IDL + clients across all programs; `lgs deploy` discovers the new one automatically.

## Key Rules

- **The `#[lez_program] mod` name is the program name.** Keep it in lockstep with `methods/guest/src/bin/<name>.rs`.
- **`idl/` and `src/generated/` are derived.** Never hand-edit them; regenerate via `lgs build idl` / `lgs build client`.
- **Account annotations are the contract.** `pda = literal(…)`, `init`, `mut`, `signer` flags drive both the IDL and runtime behavior.
- **Use `LezOutput::states_only(…)`** as the default success return; reach for richer variants only when needed.
- **Run `cargo test` to dump the IDL** — the test `__lssa_idl_print` prints `PROGRAM_IDL_JSON` between `--- LSSA IDL BEGIN/END ---` markers, useful for debugging IDL regressions.
- **Don't bypass the framework for ad-hoc dispatch.** If you find yourself writing manual instruction matching, drop down to the `default` template instead.
- **`NSSA_WALLET_HOME_DIR` is required for direct `cargo run`.** Same as the default template.