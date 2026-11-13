---
feature: cli-basics
prefix: CLI
surface: cli
scenarios:
  - id: CLI-001
    title: Help and version
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 E1
  - id: CLI-002
    title: Unknown command gives a corrective hint
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 E1
  - id: CLI-003
    title: Project commands outside a project fail clearly
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 E1
  - id: CLI-004
    title: Help flags have no side effects
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 E1
  - id: CLI-005
    title: Removed setup flag is rejected
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 E1
  - id: CLI-006
    title: Quiet mode suppresses command echoes
    priority: P2
    provenance: documented
    source: docs/commands.md
  - id: CLI-007
    title: Every top-level command answers --help
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 E1
retired: []
---

# CLI basics

The CLI is discoverable and its errors are useful. `lgs` and `logos-scaffold` are the same
binary. By default the CLI echoes the underlying commands so users can drop down a level.
Related: [Diagnostics](diagnostics.md), [Project creation](project-creation.md).

## CLI-001 — Help and version

### Expected
`--help` and `help` print the same usage listing all top-level commands. `--version` prints
the version and exits 0. Most subcommands' `--help` ends with an `Examples:` block of
copy-paste commands (see CLI-007 for the ones that do not).

### Preconditions
None.

### Steps
1. `"$SCAFFOLD_BIN" --help; "$SCAFFOLD_BIN" help; "$SCAFFOLD_BIN" --version`
2. `"$SCAFFOLD_BIN" deploy --help; lgs --version`

### Success criteria
- All exit 0, and `help` matches `--help`.
- `--version` matches `Cargo.toml`, and `lgs` prints the same version.

## CLI-002 — Unknown command gives a corrective hint

### Expected
An unknown subcommand fails and points to `--help` (or suggests a similar command).

### Preconditions
None.

### Steps
1. `"$SCAFFOLD_BIN" nonexistent-command; echo "exit=$?"`

### Success criteria
- Non-zero exit and a corrective hint.

## CLI-003 — Project commands outside a project fail clearly

### Expected
Outside a project, `build`, `deploy`, `doctor`, `localnet status` and `wallet list` each fail
with the scaffold message `This command must be run inside a logos-scaffold project`,
followed by a next step naming `logos-scaffold create <name>` (the name follows argv[0], so
`lgs` prints `lgs create <name>`) and `Searched from <cwd>.` None of them shows a raw
file-not-found error.

### Preconditions
An empty temporary directory.

### Steps
1. `cd "$(mktemp -d)"; for c in build deploy doctor "localnet status" "wallet list"; do "$SCAFFOLD_BIN" $c; echo "exit=$?"; done`

### Success criteria
- Every command exits non-zero with `must be run inside a logos-scaffold project` and the
  `logos-scaffold create <name>` next step.

## CLI-004 — Help flags have no side effects

### Expected
`new --help` and `create --help` create no files or directories.

### Preconditions
An empty temporary directory.

### Steps
1. `cd "$(mktemp -d)" && "$SCAFFOLD_BIN" new --help && "$SCAFFOLD_BIN" create --help && ls -A`

### Success criteria
- `ls -A` prints nothing (in particular, no `--help/` directory).

## CLI-005 — Removed setup flag is rejected

### Expected
`setup --help` does not mention `--wallet-install`, and `setup --wallet-install auto` is
rejected at parse time as an unknown argument.

### Preconditions
None.

### Steps
1. `"$SCAFFOLD_BIN" setup --help | grep -c wallet-install`
2. `"$SCAFFOLD_BIN" setup --wallet-install auto; echo "exit=$?"`

### Success criteria
- Step 1 prints `0`. Step 2 exits non-zero with an unexpected-argument error.

## CLI-006 — Quiet mode suppresses command echoes

### Expected
Global `-q`/`--quiet`, or `LOGOS_SCAFFOLD_QUIET=1`, suppresses echoed external commands
(`$ …` lines). Results and errors still print.

### Preconditions
A project with BUILD-001 done.

### Steps
1. `"$SCAFFOLD_BIN" setup | grep -c '^\$ '`
2. `"$SCAFFOLD_BIN" -q setup | grep -c '^\$ '`
3. `LOGOS_SCAFFOLD_QUIET=1 "$SCAFFOLD_BIN" setup | grep -c '^\$ '`

### Success criteria
- Step 1 prints a positive count. Steps 2 and 3 print `0`. Run step 2 once without the pipe and confirm `setup complete` still prints.

## CLI-007 — Every top-level command answers --help

### Expected
Every top-level command accepts `--help`, prints usage and exits 0 without touching the
filesystem, including `spel --help`, which scaffold intercepts before the passthrough. At
this revision `localnet`, `test-node`, `basecamp` and `run` have no `Examples:` block in
their `--help` (their subcommands may). Every other visible command has one, or examples in
its long description (`spel`).

### Preconditions
An empty temporary directory.

### Steps
1. `cd "$(mktemp -d)"; for c in create new init setup build deploy localnet test-node wallet spel basecamp doctor run report tui completions; do "$SCAFFOLD_BIN" $c --help > /tmp/h.txt; echo "$c exit=$? examples=$(grep -c '^Examples' /tmp/h.txt)"; done; ls -A`

### Success criteria
- Every command exits 0, and `ls -A` prints nothing.
- `examples=0` appears only for `localnet`, `test-node`, `basecamp` and `run`. Any other
  command without examples, or a change to that list, is a finding to record.
