---
feature: tui
prefix: TUI
surface: cli
scenarios:
  - id: TUI-001
    title: Dashboard status agrees with the CLI
    priority: P1
    provenance: documented
    source: DOGFOODING.md D9
    depends_on: [BUILD-001]
  - id: TUI-002
    title: Each key runs the command it names
    priority: P1
    provenance: documented
    source: DOGFOODING.md D9
    depends_on: [TUI-001]
  - id: TUI-003
    title: First run marks programs deployed; reset clears the marks
    priority: P1
    provenance: documented
    source: DOGFOODING.md D9
    depends_on: [TUI-002]
  - id: TUI-004
    title: One job at a time, and x stops its whole process tree
    priority: P1
    provenance: documented
    source: DOGFOODING.md D9
    depends_on: [TUI-002]
  - id: TUI-005
    title: Reset and quit-with-a-job ask first
    priority: P2
    provenance: documented
    source: DOGFOODING.md D9
    depends_on: [TUI-001]
  - id: TUI-006
    title: Localnet outlives the dashboard; no terminal is refused
    priority: P1
    provenance: documented
    source: DOGFOODING.md D9
    depends_on: [TUI-003]
  - id: TUI-007
    title: Start-up and corner λ
    priority: P2
    provenance: documented
    source: DOGFOODING.md D9
    depends_on: [TUI-001]
  - id: TUI-008
    title: Sequencer log pane shows plain text
    priority: P2
    provenance: documented
    source: DOGFOODING.md D9
    depends_on: [TUI-003]
  - id: TUI-009
    title: Long output wraps and scrolls to the top
    priority: P2
    provenance: documented
    source: DOGFOODING.md D9
    depends_on: [TUI-002]
  - id: TUI-010
    title: Job line stays visible with a hint and a long label
    priority: P2
    provenance: documented
    source: DOGFOODING.md D9
    depends_on: [TUI-002]
retired: []
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
can be captured:

```bash
tmux new-session -d -s d9 -x 120 -y 32 "$SCAFFOLD_BIN" tui
cap() { sleep "${1:-2}"; tmux capture-pane -p -t d9; }
```

## TUI-001 — Dashboard status agrees with the CLI

### Expected
The Status and Programs panes show the same facts as the CLI. That means `localnet status` for
the state and pid, `wallet list` or `.scaffold/state/wallet.state` for the default address, and
`methods/guest/src/bin` for the program list.

### Preconditions
BUILD-001, `tmux`, localnet stopped (`"$SCAFFOLD_BIN" localnet stop`).

### Steps
1. Start the session as above, then `cap`.
2. `"$SCAFFOLD_BIN" localnet status; grep default_address .scaffold/state/wallet.state; ls methods/guest/src/bin`

### Success criteria
- The capture shows `stopped`, the default address or `no default yet`, every program in
  step 2's listing, and the empty-Output hint naming `r`.
- Nothing on screen contradicts step 2. Any disagreement is the high-value finding.

## TUI-002 — Each key runs the command it names

### Expected
Each key's Output starts with `$ lgs <args>` naming the command it runs, shows that command's
own output, and ends with `── lgs <args>: exit <code> ──`. The keys are `r` run, `w`
run --watch, `b` build, `d` deploy for the selected program, `s` localnet start/stop, `R`
localnet reset --yes, `t` wallet topup and `c` doctor.

### Preconditions
TUI-001.

### Steps
1. `tmux send-keys -t d9 r`, then poll `cap` until `── lgs run: exit 0 ──` appears. This
   takes minutes on a cold build.
2. `tmux send-keys -t d9 t; cap 20` and `tmux send-keys -t d9 c; cap 10`.
3. `tmux send-keys -t d9 j d; cap 20` (deploy the second program).
4. `tmux send-keys -t d9 s; cap 5; "$SCAFFOLD_BIN" localnet status`, then
   `tmux send-keys -t d9 s; cap 10; "$SCAFFOLD_BIN" localnet status`.

### Success criteria
- Step 4: the first `s` runs `lgs localnet stop` and both the Status pane and `localnet status`
  show it stopped; the second runs `lgs localnet start` and both show it running again.
- Every job is framed by its `$ lgs <args>` and `── lgs <args>: exit <code> ──` lines, and
  no output follows the exit line.
- While a job runs, the Job line shows its label and a live timer.

## TUI-003 — First run marks programs deployed; reset clears the marks

### Expected
After the first `r`, every program shows its build (`docker`, or `release` under
`[build].guest = "local"`) and `✓ deployed`, and the block height advances. A second `r`
skips the deploy and keeps the marks. `d` deploys without setting `✓`, because only `run`
records deploys. After `R` and `y`, the marks clear (new chain) and the wallet is kept.

### Preconditions
TUI-002 step 1.

### Steps
1. `cap; cap 5`. Compare the block heights.
2. `tmux send-keys -t d9 r; cap 30`
3. `tmux send-keys -t d9 R; cap 1; tmux send-keys -t d9 y; cap 30`
4. `tmux send-keys -t d9 r`, then poll `cap` until `── lgs run: exit 0 ──`.

### Success criteria
- Step 1: every program shows its build kind (`docker`, or `release` under
  `[build].guest = "local"`) and `✓ deployed`, and the height grows.
- Step 2: the `✓` marks stay.
- Step 3: the marks are gone and the Wallet line still shows the same address.
- Step 4 deploys again (no `Deploy skipped`), and the `✓` marks return.

## TUI-004 — One job at a time, and x stops its whole process tree

### Expected
A key pressed while a job runs prints `… is still running; x stops it` and starts nothing.
`x` ends the job with `: stopped` or a non-zero exit, and no `cargo` or `docker` process from
it remains.

### Preconditions
TUI-002.

### Steps
1. `tmux send-keys -t d9 b; cap 3; tmux send-keys -t d9 c; cap 1`
2. `tmux send-keys -t d9 x; cap 3; pgrep -f 'cargo build'; "$SCAFFOLD_BIN" localnet status`

### Success criteria
- Step 1 shows the still-running message and no `$ lgs doctor` line.
- Step 2 shows the build's exit line, `pgrep` finds nothing, and the localnet is still
  running: `x` reaches the job's process group, never the daemonized sequencer.

## TUI-005 — Reset and quit-with-a-job ask first

### Expected
`R`, and `q` (or `Esc`) while a job runs, open a confirmation. `y` or `Enter` confirms; any
other key cancels, and the box's footer says so: ` y / Enter yes · any other key no `.

### Preconditions
TUI-001.

### Steps
1. `tmux send-keys -t d9 R; cap 1; tmux send-keys -t d9 n; cap 1`
2. Start a job (`b`), then `tmux send-keys -t d9 q; cap 1; tmux send-keys -t d9 n; cap 1`
3. With the job still running, `tmux send-keys -t d9 Escape; cap 1; tmux send-keys -t d9 n; cap 1`

### Success criteria
- Each step shows a Confirm box whose footer names both `y` and `Enter`, and after `n`
  nothing was reset and the dashboard is still open.

## TUI-006 — Localnet outlives the dashboard; no terminal is refused

### Expected
Quitting stops only the running job. The localnet, daemonized by `localnet start`, keeps
running. Without an interactive terminal, `tui` exits non-zero with a message instead of
drawing into a pipe.

### Preconditions
TUI-003, with the localnet running.

### Steps
1. `tmux send-keys -t d9 q; sleep 1; "$SCAFFOLD_BIN" localnet status`
2. `"$SCAFFOLD_BIN" tui < /dev/null; echo "exit=$?"`

### Success criteria
- Step 1 still reports the localnet running, with the same pid as before.
- Step 2 exits non-zero with `needs an interactive terminal`.

## TUI-007 — Start-up and corner λ

### Expected
The dashboard opens on a large λ with `L O G O S   S C A F F O L D` and replaces it within
three seconds. At 120×32 or larger a small λ sits in the top-right corner. It spins while a
job runs and is still when idle. Below 120 columns or 30 rows it is absent and the panes are
unchanged.

### Preconditions
TUI-001.

### Steps
1. In a new session, `cap 0.5`, then `cap`.
2. Start a job and `cap 1; cap 1`.
3. `tmux resize-window -t d9 -x 119 -y 32; cap 1`, then `tmux resize-window -t d9 -x 120 -y 29; cap 1`.

### Success criteria
- Step 1 shows the large λ, then the dashboard with the corner λ.
- Step 2's two captures show different corner shapes.
- In both step 3 captures the corner λ is gone, and the Status, Programs and Output panes are
  laid out as before.

## TUI-008 — Sequencer log pane shows plain text

### Expected
`l` switches the Output pane to the tail of `.scaffold/logs/sequencer.log`, and `l` again
switches back. The log is shown as plain text. risc0 writes ANSI colour codes into the file,
and none of them reach the screen.

### Preconditions
TUI-003 (a real sequencer that has produced blocks).

### Steps
1. `tmux send-keys -t d9 l; cap; cap 3`
2. `grep -c $'\e\[' .scaffold/logs/sequencer.log`

### Success criteria
- The captures contain no `[2m`/`[0m` fragments and no shifted timestamps (`20266-11-…`), and
  no row carries the tail of an earlier, longer line.
- Step 2 is non-zero, which confirms that the file holds escapes for the pane to clean.

## TUI-009 — Long output wraps and scrolls to the top

### Expected
A line longer than the Output pane wraps onto the next row instead of ending at the border.
PgUp scrolls by rendered rows and reaches the first line of a long, wrapped output.

### Preconditions
TUI-002.

### Steps
1. Make the build fail with a long message. Set `[build].guest = "docker"` on a host without
   `cargo-risczero` on PATH, or use any command whose error exceeds the pane width. Then
   `tmux send-keys -t d9 b; cap 10`.
2. `tmux send-keys -t d9 PPage PPage PPage; cap 1`

### Success criteria
- Step 1 shows the whole remediation, including `cargo install cargo-risczero`, across rows.
- Step 2 reaches the `$ lgs build` line.

## TUI-010 — Job line stays visible with a hint and a long label

### Expected
The Job line stays visible in every localnet state. The Status pane grows taller to fit a
`foreign listener` or `stale state` hint together with a long label such as
`lgs deploy hello_world_with_authorization`, rather than dropping a row.

### Preconditions
TUI-002, and a second project (or any listener) holding this project's `[localnet].port`, so
the state reads `foreign listener`.

### Steps
1. Select `hello_world_with_authorization`, then `tmux send-keys -t d9 d; cap 5`, and `cap`
   again after it exits.

### Success criteria
- Both captures show the `Job` row: the label with a timer while the job runs, then the
  `✓`/`✗ … exit <code>` summary.
