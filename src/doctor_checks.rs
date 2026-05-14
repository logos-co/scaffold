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

/// Probe whether the host environment has the LEZ sequencer's runtime
/// prerequisites: a risc0 `r0vm` resolvable by `risc0-zkvm` (via
/// `RISC0_SERVER_PATH`, the rzup extensions layout, or `r0vm` on `PATH`)
/// and the `logos-blockchain-circuits` directory referenced by zksign.
///
/// These are not checked elsewhere because they live outside the project
/// tree, but without them the sequencer panics on its first block-settlement
/// path (zk signature) or its first risc0 program execution, producing
/// opaque `ProgramExecutionFailed` / `Main loop exited unexpectedly` errors
/// instead of an actionable message.
pub(crate) fn check_r0vm() -> CheckRow {
    if let Some(found) = locate_r0vm() {
        return CheckRow {
            status: CheckStatus::Pass,
            name: "risc0 r0vm".to_string(),
            detail: format!("found {}", found.display()),
            remediation: None,
        };
    }
    CheckRow {
        status: CheckStatus::Fail,
        name: "risc0 r0vm".to_string(),
        detail: "r0vm not found via RISC0_SERVER_PATH, rzup, or PATH".to_string(),
        remediation: Some(
            "Install rzup (https://risczero.com/install) and run \
             `rzup install r0vm`, or set RISC0_SERVER_PATH to a r0vm binary."
                .to_string(),
        ),
    }
}

pub(crate) fn check_logos_blockchain_circuits() -> CheckRow {
    let env_path = std::env::var("LOGOS_BLOCKCHAIN_CIRCUITS")
        .ok()
        .map(PathBuf::from);
    let candidate = env_path.clone().unwrap_or_else(|| {
        dirs_home()
            .unwrap_or_default()
            .join(".logos-blockchain-circuits")
    });

    if candidate.is_dir() {
        return CheckRow {
            status: CheckStatus::Pass,
            name: "logos-blockchain-circuits".to_string(),
            detail: format!("found {}", candidate.display()),
            remediation: None,
        };
    }
    let source = if env_path.is_some() {
        "LOGOS_BLOCKCHAIN_CIRCUITS"
    } else {
        "~/.logos-blockchain-circuits"
    };
    CheckRow {
        status: CheckStatus::Fail,
        name: "logos-blockchain-circuits".to_string(),
        detail: format!("missing {} ({})", candidate.display(), source),
        remediation: Some(
            "Install with `curl -sSL https://raw.githubusercontent.com/\
             logos-blockchain/logos-blockchain/main/scripts/setup-logos-blockchain-circuits.sh | bash`, \
             or set LOGOS_BLOCKCHAIN_CIRCUITS to an existing release directory."
                .to_string(),
        ),
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn locate_r0vm() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("RISC0_SERVER_PATH") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    // rzup extensions layout: <RISC0_HOME>/extensions/v<ver>-cargo-risczero-<platform>/r0vm
    // (R0Vm's parent component is CargoRiscZero, so the binary lives under
    //  the cargo-risczero version dir, not a r0vm-* dir.)
    if let Some(risc0_home) = std::env::var_os("RISC0_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs_home().map(|h| h.join(".risc0")))
    {
        let ext = risc0_home.join("extensions");
        if let Ok(entries) = fs::read_dir(&ext) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains("-cargo-risczero-") || name.contains("-r0vm-") {
                    let candidate = entry.path().join("r0vm");
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    which("r0vm")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::model::CheckStatus;

    use super::container_runtime_row;

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
}
