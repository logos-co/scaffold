pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Schema version persisted as `[scaffold].version` in `scaffold.toml`.
/// Bumped when the file's section/field shape changes in a way that requires
/// a one-shot migration through `init`. Parsers reject any other value with
/// a targeted error pointing at `init`.
pub(crate) const SCAFFOLD_TOML_SCHEMA_VERSION: &str = "0.3.0";
/// Default `source` for `[repos.lez]`. Single field — `url` was dropped in
/// the 0.2.0 schema after audit confirmed `LEZ_URL == lez.source` in every
/// production code path.
pub(crate) const LEZ_SOURCE: &str = "https://github.com/logos-blockchain/logos-execution-zone.git";

/// Two-form git pin: SHA (used in scaffold.toml `[repos.*].pin` and in
/// `check_repo` git-head comparisons) plus tag (shown to users and used by
/// the release-line check in `doctor`).
pub(crate) struct GitRef {
    pub(crate) sha: &'static str,
    pub(crate) tag: &'static str,
}

/// LEZ release scaffold builds against. Scaffold supports exactly one LEZ
/// release line — 0.3.x — because every layer it drives changed in 0.3.0:
/// the guest program model (`lee_core::program::{Plan, run_program}`,
/// per-program account shards), deployment (`wallet program-loader deploy`
/// over segment accounts instead of `deploy-program`), funding (genesis
/// `supply_account` entries instead of a piñata faucet), fees on public
/// transactions, and the sequencer's RPC and block encoding. Nothing in
/// scaffold carries compatibility code for older pins.
pub(crate) const DEFAULT_LEZ: GitRef = GitRef {
    sha: "1a003bfbfb89fcf987f62975cd08a401169e1b8c",
    tag: "v0.3.1",
};

/// Funding account for scaffold-managed sequencers (localnet and test-node).
///
/// LEZ 0.3 has no faucet program: a chain's only native supply is what its
/// sequencer config's `genesis` hands out as `supply_account` entries. LEZ's
/// own debug config funds `CbgR6tj5…` and publishes its key in the
/// repository's `Justfile` (`wallet-import-test-accounts`), so it is a
/// well-known *development* key, never a secret. Scaffold writes the same
/// account into the genesis of every chain it starts, imports the key into
/// the project wallet during `setup`, and `wallet topup` transfers from it.
pub(crate) const DEV_FUNDER_PRIVATE_KEY_HEX: &str =
    "7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb";
pub(crate) const DEV_FUNDER_ACCOUNT_ID: &str = "CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r";
/// Genesis balance of [`DEV_FUNDER_ACCOUNT_ID`] (LEZ debug config value).
pub(crate) const DEV_FUNDER_GENESIS_BALANCE: u64 = 10_000_000_000_000;
/// Amount `wallet topup` transfers. LEZ 0.3 charges fees on public
/// transactions and the payer must hold the full fee *reserve* (about
/// 16,003,000 at the default `gas_limit`) plus any amount it sends before a
/// transaction is accepted, so the top-up covers a few dozen deployments and
/// runner calls.
pub(crate) const DEFAULT_TOPUP_AMOUNT: u64 = 1_000_000_000;

pub(crate) const DEFAULT_WALLET_PASSWORD: &str = "logos-scaffold-v0";
/// Env var naming the wallet home directory for wallet subprocesses. LEZ
/// 0.3 reads only this name (the pre-0.2 `NSSA_WALLET_HOME_DIR` is gone);
/// without it the wallet silently falls back to `~/.lee/wallet`.
pub(crate) const WALLET_HOME_ENV_VAR: &str = "LEE_WALLET_HOME_DIR";
/// Debug wallet config inside the pinned LEZ checkout; copied into the
/// project wallet home by `setup`.
pub(crate) const WALLET_CONFIG_REL_PATH: &str = "lez/wallet/configs/debug/wallet_config.json";
pub(crate) const WALLET_BIN_REL_PATH: &str = "target/release/wallet";
pub(crate) const FRAMEWORK_KIND_DEFAULT: &str = "default";
pub(crate) const FRAMEWORK_KIND_LEZ_FRAMEWORK: &str = "lez-framework";
pub(crate) const DEFAULT_FRAMEWORK_VERSION: &str = "0.1.0";
pub(crate) const SEQUENCER_BIN_REL_PATH: &str = "target/release/sequencer_service";
/// Project-relative directory holding the Risc0 guest crate (`methods/Cargo.toml`,
/// `methods/guest/...`). Shared between the build side (`build_methods_guests`),
/// which compiles the manifest, and the deploy side, which discovers the resulting
/// `.bin` artefacts under the canonical workspace `target/riscv-guest/...` layout
/// or the supported sub-crate `methods/target/...` compatibility layout.
pub(crate) const METHODS_DIR: &str = "methods";
/// `[package.metadata.risc0].methods` entry every risc0 template ships, and the
/// fallback when `methods/Cargo.toml` declares none. Values are paths relative
/// to `methods/`.
pub(crate) const DEFAULT_RISC0_METHODS_ENTRY: &str = "guest";
/// Docker image tag scaffold pins for the deterministic guest build
/// (`risczero/risc0-guest-builder:<tag>`). The tag fixes the guest Rust
/// toolchain, so it is part of what makes the emitted ELF — and therefore the
/// `program_id` derived from it — reproducible across machines.
///
/// Verified in CI: this image reports `rustc 1.97.0-dev (e638c6cfe 2026-07-15)`,
/// the same compiler build as rzup's `v1.97.0` toolchain, so the container and
/// a local `rzup install rust` agree. The Guest Build Reproducibility workflow
/// re-prints the image's `rustc --version` on every run rather than leaving
/// anyone to infer it from the tag string.
///
/// **Passing this explicitly is the whole point.** `risc0-build`'s own
/// `DEFAULT_DOCKER_TAG` is `r0.1.88.0`, whose rustc is 1.88 — too old for the
/// template's dependency graph. Anyone running `cargo risczero build` by hand
/// without exporting `RISC0_DOCKER_CONTAINER_TAG` silently gets that image and
/// an MSRV error that reads like a scaffold bug. Scaffold always sets the
/// variable (`deterministic_guest_command`), which is what keeps the pin real.
///
/// Bumping this changes every `program_id` the project produces. Projects
/// override it with `[build].risc0_docker_tag` in `scaffold.toml`; the same
/// value reaches risc0 through `RISC0_DOCKER_CONTAINER_TAG`.
pub(crate) const DEFAULT_RISC0_DOCKER_TAG: &str = "r0.1.97.0";
/// Project-relative `CARGO_TARGET_DIR` for the deterministic guest build.
/// `cargo risczero build` emits into `<this>/riscv32im-risc0-zkvm-elf/docker/`.
/// Kept out of the workspace `target/riscv-guest/` tree that
/// `risc0_build::embed_methods()` owns so the two build modes never write to
/// the same directory, and so a mode switch can clear one without touching the
/// other. Also a `deploy`-side search root — see `GUEST_BIN_SEARCH_ROOTS`.
pub(crate) const GUEST_DOCKER_TARGET_DIR: &str = "target/riscv-guest-docker";
/// Debug sequencer config inside the pinned LEZ checkout. Scaffold never runs
/// it as-is: localnet and test-node write a patched copy (see
/// `sequencer_config`) and pass that path to `sequencer_service`.
pub(crate) const SEQUENCER_CONFIG_REL_PATH: &str =
    "lez/sequencer/service/configs/debug/sequencer_config.json";
/// Default seconds to wait for the sequencer to become ready when `lgs run`
/// has to start localnet itself. Cold first runs (fresh repo clone, cold
/// nix/cargo caches) routinely overshoot the previous 20s ceiling. Override
/// per invocation with `lgs run --localnet-timeout <SECS>`.
pub(crate) const DEFAULT_RUN_LOCALNET_TIMEOUT_SEC: u64 = 120;
/// Default `source` for `[repos.basecamp]`. Built via `nix build .#app`,
/// hence `BASECAMP_ATTR = "app"`.
pub(crate) const BASECAMP_SOURCE: &str = "https://github.com/logos-co/logos-basecamp.git";
pub(crate) const BASECAMP_ATTR: &str = "app";
/// Basecamp commit pin — `logos-basecamp` tag `0.3.0`, the only basecamp
/// release scaffold supports (ADR "Scaffold Supports One Basecamp Release").
/// Projects can override via `[repos.basecamp].pin` in `scaffold.toml`, but
/// scaffold's launcher, bundled-module and env-var knowledge is 0.3.0's.
///
/// Bumping this pin is never a one-line change: the basecamp release locks a
/// `logos-package-manager` rev, and scaffold's `lgpm` CLI must match it (see
/// [`DEFAULT_LGPM_PIN`]) because that same rev is the library the app uses to
/// scan what `lgpm` installed. Companion pins in [`BASECAMP_DEPENDENCIES`] and
/// the bundled-module list in [`BASECAMP_PREINSTALLED_MODULES`] are derived
/// from the release too. ADR "Basecamp Pin Bumps Move as a Set" records the
/// rule. The pins this one replaced are kept in [`RETIRED_BASECAMP_PIN_SETS`]
/// so `basecamp setup` can move projects off them.
pub(crate) const DEFAULT_BASECAMP_PIN: &str = "bbe5da0e038ef19095690f4164a8ddaab0915821";
/// The release tag [`DEFAULT_BASECAMP_PIN`] points at, for user-facing text.
pub(crate) const DEFAULT_BASECAMP_RELEASE: &str = "0.3.0";
pub(crate) const BASECAMP_PROFILE_ALICE: &str = "alice";
pub(crate) const BASECAMP_PROFILE_BOB: &str = "bob";
/// Relative path (under the project root) to the per-profile XDG tree root.
pub(crate) const BASECAMP_PROFILES_REL: &str = ".scaffold/basecamp/profiles";
/// Subdirectories of the project root that `basecamp install` auto-discovery
/// never descends into when probing for `.lgx`-producing flakes. Hidden dirs
/// (those starting with `.`) are skipped separately and are not listed here.
/// The configured `cache_root` is prepended at call sites — it's dynamic.
pub(crate) const BASECAMP_AUTODISCOVER_SKIP_SUBDIRS: &[&str] =
    &["target", "node_modules", "result"];
/// Path under `XDG_CONFIG_HOME` / `XDG_DATA_HOME` / `XDG_CACHE_HOME` where
/// basecamp reads and writes its user state. The Qt application name is
/// `LogosBasecamp` on both stacks; the dev (`#app`) build appends `Dev` to its
/// data directory (`LogosBasecampPaths.h`) so it can sit next to an installed
/// release: dev → `LogosBasecampDev`, portable (`#bin-*`) → `LogosBasecamp`.
pub(crate) const BASECAMP_XDG_APP_SUBPATH_DEV: &str = "Logos/LogosBasecampDev";
pub(crate) const BASECAMP_XDG_APP_SUBPATH_PORTABLE: &str = "Logos/LogosBasecamp";

/// Name of basecamp's entry point on every stack.
///
/// The dev (`#app`) build does not wrap its Qt binary in place (`wrapQtApps`
/// is skipped so the process name stays `LogosBasecamp` for the macOS Dock):
/// `bin/LogosBasecamp` is a `/bin/sh` launcher that exports `QT_PLUGIN_PATH` /
/// `QML2_IMPORT_PATH` / `LD_LIBRARY_PATH` and then `exec`s the hidden
/// `bin/.LogosBasecamp`. The Linux portable bundle (`nix-bundle-dir`) uses the
/// same name for its own launcher over `bin/.LogosBasecamp.elf`, and the macOS
/// bundle keeps it as `LogosBasecamp.app/Contents/MacOS/LogosBasecamp`.
///
/// Two call sites depend on that naming and must not drift apart:
/// `resolve_basecamp_binary` probes for it, and `basecamp_comm_candidates` maps
/// it onto the names the live process actually reports.
pub(crate) const BASECAMP_BIN: &str = "LogosBasecamp";

/// Env var naming basecamp's data-tree root, i.e. the directory it loads user
/// modules and UI plugins from (`LogosBasecampPaths.h::baseDirectory()`; the
/// app's own `--user-dir` flag is a front end for it). The value is used
/// as-is — no `Dev` suffix, no relative-path resolution — which is why
/// `launch` always writes an absolute path.
///
/// Set on **every** host and stack by `launch`. Without it basecamp resolves
/// its base directory from `QStandardPaths::AppDataLocation`, which on macOS
/// ignores `XDG_DATA_HOME` entirely, so `alice` and `bob` would collapse onto
/// the shared `~/Library/Application Support/Logos/LogosBasecamp[Dev]` tree. On
/// Linux the value written is the same path `XDG_DATA_HOME` already implies,
/// so setting it there is a no-op that keeps one code path for both platforms.
pub(crate) const BASECAMP_MODULE_ROOT_ENV_VAR_USER_DIR: &str = "LOGOS_USER_DIR";

/// Subdirectories basecamp creates under its base directory
/// (`LogosBasecampPaths.h`). `modules` / `plugins` hold what `lgpm` installs;
/// `module_data` holds per-module persisted state and `logs` the app's own
/// session logs. All four live inside the tree `launch` scrubs, so none of
/// them survive a relaunch — that is the clean-slate contract, surfaced by
/// `basecamp paths` so it is visible rather than surprising.
pub(crate) const BASECAMP_BASE_DIR_MODULES: &str = "modules";
pub(crate) const BASECAMP_BASE_DIR_PLUGINS: &str = "plugins";
pub(crate) const BASECAMP_BASE_DIR_MODULE_DATA: &str = "module_data";
pub(crate) const BASECAMP_BASE_DIR_LOGS: &str = "logs";

/// `[repos.basecamp].attr` values that select the portable distribution stack.
/// Anything else (including unrecognised attrs) is treated as dev.
pub(crate) const BASECAMP_PORTABLE_ATTRS: &[&str] =
    &["bin-macos-app", "bin-appimage", "bin-bundle-dir"];

/// Default `source` / `pin` / `attr` for `[repos.lgpm]`. The `lgpm` CLI
/// lives in a separate repo (`logos-package-manager`) from basecamp; pin
/// alongside basecamp so dogfooding is reproducible. Built via
/// `nix build <source>/<pin>#<attr>`.
///
/// Pinned to the exact `logos-package-manager` rev that
/// [`DEFAULT_BASECAMP_PIN`] locks. That is not a stylistic choice: basecamp
/// embeds `logos-package-manager-module`, which builds against the same rev
/// and is what scans and loads the modules our `lgpm` CLI wrote into the
/// profile. Pinning both sides to one rev keeps the writer and the reader
/// byte-identical, so a bump on either side must move this too.
///
/// This rev also validates `.lgx` structure and Merkle content hashes on
/// install (the CLI's default signature policy is `warn`, which still runs
/// package validation — only `--allow-unsigned` disables it). Packages built
/// by `logos-module-builder` 0.2.x and later / `nix-bundle-lgx` carry those
/// hashes; tutorial-era packages do not and are rejected with
/// `Missing content hashes in manifest`. An *older* `lgpm` in turn rejects
/// packages from newer tooling (`Forbidden root entry: assets` — the icon
/// directory `logos-module-builder` 0.3.x ships). `basecamp install` turns
/// both failures into targeted hints rather than silently disabling
/// validation.
pub(crate) const LGPM_SOURCE: &str = "github:logos-co/logos-package-manager";
pub(crate) const DEFAULT_LGPM_PIN: &str = "d3af2972f51d9c542537d80d60ad8d20282ddcd1";
/// Dev stack (accepts `<host>-dev` `.lgx` variants).
pub(crate) const LGPM_ATTR: &str = "cli";
/// Portable stack (accepts bare `<host>` `.lgx` variants).
pub(crate) const LGPM_ATTR_PORTABLE: &str = "cli-portable";

/// Scaffold-level default pins for runtime companion modules that basecamp
/// does NOT bundle (listed in the Package Manager UI catalog but shipped as
/// portable-only, so dev basecamp can't load them). When
/// `basecamp modules` auto-discovery walks a project's `metadata.json` and
/// finds a dep in this table, it captures the pinned flake ref into
/// `[modules]` so `install` builds and installs the dev variant.
///
/// Keyed by the module name as it appears in `metadata.json` `dependencies`.
/// Paired conceptually with `DEFAULT_BASECAMP_PIN` — when basecamp bumps, revisit
/// these pins to stay ABI-compatible. Projects override a default by capturing
/// an explicit `[modules.<name>]` entry in `scaffold.toml`.
///
/// See the upstream issue tracking a proper `logos-modules` release pin:
/// <https://github.com/logos-co/logos-basecamp/issues/167>. Once that lands
/// scaffold can derive this table from basecamp's own manifest rather than
/// carrying an opinion.
pub(crate) const BASECAMP_DEPENDENCIES: &[(&str, &str)] = &[
    // `logos-delivery-module` `f8ad93e9…` (2026-09-22, `master`).
    //
    // Three constraints pick this rev. It must expose `packages.<sys>.lgx` (the
    // resolver's contract); its `.lgx` must carry the Merkle content hashes
    // that the `lgpm` rev in [`DEFAULT_LGPM_PIN`] validates on install; and it
    // must be built against the same `logos-protocol` / `logos-cpp-sdk` basecamp
    // 0.3.0 links, or basecamp refuses its calls at runtime. This is the newest
    // commit built with `logos-module-builder` tag `0.3.0`, whose SDK set is
    // exactly basecamp 0.3.0's (later commits move to module-builder 0.3.1,
    // already past basecamp). Its `liblogos_rln_module` dependency is declared
    // under `optional_dependencies`, so basecamp loads it without RLN; the
    // `v0.3.0-rc.1` tag makes RLN a hard dependency and is not usable here.
    // The pin it replaced (`v0.2.0`, `3258cdb0…`, module-builder 0.2.5) is in
    // [`RETIRED_DEPENDENCY_FLAKES`].
    //
    // Per-project overrides in `[modules.<name>]` take precedence, and
    // `basecamp modules` auto-discovery prefers any matching input found in the
    // project's own `flake.lock` over this table (so a project's own pin
    // always wins).
    (
        "delivery_module",
        "github:logos-co/logos-delivery-module/f8ad93e9dad006641c5b26e2626fc43b2756853c#lgx",
    ),
    // Additional companions (storage_module, etc.) added on demand as real
    // projects declare them. Keeping the starter set small avoids surprising
    // users with unnecessary companion builds.
];

/// Modules basecamp ships itself. These must NEVER be captured as dependencies
/// by the auto-discovery walk — basecamp provides them, so a project that
/// declares one as a dep needs no flake ref for it.
///
/// Basecamp installs these at *build* time into `$out/modules` and
/// `$out/plugins` next to the binary and reads them from there, so they never
/// appear in a profile's `modules/`. The names are the `metadata.json` `name`
/// of each bundled package, not the repo or flake name. In 0.3.0 `main_ui` is
/// no longer a package — the shell is part of the app — and `modules_state`
/// (the module lifecycle registry) joined the set.
///
/// Kept in sync manually with the release's `installedDev` list in
/// `<basecamp>/flake.nix`. When bumping [`DEFAULT_BASECAMP_PIN`], diff that
/// list and confirm against the built `$out/modules` + `$out/plugins`.
pub(crate) const BASECAMP_PREINSTALLED_MODULES: &[&str] = &[
    "capability_module",
    "modules_state",
    "package_downloader",
    "package_manager",
    "package_manager_ui",
];

/// Module names earlier basecamp releases bundled that are no longer packages
/// in the release scaffold supports, with why. A project module that still
/// declares one as a dependency gets a targeted error from `basecamp modules`
/// (no flake exists to capture) instead of the generic unresolved-dependency
/// fixes; as an optional dependency it is skipped with a note.
pub(crate) const RETIRED_PREINSTALLED_MODULES: &[(&str, &str)] = &[
    (
        "main_ui",
        "basecamp 0.3.0 folded the shell UI into the app; it is not a package any more",
    ),
    (
        "counter",
        "basecamp 0.2.x stopped bundling this example module",
    ),
    (
        "counter_qml",
        "basecamp 0.2.x stopped bundling this example module",
    ),
    (
        "webview_app",
        "basecamp 0.2.x stopped bundling this example app",
    ),
];

/// Default `(basecamp, lgpm)` pin pairs earlier scaffold releases wrote into
/// `scaffold.toml` (`new` / `init` / `basecamp setup` persist the literal
/// default). Scaffold supports only the release in [`DEFAULT_BASECAMP_PIN`],
/// so `basecamp setup` moves a project still carrying one of these *exact*
/// pairs to the current pair, and `basecamp doctor` warns until it has. The
/// match is on revs, not provenance: a deliberate pin to one of these is moved
/// too. A pair that matches neither side of a retired default is left alone.
pub(crate) const RETIRED_BASECAMP_PIN_SETS: &[(&str, &str, &str)] = &[
    // (release label, basecamp pin, lgpm pin)
    (
        "0.2.3",
        "aa237766baf61404e12da86b7303cb41065464c9",
        "202af6fa0f0f4493bc59c8a609dff9326f78a18d",
    ),
    (
        "v0.1.1",
        "a746cdbc521f72ee22c5a4856fd17a9802bb9d69",
        "e5c25989861f4487c3dc8c7b3bc0062bcbc3221f",
    ),
];

/// `[modules.<name>].flake` values `basecamp modules` captured from earlier
/// [`BASECAMP_DEPENDENCIES`] tables. They were written into `scaffold.toml`
/// verbatim, so a pin bump alone never reaches them; `basecamp setup` rewrites
/// an entry whose flake is *exactly* one of these to the current default.
pub(crate) const RETIRED_DEPENDENCY_FLAKES: &[(&str, &str)] = &[
    (
        "delivery_module",
        "github:logos-co/logos-delivery-module/3258cdb0132e37228aa2519e0c01c0e7429a20dd#lgx",
    ),
    (
        "delivery_module",
        "github:logos-co/logos-delivery-module/1fde1566291fe062b98255003b9166b0261c6081#lgx",
    ),
];
