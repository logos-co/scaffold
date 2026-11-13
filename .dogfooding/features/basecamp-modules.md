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
assumption note on stderr). When sub-flakes are probed, each probe gets `--override-input
<input> path:<abs-sibling>` for every `path:../<sibling>` input, so a UI sub-flake that
points at a sibling evaluates in pure mode. Explicit `--flake` / `--path` sources are also
accepted and recorded without probing: a plain `./sub#lgx` is stored as `path:./sub#lgx`,
and a `.lgx` path is stored as given and named from a sibling `metadata.json`, else from
the file stem with a `note: flake … — assumed module_name = …` line. Re-runs are
byte-identical and never overwrite an existing key, so hand edits win. `--show` prints the set without
changing anything.

### Preconditions
BCSETUP-001. For step 5, a project with no root `.#lgx` whose sub-flakes include one that
declares `<input>.url = "path:../<sibling>"` (for example `templates/minimal-module` plus
`templates/ui-qml`). For step 6, a prebuilt `.lgx` file. Back up `scaffold.toml`.

### Steps
1. `"$SCAFFOLD_BIN" basecamp modules`
2. `grep -n '^\[modules\.' scaffold.toml && shasum scaffold.toml > /tmp/m1`
3. `"$SCAFFOLD_BIN" basecamp modules && shasum scaffold.toml | diff /tmp/m1 -`
4. `"$SCAFFOLD_BIN" basecamp modules --show`
5. In the multi-flake project, with no `[modules]` yet: `"$SCAFFOLD_BIN" basecamp modules`.
6. Remove `[modules]`, then run `"$SCAFFOLD_BIN" basecamp modules --flake "./<sub>#lgx"
   --path /abs/prebuilt.lgx` and `grep -A2 '^\[modules\.' scaffold.toml`. Restore
   `scaffold.toml`.

### Success criteria
- At least one `role = "project"` entry is written, and in-project flakes are stored
  relative (`path:.#lgx`-style).
- The step 3 diff is empty, and `--show` lists the same set.
- Step 5 exits 0 and captures both sub-flakes, with no `forbidden in pure evaluation mode`
  or `nix eval … failed` error.
- Step 6 exits 0 and writes exactly the two given sources as `role = "project"`:
  `flake = "path:./<sub>#lgx"` and `flake = "/abs/prebuilt.lgx"`, plus any declared
  dependencies.

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
non-zero, including an `lgpm install` that rejects a package. Afterwards, `basecamp doctor`
shows the installed modules matching `[modules]`, with no false `uncaptured` drift for
relatively stored sources. Each profile's `modules_dir` holds only modules named in
`[modules]`, never basecamp's bundled ones.

### Preconditions
BCMOD-001. For step 4, `scaffold.toml` backed up.

### Steps
1. `"$SCAFFOLD_BIN" basecamp install`
2. `"$SCAFFOLD_BIN" basecamp install --print-output`
3. `"$SCAFFOLD_BIN" basecamp doctor` and
   `ls "$("$SCAFFOLD_BIN" basecamp paths alice --json | jq -r .modules_dir)"`
4. `mkdir -p /tmp/badlgx && head -c 512 /dev/urandom > /tmp/badlgx/broken.lgx`, then
   `"$SCAFFOLD_BIN" basecamp modules --path /tmp/badlgx/broken.lgx` and
   `"$SCAFFOLD_BIN" basecamp install; echo "exit=$?"`. Restore `scaffold.toml`.

### Success criteria
- Steps 1 and 2 exit 0, and the log file exists. Doctor shows no drift.
- The `ls` in step 3 lists only names keyed in `[modules]`, and none of
  `capability_module`, `modules_state`, `package_downloader` or `package_manager`.
- Step 4 exits non-zero, and the error names `lgpm install /tmp/badlgx/broken.lgx into
  alice`.

## BCMOD-004 — A flake with only lgx-portable fails explicitly

### Expected
When auto-discovery finds a flake exposing `.#lgx-portable` but not `.#lgx`, capture fails
with a targeted error naming the missing `.#lgx` and suggesting `--flake <ref>#lgx-portable`.
It never falls back silently. An explicit `--flake` is recorded as given, without probing.

### Preconditions
BCSETUP-001, and a project whose only flake (at the root or in a sub-directory) exposes just
`packages.<system>.lgx-portable`, with no `[modules]` captured yet.

### Steps
1. `"$SCAFFOLD_BIN" basecamp modules; echo "exit=$?"` (no arguments, so discovery runs)

### Success criteria
- Non-zero exit, and the message names `lgx` and suggests `#lgx-portable`.

## BCMOD-005 — Build module artefacts by variant

### Expected
`basecamp build --variant lgx|lgx-portable|all` (default `all`) builds every `role =
"project"` entry in dependency order without installing, and skips `role = "dependency"`.
`--module NAME` builds only that module and still applies the sibling `--override-input`
flags. Each variant's outputs are symlinked as `<NN>-<module>.lgx` into its own directory:
`lgx` into `.scaffold/basecamp/lgx/`, `lgx-portable` into `.scaffold/basecamp/portable/`.
An unknown module fails with a `no project module` error that lists the captured project
modules. A flake missing the requested variant fails with a message that names the flake,
says it `does not expose` the variant and asks for a `packages.<system>.<variant>` output,
not a raw nix trace. The CLI accepts only `lgx`, `lgx-portable` and `all`.

### Preconditions
BCMOD-001. For step 5, a temporary `[modules]` entry (`role = "project"`) whose flake
exposes `.#lgx` but not `.#lgx-portable`. Add it after steps 1–4 and remove it afterwards.

### Steps
1. `"$SCAFFOLD_BIN" basecamp build --variant all && ls -l .scaffold/basecamp/lgx/ .scaffold/basecamp/portable/`
2. `"$SCAFFOLD_BIN" basecamp build --variant lgx-portable --module <ui-module-with-sibling-input>`
3. `"$SCAFFOLD_BIN" basecamp build --module nope; echo "exit=$?"`
4. `"$SCAFFOLD_BIN" basecamp build --variant lgx && ls .scaffold/basecamp/lgx/`
5. `"$SCAFFOLD_BIN" basecamp build --variant lgx-portable --module <lgx-only-module>; echo "exit=$?"`
6. `"$SCAFFOLD_BIN" basecamp build --variant bogus; echo "exit=$?"`

### Success criteria
- Steps 1 and 2 exit 0. Step 1 fills both directories with `01-…` symlinks into the nix
  store. Step 2 has no `forbidden in pure evaluation mode` error.
- Step 3 fails and names the captured modules.
- Step 4 exits 0, builds only `#lgx` (no `#lgx-portable` line) and leaves symlinks in
  `.scaffold/basecamp/lgx/`.
- Step 5 exits non-zero, and the message says the flake does not expose `lgx-portable`.
- Step 6 is refused by argument parsing, listing the possible values, before any build.

### Notes
The Rust API takes free-form variant strings. Its rejection of an empty list, an unknown or
path-like variant and its de-duplication (`normalize_build_variants`) cannot be reached from
the CLI. The unit test `normalize_build_variants_rejects_empty_unknown_and_dedupes` covers
it.

## BCMOD-006 — build-portable stages ordered symlinks

### Expected
`basecamp build-portable` (alias for `build --variant lgx-portable`) wipes and recreates
`.scaffold/basecamp/portable/`. It fills it with `<NN>-<module_name>.lgx` symlinks into the
nix store, numbered so leaves come first. It never runs `lgpm`, and never touches
`basecamp.state` or the profiles. `--module NAME` narrows the run to one captured project
module, and the directory then holds only that module's link.

### Preconditions
BCMOD-001, with flakes exposing `.#lgx-portable`.

### Steps
1. `"$SCAFFOLD_BIN" basecamp build-portable && ls -l .scaffold/basecamp/portable/`
2. Remove one module from `[modules]` and run step 1 again.
3. `"$SCAFFOLD_BIN" basecamp build-portable --module <module> && ls .scaffold/basecamp/portable/`

### Success criteria
- The symlinks are numbered `01-…`, `02-…` in dependency order.
- After step 2 there is no stale symlink for the removed module.
- Step 3 exits 0 and the directory holds only `01-<module>.lgx`.

## BCMOD-007 — Run or develop a captured module standalone

### Expected
`basecamp run <module> --host standalone` (also the default host) runs `nix run` on the
module's flake default app, or `#<standalone_app>` if that is configured. It passes the same
sibling overrides as `install`, so the log shows `Module loaded: <sibling>`. `basecamp develop
<module>` runs `nix develop` with the same overrides from the project root and exports
`SCAFFOLD_PROJECT_ROOT`/`LOGOS_PROFILE`. `--dev-shell ATTR` selects `<flake>#<ATTR>` instead
of the default dev shell. Both commands drop the stored `#lgx` fragment and print the target
first, as `running module … (standalone host: <target>)` or `entering dev shell for module
… (<target>)`. A remote ref without a fragment still gets the requested attr, so
`standalone_app = "x"` on `github:o/r/rev` runs `github:o/r/rev#x`. A module captured without
`standalone_app` never gets `standalone_app = ""` written back. An unknown module fails
before any nix call.

### Preconditions
BCMOD-001. The module flake exposes `apps.<system>.default`. For step 4, `<attr>` is any
dev shell the flake exposes (`default` works). Back up `scaffold.toml` for step 5.

### Steps
1. `"$SCAFFOLD_BIN" basecamp run <module>`
2. `"$SCAFFOLD_BIN" basecamp develop <module>`, then run `echo $SCAFFOLD_PROJECT_ROOT; exit`
   inside the shell.
3. `"$SCAFFOLD_BIN" basecamp develop nope; echo "exit=$?"`
4. `"$SCAFFOLD_BIN" basecamp develop <module> --dev-shell <attr>`, then `exit` the shell.
5. Add `[modules.remote]` with `flake` set to a captured dependency's `github:` ref minus its
   `#lgx`, `role = "project"` and `standalone_app = "default"`. Run
   `"$SCAFFOLD_BIN" basecamp run remote` and stop it after the first line. Restore
   `scaffold.toml`.
6. `"$SCAFFOLD_BIN" basecamp modules && grep -c 'standalone_app' scaffold.toml`

### Success criteria
- The app starts without a pure-eval error, and the dev shell prints the project root.
- Step 3 fails fast and lists the captured modules.
- Step 4's first line ends in `#<attr>)`, and the shell opens.
- Step 5's first line shows `standalone host: github:…#default`.
- Step 6 prints `0` when no entry was hand-given a `standalone_app`.

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
