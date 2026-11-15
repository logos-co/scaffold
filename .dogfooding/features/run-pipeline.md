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
    title: Re-run reuses localnet, skips an unchanged deploy and names the wallet's sequencer
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D7
    depends_on: [RUN-001]
  - id: RUN-003
    title: Post-deploy hooks get the documented environment and can be overridden
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D7
    depends_on: [RUN-001]
  - id: RUN-005
    title: Run profiles skip deploy or topup
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
  - id: RUN-010
    title: Hooks get the program id and guest path on a single-program project
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D7
    depends_on: [RUN-001]
retired: [RUN-004, RUN-006, RUN-009]
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
lines. It ends with a `Deployed programs:` summary and a `Sequencer:` line. It seeds the
default wallet once localnet is up if `setup` deferred it. It also works from a subdirectory
and anchors at the project root.

### Preconditions
A fresh CREATE-001 project (not the one other features already built), nothing on port 3040,
Docker plus `cargo-risczero` (or `[build].guest = "local"`), and r0vm provisioned.

### Steps
1. `cd "$SCRATCH_ROOT" && "$SCAFFOLD_BIN" new dogfood-run && cd dogfood-run && "$SCAFFOLD_BIN" run`
2. `( cd src && "$SCAFFOLD_BIN" run --no-post-deploy )`

### Success criteria
- Step 1 exits 0, prints all five headers in order, and ends with the deployed-programs
  summary.
- Step 2 prints the same five headers from the project root (no "not a project" error); its
  deploy is dedup-skipped, because step 1 just deployed the same build.

## RUN-002 — Re-run reuses localnet, skips an unchanged deploy and names the wallet's sequencer

### Expected
A second `run` prints `localnet already running (sequencer pid=…)` and does not restart the
sequencer. When guest binaries, IDL, config and the sequencer instance are all unchanged,
step 5 prints `[5/N] Deploy skipped (guest binaries + IDL + config + sequencer unchanged;
pass --reset ...)`. The summary's `Sequencer:` line names the address the wallet targets (the
one deploy used) in either wallet config shape, and falls back to
`127.0.0.1:<[localnet].port>` only when the wallet config names none.

### Preconditions
RUN-001 with localnet still running.

### Steps
1. `"$SCAFFOLD_BIN" localnet status --json | jq .tracked_pid`
2. `"$SCAFFOLD_BIN" run --no-post-deploy 2>&1 | tee /tmp/run2.log | grep -E 'already running|Deploy skipped|^Sequencer:'`
3. Repeat step 1, then
   `jq -r '.sequencers[0].sequencer_addr // .sequencer_addr' .scaffold/wallet/wallet_config.json`

### Success criteria
- The PID is the same before and after, and both the `already running` and the `Deploy
  skipped (… unchanged …)` lines appear.
- The `Sequencer:` address equals step 3's `jq` output.

### Notes
To force a real re-deploy, pass `--reset` or delete `.scaffold/state/run_deploy.json`. The
`Sequencer:` summary prints only when no hooks run.

## RUN-003 — Post-deploy hooks get the documented environment and can be overridden

### Expected
With `[run].post_deploy` set, `run` prints `[6/6] Running N post-deploy hook(s)` and frames
each hook with `===> post_deploy[i/n]:` and `<=== post_deploy[i/n] OK`. Hooks run via
`sh -c` with cwd at the project root. They see `SEQUENCER_URL`
(`http://127.0.0.1:<[localnet].port>`, even if the wallet config names another address),
`NSSA_WALLET_HOME_DIR` and `LEE_WALLET_HOME_DIR` (same path), `SCAFFOLD_PROJECT_ROOT`,
`SCAFFOLD_IDL_DIR`, and `SCAFFOLD_TOPUP_SKIPPED`/`SCAFFOLD_DEPLOY_SKIPPED` (`1`/`0`, always
set). `SCAFFOLD_PROGRAM_ID` and `SCAFFOLD_GUEST_BIN` are set only for a single-program
project (RUN-010). A non-zero hook aborts with `post-deploy hook exited with status N`.
`--post-deploy <cmd>` (repeatable) replaces the configured hooks for one run,
`--no-post-deploy` skips them and prints the deployed-programs summary instead, and passing
both fails at argument parsing.

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
3. `"$SCAFFOLD_BIN" run --post-deploy "echo override" --post-deploy "exit 3"; echo "exit=$?"`
4. `"$SCAFFOLD_BIN" run --no-post-deploy | tail -4; "$SCAFFOLD_BIN" run --post-deploy x --no-post-deploy; echo "exit=$?"`

### Success criteria
- Step 2 runs five framed hooks. `pwd` prints the project root, and every variable but the
  last pair is non-empty. `nssa` equals `lee`. `topup`/`deploy` are `0` or `1` and match the
  headers of the same run (`deploy=1` when the deploy was dedup-skipped). `seq` is
  `http://127.0.0.1:<[localnet].port>`. The default template has several programs, so `pid`
  and `bin` print `unset`.
- Step 3 runs only the two command-line hooks: it prints `override`, then exits non-zero with
  `post-deploy hook exited with status 3`.
- Step 4: the first run has no hooks and ends with `Deployed programs:` and `Sequencer:`. The
  second exits non-zero with `cannot be used with`, before any build work.

## RUN-005 — Run profiles skip deploy or topup

### Expected
`[run.profiles.<p>] deploy = false` makes `run --profile <p>` print ``[5/6] Deploy skipped
(`deploy = false` in the run profile; ...)``, skip deploy work entirely, and still run hooks
with `SCAFFOLD_DEPLOY_SKIPPED=1`. `topup = false` prints ``[4/6] Topup skipped (`topup =
false` in the run profile; ...)``, makes no wallet call, and still deploys and runs hooks
with `SCAFFOLD_TOPUP_SKIPPED=1`. A profile shadows the inline `[run]` values entirely.
`[run].default_profile = "<p>"` selects a profile when no `--profile` is passed. An unknown
`--profile` fails listing the known profiles, and a `default_profile` with no matching
section is an `invalid scaffold.toml` error for every command.

### Preconditions
RUN-001. Back up `scaffold.toml`.

### Steps
1. Add this to `scaffold.toml`:
   ```toml
   [run.profiles.self-deploy]
   deploy = false
   post_deploy = ["echo topup=$SCAFFOLD_TOPUP_SKIPPED deploy=$SCAFFOLD_DEPLOY_SKIPPED"]

   [run.profiles.self-fund]
   topup = false
   post_deploy = ["echo topup=$SCAFFOLD_TOPUP_SKIPPED deploy=$SCAFFOLD_DEPLOY_SKIPPED"]
   ```
2. `"$SCAFFOLD_BIN" run --profile self-deploy 2>&1 | grep -E '^\[[45]/|^topup='`
3. Add `default_profile = "self-fund"` under `[run]` (create the table if it is missing), then
   `"$SCAFFOLD_BIN" run 2>&1 | grep -E '^\[[45]/|^topup='`
4. `"$SCAFFOLD_BIN" run --profile nope; echo "exit=$?"`, then set `default_profile = "nope"`
   and `"$SCAFFOLD_BIN" run; echo "exit=$?"`. Restore `scaffold.toml`.

### Success criteria
- Step 2: `[4/6] Topping up wallet...`, the `[5/6] Deploy skipped` header naming `deploy =
  false`, and `topup=0 deploy=1`.
- Step 3 (no `--profile`): the `[4/6] Topup skipped` header naming `topup = false`, a step 5
  that deploys or dedup-skips, and `topup=1`.
- Step 4: the first fails with `scaffold.toml has no [run.profiles.nope] section. Known
  profiles: [self-deploy, self-fund]`, the second with `invalid scaffold.toml:
  [run].default_profile = "nope" but no [run.profiles.nope] section`.

## RUN-007 — Reset before the run

### Expected
`run --reset` wipes sequencer state and the wallet, re-seeds, and then runs the full pipeline
with a real deploy. `--no-reset` overrides a reset set in config. `--reset` and `--no-reset`
conflict.

### Preconditions
RUN-001, in a scratch project.

### Steps
1. `"$SCAFFOLD_BIN" run --reset --no-reset; echo "exit=$?"`
2. `"$SCAFFOLD_BIN" run --reset --no-post-deploy`

### Success criteria
- Step 1 is rejected at argument parsing.
- Step 2 exits 0, prints `[3/5] Resetting localnet (wipes sequencer + wallet)...`, and has a
  real `[5/5] Deploying programs...`, not `Deploy skipped`.

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
1. Add `[run.watch]` with `exclude = ["**/*.md"]`, then
   `"$SCAFFOLD_BIN" run --watch-debounce-ms 100; echo "exit=$?"`.
2. Start `"$SCAFFOLD_BIN" run --watch --no-post-deploy > /tmp/watch.log 2>&1 &` and wait for
   `===> watching <root> for changes` in the log.
3. `touch README.md; sleep 5; grep -c '^\[1/' /tmp/watch.log`, then add a comment line to
   `methods/guest/src/bin/hello_world_with_move_function.rs`, wait for the second
   `Sequencer:` summary line in the log (the re-run is done; `===> watching` is printed
   only once), and count again.
4. Stop the watcher with `pgrep -x logos-scaffold | xargs kill`, and revert the edit.

### Success criteria
- Step 1 is rejected because `--watch` is missing.
- The count is `1` after the README touch and `2` after the guest edit (`===> change
  detected, re-running pipeline` appears once).

### Notes
Edit a program that no other program calls. A change to a tail-call callee (`hello_world`,
`hello_world_with_authorization`) changes its program id, `build` rewrites the id embedded in
the caller's source, and the watcher reacts to that write with one more re-run.

## RUN-010 — Hooks get the program id and guest path on a single-program project

### Expected
When the project has exactly one guest program, hooks also see `SCAFFOLD_PROGRAM_ID` (the
deployed program's risc0 image ID) and `SCAFFOLD_GUEST_BIN` (the absolute path to its guest
binary). A missing `spel` binary shows as `program_id: unavailable` in the deploy summary.

### Preconditions
RUN-001, in a copy of the project reduced to one program: delete every file but
`hello_world.rs` from `methods/guest/src/bin/`, and every client but `run_hello_world.rs` and
`run_hello_world_private.rs` from `src/bin/` (the other `run_*.rs` import the removed
programs' `*_ELF` constants and `cargo build --workspace` would fail with `unresolved
import`).

### Steps
1. `"$SCAFFOLD_BIN" run --post-deploy 'echo pid=${SCAFFOLD_PROGRAM_ID:-unset} bin=${SCAFFOLD_GUEST_BIN:-unset}'`
2. `"$SCAFFOLD_BIN" deploy hello_world --json | jq -r '.deploys[0].program_id'`

### Success criteria
- `pid` is a 64-hex image ID equal to step 2's `program_id`.
- `bin` is an absolute path to an existing `hello_world.bin`.
