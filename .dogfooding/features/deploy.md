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
    title: Deploy one program by name
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D3
    depends_on: [DEPLOY-001]
  - id: DEPLOY-003
    title: Deploy an explicit binary with JSON output
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D3
    depends_on: [DEPLOY-001]
  - id: DEPLOY-004
    title: Discovery deploy with JSON output
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D3
    depends_on: [DEPLOY-001]
  - id: DEPLOY-005
    title: Unknown program name lists available programs
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D3
    depends_on: [BUILD-002]
  - id: DEPLOY-006
    title: Unreachable sequencer fails fast with a hint
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D3
    depends_on: [BUILD-002]
  - id: DEPLOY-007
    title: Stale docker artefacts outside the owned root are ignored
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D8
    depends_on: [DEPLOY-001]
  - id: DEPLOY-008
    title: Missing binaries name every search root
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D3
    depends_on: [LOCALNET-001]
retired: []
---

# Deploy

`deploy` finds guest binaries for the programs in `methods/guest/src/bin/*.rs` and submits
them through the project wallet. For each submission it prints the locally computed
`program_id` (the risc0 image ID, via the vendored `spel`). Discovery ranks the deterministic
`docker/` artefact first (only in the root the project kind owns), then `release/`, then
debug. Multi-program deploys are paced one program per block. Related:
[Setup and build](setup-and-build.md), [Run pipeline](run-pipeline.md),
[Example runners](example-runners.md).

## DEPLOY-001 — Deploy all discovered programs

### Expected
Bare `deploy` submits every discovered program and prints `OK  <program> submitted` and
`program_id: <hex>` for each. It ends with a summary with zero failures. Between programs it
prints `Waiting for a new block past N before the next deployment ...`. The echoed `wallet
deploy-program <path>` names the `target/riscv-guest-docker/.../docker/` artefact after a
default build.

### Preconditions
BUILD-002 and LOCALNET-001.

### Steps
1. `"$SCAFFOLD_BIN" deploy`

### Success criteria
- Exit 0, one `OK` line and one `program_id:` line per program, and zero failures in the
  summary.
- localnet is still running afterwards (`localnet status --json | jq .ready` → `true`).

### Notes
Pacing exists because the pinned LEZ panics on blocks over about 896 KiB. Expect roughly 15
seconds per extra program. A stalled head aborts with `deploy pacing aborted …` and a
non-zero exit.
After an aborted pacing run, re-run `deploy` for the remaining programs once the sequencer
recovers, or raise `LOGOS_SCAFFOLD_DEPLOY_PACING_TIMEOUT_MS` for slow block intervals. A
deploy that continues unpaced and crashes localnet is the regression. If the upstream block
cap is lifted and pacing becomes dead weight, record that too.

## DEPLOY-002 — Deploy one program by name

### Expected
`deploy <name>` submits only that program.

### Preconditions
DEPLOY-001.

### Steps
1. `"$SCAFFOLD_BIN" deploy hello_world`

### Success criteria
- Exactly one `OK  hello_world submitted` line, plus a `program_id:`.
- It ends with the `Summary:` block: `Succeeded: 1`, `Failed: 0` and a `hello_world: submitted`
  result line.

## DEPLOY-003 — Deploy an explicit binary with JSON output

### Expected
`deploy --program-path <bin>` prints a human `OK` line with the path. With `--json`, stdout is
only one flat object: `{"status":"submitted","program":…,"program_id"?:…}`. It has no
command echoes, and absent fields (today always `tx`) are omitted, not `null`.

### Preconditions
DEPLOY-001.

### Steps
1. `BIN=$(find target/riscv-guest-docker target/riscv-guest -name hello_world.bin | head -1)`
2. `"$SCAFFOLD_BIN" deploy --program-path "$BIN"`
3. `"$SCAFFOLD_BIN" deploy --program-path "$BIN" --json | jq -e '.status and .program and (has("tx")|not)'`

### Success criteria
- Step 2 prints a human-readable `OK` line naming `$BIN`.
- Step 3 parses and the `jq -e` check exits 0. `program_id` is present once `setup` has built
  spel.

## DEPLOY-004 — Discovery deploy with JSON output

### Expected
`deploy --json` and `deploy <name> --json` print only `{"deploys":[…]}`, with one entry per
attempted program in the same shape as DEPLOY-003. A failed entry carries `error` instead of
`program_id`. The exit code is non-zero if any entry failed, and the JSON is still printed.

### Preconditions
DEPLOY-001.

### Steps
1. `"$SCAFFOLD_BIN" deploy --json | jq -e '.deploys | length > 0'`
2. `"$SCAFFOLD_BIN" deploy hello_world --json | jq '.deploys[0]'`

### Success criteria
- Both outputs are parseable JSON on stdout, with no `null` values and no human text mixed in.

## DEPLOY-005 — Unknown program name lists available programs

### Expected
`deploy <unknown>` fails and lists the discovered program names.

### Preconditions
BUILD-002.

### Steps
1. `"$SCAFFOLD_BIN" deploy nonexistent_program; echo "exit=$?"`

### Success criteria
- Non-zero exit, and the output includes `hello_world`.

## DEPLOY-006 — Unreachable sequencer fails fast with a hint

### Expected
Before submitting, `deploy` probes the sequencer over RPC (`getLastBlockId`) at the address
the wallet itself targets. That is `sequencers[0].sequencer_addr` in the wallet config from
LEZ v0.2.4, or the top-level `sequencer_addr` before that. With nothing listening (refused)
and with an unrouted address (blackholed), it fails in about a second with a
sequencer-unavailable hint naming that address. It never falls through to a vague wallet
error or a long hang.

### Preconditions
BUILD-002.

### Steps
1. `"$SCAFFOLD_BIN" localnet stop; "$SCAFFOLD_BIN" deploy; echo "exit=$?"`
2. Check that `curl -m 3 http://10.255.255.1:3040` times out rather than reporting
   `Connection refused`. A sandboxed container often refuses it at once, which makes step 3's
   timing pass for the wrong reason, so note it in the report if it does.
3. In `.scaffold/wallet/wallet_config.json` set `sequencers[0].sequencer_addr` (top-level
   `sequencer_addr` before LEZ v0.2.4) to `http://10.255.255.1:3040`, run
   `time "$SCAFFOLD_BIN" deploy`, then restore the address.

### Success criteria
- Both runs exit non-zero with the sequencer-unavailable hint, step 3 in a few seconds.
- Step 3's hint names `http://10.255.255.1:3040`. A hint naming
  `127.0.0.1:<[localnet].port>` means scaffold probed the default port instead of the
  wallet's target.
- No `reachability probe failed …; continuing` line appears.

### Notes
The probe tells "refused or unreachable" from "something answered". It does not verify that
the responder is this project's sequencer, so a foreign process on the configured port still
suppresses the hint.

## DEPLOY-007 — Stale docker artefacts outside the owned root are ignored

### Expected
In a `default` project, a `docker/` artefact under `methods/guest/target/` (a root scaffold
does not own for this project kind) never outranks the fresh build. `deploy` ships the owned
artefact and prints `note: ignoring …/methods/guest/target/…/docker/hello_world.bin` on
stderr.

### Preconditions
DEPLOY-001.

### Steps
1. `mkdir -p methods/guest/target/riscv32im-risc0-zkvm-elf/docker`
2. `cp <another program>.bin methods/guest/target/riscv32im-risc0-zkvm-elf/docker/hello_world.bin`
3. `"$SCAFFOLD_BIN" deploy hello_world`, then `rm -rf methods/guest/target`

### Success criteria
- The echoed `wallet deploy-program` path is under `target/riscv-guest-docker` (or
  `release/`), and the `note: ignoring` line appears.

### Notes
`lgs build` never clears `methods/guest/target`, so a planted or left-over ELF there would win
every later deploy if it were ranked. On a host that cannot build containers, check the
ranking the other way round as well: copy a `release/` `.bin` into
`target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker/` and confirm that `deploy` echoes
the `docker/` path.

## DEPLOY-008 — Missing binaries name every search root

### Expected
With no guest artefacts and localnet up, `deploy` fails, points back to `logos-scaffold
build`, and lists the searched roots in order: `target/riscv-guest-docker`,
`target/riscv-guest`, `methods/target`, `methods/guest/target`.

### Preconditions
LOCALNET-001, in a project whose guest artefacts are removed.

### Steps
1. `rm -rf target/riscv-guest-docker target/riscv-guest && "$SCAFFOLD_BIN" deploy; echo "exit=$?"`

### Success criteria
- Non-zero exit, all four roots listed in that order, and `build` suggested.
