---
feature: spel-template
prefix: SPEL
surface: cli
scenarios:
  - id: SPEL-001
    title: Create a spel project with pinned versions
    priority: P1
    provenance: documented
    source: DOGFOODING.md L1
  - id: SPEL-002
    title: Set up and build a spel project
    priority: P1
    provenance: documented
    source: DOGFOODING.md L1
    depends_on: [SPEL-001]
  - id: SPEL-003
    title: Regenerate the IDL
    priority: P1
    provenance: documented
    source: DOGFOODING.md L2
    depends_on: [SPEL-001]
  - id: SPEL-004
    title: Generate client and FFI bindings
    priority: P2
    provenance: documented
    source: DOGFOODING.md L3
    depends_on: [SPEL-002]
  - id: SPEL-005
    title: Deploy and invoke instructions through spel
    priority: P1
    provenance: documented
    source: DOGFOODING.md L4
    depends_on: [SPEL-002]
  - id: SPEL-006
    title: Vendored spel project
    priority: P2
    provenance: documented
    source: DOGFOODING.md L1
retired: []
---

# spel template

`new --template spel` (the README quick start) delegates generation to `spel init` from a
spel CLI that scaffold clones and builds at `DEFAULT_SPEL` in the cache. No global `spel` is
needed. spel projects build their guest through their own `Makefile` (`make build` = `cargo
risczero build` in Docker) and drive IDL and client generation through `make idl` /
`make ffi-gen`. `lgs spel -- …` runs the vendored binary against the project wallet. Related:
[Project creation](project-creation.md), [Run pipeline](run-pipeline.md),
[Deploy](deploy.md).

## SPEL-001 — Create a spel project with pinned versions

### Expected
`new <name> --template spel` prints the pins it recorded (`Pinned LEZ: v0.2.4`, `Pinned spel:
v0.7.0` at the time of writing). It creates `spel.toml`, a `Makefile`, `<name>_core/`,
`<name>_ffi/` and `methods/guest/src/bin/<name>.rs`. `methods/guest/Cargo.toml` pins
`spel-framework` to the recorded tag and never to `branch = "main"`. Any `spel` on `PATH` is
irrelevant.

### Preconditions
None (the spel CLI is built during this step). On Linux, the PCSC headers are installed.

### Steps
1. `cd "$SCRATCH_ROOT" && "$SCAFFOLD_BIN" new dogfood-spel --template spel`
2. `cd dogfood-spel && ls -d spel.toml Makefile dogfood_spel_core dogfood_spel_ffi methods/guest/src/bin`
3. `grep spel-framework methods/guest/Cargo.toml`

### Success criteria
- Exit 0, and the pins are printed.
- All listed paths exist, and `spel-framework` uses `tag = …`, not `branch`.
- No `Directory '<name>' already exists` error appears.

## SPEL-002 — Set up and build a spel project

### Expected
`setup`, `localnet start` and `doctor` behave as they do for the default template, and
`doctor` reports zero FAIL. `build` runs `make build` (the Docker guest build, whatever
`[build].guest` says) and then IDL generation, leaving `<name>-idl.json` at the project
root. `build --guest local` prints `Note: --guest local does not apply to spel projects` and
still builds in Docker, so a host without Docker cannot run this scenario. Run it on a
Docker-capable host (a GitHub-hosted runner works) instead of reporting it blocked.

### Preconditions
SPEL-001, Docker, `cargo-risczero` 3.0.5, and the risc0 C++ toolchain.

### Steps
1. `"$SCAFFOLD_BIN" setup && "$SCAFFOLD_BIN" localnet start && "$SCAFFOLD_BIN" doctor`
2. `"$SCAFFOLD_BIN" build`
3. `ls ./*-idl.json methods/guest/target/riscv32im-risc0-zkvm-elf/docker/`
4. `P=$PWD; ( cd / && "$SCAFFOLD_BIN" build --guest local "$P" )`

### Success criteria
- `doctor` shows 0 FAIL, `build` exits 0, and the IDL plus the docker `.bin` exist.
- Step 4 prints the `--guest local` note, runs `make build` (Docker) and exits 0. The
  positional project path makes it work from `/`.

### Notes
The first build takes 5–15 minutes, which is normal.

## SPEL-003 — Regenerate the IDL

### Expected
`build idl` prints ``Generating IDL via `make idl` (spel project)...``, runs `make idl`, and
the Makefile prints `✅ IDL written to <name>-idl.json`. The file named by `spel.toml`
`[program] idl` is valid JSON matching the program. No `idl/` directory is created, and no
IDL JSON is printed to the terminal in place of the file. `build idl [project-path]` works
from outside the project. `--guest` is rejected with `build idl`, since it only applies to
`build`.

### Preconditions
SPEL-001 with a working build environment.

### Steps
1. `"$SCAFFOLD_BIN" build idl | tee /tmp/idl.out`
2. `jq -e . ./*-idl.json > /dev/null; ls -d idl; grep -c '^{' /tmp/idl.out`
3. `P=$PWD; rm ./*-idl.json; ( cd / && "$SCAFFOLD_BIN" build idl "$P" ); ls "$P"/*-idl.json`
4. `"$SCAFFOLD_BIN" build --guest docker idl; echo "exit=$?"`

### Success criteria
- The `Generating IDL via` and `IDL written` lines appear, and the file parses as JSON.
- `ls -d idl` fails, and the `grep -c` prints `0` (no IDL JSON on the terminal).
- Step 3 recreates the IDL file from `/`.
- Step 4 exits non-zero with `` `--guest` applies to `logos-scaffold build` ``.

## SPEL-004 — Generate client and FFI bindings

### Expected
`build client` regenerates the IDL first (it prints the same `Generating IDL via` line as
SPEL-003), then runs `make ffi-gen` with `SPEL_CLIENT_GEN` set to the vendored
`spel-client-gen` in the scaffold cache. That writes `<name>_client.rs`, `<name>_ffi.rs` and
`<name>.h` under `<name>_ffi/generated/`. Scaffold's own echo is just `$ make ffi-gen`; the
generator path shows up in make's echoed recipe line and must be the vendored one, not one
on `PATH`.
`build client [project-path]` works from outside the project.

### Preconditions
SPEL-002 (`setup` has built `spel-client-gen`).

### Steps
1. `"$SCAFFOLD_BIN" build client 2>&1 | tee /tmp/client.out`
2. `find ./*_ffi/generated -type f | sort`
3. `P=$PWD; rm -r "$P"/*_ffi/generated; ( cd / && "$SCAFFOLD_BIN" build client "$P" ); find "$P"/*_ffi/generated -type f | sort`

### Success criteria
- In step 1 the `Generating IDL via` line comes before the `$ make ffi-gen` echo.
- All three files exist, and the echoed generator path is inside the scaffold cache.
- Step 3 regenerates the same three files from `/`.

## SPEL-005 — Deploy and invoke instructions through spel

### Expected
`deploy` prints a `program_id`. After the signer is initialized for auth-transfer,
`lgs spel -- initialize --owner Public/<id>` is included in a block. `spel -- pda state` and
`spel -- inspect <pda> --type ProgramState` then show the PDA's state. `lgs spel --` always
uses the project wallet, whatever the shell exports.

With localnet down, the LEZ v0.2.4 wallet fails with `Error: Failed to find leader`. Only some
paths turn that into scaffold's hint. `deploy` probes the sequencer first and fails with
`cannot deploy programs: …` plus `sequencer appears unavailable at <addr>` and
``Run `logos-scaffold localnet start`.``. The `wallet --` and `spel --` passthroughs add no
hint: they forward the tool's own output, and `wallet --` ends with `error: wallet
passthrough command failed`.

### Preconditions
SPEL-002 with localnet running, and `"$SCAFFOLD_BIN" wallet -- check-health` succeeds.

### Steps
1. `"$SCAFFOLD_BIN" deploy`
2. `"$SCAFFOLD_BIN" wallet -- account new public` (note `Public/<id>`)
3. `"$SCAFFOLD_BIN" wallet -- auth-transfer init --account-id Public/<id>`
4. `"$SCAFFOLD_BIN" spel -- initialize --owner Public/<id>`
5. `"$SCAFFOLD_BIN" spel -- pda state` then `"$SCAFFOLD_BIN" spel -- inspect <pda> --type ProgramState`
6. `"$SCAFFOLD_BIN" localnet stop`, then `"$SCAFFOLD_BIN" deploy; echo "exit=$?"`
7. `"$SCAFFOLD_BIN" wallet -- check-health; echo "exit=$?"`
8. `"$SCAFFOLD_BIN" spel -- initialize --owner Public/<id>; echo "exit=$?"`, then
   `"$SCAFFOLD_BIN" localnet start` to restore it.

### Success criteria
- `deploy` prints `program_id:`, `initialize` reports inclusion, and `inspect` prints the
  state.
- No `Storage not found` or empty-wallet error appears.
- Step 6 exits non-zero with `cannot deploy programs:` and `sequencer appears unavailable at`
  naming the project's sequencer address.
- Steps 7 and 8 exit non-zero. Step 7 shows `Failed to find leader` and
  `wallet passthrough command failed`. Neither shows a scaffold hint. That is current
  behavior: record it, don't fail it.

### Notes
Run step 3 before the account signs anything. An account claimed by a spel program can
never send tokens, and this cannot be undone. CLI instruction names are kebab-case.

## SPEL-006 — Vendored spel project

### Expected
`new --template spel --vendor-deps` clones spel to `.scaffold/repos/spel` at the SHA in
`[repos.spel] pin`, with `path = ".scaffold/repos/spel"`. The clone happens after `spel init`
returns, so creation never fails with `Directory '<name>' already exists`.

### Preconditions
None.

### Steps
1. `"$SCAFFOLD_BIN" new dogfood-spel-vendored --template spel --vendor-deps`
2. `git -C dogfood-spel-vendored/.scaffold/repos/spel rev-parse HEAD`
3. `grep -A3 'repos.spel' dogfood-spel-vendored/scaffold.toml`

### Success criteria
- The vendored HEAD equals the recorded pin, and `path` is the relative vendored path.
