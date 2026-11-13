---
feature: localnet
prefix: LOCALNET
surface: cli
scenarios:
  - id: LOCALNET-001
    title: Start localnet and wait until ready
    priority: P0
    provenance: documented
    source: DOGFOODING.md D1
    depends_on: [BUILD-001]
  - id: LOCALNET-002
    title: Status in human and JSON form
    priority: P1
    provenance: documented
    source: DOGFOODING.md D2
    depends_on: [LOCALNET-001]
  - id: LOCALNET-003
    title: Tail localnet logs
    priority: P2
    provenance: documented
    source: DOGFOODING.md D2
    depends_on: [LOCALNET-001]
  - id: LOCALNET-004
    title: Stop localnet
    priority: P1
    provenance: documented
    source: DOGFOODING.md D2
    depends_on: [LOCALNET-001]
  - id: LOCALNET-005
    title: Localnet survives terminal closure
    priority: P1
    provenance: documented
    source: DOGFOODING.md D2
    depends_on: [LOCALNET-001]
  - id: LOCALNET-006
    title: Reset chain state
    priority: P2
    provenance: documented
    source: docs/commands.md
    depends_on: [LOCALNET-001]
  - id: LOCALNET-007
    title: Start failure points at the next diagnostic step
    priority: P2
    provenance: documented
    source: DOGFOODING.md D2
    depends_on: [BUILD-001]
retired: []
---

# Localnet

Each project has one long-lived, daemonized developer sequencer on `[localnet].port`
(default 3040). Scaffold tracks its PID and tells a managed process apart from stale state or
a foreign listener. Related: [Setup and build](setup-and-build.md), [Deploy](deploy.md),
[Test-node lifecycle](test-node-lifecycle.md) (a separate, disposable sequencer).

## LOCALNET-001 — Start localnet and wait until ready

### Expected
`localnet start` returns only after the sequencer PID is alive and `127.0.0.1:<port>` is
reachable, and then reports a ready localnet, not just a spawned PID. If it does not become
ready within `--timeout-sec`, it fails with diagnostics.

### Preconditions
BUILD-001, nothing listening on the configured port, and r0vm provisioned.

### Steps
1. `"$SCAFFOLD_BIN" localnet start`

### Success criteria
- Exit 0, and the output says ready.
- `lsof -iTCP:3040 -sTCP:LISTEN` shows the sequencer.

## LOCALNET-002 — Status in human and JSON form

### Expected
`localnet status` reports the tracked PID, listener state, ownership and readiness, with no
contradiction between them. `--json` is parseable and has at least `tracked_pid`,
`listener_present`, `ownership` and `ready`.

### Preconditions
LOCALNET-001.

### Steps
1. `"$SCAFFOLD_BIN" localnet status`
2. `"$SCAFFOLD_BIN" localnet status --json | jq '{tracked_pid, listener_present, ownership, ready}'`

### Success criteria
- `jq` parses the output and all four keys are non-null.
- `ready` is `true`, and ownership says the process is managed.

## LOCALNET-003 — Tail localnet logs

### Expected
`localnet logs --tail N` prints up to N recent sequencer log lines. `--json` is also
available.

### Preconditions
LOCALNET-001.

### Steps
1. `"$SCAFFOLD_BIN" localnet logs --tail 200`

### Success criteria
- Prints recent log lines (or `<no log output yet>` right after a failed start).

## LOCALNET-004 — Stop localnet

### Expected
`localnet stop` terminates the managed sequencer, and a later status reports it stopped.

### Preconditions
LOCALNET-001.

### Steps
1. `"$SCAFFOLD_BIN" localnet stop`
2. `"$SCAFFOLD_BIN" localnet status --json | jq .ready`

### Success criteria
- Step 1 exits 0. Step 2 prints `false`, and no listener remains on the port.

## LOCALNET-005 — Localnet survives terminal closure

### Expected
The sequencer is daemonized (`setsid`), so closing the shell or tmux session that started it
does not kill it.

### Preconditions
BUILD-001, localnet stopped.

### Steps
1. `tmux new -d -s ln "cd $PWD && $SCAFFOLD_BIN localnet start; sleep 5"`, then wait until it
   is ready and `tmux kill-session -t ln`.
2. From a new shell: `"$SCAFFOLD_BIN" localnet status --json | jq .ready`

### Success criteria
- Step 2 prints `true`.

## LOCALNET-006 — Reset chain state

### Expected
`localnet reset` refuses to run without `--yes` unless `--dry-run` is given. `--dry-run`
prints the plan and changes nothing. `--yes` stops the sequencer, clears chain state,
restarts it and verifies that blocks are produced. `--reset-wallet` also deletes the project
wallet home and the default-address state.

### Preconditions
LOCALNET-001, in a scratch project.

### Steps
1. `"$SCAFFOLD_BIN" localnet reset; echo "exit=$?"`
2. `"$SCAFFOLD_BIN" localnet reset --dry-run`
3. `"$SCAFFOLD_BIN" localnet reset --yes`

### Success criteria
- Step 1 exits non-zero and asks for `--yes` or `--dry-run`.
- Step 2 changes nothing.
- Step 3 exits 0 with a verified, running localnet, and previously deployed programs are gone.

### Notes
`--reset-wallet` is irrecoverable. Use it only in a scratch project.

## LOCALNET-007 — Start failure points at the next diagnostic step

### Expected
A failing `localnet start` (port held by a foreign process, or a sequencer that dies during
startup) names both `logos-scaffold localnet status` and `localnet logs --tail 200` as next
steps, even when the log tail is empty.

### Preconditions
BUILD-001. A foreign listener on the port, e.g. `python3 -m http.server 3040`.

### Steps
1. `"$SCAFFOLD_BIN" localnet start; echo "exit=$?"`
2. `"$SCAFFOLD_BIN" localnet status`

### Success criteria
- Step 1 fails and names both follow-up commands.
- Step 2 identifies a foreign listener and its PID.
