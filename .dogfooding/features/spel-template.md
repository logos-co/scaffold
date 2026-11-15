---
feature: spel-template
prefix: SPEL
surface: cli
scenarios:
  - id: SPEL-001
    title: Create a spel project with pinned versions
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 L1
  - id: SPEL-002
    title: Set up and build a spel project
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 L1
    depends_on: [SPEL-001]
  - id: SPEL-003
    title: Regenerate the IDL and the client bindings
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 L2
    depends_on: [SPEL-001]
  - id: SPEL-005
    title: Deploy and invoke instructions through spel
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 L4
    depends_on: [SPEL-002]
retired: [SPEL-004, SPEL-006]
---

# spel template

`new --template spel` (the README quick start) delegates generation to `spel init` from a
spel CLI that scaffold clones and builds at `DEFAULT_SPEL` in the cache. No global `spel` is
needed. spel projects build their guest through their own `Makefile` (`make build` = `cargo
risczero build` in Docker) and drive IDL and client generation through `make idl` /
`make ffi-gen`. `lgs spel -- …` runs the vendored binary against the project wallet. Related:
[Project creation](project-creation.md) (CREATE-004 covers `--vendor-deps`),
[Run pipeline](run-pipeline.md), [Deploy](deploy.md).

The spel guest has no local build mode. On a host without Docker, SPEL-001 and SPEL-003 run
locally; run SPEL-002 and SPEL-005 on a Docker-capable host (a GitHub-hosted runner works),
or cite the `template-e2e.yml` run for SPEL-002's build and report SPEL-005 as skipped.

## SPEL-001 — Create a spel project with pinned versions

### Expected
`new <name> --template spel` prints the pins it recorded (`Pinned LEZ: v0.2.4`, `Pinned spel:
v0.7.0` at the time of writing). It creates `spel.toml`, a `Makefile`, `<name>_core/`,
`<name>_ffi/` and `methods/guest/src/bin/<name>.rs`. `methods/guest/Cargo.toml` pins
`spel-framework` to the recorded tag and never to `branch = "main"`. Any `spel` on `PATH` is
irrelevant.

### Preconditions
None (the spel CLI is built during this step, minutes on a cold cache). On Linux, the PCSC
headers are installed.

### Steps
1. `cd "$SCRATCH_ROOT" && "$SCAFFOLD_BIN" new dogfood-spel --template spel`
2. `cd dogfood-spel && ls -d spel.toml Makefile dogfood_spel_core dogfood_spel_ffi methods/guest/src/bin; grep spel-framework methods/guest/Cargo.toml`

### Success criteria
- Exit 0, the pins are printed, and no `Directory '<name>' already exists` error appears.
- All listed paths exist, and `spel-framework` uses `tag = …`, not `branch`.

### Notes
`branch = "main"` in the generated manifests means the pin flags did not reach `spel init`:
its parser stops at the first non-flag argument, so the flags must come before the project
name (logos-co/spel#286).

## SPEL-002 — Set up and build a spel project

### Expected
`setup`, `localnet start` and `doctor` behave as they do for the default template, and
`doctor` reports zero FAIL. `build` runs `make build` (the Docker guest build, whatever
`[build].guest` says) and then IDL generation, leaving `<name>-idl.json` at the project
root. `build --guest local` prints `Note: --guest local does not apply to spel projects` and
still builds in Docker.

### Preconditions
SPEL-001, Docker, `cargo-risczero` 3.0.5, and the risc0 C++ toolchain.

### Steps
1. `"$SCAFFOLD_BIN" setup && "$SCAFFOLD_BIN" localnet start && "$SCAFFOLD_BIN" doctor`
2. `"$SCAFFOLD_BIN" build && ls ./*-idl.json methods/guest/target/riscv32im-risc0-zkvm-elf/docker/`
3. `P=$PWD; ( cd / && "$SCAFFOLD_BIN" build --guest local "$P" )`

### Success criteria
- `doctor` shows 0 FAIL, `build` exits 0, and the IDL plus the docker `.bin` exist.
- Step 3 prints the `--guest local` note, runs `make build` (Docker) and exits 0; the
  positional project path makes it work from `/`.

### Notes
The first build takes 5–15 minutes.

## SPEL-003 — Regenerate the IDL and the client bindings

### Expected
`build idl` prints ``Generating IDL via `make idl` (spel project)...``, runs `make idl`, and
the Makefile prints `✅ IDL written to <name>-idl.json`. The file named by `spel.toml`
`[program] idl` is valid JSON. No `idl/` directory is created, and no IDL JSON is printed to
the terminal in place of the file. `--guest` is rejected with `build idl`.

`build client` regenerates the IDL first, then runs `make ffi-gen` with `SPEL_CLIENT_GEN` set
to the vendored `spel-client-gen` in the scaffold cache, writing `<name>_client.rs`,
`<name>_ffi.rs` and `<name>.h` under `<name>_ffi/generated/`. Scaffold's own echo is just
`$ make ffi-gen`; the generator path shows in make's echoed recipe and must be the vendored
one, not one on `PATH`.

Both take `[project-path]` and work from outside the project.

### Preconditions
SPEL-001 and `"$SCAFFOLD_BIN" setup` in the project (it builds `spel-client-gen`). No Docker
needed. The first `build idl` compiles the project (about 5 minutes).

### Steps
1. `"$SCAFFOLD_BIN" build idl | tee /tmp/idl.out; jq -e . ./*-idl.json > /dev/null; ls -d idl; grep -c '^{' /tmp/idl.out`
2. `"$SCAFFOLD_BIN" build client 2>&1 | tee /tmp/client.out | grep -n -E 'Generating IDL via|make ffi-gen|spel-client-gen'; find ./*_ffi/generated -type f | sort`
3. `P=$PWD; rm -r ./*-idl.json ./*_ffi/generated; ( cd / && "$SCAFFOLD_BIN" build client "$P" ); ls "$P"/*-idl.json "$P"/*_ffi/generated`
4. `"$SCAFFOLD_BIN" build --guest docker idl; echo "exit=$?"`

### Success criteria
- Step 1: the `Generating IDL via` and `IDL written` lines appear, the file parses, `ls -d
  idl` fails and the `grep -c` prints `0`.
- Step 2: `Generating IDL via` comes before `$ make ffi-gen`, the generator path is inside
  the scaffold cache, and all three generated files exist.
- Step 3 recreates the IDL and the three files from `/`.
- Step 4 exits non-zero with `` `--guest` applies to `logos-scaffold build` ``.

### Notes
`expected IDL file …/idl/<stem>.json is missing` means something still resolves the IDL by
the old `<idl-dir>/<stem>.json` convention instead of `spel.toml`. `Skipping IDL build for
framework kind …` means the command ran in a non-spel project. `no such command: ffi-gen`
means something invoked `spel ffi-gen`, which does not exist. `vendored spel-client-gen
binary not found` means `setup` has not built it.

## SPEL-005 — Deploy and invoke instructions through spel

### Expected
`deploy` prints a `program_id`. After the signer is initialized for auth-transfer,
`lgs spel -- initialize --owner Public/<id>` is included in a block. `spel -- pda state` and
`spel -- inspect <pda> --type ProgramState` then show the PDA's state. `lgs spel --` always
uses the project wallet, whatever the shell exports. With localnet down, the `wallet --` and
`spel --` passthroughs forward the tool's own error (`Failed to find leader` from the LEZ
v0.2.4 wallet) without a scaffold hint, and `wallet --` ends with `error: wallet passthrough
command failed`; only `deploy` and `topup` add the sequencer-unavailable hint (DEPLOY-006,
WALLET-004).

### Preconditions
SPEL-002 with localnet running, and `"$SCAFFOLD_BIN" wallet -- check-health` succeeds.

### Steps
1. `"$SCAFFOLD_BIN" deploy`
2. `"$SCAFFOLD_BIN" wallet -- account new public` (note `Public/<id>`), then
   `"$SCAFFOLD_BIN" wallet -- auth-transfer init --account-id Public/<id>`
3. `"$SCAFFOLD_BIN" spel -- initialize --owner Public/<id>`
4. `"$SCAFFOLD_BIN" spel -- pda state` then `"$SCAFFOLD_BIN" spel -- inspect <pda> --type ProgramState`
5. `"$SCAFFOLD_BIN" localnet stop; "$SCAFFOLD_BIN" wallet -- check-health; echo "exit=$?"; "$SCAFFOLD_BIN" spel -- initialize --owner Public/<id>; echo "exit=$?"; "$SCAFFOLD_BIN" localnet start`

### Success criteria
- `deploy` prints `program_id:`, `initialize` reports inclusion, and `inspect` prints the
  state. No `Storage not found` or empty-wallet error appears.
- Step 5: both commands exit non-zero with the tool's own error and no scaffold hint.

### Notes
Run the `auth-transfer init` before the account signs anything: an account claimed by a spel
program can never send tokens, and this cannot be undone. CLI instruction names are
kebab-case.
