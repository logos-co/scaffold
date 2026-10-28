---
feature: diagnostics
prefix: DIAG
surface: cli
scenarios:
  - id: DIAG-001
    title: doctor gives actionable checks
    priority: P1
    provenance: documented
    source: DOGFOODING.md D2
    depends_on: [BUILD-001]
  - id: DIAG-002
    title: doctor JSON output
    priority: P1
    provenance: documented
    source: DOGFOODING.md D2
    depends_on: [BUILD-001]
  - id: DIAG-003
    title: report produces a support bundle
    priority: P2
    provenance: documented
    source: DOGFOODING.md D5
    depends_on: [BUILD-001]
  - id: DIAG-004
    title: report never includes wallet secrets
    priority: P1
    provenance: documented
    source: DOGFOODING.md D5
    depends_on: [DIAG-003]
  - id: DIAG-005
    title: Shell completions
    priority: P2
    provenance: documented
    source: docs/commands.md
retired: []
---

# Diagnostics

`doctor` checks the project: toolchain, pins, spel/LEZ alignment, circuits, guest build
mode, wallet and localnet. It prints next steps, with `--json` for CI. `report` collects an
allowlisted, redacted `.tar.gz` for bug reports. `completions` prints shell completion
scripts. Related: [Setup and build](setup-and-build.md), [Localnet](localnet.md),
[Basecamp setup](basecamp-setup.md) (`basecamp doctor`).

## DIAG-001 — doctor gives actionable checks

### Expected
`doctor` prints `PASS`/`WARN`/`FAIL` rows and next steps for anything not passing. It covers
the configured circuits install (its `VERSION` against `[circuits].version`, plus drift from
the LEZ pin's expected release), the spel pin and its LEZ alignment, and a `guest build` row
naming the mode and the risc0 builder tag. In docker mode the row is a FAIL when
`cargo-risczero`, `docker` or the daemon is missing. The daemon check finishes within 10
seconds even if `DOCKER_HOST` is unreachable.

### Preconditions
BUILD-001.

### Steps
1. `"$SCAFFOLD_BIN" doctor`
2. Set `[circuits].version` to a wrong value, run `"$SCAFFOLD_BIN" doctor`, then restore it.

### Success criteria
- Every non-PASS row has an actionable next step.
- The `guest build` row mentions `risczero/risc0-guest-builder:<tag>`.
- Step 2 shows a circuits warning or error that names the configured install dir.

## DIAG-002 — doctor JSON output

### Expected
`doctor --json` is pure, parseable JSON with at least `status`, `summary`, `checks` and
`next_steps`.

### Preconditions
BUILD-001.

### Steps
1. `"$SCAFFOLD_BIN" doctor --json | jq -e '.status and .summary and .checks and .next_steps'`

### Success criteria
- `jq -e` exits 0.

## DIAG-003 — report produces a support bundle

### Expected
`report` prints a completion message, the archive path and a warning to inspect it before
sharing. The default location is `.scaffold/reports/*.tar.gz`. `--out PATH` and `--tail N`
are honoured. The archive contains `README.txt`, `manifest.json`, `diagnostics/doctor.json`,
`diagnostics/localnet-status.json` and `summaries/build-evidence.json`.

### Preconditions
BUILD-001, ideally after a localnet start or a build.

### Steps
1. `"$SCAFFOLD_BIN" report --tail 200`
2. `"$SCAFFOLD_BIN" report --out "$PWD/artifacts/support-report.tar.gz"`
3. `tar -tzf "$PWD/artifacts/support-report.tar.gz" | sort`

### Success criteria
- Both archives exist, the inspect warning is printed, and every file listed above is in the
  archive.

### Notes
Never upload an archive without listing its contents and asking the user.

## DIAG-004 — report never includes wallet secrets

### Expected
No file from `.scaffold/wallet/` appears in the archive. Absolute local paths in human-facing
files are scrubbed.

### Preconditions
DIAG-003.

### Steps
1. `tar -tzf <archive> | grep -i wallet`
2. `tar -xzf <archive> -O | grep -c "$HOME"`

### Success criteria
- Step 1 shows no `.scaffold/wallet` content.
- Step 2 finds no unscrubbed home paths in human-facing files (record any hit).

## DIAG-005 — Shell completions

### Expected
`completions bash` and `completions zsh` print a completion script to stdout covering both
`lgs` and `logos-scaffold`. Per-shell install instructions are in `completions <shell> --help`.

### Preconditions
None.

### Steps
1. `"$SCAFFOLD_BIN" completions zsh | head -5`
2. `"$SCAFFOLD_BIN" completions bash | grep -c logos-scaffold`

### Success criteria
- Both exit 0, the output is a script, and both binary names appear.
