use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::circuits::{circuits_install_dir, version_eq};
use crate::commands::build::{probe_within, ProbeOutcome, CONTAINER_INFO_TIMEOUT};
use crate::constants::{DEFAULT_CIRCUITS_VERSION, DEFAULT_LEZ, LOGOS_BLOCKCHAIN_CIRCUITS_ENV};
use crate::model::{BuildConfig, CheckRow, CheckStatus, CircuitsConfig};
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

/// The LEZ wallet links against libpcsclite (keycard support), so a missing
/// PCSC dev package breaks `setup` deep inside a cargo build, with a
/// `pcsc-sys` link error that says nothing about scaffold. Surface it here
/// instead. macOS ships PCSC in the system frameworks, so only probe on Linux.
pub(crate) fn check_pcsc_library() -> CheckRow {
    if !cfg!(target_os = "linux") {
        return CheckRow {
            status: CheckStatus::Pass,
            name: "PCSC library".to_string(),
            detail: "not required on this platform".to_string(),
            remediation: None,
        };
    }
    let found = which("pkg-config").is_some_and(|pkg_config| {
        std::process::Command::new(pkg_config)
            .args(["--exists", "libpcsclite"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    });
    if found {
        CheckRow {
            status: CheckStatus::Pass,
            name: "PCSC library".to_string(),
            detail: "pkg-config found libpcsclite".to_string(),
            remediation: None,
        }
    } else {
        CheckRow {
            status: CheckStatus::Warn,
            name: "PCSC library".to_string(),
            detail: "pkg-config could not find libpcsclite; building the LEZ wallet will fail with `Could not find a PCSC library`".to_string(),
            remediation: Some(
                "Install the PCSC headers: `sudo apt install pkgconf libpcsclite-dev` \
                 (Fedora: `pkgconf pcsc-lite-devel`, Arch: `pkgconf pcsclite`)"
                    .to_string(),
            ),
        }
    }
}

/// `cargo risczero`, the guest-build driver for spel projects.
pub(crate) fn check_cargo_risczero() -> CheckRow {
    match which("cargo-risczero") {
        Some(path) => CheckRow {
            status: CheckStatus::Pass,
            name: "cargo-risczero".to_string(),
            detail: format!("found {}", path.display()),
            remediation: None,
        },
        None => CheckRow {
            status: CheckStatus::Warn,
            name: "cargo-risczero".to_string(),
            detail: "not found on PATH; `make build` will fail with `no such command: risczero`"
                .to_string(),
            remediation: Some(
                "Install it with rzup: `curl -L https://risc0.com/install | bash` then \
                 `rzup install cargo-risczero 3.0.5`"
                    .to_string(),
            ),
        },
    }
}

/// The container runtime row, plus how `docker info` answered when docker is the
/// selected runtime (`None` otherwise). `doctor` hands that answer to
/// [`check_guest_build`], so one run waits out at most one timeout and the two
/// rows agree. It is passed down as a value and never stored: a later `doctor`
/// run, or any other caller, probes again.
pub(crate) fn check_container_runtime() -> (CheckRow, Option<ProbeOutcome>) {
    let docker = which("docker");
    let podman = which("podman");
    // A binary on PATH is not a working runtime. Every spel guest build goes
    // through `cargo risczero build`, which needs the daemon, so a stopped
    // daemon otherwise gives a clean `doctor` and a failing `build`.
    let selected = docker.as_ref().or(podman.as_ref());
    let outcome = selected.map_or(ProbeOutcome::Ok, |bin| daemon_probe(bin));
    let remote_var = selected.and_then(|bin| remote_host_var(bin));
    let docker_outcome = docker.is_some().then_some(outcome);
    let row = container_runtime_row(docker, podman, outcome, remote_var);
    (row, docker_outcome)
}

fn is_docker(bin: &Path) -> bool {
    bin.file_name().is_some_and(|name| name == "docker")
}

/// How the runtime's daemon answered `info`, probed at the exact path `which`
/// found, so the row and the probe talk about the same binary.
fn daemon_probe(bin: &Path) -> ProbeOutcome {
    daemon_probe_within(bin, CONTAINER_INFO_TIMEOUT)
}

fn daemon_probe_within(bin: &Path, timeout: std::time::Duration) -> ProbeOutcome {
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("info");
    probe_within(cmd, timeout)
}

/// Name of the environment variable that points this runtime at a remote
/// daemon, if it is set. Only the name is ever reported: the value can carry
/// credentials, and `lgs report` output gets pasted into issues.
fn remote_host_var(bin: &Path) -> Option<&'static str> {
    let name = if is_docker(bin) {
        "DOCKER_HOST"
    } else {
        "CONTAINER_HOST"
    };
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(|_| name)
}

fn container_runtime_row(
    docker: Option<PathBuf>,
    podman: Option<PathBuf>,
    outcome: ProbeOutcome,
    remote_var: Option<&str>,
) -> CheckRow {
    if let Some(path) = docker.as_ref().or(podman.as_ref()) {
        if outcome != ProbeOutcome::Ok {
            let name = if docker.is_some() { "docker" } else { "podman" };
            let detail = if outcome == ProbeOutcome::TimedOut {
                format!(
                    "found {name} at {} but `{name} info` did not answer within {}s; guest builds will fail",
                    path.display(),
                    CONTAINER_INFO_TIMEOUT.as_secs()
                )
            } else {
                format!(
                    "found {name} at {} but its daemon is not responding; guest builds will fail",
                    path.display()
                )
            };
            let remediation = match remote_var {
                Some(var) => format!(
                    "`{var}` is set, so {name} talks to a remote daemon that is down or unreachable: fix that connection, or unset `{var}` to use the local daemon, then re-run `logos-scaffold doctor`"
                ),
                None => format!(
                    "Start the {name} daemon (e.g. `sudo systemctl start {name}`) and re-run `logos-scaffold doctor`"
                ),
            };
            return CheckRow {
                status: CheckStatus::Warn,
                name: "container runtime".to_string(),
                detail,
                remediation: Some(remediation),
            };
        }
    }
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
pub(crate) fn check_guest_build(
    build: &BuildConfig,
    docker_probe: Option<ProbeOutcome>,
) -> CheckRow {
    let missing: Vec<&str> = ["cargo-risczero", "docker"]
        .into_iter()
        .filter(|tool| which(tool).is_none())
        .collect();
    let outcome = guest_probe_outcome(
        build.guest.is_deterministic(),
        &missing,
        docker_probe,
        crate::commands::build::docker_daemon_probe,
    );
    guest_build_row(build, &missing, outcome)
}

/// The `docker info` answer the `guest build` row reports. It is only worth
/// asking when the mode needs docker and the tools are there. `doctor` has
/// usually asked already for the container runtime row, so it passes that
/// answer down instead of waiting out a second timeout. Nothing is remembered
/// between calls: with no answer passed in, this asks again.
fn guest_probe_outcome(
    deterministic: bool,
    missing: &[&str],
    docker_probe: Option<ProbeOutcome>,
    ask: impl FnOnce() -> ProbeOutcome,
) -> ProbeOutcome {
    if !deterministic || !missing.is_empty() {
        return ProbeOutcome::Ok;
    }
    docker_probe.unwrap_or_else(ask)
}

/// Split from [`check_guest_build`] so the row shape is testable without
/// depending on what happens to be installed on the test machine.
fn guest_build_row(build: &BuildConfig, missing: &[&str], outcome: ProbeOutcome) -> CheckRow {
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
    if missing.is_empty() && outcome != ProbeOutcome::Ok {
        CheckRow {
            status: CheckStatus::Fail,
            name: "guest build".to_string(),
            detail: format!(
                "docker (risczero/risc0-guest-builder:{}) — {}",
                build.risc0_docker_tag,
                if outcome == ProbeOutcome::TimedOut {
                    format!(
                        "`docker info` did not answer within {}s",
                        CONTAINER_INFO_TIMEOUT.as_secs()
                    )
                } else {
                    "Docker daemon not running (`docker info` failed)".to_string()
                }
            ),
            remediation: Some(
                "Start Docker, or set [build].guest = \"local\" in scaffold.toml \
                 (non-reproducible program_id)"
                    .to_string(),
            ),
        }
    } else if missing.is_empty() {
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

/// Probe for the `logos-blockchain-circuits` artifact required by
/// downstream `logos-blockchain-pol` build scripts. Without it, builds that
/// compile `sequencer_service --features standalone` panic inside cargo with
/// a raw build-script trace; this surfaces a single scaffold-styled `doctor`
/// row instead.
///
/// Pass when either:
/// - `LOGOS_BLOCKCHAIN_CIRCUITS` points at a populated circuits release (the
///   sentinel `pol/verification_key.json` is present), or
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
    let sentinel = crate::circuits::CIRCUITS_SENTINEL_FILE;

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
    // Probe both LEZ repo layouts: flat (v0.1.x, `sequencer/...`) and nested
    // (v0.2.0+, `lez/sequencer/...`).
    let files = [
        lez_path.join("Cargo.toml"),
        lez_path.join("sequencer/service/Cargo.toml"),
        lez_path.join("lez/sequencer/service/Cargo.toml"),
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
    use crate::commands::build::ProbeOutcome;
    use std::path::PathBuf;

    use crate::constants::DEFAULT_CIRCUITS_VERSION;
    use crate::model::{CheckStatus, CircuitsConfig};
    use std::fs;
    use tempfile::tempdir;

    use super::{
        check_logos_blockchain_circuits_with, check_standalone_support, container_runtime_row,
        guest_build_row,
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
            ProbeOutcome::Ok,
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
        let row = guest_build_row(&build, &[], ProbeOutcome::Ok);
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
        let row = guest_build_row(&build, &["cargo-risczero", "docker"], ProbeOutcome::Ok);
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

    /// Tools on PATH but no daemon: `lgs build` refuses at `docker info`, so
    /// doctor must not report the default build as ready.
    #[test]
    fn guest_build_row_fails_when_docker_daemon_is_down() {
        let build = BuildConfig {
            guest: GuestBuildMode::Docker,
            ..BuildConfig::default()
        };
        let row = guest_build_row(&build, &[], ProbeOutcome::Failed);
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(
            row.detail.contains("daemon not running"),
            "got: {}",
            row.detail
        );
        assert!(
            row.detail.contains("risc0-guest-builder:"),
            "got: {}",
            row.detail
        );
        assert!(row.remediation.is_some());

        let local = BuildConfig {
            guest: GuestBuildMode::Local,
            ..BuildConfig::default()
        };
        assert_eq!(
            guest_build_row(&local, &[], ProbeOutcome::Failed).status,
            CheckStatus::Pass,
            "local mode never needs the daemon"
        );
    }

    #[test]
    fn daemon_probe_within_tells_a_hung_probe_from_a_failing_one() {
        use std::time::{Duration, Instant};

        let probe =
            |bin: &str, timeout| super::daemon_probe_within(std::path::Path::new(bin), timeout);
        let long = Duration::from_secs(5);
        assert_eq!(probe("true", long), ProbeOutcome::Ok);
        assert_eq!(probe("false", long), ProbeOutcome::Failed);
        assert_eq!(probe("/nonexistent/docker", long), ProbeOutcome::Failed);

        // `yes` never exits (it repeats its argument), standing in for a
        // `docker info` that hangs on an unreachable DOCKER_HOST.
        let started = Instant::now();
        assert_eq!(
            probe("yes", Duration::from_millis(200)),
            ProbeOutcome::TimedOut
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a hung probe must be cut off at the timeout"
        );
    }

    #[test]
    fn container_runtime_row_reports_a_timeout_and_names_the_remote_host_var() {
        let row = container_runtime_row(
            Some(PathBuf::from("/usr/bin/docker")),
            None,
            ProbeOutcome::TimedOut,
            Some("DOCKER_HOST"),
        );
        assert!(matches!(row.status, crate::model::CheckStatus::Warn));
        assert!(
            row.detail.contains("did not answer within"),
            "{}",
            row.detail
        );
        let fix = row.remediation.as_deref().unwrap();
        assert!(fix.contains("DOCKER_HOST"), "{fix}");
        assert!(
            !fix.contains("systemctl"),
            "a local start cannot fix a remote daemon: {fix}"
        );
    }

    #[test]
    fn container_runtime_row_keeps_the_local_fix_when_no_remote_host_is_set() {
        let row = container_runtime_row(
            Some(PathBuf::from("/usr/bin/docker")),
            None,
            ProbeOutcome::Failed,
            None,
        );
        assert!(row.detail.contains("not responding"), "{}", row.detail);
        let fix = row.remediation.as_deref().unwrap();
        assert!(fix.contains("systemctl start docker"), "{fix}");
    }

    #[test]
    fn guest_build_row_names_a_timeout_instead_of_a_stopped_daemon() {
        let build = BuildConfig {
            guest: GuestBuildMode::Docker,
            ..BuildConfig::default()
        };
        let row = guest_build_row(&build, &[], ProbeOutcome::TimedOut);
        assert!(matches!(row.status, crate::model::CheckStatus::Fail));
        assert!(
            row.detail.contains("did not answer within"),
            "{}",
            row.detail
        );
        assert!(!row.detail.contains("not running"), "{}", row.detail);
    }

    #[test]
    fn guest_probe_outcome_uses_the_answer_passed_down_and_never_remembers_one() {
        let asked = std::cell::Cell::new(0u32);
        let ask = || {
            asked.set(asked.get() + 1);
            ProbeOutcome::Ok
        };
        let outcome = super::guest_probe_outcome;

        // `doctor` already asked for the runtime row: reuse that answer.
        assert_eq!(
            outcome(true, &[], Some(ProbeOutcome::TimedOut), ask),
            ProbeOutcome::TimedOut
        );
        assert_eq!(asked.get(), 0);

        // Nothing passed down: ask, and ask again on every call.
        assert_eq!(outcome(true, &[], None, ask), ProbeOutcome::Ok);
        assert_eq!(outcome(true, &[], None, ask), ProbeOutcome::Ok);
        assert_eq!(asked.get(), 2);

        // Local guest builds and missing tools never need the daemon.
        assert_eq!(outcome(false, &[], None, ask), ProbeOutcome::Ok);
        assert_eq!(outcome(true, &["docker"], None, ask), ProbeOutcome::Ok);
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn container_runtime_row_prefers_docker() {
        let row = container_runtime_row(
            Some(PathBuf::from("/usr/local/bin/docker")),
            Some(PathBuf::from("/usr/local/bin/podman")),
            ProbeOutcome::Ok,
            None,
        );
        assert_eq!(row.status, CheckStatus::Pass);
        assert!(row.detail.contains("docker"));
    }

    #[test]
    fn container_runtime_row_passes_with_podman_when_docker_missing() {
        let row = container_runtime_row(
            None,
            Some(PathBuf::from("/usr/local/bin/podman")),
            ProbeOutcome::Ok,
            None,
        );
        assert_eq!(row.status, CheckStatus::Pass);
        assert!(row.detail.contains("podman"));
    }

    /// A client binary with a stopped daemon used to pass, which meant a clean
    /// `doctor` followed by a failing spel guest build.
    #[test]
    fn container_runtime_row_warns_when_daemon_is_down() {
        let row = container_runtime_row(
            Some(PathBuf::from("/usr/bin/docker")),
            None,
            ProbeOutcome::Failed,
            None,
        );
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(row.detail.contains("daemon is not responding"), "{row:?}");
        assert!(row.remediation.is_some(), "{row:?}");
    }

    #[test]
    fn container_runtime_row_warns_when_missing() {
        let row = container_runtime_row(None, None, ProbeOutcome::Ok, None);
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

    /// Write `contents` to `root/rel`, creating parent directories.
    fn write_marker_file(root: &std::path::Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(&path, contents).expect("write marker file");
    }

    /// The flat LEZ layout (v0.1.x): `sequencer/service/Cargo.toml`.
    #[test]
    fn standalone_support_found_in_flat_layout() {
        let tmp = tempdir().expect("tempdir");
        write_marker_file(
            tmp.path(),
            "sequencer/service/Cargo.toml",
            "[features]\nstandalone = []\n",
        );
        let row = check_standalone_support(tmp.path());
        assert_eq!(row.status, CheckStatus::Pass, "{row:?}");
    }

    /// The nested LEZ layout (v0.2.0+): the crate tree lives under `lez/`.
    /// This is the probe #234 missed and scaffold#240 added — without it a
    /// v0.2.0 checkout fails a check it should pass.
    #[test]
    fn standalone_support_found_in_nested_layout() {
        let tmp = tempdir().expect("tempdir");
        write_marker_file(
            tmp.path(),
            "lez/sequencer/service/Cargo.toml",
            "[features]\nstandalone = []\n",
        );
        let row = check_standalone_support(tmp.path());
        assert_eq!(row.status, CheckStatus::Pass, "{row:?}");
    }

    /// A checkout with the probed files present but no marker still fails —
    /// otherwise the two cases above would pass for the wrong reason.
    #[test]
    fn standalone_support_fails_when_no_layout_carries_the_marker() {
        let tmp = tempdir().expect("tempdir");
        write_marker_file(tmp.path(), "Cargo.toml", "[workspace]\n");
        write_marker_file(tmp.path(), "sequencer/service/Cargo.toml", "[package]\n");
        write_marker_file(
            tmp.path(),
            "lez/sequencer/service/Cargo.toml",
            "[package]\n",
        );
        write_marker_file(tmp.path(), "README.md", "# lez\n");
        let row = check_standalone_support(tmp.path());
        assert_eq!(row.status, CheckStatus::Fail, "{row:?}");
        assert!(row.remediation.is_some(), "{row:?}");
    }
}
