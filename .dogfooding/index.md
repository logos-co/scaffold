---
dogfooding: 1
repo: github.com/logos-co/scaffold
commit: 740e5e949622681fdb1b477066fc5fe4fa33234a
updated: 2026-11-13
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
| [Project creation](features/project-creation.md) | cli | 1 | 2 | 5 |
| [Setup and build](features/setup-and-build.md) | cli | 2 | 4 | 1 |
| [Localnet](features/localnet.md) | cli | 1 | 3 | 3 |
| [Deploy](features/deploy.md) | cli | 1 | 3 | 4 |
| [Run pipeline](features/run-pipeline.md) | cli | 1 | 2 | 7 |
| [Wallet](features/wallet.md) | cli | 0 | 3 | 5 |
| [Terminal dashboard](features/tui.md) | cli | 0 | 5 | 5 |
| [Example runners](features/example-runners.md) | cli | 0 | 3 | 2 |
| [spel template](features/spel-template.md) | cli | 0 | 4 | 2 |
| [Diagnostics](features/diagnostics.md) | cli | 0 | 3 | 3 |
| [CLI basics](features/cli-basics.md) | cli | 0 | 2 | 6 |
| [AI skills](features/ai-skills.md) | cli | 0 | 0 | 3 |
| [Test-node lifecycle](features/test-node-lifecycle.md) | cli | 0 | 4 | 3 |
| [Test-node RPC](features/test-node-rpc.md) | cli | 0 | 5 | 3 |
| [Basecamp setup](features/basecamp-setup.md) | cli | 0 | 3 | 5 |
| [Basecamp modules](features/basecamp-modules.md) | cli | 0 | 3 | 5 |
| [Basecamp launch](features/basecamp-launch.md) | cli | 0 | 3 | 5 |
| [Rust API](features/rust-api.md) | api | 0 | 1 | 2 |

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
- A `source:` of the form `DOGFOODING.md@740e5e9 <section>` cites the runbook this folder
  replaced. It is no longer in the tree; read it with `git show 740e5e9:DOGFOODING.md`.

## What to rerun after a change

The minimum for a change in each area. A feature name means all of its scenarios. When in
doubt, rerun more, and always against a real sequencer (see [product.md](product.md)).

| Change | Rerun |
|---|---|
| Onboarding, `new`/`create`, `setup`, `localnet` or `build` flows | CREATE-001, CREATE-002, BUILD-001, LOCALNET-001 to LOCALNET-005, LOCALNET-007, DEPLOY-001, WALLET-001, DIAG-001, DIAG-002, DIAG-006, [Example runners](features/example-runners.md) |
| `create`/`new` flags or template selection | CREATE-003 to CREATE-006 |
| `init`, schema migration | CREATE-007, CREATE-008, SKILLS-002 |
| Deploy behavior or deploy output | [Deploy](features/deploy.md), [Example runners](features/example-runners.md) |
| Wallet flows or wallet defaults | [Wallet](features/wallet.md) |
| The wallet-home env contract (`WALLET_HOME_ENV_VARS`, `set_wallet_home_env`), default-wallet seeding (`ensure_default_wallet_seeded`), or the LEZ pin | BUILD-001, [Wallet](features/wallet.md), [Example runners](features/example-runners.md), [Run pipeline](features/run-pipeline.md) |
| `doctor`, `report` contents or redaction | [Diagnostics](features/diagnostics.md) |
| `tui` (`src/tui/`), or anything it reads or runs: localnet status, deploy discovery, the `run` deploy cache, the args of the commands its keys run | [Terminal dashboard](features/tui.md) |
| Example runners or template `src/bin/*` | [Example runners](features/example-runners.md) |
| `run` step order, the `deploy = false` / `topup = false` branches, hook env vars, hook flags, `[run]` parsing, watch mode | [Run pipeline](features/run-pipeline.md) |
| Guest build strategy (`[build]`, `--guest`, `build_methods_guests`, `DEFAULT_RISC0_DOCKER_TAG`, `GUEST_DOCKER_TARGET_DIR`) or deploy-side discovery (`GUEST_BIN_SEARCH_ROOTS`, `discover_program_binaries`) | BUILD-002 to BUILD-005, BUILD-007, DEPLOY-001 to DEPLOY-003, DEPLOY-007, DEPLOY-008. Bumping the docker tag changes every `program_id`: record the values before and after |
| spel template scaffolding or generated outputs | [spel template](features/spel-template.md) |
| Argument parsing, help text or error messages | [CLI basics](features/cli-basics.md) |
| AI skill materialization (`apply_skills`, `skills/`, the frontmatter rewrite, the `AGENTS.md` template, `init` re-run) | [AI skills](features/ai-skills.md) |
| `basecamp setup` (pin sync, lgpm build, seeding, idempotency), `[repos.basecamp.attr]`, `basecamp doctor` | BCSETUP-001 to BCSETUP-006, BCSETUP-008 |
| `[modules]` derivation, dependency resolution, sibling `--override-input`, `basecamp install` | BCMOD-001 to BCMOD-004 |
| `basecamp paths`, `[basecamp.profiles.*]`, `env_file`, `runtime_dir`, `log_file`, `launch --log-file` | BCLAUNCH-001, BCLAUNCH-005, BCLAUNCH-006, BCLAUNCH-008 |
| `basecamp launch` (kill and scrub, XDG isolation, exported env) | BCLAUNCH-001 to BCLAUNCH-004 |
| Scrub semantics, profile-name validation, path bounds, the empty-`[modules]` guard | BCLAUNCH-003, BCLAUNCH-004, BCLAUNCH-007 |
| `basecamp build`, `build-portable`, variants, `--module` | BCMOD-005, BCMOD-006 |
| `basecamp run`, `develop`, `standalone_app` | BCMOD-007, BCMOD-008 |
| `resolve_basecamp_binary`, `basecamp_comm_candidates`, `lgpm_install_hint` | BCSETUP-007, BCLAUNCH-004. These fail silently: a wrong entry point starts an app with no Qt environment, and an unmatched process name skips the kill |
| The basecamp pin set (`DEFAULT_BASECAMP_PIN`, `DEFAULT_LGPM_PIN`, `BASECAMP_DEPENDENCIES`, `BASECAMP_PREINSTALLED_MODULES`) | All three basecamp features, on Linux and macOS, and on macOS with both `attr = "app"` and `attr = "bin-macos-app"`. Check what the release bundles in `$out/modules` and `$out/plugins`. Where the app cannot be built, run BCSETUP-007 and report partial coverage |
| The public `logos_scaffold::api` surface | [Rust API](features/rust-api.md), plus the CLI scenarios of any command whose core changed |
| `test-node` lifecycle, pin resolution, prepare/doctor, run slots, caller-checkout validation | [Test-node lifecycle](features/test-node-lifecycle.md) |
| The `test-node` RPC client (outcomes, block and clock parsing, account and proof reads, JSON shapes) | TRPC-001 to TRPC-004 |
| `test-node` state seeding | TRPC-005, TRPC-006, TRPC-008 |
| The transaction-bearing block path (`sendTransaction`/`getBlock`, the committed-block scan, user-vs-clock classification, the r0vm/sequencer spawn env) | TRPC-007 |
| `[circuits]` parsing, install-dir resolution, materialization, the `doctor` circuits checks | CREATE-001, BUILD-001, LOCALNET-001, DIAG-001, [Example runners](features/example-runners.md), [Test-node lifecycle](features/test-node-lifecycle.md), TRPC-007, API-001 |
| r0vm or circuits provisioning, the LEZ or circuits pins, the sequencer or wallet build (`SEQUENCER_BUILD_ARGS`, `setup`) | Re-verify "Toolchain" in [product.md](product.md), then [Test-node lifecycle](features/test-node-lifecycle.md) and TRPC-007 |
