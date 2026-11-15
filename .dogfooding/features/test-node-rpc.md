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
    title: Block-scoped account and proof reads, and account snapshots
    priority: P1
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
    title: A real wallet transaction is observed as committed, also after a database restart
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 T4
    depends_on: [TNODE-002, BUILD-001]
retired: [TRPC-004, TRPC-008]
---

# Test-node RPC

These subcommands give integration tests typed observations of a running node instead of
hand-rolled JSON-RPC: terminal transaction outcomes, classified blocks, clock state,
block-scoped account and proof reads, and caller-provided state seeding. They take `--url`,
so they run anywhere once the node URL is known. The same calls exist on
`api::testnode::TestNodeClient`. The most valuable check is against a real node, where the
borsh parsers meet genuine sequencer bytes. Related:
[Test-node lifecycle](test-node-lifecycle.md), [Wallet](wallet.md).

`tn` is `"$SCAFFOLD_BIN" test-node`. TRPC-001 to TRPC-003 share one plain node:

```bash
SJ=$(tn start --project "$P" --json); TN_URL=$(echo "$SJ" | jq -r .rpc_url); NODE=$(echo "$SJ" | jq -r .state_dir)
# … scenarios … then: tn stop --node "$NODE" --project "$P"
```

At the default pin the three clock accounts are `4Bdcjo…RWNU` (`/LEZ/ClockProgramAccount/0000001`),
`…RWSs` (`/0000010`) and `…RWkX` (`/0000050`); `clock read --json` lists them in that order by
base58 id, not by name. `CLOCK` below is the first id, a convenient account that always
exists.

## TRPC-001 — Read and classify blocks and clock state

### Expected
`blocks head`/`range` report each block's real `block_id` and `timestamp` and classify it.
Genesis is block 1 (`is_genesis: true`). Up to LEZ v0.2.3 it has no transactions; from v0.2.4
(the default pin) it carries the genesis setup transactions ending in a `clock(0)`
initialisation, with `has_clock_transaction: false` and `has_user_transactions: false`.
Post-genesis blocks with no user activity are clock-only: `transaction_count: 1`, and the
single tx has `is_clock: true` and `kind: "public"`, and the block is `fully_parsed: true`. `blocks wait --after
N --count K` returns K blocks after N. `clock read` decodes all three clock accounts: the
first tracks the head, the other two lag behind. `clock wait-stable` returns only after
consecutive identical samples, or a retryable timeout.

### Preconditions
The shared node.

### Steps
1. `tn blocks wait --url "$TN_URL" --after 1 --count 2 --json | jq -c '.blocks[] | {block_id, transaction_count}'`
2. `tn blocks range --url "$TN_URL" --from 1 --to 3 --json | jq -c '.blocks[] | {block_id, timestamp, is_genesis, transaction_count, has_clock_transaction, has_user_transactions, fully_parsed, tx: [.transactions[] | {kind, is_clock}] | unique}'`
3. `tn blocks head --url "$TN_URL" --json | jq .block_id; tn clock read --url "$TN_URL" --json`
4. `tn clock wait-stable --url "$TN_URL" --samples 2 --json`

### Success criteria
- Step 1 returns blocks 2 and 3.
- Step 2: block 1 is genesis (0 transactions before v0.2.4; on v0.2.4 its setup transactions,
  with both `has_…` flags `false`); blocks 2 and 3 are clock-only with `fully_parsed: true`.
- Step 3: all three clock accounts decode, the first one's `block_id` is the head's, and the
  other two are lower.
- Step 4 returns a stable sample and exits 0.

### Notes
A range with a missing block fails as a whole.

## TRPC-002 — Transactions end in one definitive outcome

### Expected
`tx submit-and-wait --json` prints exactly one terminal object: `committed` (with the actual
`block_id` and `timestamp`), `rejected` (`phase: stateless|stateful`), `timeout`,
`transport_error` or `wire_mismatch`. It exits non-zero for anything but `committed`.
Transport failures are never reported as rejections, and a stateful rejection needs
multi-block observation plus a chain scan. `tx submit` returns a hash or a structured
stateless rejection, and exits non-zero unless the status is `submitted`. `--encoding` is
`borsh-base64` (the default: base64 text, trimmed) or `borsh` (raw bytes); the same bytes
give the same outcome in either. A `borsh-base64` file that is not base64 fails locally with
`invalid base64 transaction encoding` before any RPC. (`committed` and `tx wait --hash
--after-block` are exercised by TRPC-007.)

### Preconditions
The shared node. `echo AAAA > /tmp/bad.b64; base64 -d /tmp/bad.b64 > /tmp/bad.bin; echo '!!!' > /tmp/notb64.txt`

### Steps
1. `tn tx submit-and-wait --url "$TN_URL" --file /tmp/bad.b64 --json; echo "exit=$?"`
2. `tn tx submit-and-wait --url "$TN_URL" --file /tmp/bad.bin --encoding borsh --json; echo "exit=$?"`
3. `tn tx submit-and-wait --url http://127.0.0.1:1 --file /tmp/bad.b64 --json; echo "exit=$?"`
4. `tn tx submit --url "$TN_URL" --file /tmp/bad.bin --encoding borsh --json; echo "exit=$?"`
5. `tn tx submit --url "$TN_URL" --file /tmp/notb64.txt --json; echo "exit=$?"`

### Success criteria
- Steps 1 and 2 print the same single object, `rejected` with `phase: stateless` (or
  `wire_mismatch`), and exit non-zero.
- Step 3 prints `transport_error`, not `rejected`, and exits non-zero.
- Step 4 prints one object whose `status` is `rejected` and exits non-zero.
- Step 5 exits non-zero with `invalid base64 transaction encoding` and prints no JSON.

## TRPC-003 — Block-scoped account and proof reads, and account snapshots

### Expected
`account get` reports `present` (lossless base64 bytes plus decoded balance, nonce, owner and
data), `missing` or `decode_error`, together with the `block_id` it was read at.
`account batch-get` reads every account at one block boundary. `--at-block N` pins the read.
`proof get` distinguishes a missing commitment (`proof: null`), an invalid commitment (a
local error before any RPC) and transport failure. `snapshot accounts --account-id …
--output PATH` writes a block-consistent `lgs-account-snapshot/1` JSON file that `state seed`
accepts.

### Preconditions
The shared node, and `CLOCK`. `NONE=11111111111111111111111111111111` is an id nobody wrote.

### Steps
1. `tn account get --url "$TN_URL" --account-id "$CLOCK" --json`
2. `tn account batch-get --url "$TN_URL" --account-id "$CLOCK" --account-id "$NONE" --json`
3. `tn proof get --url "$TN_URL" --commitment 00 --json; echo "exit=$?"; tn proof get --url "$TN_URL" --commitment "$(printf '0%.0s' {1..64})" --json`
4. `tn snapshot accounts --url "$TN_URL" --account-id "$CLOCK" --output /tmp/acc.json --json; jq '.format' /tmp/acc.json`

### Success criteria
- Step 1 reports `state: present` with `encoded` bytes, `program_owner`, `balance`, `nonce`,
  `data` and a `block_id`.
- Step 2: both entries share one `block_id`, and `$NONE` reads `state: missing`.
- Step 3: the first fails locally (`expected 64 hex characters or base58 of 32 bytes`); the
  second returns `proof: null` with a `root`.
- Step 4 reports `account_count: 1` with a `block_id`, and the file's format is
  `lgs-account-snapshot/1`.

## TRPC-005 — Start a node from exactly a seeded state

### Expected
`state schema` names the accepted formats (`lgs-state-snapshot/1`, `lgs-account-snapshot/1`,
or a rocksdb dir), the state format version (`nssa-v03`), the LEZ ref/commit and the seedable
fields. `state seed` validates the snapshot, writes a state dir and reports the seed kind,
LEZ commit, format version and account counts. `start --state` builds genesis from exactly
the snapshot, with none of the debug config's sample accounts. On LEZ v0.2.4 each seeded
public balance becomes a `supply_account` genesis action that credits the account's **vault
PDA**, not the account: `state seed` warns about that, and `account get` on the account
itself reads `missing`. The debug config's `supply_bridge_account` entry stays, and the
sequencer's built-in testnet state is always present; neither is a leak. `state export`
writes named public balances from a running node.

### Preconditions
TNODE-002. At the default pin, the debug-config sample accounts and their vault PDAs are:

| account | vault PDA | role here |
|---|---|---|
| `CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r` | `G1u9gmu1Gb8PG59zHZYNL5WdJNNtBKEN4U5qUdx2y6sG` | seeded with 4242 |
| `2RHZhw9h534Zr3eq2RGhQete2Hh667foECzXPmSkGni2` | `BAX2jjbhmT3CgDBi9MCFMgFsCeKECtaiyj1XDwXfZGrR` | left out of the seed |

### Steps
1. On an unseeded node (the shared one works): `tn account get --url "$TN_URL" --account-id BAX2jjbhmT3CgDBi9MCFMgFsCeKECtaiyj1XDwXfZGrR --json | jq '.state, .balance'`
2. `tn state schema --project "$P" --json`
3. `echo '{"format":"lgs-state-snapshot/1","public_accounts":[{"account_id":"CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r","balance":4242}],"private_accounts":[]}' > seed.json && tn state seed --project "$P" --input seed.json --output ./seeded --json`
4. `SJ=$(tn start --project "$P" --state ./seeded --json); SEEDED=$(echo "$SJ" | jq -r .rpc_url)`
5. `for a in G1u9gmu1Gb8PG59zHZYNL5WdJNNtBKEN4U5qUdx2y6sG CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r BAX2jjbhmT3CgDBi9MCFMgFsCeKECtaiyj1XDwXfZGrR; do tn account get --url "$SEEDED" --account-id $a --json | jq -c '{state, balance}'; done`
6. `tn state export --url "$SEEDED" --account-id G1u9gmu1Gb8PG59zHZYNL5WdJNNtBKEN4U5qUdx2y6sG --output /tmp/export.json --json; jq . /tmp/export.json`, then stop the node.

### Success criteria
- Step 1 reads `present`, balance 20000: the sample account exists without a seed. Without
  this control, step 5's `missing` proves nothing.
- Step 2 reports `nssa-v03` and the three accepted inputs. Step 3 writes `./seeded` and warns
  that the balance is credited to the vault PDA.
- Step 5 reads `present` / 4242 for the seeded vault, `missing` for the seeded account
  itself, and `missing` for the left-out vault: the seed replaced the sample accounts.
- Step 6 writes a snapshot naming the vault with balance 4242.

### Notes
For another pin or account, derive the vault: seed = `sha256("/LEZ/v0.3/VaultSeed/00000000000/"
‖ account_id)`, PDA = base58 of `sha256("/LEE/v0.2/AccountId/PDA/" ‖ 8 zero bytes ‖
vault_program_id ‖ seed)`, where `vault_program_id` is the `VAULT_ID` words (little-endian
`u32`s) in `<lez>/target/release/build/programs-*/out/lez/programs/mod.rs`. Pins up to LEZ
v0.2.3 seed through `initial_public_accounts` / `initial_private_accounts` instead, with no
default accounts. A `start` that fails with ``sequencer_config.json has no numeric
`genesis_id` `` means scaffold read a v0.2.4 config the older way. A state dir holding a
node's preserved `rocksdb/` resumes from it verbatim (TRPC-007); the JSON snapshot carries
public balances only, because the pinned RPC exposes nothing else.

## TRPC-006 — Bad snapshots are rejected up front

### Expected
`state seed` rejects an unknown format with `format mismatch`; a `public_accounts` entry with
any field other than `account_id`/`balance` with `storage schema mismatch` naming the field;
non-empty `private_accounts` on LEZ v0.2.4 with `storage schema mismatch`; a `lez_commit`
different from the project pin with `lez pin mismatch`; and undecodable accounts with
`account decode error`. All exit non-zero before anything is created: no output dir, no
`seeds/seed-*` dir, no node.

### Preconditions
TNODE-001. No node needed.

### Steps
1. Seed each bad snapshot and print the first error line:
   ```bash
   H='"format":"lgs-state-snapshot/1"'; ID=CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r
   NPK=$(seq -s, 1 32)
   bad() { echo "$2" > /tmp/bad-seed.json; tn state seed --project "$P" --input /tmp/bad-seed.json --output ./rejected 2>&1 | grep -m1 -E 'mismatch|decode error'; echo "$1 exit=${PIPESTATUS[0]}"; }
   bad format  '{"format":"bogus/1"}'
   bad field   "{$H,\"public_accounts\":[{\"account_id\":\"$ID\",\"balance\":1,\"nonce\":3}]}"
   bad private "{$H,\"public_accounts\":[],\"private_accounts\":[{\"npk\":[$NPK],\"account\":{\"balance\":5}}]}"
   bad pin     "{$H,\"lez_commit\":\"deadbeef\",\"public_accounts\":[{\"account_id\":\"$ID\",\"balance\":1}]}"
   bad id      "{$H,\"public_accounts\":[{\"account_id\":\"not-base58!!\",\"balance\":1}]}"
   ```
2. `ls -d ./rejected "$P"/.scaffold/test-nodes/seeds/seed-* 2>&1`

### Success criteria
- In order: `format mismatch`; `storage schema mismatch` naming `nonce`; `storage schema
  mismatch` (private accounts); `lez pin mismatch`; `account decode error … is not base58`.
  Every line ends in a non-zero exit.
- Step 2 finds neither `./rejected` nor any `seeds/seed-*` dir. (An existing `--output` dir
  is never removed.)

### Notes
A private entry needs `npk` (32 numbers) and `account`; a malformed one fails earlier with
`account decode error: missing field \`npk\``, which is not the check intended here.

## TRPC-007 — A real wallet transaction is observed as committed, also after a database restart

### Expected
A test node on the wallet's port executes a real `wallet topup` (`auth-transfer init` plus
`pinata claim`). `blocks wait` then shows blocks with `has_user_transactions: true`,
`transaction_count: 2`, and a non-clock `kind: "public"` tx with a real hash, and `tx wait
--hash` reports it `committed` with its block. After the node is stopped with
`--preserve-work-dir` and restarted with `--state <its dir>`, `tx wait --hash <old>
--after-block <b>` still reports `committed` with the original block, even when `<b>` is
later than the commit block: scaffold scans blocks when the node's `getTransaction` has no
answer. If a scanned block holds a privacy-preserving tx, the result is `timeout`, never a
stateful rejection.

### Preconditions
TNODE-002, plus BUILD-001 in the same project (wallet built, default wallet seeded).
Localnet stopped, so port 3040 is free.

### Steps
1. `SJ=$(tn start --project "$P" --port 3040 --preserve-work-dir --json); URL=$(echo "$SJ" | jq -r .rpc_url); DIR=$(echo "$SJ" | jq -r .state_dir)`
2. `HEAD0=$(tn blocks head --url "$URL" --json | jq -r .block_id); ( cd "$P" && "$SCAFFOLD_BIN" wallet topup )`
3. `tn blocks wait --url "$URL" --after "$HEAD0" --count 6 --timeout-sec 90 --json > /tmp/blocks.json; jq -c '.blocks[] | select(.has_user_transactions) | {block_id, transaction_count}' /tmp/blocks.json; jq '[.blocks[].fully_parsed] | all' /tmp/blocks.json`
4. `HASH=$(jq -r '[.blocks[].transactions[] | select(.is_clock | not) | .hash][0]' /tmp/blocks.json); tn tx wait --url "$URL" --hash "$HASH" --after-block "$HEAD0" --json`
5. `tn stop --node "$DIR" --project "$P" --preserve-work-dir; SJ=$(tn start --project "$P" --state "$DIR" --json); URL2=$(echo "$SJ" | jq -r .rpc_url)`
6. `LATER=$(tn blocks head --url "$URL2" --json | jq -r .block_id); tn tx wait --url "$URL2" --hash "$HASH" --after-block "$LATER" --json`,
   then stop the resumed node and `rm -rf "$DIR"`.

### Success criteria
- `topup` exits 0. At least one block has 2 transactions including a non-clock public tx, and
  the `all` check over every block (clock-only ones included) prints `true`.
- Step 4 prints `committed` with that block's `block_id` and a `timestamp`, and exits 0.
- Step 5's node resumes at the old height, and step 6 prints `committed` with the same
  `block_id` as step 4.

### Notes
If blocks keep coming but no user tx lands, r0vm is probably missing or the wrong version.
The wallet talks to the `sequencer_addr` in its config, so the node must be on that port; if
`topup` prints a sequencer-unreachable hint instead, that hint must name the configured
port. A user-tx block classified as clock-only, or `fully_parsed: false` on a plain public
tx, is a wire-format regression: this is the only scenario that checks the block parser
against a really executed transaction. At the default pin a resumed node still answers
`getTransaction` for old transactions; older pins forgot them. Both must give `committed`.
