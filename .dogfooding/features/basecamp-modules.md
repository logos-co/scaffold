---
feature: basecamp-modules
prefix: BCMOD
surface: cli
scenarios:
  - id: BCMOD-001
    title: Capture project modules into scaffold.toml
    priority: P1
    provenance: documented
    source: DOGFOODING.md B2
    depends_on: [BCSETUP-001]
  - id: BCMOD-002
    title: Declared dependencies resolve or fail fast
    priority: P1
    provenance: documented
    source: DOGFOODING.md B2
    depends_on: [BCMOD-001]
  - id: BCMOD-003
    title: Install captured modules into both profiles
    priority: P1
    provenance: documented
    source: DOGFOODING.md B2
    depends_on: [BCMOD-001]
  - id: BCMOD-004
    title: A flake with only lgx-portable fails explicitly
    priority: P2
    provenance: documented
    source: DOGFOODING.md B2
    depends_on: [BCSETUP-001]
  - id: BCMOD-005
    title: Build module artefacts by variant
    priority: P2
    provenance: documented
    source: DOGFOODING.md B5
    depends_on: [BCMOD-001]
  - id: BCMOD-006
    title: build-portable stages ordered symlinks
    priority: P2
    provenance: documented
    source: docs/commands.md
    depends_on: [BCMOD-001]
  - id: BCMOD-007
    title: Run or develop a captured module standalone
    priority: P2
    provenance: documented
    source: DOGFOODING.md B6
    depends_on: [BCMOD-001]
  - id: BCMOD-008
    title: Prebuilt lgx modules are rejected by run
    priority: P2
    provenance: documented
    source: DOGFOODING.md B6
    depends_on: [BCMOD-001]
retired: []
---

# Basecamp modules

`basecamp modules` is the only automated writer of the captured module set: top-level
`[modules.<name>]` sections in `scaffold.toml` with `flake` and `role = "project" |
"dependency"`. `install`, `build`, `build-portable`, `run` and `develop` replay that set.
Multi-flake projects get `--override-input <input> path:<sibling>` for every
`path:../<sibling>` input, keyed by the declared input name. Related:
[Basecamp setup](basecamp-setup.md), [Basecamp launch](basecamp-launch.md).

## BCMOD-001 — Capture project modules into scaffold.toml

### Expected
With no arguments, `basecamp modules` auto-discovers the root `.#lgx`, or else the immediate
sub-flakes exposing it. It writes one `[modules.<name>]` per source, named from
`metadata.json.name` for local sources and from the repo slug for `github:` refs (with one
assumption note on stderr). Explicit `--flake` / `--path` sources are also accepted. Re-runs
are byte-identical and never overwrite an existing key, so hand edits win. `--show` prints the
set without changing anything.

### Preconditions
BCSETUP-001.

### Steps
1. `"$SCAFFOLD_BIN" basecamp modules`
2. `grep -n '^\[modules\.' scaffold.toml && shasum scaffold.toml > /tmp/m1`
3. `"$SCAFFOLD_BIN" basecamp modules && shasum scaffold.toml | diff /tmp/m1 -`
4. `"$SCAFFOLD_BIN" basecamp modules --show`

### Success criteria
- At least one `role = "project"` entry is written, and in-project flakes are stored
  relative (`path:.#lgx`-style).
- The step 3 diff is empty, and `--show` lists the same set.

## BCMOD-002 — Declared dependencies resolve or fail fast

### Expected
Every name in a project source's `dependencies` / `optional_dependencies` (plain or
`{name, version, signer}` form) is resolved. A name already keyed in `[modules]` or bundled by
basecamp (`capability_module`, `modules_state`, `package_downloader`, `package_manager`,
`package_manager_ui`) needs nothing. Otherwise the source's `flake.lock` or scaffold's default
table is used, and the result is inserted with `role = "dependency"`. An unresolvable required
dep fails before anything is written, naming the dep and two fixes. An unresolvable optional
dep is skipped with a `note:`. A dep on a name an older basecamp bundled (`main_ui`,
`counter`, …) names the declaring `metadata.json` as the fix.

### Preconditions
BCMOD-001, and a module whose `metadata.json` declares a dependency.

### Steps
1. Add `"no_such_module"` to a project source's `dependencies`, restore `scaffold.toml`
   without its `[modules]`, then run `"$SCAFFOLD_BIN" basecamp modules; echo "exit=$?"`.
2. Change it to `"main_ui"` and re-run, then revert `metadata.json`.

### Success criteria
- Step 1 exits non-zero, names `no_such_module` and both fixes, and leaves `scaffold.toml`
  unchanged.
- Step 2's error points at the declaring `metadata.json`.

## BCMOD-003 — Install captured modules into both profiles

### Expected
`basecamp install` builds every captured source (dependencies first) and installs it with
`lgpm` into both `alice` and `bob`. It logs to `.scaffold/logs/<ts>-install.log` and prints a
one-line status per build with its duration. `--print-output` (or
`LOGOS_SCAFFOLD_PRINT_OUTPUT=1`) streams nix output. Any build or install failure exits
non-zero. Afterwards, `basecamp doctor` shows the installed modules matching `[modules]`,
with no false `uncaptured` drift for relatively stored sources.

### Preconditions
BCMOD-001.

### Steps
1. `"$SCAFFOLD_BIN" basecamp install`
2. `"$SCAFFOLD_BIN" basecamp install --print-output`
3. `"$SCAFFOLD_BIN" basecamp doctor`

### Success criteria
- Steps 1 and 2 exit 0, and the log file exists. Doctor shows no drift.

## BCMOD-004 — A flake with only lgx-portable fails explicitly

### Expected
A source exposing `.#lgx-portable` but not `.#lgx` fails capture or install with a targeted
error naming the missing `.#lgx` and suggesting `--flake <ref>#lgx-portable`. It never falls
back silently.

### Preconditions
BCSETUP-001, and a flake that exposes only `packages.<system>.lgx-portable`.

### Steps
1. `"$SCAFFOLD_BIN" basecamp modules --flake ./portable-only; echo "exit=$?"`

### Success criteria
- Non-zero exit, and the message names `lgx` and suggests `#lgx-portable`.

## BCMOD-005 — Build module artefacts by variant

### Expected
`basecamp build --variant lgx|lgx-portable|all` (default `all`) builds every `role =
"project"` entry in dependency order without installing, and skips `role = "dependency"`.
`--module NAME` builds only that module and still applies the sibling `--override-input`
flags. An unknown module, or a flake missing the requested variant, fails with a targeted
error.

### Preconditions
BCMOD-001.

### Steps
1. `"$SCAFFOLD_BIN" basecamp build --variant all`
2. `"$SCAFFOLD_BIN" basecamp build --variant lgx-portable --module <ui-module-with-sibling-input>`
3. `"$SCAFFOLD_BIN" basecamp build --module nope; echo "exit=$?"`

### Success criteria
- Steps 1 and 2 exit 0. Step 2 has no `forbidden in pure evaluation mode` error.
- Step 3 fails and names the captured modules.

## BCMOD-006 — build-portable stages ordered symlinks

### Expected
`basecamp build-portable` (alias for `build --variant lgx-portable`) wipes and recreates
`.scaffold/basecamp/portable/`. It fills it with `<NN>-<module_name>.lgx` symlinks into the
nix store, numbered so leaves come first. It never runs `lgpm`, and never touches
`basecamp.state` or the profiles.

### Preconditions
BCMOD-001, with flakes exposing `.#lgx-portable`.

### Steps
1. `"$SCAFFOLD_BIN" basecamp build-portable && ls -l .scaffold/basecamp/portable/`
2. Remove one module from `[modules]` and run step 1 again.

### Success criteria
- The symlinks are numbered `01-…`, `02-…` in dependency order.
- After step 2 there is no stale symlink for the removed module.

## BCMOD-007 — Run or develop a captured module standalone

### Expected
`basecamp run <module> --host standalone` (also the default host) runs `nix run` on the
module's flake default app, or `#<standalone_app>` if that is configured. It passes the same
sibling overrides as `install`, so the log shows `Module loaded: <sibling>`. `basecamp develop
<module>` runs `nix develop` with the same overrides from the project root and exports
`SCAFFOLD_PROJECT_ROOT`/`LOGOS_PROFILE`. An unknown module fails before any nix call.

### Preconditions
BCMOD-001. The module flake exposes `apps.<system>.default`.

### Steps
1. `"$SCAFFOLD_BIN" basecamp run <module>`
2. `"$SCAFFOLD_BIN" basecamp develop <module>`, then run `echo $SCAFFOLD_PROJECT_ROOT; exit`
   inside the shell.
3. `"$SCAFFOLD_BIN" basecamp develop nope; echo "exit=$?"`

### Success criteria
- The app starts without a pure-eval error, and the dev shell prints the project root.
- Step 3 fails fast and lists the captured modules.

## BCMOD-008 — Prebuilt lgx modules are rejected by run

### Expected
`basecamp run` on a module captured from a `.lgx` path is rejected, with guidance to capture a
flake source instead. `nix run` is not attempted.

### Preconditions
BCMOD-001, plus a module captured with `basecamp modules --path /abs/prebuilt.lgx`.

### Steps
1. `"$SCAFFOLD_BIN" basecamp run <path-captured-module>; echo "exit=$?"`

### Success criteria
- Non-zero exit, the guidance text is shown, and no `nix run` is echoed.
