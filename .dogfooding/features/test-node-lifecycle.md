---
feature: test-node-lifecycle
prefix: TNODE
surface: cli
scenarios:
  - id: TNODE-001
    title: Report the caller project's pins
    priority: P1
    provenance: documented
    source: DOGFOODING.md T1
    depends_on: [CREATE-001]
  - id: TNODE-002
    title: Prepare and check test-node prerequisites
    priority: P1
    provenance: documented
    source: DOGFOODING.md T1
    depends_on: [TNODE-001]
  - id: TNODE-003
    title: Start, inspect and stop an isolated node
    priority: P1
    provenance: documented
    source: DOGFOODING.md T1
    depends_on: [TNODE-002]
  - id: TNODE-004
    title: Wrap a command with a throwaway node
    priority: P1
    provenance: documented
    source: DOGFOODING.md T1
    depends_on: [TNODE-002]
  - id: TNODE-005
    title: Explicit port already in use fails fast
    priority: P2
    provenance: documented
    source: DOGFOODING.md T1
    depends_on: [TNODE-002]
  - id: TNODE-006
    title: Caller-provided LEZ checkout is never mutated
    priority: P2
    provenance: documented
    source: DOGFOODING.md T1
    depends_on: [CREATE-001]
  - id: TNODE-007
    title: Block timing overrides
    priority: P2
    provenance: documented
    source: DOGFOODING.md T1
    depends_on: [TNODE-002]
retired: []
---

# Test-node lifecycle

`test-node` runs disposable sequencers for integration tests. Each node has its own port,
config, database, log and runtime dir under `.scaffold/test-nodes/<id>/`, independent of the
developer localnet. Nodes follow the caller project's LEZ and circuits pins, and every pin
value is annotated with where it came from (`cli_override` → `project_config` →
`scaffold_default`). Commands take `--project <root>`, so they can run from anywhere. Always
validate against a real sequencer, never a stub. Related:
[Test-node RPC](test-node-rpc.md), [Localnet](localnet.md), [Rust API](rust-api.md).

## TNODE-001 — Report the caller project's pins

### Expected
`test-node pins` (and `--json`) reports the LEZ source/ref, resolved commit, checkout path,
ownership (`managed_cache` or `caller_provided`), sequencer binary path and circuits
version/path, each with its origin. For projects with `[circuits]`, the circuits version
matches the project config.

### Preconditions
CREATE-001.

### Steps
1. `"$SCAFFOLD_BIN" test-node pins --project "$P"`
2. `"$SCAFFOLD_BIN" test-node pins --project "$P" --json | jq .`

### Success criteria
- The JSON parses, includes `lez_checkout`, and the circuits version equals
  `[circuits].version`.

## TNODE-002 — Prepare and check test-node prerequisites

### Expected
`test-node prepare` resolves the pins, ensures the checkout and the circuits release
(downloaded with curl), builds `sequencer_service` and ends with `test-node prerequisites
ready`. `--json` reports the checkout, commit, binary and circuits paths. `test-node doctor`
reports pin drift, checkout presence/commit/cleanliness, sequencer binary, circuits and
platform support as separate checks. It exits non-zero only when a real prerequisite is
missing. A missing binary or missing circuits produces an error pointing at `test-node
prepare`, not a panic.

### Preconditions
TNODE-001 and r0vm provisioned (see `DOGFOODING.md` "Provisioning").

### Steps
1. `"$SCAFFOLD_BIN" test-node doctor --project "$P"` (before prepare, if fresh)
2. `"$SCAFFOLD_BIN" test-node prepare --project "$P" --json`
3. `"$SCAFFOLD_BIN" test-node doctor --project "$P" --json | jq .ok`

### Success criteria
- Step 1 names `test-node prepare` for anything missing.
- Step 2 exits 0. Step 3 prints `true`.

### Notes
The sequencer build takes about 6 minutes. Run it in the background.

## TNODE-003 — Start, inspect and stop an isolated node

### Expected
`test-node start --json` prints `rpc_url`, `pid`, `state_dir`, `config_path`, `log_path`,
`genesis_block_id` and `block_height`. The default `--port 0` picks a free port, and the
`block_id` rises within seconds. `status --node <id> --json` reports `healthy` and the RPC
URL, and exits non-zero when unhealthy. `stop --node <id>` stops only that node and removes
its state unless `--preserve-work-dir` is passed. The node never uses the localnet port and
never writes into the LEZ checkout.

### Preconditions
TNODE-002.

### Steps
1. `SJ=$("$SCAFFOLD_BIN" test-node start --project "$P" --json); echo "$SJ"`
2. `NODE=$(echo "$SJ" | jq -r .state_dir | xargs basename); "$SCAFFOLD_BIN" test-node status --node "$NODE" --project "$P" --json`
3. `"$SCAFFOLD_BIN" test-node stop --node "$NODE" --project "$P"; ls "$P/.scaffold/test-nodes"`
4. `"$SCAFFOLD_BIN" test-node status --node "$NODE" --project "$P" --json; echo "exit=$?"`

### Success criteria
- Every listed field is present, and the port is not 3040.
- Status is healthy while running. After stop, the node dir is gone and status exits
  non-zero.

## TNODE-004 — Wrap a command with a throwaway node

### Expected
`test-node run -- <cmd>` starts a node, waits until it is healthy, and exports
`LGS_TEST_NODE_RPC_URL`, `LGS_TEST_NODE_PORT`, `LGS_TEST_NODE_STATE_DIR` and related vars to
the child. It forwards the child's exit status and stops the node afterwards. `--serial` caps
concurrent node creation at one, and `--parallel N` at N.

### Preconditions
TNODE-002.

### Steps
1. `"$SCAFFOLD_BIN" test-node run --project "$P" --serial -- sh -c 'echo "rpc=$LGS_TEST_NODE_RPC_URL port=$LGS_TEST_NODE_PORT"'`
2. `"$SCAFFOLD_BIN" test-node run --project "$P" -- sh -c 'exit 7'; echo "exit=$?"`
3. `pgrep -fl sequencer_service`

### Success criteria
- Step 1 prints a real URL and port. Step 2 prints `exit=7`.
- Step 3 shows no leftover node from these runs.

## TNODE-005 — Explicit port already in use fails fast

### Expected
`test-node start --port <busy>` fails quickly with a port-conflict error and does not hang.

### Preconditions
TNODE-002, plus a listener on some port (for example a running localnet on 3040).

### Steps
1. `time "$SCAFFOLD_BIN" test-node start --project "$P" --port 3040; echo "exit=$?"`

### Success criteria
- Non-zero exit within a few seconds, with a port-conflict message.

## TNODE-006 — Caller-provided LEZ checkout is never mutated

### Expected
A caller-provided checkout (`[repos.lez].path`, or a local directory passed as `--lez-source
DIR`) is only validated: it must be a clean worktree at the requested commit. `prepare` never
resets, cleans or force-checks it out. A dirty or mismatched checkout is reported, not fixed.

### Preconditions
A local LEZ clone at the pinned commit.

### Steps
1. `"$SCAFFOLD_BIN" test-node pins --project "$P" --lez-source /path/to/lez --json | jq .ownership`
2. `touch /path/to/lez/DIRTY && "$SCAFFOLD_BIN" test-node prepare --project "$P" --lez-source /path/to/lez; echo "exit=$?"`
3. `git -C /path/to/lez status --short; git -C /path/to/lez rev-parse HEAD`

### Success criteria
- Step 1 prints `caller_provided`. Step 2 refuses, naming the dirty worktree.
- Step 3 still shows `?? DIRTY`, and HEAD is unchanged.

## TNODE-007 — Block timing overrides

### Expected
`--block-create-timeout-ms` and `--retry-pending-blocks-timeout-ms` on `start`/`run` patch
the runtime `sequencer_config.json` with millisecond strings (for example `500ms`).
Accepted values are 1 to 3,600,000. Omitting them keeps the pinned values.

### Preconditions
TNODE-002.

### Steps
1. `SJ=$("$SCAFFOLD_BIN" test-node start --project "$P" --block-create-timeout-ms 500 --retry-pending-blocks-timeout-ms 500 --json)`
2. `grep -E 'block_create_timeout|retry_pending_blocks_timeout' "$(echo "$SJ" | jq -r .config_path)"`
3. `"$SCAFFOLD_BIN" test-node start --project "$P" --block-create-timeout-ms 0; echo "exit=$?"`
4. Stop the node from step 1.

### Success criteria
- Step 2 shows `500ms` for both keys. Step 3 is rejected.
