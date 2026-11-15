---
feature: basecamp-launch
prefix: BCLAUNCH
surface: cli
scenarios:
  - id: BCLAUNCH-001
    title: Launch one profile with isolated state
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 B2
    depends_on: [BCMOD-003]
  - id: BCLAUNCH-002
    title: Two profiles interact peer to peer
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 B3
    depends_on: [BCLAUNCH-001]
  - id: BCLAUNCH-003
    title: Relaunch replaces the running instance and scrubs profile state
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 B4
    depends_on: [BCLAUNCH-001]
  - id: BCLAUNCH-005
    title: paths resolves profile paths without side effects
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B2
  - id: BCLAUNCH-006
    title: Custom profiles with env, runtime and log settings
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B2
    depends_on: [BCMOD-003]
  - id: BCLAUNCH-007
    title: Launch refuses unsafe input and unsafe runtime dirs before scrubbing
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 B4
    depends_on: [BCLAUNCH-001]
retired: [BCLAUNCH-004, BCLAUNCH-008]
---

# Basecamp launch

`basecamp launch <profile>` gives clean-slate desktop instances for dogfooding modules. Each
launch kills the profile's previous instance, scrubs its data and cache, reinstalls the
captured modules, and execs basecamp. The environment is profile-scoped: `XDG_*`,
`HOME=<profile>/home`, `LOGOS_PROFILE` and an absolute `LOGOS_USER_DIR`. Two profiles can run
side by side. Related: [Basecamp modules](basecamp-modules.md),
[Basecamp setup](basecamp-setup.md).

`PROF=.scaffold/basecamp/profiles` below. A profile's pid is
`sed -n 's/^pid=//p' $PROF/<name>/launch.state`; read its environment with
`tr '\0' '\n' < /proc/$PID/environ` on Linux or `ps eww -p $PID` on macOS.

## BCLAUNCH-001 — Launch one profile with isolated state

### Expected
`basecamp launch alice` kills any previous instance, scrubs `$PROF/alice/` data and cache,
reinstalls the captured modules, prints a variant-check summary and then execs basecamp. It
records `pid=<pid>` in the profile's `launch.state`. The process environment has
`XDG_{CONFIG,DATA,CACHE}_HOME` under the profile, `HOME=<profile>/home`,
`LOGOS_PROFILE=alice` and an absolute `LOGOS_USER_DIR` under the profile. The project's UI
module appears as a sidebar tile, and opening it logs `Module loaded: <dep>` for each core
dependency. The default runtime dir is outside the project and private (`0700`), so no Unix
socket ever lands under `.scaffold/`, and `basecamp install` keeps working after a launch.

### Preconditions
BCMOD-003 and a graphical session (Xvfb plus `xdotool` is enough when headless).

### Steps
1. `"$SCAFFOLD_BIN" basecamp launch alice` (keep it running)
2. `PID=$(sed -n 's/^pid=//p' $PROF/alice/launch.state); tr '\0' '\n' < /proc/$PID/environ | grep -E 'XDG_|LOGOS_|^HOME='`
3. `find .scaffold -type s; stat -c '%a' /tmp/lgs-*-alice 2>/dev/null || stat -f '%Lp' /tmp/lgs-*-alice; "$SCAFFOLD_BIN" basecamp install; echo "exit=$?"`

### Success criteria
- The window opens with the project modules.
- Every listed path is inside `$PROF/alice/`, and `LOGOS_DATA_DIR` is not set by scaffold.
- Step 3 finds no socket under `.scaffold`, prints `700`, and `install` exits 0.

## BCLAUNCH-002 — Two profiles interact peer to peer

### Expected
`launch alice` and `launch bob` run at the same time from two terminals. Each uses its own
profile directories, identities and history. There is no collision on Qt remote objects or
any non-module port. A p2p action from one instance is observable in the other. Each profile
has its own `HOME`, so a storage-backed module writes its node key to
`<profile>/home/.logos_storage/data/key` per profile and never touches the real
`~/.logos_storage`.

### Preconditions
BCLAUNCH-001, a module with a p2p surface, and two terminals.

### Steps
1. Terminal 1: `"$SCAFFOLD_BIN" basecamp launch alice`. Terminal 2: `"$SCAFFOLD_BIN" basecamp launch bob`.
2. Trigger the module's p2p action in `alice` and watch `bob`, then the reverse.
3. For a storage-backed module: check that `~/.logos_storage/config.json` keeps its mtime,
   and `cmp $PROF/alice/home/.logos_storage/data/key $PROF/bob/home/.logos_storage/data/key`.

### Success criteria
- Both windows show the project modules, and each `LOGOS_USER_DIR` is distinct and absolute.
- The interaction propagates in both directions. The real `~/.logos_storage` is unchanged.
- In step 3 both key files exist and `cmp` reports that they differ.

### Notes
Scaffold exports no per-module port overrides (the registry is empty), and
`[basecamp].port_base` / `port_stride` are reserved and unused, so a module-level port
collision is the owning module's bug; a module that binds a fixed port needs a distinct value
per profile under `[basecamp.profiles.<name>.env]`. If the module has no p2p wiring yet,
record that as a gap, not a pass. On macOS, both windows showing only basecamp's bundled
modules, with logs naming a base directory under the shared `~/Library/Application
Support/Logos/LogosBasecamp[Dev]`, means the app is not reading the per-profile module root:
check that `LOGOS_USER_DIR` is present, absolute and distinct per profile. If one window
crashes, save its logs from `$PROF/<name>/` before relaunching. Launching the same profile
twice in parallel is not supported.

## BCLAUNCH-003 — Relaunch replaces the running instance and scrubs profile state

### Expected
A second `launch alice` while the first is running terminates the first and leaves no
orphan. It removes everything under the profile's `xdg-data` and `xdg-cache` (including
`module_data/` and basecamp's `logs/`) and the legacy `xdg-tmp`. `xdg-config` and `home/`
are kept on purpose. The scrub never reaches outside `$PROF/<profile>/`.

### Preconditions
BCLAUNCH-001 running.

### Steps
1. `mkdir -p $PROF/alice/xdg-data/scratch && echo m > $PROF/alice/xdg-data/scratch/marker.txt; for d in home xdg-config; do mkdir -p $PROF/alice/$d && echo k > $PROF/alice/$d/keep.txt; done; pgrep -fl LogosBasecamp`
2. `"$SCAFFOLD_BIN" basecamp launch alice`, and let it come up.
3. `pgrep -fl LogosBasecamp; sed -n 's/^pid=//p' $PROF/alice/launch.state`
4. `test -e $PROF/alice/xdg-data/scratch/marker.txt && echo SURVIVED || echo SCRUBBED; ls $PROF/alice/home/keep.txt $PROF/alice/xdg-config/keep.txt`

### Success criteria
- Step 3 shows only the new PID (the one in `launch.state`) for this profile.
- Step 4 prints `SCRUBBED` and lists both `keep.txt` files.

### Notes
The Linux entry points are launcher scripts, so the kernel reports the hidden binary's name:
`.LogosBasecamp` for the dev build and `.LogosBasecamp.` for the portable bundle (cut to 15
bytes); the macOS bundle reports `LogosBasecamp`. A surviving process under any of those
names is the regression, and a silent one: the kill is skipped rather than failing.

## BCLAUNCH-005 — paths resolves profile paths without side effects

### Expected
`basecamp paths <profile>` (and `--json`) prints the XDG config/data/cache dirs, `home`,
runtime dir (`tmpdir` == `xdg_runtime_dir`), module and plugin dirs, `module_data_dir`,
`app_logs_dir`, launch state, log file and env file. It builds and changes nothing, and it
needs neither Nix nor `basecamp setup`. With no configured `runtime_dir`, the runtime dir is
`/tmp/lgs-<project-hash>-<profile>`. A profile name that is not a single path component is
rejected.

### Preconditions
Any scaffold project.

### Steps
1. `touch /tmp/before; "$SCAFFOLD_BIN" basecamp paths alice --json | jq '{tmpdir, xdg_runtime_dir, module_data_dir, app_logs_dir}'; find .scaffold -newer /tmp/before`
2. `"$SCAFFOLD_BIN" basecamp paths ../escape; echo "exit=$?"`

### Success criteria
- The JSON parses, `tmpdir` equals `xdg_runtime_dir` and starts with `/tmp/lgs-`, the two
  module paths are non-null, and `find` shows no new files.
- Step 2 exits non-zero with ``invalid profile name `../escape`: must be a single path
  component``.

## BCLAUNCH-006 — Custom profiles with env, runtime and log settings

### Expected
A `[basecamp.profiles.<name>]` profile launches like `alice`. Its `env_file` is sourced
before the global and per-profile inline env (inline values win). `runtime_dir` is exported
as both `TMPDIR` and `XDG_RUNTIME_DIR`. `--log-file` or `log_file` tees basecamp output to the
file and the terminal. A bare `--log-file` uses `<profile>/basecamp.log`, and
`--log-file=PATH` (the `=` is required) picks a path relative to the project root. Either form
wins over `log_file`. `[basecamp.env]` sets vars on every profile, and `[basecamp.env_append]`
`:`-joins onto inherited path lists.

### Preconditions
BCMOD-003.

### Steps
1. Add a `[basecamp.profiles.maker]` block with `env_file = ".scaffold/basecamp/maker.env"`,
   `runtime_dir = "/tmp/lgs-maker"`, `log_file = ".scaffold/basecamp/maker-config.log"`,
   and `[basecamp.profiles.maker.env] LOGOS_PROFILE_ROLE = "maker"`; then
   `printf 'MAKER_ONLY=1\nLOGOS_PROFILE_ROLE=env-file\n' > .scaffold/basecamp/maker.env`
2. `"$SCAFFOLD_BIN" basecamp paths maker --json`, then `"$SCAFFOLD_BIN" basecamp launch maker --log-file`
   and read the process env.
3. Close it, then run `"$SCAFFOLD_BIN" basecamp launch maker --log-file=.scaffold/maker-explicit.log`
   and close it again.

### Success criteria
- `MAKER_ONLY=1`, `LOGOS_PROFILE_ROLE=maker`, and `TMPDIR=XDG_RUNTIME_DIR=/tmp/lgs-maker`.
- The step 2 log is `$PROF/maker/basecamp.log`, not the configured `maker-config.log`, and
  has the same lines as the terminal.
- Step 3 prints `teeing basecamp output to <project>/.scaffold/maker-explicit.log`, and that
  file has the basecamp output.

### Notes
On macOS a Unix socket path is limited to 104 bytes, and launch puts
`<runtime_dir>/logos_token_<module>_<pid>` sockets in the runtime dir. Scaffold does not check
that length for a custom `runtime_dir`, so a long one is a finding to record, not a scaffold
refusal. A relative `runtime_dir` resolves against the project root, which puts sockets back
in the flake tree. Keep custom values short and absolute under `/tmp`.

## BCLAUNCH-007 — Launch refuses unsafe input and unsafe runtime dirs before scrubbing

### Expected
`launch` rejects profile names that are empty, absolute, `.`, `..`, contain a separator or
contain control characters, with an `invalid profile name` error that says it `must be a
single path component` or `must not contain control characters`. Each of these refuses
before scrubbing and leaves a running instance alive: an empty `[modules]`, a malformed
`env_file` (a key with a control character fails with `<env_file>:<line>: env var name "…"
must not contain control characters`), a log file that cannot be created, or an unusable
runtime dir (a symlink, not a directory, or owned by another user). `launch` prepares the
runtime dir before scrubbing or reinstalling, creating it `0700` and tightening loose
permissions.

### Preconditions
BCLAUNCH-001 running (in a set-up project the name check comes first; before `basecamp
setup` every launch fails with the not-set-up hint instead). Back up `scaffold.toml`.
`alive() { kill -0 $(sed -n 's/^pid=//p' $PROF/alice/launch.state) && echo alive; }`

### Steps
1. `for p in '' . .. ../escape /tmp/abs a/b "$(printf 'a\tb')"; do "$SCAFFOLD_BIN" basecamp launch "$p"; echo "exit=$?"; done; ls $PROF`
2. With `[modules]` temporarily emptied: `"$SCAFFOLD_BIN" basecamp launch alice; echo "exit=$?"; alive`
3. With `[modules]` restored, add `env_file = ".scaffold/basecamp/alice.env"` under
   `[basecamp.profiles.alice]`, run `printf 'BAD\tKEY=1\n' > .scaffold/basecamp/alice.env`, then
   `"$SCAFFOLD_BIN" basecamp launch alice; echo "exit=$?"; alive`. Restore `scaffold.toml`.
4. Close basecamp, place a marker as in BCLAUNCH-003 step 1, replace the runtime dir
   (`/tmp/lgs-*-alice`) with a symlink to `/tmp/elsewhere`, and run
   `"$SCAFFOLD_BIN" basecamp launch alice; echo "exit=$?"`. Remove the symlink.

### Success criteria
- Every launch in step 1 exits non-zero with an `invalid profile name` error, and no new
  directory appears under `$PROF` or outside it.
- Steps 2 and 3 exit non-zero and print `alive`; step 3's error names line 1 of the env file.
- Step 4 is refused, and the marker survives (nothing was scrubbed).
