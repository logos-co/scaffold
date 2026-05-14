# Contributing to `logos-scaffold`

## Who This Project Is For

`logos-scaffold` exists to help developers build on the Logos stack. Contributions are prioritized accordingly:

1. Developers using `logos-scaffold` to build a real project on the Logos stack, contributing fixes and features based on friction they actually hit.
2. Contributors who can demonstrate confirmed demand from such users — a linked issue, a discussion, or a concrete request.

Generic cleanups, cosmetic refactors, and "this would be nice" PRs from contributors with no connection to either of the above are the lowest triage priority and are likely to be closed.

## Before You Open a PR

You should have:

- A real project (even a prototype) built with `logos-scaffold` where you hit the friction this PR addresses, **or**
- Evidence of user demand from someone who does: a linked issue, discussion, or direct request.

Plus:

- Rerun the relevant [DOGFOODING](./DOGFOODING.md#minimum-rerun-guidance-for-future-changes) scenarios (`D1`–`L4`).
- Read the PR template and fill in every section.

## What Makes a High-Quality PR

- **Scoped.** One concern per PR. No drive-by refactors.
- **Green CI.**
- **Verified.** Applicable DOGFOODING scenarios rerun. See [DOGFOODING.md](./DOGFOODING.md) — "Minimum Rerun Guidance for Future Changes".
- **Documented.** If the change affects user-facing behavior, update `README.md` and `DOGFOODING.md` in the same PR.

## LLM-Assisted PRs

LLM assistance is welcome. The contributor is accountable for the diff: you have read every line, you understand why the change is correct, and you have run the verification steps above. Disclosure is not required — every PR is reviewed against the same bar regardless of how it was authored.

## Rate Limit

We cap contributions at **3 PRs per contributor per rolling 7-day window**. A GitHub Action (`.github/workflows/pr-rate-limit.yml`) enforces this automatically on PR open.

Exemptions:

- Public members of the [`logos-co`](https://github.com/logos-co) GitHub organization are auto-exempt.
- Additional trusted contributors (including private `logos-co` members) can be added to [`.github/rate-limit-allowlist.txt`](./.github/rate-limit-allowlist.txt).

If you need to exceed the cap for a coordinated change, open an issue first.

Reopening an auto-closed PR re-triggers the check and will close it again. To override, a maintainer must add the author to the allowlist, or the author must wait until older PRs fall outside the 7-day window.

## When Maintainers Close PRs

Maintainers may close any PR that does not adhere to this guideline or does not add clear requested value, pointing back to this document. Closing is not a judgment of the contributor — it is a triage signal.

## Local Development

Build the scaffold CLI itself:

```bash
cargo build
```

Run the CLI from source:

```bash
cargo run --bin logos-scaffold -- --help
cargo run --bin logos-scaffold -- new test-app
```

## Test Suite

Run all tests:

```bash
cargo test --all-targets
```

Formatting check:

```bash
cargo fmt --check
```

## Working on Generated Projects vs CLI

- This repository builds and tests the scaffold CLI.
- Generated projects are separate workspaces created by `logos-scaffold new`.
- Validate scaffold changes by creating a fresh project and running scaffold commands inside it.

## DOGFOODING Validation

Use [DOGFOODING.md](./DOGFOODING.md) as the canonical validation guide for scaffold DX.

At minimum:

- Onboarding, project creation, setup, localnet, or build changes: rerun `D1` and `D2`.
- Deploy, wallet, or diagnostics changes: rerun the affected `D3` to `D5` scenarios.
- LEZ template or generated-artifact changes: rerun `L1` to `L4`.

```bash
cargo build
cargo run --bin logos-scaffold -- new dogfood-app --lez-path /absolute/path/to/logos-execution-zone
cd dogfood-app
logos-scaffold setup
logos-scaffold localnet start
logos-scaffold doctor
logos-scaffold build
logos-scaffold deploy
logos-scaffold wallet topup
logos-scaffold localnet stop
```

Keep all temporary dogfood directories inside your local workspace and remove them after validation.
