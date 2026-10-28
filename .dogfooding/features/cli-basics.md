---
feature: cli-basics
prefix: CLI
surface: cli
scenarios:
  - id: CLI-001
    title: Help and version
    priority: P1
    provenance: documented
    source: DOGFOODING.md E1
  - id: CLI-002
    title: Unknown command gives a corrective hint
    priority: P2
    provenance: documented
    source: DOGFOODING.md E1
  - id: CLI-003
    title: Project commands outside a project fail clearly
    priority: P1
    provenance: documented
    source: DOGFOODING.md E1
  - id: CLI-004
    title: Help flags have no side effects
    priority: P2
    provenance: documented
    source: DOGFOODING.md E1
  - id: CLI-005
    title: Removed setup flag is rejected
    priority: P2
    provenance: documented
    source: DOGFOODING.md E1
  - id: CLI-006
    title: Quiet mode suppresses command echoes
    priority: P2
    provenance: documented
    source: docs/commands.md
retired: []
---

# CLI basics

The CLI is discoverable and its errors are useful. `lgs` and `logos-scaffold` are the same
binary. By default the CLI echoes the underlying commands so users can drop down a level.
Related: [Diagnostics](diagnostics.md), [Project creation](project-creation.md).

## CLI-001 — Help and version

### Expected
`--help` and `help` print the same usage listing all top-level commands. `--version` prints
the version and exits 0. Each subcommand's `--help` includes copy-paste examples.

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
with a scaffold message like `Not a logos-scaffold project … Run logos-scaffold create
<name>`, not a raw file-not-found error.

### Preconditions
An empty temporary directory.

### Steps
1. `cd "$(mktemp -d)"; for c in build deploy doctor "localnet status" "wallet list"; do "$SCAFFOLD_BIN" $c; echo "exit=$?"; done`

### Success criteria
- Every command exits non-zero with the "not a logos-scaffold project" message.

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
