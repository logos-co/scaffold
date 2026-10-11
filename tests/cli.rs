use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use flate2::read::GzDecoder;
use predicates::prelude::*;
use tar::Archive;
use tempfile::tempdir;

#[cfg(unix)]
mod common;

#[cfg(unix)]
use common::test_node::{
    assert_test_node_launched_with_config, setup_test_node_project, test_node_observed_config_path,
    TestNodeFixtures,
};

const TEST_PIN: &str = "767b5afd388c7981bcdf6f5b5c80159607e07e5b";
const VALID_ACCOUNT_ID: &str = "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV";
const VALID_PUBLIC_ADDRESS: &str = "Public/6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV";
const DEFAULT_WALLET_PASSWORD: &str = "logos-scaffold-v0";
const GUEST_BIN_REL_PATH: &str =
    "target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release";

/// Minimal valid `scaffold.toml` content for tests that only need the project
/// context to exist (no basecamp section). Older tests in this file inline
/// the same content; new tests should prefer this helper.
const MINIMAL_SCAFFOLD_TOML: &str = r#"[scaffold]
version = "0.3.0"
cache_root = "cache"

[repos.lez]
source = "https://example/lez.git"
path = "lez"
pin = "deadbeef"

[wallet]
home_dir = ".scaffold/wallet"

[framework]
kind = "default"
version = "0.1.0"

[localnet]
port = 3040
risc0_dev_mode = true
"#;

fn fake_basecamp_state() -> String {
    let bin = assert_cmd::cargo::cargo_bin!("logos-scaffold");
    format!(
        "pin=deadbeef\nbasecamp_bin={}\nlgpm_bin={}\n",
        bin.display(),
        bin.display()
    )
}

#[test]
fn root_help_lists_quiet_flag_and_examples() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--quiet")
                .and(predicate::str::contains("Examples:"))
                .and(predicate::str::contains("LOGOS_SCAFFOLD_QUIET")),
        );
}

#[test]
fn deploy_help_documents_json_for_automation() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("deploy")
        .arg("--help")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--json")
                .and(predicate::str::contains("Machine-readable output"))
                .and(predicate::str::contains("LOGOS_SCAFFOLD_WALLET_PASSWORD")),
        );
}

#[test]
fn localnet_reset_help_lists_dry_run() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("localnet")
        .arg("reset")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--dry-run"));
}

#[test]
fn localnet_reset_dry_run_prints_plan_without_mutations() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_scaffold_toml(temp.path(), &lez_path);

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("localnet")
        .arg("reset")
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("dry-run: localnet reset").and(predicate::str::contains(
                format!(
                    "planned: delete localnet chain home (sequencer DB + keys) at {}",
                    temp.path().join(".scaffold/localnet").display()
                ),
            )),
        );

    let chain_home = temp.path().join(".scaffold/localnet");
    assert!(
        !chain_home.exists(),
        "dry-run must not create the chain home at {}",
        chain_home.display()
    );
}

#[test]
fn localnet_reset_without_yes_or_dry_run_refuses_destructive_default() {
    // C1: `localnet reset` with no flags must refuse rather than silently
    // wiping the sequencer DB. The error message advertises the two valid
    // shapes (`--yes` to confirm, `--dry-run` to preview) so an agent that
    // got the syntax wrong can copy-paste the corrected form.
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_scaffold_toml(temp.path(), &lez_path);
    // Seed the chain home (sequencer DB) and the deployment records so we
    // can verify the refusal deletes neither.
    let rocksdb = temp.path().join(".scaffold/localnet/rocksdb-test");
    fs::create_dir_all(&rocksdb).expect("seed rocksdb");
    fs::write(rocksdb.join("CURRENT"), b"sentinel").expect("seed db file");
    let deployments = temp.path().join(".scaffold/state/deployments.json");
    fs::create_dir_all(deployments.parent().expect("parent")).expect("create state dir");
    fs::write(&deployments, r#"{"programs": {}}"#).expect("seed deployments.json");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("localnet")
        .arg("reset")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("destructive")
                .and(predicate::str::contains("--yes"))
                .and(predicate::str::contains("--dry-run")),
        );

    assert!(
        rocksdb.join("CURRENT").exists(),
        "refusal must leave rocksdb untouched at {}",
        rocksdb.display()
    );
    assert!(
        deployments.exists(),
        "refusal must leave deployments.json untouched"
    );
}

#[test]
fn wallet_help_documents_inner_cli_discovery() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("wallet")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("wallet -- --help"));
}

#[test]
fn wallet_passthrough_works_with_leading_quiet_flag() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("--quiet")
        .arg("wallet")
        .arg("--")
        .arg("account")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains(VALID_PUBLIC_ADDRESS));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("-q")
        .arg("wallet")
        .arg("--")
        .arg("account")
        .arg("list")
        .assert()
        .success();
}

#[test]
fn wallet_passthrough_accepts_quiet_between_subcommand_and_separator() {
    // Copilot review on PR #86: `--quiet` is `global = true`, so an agent
    // would reasonably try `lgs wallet -q -- account list` (or `--quiet`).
    // The passthrough sniffer must accept the in-between form and still
    // suppress the `$ <wallet-bin>` echo line on stdout.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("-q")
        .arg("--")
        .arg("account")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains(VALID_PUBLIC_ADDRESS));
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        !stdout.contains("$ "),
        "in-between -q must suppress the `$ <cmd>` echo line, got:\n{stdout}"
    );

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("--quiet")
        .arg("--")
        .arg("account")
        .arg("list")
        .assert()
        .success();
}

#[test]
fn logos_scaffold_quiet_env_suppresses_command_echo_case_insensitively() {
    // Pins the contract advertised by `cli_help::EXAMPLES_ROOT` and the
    // `--quiet` help string: `LOGOS_SCAFFOLD_QUIET=` accepts `1`, `true`,
    // `yes`, `on` case-insensitively. Without the env var the wallet
    // passthrough echoes `$ <wallet-bin> account list` via `run_forwarded`;
    // any accepted truthy value must suppress that line.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let baseline = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["wallet", "--", "account", "list"])
        .assert()
        .success();
    let baseline_stdout = String::from_utf8_lossy(&baseline.get_output().stdout).into_owned();
    assert!(
        baseline_stdout.contains("$ "),
        "baseline (no quiet) must echo `$ <cmd>`, got:\n{baseline_stdout}"
    );

    // Each row must suppress the `$ ` echo. Mixed-case values pin the
    // case-insensitive branch — they used to slip through the `matches!`
    // arm and still echo.
    for value in ["1", "true", "TRUE", "True", "yes", "YES", "Yes", "on", "ON"] {
        let out = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
            .current_dir(temp.path())
            .env("LOGOS_SCAFFOLD_QUIET", value)
            .args(["wallet", "--", "account", "list"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
        assert!(
            !stdout.contains("$ "),
            "LOGOS_SCAFFOLD_QUIET={value} should suppress `$ ` echo, got:\n{stdout}"
        );
    }

    // Negative cases: empty / `0` / `false` / garbage must NOT suppress.
    for value in ["", "0", "false", "no", "off", "garbage"] {
        let out = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
            .current_dir(temp.path())
            .env("LOGOS_SCAFFOLD_QUIET", value)
            .args(["wallet", "--", "account", "list"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
        assert!(
            stdout.contains("$ "),
            "LOGOS_SCAFFOLD_QUIET={value:?} must NOT suppress echo, got:\n{stdout}"
        );
    }
}

#[test]
fn create_help_does_not_mutate_filesystem() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("create")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage:"));

    assert!(
        !temp.path().join("--help").exists(),
        "--help must not be treated as project name"
    );
}

#[test]
fn wallet_help_lists_list_topup_and_default_commands() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("wallet")
        .arg("--help")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("list")
                .and(predicate::str::contains("topup"))
                .and(predicate::str::contains("default")),
        );
}

#[test]
fn deploy_help_lists_optional_program_name() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("deploy")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("[PROGRAM_NAME]"));
    // Note: output includes [OPTIONS] when extra flags are present
}

#[test]
fn report_help_lists_out_and_tail_flags() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("report")
        .arg("--help")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--out")
                .and(predicate::str::contains("--tail"))
                .and(predicate::str::contains(
                    "Collect a sanitized diagnostics archive",
                )),
        );
}

#[test]
fn report_generates_default_archive_with_warning_and_manifest() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    fs::create_dir_all(temp.path().join(".scaffold/logs")).expect("create logs dir");
    fs::write(
        temp.path().join(".scaffold/logs/sequencer.log"),
        "sequencer started\n",
    )
    .expect("write sequencer log");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("report complete")
                .and(predicate::str::contains("archive:"))
                .and(predicate::str::contains(
                    "Inspect every file before sharing",
                )),
        );

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    assert!(archive_path.exists(), "expected default report archive");

    let entries = read_report_archive_entries(&archive_path);
    assert!(archive_entry_exists(&entries, "README.txt"));
    assert!(archive_entry_exists(&entries, "manifest.json"));
    assert!(archive_entry_exists(&entries, "diagnostics/doctor.json"));
    assert!(archive_entry_exists(
        &entries,
        "diagnostics/localnet-status.json"
    ));
    assert!(archive_entry_exists(
        &entries,
        "summaries/build-evidence.json"
    ));

    let readme = archive_entry_content(&entries, "README.txt");
    assert!(
        readme.contains("best-effort basis"),
        "README should include warning, got: {readme}"
    );

    let build_evidence = archive_entry_content(&entries, "summaries/build-evidence.json");
    assert!(
        build_evidence.contains("No build commands were executed"),
        "build evidence should confirm metadata-only mode, got: {build_evidence}"
    );
}

#[test]
fn report_supports_custom_output_path() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let custom_out = temp.path().join("artifacts/support-report.tar.gz");
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .arg("--out")
        .arg(&custom_out)
        .assert()
        .success();

    assert!(
        custom_out.exists(),
        "custom report output should exist at {}",
        custom_out.display()
    );
}

#[test]
fn report_excludes_wallet_files_from_archive() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let wallet_dir = temp.path().join(".scaffold/wallet");
    fs::create_dir_all(&wallet_dir).expect("create wallet dir");
    fs::write(wallet_dir.join("config.json"), "{ \"test\": true }\n").expect("write config");
    fs::write(
        wallet_dir.join("storage.json"),
        "{ \"secret_spending_key\": [1,2,3] }\n",
    )
    .expect("write storage");
    fs::write(
        wallet_dir.join("wallet_config.json"),
        "{ \"initial_accounts\": [] }\n",
    )
    .expect("write wallet config");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    let entries = read_report_archive_entries(&archive_path);
    for (path, _) in &entries {
        assert!(
            !path.contains(".scaffold/wallet/"),
            "wallet files must be excluded, found archive path: {path}"
        );
    }
}

#[test]
fn report_redacts_sensitive_values_in_logs() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    fs::create_dir_all(temp.path().join(".scaffold/logs")).expect("create logs dir");
    fs::write(
        temp.path().join(".scaffold/logs/sequencer.log"),
        "password=super-secret\napi_token=abc123\nrpc=http://user:pass@127.0.0.1:3040\n",
    )
    .expect("write sequencer log");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    let entries = read_report_archive_entries(&archive_path);
    let log_body = archive_entry_content(&entries, "logs/sequencer.log");

    assert!(!log_body.contains("super-secret"));
    assert!(!log_body.contains("abc123"));
    assert!(!log_body.contains("user:pass@"));
    assert!(
        log_body.contains("[REDACTED]"),
        "expected redaction marker in sanitized log, got: {log_body}"
    );
}

#[test]
fn report_keeps_non_utf8_logs_via_lossy_decoding() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    fs::create_dir_all(temp.path().join(".scaffold/logs")).expect("create logs dir");
    fs::write(
        temp.path().join(".scaffold/logs/sequencer.log"),
        [b'o', b'k', b'\n', 0xff, 0xfe, b'\n'],
    )
    .expect("write non-utf8 log");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    let entries = read_report_archive_entries(&archive_path);
    let log_body = archive_entry_content(&entries, "logs/sequencer.log");

    assert!(log_body.contains("ok"), "expected preserved utf8 content");
    assert!(
        log_body.contains('\u{fffd}'),
        "expected lossy replacement chars for invalid utf8, got: {log_body:?}"
    );
}

#[test]
fn report_manifest_scrubs_absolute_paths_in_warnings() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    // No wallet stub — wallet binary is missing at lez/target/release/wallet
    write_scaffold_toml(temp.path(), &lez_path);
    write_wallet_config(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    let entries = read_report_archive_entries(&archive_path);
    let manifest = archive_entry_content(&entries, "manifest.json");
    let temp_root = temp.path().to_string_lossy();

    assert!(
        !manifest.contains(temp_root.as_ref()),
        "manifest should not leak absolute project path, got: {manifest}"
    );
    assert!(
        manifest.contains("tool probe `wallet` did not succeed"),
        "manifest should contain wallet probe warning, got: {manifest}"
    );
}

#[test]
fn report_sanitizes_localnet_status_log_path() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    let entries = read_report_archive_entries(&archive_path);
    let localnet_status = archive_entry_content(&entries, "diagnostics/localnet-status.json");
    let temp_root = temp.path().to_string_lossy();

    assert!(
        !localnet_status.contains(temp_root.as_ref()),
        "localnet status should not leak project abs path, got: {localnet_status}"
    );

    let value: serde_json::Value =
        serde_json::from_str(localnet_status).expect("valid localnet status json");
    let log_path = value
        .get("log_path")
        .and_then(serde_json::Value::as_str)
        .expect("log_path string");
    assert!(
        log_path.contains("<PROJECT_ROOT>"),
        "expected scrubbed project placeholder in localnet log path, got: {log_path}"
    );
}

#[test]
fn report_sanitizes_doctor_json_paths() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    let entries = read_report_archive_entries(&archive_path);
    let doctor = archive_entry_content(&entries, "diagnostics/doctor.json");
    let temp_root = temp.path().to_string_lossy();

    assert!(
        !doctor.contains(temp_root.as_ref()),
        "doctor report should not leak absolute paths, got: {doctor}"
    );
    assert!(
        doctor.contains("<PROJECT_ROOT>"),
        "doctor report should include scrubbed placeholder for project path, got: {doctor}"
    );
}

#[test]
fn report_scrubs_tool_command_paths_in_summary() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    let entries = read_report_archive_entries(&archive_path);
    let tool_versions = archive_entry_content(&entries, "summaries/tool-versions.json");
    let temp_root = temp.path().to_string_lossy();

    assert!(
        !tool_versions.contains(temp_root.as_ref()),
        "tool summary should not leak absolute paths, got: {tool_versions}"
    );

    let value: serde_json::Value =
        serde_json::from_str(tool_versions).expect("valid tool versions json");
    let wallet = value
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.get("name").and_then(serde_json::Value::as_str) == Some("wallet"))
        })
        .expect("wallet tool row");
    let wallet_command = wallet
        .get("command")
        .and_then(serde_json::Value::as_str)
        .expect("wallet command string");
    assert!(
        wallet_command.contains("<PROJECT_ROOT>/lez/target/release/wallet"),
        "expected scrubbed wallet command path, got: {wallet_command}"
    );
}

#[test]
fn report_redacts_multiline_private_key_blocks() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    fs::create_dir_all(temp.path().join(".scaffold/logs")).expect("create logs dir");
    fs::write(
        temp.path().join(".scaffold/logs/sequencer.log"),
        "before\n-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\n-----END PRIVATE KEY-----\nafter\n",
    )
    .expect("write sequencer log");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    let entries = read_report_archive_entries(&archive_path);
    let log_body = archive_entry_content(&entries, "logs/sequencer.log");

    assert!(!log_body.contains("MIIEvQIBADANBgkqhkiG9w0BAQEFAASC"));
    assert!(!log_body.contains("-----BEGIN PRIVATE KEY-----"));
    assert!(!log_body.contains("-----END PRIVATE KEY-----"));
    assert!(
        log_body.contains("[REDACTED SENSITIVE LINE]"),
        "expected redaction markers, got: {log_body}"
    );
}

#[test]
fn report_redacts_url_userinfo_without_colon() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    fs::create_dir_all(temp.path().join(".scaffold/logs")).expect("create logs dir");
    fs::write(
        temp.path().join(".scaffold/logs/sequencer.log"),
        "fetch https://ghp_very_secret_token@github.com/logos/repo\n",
    )
    .expect("write sequencer log");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    let entries = read_report_archive_entries(&archive_path);
    let log_body = archive_entry_content(&entries, "logs/sequencer.log");

    assert!(!log_body.contains("ghp_very_secret_token"));
    assert!(
        log_body.contains("https://[REDACTED]@github.com/logos/repo"),
        "expected token-style userinfo redaction, got: {log_body}"
    );
}

#[test]
fn report_tail_keeps_only_last_requested_lines() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    fs::create_dir_all(temp.path().join(".scaffold/logs")).expect("create logs dir");
    fs::write(
        temp.path().join(".scaffold/logs/sequencer.log"),
        "line-1\nline-2\nline-3\nline-4\n",
    )
    .expect("write sequencer log");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .arg("--tail")
        .arg("2")
        .assert()
        .success();

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    let entries = read_report_archive_entries(&archive_path);
    let log_body = archive_entry_content(&entries, "logs/sequencer.log");

    assert!(!log_body.contains("line-1"));
    assert!(!log_body.contains("line-2"));
    assert!(log_body.contains("line-3"));
    assert!(log_body.contains("line-4"));
}

#[test]
fn report_default_archive_names_are_unique_for_fast_repeats() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();

    let archives = list_report_archives(&temp.path().join(".scaffold/reports"));
    assert_eq!(
        archives.len(),
        2,
        "expected two report archives from back-to-back runs, got: {:?}",
        archives
    );
    assert_ne!(archives[0], archives[1], "archive paths must be unique");
}

#[test]
fn report_fails_outside_project_with_project_scoped_message() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "This command must be run inside a logos-scaffold project",
        ));
}

#[test]
fn report_skips_unreadable_optional_file_and_keeps_succeeding() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    fs::create_dir(temp.path().join(".env.local")).expect("make .env.local unreadable as dir");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("report")
        .assert()
        .success();

    let archive_path = find_single_report_archive(&temp.path().join(".scaffold/reports"));
    let entries = read_report_archive_entries(&archive_path);
    let manifest = archive_entry_content(&entries, "manifest.json");
    assert!(
        manifest.contains("project/env.local"),
        "manifest should record skipped env summary, got: {manifest}"
    );
    assert!(
        manifest.contains("failed to read .env.local"),
        "manifest should include skip reason, got: {manifest}"
    );
}

#[test]
fn localnet_status_json_is_parseable() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_scaffold_toml(temp.path(), &lez_path);

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("localnet")
        .arg("status")
        .arg("--json")
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");

    assert!(value.get("tracked_pid").is_some());
    assert!(value.get("listener_present").is_some());
    assert!(value.get("ownership").is_some());
    assert!(value.get("ready").is_some());
}

#[test]
fn localnet_logs_json_tails_and_is_parseable() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_scaffold_toml(temp.path(), &lez_path);
    fs::create_dir_all(temp.path().join(".scaffold/logs")).expect("create logs dir");
    fs::write(
        temp.path().join(".scaffold/logs/sequencer.log"),
        "line one\nline two\nline three\n",
    )
    .expect("write sequencer log");

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["localnet", "logs", "--tail", "2", "--json"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");

    assert_eq!(value.get("exists").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(value.get("tail").and_then(|v| v.as_u64()), Some(2));
    let lines = value
        .get("lines")
        .and_then(|v| v.as_array())
        .expect("lines array");
    assert_eq!(
        lines.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>(),
        vec!["line two", "line three"],
        "should return the last two lines"
    );
}

#[test]
fn localnet_logs_json_reports_missing_log_without_failing() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_scaffold_toml(temp.path(), &lez_path);

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["localnet", "logs", "--json"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");

    assert_eq!(value.get("exists").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(
        value.get("lines").and_then(|v| v.as_array()).map(Vec::len),
        Some(0),
        "missing log should yield an empty lines array"
    );
}

#[test]
fn localnet_logs_json_treats_whitespace_only_log_as_empty() {
    // A log file that exists but holds only newlines/whitespace must report
    // exists=true with an empty `lines` array — `content.lines()` on "\n\n"
    // would otherwise yield ["", ""]. Mirrors the plain-text "empty" branch.
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_scaffold_toml(temp.path(), &lez_path);
    fs::create_dir_all(temp.path().join(".scaffold/logs")).expect("create logs dir");
    fs::write(temp.path().join(".scaffold/logs/sequencer.log"), "\n\n  \n")
        .expect("write whitespace-only log");

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["localnet", "logs", "--json"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");

    assert_eq!(value.get("exists").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        value.get("lines").and_then(|v| v.as_array()).map(Vec::len),
        Some(0),
        "whitespace-only log must yield an empty lines array, not [\"\"]"
    );
}

#[test]
fn doctor_json_outputs_machine_readable_report() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_scaffold_toml(temp.path(), &lez_path);

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("doctor")
        .arg("--json")
        .assert();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");

    assert!(value.get("status").is_some());
    assert!(value.get("summary").is_some());
    assert!(value.get("checks").is_some());
    assert!(value.get("next_steps").is_some());
}

#[test]
fn doctor_uses_password_env_override_for_wallet_health() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("EXPECT_PASSWORD", "override-pass")
        .env("LOGOS_SCAFFOLD_WALLET_PASSWORD", "override-pass")
        .arg("doctor")
        .arg("--json")
        .assert();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");

    let checks = value
        .get("checks")
        .and_then(serde_json::Value::as_array)
        .expect("checks array");
    let wallet_usability = checks
        .iter()
        .find(|check| {
            check.get("name").and_then(serde_json::Value::as_str) == Some("wallet usability")
        })
        .expect("wallet usability check present");

    assert_eq!(
        wallet_usability
            .get("status")
            .and_then(serde_json::Value::as_str),
        Some("pass")
    );
}

fn doctor_check<'a>(report: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    report["checks"]
        .as_array()
        .expect("checks array")
        .iter()
        .find(|c| c.get("name").and_then(serde_json::Value::as_str) == Some(name))
}

fn doctor_json(project_root: &Path) -> serde_json::Value {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(project_root)
        .arg("doctor")
        .arg("--json")
        .output()
        .expect("run doctor --json");
    serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).expect("valid doctor json")
}

#[test]
fn doctor_reports_dev_funder_in_wallet() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    // Before the wallet storage exists doctor must not create it (listing
    // accounts would initialize key material as a side effect).
    let storage = temp.path().join(".scaffold/wallet/storage.json");
    let _ = fs::remove_file(&storage);
    let report = doctor_json(temp.path());
    let funder = doctor_check(&report, "dev funder in wallet").expect("dev funder row");
    assert_eq!(funder["status"], "warn", "{funder}");
    assert!(!storage.exists(), "doctor must not initialize the wallet");

    fs::write(&storage, "{}").expect("write storage stub");
    let report = doctor_json(temp.path());
    let funder = doctor_check(&report, "dev funder in wallet").expect("dev funder row");
    assert_eq!(funder["status"], "pass", "{funder}");

    mark_dev_funder_missing(temp.path());
    let report = doctor_json(temp.path());
    let funder = doctor_check(&report, "dev funder in wallet").expect("dev funder row");
    assert_eq!(funder["status"], "warn", "{funder}");

    // spel and circuits are gone from LEZ 0.3 projects.
    let names: Vec<&str> = report["checks"]
        .as_array()
        .expect("checks array")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(
        !names
            .iter()
            .any(|n| n.contains("spel") || n.contains("circuits")),
        "no spel/circuits rows expected: {names:?}"
    );
}

#[test]
fn doctor_wallet_network_config_reads_nested_sequencer_addr() {
    // LEZ 0.3 nests the address under `sequencer`; a remote one must warn.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://10.1.2.3:3040"));

    let report = doctor_json(temp.path());
    let wallet_net = doctor_check(&report, "wallet network config").expect("wallet network row");
    assert_eq!(wallet_net["status"], "warn", "{wallet_net}");
    assert!(
        wallet_net["detail"]
            .as_str()
            .is_some_and(|d| d.contains("http://10.1.2.3:3040")),
        "{wallet_net}"
    );
}

#[test]
fn doctor_wallet_failure_hint_exports_lee_wallet_home_dir() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    // Wrong password: the stub's check-health fails.
    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("EXPECT_PASSWORD", "something-else")
        .env_remove("LOGOS_SCAFFOLD_WALLET_PASSWORD")
        .arg("doctor")
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("export LEE_WALLET_HOME_DIR=$(pwd)/.scaffold/wallet"),
        "doctor must hint the LEZ 0.3 wallet home variable; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("NSSA_WALLET_HOME_DIR"),
        "the LEZ 0.1 variable is gone; got:\n{stdout}"
    );
}

#[test]
fn doctor_sequencer_port_and_wallet_network_track_custom_localnet_port() {
    // PR #84 made deploy/wallet derive the sequencer URL from `[localnet]
    // port`. Doctor's port probe and wallet-network heuristic must track the
    // same configured port rather than a hardcoded 3040 — otherwise a project
    // on a non-default port gets a misleading "sequencer port 3040" row and a
    // false "wallet may point to non-local sequencer" warning.
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_wallet_stub(&lez_path);
    let port = unused_local_port();
    write_scaffold_toml_with_localnet(temp.path(), &lez_path, Some(port), Some(true));
    write_wallet_config(temp.path(), Some(&format!("http://127.0.0.1:{port}")));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("doctor")
        .arg("--json")
        .assert();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let checks = value
        .get("checks")
        .and_then(serde_json::Value::as_array)
        .expect("checks array");

    // The sequencer-port row must name the configured port, not 3040.
    let port_check = checks
        .iter()
        .find(|c| {
            c.get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|n| n.starts_with("sequencer port "))
        })
        .expect("sequencer port check present");
    assert_eq!(
        port_check.get("name").and_then(serde_json::Value::as_str),
        Some(format!("sequencer port {port}").as_str()),
        "port check must name the configured localnet port, got: {port_check}"
    );

    // Wallet config points at the configured port, so the network heuristic
    // must Pass rather than warn about a non-local sequencer.
    let wallet_net = checks
        .iter()
        .find(|c| {
            c.get("name").and_then(serde_json::Value::as_str) == Some("wallet network config")
        })
        .expect("wallet network config check present");
    assert_eq!(
        wallet_net.get("status").and_then(serde_json::Value::as_str),
        Some("pass"),
        "wallet config on the configured localnet port must pass, got: {wallet_net}"
    );
}

#[test]
fn localnet_start_fails_when_process_exits_before_ready() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    let sequencer_bin = lez_path.join("target/release/sequencer_service");
    let config_path = lez_path.join("lez/sequencer/service/configs/debug/sequencer_config.json");
    let localnet_port = unused_local_port();
    fs::create_dir_all(sequencer_bin.parent().expect("parent")).expect("create dirs");
    fs::create_dir_all(config_path.parent().expect("parent")).expect("create config dir");
    fs::write(&config_path, r#"{"home": "."}"#).expect("write sequencer config");
    fs::write(&sequencer_bin, "#!/bin/sh\nexit 1\n").expect("write fake sequencer");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&sequencer_bin)
            .expect("metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&sequencer_bin, perms).expect("chmod");
    }

    write_scaffold_toml_with_localnet(temp.path(), &lez_path, Some(localnet_port), Some(true));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("localnet")
        .arg("start")
        .arg("--timeout-sec")
        .arg("1")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("sequencer process exited before becoming ready")
                .or(predicate::str::contains("localnet start timed out after")),
        )
        // The fake sequencer above exits before writing a line, so the printed
        // tail is `<no log output yet>` — the shape this failure takes in the
        // wild when the spawned sequencer cannot bind the port. Without a next
        // step the message is a dead end: every other `LocalnetError` carries
        // one, and `localnet status` is what names the actual conflict.
        .stderr(predicate::str::contains("<no log output yet>"))
        .stderr(predicate::str::contains("logos-scaffold localnet status"));

    assert!(
        !temp.path().join(".scaffold/state/localnet.state").exists(),
        "state file should be cleaned after failed startup"
    );
}

#[test]
fn localnet_start_patches_config_and_uses_configured_port() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    let sequencer_bin = lez_path.join("target/release/sequencer_service");
    let config_path = lez_path.join("lez/sequencer/service/configs/debug/sequencer_config.json");
    let args_log = temp.path().join("sequencer-args.log");
    let env_log = temp.path().join("sequencer-env.log");
    let cwd_log = temp.path().join("sequencer-cwd.log");
    let localnet_port = unused_local_port();

    fs::create_dir_all(sequencer_bin.parent().expect("parent")).expect("create dirs");
    fs::create_dir_all(config_path.parent().expect("parent")).expect("create config dir");
    let vendored = r#"{"home": ".", "block_create_timeout": "10s", "genesis": [{"supply_account": {"account_id": "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV", "balance": 5}}]}"#;
    fs::write(&config_path, vendored).expect("write sequencer config");

    // Fake sequencer: takes the port from `--port` (like LEZ 0.3's
    // sequencer_service), logs args, env and cwd for assertions.
    fs::write(
        &sequencer_bin,
        format!(
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s' \"${{RISC0_DEV_MODE:-}}\" > '{}'\npwd > '{}'\nport=''\nprev=''\nfor a in \"$@\"; do if [ \"$prev\" = '--port' ]; then port=\"$a\"; fi; prev=\"$a\"; done\nexec python3 -m http.server \"$port\" --bind 127.0.0.1\n",
            args_log.display(),
            env_log.display(),
            cwd_log.display(),
        ),
    )
    .expect("write fake sequencer");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&sequencer_bin)
            .expect("metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&sequencer_bin, perms).expect("chmod");
    }

    write_scaffold_toml_with_localnet(temp.path(), &lez_path, Some(localnet_port), Some(false));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("localnet")
        .arg("start")
        .arg("--timeout-sec")
        .arg("5")
        .assert()
        .success()
        .stdout(predicate::str::contains("localnet ready"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("localnet")
        .arg("stop")
        .assert()
        .success();

    // The vendored config in the lez checkout must be left untouched.
    // Regression for #114: in-place mutation broke `git_clean(lez)` and
    // silently disabled the cache-repo auto-reclone safety net.
    let vendored_after = fs::read_to_string(&config_path).expect("read vendored config");
    assert_eq!(
        vendored_after, vendored,
        "vendored sequencer_config.json must not be mutated"
    );

    // The patched copy lives under the project's `.scaffold/state/`. Resolve
    // symlinks so the comparison against the sequencer-recorded path matches
    // on macOS, where `/var` is a symlink to `/private/var` and `current_dir`
    // inside the CLI returns the canonical form.
    let patched_path = temp
        .path()
        .join(".scaffold/state/sequencer_config.json")
        .canonicalize()
        .expect("canonicalize patched config path");
    let patched_config = fs::read_to_string(&patched_path).expect("read patched config");
    let config_json: serde_json::Value =
        serde_json::from_str(&patched_config).expect("parse patched config");
    // LEZ 0.3 takes the port on the command line; the config carries no
    // port and keeps the upstream max_block_size.
    assert!(
        config_json.get("port").is_none(),
        "patched config must not carry a port, got: {patched_config}"
    );
    assert!(
        config_json.get("max_block_size").is_none(),
        "patched config must not widen max_block_size, got: {patched_config}"
    );
    assert_eq!(
        config_json["block_create_timeout"],
        serde_json::json!("2s"),
        "localnet shortens the block interval, got: {patched_config}"
    );
    // Existing genesis entries are kept and the dev funder is added.
    assert_eq!(
        config_json["genesis"],
        serde_json::json!([
            {"supply_account": {"account_id": "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV", "balance": 5}},
            {"supply_account": {"account_id": DEV_FUNDER_ACCOUNT_ID, "balance": 10_000_000_000_000_u64}}
        ]),
        "dev funder genesis entry expected, got: {patched_config}"
    );

    // The sequencer must have been invoked with the absolute path to the
    // patched copy, not the in-repo relative path.
    let args = fs::read_to_string(&args_log).expect("read args log");
    assert!(
        args.lines()
            .any(|line| line == patched_path.to_string_lossy()),
        "expected sequencer to receive patched config path {}, got args: {args}",
        patched_path.display()
    );

    let chain_home = temp
        .path()
        .canonicalize()
        .expect("canonical temp")
        .join(".scaffold/localnet");
    let expected_args = vec![
        patched_path.display().to_string(),
        "--port".to_string(),
        localnet_port.to_string(),
        "--listen-address".to_string(),
        "127.0.0.1".to_string(),
        "--home".to_string(),
        chain_home.display().to_string(),
        "--metrics-address".to_string(),
        "127.0.0.1:0".to_string(),
    ];
    assert_eq!(
        args.lines().map(ToString::to_string).collect::<Vec<_>>(),
        expected_args,
        "unexpected sequencer argv"
    );
    let cwd = fs::read_to_string(&cwd_log).expect("read cwd log");
    assert_eq!(
        Path::new(cwd.trim()).canonicalize().expect("canonical cwd"),
        lez_path.canonicalize().expect("canonical lez"),
        "sequencer must run from the lez checkout"
    );

    let env = fs::read_to_string(&env_log).expect("read env log");
    assert_eq!(env, "0", "expected risc0 dev mode override to be passed");
}

#[cfg(unix)]
#[test]
fn test_node_start_patches_block_timing_flags() {
    let temp = tempdir().expect("tempdir");
    let fixtures = setup_test_node_project(temp.path());
    let work_dir = temp.path().join("node-work");

    let runtime_json = start_test_node_and_read_runtime_config(
        temp.path(),
        &fixtures,
        &work_dir,
        &[
            "--block-create-timeout-ms",
            "100",
            "--retry-pending-blocks-timeout-ms",
            "250",
        ],
    );
    assert_eq!(
        runtime_json["block_create_timeout"],
        serde_json::json!("100ms")
    );
    assert_eq!(
        runtime_json["retry_pending_blocks_timeout"],
        serde_json::json!("250ms")
    );

    assert_vendored_test_node_timing_preserved(&fixtures);
}

#[cfg(unix)]
#[test]
fn test_node_start_uses_lez_0_3_argv_and_funds_dev_funder_at_genesis() {
    let temp = tempdir().expect("tempdir");
    let fixtures = setup_test_node_project(temp.path());
    let work_dir = temp.path().join("node-work");

    let runtime_json =
        start_test_node_and_read_runtime_config(temp.path(), &fixtures, &work_dir, &[]);
    assert_eq!(
        runtime_json["home"],
        serde_json::json!(work_dir.display().to_string()),
        "node home must be its work dir"
    );
    assert!(
        runtime_json.get("port").is_none() && runtime_json.get("genesis_id").is_none(),
        "LEZ 0.3 config carries neither port nor genesis_id: {runtime_json}"
    );
    assert_eq!(
        runtime_json["genesis"],
        serde_json::json!([
            {"supply_account": {"account_id": DEV_FUNDER_ACCOUNT_ID, "balance": 10_000_000_000_000_u64}}
        ])
    );

    let observation: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&fixtures.sequencer_observation_path).expect("read observation"),
    )
    .expect("parse observation");
    let args: Vec<String> = observation["args"]
        .as_array()
        .expect("args array")
        .iter()
        .map(|a| a.as_str().expect("string arg").to_string())
        .collect();
    let port = observation["port"].as_u64().expect("port").to_string();
    assert_eq!(
        args[1..],
        [
            "--port".to_string(),
            port,
            "--listen-address".to_string(),
            "127.0.0.1".to_string(),
            "--home".to_string(),
            work_dir.display().to_string(),
            "--metrics-address".to_string(),
            "127.0.0.1:0".to_string(),
        ],
        "unexpected sequencer argv: {args:?}"
    );
}

#[cfg(unix)]
#[test]
fn test_node_start_preserves_block_timing_without_flags() {
    let temp = tempdir().expect("tempdir");
    let fixtures = setup_test_node_project(temp.path());
    let work_dir = temp.path().join("node-work");

    let runtime_json =
        start_test_node_and_read_runtime_config(temp.path(), &fixtures, &work_dir, &[]);
    assert_eq!(
        runtime_json["block_create_timeout"],
        serde_json::json!("2s")
    );
    assert_eq!(
        runtime_json["retry_pending_blocks_timeout"],
        serde_json::json!("3s")
    );
}

#[cfg(unix)]
#[test]
fn test_node_start_preserves_unset_block_timing_field() {
    let temp = tempdir().expect("tempdir");
    let fixtures = setup_test_node_project(temp.path());
    let work_dir = temp.path().join("node-work");

    let runtime_json = start_test_node_and_read_runtime_config(
        temp.path(),
        &fixtures,
        &work_dir,
        &["--block-create-timeout-ms", "100"],
    );
    assert_eq!(
        runtime_json["block_create_timeout"],
        serde_json::json!("100ms")
    );
    assert_eq!(
        runtime_json["retry_pending_blocks_timeout"],
        serde_json::json!("3s")
    );
}

#[cfg(unix)]
#[test]
fn test_node_run_patches_block_timing_flags_for_child_command() {
    let temp = tempdir().expect("tempdir");
    let fixtures = setup_test_node_project(temp.path());

    let observed_json = run_test_node_and_observe_config(
        temp.path(),
        &fixtures,
        &[
            "--block-create-timeout-ms",
            "101",
            "--retry-pending-blocks-timeout-ms",
            "251",
        ],
    );
    assert_eq!(
        observed_json["block_create_timeout"],
        serde_json::json!("101ms")
    );
    assert_eq!(
        observed_json["retry_pending_blocks_timeout"],
        serde_json::json!("251ms")
    );
}

#[cfg(unix)]
#[test]
fn test_node_run_preserves_block_timing_without_flags_for_child_command() {
    let temp = tempdir().expect("tempdir");
    let fixtures = setup_test_node_project(temp.path());

    let observed_json = run_test_node_and_observe_config(temp.path(), &fixtures, &[]);
    assert_eq!(
        observed_json["block_create_timeout"],
        serde_json::json!("2s")
    );
    assert_eq!(
        observed_json["retry_pending_blocks_timeout"],
        serde_json::json!("3s")
    );
}

#[cfg(unix)]
#[test]
fn test_node_run_preserves_unset_block_timing_field_for_child_command() {
    let temp = tempdir().expect("tempdir");
    let fixtures = setup_test_node_project(temp.path());

    let observed_json = run_test_node_and_observe_config(
        temp.path(),
        &fixtures,
        &["--retry-pending-blocks-timeout-ms", "251"],
    );
    assert_eq!(
        observed_json["block_create_timeout"],
        serde_json::json!("2s")
    );
    assert_eq!(
        observed_json["retry_pending_blocks_timeout"],
        serde_json::json!("251ms")
    );
}

#[cfg(unix)]
#[test]
fn test_node_run_forwards_child_exit_status_with_block_timing_flags() {
    let temp = tempdir().expect("tempdir");
    let fixtures = setup_test_node_project(temp.path());

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("test-node")
        .arg("run")
        .arg("--block-create-timeout-ms")
        .arg("100")
        .arg("--retry-pending-blocks-timeout-ms")
        .arg("100")
        .arg("--timeout-sec")
        .arg("5")
        .arg("--")
        .arg(&fixtures.python_path)
        .arg("-c")
        .arg("import sys; sys.exit(7)")
        .assert()
        .code(7);
}

#[test]
fn test_node_timing_flags_reject_duration_suffixes() {
    for subcommand in ["start", "run"] {
        for flag in [
            "--block-create-timeout-ms",
            "--retry-pending-blocks-timeout-ms",
        ] {
            for value in ["100ms", "1s", "1m", "1h"] {
                let mut command = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"));
                command
                    .arg("test-node")
                    .arg(subcommand)
                    .arg(flag)
                    .arg(value);
                if subcommand == "run" {
                    command.arg("--").arg("not-executed");
                }
                command
                    .assert()
                    .failure()
                    .stderr(predicate::str::contains(flag));
            }
        }
    }
}

#[test]
fn test_node_timing_flags_reject_out_of_range_milliseconds() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("test-node")
        .arg("start")
        .arg("--block-create-timeout-ms")
        .arg("0")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--block-create-timeout-ms"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("test-node")
        .arg("run")
        .arg("--retry-pending-blocks-timeout-ms")
        .arg("3600001")
        .arg("--")
        .arg("not-executed")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--retry-pending-blocks-timeout-ms",
        ));
}

#[test]
fn localnet_stop_outside_project_succeeds() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("localnet")
        .arg("stop")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("localnet not running").or(predicate::str::contains(
                "listener detected on 127.0.0.1:3040",
            )),
        );
}

#[test]
fn localnet_stop_outside_project_with_listener_prints_hint() {
    let temp = tempdir().expect("tempdir");

    match TcpListener::bind("127.0.0.1:3040") {
        Ok(listener) => {
            listener
                .set_nonblocking(true)
                .expect("set nonblocking listener");

            Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
                .current_dir(temp.path())
                .arg("localnet")
                .arg("stop")
                .assert()
                .success()
                .stdout(
                    predicate::str::contains("127.0.0.1:3040").and(
                        predicate::str::contains("Try: kill")
                            .or(predicate::str::contains("Try: lsof -nP -iTCP:3040")),
                    ),
                );
        }
        Err(_) => {
            Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
                .current_dir(temp.path())
                .arg("localnet")
                .arg("stop")
                .assert()
                .success()
                .stdout(predicate::str::contains("localnet not running").or(
                    predicate::str::contains("listener detected on 127.0.0.1:3040"),
                ));
        }
    }
}

#[test]
fn wallet_list_proxies_account_list() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("list")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("account list")
                .and(predicate::str::contains(VALID_PUBLIC_ADDRESS)),
        );
}

#[test]
fn wallet_list_json_emits_structured_envelope() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["wallet", "list", "--json"])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");

    assert_eq!(value.get("exit_code").and_then(|v| v.as_i64()), Some(0));
    let accounts = value
        .get("accounts")
        .and_then(|v| v.as_array())
        .expect("accounts array");
    assert_eq!(
        accounts.len(),
        3,
        "stub lists two derived accounts plus the dev funder: {stdout}"
    );
    assert!(accounts
        .iter()
        .any(|a| a.as_str() == Some(&format!("/ {VALID_PUBLIC_ADDRESS}"))));
}

#[test]
fn wallet_topup_amount_flag_sets_the_transfer_amount() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args([
            "wallet",
            "topup",
            "--address",
            VALID_PUBLIC_ADDRESS,
            "--amount",
            "7",
            "--dry-run",
            "--json",
        ])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid json");
    assert_eq!(value["amount"].as_u64(), Some(7), "{value}");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["wallet", "topup", "--amount", "0", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("greater than zero"));
}

#[test]
fn wallet_topup_json_dry_run_is_structured() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args([
            "wallet",
            "topup",
            "--address",
            VALID_PUBLIC_ADDRESS,
            "--dry-run",
            "--json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");

    assert_eq!(
        value.get("status").and_then(|v| v.as_str()),
        Some("dry_run")
    );
    assert!(value
        .get("address")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.contains("Public/")));
    assert_eq!(
        value.get("method").and_then(|v| v.as_str()),
        Some("dev funder transfer")
    );
    assert_eq!(
        value.get("amount").and_then(|v| v.as_u64()),
        Some(1_000_000_000)
    );
    // Stable schema: `tx` key is always present (null for a dry run).
    assert!(value.get("tx").is_some_and(serde_json::Value::is_null));
}

#[test]
fn wallet_topup_json_success_reports_tx() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args([
            "wallet",
            "topup",
            "--address",
            VALID_PUBLIC_ADDRESS,
            "--json",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");

    assert_eq!(
        value.get("status").and_then(|v| v.as_str()),
        Some("success")
    );
    assert_eq!(
        value.get("tx").and_then(|v| v.as_str()),
        Some("a0a1a2a3"),
        "stub emits `Transaction hash is a0a1a2a3`"
    );
    assert_eq!(
        value.get("method").and_then(|v| v.as_str()),
        Some("dev funder transfer")
    );
    assert_eq!(
        value.get("amount").and_then(|v| v.as_u64()),
        Some(1_000_000_000)
    );
    // stdout must be a single clean JSON object — no `$ <cmd>` echo lines.
    assert!(
        stdout.trim_start().starts_with('{'),
        "json mode must not echo commands before the object: {stdout}"
    );
}

#[test]
fn wallet_topup_json_connectivity_failure_categorizes_reason() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("TOPUP_FAIL_CONNECT", "1")
        .args([
            "wallet",
            "topup",
            "--address",
            VALID_PUBLIC_ADDRESS,
            "--json",
        ])
        .assert()
        .failure();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");

    assert_eq!(value.get("status").and_then(|v| v.as_str()), Some("error"));
    assert_eq!(
        value.get("reason").and_then(|v| v.as_str()),
        Some("connectivity")
    );
    // Error objects carry the same attempt context as success/pending, so
    // consumers don't lose what was attempted on failure.
    assert!(value
        .get("address")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.contains("Public/")));
    assert_eq!(
        value.get("method").and_then(|v| v.as_str()),
        Some("dev funder transfer")
    );
    assert_eq!(
        value.get("network").and_then(|v| v.as_str()),
        Some("http://127.0.0.1:3040")
    );
    // Stable schema: `tx` is always present (null in the error case).
    assert!(value.get("tx").is_some_and(serde_json::Value::is_null));
}

#[test]
fn wallet_passthrough_account_list_works() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("--")
        .arg("account")
        .arg("list")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("account list")
                .and(predicate::str::contains(VALID_PUBLIC_ADDRESS)),
        );
}

#[test]
fn wallet_passthrough_requires_args_after_double_dash() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("--")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "wallet passthrough requires at least one argument after `--`",
        ));
}

#[test]
fn wallet_topup_dry_run_planned_network_uses_localnet_port_without_wallet_sequencer_addr() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_wallet_stub(&lez_path);
    let port = unused_local_port();
    write_scaffold_toml_with_localnet(temp.path(), &lez_path, Some(port), Some(true));
    write_wallet_config(temp.path(), None);

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "planned network: local sequencer (http://127.0.0.1:{port})"
        )));
}

#[test]
fn wallet_topup_dry_run_renders_dev_funder_transfer_command() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("dry-run: wallet topup command will not be executed")
                .and(predicate::str::contains(format!(
                    "LEE_WALLET_HOME_DIR={}",
                    temp.path().join(".scaffold/wallet").display()
                )))
                .and(predicate::str::contains(format!(
                    "auth-transfer send --from {DEV_FUNDER_ADDRESS} --to {VALID_PUBLIC_ADDRESS} --amount 1000000000"
                )))
                .and(predicate::str::contains(
                    "planned method: dev funder transfer",
                ))
                .and(predicate::str::contains("pinata").not())
                .and(predicate::str::contains("NSSA_WALLET_HOME_DIR").not()),
        );
    assert!(
        wallet_stub_calls(temp.path()).is_empty(),
        "dry-run must not invoke the wallet: {:?}",
        wallet_stub_calls(temp.path())
    );
}

#[test]
fn wallet_topup_sends_from_dev_funder_with_explicit_address() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .success()
        .stdout(
            predicate::str::contains(format!(
                "auth-transfer send --from {DEV_FUNDER_ADDRESS} --to {VALID_PUBLIC_ADDRESS} --amount 1000000000"
            ))
            .and(predicate::str::contains("wallet topup complete"))
            .and(predicate::str::contains("Method: dev funder transfer"))
            .and(predicate::str::contains("Tx: a0a1a2a3")),
        );

    let calls = wallet_stub_calls(temp.path());
    assert!(
        !calls.iter().any(|c| c.starts_with("account import")),
        "funder already in the wallet: no import expected, got {calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("pinata") || c.starts_with("auth-transfer init")),
        "LEZ 0.3 topup has no pinata/init steps, got {calls:?}"
    );
}

#[test]
fn wallet_topup_imports_dev_funder_when_missing() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    mark_dev_funder_missing(temp.path());

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("EXPECT_PASSWORD", DEFAULT_WALLET_PASSWORD)
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .success()
        .stdout(predicate::str::contains("wallet topup complete"));

    let calls = wallet_stub_calls(temp.path());
    let import_pos = calls
        .iter()
        .position(|c| {
            *c == format!("account import public --private-key {DEV_FUNDER_PRIVATE_KEY_HEX}")
        })
        .unwrap_or_else(|| panic!("funder import expected, got {calls:?}"));
    let send_pos = calls
        .iter()
        .position(|c| c.starts_with("auth-transfer send"))
        .unwrap_or_else(|| panic!("transfer expected, got {calls:?}"));
    assert!(import_pos < send_pos, "import must precede send: {calls:?}");
}

#[test]
fn wallet_topup_rejects_dev_funder_as_destination() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(DEV_FUNDER_ACCOUNT_ID)
        .assert()
        .failure()
        .stderr(predicate::str::contains("is the dev funder itself"));

    assert!(
        !wallet_stub_calls(temp.path())
            .iter()
            .any(|c| c.starts_with("auth-transfer send")),
        "no transfer may be attempted to the funder itself"
    );
}

#[test]
fn wallet_topup_funder_listing_connectivity_failure_shows_sequencer_hint_in_json() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("ACCOUNT_LIST_FAIL_CONNECT", "1")
        .args([
            "wallet",
            "topup",
            "--address",
            VALID_PUBLIC_ADDRESS,
            "--json",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "wallet topup could not prepare the dev funder",
        ));

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value.get("status").and_then(|v| v.as_str()), Some("error"));
    assert_eq!(
        value.get("reason").and_then(|v| v.as_str()),
        Some("connectivity")
    );
    assert!(
        !wallet_stub_calls(temp.path())
            .iter()
            .any(|c| c.starts_with("auth-transfer send")),
        "transfer must not run when the funder preflight fails"
    );
}

#[test]
fn wallet_topup_uses_password_env_override() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("EXPECT_PASSWORD", "override-pass")
        .env("LOGOS_SCAFFOLD_WALLET_PASSWORD", "override-pass")
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .success()
        .stdout(predicate::str::contains("wallet topup complete"));
}

#[test]
fn wallet_topup_falls_back_to_default_password_when_env_missing() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("EXPECT_PASSWORD", DEFAULT_WALLET_PASSWORD)
        .env_remove("LOGOS_SCAFFOLD_WALLET_PASSWORD")
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .success()
        .stdout(predicate::str::contains("wallet topup complete"));
}

#[test]
fn wallet_topup_uses_default_wallet_when_address_is_omitted() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("default")
        .arg("set")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .success();

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("topup")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "--to {VALID_PUBLIC_ADDRESS} --amount 1000000000"
        )));
}

#[test]
fn wallet_topup_errors_when_address_and_default_are_missing() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("topup")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("wallet topup requires a destination address")
                .and(predicate::str::contains("logos-scaffold wallet list")),
        );
}

#[test]
fn wallet_topup_rejects_invalid_address() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg("abc")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("invalid address format `abc`")
                .and(predicate::str::contains("Accepted formats")),
        );
}

#[test]
fn wallet_topup_shows_sequencer_hint_on_connectivity_failure() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("TOPUP_FAIL_CONNECT", "1")
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("sequencer appears unavailable")
                .and(predicate::str::contains("logos-scaffold localnet start"))
                .and(predicate::str::contains("Another project's sequencer")),
        );
}

#[test]
fn wallet_topup_timeout_exits_non_zero_with_pending_status() {
    // C3: confirmation timeout means we don't actually know whether the
    // submission landed. Surface that to the agent via a non-zero exit and
    // an explicit `status: pending` in the message, instead of the previous
    // soft-success that tricked retry loops into thinking the topup was done.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("TOPUP_FAIL_TIMEOUT", "1")
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("wallet topup submitted, but confirmation timed out")
                .and(predicate::str::contains("status: pending")),
        );
}

#[test]
fn wallet_topup_timeout_with_transport_token_reports_pending_not_connectivity() {
    // Once the classifier sees stdout+stderr combined, a sequencer dying
    // mid-transfer can carry BOTH the confirmation-timeout line AND a
    // transport token ("connection refused"). The tx reached the sequencer
    // but did not settle, so the confirmation-timeout branch must win
    // (status: pending, with the tx id) rather than the connectivity hint.
    // Reverting the check ordering at the transfer site makes this fail.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("TOPUP_FAIL_TIMEOUT_TRANSPORT", "1")
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("wallet topup submitted, but confirmation timed out")
                .and(predicate::str::contains("status: pending"))
                .and(predicate::str::contains("Tx: a0a1a2a3"))
                .and(predicate::str::contains("sequencer appears unavailable").not()),
        );
}

#[test]
fn wallet_topup_fails_outside_project_with_project_scoped_message() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "This command must be run inside a logos-scaffold project",
        ));
}

#[test]
fn wallet_default_set_persists_normalized_address_positional() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("default")
        .arg("set")
        .arg(VALID_ACCOUNT_ID)
        .assert()
        .success()
        .stdout(predicate::str::contains("default wallet updated"));

    let state_path = temp.path().join(".scaffold/state/wallet.state");
    let state = fs::read_to_string(state_path).expect("read wallet.state");
    assert_eq!(state, format!("default_address={VALID_PUBLIC_ADDRESS}\n"));
}

#[test]
fn wallet_default_set_accepts_flag_form() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("wallet")
        .arg("default")
        .arg("set")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .success()
        .stdout(predicate::str::contains("default wallet updated"));
}

#[test]
fn deploy_unknown_program_lists_available_programs() {
    let temp = tempdir().expect("tempdir");
    setup_deploy_project(temp.path(), None);
    write_guest_program(temp.path(), "alpha");
    write_guest_program(temp.path(), "beta");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("missing")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("unknown program `missing`")
                .and(predicate::str::contains("alpha"))
                .and(predicate::str::contains("beta")),
        );
}

#[test]
fn deploy_single_program_deploys_successfully() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "hello");
    write_guest_binary(temp.path(), "hello");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("hello")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("OK  hello deployed")
                .and(predicate::str::contains(
                    "deployed through program_loader (1 segment)",
                ))
                .and(predicate::str::contains("Deployed: 1"))
                .and(predicate::str::contains("Failed: 0"))
                .and(predicate::str::contains("reachability probe failed").not()),
        );
}

#[test]
fn deploy_uses_password_env_override() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "hello");
    write_guest_binary(temp.path(), "hello");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("EXPECT_PASSWORD", "override-pass")
        .env("LOGOS_SCAFFOLD_WALLET_PASSWORD", "override-pass")
        .arg("deploy")
        .arg("hello")
        .assert()
        .success()
        .stdout(predicate::str::contains("OK  hello deployed"));
}

#[test]
fn deploy_missing_binary_shows_build_hint() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "hello");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("hello")
        .assert()
        .failure()
        .stdout(
            predicate::str::contains("missing binary")
                .and(predicate::str::contains("logos-scaffold build")),
        );
}

#[test]
fn deploy_continues_and_summarizes_mixed_results() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "alpha");
    write_guest_program(temp.path(), "beta");
    write_guest_binary(temp.path(), "alpha");
    write_guest_binary(temp.path(), "beta");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("FAIL_PROGRAM", "beta.bin")
        .arg("deploy")
        .assert()
        .failure()
        .stdout(
            predicate::str::contains("OK  alpha deployed")
                .and(predicate::str::contains("FAIL beta deployment failed"))
                .and(predicate::str::contains("Deployed: 1"))
                .and(predicate::str::contains("Failed: 1")),
        );
}

#[test]
fn deploy_preflight_targets_localnet_port_when_wallet_omits_sequencer_addr() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_wallet_stub(&lez_path);
    let port = unused_local_port();
    write_scaffold_toml_with_localnet(temp.path(), &lez_path, Some(port), Some(true));
    write_wallet_config(temp.path(), None);
    write_default_wallet_state(temp.path());
    write_guest_program(temp.path(), "hello");
    write_guest_binary(temp.path(), "hello");

    let url = format!("http://127.0.0.1:{port}");
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("hello")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("cannot deploy programs")
                .and(predicate::str::contains(&url))
                .and(predicate::str::contains("sequencer appears unavailable")),
        );
}

#[test]
fn deploy_shows_hint_when_sequencer_is_unreachable_with_configured_addr() {
    let temp = tempdir().expect("tempdir");
    setup_deploy_project(temp.path(), Some("http://127.0.0.1:65535"));
    write_guest_program(temp.path(), "hello");
    write_guest_binary(temp.path(), "hello");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("hello")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("sequencer appears unavailable")
                .and(predicate::str::contains("logos-scaffold localnet start"))
                .and(predicate::str::contains("Another project's sequencer")),
        );
}

#[test]
fn deploy_shows_hint_when_sequencer_is_unreachable_with_fallback_addr() {
    // Wallet omits `sequencer_addr`; deploy falls back to `http://127.0.0.1:<localnet.port>`
    // (default 3040 when `[localnet]` is omitted). Skip if something is already listening.
    if TcpStream::connect("127.0.0.1:3040").is_ok() {
        return;
    }

    let temp = tempdir().expect("tempdir");
    setup_deploy_project(temp.path(), None);
    write_guest_program(temp.path(), "hello");
    write_guest_binary(temp.path(), "hello");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("hello")
        .assert()
        .failure()
        .stderr(predicate::str::contains("sequencer appears unavailable"));
}

#[test]
fn deploy_classifies_mid_deploy_both_tokens_failure_as_connectivity() {
    // Restores master behaviour at deploy's per-program failure arm
    // (deploy.rs:243). The sequencer is reachable at preflight (RpcStub
    // answers), then dies mid-deploy: the wallet subprocess fails with BOTH the
    // confirmation-timeout line AND a transport token ("connection refused") in
    // its combined output. deploy has no confirmation-timeout branch of its
    // own, so it must classify this as connectivity and print the
    // sequencer-unavailable hint — not "inspect sequencer logs and retry".
    //
    // This is the case the shipped branch (6f2ed30) dropped: the shared
    // helper's `is_confirmation_timeout_failure` early-return made the combined
    // message read as non-connectivity here. Removing that early-return re-arms
    // this path — re-adding it makes this test fail (hint reverts to the
    // generic retry line).
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "hello");
    write_guest_binary(temp.path(), "hello");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("DEPLOY_FAIL_TIMEOUT_TRANSPORT", "hello.bin")
        .arg("deploy")
        .arg("hello")
        .output()
        .expect("run deploy against a sequencer that dies mid-deploy");

    assert!(
        !output.status.success(),
        "a failed deploy must exit non-zero"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sequencer appears unavailable"),
        "a mid-deploy both-tokens failure must classify as connectivity and show \
         the sequencer-unavailable hint; stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("inspect sequencer logs and retry"),
        "connectivity classification must replace the generic retry hint; stdout:\n{stdout}"
    );
}

#[test]
fn deploy_prints_program_id_from_program_loader_header() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "hello");
    write_guest_binary(temp.path(), "hello");
    let expected = STUB_NEW_ACCOUNT_IDS[0];

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("hello")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("OK  hello deployed").and(predicate::str::contains(format!(
                "  program_id: {expected}"
            ))),
        );
}

#[test]
fn deploy_plain_output_has_single_program_id_line_per_program() {
    // The OK block carries program_id per program; the Summary block no
    // longer repeats it. With N programs deployed, exactly N lines should
    // match `^  program_id: …` so a single grep/awk pattern works whether
    // the user deploys one program or many.
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "alpha");
    write_guest_program(temp.path(), "beta");
    write_guest_binary(temp.path(), "alpha");
    write_guest_binary(temp.path(), "beta");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .output()
        .expect("run deploy");
    assert!(output.status.success(), "deploy must succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let count = stdout
        .lines()
        .filter(|line| line.starts_with("  program_id: "))
        .count();
    assert_eq!(
        count, 2,
        "expected exactly one `  program_id:` line per deployed program (2 total); got {count} in:\n{stdout}"
    );
}

#[test]
fn deploy_program_path_json_includes_program_id() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    let custom = temp.path().join("custom.bin");
    fs::write(&custom, minimal_r0bf_binary()).expect("write custom bin");
    let expected = STUB_NEW_ACCOUNT_IDS[0];

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("--program-path")
        .arg(&custom)
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "\"program_id\":\"{expected}\""
        )));
}

#[test]
fn deploy_json_output_is_pure_json_no_command_echo() {
    // --json must produce a single JSON line on stdout; the wallet
    // subprocess command-echo (`$ .../wallet program-loader deploy …`) belongs
    // off-channel so consumers can pipe directly to `jq`.
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    let custom = temp.path().join("custom.bin");
    fs::write(&custom, minimal_r0bf_binary()).expect("write custom bin");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("--program-path")
        .arg(&custom)
        .arg("--json")
        .output()
        .expect("run deploy --json");
    assert!(output.status.success(), "deploy --json must succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    assert!(
        trimmed.starts_with('{') && trimmed.ends_with('}'),
        "stdout must be pure JSON; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("$ "),
        "stdout must not carry the `$ <cmd>` command echo; got:\n{stdout}"
    );
    assert_eq!(
        stdout.lines().filter(|l| !l.is_empty()).count(),
        1,
        "stdout must be a single non-empty JSON line; got:\n{stdout}"
    );
}

#[test]
fn deploy_json_stdout_stays_pure_json_when_preflight_probe_returns_other() {
    // A foreign HTTP responder (404) squatting on the sequencer port maps to
    // RpcReachabilityError::Other, so `preflight_sequencer_reachability` warns
    // and continues instead of bailing. That warning runs BEFORE the `--json`
    // echo guard is installed, so it must be written to stderr: a `println!`
    // there interleaves non-JSON HTML into the `--json` stdout stream and
    // breaks `jq`. Pins the `eprintln!` fix — reverting it to `println!` makes
    // the JSON-parse assertion below fail.
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start_http_error();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    let custom = temp.path().join("custom.bin");
    fs::write(&custom, minimal_r0bf_binary()).expect("write custom bin");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("--program-path")
        .arg(&custom)
        .arg("--json")
        .output()
        .expect("run deploy --json");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // stdout must be a single, valid JSON object — this is what a `println!`
    // warning breaks (the HTML body lands ahead of the JSON on stdout).
    let trimmed = stdout.trim();
    serde_json::from_str::<serde_json::Value>(trimmed).unwrap_or_else(|err| {
        panic!("deploy --json stdout must be valid JSON; parse error: {err}\nstdout:\n{stdout}")
    });

    // And the Other arm actually fired (otherwise this test proves nothing):
    // its warning landed on stderr, not stdout.
    assert!(
        stderr.contains("sequencer reachability probe failed"),
        "the preflight Other-arm warning must be emitted on stderr; stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("sequencer reachability probe failed"),
        "the warning must NOT appear on stdout; stdout:\n{stdout}"
    );
}

#[test]
fn deploy_multi_program_json_emits_deploys_array() {
    // Auto-discovery deploy with --json must produce one JSON line of the
    // shape `{"deploys":[{...},{...}]}` — pure JSON, no command echoes,
    // no human-readable text. One entry per program with program_id
    // populated when extraction succeeds.
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "alpha");
    write_guest_program(temp.path(), "beta");
    write_guest_binary(temp.path(), "alpha");
    write_guest_binary(temp.path(), "beta");
    // Each program takes a header + one segment account, in order.
    let alpha_id = STUB_NEW_ACCOUNT_IDS[0];
    let beta_id = STUB_NEW_ACCOUNT_IDS[2];

    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("--json")
        .output()
        .expect("run deploy --json");
    assert!(
        output.status.success(),
        "multi-program deploy --json must succeed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    assert!(
        trimmed.starts_with("{\"deploys\":[") && trimmed.ends_with("]}"),
        "stdout must be a {{\"deploys\":[…]}} JSON line; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("$ "),
        "stdout must not carry the `$ <cmd>` command echo; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("OK  ") && !stdout.contains("Note:") && !stdout.contains("Summary:"),
        "stdout must not carry human-readable text; got:\n{stdout}"
    );
    assert_eq!(
        stdout.lines().filter(|l| !l.is_empty()).count(),
        1,
        "stdout must be a single JSON line; got:\n{stdout}"
    );
    assert!(
        trimmed.contains(&format!("\"program_id\":\"{alpha_id}\"")),
        "missing alpha program_id in:\n{trimmed}"
    );
    assert!(
        trimmed.contains(&format!("\"program_id\":\"{beta_id}\"")),
        "missing beta program_id in:\n{trimmed}"
    );
    assert_eq!(
        trimmed.matches("\"status\":\"deployed\"").count(),
        2,
        "expected exactly two submitted entries in:\n{trimmed}"
    );
}

#[test]
fn deploy_json_output_parses_as_valid_json() {
    // Pin the structural contract: --json stdout must round-trip through
    // serde_json::from_str. Cleanliness is already covered by
    // `deploy_json_output_is_pure_json_no_command_echo`; this is the
    // structural counterpart.
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    let custom = temp.path().join("custom.bin");
    fs::write(&custom, minimal_r0bf_binary()).expect("write custom bin");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("--program-path")
        .arg(&custom)
        .arg("--json")
        .output()
        .expect("run deploy --json");
    assert!(output.status.success(), "deploy --json must succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();

    let value: serde_json::Value =
        serde_json::from_str(trimmed).expect("single-program --json output must be valid JSON");
    let obj = value
        .as_object()
        .expect("top-level value must be an object");
    assert_eq!(
        obj.get("status").and_then(|v| v.as_str()),
        Some("deployed"),
        "missing or wrong status in: {trimmed}"
    );
    assert!(
        obj.get("program").and_then(|v| v.as_str()).is_some(),
        "missing program field in: {trimmed}"
    );
}

#[test]
fn deploy_multi_program_json_output_parses_as_valid_json() {
    // Multi-program counterpart: parse the wrapper and assert the deploys
    // array shape via serde_json (rather than substring matching).
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "alpha");
    write_guest_program(temp.path(), "beta");
    write_guest_binary(temp.path(), "alpha");
    write_guest_binary(temp.path(), "beta");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("--json")
        .output()
        .expect("run deploy --json");
    assert!(
        output.status.success(),
        "multi-program deploy --json must succeed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();

    let value: serde_json::Value =
        serde_json::from_str(trimmed).expect("multi-program --json output must be valid JSON");
    let obj = value
        .as_object()
        .expect("top-level value must be an object");
    let deploys = obj
        .get("deploys")
        .and_then(|v| v.as_array())
        .expect("missing or non-array `deploys` field");
    assert_eq!(
        deploys.len(),
        2,
        "expected exactly two deploy entries; got: {trimmed}"
    );
    for entry in deploys {
        let entry_obj = entry.as_object().expect("each entry must be an object");
        assert!(
            entry_obj.contains_key("status"),
            "entry missing status: {entry}"
        );
        assert!(
            entry_obj.contains_key("program"),
            "entry missing program: {entry}"
        );
    }
}

#[test]
fn deploy_multi_program_json_records_failed_entries_with_error() {
    // Mixed-result deploy under --json: failed programs appear in the
    // array with status:"failed" and an `error` field, no program_id.
    // Process exits non-zero (any failure fails the deploy).
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "alpha");
    write_guest_program(temp.path(), "beta");
    write_guest_binary(temp.path(), "alpha");
    write_guest_binary(temp.path(), "beta");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("FAIL_PROGRAM", "beta.bin")
        .arg("deploy")
        .arg("--json")
        .output()
        .expect("run deploy --json with one failure");
    assert!(
        !output.status.success(),
        "must exit non-zero on any failure"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    assert!(
        trimmed.starts_with("{\"deploys\":[") && trimmed.ends_with("]}"),
        "stdout must remain a JSON object even with failures; got:\n{stdout}"
    );
    assert!(
        trimmed.contains("\"status\":\"deployed\"") && trimmed.contains("\"program\":\"alpha\""),
        "alpha must be present and submitted in:\n{trimmed}"
    );
    assert!(
        trimmed.contains("\"status\":\"failed\"") && trimmed.contains("\"program\":\"beta\""),
        "beta must be present and failed in:\n{trimmed}"
    );
    assert!(
        trimmed.contains("\"error\":\""),
        "failed entries must carry an `error` field; got:\n{trimmed}"
    );
}

#[test]
fn deploy_json_omits_tx_when_wallet_returns_none() {
    // Wallet stub today emits `Transaction hash is …` lines. To prove the JSON shape
    // omits `tx` when no value is available, set WALLET_NO_TX=1 — the stub
    // honors that and skips the tx line. Result: presence of the `tx` key
    // should now imply a real value (not `null`).
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    let custom = temp.path().join("custom.bin");
    fs::write(&custom, minimal_r0bf_binary()).expect("write custom bin");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("WALLET_NO_TX", "1")
        .arg("deploy")
        .arg("--program-path")
        .arg(&custom)
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"tx\":").not());
}

#[test]
fn deploy_requires_default_wallet_payer() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_wallet_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "hello");
    write_guest_binary(temp.path(), "hello");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("hello")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("deploy needs a funded default wallet")
                .and(predicate::str::contains("wallet topup")),
        );
    assert!(
        !wallet_stub_calls(temp.path())
            .iter()
            .any(|c| c.starts_with("program-loader")),
        "no deployment may be attempted without a payer"
    );
}

#[test]
fn deploy_rejects_non_risc0_program_binary() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    let custom = temp.path().join("custom.bin");
    fs::write(&custom, b"stub-program-bin").expect("write custom bin");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("--program-path")
        .arg(&custom)
        .assert()
        .failure()
        .stdout(predicate::str::contains("is not a risc0 program binary"));
    assert!(
        !wallet_stub_calls(temp.path())
            .iter()
            .any(|c| c.starts_with("account new") || c.starts_with("program-loader")),
        "a non-R0BF file must fail before any wallet account is created"
    );
}

#[test]
fn deploy_runs_program_loader_with_fresh_header_and_segment_accounts() {
    // A user ELF of 98_305 bytes spans two 96 KiB `program_loader`
    // segments, so deploy creates three fresh accounts (header + 2
    // segments) and pays with the default wallet.
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    let user_elf_len: u32 = 98_305;
    let mut bin = b"R0BF".to_vec();
    bin.extend_from_slice(&1u32.to_le_bytes());
    bin.extend_from_slice(&3u32.to_le_bytes());
    bin.extend_from_slice(b"hdr");
    bin.extend_from_slice(&user_elf_len.to_le_bytes());
    bin.extend(std::iter::repeat_n(0u8, user_elf_len as usize));
    bin.extend_from_slice(b"kernel");
    let custom = temp.path().join("big.bin");
    fs::write(&custom, &bin).expect("write big bin");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("--program-path")
        .arg(&custom)
        .arg("--json")
        .output()
        .expect("run deploy");
    assert!(
        output.status.success(),
        "deploy must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let calls = wallet_stub_calls(temp.path());
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.as_str() == "account new public")
            .count(),
        3,
        "header + 2 segment accounts expected: {calls:?}"
    );
    let deploy_call = calls
        .iter()
        .find(|c| c.starts_with("program-loader deploy"))
        .unwrap_or_else(|| panic!("program-loader deploy expected: {calls:?}"));
    assert_eq!(
        deploy_call,
        &format!(
            "program-loader deploy --elf {} --header Public/{} --segments Public/{} Public/{} --payer {}",
            custom.display(),
            STUB_NEW_ACCOUNT_IDS[0],
            STUB_NEW_ACCOUNT_IDS[1],
            STUB_NEW_ACCOUNT_IDS[2],
            VALID_PUBLIC_ADDRESS
        )
    );

    let value: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).expect("json");
    assert_eq!(value["status"], "deployed");
    assert_eq!(value["program"], "big");
    assert_eq!(value["program_id"], STUB_NEW_ACCOUNT_IDS[0]);
    assert_eq!(value["tx"], "deploy-big.bin", "last tx hash is reported");
    let expected_sha: String = {
        use sha2::{Digest as _, Sha256};
        Sha256::digest(&bin)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    assert_eq!(value["elf_sha256"], serde_json::json!(expected_sha));
}

#[test]
fn deploy_records_program_ids_in_deployments_json() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_deploy_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "alpha");
    write_guest_program(temp.path(), "beta");
    write_guest_binary(temp.path(), "alpha");
    write_guest_binary(temp.path(), "beta");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("FAIL_PROGRAM", "beta.bin")
        .arg("deploy")
        .assert()
        .failure()
        .stdout(predicate::str::contains("Program ids are recorded in"));

    let text = fs::read_to_string(temp.path().join(".scaffold/state/deployments.json"))
        .expect("deployments.json written");
    let value: serde_json::Value = serde_json::from_str(&text).expect("parse deployments.json");
    let programs = value["programs"].as_object().expect("programs object");
    assert_eq!(
        programs.keys().collect::<Vec<_>>(),
        vec!["alpha"],
        "only successful deploys are recorded: {text}"
    );
    assert_eq!(programs["alpha"]["program"], "alpha");
    assert_eq!(programs["alpha"]["program_id"], STUB_NEW_ACCOUNT_IDS[0]);
    assert_eq!(programs["alpha"]["payer"], VALID_PUBLIC_ADDRESS);
    assert!(programs["alpha"]["elf_sha256"].is_string(), "{text}");
}

#[test]
fn spel_subcommand_is_gone() {
    // LEZ 0.3 has no spel release: the `spel` passthrough was removed and
    // clap now rejects it like any other unknown subcommand.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["spel", "--", "program-id", "x.bin"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand 'spel'"));
}

#[test]
fn basecamp_help_lists_setup_install_launch_and_profile() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("basecamp")
        .arg("--help")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("setup")
                .and(predicate::str::contains("install"))
                .and(predicate::str::contains("launch"))
                .and(predicate::str::contains("profile")),
        );
}

#[test]
fn basecamp_install_help_has_no_source_flags() {
    // `install` is pure replay: no `--flake` / `--path` / `--profile`. Those
    // live on `basecamp modules` (sole writer of the captured source set).
    let out = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("basecamp")
        .arg("install")
        .arg("--help")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("--path"),
        "install must not take --path: {stdout}"
    );
    assert!(
        !stdout.contains("--flake"),
        "install must not take --flake: {stdout}"
    );
    assert!(
        !stdout.contains("--profile"),
        "install must not take --profile: {stdout}"
    );
}

#[test]
fn basecamp_modules_help_lists_flags() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("basecamp")
        .arg("modules")
        .arg("--help")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--path")
                .and(predicate::str::contains("--flake"))
                .and(predicate::str::contains("--show")),
        );
}

#[test]
fn basecamp_launch_help_requires_profile_argument() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("basecamp")
        .arg("launch")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("PROFILE"));
}

#[test]
fn basecamp_launch_without_profile_errors() {
    let temp = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("basecamp")
        .arg("launch")
        .assert()
        .failure();
}

#[test]
fn basecamp_develop_help_lists_module_and_dev_shell() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .args(["basecamp", "develop", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("MODULE").and(predicate::str::contains("--dev-shell")));
}

#[test]
fn basecamp_develop_unknown_module_errors_with_known_list() {
    // Module lookup runs before the `nix` presence check, so this is
    // deterministic in CI (no Nix needed): an unknown module name fails fast
    // and lists the captured modules.
    let temp = tempdir().expect("tempdir");
    let toml = format!(
        "{MINIMAL_SCAFFOLD_TOML}\n[modules.swap_module]\nflake = \"github:logos-co/swap-module#lgx\"\nrole = \"project\"\n"
    );
    fs::write(temp.path().join("scaffold.toml"), toml).expect("write scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["basecamp", "develop", "nonexistent"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("no module `nonexistent`")
                .and(predicate::str::contains("swap_module")),
        );
}

#[test]
fn basecamp_build_without_captured_modules_errors_before_nix() {
    // The empty-modules guard runs before the `nix` presence check, so this is
    // deterministic in CI: `build` never discovers, it only builds captured
    // project sources.
    let temp = tempdir().expect("tempdir");
    fs::write(temp.path().join("scaffold.toml"), MINIMAL_SCAFFOLD_TOML)
        .expect("write scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["basecamp", "build"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no project modules captured"));
}

#[test]
fn basecamp_build_unknown_module_errors_with_project_list() {
    // The `--module` filter is validated before the `nix` presence check, so
    // an unknown name fails fast and lists the captured project modules.
    let temp = tempdir().expect("tempdir");
    let toml = format!(
        "{MINIMAL_SCAFFOLD_TOML}\n[modules.swap_module]\nflake = \"github:logos-co/swap-module#lgx\"\nrole = \"project\"\n"
    );
    fs::write(temp.path().join("scaffold.toml"), toml).expect("write scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["basecamp", "build", "--module", "nonexistent"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("no project module `nonexistent`")
                .and(predicate::str::contains("swap_module")),
        );
}

#[test]
fn basecamp_run_unknown_module_errors_with_known_list() {
    // `run`'s module lookup runs before the `nix` presence check (mirrors
    // `develop`), so an unknown module name is deterministic in CI.
    let temp = tempdir().expect("tempdir");
    let toml = format!(
        "{MINIMAL_SCAFFOLD_TOML}\n[modules.swap_module]\nflake = \"github:logos-co/swap-module#lgx\"\nrole = \"project\"\n"
    );
    fs::write(temp.path().join("scaffold.toml"), toml).expect("write scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["basecamp", "run", "nonexistent"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("no module `nonexistent`")
                .and(predicate::str::contains("swap_module")),
        );
}

#[cfg(unix)]
#[test]
fn self_test_run_logged_success_shape() {
    // Hidden `self-test run-logged` hook drives `run_logged` against a
    // trivial subprocess (`true`). We assert the visible output shape
    // so future reshapes of `run_logged` don't silently regress the UX.
    let temp = tempdir().expect("tempdir");
    let log = temp.path().join("self-test.log");
    let out = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .args([
            "self-test",
            "run-logged",
            "--log",
            log.to_str().unwrap(),
            "--step",
            "self-test success",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    assert!(
        stdout.contains("self-test success"),
        "expected the step label to appear, got:\n{stdout}"
    );
    assert!(
        stdout.contains("tip: tail -f"),
        "expected tail-f hint on the default logged path, got:\n{stdout}"
    );
    assert!(
        stdout.contains("✓"),
        "expected a ✓ finalization line on success, got:\n{stdout}"
    );
    assert!(
        log.exists(),
        "expected the log file to be written at {}",
        log.display()
    );
}

#[cfg(unix)]
#[test]
fn self_test_run_logged_failure_shape() {
    // Failure path must bail (non-zero exit) and name the log path.
    let temp = tempdir().expect("tempdir");
    let log = temp.path().join("self-test.log");
    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .args([
            "self-test",
            "run-logged",
            "--log",
            log.to_str().unwrap(),
            "--step",
            "self-test failure",
            "--fail",
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("self-test failure") && stderr.contains("failed with"),
        "failure bail must name the step and exit status, got:\n{stderr}"
    );
    assert!(
        stderr.contains(&log.display().to_string()),
        "failure bail must reference the captured log path, got:\n{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn self_test_run_logged_print_output_streams_and_echoes_command() {
    // Under --print-output the shape is different: no `tip: tail -f`, has
    // `running: <cmd>`, still has ✓/✗ with duration.
    let temp = tempdir().expect("tempdir");
    let log = temp.path().join("self-test.log");
    let out = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .args([
            "self-test",
            "run-logged",
            "--log",
            log.to_str().unwrap(),
            "--step",
            "self-test streamed",
            "--print-output",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    assert!(
        stdout.contains("running: ") && stdout.contains("true"),
        "--print-output must echo the command, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("tip: tail -f"),
        "--print-output must not print the log-tail hint (no capture), got:\n{stdout}"
    );
    assert!(
        stdout.contains("✓"),
        "expected ✓ finalization under --print-output, got:\n{stdout}"
    );
}

#[test]
fn basecamp_docs_prints_compatibility_rules_anywhere() {
    // LLM-driven discoverability: `basecamp docs` must work without a
    // scaffold project (no scaffold.toml needed) so an agent can retrieve
    // the rules before setting anything up. Asserts on the top-level
    // heading + the Quick checklist anchor so trivial drift in the doc
    // body doesn't break the test.
    let temp = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["basecamp", "docs"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("# Basecamp Module Requirements")
                .and(predicate::str::contains("Quick checklist")),
        );
}

#[test]
fn basecamp_help_carries_docs_breadcrumb() {
    // Every basecamp subcommand's --help should mention `basecamp docs`
    // so an LLM exploring the CLI finds the compatibility doc without
    // filesystem context.
    for subcommand in &[
        "setup",
        "modules",
        "install",
        "launch",
        "build-portable",
        "doctor",
    ] {
        let out = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
            .args(["basecamp", subcommand, "--help"])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("basecamp docs"),
            "{subcommand} --help must reference `basecamp docs`; got:\n{stdout}"
        );
    }
}

#[test]
fn basecamp_profile_subcommand_does_not_exist() {
    // The `basecamp profile` subcommand was removed (KISS — feature was a
    // stub that returned "not yet implemented" and polluted --help). If
    // someone re-adds it, they should land a real implementation at the
    // same time.
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .args(["basecamp", "profile"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand").or(
            predicate::str::contains("error: unrecognized").or(predicate::str::contains("Usage:")),
        ));
}

#[test]
fn basecamp_setup_outside_project_errors() {
    let temp = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("basecamp")
        .arg("setup")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "This command must be run inside a logos-scaffold project",
        ));
}

#[test]
fn basecamp_install_outside_project_errors() {
    let temp = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("basecamp")
        .arg("install")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "This command must be run inside a logos-scaffold project",
        ));
}

#[test]
fn basecamp_build_portable_outside_project_errors() {
    // `build-portable` takes no source-set flags; the outside-project check
    // runs first regardless.
    let temp = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["basecamp", "build-portable"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "This command must be run inside a logos-scaffold project",
        ));
}

#[test]
fn basecamp_build_portable_rejects_flake_flag() {
    // `--flake` was removed from build-portable; source set lives in state,
    // managed by `basecamp modules`. Clap must reject the flag.
    let temp = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["basecamp", "build-portable", "--flake", ".#lgx-portable"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--flake"));
}

#[test]
fn basecamp_build_portable_rejects_dry_run_flag() {
    // `build-portable` is non-destructive and does not accept `--dry-run`.
    // Clap must reject the flag with a usage error.
    let temp = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["basecamp", "build-portable", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--dry-run"));
}

#[test]
fn basecamp_build_portable_inside_empty_project_emits_hint_to_capture_first() {
    // With state.project_sources empty, build-portable must refuse cleanly
    // and point the user at `basecamp modules` (it never auto-discovers on
    // its own; that's modules' job).
    let temp = tempdir().expect("tempdir");
    fs::write(temp.path().join("scaffold.toml"), MINIMAL_SCAFFOLD_TOML)
        .expect("write scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["basecamp", "build-portable"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("basecamp modules"));
}

#[test]
fn basecamp_install_before_setup_emits_hint() {
    let temp = tempdir().expect("tempdir");
    fs::write(temp.path().join("scaffold.toml"), MINIMAL_SCAFFOLD_TOML)
        .expect("write scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("basecamp")
        .arg("install")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "basecamp not set up yet; run: logos-scaffold basecamp setup",
        ));
}

#[test]
fn basecamp_launch_before_setup_emits_hint() {
    let temp = tempdir().expect("tempdir");
    fs::write(temp.path().join("scaffold.toml"), MINIMAL_SCAFFOLD_TOML)
        .expect("write scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("basecamp")
        .arg("launch")
        .arg("alice")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "basecamp not set up yet; run: logos-scaffold basecamp setup",
        ));
}

#[test]
fn basecamp_launch_setup_hint_takes_precedence_over_profile_validation() {
    // Pins the order of the two early gates: when the state is missing AND the
    // profile name is invalid, the user must see the setup hint — not a
    // confusing "unknown profile" error for a profile that doesn't exist yet
    // anyway. A future refactor that reorders the checks should fail this test.
    let temp = tempdir().expect("tempdir");
    fs::write(temp.path().join("scaffold.toml"), MINIMAL_SCAFFOLD_TOML)
        .expect("write scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("basecamp")
        .arg("launch")
        .arg("charlie")
        .assert()
        .failure()
        .stderr(predicate::str::contains("basecamp not set up yet"))
        .stderr(predicate::str::contains("unknown profile").not());
}

#[cfg(unix)]
#[test]
fn basecamp_launch_accepts_custom_profile_name() {
    let temp = tempdir().expect("tempdir");
    let project = temp.path();
    fs::write(project.join("scaffold.toml"), MINIMAL_SCAFFOLD_TOML).expect("write scaffold.toml");

    // Fake a completed setup so we get past the first gate. A custom profile
    // name (not alice/bob) is now accepted: launch advances to the
    // modules-captured check instead of rejecting the name outright.
    let state_dir = project.join(".scaffold/state");
    fs::create_dir_all(&state_dir).expect("mkdir state");
    fs::write(state_dir.join("basecamp.state"), fake_basecamp_state()).expect("write state");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(project)
        .arg("basecamp")
        .arg("launch")
        .arg("charlie")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("unknown profile")
                .not()
                .and(predicate::str::contains("no modules captured")),
        );
}

#[cfg(unix)]
#[test]
fn basecamp_launch_bails_when_no_modules_captured() {
    // launch scrubs and replays the captured module set. If [modules]
    // is empty, the replay is silently a no-op — the profile comes up with
    // zero modules installed, which violates the clean-slate guarantee.
    // Surface as an error with a concrete hint.
    let temp = tempdir().expect("tempdir");
    let project = temp.path();
    let scaffold_toml = format!(
        "{MINIMAL_SCAFFOLD_TOML}\n\
         [repos.basecamp]\n\
         source = \"https://example/basecamp\"\n\
         pin = \"deadbeef\"\n\
         build = \"nix-flake\"\n\
         attr = \"app\"\n\
         \n\
         [basecamp]\n\
         port_base = 60000\n\
         port_stride = 10\n"
    );
    fs::write(project.join("scaffold.toml"), scaffold_toml).expect("write scaffold.toml");

    // Fake a completed setup and a seeded alice profile dir so launch passes
    // the early gates and reaches the modules check.
    let state_dir = project.join(".scaffold/state");
    fs::create_dir_all(&state_dir).expect("mkdir state");
    fs::write(state_dir.join("basecamp.state"), fake_basecamp_state()).expect("write state");
    fs::create_dir_all(project.join(".scaffold/basecamp/profiles/alice")).expect("mkdir profile");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(project)
        .arg("basecamp")
        .arg("launch")
        .arg("alice")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("basecamp modules")
                .and(predicate::str::contains("destructive by default").not()),
        );
}

#[test]
fn basecamp_launch_outside_project_errors() {
    let temp = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("basecamp")
        .arg("launch")
        .arg("alice")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "This command must be run inside a logos-scaffold project",
        ));
}

fn find_single_report_archive(reports_dir: &Path) -> PathBuf {
    let mut archives = list_report_archives(reports_dir);
    assert_eq!(
        archives.len(),
        1,
        "expected exactly one report archive in {}",
        reports_dir.display()
    );
    archives.remove(0)
}

fn list_report_archives(reports_dir: &Path) -> Vec<PathBuf> {
    let mut archives = fs::read_dir(reports_dir)
        .expect("read reports dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.ends_with(".tar.gz"))
                    .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    archives.sort();
    archives
}

fn read_report_archive_entries(archive_path: &Path) -> Vec<(String, String)> {
    let file = fs::File::open(archive_path).expect("open report archive");
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    let mut entries = Vec::new();
    for entry in archive.entries().expect("archive entries") {
        let mut entry = entry.expect("archive entry");
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry
            .path()
            .expect("archive entry path")
            .display()
            .to_string();
        let mut body = String::new();
        entry.read_to_string(&mut body).expect("archive entry body");
        entries.push((path, body));
    }

    entries
}

fn archive_entry_exists(entries: &[(String, String)], suffix: &str) -> bool {
    entries.iter().any(|(path, _)| path.ends_with(suffix))
}

fn archive_entry_content<'a>(entries: &'a [(String, String)], suffix: &str) -> &'a str {
    entries
        .iter()
        .find(|(path, _)| path.ends_with(suffix))
        .map(|(_, body)| body.as_str())
        .unwrap_or_else(|| panic!("archive missing expected entry suffix `{suffix}`"))
}

fn write_scaffold_toml(project_root: &Path, lez_path: &Path) {
    write_scaffold_toml_with_localnet(project_root, lez_path, None, None);
}

#[cfg(unix)]
fn start_test_node_and_read_runtime_config(
    project_root: &Path,
    fixtures: &TestNodeFixtures,
    work_dir: &Path,
    extra_args: &[&str],
) -> serde_json::Value {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"));
    command
        .current_dir(project_root)
        .arg("test-node")
        .arg("start")
        .arg("--work-dir")
        .arg(work_dir)
        .arg("--preserve-work-dir");
    command.args(extra_args);
    command
        .arg("--timeout-sec")
        .arg("5")
        .arg("--json")
        .assert()
        .success();

    TestNodeStopGuard::new(project_root, work_dir, true).stop();

    let runtime_config_path = work_dir.join("sequencer_config.json");
    assert_test_node_launched_with_config(
        &fixtures.sequencer_observation_path,
        &runtime_config_path,
    );
    let runtime_config =
        fs::read_to_string(&runtime_config_path).expect("read runtime sequencer config");
    serde_json::from_str(&runtime_config).expect("parse runtime sequencer config")
}

#[cfg(unix)]
fn run_test_node_and_observe_config(
    project_root: &Path,
    fixtures: &TestNodeFixtures,
    extra_args: &[&str],
) -> serde_json::Value {
    let observed_path = project_root.join("observed-config.json");
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"));
    command
        .current_dir(project_root)
        .arg("test-node")
        .arg("run");
    command.args(extra_args);
    command
        .arg("--timeout-sec")
        .arg("5")
        .arg("--")
        .arg(&fixtures.python_path)
        .arg("-c")
        .arg(
            "import json, os, sys\n\
             cfg = json.load(open(os.environ['LGS_TEST_NODE_CONFIG_PATH'], encoding='utf-8'))\n\
             json.dump({\n\
                 'config_path': os.environ['LGS_TEST_NODE_CONFIG_PATH'],\n\
                 'block_create_timeout': cfg.get('block_create_timeout'),\n\
                 'retry_pending_blocks_timeout': cfg.get('retry_pending_blocks_timeout'),\n\
             }, open(sys.argv[1], 'w', encoding='utf-8'))\n",
        )
        .arg(&observed_path)
        .assert()
        .success();

    let observed = fs::read_to_string(&observed_path).expect("read observed config");
    let observed_json: serde_json::Value =
        serde_json::from_str(&observed).expect("parse observed config");
    let sequencer_config_path =
        test_node_observed_config_path(&fixtures.sequencer_observation_path);
    assert_eq!(
        observed_json["config_path"],
        serde_json::json!(sequencer_config_path.display().to_string())
    );
    observed_json
}

#[cfg(unix)]
fn assert_vendored_test_node_timing_preserved(fixtures: &TestNodeFixtures) {
    let vendored_config =
        fs::read_to_string(&fixtures.config_path).expect("read vendored sequencer config");
    let vendored_json: serde_json::Value =
        serde_json::from_str(&vendored_config).expect("parse vendored sequencer config");
    assert_eq!(
        vendored_json["block_create_timeout"],
        serde_json::json!("2s")
    );
    assert_eq!(
        vendored_json["retry_pending_blocks_timeout"],
        serde_json::json!("3s")
    );
}

#[cfg(unix)]
struct TestNodeStopGuard {
    project_root: PathBuf,
    work_dir: PathBuf,
    preserve_work_dir: bool,
    active: bool,
}

#[cfg(unix)]
impl TestNodeStopGuard {
    fn new(project_root: &Path, work_dir: &Path, preserve_work_dir: bool) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            work_dir: work_dir.to_path_buf(),
            preserve_work_dir,
            active: true,
        }
    }

    fn stop(mut self) {
        test_node_stop_command(&self.project_root, &self.work_dir, self.preserve_work_dir)
            .assert()
            .success();
        self.active = false;
    }
}

#[cfg(unix)]
impl Drop for TestNodeStopGuard {
    fn drop(&mut self) {
        if self.active {
            match test_node_stop_command(&self.project_root, &self.work_dir, self.preserve_work_dir)
                .output()
            {
                Ok(output) if output.status.success() => {}
                Ok(output) => {
                    eprintln!(
                        "test-node cleanup failed for {}: status={}\nstdout:\n{}\nstderr:\n{}",
                        self.work_dir.display(),
                        output.status,
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
                Err(err) => {
                    eprintln!(
                        "test-node cleanup failed for {}: {err}",
                        self.work_dir.display()
                    );
                }
            }
        }
    }
}

#[cfg(unix)]
fn test_node_stop_command(
    project_root: &Path,
    work_dir: &Path,
    preserve_work_dir: bool,
) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"));
    command
        .current_dir(project_root)
        .arg("test-node")
        .arg("stop")
        .arg("--node")
        .arg(work_dir);
    if preserve_work_dir {
        command.arg("--preserve-work-dir");
    }
    command
}

fn write_scaffold_toml_with_localnet(
    project_root: &Path,
    lez_path: &Path,
    localnet_port: Option<u16>,
    risc0_dev_mode: Option<bool>,
) {
    // Schema 0.3.0: no url, no [basecamp] / [basecamp.modules.*], no
    // [repos.spel]; path is set on lez (tests need a literal path to a local
    // stub repo).
    let mut content = format!(
        "[scaffold]\nversion = \"0.3.0\"\ncache_root = \"{}\"\n\n[repos.lez]\nsource = \"https://github.com/logos-blockchain/logos-execution-zone.git\"\npin = \"{}\"\npath = \"{}\"\n\n[wallet]\nhome_dir = \".scaffold/wallet\"\n",
        project_root.join("cache").display(),
        TEST_PIN,
        lez_path.display(),
    );

    if let Some(port) = localnet_port {
        let risc0_dev_mode = risc0_dev_mode.unwrap_or(true);
        content.push_str(&format!(
            "\n[localnet]\nport = {port}\nrisc0_dev_mode = {risc0_dev_mode}\n"
        ));
    }

    fs::write(project_root.join("scaffold.toml"), content).expect("write scaffold.toml");
}

fn unused_local_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind unused local port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn setup_wallet_project(project_root: &Path, sequencer_addr: Option<&str>) {
    let lez_path = project_root.join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_wallet_stub(&lez_path);
    write_scaffold_toml(project_root, &lez_path);
    write_wallet_config(project_root, sequencer_addr);
}

/// `setup_wallet_project` plus a default wallet (the deploy payer).
fn setup_deploy_project(project_root: &Path, sequencer_addr: Option<&str>) {
    setup_wallet_project(project_root, sequencer_addr);
    write_default_wallet_state(project_root);
}

fn write_wallet_config(project_root: &Path, sequencer_addr: Option<&str>) {
    let wallet_home = project_root.join(".scaffold/wallet");
    fs::create_dir_all(&wallet_home).expect("create wallet home");
    let path = wallet_home.join("wallet_config.json");

    // LEZ 0.3 wallet config shape: `sequencer_addr` is nested under
    // `sequencer`; there is no `initial_accounts` list any more.
    let mut value = serde_json::json!({
        "seq_poll_timeout": "1s",
        "seq_tx_poll_max_blocks": 120
    });
    if let Some(addr) = sequencer_addr {
        value["sequencer"] = serde_json::json!({ "sequencer_addr": addr });
    }

    let text = serde_json::to_string_pretty(&value).expect("wallet config json");
    fs::write(path, text).expect("write wallet config");
}

/// Write `default_address=<VALID_PUBLIC_ADDRESS>` to
/// `.scaffold/state/wallet.state` — the payer `deploy` requires.
fn write_default_wallet_state(project_root: &Path) {
    let state_dir = project_root.join(".scaffold/state");
    fs::create_dir_all(&state_dir).expect("create state dir");
    fs::write(
        state_dir.join("wallet.state"),
        format!("default_address={VALID_PUBLIC_ADDRESS}\n"),
    )
    .expect("write wallet.state");
}

const DEV_FUNDER_ACCOUNT_ID: &str = "CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r";
const DEV_FUNDER_ADDRESS: &str = "Public/CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r";
const DEV_FUNDER_PRIVATE_KEY_HEX: &str =
    "7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb";

/// Account ids the wallet stub hands out (in order, cycling) for
/// `wallet account new public`. All decode to 32 bytes of base58.
const STUB_NEW_ACCOUNT_IDS: [&str; 6] = [
    "6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV",
    "8zxWNm1qh6FLsJpVBuDxdxcTm55qHPgFEdqJpPVu1fuy",
    "2ECgkFTaXzwjJBXR7ZKmXYQtpHbvTTHK9Auma4NL9AUo",
    "EkA2CTdiiPw6b2syzMxBZcZ33n4W5RCbWH3DDsonWy6w",
    "B6aAVxTLwsYhMhyF9w58WqxCYXMhjTHij82baLZabncL",
    "BzdBoL4JRa5M873cuWb9rbYgASr1pXyaAZ1YW9ertWH9",
];

/// LEZ 0.3 `wallet` stub. State lives in `$LEE_WALLET_HOME_DIR`:
///   `.funder-missing`   → the dev funder is not in the wallet until
///                         `account import public` runs (which deletes it)
///   `.account-counter`  → index into `STUB_NEW_ACCOUNT_IDS`
///   `.calls`            → one line per invocation (argv), for assertions
/// Env knobs:
///   `EXPECT_PASSWORD`     → password-reading commands require it on stdin
///   `TOPUP_FAIL_CONNECT`, `TOPUP_FAIL_TIMEOUT`, `TOPUP_FAIL_TIMEOUT_TRANSPORT`
///   `FAIL_PROGRAM=<bin>`, `DEPLOY_FAIL_TIMEOUT_TRANSPORT=<bin>`, `WALLET_NO_TX=1`
///   `ACCOUNT_LIST_FAIL_CONNECT=1` → `account list` fails with a transport error
fn write_wallet_stub(lez_path: &Path) {
    let path = lez_path.join("target/release/wallet");
    fs::create_dir_all(path.parent().expect("parent")).expect("create wallet binary dir");
    let script = r#"#!/bin/sh
set -eu

home="${LEE_WALLET_HOME_DIR:-.}"
if [ -d "$home" ]; then
  printf '%s\n' "$*" >> "$home/.calls"
fi

require_password_if_configured() {
  if [ "${EXPECT_PASSWORD:-}" = "" ]; then
    return 0
  fi
  IFS= read -r provided || true
  if [ "$provided" != "$EXPECT_PASSWORD" ]; then
    echo "password mismatch" >&2
    exit 3
  fi
}

funder_id="CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r"
funder_key="7f273098f25b71e6c005a9519f2678da8d1c7f01f6a27778e2d9948abdf901fb"

if [ "$#" -ge 2 ] && [ "$1" = "account" ] && [ "$2" = "list" ]; then
  if [ "${ACCOUNT_LIST_FAIL_CONNECT:-0}" = "1" ]; then
    echo "error sending request for url (http://127.0.0.1:3040/): Connection refused (os error 111)" >&2
    exit 1
  fi
  echo "/ Public/6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV"
  echo "/ Private/8zxWNm1qh6FLsJpVBuDxdxcTm55qHPgFEdqJpPVu1fuy"
  if [ ! -f "$home/.funder-missing" ]; then
    echo "Public/$funder_id"
  fi
  exit 0
fi

if [ "$#" -ge 5 ] && [ "$1" = "account" ] && [ "$2" = "import" ] && [ "$3" = "public" ] && [ "$4" = "--private-key" ]; then
  require_password_if_configured
  if [ "$5" != "$funder_key" ]; then
    echo "unexpected private key" >&2
    exit 5
  fi
  rm -f "$home/.funder-missing"
  echo "Imported public account Public/$funder_id"
  exit 0
fi

if [ "$#" -ge 3 ] && [ "$1" = "account" ] && [ "$2" = "new" ] && [ "$3" = "public" ]; then
  require_password_if_configured
  n=0
  if [ -f "$home/.account-counter" ]; then
    n="$(cat "$home/.account-counter")"
  fi
  echo $((n + 1)) > "$home/.account-counter"
  case $((n % 6)) in
    0) id=6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV ;;
    1) id=8zxWNm1qh6FLsJpVBuDxdxcTm55qHPgFEdqJpPVu1fuy ;;
    2) id=2ECgkFTaXzwjJBXR7ZKmXYQtpHbvTTHK9Auma4NL9AUo ;;
    3) id=EkA2CTdiiPw6b2syzMxBZcZ33n4W5RCbWH3DDsonWy6w ;;
    4) id=B6aAVxTLwsYhMhyF9w58WqxCYXMhjTHij82baLZabncL ;;
    *) id=BzdBoL4JRa5M873cuWb9rbYgASr1pXyaAZ1YW9ertWH9 ;;
  esac
  echo "Generated new account with account_id Public/$id at path /$n"
  exit 0
fi

if [ "$#" -ge 2 ] && [ "$1" = "auth-transfer" ] && [ "$2" = "send" ]; then
  require_password_if_configured
  if [ "${3:-}" != "--from" ] || [ "${4:-}" != "Public/$funder_id" ]; then
    echo "unexpected send source: $*" >&2
    exit 6
  fi
  if [ -f "$home/.funder-missing" ]; then
    echo "Error: account Public/$funder_id not found in wallet storage" >&2
    exit 9
  fi
  if [ "${TOPUP_FAIL_CONNECT:-0}" = "1" ]; then
    echo "connection refused" >&2
    exit 1
  fi
  if [ "${TOPUP_FAIL_TIMEOUT:-0}" = "1" ]; then
    echo "Error: Transaction not found in preconfigured amount of blocks" >&2
    exit 1
  fi
  if [ "${TOPUP_FAIL_TIMEOUT_TRANSPORT:-0}" = "1" ]; then
    # A sequencer dying mid-transfer: the tx was submitted (hash on stdout)
    # and the poller then failed with BOTH the confirmation-timeout line AND
    # a transport token in the same combined output. The confirmation-timeout
    # branch must own this (status: pending), not the connectivity classifier.
    echo "Transaction hash is a0a1a2a3"
    echo "Error: Transaction not found in preconfigured amount of blocks" >&2
    echo "error sending request for url (http://127.0.0.1:3040/): Connection refused (os error 111)" >&2
    exit 1
  fi
  echo "Transaction hash is a0a1a2a3"
  echo "Transaction data is Transfer"
  exit 0
fi

if [ "$#" -ge 2 ] && [ "$1" = "program-loader" ] && [ "$2" = "deploy" ]; then
  require_password_if_configured
  shift 2
  bin_path=""
  header=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --elf) bin_path="$2"; shift 2 ;;
      --header) header="$2"; shift 2 ;;
      *) shift ;;
    esac
  done
  bin_name="$(basename "$bin_path")"
  if [ "${FAIL_PROGRAM:-}" = "$bin_name" ]; then
    echo "simulated deploy failure for $bin_name" >&2
    exit 2
  fi
  if [ "${DEPLOY_FAIL_TIMEOUT_TRANSPORT:-}" = "$bin_name" ]; then
    # Sequencer dying mid-deploy: the submission failed with BOTH the
    # confirmation-timeout line AND a transport token in the combined output.
    # deploy's loop must classify this as connectivity (sequencer-unavailable
    # hint), not "inspect sequencer logs and retry".
    echo "Error: Transaction not found in preconfigured amount of blocks" >&2
    echo "error sending request for url (http://127.0.0.1:3040/): Connection refused (os error 111)" >&2
    exit 1
  fi
  # WALLET_NO_TX=1: simulate the wallet not surfacing a tx identifier so
  # tests can assert the deploy JSON omits the `tx` key when None.
  if [ "${WALLET_NO_TX:-0}" != "1" ]; then
    echo "Transaction hash is segment-$bin_name"
    echo "Transaction hash is deploy-$bin_name"
  fi
  echo "Program deployed. Header account id: ${header#Public/}"
  exit 0
fi

if [ "$#" -ge 1 ] && [ "$1" = "--version" ]; then
  echo "wallet stub 0.3.1"
  exit 0
fi

if [ "$#" -ge 1 ] && [ "$1" = "check-health" ]; then
  require_password_if_configured
  echo "ok"
  exit 0
fi

echo "unsupported wallet invocation: $*" >&2
exit 2
"#;
    fs::write(&path, script).expect("write wallet stub");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod");
    }
}

/// Mark the stub wallet as not (yet) holding the dev funder key.
fn mark_dev_funder_missing(project_root: &Path) {
    fs::write(project_root.join(".scaffold/wallet/.funder-missing"), "")
        .expect("write funder-missing marker");
}

/// argv lines the stub wallet recorded, in invocation order.
fn wallet_stub_calls(project_root: &Path) -> Vec<String> {
    fs::read_to_string(project_root.join(".scaffold/wallet/.calls"))
        .unwrap_or_default()
        .lines()
        .map(ToString::to_string)
        .collect()
}

fn write_guest_program(project_root: &Path, name: &str) {
    let dir = project_root.join("methods/guest/src/bin");
    fs::create_dir_all(&dir).expect("create guest program dir");
    fs::write(dir.join(format!("{name}.rs")), "fn main() {}\n").expect("write guest source");
}

fn write_guest_binary(project_root: &Path, name: &str) {
    let dir = project_root.join(GUEST_BIN_REL_PATH);
    fs::create_dir_all(&dir).expect("create guest binary dir");
    fs::write(dir.join(format!("{name}.bin")), minimal_r0bf_binary()).expect("write guest binary");
}

/// Smallest valid risc0 `ProgramBinary` container: `R0BF`, format version 1,
/// an empty header, a 4-byte user ELF (one `program_loader` segment) and a
/// stub kernel.
fn minimal_r0bf_binary() -> Vec<u8> {
    let mut bytes = b"R0BF".to_vec();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(b"\x7fELF");
    bytes.extend_from_slice(b"kernel");
    bytes
}

struct RpcStub {
    url: String,
    stop: Arc<AtomicBool>,
    addr: String,
    handle: Option<thread::JoinHandle<()>>,
}

impl RpcStub {
    /// Sequencer RPC stub answering every request with an advancing
    /// last-block id.
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind rpc stub");
        let addr = listener.local_addr().expect("local addr");
        let addr_str = addr.to_string();
        listener
            .set_nonblocking(true)
            .expect("set nonblocking rpc stub");

        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            let mut block_id: u64 = 123;
            while !stop_flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        respond_last_block(&mut stream, block_id);
                        block_id += 1;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            url: format!("http://{addr_str}"),
            stop,
            addr: addr_str,
            handle: Some(handle),
        }
    }

    /// Foreign HTTP responder squatting on the sequencer port: answers every
    /// request with a 404 and an HTML body. `map_ureq_error` maps this to
    /// `RpcReachabilityError::Other`, so `preflight_sequencer_reachability`
    /// takes its warn-and-continue arm rather than bailing — the exact arm
    /// whose warning must land on stderr and not the `--json` stdout stream.
    fn start_http_error() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind rpc stub");
        let addr = listener.local_addr().expect("local addr");
        let addr_str = addr.to_string();
        listener
            .set_nonblocking(true)
            .expect("set nonblocking rpc stub");

        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => respond_http_404(&mut stream),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            url: format!("http://{addr_str}"),
            stop,
            addr: addr_str,
            handle: Some(handle),
        }
    }
}

impl Drop for RpcStub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(&self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn respond_http_404(stream: &mut TcpStream) {
    let mut buf = [0_u8; 4096];
    let _ = stream.read(&mut buf);

    // HTML with a leading `<`: if this body ever reaches `--json` stdout, `jq`
    // fails on the first byte ("Invalid numeric literal at line 1, column 8").
    let body = "<!DOCTYPE HTML><html><body><p>Error code 404: Nothing matches the given URI.</p></body></html>";
    let response = format!(
        "HTTP/1.1 404 Not Found\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn respond_last_block(stream: &mut TcpStream, block_id: u64) {
    let mut buf = [0_u8; 4096];
    let _ = stream.read(&mut buf);

    let body = format!(r#"{{"jsonrpc":"2.0","result":{block_id},"id":1}}"#);
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

#[test]
fn lgs_help_usage_line_shows_lgs() {
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: lgs"));
}

#[test]
fn logos_scaffold_help_usage_line_shows_logos_scaffold() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: logos-scaffold"));
}

#[test]
fn lgs_help_subcommand_uses_invoked_bin_name() {
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .arg("help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: lgs"));
}

#[test]
fn logos_scaffold_help_subcommand_uses_invoked_bin_name() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: logos-scaffold"));
}

#[test]
fn lgs_no_args_uses_invoked_bin_name() {
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: lgs"));
}

#[test]
fn logos_scaffold_no_args_uses_invoked_bin_name() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: logos-scaffold"));
}

#[test]
fn lgs_and_logos_scaffold_advertise_same_subcommands() {
    let subcommands = [
        "create", "new", "setup", "build", "deploy", "wallet", "localnet", "doctor", "report",
    ];

    let lgs_help = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .arg("--help")
        .output()
        .expect("run lgs --help");
    let ls_help = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("--help")
        .output()
        .expect("run logos-scaffold --help");

    assert!(lgs_help.status.success(), "lgs --help failed");
    assert!(ls_help.status.success(), "logos-scaffold --help failed");

    let lgs_out = String::from_utf8_lossy(&lgs_help.stdout);
    let ls_out = String::from_utf8_lossy(&ls_help.stdout);

    for sub in subcommands {
        assert!(lgs_out.contains(sub), "lgs help missing subcommand `{sub}`");
        assert!(
            ls_out.contains(sub),
            "logos-scaffold help missing subcommand `{sub}`"
        );
    }
}

#[test]
fn lgs_and_logos_scaffold_version_match() {
    let lgs_ver = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .arg("--version")
        .output()
        .expect("run lgs --version");
    let ls_ver = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("--version")
        .output()
        .expect("run logos-scaffold --version");

    assert!(lgs_ver.status.success());
    assert!(ls_ver.status.success());

    let lgs_version_number = String::from_utf8_lossy(&lgs_ver.stdout)
        .split_whitespace()
        .last()
        .unwrap_or_default()
        .to_string();
    let ls_version_number = String::from_utf8_lossy(&ls_ver.stdout)
        .split_whitespace()
        .last()
        .unwrap_or_default()
        .to_string();

    assert_eq!(
        lgs_version_number, ls_version_number,
        "version numbers differ"
    );
    assert!(!lgs_version_number.is_empty(), "version number is empty");
}

#[test]
fn completions_bash_prints_script_covering_both_bin_names() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .args(["completions", "bash"])
        .output()
        .expect("run lgs completions bash");
    assert!(output.status.success(), "expected success exit");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("complete -F _lgs"),
        "missing primary binding: {stdout}"
    );
    assert!(
        stdout.contains("logos-scaffold"),
        "missing alias binding: {stdout}"
    );
}

#[test]
fn completions_zsh_compdef_directive_covers_both_names() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .args(["completions", "zsh"])
        .output()
        .expect("run lgs completions zsh");
    assert!(output.status.success(), "expected success exit");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let compdef_headers = stdout.matches("#compdef").count();
    assert_eq!(
        compdef_headers, 1,
        "expected exactly one #compdef header, got {compdef_headers}: {stdout}"
    );
    assert!(
        stdout.starts_with("#compdef lgs logos-scaffold\n"),
        "expected `#compdef lgs logos-scaffold` directive so autoload \
         registers both names at compinit time; got head: {:?}",
        stdout.lines().next()
    );
}

#[test]
fn completions_cover_basecamp_subcommands() {
    // Regression guard: the completion scripts are generated from the full
    // clap command tree, so every `basecamp` sub-subcommand should appear in
    // both bash and zsh output. If someone accidentally hides a basecamp
    // subcommand (e.g. gating it with clap feature flags), shell-tab users
    // would silently lose completion — this test catches that at build time.
    const BASECAMP_SUBS: &[&str] = &[
        "setup",
        "modules",
        "install",
        "launch",
        "build-portable",
        "doctor",
        "docs",
    ];

    for shell in &["bash", "zsh"] {
        let output = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
            .args(["completions", shell])
            .output()
            .unwrap_or_else(|e| panic!("run lgs completions {shell}: {e}"));
        assert!(output.status.success(), "expected success exit for {shell}");
        let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
        assert!(
            stdout.contains("basecamp"),
            "{shell} completion script missing `basecamp`:\n{stdout}"
        );
        for sub in BASECAMP_SUBS {
            assert!(
                stdout.contains(sub),
                "{shell} completion script missing `basecamp {sub}`; \
                 stdout head:\n{}",
                stdout.lines().take(40).collect::<Vec<_>>().join("\n")
            );
        }
    }
}

#[test]
fn completions_bash_output_is_syntax_clean() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .args(["completions", "bash"])
        .output()
        .expect("run lgs completions bash");
    assert!(output.status.success(), "expected success exit");

    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("lgs.bash");
    fs::write(&path, &output.stdout).expect("write script");

    let syntax = std::process::Command::new("bash")
        .arg("-n")
        .arg(&path)
        .output()
        .expect("bash -n");
    assert!(
        syntax.status.success(),
        "bash -n failed: {}",
        String::from_utf8_lossy(&syntax.stderr)
    );
}

#[test]
fn completions_zsh_output_is_syntax_clean() {
    if std::process::Command::new("zsh")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping: zsh not available");
        return;
    }

    let output = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .args(["completions", "zsh"])
        .output()
        .expect("run lgs completions zsh");
    assert!(output.status.success(), "expected success exit");

    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("_lgs");
    fs::write(&path, &output.stdout).expect("write script");

    let syntax = std::process::Command::new("zsh")
        .arg("-n")
        .arg(&path)
        .output()
        .expect("zsh -n");
    assert!(
        syntax.status.success(),
        "zsh -n failed: {}",
        String::from_utf8_lossy(&syntax.stderr)
    );
}

#[test]
fn completions_unsupported_shell_errors() {
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .args(["completions", "fish"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("fish"));
}

#[test]
fn completions_bash_help_shows_install_instructions() {
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .args(["completions", "bash", "--help"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("bash-completion/completions/lgs")
                .and(predicate::str::contains("logos-scaffold")),
        );
}

#[test]
fn completions_zsh_help_shows_install_instructions() {
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .args(["completions", "zsh", "--help"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("~/.zfunc/_lgs")
                .and(predicate::str::contains("oh-my-zsh"))
                .and(predicate::str::contains("compinit")),
        );
}

#[test]
fn completions_missing_shell_arg_errors() {
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .arg("completions")
        .assert()
        .failure();
}

#[test]
fn completions_does_not_write_filesystem() {
    let temp = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .args(["completions", "bash"])
        .assert()
        .success();

    let entries: Vec<_> = fs::read_dir(temp.path())
        .expect("read tempdir")
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        entries.is_empty(),
        "completions must not write to cwd, found: {:?}",
        entries.iter().map(|e| e.path()).collect::<Vec<_>>()
    );
}

#[test]
fn init_creates_scaffold_toml_and_dirs() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .success();

    assert!(
        temp.path().join("scaffold.toml").exists(),
        "scaffold.toml missing"
    );
    assert!(
        temp.path().join(".scaffold/state").is_dir(),
        ".scaffold/state missing"
    );
    assert!(
        temp.path().join(".scaffold/logs").is_dir(),
        ".scaffold/logs missing"
    );

    let gitignore = fs::read_to_string(temp.path().join(".gitignore")).expect("read .gitignore");
    assert!(
        gitignore.lines().any(|l| l.trim() == ".scaffold"),
        ".gitignore must contain .scaffold, got: {gitignore:?}"
    );
}

#[test]
fn init_refreshes_skills_when_already_at_v0_3_0_scaffold_toml() {
    let temp = tempdir().expect("tempdir");
    let scaffold_path = temp.path().join("scaffold.toml");
    // First, run init to lay down a fresh v0.3.0 file plus the AI skill set.
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .success();
    let original_scaffold = fs::read_to_string(&scaffold_path).expect("read scaffold.toml");

    // Second invocation must succeed (no migration needed) and refresh the
    // shipped skill set, but it must not rewrite scaffold.toml.
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("already at schema"))
        .stdout(predicate::str::contains("AI skills refreshed"));

    let after_scaffold = fs::read_to_string(&scaffold_path).expect("read scaffold.toml");
    assert_eq!(
        after_scaffold, original_scaffold,
        "init must not overwrite an already-migrated scaffold.toml"
    );

    for skill in ["lgs-cli", "lez-template", "basecamp"] {
        assert!(
            temp.path()
                .join(format!(".claude/skills/{skill}/SKILL.md"))
                .is_file(),
            "claude skill `{skill}` must be present after re-init"
        );
        assert!(
            temp.path()
                .join(format!(".cursor/rules/{skill}.mdc"))
                .is_file(),
            "cursor rule `{skill}` must be present after re-init"
        );
    }
    assert!(
        !temp
            .path()
            .join(".claude/skills/lez-framework-template")
            .exists(),
        "the spel-era lez-framework skill is no longer shipped"
    );
    assert!(
        temp.path().join("AGENTS.md").is_file(),
        "AGENTS.md must be present after re-init"
    );
}

#[test]
fn init_migrates_pre_v0_3_0_scaffold_toml_in_place() {
    let temp = tempdir().expect("tempdir");
    let scaffold_path = temp.path().join("scaffold.toml");
    let original = "# user comment\n[scaffold]\nversion = \"0.1.0\"\n\n[repos.lez]\nurl = \"u\"\nsource = \"u\"\npath = \"p\"\npin = \"abc\"\n";
    fs::write(&scaffold_path, original).expect("seed scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("migrated to schema v0.3.0")
                // `source = "u"` is not upstream LEZ: the pin is kept and the
                // user is asked to re-pin by hand.
                .and(predicate::str::contains("points at a fork (u)")),
        );

    let after = fs::read_to_string(&scaffold_path).expect("read scaffold.toml");
    assert!(
        after.contains("# user comment"),
        "user comments must be preserved; got:\n{after}"
    );
    assert!(
        after.contains("version = \"0.3.0\""),
        "[scaffold].version must be bumped; got:\n{after}"
    );
    assert!(
        !after.contains("[repos.spel]"),
        "no [repos.spel] in the 0.3.0 schema; got:\n{after}"
    );
    assert!(
        after.contains("pin = \"abc\""),
        "a fork pin must be kept; got:\n{after}"
    );
    assert!(
        !after.contains("url ="),
        "url field must be stripped; got:\n{after}"
    );
}

#[test]
fn init_migrates_v0_2_0_scaffold_toml_to_lez_0_3() {
    // The LEZ 0.1/0.2-era schema: spel repo, circuits, IDL settings and an
    // old upstream LEZ pin. `init` drops the spel-era sections and re-pins
    // LEZ to the supported 0.3 release, keeping everything else.
    let temp = tempdir().expect("tempdir");
    let scaffold_path = temp.path().join("scaffold.toml");
    let original = r#"# my project
[scaffold]
version = "0.2.0"

[repos.lez]
source = "https://github.com/logos-blockchain/logos-execution-zone.git"
pin = "cf3639d8252040d13b3d4e933feb19b42c76e14a"

[repos.spel]
source = "https://github.com/logos-co/spel.git"
pin = "73fc462eb8f0a4d00f1a846437c627ec2e523f83"

[wallet]
home_dir = ".scaffold/wallet"

[framework]
kind = "default"
version = "0.1.0"

[framework.idl]
spec = "lssa-idl/0.1.0"
path = "idl"

[localnet]
port = 3055
risc0_dev_mode = true

[circuits]
version = "0.4.1"
"#;
    fs::write(&scaffold_path, original).expect("seed scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("migrated to schema v0.3.0")
                .and(predicate::str::contains("removed [repos.spel]"))
                .and(predicate::str::contains("removed [circuits]"))
                .and(predicate::str::contains("removed [framework.idl]"))
                .and(predicate::str::contains("re-pinned [repos.lez]")),
        );

    let after = fs::read_to_string(&scaffold_path).expect("read scaffold.toml");
    assert!(after.starts_with("# my project"), "{after}");
    assert!(after.contains("version = \"0.3.0\""), "{after}");
    for gone in ["[repos.spel]", "[circuits]", "[framework.idl]"] {
        assert!(!after.contains(gone), "{gone} must be removed:\n{after}");
    }
    assert!(
        after.contains("pin = \"1a003bfbfb89fcf987f62975cd08a401169e1b8c\""),
        "LEZ must be re-pinned to v0.3.1:\n{after}"
    );
    assert!(after.contains("port = 3055"), "{after}");

    // The migrated file is accepted by project-scoped commands.
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("already at schema"));
}

#[test]
fn init_refuses_lez_framework_projects_without_touching_them() {
    let temp = tempdir().expect("tempdir");
    let scaffold_path = temp.path().join("scaffold.toml");
    let original = "[scaffold]\nversion = \"0.2.0\"\n\n[repos.lez]\nsource = \"https://github.com/logos-blockchain/logos-execution-zone.git\"\npin = \"abc\"\n\n[repos.spel]\nsource = \"https://github.com/logos-co/spel.git\"\npin = \"abc\"\n\n[framework]\nkind = \"lez-framework\"\nversion = \"0.1.0\"\n";
    fs::write(&scaffold_path, original).expect("seed scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("spel").and(predicate::str::contains("LEZ 0.3")));

    assert_eq!(
        fs::read_to_string(&scaffold_path).expect("read scaffold.toml"),
        original,
        "a refused migration must leave scaffold.toml untouched"
    );
}

#[test]
fn lez_framework_project_is_rejected_with_spel_explanation() {
    let temp = tempdir().expect("tempdir");
    let toml = MINIMAL_SCAFFOLD_TOML.replace("kind = \"default\"", "kind = \"lez-framework\"");
    fs::write(temp.path().join("scaffold.toml"), toml).expect("seed scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("doctor")
        .assert()
        .failure()
        .stderr(predicate::str::contains("spel").and(predicate::str::contains("LEZ 0.3")));
}

#[test]
fn new_rejects_lez_framework_template() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .args(["new", "demo", "--template", "lez-framework"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("spel").and(predicate::str::contains("LEZ 0.3")));
    assert!(
        !temp.path().join("demo").exists(),
        "a rejected template must not create the project directory"
    );
}

#[test]
fn init_splits_basecamp_lgpm_flake_into_repos_lgpm() {
    let temp = tempdir().expect("tempdir");
    let scaffold_path = temp.path().join("scaffold.toml");
    // Pre-0.2.0: lgpm pinned via [basecamp].lgpm_flake (canonical
    // github:owner/repo/<sha>#attr form). After `lgs init`, the flake ref
    // must be split into source/pin/attr under [repos.lgpm].
    let original = r#"[scaffold]
version = "0.1.0"

[repos.lez]
source = "https://example/lez.git"
pin = "deadbeef"

[repos.spel]
source = "https://example/spel.git"
pin = "deadbeef"

[basecamp]
pin = "deadbeef"
source = "https://example/basecamp"
lgpm_flake = "github:logos-co/logos-package-manager/cafef00dcafef00dcafef00dcafef00dcafef00d#cli"
port_base = 60000
port_stride = 10

[wallet]
home_dir = ".scaffold/wallet"
"#;
    fs::write(&scaffold_path, original).expect("seed scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .success();

    let after = fs::read_to_string(&scaffold_path).expect("read scaffold.toml");
    assert!(
        !after.contains("lgpm_flake"),
        "lgpm_flake key must be removed; got:\n{after}"
    );
    assert!(
        after.contains("[repos.lgpm]"),
        "[repos.lgpm] section must be appended; got:\n{after}"
    );
    assert!(
        after.contains("github:logos-co/logos-package-manager"),
        "[repos.lgpm].source must carry the flake source; got:\n{after}"
    );
    assert!(
        after.contains("cafef00dcafef00dcafef00dcafef00dcafef00d"),
        "[repos.lgpm].pin must be the SHA from the flake ref; got:\n{after}"
    );
    assert!(
        after.contains("attr = \"cli\""),
        "[repos.lgpm].attr must carry the flake attr; got:\n{after}"
    );

    // Re-running init on the now-migrated config must succeed (skills get
    // refreshed) and report that no migration was needed.
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("already at schema"))
        .stdout(predicate::str::contains("AI skills refreshed"));
}

#[test]
fn init_appends_gitignore_once() {
    let temp = tempdir().expect("tempdir");
    fs::write(temp.path().join(".gitignore"), "target\n.scaffold\nother\n")
        .expect("seed .gitignore");

    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .success();

    let gitignore = fs::read_to_string(temp.path().join(".gitignore")).expect("read .gitignore");
    let scaffold_count = gitignore
        .lines()
        .filter(|l| l.trim() == ".scaffold")
        .count();
    assert_eq!(
        scaffold_count, 1,
        ".gitignore must contain .scaffold exactly once, got: {gitignore:?}"
    );
}

#[test]
fn init_hint_uses_invoked_bin_name() {
    let temp_lgs = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp_lgs.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("Run 'lgs setup'"));

    let temp_long = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp_long.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("Run 'logos-scaffold setup'"));
}

#[test]
fn completions_zsh_registers_both_names_in_pristine_shell() {
    if std::process::Command::new("zsh")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping: zsh not available");
        return;
    }

    let output = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .args(["completions", "zsh"])
        .output()
        .expect("run lgs completions zsh");
    assert!(output.status.success(), "expected success exit");

    let temp = tempdir().expect("tempdir");
    let fpath_dir = temp.path().join("fpath");
    fs::create_dir_all(&fpath_dir).expect("mkdir fpath");
    fs::write(fpath_dir.join("_lgs"), &output.stdout).expect("write _lgs");

    // Run a pristine zsh (-f skips rc files) with only our fpath plus
    // system completion functions, then verify both names are registered
    // at compinit time — not deferred to first tab.
    let script = format!(
        "fpath=({} /usr/share/zsh/*/functions); \
         autoload -Uz compinit && compinit -u -d {}/zcompdump; \
         print \"lgs=${{_comps[lgs]:-MISSING}}\"; \
         print \"logos-scaffold=${{_comps[logos-scaffold]:-MISSING}}\"",
        fpath_dir.display(),
        temp.path().display(),
    );

    let zsh_output = std::process::Command::new("zsh")
        .args(["-f", "-c", &script])
        .output()
        .expect("run pristine zsh");
    let stdout = String::from_utf8_lossy(&zsh_output.stdout);
    assert!(
        stdout.contains("lgs=_lgs"),
        "expected lgs to be registered, got: {stdout}"
    );
    assert!(
        stdout.contains("logos-scaffold=_lgs"),
        "expected logos-scaffold to be registered at compinit time, got: {stdout}"
    );
}

/// Pre-0.2.0 scaffold.toml with all the legacy signals (url field on
/// [repos.lez], [basecamp.modules.*], [basecamp].lgpm_flake/pin/source).
/// Every non-init command must hard-fail when this is present and the
/// migration hint must point at `init`.
const PRE_V0_2_0_SCAFFOLD_TOML: &str = r#"# preserved comment
[scaffold]
version = "0.1.0"
cache_root = "cache"

[repos.lez]
url = "https://example/lez.git"
source = "https://example/lez.git"
path = "lez"
pin = "deadbeef"

[wallet]
home_dir = ".scaffold/wallet"

[framework]
kind = "default"
version = "0.1.0"

[framework.idl]
spec = "lssa-idl/0.1.0"
path = "idl"

[localnet]
port = 3040
risc0_dev_mode = true

[basecamp]
pin = "deadbeef"
source = "https://example/basecamp"
lgpm_flake = "github:logos-co/logos-package-manager/cafef00dcafef00dcafef00dcafef00dcafef00d#cli"
port_base = 60000
port_stride = 10

[basecamp.modules.foo]
flake = "path:./foo"
role = "project"
"#;

fn assert_pre_v0_2_0_rejection(args: &[&str]) {
    assert_old_schema_rejection(PRE_V0_2_0_SCAFFOLD_TOML, args);
}

/// The LEZ 0.1/0.2-era (v0.2.0) schema with spel, circuits and IDL settings.
/// LEZ 0.3 scaffold rejects it everywhere except `init`.
const V0_2_0_SCAFFOLD_TOML: &str = r#"[scaffold]
version = "0.2.0"
cache_root = "cache"

[repos.lez]
source = "https://github.com/logos-blockchain/logos-execution-zone.git"
path = "lez"
pin = "cf3639d8252040d13b3d4e933feb19b42c76e14a"

[repos.spel]
source = "https://github.com/logos-co/spel.git"
path = "spel"
pin = "73fc462eb8f0a4d00f1a846437c627ec2e523f83"

[wallet]
home_dir = ".scaffold/wallet"

[framework]
kind = "default"
version = "0.1.0"

[framework.idl]
spec = "lssa-idl/0.1.0"
path = "idl"

[circuits]
version = "0.4.1"
"#;

fn assert_old_schema_rejection(fixture: &str, args: &[&str]) {
    let temp = tempdir().expect("tempdir");
    fs::write(temp.path().join("scaffold.toml"), fixture).expect("seed pre-v0.2.0 scaffold.toml");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run lgs {args:?}: {e}"));

    assert!(
        !output.status.success(),
        "lgs {args:?} must hard-fail on an old-schema scaffold.toml; got status {:?}, stdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("init"),
        "lgs {args:?} stderr must point at `init`; got:\n{stderr}",
    );

    let after = fs::read_to_string(temp.path().join("scaffold.toml"))
        .expect("scaffold.toml still readable");
    assert_eq!(
        after, fixture,
        "lgs {args:?} must not mutate scaffold.toml when it rejects an old schema",
    );
}

#[test]
fn setup_hard_fails_on_v0_2_0_scaffold_toml() {
    assert_old_schema_rejection(V0_2_0_SCAFFOLD_TOML, &["setup"]);
}

#[test]
fn deploy_hard_fails_on_v0_2_0_scaffold_toml() {
    assert_old_schema_rejection(V0_2_0_SCAFFOLD_TOML, &["deploy"]);
}

#[test]
fn doctor_hard_fails_on_v0_2_0_scaffold_toml() {
    assert_old_schema_rejection(V0_2_0_SCAFFOLD_TOML, &["doctor"]);
}

#[test]
fn setup_hard_fails_on_pre_v0_2_0_scaffold_toml() {
    assert_pre_v0_2_0_rejection(&["setup"]);
}

/// scaffold#259: a project that opts into the host-toolchain guest build gets
/// told, in doctor, that its `program_id` is not reproducible.
#[test]
fn doctor_reports_the_guest_build_strategy() {
    let temp = tempdir().expect("tempdir");
    let project = temp.path();
    fs::write(
        project.join("scaffold.toml"),
        format!("{MINIMAL_SCAFFOLD_TOML}\n[build]\nguest = \"local\"\n"),
    )
    .expect("write scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(project)
        .arg("doctor")
        .assert()
        .code(predicate::in_iter([0, 1]))
        .stdout(
            predicate::str::contains("guest build")
                .and(predicate::str::contains("not reproducible")),
        );
}

/// scaffold#259: deterministic (the default) guest builds must say so
/// in doctor, tag included — that tag is part of what makes `program_id`
/// reproducible. Asserted independently of whether this machine happens to
/// have `cargo-risczero`/`docker` installed, which only changes PASS vs FAIL.
#[test]
fn doctor_reports_the_pinned_risc0_docker_tag_in_deterministic_mode() {
    let temp = tempdir().expect("tempdir");
    let project = temp.path();
    fs::write(
        project.join("scaffold.toml"),
        format!(
            "{MINIMAL_SCAFFOLD_TOML}\n{}",
            "[build]\nguest = \"docker\"\nrisc0_docker_tag = \"r0.1.91.1\"\n"
        ),
    )
    .expect("write scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(project)
        .arg("doctor")
        .assert()
        .code(predicate::in_iter([0, 1]))
        .stdout(predicate::str::contains("risc0-guest-builder:r0.1.91.1"));
}

/// `--guest` is a closed set; a typo must fail at parse time rather than
/// silently falling back to the default.
#[test]
fn build_guest_flag_rejects_unknown_modes() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("build")
        .arg("--guest")
        .arg("podman")
        .assert()
        .failure()
        .stderr(predicate::str::contains("local").and(predicate::str::contains("docker")));
}

#[test]
fn build_idl_and_client_subcommands_are_gone() {
    // spel's IDL / client generation was removed with LEZ 0.3; `build`
    // has no subcommands left and its help must not advertise them.
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("build")
        .arg("--help")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("build idl")
                .not()
                .and(predicate::str::contains("build client").not())
                .and(predicate::str::contains("IDL").not()),
        );
}

#[test]
fn build_help_documents_the_guest_build_modes() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("build")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--guest").and(predicate::str::contains("program_id")));
}

/// F4: clap's leading `error: ` is stripped before we re-wrap with anyhow,
/// so the user sees a single `error:` prefix instead of `error: error: ...`.
#[test]
fn unrecognized_subcommand_produces_single_error_prefix() {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("bogus-subcommand")
        .output()
        .expect("run lgs");
    assert!(
        !output.status.success(),
        "unrecognized subcommand must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unrecognized subcommand"),
        "stderr must surface the unrecognized-subcommand message, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("error: error:"),
        "stderr must not double-wrap the error prefix, got:\n{stderr}"
    );
}

/// Regression: commands that go through `load_project()` (basecamp, wallet,
/// deploy, report) used to wrap any `load_project` failure with the
/// "This command must be run inside a logos-scaffold project — cd into …"
/// hint. That was a lie when the user *was* inside a project but the
/// schema was stale; the only fix is `lgs init`. The wrap is removed and
/// `load_project` itself produces the right message for each branch.
#[test]
fn load_project_commands_in_stale_schema_project_show_init_hint_not_cd_hint() {
    let temp = tempdir().expect("tempdir");
    fs::write(temp.path().join("scaffold.toml"), PRE_V0_2_0_SCAFFOLD_TOML)
        .expect("seed pre-v0.2.0 scaffold.toml");

    for args in [
        &["basecamp", "setup"][..],
        &["wallet", "list"][..],
        &["deploy"][..],
        &["report"][..],
    ] {
        let output = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
            .current_dir(temp.path())
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("run lgs {args:?}: {e}"));
        assert!(
            !output.status.success(),
            "lgs {args:?} must fail on pre-0.2.0 scaffold.toml"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("logos-scaffold init"),
            "lgs {args:?} stderr must point at `init`; got:\n{stderr}"
        );
        assert!(
            !stderr.contains("cd into your scaffolded project"),
            "lgs {args:?} stderr must not show the cd-into-project hint when \
             the user is already inside a project; got:\n{stderr}"
        );
    }
}

#[test]
fn build_hard_fails_on_pre_v0_2_0_scaffold_toml() {
    assert_pre_v0_2_0_rejection(&["build"]);
}

#[test]
fn deploy_hard_fails_on_pre_v0_2_0_scaffold_toml() {
    assert_pre_v0_2_0_rejection(&["deploy"]);
}

#[test]
fn doctor_hard_fails_on_pre_v0_2_0_scaffold_toml() {
    assert_pre_v0_2_0_rejection(&["doctor"]);
}

#[test]
fn report_hard_fails_on_pre_v0_2_0_scaffold_toml() {
    assert_pre_v0_2_0_rejection(&["report"]);
}

#[test]
fn basecamp_setup_hard_fails_on_pre_v0_2_0_scaffold_toml() {
    assert_pre_v0_2_0_rejection(&["basecamp", "setup"]);
}

#[test]
fn deploy_json_hard_fails_with_clean_stdout_on_pre_v0_2_0_scaffold_toml() {
    let temp = tempdir().expect("tempdir");
    fs::write(temp.path().join("scaffold.toml"), PRE_V0_2_0_SCAFFOLD_TOML)
        .expect("seed pre-v0.2.0 scaffold.toml");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .args(["deploy", "--json"])
        .output()
        .expect("run lgs deploy --json");

    assert!(!output.status.success(), "deploy --json must fail");
    assert!(
        output.stdout.is_empty(),
        "deploy --json must keep stdout clean on rejection; got: {}",
        String::from_utf8_lossy(&output.stdout),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("init"),
        "stderr must point at `init`; got:\n{stderr}"
    );
}

#[test]
fn doctor_json_hard_fails_with_clean_stdout_on_pre_v0_2_0_scaffold_toml() {
    let temp = tempdir().expect("tempdir");
    fs::write(temp.path().join("scaffold.toml"), PRE_V0_2_0_SCAFFOLD_TOML)
        .expect("seed pre-v0.2.0 scaffold.toml");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .args(["doctor", "--json"])
        .output()
        .expect("run lgs doctor --json");

    assert!(!output.status.success(), "doctor --json must fail");
    assert!(
        output.stdout.is_empty(),
        "doctor --json must keep stdout clean on rejection; got: {}",
        String::from_utf8_lossy(&output.stdout),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("init"),
        "stderr must point at `init`; got:\n{stderr}"
    );
}

// ─── run command tests ───────────────────────────────────────────────────────

#[test]
fn run_help_lists_command_summary() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("run")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Build, start localnet, top up wallet, deploy, and run post-deploy hooks (topup/deploy skippable)",
        ));
}

#[test]
fn run_outside_project_fails_with_project_scoped_message() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("run")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "This command must be run inside a logos-scaffold project",
        ));
}

#[test]
fn run_rejects_both_post_deploy_flags() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("run")
        .arg("--post-deploy")
        .arg("echo override")
        .arg("--no-post-deploy")
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn run_help_advertises_localnet_timeout_flag() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("run")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--localnet-timeout"))
        .stdout(predicate::str::contains("default: 120"));
}

#[test]
fn run_rejects_non_numeric_localnet_timeout() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("run")
        .arg("--localnet-timeout")
        .arg("abc")
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn run_fails_at_build_step_in_mock_project() {
    // The run command calls cmd_build_shortcut which runs cargo build --workspace.
    // In a mock project without a real Cargo workspace, this fails at step 1.
    // This tests that the pipeline starts and fails fast.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("run")
        .assert()
        .failure()
        .stdout(predicate::str::contains("[1/4] Building..."));
}

#[test]
fn run_with_post_deploy_hook_shows_5_steps_in_output() {
    // When a post_deploy hook is configured, the step counter shows /5 (build, localnet,
    // topup, deploy, hooks).
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    append_run_config(temp.path(), &["echo hello"]);

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("run")
        .assert()
        .failure() // still fails at build
        .stdout(predicate::str::contains("[1/5] Building..."));
}

#[test]
fn run_with_multiple_post_deploy_hooks_uses_array_form() {
    // Multiple hooks are configured as a TOML inline array; pipeline still has 5 steps.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    append_run_config(temp.path(), &["echo one", "echo two"]);

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("run")
        .assert()
        .failure() // still fails at build
        .stdout(predicate::str::contains("[1/5] Building..."));
}

#[test]
fn run_no_post_deploy_flag_skips_configured_hooks() {
    // --no-post-deploy must collapse total_steps back to 4 even when
    // scaffold.toml configures hooks. The build still fails first, but the
    // step counter in stdout proves the override was honored.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    append_run_config(temp.path(), &["echo configured"]);

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("run")
        .arg("--no-post-deploy")
        .assert()
        .failure()
        .stdout(predicate::str::contains("[1/4] Building..."));
}

#[test]
fn run_post_deploy_flag_overrides_configured_hooks() {
    // --post-deploy replaces config hooks. With one config hook ([1/5]) and
    // two flag overrides, the resulting step count stays at /5 — proving
    // that the flag took effect and config wasn't merged on top.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    append_run_config(temp.path(), &["echo configured"]);

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("run")
        .arg("--post-deploy")
        .arg("echo override-a")
        .arg("--post-deploy")
        .arg("echo override-b")
        .assert()
        .failure()
        .stdout(predicate::str::contains("[1/5] Building..."));
}

fn append_run_config(project_root: &Path, post_deploy: &[&str]) {
    append_run_config_full(project_root, false, post_deploy);
}

fn append_run_config_full(project_root: &Path, reset: bool, post_deploy: &[&str]) {
    let toml_path = project_root.join("scaffold.toml");
    let mut content = fs::read_to_string(&toml_path).expect("read scaffold.toml");
    let quoted: Vec<String> = post_deploy.iter().map(|c| format!("\"{c}\"")).collect();
    content.push_str(&format!(
        "\n[run]\nreset = {reset}\npost_deploy = [{}]\n",
        quoted.join(", ")
    ));
    fs::write(toml_path, content).expect("write scaffold.toml");
}

#[test]
fn run_rejects_both_reset_flags() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("run")
        .arg("--reset")
        .arg("--no-reset")
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn run_help_lists_reset_flags() {
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .arg("run")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--reset").and(predicate::str::contains("--no-reset")));
}

#[test]
fn run_with_reset_flag_starts_pipeline() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    // --reset on the CLI: no banner (user typed the word) and pipeline
    // enters step 1 before the build fails on this mock project.
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("run")
        .arg("--reset")
        .assert()
        .failure()
        .stdout(predicate::str::contains("[1/4] Building..."))
        .stderr(predicate::str::contains("scaffold.toml requested reset = true").not());
}

#[test]
fn run_with_reset_in_config_emits_destructive_intent_warning() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    append_run_config_full(temp.path(), true, &[]);

    // reset = true in scaffold.toml without --reset on the CLI: the
    // destructive-intent warning must fire on stderr before step 1, so a
    // user who didn't realize `lgs run` would wipe their state finds out
    // before the build runs.
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("run")
        .assert()
        .failure()
        .stdout(predicate::str::contains("[1/4] Building..."))
        .stderr(predicate::str::contains(
            "scaffold.toml requested reset = true",
        ));
}

#[test]
fn run_no_reset_flag_overrides_config_reset_true() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    append_run_config_full(temp.path(), true, &[]);

    // --no-reset must beat the config field (CLI overrides config). When
    // effective_reset is false, the destructive-intent banner must NOT
    // fire — that's the observable signal proving the override took effect.
    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("run")
        .arg("--no-reset")
        .assert()
        .failure()
        .stdout(predicate::str::contains("[1/4] Building..."))
        .stderr(predicate::str::contains("scaffold.toml requested reset = true").not());
}

#[test]
fn basecamp_paths_json_resolves_custom_profile_manifest() {
    // `basecamp paths` is pure path resolution: it needs only a loadable
    // project (no setup, no nix) and accepts any profile name.
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_scaffold_toml(temp.path(), &lez_path);

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("basecamp")
        .arg("paths")
        .arg("carol")
        .arg("--json")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("\"profile\": \"carol\"")
                .and(predicate::str::contains("\"modules_dir\""))
                .and(predicate::str::contains(
                    ".scaffold/basecamp/profiles/carol",
                )),
        );
}

/// Basecamp keeps two more trees under its base directory —
/// `module_data/` (per-module persisted state) and `logs/` (its own rotated
/// session logs) — and both sit inside the tree `launch` scrubs, so a relaunch
/// discards them. `paths` is where that becomes visible: without these fields a
/// developer hunting for module state or an app log has no way to learn either
/// where it lives or that it will not survive the next launch.
#[test]
fn basecamp_paths_json_lists_the_base_dir_children() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    write_scaffold_toml(temp.path(), &lez_path);

    let out = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("basecamp")
        .arg("paths")
        .arg("alice")
        .arg("--json")
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("utf8");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");

    let module_root = parsed["module_root"].as_str().expect("module_root");
    for (field, child) in [
        ("modules_dir", "modules"),
        ("plugins_dir", "plugins"),
        ("module_data_dir", "module_data"),
        ("app_logs_dir", "logs"),
    ] {
        let value = parsed[field]
            .as_str()
            .unwrap_or_else(|| panic!("{field} must be present"));
        assert_eq!(
            value,
            format!("{module_root}/{child}"),
            "{field} must be the `{child}` child of the module root"
        );
    }
    // The module root is basecamp's own base directory, so everything under it
    // is inside the scrubbed profile tree rather than a user-global location.
    assert!(
        module_root.contains(".scaffold/basecamp/profiles/alice/xdg-data"),
        "module root must sit inside the profile's scrubbed tree, got: {module_root}"
    );
}
