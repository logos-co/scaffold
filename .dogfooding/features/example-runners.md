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
