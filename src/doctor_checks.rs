use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::constants::DEFAULT_LEZ;
use crate::model::{BuildConfig, CheckRow, CheckStatus};
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

/// Report the guest-build strategy, and — in `docker` mode — whether the
/// toolchain that mode needs is actually present. `docker` is the default, so
/// a project that cannot run it is a FAIL, not a WARN: `lgs build` will
/// refuse, and the fix is a one-line install (or an explicit `local` opt-out).
pub(crate) fn check_guest_build(build: &BuildConfig) -> CheckRow {
    let missing: Vec<&str> = ["cargo-risczero", "docker"]
        .into_iter()
        .filter(|tool| which(tool).is_none())
        .collect();
    guest_build_row(build, &missing)
}

/// Split from [`check_guest_build`] so the row shape is testable without
/// depending on what happens to be installed on the test machine.
fn guest_build_row(build: &BuildConfig, missing: &[&str]) -> CheckRow {
    if !build.guest.is_deterministic() {
        return CheckRow {
            status: CheckStatus::Pass,
            name: "guest build".to_string(),
            detail: "local — host risc0 toolchain (opted in via [build].guest); program_id is \
                     not reproducible across machines. Remove the setting or use \"docker\" \
                     for reproducible builds."
                .to_string(),
            remediation: None,
        };
    }

    // The pinned tag is reported on both branches: it is the value that makes
    // `program_id` reproducible, so it belongs in the diagnostic whether or
    // not this machine can currently run the build.
    if missing.is_empty() {
        CheckRow {
            status: CheckStatus::Pass,
            name: "guest build".to_string(),
            detail: format!(
                "docker — reproducible via risczero/risc0-guest-builder:{}",
                build.risc0_docker_tag
            ),
            remediation: None,
        }
    } else {
        CheckRow {
            status: CheckStatus::Fail,
            name: "guest build".to_string(),
            detail: format!(
                "docker (risczero/risc0-guest-builder:{}) — {} not found on PATH",
                build.risc0_docker_tag,
                missing.join(" and ")
            ),
            remediation: Some(
                "Install Docker and run `rzup install cargo-risczero` (a separate component \
                 from `rzup install rust`), or set [build].guest = \"local\" in scaffold.toml \
                 (non-reproducible program_id)"
                    .to_string(),
            ),
        }
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

/// Localnet and test-node run `sequencer_service` built with
/// `--features standalone` (a mock Bedrock), so the pinned checkout must
/// declare that feature at LEZ 0.3's crate path.
pub(crate) fn check_standalone_support(lez_path: &Path) -> CheckRow {
    let manifest = lez_path.join("lez/sequencer/service/Cargo.toml");
    let declares_feature = fs::read_to_string(&manifest)
        .map(|text| {
            text.lines()
                .any(|line| line.trim_start().starts_with("standalone ="))
        })
        .unwrap_or(false);
    if declares_feature {
        return CheckRow {
            status: CheckStatus::Pass,
            name: "standalone support marker".to_string(),
            detail: "sequencer_service declares the `standalone` feature".to_string(),
            remediation: None,
        };
    }

    CheckRow {
        status: CheckStatus::Fail,
        name: "standalone support marker".to_string(),
        detail: format!(
            "{} does not declare a `standalone` feature",
            manifest.display()
        ),
        remediation: Some(format!(
            "Pin [repos.lez] to a LEZ 0.3 release ({} = {}) and run `logos-scaffold setup`",
            DEFAULT_LEZ.tag, DEFAULT_LEZ.sha
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

    use super::{
        check_standalone_support, container_runtime_row, guest_build_row,
    };
    use crate::model::{BuildConfig, GuestBuildMode};

    #[test]
    fn guest_build_row_reports_local_mode_as_pass_and_says_why_it_is_not_reproducible() {
        let row = guest_build_row(
            &BuildConfig {
                guest: GuestBuildMode::Local,
                ..BuildConfig::default()
            },
            &[],
        );
        assert_eq!(row.status, CheckStatus::Pass);
        assert!(row.detail.starts_with("local"), "got: {}", row.detail);
        assert!(
            row.detail.contains("not reproducible"),
            "the local row is the only place doctor can say this; got: {}",
            row.detail
        );
    }

    #[test]
    fn guest_build_row_reports_the_pinned_tag_in_docker_mode() {
        let build = BuildConfig {
            guest: GuestBuildMode::Docker,
            risc0_docker_tag: "r0.1.91.1".to_string(),
        };
        let row = guest_build_row(&build, &[]);
        assert_eq!(row.status, CheckStatus::Pass);
        assert!(
            row.detail.contains("risc0-guest-builder:r0.1.91.1"),
            "got: {}",
            row.detail
        );
    }

    /// A project pinned to deterministic builds on a machine that cannot run
    /// them fails, not warns: `lgs build` refuses outright, so a WARN would
    /// under-report.
    #[test]
    fn guest_build_row_fails_when_docker_mode_lacks_its_toolchain() {
        let build = BuildConfig {
            guest: GuestBuildMode::Docker,
            ..BuildConfig::default()
        };
        let row = guest_build_row(&build, &["cargo-risczero", "docker"]);
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(row.detail.contains("cargo-risczero and docker"));
        // The pin is reported even when the build cannot run — see the
        // comment in `guest_build_row`.
        assert!(
            row.detail.contains(&format!(
                "risc0-guest-builder:{}",
                crate::constants::DEFAULT_RISC0_DOCKER_TAG
            )),
            "got: {}",
            row.detail
        );
        assert!(row.remediation.is_some());
    }

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

    /// Write `contents` to `root/rel`, creating parent directories.
    fn write_marker_file(root: &std::path::Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, contents).expect("write marker file");
    }

    /// LEZ 0.3 keeps the sequencer crate under `lez/`.
    #[test]
    fn standalone_support_found_at_lez_0_3_path() {
        let tmp = tempdir().expect("tempdir");
        write_marker_file(
            tmp.path(),
            "lez/sequencer/service/Cargo.toml",
            "[features]\nstandalone = [\"sequencer_bedrock_actor/mock\"]\n",
        );
        let row = check_standalone_support(tmp.path());
        assert_eq!(row.status, CheckStatus::Pass, "{row:?}");
    }

    /// A checkout with the probed files present but no marker still fails —
    /// otherwise the two cases above would pass for the wrong reason.
    #[test]
    fn standalone_support_fails_when_no_layout_carries_the_marker() {
        let tmp = tempdir().expect("tempdir");
        // A pre-0.2 flat layout that mentions the word is not enough.
        write_marker_file(
            tmp.path(),
            "sequencer/service/Cargo.toml",
            "[features]\nstandalone = []\n",
        );
        write_marker_file(
            tmp.path(),
            "lez/sequencer/service/Cargo.toml",
            "[package]\n",
        );
        let row = check_standalone_support(tmp.path());
        assert_eq!(row.status, CheckStatus::Fail, "{row:?}");
        assert!(row.remediation.is_some(), "{row:?}");
    }
}
