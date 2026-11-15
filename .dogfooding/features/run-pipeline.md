---
feature: run-pipeline
prefix: RUN
surface: cli
scenarios:
  - id: RUN-001
    title: Bare run takes a fresh project to deployed programs
    priority: P0
    provenance: documented
    source: DOGFOODING.md@740e5e9 D7
    depends_on: [CREATE-001]
  - id: RUN-002
    title: Re-run reuses localnet and skips an unchanged deploy
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D7
    depends_on: [RUN-001]
  - id: RUN-003
    title: Post-deploy hooks get the documented environment
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D7
    depends_on: [RUN-001]
  - id: RUN-004
    title: Override or skip hooks from the command line
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D7
    depends_on: [RUN-003]
  - id: RUN-005
    title: Profile with deploy disabled
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D7
    depends_on: [RUN-001]
  - id: RUN-006
    title: Profile with topup disabled
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D7
    depends_on: [RUN-001]
  - id: RUN-007
    title: Reset before the run
    priority: P2
    provenance: documented
    source: docs/commands.md
    depends_on: [RUN-001]
  - id: RUN-008
    title: Watch mode re-runs on matching changes
    priority: P2
    provenance: documented
    source: docs/configuration.md
    depends_on: [RUN-001]
  - id: RUN-009
    title: Run summary names the wallet's sequencer
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D7
    depends_on: [RUN-001]
  - id: RUN-010
    title: Hooks get the program id and guest path on a single-program project
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D7
    depends_on: [RUN-003]
retired: []
---

# Run pipeline

`lgs run` is the one-command inner loop: build (which chains setup) → IDL → ensure localnet
→ wallet topup → deploy → optional `[run].post_deploy` hooks. Every phase prints a numbered
header (`[n/N]`). Steps never renumber: a skipped step keeps its slot and says why. Related:
[Deploy](deploy.md), [Localnet](localnet.md), [Wallet](wallet.md),
[spel template](spel-template.md).

## RUN-001 — Bare run takes a fresh project to deployed programs

### Expected
In a freshly created project with no `[run]` section, `lgs run` prints `[1/5] Building...`,
`[2/5] Building IDL...`, `[3/5] Ensuring localnet...` (then `localnet ready (sequencer
pid=…)`), `[4/5] Topping up wallet...` and `[5/5] Deploying programs...` with `program_id:`
lines. It ends with a `Deployed programs:` summary and a `Sequencer:` line naming the wallet's
target (`http://127.0.0.1:3040` with the LEZ debug config on the default port; see RUN-009).
It seeds the default wallet once localnet is up if `setup` deferred it. It also works from a
subdirectory and anchors at the project root.

### Preconditions
CREATE-001, nothing on port 3040, Docker plus `cargo-risczero` (or `[build].guest = "local"`),
and r0vm provisioned.

### Steps
1. `cd "$SCRATCH_ROOT/dogfood-default" && "$SCAFFOLD_BIN" run`
2. `cd src && "$SCAFFOLD_BIN" run --no-post-deploy; cd ..`

### Success criteria
- Step 1 exits 0, prints all five headers in order, and ends with the deployed-programs
  summary.
- Step 2 builds and reaches the deploy step from the project root (no "not a project" error);
  the deploy itself is dedup-skipped there, because step 1 just deployed the same build.

## RUN-002 — Re-run reuses localnet and skips an unchanged deploy

### Expected
A second `run` prints `localnet already running (sequencer pid=…)` and does not restart the
sequencer. When guest binaries, IDL, config and the sequencer instance are all unchanged,
step 5 prints `[5/N] Deploy skipped (guest binaries + IDL + config + sequencer unchanged;
pass --reset ...)`.

### Preconditions
RUN-001 with localnet still running.

### Steps
1. `"$SCAFFOLD_BIN" localnet status --json | jq .tracked_pid`
2. `"$SCAFFOLD_BIN" run`
3. Repeat step 1.

### Success criteria
- The PID is the same before and after, and the `Deploy skipped (… unchanged …)` header
  appears.

### Notes
To force a real re-deploy, pass `--reset` or delete `.scaffold/state/run_deploy.json`.

## RUN-003 — Post-deploy hooks get the documented environment

### Expected
With `[run].post_deploy` set, `run` prints `[6/6] Running N post-deploy hook(s)` and frames
each hook with `===> post_deploy[i/n]:` and `<=== post_deploy[i/n] OK`. Hooks run via
`sh -c` with cwd at the project root. They see `SEQUENCER_URL`, `NSSA_WALLET_HOME_DIR` and
`LEE_WALLET_HOME_DIR` (same path), `SCAFFOLD_PROJECT_ROOT`, `SCAFFOLD_IDL_DIR`, and
`SCAFFOLD_TOPUP_SKIPPED`/`SCAFFOLD_DEPLOY_SKIPPED` (`1`/`0`, always set). For a
single-program project they also see `SCAFFOLD_PROGRAM_ID` and `SCAFFOLD_GUEST_BIN`. A
non-zero hook aborts with `post-deploy hook exited with status N`.

### Preconditions
RUN-001.

### Steps
1. Add this to `scaffold.toml`:
   ```toml
   [run]
   post_deploy = [
     "pwd",
     "echo seq=$SEQUENCER_URL idl=$SCAFFOLD_IDL_DIR root=$SCAFFOLD_PROJECT_ROOT",
     "echo nssa=$NSSA_WALLET_HOME_DIR lee=$LEE_WALLET_HOME_DIR",
     "echo topup=$SCAFFOLD_TOPUP_SKIPPED deploy=$SCAFFOLD_DEPLOY_SKIPPED",
     "echo pid=${SCAFFOLD_PROGRAM_ID:-unset} bin=${SCAFFOLD_GUEST_BIN:-unset}",
   ]
   ```
2. `"$SCAFFOLD_BIN" run`
3. `"$SCAFFOLD_BIN" run --post-deploy "exit 3"; echo "exit=$?"`

### Success criteria
- `pwd` prints the project root, and every variable is non-empty. `nssa` equals `lee`.
  `topup` and `deploy` are `0` or `1` and match the headers printed in the same run.
- After RUN-002 nothing changed, so step 2's deploy is dedup-skipped
  (`Deploy skipped (… unchanged …)`). The hooks still run, and `deploy=1`.
- `seq` is `http://127.0.0.1:<[localnet].port>`, even if the wallet config names another
  address (see RUN-009).
- The default template has several programs, so `SCAFFOLD_PROGRAM_ID` and
  `SCAFFOLD_GUEST_BIN` print `unset`.
- Step 3 exits non-zero with `post-deploy hook exited with status 3`.

## RUN-004 — Override or skip hooks from the command line

### Expected
`--post-deploy <cmd>` (repeatable) replaces `[run].post_deploy` for one run.
`--no-post-deploy` skips hooks and prints the deployed-programs summary instead. Passing both
fails at argument parsing with clap's `cannot be used with`.

### Preconditions
RUN-003 (with `[run].post_deploy` configured).

### Steps
1. `"$SCAFFOLD_BIN" run --post-deploy "echo override"`
2. `"$SCAFFOLD_BIN" run --no-post-deploy`
3. `"$SCAFFOLD_BIN" run --post-deploy x --no-post-deploy; echo "exit=$?"`

### Success criteria
- Step 1 runs only `echo override`.
- Step 2 runs no hooks and ends with the `Deployed programs:` summary and `Sequencer:` line.
- Step 3 exits non-zero with `cannot be used with` and does no build work.

## RUN-005 — Profile with deploy disabled

### Expected
`[run.profiles.<p>] deploy = false` makes `run --profile <p>` print ``[5/6] Deploy skipped
(`deploy = false` in the run profile; ...)``, skip deploy work entirely, and still run hooks.
Hooks see `SCAFFOLD_DEPLOY_SKIPPED=1`.

### Preconditions
RUN-001.

### Steps
1. Add this to `scaffold.toml`:
   ```toml
   [run.profiles.self-deploy]
   deploy = false
   post_deploy = ["echo deploy skipped: $SCAFFOLD_DEPLOY_SKIPPED"]
   ```
2. `"$SCAFFOLD_BIN" run --profile self-deploy`

### Success criteria
- The step 5 header says skipped because of `deploy = false`, and the hook prints
  `deploy skipped: 1`.

## RUN-006 — Profile with topup disabled

### Expected
`[run.profiles.<p>] topup = false` prints ``[4/6] Topup skipped (`topup = false` in the run
profile; ...)``, makes no wallet call, and still deploys and runs hooks with
`SCAFFOLD_TOPUP_SKIPPED=1`. Without the profile, the same hook sees `0`.

### Preconditions
RUN-001.

### Steps
1. Add `[run.profiles.self-fund]` with `topup = false` and
   `post_deploy = ["echo topup skipped: $SCAFFOLD_TOPUP_SKIPPED"]`.
2. `"$SCAFFOLD_BIN" run --profile self-fund`
3. `"$SCAFFOLD_BIN" run --post-deploy 'echo "topup skipped:" $SCAFFOLD_TOPUP_SKIPPED'`

### Success criteria
- Step 2 prints the skipped step 4 header and `topup skipped: 1`.
- Step 3 prints `topup skipped: 0`.

## RUN-007 — Reset before the run

### Expected
`run --reset` wipes sequencer state and the wallet, re-seeds, and then runs the full pipeline
with a real deploy. `--no-reset` overrides a reset set in config. `--reset` and `--no-reset`
conflict.

### Preconditions
RUN-001, in a scratch project.

### Steps
1. `"$SCAFFOLD_BIN" run --reset`
2. `"$SCAFFOLD_BIN" run --reset --no-reset; echo "exit=$?"`

### Success criteria
- Step 1 exits 0 with a real `[5/5] Deploying programs...`, not `Deploy skipped`.
- Step 2 is rejected at argument parsing.

## RUN-008 — Watch mode re-runs on matching changes

### Expected
`run --watch` runs the pipeline, then re-runs it on file changes, reusing localnet and
skipping reset on re-runs. A change triggers a re-run only if it matches a `[run.watch]
include` glob (or `include` is unset) and no `exclude` glob. `.scaffold`, `target`, `.git` and
the IDL dir are always ignored. `--watch-debounce-ms` requires `--watch` and overrides
`[run.watch].debounce_ms` (default 500 ms).

### Preconditions
RUN-001.

### Steps
1. Add `[run.watch]` with `exclude = ["**/*.md"]`.
2. Start `"$SCAFFOLD_BIN" run --watch` in the background and wait for the first pipeline to
   finish: the log ends with `===> watching <root> for changes`. Count pipelines with
   `grep -c '^\[1/' <log>`.
3. `touch README.md`, wait 2 seconds, then edit a comment in `methods/guest/src/bin/hello_world.rs`.
4. Stop the watcher (`pgrep -x logos-scaffold | xargs kill`; `pkill -f` also matches the shell that
   runs it). Run `"$SCAFFOLD_BIN" run --watch-debounce-ms 100; echo "exit=$?"`.

### Success criteria
- The README touch triggers nothing, and the guest edit triggers exactly one new pipeline
  run.
- Step 4 is rejected because `--watch` is missing.

## RUN-009 — Run summary names the wallet's sequencer

### Expected
The `Sequencer:` line at the end of `run`'s deploy summary names the address the wallet
targets, which is the one deploy used. It is the same address in either wallet config shape,
and falls back to `127.0.0.1:<[localnet].port>` only when the wallet config names none. The
post-deploy hook's `SEQUENCER_URL` stays `http://127.0.0.1:<[localnet].port>`, as
`docs/configuration.md` documents.

### Preconditions
RUN-001.

### Steps
1. `"$SCAFFOLD_BIN" run --no-post-deploy 2>&1 | grep '^Sequencer:'` (the summary prints only
   when no hooks run)
2. `jq -r '.sequencers[0].sequencer_addr // .sequencer_addr' .scaffold/wallet/wallet_config.json`

### Success criteria
- Step 1's address equals step 2's.

## RUN-010 — Hooks get the program id and guest path on a single-program project

### Expected
When the project has exactly one guest program, hooks also see `SCAFFOLD_PROGRAM_ID` (the
deployed program's risc0 image ID) and `SCAFFOLD_GUEST_BIN` (the absolute path to its guest
binary). Unset on a single-program project with the vendored `spel` built is the regression.
A missing `spel` binary shows as `program_id: unavailable` in the deploy summary.

### Preconditions
RUN-003, in a copy of the project with every program but `hello_world` removed from
`methods/guest/src/bin/` **and** the clients that import the removed programs deleted from
`src/bin/` (keep `run_hello_world.rs` and `run_hello_world_private.rs`; every other `run_*.rs`
imports another program's `*_ELF` constant, and `cargo build --workspace` fails with
`unresolved import example_program_deployment_methods::HELLO_WORLD_WITH_MOVE_FUNCTION_ELF`
otherwise).

### Steps
1. `"$SCAFFOLD_BIN" run --post-deploy 'echo pid=${SCAFFOLD_PROGRAM_ID:-unset} bin=${SCAFFOLD_GUEST_BIN:-unset}'`
2. `"$SCAFFOLD_BIN" deploy hello_world --json | jq -r '.deploys[0].program_id'`

### Success criteria
- `pid` is a 64-hex image ID equal to step 2's `program_id`.
- `bin` is an absolute path to an existing `hello_world.bin`.
