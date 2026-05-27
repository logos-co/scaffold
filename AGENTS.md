# AGENTS.md

Conventions for AI coding agents (and humans skimming context) working in this repository.

## What this repo is

A Rust CLI named `logos-scaffold` (alias `lgs`) that bootstraps Logos Execution Zone (LEZ) program-deployment projects. The CLI also manages a local sequencer, vendored wallet and `spel` binaries, IDL/client generation, and deploy. See [README.md](README.md) for the user-facing surface.

## Skills

This repo ships scoped AI skills under [`skills/`](skills/). Each `SKILL.md` declares when to activate it. Load the relevant skill before working on its area:

- [`skills/lgs-cli/SKILL.md`](skills/lgs-cli/SKILL.md) — the full `lgs` / `logos-scaffold` CLI: bootstrap, setup, build, deploy, localnet, wallet, doctor, report. Entry point that routes into the template/basecamp skills below.
- [`skills/lez-template/SKILL.md`](skills/lez-template/SKILL.md) — projects scaffolded with the bare LEZ template (raw Rust + risc0 guest programs, no framework macros).
- [`skills/lez-framework-template/SKILL.md`](skills/lez-framework-template/SKILL.md) — projects scaffolded with the lez-framework template (Anchor-on-Solana parallel: `#[lez_program]` / `#[instruction]` / `#[account(...)]` macros, auto-generated IDL).
- [`skills/basecamp/SKILL.md`](skills/basecamp/SKILL.md) — `lgs basecamp …` subcommands and Logos module projects (`.lgx` artefacts via `flake.nix#packages.<system>.lgx`).

## Build and test

```bash
cargo build
cargo test --all-targets
cargo fmt --check
```

Run the CLI from source while iterating:

```bash
cargo run --bin logos-scaffold -- --help
cargo run --bin logos-scaffold -- new test-app
```

## Where things live

- `src/cli.rs` — clap CLI definition. Update here when adding/removing subcommands.
- `src/commands/` — per-command implementations (one file per top-level subcommand).
- `src/bin/{logos-scaffold,lgs}.rs` — binary entry points (functionally identical).
- `templates/` — project templates copied by `lgs new`.
- `tests/` — integration tests using `assert_cmd`.
- `.scaffold/` — per-project state written by the CLI inside generated projects (state, logs, profiles). Not part of this repo's source tree.

## Validation

User-facing changes must rerun the applicable scenarios in [DOGFOODING.md](DOGFOODING.md). At minimum:

- Onboarding / `new` / `setup` / `localnet` / `build` changes → `D1`, `D2`.
- `deploy` / `wallet` / `doctor` / `report` changes → `D3`–`D5`.
- LEZ template or generated-artifact changes → `L1`–`L4`.

If the change affects user-facing behavior, update `README.md` and `DOGFOODING.md` in the same PR (per [CONTRIBUTING.md](CONTRIBUTING.md)).

## What to avoid

- Don't add a global install step for the vendored binaries (`wallet`, `spel`). They are intentionally project-local; users invoke them via `lgs wallet -- …` / `lgs spel -- …`.
- Don't introduce a dependency on `logos-blockchain`. Standalone sequencer flow only (see [Scope](README.md#scope)).
- Don't bypass the rate-limit workflow on PRs. See [CONTRIBUTING.md](CONTRIBUTING.md#rate-limit).
