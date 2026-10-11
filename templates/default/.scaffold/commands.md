# Command References

- standalone sequencer (what `localnet start` runs): `RUST_LOG=info target/release/sequencer_service <config> --port <port> --home <dir>`
- lez standalone docs: `https://github.com/logos-blockchain/logos-execution-zone/tree/main?tab=readme-ov-file#standalone-mode`
- wallet commands: `logos-scaffold wallet -- <args>`
- localnet json status: `logos-scaffold localnet status --json`
- doctor json status: `logos-scaffold doctor --json`
- diagnostics bundle for issue reports: `logos-scaffold report --tail 500`
