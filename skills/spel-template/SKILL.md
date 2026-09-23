---
name: spel-template
description: Use when working inside a project scaffolded with `lgs new --template spel` — SPEL framework project with #[lez_program] / #[instruction] / #[account(...)] macros, auto-generated IDL, and spel CLI tooling. Identify by scaffold.toml framework.kind = "spel", presence of spel.toml, and #[lez_program] in the source.
---

# SPEL Template Development

This skill activates when the agent is working *inside* a project scaffolded with `lgs new <name> --template spel`. The template uses the [SPEL framework](https://github.com/logos-co/spel) for an ergonomic developer experience on LEZ — similar to Anchor on Solana. For driving the `lgs` CLI itself, use the `lgs-cli` skill.

## When to Use

Identify a spel project by **all** of:

- `scaffold.toml` contains `framework.kind = "spel"`.
- `spel.toml` exists at the project root (spel CLI config).
- `methods/guest/src/bin/<program>.rs` exists (risc0 guest binary).
- The source contains `#[lez_program] mod <name> { … }` with `#[instruction]` handlers.

If `scaffold.toml` is absent or contains `framework.kind = "default"`, switch to `lez-template` instead.

## Why This Template Exists

The `default` template is the bare LEZ surface: you write everything by hand. The `spel` template is a declarative wrapper that:

- Eliminates instruction-dispatch boilerplate (`#[lez_program]` / `#[instruction]`).
- Annotates account constraints and PDA derivation declaratively (`#[account(…)]`).
- Generates IDL JSON automatically (via `spel generate-idl` / `make idl`).
- Generates FFI bindings and a typed CLI client from the IDL (via `make ffi`, which runs the `spel-client-gen` binary).
- Provides a complete Makefile for the full build → deploy → interact workflow.

## Project Structure

```
<project>/
├── Cargo.toml                      # workspace root
├── spel.toml                       # spel CLI config (IDL path, binary path, program address)
├── scaffold.toml                   # scaffold config (LEZ pin, spel pin, framework.kind = "spel")
├── Makefile                        # build / idl / ffi / deploy / setup / run targets
├── <name>_core/                    # SPEL program crate (#[lez_program] macros)
│   └── src/lib.rs
├── <name>_ffi/                     # Generated FFI bindings (do not hand-edit)
│   └── src/
├── methods/
│   └── guest/src/bin/<name>.rs     # risc0 guest binary
├── examples/                       # runnable examples against the program
├── scripts/                        # helper scripts used by the Makefile
└── <name>-idl.json                 # generated IDL, project root (do not hand-edit)
```

## Macro Vocabulary

```rust
use spel_framework::prelude::*;

#[lez_program]
mod my_program {
    #[instruction]
    pub fn initialize(
        #[account(init, pda = literal("counter"))]
        counter: AccountWithMetadata,
        #[account(signer)]
        owner: AccountWithMetadata,
    ) -> SpelResult {
        Ok(SpelOutput::states_only(vec![
            AccountPostState::new_claimed(counter.account.clone()),
            AccountPostState::new(owner.account.clone()),
        ]))
    }

    #[instruction]
    pub fn do_something(
        #[account(mut, pda = literal("counter"))]
        counter: AccountWithMetadata,
        #[account(signer)]
        owner: AccountWithMetadata,
        amount: u64,
    ) -> SpelResult {
        // ...
    }
}
```

| Annotation | Effect |
|---|---|
| `#[lez_program] mod <name>` | Program-level scaffolding: instruction enum, dispatch, IDL constant. Mod name must match `methods/guest/src/bin/<name>.rs`. |
| `#[instruction] pub fn <handler>(…)` | Instruction variant; discriminator auto-computed. |
| `#[account(init, pda = literal("<seed>"))]` | Account claimed at literal PDA. |
| `#[account(mut, pda = literal("<seed>"))]` | Mutable existing PDA account. |
| `#[account(signer)]` | Authority account. |

Non-account parameters become args in the IDL.

## Build Pipeline

```bash
make build          # cargo build + risc0 guest
# or: lgs build

make idl            # generate IDL → <name>-idl.json (project root)
# or: lgs build idl   (delegates to `spel generate-idl`)

make ffi            # generate FFI bindings → <name>_ffi/
# or: lgs build client  (runs `make ffi-gen` with the vendored spel-client-gen)
```

`lgs build` runs `make build` (the Docker guest build) and then IDL generation. FFI/client gen is a separate step.

## Deploy & Interact

```bash
lgs setup           # clone LEZ, build sequencer + wallet + spel CLI
lgs localnet start  # start the sequencer
lgs deploy          # deploy the program
lgs run             # start localnet, deploy, enter REPL

# Interact using the spel CLI (reads spel.toml automatically):
lgs spel -- --help
# Instruction names are kebab-case on the CLI, and account ids carry their
# privacy prefix — pass `Public/<id>`, not the bare id.
lgs spel -- initialize --owner Public/<SIGNER_ID>
lgs spel -- do-something --owner Public/<SIGNER_ID> --amount 42
```

## IDL

The IDL at `<name>-idl.json` (project root, named by `spel.toml`) is the contract between the program and any client. Regenerate whenever the program surface changes:

```bash
make idl            # or: lgs build idl
```

Do **not** hand-edit `<name>-idl.json` or `<name>_ffi/generated/` — both are derived artifacts.

## Differences vs. `default` Template

| Concern | `default` | `spel` |
|---|---|---|
| Instruction dispatch | hand-written | generated by `#[lez_program]` |
| Account derivation | hand-written | declarative `#[account(…)]` |
| IDL | none | auto-generated as `<name>-idl.json` |
| FFI / CLI client | hand-written | generated by `spel-client-gen` (`make ffi`) |
| Workflow | `lgs` commands | `make` targets + `spel` CLI + `lgs` for localnet |
| Recommended for | low-level / size-critical programs | most projects |

## Common Gotchas

- **Mod name must match the guest binary.** `#[lez_program] mod my_program` requires `methods/guest/src/bin/my_program.rs`.
- **Don't hand-edit `<name>-idl.json` or `<name>_ffi/generated/`** — regenerate via `make idl` / `make ffi`.
- **`spel.toml` must be up to date.** After `lgs deploy`, the program address is written to `spel.toml`. If you reset the chain, re-deploy to refresh it.
- **`lgs spel -- <cmd>`** runs the project-vendored spel binary (pinned in scaffold.toml). Use it instead of any globally installed `spel` to avoid version drift.

## Adding a New Instruction

1. Add `#[instruction] pub fn <name>(…) -> SpelResult { … }` inside `#[lez_program]` in `<name>_core/src/lib.rs`.
2. `make idl` to regenerate the IDL.
3. `make ffi` to regenerate FFI bindings.
4. Test: `lgs build && lgs deploy && lgs spel -- <instruction> …`.

## Key Rules

- **`<name>-idl.json` and `<name>_ffi/generated/` are derived.** Always regenerate; never hand-edit.
- **Use `make` targets as the canonical workflow.** They encode the correct dependency order.
- **`spel.toml` is the source of truth for program address and IDL path.**
- **The `#[lez_program] mod` name is the program name.** Keep it in lockstep with the guest binary filename.
