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
/// Basecamp commit pin — `logos-basecamp` tag `0.2.3` (upstream dropped the
/// `v` prefix after `v0.1.1`).
/// Projects can override via `[repos.basecamp].pin` in `scaffold.toml`.
///
/// Bumping this pin is never a one-line change: the basecamp release locks a
/// `logos-package-manager` rev, and scaffold's `lgpm` CLI must match it (see
/// [`DEFAULT_LGPM_PIN`]) because that same rev is the library the app uses to
/// scan what `lgpm` installed. Companion pins in [`BASECAMP_DEPENDENCIES`] and
/// the bundled-module list in [`BASECAMP_PREINSTALLED_MODULES`] are derived
/// from the release too. ADR "Basecamp Pin Bumps Move as a Set" records the
/// rule.
pub(crate) const DEFAULT_BASECAMP_PIN: &str = "aa237766baf61404e12da86b7303cb41065464c9";
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
/// basecamp reads and writes its user state. Must match the Qt
/// `QApplication::applicationName()` the pinned basecamp binary is built
/// with: dev (`#app`) → `LogosBasecampDev`, portable (`#bin-*`) →
/// `LogosBasecamp`.
pub(crate) const BASECAMP_XDG_APP_SUBPATH_DEV: &str = "Logos/LogosBasecampDev";
pub(crate) const BASECAMP_XDG_APP_SUBPATH_PORTABLE: &str = "Logos/LogosBasecamp";

/// The 0.1.x dev (`#app`) launcher and the binary it `exec`s.
///
/// Neither basecamp generation wraps its Qt binary in place (`wrapQtApps` is
/// skipped so the process name stays `LogosBasecamp` for the macOS Dock), so a
/// `/bin/sh` launcher is what exports `QT_PLUGIN_PATH` / `QML2_IMPORT_PATH` /
/// `LD_LIBRARY_PATH`. The generations disagree on which *name* is the launcher:
/// 0.1.x installs `bin/logos-basecamp` next to the raw `bin/LogosBasecamp`,
/// while 0.2.x installs no `logos-basecamp` at all and makes `bin/LogosBasecamp`
/// itself the launcher (over a hidden `bin/.LogosBasecamp`).
///
/// Two call sites depend on that pair and must not drift apart:
/// `resolve_basecamp_binary` probes the launcher first so a 0.1.x pin is never
/// started as an unwrapped binary, and `basecamp_comm_candidates` maps the
/// launcher name onto the name the live process actually reports.
pub(crate) const BASECAMP_BIN_LAUNCHER_V01: &str = "logos-basecamp";
pub(crate) const BASECAMP_BIN_V01_TARGET: &str = "LogosBasecamp";

/// Env vars naming basecamp's data-tree root, i.e. the directory it loads user
/// modules and UI plugins from. Basecamp 0.2.x renamed the override to
/// `LOGOS_USER_DIR` (`LogosBasecampPaths.h::baseDirectory()`) and dropped the
/// 0.1.x `LOGOS_DATA_DIR` entirely, so `launch` writes both names and stays
/// pin-agnostic — they are read by disjoint basecamp generations. Same
/// two-generation compat shape as [`WALLET_HOME_ENV_VARS`], and the same rule
/// on the next rename: add the new name, keep the old, never swap.
///
/// The two keys are resolved independently (see `set_absolute_module_root_var`),
/// so a caller may override one without disturbing the other — and they are
/// *not* written under the same conditions, which is why they are separate
/// constants rather than one list.
///
/// The 0.2.x override. Set on **every** host and stack by `launch`, unlike
/// `LOGOS_DATA_DIR` below.
///
/// Why unconditional: 0.2.x resolves its base directory from
/// `QStandardPaths::AppDataLocation` unless this variable is set, and on macOS
/// that location ignores `XDG_DATA_HOME` entirely. Under v0.1.1 the dev `#app`
/// output exposed no CLI-invocable binary on macOS, so only the portable stack
/// could hit that path; 0.2.x installs a launcher for every platform, so a
/// macOS dev-stack user would otherwise see `alice` and `bob` collapse onto the
/// shared `~/Library/Application Support/Logos/LogosBasecampDev` tree. On Linux
/// the value written is the same path `XDG_DATA_HOME` already implies, so
/// setting it there is a no-op that keeps one code path for both platforms.
pub(crate) const BASECAMP_MODULE_ROOT_ENV_VAR_USER_DIR: &str = "LOGOS_USER_DIR";

/// The 0.1.x data-tree override, superseded by
/// [`BASECAMP_MODULE_ROOT_ENV_VAR_USER_DIR`]. Only written on the macOS
/// portable stack — the one place a 0.1.x basecamp was observed to ignore
/// `XDG_DATA_HOME`.
pub(crate) const BASECAMP_MODULE_ROOT_ENV_VAR_DATA_DIR: &str = "LOGOS_DATA_DIR";

/// Subdirectories basecamp 0.2.x creates under its base directory
/// (`LogosBasecampPaths.h`). `modules` / `plugins` hold what `lgpm` installs;
/// `module_data` (per-module persisted state) and `logs` (rotated app logs)
/// are new in 0.2.x. All four live inside the tree `launch` scrubs, so none of
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
/// by `logos-module-builder` 0.2.x / `nix-bundle-lgx` carry those hashes;
/// tutorial-era packages do not and are rejected with
/// `Missing content hashes in manifest`. `basecamp install` turns that failure
/// into a targeted hint rather than silently disabling validation.
pub(crate) const LGPM_SOURCE: &str = "github:logos-co/logos-package-manager";
pub(crate) const DEFAULT_LGPM_PIN: &str = "202af6fa0f0f4493bc59c8a609dff9326f78a18d";
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
    // `logos-delivery-module` tag `v0.2.0` (commit `3258cdb0…`, 2026-07-31).
    //
    // Two constraints pick this rev. It must expose `packages.<sys>.lgx` (the
    // resolver's contract) — it does, via `logos-module-builder` 0.2.5 — and
    // its `.lgx` must carry the Merkle content hashes that the `lgpm` rev in
    // [`DEFAULT_LGPM_PIN`] validates on install. The previous default (the
    // `tutorial-v1-compat` head, `1fde1566…`) predates hashes and is now
    // rejected with `Missing content hashes in manifest`, so this pin moves in
    // lock-step with the basecamp/lgpm pair rather than independently.
    //
    // Per-project overrides in `[modules.<name>]` take precedence, and
    // `basecamp modules` auto-discovery prefers any matching input found in the
    // project's own `flake.lock` over this table (so a project's own pin
    // always wins).
    (
        "delivery_module",
        "github:logos-co/logos-delivery-module/3258cdb0132e37228aa2519e0c01c0e7429a20dd#lgx",
    ),
    // Additional companions (storage_module, etc.) added on demand as real
    // projects declare them. Keeping the starter set small avoids surprising
    // users with unnecessary companion builds.
];

/// Modules basecamp ships itself. These must NEVER be captured as dependencies
/// by the auto-discovery walk — basecamp provides them, so a project that
/// declares one as a dep needs no flake ref for it.
///
/// Basecamp 0.2.x installs these at *build* time into `$out/modules` and
/// `$out/plugins` next to the binary and reads them through
/// `setEmbeddedModulesDirectory()` / `setEmbeddedUiPluginsDirectory()`; v0.1.x
/// instead pushed a `preinstall/` set into the user's data dir on first launch.
/// Either way they are basecamp's, not the project's. The names are the
/// `metadata.json` `name` of each bundled package, not the repo or flake name.
///
/// Kept in sync manually with the release's `installedDev` list in
/// `<basecamp>/flake.nix`. When bumping [`DEFAULT_BASECAMP_PIN`], diff that
/// list and confirm against the built `$out/modules` + `$out/plugins`.
pub(crate) const BASECAMP_PREINSTALLED_MODULES: &[&str] = &[
    "capability_module",
    "main_ui",
    "package_downloader",
    "package_manager",
    "package_manager_ui",
];
