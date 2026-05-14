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
