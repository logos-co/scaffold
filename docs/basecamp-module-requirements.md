# Basecamp Module Requirements

`logos-scaffold basecamp` provides `setup`, `modules`, `install`, `launch`, `develop`, `build`, `build-portable`, `run`, `doctor`, `paths`, and `docs`. This document defines the module-project compatibility contract shared by those workflows. If your project satisfies the rules below, commands that consume the captured module set will resolve, build, and install your `.lgx` artefacts into the pre-seeded `alice` and `bob` profiles automatically. `build-portable` additionally targets the `lgx-portable` flake output for hand-loading into a basecamp AppImage.

## Hard requirements

1. **`scaffold.toml` at the project root.** Basecamp commands refuse to run outside a scaffold project. Run `logos-scaffold init` once if you don't have one.

2. **`basecamp setup` must have been run once** in the project. It pins the basecamp repo, builds the `basecamp` + `lgpm` binaries, and seeds the `alice` / `bob` profile directories under `.scaffold/basecamp/profiles/`. `install` and `launch` will emit a targeted hint if you skip this.

3. **At least one `flake.nix`** that exposes a `.lgx` package:
   - Either at the project root, or
   - In one or more immediate sub-directories (one per sub-flake).

4. **Each such flake must expose `packages.<system>.lgx`** — the convention `logos-module-builder` has established since `tutorial-v1` and still emits in its 0.2.x releases (where it is implemented on top of [`nix-bundle-lgx`](https://github.com/logos-co/nix-bundle-lgx)).
   - If a flake only exposes `packages.<system>.lgx-portable`, the resolver fails explicitly with a hint — it will not silently fall back. Expose `lgx` or pass `--flake <ref>#lgx-portable` on the command line to opt in.
   - If no flake exposes any `.lgx` attribute, the resolver fails with a generic hint pointing at `--path` / `--flake`.

5. **The `.lgx` must be built by 0.2.x-era module tooling** — `logos-module-builder` 0.2.x, or `nix-bundle-lgx` applied to your `#lib` output directly. Scaffold's pinned `lgpm` validates package structure and Merkle content hashes on install (its default `warn` signature policy still runs the validation; only `--allow-unsigned` disables it), and packages from tutorial-era tooling carry no hashes at all. Installing one fails with `Missing content hashes in manifest`, and `basecamp install` adds a hint naming the rebuild. Downgrading `[repos.lgpm].pin` is not a workaround: the basecamp the pin builds embeds the same validating library.

   Two packaging rules come with that tooling and are worth knowing before you hit them:
   - a `ui_qml` package must ship a **256×256 PNG icon** (bundled to `assets/icon.png` at the package root), and
   - `main` / `view` entries in `metadata.json` must point at files that actually exist in the built variant — validation now rejects a manifest that names a missing entry point rather than failing later at load time.

## What `setup` leaves behind — `.scaffold/state/basecamp.state`

`setup` records what it built as `key=value` lines in `.scaffold/state/basecamp.state`. This is the supported way for a script — a CI job, a UI harness, anything driving basecamp without a human at the keyboard — to find the binaries it just built:

```
pin=<the [repos.basecamp].pin that was built>
basecamp_bin=<absolute path to the basecamp entry point>
lgpm_bin=<absolute path to the lgpm binary>
```

Three things about `basecamp_bin` that a consumer needs and cannot infer:

- **Read it; do not reconstruct it.** The path is under the cache root (`~/.cache/logos-scaffold/basecamp/<pin>/app-result-<attr>/…` by default, and wherever `LOGOS_SCAFFOLD_CACHE_ROOT` or `[scaffold].cache_root` points otherwise), but *which file* under that output is the correct entry point differs by basecamp generation and by stack. On a dev `#app` build it is a `/bin/sh` launcher that exports `QT_PLUGIN_PATH` / `QML2_IMPORT_PATH` before exec'ing the real binary; on a portable build (`bin-bundle-dir`, `bin-bundle-dir-inspector`, `bin-appimage`) it is the unwrapped binary, because the bundle supplies those paths itself; on macOS `bin-macos-app` it is inside `LogosBasecamp.app/Contents/MacOS/`. Picking the wrong name starts an app that cannot find its Qt platform plugin or QML imports — a failure that looks like a basecamp bug, not a path bug. `setup` resolves this for you and writes the answer.
- **It is a nix out-link, so it is read-only**, and on a portable build the real ELF sits beside it as a dot-prefixed sibling (`.LogosBasecamp.elf`). A harness that needs a writable copy, or one that probes for a specific file layout, has to account for that — copy the tree out rather than expecting to modify it in place.
- **It moves when the stack moves.** The out-link is keyed by attr (`app-result-<attr>`), so `setup --inspector` writes `basecamp_bin` to a different path than a plain `setup` does, and both stacks coexist under the same pin rather than one overwriting the other's link. Re-read the file after any `setup`: a cached path does not follow the stack, so a consumer holding it keeps exec'ing whichever build it was captured from.

`lgs basecamp doctor` prints the same two paths for humans, and warns when a recorded path no longer exists on disk (a garbage-collected nix store is the usual cause — re-run `setup`).

A consumer that pins the basecamp rev itself — a CI cache key, say — is pinning it in a second place: `setup` builds whatever `[repos.basecamp].pin` says, and nothing reconciles that against an external copy of the same rev. Bump one without the other and the build silently goes cold, or worse, runs against a different basecamp than the one the cache was keyed on. Assert the two are equal in the consumer rather than assuming they stay in step.

## The captured module set — `[modules]` in scaffold.toml

The set of modules that `basecamp install` / `launch` / `build-portable` will act on lives in `scaffold.toml` as one sub-section per module, keyed by `module_name`:

```toml
[modules.tictactoe]
flake = "path:/abs/tictactoe#lgx"
role = "project"

[modules.delivery_module]
flake = "github:logos-co/logos-delivery-module/1fde1566291fe062b98255003b9166b0261c6081#lgx"
role = "dependency"
```

- **`module_name` is the key** and matches the identifier used in other sources' `metadata.json` `dependencies` array. For `tictactoe_ui`'s manifest to declare `"dependencies": ["tictactoe", "delivery_module"]`, both names must appear as keys here.
- **`role = "project"`** — a module the developer is building locally. `build-portable` attr-swaps these to `#lgx-portable`.
- **`role = "dependency"`** — a runtime companion. `install` / `launch` load them into the profile; `build-portable` skips them (the target AppImage provides its own).

`basecamp modules` is the primary automated writer of this section, but the table is also fully hand-authorable. The file stays human-editable — edit a generated entry to correct it, or write the entire table by hand when `basecamp modules` is the wrong fit (see "Hand-authored declarative use" below).

The pre-0.2.0 schema used `[basecamp.modules.<name>]`. Projects still on that layout are rejected at parse time with a hint pointing at `lgs init`; the section has moved to the top-level `[modules.<name>]` namespace.

### How entries get populated

On every `basecamp modules` run (explicit `--flake` / `--path` args or auto-discovery), scaffold derives `module_name` for each source:

- **`path:` flake refs** → read `<path>/metadata.json.name`. Exact, no guessing.
- **`.lgx` file paths** → read the sibling `metadata.json` if present; otherwise fall back to the filename stem and print a one-line assumption note.
- **`github:` / other remote refs** → derive from the repo slug (strip `logos-` prefix, `-` → `_`) and print a one-line assumption note:
  ```
  note: flake `github:logos-co/logos-storage-module/abc#lgx` — assumed module_name = `storage_module`. If wrong, edit `[modules]` in scaffold.toml.
  ```
  Edit the TOML if the guess is wrong — `basecamp modules` is **idempotent**: existing keys are never overwritten on re-run.

Then for each project source's declared `dependencies`, scaffold resolves a flake ref for any name not already in `[modules]`:

1. **Already keyed in `[modules]`** (any role) → no-op. Whatever you have wins.
2. **Modules basecamp bundles itself** (`capability_module`, `main_ui`, `package_downloader`, `package_manager`, `package_manager_ui`; see `BASECAMP_PREINSTALLED_MODULES` in `src/constants.rs` for the authoritative list) → silent skip, basecamp ships them. Basecamp 0.2.x installs these at build time next to its binary and loads them from there, so they never appear in a profile's `modules/` directory — a profile listing only your own modules is expected, not a failed install.
3. **Declaring source's own `flake.lock`** → if the project source declares an input with the same name, scaffold reads the locked `github:<owner>/<repo>/<rev>` and rewrites to `#lgx`. Preferred path for most projects: whatever rev the module is already building against is, by definition, the rev its IPC clients expect at runtime.
4. **Scaffold-default `BASECAMP_DEPENDENCIES`** → a hardcoded table keyed by module name (currently only `delivery_module`). Last-resort safety net for projects that don't carry the dep as a flake input.
5. **Unresolved** → `basecamp modules` **fails with a targeted error** naming the dep and both user-side fixes (capture as a project source, or add an explicit `[modules.<name>]` entry with `role = "dependency"`). No silent drop.

Resolved deps are inserted into `[modules]` with `role = "dependency"`. Re-running `basecamp modules` against the same sources is byte-identical.

Implication for module authors: **declare each runtime dep as a flake input in your module's `flake.nix`**, even if your module doesn't technically build-link against it. It's the cleanest way to give scaffold an authoritative pin (step 3 above) without hitting the scaffold default.

## Local sibling sub-flakes

Multi-flake projects (e.g. a `tictactoe` core plus `tictactoe-ui-cpp` and `tictactoe-ui-qml` sibling flakes) need a way for each sub-flake to resolve its `path:../<sibling>` inputs against the developer's working tree rather than whatever `github:` pin is in its lock.

**Scaffold reads each sub-flake's `flake.nix` to discover its `path:../<sibling>` inputs and emits the matching `--override-input <input_name> path:<abs>` args at both probe and build time.** The input name used in the override is the one declared in `flake.nix` — not the sibling directory name on disk. Directory and input names do not need to match.

Concretely:

- Directory layout `my-module/{tictactoe,tictactoe-ui-cpp,tictactoe-ui-qml}` with `flake.nix` in each.
- `tictactoe-ui-cpp/flake.nix` declares e.g. `inputs.tictactoe_core.url = "path:../tictactoe";` — the input is named `tictactoe_core`, the sibling directory is `tictactoe`.
- Scaffold parses the `flake.nix`, notices the `path:../tictactoe` URL, matches `tictactoe` against the sibling directories on disk, and emits `--override-input tictactoe_core path:/abs/path/to/my-module/tictactoe`.
- At both `basecamp modules` (auto-discovery probe) and `basecamp install` / `build-portable` (the actual build), the same overrides apply; evaluation and build see the same local sibling sources.

Only `path:../<sibling>` inputs are rewritten. `path:./sub`, `github:…`, `git+…` and similar schemes pass through untouched — scaffold has no opinion on them.

### Parser limits

The flake.nix parse is line-level and recognizes `<name>.url = "path:../<sibling>"` and `inputs.<name>.url = "path:../<sibling>"`. Multi-line value forms (e.g. `inputs.x = { url = "…"; flake = false; };` with `url` on its own line inside the nested attrset) are not detected today. If you hit a projects with such a declaration and sibling-override fails, flatten the declaration to the single-line form, or report it and we'll widen the parser.

### Transitive inputs must `follows` the top-level `logos-module-builder`

Multi-sub-flake projects that pull in modules which themselves depend on `logos-module-builder` (e.g. `delivery_module` → `logos-module-builder`) **must** unify that transitive reference onto the project's top-level pin using a `follows` entry.

Without it, your sub-flake's `flake.lock` ends up with two `logos-module-builder` entries: the one you pinned and a second one pulled in transitively (typically off the upstream's `main` branch, which may package modules differently than the release your project builds against). When scaffold then runs `nix build path:<sibling> --override-input <input-name> path:<this-sub-flake>`, nix resolves transitive inputs through **this sub-flake's lock**, and the stale second entry silently wins — builds that work with a direct `nix build .#lgx` fail with opaque errors when invoked through scaffold.

Concrete fix: in each sub-flake that declares both `logos-module-builder` and a module with its own `logos-module-builder` input, add the `follows`:

```nix
# tictactoe/flake.nix (example)
{
  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder/0.2.6";
    delivery_module.url = "github:logos-co/logos-delivery-module/<pinned-rev>";

    # Force delivery_module's transitive `logos-module-builder` to follow our
    # pin. Without this, delivery_module drags in its own module-builder as a
    # second entry in flake.lock. That extra entry silently wins when a UI
    # flake does `--override-input tictactoe path:...` and breaks the
    # local-dev workflow.
    delivery_module.inputs.logos-module-builder.follows = "logos-module-builder";
  };
  # ...
}
```

Symptoms when this is missing:

- `lgs basecamp install` fails inside `nix build` with errors from the other `logos-module-builder` (e.g. `no 'main' field in metadata.json`).
- `cd <sub-flake> && nix build .#lgx` works directly, because the direct build uses the sub-flake's own lock and never dereferences the extra entry.

Apply the same `follows` wiring to every transitive input that *also* pulls in `logos-module-builder`. After adding it, re-run `nix flake update` in that sub-flake and verify `flake.lock` now contains exactly one `logos-module-builder` node (or `logos-module-builder_N` aliases all resolving to the same rev).

This is a limitation of the `logos-module-builder` scaffolding rather than of scaffold, and is expected to be handled automatically upstream in a later release. Whether a given module-builder release still needs it is worth re-checking after a pin bump: the check is the same either way — one `logos-module-builder` node in `flake.lock`.

## Explicit escape hatch

If auto-discovery doesn't capture what you want, name the sources explicitly on `basecamp modules`:

```bash
# Pre-built .lgx file
logos-scaffold basecamp modules --path ./dist/my-module.lgx

# Arbitrary flake refs (remote refs, non-standard attrs)
logos-scaffold basecamp modules --flake github:me/my-module#lgx
logos-scaffold basecamp modules --flake .#some-alt-attr
```

Explicit sources skip root / sub-flake probing entirely. The entries land in `[modules]` exactly as specified, `role = "project"`; re-run `basecamp modules` with different args to replace or extend. `basecamp install` then replays whatever the table captures.

To override a single dependency pin without capturing it as a project source, edit `scaffold.toml` directly:

```toml
[modules.delivery_module]
flake = "github:myfork/logos-delivery-module/abc123#lgx"
role = "dependency"
```

`basecamp modules` preserves the entry on every subsequent run — user intent wins over derived pins.

### Hand-authored declarative use

You can also author the entire `[modules.*]` table by hand, with no `lgs basecamp setup` or `lgs basecamp modules` invocation. `basecamp install`, `basecamp build-portable`, and `basecamp doctor` all read whatever the table captures and don't require an automated writer to have produced it.

Concrete reasons to do this:

- **Drift detection only.** A project that ships its own `install` / `launch` flow (e.g. a distributed-stack project blocked on `lgpm` ↔ `bin-macos-app` variant alignment) can still seed `[modules.*]` entries by hand purely to get `lgs basecamp doctor` drift warnings against pin updates upstream.
- **CI / sandboxed environments** where `lgs basecamp setup` can't or shouldn't run, but the resolved module set is known.
- **Forking an existing module's flake reference** before running `modules` for the first time.

`basecamp modules` re-runs over a hand-authored table are still idempotent and preserve every entry — the automated and hand-authored modes mix freely.

## AppImage testing via `build-portable`

`basecamp install` / `launch` load modules into the scaffold-managed alice/bob profiles. To instead test against a released basecamp **AppImage**, use `build-portable`:

```bash
logos-scaffold basecamp build-portable
# → builds .#lgx-portable for every `role = "project"` entry in [modules]
# → topologically orders by metadata.json dependencies (leaves first,
#   so basecamp can resolve each module's deps before loading it)
# → symlinks the built artefacts into `.scaffold/basecamp/portable/` as
#   `<NN>-<module_name>.lgx` (NN = load-order index) so the AppImage's
#   "install lgx" file picker has browsable, human-named files in the
#   right order — no manual hunting through /nix/store/
# → prints the symlink paths in load order
```

`build-portable` does not touch profiles, `basecamp.state`, or the AppImage itself — it only produces artefacts. Load them into your AppImage in the printed order via its "install lgx" button; scaffold is intentionally unaware of the AppImage's install path.

If you launch a portable basecamp build by hand with `--user-dir <path>` (basecamp 0.2.x; `LOGOS_USER_DIR` is its env equivalent), the app stores its installed-modules + identity state at `<path>` rather than at its default data root. `build-portable` never sets that — it only produces artefacts — but `launch` does: it exports an absolute per-profile `LOGOS_USER_DIR` pointing at that profile's own module root on every host and stack (plus `LOGOS_DATA_DIR`, the 0.1.x name for the same override, on the macOS portable stack), because basecamp does not always honor `XDG_DATA_HOME` — on macOS it would otherwise collapse every profile onto the shared `~/Library/Application Support/Logos/LogosBasecamp[Dev]`. So hand-launching with `--user-dir` and `launch`-ing a profile are the same data-tree redirect reached from two entry points, not independent mechanisms: the flag isolates one ad-hoc launch, while `launch` wires the env equivalent per profile so scaffold's isolation under `.scaffold/basecamp/profiles/{alice,bob}/` actually reaches the app. A non-empty `LOGOS_USER_DIR` / `LOGOS_DATA_DIR` you declare yourself in `[basecamp.env]` or `[basecamp.profiles.<name>.env]` is honored rather than overwritten — `launch` only rewrites it to absolute against the project root when it is relative. An empty or whitespace-only value is the exception: it counts as unset and is replaced by the profile default (see "Env exported to the basecamp process" below).

The `.scaffold/basecamp/portable/` directory is wiped and recreated on every `build-portable` run, so re-running after you've removed a module via `basecamp modules` doesn't leave stale symlinks behind.

If a flake exposes only `lgx` (not `lgx-portable`), `build-portable` fails with a targeted hint — mirror of the `install` portable-only failure, in reverse.

## Per-profile launch configuration

`launch` defaults to the pre-seeded `alice` / `bob` profiles, but a project can declare its own profiles — with domain names — under `[basecamp.profiles.<name>]` in `scaffold.toml`. Any declared profile launches; unknown profiles are seeded on first launch. When `[basecamp.profiles.*]` is omitted entirely, the `alice` / `bob` default is preserved.

```toml
[basecamp.profiles.maker]
env_file    = ".env"                          # dotenv KEY=VALUE, sourced before `env`
env         = { SWAP_UI_AUTO_ROLE = "maker" }  # per-profile overrides (win over [basecamp.env])
runtime_dir = "/tmp/lgs-maker"                 # see the socket-path budget below
log_file    = ".scaffold/basecamp/profiles/maker/basecamp.log"

[basecamp.profiles.taker]
env_file    = ".env.taker"
env         = { SWAP_UI_AUTO_ROLE = "taker" }
```

- **Env layering** (last writer wins): `[basecamp.env_append]` path joins first, then the per-profile **`env_file`**, then `[basecamp.env]` globals, then the profile's inline **`env`**.
- **`launch --log-file[=PATH]`** tees basecamp's stdout/stderr to the terminal *and* a file (bare `--log-file` → `.scaffold/basecamp/profiles/<profile>/basecamp.log`; overrides `log_file`). Without it, `launch` `exec`s as before.
- **`lgs basecamp paths <profile> [--json]`** prints the resolved per-profile path manifest (xdg dirs, runtime dir, basecamp's `module_root` and its `modules` / `plugins` / `module_data` / `logs` children, `launch.state`, log file, env file) without building or mutating anything.

Everything under `module_root` lives inside the tree `launch` scrubs, so **basecamp 0.2.x's `module_data/` (per-module persisted state) and `logs/` (its own rotated session logs) do not survive a relaunch**. That is the clean-slate contract working as intended — a module that needs state across launches must not keep it there — but it is new surface in 0.2.x, so `paths` names both directories rather than leaving you to find them.

A project that ships more than one basecamp variant can map the flake attr per host instead of hard-coding one:

```toml
[repos.basecamp.attr]              # scalar `attr = "app"` still works as the fallback
aarch64-darwin = "bin-macos-app"
aarch64-linux  = "bin-appimage"
```

### Env exported to the basecamp process

`launch` sets the variables below on the basecamp child, on top of the environment `lgs` itself inherited (nothing is cleared). `<profile-dir>` is `.scaffold/basecamp/profiles/<profile>/`, and the env layering above can override any of them.

| Variable | Value | Set when |
|---|---|---|
| `XDG_CONFIG_HOME` | `<profile-dir>/xdg-config` | always |
| `XDG_DATA_HOME` | `<profile-dir>/xdg-data` | always |
| `XDG_CACHE_HOME` | `<profile-dir>/xdg-cache` | always |
| `TMPDIR` | the resolved `runtime_dir`, else `<profile-dir>/xdg-tmp` | always |
| `XDG_RUNTIME_DIR` | the resolved `runtime_dir` | only when one resolves — a configured `runtime_dir`, or the `/tmp/lgs-<profile>` macOS default |
| `LOGOS_PROFILE` | the profile name | always |
| `LOGOS_USER_DIR` | `<profile-dir>/xdg-data/Logos/LogosBasecamp[Dev]` — basecamp's base directory for this profile | always |
| `LOGOS_DATA_DIR` | same default as `LOGOS_USER_DIR`, resolved independently of it | macOS **and** a portable `[repos.basecamp].attr` (`bin-macos-app`, `bin-appimage`, `bin-bundle-dir`) |

Module-owned port-override variables are not in this list: no module has published a name yet, so scaffold exports none.

The last two rows are finalized *after* the env layering, so they are the one place a declared value is post-processed rather than simply taken as-is: an absolute value you declared is kept, a relative one is rewritten to absolute against the project root, and an empty (or whitespace-only) one is treated as unset and replaced by the profile default. Each key goes through that on its own, so declaring only one of them leaves the other at the profile default and the two can end up pointing at different trees.

Both names exist because basecamp 0.1.x reads `LOGOS_DATA_DIR` while 0.2.x reads `LOGOS_USER_DIR` (and its `--user-dir` flag), so writing both keeps `launch` agnostic to the pinned generation. They are written under different conditions, though. `LOGOS_USER_DIR` is set **always**: 0.2.x otherwise resolves its base directory from Qt's `AppDataLocation`, which on macOS ignores `XDG_DATA_HOME` entirely and would collapse every profile onto one shared tree — and unlike 0.1.x, whose dev build had no macOS-invocable binary, 0.2.x ships a launcher for every platform, so the dev stack is exposed to that too. On hosts that honor XDG the exported value is the same directory `XDG_DATA_HOME` already implies, so it changes nothing there. `LOGOS_DATA_DIR` stays on the macOS-portable gate where the 0.1.x behaviour was actually observed.

### The macOS `sun_path == 104` socket-path budget

When a module loads, liblogos opens a Unix-domain socket (a `QLocalServer` named `logos_token_<module>`) under the temp root — `TMPDIR`, which `launch` always sets. macOS caps the full socket path (`sockaddr_un.sun_path`) at **104 bytes**, so a long runtime root overflows it and basecamp aborts module loading with:

```
[SubprocessContainer] Unix socket path too long (122 >= 104)
```

the in-profile runtime root `…/.scaffold/basecamp/profiles/<profile>/xdg-tmp` is long: for a typical project path plus the `logos_token_<module>` socket name it easily blows the 104-byte budget. To avoid that, `launch` resolves `runtime_dir` with this precedence, exporting a resolved value as both `TMPDIR` and `XDG_RUNTIME_DIR`:

1. **`[basecamp.profiles.<name>].runtime_dir`** if set (project-relative paths are joined to the project root).
2. **`/tmp/lgs-<profile>`** — the automatic default on macOS (short, well under the budget).
3. The in-profile `xdg-tmp` on Linux, whose `sun_path` budget is 108 bytes — four more than macOS, so a deep project root can overflow there too; set `runtime_dir` explicitly if you hit it. Nothing resolves in this case, so `TMPDIR` points at `xdg-tmp` and `launch` exports no `XDG_RUNTIME_DIR` of its own (matching the table above) — the child still inherits whatever ambient value the shell had, since `launch` clears nothing.

If you override `runtime_dir` on macOS, keep it short (a `/tmp/…` root is safest) — a deep or project-relative path can still exceed 104 bytes once the socket name is appended.

## Quick checklist

- [ ] `scaffold.toml` exists at the project root.
- [ ] `logos-scaffold basecamp setup` has been run.
- [ ] Anything driving basecamp from a script reads `basecamp_bin` from `.scaffold/state/basecamp.state` rather than reconstructing a path under the cache root (see "What `setup` leaves behind" above).
- [ ] Each sub-flake exposes `packages.<system>.lgx`.
- [ ] The `.lgx` is built by `logos-module-builder` 0.2.x (or `nix-bundle-lgx`), so it carries the content hashes `lgpm` validates on install.
- [ ] Sibling sub-flake URLs use the `path:../<sibling-dir>` form, declared on a single `<name>.url = "…"` line (not split across multiple lines inside a nested attrset — parser limitation).
- [ ] Transitive `logos-module-builder` references are unified with a `follows` onto the top-level pin (see "Transitive inputs must `follows` …" above).
- [ ] No project relies on `lgx-portable` as the only output without passing `--flake` explicitly.
- [ ] On macOS, the per-profile `runtime_dir` stays short enough that `<runtime_dir>/logos_token_<module>_<pid>` fits the 104-byte `sun_path` budget (the `/tmp/lgs-<profile>` default does; a custom override must too).
