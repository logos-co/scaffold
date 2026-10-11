use std::fs;
use std::process::{Command, Stdio};

use anyhow::bail;

use super::wallet_support::{set_wallet_home_env, wallet_password};
use crate::commands::wallet_support::WALLET_CONFIG_PRIMARY;
use crate::constants::{
    DEFAULT_LEZ, DEV_FUNDER_ACCOUNT_ID, SEQUENCER_BIN_REL_PATH, WALLET_BIN_REL_PATH,
};
use crate::doctor_checks::{
    check_binary, check_container_runtime, check_guest_build, check_path, check_port_warn,
    check_repo, check_standalone_support, one_line, print_rows,
};
use crate::model::{CheckRow, CheckStatus, DoctorReport, DoctorSummary, Project};
use crate::process::{pid_running, run_capture, run_with_stdin, set_command_echo};
use crate::project::{load_project, resolve_cache_root, resolve_repo_path};
use crate::state::read_localnet_state;
use crate::DynResult;

const STEP_SETUP: &str = "logos-scaffold setup";
const STEP_LOCALNET_START: &str = "logos-scaffold localnet start";
// The LEZ 0.3 wallet reads its home from `LEE_WALLET_HOME_DIR` only.
const STEP_EXPORT_WALLET_HOME: &str = "export LEE_WALLET_HOME_DIR=$(pwd)/.scaffold/wallet";
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
    let wallet_home = project.root.join(&project.config.wallet_home_dir);
    let localnet_state_path = project.root.join(".scaffold/state/localnet.state");

    let mut rows = Vec::new();

    rows.push(check_binary("git", true));
    rows.push(check_binary("rustc", true));
    rows.push(check_binary("cargo", true));
    rows.push(check_binary("lsof", true));
    rows.push(check_binary("ps", true));
    rows.push(check_binary("kill", true));
    rows.push(check_container_runtime());
    rows.push(check_binary("nix", false));
    rows.push(check_guest_build(&project.config.build));

    rows.push(check_repo("lez", &lez, &project.config.lez.pin));

    // Drift check vs. the scaffold-shipped default — distinct from
    // `check_repo("lez", …)` above, which only validates the on-disk clone
    // is at the *configured* pin (whatever the user wrote in scaffold.toml).
    rows.push(CheckRow {
        status: if project.config.lez.pin == DEFAULT_LEZ.sha {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        name: "lez pin matches scaffold default".to_string(),
        detail: format!(
            "configured pin={} expected={} ({})",
            project.config.lez.pin, DEFAULT_LEZ.sha, DEFAULT_LEZ.tag
        ),
        remediation: if project.config.lez.pin == DEFAULT_LEZ.sha {
            None
        } else {
            Some(format!(
                "Set repos.lez.pin in scaffold.toml to {} ({}; scaffold supports LEZ 0.3 only) \
                 and run `{}`",
                DEFAULT_LEZ.sha, DEFAULT_LEZ.tag, STEP_SETUP
            ))
        },
    });

    rows.push(check_standalone_support(&lez));

    let (resolved_cache_root, cache_root_source) = resolve_cache_root(&project)?;
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
        let sequencer_addr = serde_json::from_str::<serde_json::Value>(&cfg_text)
            .ok()
            .and_then(|cfg| super::wallet_support::wallet_config_sequencer_addr(&cfg))
            .unwrap_or_default();
        let points_local = sequencer_addr.contains(&format!("127.0.0.1:{localnet_port}"))
            || sequencer_addr.contains(&format!("localhost:{localnet_port}"));
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
                detail: format!(
                    "wallet sequencer.sequencer_addr is {:?}, not the project localnet",
                    sequencer_addr
                ),
                remediation: Some(format!(
                    "Run `{STEP_SETUP}` to point .scaffold/wallet/wallet_config.json at http://127.0.0.1:{localnet_port}"
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
        set_wallet_home_env(&mut health_cmd, &wallet_home);
        health_cmd
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
                            format!("Verify wallet config and run `{STEP_EXPORT_WALLET_HOME}`, then `logos-scaffold doctor`"),
                        ),
                    });
                }
            }
            Err(err) => rows.push(CheckRow {
                status: CheckStatus::Fail,
                name: "wallet usability".to_string(),
                detail: err.to_string(),
                remediation: Some(
                    format!("Verify wallet binary and run `{STEP_EXPORT_WALLET_HOME}`, then `logos-scaffold doctor`"),
                ),
            }),
        }

        rows.push(check_dev_funder(&wallet_binary_path, &wallet_home));
    }

    Ok(finalize_report(rows))
}

/// `wallet topup` transfers from the dev funder, so the project wallet must
/// hold its key (imported by `setup`). Listing accounts is local — it does
/// not need a running sequencer.
fn check_dev_funder(wallet_binary: &std::path::Path, wallet_home: &std::path::Path) -> CheckRow {
    let name = "dev funder in wallet".to_string();
    // `account list` creates the wallet storage when it does not exist yet;
    // doctor must not initialize key material as a side effect.
    if !wallet_home.join("storage.json").is_file() {
        return CheckRow {
            status: CheckStatus::Warn,
            name,
            detail: "wallet storage not initialized yet".to_string(),
            remediation: Some(format!("Run `{STEP_SETUP}`")),
        };
    }
    let mut list_cmd = Command::new(wallet_binary);
    set_wallet_home_env(&mut list_cmd, wallet_home);
    list_cmd.arg("account").arg("list");
    match run_with_stdin(list_cmd, format!("{}\n", wallet_password())) {
        Ok(out) if out.status.success() && out.stdout.contains(DEV_FUNDER_ACCOUNT_ID) => CheckRow {
            status: CheckStatus::Pass,
            name,
            detail: format!("Public/{DEV_FUNDER_ACCOUNT_ID} imported (source of `wallet topup`)"),
            remediation: None,
        },
        Ok(out) if out.status.success() => CheckRow {
            status: CheckStatus::Warn,
            name,
            detail:
                "the project wallet does not hold the dev funder key; `wallet topup` will import it"
                    .to_string(),
            remediation: Some(format!("Run `{STEP_SETUP}`")),
        },
        Ok(out) => CheckRow {
            status: CheckStatus::Warn,
            name,
            detail: one_line(&format!("{}{}", out.stderr, out.stdout)),
            remediation: Some(format!("Run `{STEP_SETUP}`")),
        },
        Err(err) => CheckRow {
            status: CheckStatus::Warn,
            name,
            detail: err.to_string(),
            remediation: Some(format!("Run `{STEP_SETUP}`")),
        },
    }
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
    // Remediations that map to none of the canonical steps above (`Install
    // `nix``, ``run `logos-scaffold basecamp setup``, …). Without these the
    // list collapses to the trailing re-run step alone, i.e. "next step: run
    // the command you just ran" — the check row carries the only actionable
    // text and the section that exists to surface it says nothing.
    let mut unmapped: Vec<String> = Vec::new();

    for row in rows {
        if !matches!(row.status, CheckStatus::Warn | CheckStatus::Fail) {
            continue;
        }

        has_warn_or_fail = true;

        let Some(remediation) = row.remediation.as_deref() else {
            continue;
        };
        let mut mapped = false;
        if remediation.contains(STEP_SETUP) {
            include_setup = true;
            mapped = true;
        }
        if remediation.contains(STEP_LOCALNET_START) {
            include_localnet_start = true;
            mapped = true;
        }
        if remediation.contains(STEP_EXPORT_WALLET_HOME) {
            include_wallet_home = true;
            mapped = true;
        }
        // Two rows can share one remediation (e.g. a missing tool reported by
        // more than one check); emit it once.
        if !mapped && !unmapped.iter().any(|seen| seen == remediation) {
            unmapped.push(remediation.to_string());
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
    out.extend(unmapped);
    if has_warn_or_fail {
        out.push(STEP_DOCTOR.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warn_row(name: &str, remediation: Option<&str>) -> CheckRow {
        CheckRow {
            status: CheckStatus::Warn,
            name: name.to_string(),
            detail: String::new(),
            remediation: remediation.map(str::to_string),
        }
    }

    /// A warn whose remediation is none of the three canonical steps used to
    /// produce `["logos-scaffold doctor"]` and nothing else — telling the
    /// reader to re-run the command that just printed the advice, while the
    /// only actionable text sat in the check row. `basecamp doctor` on an
    /// un-set-up project is the standing case: its sole warn remediates with
    /// `basecamp setup`.
    #[test]
    fn next_steps_carry_a_remediation_that_maps_to_no_canonical_step() {
        let steps = derive_next_steps(&[warn_row(
            "basecamp state",
            Some("run `logos-scaffold basecamp setup`"),
        )]);
        assert_eq!(
            steps,
            vec![
                "run `logos-scaffold basecamp setup`".to_string(),
                STEP_DOCTOR.to_string(),
            ],
            "an unmapped remediation must reach the next-steps list, not be dropped in favor of a bare re-run"
        );
    }

    /// Canonical steps keep leading (they are the ordered recovery path);
    /// unmapped remediations follow, deduplicated, and the re-run stays last.
    #[test]
    fn next_steps_keep_canonical_order_and_dedupe_unmapped_remediations() {
        let steps = derive_next_steps(&[
            warn_row("tool nix", Some("Install `nix`")),
            warn_row(
                "wallet usability",
                Some(format!("Run `{STEP_LOCALNET_START}`, then `{STEP_DOCTOR}`").as_str()),
            ),
            warn_row("another nix consumer", Some("Install `nix`")),
        ]);
        assert_eq!(
            steps,
            vec![
                STEP_LOCALNET_START.to_string(),
                "Install `nix`".to_string(),
                STEP_DOCTOR.to_string(),
            ]
        );
    }

    /// An all-clear report still yields no next steps, and a warn with no
    /// remediation at all still yields the re-run only — neither path gains a
    /// spurious entry from the unmapped branch.
    #[test]
    fn next_steps_stay_empty_when_every_check_passes() {
        let pass = CheckRow {
            status: CheckStatus::Pass,
            name: "tool git".to_string(),
            detail: "found".to_string(),
            remediation: None,
        };
        assert!(derive_next_steps(&[pass]).is_empty());
        assert_eq!(
            derive_next_steps(&[warn_row("no advice", None)]),
            vec![STEP_DOCTOR.to_string()]
        );
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
