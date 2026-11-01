# logos-scaffold

Back to [index](index.md).

## What it is
`logos-scaffold` is a Rust CLI, shipped as two identical binaries `logos-scaffold` and `lgs`.
It creates, builds, runs and deploys Logos (LEZ) programs against a project-local execution
zone sequencer. It also gives integration tests disposable sequencers (`test-node`) and builds
and launches Logos `.lgx` modules in isolated basecamp desktop profiles (`basecamp`). The same
capabilities are exposed as a typed Rust library under `logos_scaffold::api`.

## Users
- Program developers who want a Foundry/Anchor-style inner loop: `lgs new`, then `lgs run`.
- Test authors who need short-lived, isolated sequencers and typed RPC observations.
- Logos module developers who build `.lgx` modules and dogfood them in two basecamp
  instances (`alice`/`bob`) on one machine.
- Tool authors who embed scaffold as a library instead of shelling out.

## Surfaces
- CLI definitions (source of truth for flags): `src/cli.rs`; one module per command in
  `src/commands/`. Reference: `docs/commands.md`, `docs/configuration.md`.
- Project creation and adoption: `new`/`create`, `init`. Templates: `templates/default`
  (bare LEZ), and `--template spel`, which delegates to the pinned `spel init`.
- Toolchain and build: `setup [--prebuilt]`, `build [--guest local|docker]`, `build idl`,
  `build client`.
- Local sequencer: `localnet start|stop|status|logs|reset`.
- Deploy and the inner loop: `deploy`, `run` (profiles, hooks, `--watch`, `--reset`).
- Wallet: `wallet list|topup|default set`, `wallet -- <raw wallet args>`.
- spel passthrough: `spel -- <raw spel args>`.
- Diagnostics: `doctor [--json]`, `report`, `completions <bash|zsh>`.
- Test nodes: `test-node pins|prepare|doctor|start|status|stop|run|tx|blocks|clock|account|proof|snapshot|state`.
- Basecamp: `basecamp setup|modules|install|launch|develop|build|build-portable|run|paths|doctor|docs`.
- AI skills shipped into every project: `skills/` → `.claude/skills/`, `.cursor/rules/`,
  `AGENTS.md`.
- Rust API: `src/api/` (`Project`, `api::testnode::{TestNode, TestNodeClient}`, `api::Error`).
- Hidden `self-test` subcommand: test-only, not a user surface.

## How to build and run
Do this from a fresh clone. `DOGFOODING.md` is the detailed runbook and these scenarios
reference its sections.

```bash
export REPO_ROOT=/abs/path/to/logos-scaffold
export SCRATCH_ROOT=/abs/path/to/dogfood-runs     # outside the repo
cd "$REPO_ROOT" && cargo build
export SCAFFOLD_BIN="$REPO_ROOT/target/debug/logos-scaffold"   # or use lgs
mkdir -p "$SCRATCH_ROOT"
```

Toolchain per area:
- **All areas:** Unix (Linux or macOS), `git`, `rustc`/`cargo` ≥ 1.81, `lsof`, `ps`, `kill`,
  and `curl` (circuits download). Linux also needs `pkg-config` plus the PCSC headers
  (`libpcsclite-dev`), because the LEZ wallet links against libpcsclite.
- **Guest builds (default `docker` mode):** a running Docker or Podman daemon and
  `cargo-risczero` (`rzup install cargo-risczero 3.0.5`). Without Docker, use
  `build --guest local` or `[build].guest = "local"`, and say so in the report.
- **Real sequencer (localnet, deploy, run, test-node):** `r0vm` at the exact risc0 version in
  the pinned LEZ `Cargo.lock`, the risc0 Rust toolchain, and for the spel template also the
  risc0 C++ toolchain. Follow "Provisioning the Real LEZ Sequencer Toolchain" in
  `DOGFOODING.md`, which includes curl fallbacks for TLS-intercepting proxies.
- **Basecamp:** Nix with flakes, unrestricted GitHub access for Nix, at least 5 GB free RAM
  to evaluate the basecamp flake, and a graphical session (Xvfb works for launch). You also
  need a module project built with `logos-module-builder` 0.3.0, such as that repo's
  `templates/minimal-module` at tag `0.3.0`.
- **Rust API:** run `cargo doc --no-deps` and `cargo test --doc` from the repo root.

Long steps: the first `setup` takes about 3 minutes (wallet + spel). The sequencer build
takes about 6 minutes. The first docker guest build pulls a ~1.7 GB image. A cold basecamp
setup builds a Qt closure unless `https://cache.nix.logos.co/public` is trusted. Run long
steps in the background and wait on a sentinel file.

Useful env vars: `LOGOS_SCAFFOLD_CACHE_ROOT`, `LOGOS_SCAFFOLD_WALLET_PASSWORD`,
`LOGOS_BLOCKCHAIN_CIRCUITS` (local circuits override), `LOGOS_SCAFFOLD_QUIET`,
`LOGOS_SCAFFOLD_PRINT_OUTPUT`, `LOGOS_SCAFFOLD_DEPLOY_PACING_TIMEOUT_MS`, and `RISC0_HOME`.

No credentials are needed. A `GH_TOKEN` helps only to avoid GitHub API rate limits while
provisioning.

## Environments and side effects
- **Local only:** everything runs against project-local state. The localnet sequencer
  listens on `127.0.0.1:<[localnet].port>` (default 3040). Test nodes use their own ports
  under `.scaffold/test-nodes/`. Basecamp profiles live under `.scaffold/basecamp/`.
  Nothing deploys to a public network, and no DevNet target exists today.
- **Shared on the machine:** the scaffold cache (`<cache_root>/repos/<name>/<pin>`) is shared
  between projects. Basecamp's default runtime dir is `/tmp/lgs-<project-hash>-<profile>`.
  `rzup`'s home (`~/.risc0`) and the Nix store are global.
- **Destructive commands, safe in a scratch project:** `localnet reset --yes`, especially
  with `--reset-wallet` (irrecoverable wallet deletion), `run --reset`, and
  `basecamp launch` (scrubs the profile).
- **Never upload without asking the user:** `report` archives. Redaction is best-effort, so
  list the archive's contents first.
- **Network:** the code fetches from GitHub (LEZ, spel, circuits, Nix inputs) and from
  Docker Hub (`risczero/risc0-guest-builder`). It does no other outbound calls and has no
  telemetry.

## Out of scope
- DevNet or any public-network deploy: not implemented, even though older requirement text
  mentions it.
- Loading `.lgx` into a basecamp AppImage. Scaffold stops at producing the artefacts, by
  design (ADR "AppImage Path is Outside Scaffold's Scope").
- `basecamp run --host basecamp` (not available yet) and the hidden `self-test` command.
- Unit and integration tests in `tests/`. Those are CI's job; these scenarios cover user
  behaviour.

## Glossary
- **LEZ:** Logos Execution Zone. It provides the `sequencer_service` and `wallet` binaries
  that scaffold builds from a pinned commit.
- **spel:** the SPEL framework CLI (`logos-co/spel`). It is vendored per project and used for
  `program-id`, IDL and client generation, and the spel template.
- **program_id:** the risc0 image ID of a guest ELF, computed locally by `spel program-id`.
- **Guest build mode:** `docker` (default; reproducible via the pinned
  `risczero/risc0-guest-builder` image) or `local` (host toolchain; not reproducible).
- **Circuits:** the `logos-blockchain-circuits` release pinned in `[circuits]` and
  materialized into `.scaffold/circuits`.
- **Basecamp / lgpm:** the Logos desktop app and its package manager. Scaffold supports
  basecamp 0.3.0 only, and the basecamp and lgpm pins move as one set.
- **Profile:** an isolated basecamp XDG/HOME tree (`alice`, `bob`, or a custom profile).
- **Test node:** a disposable sequencer instance for integration tests.
