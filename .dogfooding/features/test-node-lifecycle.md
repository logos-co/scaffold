---
feature: test-node-lifecycle
prefix: TNODE
surface: cli
scenarios:
  - id: TNODE-001
    title: Report the caller project's pins
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 T1
    depends_on: [CREATE-001]
  - id: TNODE-002
    title: Prepare and check test-node prerequisites
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 T1
    depends_on: [TNODE-001]
  - id: TNODE-003
    title: Start, inspect and stop an isolated node
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 T1
    depends_on: [TNODE-002]
  - id: TNODE-004
    title: Wrap a command with a throwaway node
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 T1
    depends_on: [TNODE-002]
  - id: TNODE-005
    title: Start options - a busy port fails fast, block timing overrides apply
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 T1
    depends_on: [TNODE-002]
  - id: TNODE-006
    title: Caller-provided LEZ checkout is never mutated
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 T1
    depends_on: [TNODE-001]
retired: [TNODE-007]
---

# Test-node lifecycle

`test-node` runs disposable sequencers for integration tests. Each node has its own port,
config, database, log and runtime dir under `.scaffold/test-nodes/<id>/`, independent of the
developer localnet. Nodes follow the caller project's LEZ and circuits pins, and every pin
value is annotated with where it came from (`cli_override` → `project_config` →
`scaffold_default`). Commands take `--project <root>`, so they can run from anywhere. Always
validate against a real sequencer, never a stub. Related:
[Test-node RPC](test-node-rpc.md), [Localnet](localnet.md), [Rust API](rust-api.md).

`tn` below is `"$SCAFFOLD_BIN" test-node`, and `$P` is the project root (absolute unless a
step says otherwise). Nodes are meant to be thrown away: use a fresh one per scenario.

## TNODE-001 — Report the caller project's pins

### Expected
`test-node pins` (and `--json`) reports the LEZ source/ref, resolved commit, checkout path,
ownership (`managed_cache` or `caller_provided`), sequencer binary path and circuits
version/path, each with its origin. The circuits version matches `[circuits].version`.
`pins` is read-only. `--lez-ref <ref>` overrides the ref (`lez_ref_origin: cli_override`) and
points `lez_checkout` at `<cache_root>/repos/lez/<ref>` without cloning anything;
`lez_resolved_commit` stays `null` until that checkout exists.

### Preconditions
CREATE-001.

### Steps
1. `tn pins --project "$P"; tn pins --project "$P" --json | jq .; grep -A2 '^\[circuits\]' "$P/scaffold.toml"`
2. `tn pins --project "$P" --lez-ref dogfood-unused-ref --json | jq '{lez_ref, lez_ref_origin, lez_checkout, checkout_ownership, lez_resolved_commit}'`,
   then `ls <lez_checkout>; echo "exit=$?"`.

### Success criteria
- The JSON parses, includes `lez_checkout` and origins, and the circuits version equals
  `[circuits].version`.
- Step 2 reports `lez_ref: dogfood-unused-ref`, `lez_ref_origin: cli_override`,
  `checkout_ownership: managed_cache`, a `lez_checkout` ending in
  `/repos/lez/dogfood-unused-ref` and `lez_resolved_commit: null`. That directory does not
  exist.

## TNODE-002 — Prepare and check test-node prerequisites

### Expected
`test-node prepare` resolves the pins, ensures the checkout and the circuits release
(downloaded with curl), builds `sequencer_service` and ends with `test-node prerequisites
ready`. `--json` reports the checkout, commit, binary and circuits paths. `test-node doctor`
reports pin drift, checkout presence/commit/cleanliness, sequencer binary, circuits and
platform support as separate checks, and exits non-zero only when a real prerequisite is
missing; the fix it names is `test-node prepare`. `prepare --cache-root DIR` puts the managed
LEZ checkout under `DIR/repos/lez/<ref>` instead of the project's cache root, and `prepare
--lez-ref <ref>` builds that ref. Circuits still go to the project's configured install dir.

### Preconditions
TNODE-001 and r0vm provisioned (see "Toolchain" in [product.md](../product.md)). For step 1,
a second project that has never run `setup`, `prepare` or a test node (`lgs new` is enough;
call it `$FRESH`): in a prepared project nothing is missing and the hint cannot show.

### Steps
1. `tn doctor --project "$FRESH"; echo "exit=$?"`
2. `PJ=$(tn prepare --project "$P" --json); echo "$PJ"; tn doctor --project "$P" --json | jq .ok`
3. Check the overrides without a second 20-minute sequencer build, by seeding the other cache
   root with a copy of the built checkout:
   ```bash
   PIN=$(echo "$PJ" | jq -r .lez_commit); SRC=$(echo "$PJ" | jq -r .checkout)
   mkdir -p "$SCRATCH_ROOT/tn-cache/repos/lez" && cp -a "$SRC" "$SCRATCH_ROOT/tn-cache/repos/lez/$PIN"
   tn prepare --project "$P" --cache-root "$SCRATCH_ROOT/tn-cache" --lez-ref "$PIN" --json | jq '{checkout, checkout_ownership, lez_commit, circuits_path}'
   git -C "$SRC" status --short
   ```

### Success criteria
- Step 1 exits non-zero: a FAIL row for the missing circuits (and for the checkout or binary
  on a cold cache), each with a fix naming `test-node prepare`, and no panic.
- Step 2 exits 0 and prints `true`.
- Step 3 exits 0 with `checkout` equal to `$SCRATCH_ROOT/tn-cache/repos/lez/$PIN`,
  `checkout_ownership: managed_cache`, `lez_commit` equal to `$PIN`, and `circuits_path` still
  under `$P`. The original checkout has no changes.

### Notes
A cold `prepare` builds the sequencer (see "Long steps" in product.md); background it.
Without step 3's copy, `prepare` clones and builds the ref from scratch in the new cache
root, which is the same code path at twenty times the cost. `start` and `run` do not take
these overrides: they always use the project's own LEZ checkout.

## TNODE-003 — Start, inspect and stop an isolated node

### Expected
`test-node start --json` prints `rpc_url`, `pid`, `state_dir`, `config_path`, `log_path`,
`genesis_block_id` and `block_height`. The default `--port 0` picks a free port, and the
block height rises within seconds. The RPC listens on `127.0.0.1` only. `status --node <id>
--json` reports `healthy` and the RPC URL, and exits non-zero when unhealthy or unknown.
`stop --node <id>` stops only that node and removes its state unless `--preserve-work-dir`
is passed. `--work-dir DIR` uses `DIR` as the runtime directory instead of
`.scaffold/test-nodes/<id>`, and `--node` accepts that path. `--work-dir` refuses a
directory that already holds files and leaves it untouched. A relative `--project` works.
The node never uses the localnet port and never writes into the LEZ checkout.

### Preconditions
TNODE-002.

### Steps
1. `SJ=$(tn start --project "$P" --json); echo "$SJ"; NODE=$(echo "$SJ" | jq -r .state_dir | xargs basename)`
2. `ss -ltnp | grep sequencer; tn status --node "$NODE" --project "$P" --json`
3. `tn stop --node "$NODE" --project "$P"; ls "$P/.scaffold/test-nodes"; tn status --node "$NODE" --project "$P" --json; echo "exit=$?"`
4. `git -C "$(tn pins --project "$P" --json | jq -r .lez_checkout)" status --short; ls "$(tn pins --project "$P" --json | jq -r .circuits_path)"`
5. `cd "$P" && SJ=$(tn start --project . --work-dir "$SCRATCH_ROOT/tn-work" --json) && echo "$SJ" | jq -r .state_dir && jq -r .home "$(echo "$SJ" | jq -r .config_path)" && tn stop --node "$SCRATCH_ROOT/tn-work" --project .`
6. `mkdir -p "$SCRATCH_ROOT/tn-keep" && echo '{}' > "$SCRATCH_ROOT/tn-keep/important.json" && tn start --project "$P" --work-dir "$SCRATCH_ROOT/tn-keep"; echo "exit=$?"; ls "$SCRATCH_ROOT/tn-keep"`

### Success criteria
- Step 1 has every listed field, and the port is not 3040.
- Step 2: the node's port is bound to `127.0.0.1`, not `0.0.0.0`, and status is `healthy`.
- Step 3: `test node stopped`, the node dir is gone, and status exits non-zero with
  `unknown test node`.
- Step 4: the LEZ checkout has no new or modified files, and the circuits install exists
  (start materialises it if it was missing).
- Step 5 starts although `--project` is relative, prints `$SCRATCH_ROOT/tn-work` as the
  state dir and an absolute `home`, and `stop` accepts the path as `--node`.
- Step 6 exits non-zero with `is not empty`, starts no node, and `important.json` is still there.

## TNODE-004 — Wrap a command with a throwaway node

### Expected
`test-node run -- <cmd>` starts a node, waits until it is healthy, and exports
`LGS_TEST_NODE_RPC_URL`, `LGS_TEST_NODE_PORT`, `LGS_TEST_NODE_STATE_DIR`,
`LGS_TEST_NODE_CONFIG_PATH` and related vars to the child. Stdout is exactly the child's
output: scaffold adds no `$ <command>` echo lines to it. It forwards the child's exit status
and stops the node afterwards. `--serial` caps concurrent node creation at one, and
`--parallel N` at N.

### Preconditions
TNODE-002.

### Steps
1. `tn run --project "$P" --serial -- sh -c 'echo "rpc=$LGS_TEST_NODE_RPC_URL port=$LGS_TEST_NODE_PORT"' 2>/dev/null`
2. `tn run --project "$P" -- sh -c 'exit 7'; echo "exit=$?"`
3. `for i in 1 2 3; do tn run --project "$P" --parallel 2 -- sh -c 'echo "$LGS_TEST_NODE_PORT $(date +%T)"; sleep 5' 2>/dev/null & done; wait`
4. `pgrep -x sequencer_servi` (with localnet stopped; the kernel truncates the name to 15
   characters, and `pgrep -f sequencer_service` also matches the shell running this line)

### Success criteria
- Step 1 prints one line with a real URL and port, and nothing else. Step 2 prints `exit=7`.
- Step 3 prints three distinct ports; the third timestamp is about 5 seconds after the first
  two (it started only after one of them stopped). All three runs exit 0.
- Step 4 prints nothing: no node is left over.

## TNODE-005 — Start options - a busy port fails fast, block timing overrides apply

### Expected
`test-node start --port <busy>` fails at once with a port-conflict error that suggests
`--port 0`. `--block-create-timeout-ms` and `--retry-pending-blocks-timeout-ms` on
`start`/`run` patch the runtime `sequencer_config.json` with millisecond strings (`500ms`).
Accepted values are 1 to 3,600,000. Omitting them keeps the pinned values.

### Preconditions
TNODE-002, plus a listener on some port (a running localnet on 3040, or `python3 -m
http.server 3040`).

### Steps
1. `time tn start --project "$P" --port 3040; echo "exit=$?"`
2. `tn start --project "$P" --block-create-timeout-ms 0; echo "exit=$?"`
3. `tn run --project "$P" --block-create-timeout-ms 500 --retry-pending-blocks-timeout-ms 500 -- sh -c 'grep -E "block_create_timeout|retry_pending_blocks_timeout" "$LGS_TEST_NODE_CONFIG_PATH"'`

### Success criteria
- Step 1 exits non-zero within a second with `port 3040 is already in use. Pass --port 0 …`.
- Step 2 is rejected (`0 is not in 1..=3600000`) and starts nothing.
- Step 3 shows `500ms` for both keys. (`start` takes the same flags; TRPC scenarios may use
  them to shorten block waits.)

### Notes
Values near or below the stable-read sample cadence can keep `clock wait-stable` and
block-scoped account reads from converging.

## TNODE-006 — Caller-provided LEZ checkout is never mutated

### Expected
A caller-provided checkout (`[repos.lez].path`, or a local directory passed as `--lez-source
DIR`) is only validated: it must be a clean worktree at the requested commit. `prepare` never
resets, cleans or force-checks it out. A dirty or mismatched checkout is reported, not fixed.

### Preconditions
A local LEZ clone at the pinned commit with a normal `origin`, for example
`git clone <cache>/repos/lez/<pin> /tmp/lez-src && git -C /tmp/lez-src remote set-url origin
https://github.com/logos-blockchain/logos-execution-zone.git` (CREATE-005 uses the same
clone).

### Steps
1. `tn pins --project "$P" --lez-source /tmp/lez-src --json | jq .checkout_ownership`
2. `touch /tmp/lez-src/DIRTY && tn prepare --project "$P" --lez-source /tmp/lez-src; echo "exit=$?"`
3. `git -C /tmp/lez-src status --short; git -C /tmp/lez-src rev-parse HEAD; rm /tmp/lez-src/DIRTY`

### Success criteria
- Step 1 prints `caller_provided`. Step 2 exits non-zero, naming the uncommitted changes and
  saying scaffold will not reset a caller-provided worktree.
- Step 3 still shows `?? DIRTY`, and HEAD is unchanged.
