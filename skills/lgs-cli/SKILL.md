---
name: lgs-cli
description: Use for the `lgs` / `logos-scaffold` CLI as a whole — bootstrap a project, run setup/build/deploy/localnet/wallet/doctor/report, diagnose CLI errors, or adopt scaffold in an existing project. Covers the full CLI surface, the `.scaffold/` state layout, and error → recovery patterns. Entry point that routes into `lez-template`, `lez-framework-template`, or `basecamp` once project context is identified.
---

# Using `logos-scaffold`

`logos-scaffold` (alias `lgs` — functionally identical) is a Rust CLI for bootstrapping LEZ (Logos Execution Zone) `program_deployment` projects in standalone mode. This skill is the entry point any time the user asks Claude to use scaffold itself — either to create / drive a project, or to recover from a failure.

## When to Use

- The user wants to create a new LEZ project (`lgs new`, `lgs create`).
- The user wants to run any `lgs` / `logos-scaffold` subcommand against an existing project (setup, build, deploy, localnet, wallet, spel, basecamp, doctor, report).
- A scaffold command failed and needs diagnosis (use the playbook + error table below).
- The user wants to adopt scaffold in an existing Rust/LEZ project (`lgs init`) or migrate an older `scaffold.toml`.

Once a project exists on disk, also pull in the matching template / integration skill:

- `lez-template` — bare LEZ standalone (Rust + risc0). Identify by absence of `framework = "lez-framework"` in `scaffold.toml` and presence of `methods/guest/src/bin/*.rs`.
- `lez-framework-template` — declarative macros (Anchor parallel). Identify by `framework = "lez-framework"` in `scaffold.toml` plus `crates/lez-client-gen/` and `idl/`.
- `basecamp` — `lgs basecamp …` lifecycle for Logos module projects (capture / install / launch with profile-isolated state). Activates additionally on any `lgs basecamp` invocation, presence of `[basecamp.modules]` in `scaffold.toml`, or `.scaffold/basecamp/profiles/`. Independent of the template skills — can layer onto either template or stand alone in an external module project.

## Command Map

`lgs` and `logos-scaffold` are interchangeable. Group by purpose:

| Group | Command | Purpose |
|---|---|---|
| Project | `new <name>` / `create <name>` | Scaffold a new project. Flags: `--template {default,lez-framework}`, `--vendor-deps`, `--lez-path`, `--cache-root`. |
| Project | `init` | Adopt scaffold in an existing project (writes `scaffold.toml`, creates `.scaffold/`, appends to `.gitignore`). Re-run to migrate older schemas or refresh shipped AI skills in place. |
| Project | `setup` | Sync LEZ + spel to pinned commits, build `sequencer_service` / `wallet` / `spel` locally, seed default wallet. Project-local; no PATH installs. |
| Project | `build [project-path]` | Runs `setup` then `cargo build --workspace`; auto-compiles `methods/Cargo.toml` if present. |
| Project | `deploy [program-name]` | Deploys one or all guest programs discovered in `methods/guest/src/bin/*.rs`. Prints `program_id` (risc0 image ID) on success. `--json` only structured when combined with `--program-path`. |
| Project | `run [--profile NAME] [--reset \| --no-reset] [--post-deploy <cmd>…] [--no-post-deploy] [--watch] [--localnet-timeout N]` | Inner loop: build (chains setup) → IDL → localnet → topup → deploy → optional post-deploy hooks. Prefer over manual setup/build/deploy/topup for daily work. Works with zero config; deploy is skipped when inputs + sequencer are unchanged. Does **not** run `check-health` or any `basecamp` command. |
| Runtime | `localnet start [--timeout-sec N]` | Spawn sequencer; waits for pid alive + 127.0.0.1:3040 reachable. |
| Runtime | `localnet stop` | Stop tracked sequencer. |
| Runtime | `localnet status [--json]` | Distinguishes managed / stale / foreign listener. |
| Runtime | `localnet logs [--tail N]` | Tail `.scaffold/logs/sequencer.log`. |
| Runtime | `wallet list [--long]` | List known wallet accounts. |
| Runtime | `wallet topup [<address>] [--dry-run]` | Auth-transfer init (if needed) + Piñata claim. Uses project default if address omitted. |
| Runtime | `wallet default set <address-ref>` | Persist project default wallet to `.scaffold/state/wallet.state`. |
| Runtime | `wallet -- <args>` | Raw passthrough to project-local wallet binary; preserves project wallet env. |
| Runtime | `spel -- <args>` | Raw passthrough to project-vendored `spel` binary. |
| Modules | `basecamp setup` | One-time: pin basecamp + lgpm, build, seed `alice` / `bob` profiles. |
| Modules | `basecamp modules [--show] [--flake REF]… [--path PATH]…` | Sole writer of `[basecamp.modules.<name>]` in `scaffold.toml`. |
| Modules | `basecamp install [--print-output]` | Build captured sources and install via `lgpm` into both profiles. |
| Modules | `basecamp launch <profile>` | Scrub profile, replay modules, exec basecamp. Profiles: `alice`, `bob`. |
| Modules | `basecamp build-portable` | Build `.#lgx-portable` for `role = "project"` entries; symlink under `.scaffold/basecamp/portable/`. |
| Modules | `basecamp doctor [--json]` | Basecamp-specific health (modules, variant check, dep drift, discovery drift). |
| Modules | `basecamp docs` | Print canonical `docs/basecamp-module-requirements.md`. |
| Diagnostics | `doctor [--json]` | Top-level health checks + actionable next steps. |
| Diagnostics | `report [--out PATH] [--tail N]` | Sanitised `.tar.gz` diagnostics bundle. |
| System | `completions <bash\|zsh>` | Print shell completion script (covers both `lgs` and `logos-scaffold`). |
| System | `help` / `<cmd> --help` | Help. Safe — `--help` does not create files. |

## First Success Path

From a scratch directory (per `templates/default/README.md`):

```bash
lgs new my-app
cd my-app
lgs run
lgs wallet -- check-health
```

`lgs run` chains setup → build → IDL → localnet → topup → deploy. The
step-by-step path (`setup` / `localnet start` / `build` / `deploy` / `topup`)
lives in the repo README under "Step-by-step (optional)" — reach for it to
debug or learn a single phase.

Checkpoint at any time:

```bash
lgs localnet status
lgs doctor
```

## Adopting an Existing Project

```bash
cd my-existing-project
lgs init      # writes scaffold.toml, creates .scaffold/, appends .gitignore
lgs setup
```

`init` does not touch `Cargo.toml` or `src/`. Re-run `init` to migrate older `scaffold.toml` schemas (legacy `[basecamp]` keys move to `[repos.basecamp]` / `[modules.*]`; legacy `url` on `[repos.{lez,spel}]` is dropped) or to refresh the shipped AI skills. Already-current configs succeed and leave `scaffold.toml` unchanged.

## `.scaffold/` Layout

Everything scaffold writes lives under `.scaffold/` inside the project. Treat this directory as the source of truth for runtime state, not the user's home.

| Path | Purpose |
|---|---|
| `.scaffold/state/localnet.state` | Sequencer PID. Stale entries are how `localnet status` reports `ownership: stale_state`. |
| `.scaffold/state/wallet.state` | Project default wallet address. Excluded from `report` archives. |
| `.scaffold/state/basecamp.state` | Basecamp + lgpm binary paths and pin-derived metadata. |
| `.scaffold/logs/sequencer.log` | Sequencer stdout/stderr. Tail with `lgs localnet logs --tail N`. |
| `.scaffold/logs/<ts>-install.log` | `basecamp install` per-source nix build logs. |
| `.scaffold/logs/<ts>-setup-*.log` | `basecamp setup` build logs. |
| `.scaffold/wallet/` | Project wallet home (`NSSA_WALLET_HOME_DIR`). **Never** included in `report` archives — contains keys. |
| `.scaffold/basecamp/profiles/{alice,bob}/` | Per-profile XDG roots for basecamp. |
| `.scaffold/basecamp/portable/<NN>-<name>.lgx` | Symlinks to `.#lgx-portable` builds for AppImage hand-loading. Wiped each `build-portable`. |
| `.scaffold/repos/{lez,spel}/` | Vendored repo checkouts (only when project was created with `--vendor-deps`). |
| `.scaffold/reports/report-<unix-ts>.tar.gz` | Output of `lgs report`. |
| `.scaffold/commands.md` | Canned reference scaffold ships into projects (sequencer command, status / doctor JSON commands, etc.). |

Non-vendored projects share a cache root: `<cache_root>/repos/<name>/<pin>/...` (configurable at `lgs new` via `--cache-root`).

## Debugging Playbook

Apply in order. Stop as soon as the issue is identified.

1. **`lgs doctor`** — prints actionable checks and next steps. Use `--json` for parsing. Doctor inspects: required binaries (git/rustc/cargo/lsof/ps/kill, docker or podman), LEZ + spel repo presence and pin alignment, sequencer + wallet + spel binaries, port 3040 reachability, runtime state file, wallet network config, wallet `--version` and `check-health`.
2. **`lgs localnet status [--json]`** — distinguishes `managed`, `stale_state`, `foreign` listener, and missing.
3. **`lgs localnet logs --tail 200`** — tail recent sequencer output.
4. **`cat .scaffold/logs/sequencer.log`** — full log if `--tail` isn't enough.
5. **`lgs report --tail 500`** — produce a shareable `.tar.gz` under `.scaffold/reports/`. **Always inspect the archive before sharing publicly** (`tar -tzf <path>` to list contents).

## Error → Recovery Table

| Symptom | Root cause | Fix |
|---|---|---|
| `localnet status` reports `ownership: stale_state` | Tracked PID no longer running. | `lgs localnet stop` then `lgs localnet start`. |
| `cannot start localnet: port 3040 already in use (pid=...)` | Foreign listener on 3040. | Identify holder, `kill <pid>`, retry `localnet start`. |
| `sequencer process exited before becoming ready (pid=<pid>)` | Sequencer crashed at startup. | `lgs localnet logs --tail 200`; investigate root cause before retrying. |
| `localnet start timed out after <N>s` | Slow startup (e.g. risc0 dev mode). | Increase `--timeout-sec`; check logs. |
| `missing sequencer binary at <path>; run \`logos-scaffold setup\`` | Setup never ran or binaries got cleaned. | `lgs setup`. |
| `Not a logos-scaffold project ... Run logos-scaffold create <name>` | Project-scoped command run outside a project. | `cd` into the project root, or run `lgs init` to adopt the current dir. |
| `scaffold.toml` schema mismatch (e.g. missing `[repos.spel]`) | Project predates current schema. | `lgs init` (idempotent migration); then `lgs setup`. |
| Doctor warns LEZ pin drift | `[repos.lez].pin` differs from scaffold default. | Either update `scaffold.toml` to the default and `lgs setup`, or accept the divergence. |
| Doctor warns spel/LEZ protocol mismatch | `spel-cli/Cargo.toml` vendors a different LEZ than scaffold. | Bump `[repos.spel].pin` to a commit whose spel-cli pins matching LEZ. |
| `wallet -- check-health` fails | Sequencer down or `NSSA_WALLET_HOME_DIR` not set for direct `cargo run`. | `lgs localnet start`; for direct runners: `export NSSA_WALLET_HOME_DIR=$(pwd)/.scaffold/wallet`. |
| `basecamp not set up yet` hint | `lgs basecamp setup` never ran in this project. | `lgs basecamp setup` (one-time per project). |
| `basecamp install` fails with `no \`main\` field in metadata.json` | Sub-flake transitively pulls a newer `logos-module-builder`. | Add `inputs.<dep>.inputs.logos-module-builder.follows = "logos-module-builder";` in the offending sub-flake; `nix flake update`. See `docs/basecamp-module-requirements.md`. |
| `basecamp build-portable` fails: `.#lgx-portable` not exposed | Flake only exposes `.#lgx`. | Add the `lgx-portable` output, or pass `--flake <ref>#lgx-portable` to opt in explicitly. |
| Working tree dirty in vendored repo (lez / spel) | Manual edits in cached checkout. | Commit, stash, or reset the change in `.scaffold/repos/<name>/`. |

## Environment Overrides

| Variable | Purpose |
|---|---|
| `LOGOS_SCAFFOLD_WALLET_PASSWORD` | Override the default wallet password. Forwarded through `wallet --` passthrough. |
| `NSSA_WALLET_HOME_DIR` | Wallet home dir; required for direct `cargo run --bin run_*`. Scaffold wallet commands set it automatically. |
| `LOGOS_SCAFFOLD_PRINT_OUTPUT` | Equivalent to `--print-output`; streams nix output instead of writing to `.scaffold/logs/`. |
| `EXAMPLE_PROGRAMS_BUILD_DIR` | Override the default risc0 guest build dir for explicit `--program-path` invocations. |

## JSON Outputs

Use `--json` whenever piping to other tools:

```bash
lgs localnet status --json    # { tracked_pid, listener_present, ownership, ready }
lgs doctor --json             # { status, summary, checks, next_steps }
lgs deploy --program-path "<path>" --json   # { status, program, tx?, program_id? }
lgs basecamp doctor --json
```

`lgs deploy --json` only produces structured JSON when combined with `--program-path`. On the discovery path, `--json` is silently accepted but ignored.

## DOGFOODING Cross-Reference

The canonical scenarios in `DOGFOODING.md`:

- **D1–D6** — default template (bootstrap, localnet/doctor, deploy variants, wallet, report, runner interaction).
- **L1–L4** — lez-framework template (bootstrap, IDL regen, client regen, deploy + counter).
- **E1–E2** — CLI surface (help/version/error quality, advanced `new` flags).
- **B1–B5** — basecamp (setup, modules+install+launch, p2p, clean-slate, build-portable).

When reproducing a failure, name the matching scenario in the bug report.

## Key Rules

- **Never** `rm -rf .scaffold/wallet/` — it contains keys; `lgs report` deliberately excludes it.
- **Never** edit `[basecamp.modules]` while `lgs basecamp modules` is running. Otherwise hand-edits in that section are preserved across re-runs.
- **Always** `lgs localnet stop` before any destructive reset; otherwise scaffold may leave a stale PID.
- **Always** inspect `.scaffold/reports/*.tar.gz` (`tar -tzf <path>`) before sharing publicly. Sanitisation is best-effort, not absolute.
- **Prefer** project-local binaries via `lgs wallet -- ...` and `lgs spel -- ...` over global installs. Nothing scaffold builds is added to PATH on purpose.
- **Don't** assume `--json` is structured for every command; it's structured only where the table above says so (status, doctor, deploy w/ `--program-path`, basecamp doctor).
- Project-scoped commands run outside a project root produce a clear `Not a logos-scaffold project ...` error — don't try to work around it; `cd` into the project or `lgs init`.
