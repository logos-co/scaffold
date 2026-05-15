# Scaffold — Architecture Decision Records

## Project Structure (Single-Repo Template)

Developers need one bootstrap target that is immediately runnable and easy to modify.
Use a single generated project containing contract, CLI client, configuration, and deployment scripts.
Single-template onboarding is very simple.

## CLI

The workflow should be discoverable for new developers.
Expose one CLI surface with subcommands for init, build, deploy, and interact.
One CLI improves onboarding but makes it hard to maintain backward-compatibility.

## Local Runtime

Local development should work without requiring manually managed external node setup.
Provide embedded localnet lifecycle commands as part of scaffold workflow.
The scaffolded toolchain can start, stop, and reset a localnet environment
that supports deploy and wallet-based interaction for the generated example contract.

## Build Pipeline

Contract compilation should align with Rust ecosystem standards
and avoid unnecessary abstraction.
Use native Cargo-based build flow as the primary compilation path.

## Network Configuration

Developers need explicit, editable environment targeting for local and DevNet workflows.
Use environment-file based network configuration as the default model.
Generated projects include env files for local and DevNet,
wallet interaction settings used by deploy and wallet-based interaction commands.
Env files are familiar and automation-friendly,
but require strict handling to avoid credential leakage.

## Portable Artefact Build is Separate from Install

The `.#lgx-portable` output exists to load into a released basecamp AppImage — a
different delivery path with different XDG paths and a different install mechanism
than the scaffold-managed `alice`/`bob` profiles. `basecamp build-portable` targets
that output and stops once the artefacts exist; it never invokes `lgpm`, never
mutates profile state, and never touches `basecamp.state`. This keeps the two
delivery flows strictly separate, trading a slight command-surface duplication for
clean boundaries.

## Build-Portable Stages Symlinks in a Scaffold-Owned Directory

`basecamp build-portable` produces `.lgx` artefacts in the nix store — paths
like `/nix/store/xxx-source/foo.lgx`. Operators then load those into a
basecamp AppImage via its "install lgx" file picker, which opens in the
filesystem. Browsing to `/nix/store/` by hand through a file dialog is
painful: the hashes are opaque, there's no ordering, and the user has to
mentally reconstruct which artefact belongs to which module.

`build-portable` now writes a mirror of each built artefact as a symlink
under `<project>/.scaffold/basecamp/portable/<NN>-<module_name>.lgx`
(or `<NN>-<module_name>-<stem>.lgx` when a source emits multiple outputs).
The `NN` prefix is a two-digit zero-padded load-order index, so the
directory lists the artefacts in the exact order basecamp needs to load
them — modules with no project-internal deps first, modules that depend
on them afterwards. Ordering is derived from each source's `metadata.json`
`dependencies` array via a topological sort among the captured
`role = "project"` modules; non-project deps are ignored because they're
resolved at runtime via the basecamp preinstall or package-manager
catalog, not at hand-load time.

The symlink directory is wiped and recreated on every `build-portable`
run. That keeps re-runs idempotent: removing a module via `basecamp
modules` and re-running `build-portable` leaves no stale symlinks. The
symlinks point at live nix-store paths; the store entries themselves are
retained by their `result-lgx-portable` GC root at each flake root, so
scaffold's symlink does not itself pin the artefacts.

Tradeoffs: scaffold writes under `.scaffold/basecamp/portable/` at
`build-portable` time, relaxing an earlier design goal that
`build-portable` never touch `.scaffold/`. The upside is operator
ergonomics for the one actual friction point in the AppImage test
flow — no more hunting through the nix store in a file picker.

## AppImage Path is Outside Scaffold's Scope

`build-portable` could have tried to auto-locate a basecamp AppImage and copy
artefacts into its module directory. It does not. The scaffold does not know the
AppImage's install location, and probing filesystem heuristics would be unreliable
across Linux distributions and macOS bundles. Instead, scaffold produces the `.lgx`
artefacts and prints their absolute store paths; the developer copies them into
whatever AppImage they are testing against. The tradeoff is an extra manual step;
the upside is no fragile heuristics and no surprise writes into user-managed
locations.

## Module Identity Lives in scaffold.toml

Runtime IPC composition means a module declares dependencies on other modules by
*name* (in `metadata.json`'s `dependencies` array). Scaffold needs a mapping from
those names to concrete flake refs in the captured module set — otherwise
`basecamp install` cannot know which flake provides `delivery_module` when a
project source declares that dep.

`scaffold.toml` carries a top-level `[modules]` table keyed by `module_name`
with explicit `flake` and `role` (`project` | `dependency`) fields.
`basecamp modules` writes the table at capture time; dep resolution at
build time is a key lookup against that table. `scaffold.toml` is the
sole human-readable source of truth for the captured module set;
`basecamp.state` is reduced to pin-artefact metadata only. (Originally
nested under `[basecamp.modules.*]`; lifted to top level in schema 0.2.0
because modules are project-level Logos artifacts and basecamp is one
consumer of them, not their owner.)

Populating `module_name` on capture:
- `path:` flake sources — read directly from the source's `metadata.json.name`
  (the source tree is on the local filesystem).
- `github:` flake sources — `metadata.json` is only available after building,
  so scaffold derives a best-guess name from the repo slug (strip `logos-`
  prefix, `-` → `_`) and prints a one-line assumption note at capture time
  inviting the user to edit `scaffold.toml` if wrong.

The tradeoff: `basecamp modules` now writes to `scaffold.toml`, widening its
write surface beyond the derived `basecamp.state`. The upside is that the
captured module set becomes reviewable in version control and diff tooling
(a single TOML section, not a line-oriented state file), and dep-resolution
lookups become deterministic key matches — the "is this dep covered by
something I already captured?" question has an unambiguous answer.

## Sibling `--override-input` Resolves By Declared Input Name

Multi-sub-flake projects rely on `--override-input <input> path:<sibling-abs>` so a sub-flake's `path:../<sibling>` inputs resolve to the developer's working tree instead of whatever `github:` ref is in its lock. The first implementation keyed overrides by the **sibling directory name on disk** — a convention where input names are expected to match directory names. Two problems:

- Projects with snake_case input names and kebab-case directory names (e.g. `inputs.tictactoe_solo_ai.url = "path:../logos-tictactoe-solo-ai"`) fail: nix emits `input has an override for a non-existent input <dirname>`, drops the override, falls through to the original `path:..` URL, and errors out under pure-eval.
- The convention isn't visible at call sites: there's no error telling the developer to rename either the input or the directory; the break appears as a nix lock-resolution error from inside the store copy.

Scaffold now reads the target sub-flake's `flake.nix` at both probe time (`basecamp modules`) and build time (`basecamp install` / `build-portable`), looking for `<name>.url = "path:../<sibling>"` patterns, and emits `--override-input <name> path:<sibling-abs>` — keyed by the **declared input name**, not the directory name. Directory and input names no longer need to match.

The parser is line-level and recognizes the common single-line declarative forms (`<name>.url = "…"` and `inputs.<name>.url = "…"`). Multi-line nested-attrset forms (`inputs.x = { url = "…"; flake = false; };` split across lines) are out of scope — they fall through silently and the probe or build may fail with the raw pure-eval error. Widening the parser is cheaper than adding a nix-based input enumerator, since the latter also hits pure-eval constraints on the very inputs we're trying to unstick.

The tradeoff: adding a minimal flake.nix text parser to scaffold. The upside: one less convention the user has to learn to get their project building, and the error mode goes away for legitimate non-convention projects. Build-time filtering via `flake_declared_inputs` (nix flake metadata) remains as defense-in-depth; after this change, the parser's output is already constrained to declared names, so the filter is redundant on the common path and only fires when the metadata fetch itself succeeded against some nix setup the parser didn't.

## Flake Attribute Selection is a Resolver Parameter

`install` and `build-portable` share a single source-resolution routine that is
parameterized on the flake attribute name (`lgx` vs `lgx-portable`). Hardcoding two
parallel resolvers would duplicate the precedence rules (explicit → root flake →
sub-flakes → targeted failure) and bug-surface the sibling path-input override
logic. The tradeoff is one extra argument on an internal helper; the upside is
that any resolver fix automatically applies to both commands, and the "only
`.#lgx-portable` found" and "only `.#lgx` found" error paths are symmetric by
construction.

## Vendoring `spel` Per-Project for Program ID Surfacing

`deploy` needs to print the deployed program's on-chain ID (the risc0 image
ID) so the user can hand it to a client without rerunning a separate tool.
Three options were considered:

1. Compute the image ID in-process by depending on a risc0 crate
   (`risc0-binfmt` / `risc0-zkvm`).
2. Re-implement the SHA-256 + page-tree construction directly in scaffold.
3. Shell out to `spel inspect` and parse its output.

Option (1) ties scaffold's image-ID computation to a specific risc0 release;
version skew with the user's project's risc0 dependency would silently
produce wrong IDs. It also pulls a sizable risc0 dep tree into scaffold's
own build. Option (2) is even more brittle — any upstream change to the
construction silently produces wrong IDs.

Scaffold vendors `spel` per project the same way it vendors LEZ: clone
`logos-co/spel` into the project, pin a commit (`DEFAULT_SPEL_PIN`,
currently tag `v0.2.0-rc.5`), build it during `setup`, and invoke the
project-local binary from `deploy`. `spel` itself depends on the same
risc0 crate the user's project does, so the image ID it computes is
byte-for-byte the value the sequencer will use. The pin is a scaffold-
shipped default overridable via `[repos.spel].pin` in `scaffold.toml`;
`doctor` reports drift the same way it does for LEZ.

**LEZ-version alignment.** Spel itself vendors LEZ as a Cargo dependency
for its sequencer-RPC client and wallet helpers. Picking a spel pin
whose `spel-cli/Cargo.toml` references a different LEZ commit than
scaffold's own `DEFAULT_LEZ_PIN` is a latent footgun: spel and the
scaffold-built sequencer would speak different versions of the
sequencer protocol, breaking `lgs spel -- ...` subcommands that hit
the network. Image-ID computation is unaffected (it's purely guest-ELF
+ risc0-zkvm), but other spel surfaces would silently diverge.

We pick `DEFAULT_SPEL_PIN` to satisfy this alignment at every bump
(currently `v0.2.0-rc.5`, which pins LEZ via `tag = "v0.2.0-rc1"` —
the same commit `DEFAULT_LEZ_PIN` resolves to). The counter-intuitive
naming — `v0.2.0-rc.5` is *newer* than the unsuffixed `v0.2.0` tag —
is upstream's choice and a maintenance hazard worth flagging when
revising the pin. `doctor` enforces alignment at runtime by reading
`<spel.path>/spel-cli/Cargo.toml` and warning if neither
`DEFAULT_LEZ_PIN` nor `DEFAULT_LEZ_TAG` appears in spel's dependency
declarations.

The image ID is computed locally from the submitted ELF — scaffold does
not (yet) verify on-chain inclusion. LEZ wallet/RPC don't currently
surface deploy receipts; once they do, scaffold can add an inclusion
check after submission and the human-output caveat note can retire.
Until then, the deploy summary explicitly distinguishes "wallet
accepted" from "chain confirmed."

The tradeoff is that `setup` now does an extra multi-minute build on first
run and the deploy path forks a subprocess to recover a value that could
in principle be computed inline. The upside: scaffold has zero risc0 dep
of its own, the program ID always matches what the sequencer sees, and a
`logos-scaffold spel -- <args>` proxy falls out for free so users can run
any spel subcommand without a global install. `extract_program_id` is
bounded by a wall-clock timeout and falls back to a non-fatal "unavailable"
hint so a broken or hung `spel` can never gate a successful deploy.

## Schema Migration via `init` (0.2.0 generalization)

When `[repos.spel]` was first added, every scaffolded project had a
config lacking it. The original solution: hard-fail in the parser with
a targeted error pointing at `init`, which appends the missing section
in place. Default-filling silently was rejected because the warning
would fire on every invocation forever (`deploy`, `doctor`, `localnet`,
`wallet`, `spel`, `report` all parse) and would corrupt
machine-parseable outputs (`doctor --json`, `deploy --json`).

The 0.2.0 schema migration generalizes that pattern. `init` is the
single migration entry point for any pre-0.2.0 scaffold.toml shape:

- pre-spel files (no `[repos.spel]`)
- legacy `url` field on `[repos.{lez,spel}]`
- `[basecamp].pin` / `.source` / `.lgpm_flake` (moved to
  `[repos.basecamp]` / `[repos.lgpm]`)
- `[basecamp.modules.*]` (moved to top-level `[modules.*]`)

`init` walks the file via `toml_edit` so comments, key ordering, and
unrelated sections survive the rewrite. It bumps `[scaffold].version`
to `0.2.0`, prints a one-line summary of what changed, and refuses
already-migrated files. Pre-0.2.0 lgpm flake refs of the form
`github:owner/repo/<sha>#attr` are split into `source`/`pin`/`attr`
automatically; unparseable refs trigger a hand-edit hint with a
default pin written in place so the file is at least valid.

Every other command — `deploy`, `doctor`, `setup`, `localnet`,
`wallet`, `spel`, `report`, `basecamp *` — refuses pre-0.2.0 files
with a single line: *"scaffold.toml uses an old schema (...). Run
`logos-scaffold init` to migrate to v0.2.0; existing settings are
preserved."* JSON consumers stay clean.

## scaffold.toml Schema: Three Namespaces

Schema version 0.2.0 (`[scaffold].version = "0.2.0"`) factors the file
into three orthogonal namespaces:

- **`[repos.<name>]`** — every pinned external git dependency. One
  schema for all four: `lez`, `spel`, `basecamp`, `lgpm`. Fields:
  `source` (clone URL or flake source), `pin` (git SHA), optional
  `build` (`"cargo"` default, `"nix-flake"` for nix-built deps),
  optional `attr` (flake output attribute), optional `path` override.
  Adding a fifth dep is a one-section addition with no code dispatch
  on section name.
- **`[modules.<name>]`** — Logos modules the project ships. `flake`
  + `role`. Basecamp consumes them via `basecamp install` / `launch`
  / `build-portable`, but they're not basecamp's property — a future
  test harness or alternative loader can pick them up the same way.
- **`[<feature>]`** — runtime config per feature: `[scaffold]`,
  `[wallet]`, `[framework]`, `[localnet]`, `[basecamp]` (port
  allocation only).

Earlier schemas (pre-0.2.0) mixed all three: `[basecamp]` carried
pin / source / lgpm_flake / modules together with port_base, and
`[repos.{lez,spel}]` carried both `url` and `source`. Migration is via
`logos-scaffold init`, which detects any old-shape signal (pre-0.2.0
version stamp, `url` field, `[basecamp].pin`/`.source`/`.lgpm_flake`,
`[basecamp.modules.*]`) and rewrites in place via `toml_edit` so
comments and key ordering survive.

### Pin-Only Storage, Path Derived at Runtime

For every `[repos.<name>]`, `scaffold.toml` stores `pin` and
`source` only; the on-disk clone location is derived at runtime from
`resolve_cache_root() + "repos/<name>/" + pin`. The file is therefore
portable across machines and CI by default — nothing in it depends on
the host's `$HOME` or cache layout. The `cache_root` chain
(env → `[scaffold].cache_root` → XDG default) decides where the clone
lands; the `pin` decides which commit is checked out.

`[repos.lez]` and `[repos.spel]` carry an additional optional `path`
field that `[repos.basecamp]` and `[repos.lgpm]` don't need. When set
it's authoritative — used literally if absolute, joined to
`project.root` if relative — and exists for two cases:

- `--vendor-deps` projects, where `new` writes the relative literal
  `.scaffold/repos/{lez,spel}` so the clone lives inside the project
  tree.
- Pre-portability `scaffold.toml` files in the wild that still carry
  an absolute cache path; the loader honors them verbatim so no
  migration is forced for that one field. Setting `path = ""` and
  re-running `setup` opts back into portability.

`setup` decides sync mode from the same field: empty path →
cache-managed → auto-reclone on origin mismatch; non-empty path →
vendored or user-overridden → fail-on-mismatch so a developer's
checkout is never silently rewritten.

### Build Dispatch

`[repos.<name>].build` is a discriminator with two values:

- `"cargo"` (default; emitted only when the user opts into a non-
  default elsewhere). `setup` builds the clone via
  `cargo build --release -p <hardcoded target>`. Targets stay
  code-side; making them data-driven didn't earn its complexity.
- `"nix-flake"`. `basecamp setup` builds via
  `nix build <source>/<pin>#<attr>`. `attr` is required.

The two paths share `resolve_repo_path` for the on-disk location and
`source` / `pin` storage; they diverge only in the build command they
issue.

### Schema-Stale Files Are Hard-Failed

The parser refuses to load any pre-0.2.0 file. The error names the
specific stale shape (e.g. `[basecamp].lgpm_flake (moved to
[repos.lgpm])`) and points at `init`. Re-running `init` migrates and
the user proceeds. Default-filling the missing fields with a stderr
warning was rejected because the warning would corrupt
machine-parseable outputs (`doctor --json`, `deploy --json`) and
because users would never get nudged hard enough to actually
migrate. Hard-fail + targeted hint is the same model as the original
`[repos.spel]` migration.

## `lgs run` is a Single-Command Inner Loop

The dev's "change → build → deploy → interact" cycle was multiple commands
(`build`, `localnet start`, `wallet topup`, `deploy`, then ad-hoc post-deploy
calls). Scaffold could have stayed at primitives and let projects script the
sequence themselves; instead, `lgs run` collapses the sequence into one
command with numbered step headers and optional post-deploy hooks.

Tradeoff: scaffold takes on responsibility for the pipeline shape. Adding or
reordering steps becomes a coordinated change across `cmd_run`, cli args,
serializer, and the dogfooding doc. Upside: every project gets the same
discoverable inner-loop story without per-project shell-script drift, and
the env contract that hooks see is uniform across projects.

The pipeline composes the existing primitives (`cmd_build_shortcut`,
`build_idl_for_current_project`, `cmd_localnet`, `cmd_wallet_topup_inner`,
`cmd_deploy`) — no parallel implementation. Step ordering is fixed; if a
new step is needed, it joins the chain rather than offering a knob.

## Hook Env Contract is a Documented Public Surface

Post-deploy hooks run via `sh -c` with `cwd` at the project root. The env
they see is stable, documented in README, and validated by unit and
integration tests:

- `SEQUENCER_URL` / `NSSA_WALLET_HOME_DIR` / `SCAFFOLD_PROJECT_ROOT` /
  `SCAFFOLD_IDL_DIR` — pipeline state.
- `SCAFFOLD_PROGRAM_ID` / `SCAFFOLD_GUEST_BIN` — single-program shortcuts.
  Set only when exactly one program is deployable; absent for
  multi-program projects so hooks fail loudly rather than silently
  picking up the wrong program.

`NSSA_WALLET_HOME_DIR` keeps its upstream-wallet name rather than being
renamed to a `SCAFFOLD_*` prefix: hooks that exec the wallet binary
(directly or via `cargo run --bin run_*`) need the var under the name the
binary's `WalletCore::from_env()` reads. Renaming for hook ergonomics
would silently break those hooks.

The single-program metadata is resolved once per `lgs run` invocation
and reused across every hook so multiple hooks don't multiply the cost
of `spel inspect`.

## Build Output Discovery

The deploy command must work for any scaffolded project regardless of its name.
Binary discovery should derive paths from the actual project structure, not assume the template's naming.
The implementation walks both `target/riscv-guest/` (the canonical risc0 layout used by the
scaffold template) and `methods/target/` (the sub-crate workspace layout), matching `<program>.bin`
files whose path components include both a `riscv32im*` target triple and a `release` directory.
Release builds are preferred; if only a debug build exists, that is used as a fallback. When
multiple matches exist, the shallowest path wins. This keeps the scaffold general and avoids
coupling to a specific project name or workspace layout.

## Guest Build Discovery

The deploy-side discovery above only works if guest binaries actually exist on disk. Risc0
guest crates target `riscv32im-risc0-zkvm-elf` and are intentionally excluded from the parent
workspace in real projects (the scaffold template lists them as members for ergonomics, but
projects that grew their own workspace — for example to add unrelated host crates — typically
exclude `methods` and `methods/guest` so `cargo build --workspace` doesn't try to build them
for the host triple).

`build` therefore probes for `methods/Cargo.toml` after the workspace build and, when present,
runs `cargo build --release --manifest-path methods/Cargo.toml` so the guest crate compiles
regardless of whether it is a workspace member. The probe is a single stat; absence is a no-op,
so non-Risc0 projects pay nothing. Release mode is chosen so the produced `.bin` lands in the
same `release/` path component the deploy-side discovery requires — the two halves are designed
together. The shared `methods` directory name lives in `crate::constants::METHODS_DIR`.
