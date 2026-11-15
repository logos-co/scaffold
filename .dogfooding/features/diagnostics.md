---
feature: diagnostics
prefix: DIAG
surface: cli
scenarios:
  - id: DIAG-001
    title: doctor gives actionable checks, also as JSON
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D2
    depends_on: [BUILD-001]
  - id: DIAG-004
    title: report produces a support bundle without wallet secrets
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D5
    depends_on: [BUILD-001]
  - id: DIAG-005
    title: Shell completions
    priority: P2
    provenance: documented
    source: docs/commands.md
  - id: DIAG-006
    title: doctor checks the wallet's sequencer address in either config shape
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D2
    depends_on: [BUILD-001]
retired: [DIAG-002, DIAG-003]
---

# Diagnostics

`doctor` checks the project: toolchain, pins, spel/LEZ alignment, circuits, guest build
mode, wallet and localnet. It prints next steps, with `--json` for CI. `report` collects an
allowlisted, redacted `.tar.gz` for bug reports. `completions` prints shell completion
scripts. Related: [Setup and build](setup-and-build.md), [Localnet](localnet.md),
[Basecamp setup](basecamp-setup.md) (`basecamp doctor`).

`10.255.255.1` stands for an address that never answers. Before any step that uses it, check
that `curl -m 3 http://10.255.255.1:3040` times out (exit 28). A sandbox that refuses it at
once makes the timing checks pass for the wrong reason: note that in the report.

## DIAG-001 — doctor gives actionable checks, also as JSON

### Expected
`doctor` prints `PASS`/`WARN`/`FAIL` rows and next steps for anything not passing, and exits
non-zero on any FAIL. It covers the configured circuits install (its `VERSION` against
`[circuits].version`, plus drift from the LEZ pin's expected release), the spel pin and its
LEZ alignment, and a `guest build` row naming the mode and the risc0 builder tag. In docker
mode that row is a FAIL when `cargo-risczero`, `docker` or the daemon is missing, and the
`container runtime` row is a WARN; with `[build].guest = "local"` both pass (BUILD-005). The
`docker info` probe is bounded: a runtime that does not answer costs one 10s timeout, and
both rows say it `did not answer within 10s`. When `DOCKER_HOST` (or `CONTAINER_HOST` for
podman) is set, the remediation names that variable, never its value. `--json` is pure JSON
with at least `status`, `summary`, `checks` and `next_steps`.

### Preconditions
BUILD-001. Back up `scaffold.toml`. Step 4 needs a project without `[build].guest`.

### Steps
1. `"$SCAFFOLD_BIN" doctor; "$SCAFFOLD_BIN" doctor --json | jq -e '.status and .summary and .checks and .next_steps'`
2. Set `[circuits].version = "0.0.1"`, run `"$SCAFFOLD_BIN" doctor | grep -A1 circuits; echo "exit=$?"`, restore.
3. Set `[circuits].install_dir` to an empty directory, run the same command, restore.
4. `time DOCKER_HOST=tcp://10.255.255.1:2375 "$SCAFFOLD_BIN" doctor | grep -A1 -E 'container runtime|guest build'`

### Success criteria
- Step 1: every non-PASS row has an actionable next step, the `guest build` row names its
  mode (and `risczero/risc0-guest-builder:<tag>` in docker mode), and `jq -e` exits 0.
- Step 2 is a FAIL naming the installed version, the install dir and the configured version,
  with `Run \`logos-scaffold setup\``. Step 3 reports `missing configured circuits release at
  <that dir>` with the same remediation.
- Step 4 finishes in well under 30s. `container runtime` is WARN and `guest build` is FAIL,
  both with `` `docker info` did not answer within 10s``. Both remediations name
  `DOCKER_HOST` and neither prints `10.255.255.1`.

## DIAG-004 — report produces a support bundle without wallet secrets

### Expected
`report` prints a completion message, the archive path and a warning to inspect it before
sharing. The default location is `.scaffold/reports/*.tar.gz`; `--out PATH` and `--tail N`
are honoured. The archive contains `README.txt`, `manifest.json`, `diagnostics/doctor.json`,
`diagnostics/localnet-status.json` and `summaries/build-evidence.json`. No file from
`.scaffold/wallet/` is in it, and absolute local paths are scrubbed. `report` embeds doctor's
output, so it inherits the bounded runtime probe.

### Preconditions
BUILD-001, ideally after a localnet start or a build.

### Steps
1. `"$SCAFFOLD_BIN" report --tail 200; ls .scaffold/reports/`
2. `A="$PWD/artifacts/support-report.tar.gz"; "$SCAFFOLD_BIN" report --out "$A" && tar -tzf "$A" | sort`
3. `tar -tzf "$A" | grep -i wallet; tar -xzf "$A" -O | grep -c "$HOME"`
4. `time DOCKER_HOST=tcp://10.255.255.1:2375 "$SCAFFOLD_BIN" report --out /tmp/r.tar.gz`

### Success criteria
- Both archives exist, the inspect warning is printed, and every file listed above is in the
  archive.
- Step 3 shows no `.scaffold/wallet` content and prints `0` (record any hit).
- Step 4 completes in well under 30s.

### Notes
Never upload an archive without listing its contents and asking the user.

## DIAG-005 — Shell completions

### Expected
`completions bash` and `completions zsh` print a completion script to stdout covering both
`lgs` and `logos-scaffold`. Per-shell install instructions are in `completions <shell> --help`.

### Preconditions
None.

### Steps
1. `"$SCAFFOLD_BIN" completions zsh > /tmp/c.zsh; head -1 /tmp/c.zsh; "$SCAFFOLD_BIN" completions bash > /tmp/c.bash; grep -c -w lgs /tmp/c.bash; grep -c logos-scaffold /tmp/c.bash`
2. `"$SCAFFOLD_BIN" completions zsh --help | grep -c zfunc`

### Success criteria
- Step 1 prints `#compdef lgs logos-scaffold` and two non-zero counts.
- Step 2 is non-zero: the help carries install instructions.

## DIAG-006 — doctor checks the wallet's sequencer address in either config shape

### Expected
The `wallet network config` row passes when the address the wallet will use is a loopback
host (`127.0.0.1`, `localhost` or `::1`) on exactly `[localnet].port`. Otherwise it warns,
and the detail names the address it found, or says `wallet config names no sequencer
address`. The remediation names the key the config's own shape uses:
`sequencers[0].sequencer_addr` for a LEZ v0.2.4 config, top-level `sequencer_addr` for an
older one. A top-level `sequencer_addr` added next to a `sequencers` list does not change the
verdict, because the v0.2.4 wallet never reads it. A missing config warns with `Run
\`logos-scaffold setup\``; an unparseable one warns that it could not be parsed, with the
remediation to fix the JSON by hand or delete the file and run `setup` (`setup` never
overwrites an existing config). `doctor` probes the wallet's target (bounded, like `deploy`)
before calling `wallet check-health`, which has no timeout of its own, so an address that
never answers costs seconds, not minutes.

### Preconditions
BUILD-001 and localnet running. `C=.scaffold/wallet/wallet_config.json`. Run each mutation as
its own command and restore `$C` from a backup after each one, also when a step is
interrupted (`cp $C /tmp/wc.bak; trap 'cp /tmp/wc.bak $C' EXIT`); a hanging step inside a
scripted loop otherwise lets the next step run on the wrong config.
`row() { "$SCAFFOLD_BIN" doctor | grep -A1 -E 'wallet network config|wallet usability'; }`

### Steps
1. `row`
2. `jq '.sequencers[0].sequencer_addr="http://10.255.255.1:3040"' /tmp/wc.bak > $C; time row`
3. `jq '.sequencers[0].sequencer_addr="http://10.255.255.1:3040" | .sequencer_addr="http://127.0.0.1:3040"' /tmp/wc.bak > $C; row`
4. `jq '.sequencers[0].sequencer_addr="http://127.0.0.1:30400"' /tmp/wc.bak > $C; row`
5. `echo '{"sequencer_addr": "http://10.0.0.1:3040"}' > $C; row`
6. `echo '{ "sequencers": [ ' > $C; row`, then restore `$C`.

### Success criteria
- Step 1 is PASS on a fresh setup.
- Step 2 finishes in a few seconds. `wallet network config` is WARN naming
  `http://10.255.255.1:3040`, with the remediation `Set sequencers[0].sequencer_addr in
  .scaffold/wallet/wallet_config.json to http://127.0.0.1:<port>`; `wallet usability` is WARN
  with `wallet cannot reach its configured sequencer at http://10.255.255.1:3040`.
- Step 3 is the same WARN (a PASS is the regression). Step 4 is WARN: no prefix match on the
  port.
- Step 5 is WARN with the remediation naming the top-level `sequencer_addr`.
- Step 6 is WARN, `could not parse`, with the fix-or-delete remediation.
