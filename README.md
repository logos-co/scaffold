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
lgs new my-app
cd my-app
lgs run
lgs wallet -- check-health   # confirm wallet + localnet after the first pipeline
```

`lgs run` builds your project, starts a local sequencer, funds a wallet, and
deploys your programs. That is the whole inner loop in one command.

```console
$ lgs run
[1/4] Building...
[2/4] Ensuring localnet...
localnet ready (sequencer pid=<pid>)
[3/4] Topping up wallet...
[4/4] Deploying programs...
OK  hello_world deployed
  program_id: <base58 header account id>

Deployed programs:
  hello_world
    program_id: <base58 header account id>
    Binary: <project>/target/riscv-guest-docker/.../hello_world.bin

Sequencer: http://127.0.0.1:3040
```

Step labels above are exactly what the CLI prints; the values in angle
brackets are yours.

Scaffold targets **LEZ 0.3** (pinned to `v0.3.1`) and nothing older. The
first run compiles the LEZ sequencer and wallet from source, so expect it to
take a while. Every run after that reuses a running localnet and skips deploy when
nothing changed. If a step fails, `lgs doctor` reports what is missing.

## What you get

- `lgs run` chains build, localnet, wallet topup, and deploy into one command. Add `--watch` to re-run it on every file change.
- `lgs localnet` gives each project one long-lived sequencer. It is daemonized, so closing the terminal does not kill it, and its status tells a managed process apart from a stale one or a foreign listener.
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

- `git`, `rustc`, `cargo` (Rust 1.81 or newer for scaffold itself; the LEZ
  build installs its own pinned toolchain, 1.98.1, through rustup)
- Unix process helpers: `lsof`, `ps`, `kill`
- The LEZ build's system libraries: `clang`/`libclang`, OpenSSL headers,
  `pkg-config`, and the PC/SC library the LEZ 0.3 wallet links for keycard
  support (`apt install clang libclang-dev libssl-dev pkg-config
  libpcsclite-dev` on Debian/Ubuntu; `brew install pkg-config openssl` on
  macOS)

Some workflows need more:

- Network access to GitHub for the first `setup`: the LEZ build downloads
  its own `logos-blockchain-circuits` release (into `~/.cache/logos`)
- Docker and `cargo-risczero`, for the default reproducible guest build
  (`[build].guest = "docker"`). Set `[build].guest = "local"` to use your host
  toolchain instead — no container runtime, but the guest binary it produces
  is not byte-identical across machines — see
  [docs/configuration.md](docs/configuration.md#build--guest-program-build-strategy)
- `nix` with flakes enabled, for `basecamp` subcommands

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
| `lgs build` | Build the workspace and guest programs (`--guest local` for a faster, Docker-free, non-reproducible guest build) |
| `lgs deploy` | Deploy guest programs to the running localnet |
| `lgs localnet` | Start, stop, inspect, or reset the local sequencer |
| `lgs run` | Chain the whole loop: build, localnet, topup, deploy |
| `lgs test-node` | Manage isolated sequencers for integration tests |
| `lgs wallet` | List accounts, top up from the localnet's dev funder, set the project default |
| `lgs basecamp` | Build and launch Logos desktop profiles |
| `lgs doctor` | Report what is missing or misconfigured |
| `lgs report` | Build a redacted diagnostics bundle for a GitHub issue |

### Step-by-step (optional)

Use these when debugging a single phase or learning what `run` orchestrates:

```bash
lgs setup
lgs localnet start
lgs build
lgs wallet topup   # the default account pays deployment fees
lgs deploy
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
does not touch your `Cargo.toml` or `src/`.

### Migrate an older scaffolded project

Projects created for LEZ 0.1/0.2 (`scaffold.toml` schema `0.2.0`, with
`[repos.spel]` and `[circuits]`) are rejected with an error pointing at
`init`. Migrate with:

```bash
cd my-existing-project
lgs init    # schema 0.3.0: drops [repos.spel]/[circuits]/[framework.idl], re-pins LEZ to v0.3.1
lgs setup   # rebuilds the LEZ 0.3 sequencer and wallet
```

Other settings and comments are preserved. `init` cannot port your program
code: LEZ 0.3 changed the guest program model (`lee_core::program::{Plan,
run_program}`, per-program account shards) and the client crates (`nssa` →
`lee`), so compare with a freshly generated project. `lez-framework` (spel)
projects are refused: spel has no LEZ 0.3 release yet.

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
[LEZ](https://github.com/logos-blockchain/logos-execution-zone/), and supports
the LEZ 0.3 release line only. It runs the sequencer in standalone mode (a mock
Bedrock), so it does not need a `logos-blockchain` node.

## Example runs

A project created from the default template ships LEZ's example runners. Each
deploys a fresh copy of its program and calls it; they take the guest binary,
plain base58 account ids, and a funded payer. The generated project's README
walks through them, for example:

```bash
export LEE_WALLET_HOME_DIR=$(pwd)/.scaffold/wallet
cargo run --bin run_hello_world -- "$BIN_DIR/hello_world.bin" <account_id> <payer_account_id>
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
