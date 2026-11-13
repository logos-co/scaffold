---
feature: basecamp-setup
prefix: BCSETUP
surface: cli
scenarios:
  - id: BCSETUP-001
    title: basecamp setup builds the pin set and seeds profiles
    priority: P1
    provenance: documented
    source: DOGFOODING.md B1
  - id: BCSETUP-002
    title: Re-running setup on an unchanged pin skips the build
    priority: P1
    provenance: documented
    source: DOGFOODING.md B1
    depends_on: [BCSETUP-001]
  - id: BCSETUP-003
    title: basecamp doctor confirms setup
    priority: P1
    provenance: documented
    source: DOGFOODING.md B1
    depends_on: [BCSETUP-001]
  - id: BCSETUP-004
    title: Retired default pins are moved forward
    priority: P2
    provenance: documented
    source: DOGFOODING.md B1
    depends_on: [BCSETUP-001]
  - id: BCSETUP-005
    title: Pin edits without setup are refused
    priority: P2
    provenance: documented
    source: DOGFOODING.md B1
    depends_on: [BCSETUP-001]
  - id: BCSETUP-006
    title: Module contract docs and help are available offline
    priority: P2
    provenance: documented
    source: DOGFOODING.md B1
  - id: BCSETUP-007
    title: lgpm pin set and lgx contract hold without an app build
    priority: P2
    provenance: documented
    source: DOGFOODING.md B7
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
`~/Library/Application Support/Logos`.

### Preconditions
Nix with flakes, plus a working GitHub probe: `curl -sS -o /dev/null -w '%{http_code}'
https://api.github.com/repos/NixOS/nixpkgs/commits/HEAD` → `200`. About 5 GB of free RAM. A
module project (`$MODULE_PROJECT`) built with `logos-module-builder` 0.3.0. Run
`"$SCAFFOLD_BIN" init` there if it has no `scaffold.toml`.

### Steps
1. `cd "$MODULE_PROJECT" && "$SCAFFOLD_BIN" basecamp setup`
2. `ls .scaffold/basecamp/profiles`

### Success criteria
- Exit 0, and `alice` and `bob` exist.
- No new files appear under the global Logos data dirs.

### Notes
On a host with too little RAM, realise the CI-built `#app` store path and link it as
described in `DOGFOODING.md` B1, then record that mode. If Nix cannot reach GitHub, run
BCSETUP-006 and BCSETUP-007 only and report the blocker.

## BCSETUP-002 — Re-running setup on an unchanged pin skips the build

### Expected
A second `basecamp setup` prints `basecamp (.#<attr>) already built at /nix/store/…;
skipping nix build` and exits 0 in seconds without re-evaluating the basecamp flake. `lgpm`
is re-substituted quickly. Profiles are not reseeded.

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
one of the pair is at the default. `--json` carries the same checks as parseable JSON.

### Preconditions
BCSETUP-001.

### Steps
1. `"$SCAFFOLD_BIN" basecamp doctor`
2. `"$SCAFFOLD_BIN" basecamp doctor --json | jq .`

### Success criteria
- At least 4 PASS rows, the `basecamp pin set` row passes, and the JSON parses.

## BCSETUP-004 — Retired default pins are moved forward

### Expected
With the previous default set in `scaffold.toml` (basecamp 0.2.3:
`[repos.basecamp].pin = "aa237766baf61404e12da86b7303cb41065464c9"`, `[repos.lgpm].pin =
"202af6fa0f0f4493bc59c8a609dff9326f78a18d"`), `basecamp doctor` warns `basecamp 0.2.3, a
retired scaffold default; scaffold supports basecamp 0.3.0 only`. `basecamp setup` then
prints one rewrite line per pin, rewrites both and leaves doctor green. If only one half is
a retired default, nothing moves, and doctor tells you to set both by hand.

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
with an `lgpm build pin` doctor row.

### Preconditions
BCSETUP-001, with `scaffold.toml` backed up.

### Steps
1. Change `[repos.lgpm].pin` to another rev, then run `"$SCAFFOLD_BIN" basecamp install; echo "exit=$?"`.
2. `"$SCAFFOLD_BIN" basecamp doctor | grep 'build pin'`, then restore `scaffold.toml`.

### Success criteria
- `install` exits non-zero with the `lgpm was set up for pin` message, and doctor shows
  `lgpm build pin`.

## BCSETUP-006 — Module contract docs and help are available offline

### Expected
`basecamp --help` lists `setup`, `modules`, `install`, `launch`, `develop`, `build`,
`build-portable`, `run`, `doctor`, `paths` and `docs`. `basecamp docs` prints the embedded
`docs/basecamp-module-requirements.md`, covering per-profile `env_file`, `runtime_dir`,
`log_file`, custom profile names and per-platform `[repos.basecamp.attr]`. Outside a project,
basecamp commands fail with the "not a logos-scaffold project" message.

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

### Preconditions
Nix with GitHub access and `$MODULE_PROJECT`. No basecamp build needed.

### Steps
1. Follow the commands in `DOGFOODING.md` B7, steps 1–4.

### Success criteria
- Both lgpm attrs build, the real install exits 0, and both negative installs fail with the
  exact strings above.

### Notes
Repack the negative-test packages with `tar czf out.lgx manifest.json variants`. A
`./`-prefixed archive fails for an unrelated reason. A bare `SIGKILL` while evaluating is out
of memory, not a broken pin.
