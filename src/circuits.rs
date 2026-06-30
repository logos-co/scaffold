//! On-demand provisioning of the `logos-blockchain-circuits` release artefact
//! that LEZ's `logos-blockchain-*` transitive crates need at build time.
//!
//! The build scripts of `logos-blockchain-pol`, `-poc`, `-poq`, and `-zksign`
//! call into `logos-blockchain-circuits-utils::circuits_dir()`, which panics
//! unless one of these is present:
//!
//! 1. `LOGOS_BLOCKCHAIN_CIRCUITS` env var pointing to a circuits directory.
//! 2. `~/.logos-blockchain-circuits/` populated with a circuits release.
//!
//! Upstream's only documented installation path is the LEZ Nix flake — it
//! consumes the `logos-blockchain-circuits` flake, which fetches the matching
//! tagged release tarball from GitHub Releases. Scaffold drives a plain
//! `cargo build`, so without help the standalone-sequencer build (and the
//! user-project build, since the patch table redirects the same crates)
//! fails on a fresh machine.
//!
//! `ensure_circuits_for_project` materialises the matching release tarball
//! into the project's configured `[circuits].install_dir` (default
//! `.scaffold/circuits`) and exports `LOGOS_BLOCKCHAIN_CIRCUITS` for the rest
//! of the process. It's idempotent: a pre-set env var, or an install dir whose
//! `VERSION` already matches `[circuits].version`, short-circuits the download;
//! a stale install (an older version) is replaced.
//!
//! Why we always export the env var rather than relying on
//! `~/.logos-blockchain-circuits/`: every `logos-blockchain` rev we depend on
//! today (LEZ rc1's pinned `81dbb45…` AND spel rc.5's vendored `5510f55…`)
//! ships its own copy of `circuits-utils`, and the home-dir branch of
//! `circuits_dir()` `assert!`s that the directory's `VERSION` matches that
//! crate's hard-coded `EXPECTED_CIRCUITS_VERSION`. Those constants disagree
//! across the two revs (`v0.4.1` vs. `v0.4.2`), so a single home-dir install
//! can never satisfy both at once. The env-var branch skips the version
//! assertion entirely and just uses the path, which is exactly the escape
//! hatch we need: scaffold-managed projects only consume the verification
//! keys at compile time, and both revs are content-compatible at the file
//! layout we install.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail};
use flate2::read::GzDecoder;
use tar::Archive;

use crate::constants::{CIRCUITS_RELEASE_BASE_URL, LOGOS_BLOCKCHAIN_CIRCUITS_ENV};
use crate::model::{CircuitsConfig, Project};
use crate::process::which;
use crate::DynResult;

/// One of the per-circuit subdirectories every release tarball contains. Used
/// as a sentinel for "this directory is a populated circuits release" in
/// cache-hit checks and post-extract verification — picking a single circuit
/// (rather than `VERSION`) keeps the check robust to future releases that
/// might rename the version marker.
pub(crate) const CIRCUITS_SENTINEL_FILE: &str = "pol/verification_key.json";

/// Ensure `LOGOS_BLOCKCHAIN_CIRCUITS` is set in this process so subsequently
/// spawned `cargo` invocations inherit it. No-op when the env var is already
/// exported and points at a populated circuits dir (user override).
///
/// Otherwise: download the matching tagged release into the project's
/// configured `[circuits].install_dir` (default `.scaffold/circuits`,
/// idempotent) and export the env var pointing there. We deliberately do NOT
/// fall back to `~/.logos-blockchain-circuits/` — see the module docs for why.
pub(crate) fn ensure_circuits_for_project(project: &Project) -> DynResult<()> {
    if circuits_path_from_env().is_some() {
        return Ok(());
    }

    let path = ensure_circuits_release_for_config(&project.root, &project.config.circuits)?;
    // SAFETY: `set_var` is `unsafe` from Rust 2024 because it is racy when
    // other threads call `env::var`. Scaffold is a single-threaded CLI by the
    // time this runs (only the main thread has touched env vars; subprocesses
    // are spawned synchronously after this call returns). The alternative —
    // threading the path through every `Command::new("cargo")` call site —
    // is strictly more invasive without changing the safety story for the
    // subprocesses, which inherit `environ` either way.
    std::env::set_var(LOGOS_BLOCKCHAIN_CIRCUITS_ENV, &path);
    Ok(())
}

/// Resolve the directory the circuits release lives at (or would be
/// materialised at) for `project`, without downloading anything. The
/// `LOGOS_BLOCKCHAIN_CIRCUITS` env override wins when it points at a
/// populated checkout — mirroring `ensure_circuits_for_project`.
pub(crate) fn circuits_dir_for_project(project: &Project) -> PathBuf {
    if let Some(path) = circuits_path_from_env() {
        return path;
    }
    circuits_install_dir(&project.root, &project.config.circuits)
}

pub(crate) fn circuits_install_dir(project_root: &Path, config: &CircuitsConfig) -> PathBuf {
    let path = PathBuf::from(&config.install_dir);
    if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    }
}

fn circuits_path_from_env() -> Option<PathBuf> {
    let raw = std::env::var(LOGOS_BLOCKCHAIN_CIRCUITS_ENV).ok()?;
    let path = PathBuf::from(raw);
    // Defensively ignore obviously-stale env values rather than panic-by-proxy
    // inside a downstream build script. If the dir is missing we fall through
    // to the cache install path below, which is what the user wants.
    if path.join(CIRCUITS_SENTINEL_FILE).is_file() {
        Some(path)
    } else {
        None
    }
}

fn ensure_circuits_release_for_config(
    project_root: &Path,
    config: &CircuitsConfig,
) -> DynResult<PathBuf> {
    let triple = release_triple()?;
    let dir = circuits_install_dir(project_root, config);
    ensure_circuits_release_at(
        &dir,
        &config.version,
        triple,
        config.url_template.as_deref(),
    )
}

fn ensure_circuits_release_at(
    dir: &Path,
    version: &str,
    triple: &str,
    url_template: Option<&str>,
) -> DynResult<PathBuf> {
    // Cache hit only when the sentinel is present AND, if the release ships a
    // top-level VERSION marker, it matches the requested version. The install
    // dir is no longer version-namespaced (it can be a fixed
    // `[circuits].install_dir`), so a stale tree would otherwise survive a
    // `[circuits].version` bump forever — `setup` hits this same short-circuit.
    if dir.join(CIRCUITS_SENTINEL_FILE).is_file() && installed_version_matches(dir, version) {
        return Ok(dir.to_path_buf());
    }
    // Stale install: clear it so extraction lands a clean tree. Only wipe a
    // directory that's empty or recognisably a prior circuits release —
    // `[circuits].install_dir` is user-controlled and may be absolute, so a
    // config typo must never make us `remove_dir_all` an unrelated directory.
    if dir.exists() {
        if !is_empty_or_circuits_install(dir) {
            bail!(
                "refusing to delete {} for a fresh circuits install: it is not empty and does \
                 not look like a circuits release (no `VERSION` or `{CIRCUITS_SENTINEL_FILE}`). \
                 Check `[circuits].install_dir`; remove the directory manually if this is intended.",
                dir.display()
            );
        }
        fs::remove_dir_all(dir)
            .map_err(|e| anyhow!("remove stale circuits dir {}: {e}", dir.display()))?;
    }

    fs::create_dir_all(dir)
        .map_err(|e| anyhow!("create circuits cache dir {}: {e}", dir.display()))?;

    let url = circuits_release_url(version, triple, url_template);

    println!(
        "Downloading logos-blockchain-circuits v{} ({triple})",
        version.trim_start_matches('v')
    );
    println!("  from {url}");
    println!("  into {}", dir.display());

    let tarball = download_to_tempfile(&url)?;
    let bytes = fs::read(tarball.path())
        .map_err(|e| anyhow!("read downloaded tarball {}: {e}", tarball.path().display()))?;
    extract_tarball(&bytes, &dir)?;

    if !dir.join(CIRCUITS_SENTINEL_FILE).is_file() {
        bail!(
            "circuits tarball extracted but sentinel `{CIRCUITS_SENTINEL_FILE}` is missing under {}",
            dir.display()
        );
    }

    Ok(dir.to_path_buf())
}

/// Whether `dir` is safe to `remove_dir_all` for a fresh circuits install:
/// either empty, or recognisably a prior circuits release (has the sentinel or
/// a `VERSION` file). Guards against a mistyped, user-controlled `install_dir`
/// pointing at an unrelated directory.
fn is_empty_or_circuits_install(dir: &Path) -> bool {
    if dir.join(CIRCUITS_SENTINEL_FILE).is_file() || dir.join("VERSION").is_file() {
        return true;
    }
    match fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_none(),
        // Can't inspect it (not a dir, permissions) → don't delete it.
        Err(_) => false,
    }
}

/// Whether the install at `dir` already satisfies `version`. Release tarballs
/// ship a top-level `VERSION` file; when present we require it to match
/// (normalising a leading `v` on either side, so `v0.4.1` vs `0.4.1` is a hit
/// rather than an endless re-download). When absent — an older or renamed
/// layout — we don't force a re-download and treat the sentinel as enough,
/// preserving the prior behaviour.
fn installed_version_matches(dir: &Path, version: &str) -> bool {
    match fs::read_to_string(dir.join("VERSION")) {
        Ok(text) => version_eq(text.trim(), version),
        Err(_) => true,
    }
}

/// Compare two circuits version strings, normalising a leading `v` on either
/// side so `v0.4.1` and `0.4.1` are treated as equal. Used by both the install
/// cache-hit check and `doctor`, which must not flag spurious version drift.
pub(crate) fn version_eq(a: &str, b: &str) -> bool {
    a.trim().trim_start_matches('v') == b.trim().trim_start_matches('v')
}

fn circuits_release_url(version: &str, triple: &str, template: Option<&str>) -> String {
    // Normalize a leading `v`: `[circuits].version` may be `v0.4.1` (treated as
    // equal to `0.4.1` everywhere else), but the release layout carries a single
    // `v` in the path, so substituting the raw `v`-prefixed string would yield
    // an unreachable `vv0.4.1`.
    let version = version.trim_start_matches('v');
    match template {
        Some(template) => template
            .replace("{version}", version)
            .replace("{triple}", triple),
        None => format!(
            "{CIRCUITS_RELEASE_BASE_URL}/v{version}/logos-blockchain-circuits-v{version}-{triple}.tar.gz",
        ),
    }
}

/// Map (`std::env::consts::OS`, `ARCH`) onto the suffix the
/// `logos-blockchain-circuits` GitHub release tarballs use.
///
/// Only the platforms scaffold itself supports today are listed. macOS aarch64
/// works upstream and is the dev-machine baseline; x86_64 macOS isn't shipped
/// by upstream's flake either, so we surface the same constraint here.
pub(crate) fn release_triple() -> DynResult<&'static str> {
    let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("macos", "aarch64") => "macos-aarch64",
        (os, arch) => bail!(
            "unsupported platform for logos-blockchain-circuits release ({os}/{arch}). \
             Set {LOGOS_BLOCKCHAIN_CIRCUITS_ENV} to a circuits checkout to bypass."
        ),
    };
    Ok(triple)
}

/// Stream `url` to a temp file via `curl`. We deliberately shell out instead
/// of using the in-tree `ureq`-based HTTP client: ureq's bundled rustls trusts
/// only the Mozilla CA set baked into `webpki-roots`, which fails on hosts
/// behind a corporate-CA TLS-intercepting proxy (a common CI shape, and the
/// shape several `lez-framework` early adopters reported). `curl` reads the
/// system trust store (and respects `CURL_CA_BUNDLE` / `SSL_CERT_FILE`), so
/// it works on both vanilla machines and proxied environments.
///
/// Returns the temp file holding the downloaded bytes; the caller reads from
/// it, then drops it so the temp is reaped.
fn download_to_tempfile(url: &str) -> DynResult<tempfile::NamedTempFile> {
    if which("curl").is_none() {
        bail!(
            "`curl` is required to fetch the {} release tarball but is not on PATH. \
             Install curl, or set {LOGOS_BLOCKCHAIN_CIRCUITS_ENV} to a pre-extracted \
             circuits directory to bypass the download.",
            "logos-blockchain-circuits",
        );
    }

    let tmp =
        tempfile::NamedTempFile::new().map_err(|e| anyhow!("create download temp file: {e}"))?;

    let status = Command::new("curl")
        .arg("--fail")
        .arg("--location")
        .arg("--silent")
        .arg("--show-error")
        .arg("--retry")
        .arg("3")
        .arg("--retry-delay")
        .arg("2")
        .arg("--output")
        .arg(tmp.path())
        .arg(url)
        // Inherit stderr so curl's `--show-error` output reaches the user's
        // terminal verbatim — matches scaffold's other shell-out patterns.
        .stderr(Stdio::inherit())
        .stdout(Stdio::null())
        .status()
        .map_err(|e| anyhow!("spawn curl for {url}: {e}"))?;

    if !status.success() {
        bail!("curl failed to download {url} ({status})");
    }

    let len = tmp
        .as_file()
        .metadata()
        .map(|m| m.len())
        .unwrap_or_default();
    if len == 0 {
        bail!("downloaded {url} is empty");
    }

    Ok(tmp)
}

/// Extract a `.tar.gz` into `dest`, stripping the single top-level directory
/// the release tarballs ship (`logos-blockchain-circuits-vX.Y.Z-os-arch/...`)
/// so callers can point `LOGOS_BLOCKCHAIN_CIRCUITS` at `dest` directly.
fn extract_tarball(bytes: &[u8], dest: &Path) -> DynResult<()> {
    let mut archive = Archive::new(GzDecoder::new(bytes));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let Some(stripped) = safe_stripped_entry_path(&path)? else {
            // Top-level directory entry itself; the strip leaves nothing to
            // create, and `entry.unpack` would otherwise write to `dest`.
            continue;
        };
        // The download source is user-influenced ([circuits].url_template), so
        // tar entries are untrusted. A symlink/hardlink could redirect a later
        // write outside `dest` even when its own path is in-bounds.
        let etype = entry.header().entry_type();
        if etype.is_symlink() || etype.is_hard_link() {
            bail!(
                "refusing to extract circuits tar entry `{}` of type {etype:?} \
                 (symlinks and hardlinks are not allowed)",
                stripped.display()
            );
        }
        let target = dest.join(&stripped);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&target)?;
    }
    Ok(())
}

/// Strip the single top-level dir from a tar entry path and reject anything
/// that would escape the destination. Returns the safe relative path, or
/// `None` for the top-level dir entry itself (nothing to write). Because the
/// download source is user-influenced via `[circuits].url_template`, an entry
/// like `root/../../etc/x` (which strips to `../../etc/x`) must not be joined
/// onto `dest`.
fn safe_stripped_entry_path(path: &Path) -> DynResult<Option<PathBuf>> {
    let mut components = path.components();
    components.next();
    let stripped: PathBuf = components.as_path().to_path_buf();
    if stripped.as_os_str().is_empty() {
        return Ok(None);
    }
    if stripped.is_absolute()
        || stripped
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!(
            "refusing to extract circuits tar entry with unsafe path `{}`",
            stripped.display()
        );
    }
    Ok(Some(stripped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn installed_version_matches_normalizes_v_prefix_and_detects_bumps() {
        let tmp = tempfile::tempdir().unwrap();
        // No VERSION marker → don't force a re-download.
        assert!(installed_version_matches(tmp.path(), "0.4.1"));
        // A leading `v` on either side must not trigger a re-download loop.
        fs::write(tmp.path().join("VERSION"), "v0.4.1\n").unwrap();
        assert!(installed_version_matches(tmp.path(), "0.4.1"));
        fs::write(tmp.path().join("VERSION"), "0.4.1").unwrap();
        assert!(installed_version_matches(tmp.path(), "v0.4.1"));
        // A genuine version bump is detected so the stale tree is replaced.
        assert!(!installed_version_matches(tmp.path(), "0.4.2"));
    }

    #[test]
    fn safe_stripped_entry_path_strips_top_dir_and_rejects_traversal() {
        assert_eq!(
            safe_stripped_entry_path(Path::new("release-root/pol/vk.json")).unwrap(),
            Some(PathBuf::from("pol/vk.json"))
        );
        // Top-level dir entry → nothing to write.
        assert_eq!(
            safe_stripped_entry_path(Path::new("release-root")).unwrap(),
            None
        );
        // A `..` that escapes `dest` after stripping is rejected.
        assert!(safe_stripped_entry_path(Path::new("release-root/../../escape")).is_err());
    }

    fn make_tarball() -> Vec<u8> {
        let mut header = tar::Header::new_gnu();
        header.set_size(8);
        header.set_mode(0o644);
        header.set_cksum();
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder
                .append_data(
                    &mut header.clone(),
                    "release-root/pol/verification_key.json",
                    &b"{\"k\":1}"[..],
                )
                .unwrap();
            // Same payload size, different file. Header reuse keeps the size
            // consistent without recomputing checksum metadata.
            builder
                .append_data(
                    &mut header.clone(),
                    "release-root/zksign/verification_key.json",
                    &b"{\"k\":2}"[..],
                )
                .unwrap();
            builder.finish().unwrap();
        }
        let mut gz = Vec::new();
        {
            let mut encoder =
                flate2::write::GzEncoder::new(&mut gz, flate2::Compression::default());
            encoder.write_all(&tar_bytes).unwrap();
            encoder.finish().unwrap();
        }
        gz
    }

    #[test]
    fn extract_tarball_strips_release_root_dir() {
        let tmp = tempfile::tempdir().unwrap();
        extract_tarball(&make_tarball(), tmp.path()).unwrap();
        assert!(tmp.path().join("pol/verification_key.json").is_file());
        assert!(tmp.path().join("zksign/verification_key.json").is_file());
        // The release-root prefix must not survive — otherwise
        // `LOGOS_BLOCKCHAIN_CIRCUITS=<dest>` wouldn't point at `pol/...`.
        assert!(!tmp.path().join("release-root").exists());
    }

    #[test]
    fn ensure_release_refuses_to_wipe_non_circuits_dir() {
        // A mistyped `[circuits].install_dir` pointing at a non-empty,
        // non-circuits directory must bail rather than `remove_dir_all` it.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("important");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("my-notes.txt"), "do not delete").unwrap();
        let triple = release_triple().expect("supported test platform");

        let err = ensure_circuits_release_at(&dir, "9.9.9", triple, None).unwrap_err();
        assert!(
            err.to_string().contains("refusing to delete"),
            "expected refusal, got: {err}"
        );
        // The user's file is untouched (we bailed before deleting or downloading).
        assert!(dir.join("my-notes.txt").is_file());
    }

    #[test]
    fn circuits_install_dir_joins_relative_path_to_project_root() {
        let cfg = CircuitsConfig {
            install_dir: "vendor/circuits".to_string(),
            ..CircuitsConfig::default()
        };
        assert_eq!(
            circuits_install_dir(Path::new("/project"), &cfg),
            PathBuf::from("/project/vendor/circuits")
        );
    }

    #[test]
    fn custom_url_template_replaces_version_and_triple() {
        let url = circuits_release_url(
            "9.9.9",
            "linux-x86_64",
            Some("https://example.invalid/v{version}/circuits-{triple}.tar.gz"),
        );
        assert_eq!(
            url,
            "https://example.invalid/v9.9.9/circuits-linux-x86_64.tar.gz"
        );
    }

    #[test]
    fn release_url_normalizes_leading_v_in_version() {
        // A `v`-prefixed `[circuits].version` must not yield a `vv0.4.1` URL.
        let bare = circuits_release_url("0.4.1", "linux-x86_64", None);
        let prefixed = circuits_release_url("v0.4.1", "linux-x86_64", None);
        assert_eq!(bare, prefixed);
        assert!(!prefixed.contains("vv0.4.1"), "{prefixed}");
        // Same normalization applies to a custom template.
        let tmpl = circuits_release_url(
            "v0.4.1",
            "linux-x86_64",
            Some("https://example.invalid/v{version}/c-{triple}.tar.gz"),
        );
        assert_eq!(tmpl, "https://example.invalid/v0.4.1/c-linux-x86_64.tar.gz");
    }

    #[test]
    fn circuits_path_from_env_rejects_unpopulated_dir() {
        let tmp = tempfile::tempdir().unwrap();
        // Saved/restored guard so this test doesn't leak an env var into
        // sibling tests in the same process.
        let prev = std::env::var(LOGOS_BLOCKCHAIN_CIRCUITS_ENV).ok();
        std::env::set_var(LOGOS_BLOCKCHAIN_CIRCUITS_ENV, tmp.path());
        let resolved = circuits_path_from_env();
        match prev {
            Some(v) => std::env::set_var(LOGOS_BLOCKCHAIN_CIRCUITS_ENV, v),
            None => std::env::remove_var(LOGOS_BLOCKCHAIN_CIRCUITS_ENV),
        }
        assert!(
            resolved.is_none(),
            "stale env-var dirs must not be returned — fall through to install path"
        );
    }
}
