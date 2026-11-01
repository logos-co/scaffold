---
dogfooding: 1
repo: github.com/logos-co/logos-scaffold
commit: 1da8b263a700976ca63d715cb1c14d48c1f1bc39
updated: 2026-10-28
---

# logos-scaffold — dogfooding context

`logos-scaffold` (alias `lgs`) is a Rust CLI and library for Logos program developers. It
creates projects from the bare-LEZ or spel template, builds a pinned, project-local LEZ
toolchain, runs a local sequencer, deploys reproducibly built risc0 guests and runs the whole
inner loop with `lgs run`. It also runs disposable test-node sequencers for integration tests
and builds and launches `.lgx` modules in isolated basecamp profiles. Details:
[product.md](product.md). The detailed runbook these scenarios come from is `DOGFOODING.md`
at the repo root.

| Feature | Surface | P0 | P1 | P2 |
|---|---|---|---|---|
| [Project creation](features/project-creation.md) | cli | 1 | 2 | 5 |
| [Setup and build](features/setup-and-build.md) | cli | 2 | 3 | 1 |
| [Localnet](features/localnet.md) | cli | 1 | 3 | 3 |
| [Deploy](features/deploy.md) | cli | 1 | 3 | 4 |
| [Run pipeline](features/run-pipeline.md) | cli | 1 | 2 | 5 |
| [Wallet](features/wallet.md) | cli | 0 | 3 | 4 |
| [Example runners](features/example-runners.md) | cli | 0 | 2 | 1 |
| [spel template](features/spel-template.md) | cli | 0 | 4 | 2 |
| [Diagnostics](features/diagnostics.md) | cli | 0 | 3 | 2 |
| [CLI basics](features/cli-basics.md) | cli | 0 | 2 | 4 |
| [AI skills](features/ai-skills.md) | cli | 0 | 0 | 3 |
| [Test-node lifecycle](features/test-node-lifecycle.md) | cli | 0 | 4 | 3 |
| [Test-node RPC](features/test-node-rpc.md) | cli | 0 | 5 | 3 |
| [Basecamp setup](features/basecamp-setup.md) | cli | 0 | 3 | 4 |
| [Basecamp modules](features/basecamp-modules.md) | cli | 0 | 3 | 5 |
| [Basecamp launch](features/basecamp-launch.md) | cli | 0 | 3 | 5 |
| [Rust API](features/rust-api.md) | api | 0 | 1 | 2 |

## How this folder works

This folder specifies the product behavior to check when dogfooding. Run results never go
here: they go in `runs/`, which is gitignored.

- Each file in `features/` is one feature. Its frontmatter lists the scenarios. Each scenario
  is a `## <ID> — <title>` section with Expected / Preconditions / Steps / Success criteria.
- **IDs** (`WALLET-001`) are stable. Never renumber or reuse them. New scenarios take the next
  number. Removed IDs move to the feature's `retired: [...]` list.
- **Priority:** P0 = the product does not work at all if this is broken. P1 = baseline
  functionality, the primary user flows. P2 = extras whose failure doesn't materially affect
  users.
- **Provenance:** `confirmed` (the user said so), `documented` (docs or spec say so),
  `inferred` (read from code, and possibly wrong). If a code change contradicts a `confirmed`
  expectation, ask the user. Don't rewrite the expectation.
- `depends_on: [ID, ...]` names the direct prerequisites. If a prerequisite fails, the
  scenarios that depend on it are reported as blocked.
- `commit` above is the product commit this context was last fully reconciled against. Move it
  only after reviewing every change since then.
- Frontmatter: one key per line. Lists are inline (`[A-001, B-002]`) or one `- A-001` line per
  item. No other YAML.
