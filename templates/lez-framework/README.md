# LEZ Framework Template

This project was generated with:

```bash
logos-scaffold new <name> --template lez-framework
```

It uses the [LEZ Framework](https://github.com/jimmy-claw/lez-framework) for an
ergonomic developer experience similar to Anchor on Solana:

- `#[lez_program]` macro eliminates boilerplate
- `#[instruction]` attribute marks instruction handlers
- `#[account(...)]` annotations for account constraints and PDA derivation
- Compile-time IDL generation via `PROGRAM_IDL_JSON`

## First Run

```bash
logos-scaffold run
logos-scaffold doctor
```

`logos-scaffold run` includes build and IDL generation for lez-framework
projects, then starts localnet, tops up the wallet, and deploys — all in one
pipeline.

### First run step-by-step (optional)

```bash
logos-scaffold setup
logos-scaffold localnet start
logos-scaffold doctor
```

## Build

Equivalent to the build phase inside `run`; use it to iterate on the program
without the full pipeline:

```bash
logos-scaffold build
```

## IDL

Equivalent to the IDL phase inside `run`:

```bash
logos-scaffold build idl
```

## Diagnostics Bundle

```bash
logos-scaffold report [--out PATH] [--tail N]
```

Inspect the generated archive before attaching it to public issues.

## Project Structure

- Program: `methods/guest/src/bin/lez_counter.rs`
- Generated IDL: `idl/lez_counter.json`
- Runner: `src/bin/run_lez_counter.rs`

## Writing Programs

```rust
#[lez_program]
mod my_program {
    #[instruction]
    pub fn my_handler(
        #[account(init, pda = literal("state"))]
        state: AccountWithMetadata,
        #[account(signer)]
        authority: AccountWithMetadata,
    ) -> LezResult {
        // your logic here
    }
}
```
