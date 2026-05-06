use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::constants::DEFAULT_LEZ;
use crate::model::{CheckRow, CheckStatus};
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
/// - `$HOME/.logos-blockchain-circuits/` exists and is a directory.
///
/// A set-but-invalid env var (empty, nonexistent path, or non-directory)
/// is reported distinctly from "unset" in the failure detail — the user
/// otherwise sees "unset" when their typo'd env var is the actual cause.
/// In either case, the home-dir probe still runs as a fallback.
pub(crate) fn check_logos_blockchain_circuits() -> CheckRow {
    let env_raw = std::env::var_os("LOGOS_BLOCKCHAIN_CIRCUITS")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let home_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.join(".logos-blockchain-circuits"));
    check_logos_blockchain_circuits_with(env_raw.as_deref(), home_dir.as_deref())
}

/// Pure helper: takes the resolved env-var path (or `None` when unset/empty)
/// and the resolved `~/.logos-blockchain-circuits` path (or `None` when
/// `$HOME` is missing), returns the `CheckRow`. Split out so the row
/// shaping is testable without process-global env mutation.
fn check_logos_blockchain_circuits_with(
    env_path: Option<&Path>,
    home_dir: Option<&Path>,
) -> CheckRow {
    if let Some(path) = env_path.filter(|p| p.is_dir()) {
        return CheckRow {
            status: CheckStatus::Pass,
            name: "logos-blockchain-circuits".to_string(),
            detail: format!("found via $LOGOS_BLOCKCHAIN_CIRCUITS at {}", path.display()),
            remediation: None,
        };
    }

    if let Some(path) = home_dir.filter(|p| p.is_dir()) {
        return CheckRow {
            status: CheckStatus::Pass,
            name: "logos-blockchain-circuits".to_string(),
            detail: format!("found at {}", path.display()),
            remediation: None,
        };
    }

    let home_hint = home_dir
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.logos-blockchain-circuits".to_string());

    let env_status = match env_path {
        None => "unset".to_string(),
        Some(p) if !p.exists() => format!("set to nonexistent path `{}`", p.display()),
        Some(p) => format!("set to non-directory `{}`", p.display()),
    };

    CheckRow {
        status: CheckStatus::Fail,
        name: "logos-blockchain-circuits".to_string(),
        detail: format!(
            "not found: $LOGOS_BLOCKCHAIN_CIRCUITS {env_status} and {home_hint} is missing"
        ),
        remediation: Some(format!(
            "Obtain the logos-blockchain-circuits release and either set LOGOS_BLOCKCHAIN_CIRCUITS=<path> or place it at {home_hint}"
        )),
    }
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

    use crate::model::CheckStatus;
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

    #[test]
    fn circuits_check_passes_via_env_var() {
        let tmp = tempdir().expect("tempdir");
        let row = check_logos_blockchain_circuits_with(Some(tmp.path()), None);
        assert_eq!(row.status, CheckStatus::Pass);
        assert!(
            row.detail.contains("$LOGOS_BLOCKCHAIN_CIRCUITS"),
            "detail must credit the env var, got: {}",
            row.detail
        );
    }

    #[test]
    fn circuits_check_passes_via_home_dir_when_env_var_unset() {
        let tmp = tempdir().expect("tempdir");
        let home_path = tmp.path().join(".logos-blockchain-circuits");
        fs::create_dir(&home_path).expect("mkdir home circuits");
        let row = check_logos_blockchain_circuits_with(None, Some(&home_path));
        assert_eq!(row.status, CheckStatus::Pass);
        assert!(row.detail.contains(home_path.to_str().unwrap()));
    }

    #[test]
    fn circuits_check_fails_with_unset_marker_when_env_unset_and_home_missing() {
        let tmp = tempdir().expect("tempdir");
        let missing_home = tmp.path().join(".logos-blockchain-circuits"); // not created
        let row = check_logos_blockchain_circuits_with(None, Some(&missing_home));
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(
            row.detail.contains("unset"),
            "unset case must say 'unset', got: {}",
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
        let missing_home = tmp.path().join(".logos-blockchain-circuits");
        let row = check_logos_blockchain_circuits_with(Some(&bogus), Some(&missing_home));
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(
            row.detail.contains("set to nonexistent path"),
            "must distinguish nonexistent path, got: {}",
            row.detail
        );
        assert!(row.detail.contains(bogus.to_str().unwrap()));
        assert!(!row.detail.contains("unset"));
    }

    #[test]
    fn circuits_check_fail_distinguishes_set_to_non_directory() {
        // Env var pointing at a regular file (not a directory) is the
        // other "set but invalid" shape — also must not say "unset".
        let tmp = tempdir().expect("tempdir");
        let file_path = tmp.path().join("a-file");
        fs::write(&file_path, "not a directory").expect("write file");
        let missing_home = tmp.path().join(".logos-blockchain-circuits");
        let row = check_logos_blockchain_circuits_with(Some(&file_path), Some(&missing_home));
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(
            row.detail.contains("set to non-directory"),
            "must distinguish non-directory, got: {}",
            row.detail
        );
        assert!(!row.detail.contains("unset"));
    }
}
