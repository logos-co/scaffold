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
    title: Re-running setup on an unchanged pin skips the build
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 B1
    depends_on: [BCSETUP-001]
  - id: BCSETUP-003
    title: basecamp doctor confirms setup
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 B1
    depends_on: [BCSETUP-001]
  - id: BCSETUP-004
    title: Retired default pins are moved forward
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B1
    depends_on: [BCSETUP-001]
  - id: BCSETUP-005
    title: Pin edits without setup are refused
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B1
    depends_on: [BCSETUP-001]
  - id: BCSETUP-006
    title: Module contract docs and help are available offline
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B1
  - id: BCSETUP-007
    title: lgpm pin set and lgx contract hold without an app build
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B7
  - id: BCSETUP-008
    title: Without Nix or before setup, basecamp commands give a one-line hint
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B1
retired: []
---

# Basecamp setup

`basecamp setup` is opt-in. It builds the pinned basecamp app and `lgpm` with Nix
(`[repos.basecamp]` and `[repos.lgpm]`, both `build = "nix-flake"`) and seeds the `alice`
and `bob` profiles under `.scaffold/basecamp/profiles/`. Scaffold supports basecamp 0.3.0
only, and the basecamp and lgpm pins move as one set. Related:
[Basecamp modules](basecamp-modules.md), [Basecamp launch](basecamp-launch.md).

## BCSETUP-001 — basecamp setup builds the pin set and seeds profiles

### Expected
In a module project, the first `basecamp setup` clones basecamp into
`<cache>/repos/basecamp/<pin>`, or into `<pin>-src-<hash>` for a fork. It builds basecamp
and `lgpm` via Nix, logging to `.scaffold/logs/<ts>-setup-*.log`, seeds
`.scaffold/basecamp/profiles/{alice,bob}/` and reports completion. A per-platform
`[repos.basecamp.attr]` map selects the host's attr and survives re-serialization. Nothing
lands on `PATH`, and nothing is written to the global `~/.local/share/Logos` or
`~/Library/Application Support/Logos`. Basecamp's bundled modules stay inside its nix output
(`modules/`: `capability_module`, `modules_state`, `package_downloader`, `package_manager`;
`plugins/`: `package_manager_ui` and the manifest-less `main_ui`). They are never copied into
a profile, so a freshly seeded profile's `modules_dir` is empty and later holds only the
project's own modules.

### Preconditions
Nix with flakes, plus a working GitHub probe: `curl -sS -o /dev/null -w '%{http_code}'
https://api.github.com/repos/NixOS/nixpkgs/commits/HEAD` → `200`. About 5 GB of free RAM. A
module project (`$MODULE_PROJECT`) built with `logos-module-builder` 0.3.0. Run
`"$SCAFFOLD_BIN" init` there if it has no `scaffold.toml`.

### Steps
1. `cd "$MODULE_PROJECT" && "$SCAFFOLD_BIN" basecamp setup`
2. `ls .scaffold/basecamp/profiles`
3. `ls -A "$("$SCAFFOLD_BIN" basecamp paths alice --json | jq -r .modules_dir)"`
4. `APP=$(sed -n 's/^basecamp_bin=//p' .scaffold/state/basecamp.state | sed 's#/bin/LogosBasecamp$##')`,
   then `ls "$APP/modules" "$APP/plugins"`.

### Success criteria
- Exit 0, and `alice` and `bob` exist.
- No new files appear under the global Logos data dirs.
- Step 3 prints nothing. Step 4 lists the bundled modules and plugins named above.

### Notes
On a host with too little RAM, realise the CI-built `#app` store path and link it as
described under "Basecamp environment" in [product.md](../product.md), then record that mode. BCSETUP-007 is that low-memory
fallback (it still needs Nix with GitHub access). If Nix cannot reach GitHub at all, run
BCSETUP-006 and BCSETUP-008 only and report the blocker. Step 4 assumes the dev `#app` attr,
whose entry point is `bin/LogosBasecamp`. On another attr (a macOS `.app` or an AppImage)
skip it and record why. Step 3 holds only in a project where `basecamp install` has not run.

## BCSETUP-002 — Re-running setup on an unchanged pin skips the build

### Expected
With a commit-SHA `[repos.basecamp].pin` (the default) whose result link already exists, a
second `basecamp setup` prints `basecamp (.#<attr>) already built at /nix/store/…; skipping
nix build` and exits 0 in seconds without re-evaluating the basecamp flake. A tag or branch
pin always rebuilds. `lgpm` is re-substituted quickly. Profiles are seeded again
(`seeded profiles: alice, bob`), which is harmless.

### Preconditions
BCSETUP-001.

### Steps
1. `time "$SCAFFOLD_BIN" basecamp setup`

### Success criteria
- The `already built … skipping nix build` line appears, with no `building basecamp` step,
  and the run takes seconds.

## BCSETUP-003 — basecamp doctor confirms setup

### Expected
Right after setup (before any modules are captured), `basecamp doctor` shows four PASS rows:
`basecamp binary`, `lgpm binary`, `basecamp profile alice` and `basecamp profile bob`. It
also shows a `basecamp pin set` row that passes on default pins and warns only when exactly
one of the pair is at the default. A project pinned away from both defaults (to revs that are
not retired defaults either) also passes, with the note `both pinned away from the scaffold
defaults (scaffold supports basecamp 0.3.0)`. `--json` carries the same checks as parseable
JSON.

### Preconditions
BCSETUP-001. For step 3, `scaffold.toml` backed up.

### Steps
1. `"$SCAFFOLD_BIN" basecamp doctor`
2. `"$SCAFFOLD_BIN" basecamp doctor --json | jq .`
3. Set `[repos.basecamp].pin` and `[repos.lgpm].pin` to two other revs (any commit SHAs that
   are neither the defaults nor the retired 0.2.3 pair), then run
   `"$SCAFFOLD_BIN" basecamp doctor --json | jq '.checks[] | select(.name == "basecamp pin set")'`.
   Restore `scaffold.toml`.

### Success criteria
- At least 4 PASS rows, the `basecamp pin set` row passes, and the JSON parses.
- In step 3 the row has `"status": "pass"` and its `detail` contains `both pinned away from
  the scaffold defaults`. A `basecamp build pin` warning next to it is expected, since setup
  has not run for the new pins.

## BCSETUP-004 — Retired default pins are moved forward

### Expected
With the previous default set in `scaffold.toml` (basecamp 0.2.3:
`[repos.basecamp].pin = "aa237766baf61404e12da86b7303cb41065464c9"`, `[repos.lgpm].pin =
"202af6fa0f0f4493bc59c8a609dff9326f78a18d"`), `basecamp doctor` warns `basecamp 0.2.3, a
retired scaffold default; scaffold supports basecamp 0.3.0 only`. `basecamp setup` then
prints one rewrite line per pin, rewrites both and leaves doctor green. It also upgrades the
pair when one half is retired and the other is empty or at the current default. Only a
retired basecamp pin next to a hand-picked lgpm pin is left alone, with doctor telling you
to set both by hand.

### Preconditions
BCSETUP-001, with `scaffold.toml` backed up.

### Steps
1. Set both retired pins, then run `"$SCAFFOLD_BIN" basecamp doctor`.
2. `"$SCAFFOLD_BIN" basecamp setup && grep -A3 'repos.basecamp\|repos.lgpm' scaffold.toml`
3. Set only the basecamp pin to the retired value and the lgpm pin to any other rev, then run
   `basecamp setup` and `basecamp doctor`.

### Success criteria
- Step 1 warns. Step 2 prints two `-> ` rewrite lines and the current default pins.
- In step 3 neither pin changes, and the remediation names both.

## BCSETUP-005 — Pin edits without setup are refused

### Expected
After `[repos.basecamp].pin` changes without a re-run of `setup`, `basecamp install` and
`basecamp launch` refuse with `basecamp was set up for pin <old> but scaffold.toml now pins
<new>; run: logos-scaffold basecamp setup`, and doctor shows a `basecamp build pin` warning.
Editing only `[repos.lgpm].pin` trips the same guard, worded `lgpm was set up for pin …`,
with an `lgpm build pin` doctor row. `.scaffold/state/basecamp.state` records both pins
(`pin=` and `lgpm_pin=`). A state file written before `lgpm_pin=` existed has nothing to
compare the lgpm pin against, so it passes until the next `setup` writes the line.

### Preconditions
BCSETUP-001, with `scaffold.toml` and `.scaffold/state/basecamp.state` backed up.

### Steps
1. Change `[repos.lgpm].pin` to another rev, then run `"$SCAFFOLD_BIN" basecamp install; echo "exit=$?"`.
2. `"$SCAFFOLD_BIN" basecamp doctor | grep 'build pin'`, then restore `scaffold.toml`.
3. `sed -i.bak '/^lgpm_pin=/d' .scaffold/state/basecamp.state`, change `[repos.lgpm].pin`
   again, and run `"$SCAFFOLD_BIN" basecamp doctor | grep 'build pin'`.
4. Restore `scaffold.toml`, run `"$SCAFFOLD_BIN" basecamp setup`, then
   `grep '^lgpm_pin=' .scaffold/state/basecamp.state`.

### Success criteria
- `install` exits non-zero with the `lgpm was set up for pin` message, and doctor shows
  `lgpm build pin`.
- Step 3 prints no `build pin` row. Step 4 prints an `lgpm_pin=` line with the default pin.

## BCSETUP-006 — Module contract docs and help are available offline

### Expected
`basecamp --help` lists `setup`, `modules`, `install`, `launch`, `develop`, `build`,
`build-portable`, `run`, `doctor`, `paths` and `docs`. `basecamp docs` prints the embedded
`docs/basecamp-module-requirements.md`, covering per-profile `env_file`, `runtime_dir`,
`log_file`, custom profile names and per-platform `[repos.basecamp.attr]`. Outside a project,
basecamp commands fail with `This command must be run inside a logos-scaffold project`.

### Preconditions
None. Nix is not needed.

### Steps
1. `"$SCAFFOLD_BIN" basecamp --help`
2. `"$SCAFFOLD_BIN" basecamp docs | diff - "$REPO_ROOT/docs/basecamp-module-requirements.md"`
3. `cd "$(mktemp -d)" && "$SCAFFOLD_BIN" basecamp doctor; echo "exit=$?"`

### Success criteria
- All 11 subcommands are listed, the docs match the repo copy, and step 3 fails with the
  project-context message.

## BCSETUP-007 — lgpm pin set and lgx contract hold without an app build

### Expected
Without building basecamp, `[repos.lgpm]` builds `#cli` and `#cli-portable`. The project's
`.lgx` has a non-empty `hashes.root` and a `<host>-dev` variant. A real install with
scaffold's argument shape (`lgpm --modules-dir … --ui-plugins-dir … install --file
<lgx>`) prints `Installed to:` and exits 0 (`Package is unsigned` is only a warning). A
hash-stripped package fails with `Package validation failed: Missing content hashes in
manifest`. A dev `.lgx` under the portable lgpm fails with `Package does not contain variant
for platform: …`. Both strings are ones `basecamp install` turns into hints.

The reverse skew, a `logos-module-builder` 0.3.x package with an icon installed by the
retired lgpm `202af6fa0f0f4493bc59c8a609dff9326f78a18d`, fails with `Package validation
failed: Forbidden root entry: assets`. `basecamp install` maps that to the pin-set hint (`…
it was built by newer module tooling than the installer …`). Other `Package validation
failed:` errors, such as `Manifest: 'name' field is empty`, get no hint: the raw lgpm stderr
is shown, never the content-hash rebuild hint.

The entry point scaffold execs for the dev `#app` attr is `bin/LogosBasecamp`. It is a shell
launcher that sets the Qt paths and then execs the hidden `bin/.LogosBasecamp`. There is no
`bin/logos-basecamp`. The launcher reaches `Module loaded: package_downloader`, and the hidden
binary run directly does not.

### Preconditions
Nix with GitHub access and `$MODULE_PROJECT`. No basecamp build needed. Step 4 needs a UI
module with an icon. Step 5 needs any basecamp `#app` output on hand (`$APP`).

### Steps
1. Build both lgpm attrs, build and inspect the `.lgx`, install it, then try the two
   negative installs:
   ```bash
   cd "$MODULE_PROJECT"
   LGPM=$(grep -A2 '^\[repos.lgpm\]' scaffold.toml | sed -n 's/^pin = "\(.*\)"/\1/p')
   nix build "github:logos-co/logos-package-manager/$LGPM#cli"          --out-link /tmp/lgpm-dev
   nix build "github:logos-co/logos-package-manager/$LGPM#cli-portable" --out-link /tmp/lgpm-portable
   nix build .#lgx --out-link /tmp/mod-lgx
   X=$(mktemp -d) && tar xzf /tmp/mod-lgx/*.lgx -C "$X" && jq '.hashes.root, .name' "$X/manifest.json" && ls "$X/variants"
   mkdir -p /tmp/p/modules /tmp/p/plugins
   /tmp/lgpm-dev/bin/lgpm --modules-dir /tmp/p/modules --ui-plugins-dir /tmp/p/plugins \
     install --file /tmp/mod-lgx/*.lgx
   # (a) delete `hashes` from "$X/manifest.json", repack as the Notes say, install with lgpm-dev
   # (b) install the unmodified dev .lgx with the portable lgpm:
   /tmp/lgpm-portable/bin/lgpm --modules-dir /tmp/p/modules --ui-plugins-dir /tmp/p/plugins \
     install --file /tmp/mod-lgx/*.lgx
   ```
2. Copy the project `.lgx` contents, set `"name": ""` in `manifest.json`, repack and install
   it with the dev lgpm.
3. `nix build github:logos-co/logos-package-manager/202af6fa0f0f4493bc59c8a609dff9326f78a18d#cli --out-link /tmp/lgpm-old`
4. Install a 0.3.x UI module `.lgx` that has an icon with `/tmp/lgpm-old/bin/lgpm`, using the
   same argument shape as the real install in step 1.
5. `ls -a "$APP"/bin && head -c 2 "$APP"/bin/LogosBasecamp`, then start each entry point with
   `env -i HOME=/tmp PATH=/usr/bin:/bin QT_QPA_PLATFORM=offscreen "$APP"/bin/<entry>`, once
   with `LogosBasecamp` and once with `.LogosBasecamp`, and compare their logs.

### Success criteria
- Both lgpm attrs build, the real install exits 0, and both negative installs fail with the
  exact strings above.
- Step 2 fails with a `Package validation failed:` line that is not the content-hash one
  (expected: `Manifest: 'name' field is empty`).
- Step 4 fails with `Package validation failed: Forbidden root entry: assets`.
- Step 5 lists `LogosBasecamp`, `.LogosBasecamp`, `logos_host` and `ui-host`, and no
  `logos-basecamp`. `head` prints `#!`. Only the launcher logs `Module loaded:
  package_downloader`.

### Notes
Repack the negative-test packages with `tar czf out.lgx manifest.json variants`. A
`./`-prefixed archive fails for an unrelated reason. On x86_64 Linux the variant error reads
`platform: linux-x86_64 (package provides: linux-amd64-dev)`; lgpm treats the two spellings
as aliases, so that is still the stack mismatch. A `.lgx` with no `hashes.root` was built by
older module tooling, which is the module's pin to fix. An `lgpm` pin that does not build at
all means the default pin set is wrong: stop and fix `DEFAULT_LGPM_PIN` first. A bare `SIGKILL` while evaluating is out
of memory, not a broken pin. The mapping from these raw strings to `basecamp install` hints
needs a set-up project on a mismatched pin pair. `setup` moves a retired lgpm pin forward, so
that pair is hard to reach from the CLI. Two unit tests in `src/commands/basecamp.rs` cover
it: `lgpm_install_hint_explains_a_package_newer_than_the_installer` and
`lgpm_install_hint_does_not_blame_hashes_for_other_validation_failures`.

## BCSETUP-008 — Without Nix or before setup, basecamp commands give a one-line hint

### Expected
A basecamp command that needs Nix, run without `nix` on PATH, fails before any Nix call with
`this command requires Nix (with flakes enabled) on PATH`, not an `os error 2`. A command that
needs the built app, run before `basecamp setup`, fails with
`basecamp not set up yet; run: logos-scaffold basecamp setup`. Neither prints a raw Nix or
`lgpm` stack trace. This is the useful coverage on a host where Nix is blocked.

### Preconditions
A default-template project. For step 1, a shell whose PATH has no `nix`.

### Steps
1. `env PATH=/usr/bin:/bin "$SCAFFOLD_BIN" basecamp setup; echo "exit=$?"`
2. In a project where `basecamp setup` never ran: `"$SCAFFOLD_BIN" basecamp launch alice; echo "exit=$?"`

### Success criteria
- Step 1 exits non-zero with the requires-Nix hint.
- Step 2 exits non-zero with the not-set-up hint naming `logos-scaffold basecamp setup`.
