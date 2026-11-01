---
feature: setup-and-build
prefix: BUILD
surface: cli
scenarios:
  - id: BUILD-001
    title: setup builds the project-local toolchain
    priority: P0
    provenance: documented
    source: DOGFOODING.md D1
    depends_on: [CREATE-001]
  - id: BUILD-002
    title: Default build produces deterministic guest binaries
    priority: P0
    provenance: documented
    source: DOGFOODING.md D8
    depends_on: [BUILD-001]
  - id: BUILD-003
    title: Missing Docker toolchain fails fast with an escape hatch
    priority: P1
    provenance: documented
    source: DOGFOODING.md D8
    depends_on: [CREATE-001]
  - id: BUILD-004
    title: Deterministic guest build is byte-reproducible
    priority: P1
    provenance: documented
    source: DOGFOODING.md D8
    depends_on: [BUILD-002]
  - id: BUILD-005
    title: Local guest build opt-out
    priority: P1
    provenance: documented
    source: DOGFOODING.md D8
    depends_on: [BUILD-001]
  - id: BUILD-006
    title: setup --prebuilt installs only a verified sequencer
    priority: P2
    provenance: documented
    source: DOGFOODING.md D1
    depends_on: [CREATE-001]
retired: []
---

# Setup and build

`setup` syncs the pinned LEZ and spel and builds `sequencer_service`, `wallet` and `spel`
into the project-local cache checkout. It also materializes the pinned circuits and seeds a
default wallet. `build` chains `setup`, builds the workspace, and builds the risc0 guests in
the mode set by `[build].guest`. That is `docker` (default, reproducible `program_id`) or
`local` (host toolchain). The last `build` decides what `deploy` ships.
Related: [Deploy](deploy.md), [Localnet](localnet.md), [Diagnostics](diagnostics.md).

## BUILD-001 — setup builds the project-local toolchain

### Expected
`setup` checks out LEZ and spel at their pins. It builds `sequencer_service`, `wallet` and
`spel` inside the cache checkout (or the vendored path) and materializes the `[circuits]`
release into `.scaffold/circuits`, then prints `setup complete`. Default-wallet seeding ends
in one of these lines, and each is a pass: `default wallet seeded from preconfigured
account`, `default wallet seeded by initializing wallet storage …`, or (with no localnet
running, LEZ v0.2.4+) `default wallet seeding deferred: wallet commands need a running
sequencer …`. Nothing is installed to `PATH`.

### Preconditions
A project from CREATE-001. On Linux, the PCSC headers are installed.

### Steps
1. `cd "$SCRATCH_ROOT/dogfood-default" && "$SCAFFOLD_BIN" setup`
2. `LEZ=$("$SCAFFOLD_BIN" test-node pins --json | jq -r .lez_checkout); ls "$LEZ/target/release/"{sequencer_service,wallet}`
3. `ls .scaffold/circuits/pol/verification_key.json`
4. `which wallet spel sequencer_service`

### Success criteria
- Exit 0 and `setup complete`.
- Both LEZ binaries and the circuits verification key exist.
- None of the binaries comes from the scaffold cache via `PATH`.
- The output never contains `warning: could not seed default wallet automatically`.

### Notes
If seeding was deferred, the next `build` or `run` with localnet up prints the seeded line.

## BUILD-002 — Default build produces deterministic guest binaries

### Expected
With no `[build]` section, `build` runs `setup`, then `cargo build --workspace`, then prints
`Building guest methods (deterministic, risc0-guest-builder:<tag>)...` followed by risc0's
`ELFs ready at: ImageID: <hex> - <path>` lines. It leaves one `<program>.bin` per program
under `target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker/`.

### Preconditions
BUILD-001, a running Docker daemon, and `cargo-risczero` on `PATH`.

### Steps
1. `"$SCAFFOLD_BIN" build`
2. `ls target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker/`

### Success criteria
- Exit 0.
- `hello_world.bin` and the other template programs are listed.

### Notes
The first run pulls about 1.7 GB of image and takes several minutes. Slowness is not a
failure.

## BUILD-003 — Missing Docker toolchain fails fast with an escape hatch

### Expected
In default (`docker`) mode, a missing `cargo-risczero`, a missing `docker`, or a daemon that
does not answer stops `build` before `setup` compiles anything and before any image pull. The
message names the missing piece and offers `[build].guest = "local"`.

### Preconditions
Docker daemon stopped (or `cargo-risczero` removed from `PATH`).

### Steps
1. `"$SCAFFOLD_BIN" build; echo "exit=$?"`

### Success criteria
- Non-zero exit within seconds, with no cargo compile output.
- The message names the missing piece and mentions `guest = "local"`.

## BUILD-004 — Deterministic guest build is byte-reproducible

### Expected
Two docker-mode builds of unchanged source, the second from a clean artefact tree, produce
byte-identical `.bin` files.

### Preconditions
BUILD-002.

### Steps
1. `D=target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker; shasum -a 256 $D/*.bin > /tmp/first.sha`
2. `rm -rf target/riscv-guest-docker && "$SCAFFOLD_BIN" build`
3. `shasum -a 256 -c /tmp/first.sha`

### Success criteria
- Every line of step 3 reports `OK`.

### Notes
CI also runs this (`.github/workflows/guest-reproducibility.yml`). A comparison across
machines is only meaningful with the same `Cargo.lock`.

## BUILD-005 — Local guest build opt-out

### Expected
`build --guest local`, or `[build].guest = "local"`, builds guests with the host toolchain
into `target/riscv-guest/.../release/` without needing Docker. It prints `Clearing
deterministic guest artefacts in …` and removes `target/riscv-guest-docker`. `doctor`
reports `local — … not reproducible`. The explicit choice prints no nag during `build`.

### Preconditions
BUILD-001 and the risc0 Rust toolchain installed.

### Steps
1. `"$SCAFFOLD_BIN" build --guest local`
2. `ls target/riscv-guest-docker 2>&1; find target/riscv-guest -name '*.bin' -path '*release*'`
3. `printf '\n[build]\nguest = "local"\n' >> scaffold.toml && "$SCAFFOLD_BIN" doctor | grep "guest build"`

### Success criteria
- Step 2 shows no docker tree and lists the release `.bin` files.
- The step 3 row reads `PASS | guest build | local …` and says it is not reproducible.

## BUILD-006 — setup --prebuilt installs only a verified sequencer

### Expected
`setup --prebuilt` downloads `sequencer_service` instead of building it, and installs it only
if it matches the published `sequencer_service.sha256`. A missing, empty or malformed
checksum prints `not installing an unverified binary` and falls back to the source build. A
digest mismatch is a hard error. `wallet` is always built from source.

### Preconditions
A fresh project (CREATE-001).

### Steps
1. `"$SCAFFOLD_BIN" setup --prebuilt`

### Success criteria
- Either the download is verified and installed, or the unverified warning appears followed
  by a source build.
- An unverified binary never lands in `target/release/`.
