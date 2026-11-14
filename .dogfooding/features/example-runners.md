---
feature: example-runners
prefix: RUNNER
surface: cli
scenarios:
  - id: RUNNER-001
    title: run_hello_world writes the greeting on chain
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D6
    depends_on: [DEPLOY-001, WALLET-005]
  - id: RUNNER-002
    title: run_hello_world_with_move_function writes a custom message
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D6
    depends_on: [DEPLOY-001, WALLET-005]
  - id: RUNNER-003
    title: Bad runner input fails with a clear error
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D6
    depends_on: [DEPLOY-001]
  - id: RUNNER-004
    title: A private runner's write is visible after sync-private
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D6
    depends_on: [DEPLOY-001, WALLET-005]
  - id: RUNNER-005
    title: Tail-call runner lands its greeting with build-synced callee ids
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D6
    depends_on: [DEPLOY-001, WALLET-005]
retired: []
---

# Example runners

The default template ships `cargo run --bin run_*` clients for its example programs. They are
the final proof that the pipeline works: a deploy means nothing if a client cannot change
state through it. Related: [Deploy](deploy.md), [Wallet](wallet.md).

## RUNNER-001 — run_hello_world writes the greeting on chain

### Expected
Running `run_hello_world` against a fresh public account prints `submitted transaction:
tx_hash=…` and a `verification hint:` line, and exits 0. Once the block lands, the account's
`data` is hex for `Hola mundo!` and its `program_owner` is the hello_world program.

### Preconditions
DEPLOY-001 and a fresh account: `"$SCAFFOLD_BIN" wallet -- account new public` (call it A).

### Steps
1. `export NSSA_WALLET_HOME_DIR="$PWD/.scaffold/wallet" LEE_WALLET_HOME_DIR="$PWD/.scaffold/wallet"`
2. `cargo run --bin run_hello_world -- <A-base58>`
3. Poll `"$SCAFFOLD_BIN" wallet -- account get --account-id Public/<A-base58>` for up to
   about 30 seconds.

### Success criteria
- The runner exits 0 and prints a `tx_hash`.
- The account `data` decodes to `Hola mundo!`, and `program_owner` is non-null.

### Notes
Decode `data` portably with `python3 -c 'import sys,json,re; print(bytes.fromhex(re.search(r"\"data\":\"([0-9a-f]*)\"", sys.stdin.read()).group(1)).decode())'`
(minimal hosts have no `xxd`).
Export both wallet-home names. With only `NSSA_…`, a v0.2.x wallet silently uses
`~/.lee/wallet`.
The runner takes the bare base58 id. `wallet account get` needs the full `Public/<base58>`
form; the bare id fails with `Unsupported privacy kind`. Reads lag block production (the
default localnet block interval is 15 seconds), so poll before recording a failure. A
pre-v0.2.0 LEZ pin given only `LEE_…` has no wallet home at all and fails loudly in
`WalletCore::from_env()`.

## RUNNER-002 — run_hello_world_with_move_function writes a custom message

### Expected
`run_hello_world_with_move_function write-public <B> "<msg>"` stores the message in account
B's `data`.

### Preconditions
DEPLOY-001 and a second fresh account B. Do not reuse A: the first program to write an
account owns it, and a later write by another program is rejected with
`UnauthorizedDataModification`.

### Steps
1. `cargo run --bin run_hello_world_with_move_function -- write-public <B-base58> "dogfood-test-message"`
2. Poll `"$SCAFFOLD_BIN" wallet -- account get --account-id Public/<B-base58>`.

### Success criteria
- Exit 0 with a `submitted transaction: tx_hash=…` line and a `verification hint:` line
  pointing at `wallet account get`.
- B's `data` is the hex of `dogfood-test-message`.

### Notes
A runner can exit 0 while execution was rejected. Check the sequencer log for `failed
execution check` if the state does not change.

## RUNNER-003 — Bad runner input fails with a clear error

### Expected
A non-base58 account ID gives a clear parse error, not a panic. With localnet down, the
runner fails with a connection error.

### Preconditions
DEPLOY-001.

### Steps
1. `cargo run --bin run_hello_world -- not-base58!; echo "exit=$?"`
2. `"$SCAFFOLD_BIN" localnet stop; cargo run --bin run_hello_world -- <A-base58>; echo "exit=$?"`,
   then `"$SCAFFOLD_BIN" localnet start`.

### Success criteria
- Non-zero exit with a readable error, and no `panicked at` / `unwrap()` backtrace.
- Step 2 exits non-zero with a connection error; record its exact text.

## RUNNER-004 — A private runner's write is visible after sync-private

### Expected
`run_hello_world_private <P>` exits 0 and prints `tx_hash=…` and two hints:
`wallet account sync-private` and `wallet account get --account-id Private/<P>`. After the
block lands, `account sync-private` followed by that `account get` shows the wallet's
decrypted copy of P with `data` = hex of `Hola mundo!` and the hello_world `program_owner`.

### Preconditions
DEPLOY-001 and a fresh private account: `"$SCAFFOLD_BIN" wallet -- account new private`
(call it P).

### Steps
1. Export both wallet-home names, as in RUNNER-001.
2. `RISC0_DEV_MODE=1 cargo run --bin run_hello_world_private -- <P-base58>`
3. `"$SCAFFOLD_BIN" wallet -- account sync-private`, then
   `"$SCAFFOLD_BIN" wallet -- account get --account-id Private/<P-base58>`. Repeat both for up
   to about 30 seconds.

### Success criteria
- The runner exits 0 and prints both hints.
- After a sync, P's `data` decodes to `Hola mundo!` and `program_owner` is non-null.

### Notes
The chain holds only a commitment for P; `account get` on a `Private/` ID reads the wallet's
local copy, which only `sync-private` updates. `Account is Uninitialized` right after the
runner means the block had not landed when you synced, not a failure. `RISC0_DEV_MODE=1`
matches the localnet's dev-mode proving; without it the host builds a real proof.
The other private paths (`run_hello_world_with_move_function write-private` and
`move-data-public-to-private`, `run_hello_world_through_tail_call_private`) are checked the
same way. Use a fresh private account per runner. Unchanged state after several blocks is a
finding.

## RUNNER-005 — Tail-call runner lands its greeting with build-synced callee ids

### Expected
The tail-call callers embed their callees' program ids. `build` rewrites
`HELLO_WORLD_PROGRAM_ID_HEX` in `simple_tail_call.rs` and
`HELLO_WORLD_WITH_AUTHORIZATION_PROGRAM_ID_HEX` in `tail_call_with_pda.rs` to match the build.
It rebuilds once and prints `Updated … program id …`. An unchanged rebuild prints nothing.
`run_hello_world_through_tail_call` signs its transaction, and on a fresh account it lands
`Hello from tail call`.

### Preconditions
DEPLOY-001 and a fresh account: `"$SCAFFOLD_BIN" wallet -- account new public` (call it A).

### Steps
1. `"$SCAFFOLD_BIN" build 2>&1 | grep 'program id'`, run twice.
2. `export NSSA_WALLET_HOME_DIR="$PWD/.scaffold/wallet" LEE_WALLET_HOME_DIR="$PWD/.scaffold/wallet"`
3. `cargo run --bin run_hello_world_through_tail_call -- <A-base58>`
4. Poll `"$SCAFFOLD_BIN" wallet -- account get --account-id Public/<A-base58>` for up to
   about 30 seconds, then `grep -c 'Unknown program' .scaffold/logs/sequencer.log`.

### Success criteria
- The second run of step 1 prints no `Updated … program id` line.
- The account `data` decodes to `Hello from tail call`.
- The sequencer log has no `Unknown program` rejection for the tail-call transaction, and no
  `ClaimedUnauthorizedAccount`.
