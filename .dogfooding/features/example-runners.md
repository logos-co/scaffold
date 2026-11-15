---
feature: example-runners
prefix: RUNNER
surface: cli
scenarios:
  - id: RUNNER-001
    title: Public runners write their data on chain
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D6
    depends_on: [DEPLOY-001]
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
    depends_on: [DEPLOY-001]
  - id: RUNNER-005
    title: Tail-call runner lands its greeting with build-synced callee ids
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D6
    depends_on: [DEPLOY-001]
retired: [RUNNER-002]
---

# Example runners

The default template ships `cargo run --bin run_*` clients for its example programs. They are
the final proof that the pipeline works: a deploy means nothing if a client cannot change
state through it. Related: [Deploy](deploy.md), [Wallet](wallet.md).

Common to every scenario here:

```bash
export NSSA_WALLET_HOME_DIR="$PWD/.scaffold/wallet" LEE_WALLET_HOME_DIR="$PWD/.scaffold/wallet"
acct() { "$SCAFFOLD_BIN" -q wallet -- account new "$1" | grep -o 'account_id [A-Za-z]*/[A-Za-z0-9]*' | cut -d/ -f2; }   # acct public → bare base58 id
data() { "$SCAFFOLD_BIN" -q wallet -- account get --account-id "$1" | python3 -c 'import sys,re; m=re.search(r"\"data\":\"([0-9a-f]*)\"", sys.stdin.read()); print(bytes.fromhex(m.group(1)).decode() if m else "no data yet")'; }
```

- Export both wallet-home names: with only `NSSA_…`, a v0.2.x wallet silently uses
  `~/.lee/wallet`.
- Runners take the bare base58 id; `wallet account get` needs `Public/<id>` or `Private/<id>`
  (a bare id fails with `Unsupported privacy kind`).
- Use a fresh account per runner. The first program to write an account owns it, and a later
  write by another program is rejected with `UnauthorizedDataModification`.
- Reads lag block production (15 seconds by default): poll `data` for up to about 90 seconds
  before recording a failure. A runner can exit 0 while execution was rejected; if the state
  never changes, check `.scaffold/logs/sequencer.log` for `failed execution check`.
- The first `cargo run` compiles the clients (about a minute).

## RUNNER-001 — Public runners write their data on chain

### Expected
`run_hello_world <A>` and `run_hello_world_with_move_function write-public <B> "<msg>"` each
print `submitted transaction: tx_hash=…` and a `verification hint:` line pointing at `wallet
account get`, and exit 0. Once the block lands, A's `data` is `Hola mundo!`, B's is the
message, and each account's `program_owner` is set.

### Preconditions
DEPLOY-001, and two fresh accounts: `A=$(acct public); B=$(acct public)`.

### Steps
1. `cargo run --bin run_hello_world -- "$A"`
2. `cargo run --bin run_hello_world_with_move_function -- write-public "$B" "dogfood-test-message"`
3. Poll `data Public/$A; data Public/$B`, then
   `"$SCAFFOLD_BIN" wallet -- account get --account-id Public/$A | grep -o '"program_owner":[^,]*'`

### Success criteria
- Both runners exit 0 and print a `tx_hash` and the verification hint.
- `data` prints `Hola mundo!` for A and `dogfood-test-message` for B, and `program_owner` is
  non-null.

## RUNNER-003 — Bad runner input fails with a clear error

### Expected
A non-base58 account ID gives a clear parse error, not a panic. With localnet down, the
runner fails with a connection error.

### Preconditions
DEPLOY-001 and any account id `A`.

### Steps
1. `cargo run --bin run_hello_world -- not-base58!; echo "exit=$?"`
2. `"$SCAFFOLD_BIN" localnet stop; cargo run --bin run_hello_world -- "$A"; echo "exit=$?"; "$SCAFFOLD_BIN" localnet start`

### Success criteria
- Step 1 exits non-zero with `invalid account_id … (expected base58 account id)`, and no
  `panicked at` / `unwrap()` backtrace.
- Step 2 exits non-zero with a connection error (`failed to query account nonce from
  sequencer … Connection refused`); record its exact text.

## RUNNER-004 — A private runner's write is visible after sync-private

### Expected
`run_hello_world_private <P>` exits 0 and prints `tx_hash=…` and two hints:
`wallet account sync-private` and `wallet account get --account-id Private/<P>`. After the
block lands, `account sync-private` followed by that `account get` shows the wallet's
decrypted copy of P with `data` = `Hola mundo!` and the hello_world `program_owner`.

### Preconditions
DEPLOY-001 and a fresh private account: `P=$(acct private)`.

### Steps
1. `grep -A1 'name = "risc0-zkvm"' Cargo.lock` (expect the provisioned `r0vm`'s version, 3.0.5)
2. `RISC0_DEV_MODE=1 cargo run --bin run_hello_world_private -- "$P"`
3. Poll `"$SCAFFOLD_BIN" -q wallet -- account sync-private >/dev/null; data Private/$P`

### Success criteria
- The runner exits 0 and prints both hints.
- After a sync, `data` prints `Hola mundo!` and `program_owner` is non-null.

### Notes
The chain holds only a commitment for P; `account get` on a `Private/` ID reads the wallet's
local copy, which only `sync-private` updates, so `Account is Uninitialized` right after the
runner only means the block had not landed. `RISC0_DEV_MODE=1` matches the localnet's
dev-mode proving; without it the host builds a real proof. A host `risc0-zkvm` newer than
`r0vm` fails with `Your installation of the r0vm server is not compatible with your host's
risc0-zkvm crate`: the template pins both with `=`, and an older project needs `cargo update
-p risc0-zkvm --precise 3.0.5`, then `lgs build` and `lgs deploy`. The other private paths
(`run_hello_world_with_move_function write-private` and `move-data-public-to-private`,
`run_hello_world_through_tail_call_private`) are checked the same way when their code
changes.

## RUNNER-005 — Tail-call runner lands its greeting with build-synced callee ids

### Expected
The tail-call callers embed their callees' program ids. `build` rewrites
`HELLO_WORLD_PROGRAM_ID_HEX` in `simple_tail_call.rs` and
`HELLO_WORLD_WITH_AUTHORIZATION_PROGRAM_ID_HEX` in `tail_call_with_pda.rs` to match the build,
rebuilds once and prints `Updated … program id …`. An unchanged rebuild prints nothing.
`run_hello_world_through_tail_call` signs its transaction, and on a fresh account it lands
`Hello from tail call`.

### Preconditions
DEPLOY-001 and a fresh account: `T=$(acct public)`.

### Steps
1. `"$SCAFFOLD_BIN" build 2>&1 | grep -c 'Updated .* program id'` (the project is already
   built, so this is the unchanged rebuild)
2. `cargo run --bin run_hello_world_through_tail_call -- "$T"`
3. Poll `data Public/$T`, then
   `grep -c -E 'Unknown program|ClaimedUnauthorizedAccount' .scaffold/logs/sequencer.log`

### Success criteria
- Step 1 prints `0`. (The first build of a fresh project is where the `Updated …` lines
  appear: look for them in BUILD-002's or BUILD-005's output.)
- `data` prints `Hello from tail call`, and step 3's count is `0`.
