# Command References (`lez-framework`)

- setup tools + sequencer + wallet: `logos-scaffold setup`
- start localnet sequencer: `logos-scaffold localnet start`
- stop localnet sequencer: `logos-scaffold localnet stop`
- check localnet status: `logos-scaffold localnet status`
- build workspace + IDL + generate clients (`src/generated`): `logos-scaffold build`
- regenerate IDL only (no client regeneration): `logos-scaffold build idl`
- run counter init: `cargo run --bin run_lez_counter -- init --authority <account_id>`
- run counter increment: `cargo run --bin run_lez_counter -- increment --authority <account_id> --amount <n>`
- read the counter: `cargo run --bin run_lez_counter -- show`
- health diagnostics: `logos-scaffold doctor`
- diagnostics bundle for issue reports: `logos-scaffold report --tail 500`
- wallet commands: `logos-scaffold wallet -- <args>`
