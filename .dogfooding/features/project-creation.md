---
feature: project-creation
prefix: CREATE
surface: cli
scenarios:
  - id: CREATE-001
    title: Create a default-template project
    priority: P0
    provenance: documented
    source: DOGFOODING.md@740e5e9 D1
  - id: CREATE-003
    title: Template selection and the create alias
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 E2
    depends_on: [CREATE-001]
  - id: CREATE-004
    title: Vendor dependencies into the project
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 E2
  - id: CREATE-005
    title: Cache checkouts are isolated by cache root, pin and LEZ source
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 E2
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
    depends_on: [CREATE-001]
retired: [CREATE-002, CREATE-006]
---

# Project creation

Developers get a runnable project in one command (`new`/`create`), or adopt scaffold in an
existing directory with `init`. `init` is also the single migration entry point for older
`scaffold.toml` schemas. Related: [Setup and build](setup-and-build.md),
[spel template](spel-template.md), [AI skills](ai-skills.md).

## CREATE-001 — Create a default-template project

### Expected
`new <name>` creates `<name>/` from the `default` template and prints the destination path,
the pinned LEZ commit and the cache root. The generated `scaffold.toml` is schema `0.2.0`
with `[repos.lez]` and `[repos.spel]` (source and pin, no `path` key), `[repos.basecamp]`,
`[repos.lgpm]`, a `[circuits]` table, and `[wallet].home_dir`. It has no wallet binary field.
The generated `Cargo.toml` pins `risc0-zkvm` and `risc0-build` with an exact `=3.0.5`
requirement, the version of the `r0vm` that scaffold provisions.

### Preconditions
Freshly built `$SCAFFOLD_BIN`, a writable `$SCRATCH_ROOT`, and network access: the first
`new` on a cache root clones LEZ into it (about a minute).

### Steps
1. `cd "$SCRATCH_ROOT" && "$SCAFFOLD_BIN" new dogfood-default`
2. `grep -n '^\[' dogfood-default/scaffold.toml; grep -n 'binary\|home_dir' dogfood-default/scaffold.toml`
3. `ls dogfood-default/src/bin; grep -n 'risc0' dogfood-default/Cargo.toml`

### Success criteria
- Exit 0, and the output names the path, LEZ pin and cache root.
- `scaffold.toml` has `[circuits]` and `[wallet]` with `home_dir`, and no `binary =` line.
- `src/bin/` contains the `run_hello_world*.rs` example runners, and `risc0-zkvm` and
  `risc0-build` are pinned with `"=3.0.5"`.

## CREATE-003 — Template selection and the create alias

### Expected
`create <name>` behaves exactly like `new <name>`. An unknown `--template` fails with an
error that lists `default` and `spel` and mentions the deprecated `lez-framework` alias; it
never falls back to `default` silently. `--template lez-framework` still works, prints a
deprecation warning and produces a spel project.

### Preconditions
CREATE-001. Run step 3 after SPEL-001, so the spel CLI is already built; otherwise this step
builds it first (minutes).

### Steps
1. `cd "$SCRATCH_ROOT" && "$SCAFFOLD_BIN" create dogfood-create`, then
   `diff <(cd dogfood-default && find . -path ./.scaffold -prune -o -type f -print | sort) <(cd dogfood-create && find . -path ./.scaffold -prune -o -type f -print | sort)`
   (compare before anything is built in `dogfood-default`, or prune `target` too).
2. `"$SCAFFOLD_BIN" new dogfood-invalid --template nonexistent-template; echo "exit=$?"; ls -d dogfood-invalid`
3. `"$SCAFFOLD_BIN" new dogfood-lez-alias --template lez-framework && ls dogfood-lez-alias/spel.toml`

### Success criteria
- Step 1 exits 0 and the `diff` is empty.
- Step 2 exits non-zero, names `default` and `spel`, and creates no directory.
- Step 3 warns that `lez-framework` is deprecated in favour of `--template spel`, and the
  result contains `spel.toml`.

## CREATE-004 — Vendor dependencies into the project

### Expected
`--vendor-deps` clones the pinned dependency into the project and records its relative path
in `scaffold.toml`. For the default template that is LEZ at creation
(`.scaffold/repos/lez`, `[repos.lez].path`); spel is vendored there when `setup` first needs
it. For `--template spel` it is spel too (`.scaffold/repos/spel`, `[repos.spel].path`), and
the clone happens after `spel init` returns, so creation never fails with `Directory '<name>'
already exists`.

### Preconditions
For step 2, SPEL-001 has built the spel CLI (or allow minutes for it).

### Steps
1. `"$SCAFFOLD_BIN" new dogfood-vendor --vendor-deps`, then
   `git -C dogfood-vendor/.scaffold/repos/lez rev-parse HEAD; grep -A4 'repos.lez' dogfood-vendor/scaffold.toml`
2. `"$SCAFFOLD_BIN" new dogfood-spel-vendored --template spel --vendor-deps`, then
   `git -C dogfood-spel-vendored/.scaffold/repos/spel rev-parse HEAD; grep -A3 'repos.spel' dogfood-spel-vendored/scaffold.toml`

### Success criteria
- Each vendored HEAD equals the pin recorded in its `[repos.*]` table, and `path` is the
  relative `.scaffold/repos/<name>`.

## CREATE-005 — Cache checkouts are isolated by cache root, pin and LEZ source

### Expected
`--cache-root DIR` is honoured and written to `scaffold.toml`, and non-vendored LEZ lands in
`DIR/repos/lez/<pin>/`. Checkouts are never shared across pins or sources: a project with a
different `[repos.lez].pin` gets `DIR/repos/lez/<other-pin>/`, and a project created with
`--lez-path <checkout>` (legacy alias `--lssa-path`) gets `DIR/repos/lez/<pin>-src-<hash>/`,
which `test-node pins --json` reports with `checkout_ownership: managed_cache`. Existing
checkouts keep their HEAD and build output. A `--lez-path` that does not exist fails with an
error naming it.

### Preconditions
`LOGOS_SCAFFOLD_CACHE_ROOT` **unset** for every command here (`env -u
LOGOS_SCAFFOLD_CACHE_ROOT …`): the variable outranks `[scaffold].cache_root`, so with it
exported this scenario tests the wrong layer. An older LEZ commit SHA for step 2 (for
example an earlier release tag's commit). A local LEZ clone for step 3, for example
`git clone <cache>/repos/lez/<pin> /tmp/lez-src` (TNODE-006 uses the same clone).

### Steps
1. `"$SCAFFOLD_BIN" new dogfood-cache --cache-root "$SCRATCH_ROOT/custom-cache"`
2. `"$SCAFFOLD_BIN" new dogfood-cache2 --cache-root "$SCRATCH_ROOT/custom-cache"`, set
   `[repos.lez].pin` in `dogfood-cache2/scaffold.toml` to the older SHA, then
   `( cd dogfood-cache2 && "$SCAFFOLD_BIN" setup )`. Only the clone matters: stop `setup` once
   `custom-cache/repos/lez/<older-sha>/.git` exists (`pgrep -x cargo | xargs kill` afterwards).
3. `"$SCAFFOLD_BIN" new dogfood-lezpath --cache-root "$SCRATCH_ROOT/custom-cache" --lez-path /tmp/lez-src`,
   then `"$SCAFFOLD_BIN" test-node pins --project dogfood-lezpath --json | jq '.lez_checkout, .checkout_ownership'`
4. `for d in "$SCRATCH_ROOT"/custom-cache/repos/lez/*/; do echo "$d $(git -C "$d" rev-parse HEAD)"; done`,
   then `( cd dogfood-cache2 && "$SCAFFOLD_BIN" doctor | grep 'cache root' )`
5. `"$SCAFFOLD_BIN" new dogfood-badpath --lez-path /does/not/exist; echo "exit=$?"`

### Success criteria
- `scaffold.toml` of each project has `cache_root = "<custom-cache>"`.
- Step 4 lists three directories: `<pin>`, `<older-sha>` and `<pin>-src-<hash>`, each at its
  own commit; the first one's HEAD did not move. `doctor` says `(from scaffold.toml
  [scaffold].cache_root)`.
- Step 3 reports the `<pin>-src-<hash>` directory with `managed_cache`.
- Step 5 exits non-zero and names `/does/not/exist`.

### Notes
Projects created by scaffold ≤ 0.4.0 with a non-default source rebuild once into
`<pin>-src-<hash>/` (about 10 minutes). That is expected.

## CREATE-007 — Adopt an existing directory with init

### Expected
In a directory with no `scaffold.toml`, `init` writes a schema-0.2.0 `scaffold.toml` with
defaults, creates `.scaffold/{state,logs}`, appends `.scaffold` to `.gitignore` and installs
the AI skills (`AGENTS.md`, `.claude/skills/`, `.cursor/rules/`). `--dry-run` previews
without writing.

### Preconditions
None.

### Steps
1. `mkdir "$SCRATCH_ROOT/adopt" && cd "$SCRATCH_ROOT/adopt" && "$SCAFFOLD_BIN" init --dry-run && ls -A`
2. `"$SCAFFOLD_BIN" init && ls -A . .scaffold && cat .gitignore`

### Success criteria
- After step 1 the directory is still empty.
- After step 2 it contains `scaffold.toml`, `.scaffold/state`, `.scaffold/logs`, `AGENTS.md`,
  `.claude` and `.cursor`, and `.gitignore` contains `.scaffold`.

## CREATE-008 — Migrate an old scaffold.toml with init

### Expected
Every command except `init` refuses a pre-0.2.0 `scaffold.toml` with a single line that names
the stale shape and says to run `logos-scaffold init`. `init` rewrites the file in place,
keeping comments and key order. It moves `[basecamp].pin/.source/.lgpm_flake` to
`[repos.basecamp]`/`[repos.lgpm]` and `[basecamp.modules.*]` to `[modules.*]`, drops the
legacy `url` fields and bumps `[scaffold].version` to `0.2.0`. It writes `scaffold.toml.bak`
unless `--no-backup` is passed. Re-running `init` on a current file leaves it unchanged.

### Preconditions
A copy of a CREATE-001 project (no build needed).

### Steps
1. Edit `scaffold.toml`: set `[scaffold].version = "0.1.0"`, add `url = "https://example"`
   under `[repos.lez]` plus a comment line, and add the old basecamp shape:
   ```toml
   [basecamp]
   pin = "abc123"
   source = "https://github.com/logos-co/logos-basecamp.git"
   lgpm_flake = "github:logos-co/logos-package-manager"

   [basecamp.modules.my_mod]
   flake = "path:./mod"
   role = "project"
   ```
2. `"$SCAFFOLD_BIN" doctor --json; echo "exit=$?"`
3. `"$SCAFFOLD_BIN" init --dry-run && "$SCAFFOLD_BIN" init && diff scaffold.toml.bak scaffold.toml`
4. `shasum scaffold.toml > /tmp/mig.sha && "$SCAFFOLD_BIN" init && shasum -c /tmp/mig.sha`

### Success criteria
- Step 2 exits non-zero with the "old schema … Run `logos-scaffold init`" line, and no JSON
  is mixed in. The dry run writes nothing.
- After step 3 the version is `0.2.0`, `url` is gone, the comment survives and `.bak` exists.
  The file has `[repos.basecamp]` (`pin = "abc123"`), `[repos.lgpm]` and `[modules.my_mod]`,
  and no `[basecamp]` tables. Because the old `lgpm_flake` had no `/<rev>` suffix, `init`
  keeps the default lgpm pin and prints a `!` warning to edit it.
- Step 4 succeeds and leaves `scaffold.toml` byte-identical.
