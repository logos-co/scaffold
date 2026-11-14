---
feature: diagnostics
prefix: DIAG
surface: cli
scenarios:
  - id: DIAG-001
    title: doctor gives actionable checks
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D2
    depends_on: [BUILD-001]
  - id: DIAG-002
    title: doctor JSON output
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D2
    depends_on: [BUILD-001]
  - id: DIAG-003
    title: report produces a support bundle
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D5
    depends_on: [BUILD-001]
  - id: DIAG-004
    title: report never includes wallet secrets
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D5
    depends_on: [DIAG-003]
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
retired: []
---

# Diagnostics

`doctor` checks the project: toolchain, pins, spel/LEZ alignment, circuits, guest build
mode, wallet and localnet. It prints next steps, with `--json` for CI. `report` collects an
allowlisted, redacted `.tar.gz` for bug reports. `completions` prints shell completion
scripts. Related: [Setup and build](setup-and-build.md), [Localnet](localnet.md),
[Basecamp setup](basecamp-setup.md) (`basecamp doctor`).

## DIAG-001 — doctor gives actionable checks

### Expected
`doctor` prints `PASS`/`WARN`/`FAIL` rows and next steps for anything not passing. It covers
the configured circuits install (its `VERSION` against `[circuits].version`, plus drift from
the LEZ pin's expected release), the spel pin and its LEZ alignment, and a `guest build` row
naming the mode and the risc0 builder tag. In docker mode the row is a FAIL when
`cargo-risczero`, `docker` or the daemon is missing. The `docker info` probe is bounded: a
container runtime that does not answer costs at most one 10s timeout, and both the
`container runtime` (WARN) and `guest build` (FAIL) rows say it `did not answer within 10s`.
When `DOCKER_HOST` (or `CONTAINER_HOST` for podman) is set, the remediation names that variable,
never its value, and says to fix the remote connection or unset it.

### Preconditions
BUILD-001.

### Steps
1. `"$SCAFFOLD_BIN" doctor`. On a host with no working container runtime, in a project with
   `[build].guest = "local"`, the `container runtime` row is a PASS reading `not needed with
   [build].guest = "local" (…)`; without that setting (and in every spel project, which always
   builds through Docker) it is the WARN `… guest builds will fail`.
2. Set `[circuits].version` to a wrong value, run `"$SCAFFOLD_BIN" doctor`, then restore it.
3. Set `[circuits].install_dir` to an empty directory, run `"$SCAFFOLD_BIN" doctor`, then restore
   it.
4. `time DOCKER_HOST=tcp://10.255.255.1:2375 "$SCAFFOLD_BIN" doctor | grep -A1 -E 'container runtime|guest build'`
   (first check that `curl -m 3 http://10.255.255.1:2375` times out rather than being refused;
   a refused connection fails fast and does not exercise the bound)

### Success criteria
- Every non-PASS row has an actionable next step.
- The `guest build` row mentions `risczero/risc0-guest-builder:<tag>`.
- Step 2 shows a circuits warning or error that names the configured install dir.
- Step 3 reports `missing configured circuits release at <that dir>` with the remediation
  `Run \`logos-scaffold setup\``.
- Step 4 finishes in well under 30s (one probe timeout, not one per row). `container runtime`
  is WARN with `` `docker info` did not answer within 10s``, and `guest build` is FAIL with
  `` `docker info` did not answer within 10s``. Both remediations name `DOCKER_HOST` and neither
  prints `10.255.255.1`. A `doctor` that hangs is the regression.

## DIAG-002 — doctor JSON output

### Expected
`doctor --json` is pure, parseable JSON with at least `status`, `summary`, `checks` and
`next_steps`.

### Preconditions
BUILD-001.

### Steps
1. `"$SCAFFOLD_BIN" doctor --json | jq -e '.status and .summary and .checks and .next_steps'`

### Success criteria
- `jq -e` exits 0.

## DIAG-003 — report produces a support bundle

### Expected
`report` prints a completion message, the archive path and a warning to inspect it before
sharing. The default location is `.scaffold/reports/*.tar.gz`. `--out PATH` and `--tail N`
are honoured. The archive contains `README.txt`, `manifest.json`, `diagnostics/doctor.json`,
`diagnostics/localnet-status.json` and `summaries/build-evidence.json`.

### Preconditions
BUILD-001, ideally after a localnet start or a build.

### Steps
1. `"$SCAFFOLD_BIN" report --tail 200`
2. `"$SCAFFOLD_BIN" report --out "$PWD/artifacts/support-report.tar.gz"`
3. `tar -tzf "$PWD/artifacts/support-report.tar.gz" | sort`
4. `time DOCKER_HOST=tcp://10.255.255.1:2375 "$SCAFFOLD_BIN" report --out /tmp/r.tar.gz`

### Success criteria
- Both archives exist, the inspect warning is printed, and every file listed above is in the
  archive.
- Step 4 completes in well under 30s: `report` embeds doctor's output, so it inherits the
  bounded runtime probe and never blocks on an unreachable daemon.

### Notes
Never upload an archive without listing its contents and asking the user.

## DIAG-004 — report never includes wallet secrets

### Expected
No file from `.scaffold/wallet/` appears in the archive. Absolute local paths in human-facing
files are scrubbed.

### Preconditions
DIAG-003.

### Steps
1. `tar -tzf <archive> | grep -i wallet`
2. `tar -xzf <archive> -O | grep -c "$HOME"`

### Success criteria
- Step 1 shows no `.scaffold/wallet` content.
- Step 2 finds no unscrubbed home paths in human-facing files (record any hit).

## DIAG-005 — Shell completions

### Expected
`completions bash` and `completions zsh` print a completion script to stdout covering both
`lgs` and `logos-scaffold`. Per-shell install instructions are in `completions <shell> --help`.

### Preconditions
None.

### Steps
1. `"$SCAFFOLD_BIN" completions zsh | head -5`
2. `"$SCAFFOLD_BIN" completions bash | grep -c logos-scaffold`

### Success criteria
- Both exit 0, the output is a script, and both binary names appear.

## DIAG-006 — doctor checks the wallet's sequencer address in either config shape

### Expected
The `wallet network config` row passes when the address the wallet will use is a loopback
host (`127.0.0.1`, `localhost` or `::1`) on exactly `[localnet].port`; `127.0.0.1:30400` does
not pass for port 3040. Otherwise it warns, and the detail names the address it found, or says
`wallet config names no sequencer address`. A missing config warns with `Run
\`logos-scaffold setup\``. An unparseable one warns that it could not be parsed, with the
remediation to fix the JSON by hand or delete the file and run `logos-scaffold setup`
(`setup` never overwrites an existing config). The remediation names the key the config's own shape uses:
`sequencers[0].sequencer_addr` for a LEZ v0.2.4 config, top-level `sequencer_addr` for an older
one. A top-level `sequencer_addr` added next to a `sequencers` list does not change the
verdict, because the v0.2.4 wallet never reads it.

### Preconditions
BUILD-001. Back up `.scaffold/wallet/wallet_config.json` first.

### Steps
1. `"$SCAFFOLD_BIN" doctor | grep -A1 'wallet network config'`
2. Set `sequencers[0].sequencer_addr` to `http://10.255.255.1:3040` and repeat step 1.
3. Also add a top-level `"sequencer_addr": "http://127.0.0.1:3040"` and repeat step 1.
4. Set `sequencers[0].sequencer_addr` to `http://127.0.0.1:30400` and repeat step 1.
5. Replace the file with the pre-v0.2.4 shape `{"sequencer_addr": "http://10.0.0.1:3040"}` and
   repeat step 1.
6. Truncate the file to `{ "sequencers": [ ` and repeat step 1.
7. Restore the config.

### Success criteria
- Step 1 is PASS on a fresh setup.
- Steps 2 and 3 are WARN, naming `http://10.255.255.1:3040`. Their remediation reads
  `Set sequencers[0].sequencer_addr in .scaffold/wallet/wallet_config.json to
  http://127.0.0.1:<port>`. A PASS in step 3 is the regression.
- Step 4 is WARN (no prefix match on the port).
- Step 5 is WARN with the remediation naming the top-level `sequencer_addr`.
- Step 6 is WARN, `could not parse`, with the fix-or-delete remediation; plain `setup` alone
  would not repair it.
