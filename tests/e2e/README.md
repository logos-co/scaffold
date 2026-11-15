# End-to-end suite

The dogfooding scenarios of [`.dogfooding/features/`](../../.dogfooding/index.md), written as a
program. It drives the real `logos-scaffold` binary against a real LEZ sequencer: it creates
projects, builds LEZ and the guests, starts localnet, sends wallet transactions, deploys, runs
the example clients and reads the chain back. Nothing is stubbed. No model is needed to run it
or to read its verdict.

```bash
# Everything. About an hour on 4 CPUs once LEZ is built in the cache; the first run on a
# cold cache also builds LEZ (see "Long steps" in .dogfooding/product.md).
LGS_E2E=1 cargo test --release --test e2e

# Some scenarios: IDs or prefixes.
LGS_E2E=1 cargo test --release --test e2e -- CLI CREATE-001 TRPC-00

# What is automated and what is not, without running anything:
cargo test --test e2e -- --list
```

Each scenario is reported under its ID as `passed`, `failed`, `blocked` or `skipped`, and the
run writes `report.md`, `report.json` and one log per scenario (every command with its exit
code, timing and output).

| Status | Meaning | Exit code |
|---|---|---|
| `passed` | Every check of the scenario held. A note says when a step could not run on this host. | 0 |
| `failed` | The product did not do what the scenario expects. | 1 |
| `blocked` | Something the scenario needs first did not work (a shared build, a start). The scenario's own subject was never reached. | 1 |
| `skipped` | The host cannot run it (no Docker daemon, no tmux). | 0 |

## What a plain `cargo test` does

Without `LGS_E2E=1` this target runs no scenario. It only checks that the suite and
`.dogfooding/features/` name the same scenarios: every ID in the feature files must be either
automated (`SCENARIOS` in `registry.rs`) or listed with a reason (`MANUAL` there), and nothing
may name a retired ID. That check is cheap and runs in `ci.yml`, so a scenario cannot be added,
retired or renamed without this suite being touched.

The scenarios themselves run in `.github/workflows/dogfood-e2e.yml`: when the suite changes,
after merges that touch the product, weekly, and on demand (`workflow_dispatch`, with an
optional list of IDs).

## What you need

The same as a manual dogfooding run; see "Toolchain" in
[`.dogfooding/product.md`](../../.dogfooding/product.md).

- Linux or macOS, `git`, `bash`, `tar`, `cp`, a Rust toolchain through `rustup`, network access
  to GitHub, and on Linux the PCSC headers.
- `r0vm` and the risc0 Rust toolchain (`rzup install rust`, `rzup install cargo-risczero 3.0.5`,
  and `rzup install cpp` for the spel template).
- Port 3040 free. The localnet port is fixed, so the suite cannot run next to another localnet.
- Optional: a running Docker daemon (`BUILD-002`, `SPEL-002`, `SPEL-005`; otherwise skipped),
  `tmux` (the `TUI-*` scenarios; otherwise skipped).

Guests are built with the host toolchain (`[build].guest = "local"`), so the suite does not
depend on Docker. The reproducible Docker build has its own workflow,
`guest-reproducibility.yml`.

## Settings

| Variable | Default | Meaning |
|---|---|---|
| `LGS_E2E` | unset | `1` runs the scenarios. |
| `LGS_E2E_WORKDIR` | a new temp dir, removed after a green run | Where projects are generated. Give a fixed path to keep it: the next run reuses the projects' `target/` directories, so only the first run compiles the template's dependencies. |
| `LGS_E2E_CACHE_ROOT` | `<workdir>/_cache` | The scaffold cache root (LEZ and spel checkouts and builds). The default is private and therefore cold: the first run builds LEZ. Point it at a warm cache to save that, but note that a few scenarios move files in the LEZ checkout aside and back (the wallet binary, the sequencer database marker), so do not share it with work in progress. |
| `LGS_E2E_BIN` | the binary cargo just built | Another `logos-scaffold` to test (a release, an older build); `lgs` must sit next to it. The Rust API scenarios still exercise the library of this checkout. |
| `LGS_E2E_OUT` | `<workdir>/_report` | Where `report.md`, `report.json` and `logs/` go. |
| `LGS_E2E_KEEP` | unset | Keep a temp work dir after a green run. |

Commands run with a scrubbed environment: the scaffold variables that change behaviour
(`LOGOS_SCAFFOLD_QUIET`, `LOGOS_SCAFFOLD_WALLET_PASSWORD`, `LOGOS_BLOCKCHAIN_CIRCUITS`,
`DOCKER_HOST`, …) are removed unless a scenario sets them, and so is the toolchain that
`cargo test` exports to its children (`RUSTUP_TOOLCHAIN`, `CARGO_*`, its `PATH` entry).
Without the second, LEZ and the generated projects would be compiled with the wrong Rust.

## How it is put together

| File | Contents |
|---|---|
| `registry.rs` | The scenario list in run order, the manual list, the sync check, the run loop and the report. |
| `harness.rs` | Verdicts, command execution (timeout, process group, log), guards that restore files, local listeners, the shared context. |
| `fixtures.rs` | Steps many scenarios share, each run once: the main project, its `setup`, its build, its topped-up wallet, its deploy; the `run` project; the spel project; `test-node prepare`. |
| `scenarios/<feature>.rs` | One function per scenario; its doc comment is the scenario's ID and title. |

**Order.** Scenarios share one sequencer port and, through the cache root, one chain, so they
run one at a time in the order of `SCENARIOS`. That order follows the run plan in
`product.md`: everything that needs the main project's build runs on it once, the scenarios
that wipe the chain close each block.

**Fixtures and verdicts.** A fixture such as "the main project after `lgs build`" runs once.
The scenario that is *about* that step (`BUILD-005`) gets the step's own verdict; any other
scenario that needs it and finds it broken is `blocked`, not `failed`. So one broken build
gives one `failed` and a list of `blocked`, as a dogfooding report would.

**Timeouts.** Every command has one, and runs in its own process group so that a timeout
kills what it started. A command that hangs is a finding with a log, not a stuck job.

**Hung peers.** Where a scenario needs "an address that never answers", the suite uses
`10.255.255.1` only after checking that a connection to it times out; if the host refuses it
at once, those steps are noted as not run. For a container runtime that never answers it uses
a local socket that accepts and stays silent, which works everywhere.

## Coverage

`cargo test --test e2e -- --list` prints the current split. 73 of the 89 scenarios are
automated. Not automated, and why:

| Scenarios | Why |
|---|---|
| `BCSETUP-001/002/004`, `BCMOD-*`, `BCLAUNCH-001/002/003/006/007` | Need the basecamp app built with Nix (about 5 GB of RAM or the Logos binary cache), module projects, and for launch a graphical session and a second profile to interact with. |
| `BCSETUP-007` | Nix builds of `lgpm` and two module projects; tens of minutes without the Logos binary cache. |
| `BUILD-004` | Byte reproducibility across a cleared BuildKit cache already is the `guest-reproducibility.yml` workflow. |
| `API-002` | `cargo test` already runs the documentation examples as doctests. |

Automated with a step left out (the report's note says so each time):

- `TUI-007` step 3 (wrapping and PgUp on a long failing build).
- `TUI-004` uses `w` (`run --watch`) as the long-running job instead of `b`, which can finish
  before the keys are pressed.
- `BUILD-003` steps 1–2 only where no Docker daemon runs; `BUILD-006` cannot reach the
  verified-download path while no prebuilt is published for the pin.
- Steps that need the spel CLI, a Docker CLI, or the black-hole address, where the host lacks
  them.
- `BUILD-005` step 4 (which artefact `deploy` ships after a local build) is asserted in
  `DEPLOY-001`, on the deploy every later scenario shares.
- `RUNNER-004` runs `run_hello_world_private`; the other private paths the scenario's notes
  list are not run.

## Changing scenarios

When a scenario in `.dogfooding/features/` changes, change its function here in the same
change. When you add one, add a function and a line in `SCENARIOS`, or a line in `MANUAL`
with the reason. When you retire one, remove it here too. `cargo test` fails until both sides
agree.

A scenario function reads like the scenario:

```rust
/// DEPLOY-005 — Unknown program or missing binaries give an actionable error
pub fn unknown_or_missing(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_deployed(cx, false)?;          // precondition
    cx.lgs(&p.root, &["deploy", "nonexistent_program"])        // step
        .run()
        .fails()?                                              // success criteria
        .has("unknown program")?
        .has("hello_world")?;
    Ok(())
}
```

Use `check!(cond, "what was expected: {evidence}")` for anything `Out` has no helper for,
`cx.note(…)` when a step cannot run on the host, and a guard (`Restore`, `MovedAway`) around
every file the scenario edits, so a failure halfway does not leave the project or the cache
changed for the scenarios after it.
