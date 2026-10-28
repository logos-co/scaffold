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
root. `build --guest local` prints a note that spel always builds in Docker.

### Preconditions
SPEL-001, Docker, `cargo-risczero` 3.0.5, and the risc0 C++ toolchain.

### Steps
1. `"$SCAFFOLD_BIN" setup && "$SCAFFOLD_BIN" localnet start && "$SCAFFOLD_BIN" doctor`
2. `"$SCAFFOLD_BIN" build`
3. `ls ./*-idl.json methods/guest/target/riscv32im-risc0-zkvm-elf/docker/`

### Success criteria
- `doctor` shows 0 FAIL, `build` exits 0, and the IDL plus the docker `.bin` exist.

### Notes
The first build takes 5–15 minutes, which is normal.

## SPEL-003 — Regenerate the IDL

### Expected
`build idl` prints `✅ IDL written to <name>-idl.json` and writes the file named by
`spel.toml` `[program] idl` as valid JSON matching the program. It writes no `idl/` directory
and prints no IDL JSON to the terminal in place of the file.

### Preconditions
SPEL-001 with a working build environment.

### Steps
1. `"$SCAFFOLD_BIN" build idl`
2. `jq -e . ./*-idl.json > /dev/null`

### Success criteria
- The `IDL written` line appears, and the file parses as JSON.

## SPEL-004 — Generate client and FFI bindings

### Expected
`build client` regenerates the IDL first, then writes `<name>_client.rs`, `<name>_ffi.rs` and
`<name>.h` under `<name>_ffi/generated/`. The echoed generator command points at the vendored
`spel-client-gen` in the scaffold cache, not one on `PATH`.

### Preconditions
SPEL-002 (`setup` has built `spel-client-gen`).

### Steps
1. `"$SCAFFOLD_BIN" build client`
2. `find ./*_ffi/generated -type f | sort`

### Success criteria
- All three files exist, and the echoed generator path is inside the scaffold cache.

## SPEL-005 — Deploy and invoke instructions through spel

### Expected
`deploy` prints a `program_id`. After the signer is initialized for auth-transfer,
`lgs spel -- initialize --owner Public/<id>` is included in a block. `spel -- pda state` and
`spel -- inspect <pda> --type ProgramState` then show the PDA's state. `lgs spel --` always
uses the project wallet, whatever the shell exports.

### Preconditions
SPEL-002 with localnet running.

### Steps
1. `"$SCAFFOLD_BIN" deploy`
2. `"$SCAFFOLD_BIN" wallet -- account new public` (note `Public/<id>`)
3. `"$SCAFFOLD_BIN" wallet -- auth-transfer init --account-id Public/<id>`
4. `"$SCAFFOLD_BIN" spel -- initialize --owner Public/<id>`
5. `"$SCAFFOLD_BIN" spel -- pda state` then `"$SCAFFOLD_BIN" spel -- inspect <pda> --type ProgramState`

### Success criteria
- `deploy` prints `program_id:`, `initialize` reports inclusion, and `inspect` prints the
  state.
- No `Storage not found` or empty-wallet error appears.

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
