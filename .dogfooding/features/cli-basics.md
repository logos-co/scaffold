---
feature: cli-basics
prefix: CLI
surface: cli
scenarios:
  - id: CLI-001
    title: Help, version and unknown commands
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 E1
  - id: CLI-003
    title: Project commands outside a project fail clearly
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 E1
  - id: CLI-006
    title: Quiet mode suppresses command echoes
    priority: P2
    provenance: documented
    source: docs/commands.md
  - id: CLI-008
    title: A closed output pipe ends the command quietly
    priority: P2
    provenance: inferred
    source: user 2026-11-13
retired: [CLI-002, CLI-004, CLI-005, CLI-007]
---

# CLI basics

The CLI is discoverable and its errors are useful. `lgs` and `logos-scaffold` are the same
binary. By default the CLI echoes the underlying commands so users can drop down a level.
Related: [Diagnostics](diagnostics.md), [Project creation](project-creation.md).

## CLI-001 — Help, version and unknown commands

### Expected
`--help`, `help` and a bare invocation print the same usage. `help <command>` prints exactly
what `<command> --help` prints. `--version` matches `Cargo.toml` under both binary names.
Every top-level command answers `--help` with exit 0 and touches nothing on disk, including
`new`/`create` (no `--help/` directory) and `spel`, which scaffold intercepts before the
passthrough. `localnet`, `test-node`, `basecamp` and `run` have no `Examples:` block in their
`--help` (their subcommands may); every other visible command has one, or examples in its
long description (`spel`). An unknown command fails and points to `--help`.

### Preconditions
None. Step 3 makes its own empty directory.

### Steps
1. `"$SCAFFOLD_BIN" --version; "${SCAFFOLD_BIN%/*}/lgs" --version; grep -m1 '^version' "$REPO_ROOT/Cargo.toml"`
2. `diff <("$SCAFFOLD_BIN" --help) <("$SCAFFOLD_BIN" help); diff <("$SCAFFOLD_BIN" --help) <("$SCAFFOLD_BIN"); diff <("$SCAFFOLD_BIN" help deploy) <("$SCAFFOLD_BIN" deploy --help); "$SCAFFOLD_BIN" help test-node start | head -3`
3. `cd "$(mktemp -d)"; for c in create new init setup build deploy localnet test-node wallet spel basecamp doctor run report tui completions; do "$SCAFFOLD_BIN" $c --help > /tmp/h.txt; echo "$c exit=$? examples=$(grep -c '^Examples' /tmp/h.txt)"; done; ls -A`
4. `"$SCAFFOLD_BIN" nonexistent-command; echo "exit=$?"; "$SCAFFOLD_BIN" help nope; echo "exit=$?"`

### Success criteria
- Step 1 prints the same version three times.
- Every `diff` in step 2 prints nothing, and `help test-node start` shows
  `Usage: logos-scaffold test-node start`.
- Step 3: every command exits 0 and `ls -A` prints nothing. `examples=0` appears only for
  `localnet`, `test-node`, `basecamp` and `run`; any change to that list is a finding.
- Step 4: both exit non-zero with `unrecognized subcommand` and a pointer to `--help`.

## CLI-003 — Project commands outside a project fail clearly

### Expected
Outside a project, project-scoped commands fail with `This command must be run inside a
logos-scaffold project`, a next step naming `logos-scaffold create <name>` (the name follows
argv[0], so `lgs` prints `lgs create <name>`) and `Searched from <cwd>.` None shows a raw
file-not-found error.

### Preconditions
None.

### Steps
1. `cd "$(mktemp -d)"; for c in build deploy doctor "localnet status" "wallet list" "basecamp doctor"; do "$SCAFFOLD_BIN" $c; echo "exit=$?"; done`

### Success criteria
- Every command exits non-zero with `must be run inside a logos-scaffold project` and the
  `create <name>` next step.

## CLI-006 — Quiet mode suppresses command echoes

### Expected
Global `-q`/`--quiet`, or `LOGOS_SCAFFOLD_QUIET=1`, suppresses echoed external commands
(`$ …` lines). Results and errors still print.

### Preconditions
A project with BUILD-001 done (a repeated `setup` is quick).

### Steps
1. `"$SCAFFOLD_BIN" setup | grep -c '^\$ '`
2. `"$SCAFFOLD_BIN" -q setup | tee /tmp/q.txt | grep -c '^\$ '; grep -c 'setup complete' /tmp/q.txt`
3. `LOGOS_SCAFFOLD_QUIET=1 "$SCAFFOLD_BIN" setup | grep -c '^\$ '`

### Success criteria
- Step 1 prints a positive count. Steps 2 and 3 print `0` echoes, and step 2 still finds
  `setup complete`.

## CLI-008 — A closed output pipe ends the command quietly

### Expected
When the reader of stdout hangs up (`lgs … | head -1`), the command ends like other Unix
tools: killed by SIGPIPE (shell status 141), with nothing on stderr. It never prints a Rust
panic or `error: Broken pipe`, and never exits 101. A broken pipe on a *child's* stdin (a
wallet that exits before reading its password) is still reported as an error.

### Preconditions
Bash (`PIPESTATUS`). A project with BUILD-001 done for step 2.

### Steps
1. `for c in "--help" "completions zsh" "test-node start --help"; do "$SCAFFOLD_BIN" $c 2>/tmp/err.txt | head -1 >/dev/null; echo "$c: status=${PIPESTATUS[0]} stderr=[$(cat /tmp/err.txt)]"; done`
2. In a project: `"$SCAFFOLD_BIN" doctor 2>/tmp/err.txt | head -1 >/dev/null; echo "status=${PIPESTATUS[0]} stderr=[$(cat /tmp/err.txt)]"`

### Success criteria
- Every status is 141 or 0 (0 when the whole output fit in the pipe before `head` exited),
  never 101 or 1, and every `stderr=[...]` is empty.
