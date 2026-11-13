---
feature: example-runners
prefix: RUNNER
surface: cli
scenarios:
  - id: RUNNER-001
    title: run_hello_world writes the greeting on chain
    priority: P1
    provenance: documented
    source: DOGFOODING.md D6
    depends_on: [DEPLOY-001, WALLET-005]
  - id: RUNNER-002
    title: run_hello_world_with_move_function writes a custom message
    priority: P1
    provenance: documented
    source: DOGFOODING.md D6
    depends_on: [DEPLOY-001, WALLET-005]
  - id: RUNNER-003
    title: Bad runner input fails with a clear error
    priority: P2
    provenance: documented
    source: DOGFOODING.md D6
    depends_on: [DEPLOY-001]
  - id: RUNNER-004
    title: A private runner's write is visible after sync-private
    priority: P1
    provenance: documented
    source: DOGFOODING.md D6
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
Export both wallet-home names. With only `NSSA_…`, a v0.2.x wallet silently uses
`~/.lee/wallet`.

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
- Exit 0, and B's `data` is the hex of `dogfood-test-message`.

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

### Success criteria
- Non-zero exit with a readable error, and no `panicked at` / `unwrap()` backtrace.

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
