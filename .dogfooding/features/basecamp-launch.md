---
feature: basecamp-launch
prefix: BCLAUNCH
surface: cli
scenarios:
  - id: BCLAUNCH-001
    title: Launch one profile with isolated state
    priority: P1
    provenance: documented
    source: DOGFOODING.md B2
    depends_on: [BCMOD-003]
  - id: BCLAUNCH-002
    title: Two profiles interact peer to peer
    priority: P1
    provenance: documented
    source: DOGFOODING.md B3
    depends_on: [BCLAUNCH-001]
  - id: BCLAUNCH-003
    title: Relaunch scrubs profile state
    priority: P1
    provenance: documented
    source: DOGFOODING.md B4
    depends_on: [BCLAUNCH-001]
  - id: BCLAUNCH-004
    title: Relaunch replaces the running instance
    priority: P2
    provenance: documented
    source: DOGFOODING.md B4
    depends_on: [BCLAUNCH-001]
  - id: BCLAUNCH-005
    title: paths resolves profile paths without side effects
    priority: P2
    provenance: documented
    source: DOGFOODING.md B2
    depends_on: [BCSETUP-001]
  - id: BCLAUNCH-006
    title: Custom profiles with env, runtime and log settings
    priority: P2
    provenance: documented
    source: DOGFOODING.md B2
    depends_on: [BCMOD-003]
  - id: BCLAUNCH-007
    title: Unsafe input is refused before any filesystem work
    priority: P2
    provenance: documented
    source: DOGFOODING.md B4
    depends_on: [BCSETUP-001]
  - id: BCLAUNCH-008
    title: Runtime dir is private and outside the project
    priority: P2
    provenance: documented
    source: DOGFOODING.md B2
    depends_on: [BCLAUNCH-001]
retired: []
---

# Basecamp launch

`basecamp launch <profile>` gives clean-slate desktop instances for dogfooding modules. Each
launch kills the profile's previous instance, scrubs its data and cache, reinstalls the
captured modules, and execs basecamp. The environment is profile-scoped: `XDG_*`,
`HOME=<profile>/home`, `LOGOS_PROFILE` and an absolute `LOGOS_USER_DIR`. Two profiles can run
side by side. Related: [Basecamp modules](basecamp-modules.md),
[Basecamp setup](basecamp-setup.md).

## BCLAUNCH-001 — Launch one profile with isolated state

### Expected
`basecamp launch alice` prints a variant-check summary, kills any previous instance, scrubs
`.scaffold/basecamp/profiles/alice/` data and cache, reinstalls the captured modules and
execs basecamp. It records `pid=<pid>` in the profile's `launch.state`. The process
environment has `XDG_{CONFIG,DATA,CACHE}_HOME` under the profile,
`HOME=<profile>/home`, `LOGOS_PROFILE=alice` and an absolute `LOGOS_USER_DIR` under the
profile. The project's UI module appears as a sidebar tile, and opening it logs `Module
loaded: <dep>` for each core dependency.

### Preconditions
BCMOD-003 and a graphical session (Xvfb plus `xdotool` is enough when headless).

### Steps
1. `"$SCAFFOLD_BIN" basecamp launch alice` (keep it running)
2. `PID=$(sed -n 's/^pid=//p' .scaffold/basecamp/profiles/alice/launch.state)`; on Linux
   `tr '\0' '\n' < /proc/$PID/environ | grep -E 'XDG_|LOGOS_|^HOME='`, on macOS `ps eww -p $PID`.

### Success criteria
- The window opens with the project modules.
- Every listed path is inside `.scaffold/basecamp/profiles/alice/`, and `LOGOS_DATA_DIR` is
  not set by scaffold.

## BCLAUNCH-002 — Two profiles interact peer to peer

### Expected
`launch alice` and `launch bob` run at the same time from two terminals. Each uses its own
profile directories, identities and history. There is no collision on Qt remote objects or
any non-module port. A p2p action from one instance is observable in the other.

### Preconditions
BCLAUNCH-001, a module with a p2p surface, and two terminals.

### Steps
1. Terminal 1: `"$SCAFFOLD_BIN" basecamp launch alice`. Terminal 2: `"$SCAFFOLD_BIN" basecamp launch bob`.
2. Trigger the module's p2p action in `alice` and watch `bob`, then the reverse.
3. For a storage-backed module, check that `~/.logos_storage/config.json` keeps its mtime.

### Success criteria
- Both windows show the project modules, and each `LOGOS_USER_DIR` is distinct and absolute.
- The interaction propagates in both directions. The real `~/.logos_storage` is unchanged.

### Notes
Scaffold exports no per-module port overrides (the registry is empty), so a module-level
port collision is the owning module's bug. If the module has no p2p wiring yet, record that
as a gap, not a pass.

## BCLAUNCH-003 — Relaunch scrubs profile state

### Expected
A relaunch removes everything under the profile's `xdg-data` and `xdg-cache` (including
`module_data/` and basecamp's `logs/`) and the legacy `xdg-tmp`. `xdg-config` and `home/`
are kept on purpose. The scrub never reaches outside `.scaffold/basecamp/profiles/<profile>/`.

### Preconditions
BCLAUNCH-001, with the instance closed.

### Steps
1. `mkdir -p .scaffold/basecamp/profiles/alice/xdg-data/scratch && echo m > .scaffold/basecamp/profiles/alice/xdg-data/scratch/marker.txt`
2. `"$SCAFFOLD_BIN" basecamp launch alice`, let it come up, then close it.
3. `test -e .scaffold/basecamp/profiles/alice/xdg-data/scratch/marker.txt && echo SURVIVED || echo SCRUBBED`

### Success criteria
- Step 3 prints `SCRUBBED`.

## BCLAUNCH-004 — Relaunch replaces the running instance

### Expected
A second `launch alice` while the first is running terminates the first and leaves no
orphan. Depending on the stack, the process name is `.LogosBasecamp`, `.LogosBasecamp.` or
`LogosBasecamp`.

### Preconditions
BCLAUNCH-001 running.

### Steps
1. `pgrep -fl LogosBasecamp`
2. `"$SCAFFOLD_BIN" basecamp launch alice`
3. `pgrep -fl LogosBasecamp`

### Success criteria
- Step 3 shows only the new PID (the one in `launch.state`) for this profile.

## BCLAUNCH-005 — paths resolves profile paths without side effects

### Expected
`basecamp paths <profile>` (and `--json`) prints the XDG config/data/cache dirs, `home`,
runtime dir (`tmpdir` == `xdg_runtime_dir`), module and plugin dirs, launch state, log file
and env file. It builds and changes nothing. With no configured `runtime_dir`, the runtime
dir is `/tmp/lgs-<project-hash>-<profile>`.

### Preconditions
BCSETUP-001.

### Steps
1. `"$SCAFFOLD_BIN" basecamp paths alice --json | jq '{tmpdir, xdg_runtime_dir}'`
2. `find .scaffold -newer scaffold.toml | head`

### Success criteria
- The JSON parses, `tmpdir` equals `xdg_runtime_dir` and starts with `/tmp/lgs-`, and step 2
  shows no new files.

## BCLAUNCH-006 — Custom profiles with env, runtime and log settings

### Expected
A `[basecamp.profiles.<name>]` profile launches like `alice`. Its `env_file` is sourced
before the global and per-profile inline env (inline values win). `runtime_dir` is exported
as both `TMPDIR` and `XDG_RUNTIME_DIR`. `--log-file` or `log_file` tees basecamp output to the
file and the terminal. `[basecamp.env]` sets vars on every profile, and `[basecamp.env_append]`
`:`-joins onto inherited path lists.

### Preconditions
BCMOD-003.

### Steps
1. Add a `[basecamp.profiles.maker]` block with `env_file = ".scaffold/basecamp/maker.env"`,
   `runtime_dir = "/tmp/lgs-maker"`, a `log_file`, and `[basecamp.profiles.maker.env]
   LOGOS_PROFILE_ROLE = "maker"`.
2. `printf 'MAKER_ONLY=1\nLOGOS_PROFILE_ROLE=env-file\n' > .scaffold/basecamp/maker.env`
3. `"$SCAFFOLD_BIN" basecamp paths maker --json`, then `"$SCAFFOLD_BIN" basecamp launch maker --log-file`
4. Read the process env as in BCLAUNCH-001.

### Success criteria
- `MAKER_ONLY=1`, `LOGOS_PROFILE_ROLE=maker`, and `TMPDIR=XDG_RUNTIME_DIR=/tmp/lgs-maker`.
- The log file has the same lines as the terminal.

## BCLAUNCH-007 — Unsafe input is refused before any filesystem work

### Expected
Profile names that are empty, absolute, `.`, `..`, contain a separator or contain control
characters are rejected by both `paths` and `launch`. Each of these refuses before scrubbing
and leaves a running instance alive: an empty `[modules]`, a malformed `env_file` (for
example a key with a control character), an unusable `runtime_dir`, or a log file that cannot
be created.

### Preconditions
BCSETUP-001. For the "instance alive" checks, BCLAUNCH-001 must be running.

### Steps
1. `"$SCAFFOLD_BIN" basecamp paths ../escape; echo "exit=$?"`
2. With `[modules]` temporarily emptied: `"$SCAFFOLD_BIN" basecamp launch alice; echo "exit=$?"`
3. `kill -0 $(sed -n 's/^pid=//p' .scaffold/basecamp/profiles/alice/launch.state) && echo alive`

### Success criteria
- Steps 1 and 2 exit non-zero. Step 3 prints `alive`, and the profile's data is not
  scrubbed.

## BCLAUNCH-008 — Runtime dir is private and outside the project

### Expected
`launch` prepares the runtime dir before scrubbing or reinstalling. It refuses a runtime dir
that is a symlink, is not a directory, or is owned by another user. It creates the dir `0700`
and tightens loose permissions. The default dir is outside the project, so no Unix socket
ever lands under `.scaffold/`, and `basecamp install` keeps working after a launch.

### Preconditions
BCLAUNCH-001 has run at least once.

### Steps
1. `find .scaffold -type s`
2. `stat -c '%a' /tmp/lgs-*-alice 2>/dev/null || stat -f '%Lp' /tmp/lgs-*-alice`
3. `"$SCAFFOLD_BIN" basecamp install`
4. Close basecamp, replace the runtime dir with a symlink to `/tmp/elsewhere`, and run
   `"$SCAFFOLD_BIN" basecamp launch alice; echo "exit=$?"`.

### Success criteria
- Step 1 finds nothing, step 2 prints `700`, and step 3 exits 0.
- Step 4 is refused without scrubbing.
