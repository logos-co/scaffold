# AGENTS.md

Context for AI coding agents working in this repository.

## What this repo is

`logos-scaffold` is a Rust CLI that builds, runs, and deploys Logos programs
against a local execution zone. It ships two binaries from one crate:
`logos-scaffold` and the alias `lgs`. They are functionally identical.

The crate also exposes a typed Rust API under `logos_scaffold::api`, so the CLI
and the library are two front ends over the same code. A change to command
behaviour usually needs a matching change to the API surface.

## Driving the CLI

If your task is to *use* scaffold rather than change it, read the skills in
`skills/` instead of this file. They are the maintained instructions:

| Skill | Use it for |
|---|---|
| `skills/lgs-cli` | Entry point. Driving the CLI, diagnosing errors, adopting scaffold in an existing project. |
| `skills/lez-template` | Working inside a default-template project. |
| `skills/lez-framework-template` | Working inside a `--template lez-framework` project. |
| `skills/basecamp` | Any `lgs basecamp` subcommand, or a project that builds `.lgx` modules. |

`skills/lgs-cli` routes into the other three once it knows the project type.

## Layout

| Path | Contents |
|---|---|
| `src/commands/` | One module per CLI command. Start here for behaviour changes. |
| `src/api/` | Public Rust API. Keep in sync with command behaviour. |
| `src/cli.rs` | clap definitions. The source of truth for flags. |
| `templates/` | Project templates: `default` and `lez-framework`. |
| `skills/` | Agent skills, shipped into scaffolded projects. |
| `tests/` | Integration tests driven through `assert_cmd`. |
| `docs/` | User documentation. |

## Before you change anything

Read `CONTRIBUTING.md` first. This project has an explicit triage bar: changes
should trace to a real project hitting real friction, or to demonstrated user
demand. Speculative refactors are likely to be closed.

## Checks

CI runs exactly these three, in this order:

```bash
cargo fmt --check
cargo check
cargo test
```

`.github/workflows/ci.yml` is authoritative. Run all three before opening a PR.

## Documentation rules

- `src/cli.rs` and `--help` are the source of truth for flags. `docs/commands.md`
  documents them and must be updated in the same change.
- `README.md` is the front door. Keep it short and keep install and first-run
  above the fold. Reference material belongs in `docs/`.
- `DOGFOODING.md` is the canonical runbook. Update it whenever first-class
  commands, templates, or supported workflows change.
- `ADR.md` records architecture decisions and `FURPS.md` records requirements.
  Add to them rather than rewriting history.

## Constraints worth knowing

- The CLI is Unix-only. Localnet and process detection shell out to `lsof`,
  `ps`, and `kill`.
- Dependencies (LEZ, spel, basecamp, lgpm) are pinned by commit in
  `scaffold.toml` and resolved under a cache root at runtime. Do not hardcode
  checkout paths.
- Project-local binaries are never installed to PATH. Reach them through
  `lgs wallet -- …` and `lgs spel -- …`.
- Commands that produce machine-readable output take `--json`. Adding output to
  a command means considering both paths.
