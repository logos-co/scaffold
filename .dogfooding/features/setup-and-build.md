---
feature: setup-and-build
prefix: BUILD
surface: cli
scenarios:
  - id: BUILD-001
    title: setup builds the project-local toolchain
    priority: P0
    provenance: documented
    source: DOGFOODING.md@740e5e9 D1
    depends_on: [CREATE-001]
  - id: BUILD-002
    title: Default build produces deterministic guest binaries
    priority: P0
    provenance: documented
    source: DOGFOODING.md@740e5e9 D8
    depends_on: [BUILD-001]
  - id: BUILD-003
    title: Missing Docker toolchain fails fast with an escape hatch
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D8
    depends_on: [CREATE-001]
  - id: BUILD-004
    title: Docker guest builds are byte-reproducible and their ids differ from local builds
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D8
    depends_on: [BUILD-002, LOCALNET-001]
  - id: BUILD-005
    title: Local guest build opt-out
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 D8
    depends_on: [BUILD-001]
  - id: BUILD-006
    title: setup --prebuilt installs only a verified sequencer
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 D1
    depends_on: [CREATE-001]
retired: [BUILD-007]
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
2. `LEZ=$("$SCAFFOLD_BIN" test-node pins --json | jq -r .lez_checkout); ls "$LEZ/target/release/"{sequencer_service,wallet} .scaffold/circuits/pol/verification_key.json`
3. `which wallet spel sequencer_service`

### Success criteria
- Exit 0 and `setup complete`. Both LEZ binaries and the circuits verification key exist.
- Step 3 finds nothing from the scaffold cache.
- The output never contains `warning: could not seed default wallet automatically`.

### Notes
If seeding was deferred, the next `setup`, `build` or `run` with localnet up prints the
seeded line. Cold durations are under "Long steps" in [product.md](../product.md).

## BUILD-002 — Default build produces deterministic guest binaries

### Expected
With no `[build]` section, `build` runs `setup`, then `cargo build --workspace`, then prints
`Building guest methods (deterministic, risc0-guest-builder:<tag>)...` followed by risc0's
`ELFs ready at: ImageID: <hex> - <path>` lines. It leaves one `<program>.bin` per program
under `target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker/`. `build [project-path]`
builds the same project from outside it.

### Preconditions
BUILD-001, a running Docker daemon, and `cargo-risczero` on `PATH`.

### Steps
1. `"$SCAFFOLD_BIN" build 2>&1 | tee /tmp/build1.log`
2. `ls target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker/`
3. `(cd .. && "$SCAFFOLD_BIN" build "$OLDPWD")`

### Success criteria
- Exit 0, and `hello_world.bin` and the other template programs are listed.
- Step 3 exits 0 with no "must be run inside a logos-scaffold project" error.

### Notes
The first run pulls about 1.7 GB of image. `Cargo.lock not found in path
…/methods/guest/Cargo.lock` on stderr is expected: scaffold projects keep one lockfile at the
workspace root. The container builds `--locked`, so a stale root `Cargo.lock` is a fatal,
different error; re-run a plain `build` and report it if that does not clear it. Two other
signatures to report: an MSRV error naming `rustc 1.88.0-dev` (the build used risc0's default
image, not scaffold's pinned tag), and a build that leaves extension-less ELFs but no `.bin`
(risc0's output layout moved; capture `ls -R target/riscv-guest-docker`).

## BUILD-003 — Missing Docker toolchain fails fast with an escape hatch

### Expected
In default (`docker`) mode, a missing `cargo-risczero`, a missing `docker`, or a daemon that
does not answer stops `build` before `setup` compiles anything and before any image pull. The
message names the missing piece and offers `[build].guest = "local"`. `doctor`'s `guest
build` row agrees.

### Preconditions
A project without `[build].guest`. Docker daemon stopped for steps 1–2 (skip them on a host
where it must stay up; step 3 works anywhere).

### Steps
1. `"$SCAFFOLD_BIN" build; echo "exit=$?"`
2. `"$SCAFFOLD_BIN" doctor | grep "guest build"`
3. `env PATH=<PATH without the dir holding cargo-risczero> "$SCAFFOLD_BIN" build; echo "exit=$?"`

### Success criteria
- Steps 1 and 3 exit non-zero within seconds, with no cargo compile output, and mention
  `guest = "local"`. Step 1 says the Docker daemon is not running (`docker info` failed);
  step 3 says ``needs `cargo-risczero` on PATH`` with the `rzup install cargo-risczero` hint.
- Step 2 is a `FAIL | guest build | docker (risczero/risc0-guest-builder:<tag>)` row with the
  same reason (`… not found on PATH`, `Docker daemon not running`, or `did not answer within
  10s` for a hanging daemon). A PASS row next to a refusing `build` is the regression.

## BUILD-004 — Docker guest builds are byte-reproducible and their ids differ from local builds

### Expected
Two docker-mode builds of unchanged source, the second from a clean artefact tree and an
empty container build cache, produce byte-identical `.bin` files. The `program_id` that
`deploy --json` reports for a docker build equals the `ImageID` risc0 printed for that
program during the build, and differs from the `program_id` of a `--guest local` build of
the same source.

### Preconditions
BUILD-002 and localnet up (LOCALNET-001).

### Steps
1. `D=target/riscv-guest-docker/riscv32im-risc0-zkvm-elf/docker; shasum -a 256 $D/*.bin > /tmp/first.sha`
2. `rm -rf target/riscv-guest-docker && docker builder prune -af`
3. `"$SCAFFOLD_BIN" build 2>&1 | tee /tmp/second.log`
4. `grep -E '^#[0-9]+ CACHED' /tmp/second.log; shasum -a 256 -c /tmp/first.sha`
5. `grep ImageID /tmp/second.log; "$SCAFFOLD_BIN" deploy --json | tee /tmp/ids-docker.json`
6. `"$SCAFFOLD_BIN" build --guest local && "$SCAFFOLD_BIN" deploy --json | tee /tmp/ids-local.json`,
   then `jq -r '.deploys[] | "\(.program) \(.program_id)"' /tmp/ids-docker.json /tmp/ids-local.json`.
   Finish with a plain `"$SCAFFOLD_BIN" build` so later scenarios deploy the docker artefacts.

### Success criteria
- Step 4's `grep` prints nothing (no build step came from cache) and every `shasum` line is
  `OK`. A run that shows `CACHED` steps has not tested reproducibility: report it as not run.
- Step 5: each program's `program_id` equals its `ImageID`.
- Step 6: each program's local `program_id` differs from its docker one, and the local build
  prints `Clearing deterministic guest artefacts in …`.

### Notes
Without step 2's prune, BuildKit replays every step from cache and re-exports the first
build's ELFs. The prune clears the host's whole BuildKit cache (pulled images stay); on
Podman use its equivalent. CI runs the same check in
`.github/workflows/guest-reproducibility.yml` (path-gated, weekly, and `workflow_dispatch`);
on a host without Docker cite a run of it and say so. Compare across machines only with the
same `Cargo.lock`: the container builds `--locked`, and two machines that generated their own
lockfiles legitimately get different ids. The same id in both modes usually means `deploy`
picked the same artefact twice; a `deploy <program>` without `--json` echoes the path.

## BUILD-005 — Local guest build opt-out

### Expected
`build --guest local`, or `[build].guest = "local"`, builds guests with the host toolchain
into `target/riscv-guest/.../release/` without needing Docker, and removes an existing
`target/riscv-guest-docker` tree. `doctor` reports `local — … not reproducible`, and its
`container runtime` row is a PASS (`not needed with [build].guest = "local"`) even with no
container runtime. `deploy` then ships the `release/` artefact.

### Preconditions
BUILD-001 and the risc0 Rust toolchain installed. Localnet up for step 4.

### Steps
1. `"$SCAFFOLD_BIN" build --guest local`
2. `ls target/riscv-guest-docker 2>&1; find target/riscv-guest -name '*.bin' -path '*release*'`
3. `printf '\n[build]\nguest = "local"\n' >> scaffold.toml && "$SCAFFOLD_BIN" doctor | grep -E "guest build|container runtime"`
4. `"$SCAFFOLD_BIN" deploy hello_world 2>&1 | grep 'deploy-program'`

### Success criteria
- Step 2 shows no docker tree and lists one release `.bin` per program.
- Step 3 reads `PASS | guest build | local …` and says it is not reproducible; the
  `container runtime` row is not a WARN about failing guest builds.
- Step 4's echoed `wallet deploy-program <path>` names
  `target/riscv-guest/…/release/hello_world.bin`.

### Notes
On a host without Docker this is the build every deploying scenario uses; say so in the
report. The first build also installs the template's host Rust toolchain through rustup.

## BUILD-006 — setup --prebuilt installs only a verified sequencer

### Expected
`setup --prebuilt` downloads `sequencer_service` instead of building it, and installs it only
if it matches the published `sequencer_service.sha256`. A missing, empty or malformed
checksum prints `not installing an unverified binary` and falls back to the source build. A
digest mismatch is a hard error. With no release published for the pin it prints `no prebuilt
published yet for tag lssa-prebuilt-<sha>-<arch>-<os>, falling back to source build`.
`wallet` is always built from source. `build --prebuilt` passes the flag to its `setup`.

### Preconditions
A fresh project on a cache root where the pin's sequencer is not built yet (a new
`--cache-root`), so the prebuilt path is taken.

### Steps
1. `env PATH="$NOCARGO" "$SCAFFOLD_BIN" setup --prebuilt; echo "exit=$?"` (`NOCARGO`: see "Rules
   for a run" in [product.md](../product.md))

### Success criteria
- One of the three outcomes above is printed after `Checking for prebuilt binaries (tag: …)`.
- An unverified binary never lands in `<lez>/target/release/`.

### Notes
At the current default pin no prebuilt is published, so the outcome is the fallback and the
verification path is not exercised: say so in the report. Without `cargo` the command prints
its decision and then stops by itself at the first build (non-zero exit), which is all this
scenario needs: BUILD-001 covers the source build. `build --prebuilt` passes the flag to the
same code.
