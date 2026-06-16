use std::fs;
use std::process::{Command, Stdio};

use anyhow::bail;

use super::wallet_support::wallet_password;
use crate::commands::wallet_support::WALLET_CONFIG_PRIMARY;
use crate::constants::{
    DEFAULT_LEZ, DEFAULT_SPEL, SEQUENCER_BIN_REL_PATH, SPEL_BIN_REL_PATH, WALLET_BIN_REL_PATH,
};
use crate::doctor_checks::{
    check_binary, check_container_runtime, check_logos_blockchain_circuits, check_path,
    check_port_warn, check_repo, check_standalone_support, one_line, print_rows,
};
use crate::model::{CheckRow, CheckStatus, DoctorReport, DoctorSummary, Project};
use crate::process::{pid_running, run_capture, run_with_stdin, set_command_echo};
use crate::project::{load_project, resolve_cache_root, resolve_repo_path};
use crate::state::read_localnet_state;
use crate::DynResult;

const STEP_SETUP: &str = "logos-scaffold setup";
const STEP_LOCALNET_START: &str = "logos-scaffold localnet start";
const STEP_EXPORT_WALLET_HOME: &str = "export NSSA_WALLET_HOME_DIR=$(pwd)/.scaffold/wallet";
const STEP_DOCTOR: &str = "logos-scaffold doctor";

pub(crate) fn cmd_doctor(as_json: bool) -> DynResult<()> {
    if as_json {
        set_command_echo(false);
    }

    let result = cmd_doctor_inner(as_json);

    if as_json {
        set_command_echo(true);
    }

    result
}

fn cmd_doctor_inner(as_json: bool) -> DynResult<()> {
    let project = load_project()?;
    let report = build_doctor_report(&project)?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        if report.summary.fail > 0 {
            bail!("doctor reported FAIL checks");
        }
        return Ok(());
    }

    print_rows(&report.checks);
    println!(
        "Summary: {} PASS, {} WARN, {} FAIL",
        report.summary.pass, report.summary.warn, report.summary.fail
    );
    println!("Doctor status: {}", report.status);

    if !report.next_steps.is_empty() {
        println!("Next steps:");
        for step in report.next_steps {
            println!("- {step}");
        }
    }

    if report.summary.fail > 0 {
        bail!("doctor reported FAIL checks");
    }

    Ok(())
}

pub(crate) fn build_doctor_report(project: &Project) -> DynResult<DoctorReport> {
    let lez = resolve_repo_path(project, &project.config.lez, "lez")?;
    let spel = resolve_repo_path(project, &project.config.spel, "spel")?;
    let wallet_home = project.root.join(&project.config.wallet_home_dir);
    let localnet_state_path = project.root.join(".scaffold/state/localnet.state");

    let mut rows = vec![
        check_binary("git", true),
        check_binary("rustc", true),
        check_binary("cargo", true),
        check_binary("lsof", true),
        check_binary("ps", true),
        check_binary("kill", true),
        check_container_runtime(),
        check_binary("nix", false),
        check_logos_blockchain_circuits(),
    ];

    rows.push(check_repo("lez", &lez, &project.config.lez.pin));

    // Drift check vs. the scaffold-shipped default — distinct from
    // `check_repo("lez", …)` above, which only validates the on-disk clone
    // is at the *configured* pin (whatever the user wrote in scaffold.toml).
    let lez_pin_matches = project.config.lez.pin == DEFAULT_LEZ.sha;
    rows.push(CheckRow {
        status: if lez_pin_matches {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        name: "lez pin matches scaffold default".to_string(),
        detail: format!(
            "configured pin={} expected={}",
            project.config.lez.pin, DEFAULT_LEZ.sha
        ),
        remediation: if lez_pin_matches {
            None
        } else {
            Some(format!(
                "Set repos.lez.pin in scaffold.toml to {} and run `{}`",
                DEFAULT_LEZ.sha, STEP_SETUP
            ))
        },
    });

    rows.push(check_standalone_support(&lez));

    rows.push(check_repo("spel", &spel, &project.config.spel.pin));

    let spel_pin_matches = project.config.spel.pin == DEFAULT_SPEL.sha;
    rows.push(CheckRow {
        status: if spel_pin_matches {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        name: "spel pin matches scaffold default".to_string(),
        detail: format!(
            "configured pin={} expected={} ({})",
            project.config.spel.pin, DEFAULT_SPEL.sha, DEFAULT_SPEL.tag
        ),
        remediation: if spel_pin_matches {
            None
        } else {
            Some(format!(
                "Set repos.spel.pin in scaffold.toml to {} ({}) and run `{}`",
                DEFAULT_SPEL.sha, DEFAULT_SPEL.tag, STEP_SETUP
            ))
        },
    });

    rows.push(check_path(
        "spel binary",
        &spel.join(SPEL_BIN_REL_PATH),
        "Run `logos-scaffold setup`",
    ));

    rows.push(check_spel_lez_alignment(&spel));

    let (resolved_cache_root, cache_root_source) = resolve_cache_root(project)?;
    rows.push(CheckRow {
        status: CheckStatus::Pass,
        name: "cache root".to_string(),
        detail: format!(
            "{} (from {})",
            resolved_cache_root.display(),
            cache_root_source.label()
        ),
        remediation: None,
    });

    rows.push(check_path(
        "sequencer binary",
        &lez.join(SEQUENCER_BIN_REL_PATH),
        "Run `logos-scaffold setup`",
    ));

    let wallet_binary_path = lez.join(WALLET_BIN_REL_PATH);
    rows.push(check_path(
        "wallet binary",
        &wallet_binary_path,
        "Run `logos-scaffold setup`",
    ));

    // Probe the sequencer port the project actually uses (`[localnet] port`,
    // default 3040). `deploy`/`wallet` derive the same address via
    // `default_sequencer_http_url_for_project`, so a custom port keeps the
    // diagnostic aligned with where those commands really connect.
    let localnet_port = project.config.localnet.port;
    rows.push(check_port_warn(
        &format!("sequencer port {localnet_port}"),
        &format!("127.0.0.1:{localnet_port}"),
        "Run `logos-scaffold localnet start` (required before running example binaries)",
    ));

    if localnet_state_path.exists() {
        match read_localnet_state(&localnet_state_path) {
            Ok(state) => {
                let (status, detail, remediation) = match state.sequencer_pid {
                    Some(pid) => {
                        let running = pid_running(pid);
                        let status = if running {
                            CheckStatus::Pass
                        } else {
                            CheckStatus::Warn
                        };
                        let remediation = if running {
                            None
                        } else {
                            Some("Run `logos-scaffold localnet start` (required before running example binaries)".to_string())
                        };
                        (status, format!("sequencer pid={pid} running={running}"), remediation)
                    }
                    None => (
                        CheckStatus::Warn,
                        "state file present but sequencer pid missing".to_string(),
                        Some("Run `logos-scaffold localnet start` (required before running example binaries)".to_string()),
                    ),
                };

                rows.push(CheckRow {
                    status,
                    name: "runtime state file".to_string(),
                    detail,
                    remediation,
                });
            }
            Err(err) => rows.push(CheckRow {
                status: CheckStatus::Warn,
                name: "runtime state file".to_string(),
                detail: err.to_string(),
                remediation: Some(
                    "Recreate state via `logos-scaffold localnet start` (required before running example binaries)"
                        .to_string(),
                ),
            }),
        }
    } else {
        rows.push(CheckRow {
            status: CheckStatus::Warn,
            name: "runtime state file".to_string(),
            detail: "missing .scaffold/state/localnet.state".to_string(),
            remediation: Some(
                "Run `logos-scaffold localnet start` (required before running example binaries)"
                    .to_string(),
            ),
        });
    }

    let wallet_cfg = wallet_home.join(WALLET_CONFIG_PRIMARY);
    if wallet_cfg.exists() {
        let cfg_text = fs::read_to_string(&wallet_cfg)?;
        let points_local = cfg_text.contains(&format!("127.0.0.1:{localnet_port}"))
            || cfg_text.contains(&format!("localhost:{localnet_port}"));
        if points_local {
            rows.push(CheckRow {
                status: CheckStatus::Pass,
                name: "wallet network config".to_string(),
                detail: "wallet points to local sequencer".to_string(),
                remediation: None,
            });
        } else {
            rows.push(CheckRow {
                status: CheckStatus::Warn,
                name: "wallet network config".to_string(),
                detail: "wallet may point to non-local sequencer".to_string(),
                remediation: Some(format!(
                    "Set .scaffold/wallet/wallet_config.json sequencer_addr=http://127.0.0.1:{localnet_port}"
                )),
            });
        }
    } else {
        rows.push(CheckRow {
            status: CheckStatus::Warn,
            name: "wallet network config".to_string(),
            detail: "missing .scaffold/wallet/wallet_config.json".to_string(),
            remediation: Some("Run `logos-scaffold setup`".to_string()),
        });
    }

    if wallet_binary_path.exists() {
        let mut version_cmd = Command::new(&wallet_binary_path);
        version_cmd.arg("--version");
        match run_capture(&mut version_cmd, "wallet --version") {
            Ok(out) => rows.push(CheckRow {
                status: CheckStatus::Pass,
                name: "wallet version".to_string(),
                detail: one_line(&out.stdout),
                remediation: None,
            }),
            Err(err) => rows.push(CheckRow {
                status: CheckStatus::Warn,
                name: "wallet version".to_string(),
                detail: err.to_string(),
                remediation: Some("Ensure wallet binary is healthy".to_string()),
            }),
        }

        let mut health_cmd = Command::new(&wallet_binary_path);
        health_cmd
            .env("NSSA_WALLET_HOME_DIR", wallet_home.display().to_string())
            .arg("check-health")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        match run_with_stdin(health_cmd, format!("{}\n", wallet_password())) {
            Ok(out) => {
                if out.status.success() {
                    rows.push(CheckRow {
                        status: CheckStatus::Pass,
                        name: "wallet usability".to_string(),
                        detail: "wallet check-health succeeded".to_string(),
                        remediation: None,
                    });
                } else if is_localnet_connectivity_failure(&out.stdout, &out.stderr, localnet_port)
                {
                    rows.push(CheckRow {
                        status: CheckStatus::Warn,
                        name: "wallet usability".to_string(),
                        detail: format!(
                            "wallet cannot reach local sequencer at http://127.0.0.1:{localnet_port}"
                        ),
                        remediation: Some(
                            "Run `logos-scaffold localnet start` (required before running example binaries), then `logos-scaffold doctor`"
                                .to_string(),
                        ),
                    });
                } else {
                    rows.push(CheckRow {
                        status: CheckStatus::Fail,
                        name: "wallet usability".to_string(),
                        detail: one_line(&out.stderr),
                        remediation: Some(
                            "Verify wallet config and run `export NSSA_WALLET_HOME_DIR=$(pwd)/.scaffold/wallet`, then `logos-scaffold doctor`"
                                .to_string(),
                        ),
                    });
                }
            }
            Err(err) => rows.push(CheckRow {
                status: CheckStatus::Fail,
                name: "wallet usability".to_string(),
                detail: err.to_string(),
                remediation: Some(
                    "Verify wallet binary and run `export NSSA_WALLET_HOME_DIR=$(pwd)/.scaffold/wallet`, then `logos-scaffold doctor`"
                        .to_string(),
                ),
            }),
        }
    }

    Ok(finalize_report(rows))
}

/// Roll up rows into a `DoctorReport` with summary, status, and next-steps.
/// Shared by top-level `doctor` and `basecamp doctor` so output formatting
/// stays identical.
pub(crate) fn finalize_report(rows: Vec<CheckRow>) -> DoctorReport {
    let summary = DoctorSummary {
        pass: rows
            .iter()
            .filter(|r| matches!(r.status, CheckStatus::Pass))
            .count(),
        warn: rows
            .iter()
            .filter(|r| matches!(r.status, CheckStatus::Warn))
            .count(),
        fail: rows
            .iter()
            .filter(|r| matches!(r.status, CheckStatus::Fail))
            .count(),
    };

    let doctor_status = if summary.fail > 0 {
        "Failing checks"
    } else if summary.warn > 0 {
        "Needs attention"
    } else {
        "Ready"
    };

    let next_steps = derive_next_steps(&rows);

    DoctorReport {
        status: doctor_status.to_string(),
        summary,
        checks: rows,
        next_steps,
    }
}

/// Print a `DoctorReport` to stdout in human-readable form. Matches what
/// `cmd_doctor` emits — same formatting for `basecamp doctor`.
pub(crate) fn print_report(report: &DoctorReport) {
    print_rows(&report.checks);
    println!(
        "Summary: {} PASS, {} WARN, {} FAIL",
        report.summary.pass, report.summary.warn, report.summary.fail
    );
    println!("Doctor status: {}", report.status);
    if !report.next_steps.is_empty() {
        println!("Next steps:");
        for step in &report.next_steps {
            println!("- {step}");
        }
    }
}

/// Verify that the spel pin scaffold builds also pins LEZ at the same
/// commit/tag scaffold itself uses (`DEFAULT_LEZ.sha` /
/// `DEFAULT_LEZ.tag`). When the two diverge, spel's sequencer-RPC client
/// speaks a different LEZ protocol than the sequencer scaffold builds,
/// which can break `lgs spel -- ...` subcommands that hit the sequencer
/// (image-ID computation via `spel inspect` is unaffected — it only
/// touches the guest ELF).
///
/// Reads `<spel_path>/spel-cli/Cargo.toml` and looks for either the SHA
/// or the tag form of scaffold's pinned LEZ. Skipped (Pass) when the
/// file isn't present yet — that means spel hasn't been cloned/built;
/// the `spel binary` row already covers that case.
fn check_spel_lez_alignment(spel_path: &std::path::Path) -> CheckRow {
    let manifest = spel_path.join("spel-cli/Cargo.toml");
    if !manifest.exists() {
        return CheckRow {
            status: CheckStatus::Pass,
            name: "spel vendors matching LEZ".to_string(),
            detail: "skipped: spel not cloned yet (run `logos-scaffold setup`)".to_string(),
            remediation: None,
        };
    }
    let text = match fs::read_to_string(&manifest) {
        Ok(t) => t,
        Err(err) => {
            return CheckRow {
                status: CheckStatus::Warn,
                name: "spel vendors matching LEZ".to_string(),
                detail: format!("could not read {}: {err}", manifest.display()),
                remediation: Some(
                    "Re-run `logos-scaffold setup` to refresh the vendored spel checkout"
                        .to_string(),
                ),
            };
        }
    };
    // spel's spel-cli/Cargo.toml references LEZ on every dep line, e.g.
    //   nssa = { git = "https://github.com/.../logos-execution-zone.git", tag = "v0.2.0-rc1" }
    //   wallet = { git = "...", rev = "ffcbc15972adbf557939bf3e2852af276422631b" }
    // Match either form against scaffold's pin.
    let aligned = text.contains(DEFAULT_LEZ.tag) || text.contains(DEFAULT_LEZ.sha);
    if aligned {
        CheckRow {
            status: CheckStatus::Pass,
            name: "spel vendors matching LEZ".to_string(),
            detail: format!(
                "spel pins LEZ at {} ({}) — matches scaffold",
                DEFAULT_LEZ.tag, DEFAULT_LEZ.sha
            ),
            remediation: None,
        }
    } else {
        CheckRow {
            status: CheckStatus::Warn,
            name: "spel vendors matching LEZ".to_string(),
            detail: format!(
                "spel-cli/Cargo.toml does not reference LEZ {} or {}; spel may speak a different sequencer-RPC protocol than scaffold's wallet/sequencer build",
                DEFAULT_LEZ.tag, DEFAULT_LEZ.sha
            ),
            remediation: Some(format!(
                "Bump repos.spel.pin to a spel commit whose spel-cli/Cargo.toml pins LEZ at {}, then run `{}`",
                DEFAULT_LEZ.tag, STEP_SETUP
            )),
        }
    }
}

// Decide whether a wallet-check failure should be downgraded from Fail to
// Warn because the local sequencer is simply unreachable. Conservative on
// purpose: doctor's job is to surface failures, so a misclassification in
// either direction has to be a real connectivity error before the user
// sees the friendly "start localnet" hint instead of the raw failure.
//
// The hard rule is that mentioning the sequencer URL is **not enough** —
// wallet binaries routinely echo the configured address in unrelated
// error contexts (RPC rejection, signature mismatch, malformed payload).
// We require *both* an explicit transport-error token *and* the address,
// so an unrelated failure that happens to print the URL is left as Fail.
fn is_localnet_connectivity_failure(stdout: &str, stderr: &str, localnet_port: u16) -> bool {
    let text = format!("{stdout}\n{stderr}").to_lowercase();

    let mentions_localnet_address = text.contains(&format!("127.0.0.1:{localnet_port}"))
        || text.contains(&format!("localhost:{localnet_port}"));

    let has_transport_error_token = text.contains("connection refused")
        || text.contains("econnrefused")
        || text.contains("tcp connect error")
        || text.contains("network is unreachable")
        || text.contains("no route to host")
        || text.contains("failed to connect")
        || text.contains("couldn't connect")
        || text.contains("connecterror")
        || text.contains("dns error");

    mentions_localnet_address && has_transport_error_token
}

fn derive_next_steps(rows: &[CheckRow]) -> Vec<String> {
    let mut has_warn_or_fail = false;
    let mut include_setup = false;
    let mut include_localnet_start = false;
    let mut include_wallet_home = false;

    for row in rows {
        if !matches!(row.status, CheckStatus::Warn | CheckStatus::Fail) {
            continue;
        }

        has_warn_or_fail = true;

        let remediation = row.remediation.as_deref().unwrap_or("");
        if remediation.contains(STEP_SETUP) {
            include_setup = true;
        }
        if remediation.contains(STEP_LOCALNET_START) {
            include_localnet_start = true;
        }
        if remediation.contains(STEP_EXPORT_WALLET_HOME)
            || remediation.contains("NSSA_WALLET_HOME_DIR")
        {
            include_wallet_home = true;
        }
    }

    let mut out = Vec::new();
    if include_setup {
        out.push(STEP_SETUP.to_string());
    }
    if include_localnet_start {
        out.push(STEP_LOCALNET_START.to_string());
    }
    if include_wallet_home {
        out.push(STEP_EXPORT_WALLET_HOME.to_string());
    }
    if has_warn_or_fail {
        out.push(STEP_DOCTOR.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_spel_cargo(spel_root: &std::path::Path, contents: &str) {
        let dir = spel_root.join("spel-cli");
        fs::create_dir_all(&dir).expect("mkdir spel-cli");
        fs::write(dir.join("Cargo.toml"), contents).expect("write Cargo.toml");
    }

    #[test]
    fn spel_lez_alignment_passes_when_spel_pins_default_lez_tag() {
        let tmp = tempdir().expect("tempdir");
        write_spel_cargo(
            tmp.path(),
            &format!(
                "[package]\nname = \"spel\"\n\n[dependencies]\n\
                 nssa = {{ git = \"https://github.com/x/lez.git\", tag = \"{}\" }}\n",
                DEFAULT_LEZ.tag,
            ),
        );
        let row = check_spel_lez_alignment(tmp.path());
        assert_eq!(row.status, CheckStatus::Pass, "got: {row:?}");
    }

    #[test]
    fn spel_lez_alignment_passes_when_spel_pins_default_lez_sha() {
        let tmp = tempdir().expect("tempdir");
        write_spel_cargo(
            tmp.path(),
            &format!(
                "[dependencies]\n\
                 wallet = {{ git = \"https://github.com/x/lez.git\", rev = \"{}\" }}\n",
                DEFAULT_LEZ.sha,
            ),
        );
        let row = check_spel_lez_alignment(tmp.path());
        assert_eq!(row.status, CheckStatus::Pass, "got: {row:?}");
    }

    #[test]
    fn spel_lez_alignment_warns_when_spel_pins_other_lez() {
        let tmp = tempdir().expect("tempdir");
        write_spel_cargo(
            tmp.path(),
            "[dependencies]\n\
             wallet = { git = \"https://github.com/x/lez.git\", rev = \"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\" }\n",
        );
        let row = check_spel_lez_alignment(tmp.path());
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(
            row.detail.contains(DEFAULT_LEZ.tag) && row.detail.contains(DEFAULT_LEZ.sha),
            "warn detail must name both expected forms: {row:?}"
        );
        assert!(row.remediation.is_some(), "must include remediation");
    }

    #[test]
    fn spel_lez_alignment_passes_silently_when_spel_not_built() {
        // No spel-cli/Cargo.toml on disk → spel hasn't been cloned;
        // the `spel binary` row covers that case, this row stays Pass.
        let tmp = tempdir().expect("tempdir");
        let row = check_spel_lez_alignment(tmp.path());
        assert_eq!(row.status, CheckStatus::Pass);
        assert!(row.detail.contains("skipped"));
    }

    #[test]
    fn connectivity_heuristic_triggers_on_refused_connection_with_address() {
        let stderr = "Error: reqwest::Error { kind: Request, url: \"http://127.0.0.1:3040/\", \
                      source: hyper::Error(Connect, ConnectError(\"tcp connect error\", \
                      Os { code: 111, kind: ConnectionRefused, message: \"Connection refused\" })) }";
        assert!(is_localnet_connectivity_failure("", stderr, 3040));
    }

    #[test]
    fn connectivity_heuristic_triggers_on_localhost_alias_with_econnrefused() {
        let stderr = "wallet: rpc call to http://localhost:3040 failed: ECONNREFUSED";
        assert!(is_localnet_connectivity_failure("", stderr, 3040));
    }

    #[test]
    fn connectivity_heuristic_does_not_trigger_on_signature_mismatch_echoing_address() {
        // Genuine non-connectivity failure: the sequencer answered, rejected
        // the payload, and the wallet echoed the target URL in its error
        // context. This must stay Fail, not get downgraded to Warn.
        let stderr = "Error: rpc call to http://127.0.0.1:3040/ failed: \
                      signature mismatch for sender 0xabcd...";
        assert!(!is_localnet_connectivity_failure("", stderr, 3040));
    }

    #[test]
    fn connectivity_heuristic_does_not_trigger_on_malformed_payload_echoing_address() {
        // Another genuine failure shape: sequencer rejected a malformed
        // request and the URL appears in the trace. Must stay Fail.
        let stdout = "POST http://localhost:3040/ -> 400 Bad Request: invalid abi-encoded calldata";
        assert!(!is_localnet_connectivity_failure(stdout, "", 3040));
    }

    #[test]
    fn connectivity_heuristic_does_not_trigger_on_address_alone() {
        // Bare address mention with no transport-error token is the exact
        // false-positive shape we are tightening against. Issue #113.
        let stdout = "wallet check-health: target http://127.0.0.1:3040/, sender 0xdead";
        assert!(!is_localnet_connectivity_failure(stdout, "", 3040));
    }

    #[test]
    fn connectivity_heuristic_does_not_trigger_on_transport_error_without_address() {
        // Transport-error token alone, on a non-localnet endpoint, must
        // not be classified as a localnet-connectivity failure: it could
        // be a different network call entirely (proxy, external RPC).
        let stderr = "io error: connection refused while reaching https://example.com/";
        assert!(!is_localnet_connectivity_failure("", stderr, 3040));
    }

    #[test]
    fn connectivity_heuristic_is_case_insensitive() {
        // Some toolchains emit Title-Case error variants.
        let stderr = "Connection Refused (os error 111) talking to http://127.0.0.1:3040/";
        assert!(is_localnet_connectivity_failure("", stderr, 3040));
    }

    #[test]
    fn connectivity_heuristic_honors_configured_port() {
        // Issue #40: a project on a non-default port must still get the
        // friendly "start localnet" downgrade. A refused connection to the
        // configured port (14321) is a connectivity failure...
        let stderr = "tcp connect error talking to http://127.0.0.1:14321/: Connection refused";
        assert!(is_localnet_connectivity_failure("", stderr, 14321));
        // ...but the same error must NOT be mistaken for a localnet failure
        // when doctor is checking the default port 3040 — different endpoint.
        assert!(!is_localnet_connectivity_failure("", stderr, 3040));
    }
}
