pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Schema version persisted as `[scaffold].version` in `scaffold.toml`.
/// Bumped when the file's section/field shape changes in a way that requires
/// a one-shot migration through `init`. Parsers reject any other value with
/// a targeted error pointing at `init`.
pub(crate) const SCAFFOLD_TOML_SCHEMA_VERSION: &str = "0.2.0";
/// Default `source` for `[repos.lez]`. Single field — `url` was dropped in
/// the 0.2.0 schema after audit confirmed `LEZ_URL == lez.source` in every
/// production code path.
pub(crate) const LEZ_SOURCE: &str = "https://github.com/logos-blockchain/logos-execution-zone.git";
pub(crate) const SPEL_SOURCE: &str = "https://github.com/logos-co/spel.git";

/// Two-form git pin: SHA (used in scaffold.toml `[repos.*].pin` and in
/// `check_repo` git-head comparisons) plus tag (used by `check_spel_lez_alignment`
/// and by user-project Cargo.toml git-dep substitution).
pub(crate) struct GitRef {
    pub(crate) sha: &'static str,
    pub(crate) tag: &'static str,
}

// Cross-framework invariant: DEFAULT_SPEL must point at a spel commit
// whose `spel-cli/Cargo.toml` vendors LEZ at the same ref as DEFAULT_LEZ.
// Otherwise spel's sequencer-RPC client speaks a different protocol than
// scaffold's own wallet/sequencer build. `check_spel_lez_alignment` in
// `commands/doctor.rs` enforces this at runtime — re-run `doctor` after
// bumping either pin.
pub(crate) const DEFAULT_LEZ: GitRef = GitRef {
    sha: "cf3639d8252040d13b3d4e933feb19b42c76e14a",
    tag: "v0.1.2",
};
pub(crate) const DEFAULT_SPEL: GitRef = GitRef {
    sha: "73fc462eb8f0a4d00f1a846437c627ec2e523f83",
    tag: "v0.5.0",
};

/// `logos-blockchain-circuits` GitHub release version that contains the
/// proving/verification keys and witness generators every
/// `logos-blockchain-{pol,poc,poq,zksign}` build script reads at compile time
/// via `logos-blockchain-circuits-utils::circuits_dir()`.
///
/// Pinned to the version LEZ v0.1.2's `flake.lock` resolves to (its
/// `logos-blockchain-circuits` input is `d6cf41f…`, whose `flake.nix` declares
/// `circuitsVersion = "0.4.1"`). A mismatched circuits release silently
/// produces incompatible verifier keys, so bump this in lock-step with
/// `DEFAULT_LB_PIN` / `DEFAULT_LEZ`.
///
/// Default for `[circuits].version`. Materialised on demand into the
/// project's `[circuits].install_dir` (default `.scaffold/circuits`) by
/// `circuits::ensure_circuits_for_project`. Override by setting
/// `LOGOS_BLOCKCHAIN_CIRCUITS` to a populated checkout; the env var
/// short-circuits the download.
pub(crate) const DEFAULT_CIRCUITS_VERSION: &str = "0.4.1";
pub(crate) const LOGOS_BLOCKCHAIN_CIRCUITS_ENV: &str = "LOGOS_BLOCKCHAIN_CIRCUITS";
pub(crate) const CIRCUITS_RELEASE_BASE_URL: &str =
    "https://github.com/logos-blockchain/logos-blockchain-circuits/releases/download";

pub(crate) const DEFAULT_HELLO_WORLD_IMAGE_ID_HEX: &str =
    "4880b298f59699c1e4263c5c2245c80123632d608b9116f4b253c63e6c340771";
pub(crate) const DEFAULT_WALLET_PASSWORD: &str = "logos-scaffold-v0";
/// Env vars naming the wallet home directory for wallet subprocesses. LEZ
/// v0.2.0 renamed the variable to `LEE_WALLET_HOME_DIR`; earlier pins read
/// `NSSA_WALLET_HOME_DIR`. Scaffold sets both on every wallet invocation that
/// touches the wallet home, so the binary from either pin targets the project
/// wallet — a v0.2.0 wallet that only sees the old name silently falls back to
/// `~/.lee/wallet` (scaffold#240). `doctor`'s `wallet --version` probe is the
/// one deliberate exception: it reads no wallet state.
///
/// Every name is set to the *same* path, so the order of this list is not
/// significant. If a future pin ever reads two of these with different
/// meanings, that stops being true and the callers of
/// [`crate::commands::wallet_support::set_wallet_home_env`] need revisiting.
/// When upstream renames the variable again: add the new name, keep the old
/// one, never swap.
pub(crate) const WALLET_HOME_ENV_VARS: &[&str] = &["NSSA_WALLET_HOME_DIR", "LEE_WALLET_HOME_DIR"];
pub(crate) const WALLET_CONFIG_REL_PATH: &str = "wallet/configs/debug/wallet_config.json";
pub(crate) const WALLET_CONFIG_NESTED_REL_PATH: &str =
    "lez/wallet/configs/debug/wallet_config.json";
pub(crate) const WALLET_CONFIG_REL_PATHS: &[&str] =
    &[WALLET_CONFIG_NESTED_REL_PATH, WALLET_CONFIG_REL_PATH];
pub(crate) const WALLET_BIN_REL_PATH: &str = "target/release/wallet";
pub(crate) const FRAMEWORK_KIND_DEFAULT: &str = "default";
pub(crate) const FRAMEWORK_KIND_LEZ_FRAMEWORK: &str = "lez-framework";
pub(crate) const DEFAULT_FRAMEWORK_VERSION: &str = "0.1.0";
pub(crate) const DEFAULT_FRAMEWORK_IDL_SPEC: &str = "lssa-idl/0.1.0";
pub(crate) const DEFAULT_FRAMEWORK_IDL_PATH: &str = "idl";
pub(crate) const SEQUENCER_BIN_REL_PATH: &str = "target/release/sequencer_service";
/// Project-relative directory holding the Risc0 guest crate (`methods/Cargo.toml`,
/// `methods/guest/...`). Shared between the build side (`build_methods_guests`),
/// which compiles the manifest, and the deploy side, which discovers the resulting
/// `.bin` artefacts under the canonical workspace `target/riscv-guest/...` layout
/// or the supported sub-crate `methods/target/...` compatibility layout.
pub(crate) const METHODS_DIR: &str = "methods";
pub(crate) const SEQUENCER_CONFIG_REL_PATH: &str =
    "sequencer/service/configs/debug/sequencer_config.json";
pub(crate) const SEQUENCER_CONFIG_NESTED_REL_PATH: &str =
    "lez/sequencer/service/configs/debug/sequencer_config.json";
pub(crate) const SEQUENCER_CONFIG_REL_PATHS: &[&str] =
    &[SEQUENCER_CONFIG_NESTED_REL_PATH, SEQUENCER_CONFIG_REL_PATH];
pub(crate) const SPEL_BIN_REL_PATH: &str = "target/release/spel";
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
