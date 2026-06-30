use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::circuits::{circuits_install_dir, version_eq};
use crate::constants::{DEFAULT_CIRCUITS_VERSION, DEFAULT_LEZ, LOGOS_BLOCKCHAIN_CIRCUITS_ENV};
use crate::model::{CheckRow, CheckStatus, CircuitsConfig};
use crate::process::{port_open, which};
use crate::repo::{git_clean, git_head_sha};

pub(crate) fn check_binary(binary: &str, required: bool) -> CheckRow {
    if let Some(path) = which(binary) {
        CheckRow {
            status: CheckStatus::Pass,
            name: format!("tool {binary}"),
            detail: format!("found {}", path.display()),
            remediation: None,
        }
    } else {
        CheckRow {
            status: if required {
                CheckStatus::Fail
            } else {
                CheckStatus::Warn
            },
            name: format!("tool {binary}"),
            detail: "not found on PATH".to_string(),
            remediation: Some(match binary {
                "wallet" => "Run `cargo install --path wallet --force`".to_string(),
                _ => format!("Install `{binary}`"),
            }),
        }
    }
}

pub(crate) fn check_container_runtime() -> CheckRow {
    container_runtime_row(which("docker"), which("podman"))
}

fn container_runtime_row(docker: Option<PathBuf>, podman: Option<PathBuf>) -> CheckRow {
    match (docker, podman) {
        (Some(path), _) => CheckRow {
            status: CheckStatus::Pass,
            name: "container runtime".to_string(),
            detail: format!("found docker at {}", path.display()),
            remediation: None,
        },
        (None, Some(path)) => CheckRow {
            status: CheckStatus::Pass,
            name: "container runtime".to_string(),
            detail: format!("found podman at {}", path.display()),
            remediation: None,
        },
        (None, None) => CheckRow {
            status: CheckStatus::Warn,
            name: "container runtime".to_string(),
            detail: "neither docker nor podman found on PATH".to_string(),
            remediation: Some(
                "Install Docker or Podman (required for guest builds that use risc0 tooling)"
                    .to_string(),
            ),
        },
    }
}

pub(crate) fn check_repo(name: &str, path: &Path, pin: &str) -> CheckRow {
    if !path.exists() {
        return CheckRow {
            status: CheckStatus::Fail,
            name: format!("repo {name}"),
            detail: format!("missing {}", path.display()),
            remediation: Some("Run `logos-scaffold setup`".to_string()),
        };
    }

    match git_head_sha(path) {
        Ok(head) => {
            let mut status = if head == pin {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            };

            let mut detail = format!("pin={pin}, head={head}");
            if let Ok(clean) = git_clean(path) {
                if !clean {
                    if status == CheckStatus::Pass {
                        status = CheckStatus::Warn;
                    }
                    detail.push_str("; working tree dirty");
                }
            }

            CheckRow {
                status,
                name: format!("repo {name}"),
                detail,
                remediation: if status == CheckStatus::Fail {
                    Some("Run `logos-scaffold setup`".to_string())
                } else {
                    None
                },
            }
        }
        Err(err) => CheckRow {
            status: CheckStatus::Fail,
            name: format!("repo {name}"),
            detail: err.to_string(),
            remediation: Some("Ensure repo path is valid git repository".to_string()),
        },
    }
}

pub(crate) fn check_path(name: &str, path: &Path, remediation: &str) -> CheckRow {
    if path.exists() {
        CheckRow {
            status: CheckStatus::Pass,
            name: name.to_string(),
            detail: format!("found {}", path.display()),
            remediation: None,
        }
    } else {
        CheckRow {
            status: CheckStatus::Fail,
            name: name.to_string(),
            detail: format!("missing {}", path.display()),
            remediation: Some(remediation.to_string()),
        }
    }
}

pub(crate) fn check_port_warn(name: &str, addr: &str, remediation: &str) -> CheckRow {
    if port_open(addr) {
        CheckRow {
            status: CheckStatus::Pass,
            name: name.to_string(),
            detail: format!("{addr} reachable"),
            remediation: None,
        }
    } else {
        CheckRow {
            status: CheckStatus::Warn,
            name: name.to_string(),
            detail: format!("{addr} not reachable"),
            remediation: Some(remediation.to_string()),
        }
    }
}

/// Probe for the `logos-blockchain-circuits` artifact required by
/// downstream `logos-blockchain-pol` build scripts. Without this, `setup`
/// (which compiles `sequencer_service --features standalone`) panics
/// inside cargo with a raw build-script trace. Surfaced both as a doctor
/// row and as a precheck in `setup` so the user sees a single
/// scaffold-styled error before any compile work runs.
///
/// Pass when either:
/// - `LOGOS_BLOCKCHAIN_CIRCUITS` is set to a path that exists and is a directory, or
/// - the configured `[circuits].install_dir` (default `.scaffold/circuits`)
///   holds the release, with its `VERSION` matching `[circuits].version`.
///
/// A set-but-invalid env var (empty, nonexistent path, or non-directory)
/// is reported distinctly from "unset" in the failure detail — the user
/// otherwise sees "unset" when their typo'd env var is the actual cause.
/// In either case, the configured install dir is checked as the fallback.
pub(crate) fn check_logos_blockchain_circuits(
    project_root: &Path,
    config: &CircuitsConfig,
) -> CheckRow {
    let env_raw = std::env::var_os(LOGOS_BLOCKCHAIN_CIRCUITS_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let install_dir = circuits_install_dir(project_root, config);
    check_logos_blockchain_circuits_with(env_raw.as_deref(), &install_dir, config)
}

/// Pure helper: takes the resolved env-var path (or `None` when unset/empty)
/// and configured install dir, then returns the `CheckRow`. Split out so the
/// row shaping is testable without process-global env mutation.
fn check_logos_blockchain_circuits_with(
    env_path: Option<&Path>,
    install_dir: &Path,
    config: &CircuitsConfig,
) -> CheckRow {
    let sentinel = "pol/verification_key.json";

    if let Some(path) = env_path.filter(|p| p.join(sentinel).is_file()) {
        return CheckRow {
            status: CheckStatus::Pass,
            name: "logos-blockchain-circuits".to_string(),
            detail: format!(
                "found via ${LOGOS_BLOCKCHAIN_CIRCUITS_ENV} at {}",
                path.display()
            ),
            remediation: None,
        };
    }

    let env_status = env_path.map(invalid_env_status);

    if !install_dir.join(sentinel).is_file() {
        let env_detail = env_status
            .map(|s| format!("; ${LOGOS_BLOCKCHAIN_CIRCUITS_ENV} {s}"))
            .unwrap_or_default();
        return CheckRow {
            status: CheckStatus::Fail,
            name: "logos-blockchain-circuits".to_string(),
            detail: format!(
                "missing configured circuits release at {}{env_detail}",
                install_dir.display()
            ),
            remediation: Some("Run `logos-scaffold setup`".to_string()),
        };
    }

    // Mirror `circuits::installed_version_matches`: a missing/unreadable VERSION
    // file is an older/renamed layout the installer treats as acceptable (it
    // won't re-download), so `doctor` must not hard-fail on it — it just can't
    // verify the installed version.
    let version_path = install_dir.join("VERSION");
    let installed_version = fs::read_to_string(&version_path)
        .ok()
        .map(|text| text.trim().to_string());

    // Drift is only a failure when we can actually read the installed version.
    if let Some(installed) = &installed_version {
        if !version_eq(installed, &config.version) {
            return CheckRow {
                status: CheckStatus::Fail,
                name: "logos-blockchain-circuits".to_string(),
                detail: format!(
                    "installed version={} at {}; configured [circuits].version={}",
                    installed,
                    install_dir.display(),
                    config.version
                ),
                remediation: Some("Run `logos-scaffold setup`".to_string()),
            };
        }
    }

    let version_label = installed_version
        .as_deref()
        .unwrap_or("unknown (no VERSION file)");

    if !version_eq(&config.version, DEFAULT_CIRCUITS_VERSION) {
        return CheckRow {
            status: CheckStatus::Warn,
            name: "logos-blockchain-circuits".to_string(),
            detail: format!(
                "installed version={} at {}; scaffold's default circuits pin is {}",
                version_label,
                install_dir.display(),
                DEFAULT_CIRCUITS_VERSION
            ),
            remediation: Some(format!(
                "Set [circuits].version to {DEFAULT_CIRCUITS_VERSION} unless this project intentionally overrides scaffold's default circuits pin"
            )),
        };
    }

    CheckRow {
        status: CheckStatus::Pass,
        name: "logos-blockchain-circuits".to_string(),
        detail: format!(
            "installed version={} at {}",
            version_label,
            install_dir.display()
        ),
        remediation: None,
    }
}

fn invalid_env_status(path: &Path) -> String {
    if is_broken_symlink(path) {
        format!("set to broken symlink `{}`", path.display())
    } else if !path.exists() {
        format!("set to nonexistent path `{}`", path.display())
    } else if path.is_dir() {
        format!("set to unpopulated directory `{}`", path.display())
    } else {
        format!("set to non-directory `{}`", path.display())
    }
}

fn is_broken_symlink(path: &Path) -> bool {
    !path.exists()
        && fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
}

pub(crate) fn check_standalone_support(lez_path: &Path) -> CheckRow {
    let files = [
        lez_path.join("Cargo.toml"),
        lez_path.join("sequencer/service/Cargo.toml"),
        lez_path.join("README.md"),
    ];

    for path in files {
        if let Ok(text) = fs::read_to_string(path) {
            if text.contains("standalone") {
                return CheckRow {
                    status: CheckStatus::Pass,
                    name: "standalone support marker".to_string(),
                    detail: "found `standalone` marker in lez repository".to_string(),
                    remediation: None,
                };
            }
        }
    }

    CheckRow {
        status: CheckStatus::Fail,
        name: "standalone support marker".to_string(),
        detail: "could not find `standalone` marker in lez repo".to_string(),
        remediation: Some(format!(
            "Use a logos-execution-zone source that contains standalone mode and pin {}",
            DEFAULT_LEZ.sha
        )),
    }
}

pub(crate) fn print_rows(rows: &[CheckRow]) {
    println!("STATUS | CHECK | DETAILS");
    println!("-------|-------|--------");

    for row in rows {
        let status = match row.status {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warn => "WARN",
            CheckStatus::Fail => "FAIL",
        };
        println!("{status} | {} | {}", row.name, one_line(&row.detail));
        if matches!(row.status, CheckStatus::Warn | CheckStatus::Fail) {
            if let Some(remediation) = &row.remediation {
                println!("  remediation: {remediation}");
            }
        }
    }
}

pub(crate) fn one_line(text: &str) -> String {
    text.replace('\n', " ").replace('\r', " ")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::constants::DEFAULT_CIRCUITS_VERSION;
    use crate::model::{CheckStatus, CircuitsConfig};
    use std::fs;
    use tempfile::tempdir;

    use super::{check_logos_blockchain_circuits_with, container_runtime_row};

    #[test]
    fn container_runtime_row_prefers_docker() {
        let row = container_runtime_row(
            Some(PathBuf::from("/usr/local/bin/docker")),
            Some(PathBuf::from("/usr/local/bin/podman")),
        );
        assert_eq!(row.status, CheckStatus::Pass);
        assert!(row.detail.contains("docker"));
    }

    #[test]
    fn container_runtime_row_passes_with_podman_when_docker_missing() {
        let row = container_runtime_row(None, Some(PathBuf::from("/usr/local/bin/podman")));
        assert_eq!(row.status, CheckStatus::Pass);
        assert!(row.detail.contains("podman"));
    }

    #[test]
    fn container_runtime_row_warns_when_missing() {
        let row = container_runtime_row(None, None);
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(row.detail.contains("neither docker nor podman"));
        assert!(row.remediation.is_some());
    }

    fn default_circuits_config() -> CircuitsConfig {
        CircuitsConfig::default()
    }

    fn write_circuits_dir(path: &std::path::Path, version: &str) {
        fs::create_dir_all(path.join("pol")).expect("mkdir circuits pol");
        fs::write(path.join("pol/verification_key.json"), "{}").expect("write sentinel");
        fs::write(path.join("VERSION"), version).expect("write VERSION");
    }

    #[test]
    fn circuits_check_passes_via_env_var() {
        let tmp = tempdir().expect("tempdir");
        write_circuits_dir(tmp.path(), DEFAULT_CIRCUITS_VERSION);
        let install = tmp.path().join("missing-configured-install");
        let row = check_logos_blockchain_circuits_with(
            Some(tmp.path()),
            &install,
            &default_circuits_config(),
        );
        assert_eq!(row.status, CheckStatus::Pass);
        assert!(
            row.detail.contains("$LOGOS_BLOCKCHAIN_CIRCUITS"),
            "detail must credit the env var, got: {}",
            row.detail
        );
    }

    #[test]
    fn circuits_check_passes_via_configured_install_dir() {
        let tmp = tempdir().expect("tempdir");
        let install = tmp.path().join(".scaffold/circuits");
        write_circuits_dir(&install, DEFAULT_CIRCUITS_VERSION);
        let row = check_logos_blockchain_circuits_with(None, &install, &default_circuits_config());
        assert_eq!(row.status, CheckStatus::Pass);
        assert!(row.detail.contains(install.to_str().unwrap()));
    }

    #[test]
    fn circuits_check_fails_when_configured_install_missing() {
        let tmp = tempdir().expect("tempdir");
        let missing = tmp.path().join(".scaffold/circuits");
        let row = check_logos_blockchain_circuits_with(None, &missing, &default_circuits_config());
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(
            row.detail.contains("missing configured circuits release"),
            "missing case must name configured install, got: {}",
            row.detail
        );
        assert!(row.remediation.is_some());
    }

    #[test]
    fn circuits_check_fail_distinguishes_set_but_nonexistent_path() {
        // R-Copilot-2: when the env var is set to a path that does NOT
        // exist, the detail must surface that — saying "unset" misleads
        // a user with a typo'd LOGOS_BLOCKCHAIN_CIRCUITS.
        let tmp = tempdir().expect("tempdir");
        let bogus = tmp.path().join("nonexistent");
        let missing = tmp.path().join(".scaffold/circuits");
        let row = check_logos_blockchain_circuits_with(
            Some(&bogus),
            &missing,
            &default_circuits_config(),
        );
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(
            row.detail.contains("set to nonexistent path"),
            "must distinguish nonexistent path, got: {}",
            row.detail
        );
        assert!(row.detail.contains(bogus.to_str().unwrap()));
        assert!(!row.detail.contains("unset"));
    }

    #[cfg(unix)]
    #[test]
    fn circuits_check_fail_distinguishes_broken_symlink() {
        let tmp = tempdir().expect("tempdir");
        let broken = tmp.path().join("broken-circuits-link");
        std::os::unix::fs::symlink(tmp.path().join("missing-target"), &broken)
            .expect("create broken symlink");
        let missing = tmp.path().join(".scaffold/circuits");
        let row = check_logos_blockchain_circuits_with(
            Some(&broken),
            &missing,
            &default_circuits_config(),
        );
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(
            row.detail.contains("set to broken symlink"),
            "must distinguish broken symlink, got: {}",
            row.detail
        );
        assert!(!row.detail.contains("unset"));
    }

    #[test]
    fn circuits_check_fail_distinguishes_set_to_non_directory() {
        // Env var pointing at a regular file (not a directory) is the
        // other "set but invalid" shape — also must not say "unset".
        let tmp = tempdir().expect("tempdir");
        let file_path = tmp.path().join("a-file");
        fs::write(&file_path, "not a directory").expect("write file");
        let missing = tmp.path().join(".scaffold/circuits");
        let row = check_logos_blockchain_circuits_with(
            Some(&file_path),
            &missing,
            &default_circuits_config(),
        );
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(
            row.detail.contains("set to non-directory"),
            "must distinguish non-directory, got: {}",
            row.detail
        );
        assert!(!row.detail.contains("unset"));
    }

    #[test]
    fn circuits_check_fails_on_installed_version_drift() {
        let tmp = tempdir().expect("tempdir");
        let install = tmp.path().join(".scaffold/circuits");
        write_circuits_dir(&install, "9.9.9");
        let row = check_logos_blockchain_circuits_with(None, &install, &default_circuits_config());
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(row.detail.contains("installed version=9.9.9"), "{row:?}");
        assert!(row.detail.contains(DEFAULT_CIRCUITS_VERSION), "{row:?}");
    }

    #[test]
    fn circuits_check_passes_when_version_file_is_absent() {
        // An install with the sentinel but no VERSION file is an older/renamed
        // layout the installer accepts (`installed_version_matches` returns
        // true), so doctor must not hard-fail on it.
        let tmp = tempdir().expect("tempdir");
        let install = tmp.path().join(".scaffold/circuits");
        fs::create_dir_all(install.join("pol")).expect("mkdir pol");
        fs::write(install.join("pol/verification_key.json"), "{}").expect("write sentinel");
        // deliberately no VERSION file
        let row = check_logos_blockchain_circuits_with(None, &install, &default_circuits_config());
        assert_eq!(row.status, CheckStatus::Pass, "{row:?}");
    }

    #[test]
    fn circuits_check_passes_when_version_differs_only_by_v_prefix() {
        // A `v`-prefixed `[circuits].version` against an unprefixed installed
        // VERSION (or vice versa) is NOT drift — the install path already
        // normalises the leading `v`, so `doctor` must not cry false drift or
        // a spurious LEZ-pin warning.
        let tmp = tempdir().expect("tempdir");
        let install = tmp.path().join(".scaffold/circuits");
        write_circuits_dir(&install, DEFAULT_CIRCUITS_VERSION);
        let cfg = CircuitsConfig {
            version: format!("v{DEFAULT_CIRCUITS_VERSION}"),
            ..CircuitsConfig::default()
        };
        let row = check_logos_blockchain_circuits_with(None, &install, &cfg);
        assert_eq!(row.status, CheckStatus::Pass, "{row:?}");
    }

    #[test]
    fn circuits_check_warns_when_configured_version_drifts_from_lez_pin() {
        let tmp = tempdir().expect("tempdir");
        let install = tmp.path().join(".scaffold/circuits");
        write_circuits_dir(&install, "9.9.9");
        let cfg = CircuitsConfig {
            version: "9.9.9".to_string(),
            ..CircuitsConfig::default()
        };
        let row = check_logos_blockchain_circuits_with(None, &install, &cfg);
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(
            row.detail.contains("scaffold's default circuits pin is"),
            "{row:?}"
        );
        assert!(row.remediation.is_some());
    }
}
