---
name: basecamp
description: Use when running any `lgs basecamp …` subcommand or working on a Logos module project (a project that builds `.lgx` artefacts via `flake.nix#packages.<system>.lgx`). Covers `setup` / `modules` / `install` / `launch` / `build-portable` / `doctor` / `docs`, the `[basecamp.modules]` schema, per-profile XDG isolation under `.scaffold/basecamp/profiles/{alice,bob}/`, clean-slate launch semantics, two-instance p2p dogfooding, and AppImage-targeted portable builds. Activates additionally on presence of `[basecamp.modules]` in `scaffold.toml` or `.scaffold/basecamp/profiles/`. Independent of template skills.
---

# Basecamp Integration

Basecamp is the runtime host for Logos `.lgx` modules — a separate process plus per-profile state, with `lgpm` as the module package manager. `lgs basecamp …` orchestrates the full lifecycle from a module project on disk: pin the basecamp + lgpm binaries, capture which modules to install, build them, install them into pre-seeded `alice` / `bob` profiles, and launch profile-isolated instances side-by-side for p2p dogfooding. For driving the `lgs` CLI itself, use the `lgs-cli` skill.

## When to Use

This skill activates whenever **any** of these is true:

- The user invokes any `lgs basecamp …` subcommand.
- The user describes building a Logos module / `.lgx` / basecamp app, or working in a module project (a `flake.nix` exposing `packages.<system>.lgx`).
- `[basecamp.modules.*]` entries already exist in `scaffold.toml`.
- `.scaffold/basecamp/profiles/` already exists (basecamp setup has been run in this project before).

Basecamp activates **independently** of `lez-template` / `lez-framework-template`. A project can be both a templated LEZ project and a basecamp-hosted module project; both skills then apply. Basecamp can also stand alone in an external module project with no LEZ template at all (the canonical case in DOGFOODING B-series — `tictactoe` and similar).

The canonical compatibility doc — including the full `[basecamp.modules]` schema and dependency-resolution rules — is `docs/basecamp-module-requirements.md`, mirrored to consumers via `lgs basecamp docs`. Treat it as the source of truth and reference it instead of duplicating its contents.

## Module Project Requirements (Hard Contract)

For `lgs basecamp …` to do anything useful, the project on disk must satisfy:

1. **`scaffold.toml` at the project root.** Run `lgs init` once if missing.
2. **`lgs basecamp setup` has been run** (one-time per project; idempotent on unchanged pin). Pins basecamp + lgpm, builds them via Nix, seeds `.scaffold/basecamp/profiles/{alice,bob}/`.
3. **At least one `flake.nix`** — at the project root or in immediate sub-directories — exposing `packages.<system>.lgx`. This is the convention from `logos-module-builder` (tag `tutorial-v1`).
4. **For `build-portable`:** the same flakes also expose `packages.<system>.lgx-portable`. There is no silent fallback from `#lgx-portable` to `#lgx` — variant choice is the user's; if `#lgx-portable` is missing, the command fails with a targeted hint.

A flake that exposes only `#lgx-portable` and not `#lgx` fails the regular install path explicitly; opt in with `--flake <ref>#lgx-portable` on `basecamp modules` instead.

## Command Surface

| Command | Purpose |
|---|---|
| `lgs basecamp setup` | One-time: pin basecamp + lgpm, build, seed `alice` / `bob` profiles. Idempotent on unchanged pin. |
| `lgs basecamp modules [--show] [--flake REF]… [--path PATH]…` | **Sole writer of `[basecamp.modules.*]` in `scaffold.toml`.** Auto-discovers project sub-flakes that expose `#lgx`, or takes explicit `--flake` / `--path` sources. Resolves `metadata.json` `dependencies` recursively. Manual edits in `[basecamp.modules]` are preserved across re-runs. `--show` prints the captured set without mutating state. |
| `lgs basecamp install [--print-output]` | Build every captured source (`role = "project"` and `role = "dependency"`) and shell out to `lgpm` to install into both profiles. Logs to `.scaffold/logs/<ts>-install.log`; `--print-output` (or `LOGOS_SCAFFOLD_PRINT_OUTPUT=1`) streams nix output instead. If `[basecamp.modules]` is empty, transparently invokes `modules` in auto-discover mode first. |
| `lgs basecamp launch <profile> [--no-clean]` | Profile is `alice` or `bob`. **Default**: kills any prior `logos_host` / `logos-basecamp` descendants for that profile, scrubs the profile's XDG dirs, replays each captured source's install, sets `LOGOS_PROFILE` + `XDG_{CONFIG,DATA,CACHE}_HOME`, and `exec`s basecamp. **`--no-clean`** is the only escape hatch — skip scrub + replay, exec against existing profile state. |
| `lgs basecamp build-portable` | Build `.#lgx-portable` for `role = "project"` entries only, in dependency order (leaves first); `role = "dependency"` entries are skipped (target AppImage provides them). Symlinks artefacts into `.scaffold/basecamp/portable/<NN>-<module_name>.lgx` for hand-loading via the AppImage's "install lgx" file picker. Wipes-and-recreates the portable dir each run. |
| `lgs basecamp doctor [--json]` | Basecamp-specific health: captured-modules summary, manifest variant check per profile, drift between `[basecamp.modules]` and on-disk profile state. |
| `lgs basecamp docs` | Print `docs/basecamp-module-requirements.md` (embedded at compile time). Runs **outside** a scaffold project too — useful for retrieving the contract before `lgs init`. |

`modules` and `launch` are the two with non-obvious semantics. The other commands are mostly mechanical.

## `[basecamp.modules]` Schema

Each entry is a TOML sub-section keyed by `module_name`:

```toml
[basecamp.modules.tictactoe]
flake = "path:/abs/path/to/tictactoe#lgx"
role  = "project"

[basecamp.modules.delivery_module]
flake = "github:logos-co/logos-delivery-module/<rev>#lgx"
role  = "dependency"
```

- **`module_name` (the key)** matches the identifier other sources' `metadata.json` `dependencies` array uses to refer to this module. If `tictactoe_ui`'s manifest declares `"dependencies": ["tictactoe", "delivery_module"]`, both names must appear as keys here.
- **`role = "project"`** — a module the developer is building locally. `build-portable` attr-swaps these to `#lgx-portable`.
- **`role = "dependency"`** — a runtime companion. `install` / `launch` load them; `build-portable` skips them.

`basecamp modules` derives `module_name` differently per source kind:

- `path:` flake refs → read `<path>/metadata.json.name`. Exact, no guessing.
- `.lgx` file paths → read sibling `metadata.json` if present, else fall back to filename stem with a one-line assumption note.
- `github:` / other remote refs → derive from repo slug (strip `logos-` prefix, `-` → `_`) with a one-line assumption note. Edit the TOML if the guess is wrong; re-runs are byte-identical and never overwrite existing keys.

For each project source's declared `dependencies`, the resolution order is: already-keyed → no-op; basecamp **preinstalls** (`capability_module`, `package_manager`, `counter`, `webview_app`, and their `_ui` siblings) → silent skip; declaring source's own `flake.lock` → use the locked rev rewritten to `#lgx`; scaffold-default `BASECAMP_DEPENDENCIES` table → fallback; **unresolved** → fail fast naming the dep and the two user-side fixes (capture as a project source, or add an explicit `[basecamp.modules.<name>]` with `role = "dependency"`). No silent drop.

**Implication for module authors:** declare each runtime dep as a flake input in your module's `flake.nix`, even if you don't link against it — that's the cleanest way to give scaffold an authoritative pin without hitting the scaffold default.

## Sibling Sub-Flake Overrides

Multi-flake projects (e.g. `tictactoe` core plus `tictactoe-ui-cpp` / `tictactoe-ui-qml` siblings) need each sub-flake's `path:../<sibling>` inputs to resolve against the working tree, not the locked `github:` pin. Scaffold parses each sub-flake's `flake.nix` for `<name>.url = "path:../<sibling>"` declarations and emits `--override-input <input_name> path:<abs>` at both probe and build time. The input *name* used in the override comes from `flake.nix`, not the sibling directory name on disk.

Only `path:../<sibling>` inputs are rewritten. `path:./sub`, `github:`, `git+`, etc. pass through untouched. The parser is **line-level** — multi-line `inputs.x = { url = "…"; flake = false; };` declarations with `url` on its own line are not detected. Flatten such declarations to single-line form when they fail to override.

## Profiles & XDG Isolation

Basecamp state is **always project-local** under `<project>/.scaffold/basecamp/`. Never user home (`~/.local/share/Logos/`, `~/Library/Application Support/Logos/`) — writes outside `.scaffold/basecamp/` are bugs.

| Path | Purpose |
|---|---|
| `.scaffold/basecamp/profiles/alice/`, `.../bob/` | Per-profile XDG roots. `launch <profile>` sets `XDG_{CONFIG,DATA,CACHE}_HOME` to the profile root. |
| `.scaffold/basecamp/portable/<NN>-<name>.lgx` | Symlinks to `.#lgx-portable` builds, ordered by dependency topology. Wiped each `build-portable`. |
| `.scaffold/state/basecamp.state` | Pinned basecamp + lgpm binary paths; pin-derived metadata. |
| `.scaffold/logs/<ts>-setup-*.log`, `<ts>-install.log` | Build logs. |

Each `launch` also sets `LOGOS_PROFILE=<name>` for child processes. The two-instance dogfooding flow (B3) relies on this isolation: `alice` and `bob` running in parallel see independent identity keys, message history, and storage.

## Clean-Slate Semantics (B4 Guard)

`lgs basecamp launch alice` (default, no `--no-clean`):

1. Kill any prior `logos_host` / `logos-basecamp` descendants for the alice profile.
2. `rm -rf` the profile's XDG dirs — **strictly bounded to** `<project>/.scaffold/basecamp/profiles/alice/`. A `launch` that scrubs anything outside that root is a severe safety regression.
3. Replay every captured source's install (build → `lgpm install`) into the freshly-scrubbed profile.
4. `exec` basecamp with the profile env set.

`--no-clean` is the only escape hatch — skip steps 1–3, exec against whatever is on disk.

**Empty `[basecamp.modules]` + default `launch`** intentionally **bails before scrubbing** (the regression guard from `fix(basecamp): bail on empty [basecamp.modules] in launch without --no-clean`). The empty-install + scrubbed-profile combo is precisely what the bail prevents. To launch with no modules (rare, mostly for inspecting basecamp itself), pass `--no-clean`.

## Two-Instance P2P Dogfooding (B3)

The canonical basecamp use case. Two terminals, both rooted at the module project:

```bash
# Terminal 1
lgs basecamp launch alice

# Terminal 2
lgs basecamp launch bob
```

Each window opens against its own `.scaffold/basecamp/profiles/{alice,bob}/`, with `LOGOS_PROFILE` set respectively. Per-profile port-override env vars are set on `launch` to avoid collision on Qt remote-objects and similar non-module ports. A p2p interaction (chat, delivery, storage) triggered from `alice` should be observable in `bob` within the module's expected latency.

If two windows open but share identity keys / message history, that is a clean-slate regression — capture it. A non-module port collision (Qt remote objects, etc.) is an upstream finding against the colliding component, not something to patch around in scaffold. Running two `launch alice` invocations in parallel is undefined in v1.

## `build-portable` (B5)

Targets the AppImage release path: build `.#lgx-portable` artefacts that can be hand-loaded via the AppImage's "install lgx" file picker, not loaded into the scaffold-managed profiles.

Behavior: builds **only** `role = "project"` entries (the dev's local modules), in topological dependency order so basecamp can resolve each module's deps before loading it. `role = "dependency"` entries are skipped — the target AppImage provides its own copies. Symlinks land in `.scaffold/basecamp/portable/` as `<NN>-<module_name>.lgx` (`NN` is the load-order index). The directory is wiped-and-recreated each run, so removing a module via `basecamp modules` doesn't leave stale symlinks.

Silent fallback from `#lgx-portable` to `#lgx` is a contract violation — the variant choice belongs to the user. Missing `#lgx-portable` attribute → fail with a targeted hint naming the missing attr.

## Common Errors

| Symptom | Root cause | Fix |
|---|---|---|
| `basecamp not set up yet` hint on `install` / `launch` | `lgs basecamp setup` never ran in this project. | `lgs basecamp setup` (one-time). |
| `basecamp install` fails inside `nix build` with `no 'main' field in metadata.json` | A sub-flake transitively pulls a newer `logos-module-builder` (typically off `main`) that's incompatible with `basecamp v0.1.1`. The stale entry silently wins through the sub-flake's `flake.lock`. | In the offending sub-flake's `flake.nix`: add `inputs.<dep>.inputs.logos-module-builder.follows = "logos-module-builder";` for each dep that itself declares `logos-module-builder`, then `nix flake update`. Verify only one `logos-module-builder` node remains in `flake.lock`. |
| `basecamp modules` fails with an unresolved-dep error | A `metadata.json` `dependencies` entry isn't already keyed in `[basecamp.modules]`, isn't a basecamp preinstall, isn't in the source's `flake.lock`, and isn't in the scaffold default table. | Either add the dep as a flake input in the source's `flake.nix` (preferred — gives an authoritative pin), or hand-add `[basecamp.modules.<name>]` with `flake = "<ref>#lgx"` and `role = "dependency"`. |
| `basecamp install` succeeds but `doctor` flags drift | `[basecamp.modules]` was edited or sources changed without a re-install. | `lgs basecamp install`. |
| Flake exposes only `#lgx-portable` and not `#lgx` | Project author opted into portable-only output. | `lgs basecamp modules --flake <ref>#lgx-portable` to opt in explicitly; or expose `#lgx` upstream. |
| `build-portable` fails with missing `#lgx-portable` attr | A captured `role = "project"` flake doesn't expose the portable variant. | Add the `lgx-portable` output to that flake, or remove the entry from `[basecamp.modules]` if not actually a project source. |
| Sibling `path:../<sibling>` inputs not overridden | Multi-line `inputs.<name> = { url = "…"; … };` declaration with `url` on its own line. The line-level parser doesn't detect it. | Flatten to single-line `<name>.url = "path:../<sibling>";`. |
| Auto-discovered `module_name` is wrong (e.g. for `github:` refs) | Heuristic-derived from repo slug. | Edit the entry directly in `scaffold.toml` — `basecamp modules` is idempotent and never overwrites existing keys. |
| `basecamp launch` writes outside `.scaffold/basecamp/profiles/<profile>/` | Severe regression. | Stop and capture the offending path before continuing. Do not retry. |

## Diagnostics & Logs

- `lgs basecamp doctor` (+ `--json`) — captured-modules summary, manifest variant check per seeded profile, drift between `[basecamp.modules]` and on-disk profile state.
- `.scaffold/logs/<ts>-setup-*.log` — `basecamp setup` build logs.
- `.scaffold/logs/<ts>-install.log` — `basecamp install` per-source nix build logs (one file per run).
- `--print-output` flag on `install` (or `LOGOS_SCAFFOLD_PRINT_OUTPUT=1` env) — stream nix output directly to the terminal instead of writing to a log file. Useful for CI where you want the full transcript in stdout.
- `lgs report --tail 500` (general scaffold report) — bundles relevant `.scaffold/logs/` and state for issue reports. Always inspect the archive (`tar -tzf <path>`) before sharing publicly.

## DOGFOODING Cross-Reference

Canonical scenarios in `DOGFOODING.md`:

- **B1** — basecamp + lgpm setup and idempotent re-run.
- **B2** — module capture, install, single-instance launch (`alice`).
- **B3** — two-instance p2p (`alice` + `bob` parallel terminals).
- **B4** — clean-slate scrub semantics on relaunch; `--no-clean` escape hatch; empty-`[basecamp.modules]` bail guard.
- **B5** — `build-portable` artefact production for AppImage hand-loading.

When reproducing a basecamp failure, name the matching scenario.

## Key Rules

- **Never edit `[basecamp.modules]` while `lgs basecamp modules` is running.** Outside that window, hand-edits are preserved across re-runs — manual entries always win over derived pins.
- **Basecamp state is always project-local under `.scaffold/basecamp/`.** Writes to `~/.local/share/Logos/` or `~/Library/Application Support/Logos/` are bugs.
- **Nothing basecamp builds lands on `PATH`.** `lgs` invokes the project-local binaries directly via `.scaffold/state/basecamp.state`. If `lgpm` or `basecamp` ends up on PATH, that's a regression.
- **`basecamp setup` is idempotent on unchanged pin.** A re-run that rebuilds when the pin hasn't changed is a regression.
- **`build-portable` is the only path that touches `#lgx-portable`.** `install` / `launch` always use `#lgx`. Silent variant fallback in either direction is a contract violation.
- **Module authors should declare runtime deps as flake inputs**, even non-link ones — gives scaffold an authoritative pin via step 3 of dep resolution.
- **`logos-module-builder` `follows` wiring is mandatory** in any sub-flake that pulls in a module which itself depends on `logos-module-builder`. Missing `follows` is the single most common `install` failure mode.
- **`lgs basecamp docs` runs outside a scaffold project.** Use it to retrieve the compatibility contract before `lgs init` when bootstrapping a new module project.
