---
dogfooding: 1
repo: github.com/logos-co/scaffold
commit: 23cdd95222ddd0ca820b9e6d28ad385890f0a712
updated: 2026-11-15
---

# logos-scaffold — dogfooding context

`logos-scaffold` (alias `lgs`) is a Rust CLI and library for Logos program developers. It
creates projects from the bare-LEZ or spel template, builds a pinned, project-local LEZ
toolchain, runs a local sequencer, deploys reproducibly built risc0 guests and runs the whole
inner loop with `lgs run`. It also runs disposable test-node sequencers for integration tests
and builds and launches `.lgx` modules in isolated basecamp profiles. Details:
[product.md](product.md), which also holds the environment setup and the rules for a run.

| Feature | Surface | P0 | P1 | P2 |
|---|---|---|---|---|
| [Project creation](features/project-creation.md) | cli | 1 | 2 | 3 |
| [Setup and build](features/setup-and-build.md) | cli | 2 | 3 | 1 |
| [Localnet](features/localnet.md) | cli | 1 | 2 | 2 |
| [Deploy](features/deploy.md) | cli | 1 | 1 | 3 |
| [Run pipeline](features/run-pipeline.md) | cli | 1 | 2 | 4 |
| [Wallet](features/wallet.md) | cli | 0 | 2 | 3 |
| [Terminal dashboard](features/tui.md) | cli | 0 | 4 | 1 |
| [Example runners](features/example-runners.md) | cli | 0 | 2 | 2 |
| [spel template](features/spel-template.md) | cli | 0 | 4 | 0 |
| [Diagnostics](features/diagnostics.md) | cli | 0 | 2 | 2 |
| [CLI basics](features/cli-basics.md) | cli | 0 | 2 | 2 |
| [AI skills](features/ai-skills.md) | cli | 0 | 0 | 2 |
| [Test-node lifecycle](features/test-node-lifecycle.md) | cli | 0 | 4 | 2 |
| [Test-node RPC](features/test-node-rpc.md) | cli | 0 | 5 | 1 |
| [Basecamp setup](features/basecamp-setup.md) | cli | 0 | 2 | 3 |
| [Basecamp modules](features/basecamp-modules.md) | cli | 0 | 3 | 2 |
| [Basecamp launch](features/basecamp-launch.md) | cli | 0 | 3 | 3 |
| [Rust API](features/rust-api.md) | api | 0 | 2 | 2 |

## How this folder works

This folder specifies the product behavior to check when dogfooding. Run results never go
here: they go in `runs/`, which is gitignored.

- Each file in `features/` is one feature. Its frontmatter lists the scenarios. Each scenario
  is a `## <ID> — <title>` section with Expected / Preconditions / Steps / Success criteria.
- **IDs** (`WALLET-001`) are stable. Never renumber or reuse them. New scenarios take the next
  number. Removed IDs move to the feature's `retired: [...]` list.
- **Priority:** P0 = the product does not work at all if this is broken. P1 = baseline
  functionality, the primary user flows. P2 = extras whose failure doesn't materially affect
  users.
- **Provenance:** `confirmed` (the user said so), `documented` (docs or spec say so),
  `inferred` (read from code, and possibly wrong). If a code change contradicts a `confirmed`
  expectation, ask the user. Don't rewrite the expectation.
- `depends_on: [ID, ...]` names the direct prerequisites. If a prerequisite fails, the
  scenarios that depend on it are reported as blocked.
- `commit` above is the product commit this context was last fully reconciled against. Move it
  only after reviewing every change since then.
- Frontmatter: one key per line. Lists are inline (`[A-001, B-002]`) or one `- A-001` line per
  item. No other YAML.
- A scenario covers one user-visible behaviour with everything that shares its setup: the
  happy path, its flags and its error cases. Before adding a scenario, look for one whose
  preconditions already match and add a step there. Keep P0 scenarios to the bare flow.
- When a scenario is merged into another, its ID is retired and "Retired IDs" below says
  where its checks went.
- A `source:` of the form `DOGFOODING.md@740e5e9 <section>` cites the runbook this folder
  replaced. It is no longer in the tree; read it with `git show 740e5e9:DOGFOODING.md`.

## What to rerun after a change

The minimum for a change in each area. A feature name means all of its scenarios. When in
doubt, rerun more, and always against a real sequencer (see [product.md](product.md)).

| Change | Rerun |
|---|---|
| Onboarding, `new`/`create`, `setup`, `localnet` or `build` flows | CREATE-001, BUILD-001, [Localnet](features/localnet.md), DEPLOY-001, WALLET-001, DIAG-001, DIAG-006, [Example runners](features/example-runners.md) |
| `create`/`new` flags or template selection | CREATE-003 to CREATE-005 |
| `init`, schema migration | CREATE-007, CREATE-008, SKILLS-002 |
| Deploy behavior or deploy output | [Deploy](features/deploy.md), [Example runners](features/example-runners.md) |
| Wallet flows or wallet defaults | [Wallet](features/wallet.md) |
| The wallet-home env contract (`WALLET_HOME_ENV_VARS`, `set_wallet_home_env`), default-wallet seeding (`ensure_default_wallet_seeded`), or the LEZ pin | BUILD-001, [Wallet](features/wallet.md), [Example runners](features/example-runners.md), [Run pipeline](features/run-pipeline.md) |
| `doctor`, `report` contents or redaction | [Diagnostics](features/diagnostics.md) |
| `tui` (`src/tui/`), or anything it reads or runs: localnet status, deploy discovery, the `run` deploy cache, the args of the commands its keys run | [Terminal dashboard](features/tui.md) |
| Example runners or template `src/bin/*` | [Example runners](features/example-runners.md), RUN-010 |
| `run` step order, profiles (`deploy = false`, `topup = false`, `default_profile`), hook env vars, hook flags, `[run]` parsing, watch mode | [Run pipeline](features/run-pipeline.md) |
| Guest build strategy (`[build]`, `--guest`, `build_methods_guests`, `DEFAULT_RISC0_DOCKER_TAG`, `GUEST_DOCKER_TARGET_DIR`) or deploy-side discovery (`GUEST_BIN_SEARCH_ROOTS`, `discover_program_binaries`) | BUILD-002 to BUILD-005, DEPLOY-001, DEPLOY-002, DEPLOY-005, DEPLOY-007. Bumping the docker tag changes every `program_id`: record the values before and after |
| spel template scaffolding or generated outputs | [spel template](features/spel-template.md), CREATE-004 |
| Argument parsing, help text or error messages | [CLI basics](features/cli-basics.md) |
| AI skill materialization (`apply_skills`, `skills/`, the frontmatter rewrite, the `AGENTS.md` template, `init` re-run) | [AI skills](features/ai-skills.md) |
| `basecamp setup` (pin sync, lgpm build, seeding, idempotency), `[repos.basecamp.attr]`, `basecamp doctor` | [Basecamp setup](features/basecamp-setup.md) |
| `[modules]` derivation, dependency resolution, sibling `--override-input`, `basecamp install` | BCMOD-001 to BCMOD-003 |
| `basecamp paths`, `[basecamp.profiles.*]`, `env_file`, `runtime_dir`, `log_file`, `launch --log-file` | BCLAUNCH-001, BCLAUNCH-005, BCLAUNCH-006, BCLAUNCH-007 |
| `basecamp launch` (kill and scrub, XDG isolation, exported env), profile-name validation, path bounds, the empty-`[modules]` guard | BCLAUNCH-001 to BCLAUNCH-003, BCLAUNCH-007 |
| `basecamp build`, `build-portable`, variants, `--module` | BCMOD-005 |
| `basecamp run`, `develop`, `standalone_app` | BCMOD-007 |
| `resolve_basecamp_binary`, `basecamp_comm_candidates`, `lgpm_install_hint` | BCSETUP-001, BCSETUP-007, BCLAUNCH-003. These fail silently: a wrong entry point starts an app with no Qt environment, and an unmatched process name skips the kill |
| The basecamp pin set (`DEFAULT_BASECAMP_PIN`, `DEFAULT_LGPM_PIN`, `BASECAMP_DEPENDENCIES`, `BASECAMP_PREINSTALLED_MODULES`) | All three basecamp features, on Linux and macOS, and on macOS with both `attr = "app"` and `attr = "bin-macos-app"`. Check what the release bundles in `$out/modules` and `$out/plugins`. Where the app cannot be built, run BCSETUP-007 and report partial coverage |
| The public `logos_scaffold::api` surface | [Rust API](features/rust-api.md), plus the CLI scenarios of any command whose core changed |
| `test-node` lifecycle, pin resolution, prepare/doctor, run slots, caller-checkout validation | [Test-node lifecycle](features/test-node-lifecycle.md), API-004 |
| The `test-node` RPC client (outcomes, block and clock parsing, account and proof reads, JSON shapes) | TRPC-001 to TRPC-003 |
| `test-node` state seeding | TRPC-005, TRPC-006, TRPC-007 |
| The transaction-bearing block path (`sendTransaction`/`getBlock`, the committed-block scan, user-vs-clock classification, the r0vm/sequencer spawn env) | TRPC-007 |
| `[circuits]` parsing, install-dir resolution, materialization, the `doctor` circuits checks | CREATE-001, BUILD-001, LOCALNET-001, DIAG-001, [Example runners](features/example-runners.md), [Test-node lifecycle](features/test-node-lifecycle.md), TRPC-007, API-001 |
| r0vm or circuits provisioning, the LEZ or circuits pins, the sequencer or wallet build (`SEQUENCER_BUILD_ARGS`, `setup`) | Re-verify "Toolchain" in [product.md](product.md), then [Test-node lifecycle](features/test-node-lifecycle.md) and TRPC-007 |

## Retired IDs

Each retired scenario was merged into the one named here, unless marked removed. Its checks
are steps of that scenario now.

| Retired | Now in | | Retired | Now in |
|---|---|---|---|---|
| CLI-002, CLI-004, CLI-007 | CLI-001 | | RUN-004 | RUN-003 |
| CLI-005 | removed (see below) | | RUN-006 | RUN-005 |
| CREATE-002 | CREATE-003 | | RUN-009 | RUN-002 |
| CREATE-006 | CREATE-005 | | WALLET-005 | WALLET-002 |
| SKILLS-003 | SKILLS-002 | | WALLET-006 | WALLET-003 |
| BUILD-007 | BUILD-004 | | WALLET-008 | WALLET-004 |
| LOCALNET-003 | LOCALNET-002 | | TUI-003 | TUI-002 |
| LOCALNET-005 | LOCALNET-004 | | TUI-005 | TUI-004 |
| DEPLOY-003, DEPLOY-004 | DEPLOY-002 | | TUI-008, TUI-009, TUI-010 | TUI-007 |
| DEPLOY-008 | DEPLOY-005 | | RUNNER-002 | RUNNER-001 |
| DIAG-002 | DIAG-001 | | SPEL-004 | SPEL-003 |
| DIAG-003 | DIAG-004 | | SPEL-006 | CREATE-004 |
| TNODE-007 | TNODE-005 | | TRPC-004 | TRPC-003 |
| TRPC-008 | TRPC-007 | | BCSETUP-003 | BCSETUP-002 |
| BCSETUP-005 | BCSETUP-004 | | BCSETUP-008 | BCSETUP-006 |
| BCMOD-004 | BCMOD-002 | | BCMOD-006 | BCMOD-005 |
| BCMOD-008 | BCMOD-007 | | BCLAUNCH-004 | BCLAUNCH-003 |
| BCLAUNCH-008 | BCLAUNCH-001, BCLAUNCH-007 | | | |

CLI-005 checked that `setup --wallet-install`, a flag removed several releases ago, is
rejected. Any unknown flag is rejected by the argument parser, so the scenario no longer
tested anything specific.
