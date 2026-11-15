---
feature: deploy
prefix: DEPLOY
surface: cli
scenarios:
  - id: DEPLOY-001
    title: Deploy all discovered programs
    priority: P0
    provenance: documented
    source: DOGFOODING.md@740e5e9 D1
    depends_on: [BUILD-002, LOCALNET-001]
  - id: DEPLOY-002
    title: Deploy one program by name or path, with JSON output
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D3
    depends_on: [DEPLOY-001]
  - id: DEPLOY-005
    title: Unknown program or missing binaries give an actionable error
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D3
    depends_on: [DEPLOY-001]
  - id: DEPLOY-006
    title: Unreachable sequencer fails fast with a hint
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D3
    depends_on: [DEPLOY-001]
  - id: DEPLOY-007
    title: Deploy ranks only the owned docker artefact first
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D8
    depends_on: [DEPLOY-001]
retired: [DEPLOY-003, DEPLOY-004, DEPLOY-008]
---

# Deploy

`deploy` finds guest binaries for the programs in `methods/guest/src/bin/*.rs` and submits
them through the project wallet. For each submission it prints the locally computed
`program_id` (the risc0 image ID, via the vendored `spel`). Discovery ranks the deterministic
`docker/` artefact first (only in the root the project kind owns), then `release/`, then
debug. Multi-program deploys are paced one program per block. Related:
[Setup and build](setup-and-build.md), [Run pipeline](run-pipeline.md),
[Example runners](example-runners.md).

BUILD-002 is the build these scenarios expect. On a host without Docker, BUILD-005's local
build stands in: every `docker/` path below then reads `target/riscv-guest/…/release/`, and
the report says so.

## DEPLOY-001 — Deploy all discovered programs

### Expected
Bare `deploy` submits every discovered program and prints `OK  <program> submitted` and
`program_id: <hex>` for each. Between programs it prints `Waiting for a new block past N
before the next deployment ...`. It ends with a summary with zero failures. The echoed
`wallet deploy-program <path>` names the `target/riscv-guest-docker/.../docker/` artefact
after a default build.

### Preconditions
BUILD-002 and LOCALNET-001.

### Steps
1. `"$SCAFFOLD_BIN" deploy; "$SCAFFOLD_BIN" localnet status --json | jq .ready`

### Success criteria
- Exit 0, one `OK` line and one `program_id:` line per program, and `Failed: 0`.
- Localnet is still ready afterwards.

### Notes
Pacing exists because the pinned LEZ panics on blocks over about 896 KiB: expect roughly 15
seconds per extra program. A stalled head aborts with `deploy pacing aborted …` and a
non-zero exit; re-run `deploy` once the sequencer recovers, or raise
`LOGOS_SCAFFOLD_DEPLOY_PACING_TIMEOUT_MS` for slow block intervals. A deploy that continues
unpaced and crashes localnet is the regression.

## DEPLOY-002 — Deploy one program by name or path, with JSON output

### Expected
`deploy <name>` submits only that program. `deploy --program-path <bin>` prints a human `OK`
line with the path. With `--json`, stdout is JSON only, with no command echoes: `{"deploys":
[…]}` for discovery deploys (bare or by name) and one flat object for `--program-path`. Each
entry is `{"status":"submitted","program":…,"program_id":…}`; absent fields (today always
`tx`) are omitted, not `null`. A failed entry carries `error` instead of `program_id`, the
exit code is then non-zero, and the JSON is still printed.

### Preconditions
DEPLOY-001.

### Steps
1. `"$SCAFFOLD_BIN" deploy hello_world`
2. `"$SCAFFOLD_BIN" deploy hello_world --json | jq -e '.deploys | length == 1 and (.[0] | .status and .program and .program_id and (has("tx")|not))'`
3. `BIN=$(find target/riscv-guest-docker target/riscv-guest -name hello_world.bin | head -1); "$SCAFFOLD_BIN" deploy --program-path "$BIN"`
4. `"$SCAFFOLD_BIN" deploy --program-path "$BIN" --json | jq -e '.status and .program and (has("tx")|not)'`

### Success criteria
- Step 1 prints exactly one `OK  hello_world submitted` line, a `program_id:`, and a
  `Summary:` with `Succeeded: 1`, `Failed: 0`.
- Step 3 prints a human `OK` line naming `$BIN`.
- Steps 2 and 4 parse as pure JSON and the `jq -e` checks exit 0. (A bare `deploy --json`
  has the same shape with one entry per program; BUILD-004 uses it.)

## DEPLOY-005 — Unknown program or missing binaries give an actionable error

### Expected
`deploy <unknown>` fails and lists the discovered program names. With no guest artefacts,
`deploy` fails, points back to `logos-scaffold build`, and lists the searched roots in order:
`target/riscv-guest-docker`, `target/riscv-guest`, `methods/target`, `methods/guest/target`.
Nothing was submitted, so no `Note: Submission confirmed by wallet exit status …` line is
printed.

### Preconditions
DEPLOY-001 with localnet up.

### Steps
1. `"$SCAFFOLD_BIN" deploy nonexistent_program; echo "exit=$?"`
2. Move the artefacts away instead of deleting them, so nothing has to be rebuilt:
   `mkdir /tmp/guest-bak && mv target/riscv-guest-docker target/riscv-guest /tmp/guest-bak/ 2>/dev/null; "$SCAFFOLD_BIN" deploy; echo "exit=$?"; mv /tmp/guest-bak/* target/`

### Success criteria
- Step 1 exits non-zero, and the output lists `hello_world` among the available programs.
- Step 2 exits non-zero with ``missing binary `hello_world.bin` (searched: …)`` naming all
  four roots in that order, suggests `build`, and has no `Submission confirmed` note.

## DEPLOY-006 — Unreachable sequencer fails fast with a hint

### Expected
Before submitting, `deploy` probes the sequencer over RPC (`getLastBlockId`) at the address
the wallet itself targets: `sequencers[0].sequencer_addr` in the wallet config from LEZ
v0.2.4, or the top-level `sequencer_addr` before that. With nothing listening (refused) and
with an unrouted address (blackholed), it fails in about a second with a
sequencer-unavailable hint naming that address. It never falls through to a vague wallet
error or a long hang.

### Preconditions
DEPLOY-001. Back up `.scaffold/wallet/wallet_config.json`.

### Steps
1. `"$SCAFFOLD_BIN" localnet stop; "$SCAFFOLD_BIN" deploy; echo "exit=$?"`
2. `curl -m 3 http://10.255.255.1:3040; echo "curl=$?"`
3. Set `sequencers[0].sequencer_addr` to `http://10.255.255.1:3040`, run
   `time "$SCAFFOLD_BIN" deploy; echo "exit=$?"`, restore the config, and
   `"$SCAFFOLD_BIN" localnet start`.

### Success criteria
- Step 1 exits non-zero at once with `cannot deploy programs: …` and `sequencer appears
  unavailable at http://127.0.0.1:<port>` plus ``Run `logos-scaffold localnet start`.``
- Step 3 exits non-zero in a few seconds, and its hint names `http://10.255.255.1:3040`. A
  hint naming `127.0.0.1:<[localnet].port>` means scaffold probed the default port instead of
  the wallet's target.
- No `reachability probe failed …; continuing` line appears.

### Notes
Step 2 must time out (`curl=28`). A sandbox that refuses the address at once makes step 3's
timing pass for the wrong reason: note it in the report. The probe tells "refused or
unreachable" from "something answered". It does not verify that the responder is this
project's sequencer, so a foreign process on the configured port suppresses the hint.

## DEPLOY-007 — Deploy ranks only the owned docker artefact first

### Expected
A `docker/` artefact under `target/riscv-guest-docker` (the root a `default` project owns)
outranks `release/`. One under `methods/guest/target/` (a root scaffold does not own for this
project kind) never does: `deploy` ships the owned artefact and prints `note: ignoring
…/methods/guest/target/…/docker/hello_world.bin` on stderr.

### Preconditions
DEPLOY-001.

### Steps
1. `mkdir -p methods/guest/target/riscv32im-risc0-zkvm-elf/docker && cp <another program>.bin methods/guest/target/riscv32im-risc0-zkvm-elf/docker/hello_world.bin`
2. `"$SCAFFOLD_BIN" deploy hello_world 2>&1 | grep -E 'deploy-program|note: ignoring'; rm -rf methods/guest/target`
3. Only after a local build (no `target/riscv-guest-docker`): copy the `release/`
   `hello_world.bin` into `target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker/`, run
   `"$SCAFFOLD_BIN" deploy hello_world 2>&1 | grep deploy-program`, then remove
   `target/riscv-guest-docker`.

### Success criteria
- Step 2: the echoed `wallet deploy-program` path is the project's own build
  (`target/riscv-guest-docker/…/docker/` or `target/riscv-guest/…/release/`), and the `note:
  ignoring` line names the planted file.
- Step 3: the echoed path is the `docker/` copy.

### Notes
`lgs build` never clears `methods/guest/target`, so a planted or left-over ELF there would
win every later deploy if it were ranked. After a docker build, DEPLOY-001's echoed path
already shows the `docker/` ranking and step 3 is redundant.
