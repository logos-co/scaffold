---
feature: basecamp-modules
prefix: BCMOD
surface: cli
scenarios:
  - id: BCMOD-001
    title: Capture project modules into scaffold.toml
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 B2
    depends_on: [BCSETUP-001]
  - id: BCMOD-002
    title: Capture fails fast on unresolvable dependencies or a portable-only flake
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 B2
    depends_on: [BCMOD-001]
  - id: BCMOD-003
    title: Install captured modules into both profiles
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 B2
    depends_on: [BCMOD-001]
  - id: BCMOD-005
    title: Build module artefacts by variant, staged as ordered symlinks
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B5
    depends_on: [BCMOD-001]
  - id: BCMOD-007
    title: Run or develop a captured module standalone
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B6
    depends_on: [BCMOD-001]
retired: [BCMOD-004, BCMOD-006, BCMOD-008]
---

# Basecamp modules

`basecamp modules` is the only automated writer of the captured module set: top-level
`[modules.<name>]` sections in `scaffold.toml` with `flake` and `role = "project" |
"dependency"`. `install`, `build`, `build-portable`, `run` and `develop` replay that set.
Multi-flake projects get `--override-input <input> path:<sibling>` for every
`path:../<sibling>` input, keyed by the declared input name. Related:
[Basecamp setup](basecamp-setup.md), [Basecamp launch](basecamp-launch.md).

The scenarios edit `scaffold.toml` and `metadata.json`: back both up first and restore them
after each scenario.

## BCMOD-001 — Capture project modules into scaffold.toml

### Expected
With no arguments, `basecamp modules` auto-discovers the root `.#lgx`, or else the immediate
sub-flakes exposing it. It writes one `[modules.<name>]` per source, named from
`metadata.json.name` for local sources and from the repo slug for `github:` refs (with one
assumption note on stderr). When sub-flakes are probed, each probe gets `--override-input
<input> path:<abs-sibling>` for every `path:../<sibling>` input, so a UI sub-flake that
points at a sibling evaluates in pure mode. Explicit `--flake` / `--path` sources are
recorded without probing: a plain `./sub#lgx` is stored as `path:./sub#lgx`, and a `.lgx`
path is stored as given and named from a sibling `metadata.json`, else from the file stem
with a `note: flake … — assumed module_name = …` line. Re-runs are byte-identical and never
overwrite an existing key, so hand edits win. `--show` prints the set without changing
anything.

### Preconditions
BCSETUP-001. For step 3, a project with no root `.#lgx` whose sub-flakes include one that
declares `<input>.url = "path:../<sibling>"` (`templates/minimal-module` plus
`templates/ui-qml`). For step 4, a prebuilt `.lgx` file.

### Steps
1. `"$SCAFFOLD_BIN" basecamp modules && grep -n -A2 '^\[modules\.' scaffold.toml && shasum scaffold.toml > /tmp/m1`
2. `"$SCAFFOLD_BIN" basecamp modules && shasum -c /tmp/m1 && "$SCAFFOLD_BIN" basecamp modules --show`
3. In the multi-flake project, with no `[modules]` yet: `"$SCAFFOLD_BIN" basecamp modules`.
4. Remove `[modules]`, then run `"$SCAFFOLD_BIN" basecamp modules --flake "./<sub>#lgx"
   --path /abs/prebuilt.lgx` and `grep -A2 '^\[modules\.' scaffold.toml`.

### Success criteria
- Step 1 writes at least one `role = "project"` entry, with in-project flakes stored
  relative (`path:.#lgx`-style).
- Step 2 leaves the file byte-identical, and `--show` lists the same set.
- Step 3 exits 0 and captures both sub-flakes, with no `forbidden in pure evaluation mode`
  or `nix eval … failed` error.
- Step 4 exits 0 and writes exactly the two given sources as `role = "project"`:
  `flake = "path:./<sub>#lgx"` and `flake = "/abs/prebuilt.lgx"`, plus any declared
  dependencies.

### Notes
Only `path:../<sibling>` inputs are rewritten; `path:./sub`, `github:` and `git+` inputs pass
through. The parser reads `flake.nix` line by line, so a multi-line input declaration can be
missed: record the `flake.nix` excerpt if a sibling override is not applied. An unresolved
transitive `logos-module-builder` input surfaces as Nix's own error (`nix eval <ref> failed
(…): <stderr>` plus the `logos-scaffold basecamp docs` pointer), never as `no .lgx sources
found`.

## BCMOD-002 — Capture fails fast on unresolvable dependencies or a portable-only flake

### Expected
Every name in a project source's `dependencies` / `optional_dependencies` (plain or
`{name, version, signer}` form) is resolved. A name already keyed in `[modules]` or bundled by
basecamp (`capability_module`, `modules_state`, `package_downloader`, `package_manager`,
`package_manager_ui`) needs nothing. Otherwise the source's `flake.lock` or scaffold's default
table is used, and the result is inserted with `role = "dependency"`. An unresolvable required
dep fails before anything is written, naming the dep and two fixes. An unresolvable optional
dep is skipped with a `note:`. A dep on a name an older basecamp bundled (`main_ui`,
`counter`, …) names the declaring `metadata.json` as the fix.

When auto-discovery finds a flake exposing `.#lgx-portable` but not `.#lgx`, capture fails
with a targeted error naming the missing `.#lgx` and suggesting `--flake <ref>#lgx-portable`.
It never falls back silently.

### Preconditions
BCMOD-001, and a module whose `metadata.json` declares a dependency. For step 3, a project
whose only flake exposes just `packages.<system>.lgx-portable`, with no `[modules]` yet.

### Steps
1. Add `"no_such_module"` to a project source's `dependencies`, remove `[modules]` from
   `scaffold.toml`, then run `"$SCAFFOLD_BIN" basecamp modules; echo "exit=$?"`.
2. Change it to `"main_ui"` and re-run, then revert `metadata.json`.
3. In the portable-only project: `"$SCAFFOLD_BIN" basecamp modules; echo "exit=$?"`

### Success criteria
- Step 1 exits non-zero, names `no_such_module` and both fixes, and leaves `scaffold.toml`
  unchanged. Step 2's error points at the declaring `metadata.json`.
- Step 3 exits non-zero, names `lgx` and suggests `#lgx-portable`.

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
BCMOD-001.

### Steps
1. `"$SCAFFOLD_BIN" basecamp install && "$SCAFFOLD_BIN" basecamp install --print-output`
2. `"$SCAFFOLD_BIN" basecamp doctor; ls "$("$SCAFFOLD_BIN" basecamp paths alice --json | jq -r .modules_dir)"; ls .scaffold/logs/*-install.log`
3. `mkdir -p /tmp/badlgx && head -c 512 /dev/urandom > /tmp/badlgx/broken.lgx`, then
   `"$SCAFFOLD_BIN" basecamp modules --path /tmp/badlgx/broken.lgx` and
   `"$SCAFFOLD_BIN" basecamp install; echo "exit=$?"`. Restore `scaffold.toml`.

### Success criteria
- Step 1 exits 0 both times, and the log file exists. Doctor shows no drift.
- The `ls` lists only names keyed in `[modules]`, and none of `capability_module`,
  `modules_state`, `package_downloader` or `package_manager`.
- Step 3 exits non-zero, and the error names `lgpm install /tmp/badlgx/broken.lgx into
  alice`.

## BCMOD-005 — Build module artefacts by variant, staged as ordered symlinks

### Expected
`basecamp build --variant lgx|lgx-portable|all` (default `all`) builds every `role =
"project"` entry in dependency order without installing, and skips `role = "dependency"`.
Each variant's outputs are symlinked as `<NN>-<module>.lgx` (leaves first) into its own
directory, which is wiped and recreated each time: `lgx` into `.scaffold/basecamp/lgx/`,
`lgx-portable` into `.scaffold/basecamp/portable/`. `--module NAME` builds only that module,
still applies the sibling `--override-input` flags, and leaves only that module's link.
`basecamp build-portable` is an alias for `build --variant lgx-portable`. Neither runs
`lgpm`, nor touches `basecamp.state` or the profiles. An unknown module fails with a `no
project module` error that lists the captured project modules. A flake missing the requested
variant fails with a message that names the flake, says it `does not expose` the variant and
asks for a `packages.<system>.<variant>` output, not a raw nix trace. The CLI accepts only
`lgx`, `lgx-portable` and `all`.

### Preconditions
BCMOD-001 with flakes exposing both variants. For step 4, a temporary `[modules]` entry
(`role = "project"`) whose flake exposes `.#lgx` but not `.#lgx-portable`.

### Steps
1. `"$SCAFFOLD_BIN" basecamp build && ls -l .scaffold/basecamp/lgx/ .scaffold/basecamp/portable/`
2. `"$SCAFFOLD_BIN" basecamp build --variant lgx 2>&1 | grep -c 'lgx-portable'; "$SCAFFOLD_BIN" basecamp build-portable --module <ui-module-with-sibling-input> && ls .scaffold/basecamp/portable/`
3. `"$SCAFFOLD_BIN" basecamp build --module nope; echo "exit=$?"; "$SCAFFOLD_BIN" basecamp build --variant bogus; echo "exit=$?"`
4. Add the lgx-only entry: `"$SCAFFOLD_BIN" basecamp build --variant lgx-portable --module <lgx-only-module>; echo "exit=$?"`. Remove the entry.
5. Remove one module from `[modules]`: `"$SCAFFOLD_BIN" basecamp build-portable && ls .scaffold/basecamp/portable/`

### Success criteria
- Step 1 exits 0 and fills both directories with `01-…`, `02-…` symlinks into the nix store,
  in dependency order.
- Step 2: the `lgx` build has no `#lgx-portable` line (`0`); the `--module` build has no
  `forbidden in pure evaluation mode` error and leaves only `01-<module>.lgx`.
- Step 3: the first fails naming the captured modules; the second is refused by argument
  parsing, listing the possible values, before any build.
- Step 4 exits non-zero, saying the flake does not expose `lgx-portable`.
- Step 5 leaves no stale symlink for the removed module.

### Notes
The Rust API takes free-form variant strings; its rejection of empty, unknown or path-like
variants and its de-duplication cannot be reached from the CLI and are covered by the unit
test `normalize_build_variants_rejects_empty_unknown_and_dedupes`.

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
before any nix call. A module captured from a `.lgx` path is rejected by `run`, with guidance
to capture a flake source instead; `nix run` is not attempted.

### Preconditions
BCMOD-001. The module flake exposes `apps.<system>.default`. `<attr>` is any dev shell the
flake exposes (`default` works). For step 5, a module captured with `basecamp modules --path
/abs/prebuilt.lgx`.

### Steps
1. `"$SCAFFOLD_BIN" basecamp run <module>`
2. `"$SCAFFOLD_BIN" basecamp develop <module>`, then run `echo $SCAFFOLD_PROJECT_ROOT; exit`
   inside the shell. Repeat with `--dev-shell <attr>`.
3. `"$SCAFFOLD_BIN" basecamp develop nope; echo "exit=$?"`
4. Add `[modules.remote]` with `flake` set to a captured dependency's `github:` ref minus its
   `#lgx`, `role = "project"` and `standalone_app = "default"`. Run
   `"$SCAFFOLD_BIN" basecamp run remote` and stop it after the first line. Remove the entry.
5. `"$SCAFFOLD_BIN" basecamp run <path-captured-module>; echo "exit=$?"`
6. `"$SCAFFOLD_BIN" basecamp modules && grep -c 'standalone_app' scaffold.toml`

### Success criteria
- Step 1: the app starts without a pure-eval error.
- Step 2: both shells open and print the project root; with `--dev-shell` the first line
  ends in `#<attr>)`.
- Step 3 fails fast and lists the captured modules.
- Step 4's first line shows `standalone host: github:…#default`.
- Step 5 exits non-zero with the guidance text, and no `nix run` is echoed.
- Step 6 prints `0` when no entry was hand-given a `standalone_app`.
