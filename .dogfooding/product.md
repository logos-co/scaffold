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
- Toolchain and build: `setup [--prebuilt]`, `build [project-path] [--prebuilt]
  [--guest local|docker]`, `build idl [project-path]`, `build client [project-path]`.
- Local sequencer: `localnet start|stop|status|logs|reset`.
- Deploy and the inner loop: `deploy`, `run` (profiles, hooks, `--watch`, `--reset`).
- Terminal dashboard over the inner loop: `tui` (status in process; every action key runs the
  matching CLI command as a child process).
- Wallet: `wallet list|topup|default set`, `wallet -- <raw wallet args>`.
- spel passthrough: `spel -- <raw spel args>`.
- Diagnostics: `doctor [--json]`, `report`, `completions <bash|zsh>`.
- Test nodes: `test-node pins|prepare|doctor|start|status|stop|run|tx|blocks|clock|account|proof|snapshot|state`.
- Basecamp: `basecamp setup|modules|install|launch|develop|build|build-portable|run|paths|doctor|docs`.
- AI skills shipped into every project: `skills/` → `.claude/skills/`, `.cursor/rules/`,
  `AGENTS.md`.
- Rust API: `src/api/` (`Project`, `api::testnode::{TestNode, TestNodeClient}`, `api::Error`).
- Global: `-q/--quiet` (or `LOGOS_SCAFFOLD_QUIET`), `--version`, and `help [COMMAND]...` (a hidden alias of
  `[COMMAND]... --help`; a bare `lgs` prints the same text).
- Hidden `self-test` subcommand: test-only, not a user surface.

## How to build and run
Everything a run needs is in this file and the feature files; there is no separate runbook.

### Workspace
Do this from a fresh clone, and test the freshly built binary rather than an installed one.

```bash
export REPO_ROOT=/abs/path/to/scaffold   # a checkout of github.com/logos-co/scaffold
export SCRATCH_ROOT=/abs/path/to/dogfood-runs     # outside the repo
cd "$REPO_ROOT" && cargo build
export SCAFFOLD_BIN="$REPO_ROOT/target/debug/logos-scaffold"   # lgs is the same binary
mkdir -p "$SCRATCH_ROOT"
```

Scenarios use these names throughout: `$P` is a generated project root (usually
`$SCRATCH_ROOT/dogfood-default`), `$MODULE_PROJECT` is a Logos module project for the
basecamp features, and `$TN_URL` is a running test node's `rpc_url`.

| Where | What runs there |
|---|---|
| Repo root | `cargo build`, help/version checks, the Rust API scenarios |
| `$SCRATCH_ROOT` | `new`/`create`/`init`, so generated projects never land in the repo |
| A generated project root | `setup`, `build`, `localnet`, `deploy`, `run`, `wallet`, `doctor`, `report`, `tui`, `cargo run --bin run_*` |
| Anywhere | `test-node …` with `--project <root>`; the RPC subcommands only need `--url` |

Project-scoped commands run from the repo root are only correct when a scenario checks the
"outside a project" error. Use a fresh project per run unless the scenario is about
persistence, and keep spel projects separate from default-template ones.

LEZ tooling is project-local state. A non-vendored project uses
`<cache_root>/repos/lez/<pin>/` (`<pin>-src-<hash>/` for a non-default LEZ source); a vendored
one uses `<project>/.scaffold/repos/lez`. The wallet under test is always
`<lez>/target/release/wallet`, reached through `"$SCAFFOLD_BIN" wallet …`. A `wallet` on
`PATH` is never the binary under test.

### Rules for a run
- **Run the real thing.** Localnet, deploy, run and test-node scenarios need a real LEZ
  sequencer executing real transactions. A stub or a scripted JSON-RPC server can be added to
  a real run, but never replaces it.
- **Install what is missing.** A fresh container has none of r0vm, the circuits release or the
  LEZ binaries. "Toolchain" below installs them with `curl` and network access to GitHub.
  Once installed they persist in the scaffold cache and in rzup's home, so reuse them.
- **A failed installer is a routing problem.** `rzup` fails with
  `InvalidCertificate(UnknownIssuer)` behind a TLS-intercepting proxy: fetch the release
  tarball with `curl`. Unauthenticated GitHub API calls get rate-limited: add
  `-H "Authorization: Bearer $GH_TOKEN"`. Report a scenario as blocked only after those
  fallbacks fail.
- **Background the long steps** with something that outlives one shell, and wait on a
  sentinel (the built binary, or an `EXIT=` marker), not a fixed sleep. Durations are listed
  under "Long steps".
- **Record evidence per scenario:** the command, its cwd, the exit code and a short output
  excerpt. Keep `--json` output exactly as printed. Keep unredacted account IDs in the local
  evidence so repeated runs stay traceable.
- **Capture a contradiction before fixing it.** If tracked PID, listener and readiness
  disagree, save `localnet status` first and restart afterwards.
- **Mutate configs one guarded command at a time.** Several scenarios edit `scaffold.toml`
  or `wallet_config.json`. Back the file up, restore it in a `trap`, and never chain the
  mutations in one loop: when a step hangs and is killed, the loop carries on with the wrong
  config.
- **Mind what your shell exports.** `LOGOS_SCAFFOLD_CACHE_ROOT` outranks `[scaffold].cache_root`
  and `--cache-root` for every command after creation, and `LOGOS_SCAFFOLD_WALLET_PASSWORD`
  replaces the default password. A private cache root per run is good practice (a shared one
  can be reset or rebuilt under you); scenarios that test these layers say to unset them.
- **Kill by process name, not by command line.** `pkill -f <text>` and `pgrep -f <text>` also
  match the shell that runs them. Use `pgrep -x logos-scaffold`, `pgrep -x sequencer_servi`
  (the kernel truncates names to 15 characters), `pgrep -x cargo`.
- **Never kill a `setup`, `build` or `run` that is compiling** to save time. Where the Rust
  toolchain LEZ pins is not installed yet, the kill can interrupt rustup's download and
  leave that toolchain broken (`missing manifest in toolchain …`). When a scenario only
  needs what happens before the first build, take `cargo` away instead and let the command
  stop by itself:
  `NOCARGO=$(echo "$PATH" | tr ':' '\n' | while read -r d; do [ -x "$d/cargo" ] || echo "$d"; done | paste -sd:)`,
  then `env PATH="$NOCARGO" "$SCAFFOLD_BIN" setup`.
- **Exit code 0 is not proof.** A runner can submit a transaction the sequencer then rejects,
  and a wallet command can silently use `~/.lee/wallet`. The scenarios say what to read back.

### Run plan
A full run is dominated by a handful of long builds, so share them. One order that builds
everything once:

1. **No project needed:** CLI-001, CLI-003, DIAG-005, API-002.
2. **Create:** CREATE-001 (`dogfood-default`), then CREATE-003, CREATE-007, CREATE-008,
   SKILLS-002. Start SPEL-001 early: it builds the spel CLI that CREATE-003 step 3,
   CREATE-004 step 2 and SKILLS-001 reuse.
3. **Main project (`dogfood-default`):** BUILD-001 → LOCALNET → BUILD-002/004 (or BUILD-005
   without Docker) → WALLET → DEPLOY → RUNNER → DIAG → TUI. CLI-006, CLI-008 step 2 and the
   test-node and API features use this project too. Stop localnet before TRPC-007 and
   TNODE-004 step 4.
4. **A second, fresh project (`dogfood-run`):** RUN-001 onward; RUN-010 in a copy.
5. **Cheap side projects:** CREATE-004, CREATE-005, BUILD-003, BUILD-006 and TNODE-002 step 1
   only create a project or stop early; none needs a full build.
6. **spel project:** SPEL-002, SPEL-003, SPEL-005.
7. **Basecamp:** BCSETUP-006, BCLAUNCH-005 and BCSETUP-007 anywhere; the rest after
   BCSETUP-001.

Only one localnet (and one `--port 3040` test node) fits on port 3040 at a time, and
projects that share a cache root and LEZ pin share one chain database, so run the scenarios
that reset or stop localnet one at a time.

What a host cannot provide changes the run like this. Say which rows applied in the report.

| Host lacks | Do instead | Report as |
|---|---|---|
| Docker daemon | Build with `[build].guest = "local"` (BUILD-005). DEPLOY, RUN, RUNNER and TUI run on that build; DEPLOY-007 step 3 checks the docker ranking by hand. | BUILD-002, BUILD-004, SPEL-002: skipped, citing the `guest-reproducibility.yml` and `template-e2e.yml` runs (both take `workflow_dispatch`). SPEL-005: skipped, or run on a Docker-capable host. |
| A published prebuilt for the pin | Stop BUILD-006 after the fallback line. | BUILD-006: passed for the fallback, verification path not exercised. |
| Nix, or GitHub access for Nix | BCSETUP-006, BCLAUNCH-005. | Every other basecamp scenario: blocked. |
| RAM for the basecamp flake, and `cache.nix.logos.co` | BCSETUP-006, BCLAUNCH-005, BCSETUP-007. | BCSETUP-001 and its dependants: blocked. |
| A graphical session | Start `Xvfb` (see "Basecamp environment"). | — |

### Toolchain
- **All areas:** Unix (Linux or macOS), `git`, `rustc`/`cargo` ≥ 1.85, `lsof`, `ps`, `kill`,
  and `curl` (circuits download). Linux also needs `pkg-config` plus the PCSC headers
  (Debian/Ubuntu `pkgconf libpcsclite-dev`, Fedora `pkgconf pcsc-lite-devel`, Arch
  `pkgconf pcsclite`), because the LEZ wallet links against libpcsclite. Without them `setup`
  and `new --template spel` fail minutes in with a raw `pcsc-sys` error.
- **Guest builds (default `docker` mode):** a running Docker or Podman daemon and
  `cargo-risczero` (`rzup install cargo-risczero 3.0.5`; `rzup install rust` does not provide
  it). Without Docker, see "Run plan" below for what replaces what.
- **Guest compilation (`build`):** the risc0 Rust toolchain (step 3 below), and for the spel
  template also the risc0 C++ toolchain (step 4).
- **Real sequencer (localnet, deploy, run, test-node):** `r0vm` at the exact risc0 version in
  the pinned LEZ `Cargo.lock` (steps 1 and 2).
- **Circuits:** nothing to install by hand. Projects with a `[circuits]` table (every freshly
  generated one) materialize the pinned release into `.scaffold/circuits` during `setup`,
  `build`, `build idl`, `localnet` and test-node startup. Set `LOGOS_BLOCKCHAIN_CIRCUITS=<path>`
  only to override with a local copy, or for an older project without `[circuits]`.
  `~/.logos-blockchain-circuits/` is not consulted.
- **Dashboard:** `tmux`, to drive and capture `tui` non-interactively.
- **Basecamp:** see "Basecamp environment" below.
- **Rust API:** run `cargo doc --no-deps` and `cargo test --doc` from the repo root.

Provision the real sequencer toolchain once. `$P` is a generated project (CREATE-001).

**1. Find the risc0 version the pinned LEZ needs.** The r0vm lookup needs an exact match.

```bash
LEZ=$("$SCAFFOLD_BIN" test-node pins --project "$P" --json | jq -r .lez_checkout)
grep -A1 'name = "risc0-zkvm"' "$LEZ/Cargo.lock"   # e.g. version = "3.0.5"
```

Before `setup` or `test-node prepare` has cloned LEZ, `$LEZ` does not exist yet. Read the
version from the LEZ repository at the pin `test-node pins` prints instead.

**2. Install r0vm at that version.** Try `rzup install r0vm <ver>` first
(`curl -sSL https://risczero.com/install | bash` installs rzup). If rzup cannot download,
take `r0vm` from the release tarball:

```bash
VER=3.0.5; TRIPLE=x86_64-unknown-linux-gnu       # aarch64-apple-darwin on macOS
RZUP_HOME="${RISC0_HOME:-$HOME/.risc0}"           # rzup's home, as rzup and scaffold resolve it
curl -sSL -H "Authorization: Bearer $GH_TOKEN" -o /tmp/cr.tgz \
  "https://github.com/risc0/risc0/releases/download/v$VER/cargo-risczero-$TRIPLE.tgz"
mkdir -p /tmp/cr && tar xzf /tmp/cr.tgz -C /tmp/cr
EXT="$RZUP_HOME/extensions/v$VER-cargo-risczero-$TRIPLE"
mkdir -p "$EXT" && cp /tmp/cr/r0vm "$EXT/r0vm" && chmod +x "$EXT/r0vm"
"$EXT/r0vm" --version                              # → risc0-r0vm 3.0.5
```

Scaffold looks for `<rzup home>/extensions/v<risc0-zkvm-version>-cargo-risczero-<arch>-<os>/r0vm`
and passes it to the sequencer as `RISC0_SERVER_PATH`. The rzup home is `$RISC0_HOME` when
set (a relative value is taken from the directory scaffold runs in), else `~/.risc0`. The
sequencer runs with `RISC0_DEV_MODE=1`, so r0vm executes guests without proving and no GPU
is needed.

**3. For guest compilation, install the risc0 Rust toolchain.** Without one, `build` fails
with `Risc Zero Rust toolchain not found. Try running rzup install rust`. If rzup is unusable,
unpack the `risc0/rust` release asset into rzup's layout. The directory name must be
`v<version>-rust-<triple>`; no `settings.toml` is needed, and the highest installed version
wins. Take the newest tag: older toolchains fail on guest dependency MSRVs
(`rustc 1.88.0-dev is not supported by the following packages`).

```bash
TCVER=1.91.1   # tag r0.1.91.1; use the newest published
curl -sSL -H "Authorization: Bearer $GH_TOKEN" -o /tmp/rust-tc.tar.gz \
  "https://github.com/risc0/rust/releases/download/r0.$TCVER/rust-toolchain-$TRIPLE.tar.gz"
DEST="$RZUP_HOME/toolchains/v$TCVER-rust-$TRIPLE"
mkdir -p "$DEST" && tar xzf /tmp/rust-tc.tar.gz -C "$DEST"
"$DEST/bin/rustc" --version                        # → rustc 1.91.1-dev
```

**4. For the spel template, also install `cargo-risczero` and the risc0 C++ toolchain.** The
spel guest builds through `cargo risczero build` (without it: `error: no such command:
risczero`) and compiles C (without the toolchain: `failed to find tool
"/no_risc0_cpp_toolchain_installed_run_rzup_install_cpp"`). The asset name is
platform-specific and does not follow `$TRIPLE`, and the directory uses the semver form of
the date tag (`2024.01.05` → `v2024.1.5`).

```bash
CPPASSET=riscv32im-linux-x86_64                    # riscv32im-osx-arm64 on macOS arm64
curl -sSL -H "Authorization: Bearer $GH_TOKEN" -o /tmp/cpp-tc.tar.xz \
  "https://github.com/risc0/toolchain/releases/download/2024.01.05/$CPPASSET.tar.xz"
CPPDEST="$RZUP_HOME/toolchains/v2024.1.5-cpp-$TRIPLE"
mkdir -p /tmp/cpp-tc && tar xJf /tmp/cpp-tc.tar.xz -C /tmp/cpp-tc
mkdir -p "$CPPDEST" && mv /tmp/cpp-tc/"$CPPASSET"/* "$CPPDEST/"
ln -sfn "$CPPDEST" "$RZUP_HOME/cpp"
"$CPPDEST/bin/riscv32-unknown-elf-gcc" --version   # → gcc 13.2.0
```

**5. Build the sequencer, and the wallet when a scenario submits transactions.**
`test-node prepare` takes `--project`; `setup` discovers the project from the cwd, so run it
inside the project.

```bash
"$SCAFFOLD_BIN" test-node prepare --project "$P"            # → "test-node prerequisites ready"
"$SCAFFOLD_BIN" test-node doctor  --project "$P" --json | jq .ok   # → true
( cd "$P" && "$SCAFFOLD_BIN" setup )                        # wallet + spel → "setup complete"
```

**6. Check the result.** If one of these is missing, fix the step that produces it; do not
skip the real run.

```bash
"$EXT/r0vm" --version
ls "$LEZ/target/release/sequencer_service" "$LEZ/target/release/wallet"
ls "$P"/.scaffold/circuits/pol/verification_key.json
```

### Basecamp environment
- **Nix with flakes, and unrestricted GitHub access for Nix.** Nix resolves `github:` inputs
  through `api.github.com/repos/…/commits/HEAD` and `github.com/…/archive/<rev>.tar.gz`. A
  proxy that allowlists GitHub per repository answers `403`, and neither a working `git
  clone` nor `nix --version` proves access. Probe it first:
  `curl -sS -o /dev/null -w '%{http_code}\n' https://api.github.com/repos/NixOS/nixpkgs/commits/HEAD`
  must print `200`. Rewriting the project's input to `git+https://` does not help, because
  the transitive inputs are locked as `github:`. If the probe fails, run BCSETUP-006 and
  BCLAUNCH-005 and report the blocker.
- **Memory.** Evaluating the basecamp 0.3.0 flake (about 3.3k lock nodes) peaks above about
  4.8 GB of RAM before any build starts, and running out shows as a bare `SIGKILL` with no
  error text. `nix build .#app --dry-run` only evaluates, so a `SIGKILL` there confirms it.
  On a smaller host, reuse basecamp's own CI build instead of evaluating. It is pushed to
  `https://cache.nix.logos.co/ci`; for 0.3.0 on x86_64-linux:

  ```bash
  OUT=/nix/store/yf8mbqrxkgw0kjyfgs8f4491rjx9vypz-logos-basecamp-0.3.0
  nix-store -r "$OUT" --option extra-substituters https://cache.nix.logos.co/ci --option require-sigs false
  CACHE=$("$SCAFFOLD_BIN" doctor --json | jq -r '.checks[] | select(.name=="cache root") | .detail' | cut -d' ' -f1)   # or $LOGOS_SCAFFOLD_CACHE_ROOT
  mkdir -p "$CACHE/basecamp/bbe5da0e038ef19095690f4164a8ddaab0915821"
  nix build "$OUT" --out-link "$CACHE/basecamp/bbe5da0e038ef19095690f4164a8ddaab0915821/app-result"
  ```

  `basecamp setup` reuses an existing build for an unchanged commit pin, so everything after
  the basecamp build (clone, `lgpm`, seeding, state, install, launch) still runs for real.
  Record that setup ran in this mode. This route needs `cache.nix.logos.co`: if
  `curl -m 15 https://cache.nix.logos.co/nix-cache-info` times out, `nix-store -r` fails after
  minutes with `there is no substituter that can build it`. Where neither route works, report
  BCSETUP-001 and its dependants as blocked (do not seed state by hand to get past it), and
  run BCSETUP-007, which covers the `lgpm` pin and the `.lgx` contract without an app build.
- **Binary cache.** The pinned flake declares `extra-substituters =
  https://cache.nix.logos.co/public`, but scaffold runs a plain `nix build`, which honours it
  only for a trusted user. Without it a cold `basecamp setup` builds a Qt closure from source.
  Add the substituter and its key to `~/.config/nix/nix.conf` (as a `trusted-users` member),
  and say which mode any reported duration was measured in.
- **A module project.** Its `flake.nix` exposes `packages.<system>.lgx`, built with
  `logos-module-builder` 0.3.0. That repo's `templates/minimal-module` (a core module) is the
  smallest one; add `templates/ui-qml` as a second sub-flake to cover a UI plugin (it has
  an icon, which BCSETUP-007 needs). Copy them out of a clone of that repo at tag `0.3.0`:
  `nix flake init -t github:logos-co/logos-module-builder/0.3.0#minimal-module` fails, because
  the flake exports no templates. Pin the copied template's `logos-module-builder` input to
  `github:logos-co/logos-module-builder/0.3.0`, since the templates track `main`. Without
  the Logos binary cache the first `nix build .#lgx` compiles the Logos libraries (about 35
  minutes on 4 CPUs). Packages from older tooling do not install. The contract
  is `docs/basecamp-module-requirements.md` (`"$SCAFFOLD_BIN" basecamp docs`).
- **A graphical session for launch.** With `QT_QPA_PLATFORM=xcb` and no reachable X display,
  basecamp 0.3.0 aborts at startup (SIGABRT, empty log, no `launch.state`). In a headless
  container start `Xvfb :<n>` and confirm it with `xdpyinfo` before blaming the launch.
  `QT_QPA_PLATFORM=offscreen` starts the app and its modules but cannot be driven. `xdotool`
  and ImageMagick's `import` are enough to open an app tile and screenshot it.

### Long steps
Measured cold on 4 CPUs with `CARGO_BUILD_JOBS=2` (a 16-core machine is several times
faster):

| Step | Time |
|---|---|
| First `setup` on a cache root (sequencer + wallet + spel) | 40 min |
| `test-node prepare` alone (sequencer only) | 20–25 min |
| First `build --guest local` of a project (also installs the template's Rust 1.92.0 through rustup) | 9 min |
| First docker guest build | pulls a ~1.7 GB image, then several minutes |
| First `build idl` of a spel project | 5 min |
| First spel `build` | 5–15 min |
| `deploy` of the default template (five programs, one per block) | 3 min |
| Cold `basecamp setup` without the trusted binary cache | builds a Qt closure |

Everything after the first build on a cache root is seconds to a few minutes, which is why
the run plan keeps one cache root.

### Environment variables
`LOGOS_SCAFFOLD_CACHE_ROOT`, `LOGOS_SCAFFOLD_WALLET_PASSWORD`, `LOGOS_BLOCKCHAIN_CIRCUITS`
(local circuits override), `LOGOS_SCAFFOLD_QUIET`, `LOGOS_SCAFFOLD_PRINT_OUTPUT`,
`LOGOS_SCAFFOLD_DEPLOY_PACING_TIMEOUT_MS`, and `RISC0_HOME`.

No credentials are needed. A `GH_TOKEN` helps only to avoid GitHub API rate limits while
provisioning.

## Environments and side effects
- **Local only:** everything runs against project-local state. The localnet sequencer
  listens on `127.0.0.1:<[localnet].port>` (default 3040), and test nodes on `127.0.0.1` too: the sequencer RPC has no
  caller authentication. Test nodes use their own ports
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
  Docker Hub (`risczero/risc0-guest-builder`). `new` clones LEZ on first use of a cache root,
  and `setup`, `build` and `run` fetch the pinned repos every time. It does no other outbound calls and has no
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
