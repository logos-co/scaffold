---
feature: wallet
prefix: WALLET
surface: cli
scenarios:
  - id: WALLET-001
    title: Top up the default wallet without an address
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D1
    depends_on: [BUILD-001, LOCALNET-001]
  - id: WALLET-002
    title: List wallet accounts
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D4
    depends_on: [BUILD-001, LOCALNET-001]
  - id: WALLET-003
    title: Set the project default address
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D4
    depends_on: [WALLET-002]
  - id: WALLET-004
    title: Topup dry run renders the command
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D4
    depends_on: [WALLET-001]
  - id: WALLET-005
    title: Passthrough uses the project wallet home
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D4
    depends_on: [BUILD-001, LOCALNET-001]
  - id: WALLET-006
    title: Invalid or conflicting addresses are rejected
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D4
    depends_on: [BUILD-001]
  - id: WALLET-007
    title: Wallet password override
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D4
    depends_on: [WALLET-001]
  - id: WALLET-008
    title: Topup follows the wallet's configured sequencer
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D4
    depends_on: [WALLET-001]
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
3. `"$SCAFFOLD_BIN" wallet -- account list`

### Success criteria
- Exit 0, and no "no destination configured" message.
- Step 2 prints an address that appears in `wallet list`.
- In step 3 that address is on a `/ `-prefixed (stored account) line. Seeding adopts only
  those; an address matching a `Preconfigured …` line is worth reporting.

## WALLET-002 — List wallet accounts

### Expected
`wallet list` (and `--long`, `--json`) lists the accounts in the project wallet home.

### Preconditions
BUILD-001 and LOCALNET-001.

### Steps
1. `"$SCAFFOLD_BIN" wallet list`
2. `"$SCAFFOLD_BIN" wallet list --long`
3. `"$SCAFFOLD_BIN" wallet list --json | jq .`

### Success criteria
- At least the default `Public/<id>` account is listed.
- Step 3 is valid JSON on stdout with no command echoes, listing the same accounts: `.accounts`
  holds exactly the account lines (`/ Public/<id>`, …), not `Stored statistics at …`.

## WALLET-003 — Set the project default address

### Expected
`wallet default set <addr>` and `wallet default set --address <addr>` both persist the
normalized address as the project default.

### Preconditions
WALLET-002. Take an address from its output.

### Steps
1. `"$SCAFFOLD_BIN" wallet default set Public/<id>`
2. `grep default_address .scaffold/state/wallet.state`
3. Repeat with `"$SCAFFOLD_BIN" wallet default set --address Public/<other-id>` and step 2.

### Success criteria
- After each form, the state file holds the address just set.

## WALLET-004 — Topup dry run renders the command

### Expected
`wallet topup --dry-run` prints the faucet claim command it would run and changes nothing.

### Preconditions
WALLET-001 (dry run still resolves the default address first).

### Steps
1. `"$SCAFFOLD_BIN" wallet topup --dry-run`
2. `"$SCAFFOLD_BIN" wallet topup --dry-run --json | jq .`

### Success criteria
- The output shows a `pinata claim` command line, and no transaction is submitted.
- Step 2 is a single JSON object on stdout with no `$ <cmd>` echoes, naming the destination
  and the `network`.

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
4. `"$SCAFFOLD_BIN" wallet account list; echo "exit=$?"` (no `--`)
5. Temporarily rename `<lez>/target/release/wallet`, run `"$SCAFFOLD_BIN" wallet list; echo "exit=$?"`,
   then restore it.

### Success criteria
- Steps 1 and 2 exit 0, and step 2 lists the project's accounts, not a `~/.lee/wallet` set.
- `.scaffold/wallet` holds account storage, not only `wallet_config.json`.
- Step 4 is rejected by argument parsing; nothing is forwarded to the wallet.
- Step 5 fails with `missing wallet binary at <lez>/target/release/wallet. Run
  \`logos-scaffold setup\`.`, and works without any `wallet` on PATH.

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
WALLET-001.

### Steps
1. `LOGOS_SCAFFOLD_WALLET_PASSWORD=custom-pw "$SCAFFOLD_BIN" wallet topup --dry-run`

### Success criteria
- The command honours the override, either by showing it in the rendered command or by
  behaving differently from the default. Record which.

## WALLET-008 — Topup follows the wallet's configured sequencer

### Expected
`wallet topup` targets the sequencer the wallet config names: `sequencers[0].sequencer_addr`
from LEZ v0.2.4, or the top-level `sequencer_addr` before that. It falls back to
`127.0.0.1:<[localnet].port>` only when the config names none. From v0.2.4 the wallet reports
a dead sequencer only as `Failed to find leader`. When an RPC probe confirms the sequencer is
down, topup maps that to the sequencer-unavailable hint naming the configured address. With
the sequencer up, the wallet error stays as it is.

### Preconditions
WALLET-001 (topup needs a default address before it reaches the network). Back up
`.scaffold/wallet/wallet_config.json` first.

### Steps
1. Set `sequencers[0].sequencer_addr` in `.scaffold/wallet/wallet_config.json` to
   `http://127.0.0.1:3999` (a closed port).
2. `"$SCAFFOLD_BIN" wallet topup --dry-run`
3. `"$SCAFFOLD_BIN" wallet topup; echo "exit=$?"`
4. Restore the config.

### Success criteria
- Step 2's `planned network:` line names `http://127.0.0.1:3999`.
- Step 3 exits non-zero, and its output contains
  `sequencer appears unavailable at http://127.0.0.1:3999` (the hint goes on with two more
  lines). A bare `Failed to find leader` followed by an account-verification hint is the
  regression.

### Notes
Unlike `deploy`, topup has no reachability preflight, so against an unrouted address it first
waits out the wallet's own timeout (about a minute). Don't treat that delay as a failure.
As in DEPLOY-006, the probe only checks that something answered, so a foreign process on the
configured port suppresses the hint.
