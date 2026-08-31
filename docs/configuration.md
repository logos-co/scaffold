# Configuring `lgs run`

`lgs run` is the inner loop: it chains build (which chains setup), IDL build,
localnet start, wallet topup, and deploy into one command. It works with no
configuration at all. This page covers the `[run]` section of `scaffold.toml`
for projects that need to change what the pipeline does, plus the `[build]`
section that decides how the pipeline compiles guest programs.

For the command surface itself, see [commands.md](./commands.md).

To run one or more post-deploy hooks automatically (e.g. submit a transaction
with [spel](https://github.com/logos-co/spel)), add a `[run]` section to
`scaffold.toml`. `post_deploy` is a list of shell commands executed in order;
the run aborts at the first non-zero exit:

```toml
[run]
post_deploy = [
  "lgs spel -- --idl $SCAFFOLD_IDL_DIR/counter.json -p $SCAFFOLD_GUEST_BIN init",
  "lgs spel -- --idl $SCAFFOLD_IDL_DIR/counter.json -p $SCAFFOLD_GUEST_BIN increment --by 5",
]
```

The `lgs spel --` passthrough invokes the project-vendored `spel` binary
so hooks pick up the same pinned version `deploy` used.

A single command may also be written as a plain string for brevity:
`post_deploy = "echo done"`.

Each hook runs via `sh -c` with cwd set to the project root and these
environment variables pre-set:

| Variable | Value |
|---|---|
| `SEQUENCER_URL` | `http://127.0.0.1:<port>` (from `scaffold.toml`) |
| `NSSA_WALLET_HOME_DIR` | Absolute path to project wallet directory (name read by LEZ up to v0.1.2) |
| `LEE_WALLET_HOME_DIR` | Same path under the name LEZ v0.2.0 reads. Both are always set, so a hook that execs the wallet binary works on either pin |
| `SCAFFOLD_PROJECT_ROOT` | Absolute path to project root |
| `SCAFFOLD_IDL_DIR` | Absolute path to IDL output directory |
| `SCAFFOLD_TOPUP_SKIPPED` | `1` when step 4 was skipped (`topup = false`), `0` when scaffold topped up the wallet. Always set |
| `SCAFFOLD_DEPLOY_SKIPPED` | `1` when step 5 deployed nothing — either `deploy = false`, or the deploy cache found guest binaries, IDL, config and sequencer unchanged — and `0` when it deployed. Always set |
| `SCAFFOLD_PROGRAM_ID` | risc0 image ID (hex) of the deployed program. Set only when the project has exactly one deployable program; unset if `spel program-id` cannot extract the ID |
| `SCAFFOLD_GUEST_BIN` | Absolute path to the guest `.bin`. Set only when the project has exactly one deployable program |

`SCAFFOLD_TOPUP_SKIPPED` and `SCAFFOLD_DEPLOY_SKIPPED` describe the run as a
whole, so they are set on every hook invocation — including for projects with
no deployable programs, where `SCAFFOLD_PROGRAM_ID` and `SCAFFOLD_GUEST_BIN`
are absent. Hooks should branch on their `1`/`0` value, not on whether they
exist.

`SCAFFOLD_PROGRAM_ID` and `SCAFFOLD_GUEST_BIN` are unset for
multi-program projects so hooks fail loudly rather than silently
picking up the wrong program.

## Self-deploying projects (`deploy = false`)

`run` deploys programs it finds under `methods/guest/src/bin`. A project
that owns deployment itself — it deploys from a `post_deploy` hook, or keeps
its guest program outside that default directory — can set `deploy = false`
to skip scaffold's deploy step (step 5). The pipeline then runs
build → IDL → localnet → topup → `post_deploy`, without requiring the default
program directory to exist. It works inline under `[run]` or per profile:

```toml
[run.profiles.demo]
deploy = false
post_deploy = ["scripts/deploy-and-demo.sh"]
```

It is equally valid inline under `[run]` (applies to the default, profile-less run):

```toml
[run]
deploy = false
post_deploy = ["scripts/deploy-and-demo.sh"]
```

`deploy` defaults to `true`; omit it for the normal deploy loop.

A named profile is used **as-is**: selecting one (via `--profile NAME` or
`[run].default_profile`) shadows the inline `[run]` values entirely rather
than inheriting them. So `deploy = false` under `[run]` combined with
`--profile demo` — where `demo` omits the key — deploys, because `demo`
supplies its own default of `true`. This applies to every profile key
(`reset`, `deploy`, `topup`, `post_deploy`): whatever a profile does not
state, it defaults, it does not inherit. Set the key in each profile that
needs it.

## Self-funding projects (`topup = false`)

`run` tops up the project's default wallet before deploying (step 4). A
project that funds its own accounts — e.g. its demo binary claims from the
faucet at runtime, or a `post_deploy` hook handles funding — can set
`topup = false` to skip that step and keep funding in one place instead of
splitting it between scaffold and the project. The pipeline then runs
build → IDL → localnet → deploy → `post_deploy`. It works inline under
`[run]` or per profile, and combines with `deploy = false`:

```toml
[run.profiles.demo]
topup = false
post_deploy = ["cargo run --bin demo"]
```

Hooks see `SCAFFOLD_TOPUP_SKIPPED=1` on such a run, so a funding hook can
claim only when scaffold did not. The topup step itself needs a destination
address, but on a pin whose wallet config ships preconfigured accounts one is
already in place by step 4: step 1 of every `lgs run` chains `lgs setup`,
which seeds `.scaffold/state/wallet.state` from the first preconfigured
public account whenever that file is missing — and `--reset`, which wipes the
wallet later, at step 3, re-seeds it before the run continues.

Skipping topup does not move the funding requirement, only the responsibility
for meeting it. With the default `deploy = true`, step 5 still runs against
whatever the wallet holds — so if the deploy path needs funds, the project
has to provide them *before* deploy, not from a `post_deploy` hook that runs
after it. A project that funds from a hook generally wants `deploy = false`
too, and to deploy from that same hook after funding.

`topup` defaults to `true`; omit it for the normal topup-then-deploy loop.

## One-off override / skip

To run a different hook without editing `scaffold.toml`:

```bash
lgs run --post-deploy "scripts/smoke.sh"
lgs run --post-deploy "step-a" --post-deploy "step-b"   # repeatable
lgs run --no-post-deploy                                 # skip all hooks
```

`--post-deploy` and `--no-post-deploy` conflict with each other and
both override whatever `[run].post_deploy` defines.

## Watch mode

`lgs run --watch` re-runs the pipeline on each filesystem change (localnet
is reused; reset is skipped on re-runs). Scope what counts as a change with
`[run.watch]` and tune the coalescing window:

```toml
[run.watch]
include = ["programs/**/guest/**", "contracts/**/*.sol"]
exclude = ["**/*.md", "Cargo.lock"]
debounce_ms = 1500
```

A changed path triggers a re-run **iff** it matches at least one `include`
glob (or `include` is unset, meaning "any path") **and** matches zero
`exclude` globs — `exclude` always wins. Globs are project-relative,
gitignore-style: `**` spans path segments, `*`/`?` match within a segment,
and a slash-less pattern (`Cargo.lock`) matches at any depth. `.scaffold`,
`target`, `.git`, and the IDL output dir are always ignored regardless of
these filters. Override the debounce per invocation with
`lgs run --watch --watch-debounce-ms 1500` (CLI wins over
`[run.watch].debounce_ms`, which wins over the 500ms default).

To check what a configured run actually did, `lgs localnet status` reports the
sequencer and `lgs doctor` reports the project.

## `[build]` — guest program build strategy

`lgs build` compiles the risc0 guest programs under `methods/` one of two
ways. The choice decides whether the guest ELF — and therefore the
`program_id` the sequencer stores for it — is reproducible.

```toml
[build]
guest = "docker"                  # "local" (default) | "docker"
risc0_docker_tag = "r0.1.97.0"    # optional; scaffold's pin is the default
```

| `guest` | How | Needs | `program_id` |
|---|---|---|---|
| `local` (default) | `cargo build --release --manifest-path methods/Cargo.toml`, i.e. the guest crate's own `risc0_build::embed_methods()` | nothing beyond the host Rust toolchain | **Not reproducible.** Depends on the host's Rust and clang versions, so two developers — or a developer and CI — can produce different bytes from the same source |
| `docker` | `cargo risczero build` inside `risczero/risc0-guest-builder:<tag>` | Docker daemon running, `cargo-risczero` on `PATH` | **Reproducible.** Same source plus same tag gives the same bytes, and therefore the same `program_id`, on any machine |

Use `local` while you are iterating: it is much faster and needs no container
runtime. Switch to `docker` before a `program_id` starts to matter — anything
you publish, deploy somewhere other than your own localnet, or verify in CI.
`lgs build --guest <local|docker>` overrides the setting for one invocation,
which is the quickest way to check what a program's reproducible `program_id`
actually is.

`risc0_docker_tag` pins the guest Rust toolchain. **Changing it changes every
`program_id` the project produces**, so treat a bump the same way you would
treat a dependency bump: deliberate, and re-verified downstream.

### What each mode writes, and what `deploy` picks up

- `local` → `target/riscv-guest/<methods>/<guest>/riscv32im-risc0-zkvm-elf/release/<program>.bin`
- `docker` → `target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker/<program>.bin`

`lgs deploy` prefers a `docker` artefact over a `release` one, and each `lgs
build` deletes the other mode's tree before it runs. The invariant is that the
last `lgs build` decides what `lgs deploy` ships — you never have to reason
about which of two `.bin` files on disk is current.

`lgs doctor` reports the active strategy as a `guest build` check, and fails
when `guest = "docker"` but Docker or `cargo-risczero` is missing.

### Costs of the deterministic path

- The whole project directory is the Docker build context (risc0 excludes
  `.git`, `target`, `node_modules`, and `tmp`, but nothing else). A project
  root holding large generated data — extracted circuits under
  `.scaffold/circuits`, vendored dependency checkouts — makes each build's
  context transfer slower.
- The builder image is ~1.7 GB on first pull, and the container rebuilds the
  guest from scratch rather than reusing your local cargo cache.
- The guest is still compiled twice: `cargo build --workspace` runs the
  `methods` crate's `embed_methods()` build script (that is where the crate's
  `*_ELF` / `*_ID` constants come from), and the container build then produces
  the artefact `deploy` ships. Only the container output is deployed.
- The published builder images are `linux/amd64`; on Apple silicon the build
  runs under emulation.
- The container build runs `cargo build --locked`, so the workspace
  `Cargo.lock` must exist and be up to date with `Cargo.toml`. That is part of
  what makes the build reproducible, and it is also the most common way this
  mode fails after a dependency edit — run a normal `lgs build` first to
  refresh the lockfile.
- risc0 prints `Cargo.lock not found in path .../methods/guest/Cargo.lock`
  before it starts. It is looking next to the guest manifest; scaffold projects
  keep one lockfile at the workspace root, which the container does use. The
  message is informational and the build proceeds.
