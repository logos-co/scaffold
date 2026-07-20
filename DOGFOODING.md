# `logos-scaffold` dogfooding Scenarios

This document is the canonical dogfooding runbook for `logos-scaffold`.
Use it to evaluate the latest repository state, not as a dated findings report.
Earlier one-off dogfood notes are historical context only; future runs should start here.

> Maintenance note: update this document whenever first-class commands, templates, supported workflows, or major user-facing behaviors are added, removed, or materially changed. If the product surface changes and this runbook does not, the runbook is wrong.

## Agent Execution Directives

If you are an automated agent running this runbook, treat the following as binding. They exist because a previous run stopped at "this environment can't run a real sequencer" when, in fact, it could — the toolchain just had to be installed by hand.

1. **Do the real thing.** End-to-end dogfooding means a real, running LEZ sequencer executing real transactions — not a stub, a mock, or a "validated by composition" shortcut. A stub (e.g. a scripted JSON-RPC server) is acceptable only as an *addition* to a real run, never a substitute. If you catch yourself writing "this environment can't…", stop and apply directive 3 before you believe it.
2. **Install every dependency yourself.** The real path needs the risc0 toolchain (`r0vm`), the `logos-blockchain-circuits` release, and the LEZ-built `sequencer_service` (plus `wallet` for transactions). None ship in a fresh container. Provision them per "Provisioning the Real LEZ Sequencer Toolchain" below. The container has `curl`, network access to GitHub, and (usually) a `GH_TOKEN` env var — that is enough.
3. **Push through difficulties; a failed first attempt is a routing problem, not a verdict.** Expect the obvious installers to fail and route around them:
   - `rzup` downloads over rustls and fails with `InvalidCertificate(UnknownIssuer)` behind a TLS-intercepting proxy. Fall back to `curl`, which trusts the system CA, to fetch the release tarball directly.
   - Unauthenticated GitHub API calls hit rate limits; add `-H "Authorization: Bearer $GH_TOKEN"`.
   - `git push` and PR creation via the default proxy/MCP may `403`; push and open PRs with `https://x-access-token:$GH_TOKEN@github.com/<owner>/<repo>.git` and the GitHub REST API.
   Do not report "impossible" until you have exhausted the curl + token fallbacks.
4. **Builds are long; background them so they survive turns.** The sequencer build is ~6 min, `setup` (wallet + spel) ~3 min. Launch them with a mechanism that outlives a single shell — the harness's background-run, or a Monitor with an `until` loop — because `nohup … &` from a one-shot shell gets killed when the shell tears down. Wait on a sentinel (the built binary path, or an `EXIT=` marker you append), not a fixed sleep.
5. **Leave the box provisioned.** Once installed, the toolchain persists in the container cache and the `~/.risc0` extensions dir, so later runs are fast. Reuse it rather than reinstalling.

## Purpose and Audience

- Dogfooders: use this as a repeatable checklist when validating the latest scaffold DX.
- Contributors: use this document to decide which scenarios must be rerun for a given change.

This guide is intentionally scenario-oriented:

- It defines what to exercise.
- It defines what success looks like.
- It calls out the failures and caveats that are worth recording.
- It does not replace generated project READMEs or CLI help text.

## Usage Model

The recommended dogfooding pattern is:

1. Build the local scaffold binary from the repository under test.
2. Create fresh generated projects in a scratch workspace outside the repo.
3. Run project-level scenarios from inside the generated project root.
4. Capture command, cwd, exit code, and a short output excerpt for each scenario.
5. If the behavior differs from this runbook, update the runbook when the difference is intentional and file a bug when it is not.

For repo dogfooding, prefer the freshly built local binary over an already-installed global binary.

Scaffold now treats LEZ tooling as project-local state. For non-vendored projects, the shared cache layout is `<cache_root>/repos/lez/<pin>/...`; for vendored projects, LEZ lives under `<project>/.scaffold/repos/lez`. In both cases, the wallet binary under test is the LEZ-local build artifact at `<lez>/target/release/wallet`, invoked through `logos-scaffold wallet ...` rather than a `wallet` binary on `PATH`.

```bash
export REPO_ROOT=/absolute/path/to/logos-scaffold
export SCRATCH_ROOT=/absolute/path/to/dogfood-runs
cd "$REPO_ROOT"
cargo build
export SCAFFOLD_BIN="$REPO_ROOT/target/debug/logos-scaffold"
mkdir -p "$SCRATCH_ROOT"
```

You may replace `"$SCAFFOLD_BIN"` with `logos-scaffold` when the install path itself is part of what you are validating.

## Execution Contexts

| Context | Purpose | Typical commands |
| --- | --- | --- |
| Repo root | Build the latest CLI, inspect docs, validate help/version output, verify out-of-project errors | `cargo build`, `"$SCAFFOLD_BIN" --help`, `"$SCAFFOLD_BIN" --version`, `"$SCAFFOLD_BIN" build` (expect error) |
| Scratch workspace | Create fresh generated projects without polluting the repo; test advanced creation flags | `"$SCAFFOLD_BIN" new dogfood-default`, `"$SCAFFOLD_BIN" new ... --template ...` |
| Generated project root | Execute scaffold workflows and example runners against a fresh project | `setup`, `localnet`, `build`, `deploy`, `wallet`, `doctor`, `report`, `cargo run --bin run_*` |
| Test-node host | Drive isolated, short-lived integration-test sequencers from inside a project (or any directory via `--project <root>`); the RPC-scoped reads target a node URL directly | `test-node prepare`, `test-node start --json`, `test-node run -- <cmd>`, `test-node tx submit-and-wait --url ...` |

Do not run project-scoped commands from the repository root unless the scenario is explicitly checking the "outside project" error path. `test-node` commands are the exception: they accept an explicit `--project <root>` and may be driven from outside a project directory.

Scaffold is also consumable as a Rust library (`logos_scaffold::api`): the same setup/localnet/wallet/deploy/doctor/report and `test-node` capabilities are exposed as typed functions returning typed results and categorized errors, so downstream tests and tooling can embed scaffold without shelling out to the CLI. Scenario `A1` validates that surface.

## Shared Preconditions

- Unix-like environment with `git`, `rustc`, `cargo`, `lsof`, `ps`, and `kill`.
- Docker or Podman available for guest builds.
- `logos-blockchain-circuits` release on disk when validating older projects without `[circuits]`: set `LOGOS_BLOCKCHAIN_CIRCUITS=<path>` (scaffold no longer consults `~/.logos-blockchain-circuits/`). New projects should carry a `[circuits]` table in `scaffold.toml`; `setup`, `build`, `build idl`, `localnet`, and test-node startup resolve and materialize the configured release instead of relying on ambient shell state.
- No conflicting listener on the scaffold localnet port before `localnet start`.
- Network access available for setup/build flows that fetch dependencies.
- No preinstalled `wallet` binary is required. If one exists on `PATH`, do not treat it as the runtime under test for scaffold wallet scenarios.
- Optional but supported: `LOGOS_SCAFFOLD_WALLET_PASSWORD` when validating password override behavior.
- For `B`-series (basecamp) scenarios: Nix with flakes enabled, plus a module project on disk whose `flake.nix` exposes a `packages.<system>.lgx` output (e.g., a `tictactoe`-style project built against the `logos-module-builder` `tutorial-v1` convention). `docs/basecamp-module-requirements.md` (also reachable via `"$SCAFFOLD_BIN" basecamp docs`) is the canonical contract.

The `lgs` binary is a short alias for `logos-scaffold` produced by the same crate; `"$SCAFFOLD_BIN"` and `lgs` are interchangeable in the commands below.

## Provisioning the Real LEZ Sequencer Toolchain

The `T`-series (and any real `localnet` / `deploy` / `run` validation) needs a real LEZ sequencer. A fresh container has none of the pieces; provision them once — they then live in the scaffold cache and `~/.risc0`, and persist for later runs. This whole section was reverse-engineered from a real run; follow it rather than re-deriving it.

Assume `"$P"` is a generated project root (e.g. `$SCRATCH_ROOT/dogfood-default`) and `GH_TOKEN` is set.

**1. Discover the risc0 version the pinned LEZ needs.** scaffold's r0vm auto-detect requires an *exact* version match, read from the LEZ `Cargo.lock`:

```bash
LEZ=$("$SCAFFOLD_BIN" test-node pins --project "$P" --json | jq -r .lez_checkout)
grep -A1 'name = "risc0-zkvm"' "$LEZ/Cargo.lock"   # e.g. version = "3.0.5"
```

**2. Install r0vm at that version.** Try `rzup install r0vm <ver>` first (`curl -sSL https://risczero.com/install | bash` installs rzup). In a TLS-intercepting environment rzup fails with `InvalidCertificate(UnknownIssuer)`; fall back to `curl` for the release tarball (which bundles `r0vm` and `cargo-risczero`):

```bash
VER=3.0.5; TRIPLE=x86_64-unknown-linux-gnu       # aarch64-apple-darwin on macOS
curl -sSL -H "Authorization: Bearer $GH_TOKEN" -o /tmp/cr.tgz \
  "https://github.com/risc0/risc0/releases/download/v$VER/cargo-risczero-$TRIPLE.tgz"
mkdir -p /tmp/cr && tar xzf /tmp/cr.tgz -C /tmp/cr
EXT="$HOME/.risc0/extensions/v$VER-cargo-risczero-$TRIPLE"   # scaffold's expected path
mkdir -p "$EXT" && cp /tmp/cr/r0vm "$EXT/r0vm" && chmod +x "$EXT/r0vm"
"$EXT/r0vm" --version                              # → risc0-r0vm 3.0.5
```

`find_r0vm_path_for_lez` looks at `~/.risc0/extensions/v<risc0-zkvm-version>-cargo-risczero-<arch>-<os>/r0vm`; placing r0vm there is what wires it into a spawned sequencer (scaffold sets `RISC0_SERVER_PATH` from it). The sequencer runs with `RISC0_DEV_MODE=1` (the test-node default), so r0vm executes guests without real proving — which is why no GPU/prover is needed.

**2b. Install the risc0 Rust toolchain (needed for `build`/`deploy`, not for the T-series).** `r0vm` alone runs guests; it does *not* compile them. `logos-scaffold build` (and therefore `deploy`/`run`/`D`/`L` scenarios) shells out through `risc0-build`, which asks the `rzup` library for a default-installed Rust toolchain and panics with `Risc Zero Rust toolchain not found. Try running 'rzup install rust'` if none exists. `rzup install rust` fails the same way behind a TLS-intercepting proxy, so install it by hand from the `risc0/rust` release and register it where the `rzup` library looks (`~/.risc0/toolchains/` + `~/.risc0/settings.toml`):

```bash
RVER=1.94.1   # match the toolchain the risc0 release line ships; `rzup install rust` would pick the latest r0.* tag
curl -sSL -H "Authorization: Bearer $GH_TOKEN" -o /tmp/rust-toolchain.tgz \
  "https://github.com/risc0/rust/releases/download/r0.$RVER/rust-toolchain-$TRIPLE.tar.gz"
VDIR="$HOME/.risc0/toolchains/v$RVER-rust-$TRIPLE"
mkdir -p "$VDIR" && tar xzf /tmp/rust-toolchain.tgz -C "$VDIR"
printf '[default_versions]\nrust = "%s"\n' "$RVER" > "$HOME/.risc0/settings.toml"
ln -sfn "$VDIR" "$HOME/.rustup/toolchains/risc0"
ls "$VDIR/lib/rustlib" | grep riscv32im   # → riscv32im-risc0-zkvm-elf (guest target present)
```

Circuits need no equivalent hand-provisioning for generated projects: `build`/`setup` resolve the `[circuits]` table in `scaffold.toml`, materialise the configured release into `[circuits].install_dir` (default `.scaffold/circuits`), and export `LOGOS_BLOCKCHAIN_CIRCUITS` for the rest of the process. A pre-set `LOGOS_BLOCKCHAIN_CIRCUITS` pointing at a populated release short-circuits the download; only older projects without `[circuits]` still require it.

**3. Build the real sequencer.** `test-node prepare` downloads the circuits release (via `curl`, automatically) and builds `sequencer_service`. It is long (~6 min); run it, then confirm doctor is green:

```bash
"$SCAFFOLD_BIN" test-node prepare --project "$P"            # → "test-node prerequisites ready"
"$SCAFFOLD_BIN" test-node doctor  --project "$P" --json | jq .ok   # → true (all checks pass)
```

**4. (Only for real transactions — T4) build the wallet via `setup`.** One gotcha distinguishes `setup` from `test-node prepare`: it uses cwd discovery, so it must run **inside** the project (no `--project` flag). Projects with a `[circuits]` table materialise their configured release during `setup` itself; only for older projects without `[circuits]` must `LOGOS_BLOCKCHAIN_CIRCUITS` be exported first:

```bash
# Only needed for older projects without [circuits] in scaffold.toml:
export LOGOS_BLOCKCHAIN_CIRCUITS="$("$SCAFFOLD_BIN" test-node pins --project "$P" --json | jq -r .circuits_path)"
( cd "$P" && "$SCAFFOLD_BIN" setup )   # builds wallet + spel, seeds the default wallet (~3 min) → "setup complete"
```

Sanity-check the provisioned toolchain before running the T-series:

```bash
"$EXT/r0vm" --version
ls "$LEZ/target/release/sequencer_service"          # real sequencer (T1–T3)
ls "$LEZ/target/release/wallet"                     # real wallet (T4)
ls "$LOGOS_BLOCKCHAIN_CIRCUITS"/pol/verification_key.json
```

If any of these is missing, do not "skip the real run" — go back and fix the step that produced it.

## Scenario Index

| ID | Template | Level | Goal | Command surface |
| --- | --- | --- | --- | --- |
| D1 | `default` | Core | Fresh project creation and first-success bootstrap | `new`, `create`, `setup`, `localnet start`, `build`, `deploy`, `wallet topup`, `wallet -- check-health` |
| D2 | `default` | Core | Localnet lifecycle visibility and doctor checks | `localnet status`, `localnet logs`, `localnet stop`, `doctor`, JSON variants |
| D3 | `default` | Advanced | Deploy path variations and machine-readable single-program submission | `deploy [program-name]`, `deploy --program-path`, `deploy --program-path --json` |
| D4 | `default` | Core | Wallet management, default-address behavior, and passthrough UX | `wallet list`, `wallet default set`, `wallet topup --dry-run`, `wallet topup`, `wallet -- ...` |
| D5 | `default` | Advanced | Diagnostics bundle and support artifact hygiene | `report`, `report --out`, `report --tail` |
| D6 | `default` | Core | Example runner interaction and account state verification | `cargo run --bin run_hello_world`, `cargo run --bin run_hello_world_with_move_function`, `wallet -- account get` |
| D7 | `default` | Core | One-step `run` pipeline and post-deploy hooks | `run`, `run --post-deploy`, `run --no-post-deploy`, `[run]` config |
| L1 | `lez-framework` | Core | Fresh LEZ project bootstrap to ready state | `new --template lez-framework`, `setup`, `localnet start`, `doctor`, `build` |
| L2 | `lez-framework` | Core | LEZ IDL regeneration | `build idl` |
| L3 | `lez-framework` | Advanced | LEZ client generation from current IDL | `build client` |
| L4 | `lez-framework` | Core | LEZ deploy and counter interaction | `deploy`, `cargo run --bin run_lez_counter` |
| E1 | N/A | Core | CLI discoverability and error quality | `--help`, `help`, `--version`, unknown commands, out-of-project errors |
| E2 | N/A | Advanced | Project creation with advanced flags and invalid inputs | `new --template`, `new --vendor-deps`, `new --cache-root` |
| E3 | N/A | Core | AI skills materialized into generated and adopted projects | `new`, `new --template lez-framework`, `init`, `init` re-run |
| B1 | external module project | Core | Basecamp + lgpm setup and idempotent re-run | `init`, `basecamp setup`, `basecamp doctor`, `basecamp docs` |
| B2 | external module project | Core | Module capture, install, paths, and single-instance launch | `basecamp modules`, `basecamp modules --show`, `basecamp install`, `basecamp paths`, `basecamp launch <profile>` |
| B3 | external module project | Core | Two-instance p2p dogfooding | `basecamp launch <profile>` (parallel) |
| B4 | external module project | Advanced | Clean-slate and profile safety on relaunch | `basecamp launch <profile>` (×2), custom profile names |
| B5 | external module project | Advanced | Module artefact builds by variant | `basecamp build`, `basecamp build-portable`, `--variant`, `--module` |
| B6 | external module project | Advanced | Captured module run loop | `basecamp run <module>`, `--host standalone` |
| A1 | N/A | Advanced | Public Rust API surface for embedding scaffold in tests/tooling | `logos_scaffold::api::Project`, `cargo doc`, doctests |
| T1 | `default` | Advanced | Isolated test-node lifecycle and caller-project pins | `test-node pins`, `test-node prepare`, `test-node doctor`, `test-node start`, `test-node status`, `test-node stop`, `test-node run` |
| T2 | `default` | Advanced | Typed RPC reads against a running test node | `test-node tx submit-and-wait`, `test-node blocks head/range/wait`, `test-node clock read/wait-stable`, `test-node account get/batch-get`, `test-node proof get`, `test-node snapshot accounts` |
| T3 | `default` | Advanced | Caller-provided state seeding | `test-node state schema`, `test-node state export`, `test-node state seed`, `test-node start --state` |
| T4 | `default` | Advanced | Real committed user transaction (test-node + wallet) | `test-node start --port`, `wallet topup`, `test-node blocks wait`, `test-node tx wait` |

## Standing Validation Notes

- Project context matters. Many scaffold commands are meant to be run only inside a generated project root. Running them elsewhere should produce a clear error, not silent misbehavior.
- Localnet readiness, listener ownership, and wallet connectivity are high-value validation points. Record contradictions instead of smoothing over them.
- Machine-readable paths matter for tooling. Preserve `--json` outputs when a scenario includes them.
- `report` is sanitized on a best-effort basis, not on an absolute guarantee. Always inspect the archive before sharing it.
- When wallet behavior depends on an omitted address, verify whether the project default wallet was seeded and persisted as expected.
- Example runner programs (`cargo run --bin run_*`) are the final proof that the scaffold pipeline works end-to-end. A successful deploy means nothing if the runner cannot interact with the deployed program.

## D1. Default Template Bootstrap and First Success

### Goal

Validate that the default template can be scaffolded from the latest repo and reach the documented first-success path.

### Preconditions

- `cargo build` completed at the repo root.
- `"$SCAFFOLD_BIN"` points to the freshly built binary.
- Scratch workspace exists and is writable.

### Commands / Actions

From the scratch workspace:

```bash
cd "$SCRATCH_ROOT"
"$SCAFFOLD_BIN" new dogfood-default
"$SCAFFOLD_BIN" create dogfood-default-create
cd dogfood-default
"$SCAFFOLD_BIN" setup
"$SCAFFOLD_BIN" localnet start
"$SCAFFOLD_BIN" build
"$SCAFFOLD_BIN" deploy
"$SCAFFOLD_BIN" wallet topup
"$SCAFFOLD_BIN" wallet -- check-health
```

Use `new` for the main runnable project and `create` as the lightweight alias-parity check in a separate directory. Both commands also accept `--template`, `--vendor-deps`, `--lez-path`, and `--cache-root`, but this scenario uses defaults only. See E2 for advanced flag coverage.

### Expected Success Signals

- Project creation succeeds and prints the destination path, pinned LEZ commit, and cache root.
- Generated `scaffold.toml` includes a `[circuits]` table. The default install dir is project-local (`.scaffold/circuits`), and the configured version/download template/install dir become the single source of truth for commands that need `logos-blockchain-circuits`.
- `setup` completes after syncing LEZ to the configured pin, building both `sequencer_service` and `wallet` inside the project's LEZ tree, and either seeding the default wallet or reporting that a default wallet is already configured. With `--prebuilt`: `sequencer_service` is downloaded instead of built from source (falls back to source build if no artifact is published); `wallet` is always built from source regardless of `--prebuilt`.
- `localnet start` reports a ready localnet rather than only a spawned PID.
- `build` exits successfully after preparing the project workspace, resolving the configured circuits release, and — when the project has a `methods/Cargo.toml` (Risc0 guest crate excluded from the main workspace) — also prints `Building guest methods...` and produces a `methods/target/.../release` artifact.
- `deploy` prints a submission summary with zero failures when built binaries are present.
- `wallet topup` succeeds without an explicit address because the project default wallet was seeded during setup.
- `wallet -- check-health` succeeds against the running localnet without requiring a global `wallet` install or manual `PATH` changes.
- Generated `scaffold.toml` stores `[wallet].home_dir` but does not carry a wallet binary override; wallet location is derived from the pinned LEZ checkout.

### Failure Signals / Common Pitfalls

- Running `setup`, `build`, `deploy`, or wallet commands outside the generated project root should fail with a project-scoped message.
- A foreign listener or stale state on the localnet port is a real dogfooding finding; capture `localnet status`, not just the final error.
- If `wallet topup` without an address says no destination is configured, record that as a regression in default-wallet seeding or persistence.
- If `setup` or wallet commands depend on `wallet` being installed globally or on `PATH`, record that as a regression in the self-contained project model.
- If `deploy` fails due to missing binaries after a successful `build`, capture the exact missing path.

### Evidence to Capture

- Scaffold creation output for both `new` and `create`.
- `setup`, `localnet start`, `build`, `deploy`, and wallet command excerpts.
- The generated project path and the exact binary path used for the run.
- `scaffold.toml` excerpt showing `[circuits]`, plus `ls .scaffold/circuits` or the configured install dir after `setup`/`build`.

### Execution Notes

- Use fresh directories per run. Do not reuse an old generated project unless the scenario explicitly targets upgrade or persistence behavior.
- Keep the alias check isolated so a failure in `create` does not contaminate the primary bootstrap project.

## D2. Default Template Operational Health: Localnet and Doctor

### Goal

Validate that localnet lifecycle commands and doctor diagnostics provide usable human and machine-readable state.

### Preconditions

- A default-template project exists.
- `setup` has already completed for that project.

### Commands / Actions

From the generated project root:

```bash
"$SCAFFOLD_BIN" localnet status
"$SCAFFOLD_BIN" localnet status --json
"$SCAFFOLD_BIN" doctor
"$SCAFFOLD_BIN" doctor --json
"$SCAFFOLD_BIN" localnet logs --tail 200
"$SCAFFOLD_BIN" localnet stop
"$SCAFFOLD_BIN" localnet status
```

If the scenario begins with localnet stopped, run `"$SCAFFOLD_BIN" localnet start` first and capture both the started and stopped states.

### Expected Success Signals

- Human-readable `localnet status` clearly reports tracked PID, listener state, ownership, and readiness.
- `localnet status --json` returns parseable JSON with at least `tracked_pid`, `listener_present`, `ownership`, and `ready`.
- `doctor` returns actionable next steps rather than only raw failures.
- `doctor` validates the configured circuits install path, checks the top-level `VERSION` file against `[circuits].version`, and warns when project config drifts from the LEZ pin's expected circuits release.
- `doctor --json` returns parseable JSON with at least `status`, `summary`, `checks`, and `next_steps`.
- `localnet logs --tail 200` returns useful recent log lines when logs exist.
- `localnet stop` succeeds cleanly and subsequent status reflects the stopped state.

### Failure Signals / Common Pitfalls

- Contradictions between tracked PID, listener ownership, and readiness are high-value findings.
- Empty or unhelpful logs after a failed startup are worth recording.
- If `doctor` omits next steps or machine-readable output becomes malformed, treat that as a DX regression.

### Evidence to Capture

- Human-readable and JSON output for both `localnet status` and `doctor`.
- If `[circuits]` is edited for the run, capture the matching `doctor` warning/error and the configured install dir.
- A short `localnet logs` excerpt.
- Stop behavior and the post-stop status output.

### Execution Notes

- Preserve raw JSON output exactly.
- If state is contradictory, do not silently restart localnet before capturing the failing state.

## D3. Default Template Deploy Variants and JSON Output

### Goal

Validate targeted deployment flows, including the machine-readable single-program submission path via `--program-path`.

### Preconditions

- Default-template project has already completed `build`.
- Localnet is reachable.
- Guest binaries exist under the generated project's `target/riscv-guest/.../release` directory.

### Commands / Actions

From the generated project root:

```bash
export EXAMPLE_PROGRAMS_BUILD_DIR="$PWD/target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release"
"$SCAFFOLD_BIN" deploy hello_world
"$SCAFFOLD_BIN" deploy --program-path "$EXAMPLE_PROGRAMS_BUILD_DIR/hello_world.bin"
"$SCAFFOLD_BIN" deploy --program-path "$EXAMPLE_PROGRAMS_BUILD_DIR/hello_world.bin" --json
"$SCAFFOLD_BIN" deploy nonexistent_program
```

Use a known default-template program name such as `hello_world`. If the generated project exposes a different set of programs in `methods/guest/src/bin`, record the discovered list.

`--json` only produces structured JSON output when combined with `--program-path`. On the discovery-based path (`deploy` or `deploy <name>`), the `--json` flag is accepted but silently ignored. This scenario validates that distinction.

### Expected Success Signals

- `deploy hello_world` reports `OK  hello_world submitted` and ends with a human-readable success summary.
- `deploy --program-path ... --json` prints a parseable JSON object with at least `status`, `program`, and `tx` fields.
- `deploy --program-path ...` without `--json` prints a human-readable `OK` line with the binary path.
- `deploy nonexistent_program` fails with an error listing the available discovered programs.

### Failure Signals / Common Pitfalls

- If `deploy hello_world --json` starts producing JSON output (instead of the normal human-readable summary), record that as a behavior change worth verifying.
- If localnet is unreachable, deploy should fail with a sequencer-unavailable hint instead of a vague wallet error.
- Unknown program names should report the available discovered programs.
- Missing binaries should point back to `logos-scaffold build`.

### Evidence to Capture

- One successful human-readable deploy excerpt from the discovery path.
- One successful JSON deploy output from the `--program-path` path.
- The error output for an unknown program name.
- Any failure-path excerpt for unreachable sequencer or missing binary when intentionally probed.

### Execution Notes

- Keep the `--program-path --json` examples separate from discovery-based deploys. Only `--program-path` produces JSON.
- When recording a custom `--program-path`, preserve the absolute path used in the run log.

## D4. Default Template Wallet Workflows and Passthrough

### Goal

Validate wallet-focused scaffold behavior beyond the basic bootstrap path.

### Preconditions

- Default-template project exists.
- Setup completed successfully.
- Localnet is running if you are validating non-dry-run topup or passthrough health checks.

### Commands / Actions

From the generated project root:

```bash
"$SCAFFOLD_BIN" wallet list
"$SCAFFOLD_BIN" wallet list --long
"$SCAFFOLD_BIN" wallet default set Public/<account-id>
"$SCAFFOLD_BIN" wallet topup --dry-run
"$SCAFFOLD_BIN" wallet topup
"$SCAFFOLD_BIN" wallet -- account list
"$SCAFFOLD_BIN" wallet -- check-health
```

Use a real address from `wallet list` when explicitly validating `wallet default set`.

### Expected Success Signals

- `wallet list` and `wallet list --long` proxy wallet account enumeration from the project-scoped wallet home using the LEZ-local wallet binary.
- `wallet default set` accepts either positional address or `--address` and persists the normalized project default.
- `wallet topup --dry-run` renders the underlying faucet claim command instead of mutating state.
- `wallet topup` without an explicit address uses the saved default wallet.
- `wallet -- ...` preserves the project wallet environment while forwarding the raw wallet command to `<lez>/target/release/wallet`.

Optional: validate `LOGOS_SCAFFOLD_WALLET_PASSWORD` override behavior by setting the env var to a non-default value and observing whether wallet commands honor it.

```bash
LOGOS_SCAFFOLD_WALLET_PASSWORD="custom-pw" "$SCAFFOLD_BIN" wallet topup --dry-run
```

### Failure Signals / Common Pitfalls

- Invalid addresses should be rejected with an "Accepted formats" hint.
- If both positional address and `--address` are supplied together, that is a user error and should remain clearly reported.
- Connectivity failures during topup should mention localnet/sequencer reachability rather than only raw wallet output.
- Passthrough flows require the literal `--`; if the CLI starts accepting or mangling passthrough without it, record that change.
- If wallet flows only succeed when `wallet` is separately installed on `PATH`, or if missing-binary errors point anywhere other than the LEZ-local `target/release/wallet`, record that as a regression.

### Evidence to Capture

- `wallet list` output with account identifiers redacted only if needed for sharing.
- `wallet topup --dry-run` output showing the rendered command.
- One successful passthrough example, ideally `wallet -- check-health` or `wallet -- account list`.
- If `LOGOS_SCAFFOLD_WALLET_PASSWORD` override was tested, the dry-run output showing the password was or was not forwarded.

### Execution Notes

- Do not let the shell consume the passthrough separator. Record the exact argv form you used.
- If you redact account IDs for public sharing, keep the unredacted originals in a local evidence log so repeated runs stay traceable.

## D5. Default Template Diagnostics Bundle

### Goal

Validate that scaffold support artifacts can be collected and inspected safely.

### Preconditions

- Default-template project exists.
- The project has enough state to make the report meaningful, ideally after setup and at least one localnet or build action.

### Commands / Actions

From the generated project root:

```bash
"$SCAFFOLD_BIN" report
"$SCAFFOLD_BIN" report --tail 200
"$SCAFFOLD_BIN" report --out "$PWD/artifacts/support-report.tar.gz"
```

Inspect the produced archive before sharing it:

```bash
find .scaffold/reports -maxdepth 1 -name '*.tar.gz' -print | sort
REPORT_ARCHIVE="$(find .scaffold/reports -maxdepth 1 -name '*.tar.gz' | sort | tail -n 1)"
tar -tzf "$REPORT_ARCHIVE" | sort
tar -tzf "$PWD/artifacts/support-report.tar.gz" | sort
```

### Expected Success Signals

- `report` prints a completion message, archive path, and a warning to inspect files before sharing.
- The default output lands under `.scaffold/reports/`.
- A custom `--out` path is honored.
- The archive contains support files such as `README.txt`, `manifest.json`, `diagnostics/doctor.json`, `diagnostics/localnet-status.json`, and `summaries/build-evidence.json`.

### Failure Signals / Common Pitfalls

- If raw wallet files under `.scaffold/wallet/` appear in the archive, treat that as a severe regression.
- If absolute local paths leak without scrubbing in human-facing report files, record it.
- If the archive is produced but the warning about manual inspection disappears, record it.

### Evidence to Capture

- Report completion output.
- Archive path(s).
- A short file listing from the tarball.

### Execution Notes

- Never attach the archive to an external system without first listing its contents.
- Keep the tar listing with the run evidence so redaction regressions can be compared across releases.

## D6. Default Template Example Runner Interaction

### Goal

Validate that deployed programs can actually be invoked via the generated example runner binaries and that account state changes are observable.

D1 validates the scaffold pipeline up to deploy and wallet health. This scenario validates the final step: running programs against the localnet and confirming observable state mutations.

### Preconditions

- Default-template project exists with D1 completed (setup, build, deploy done).
- Localnet is running and `wallet -- check-health` succeeds.
- Create a fresh public account for this scenario:

```bash
"$SCAFFOLD_BIN" wallet -- account new public
```

Capture the account ID from the output (format: `Public/<base58>`). Use the base58 portion as `<account-id>` below.

### Commands / Actions

From the generated project root:

```bash
export NSSA_WALLET_HOME_DIR="$PWD/.scaffold/wallet"
cargo run --bin run_hello_world -- <account-id>
"$SCAFFOLD_BIN" wallet -- account get --account-id <account-id>
cargo run --bin run_hello_world_with_move_function -- write-public <account-id> "dogfood-test-message"
"$SCAFFOLD_BIN" wallet -- account get --account-id <account-id>
```

The first runner (`run_hello_world`) submits a basic public transaction. The second (`run_hello_world_with_move_function write-public`) writes a custom greeting string to the account, producing an observable `data_b64` field change.

### Expected Success Signals

- Both runners print `submitted transaction: status=... tx_hash=...` on success.
- Both runners print a `verification hint:` line pointing to `wallet account get`.
- After `run_hello_world_with_move_function write-public`, `wallet account get` shows account data containing the encoded greeting string.
- Runner exit code is 0.

### Failure Signals / Common Pitfalls

- If a runner exits 0 but the account remains `Uninitialized`, the transaction may have been submitted without effect. Record both the runner output and the account state.
- Panic output from a runner (e.g., `unwrap()` on wallet/sequencer errors) instead of a structured error is worth recording.
- Invalid account ID format (not base58) should produce a clear parse error from the runner, not a panic.
- If localnet is down, runners should fail with a connection-refused error. Capture the exact error text.

### Evidence to Capture

- Runner output including `status` and `tx_hash` for at least one successful run.
- `wallet account get` output showing account state after interaction.
- The exact account ID used (for traceability across repeated runs).

### Execution Notes

- `NSSA_WALLET_HOME_DIR` must be set for runners that initialize `WalletCore::from_env()`. The scaffold wallet commands set this automatically, but direct `cargo run` does not.
- Use the fresh public account created in the preconditions rather than reusing accounts from other scenarios. This avoids confusion about pre-existing state.
- If additional runners are available (e.g., `run_hello_world_private`, `run_hello_world_through_tail_call`), exercising them is valuable but not required for this scenario.

## D7. `run` Pipeline and Post-Deploy Hooks

### Goal

Validate that `lgs run` collapses the build → IDL → localnet → topup → deploy chain into a single command, fires `[run].post_deploy` hooks with the documented environment, and that `--post-deploy` / `--no-post-deploy` flags override the configured hooks correctly.

### Preconditions

- A default-template project exists at `$SCRATCH_ROOT/dogfood-default` with `setup` already complete.
- No existing scaffold localnet running on the configured port (the scenario will start one). If one exists from a prior scenario, stop it first.
- `wallet topup` has worked at least once for this project (D1 or D4 covers this).

### Commands / Actions

From the project root, exercise the bare pipeline:

```bash
"$SCAFFOLD_BIN" run
```

Then add a `[run]` section to `scaffold.toml` and re-run with hooks:

```toml
[run]
post_deploy = [
  "echo 'sequencer:' $SEQUENCER_URL",
  "echo 'idl:' $SCAFFOLD_IDL_DIR",
  "echo 'project root:' $SCAFFOLD_PROJECT_ROOT",
  "echo 'wallet home:' $NSSA_WALLET_HOME_DIR",
  "echo 'program id:' ${SCAFFOLD_PROGRAM_ID:-unavailable}",
  "echo 'guest bin:' ${SCAFFOLD_GUEST_BIN:-unavailable}",
]
```

```bash
"$SCAFFOLD_BIN" run
"$SCAFFOLD_BIN" run --post-deploy "echo override"    # one-shot override
"$SCAFFOLD_BIN" run --no-post-deploy                 # skip hooks
"$SCAFFOLD_BIN" run --post-deploy "x" --no-post-deploy  # expect clap conflict error
```

### Expected Success Signals

- The first `run` (no hooks configured) prints a numbered step header for each phase (`[1/5] Building...` through `[5/5] Deploying...`) and ends with a deployed-programs summary.
- A second `run` reuses the running localnet (`localnet already running (sequencer pid=...)`) instead of starting a new sequencer.
- After adding the `[run]` block, `run` reports `[6/6] Running N post-deploy hook(s)` and each hook prints a non-empty value for its env var. `cwd` for each hook is the project root (verifiable with a `pwd` hook). For a single-program project, `$SCAFFOLD_PROGRAM_ID` is the deployed program's risc0 image ID and `$SCAFFOLD_GUEST_BIN` is the absolute path to the guest binary.
- `--post-deploy "echo override"` ignores `[run].post_deploy` and runs only the override.
- `--no-post-deploy` skips the post-deploy step entirely; the run prints the deployed-programs summary instead.
- `--post-deploy` with `--no-post-deploy` errors at clap parse time with a `cannot be used with` message; exit code is non-zero.
- A non-zero hook exit aborts the run with a clear `post-deploy hook exited with status N` message.

### Failure Signals / Common Pitfalls

- A `run` invocation that restarts the sequencer when one is already running healthy is a regression in the localnet-reuse path.
- Hooks running with `cwd` somewhere other than the project root, or missing any of `SEQUENCER_URL` / `NSSA_WALLET_HOME_DIR` / `SCAFFOLD_PROJECT_ROOT` / `SCAFFOLD_IDL_DIR`, is a regression in the env contract.
- `$SCAFFOLD_PROGRAM_ID` unset after a successful deploy on a single-program project with a vendored `spel` binary is a regression. Hint: `lgs setup` builds the spel binary; if it's missing, `program_id: unavailable` will also appear in the deploy summary.

### Evidence to Capture

- Console output of the first `run` showing the step headers and the deployed-programs summary.
- Output of `run` after the `[run]` block is added, showing the `===> post_deploy[i/n]:` markers and the resolved env values.
- Output of `run --post-deploy "echo override"` showing only the override hook fires.
- Output of `run --no-post-deploy` showing the deployed-programs summary instead of hooks.

## L1. LEZ Template Bootstrap

### Goal

Validate that the LEZ template scaffolds and reaches a ready-to-build state.

### Preconditions

- Latest scaffold binary has been built from the repo root.
- Scratch workspace exists.

### Commands / Actions

From the scratch workspace:

```bash
cd "$SCRATCH_ROOT"
"$SCAFFOLD_BIN" new dogfood-lez --template lez-framework
cd dogfood-lez
ls -d idl crates/lez-client-gen methods/guest/src/bin src/bin
"$SCAFFOLD_BIN" setup
"$SCAFFOLD_BIN" localnet start
"$SCAFFOLD_BIN" doctor
"$SCAFFOLD_BIN" build
```

The `ls` step verifies that LEZ-specific directories were scaffolded before proceeding with the build pipeline.

### Expected Success Signals

- Project creation succeeds with the LEZ template.
- The generated project contains `idl/`, `crates/lez-client-gen/`, `methods/guest/src/bin/lez_counter.rs`, and `src/bin/run_lez_counter.rs`.
- `setup`, `localnet start`, and `doctor` behave the same way they do for the default template.
- `build` succeeds for the LEZ project workspace and also runs IDL generation and client generation automatically.

### Failure Signals / Common Pitfalls

- If the generated project is missing LEZ-specific paths such as `idl/`, `crates/lez-client-gen/`, or `methods/guest/src/bin/lez_counter.rs`, record that immediately.
- If LEZ bootstrap behavior diverges from the default template in setup/localnet/doctor flows, capture the difference explicitly.
- If `build` does not automatically trigger IDL + client generation for the LEZ template, record that as a regression.

### Evidence to Capture

- LEZ project creation output.
- Directory listing showing LEZ-specific scaffolded paths.
- `setup`, `localnet start`, `doctor`, and `build` excerpts.

### Execution Notes

- Keep LEZ runs separate from default-template runs. The template-specific directories and follow-up commands are part of the validation.

## L2. LEZ IDL Regeneration

### Goal

Validate that LEZ projects can regenerate IDL from the current project source.

### Preconditions

- LEZ project exists.
- The LEZ project build environment is working.

### Commands / Actions

From the LEZ project root:

```bash
"$SCAFFOLD_BIN" build idl
find idl -maxdepth 1 -type f -name '*.json' | sort
```

### Expected Success Signals

- `build idl` writes one or more JSON files under `idl/`.
- Command output includes explicit `Wrote IDL ...` lines.
- The regenerated files are valid JSON and match the current program surface.

### Failure Signals / Common Pitfalls

- If the command prints that IDL build is being skipped due to framework kind, the scenario is running in the wrong project.
- Missing IDL marker output or empty IDL generation is a real regression for the LEZ template.

### Evidence to Capture

- `build idl` output.
- Listing of generated files under `idl/`.
- If relevant, a diff between pre-existing and regenerated IDL.

### Execution Notes

- Preserve the raw `Wrote IDL ...` lines. They make it much easier to diagnose partial-generation failures.

## L3. LEZ Client Generation

### Goal

Validate that LEZ client bindings can be regenerated from the current IDL set.

### Preconditions

- LEZ project exists.
- `build idl` has been run successfully, either directly or via `build client`.

### Commands / Actions

From the LEZ project root:

```bash
"$SCAFFOLD_BIN" build client
find src/generated -type f | sort
```

### Expected Success Signals

- `build client` reports that it is regenerating IDL before generating client code.
- Client artifacts are written under `src/generated`.
- The generated files reflect the current contents of `idl/`.

### Failure Signals / Common Pitfalls

- If `build client` does not refresh IDL first, record that behavior change.
- Missing `src/generated` output or missing generator crate paths are LEZ-specific regressions.

### Evidence to Capture

- `build client` output.
- Listing of files under `src/generated`.
- Any diff in generated client code when the scenario is rerun after a program change.

### Execution Notes

- Treat generated client output as part of the scenario evidence, not as disposable noise.
- When the generator fails, capture the exact manifest path and working directory that were used.

## L4. LEZ Template Deploy and Counter Interaction

### Goal

Validate that the LEZ counter program can be deployed and that the generated runner binary can invoke `init` and `increment` subcommands against the running localnet.

### Preconditions

- LEZ project exists with L1 completed (setup, build, localnet running).
- `wallet -- check-health` succeeds.
- At least one public account exists. If not:

```bash
"$SCAFFOLD_BIN" wallet -- account new public
```

### Commands / Actions

From the LEZ project root:

```bash
"$SCAFFOLD_BIN" deploy
export NSSA_WALLET_HOME_DIR="$PWD/.scaffold/wallet"
cargo run --bin run_lez_counter -- init --to <account-id>
cargo run --bin run_lez_counter -- increment --counter <account-id> --authority <account-id> --amount 5
```

### Expected Success Signals

- `deploy` submits the `lez_counter` program and prints a success summary.
- `run_lez_counter init` prints confirmation that the counter was initialized at the target account.
- `run_lez_counter increment` prints confirmation of the increment operation.

Note: as of this writing, the LEZ counter runner contains `TODO` placeholders for actual transaction submission. If the runner only prints diagnostic messages without submitting transactions, record that as the current state. When transaction submission is implemented, update this scenario with account-state verification steps matching D6.

### Failure Signals / Common Pitfalls

- If `deploy` cannot find `lez_counter` in the discovered program list, record the actual discovered list.
- If the runner panics on wallet initialization, `NSSA_WALLET_HOME_DIR` may not be set.
- If the runner accepts the subcommand but does nothing (due to TODO stubs), record the output and note the gap.

### Evidence to Capture

- `deploy` output for the LEZ project.
- `run_lez_counter init` and `increment` output.
- Whether the runner actually submitted transactions or only printed placeholder messages.

### Execution Notes

- `NSSA_WALLET_HOME_DIR` must be set for the runner. Scaffold wallet commands set this automatically, but direct `cargo run` does not.
- Keep LEZ interaction evidence separate from default-template interaction evidence.

## E1. CLI Discoverability and Error Quality

### Goal

Validate that the scaffold CLI provides consistent, non-destructive help and version output, useful error messages for unknown commands, and clear project-context errors when commands are run outside a generated project.

### Preconditions

- Latest scaffold binary has been built from the repo root.
- A scratch workspace exists (for verifying that help flags do not create files).

### Commands / Actions

From the repo root:

```bash
"$SCAFFOLD_BIN" --help
"$SCAFFOLD_BIN" --version
"$SCAFFOLD_BIN" help
"$SCAFFOLD_BIN" setup --help
"$SCAFFOLD_BIN" setup --wallet-install auto
"$SCAFFOLD_BIN" nonexistent-command
"$SCAFFOLD_BIN" build
"$SCAFFOLD_BIN" deploy
"$SCAFFOLD_BIN" doctor
"$SCAFFOLD_BIN" localnet status
"$SCAFFOLD_BIN" wallet list
```

From the scratch workspace (verify help flags do not mutate the filesystem):

```bash
cd "$SCRATCH_ROOT"
ls -la before_help_test > /dev/null 2>&1 || true
"$SCAFFOLD_BIN" create --help
"$SCAFFOLD_BIN" new --help
ls -la
```

Check that no new directories were created by the `--help` invocations.

### Expected Success Signals

- `--help` prints a usage summary listing all top-level commands.
- `--version` prints the version string and exits.
- `help` prints the same top-level usage summary as `--help` and exits successfully.
- `setup --help` documents the setup workflow without a `--wallet-install` flag.
- Legacy `setup --wallet-install auto` is rejected during argument parsing as an unknown argument.
- `nonexistent-command` fails with an error and directs the user to `--help` or an equivalent corrective hint.
- `build`, `deploy`, `doctor`, `localnet status`, and `wallet list` run from outside a project fail with a message like `Not a logos-scaffold project ... Run logos-scaffold create <name>`.
- `create --help` and `new --help` do not create directories or files in the current working directory.

### Failure Signals / Common Pitfalls

- If `create --help` or `new --help` creates a directory named `--help`, that is a significant UX regression. Record it and the exact argv used.
- If project-context errors are missing or unhelpful (e.g., a raw file-not-found instead of a scaffold-specific message), record the exact output.
- If some subcommands support `--help` and others do not, document the inconsistency.
- If `setup --help` still advertises `--wallet-install`, or the deprecated flag is silently accepted, record that as a command-surface regression.

### Evidence to Capture

- `--help` output.
- `--version` output.
- Error output for unknown command and out-of-project commands.
- Directory listing before and after `create --help` / `new --help` to confirm no side effects.

### Execution Notes

- Run the `create --help` test in an isolated temporary directory so any accidental file creation does not pollute the scratch workspace.
- Do not interpret missing `--help` support on a subcommand as a blocker. Record it as a finding and move on.

## E2. Project Creation with Advanced Flags

### Goal

Validate that `create`/`new` handle the `--template`, `--vendor-deps`, `--lez-path` (legacy alias: `--lssa-path`), and `--cache-root` flags correctly, including error cases for invalid inputs.

### Preconditions

- Latest scaffold binary has been built from the repo root.
- Scratch workspace exists and is writable.

### Commands / Actions

From the scratch workspace:

```bash
cd "$SCRATCH_ROOT"
"$SCAFFOLD_BIN" new dogfood-invalid-template --template nonexistent-template
"$SCAFFOLD_BIN" new dogfood-lez-explicit --template lez-framework
ls -d dogfood-lez-explicit/idl dogfood-lez-explicit/crates/lez-client-gen
"$SCAFFOLD_BIN" new dogfood-vendor --vendor-deps
"$SCAFFOLD_BIN" new dogfood-cache --cache-root "$SCRATCH_ROOT/custom-cache"
find "$SCRATCH_ROOT/custom-cache/repos/lez" -maxdepth 2 -mindepth 1 -type d | sort
grep -n "^\[wallet\]\|^home_dir\|^binary" dogfood-cache/scaffold.toml
```

### Expected Success Signals

- Invalid `--template` name fails with a clear error listing the available templates (`default`, `lez-framework`).
- `--template lez-framework` creates a project with LEZ-specific structure (same as L1).
- `--vendor-deps` is accepted without error and creates a project that vendors the pinned LEZ repo under `.scaffold/repos/lez`.
- `--cache-root` is honored and scaffold uses the specified directory for cache operations, with non-vendored LEZ clones isolated by pin under `<cache-root>/repos/lez/<pin>/`.
- Generated `scaffold.toml` includes `[wallet].home_dir` and does not include a deprecated `wallet.binary` field.

### Failure Signals / Common Pitfalls

- If an invalid template name silently falls back to `default`, record that as a regression.
- If `--vendor-deps` or `--cache-root` are silently ignored or produce an error, record the exact output.
- If `--lez-path` is tested and the path does not exist, verify the error message points to the bad path.
- If non-vendored cache reuse collapses different LEZ pins into a single shared `repos/lez` checkout, record that as a cache-isolation regression.

### Evidence to Capture

- Error output for invalid `--template`.
- Creation output for `--template lez-framework` with directory listing.
- Creation output for `--vendor-deps` and `--cache-root` if tested.
- Directory listing proving the pin-isolated cache path.
- `scaffold.toml` excerpt showing wallet home config without a wallet binary field.

### Execution Notes

- Clean up the generated projects after this scenario to avoid consuming disk space with multiple scaffolded projects.
- The `--lez-path` flag is optional to test here because it requires a real LEZ checkout. Only probe it if one is available.

## E3. AI Skills Materialized Into Every Project

### Goal

Validate that `lgs new` and `lgs init` both drop the canonical AI skill set
into a generated project so that Claude Code, Cursor, and Codex pick them up
without manual configuration. Skills are version-controlled in the generated
project (no `.gitignore` exclusion).

### Preconditions

- Latest scaffold binary built from the repo root (`"$SCAFFOLD_BIN"`).
- Scratch workspace exists.

### Commands / Actions

From the scratch workspace:

```bash
cd "$SCRATCH_ROOT"
"$SCAFFOLD_BIN" new dogfood-skills-default
"$SCAFFOLD_BIN" new dogfood-skills-lez --template lez-framework

mkdir dogfood-skills-init && cd dogfood-skills-init
"$SCAFFOLD_BIN" init
shasum AGENTS.md .claude/skills/lgs-cli/SKILL.md .cursor/rules/lgs-cli.mdc
"$SCAFFOLD_BIN" init   # re-init must succeed and not change skill content
shasum AGENTS.md .claude/skills/lgs-cli/SKILL.md .cursor/rules/lgs-cli.mdc
```

Inspect the generated layout in each of the three projects:

```bash
find dogfood-skills-default/.claude/skills dogfood-skills-default/.cursor/rules -type f | sort
find dogfood-skills-lez/.claude/skills dogfood-skills-lez/.cursor/rules -type f | sort
ls dogfood-skills-default/AGENTS.md dogfood-skills-lez/AGENTS.md dogfood-skills-init/AGENTS.md
```

### Expected Success Signals

- Every generated project (default template, lez-framework template, and `init`-adopted bare directory) contains exactly four `.claude/skills/<name>/SKILL.md` files: `lgs-cli`, `lez-template`, `lez-framework-template`, `basecamp`.
- The same four skills appear under `.cursor/rules/<name>.mdc`.
- `AGENTS.md` exists at every project root, lists all four skills with their descriptions, and links to `.claude/skills/<name>/SKILL.md`.
- Re-running `init` on an already-migrated project succeeds (no longer bails) and prints `AI skills refreshed under .claude/skills/, .cursor/rules/, AGENTS.md.` The `shasum` output before and after a re-init is byte-identical for all three skill files.
- `.claude/skills/<name>/SKILL.md` is byte-identical to the canonical source under `<scaffold-repo>/skills/<name>/SKILL.md` (run `diff` if validating against a built-from-source binary).
- `.cursor/rules/<name>.mdc` frontmatter contains `description:` and `alwaysApply: false`, and does **not** contain a `name:` field. The body after the closing `---` is identical to the SKILL.md body.
- The generated `.gitignore` does not exclude `.claude/`, `.cursor/`, or `AGENTS.md`.

### Failure Signals / Common Pitfalls

- A skill missing from one of the three locations in any generated project is a regression — every project gets the same four-skill set per the v0.1 contract.
- A `.cursor/rules/<name>.mdc` that still carries the `name:` line from the source SKILL.md is a regression in the frontmatter rewrite.
- A re-`init` that errors with "already at schema" is a stale build — that bail was removed when skill refresh became part of init's contract.
- A re-`init` that mutates skill content without a corresponding canonical-source change is a regression in idempotency.
- Skills appearing in `.gitignore` is a regression — they are version-controlled by design.
- Hand-edited team skills under `.claude/skills/<other>/` that get clobbered by `init` are a regression — `apply_skills` only owns the four shipped names.

### Evidence to Capture

- File listings under `.claude/skills/`, `.cursor/rules/`, and the existence of `AGENTS.md` for each of the three project flavors.
- One `.cursor/rules/<name>.mdc` head excerpt showing the rewritten frontmatter.
- `AGENTS.md` excerpt showing the four-row table.
- `shasum` pairs from the re-`init` idempotency check.

### Execution Notes

- This scenario does not require `setup`, `localnet`, or any network access — it validates only the materialization contract.
- Pair with E2 when validating template-related changes; pair with B1 when validating `init` behavior alongside basecamp adoption.

## B1. Basecamp Setup From a Module Project

### Goal

Validate that a module project can fetch the pinned basecamp + `lgpm` binaries, seed the default profiles, preserve configurable profile schema, and re-run `setup` idempotently.

### Preconditions

- Nix with flakes enabled.
- Latest scaffold binary built from the repo root (`"$SCAFFOLD_BIN"`).
- A module project on disk whose `flake.nix` exposes `packages.<system>.lgx` (see `"$SCAFFOLD_BIN" basecamp docs`). Reachable as `$MODULE_PROJECT`.
- `scaffold.toml` is present at the project root; if not, run `"$SCAFFOLD_BIN" init` once.

### Commands / Actions

From the module project root:

```bash
cd "$MODULE_PROJECT"
test -f scaffold.toml || "$SCAFFOLD_BIN" init
"$SCAFFOLD_BIN" basecamp --help
"$SCAFFOLD_BIN" basecamp docs | head
grep -n '^\[repos.basecamp.attr\]\|^\[basecamp.profiles' scaffold.toml || true
"$SCAFFOLD_BIN" basecamp setup
ls .scaffold/basecamp/profiles
"$SCAFFOLD_BIN" basecamp doctor
"$SCAFFOLD_BIN" basecamp doctor --json
"$SCAFFOLD_BIN" basecamp setup
```

### Expected Success Signals

- `basecamp --help` lists `setup`, `modules`, `install`, `launch`, `paths`, `build-portable`, `doctor`, and `docs`.
- `basecamp docs` prints the canonical project-compatibility rules, including per-profile `env_file`, `runtime_dir`, `log_file`, custom profile names, and per-platform `[repos.basecamp.attr]`.
- First `basecamp setup` clones the pinned basecamp repo into a pin-isolated cache path, builds `basecamp` and `lgpm` via Nix, seeds `.scaffold/basecamp/profiles/alice/` and `.scaffold/basecamp/profiles/bob/`, and reports completion.
- If `[repos.basecamp.attr]` is a per-platform map, setup uses the current host's attr and preserves the map plus scalar fallback on serialize.
- `basecamp doctor` reports the basecamp + lgpm binaries as present and both profiles as seeded; `--json` returns parseable JSON with the same checks.
- Second `basecamp setup` is idempotent: pin unchanged → no rebuild reported, exit 0.
- All commands run only inside the project; running them from outside the project must fail with the existing scaffold "not a logos-scaffold project" message.

### Failure Signals / Common Pitfalls

- Raw nix or `lgpm` stack traces with no scaffold-side hint are a UX regression — the setup-missing path is supposed to be a single one-line hint.
- A `setup` re-run that rebuilds when the pin has not changed is a regression in idempotency.
- Profile directories under `.scaffold/basecamp/profiles/` missing after first `setup` is a fail.
- If `basecamp` commands write to the user's global `~/.local/share/Logos/` or `~/Library/Application Support/Logos/`, that is a severe regression — basecamp state is project-local under `.scaffold/basecamp/`.
- If the basecamp binary lands on `PATH`, that is a contract violation.

### Evidence to Capture

- `basecamp --help` output.
- First and second `basecamp setup` output (to compare rebuild vs. no-rebuild).
- `basecamp doctor` and `basecamp doctor --json` output.
- Listing of `.scaffold/basecamp/profiles/`.
- Relevant `scaffold.toml` excerpt for `[repos.basecamp.attr]` and `[basecamp.profiles.*]` when present.

### Execution Notes

- Do not pollute the user's home; basecamp setup must stay under `<project>/.scaffold/basecamp/`. If something writes outside that root, stop and capture it before continuing.
- Pin-changed re-runs (rebuild path) are a separate validation; capture them when intentionally bumping the pin, not as part of this scenario.

## B2. Module Capture, Install, and Single-Instance Launch

### Goal

Validate the per-project source of truth for module identity (`[modules]` in `scaffold.toml`), the install pipeline that builds `.lgx` artefacts and loads them via `lgpm`, resolved profile paths, and a single-profile launch.

### Preconditions

- B1 completed in the same project.
- Module project's `flake.nix` (root or one or more sub-flakes) exposes `packages.<system>.lgx`. Sub-flake projects (e.g., `tictactoe-ui-cpp/`, `tictactoe-ui-qml/`) are valid.
- A graphical environment if you intend to actually drive the launched basecamp UI; `launch` itself does not require X/Wayland to start, but interactive validation does.

### Commands / Actions

From the module project root:

```bash
"$SCAFFOLD_BIN" basecamp modules
grep -n '^\[modules\.' scaffold.toml
"$SCAFFOLD_BIN" basecamp modules --show
"$SCAFFOLD_BIN" basecamp install
"$SCAFFOLD_BIN" basecamp install --print-output
"$SCAFFOLD_BIN" basecamp doctor
"$SCAFFOLD_BIN" basecamp paths alice
"$SCAFFOLD_BIN" basecamp paths alice --json
"$SCAFFOLD_BIN" basecamp launch alice
```

To validate custom profile schema, add one profile and inspect it before launch:

```toml
[basecamp.profiles.maker]
env_file = ".scaffold/basecamp/maker.env"
runtime_dir = "/tmp/lgs-maker"
log_file = ".scaffold/basecamp/profiles/maker/basecamp.log"

[basecamp.profiles.maker.env]
LOGOS_PROFILE_ROLE = "maker"
```

```bash
printf 'MAKER_ONLY=1\nLOGOS_PROFILE_ROLE=env-file\n' > .scaffold/basecamp/maker.env
"$SCAFFOLD_BIN" basecamp paths maker --json
"$SCAFFOLD_BIN" basecamp launch maker --log-file
```

If your project does not auto-discover correctly, capture explicit sources:

```bash
"$SCAFFOLD_BIN" basecamp modules --flake "./tictactoe#lgx" --flake "./tictactoe-ui-qml#lgx"
"$SCAFFOLD_BIN" basecamp modules --path /abs/path/to/prebuilt.lgx
```

### Expected Success Signals

- `basecamp modules` either auto-discovers project sub-flakes exposing `.#lgx` or accepts explicit `--path` / `--flake` sources and writes one `[modules.<name>]` sub-section per source into `scaffold.toml`. The file remains human-editable; re-runs are byte-identical and never overwrite existing keys.
- For each captured project source, scaffold also resolves declared `dependencies` and inserts `role = "dependency"` entries unless the dep is already keyed, is a basecamp preinstall (`capability_module`, `package_manager`, `package_manager_ui`, `counter`, `counter_qml`, `webview_app`, `basecamp_main_ui`; see `BASECAMP_PREINSTALLED_MODULES` in `src/constants.rs` for the authoritative list), or is resolvable via the source's own `flake.lock` / the scaffold-default table.
- An unresolvable dep fails fast with a targeted error naming the dep and the two user-side fixes (capture as a project source, or add `[modules.<name>]` with `role = "dependency"`); no silent drop.
- `basecamp modules --show` prints the captured set without mutating state.
- `basecamp install` builds each project source (sibling `--override-input` rewrites apply for `path:../<sibling>` inputs in multi-flake projects) and shells out to `lgpm` to install into both `alice` and `bob`. By default it logs to `.scaffold/logs/<ts>-install.log` and prints a one-line status; `--print-output` (or `LOGOS_SCAFFOLD_PRINT_OUTPUT=1`) streams nix output directly.
- `basecamp doctor` reports each profile's installed modules matching the captured set; drift between `[modules]` and on-disk profile state is flagged, not hidden.
- `basecamp paths <profile> --json` is pure path resolution: it emits parseable JSON for XDG config/data/cache, runtime dir, module/plugin dirs, launch state, log file, and env file without building or mutating anything.
- Custom profile names launch like default profiles when they are a single safe path component; `env_file` is sourced before global/profile inline env, `runtime_dir` is exported as both `TMPDIR` and `XDG_RUNTIME_DIR`, and `--log-file` overrides the configured `log_file`.
- `basecamp launch alice` kills any prior `logos_host` / `logos-basecamp` descendants for that profile, scrubs the profile's XDG dirs under `.scaffold/basecamp/profiles/alice/`, reinstalls each captured source for that profile, sets `XDG_{CONFIG,DATA,CACHE}_HOME` plus `LOGOS_PROFILE=alice`, and `exec`s basecamp.

### Failure Signals / Common Pitfalls

- A flake that exposes only `.#lgx-portable` and not `.#lgx` must fail explicitly with a hint pointing at `--flake <ref>#lgx-portable` for opt-in. Silent fallback is a contract violation.
- Re-running `basecamp modules` overwriting an existing key is a regression — manual edits in `scaffold.toml` must win.
- An unresolved transitive `logos-module-builder` input that fails without naming the missing `follows` is a regression.
- `install` succeeding when a build or `lgpm install` step actually failed is a fail; exit codes must be non-zero on any source failure.
- `launch alice` with an empty `[modules]` must bail (rather than scrubbing the profile and leaving it empty).
- Custom profile names that are empty, absolute, `.`, `..`, separator-containing, or contain control characters must be rejected before any filesystem work.
- An env file key containing control characters must fail before spawning basecamp.
- Sibling `--override-input` not being applied at probe time would surface as a build that resolves the wrong sibling pin during `basecamp modules` auto-discovery; record any such mismatch with the exact derived module names.

### Evidence to Capture

- `scaffold.toml` excerpt showing `[modules.<name>]` sub-sections with `flake`, `role`, and (for project sources) the in-project relative path used.
- `basecamp modules --show` output.
- `basecamp install` log path under `.scaffold/logs/` plus the printed one-line status, or the `--print-output` stream.
- `basecamp doctor` output post-install.
- `basecamp paths <profile> --json` output for both a default profile and one configured profile.
- The first lines of `basecamp launch alice` showing the kill → scrub → reinstall → exec sequence.
- For log checks, the log path and first lines proving stdout/stderr were tee'd to file and terminal.

### Execution Notes

- `basecamp modules` is the sole automated writer of `[modules]`. If the user manually edited an entry, do not re-run `basecamp modules` mid-scenario without recording the pre-edit state — manual entries are intentionally preserved.
- Only `path:../<sibling>` flake inputs are sibling-rewritten; `path:./sub`, `github:`, and `git+` schemes pass through. If a project uses multi-line input declarations, the line-level parser may not detect them — record any sibling-override miss along with the offending `flake.nix` excerpt.

## B3. Two-Instance P2P Dogfooding

### Goal

Validate the canonical basecamp use case: two profiles running simultaneously on one machine and exercising p2p features (chat, delivery, storage) of the project's `.lgx` modules.

### Preconditions

- B1 and B2 completed in the same project.
- `basecamp install` has captured at least one project source and produced a successful install for both `alice` and `bob`.
- A graphical environment for both basecamp windows.

### Commands / Actions

From two terminals, both rooted at the module project:

Terminal 1:

```bash
"$SCAFFOLD_BIN" basecamp launch alice
```

Terminal 2:

```bash
"$SCAFFOLD_BIN" basecamp launch bob
```

If the project defines custom profiles such as `maker` and `taker`, repeat the same two-terminal check with those names.

Within the running UIs, exercise whatever p2p surface the module exposes (chat exchange, delivery between peers, storage round-trip). Capture screenshots or short transcripts.

### Expected Success Signals

- Both basecamp windows open against their own profile dirs under `.scaffold/basecamp/profiles/{alice,bob}/`.
- Custom profile pairs open against their own profile dirs under `.scaffold/basecamp/profiles/<profile>/` and their configured runtime/log/env paths.
- Each window shows the project's `.lgx` modules installed and ready.
- `LOGOS_PROFILE=alice` and `LOGOS_PROFILE=bob` are visible in each respective process environment (helpful for debugging).
- The two instances do not collide on Qt remote-objects or any non-module port; per-profile port-override env vars (per the spec) are set on each `launch`.
- A p2p interaction triggered from `alice` is observable in `bob` (and vice versa) within the module's expected latency window.

### Failure Signals / Common Pitfalls

- Two windows opening but sharing identity keys, profile state, or message history is a clean-slate / XDG-isolation regression.
- A non-module port collision (Qt remote objects, etc.) is a real finding — file upstream against the affected component, do not patch around it inside scaffold.
- A module that does not honor an externally-provided port override is documented as a known gap pending an upstream fix on that module; capture the module name, the env var that should have worked, and the observed collision.
- One window crashing while the other survives is recordable evidence; capture the crashing instance's logs from `.scaffold/basecamp/profiles/<name>/` before relaunching.
- Running `basecamp launch alice` twice in parallel is undefined in v1 — record the behavior if you trip it accidentally, but don't treat it as a supported scenario.

### Evidence to Capture

- The exact two-terminal command sequence used.
- A short transcript or screenshot pair showing a p2p interaction propagating from one instance to the other.
- The env block of each running process (e.g., `tr '\0' '\n' < /proc/<pid>/environ | grep -E 'XDG_|LOGOS_'`).
- Any port-collision error text verbatim, with the module that owns the colliding port.
- If custom profiles are used, `basecamp paths <profile> --json` for each profile and the resolved log/runtime dirs.

### Execution Notes

- Do not start `alice` and `bob` from the same shell with `&` backgrounding unless you also redirect their logs; use two terminals for clean log separation.
- If the underlying module surface is not yet wired for p2p between profiles, record the gap and the module's TODO state rather than declaring B3 a pass.

## B4. Clean-Slate Verification

### Goal

Validate that `basecamp launch <profile>` scrubs profile state on every invocation and that profile/path safety guards bound all filesystem work to the project.

### Preconditions

- B2 completed (alice has captured modules and at least one successful install).

### Commands / Actions

From the module project root:

```bash
"$SCAFFOLD_BIN" basecamp launch alice    # let it come up, then close it
"$SCAFFOLD_BIN" basecamp paths alice --json
ls .scaffold/basecamp/profiles/alice
mkdir -p .scaffold/basecamp/profiles/alice/.scaffold-xdg-data/scratch
echo "marker-$(date -u +%s)" > .scaffold/basecamp/profiles/alice/.scaffold-xdg-data/scratch/marker.txt
"$SCAFFOLD_BIN" basecamp launch alice    # scrub-and-reinstall
test -e .scaffold/basecamp/profiles/alice/.scaffold-xdg-data/scratch/marker.txt && echo "REGRESSION: marker survived clean launch" || echo "OK: marker scrubbed"
"$SCAFFOLD_BIN" basecamp paths ../escape
```

### Expected Success Signals

- `launch alice` removes any user-introduced files under the alice profile XDG dirs and reinstalls each captured source before `exec`ing basecamp.
- `rm -rf` on `launch` is bounded to `<project>/.scaffold/basecamp/profiles/<profile>/`. Never any path outside that root.
- A `launch` that finds no modules in `[modules]` bails before scrubbing (the empty-install + scrubbed profile combination is the regression we're guarding against).
- `basecamp paths` rejects the same unsafe profile names as `launch` and remains non-mutating for valid profiles.

### Failure Signals / Common Pitfalls

- The `marker.txt` file surviving `launch alice` is a regression: clean-slate is the v1 contract.
- A `launch` scrubbing a path outside the profile's XDG dirs is a severe safety regression — capture the offending path and stop.
- An empty `[modules]` plus a `launch` that wipes the profile and leaves it empty is a real regression; the empty-modules bail must fire first.
- A custom `runtime_dir` on macOS that makes `<runtime_dir>/logos_token_<module>_<pid>` exceed the 104-byte Unix socket path budget is a dogfooding finding; keep custom values short, preferably under `/tmp`.

### Evidence to Capture

- The marker write and the post-launch listing showing it was scrubbed.
- The exact path under which the marker was placed and the path basecamp scrubbed (verify they match the profile root).
- Any unexpected paths touched by `launch` outside `.scaffold/basecamp/profiles/<profile>/`.
- The unsafe-profile rejection output from `basecamp paths ../escape`.

### Execution Notes

- Use a marker filename and timestamp you can search for after the fact; do not rely on visual inspection alone.
- Clean-slate state is project-local; never test scrub behavior against the user's global Logos directories.

## B5. Module Artefact Builds by Variant

### Goal

Validate that project sources captured under `[modules]` with `role = "project"` can be built against their `#lgx` and `#lgx-portable` flake outputs, that `--module` narrows the build, and that the old `build-portable` command remains a compatibility alias for the portable variant.

### Preconditions

- B2 completed (project sources are captured and `basecamp install` has succeeded against `.#lgx`).
- The same flakes expose `packages.<system>.lgx` and, for portable checks, `packages.<system>.lgx-portable`.

### Commands / Actions

From the module project root:

```bash
"$SCAFFOLD_BIN" basecamp build --variant all
"$SCAFFOLD_BIN" basecamp build --variant lgx --module <module-name>
"$SCAFFOLD_BIN" basecamp build --variant lgx-portable --module <module-name>
"$SCAFFOLD_BIN" basecamp build-portable
find .scaffold/basecamp -maxdepth 3 -type f -o -type l | sort
```

### Expected Success Signals

- `basecamp build --variant all` builds both `.#lgx` and `.#lgx-portable` for each `role = "project"` entry in dependency order, then writes/symlinks outputs under `.scaffold/basecamp/<variant-dir>/`.
- `--module <module-name>` builds only that captured project module and fails clearly for an unknown module.
- `build-portable` behaves like `basecamp build --variant lgx-portable` and keeps the historical `.scaffold/basecamp/portable/` output directory.
- `role = "dependency"` entries are skipped by build commands; dependencies are runtime inputs provided by install/basecamp.
- A flake that does not expose the requested variant fails with a targeted error naming the missing attribute, not a raw nix trace or silent fallback.

### Failure Signals / Common Pitfalls

- Any requested variant that silently falls back to another variant is a contract violation.
- An empty, duplicated, unknown, or path-like variant value through the Rust API must be rejected or normalized before filesystem work.
- Building dependency entries (those with `role = "dependency"`) is wasted work and a behavior regression.
- Out-of-order builds that ignore the dependency graph between project sources are a regression introduced by changes to ordering logic.

### Evidence to Capture

- `basecamp build` and `basecamp build-portable` output excerpts including the per-source build lines.
- The directory listing of the produced artefacts under `.scaffold/`.
- For any failure, the exact missing flake attribute and the offending project source.

### Execution Notes

- This scenario does not exercise the AppImage itself. Hand-loading into a basecamp AppImage is owned by the AppImage release, not by scaffold.

## B6. Captured Module Run Loop

### Goal

Validate that `basecamp run` launches a captured module from its flake for the local development loop, and that host selection is predictable.

### Preconditions

- B2 completed and `[modules.<name>]` contains at least one `role = "project"` module captured from a flake, not from a prebuilt `.lgx` file.
- For standalone UI checks, the module flake exposes `apps.<system>.default` or the attr named by `[modules.<name>].standalone_app`.

### Commands / Actions

From the module project root:

```bash
"$SCAFFOLD_BIN" basecamp run <module-name> --host standalone
"$SCAFFOLD_BIN" basecamp run <module-name>
```

For one negative-path check, capture or hand-edit a module entry that points at a prebuilt `.lgx` path and run:

```bash
"$SCAFFOLD_BIN" basecamp run <path-captured-module>
```

### Expected Success Signals

- `--host standalone` invokes `nix run` for the module flake's default app, or `#<standalone_app>` when that config key is set.
- With no `--host`, the run defaults to `standalone` (the only host today).
- A module captured as a prebuilt `.lgx` path is rejected with guidance to edit/remove the entry and capture a flake source; `nix run` is not attempted.
- Running a module as a configured Basecamp peer (one-shot build + install + launch) is not yet available; use `basecamp install` then `basecamp launch <profile>`. Tracked as follow-up work.

### Failure Signals / Common Pitfalls

- Running a `.lgx` path source through `nix run` is a regression; path captures are installable artefacts, not flake apps.
- A remote flake ref without an explicit fragment must still receive the requested app/build attr when scaffold constructs the Nix command.
- An omitted `standalone_app` must not serialize back as `standalone_app = ""`.

### Evidence to Capture

- Command output for standalone/default-host/basecamp-host paths.
- The `[modules.<name>]` excerpt showing `flake`, `role`, optional `standalone_app`, and whether the source is a flake or `.lgx` path.
- Any rejected `.lgx` path-source error verbatim.

## A1. Public Rust API Surface

### Goal

Validate that the public `logos_scaffold::api` library surface exists, is documented, and lets a Rust consumer drive a scaffold project (open by explicit root, inspect paths, read localnet status, categorized errors) without shelling out to the CLI. The API is the library boundary the `test-node` integration features build on.

### Preconditions

- Latest scaffold checkout at the repo root.
- A generated default-template project exists at `$SCRATCH_ROOT/dogfood-default` (D1).

### Commands / Actions

From the repo root, confirm the surface builds and documents cleanly:

```bash
cargo doc --no-deps
cargo test --doc
```

Then exercise the API from a throwaway consumer. Either add a dev-dependency on the local crate from a scratch crate, or drive it through a one-off integration test inside the repo:

```rust
use logos_scaffold::api::{LocalnetStartOptions, Project};

fn main() -> logos_scaffold::api::Result<()> {
    // Explicit root — no cwd discovery.
    let project = Project::open("/abs/path/to/dogfood-default")?;
    println!("rpc = {}", project.localnet_rpc_url());

    let paths = project.paths()?;
    println!("sequencer = {}", paths.sequencer_binary.display());
    println!("cache_root = {} (from {})", paths.cache_root.display(), paths.cache_root_source);

    // Typed status — same model as `localnet status --json`.
    let status = project.localnet_status();
    println!("ready = {}", status.ready);

    // Opening a non-project directory yields a categorized Config error.
    if let Err(err) = Project::open("/tmp") {
        println!("expected config error: {err}");
    }
    Ok(())
}
```

### Expected Success Signals

- `cargo doc --no-deps` builds rustdoc for the `api` module, and `cargo test --doc` passes the `api` doctests (setup / localnet lifecycle / wallet topup / deploy / doctor / report / test-node examples).
- `Project::open(root)` loads a project from an explicit root with no dependency on the process working directory; `Project::discover(dir)` walks upward to find `scaffold.toml`.
- `Project::paths()` reports the resolved cache root (and which layer supplied it), pinned repo checkouts, vendored binary paths, wallet home, localnet state/log, and circuits dir — whether or not they exist yet.
- `Project::localnet_status()` returns the same typed model the CLI prints under `localnet status --json`.
- Errors are categorized (`api::Error::{Config, MissingTool, RepoState, Process, Timeout, Transport, Command, Other}`); a missing/unreadable `scaffold.toml` is a `Config` error, and external-command failures carry a structured `CommandFailed` (rendered command, exit code, captured output).

### Failure Signals / Common Pitfalls

- A doctest failure in the `api` module means a documented example drifted from the surface — fix the example or the doc, do not delete the test.
- If `Project::open` on a directory without `scaffold.toml` returns an uncategorized error (not `Error::Config`), that is a regression in the error contract.
- If an operation silently depends on the process cwd instead of the project root passed in, record it — explicit-root targeting is the core API guarantee.

### Evidence to Capture

- `cargo doc` / `cargo test --doc` output lines for the `api` module.
- The consumer program's output showing the resolved paths, status, and the categorized error.

### Execution Notes

- This scenario validates the library boundary only; it does not require a running localnet. Pair it with T1–T3 when validating the `test-node` API.

## T1. Isolated Test-Node Lifecycle and Caller-Project Pins

### Goal

Validate that `test-node` spins up isolated, short-lived sequencer instances (own port, config, database, logs, runtime dir) for integration tests, that the prerequisite/pin commands resolve the caller project's LEZ and circuits pins, and that lifecycle commands (`start`/`status`/`stop`/`run`) are clean and machine-readable.

### Preconditions

- The real sequencer toolchain is provisioned (r0vm + circuits + built `sequencer_service`) per "Provisioning the Real LEZ Sequencer Toolchain". `test-node prepare` builds/fetches the sequencer and circuits on demand; `setup` is not required for T1–T3 (it is only needed for T4's wallet).
- No requirement that the developer `localnet` is running — test nodes are independent of it.
- **Run this against a real node.** On a provisioned box, `test-node doctor --json` returns `"ok": true` with every check `pass`, `test-node prepare` ends with `test-node prerequisites ready`, and `test-node start` yields a real `pid`/`rpc_url` whose `block_id` rises within seconds as the sequencer produces clock blocks. Do not substitute a stub for the node here.

### Commands / Actions

From the generated project root:

```bash
"$SCAFFOLD_BIN" test-node pins
"$SCAFFOLD_BIN" test-node pins --json
"$SCAFFOLD_BIN" test-node doctor
"$SCAFFOLD_BIN" test-node prepare --json
"$SCAFFOLD_BIN" test-node start --json
# capture the node id (state_dir basename) and rpc_url from the JSON
"$SCAFFOLD_BIN" test-node status --node <node-id> --json
"$SCAFFOLD_BIN" test-node stop --node <node-id>
"$SCAFFOLD_BIN" test-node run --serial --block-create-timeout-ms 500 --retry-pending-blocks-timeout-ms 500 -- sh -c 'echo "rpc=$LGS_TEST_NODE_RPC_URL port=$LGS_TEST_NODE_PORT"'
```

The pin/prepare/doctor commands also accept `--project <root>` so they can be driven from outside the project directory.

### Expected Success Signals

- `test-node pins` reports the LEZ source/ref, resolved commit, checkout path and ownership (`managed_cache` vs `caller_provided`), sequencer binary path, and circuits version/path — each annotated with its origin (`cli_override` -> `project_config` -> `scaffold_default`). For projects with `[circuits]`, the reported circuits version matches the project config; startup should still materialize the configured circuits install before launching the node.
- `test-node doctor` reports pin drift, checkout presence/commit/cleanliness, sequencer binary, circuits release, and platform support as separate categorized checks; exits non-zero only when a real prerequisite is missing.
- `test-node prepare` resolves the project's pins, ensures the checkout + circuits, builds the standalone sequencer, and (with `--json`) reports the checkout, resolved commit, binary path, and circuits path.
- `test-node start --json` prints at least `rpc_url`, `pid`, `state_dir`, `config_path`, `log_path`, `genesis_block_id`, and current `block_height`; the node runs on its own port under `.scaffold/test-nodes/<id>/` and does not touch the vendored LEZ checkout or the developer localnet.
- `test-node start --port 0` (the default) selects an unused localhost port; `test-node status --node <id> --json` reports `healthy` and the served `rpc_url`, exiting non-zero when unhealthy.
- `test-node start` / `test-node run` with `--block-create-timeout-ms` and `--retry-pending-blocks-timeout-ms` patch those sequencer config values as millisecond strings (for example, `500ms` for faster local tests) in the runtime `sequencer_config.json`; accepted values are 1 to 3,600,000 ms, and omitting them preserves the pinned debug config values. Values near or below the stable-read sample cadence can keep `clock wait-stable` and account-boundary reads from converging.
- `test-node stop --node <id>` terminates only that node and removes its runtime state (unless `--preserve-work-dir`).
- `test-node run -- <cmd>` starts a node, waits for health, exports `LGS_TEST_NODE_RPC_URL` / `LGS_TEST_NODE_PORT` / `LGS_TEST_NODE_STATE_DIR` (and friends) to the child, forwards the child's exit status, and stops the node afterward; `--serial` caps concurrent node creation at one and `--parallel N` at N.

### Failure Signals / Common Pitfalls

- A node that reuses the developer `localnet` port, writes into the vendored LEZ checkout, or otherwise is not isolated is a contract violation.
- `test-node start` with an explicit `--port` already in use must fail fast with a port-conflict error, not hang.
- If a caller-provided LEZ checkout (`[repos.lez].path`, or a local dir via `--lez-source`) is reset/force-checked-out by `prepare`, that is a severe regression — caller checkouts are validated, never mutated.
- A `run` that leaks the node (does not stop it) after the child exits, or does not forward the child's non-zero exit, is a regression.
- Missing sequencer binary or circuits must produce a clear scaffold-side error pointing at `test-node prepare`, not a raw panic.

### Evidence to Capture

- `test-node pins --json` and `test-node doctor` output.
- The circuits version from `test-node pins --json`, and the configured install dir after `test-node start` or another command that materializes project circuits.
- `test-node start --json` output (the full connection record) and the `.scaffold/test-nodes/<id>/` listing.
- `test-node status --json` for the running and stopped states.
- `test-node run` output showing the exported `LGS_TEST_NODE_*` env reaching the child.

### Execution Notes

- Test nodes are designed to be ephemeral; prefer fresh nodes per scenario and rely on `stop` (or handle `Drop` in the API) for teardown. Use `--preserve-work-dir` only when you need to inspect a node's database/logs after the fact.
- Keep test-node runs independent of the `localnet` scenarios (D1/D2): they are separate sequencer instances by design.

## T2. Test-Node Typed RPC Reads: Transactions, Blocks, Clock, Accounts, Proofs

### Goal

Validate that the `test-node` RPC subcommands give integration tests definitive, structured observations against a running node — terminal transaction outcomes, block/clock context for replay, and stable account/proof reads for parity assertions — instead of hand-rolled JSON-RPC scraping.

### Preconditions

- A test node is running (T1): capture its `rpc_url` (e.g., `export TN_URL=http://127.0.0.1:<port>`).
- A transaction file is available for the `tx` checks. For a quick negative-path check, any base64 borsh blob works; for a committed-path check, use a transaction produced by an example runner or wallet against the same node.

### Commands / Actions

Against the running node URL:

```bash
# Blocks and clock (no transaction needed)
"$SCAFFOLD_BIN" test-node blocks head --url "$TN_URL" --json
"$SCAFFOLD_BIN" test-node blocks range --url "$TN_URL" --from 1 --to 3 --json
"$SCAFFOLD_BIN" test-node blocks wait --url "$TN_URL" --after 1 --count 1 --json
"$SCAFFOLD_BIN" test-node clock read --url "$TN_URL" --json
"$SCAFFOLD_BIN" test-node clock wait-stable --url "$TN_URL" --samples 2 --json

# Transactions
"$SCAFFOLD_BIN" test-node tx submit-and-wait --url "$TN_URL" --file ./tx.b64 --encoding borsh-base64 --json
"$SCAFFOLD_BIN" test-node tx submit --url "$TN_URL" --file ./tx.b64 --json
"$SCAFFOLD_BIN" test-node tx wait --url "$TN_URL" --hash <tx-hash> --json

# Accounts and proofs (parity assertions)
"$SCAFFOLD_BIN" test-node account get --url "$TN_URL" --account-id <id> --json
"$SCAFFOLD_BIN" test-node account batch-get --url "$TN_URL" --account-id <id-a> --account-id <id-b> --json
"$SCAFFOLD_BIN" test-node proof get --url "$TN_URL" --commitment <hex-or-base58> --json
"$SCAFFOLD_BIN" test-node snapshot accounts --url "$TN_URL" --account-id <id> --output ./accounts-snapshot.json --json
```

### Expected Success Signals

- `tx submit-and-wait --json` emits exactly one terminal outcome object: `committed` (with the actual sequencer `block_id` and `timestamp`), `rejected` (`phase` = `stateless` | `stateful`, with `reason` or `observed_after_block_id`), `timeout` (`last_observed_block_id`), `transport_error`, or `wire_mismatch`; it exits non-zero for anything but `committed`. Transport failures are never reported as business rejections, and a stateful rejection follows an explicit multi-block observation rule (not a single sleep).
- `tx submit` returns the node-assigned tx hash or a structured stateless rejection; `tx wait` observes a previously submitted hash, honoring `--after-block` when supplied.
- `blocks head` / `blocks range` report each block's id and timestamp and classify it explicitly: genesis (the only zero-transaction block — no clock tick to replay), clock-only (empty post-genesis blocks still advance clock state via the mandatory clock transaction), and blocks carrying user transactions, with per-tx hashes for public/deployment transactions.
- `blocks wait` returns the requested number of blocks after the boundary.
- `clock read` returns all three `/LEZ/ClockProgramAccount/...` accounts with decoded `block_id`/`timestamp`; `clock wait-stable` returns a snapshot only after consecutive identical samples (head + clock state), or a retryable timeout error.
- `account get` distinguishes `present` (with lossless base64 account bytes plus decoded balance/nonce/owner/data), `missing` (never written), and `decode_error`; every read reports the `block_id` it was scoped to. `batch-get` reads all accounts at one consistent block boundary.
- `proof get` distinguishes a missing commitment (`proof: null`), an invalid commitment (local error before any RPC), and transport failures.
- `snapshot accounts` writes a block-consistent JSON snapshot to the `--output` path.

**Against a real node specifically** (this is the highest-value check — the client's hand-rolled borsh parsers must match genuine sequencer output, not just the unit-test fixtures): a freshly started node produces clock-only blocks where `blocks head --json` shows `transaction_count: 1`, the single tx has `is_clock: true` and `kind: "public"`, and `fully_parsed: true`; `clock read --json` decodes real `ClockAccountData` where the `/0000001` account's `block_id` tracks the head while `/0000010` and `/0000050` lag at their slower cadence; `account get` on a seeded account returns the exact seeded balance (see T3). Any mismatch here is a wire-format regression that the in-process stub would not catch.

### Failure Signals / Common Pitfalls

- A `tx submit-and-wait` that collapses transport errors or timeouts into "rejected" (or that exits zero for a non-committed outcome) is a regression in the divergence-detection contract.
- A block classification that marks genesis as having a clock transaction, or hides empty post-genesis (clock-only) blocks, breaks clock-sensitive replay.
- An account read that does not report its block boundary, or that races a clock block and returns inconsistent data instead of a stable result / structured retryable error, is a parity regression.
- A `proof get` that conflates "missing commitment" with "invalid commitment" or with a transport failure is a regression.

### Evidence to Capture

- One `tx submit-and-wait --json` committed object and (if probed) one non-committed outcome.
- `blocks range --json` output showing the genesis / clock-only / user-tx classification.
- `clock read --json` and a `clock wait-stable` result.
- `account get --json` for present and missing accounts, and a `proof get --json` for present and missing commitments.

### Execution Notes

- The RPC-scoped subcommands target a node URL, not a project directory, so they can run anywhere once `$TN_URL` is known.
- These are the typed equivalents of the `logos_scaffold::api::testnode::TestNodeClient` methods; when validating an API change, exercise both the CLI and the client.

## T3. Test-Node Caller-Provided State Seeding

### Goal

Validate that a test node can start from a caller-provided state snapshot — not only from empty localnet state — with validation up front and exact-state startup (no implicit wallets or default testnet accounts).

### Preconditions

- T1 prerequisites met (sequencer binary + circuits available for the project).
- A running node (T1) if you intend to `state export` from it; otherwise a hand-written snapshot file suffices.

### Commands / Actions

From the generated project root:

```bash
"$SCAFFOLD_BIN" test-node state schema --json

# Author a minimal snapshot (public account with a balance), then seed from it:
cat > ./seed.json <<'JSON'
{ "format": "lgs-state-snapshot/1",
  "public_accounts": [ { "account_id": "<base58-account-id>", "balance": 5000 } ],
  "private_accounts": [] }
JSON
"$SCAFFOLD_BIN" test-node state seed --input ./seed.json --output ./seeded-state --json
"$SCAFFOLD_BIN" test-node start --state ./seeded-state --json

# Optionally export public balances from a running node into a snapshot:
"$SCAFFOLD_BIN" test-node state export --url "$TN_URL" --account-id <id> --output ./exported.json --json

# Negative path: an unsupported format must be rejected with a categorized error.
echo '{"format":"bogus/1"}' > ./bad.json
"$SCAFFOLD_BIN" test-node state seed --input ./bad.json   # expect format-mismatch error, non-zero exit
```

### Expected Success Signals

- `state schema` identifies the exact snapshot formats the project's pins accept (`lgs-state-snapshot/1`, the `lgs-account-snapshot/1` output of `snapshot accounts`, or a rocksdb state directory), the state format version (`nssa-v03`), the LEZ ref/commit, and the seedable account fields.
- `state seed` validates the snapshot before producing a state directory and reports the seed kind (`config` vs `database`), the LEZ commit, the state format version, and the public/private account counts. Validation errors are distinguished: format mismatch, storage-schema mismatch (e.g. public-account data, which the genesis config cannot seed), LEZ pin mismatch, and account decode errors.
- `test-node start --state ./seeded-state` starts from exactly the snapshot's accounts — the sequencer builds genesis state from `initial_public_accounts` / `initial_private_accounts` with no implicit wallets, sample programs, or default testnet accounts. A database-seeded directory (a node's preserved `rocksdb/`) resumes from it verbatim.
- `state export` writes named public-account balances from a running node into an `lgs-state-snapshot/1` file (the pinned RPC exposes public balances only; full-fidelity state comes from a stopped node's database directory, which the command output notes).
- **Confirmed against a real node:** seed an account at a distinctive balance (e.g. 4242), `start --state`, then `test-node account get --account-id <id>` returns `state: present` with `balance: 4242` and the account exists at genesis — proving the sequencer built genesis from exactly the snapshot (no testnet defaults). This is the end-to-end proof that exact-state seeding works, not just that the file validated.

### Failure Signals / Common Pitfalls

- A node started with `--state` that injects extra accounts (default testnet wallets, sample programs) beyond the snapshot is a regression — seeded startup must be exact.
- A snapshot needing unsupported state (e.g. public-account data/nonce via the genesis config) that fails late inside the node instead of up front during `state seed` validation is a regression.
- A pin mismatch (snapshot's `lez_commit` differs from the project's resolved pin) that is silently accepted is a regression.
- An unknown snapshot format accepted instead of rejected with a `format mismatch` error is a regression.

### Evidence to Capture

- `state schema --json` output.
- `state seed --json` output showing the seed kind, account counts, and (for the negative path) the categorized error.
- `test-node start --state ... --json` output and a `test-node account get` against a seeded account confirming the exact seeded balance.

### Execution Notes

- Pair this scenario with T2: after seeding and starting, use `account get` / `account batch-get` to assert the seeded accounts match the snapshot exactly.
- Database seeding (`--state <dir with rocksdb/>`) is the full-fidelity path; the JSON snapshot path is balance-and-commitment level by design at the pinned revision.

## T4. Real Committed User Transaction (Test-Node + Wallet)

### Goal

The capstone end-to-end proof: a real wallet transaction, executed by a real test-node sequencer, observed as committed through the test-node client. This ties the `test-node` feature to the rest of the stack and validates the transaction-bearing block path against genuine sequencer output — the one thing a stub cannot prove.

### Preconditions

- Full toolchain provisioned **including the wallet**: r0vm + circuits + real `sequencer_service` (T1) **and** `setup` complete so the LEZ-local `wallet` is built and the default wallet is seeded (see "Provisioning the Real LEZ Sequencer Toolchain", step 4).
- The wallet targets the `sequencer_addr` in the wallet config (`$NSSA_WALLET_HOME_DIR/wallet_config.json`) when set; otherwise it defaults to `http://127.0.0.1:<localnet.port>` (default 3040). Start the test-node on that port (or update `sequencer_addr`) so the wallet talks to it.

### Commands / Actions

From the project root:

```bash
SJ=$("$SCAFFOLD_BIN" test-node start --project "$P" --port 3040 --json)
URL=$(echo "$SJ" | jq -r .rpc_url); NODE=$(echo "$SJ" | jq -r .state_dir | xargs basename)
HEAD0=$("$SCAFFOLD_BIN" test-node blocks head --url "$URL" --json | jq -r .block_id)

# Submit a REAL faucet transaction against the test-node:
"$SCAFFOLD_BIN" wallet topup        # account-get → (auth-transfer init if uninitialized) → pinata claim

# Observe the committed USER transaction(s) through the test-node client:
"$SCAFFOLD_BIN" test-node blocks wait --url "$URL" --after "$HEAD0" --count 6 --timeout-sec 90 --json \
  | jq -c '.blocks[] | select(.has_user_transactions) | {block_id, transaction_count, user:[.transactions[]|select(.is_clock|not)|{hash:.hash[0:12],kind}]}'

"$SCAFFOLD_BIN" test-node stop --node "$NODE" --project "$P"
```

### Expected Success Signals

- `wallet topup` exits 0 against the test-node: the real sequencer executes the `auth-transfer init` and `pinata claim` transactions (via r0vm in dev mode).
- `blocks wait` surfaces one or more blocks with `has_user_transactions: true`, each with `transaction_count: 2` (the mandatory clock tx plus the user tx) and a non-clock `kind: "public"` user tx carrying a real sha256 hash — parsed by the same client/borsh path the unit tests exercise, now against genuine block bytes.
- Equivalently, a wallet-reported tx hash (when one is printed) resolves to `committed` via `test-node tx wait --url "$URL" --hash <hash> --json`.

### Failure Signals / Common Pitfalls

- `wallet topup` failing with a sequencer-unreachable hint means the test-node is not on the wallet's expected port — start it with `--port <localnet.port>`.
- A block parser that mis-classifies the user-tx block as clock-only, or reports `fully_parsed: false` on a plain public/deploy tx, against real bytes is a wire-format regression — the highest-value signal this scenario protects.
- A real sequencer that boots but never executes the tx (block count rises with clock-only blocks but no user tx ever lands) usually means r0vm is missing or version-mismatched — recheck the exact version match in provisioning step 2.

### Evidence to Capture

- `wallet topup` output (the account-get → init → claim sequence) and its exit code.
- The `blocks wait` JSON showing the real committed user transaction(s).
- The node id and confirmation it was stopped and its runtime dir cleaned up.

### Execution Notes

- This is the only scenario that requires both the sequencer and the wallet built; it is the capstone real-e2e check. The block parsing it exercises is shared with T2, so a green T4 is strong evidence the entire `blocks` / `tx` client surface is correct against real output.
- A previous agent run validated this exact flow: `wallet topup` produced `auth-transfer init` and `pinata claim` txs that landed in real blocks (`transaction_count: 2`, a `public` user tx distinct from the clock tx), observed through `blocks wait`. Reproduce it; do not downgrade to a stub.

## Minimum Rerun Guidance for Future Changes

- Changes to onboarding, project creation, setup, localnet, or build flows: rerun `D1`, `D2`, and `D6`.
- Changes to deploy behavior or deploy output formatting: rerun `D3` and `D6`.
- Changes to wallet flows or wallet-related defaults: rerun `D4`.
- Changes to diagnostics, report contents, or redaction logic: rerun `D5`.
- Changes to example runner binaries or template `src/bin/*` code: rerun `D6`.
- Changes to `run` step ordering, post-deploy env vars, post-deploy CLI override flag handling, or `[run]` config parsing: rerun `D7`.
- Changes to LEZ template scaffolding or generated outputs: rerun `L1`, `L2`, `L3`, and `L4`.
- Changes to CLI argument parsing, help text, or error messages: rerun `E1`.
- Changes to `create`/`new` flags or template selection logic: rerun `E2`.
- Changes to AI skill materialization (`apply_skills`, the canonical `skills/` source, frontmatter rewrite, `AGENTS.md` template, or `init` re-run semantics): rerun `E3`.
- Changes to `basecamp setup` (pin sync, lgpm build, profile seeding, idempotency), per-platform `[repos.basecamp.attr]`, or `basecamp doctor`: rerun `B1`.
- Changes to `[modules]` derivation, dependency resolution, sibling `--override-input` handling, or `basecamp install` invocation of `lgpm`: rerun `B2`.
- Changes to `basecamp paths`, `[basecamp.profiles.*]`, `env_file`, `runtime_dir`, `log_file`, `launch --log-file`, or single-profile launch path resolution: rerun `B2`.
- Changes to `basecamp launch` (kill-and-scrub semantics, XDG isolation, runtime/log/env export, port-override env vars, p2p surface): rerun `B3`.
- Changes to clean-slate scrub semantics, profile-name validation, path-root bounds, or the empty `[modules]` guard on `launch`: rerun `B4`.
- Changes to `basecamp build`, `basecamp build-portable`, variant normalization, `--module` filtering, or build attr selection: rerun `B5`.
- Changes to `basecamp run`, `standalone_app`, or module source validation for run: rerun `B6`.
- Changes to the public `logos_scaffold::api` surface (entry points, typed result models, categorized errors, `CommandFailed`, or the documented examples/doctests): rerun `A1`, and rerun the matching CLI scenario for any command whose `*_for_project` core changed.
- Changes to `test-node` lifecycle, pin resolution, prepare/doctor, run-slot concurrency, or caller-checkout validation: rerun `T1` (and `A1` if the `api::testnode` lifecycle types changed).
- Changes to the `test-node` RPC client (transaction outcomes, block/clock parsing, account/proof reads, or their JSON shapes): rerun `T2`.
- Changes to `test-node` state seeding (snapshot formats, validation classes, genesis-config injection, or database seeding): rerun `T3`.
- Changes to the transaction-bearing block path (`sendTransaction`/`getBlock` handling, the committed-block scan, user-vs-clock classification, or the r0vm/sequencer spawn env): rerun `T4` (the only check that proves the path against a real executed transaction).
- Changes to `[circuits]` config parsing/serialization, circuits install-dir resolution, circuits materialization/export, or `doctor` circuits checks: rerun `D1`, `D2`, `D6`, `T1`, `T4`, and `A1`.
- Changes to circuits/r0vm provisioning, the LEZ/circuits pins, or the sequencer/wallet build invocation (`SEQUENCER_BUILD_ARGS`, `setup`): re-verify "Provisioning the Real LEZ Sequencer Toolchain", then rerun `T1` and `T4`.

The `T`-series must be run against a real sequencer (see the Agent Execution Directives and the provisioning section). When in doubt, rerun more scenarios rather than fewer — and never substitute a stub for the real node in a `T` scenario.
