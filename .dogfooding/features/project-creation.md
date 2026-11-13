---
feature: project-creation
prefix: CREATE
surface: cli
scenarios:
  - id: CREATE-001
    title: Create a default-template project
    priority: P0
    provenance: documented
    source: DOGFOODING.md D1
  - id: CREATE-002
    title: create is an alias of new
    priority: P2
    provenance: documented
    source: DOGFOODING.md D1
  - id: CREATE-003
    title: Invalid template name is rejected
    priority: P2
    provenance: documented
    source: DOGFOODING.md E2
  - id: CREATE-004
    title: Vendor dependencies into the project
    priority: P2
    provenance: documented
    source: DOGFOODING.md E2
  - id: CREATE-005
    title: Custom cache root keeps checkouts pin-isolated
    priority: P2
    provenance: documented
    source: DOGFOODING.md E2
  - id: CREATE-006
    title: Non-default LEZ source gets its own checkout
    priority: P2
    provenance: documented
    source: DOGFOODING.md E2
  - id: CREATE-007
    title: Adopt an existing directory with init
    priority: P1
    provenance: documented
    source: docs/commands.md
  - id: CREATE-008
    title: Migrate an old scaffold.toml with init
    priority: P1
    provenance: documented
    source: ADR.md
retired: []
---

# Project creation

Developers get a runnable project in one command (`new`/`create`), or adopt scaffold in an
existing directory with `init`. `init` is also the single migration entry point for older
`scaffold.toml` schemas. Related: [Setup and build](setup-and-build.md),
[spel template](spel-template.md), [AI skills](ai-skills.md).

## CREATE-001 — Create a default-template project

### Expected
`new <name>` creates `<name>/` from the `default` template and prints the destination path,
the pinned LEZ commit and the cache root. The generated `scaffold.toml` is schema `0.2.0`. It
includes `[repos.lez]` and `[repos.spel]` (with pins and an empty `path`), a `[circuits]`
table whose install dir is `.scaffold/circuits`, and `[wallet].home_dir`. It has no wallet
binary field.

### Preconditions
Freshly built `$SCAFFOLD_BIN` and a writable `$SCRATCH_ROOT`.

### Steps
1. `cd "$SCRATCH_ROOT" && "$SCAFFOLD_BIN" new dogfood-default`
2. `grep -n '^\[' dogfood-default/scaffold.toml`
3. `ls dogfood-default/src/bin`

### Success criteria
- Exit 0, and the output names the path, LEZ pin and cache root.
- `scaffold.toml` has `[circuits]` and `[wallet]` with `home_dir`, and no `binary =` line.
- `src/bin/` contains the `run_hello_world*.rs` example runners.

## CREATE-002 — create is an alias of new

### Expected
`create <name>` behaves exactly like `new <name>`.

### Preconditions
None.

### Steps
1. `"$SCAFFOLD_BIN" create dogfood-default-create` in `$SCRATCH_ROOT`.

### Success criteria
- Exit 0, and the project layout matches CREATE-001's.

## CREATE-003 — Invalid template name is rejected

### Expected
An unknown `--template` fails with an error that lists the available templates (`default`,
`spel`) and mentions the deprecated `lez-framework` alias. It never falls back to `default`
silently. `--template lez-framework` still works, prints a deprecation warning and produces
a spel project.

### Preconditions
None.

### Steps
1. `"$SCAFFOLD_BIN" new dogfood-invalid --template nonexistent-template; echo "exit=$?"`
2. `"$SCAFFOLD_BIN" new dogfood-lez-alias --template lez-framework`

### Success criteria
- Step 1 exits non-zero, names `default` and `spel`, and does not create `dogfood-invalid/`.
- Step 2 warns about the deprecation, and the result contains `spel.toml`.

## CREATE-004 — Vendor dependencies into the project

### Expected
`--vendor-deps` clones the pinned LEZ into `<project>/.scaffold/repos/lez` and records
`path = ".scaffold/repos/lez"` in `scaffold.toml`.

### Preconditions
None.

### Steps
1. `"$SCAFFOLD_BIN" new dogfood-vendor --vendor-deps`
2. `git -C dogfood-vendor/.scaffold/repos/lez rev-parse HEAD`
3. `grep -A4 'repos.lez' dogfood-vendor/scaffold.toml`

### Success criteria
- The vendored HEAD equals `[repos.lez].pin`, and `path` is the relative `.scaffold/repos/lez`.

## CREATE-005 — Custom cache root keeps checkouts pin-isolated

### Expected
`--cache-root DIR` is honoured, and non-vendored LEZ lands in `DIR/repos/lez/<pin>/`.
Different pins never share a checkout.

### Preconditions
None.

### Steps
1. `"$SCAFFOLD_BIN" new dogfood-cache --cache-root "$SCRATCH_ROOT/custom-cache"`
2. `find "$SCRATCH_ROOT/custom-cache/repos/lez" -mindepth 1 -maxdepth 2 -type d`

### Success criteria
- A directory named after the pin exists under the custom cache root.

## CREATE-006 — Non-default LEZ source gets its own checkout

### Expected
`--lez-path <checkout>` (legacy alias `--lssa-path`) clones into
`<cache>/repos/lez/<pin>-src-<hash>/`. It leaves the default-source `<pin>/` checkout and its
`target/` alone. `test-node pins --json` in that project reports the same `<pin>-src-<hash>`
directory with `ownership: managed_cache`. A path that does not exist fails with an error
naming that path.

### Preconditions
A real LEZ checkout on disk. Skip this scenario if none is available.

### Steps
1. `"$SCAFFOLD_BIN" new dogfood-lezpath --lez-path /abs/path/to/lez`
2. `( cd dogfood-lezpath && "$SCAFFOLD_BIN" setup )`
3. `"$SCAFFOLD_BIN" test-node pins --project dogfood-lezpath --json | jq '.lez_checkout, .ownership'`
4. `"$SCAFFOLD_BIN" new dogfood-badpath --lez-path /does/not/exist`

### Success criteria
- `<pin>-src-<hash>/` exists, and the mtime of `<pin>/target` is unchanged.
- Step 3 reports that directory with `managed_cache`.
- Step 4 fails and names `/does/not/exist`.

### Notes
Projects created by scaffold ≤ 0.4.0 with a non-default source rebuild once into
`<pin>-src-<hash>/` (about 10 minutes). That is expected.

## CREATE-007 — Adopt an existing directory with init

### Expected
In a directory with no `scaffold.toml`, `init` writes a schema-0.2.0 `scaffold.toml` with
defaults, creates `.scaffold/{state,logs}`, appends `.scaffold` to `.gitignore` and installs
the AI skills. `--dry-run` previews without writing.

### Preconditions
None.

### Steps
1. `mkdir "$SCRATCH_ROOT/adopt" && cd "$SCRATCH_ROOT/adopt"`
2. `"$SCAFFOLD_BIN" init --dry-run && ls -A`
3. `"$SCAFFOLD_BIN" init && ls -A && cat .gitignore`

### Success criteria
- After step 2 the directory is still empty.
- After step 3 the directory contains `scaffold.toml`, `.scaffold/state`, `.scaffold/logs`
  and `AGENTS.md`, and `.gitignore` contains `.scaffold`.

## CREATE-008 — Migrate an old scaffold.toml with init

### Expected
Every command except `init` refuses a pre-0.2.0 `scaffold.toml` with a single line that names
the stale shape and says to run `logos-scaffold init`. `init` rewrites the file in place,
keeping comments and key order. It moves `[basecamp].pin/.source/.lgpm_flake` to
`[repos.basecamp]`/`[repos.lgpm]` and `[basecamp.modules.*]` to `[modules.*]`, drops the
legacy `url` fields and bumps `[scaffold].version` to `0.2.0`. It writes `scaffold.toml.bak`
unless `--no-backup` is passed. Re-running `init` on a current file leaves it unchanged.

### Preconditions
A project from CREATE-001.

### Steps
1. Edit `scaffold.toml`: set `[scaffold].version = "0.1.0"` and add `url = "https://example"`
   under `[repos.lez]`, plus a comment line.
2. `"$SCAFFOLD_BIN" doctor --json; echo "exit=$?"`
3. `"$SCAFFOLD_BIN" init --dry-run` then `"$SCAFFOLD_BIN" init`
4. `diff scaffold.toml.bak scaffold.toml`, then `"$SCAFFOLD_BIN" init` a second time.

### Success criteria
- Step 2 exits non-zero with the "old schema … Run `logos-scaffold init`" line, and no JSON
  is mixed in.
- After step 3 the version is `0.2.0`, `url` is gone, the comment survives and `.bak` exists.
- The second `init` succeeds and leaves `scaffold.toml` byte-identical.
