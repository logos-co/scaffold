---
feature: localnet
prefix: LOCALNET
surface: cli
scenarios:
  - id: LOCALNET-001
    title: Start localnet and wait until ready
    priority: P0
    provenance: documented
    source: DOGFOODING.md@740e5e9 D1
    depends_on: [BUILD-001]
  - id: LOCALNET-002
    title: Status and logs in human and JSON form
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D2
    depends_on: [LOCALNET-001]
  - id: LOCALNET-004
    title: Stop localnet, and a started one survives terminal closure
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D2
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
    source: DOGFOODING.md@740e5e9 D2
    depends_on: [BUILD-001]
retired: [LOCALNET-003, LOCALNET-005]
---

# Localnet

Each project has one long-lived, daemonized developer sequencer on `[localnet].port`
(default 3040). Scaffold tracks its PID and tells a managed process apart from stale state or
a foreign listener. Related: [Setup and build](setup-and-build.md), [Deploy](deploy.md),
[Test-node lifecycle](test-node-lifecycle.md) (a separate, disposable sequencer).

## LOCALNET-001 — Start localnet and wait until ready

### Expected
`localnet start` returns only after the sequencer PID is alive and `127.0.0.1:<port>` is
reachable, and then reports a ready localnet, not just a spawned PID. The RPC is bound to
loopback only: it has no caller authentication, and the sequencer's own default is every
interface. If it does not become ready within `--timeout-sec`, it fails with diagnostics.

### Preconditions
BUILD-001, nothing listening on the configured port, and r0vm provisioned.

### Steps
1. `"$SCAFFOLD_BIN" localnet start`
2. `ss -ltnp | grep sequencer` (or `lsof -nP -iTCP -sTCP:LISTEN | grep sequencer`)

### Success criteria
- Exit 0, and the output says `localnet ready (sequencer pid=…)`. The echoed
  `sequencer_service` command line carries `--listen-address 127.0.0.1`.
- The sequencer listens on `127.0.0.1:<port>` and not on `0.0.0.0:<port>`.

## LOCALNET-002 — Status and logs in human and JSON form

### Expected
`localnet status` reports the tracked PID, listener state, ownership and readiness, with no
contradiction between them. `--json` has at least `tracked_pid`, `listener_present`,
`ownership` and `ready`. `localnet logs --tail N` prints up to N recent sequencer log lines,
also as `--json`.

### Preconditions
LOCALNET-001.

### Steps
1. `"$SCAFFOLD_BIN" localnet status`
2. `"$SCAFFOLD_BIN" localnet status --json | jq '{tracked_pid, listener_present, ownership, ready}'`
3. `"$SCAFFOLD_BIN" localnet logs --tail 200 | tail -3; "$SCAFFOLD_BIN" localnet logs --tail 20 --json | jq '.tail | length'`

### Success criteria
- All four keys are non-null, `ready` is `true` and `ownership` is `managed`; the human form
  agrees.
- Step 3 prints recent log lines, and the JSON holds at most 20.

## LOCALNET-004 — Stop localnet, and a started one survives terminal closure

### Expected
`localnet stop` terminates the managed sequencer, and a later status reports it stopped. The
sequencer is daemonized (`setsid`), so closing the shell or tmux session that started it does
not kill it.

### Preconditions
LOCALNET-001 running. `tmux`.

### Steps
1. `"$SCAFFOLD_BIN" localnet stop; "$SCAFFOLD_BIN" localnet status --json | jq .ready; ss -ltnp | grep -c sequencer`
2. `tmux new -d -s ln "cd $PWD && $SCAFFOLD_BIN localnet start; sleep 5"`, wait until
   `localnet status --json | jq .ready` prints `true`, then `tmux kill-session -t ln`.
3. From a new shell: `"$SCAFFOLD_BIN" localnet status --json | jq .ready`

### Success criteria
- Step 1 exits 0, prints `false`, and no sequencer listener remains.
- Step 3 prints `true`. (Localnet stays up for the scenarios that follow.)

## LOCALNET-006 — Reset chain state

### Expected
`localnet reset` refuses to run without `--yes` unless `--dry-run` is given. `--dry-run`
prints the plan and changes nothing. `--yes` stops the sequencer, clears chain state,
restarts it and verifies that blocks are produced; `--verify-timeout-sec` bounds that check.
The wallet files are kept unless `--reset-wallet` is passed, which also deletes the project
wallet home and the default-address state.

### Preconditions
LOCALNET-001 in a scratch project whose default account exists on chain (after WALLET-001 or
any deploy). `ADDR=$(sed -n 's/^default_address=Public\///p' .scaffold/state/wallet.state)`
(the bare base58 id; `account get` rejects the `Public/` prefix).

### Steps
1. `"$SCAFFOLD_BIN" localnet reset; echo "exit=$?"`
2. `PID=$("$SCAFFOLD_BIN" localnet status --json | jq .tracked_pid); "$SCAFFOLD_BIN" localnet reset --dry-run; "$SCAFFOLD_BIN" localnet status --json | jq .tracked_pid`
3. `"$SCAFFOLD_BIN" localnet reset --yes --verify-timeout-sec 120`
4. `"$SCAFFOLD_BIN" test-node blocks head --url http://127.0.0.1:3040 --json | jq .block_id; "$SCAFFOLD_BIN" test-node account get --url http://127.0.0.1:3040 --account-id "$ADDR" --json | jq -r .state; ls .scaffold/wallet`

### Success criteria
- Step 1 exits non-zero and asks for `--yes` or `--dry-run`.
- Step 2 prints the plan and the same PID before and after.
- Step 3 exits 0 with `localnet reset complete; sequencer producing blocks (block_height=1)`.
- Step 4: the head is back at a low block id, the default account reads `missing` (the
  chain, deployed programs included, was wiped), and the wallet files are still there.

### Notes
`--reset-wallet` is irrecoverable. Use it only in a scratch project. The sequencer database
lives in the LEZ checkout (`<cache>/repos/lez/<pin>/rocksdb`, the dry run names it), so
projects sharing a cache root and pin share, and reset, one chain.

## LOCALNET-007 — Start failure points at the next diagnostic step

### Expected
A `localnet start` that fails because a **foreign process holds the port** stops before
spawning anything and names the port, the process id and `Try: kill <pid>`; `localnet
status` calls the listener foreign. `run` fails the same way at its localnet step. A start
that fails because the **sequencer dies during startup or times out** names both
`logos-scaffold localnet status` and `localnet logs --tail 200` as next steps, even when the
log tail is empty.

### Preconditions
BUILD-001, localnet stopped, and a project that builds (on a host without Docker set
`[build].guest = "local"`, or step 2 fails earlier at `[1/5] Building...`).

### Steps
1. Hold the port (`python3 -m http.server 3040 --bind 127.0.0.1 &`), then
   `"$SCAFFOLD_BIN" localnet start; echo "exit=$?"; "$SCAFFOLD_BIN" localnet status; echo "exit=$?"`
2. `"$SCAFFOLD_BIN" run --localnet-timeout 5; echo "exit=$?"`, then kill the listener.
3. Make the sequencer die at startup:
   `DB="$("$SCAFFOLD_BIN" test-node pins --json | jq -r .lez_checkout)/rocksdb"; cp "$DB/CURRENT" /tmp/CURRENT.bak; echo junk > "$DB/CURRENT"; "$SCAFFOLD_BIN" localnet start; echo "exit=$?"; cp /tmp/CURRENT.bak "$DB/CURRENT"`

### Success criteria
- Step 1: `start` fails with `cannot start localnet: port <port> is already in use
  (pid=<pid>)` and `Try: kill <pid>`. `status` shows `ownership: foreign` with that PID and
  exits 0.
- Step 2 fails at the localnet step with the same message and deploys nothing.
- Step 3 fails with `sequencer process exited before becoming ready`, the sequencer's own
  error (`Failed to open database … RocksDb error`) under `last logs:`, and `Next: run
  \`logos-scaffold localnet status\` … then \`logos-scaffold localnet logs --tail 200\``.
