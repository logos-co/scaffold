---
feature: wallet
prefix: WALLET
surface: cli
scenarios:
  - id: WALLET-001
    title: Top up the default wallet without an address
    priority: P1
    provenance: documented
    source: DOGFOODING.md D1
    depends_on: [BUILD-001, LOCALNET-001]
  - id: WALLET-002
    title: List wallet accounts
    priority: P1
    provenance: documented
    source: DOGFOODING.md D4
    depends_on: [BUILD-001, LOCALNET-001]
  - id: WALLET-003
    title: Set the project default address
    priority: P2
    provenance: documented
    source: DOGFOODING.md D4
    depends_on: [WALLET-002]
  - id: WALLET-004
    title: Topup dry run renders the command
    priority: P2
    provenance: documented
    source: DOGFOODING.md D4
    depends_on: [BUILD-001]
  - id: WALLET-005
    title: Passthrough uses the project wallet home
    priority: P1
    provenance: documented
    source: DOGFOODING.md D4
    depends_on: [BUILD-001, LOCALNET-001]
  - id: WALLET-006
    title: Invalid or conflicting addresses are rejected
    priority: P2
    provenance: documented
    source: DOGFOODING.md D4
    depends_on: [BUILD-001]
  - id: WALLET-007
    title: Wallet password override
    priority: P2
    provenance: documented
    source: DOGFOODING.md D4
    depends_on: [BUILD-001]
retired: []
---

# Wallet

The wallet is the LEZ-local `<lez>/target/release/wallet`, always run with the project wallet
home `.scaffold/wallet`. That home is exported under both `NSSA_WALLET_HOME_DIR` and
`LEE_WALLET_HOME_DIR`. No global `wallet` install is needed. The project default address
lives in `.scaffold/state/wallet.state`. Since LEZ v0.2.4 every wallet command needs a
running sequencer. Related: [Setup and build](setup-and-build.md),
[Example runners](example-runners.md), [Run pipeline](run-pipeline.md).

## WALLET-001 — Top up the default wallet without an address

### Expected
`wallet topup` with no address uses the seeded project default. It runs `wallet account get`,
then `auth-transfer init` only if the account is uninitialized, then `pinata claim`, and exits
0. `.scaffold/state/wallet.state` has a `default_address=` that `wallet list` reports.

### Preconditions
BUILD-001 and LOCALNET-001. Re-run `setup` once if seeding was deferred.

### Steps
1. `"$SCAFFOLD_BIN" wallet topup`
2. `grep default_address .scaffold/state/wallet.state`

### Success criteria
- Exit 0, and no "no destination configured" message.
- Step 2 prints an address that appears in `wallet list`.

## WALLET-002 — List wallet accounts

### Expected
`wallet list` (and `--long`, `--json`) lists the accounts in the project wallet home.

### Preconditions
BUILD-001 and LOCALNET-001.

### Steps
1. `"$SCAFFOLD_BIN" wallet list`
2. `"$SCAFFOLD_BIN" wallet list --long`

### Success criteria
- At least the default `Public/<id>` account is listed.

## WALLET-003 — Set the project default address

### Expected
`wallet default set <addr>` and `wallet default set --address <addr>` both persist the
normalized address as the project default.

### Preconditions
WALLET-002. Take an address from its output.

### Steps
1. `"$SCAFFOLD_BIN" wallet default set Public/<id>`
2. `grep default_address .scaffold/state/wallet.state`

### Success criteria
- The state file holds the new address.

## WALLET-004 — Topup dry run renders the command

### Expected
`wallet topup --dry-run` prints the faucet claim command it would run and changes nothing.

### Preconditions
BUILD-001.

### Steps
1. `"$SCAFFOLD_BIN" wallet topup --dry-run`

### Success criteria
- The output shows a `pinata claim` command line, and no transaction is submitted.

## WALLET-005 — Passthrough uses the project wallet home

### Expected
`wallet -- <args>` forwards the raw arguments to the LEZ-local wallet binary with the project
wallet home under both env names. It requires the literal `--`.

### Preconditions
BUILD-001 and LOCALNET-001.

### Steps
1. `"$SCAFFOLD_BIN" wallet -- check-health`
2. `"$SCAFFOLD_BIN" wallet -- account list`
3. `ls .scaffold/wallet`

### Success criteria
- Both commands exit 0, and step 2 lists the project's accounts, not a `~/.lee/wallet` set.
- `.scaffold/wallet` holds account storage, not only `wallet_config.json`.

### Notes
A lone `wallet_config.json` after a wallet command means storage silently went to
`~/.lee/wallet`.

## WALLET-006 — Invalid or conflicting addresses are rejected

### Expected
A malformed address is rejected with an `Accepted formats` hint. Passing both a positional
address and `--address` is a clear usage error.

### Preconditions
BUILD-001.

### Steps
1. `"$SCAFFOLD_BIN" wallet default set not-an-address; echo "exit=$?"`
2. `"$SCAFFOLD_BIN" wallet topup Public/abc --address Public/def; echo "exit=$?"`

### Success criteria
- Both exit non-zero. Step 1 mentions `Accepted formats`.

## WALLET-007 — Wallet password override

### Expected
Wallet-facing commands use `LOGOS_SCAFFOLD_WALLET_PASSWORD` instead of the local dev default
when it is set.

### Preconditions
BUILD-001.

### Steps
1. `LOGOS_SCAFFOLD_WALLET_PASSWORD=custom-pw "$SCAFFOLD_BIN" wallet topup --dry-run`

### Success criteria
- The command honours the override, either by showing it in the rendered command or by
  behaving differently from the default. Record which.
