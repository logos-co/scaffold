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
/// Materialised on demand into `<cache_root>/circuits/v<ver>-<triple>/` by
/// `circuits::ensure_circuits_for_subprocess`. Override the version hop by
/// setting `LOGOS_BLOCKCHAIN_CIRCUITS` to a populated checkout; the env var
/// short-circuits the download.
pub(crate) const DEFAULT_CIRCUITS_VERSION: &str = "0.4.1";
pub(crate) const LOGOS_BLOCKCHAIN_CIRCUITS_ENV: &str = "LOGOS_BLOCKCHAIN_CIRCUITS";
pub(crate) const CIRCUITS_RELEASE_BASE_URL: &str =
    "https://github.com/logos-blockchain/logos-blockchain-circuits/releases/download";

pub(crate) const DEFAULT_HELLO_WORLD_IMAGE_ID_HEX: &str =
    "4880b298f59699c1e4263c5c2245c80123632d608b9116f4b253c63e6c340771";
pub(crate) const DEFAULT_WALLET_PASSWORD: &str = "logos-scaffold-v0";
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
/// `.bin` artefacts under `methods/target/...`.
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
/// Basecamp commit pin — `logos-basecamp` tag `v0.1.1`.
/// Projects can override via `[repos.basecamp].pin` in `scaffold.toml`.
pub(crate) const DEFAULT_BASECAMP_PIN: &str = "a746cdbc521f72ee22c5a4856fd17a9802bb9d69";
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

/// `[repos.basecamp].attr` values that select the portable distribution stack.
/// Anything else (including unrecognised attrs) is treated as dev.
pub(crate) const BASECAMP_PORTABLE_ATTRS: &[&str] =
    &["bin-macos-app", "bin-appimage", "bin-bundle-dir"];

/// Default `source` / `pin` / `attr` for `[repos.lgpm]`. The `lgpm` CLI
/// lives in a separate repo (`logos-package-manager`) from basecamp; pin
/// alongside basecamp so dogfooding is reproducible. Built via
/// `nix build <source>/<pin>#<attr>`.
///
/// Pinned to `logos-package-manager` tag `tutorial-v1` (the last pre-validation
/// commit). PR #8 introduced content-hash validation in the manifest; later
/// lgpm commits tightened it further. Neither is compatible today with the
/// `.lgx` files emitted by `logos-module-builder` tag `tutorial-v1`, which
/// does not populate content hashes. Revisit when module-builder starts
/// emitting hashes (or lgpm gains a compatibility mode).
pub(crate) const LGPM_SOURCE: &str = "github:logos-co/logos-package-manager";
pub(crate) const DEFAULT_LGPM_PIN: &str = "e5c25989861f4487c3dc8c7b3bc0062bcbc3221f";
/// Dev stack (accepts `<host>-dev` `.lgx` variants).
pub(crate) const LGPM_ATTR: &str = "cli";
/// Portable stack (accepts bare `<host>` `.lgx` variants).
pub(crate) const LGPM_ATTR_PORTABLE: &str = "cli-portable";

/// Scaffold-level default pins for runtime companion modules that basecamp
/// v0.1.1 does NOT preinstall (listed in the Package Manager UI catalog but
/// shipped as portable-only, so dev basecamp can't load them). When
/// `basecamp modules` auto-discovery walks a project's `metadata.json` and
/// finds a dep in this table, it captures the pinned flake ref into
/// `[basecamp.modules]` so `install` builds and installs the dev variant.
///
/// Keyed by the module name as it appears in `metadata.json` `dependencies`.
/// Paired conceptually with `DEFAULT_BASECAMP_PIN` — when basecamp bumps, revisit
/// these pins to stay ABI-compatible. Per-project overrides go in
/// `[basecamp.dependencies]` in `scaffold.toml`.
///
/// See the upstream issue tracking a proper `logos-modules` release pin:
/// <https://github.com/logos-co/logos-basecamp/issues/167>. Once that lands
/// scaffold can derive this table from basecamp's own manifest rather than
/// carrying an opinion.
pub(crate) const BASECAMP_DEPENDENCIES: &[(&str, &str)] = &[
    // `logos-delivery-module/1.0.0` (tutorial-v1 era) predates the `#lgx`
    // flake-output convention and does NOT expose `packages.<sys>.lgx` — a
    // cold `basecamp install` against that pin fails at the resolver.
    //
    // Pin to the head of `tutorial-v1-compat` on logos-delivery-module
    // (commit `1fde1566…`, 2026-04-22) — the rev that both `tictactoe` and
    // `yolo-board-module` use in their own flakes. This is the known-good
    // default; per-project overrides in `[basecamp.dependencies]` in
    // `scaffold.toml` take precedence, and `basecamp modules` auto-discovery
    // prefers any matching input found in the project's own `flake.lock`
    // over this table (so a project's own pin always wins).
    (
        "delivery_module",
        "github:logos-co/logos-delivery-module/1fde1566291fe062b98255003b9166b0261c6081#lgx",
    ),
    // Additional companions (storage_module, etc.) added on demand as real
    // projects declare them. Keeping the starter set small avoids surprising
    // users with unnecessary companion builds.
];

/// Modules that basecamp v0.1.1 preinstalls on first launch (from its
/// `preinstall/` dir). These must NEVER be captured as dependencies by the
/// auto-discovery walk — basecamp provides them itself.
///
/// Kept in sync with `<basecamp>/preinstall/*.lgx` manually. Inspect the nix
/// build output to verify this list stays accurate when bumping the basecamp pin.
pub(crate) const BASECAMP_PREINSTALLED_MODULES: &[&str] = &[
    "capability_module",
    "package_manager",
    "package_manager_ui",
    "counter",
    "counter_qml",
    "webview_app",
    "basecamp_main_ui",
];
