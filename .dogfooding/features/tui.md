---
feature: tui
prefix: TUI
surface: cli
scenarios:
  - id: TUI-001
    title: Dashboard status agrees with the CLI
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D9
    depends_on: [BUILD-001]
  - id: TUI-002
    title: Each key runs the command it names, and run marks programs deployed
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D9
    depends_on: [TUI-001]
  - id: TUI-004
    title: One job at a time, x stops its process tree, destructive keys ask first
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D9
    depends_on: [TUI-002]
  - id: TUI-006
    title: Localnet outlives the dashboard; no terminal is refused
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D9
    depends_on: [TUI-002]
  - id: TUI-007
    title: Rendering holds up - start-up λ, log pane, wrapping, job line
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D9
    depends_on: [TUI-002]
retired: [TUI-003, TUI-005, TUI-008, TUI-009, TUI-010]
---

# Terminal dashboard

`lgs tui` is a terminal dashboard over the inner loop of one project. A Status pane shows the
localnet state, pid and block height, plus the default wallet address and the current Job. A
Programs pane lists each guest program, its build and whether the last `run` deployed it. An
Output pane shows the running job's output or the sequencer log. Status is read in process.
Every action key spawns the CLI itself (`lgs <args>`) as a child in the project root, one at a
time, each in its own session. Related: [Run pipeline](run-pipeline.md),
[Localnet](localnet.md), [Deploy](deploy.md), [Wallet](wallet.md).

All steps run from the project root, with the dashboard driven through `tmux` so its screen
can be captured. Use at least 100 columns; narrower terminals truncate program names. The
scenarios run in order in one session unless a step says "new session".

```bash
tui() { tmux kill-session -t d9 2>/dev/null; tmux new-session -d -s d9 -x 120 -y 32 "$SCAFFOLD_BIN" tui; }
key() { tmux send-keys -t d9 "$@"; }
cap() { sleep "${1:-2}"; tmux capture-pane -p -t d9; }
until_exit() { until cap 5 | grep -q "── lgs $1: exit"; do :; done; cap 0; }   # until_exit run
```

## TUI-001 — Dashboard status agrees with the CLI

### Expected
The Status and Programs panes show the same facts as the CLI: `localnet status` for the state
and pid, `.scaffold/state/wallet.state` for the default address (abbreviated on screen), and
`methods/guest/src/bin` for the program list.

### Preconditions
BUILD-001 and a built project, `tmux`, localnet stopped (`"$SCAFFOLD_BIN" localnet stop`).

### Steps
1. `tui; cap 5`
2. `"$SCAFFOLD_BIN" localnet status; grep default_address .scaffold/state/wallet.state; ls methods/guest/src/bin`

### Success criteria
- The capture shows `stopped`, the default address or `no default yet`, every program in
  step 2's listing, and the empty-Output hint naming `r`.
- Nothing on screen contradicts step 2. Any disagreement is the high-value finding.

## TUI-002 — Each key runs the command it names, and run marks programs deployed

### Expected
Each key's Output starts with `$ lgs <args>` naming the command it runs, shows that command's
own output, and ends with `── lgs <args>: exit <code> ──`. The keys are `r` run, `w`
run --watch, `b` build, `d` deploy for the selected program, `s` localnet start/stop, `R`
localnet reset --yes, `t` wallet topup and `c` doctor. After the first `r`, every program
shows its build (`docker`, or `release` under `[build].guest = "local"`) and `✓ deployed`, and
the block height advances. A second `r` skips the deploy and keeps the marks. `d` deploys
without setting `✓`, because only `run` records deploys. After `R` and `y`, the marks clear
(new chain) and the wallet is kept.

### Preconditions
TUI-001's session.

### Steps
1. `key r; until_exit run; cap 5` (minutes on a cold build). Compare the two block heights.
2. `key t; until_exit "wallet topup"; key c; until_exit doctor; key j d; until_exit "deploy hello_world_with_authorization"`
3. `key s; cap 5; "$SCAFFOLD_BIN" localnet status`, then again: `key s; cap 10; "$SCAFFOLD_BIN" localnet status`
4. `key r; until_exit run`
5. `key R; cap 1; key y; cap 30`, then `key r; until_exit run`

### Success criteria
- Every job is framed by its `$ lgs <args>` and `── lgs <args>: exit <code> ──` lines with
  no output after the exit line; while it runs, the Job line shows its label and a timer.
- Step 1: every program shows its build kind and `✓ deployed`, and the height grows.
- Step 3: the first `s` runs `lgs localnet stop` and both the Status pane and the CLI show it
  stopped; the second runs `lgs localnet start` and both show it running.
- Step 4 prints `Deploy skipped` and the `✓` marks stay.
- Step 5: after the reset the marks are gone and the Wallet line shows the same address; the
  following `r` deploys again (no `Deploy skipped`) and the marks return.

## TUI-004 — One job at a time, x stops its process tree, destructive keys ask first

### Expected
A key pressed while a job runs prints `… is still running; x stops it` and starts nothing.
`x` ends the job with `: stopped` or a non-zero exit, and no `cargo` or `docker` process from
it remains; the daemonized sequencer is not touched. `R`, and `q` (or `Esc`) while a job
runs, open a confirmation. `y` or `Enter` confirms; any other key cancels, and the box's
footer says so: ` y / Enter yes · any other key no `.

### Preconditions
TUI-002's session, with localnet running.

### Steps
1. `key R; cap 1; key n; cap 2`
2. `key b; cap 3; key c; cap 1`
3. `key q; cap 1; key n; cap 2; key Escape; cap 1; key n; cap 2`
4. `key x; cap 3; pgrep -x cargo; "$SCAFFOLD_BIN" localnet status --json | jq .ready`

### Success criteria
- Steps 1 and 3 show a Confirm box whose footer names both `y` and `Enter`; after `n`
  nothing was reset, the job still runs and the dashboard is still open.
- Step 2 shows the still-running message and no `$ lgs doctor` line.
- Step 4 shows `lgs build: stopped` (or a non-zero exit line), `pgrep` finds no build
  process, and localnet is still ready.

## TUI-006 — Localnet outlives the dashboard; no terminal is refused

### Expected
Quitting stops only the running job. The localnet, daemonized by `localnet start`, keeps
running. Without an interactive terminal, `tui` exits non-zero with a message instead of
drawing into a pipe.

### Preconditions
TUI-002's session, with localnet running.

### Steps
1. `PID=$("$SCAFFOLD_BIN" localnet status --json | jq .tracked_pid); key q; sleep 1; tmux has-session -t d9; "$SCAFFOLD_BIN" localnet status --json | jq '.tracked_pid, .ready'`
2. `"$SCAFFOLD_BIN" tui < /dev/null; echo "exit=$?"`

### Success criteria
- Step 1: the tmux session is gone, and localnet is ready with the same pid.
- Step 2 exits non-zero with `needs an interactive terminal`.

## TUI-007 — Rendering holds up - start-up λ, log pane, wrapping, job line

### Expected
- The dashboard opens on a large λ with `L O G O S   S C A F F O L D` and replaces it within
  three seconds. At 120×32 or larger a small λ sits in the top-right corner; it spins while a
  job runs and is still when idle. Below 120 columns or 30 rows it is absent and the panes
  are unchanged.
- `l` switches the Output pane to the tail of `.scaffold/logs/sequencer.log` and back. The
  log is shown as plain text: risc0 writes ANSI colour codes into the file, and none reach
  the screen.
- A line longer than the Output pane wraps instead of ending at the border, and PgUp scrolls
  by rendered rows to the first line of a long, wrapped output.
- The Job line stays visible in every localnet state. The Status pane grows to fit a
  `foreign listener` or `stale state` hint together with a long label such as
  `lgs deploy hello_world_with_authorization`.

### Preconditions
TUI-002 has run (a sequencer log with blocks exists). Steps 3 and 4 need the stated
conditions; say which ones the host could not provide.

### Steps
1. New session: `tui; cap 0.5; cap 4`. Then `key l; cap 2; cap 3; key l`, and
   `grep -c $'\e\[' .scaffold/logs/sequencer.log`.
2. `key b; cap 1; cap 1; key x`, then
   `tmux resize-window -t d9 -x 119 -y 32; cap 1; tmux resize-window -t d9 -x 120 -y 29; cap 1`
3. New session, with a build that fails with a long message (no Docker daemon and no
   `[build].guest = "local"`, or `cargo-risczero` off `PATH`): `tui; cap 4; key b; cap 10; key PPage PPage PPage; cap 1`
4. New session, with localnet stopped and another listener on the port
   (`python3 -m http.server 3040 --bind 127.0.0.1 &`): `tui; cap 4; key j d; cap 5`, then kill
   the listener.

### Success criteria
- Step 1: the first capture shows the large λ, the second the dashboard with the corner λ.
  The log captures contain no `[2m`/`[0m` fragments, no shifted timestamps
  (`20266-11-…`) and no leftovers of an earlier, longer line, while the `grep -c` is non-zero
  (the file does hold escapes).
- Step 2: the two captures during the job show different corner shapes. After each resize
  the corner λ is gone and the Status, Programs and Output panes are laid out as before.
- Step 3: the whole remediation is readable across rows, and the last capture reaches the
  `$ lgs build` line. (The pane keeps earlier jobs' output, hence the new session.)
- Step 4: the Status pane shows `foreign listener pid <pid>` with its hint, and the `Job`
  row with `✗ lgs deploy hello_world_with_authorization: exit 1` is still visible.
