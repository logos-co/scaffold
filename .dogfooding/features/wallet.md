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
    title: List accounts and pass raw commands through to the project wallet
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D4
    depends_on: [BUILD-001, LOCALNET-001]
  - id: WALLET-003
    title: Set the project default address and reject bad addresses
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D4
    depends_on: [WALLET-002]
  - id: WALLET-004
    title: Topup dry run, and topup follows the wallet's configured sequencer
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D4
    depends_on: [WALLET-001]
  - id: WALLET-007
    title: Wallet password override
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D4
    depends_on: [WALLET-001]
retired: [WALLET-005, WALLET-006, WALLET-008]
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
0. `.scaffold/state/wallet.state` has a `default_address=` that the wallet lists as a stored
account.

### Preconditions
BUILD-001 and LOCALNET-001. Re-run `setup` once if seeding was deferred.

### Steps
1. `"$SCAFFOLD_BIN" wallet topup`
2. `grep default_address .scaffold/state/wallet.state; "$SCAFFOLD_BIN" wallet -- account list`

### Success criteria
- Exit 0, and no "no destination configured" message.
- The default address is on a `/ `-prefixed (stored account) line of the list. Seeding adopts
  only those; an address matching a `Preconfigured …` line is worth reporting.

### Notes
A first topup waits for two blocks (about a minute at the default 15-second interval).

## WALLET-002 — List accounts and pass raw commands through to the project wallet

### Expected
`wallet list` (and `--long`, `--json`) lists the accounts in the project wallet home.
`wallet -- <args>` forwards the raw arguments to the LEZ-local wallet binary with the project
wallet home under both env names. It requires the literal `--`, and it needs no `wallet` on
`PATH`.

### Preconditions
BUILD-001 and LOCALNET-001.

### Steps
1. `"$SCAFFOLD_BIN" wallet list; "$SCAFFOLD_BIN" wallet list --long`
2. `"$SCAFFOLD_BIN" wallet list --json | jq .accounts`
3. `"$SCAFFOLD_BIN" wallet -- check-health; "$SCAFFOLD_BIN" wallet -- account list; ls .scaffold/wallet`
4. `"$SCAFFOLD_BIN" wallet account list; echo "exit=$?"` (no `--`)
5. `W="$("$SCAFFOLD_BIN" test-node pins --json | jq -r .lez_checkout)/target/release/wallet"; mv "$W" "$W.off"; "$SCAFFOLD_BIN" wallet list; echo "exit=$?"; mv "$W.off" "$W"`

### Success criteria
- Step 1 lists at least the default `Public/<id>` account; `--long` adds balances.
- Step 2 is pure JSON with no command echoes, and `.accounts` holds exactly the account
  lines (`/ Public/<id>`, …), not `Stored statistics at …`.
- Step 3: both commands exit 0, the list shows the same accounts as step 1 (not a
  `~/.lee/wallet` set), and `.scaffold/wallet` holds account storage, not only
  `wallet_config.json` (a lone config means storage silently went to `~/.lee/wallet`).
- Step 4 is rejected by argument parsing; nothing is forwarded to the wallet.
- Step 5 fails with `missing wallet binary at <lez>/target/release/wallet. Run
  \`logos-scaffold setup\`.`

## WALLET-003 — Set the project default address and reject bad addresses

### Expected
`wallet default set <addr>` and `wallet default set --address <addr>` both persist the
normalized address as the project default; a bare base58 id is accepted. A malformed address
is rejected with an `Accepted formats` hint. Passing both a positional address and
`--address` is a clear usage error.

### Preconditions
WALLET-002. A fresh wallet has one public account, so create a second:
`"$SCAFFOLD_BIN" wallet -- account new public`. Note the current default to restore it.

### Steps
1. `"$SCAFFOLD_BIN" wallet default set Public/<new-id> && grep default_address .scaffold/state/wallet.state`
2. `"$SCAFFOLD_BIN" wallet default set --address Public/<original-id> && grep default_address .scaffold/state/wallet.state`
3. `"$SCAFFOLD_BIN" wallet default set not-an-address; echo "exit=$?"`
4. `"$SCAFFOLD_BIN" wallet topup Public/abc --address Public/def; echo "exit=$?"`

### Success criteria
- After steps 1 and 2 the state file holds the address just set.
- Step 3 exits non-zero and mentions `Accepted formats`. Step 4 exits non-zero with `provide
  address either as positional argument or \`--address\`, not both`.

## WALLET-004 — Topup dry run, and topup follows the wallet's configured sequencer

### Expected
`wallet topup --dry-run` prints the faucet claim command it would run and submits nothing;
`--json` gives one object. Topup targets the sequencer the wallet config names:
`sequencers[0].sequencer_addr` from LEZ v0.2.4, or the top-level `sequencer_addr` before
that. It falls back to `127.0.0.1:<[localnet].port>` only when the config names none. From
v0.2.4 the wallet reports a dead sequencer only as `Failed to find leader`; when an RPC probe
confirms the sequencer is down, topup maps that to the sequencer-unavailable hint naming the
configured address. With the sequencer up, the wallet error stays as it is.

### Preconditions
WALLET-001 (topup resolves the default address before it reaches the network). Back up
`.scaffold/wallet/wallet_config.json`.

### Steps
1. `"$SCAFFOLD_BIN" wallet topup --dry-run`
2. `"$SCAFFOLD_BIN" wallet topup --dry-run --json | jq -e '.status == "dry_run" and .address and .network'`
3. Set `sequencers[0].sequencer_addr` to `http://127.0.0.1:3999` (a closed port), then
   `"$SCAFFOLD_BIN" wallet topup --dry-run | grep 'planned network'; "$SCAFFOLD_BIN" wallet topup; echo "exit=$?"`.
   Restore the config.

### Success criteria
- Step 1 shows a `pinata claim` command line and a `planned network:` line naming the local
  sequencer. Step 2 is a single JSON object with no `$ <cmd>` echoes, and `jq -e` exits 0.
- Step 3: the `planned network:` line names `http://127.0.0.1:3999`, and the real topup exits
  non-zero with `sequencer appears unavailable at http://127.0.0.1:3999`. A bare `Failed to
  find leader` followed by an account-verification hint is the regression.

### Notes
Unlike `deploy`, topup has no reachability preflight. Against a refused port it fails at
once; against an unrouted (blackholed) address it first waits out the wallet's own timeout,
which can be minutes, so don't use one here. As in DEPLOY-006, the probe only checks that
something answered, so a foreign process on the configured port suppresses the hint.

## WALLET-007 — Wallet password override

### Expected
Wallet-facing commands (`doctor`'s health check, `topup`, `deploy`, `setup`'s seeding) send
`LOGOS_SCAFFOLD_WALLET_PASSWORD` to the wallet's stdin instead of the local dev default when
it is set. The dry run does not show it, and `wallet list` / `wallet --` never read it, so a
wrong password is not an observable failure there.

### Preconditions
WALLET-001, and localnet running. No other scaffold command may run in this cache while the
wallet binary is wrapped.

### Steps
1. Wrap the wallet binary so its stdin is recorded:
   ```bash
   W="$("$SCAFFOLD_BIN" test-node pins --json | jq -r .lez_checkout)/target/release/wallet"
   mv "$W" "$W.real"; trap 'mv "$W.real" "$W"' EXIT
   printf '#!/bin/bash\nIN=$(cat); echo "args=$* stdin=[$IN]" >> /tmp/wallet-stdin.log; echo "$IN" | exec %s.real "$@"\n' "$W" > "$W"; chmod +x "$W"
   ```
2. `LOGOS_SCAFFOLD_WALLET_PASSWORD=custom-pw "$SCAFFOLD_BIN" doctor >/dev/null; env -u LOGOS_SCAFFOLD_WALLET_PASSWORD "$SCAFFOLD_BIN" doctor >/dev/null; cat /tmp/wallet-stdin.log`
3. `mv "$W.real" "$W"; trap - EXIT`

### Success criteria
- The log has `args=check-health stdin=[custom-pw]` for the first `doctor`, and a different,
  non-empty password (the built-in default) for the second.
