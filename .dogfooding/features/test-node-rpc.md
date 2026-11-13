---
feature: test-node-rpc
prefix: TRPC
surface: cli
scenarios:
  - id: TRPC-001
    title: Read and classify blocks and clock state
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 T2
    depends_on: [TNODE-003]
  - id: TRPC-002
    title: Transactions end in one definitive outcome
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 T2
    depends_on: [TNODE-003]
  - id: TRPC-003
    title: Block-scoped account and proof reads
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 T2
    depends_on: [TNODE-003]
  - id: TRPC-004
    title: Snapshot accounts to a file
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 T2
    depends_on: [TNODE-003]
  - id: TRPC-005
    title: Start a node from exactly a seeded state
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 T3
    depends_on: [TNODE-002]
  - id: TRPC-006
    title: Bad snapshots are rejected up front
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 T3
    depends_on: [TNODE-001]
  - id: TRPC-007
    title: A real wallet transaction is observed as committed
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 T4
    depends_on: [TNODE-002, BUILD-001]
  - id: TRPC-008
    title: Committed transactions are found after a database restart
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 T3
    depends_on: [TRPC-005]
retired: []
---

# Test-node RPC

These subcommands give integration tests typed observations of a running node instead of
hand-rolled JSON-RPC: terminal transaction outcomes, classified blocks, clock state,
block-scoped account and proof reads, and caller-provided state seeding. They take `--url`,
so they run anywhere once the node URL is known. The same calls exist on
`api::testnode::TestNodeClient`. The most valuable check is against a real node, where the
borsh parsers meet genuine sequencer bytes. Related:
[Test-node lifecycle](test-node-lifecycle.md), [Wallet](wallet.md).

## TRPC-001 — Read and classify blocks and clock state

### Expected
`blocks head`/`range` report each block's real `block_id` and `timestamp` and classify it.
Genesis is block 1. Up to LEZ v0.2.3 it has no transactions. From v0.2.4 (the default pin) it
carries the genesis setup transactions ending in a `clock(0)` initialisation, and is still
classified as genesis, with `has_clock_transaction: false` and
`has_user_transactions: false`. Post-genesis blocks with no user activity are
clock-only: `transaction_count: 1`, the single tx has `is_clock: true`, `kind: "public"`, and
`fully_parsed: true`. Blocks with user transactions list hashes. `blocks wait --after N
--count K` returns K blocks after N. `clock read` decodes all three
`/LEZ/ClockProgramAccount/...` accounts: `/0000001` tracks the head, and `/0000010` and
`/0000050` lag behind. `clock wait-stable` returns only after consecutive identical samples,
or a retryable timeout.

### Preconditions
TNODE-003 running, with `TN_URL` set to its `rpc_url`.

### Steps
1. `"$SCAFFOLD_BIN" test-node blocks head --url "$TN_URL" --json`
2. `"$SCAFFOLD_BIN" test-node blocks range --url "$TN_URL" --from 1 --to 3 --json` (genesis is block 1, and a range with a missing block fails as a whole)
3. `"$SCAFFOLD_BIN" test-node blocks wait --url "$TN_URL" --after 1 --count 1 --json`
4. `"$SCAFFOLD_BIN" test-node clock read --url "$TN_URL" --json`
5. `"$SCAFFOLD_BIN" test-node clock wait-stable --url "$TN_URL" --samples 2 --json`

### Success criteria
- Block 1 is classified as genesis (0 transactions before LEZ v0.2.4; on v0.2.4 its setup
  transactions with `has_clock_transaction: false` and `has_user_transactions: false`), and
  later blocks are clock-only with `fully_parsed: true`.
- All three clock accounts decode, and `/0000001`'s `block_id` is close to the head.

## TRPC-002 — Transactions end in one definitive outcome

### Expected
`tx submit-and-wait --json` prints exactly one terminal object: `committed` (with the actual
`block_id` and `timestamp`), `rejected` (`phase: stateless|stateful`), `timeout`,
`transport_error` or `wire_mismatch`. It exits non-zero for anything but `committed`.
Transport failures are never reported as rejections, and a stateful rejection needs
multi-block observation plus a chain scan. `tx submit` returns a hash or a structured
stateless rejection, and exits non-zero unless the status is `submitted`. `tx wait --hash`
honours `--after-block`. `--encoding` is `borsh-base64` (the default: base64 text, trimmed)
or `borsh` (raw bytes). The same bytes give the same outcome in either encoding. A
`borsh-base64` file that is not base64 fails locally with `invalid base64 transaction
encoding` before any RPC.

### Preconditions
TNODE-003 running and a transaction file. Any base64 borsh blob works for the negative path.

### Steps
1. `echo AAAA > /tmp/bad.b64 && "$SCAFFOLD_BIN" test-node tx submit-and-wait --url "$TN_URL" --file /tmp/bad.b64 --json; echo "exit=$?"`
2. `"$SCAFFOLD_BIN" test-node tx submit-and-wait --url http://127.0.0.1:1 --file /tmp/bad.b64 --json; echo "exit=$?"`
3. `base64 -d /tmp/bad.b64 > /tmp/bad.bin && "$SCAFFOLD_BIN" test-node tx submit-and-wait --url "$TN_URL" --file /tmp/bad.bin --encoding borsh --json; echo "exit=$?"`
4. `"$SCAFFOLD_BIN" test-node tx submit --url "$TN_URL" --file /tmp/bad.bin --encoding borsh --json; echo "exit=$?"`
5. `echo '!!!' > /tmp/notb64.txt && "$SCAFFOLD_BIN" test-node tx submit --url "$TN_URL" --file /tmp/notb64.txt --json; echo "exit=$?"`

### Success criteria
- Step 1 prints one `rejected` (stateless) or `wire_mismatch` object and exits non-zero.
- Step 2 prints `transport_error`, not `rejected`, and exits non-zero.
- Step 3 gives the same outcome kind as step 1 and exits non-zero.
- Step 4 prints one object whose `status` is `rejected` (or `submitted` if the node takes it
  into its mempool) and exits non-zero unless `submitted`.
- Step 5 exits non-zero with `invalid base64 transaction encoding` and prints no JSON.

## TRPC-003 — Block-scoped account and proof reads

### Expected
`account get` reports `present` (lossless base64 bytes plus decoded balance, nonce, owner and
data), `missing` or `decode_error`, together with the `block_id` it was read at.
`account batch-get` reads every account at one block boundary. `--at-block N` pins the read.
`proof get` distinguishes a missing commitment (`proof: null`), an invalid commitment (a
local error before any RPC) and transport failure.

### Preconditions
TNODE-003 running. A known account, for example the vault PDA that TRPC-005 seeds (on LEZ
v0.2.4 the seeded account itself reads `missing`).

### Steps
1. `"$SCAFFOLD_BIN" test-node account get --url "$TN_URL" --account-id <id> --json`
2. `"$SCAFFOLD_BIN" test-node account batch-get --url "$TN_URL" --account-id <id> --account-id <never-written-id> --json`
3. `"$SCAFFOLD_BIN" test-node proof get --url "$TN_URL" --commitment 00 --json; echo "exit=$?"`
4. `"$SCAFFOLD_BIN" test-node proof get --url "$TN_URL" --commitment <64-hex-zeroes> --json`

### Success criteria
- Every read has a `block_id`, and batch entries share one `block_id`.
- Step 1 reports `state: present` with base64 bytes and decoded balance, nonce, owner and
  data. In step 2 the never-written id reads `state: missing`.
- Step 3 fails locally as invalid, before any RPC. Step 4 returns `proof: null`.

## TRPC-004 — Snapshot accounts to a file

### Expected
`snapshot accounts --account-id … --output PATH` writes a block-consistent
`lgs-account-snapshot/1` JSON file that `state seed` can use.

### Preconditions
TNODE-003 running.

### Steps
1. `"$SCAFFOLD_BIN" test-node snapshot accounts --url "$TN_URL" --account-id <id> --output /tmp/acc.json --json`
2. `jq .format /tmp/acc.json`

### Success criteria
- The file exists, and its format is `lgs-account-snapshot/1`.

## TRPC-005 — Start a node from exactly a seeded state

### Expected
`state schema` names the accepted formats (`lgs-state-snapshot/1`, `lgs-account-snapshot/1`,
or a rocksdb dir), the state format version (`nssa-v03`), the LEZ ref/commit and the seedable
fields. `state seed` validates the snapshot, writes a state dir and reports the seed kind,
LEZ commit, format version and account counts. `start --state` builds genesis from exactly
the snapshot, with none of the debug config's sample accounts. On LEZ v0.2.4 each seeded public
balance becomes a `supply_account` genesis action that credits the account's **vault PDA**,
not the account. `state seed` warns about that, and `account get` on the account itself reads
`missing`. The debug config's `supply_bridge_account` entry stays, and the sequencer's built-in
testnet state is always present; neither is a leak. v0.2.4
also rejects non-empty `private_accounts`. `state export` writes named public balances from a
running node.

### Preconditions
TNODE-002.

### Steps
1. `"$SCAFFOLD_BIN" test-node state schema --project "$P" --json`
2. Write `seed.json`: `{"format":"lgs-state-snapshot/1","public_accounts":[{"account_id":"<base58>","balance":4242}],"private_accounts":[]}`
3. `"$SCAFFOLD_BIN" test-node state seed --project "$P" --input seed.json --output ./seeded --json`
4. `SJ=$("$SCAFFOLD_BIN" test-node start --project "$P" --state ./seeded --json); TN_URL=$(echo "$SJ" | jq -r .rpc_url)`
5. `"$SCAFFOLD_BIN" test-node account get --url "$TN_URL" --account-id <vault> --json`, where
   `<vault>` is the account's vault PDA: the public PDA of the vault program with seed
   `sha256("/LEZ/v0.3/VaultSeed/00000000000/" ‖ account_id)`. The vault program id is the
   `vault_program_id` inside any genesis `supply_account` transaction. For the debug config's `CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r` it is
   `G1u9gmu1Gb8PG59zHZYNL5WdJNNtBKEN4U5qUdx2y6sG`.
6. Repeat step 5 for the vault of a debug-config sample account the seed left out (for example
   `2RHZhw9h534Zr3eq2RGhQete2Hh667foECzXPmSkGni2` →
   `Ey6P7CkwXHLskqFjQzU8C1kTwb54TkZfhzv9wUijb3Wc`).
7. `"$SCAFFOLD_BIN" test-node state export --url "$TN_URL" --account-id <vault> --output /tmp/export.json --json; jq . /tmp/export.json`

### Success criteria
- Step 3 warns that the balance is credited to the vault PDA.
- Step 5 reports `state: present` with `balance: 4242`.
- Step 6 reports `missing`. That proves the seed replaced the sample accounts.
- Step 7 writes a snapshot naming `<vault>` with balance 4242.
### Notes
Pins up to LEZ v0.2.3 seed through `initial_public_accounts` / `initial_private_accounts`
instead, with no default accounts. A `start` that fails with ``error:
sequencer_config.json has no numeric `genesis_id` `` means scaffold read a v0.2.4 config the
older way. A state dir holding a node's preserved `rocksdb/` resumes from it
verbatim; that is the full-fidelity path, while the JSON snapshot carries balances only.
`state export` covers public balances only, because the pinned RPC exposes nothing else.

## TRPC-006 — Bad snapshots are rejected up front

### Expected
`state seed` rejects an unknown format with `format mismatch`. It rejects a `public_accounts`
entry with any field other than `account_id`/`balance` with `storage schema mismatch`
naming the field. It rejects a `lez_commit` different from the project pin, and it reports
account decode errors. All of these exit non-zero before any node starts.

### Preconditions
TNODE-001.

### Steps
1. `echo '{"format":"bogus/1"}' > bad.json && "$SCAFFOLD_BIN" test-node state seed --project "$P" --input bad.json --output ./rejected; echo "exit=$?"`
2. Seed a snapshot whose public account has `"nonce": 3`.
3. On a LEZ v0.2.4 pin, seed a snapshot with a non-empty `private_accounts` list.
4. Seed a snapshot whose `lez_commit` is not the project's resolved LEZ commit.
5. `echo '{"format":"lgs-state-snapshot/1","public_accounts":[{"account_id":"not-base58!!","balance":1}]}' > bad-id.json && "$SCAFFOLD_BIN" test-node state seed --project "$P" --input bad-id.json; echo "exit=$?"`
6. `ls -d ./rejected "$P"/.scaffold/test-nodes/seeds/seed-* 2>&1`

### Success criteria
- Step 1 fails with `format mismatch`. Step 2 fails with `storage schema mismatch` naming
  `nonce`. Step 3 fails with `storage schema mismatch`. Step 4 fails with `lez pin mismatch`.
- Step 5 fails with `account decode error` and `is not base58`.
- Every step exits non-zero, and no node is started.
- Step 6 finds neither `./rejected` nor any `seeds/seed-*` dir: a rejected seed is validated
  before anything is created. An existing `--output` dir is never removed.

## TRPC-007 — A real wallet transaction is observed as committed

### Expected
A test node on the wallet's port executes a real `wallet topup` (`auth-transfer init` plus
`pinata claim`). `blocks wait` then shows blocks with `has_user_transactions: true`,
`transaction_count: 2`, and a non-clock `kind: "public"` tx with a real hash.

### Preconditions
TNODE-002, plus BUILD-001 in the same project (wallet built, default wallet seeded).
Localnet stopped, so port 3040 is free.

### Steps
1. `SJ=$("$SCAFFOLD_BIN" test-node start --project "$P" --port 3040 --json); URL=$(echo "$SJ" | jq -r .rpc_url)`
2. `HEAD0=$("$SCAFFOLD_BIN" test-node blocks head --url "$URL" --json | jq -r .block_id)`
3. `( cd "$P" && "$SCAFFOLD_BIN" wallet topup )`
4. `"$SCAFFOLD_BIN" test-node blocks wait --url "$URL" --after "$HEAD0" --count 6 --timeout-sec 90 --json > /tmp/blocks.json`,
   then `jq -c '.blocks[] | select(.has_user_transactions)' /tmp/blocks.json` and
   `jq '[.blocks[].fully_parsed] | all' /tmp/blocks.json`.
5. Take a user tx hash from step 4 (`.transactions[] | select(.is_clock | not) | .hash`) and run
   `"$SCAFFOLD_BIN" test-node tx wait --url "$URL" --hash <hash> --after-block "$HEAD0" --json`.
6. `"$SCAFFOLD_BIN" test-node stop --node <id>`, then check that `.scaffold/test-nodes/<id>` is gone.

### Success criteria
- `topup` exits 0. At least one block has 2 transactions, including a non-clock public tx,
  and the `all` check over every block (clock-only blocks included) prints `true`.
- Step 5 prints a `committed` object with that block's `block_id` and `timestamp`, and exits 0.
- Step 6 leaves no runtime dir behind.
- If `topup` instead prints a sequencer-unreachable hint, the node is not on `[localnet] port`:
  that hint must name the configured port, not a hardcoded `:3040`.

### Notes
If blocks keep coming but no user tx lands, r0vm is probably missing or the wrong version.
The wallet talks to the `sequencer_addr` in its config, so start the node on that port or
point the config at the node. A user-tx block classified as clock-only, or `fully_parsed:
false` on a plain public or deploy tx, is a wire-format regression: this is the only scenario
that checks the block parser against a really executed transaction.

## TRPC-008 — Committed transactions are found after a database restart

### Expected
A node resumed from a preserved database answers `getTransaction` with `null` for older
transactions. `tx wait --hash <old> --after-block <b>` still reports `committed` with the
original block, because scaffold scans blocks, even when `<b>` is later than the commit
block. If a scanned block holds a privacy-preserving tx, the result is `timeout`, never a
stateful rejection.

### Preconditions
A node that committed a known tx (TRPC-007), stopped with `--preserve-work-dir`, and
restarted with `--state <its dir>`.

### Steps
1. `"$SCAFFOLD_BIN" test-node tx wait --url "$URL" --hash <old-hash> --after-block <later-block> --json`

### Success criteria
- `committed`, with the original `block_id`.
