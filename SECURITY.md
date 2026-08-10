# Security Model

## Development-Only Wallet Material

`logos-scaffold` is designed for local standalone development flows.

- Wallet home: `.scaffold/wallet`
- Default wallet state: `.scaffold/state/wallet.state`
- Localnet address: `http://127.0.0.1:3040`

Do not use scaffold-generated wallets or keys for real funds or production environments.

## Deterministic Local Password

Scaffold wallet automation uses a deterministic local password by default for onboarding UX.

Override with:

```bash
export LOGOS_SCAFFOLD_WALLET_PASSWORD='<your-local-dev-password>'
```

This override applies to scaffold commands that submit wallet password input (`setup`, `wallet`, `deploy`, `doctor` checks).

Export it **before the first `setup`** (or the first `run`, which chains `setup`). Against a LEZ pin whose debug wallet config ships no preconfigured account — v0.2.0 ships none — `setup` seeds the default wallet by running `wallet account list` against `.scaffold/wallet` with that password on stdin, which makes the wallet CLI create its persistent storage on first use. The deterministic password therefore guards freshly created key material, not just storage the debug config pre-seeded, and the resulting address is recorded in `.scaffold/state/wallet.state` as the default topup destination. If storage was already created under the default, export the override and re-run with `run --reset`, which wipes the wallet and re-seeds it.

## Repository Hygiene

- Keep `.scaffold/` out of source control.
- Generated projects also ignore `.env.local` by default.
- Treat wallet config and local logs as sensitive development artifacts.

## Network Behavior

Scaffold does not include telemetry.
Network activity is limited to explicit operations you trigger (for example, syncing pinned repositories or calling configured local endpoints).
