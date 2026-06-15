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
//! `ensure_circuits_for_subprocess` materialises the matching release tarball
//! into the scaffold cache and exports `LOGOS_BLOCKCHAIN_CIRCUITS` for the
//! rest of the process. It's idempotent: a populated cache dir or a pre-set
//! env var both short-circuit the download.
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

use crate::constants::{
    CIRCUITS_RELEASE_BASE_URL, DEFAULT_CIRCUITS_VERSION, LOGOS_BLOCKCHAIN_CIRCUITS_ENV,
};
use crate::process::which;
use crate::DynResult;

/// One of the per-circuit subdirectories every release tarball contains. Used
/// as a sentinel for "this directory is a populated circuits release" in
/// cache-hit checks and post-extract verification — picking a single circuit
/// (rather than `VERSION`) keeps the check robust to future releases that
/// might rename the version marker.
const CIRCUITS_SENTINEL_FILE: &str = "pol/verification_key.json";

/// Ensure `LOGOS_BLOCKCHAIN_CIRCUITS` is set in this process so subsequently
/// spawned `cargo` invocations inherit it. No-op when the env var is already
/// exported and points at a populated circuits dir (user override).
///
/// Otherwise: download the matching tagged release into
/// `<cache_root>/circuits/v<ver>-<os>-<arch>` (idempotent) and export the
/// env var pointing there. We deliberately do NOT fall back to
/// `~/.logos-blockchain-circuits/` — see the module docs for why.
pub(crate) fn ensure_circuits_for_subprocess(cache_root: &Path) -> DynResult<()> {
    if circuits_path_from_env().is_some() {
        return Ok(());
    }

    let path = ensure_circuits_release(cache_root, DEFAULT_CIRCUITS_VERSION)?;
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
/// materialised at) for `cache_root`, without downloading anything. The
/// `LOGOS_BLOCKCHAIN_CIRCUITS` env override wins when it points at a
/// populated checkout — mirroring `ensure_circuits_for_subprocess`.
pub(crate) fn circuits_dir_for_cache_root(cache_root: &Path) -> DynResult<PathBuf> {
    if let Some(path) = circuits_path_from_env() {
        return Ok(path);
    }
    let triple = release_triple()?;
    Ok(cache_root
        .join("circuits")
        .join(format!("v{DEFAULT_CIRCUITS_VERSION}-{triple}")))
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

/// Materialise (or hit the cache for) the circuits release at `version`,
/// returning the directory that contains `pol/`, `poc/`, ... — the layout
/// `LOGOS_BLOCKCHAIN_CIRCUITS` consumers expect.
fn ensure_circuits_release(cache_root: &Path, version: &str) -> DynResult<PathBuf> {
    let triple = release_triple()?;
    let dir = cache_root
        .join("circuits")
        .join(format!("v{version}-{triple}"));

    if dir.join(CIRCUITS_SENTINEL_FILE).is_file() {
        return Ok(dir);
    }

    fs::create_dir_all(&dir)
        .map_err(|e| anyhow!("create circuits cache dir {}: {e}", dir.display()))?;

    let url = format!(
        "{CIRCUITS_RELEASE_BASE_URL}/v{version}/logos-blockchain-circuits-v{version}-{triple}.tar.gz",
    );

    println!("Downloading logos-blockchain-circuits v{version} ({triple})");
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

    Ok(dir)
}

/// Map (`std::env::consts::OS`, `ARCH`) onto the suffix the
/// `logos-blockchain-circuits` GitHub release tarballs use.
///
/// Only the platforms scaffold itself supports today are listed. macOS aarch64
/// works upstream and is the dev-machine baseline; x86_64 macOS isn't shipped
/// by upstream's flake either, so we surface the same constraint here.
fn release_triple() -> DynResult<&'static str> {
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
        let mut components = path.components();
        // Drop the single top-level dir from every entry. If a tarball ever
        // ships without one (e.g. plain files at the root), preserve the
        // original layout — `unpack` will fall back to using `path`.
        components.next();
        let stripped: PathBuf = components.as_path().to_path_buf();
        let target = if stripped.as_os_str().is_empty() {
            // Top-level directory entry itself; the strip leaves nothing to
            // create, and `entry.unpack` would otherwise try to write to
            // `dest` directly.
            continue;
        } else {
            dest.join(&stripped)
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
    fn ensure_circuits_release_short_circuits_when_sentinel_present() {
        let tmp = tempfile::tempdir().unwrap();
        let triple = release_triple().expect("supported test platform");
        let preinstalled = tmp.path().join("circuits").join(format!("v9.9.9-{triple}"));
        fs::create_dir_all(preinstalled.join("pol")).unwrap();
        fs::write(preinstalled.join(CIRCUITS_SENTINEL_FILE), "{}").unwrap();

        let resolved = ensure_circuits_release(tmp.path(), "9.9.9").expect("cache hit");
        assert_eq!(resolved, preinstalled);
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
