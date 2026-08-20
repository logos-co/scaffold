# logos-scaffold

The command-line toolkit for building, running, and deploying Logos programs
against a local execution zone.

[![crates.io](https://img.shields.io/crates/v/logos-scaffold.svg)](https://crates.io/crates/logos-scaffold)
[![docs.rs](https://img.shields.io/docsrs/logos-scaffold)](https://docs.rs/logos-scaffold)
[![CI](https://img.shields.io/github/actions/workflow/status/logos-co/scaffold/ci.yml?branch=master&label=CI)](https://github.com/logos-co/scaffold/actions/workflows/ci.yml)
[![license](https://img.shields.io/crates/l/logos-scaffold.svg)](#license)

**[Quick start](#quick-start)** | [Commands](docs/commands.md) | [Configuration](docs/configuration.md) | [Contributing](CONTRIBUTING.md) | [Security](SECURITY.md)

## Quick start

```bash
cargo install logos-scaffold
lgs new my-app --template lez-framework
cd my-app
lgs run
lgs wallet -- check-health   # confirm wallet + localnet after the first pipeline
```

`lgs run` builds your project, starts a local sequencer, funds a wallet, and
deploys your programs. That is the whole inner loop in one command.

```console
$ lgs run
[1/5] Building...
[2/5] Building IDL...
[3/5] Ensuring localnet...
localnet ready (sequencer pid=<pid>)
[4/5] Topping up wallet...
[5/5] Deploying programs...
  program_id: <hex>

Deployed programs:
  lez_counter
    Binary: <project>/target/riscv-guest/.../lez_counter.bin

Sequencer: http://127.0.0.1:3040
```

Step labels above are exactly what the CLI prints; the values in angle
brackets are yours.

The first run compiles the LEZ toolchain from source, so expect it to take a
while. Every run after that reuses a running localnet and skips deploy when
nothing changed. If a step fails, `lgs doctor` reports what is missing.

## What you get

- `lgs run` chains build, IDL, localnet, wallet topup, and deploy into one command. Add `--watch` to re-run it on every file change.
- `lgs localnet` gives each project one long-lived sequencer. Its status tells a managed process apart from a stale one or a foreign listener.
- `lgs test-node` spins up throwaway sequencers for integration tests. Each gets its own port, database, and state, so tests run in parallel without colliding.
- `lgs basecamp` builds your project's Logos modules and launches them in clean-slate desktop profiles.

Full surface: [docs/commands.md](docs/commands.md).

> [!NOTE]
> Coming from Foundry or Anchor? `lgs new` is `forge init` / `anchor init`.
> `lgs localnet start` is `anvil` / `solana-test-validator`. `lgs deploy` is
> `forge script --broadcast` / `anchor deploy`. The difference is `lgs run`,
> which chains that whole sequence for you.

## Prerequisites

Every command needs these:

- `git`, `rustc`, `cargo` (Rust 1.81 or newer)
- Unix process helpers: `lsof`, `ps`, `kill`

Some workflows need more:

- `curl`, used by the first `setup` to fetch the pinned
  `logos-blockchain-circuits` release
- A container runtime, Docker or Podman, for guest builds
- `nix` with flakes enabled, for `basecamp` subcommands

Circuits are not a manual step. Scaffold downloads the release pinned in
`[circuits]` into `.scaffold/circuits` the first time a command needs it. Set
`LOGOS_BLOCKCHAIN_CIRCUITS=<path>` only to point at a checkout you already
have.

Run `lgs doctor` to check all of this at once. Add `--json` for CI.

## Install

From crates.io:

```bash
cargo install logos-scaffold
```

From a clone:

```bash
cargo install --path .
```

Either way you get two binaries on your PATH: `logos-scaffold` and the shorter
alias `lgs`. They are functionally identical, so use whichever you prefer.

### Shell completions

`lgs completions <shell>` prints a completion script to stdout. The script
completes both `lgs` and `logos-scaffold`. Per-shell install instructions live
in the CLI help itself:

```bash
lgs completions bash --help
lgs completions zsh --help
```

## Everyday commands

Every command works under both binary names. See
[docs/commands.md](docs/commands.md) for the full surface and the exact
semantics of each one.

| Command | What it does |
|---|---|
| `lgs new <name>` | Create a project from a template (`create` is an alias) |
| `lgs init` | Add scaffold to an existing project, or migrate an older one |
| `lgs setup` | Sync pinned dependencies and build project-local binaries |
| `lgs build` | Build the workspace and guest programs |
| `lgs deploy` | Deploy guest programs to the running localnet |
| `lgs localnet` | Start, stop, inspect, or reset the local sequencer |
| `lgs run` | Chain the whole loop: build, IDL, localnet, topup, deploy |
| `lgs test-node` | Manage isolated sequencers for integration tests |
| `lgs wallet` | List accounts, top up, set the project default |
| `lgs basecamp` | Build and launch Logos desktop profiles |
| `lgs doctor` | Report what is missing or misconfigured |
| `lgs report` | Build a redacted diagnostics bundle for a GitHub issue |

### Step-by-step (optional)

Use these when debugging a single phase or learning what `run` orchestrates:

```bash
lgs setup
lgs localnet start
lgs build
lgs deploy
lgs wallet topup
```

### Adopt scaffold in an existing project

If you already have a Rust/LEZ project, add scaffold to it without
regenerating:

```bash
cd my-existing-project
lgs init
lgs setup
```

`init` only writes `scaffold.toml` and creates `.scaffold/` directories. It
does not touch your `Cargo.toml` or `src/`. Edit `scaffold.toml` if you need
non-default framework settings, for example `lez-framework`.

### Migrate an older scaffolded project

If `scaffold.toml` predates a section scaffold now requires (for example
`[repos.spel]`), commands fail with an error pointing at `init`. Migrate with:

```bash
cd my-existing-project
lgs init    # appends the missing section to scaffold.toml in place
lgs setup   # picks up the new section
```

Existing fields are preserved verbatim.

## Configuration

`lgs run` works with no configuration. To add post-deploy hooks, named
profiles, watch-mode filters, or to skip scaffold's deploy or topup step, see
[docs/configuration.md](docs/configuration.md).

## Using scaffold as a Rust library

Everything the CLI does is also exposed as a typed Rust API under
`logos_scaffold::api`, so tests and dev tooling can drive scaffold-managed
projects (setup, localnet lifecycle, wallet top-ups, deploys, diagnostics)
without shelling out to `lgs` and parsing text:

```rust
use logos_scaffold::api::{LocalnetStartOptions, Project};

let project = Project::open("/path/to/my-app")?;
let node = project.localnet_start(&LocalnetStartOptions::default())?;
println!("sequencer pid={} rpc={}", node.pid(), node.rpc_url());
node.stop()?;
```

See the [`api` module rustdoc](https://docs.rs/logos-scaffold) for the full
surface, typed result models, and categorized errors.

## LEZ Framework

For a developer experience closer to Anchor on Solana, use the
[LEZ Framework](https://github.com/jimmy-claw/lez-framework) template:

```bash
lgs new my-app --template lez-framework
```

See the [LEZ Framework template README](./templates/lez-framework/README.md)
for details.

## Troubleshooting

If `localnet start` fails, read the sequencer log:

```bash
lgs localnet logs --tail 200
```

If status reports `ownership: foreign`, another process holds
`127.0.0.1:3040`. Stop it before starting scaffold's localnet.

If status reports stale state, cycle it:

```bash
lgs localnet stop
lgs localnet start
```

For tooling and CI, every diagnostic has a machine-readable form:

```bash
lgs localnet status --json
lgs doctor --json
lgs report --tail 500
```

## Platform and scope

The CLI is Unix-only. Localnet and process/port detection rely on `lsof`, `ps`,
and `kill`.

Scaffold has a single external dependency,
[LEZ](https://github.com/logos-blockchain/logos-execution-zone/). It supports
the standalone sequencer flow only, and does not depend on `logos-blockchain`.

## Example runs

A project created from the default template ships example client binaries.
From that project's root, run them directly, without passing `.bin` paths:

```bash
cargo run --bin run_hello_world -- <public_account_id>
cargo run --bin run_hello_world_private -- <private_account_id>
cargo run --bin run_hello_world_with_authorization -- <public_account_id>
cargo run --bin run_hello_world_with_move_function -- write-public <public_account_id> <text>
cargo run --bin run_hello_world_through_tail_call -- <public_account_id>
cargo run --bin run_hello_world_through_tail_call_private -- <private_account_id>
cargo run --bin run_hello_world_with_authorization_through_tail_call_with_pda
```

To point at custom binaries:

```bash
export EXAMPLE_PROGRAMS_BUILD_DIR=$(pwd)/target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release
cargo run --bin run_hello_world -- --program-path "$EXAMPLE_PROGRAMS_BUILD_DIR/hello_world.bin" <public_account_id>
cargo run --bin run_hello_world_through_tail_call_private -- --simple-tail-call-path "$EXAMPLE_PROGRAMS_BUILD_DIR/simple_tail_call.bin" --hello-world-path "$EXAMPLE_PROGRAMS_BUILD_DIR/hello_world.bin" <private_account_id>
```

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. It states who this project
prioritizes and what a PR needs before review. Contributors will also want
[AGENTS.md](AGENTS.md), the [FURPS+ requirements](FURPS.md), the
[architecture decision records](ADR.md), and the
[dogfooding runbook](DOGFOODING.md).

Report a bug with `lgs report`, which builds a redacted diagnostics bundle to
attach to the issue.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
