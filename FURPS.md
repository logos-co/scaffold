# Scaffold — FURPS+

## FURPS+

### Functionality

1. One public DevNet vertical slice: generate wallet, fund wallet, deploy contract, execute one transaction type, verify result.
2. Integrate wallet generation as part of the scaffold workflow for bootstrap and interaction flows.
3. Support native token topup for wallet operations on local and DevNet environments.
4. Deploy command auto-discovers program binaries from `methods/target/` by matching program names, so non-template projects deploy without `--program-path`.
5. Build command auto-compiles `methods/Cargo.toml` when present, so projects whose parent workspace excludes the Risc0 guest crate produce guest binaries via `lgs build` without a separate `cargo build --manifest-path methods/Cargo.toml`.
6. Deploy outputs the deployed program's on-chain ID (the risc0 image ID) for every successful submission, in both the human-readable output and the `--json` output, so users can hand the value to a client without rerunning a separate inspection tool. The value is computed locally from the submitted ELF; on-chain inclusion verification is future work and depends on LEZ exposing deploy receipts.
7. Scaffold vendors the `spel` CLI per project — clones `logos-co/spel` to a project-local path, pinned via `[repos.spel]` in `scaffold.toml`, and builds it during `setup` — mirroring the LEZ vendoring pattern. `deploy` invokes the project-local binary; no global `spel` install is required. The default spel pin is selected so spel itself vendors the same LEZ commit scaffold pins; `doctor` enforces this alignment at runtime by inspecting `spel-cli/Cargo.toml` and warning if spel's vendored LEZ diverges from `DEFAULT_LEZ_PIN`.
8. `logos-scaffold spel -- <args...>` (and the `lgs spel -- <args...>` alias) proxies trailing arguments to the project-vendored `spel` binary, so any spel command (`inspect`, `pda`, `generate-idl`, etc.) runs against the project's pinned version without a global install. Exit codes are forwarded.
9. `logos-scaffold deploy --json` output is a pure JSON object on stdout — no command-echo, no informational text — and absent values are omitted from the object rather than emitted as `null`. Consumers can pipe directly to `jq` and test field presence with `has(...)`. Subprocess command-echoes and progress messages stay off the JSON channel. Two shapes by code path: `--program-path … --json` emits a bare object `{"status":"submitted","program":...,"tx"?:...,"program_id"?:...}`; auto-discovery `--json` emits `{"deploys":[<entry>,...]}` with one entry per attempted program. Failed entries replace `program_id` with `error`. Auto-discovery exit code is non-zero when any entry failed; the JSON object is still emitted so consumers can inspect partial results.

### Usability

1. Single command bootstrap with no manual project wiring required.
2. Generated layout clearly separates contract code, client code, config, and deploy scripts.
3. Deterministic wallet generation and .env handling for repeatability.
4. Clear happy-path docs, reproducible setup, discoverable commands.
5. CLI prints underlying commands for each step so users can drop down to lower-level tooling.

### Reliability

1. The vertical slice must succeed 3 times in a row on a clean machine with deterministic wallets.
2. Local network can be started and torn down in isolation without modifying host-global blockchain state.

### Performance

1. Each workshop step must complete within a demo-tolerable threshold (a few minutes).

### Supportability

1. Scaffold version and toolchain versions are explicit in generated output so projects remain buildable over time.
2. Network configuration for local and DevNet deployment is .env based config.
3. The scaffolded project includes command references for build, deploy, and interaction steps.
4. `logos-scaffold doctor` reports the `spel` repo presence and pin status, mirroring the existing LEZ checks, so drift from `DEFAULT_SPEL_PIN` is surfaced before it bites at deploy time.
5. `scaffold.toml` files predating the `[repos.spel]` section produce a targeted error from the config loader pointing at `logos-scaffold init` as the fix; `init` is safe to re-run and back-fills the missing section without overwriting customized fields.

### + (Privacy, Anonymity, Censorship-Resistance)

- Local workflow does not require uploading source code, artifacts, or private keys to third-party services.
- CLI interaction flow works with locally controlled wallet keys and does not require custodial key management.
- Local development and testing can run fully offline from public networks.
- DevNet interaction uses explicit wallet and RPC configuration so developers can avoid accidental cross-network key reuse.

### Dependencies

#### Internal Dependencies

- Logos Core DevEx for overall developer journey alignment and terminology.
- Logos Blockchain and Logos Execution Environment for functionality.
- Wallet Module for interactions with Logos Execution Environment.
- `logos-co/spel` CLI — vendored per project at a pinned commit (`DEFAULT_SPEL_PIN`, currently tag `v0.2.0`); supplies the `spel inspect` output that `deploy` parses for the program ID.

#### Runtime Dependencies

- Local network runtime availability for local deploy and interaction workflows.
- DevNet RPC endpoint availability and stable chain configuration.
- Deterministic local/DevNet account and chain configuration via environment files.

#### Wallet Dependencies

- Wallet available for signing transactions initiated by CLI interaction commands.
- Network-aware wallet configuration to prevent cross-network key misuse.

## FURPS+ — `lgs run`

### Functionality

1. `lgs run` collapses the inner-loop sequence — build → IDL build → ensure-localnet → wallet topup → deploy → optional post-deploy hooks — into one command. Every step's failure aborts the pipeline with a numbered step header (`[3/N] …`) so the failing phase is unambiguous in console output.
2. Source edits drive fresh on-chain program identity automatically: when the guest ELF changes, its risc0 image ID changes, and the new program's storage starts empty. Scaffold relies on this for the default cycle and adds no per-run reset.
3. Post-deploy hooks: `[run].post_deploy` is a list of shell commands executed in order via `sh -c` with `cwd` set to the project root. Hooks see a documented env contract: `SEQUENCER_URL`, `NSSA_WALLET_HOME_DIR`, `SCAFFOLD_PROJECT_ROOT`, `SCAFFOLD_IDL_DIR`, plus single-program shortcuts `SCAFFOLD_PROGRAM_ID` / `SCAFFOLD_GUEST_BIN` (set only when exactly one program is deployable).
4. CLI overrides: `--post-deploy <cmd>` (repeatable) replaces `[run].post_deploy` for one invocation. `--no-post-deploy` skips hooks entirely. The two flags conflict and are rejected at clap parse time.
5. Localnet reuse: if a managed sequencer is already running, the run reuses it. If the configured port is held by an unrelated process, the run aborts with a diagnostic naming the foreign PID.
6. Topup safety: a wallet-topup confirmation timeout aborts before deploy so the developer is never left wondering whether deploy used a half-funded wallet.

### Usability

1. The command produces a single human-readable output stream with numbered step headers and one-line summaries per phase. No JSON output flag — `--json` is reserved for `deploy`'s programmatic consumers.
2. The single-program shortcuts (`SCAFFOLD_PROGRAM_ID` / `SCAFFOLD_GUEST_BIN`) cover the most common dogfooding shape (one guest program per project) without leaking ambiguous values into multi-program projects — they're unset when the project has more than one deployable program.
3. Hook log markers (`===> post_deploy[i/n]:` and `<=== post_deploy[i/n] OK`) frame each hook's stdout for grep-friendly log reading.

### Reliability

1. The conflicting flag pair (`--post-deploy --no-post-deploy`) is rejected at parse time, not silently coerced.
2. The pipeline anchors itself at the discovered project root: `lgs run` from a subdirectory builds and deploys from the project root, not from cwd.

### Performance

1. The run is bounded by the underlying tools (cargo build, IDL test harness, sequencer startup, wallet topup, wallet deploy-program); scaffold adds no waiting steps beyond what each underlying command already imposes.
2. Single-program metadata (program ID, guest binary path) is resolved once per invocation and reused across every post-deploy hook, so multiple hooks don't multiply `spel inspect` cost.

### Supportability

1. `[run]` round-trips cleanly through `parse_config` / `serialize_config`. Default values are omitted from the serialized output to keep diffs minimal.
2. The hook env contract is documented in `README.md` and validated by unit and integration tests in `src/commands/run.rs::tests` and `tests/cli.rs`.
3. Flag-conflict rejection messages list the conflicting flags and exit non-zero, matching clap's standard error format.

### + (Privacy, Anonymity, Censorship-Resistance)

- Hooks run locally with the developer's own wallet; no network egress beyond what the deploy step already needs.
- Post-deploy hooks have direct access to the deployer's wallet home via `NSSA_WALLET_HOME_DIR`. Hooks are user-authored and trusted — same threat model as `scaffold.toml` itself.

### Dependencies

#### Internal Dependencies

- `cmd_build_shortcut` for the build phase.
- `build_idl_for_current_project` for IDL generation (no-op for non-lez-framework projects).
- `cmd_localnet` (start) for localnet lifecycle when no managed sequencer is already running.
- `cmd_wallet_topup_inner` for the topup phase.
- `cmd_deploy` for deploy submission and `extract_program_id` for image-ID extraction.

## FURPS+ — Basecamp

### Functionality

1. Fetch and build a pinned basecamp (`nix build '.#app'`) and pinned `lgpm` as project-local artifacts, in the same pin-isolated cache layout used for LEZ.
2. Pre-seed two isolated basecamp profiles (`alice`, `bob`) per project for p2p dogfooding.
3. Build and install the project's `.lgx` module(s) into one or both profiles via `lgpm`, with source resolution that follows the `.#lgx` flake-output convention used by existing modules.
4. Launch basecamp for a named profile with clean-slate semantics: kill any prior process tree for that profile, scrub the profile directory, reinstall recorded `.lgx` sources, and `exec` basecamp with profile-scoped `XDG_*` environment.
5. Set per-profile values for each module's documented port-override env vars on `launch` (names owned by each module), so multiple profiles can coexist without port collisions on the same machine.
6. `basecamp build-portable` builds the project's `.#lgx-portable` flake outputs (the variant that loads cleanly into a release basecamp AppImage), orders them topologically by `metadata.json` dependencies so leaves load first, symlinks the results into `<project>/.scaffold/basecamp/portable/` with names carrying the load order, and prints those symlink paths. The wipe-and-recreate on every run keeps the staging dir idempotent.
7. Source resolution for `build-portable` reuses the same auto-discovery + `--path` / `--flake` escape hatches as `install`, but targets `#lgx-portable` instead of `#lgx`.
8. `scaffold.toml` gains one `[modules.<module_name>]` sub-section per captured module, with `flake` and `role` (`project` | `dependency`) fields. The collection of these sub-sections is the sole source of truth for the captured module set; `basecamp.state` holds only derived artefacts (pin outputs, binaries). Sub-section form fits scaffold's existing line-oriented TOML parser — no inline tables.
9. `basecamp modules` writes `[modules]` during capture. For each captured source, the command derives `module_name` as follows:
   - `path:` flake ref → read `<flake-path>/metadata.json`, use `.name`. Deterministic.
   - `.lgx` file path → read `metadata.json` from the sibling directory if present; otherwise fall back to the filename stem.
   - `github:` flake ref → heuristic: strip `logos-` prefix from the repo stem, replace `-` with `_`. Printed at capture time with an assumption note (see Usability 7).
10. Dep resolution walks each project source's `metadata.json` `dependencies` array and, for each declared name:
    - Already keyed in `[modules]` → no-op (already covered, irrespective of role).
    - In `BASECAMP_PREINSTALLED_MODULES` → no-op (basecamp ships it).
    - Not covered → resolve a flake ref via the declaring source's `flake.lock`, then the scaffold-default pin table. On success, insert into `[modules]` with `role = "dependency"`.
    - Unresolved after all fallbacks → fail with a targeted error naming the two user-side fixes (capture as project source, or add an explicit dependency entry). No silent skip.
11. `[basecamp.dependencies]` (the legacy override table) is removed. Its role is subsumed by explicit `role = "dependency"` entries in `[modules]`.

### Usability

1. `basecamp setup` is opt-in — it is never triggered implicitly by `new`, the top-level `setup`, or `build`.
2. When `install` or `launch` run without prior `basecamp setup`, the CLI prints a single one-line hint pointing at the required command instead of erroring with a raw subprocess trace.
3. When only `.#lgx-portable` is found on a project, the CLI fails explicitly, names the missing `.#lgx` output, and suggests `--flake <ref>#lgx-portable` for explicit opt-in.
4. Commands follow the existing `logos-scaffold` CLI idioms (subcommand groups, `--help` output, project-context errors).
5. Projects exposing only `.#lgx` (no `.#lgx-portable`) receive a targeted hint naming the missing attribute and suggesting `--flake <ref>#lgx-portable` for explicit opt-in — mirror of the `install` failure mode, in reverse.
6. `build-portable` stages a user-facing mirror of every built artefact as a symlink under `<project>/.scaffold/basecamp/portable/<NN>-<module_name>.lgx`. The two-digit `NN` is the load-order index so a file-browser lists the artefacts in the exact order basecamp needs to load them — the AppImage's "install lgx" picker sees human-named files in the right order rather than opaque `/nix/store/…-source/…` paths. Nix's own `./result-lgx-portable` symlinks still land next to each flake; the scaffold-owned dir is a separate concern layered on top.
7. For each `github:` flake where scaffold derives `module_name` from the repo slug, `basecamp modules` prints exactly one assumption note at capture time: the flake ref and the inferred `module_name`, with "edit `[modules]` in scaffold.toml if wrong." One-time UX cost, never repeats.
8. `scaffold.toml` is human-editable at all times. `basecamp modules` is idempotent: if a key already exists in `[modules]`, its `module_name` and `role` are preserved (user intent wins over auto-derivation).
9. Unresolved dep diagnostics are a fail-fast error at `basecamp modules` time — the dep name must resolve to an entry in `[modules]`, a `metadata.json` source flake-input pin, the scaffold default pin table, or the basecamp preinstall list, otherwise the command exits non-zero before writing any state. No warn-and-skip path.
10. No migration path: the whole `basecamp` subcommand is unreleased. Users on earlier iterations re-run `basecamp modules` against a fresh scaffold.toml.

### Reliability

1. Two `basecamp launch` invocations for different profiles on the same machine run concurrently without colliding on XDG paths, p2p identity keys, or module ports (subject to modules honoring the external port-override contract).
2. `basecamp setup` is idempotent when the pinned commit is unchanged: no rebuild, no reseeding, no state mutation.
3. `basecamp launch <profile>` produces a reproducible profile state — clean-slate on every invocation.
4. `rm -rf` during scrub targets only paths under `<project>/.scaffold/basecamp/profiles/<name>/`.
5. `build-portable` writes only under `<project>/.scaffold/basecamp/portable/` (a wiped-and-recreated staging dir of symlinks into the nix store), never invokes `lgpm`, and never touches `basecamp.state` or the `alice`/`bob` profile trees — so a failed portable build cannot corrupt install/launch state.
6. Dep resolution is deterministic given the same `scaffold.toml` and source `metadata.json` files — no reliance on github repo naming conventions, no string substring matches, no ordering dependencies.
7. `basecamp modules` writes to `scaffold.toml` atomically (write-temp-then-rename) so a crash mid-write cannot corrupt an otherwise-valid scaffold.toml.
8. Re-running `basecamp modules` with an unchanged project set is a no-op against `scaffold.toml` contents; hashes of the serialized section match byte-for-byte on re-entry.

### Performance

1. `basecamp install` completes in the low-seconds range with a warm Nix cache; cold first-run wall-clock is bounded by upstream `nix build '.#lgx'` time.
2. `basecamp setup` first-run wall-clock is bounded by upstream basecamp + `lgpm` build time; re-runs on unchanged pin are effectively instant.

### Supportability

1. `logos-scaffold doctor` gains a basecamp section when `.scaffold/basecamp/` exists, covering binary presence, profile integrity, and installed-module state.
2. Basecamp and `lgpm` pinned commits are explicit in `scaffold.toml`.
3. `.scaffold/state/basecamp.state` is plain-text and line-oriented, matching existing scaffold state conventions.
4. Dogfooding scenarios (`B1`–`B4` in `DOGFOODING.md`) cover setup, single-instance, multi-instance p2p, and clean-slate behaviors.
5. `build-portable`'s manual load-into-AppImage step is explicit: scaffold stages browsable symlinks under `.scaffold/basecamp/portable/` but does not know or auto-feed the AppImage's install dialog. The AppImage lifecycle is intentionally outside scaffold's scope — see ADR "AppImage Path is Outside Scaffold's Scope".
6. Known limitation: multi-sub-flake projects must unify transitive `logos-module-builder` references via `inputs.<dep>.inputs.logos-module-builder.follows = "logos-module-builder"`. Without it, `install` can fail via the overridden sibling's lock even when a direct `nix build` succeeds. Documented fully in `docs/basecamp-module-requirements.md`; expected to become obsolete once upstream `logos-module-builder` scaffolding emits this `follows` automatically.
7. Assumption notes from Usability 7 are printed to stderr (not the captured log), so pasting them into a bug report is straightforward.
8. `scaffold.toml` diffs in version control surface module-identity changes as explicit, reviewable edits — same footing as any other project config change.

### + (Privacy, Anonymity, Censorship-Resistance)

- Per-profile isolation of p2p identity keys: fresh profile directory produces a fresh libp2p / Waku identity with no cross-profile leakage.
- No state mutation outside the project's `.scaffold/` directory — the user's global Logos state is never touched.
- No telemetry, no upload of module artifacts, identities, or profile state to third-party services.

### Dependencies

#### Internal Dependencies

- Logos Basecamp (dev variant only).
- Logos Package Manager (`lgpm`).
- Module repositories (delivery, storage, etc.) exposing env-var overrides for every listening port, with env var names chosen and documented by each module; tracked via upstream issues (e.g., [logos-delivery-module#18](https://github.com/logos-co/logos-delivery-module/issues/18)).
- Module `metadata.json` schema: `name` (string), `dependencies` (array of strings). Already documented in `docs/basecamp-module-requirements.md`.

#### Runtime Dependencies

- Nix with flakes enabled on the developer machine.
- Qt build toolchain (supplied via the basecamp flake dev shell).
- Unix-like OS (Linux, macOS). Windows is out of scope.

#### Module Dependencies

- `.#lgx` flake output on the project (or sub-flakes) — `.#lgx-portable`-only projects fail explicitly until they expose `.#lgx`.
- `.#lgx-portable` flake output for any module the developer wants to test against a basecamp AppImage. Projects without it get a clear error from `build-portable`, not a silent miss.
- Modules that bind sockets must honor external port override via env var (names chosen by each module) for multi-instance launch to be fully useful.
