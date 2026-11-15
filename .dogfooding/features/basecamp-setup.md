---
feature: basecamp-setup
prefix: BCSETUP
surface: cli
scenarios:
  - id: BCSETUP-001
    title: basecamp setup builds the pin set and seeds profiles
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 B1
  - id: BCSETUP-002
    title: Re-running setup skips the build, and basecamp doctor confirms it
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 B1
    depends_on: [BCSETUP-001]
  - id: BCSETUP-004
    title: Pin changes - retired defaults move forward, unsynced edits are refused
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B1
    depends_on: [BCSETUP-001]
  - id: BCSETUP-006
    title: Without Nix or before setup - help, docs and one-line hints
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B1
  - id: BCSETUP-007
    title: lgpm pin set and lgx contract hold without an app build
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B7
retired: [BCSETUP-003, BCSETUP-005, BCSETUP-008]
---

# Basecamp setup

`basecamp setup` is opt-in. It builds the pinned basecamp app and `lgpm` with Nix
(`[repos.basecamp]` and `[repos.lgpm]`, both `build = "nix-flake"`) and seeds the `alice`
and `bob` profiles under `.scaffold/basecamp/profiles/`. Scaffold supports basecamp 0.3.0
only, and the basecamp and lgpm pins move as one set. Related:
[Basecamp modules](basecamp-modules.md), [Basecamp launch](basecamp-launch.md).

What a host can run: BCSETUP-006 and BCLAUNCH-005 need neither Nix nor a basecamp build.
BCSETUP-007 needs Nix with GitHub access but no basecamp app. Everything else in the three
basecamp features needs BCSETUP-001; when it cannot obtain the app ("Basecamp environment"
in [product.md](../product.md) lists the routes and how to tell), report it and its
dependants as blocked.

## BCSETUP-001 — basecamp setup builds the pin set and seeds profiles

### Expected
In a module project, the first `basecamp setup` clones basecamp into
`<cache>/repos/basecamp/<pin>` (`<pin>-src-<hash>` for a fork). It builds basecamp and `lgpm`
via Nix, logging to `.scaffold/logs/<ts>-setup-*.log`, seeds
`.scaffold/basecamp/profiles/{alice,bob}/` and reports completion. A per-platform
`[repos.basecamp.attr]` map selects the host's attr and survives re-serialization. Nothing
lands on `PATH`, and nothing is written to the global `~/.local/share/Logos` or
`~/Library/Application Support/Logos`. Basecamp's bundled modules stay inside its nix output
(`modules/`: `capability_module`, `modules_state`, `package_downloader`, `package_manager`;
`plugins/`: `package_manager_ui` and the manifest-less `main_ui`). They are never copied into
a profile, so a freshly seeded profile's `modules_dir` is empty.

The entry point scaffold execs for the dev `#app` attr is `bin/LogosBasecamp`, a shell
launcher that sets the Qt paths and then execs the hidden `bin/.LogosBasecamp`. There is no
`bin/logos-basecamp`. The launcher reaches `Module loaded: package_downloader`; the hidden
binary run directly does not.

### Preconditions
Nix with flakes and the GitHub probe from product.md printing `200`. About 5 GB of free RAM,
or the CI-build route from product.md (record which). A module project (`$MODULE_PROJECT`)
built with `logos-module-builder` 0.3.0; run `"$SCAFFOLD_BIN" init` there if it has no
`scaffold.toml`.

### Steps
1. `cd "$MODULE_PROJECT" && "$SCAFFOLD_BIN" basecamp setup && ls .scaffold/basecamp/profiles`
2. `ls -A "$("$SCAFFOLD_BIN" basecamp paths alice --json | jq -r .modules_dir)"`
3. `APP=$(sed -n 's/^basecamp_bin=//p' .scaffold/state/basecamp.state | sed 's#/bin/LogosBasecamp$##'); ls "$APP/modules" "$APP/plugins"; ls -a "$APP/bin"; head -c 2 "$APP/bin/LogosBasecamp"`
4. Start each entry point for a few seconds and compare the logs:
   `env -i HOME=/tmp PATH=/usr/bin:/bin QT_QPA_PLATFORM=offscreen "$APP"/bin/<entry>`, once with
   `LogosBasecamp` and once with `.LogosBasecamp`.

### Success criteria
- Exit 0, `alice` and `bob` exist, and no new files appear under the global Logos data dirs.
- Step 2 prints nothing (before any `basecamp install`). Step 3 lists the bundled modules and
  plugins named above, and `bin/` holds `LogosBasecamp`, `.LogosBasecamp`, `logos_host` and
  `ui-host` but no `logos-basecamp`; `head` prints `#!`.
- Step 4: only the launcher logs `Module loaded: package_downloader`.

### Notes
Steps 3 and 4 assume the dev `#app` attr. On another attr (a macOS `.app` or an AppImage)
skip them and record why.

## BCSETUP-002 — Re-running setup skips the build, and basecamp doctor confirms it

### Expected
With a commit-SHA `[repos.basecamp].pin` (the default) whose result link already exists, a
second `basecamp setup` prints `basecamp (.#<attr>) already built at /nix/store/…; skipping
nix build` and exits 0 in seconds without re-evaluating the basecamp flake. A tag or branch
pin always rebuilds. `lgpm` is re-substituted quickly, and profiles are seeded again
(`seeded profiles: alice, bob`), which is harmless.

Right after setup (before any modules are captured), `basecamp doctor` shows four PASS rows:
`basecamp binary`, `lgpm binary`, `basecamp profile alice` and `basecamp profile bob`, plus a
`basecamp pin set` row that passes on default pins and warns only when exactly one of the
pair is at the default. A project pinned away from both defaults (to revs that are not
retired defaults either) also passes, with the note `both pinned away from the scaffold
defaults (scaffold supports basecamp 0.3.0)`. `--json` carries the same checks.

### Preconditions
BCSETUP-001. Back up `scaffold.toml` for step 3.

### Steps
1. `time "$SCAFFOLD_BIN" basecamp setup`
2. `"$SCAFFOLD_BIN" basecamp doctor; "$SCAFFOLD_BIN" basecamp doctor --json | jq -e .checks > /dev/null`
3. Set `[repos.basecamp].pin` and `[repos.lgpm].pin` to two other commit SHAs (neither the
   defaults nor the retired 0.2.3 pair), run
   `"$SCAFFOLD_BIN" basecamp doctor --json | jq '.checks[] | select(.name == "basecamp pin set")'`,
   and restore `scaffold.toml`.

### Success criteria
- Step 1 prints the `already built … skipping nix build` line, has no `building basecamp`
  step, and takes seconds.
- Step 2 shows at least the four PASS rows and a passing `basecamp pin set`; the JSON parses.
- Step 3's row has `"status": "pass"` and a `detail` containing `both pinned away from the
  scaffold defaults`. A `basecamp build pin` warning next to it is expected.

## BCSETUP-004 — Pin changes - retired defaults move forward, unsynced edits are refused

### Expected
With the previous default set in `scaffold.toml` (basecamp 0.2.3:
`[repos.basecamp].pin = "aa237766baf61404e12da86b7303cb41065464c9"`, `[repos.lgpm].pin =
"202af6fa0f0f4493bc59c8a609dff9326f78a18d"`), `basecamp doctor` warns `basecamp 0.2.3, a
retired scaffold default; scaffold supports basecamp 0.3.0 only`. `basecamp setup` then
prints one rewrite line per pin, rewrites both and leaves doctor green. It also upgrades the
pair when one half is retired and the other is empty or at the current default. Only a
retired basecamp pin next to a hand-picked lgpm pin is left alone, with doctor telling you
to set both by hand.

After a pin changes without a re-run of `setup`, `basecamp install` and `basecamp launch`
refuse with `basecamp was set up for pin <old> but scaffold.toml now pins <new>; run:
logos-scaffold basecamp setup` (`lgpm was set up for pin …` for the lgpm pin), and doctor
shows a `basecamp build pin` / `lgpm build pin` warning. `.scaffold/state/basecamp.state`
records both pins (`pin=` and `lgpm_pin=`). A state file written before `lgpm_pin=` existed
has nothing to compare against, so it passes until the next `setup` writes the line.

### Preconditions
BCSETUP-001. Back up `scaffold.toml` and `.scaffold/state/basecamp.state`; restore
`scaffold.toml` between steps.

### Steps
1. Change `[repos.lgpm].pin` to another rev: `"$SCAFFOLD_BIN" basecamp install; echo "exit=$?"; "$SCAFFOLD_BIN" basecamp doctor | grep 'build pin'`
2. `sed -i.bak '/^lgpm_pin=/d' .scaffold/state/basecamp.state`, change `[repos.lgpm].pin`
   again, and run `"$SCAFFOLD_BIN" basecamp doctor | grep 'build pin'`.
3. Set only the basecamp pin to the retired value and the lgpm pin to any other rev, then
   `"$SCAFFOLD_BIN" basecamp setup; "$SCAFFOLD_BIN" basecamp doctor; grep -A3 'repos.basecamp\|repos.lgpm' scaffold.toml`
4. Set both retired pins: `"$SCAFFOLD_BIN" basecamp doctor`, then
   `"$SCAFFOLD_BIN" basecamp setup && grep -A3 'repos.basecamp\|repos.lgpm' scaffold.toml && grep '^lgpm_pin=' .scaffold/state/basecamp.state`

### Success criteria
- Step 1: `install` exits non-zero with the `lgpm was set up for pin` message, and doctor
  shows `lgpm build pin`. Step 2 prints no `build pin` row.
- Step 3 changes neither pin, and the remediation names both.
- Step 4: doctor warns about the retired default; `setup` prints two `-> ` rewrite lines,
  `scaffold.toml` holds the current default pins, and the state file has an `lgpm_pin=` line
  again.

## BCSETUP-006 — Without Nix or before setup - help, docs and one-line hints

### Expected
`basecamp --help` lists `setup`, `modules`, `install`, `launch`, `develop`, `build`,
`build-portable`, `run`, `doctor`, `paths` and `docs`. `basecamp docs` prints the embedded
`docs/basecamp-module-requirements.md`, covering per-profile `env_file`, `runtime_dir`,
`log_file`, custom profile names and per-platform `[repos.basecamp.attr]`. A basecamp command
that needs Nix, run without `nix` on PATH, fails before any Nix call with `this command
requires Nix (with flakes enabled) on PATH`, not an `os error 2`. A command that needs the
built app, run before `basecamp setup`, fails with `basecamp not set up yet; run:
logos-scaffold basecamp setup`. Neither prints a raw Nix or `lgpm` stack trace. (Outside a
project: CLI-003.)

### Preconditions
A default-template project where `basecamp setup` never ran. Nix is not needed. For step 2,
a `PATH` that has no `nix` (check with `env PATH=/usr/bin:/bin which nix`).

### Steps
1. `"$SCAFFOLD_BIN" basecamp --help; "$SCAFFOLD_BIN" basecamp docs | diff - "$REPO_ROOT/docs/basecamp-module-requirements.md"`
2. `env PATH=/usr/bin:/bin "$SCAFFOLD_BIN" basecamp setup; echo "exit=$?"`
3. `"$SCAFFOLD_BIN" basecamp launch alice; echo "exit=$?"`

### Success criteria
- All 11 subcommands are listed, and the docs match the repo copy.
- Step 2 exits non-zero with the requires-Nix hint, step 3 with the not-set-up hint.

## BCSETUP-007 — lgpm pin set and lgx contract hold without an app build

### Expected
Without building basecamp, `[repos.lgpm]` builds `#cli` and `#cli-portable`. The project's
`.lgx` has a non-empty `hashes.root` and a `<host>-dev` variant. A real install with
scaffold's argument shape (`lgpm --modules-dir … --ui-plugins-dir … install --file <lgx>`)
prints `Installed to:` and exits 0 (`Package is unsigned` is only a warning). The failures
that `basecamp install` turns into hints carry these exact strings:

| Package and installer | lgpm error | `basecamp install` hint |
|---|---|---|
| `hashes` stripped, current lgpm | `Package validation failed: Missing content hashes in manifest` | rebuild with current module tooling |
| dev `.lgx`, portable lgpm | `Package does not contain variant for platform: …` | variant mismatch |
| 0.3.x UI package with an icon, retired lgpm `202af6fa0f0f4493bc59c8a609dff9326f78a18d` | `Package validation failed: Forbidden root entry: assets` | pin set (`… built by newer module tooling than the installer …`) |
| `"name": ""`, current lgpm | `Package validation failed: Manifest: 'name' field is empty` | none: raw lgpm stderr |

### Preconditions
Nix with GitHub access. A core module project (`$MODULE_PROJECT`) and a UI module with an
icon (`$UI_PROJECT`): `templates/minimal-module` and `templates/ui-qml` of
`logos-module-builder` 0.3.0, set up as "Basecamp environment" in product.md describes.

### Steps
1. Build both lgpm attrs and the `.lgx`, inspect it, and install it for real:
   ```bash
   cd "$MODULE_PROJECT"
   LGPM=$(grep -A2 '^\[repos.lgpm\]' scaffold.toml | sed -n 's/^pin = "\(.*\)"/\1/p')
   nix build "github:logos-co/logos-package-manager/$LGPM#cli"          --out-link /tmp/lgpm-dev
   nix build "github:logos-co/logos-package-manager/$LGPM#cli-portable" --out-link /tmp/lgpm-portable
   nix build .#lgx --out-link /tmp/mod-lgx
   X=$(mktemp -d) && tar xzf /tmp/mod-lgx/*.lgx -C "$X" && jq '.hashes.root, .name' "$X/manifest.json" && ls "$X/variants"
   inst() { mkdir -p /tmp/p/modules /tmp/p/plugins; "$1"/bin/lgpm --modules-dir /tmp/p/modules --ui-plugins-dir /tmp/p/plugins install --file "$2"; echo "exit=$?"; }
   inst /tmp/lgpm-dev /tmp/mod-lgx/*.lgx
   ```
2. The negative installs. Repack with `tar czf out.lgx manifest.json variants` from inside
   `$X` (a `./`-prefixed archive fails for an unrelated reason):
   ```bash
   inst /tmp/lgpm-portable /tmp/mod-lgx/*.lgx
   (cd "$X" && cp manifest.json /tmp/manifest.orig && jq 'del(.hashes)' /tmp/manifest.orig > manifest.json && tar czf /tmp/nohash.lgx manifest.json variants); inst /tmp/lgpm-dev /tmp/nohash.lgx
   (cd "$X" && jq '.name=""' /tmp/manifest.orig > manifest.json && tar czf /tmp/noname.lgx manifest.json variants); inst /tmp/lgpm-dev /tmp/noname.lgx
   ```
3. `nix build github:logos-co/logos-package-manager/202af6fa0f0f4493bc59c8a609dff9326f78a18d#cli --out-link /tmp/lgpm-old`,
   build the UI module (`cd "$UI_PROJECT" && nix build .#lgx --out-link /tmp/ui-lgx`), then
   `inst /tmp/lgpm-old /tmp/ui-lgx/*.lgx; inst /tmp/lgpm-dev /tmp/ui-lgx/*.lgx`

### Success criteria
- Step 1: both lgpm attrs build, `hashes.root` and `name` are non-empty, a `<host>-dev`
  variant exists, and the install prints `Installed to:` with `exit=0`.
- Step 2 fails three times, in order, with the variant, content-hash and empty-name strings
  from the table.
- Step 3: the retired lgpm fails with `Forbidden root entry: assets`; the current one
  installs the same package.

### Notes
On x86_64 Linux the variant error reads `platform: linux-x86_64 (package provides:
linux-amd64-dev)`; lgpm treats the two spellings as aliases, so that is still the stack
mismatch. An `lgpm` pin that does not build at all means the default pin set is wrong: stop
and fix `DEFAULT_LGPM_PIN` first. A bare `SIGKILL` while evaluating is out of memory, not a
broken pin. The mapping from these strings to `basecamp install` hints needs a set-up project
on a mismatched pin pair, which `setup` prevents by moving a retired lgpm pin forward; the
unit tests `lgpm_install_hint_explains_a_package_newer_than_the_installer` and
`lgpm_install_hint_does_not_blame_hashes_for_other_validation_failures` cover it.
