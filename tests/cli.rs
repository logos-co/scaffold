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
version = "0.2.0"
cache_root = "cache"

[repos.lez]
source = "https://example/lez.git"
path = "lez"
pin = "deadbeef"

[repos.spel]
source = "https://example/spel.git"
path = "spel"
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
"#;

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
            "This command must be run inside a logos-scaffold project.",
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

#[test]
fn localnet_start_fails_when_process_exits_before_ready() {
    let temp = tempdir().expect("tempdir");
    let lez_path = temp.path().join("lez");
    let sequencer_bin = lez_path.join("target/release/sequencer_service");
    let config_path = lez_path.join("sequencer/service/configs/debug/sequencer_config.json");
    fs::create_dir_all(sequencer_bin.parent().expect("parent")).expect("create dirs");
    fs::create_dir_all(config_path.parent().expect("parent")).expect("create config dir");
    fs::write(&config_path, r#"{"port": 3040}"#).expect("write sequencer config");
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

    write_scaffold_toml(temp.path(), &lez_path);

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
                .or(predicate::str::contains("localnet start timed out after"))
                .or(predicate::str::contains(
                    "cannot start localnet: port 3040 is already in use",
                )),
        );

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
    let config_path = lez_path.join("sequencer/service/configs/debug/sequencer_config.json");
    let args_log = temp.path().join("sequencer-args.log");
    let env_log = temp.path().join("sequencer-env.log");
    let localnet_port = unused_local_port();

    fs::create_dir_all(sequencer_bin.parent().expect("parent")).expect("create dirs");
    fs::create_dir_all(config_path.parent().expect("parent")).expect("create config dir");
    fs::write(&config_path, r#"{"port": 3040}"#).expect("write sequencer config");

    // Fake sequencer: reads port from sequencer_config.json (like the real one),
    // logs args and env for assertions.
    fs::write(
        &sequencer_bin,
        format!(
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s' \"${{RISC0_DEV_MODE:-}}\" > '{}'\nport=$(python3 -c \"import json,sys; print(json.load(open(sys.argv[1]))['port'])\" \"$1\")\nexec python3 -m http.server \"$port\" --bind 127.0.0.1\n",
            args_log.display(),
            env_log.display(),
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

    // Verify sequencer_config.json was patched with the configured port
    let patched_config = fs::read_to_string(&config_path).expect("read patched config");
    let config_json: serde_json::Value =
        serde_json::from_str(&patched_config).expect("parse patched config");
    assert_eq!(
        config_json["port"],
        serde_json::Value::Number(localnet_port.into()),
        "expected port in sequencer_config.json to be patched to {localnet_port}, got: {patched_config}"
    );

    // Verify --port was NOT passed as a CLI arg
    let args = fs::read_to_string(&args_log).expect("read args log");
    assert!(
        !args.contains("--port"),
        "expected --port NOT to appear in sequencer args, got: {args}"
    );

    let env = fs::read_to_string(&env_log).expect("read env log");
    assert_eq!(env, "0", "expected risc0 dev mode override to be passed");
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
                .and(predicate::str::contains("Preconfigured Public/")),
        );
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
                .and(predicate::str::contains("Preconfigured Public/")),
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
fn wallet_topup_dry_run_renders_pinata_claim_command() {
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
                .and(predicate::str::contains(
                    "planned preflight: check destination wallet initialization",
                ))
                .and(predicate::str::contains("auth-transfer init --account-id"))
                .and(predicate::str::contains("pinata claim --to"))
                .and(predicate::str::contains(
                    "planned method: pinata faucet claim",
                )),
        );
}

#[test]
fn wallet_topup_runs_pinata_claim_with_explicit_address() {
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
            predicate::str::contains("pinata claim --to Public/")
                .and(predicate::str::contains("wallet topup complete")),
        );
}

#[test]
fn wallet_topup_initializes_when_account_uninitialized_before_pinata() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("TOPUP_ACCOUNT_STATE", "uninitialized")
        .env("TOPUP_GUARD_REQUIRE_INIT", "1")
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .success()
        .stdout(predicate::str::contains("wallet topup complete"));

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let init_pos = stdout
        .find("auth-transfer init --account-id Public/")
        .expect("init command should be present");
    let pinata_pos = stdout
        .find("pinata claim --to Public/")
        .expect("pinata command should be present");
    assert!(
        init_pos < pinata_pos,
        "auth-transfer init must run before pinata claim, got output:\n{stdout}"
    );
}

#[test]
fn wallet_topup_skips_init_when_account_already_initialized() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("TOPUP_ACCOUNT_STATE", "initialized")
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        !stdout.contains("auth-transfer init --account-id"),
        "init command must not run for initialized accounts, got output:\n{stdout}"
    );
    assert!(
        stdout.contains("pinata claim --to Public/"),
        "pinata claim should still run, got output:\n{stdout}"
    );
}

#[test]
fn wallet_topup_preflight_failure_blocks_pinata() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("TOPUP_PREFLIGHT_FAIL", "1")
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "wallet topup failed while checking account initialization",
        ));

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        !stdout.contains("pinata claim --to"),
        "pinata must not run when preflight fails, got output:\n{stdout}"
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
        .stdout(predicate::str::contains("pinata claim --to Public/"));
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
fn wallet_topup_init_connectivity_failure_shows_sequencer_hint() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("TOPUP_ACCOUNT_STATE", "uninitialized")
        .env("TOPUP_INIT_FAIL_CONNECT", "1")
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

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        !stdout.contains("pinata claim --to"),
        "pinata must not run when init fails with connectivity error, got output:\n{stdout}"
    );
}

#[test]
fn wallet_topup_continues_when_init_reports_already_initialized() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    let assert = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("TOPUP_ACCOUNT_STATE", "uninitialized")
        .env("TOPUP_INIT_FAIL_ALREADY", "1")
        .arg("wallet")
        .arg("topup")
        .arg("--address")
        .arg(VALID_PUBLIC_ADDRESS)
        .assert()
        .success()
        .stdout(
            predicate::str::contains("destination already initialized; continuing")
                .and(predicate::str::contains("wallet topup complete")),
        );

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(
        stdout.contains("pinata claim --to Public/"),
        "pinata should run after tolerated init race, got output:\n{stdout}"
    );
}

#[test]
fn wallet_topup_timeout_is_reported_as_non_fatal() {
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
        .success()
        .stdout(predicate::str::contains(
            "wallet topup submitted, but confirmation timed out",
        ));
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
            "This command must be run inside a logos-scaffold project.",
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
    setup_wallet_project(temp.path(), None);
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
fn deploy_single_program_submits_successfully() {
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
        .success()
        .stdout(
            predicate::str::contains("OK  hello submitted")
                .and(predicate::str::contains(
                    "Submission confirmed by wallet exit status",
                ))
                .and(predicate::str::contains("Succeeded: 1"))
                .and(predicate::str::contains("Failed: 0"))
                .and(predicate::str::contains("reachability probe failed").not()),
        );
}

#[test]
fn deploy_uses_password_env_override() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_wallet_project(temp.path(), Some(&rpc.url));
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
        .stdout(predicate::str::contains("OK  hello submitted"));
}

#[test]
fn deploy_missing_binary_shows_build_hint() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_wallet_project(temp.path(), Some(&rpc.url));
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
    setup_wallet_project(temp.path(), Some(&rpc.url));
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
            predicate::str::contains("OK  alpha submitted")
                .and(predicate::str::contains("FAIL beta deployment failed"))
                .and(predicate::str::contains("Succeeded: 1"))
                .and(predicate::str::contains("Failed: 1")),
        );
}

#[test]
fn deploy_shows_hint_when_sequencer_is_unreachable_with_configured_addr() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:65535"));
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
    // This test assumes fallback `http://127.0.0.1:3040` is unreachable.
    // Skip in environments where another process is already listening there.
    if TcpStream::connect("127.0.0.1:3040").is_ok() {
        return;
    }

    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), None);
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
fn deploy_prints_program_id_from_vendored_spel() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_wallet_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "hello");
    write_guest_binary(temp.path(), "hello");
    let expected = expected_stub_program_id("hello.bin");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("hello")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("OK  hello submitted").and(predicate::str::contains(format!(
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
    setup_wallet_project(temp.path(), Some(&rpc.url));
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
    setup_wallet_project(temp.path(), Some(&rpc.url));
    let custom = temp.path().join("custom.bin");
    fs::write(&custom, b"stub-program-bin").expect("write custom bin");
    let expected = expected_stub_program_id("custom.bin");

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
fn deploy_program_id_unavailable_when_spel_missing() {
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_wallet_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "hello");
    write_guest_binary(temp.path(), "hello");
    // Remove the spel stub the helper placed; deploy should still succeed
    // and print the "unavailable" hint instead of a hex.
    fs::remove_file(temp.path().join("spel/target/release/spel")).expect("remove spel stub");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("deploy")
        .arg("hello")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("OK  hello submitted")
                .and(predicate::str::contains("program_id: unavailable"))
                .and(predicate::str::contains("logos-scaffold setup")),
        );
}

#[test]
fn deploy_json_output_is_pure_json_no_command_echo() {
    // --json must produce a single JSON line on stdout; the wallet
    // subprocess command-echo (`$ .../wallet deploy-program …`) belongs
    // off-channel so consumers can pipe directly to `jq`.
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_wallet_project(temp.path(), Some(&rpc.url));
    let custom = temp.path().join("custom.bin");
    fs::write(&custom, b"stub-program-bin").expect("write custom bin");

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
fn deploy_multi_program_json_emits_deploys_array() {
    // Auto-discovery deploy with --json must produce one JSON line of the
    // shape `{"deploys":[{...},{...}]}` — pure JSON, no command echoes,
    // no human-readable text. One entry per program with program_id
    // populated when extraction succeeds.
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_wallet_project(temp.path(), Some(&rpc.url));
    write_guest_program(temp.path(), "alpha");
    write_guest_program(temp.path(), "beta");
    write_guest_binary(temp.path(), "alpha");
    write_guest_binary(temp.path(), "beta");
    let alpha_id = expected_stub_program_id("alpha.bin");
    let beta_id = expected_stub_program_id("beta.bin");

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
        trimmed.matches("\"status\":\"submitted\"").count(),
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
    setup_wallet_project(temp.path(), Some(&rpc.url));
    let custom = temp.path().join("custom.bin");
    fs::write(&custom, b"stub-program-bin").expect("write custom bin");

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
        Some("submitted"),
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
    setup_wallet_project(temp.path(), Some(&rpc.url));
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
    setup_wallet_project(temp.path(), Some(&rpc.url));
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
        trimmed.contains("\"status\":\"submitted\"") && trimmed.contains("\"program\":\"alpha\""),
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
    // Wallet stub today emits a `tx_hash=…` line. To prove the JSON shape
    // omits `tx` when no value is available, set WALLET_NO_TX=1 — the stub
    // honors that and skips the tx line. Result: presence of the `tx` key
    // should now imply a real value (not `null`).
    let temp = tempdir().expect("tempdir");
    let rpc = RpcStub::start();
    setup_wallet_project(temp.path(), Some(&rpc.url));
    let custom = temp.path().join("custom.bin");
    fs::write(&custom, b"stub-program-bin").expect("write custom bin");

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
fn spel_proxy_forwards_args_to_vendored_binary() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("spel")
        .arg("--")
        .arg("inspect")
        .arg("methods/guest/foo.bin")
        .assert()
        .success()
        .stdout(predicate::str::contains("ImageID (hex bytes):"));
}

#[test]
fn spel_proxy_forwards_nonzero_exit_code() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .env("SPEL_FAIL", "1")
        .arg("spel")
        .arg("--")
        .arg("inspect")
        .arg("foo.bin")
        .assert()
        .failure();
}

#[test]
fn spel_proxy_hints_when_binary_missing() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    fs::remove_file(temp.path().join("spel/target/release/spel")).expect("remove spel stub");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("spel")
        .arg("--")
        .arg("inspect")
        .arg("foo.bin")
        .assert()
        .failure()
        .stderr(predicate::str::contains("logos-scaffold setup"));
}

#[test]
fn spel_proxy_requires_args_after_dash_dash() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("spel")
        .arg("--")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "spel passthrough requires at least one argument",
        ));
}

#[test]
fn spel_without_dash_dash_suggests_passthrough_form() {
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("spel")
        .arg("inspect")
        .arg("foo.bin")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("Did you mean")
                .and(predicate::str::contains("logos-scaffold spel -- inspect")),
        );
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

#[cfg(unix)]
#[test]
fn self_test_run_logged_success_shape() {
    // Hidden `self-test run-logged` hook drives `run_logged` against a
    // trivial subprocess (`/bin/true`). We assert the visible output shape
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
        stdout.contains("running: ") && stdout.contains("/bin/true"),
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
            "This command must be run inside a logos-scaffold project.",
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
            "This command must be run inside a logos-scaffold project.",
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
            "This command must be run inside a logos-scaffold project.",
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
fn basecamp_launch_rejects_unknown_profile() {
    let temp = tempdir().expect("tempdir");
    let project = temp.path();
    fs::write(project.join("scaffold.toml"), MINIMAL_SCAFFOLD_TOML).expect("write scaffold.toml");

    // Fake a completed setup so we get past the first gate and reach profile validation.
    // Paths are /bin/true / /bin/echo — they exist on Linux, and launch never actually
    // reaches `exec` because the profile check fails first.
    let state_dir = project.join(".scaffold/state");
    fs::create_dir_all(&state_dir).expect("mkdir state");
    fs::write(
        state_dir.join("basecamp.state"),
        "pin=deadbeef\nbasecamp_bin=/bin/true\nlgpm_bin=/bin/echo\n",
    )
    .expect("write state");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(project)
        .arg("basecamp")
        .arg("launch")
        .arg("charlie")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown profile `charlie`"));
}

#[cfg(unix)]
#[test]
fn basecamp_launch_bails_when_no_modules_captured() {
    // launch without --no-clean scrubs and replays the captured module set.
    // If [basecamp.modules] is empty, the replay is silently a no-op — the
    // profile comes up with zero modules installed, which violates the
    // clean-slate guarantee. Surface as an error with a concrete hint.
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
    fs::write(
        state_dir.join("basecamp.state"),
        "pin=deadbeef\nbasecamp_bin=/bin/true\nlgpm_bin=/bin/echo\n",
    )
    .expect("write state");
    fs::create_dir_all(project.join(".scaffold/basecamp/profiles/alice")).expect("mkdir profile");

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(project)
        .arg("basecamp")
        .arg("launch")
        .arg("alice")
        .assert()
        .failure()
        .stderr(predicate::str::contains("basecamp modules"))
        .stderr(predicate::str::contains("--no-clean"));
}

#[cfg(unix)]
#[test]
fn basecamp_launch_no_clean_bypasses_empty_modules_check() {
    // --no-clean is the documented escape hatch for keeping whatever's already
    // installed in the profile. It must skip the empty-modules check entirely.
    // We don't run launch to completion (basecamp_bin is /bin/true), but the
    // command should get past the modules check and fail later — not fail on
    // an "install modules first" hint.
    let temp = tempdir().expect("tempdir");
    let project = temp.path();
    fs::write(
        project.join("scaffold.toml"),
        r#"[scaffold]
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
lgpm_flake = ""
port_base = 60000
port_stride = 10
"#,
    )
    .expect("write scaffold.toml");
    let state_dir = project.join(".scaffold/state");
    fs::create_dir_all(&state_dir).expect("mkdir state");
    fs::write(
        state_dir.join("basecamp.state"),
        "pin=deadbeef\nbasecamp_bin=/bin/true\nlgpm_bin=/bin/echo\n",
    )
    .expect("write state");
    fs::create_dir_all(project.join(".scaffold/basecamp/profiles/alice")).expect("mkdir profile");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(project)
        .arg("basecamp")
        .arg("launch")
        .arg("alice")
        .arg("--no-clean")
        .output()
        .expect("run launch");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("basecamp modules") || !stderr.contains("--no-clean"),
        "--no-clean should not trigger the empty-modules hint, got stderr:\n{stderr}"
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
            "This command must be run inside a logos-scaffold project.",
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

fn write_scaffold_toml_with_localnet(
    project_root: &Path,
    lez_path: &Path,
    localnet_port: Option<u16>,
    risc0_dev_mode: Option<bool>,
) {
    let spel_path = project_root.join("spel");
    // Schema 0.2.0: no url, no [basecamp] / [basecamp.modules.*],
    // path is set on lez/spel for back-compat (tests need a literal path
    // to a local stub repo).
    let mut content = format!(
        "[scaffold]\nversion = \"0.2.0\"\ncache_root = \"{}\"\n\n[repos.lez]\nsource = \"https://github.com/logos-blockchain/logos-execution-zone.git\"\npin = \"{}\"\npath = \"{}\"\n\n[repos.spel]\nsource = \"https://github.com/logos-co/spel.git\"\npin = \"{}\"\npath = \"{}\"\n\n[wallet]\nhome_dir = \".scaffold/wallet\"\n",
        project_root.join("cache").display(),
        TEST_PIN,
        lez_path.display(),
        TEST_PIN,
        spel_path.display(),
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
    write_spel_stub(&project_root.join("spel"));
    write_scaffold_toml(project_root, &lez_path);
    write_wallet_config(project_root, sequencer_addr);
}

/// Place a minimal `spel` stub at `<spel_path>/target/release/spel`. It
/// emits the canonical `   ImageID (hex bytes): <hex>` line that
/// `extract_program_id` parses, with a deterministic-per-binary hash so
/// tests can assert exact values. Honors:
///   `SPEL_FAIL=1`              → exit non-zero (proxy exit-code test)
///   `SPEL_PROGRAM_ID_FAIL=<n>` → exit non-zero only when arg2 basename
///                                contains `<n>` (program-id-unavailable
///                                test for one program out of several).
fn write_spel_stub(spel_path: &Path) {
    let bin = spel_path.join("target/release/spel");
    fs::create_dir_all(bin.parent().expect("parent")).expect("mkdir spel target");
    let script = r#"#!/bin/sh
set -eu

if [ "${SPEL_FAIL:-0}" = "1" ]; then
  echo "spel stub: forced failure" >&2
  exit 7
fi

if [ "$#" -ge 2 ] && [ "$1" = "inspect" ]; then
  bin_path="$2"
  bin_name="$(basename "$bin_path")"
  if [ -n "${SPEL_PROGRAM_ID_FAIL:-}" ]; then
    case "$bin_name" in
      *"$SPEL_PROGRAM_ID_FAIL"*)
        echo "spel stub: forced inspect failure for $bin_name" >&2
        exit 8
        ;;
    esac
  fi
  # Build a deterministic 64-char hex ID directly from the basename: emit
  # each byte as two lowercase hex digits, repeat the resulting string until
  # it is at least 64 chars, then truncate. No CRC or hash table — both the
  # stub and `expected_stub_program_id` in Rust use the same trivial formula.
  hex="$(printf '%s' "$bin_name" | od -An -vtx1 | tr -d ' \n')"
  while [ "${#hex}" -lt 64 ]; do
    hex="$hex$hex"
  done
  full="$(printf '%s' "$hex" | cut -c1-64)"
  printf '   ImageID (hex bytes): %s\n' "$full"
  exit 0
fi

# Echo other invocations so the proxy test can verify args reach the binary.
echo "spel stub invoked: $*"
exit 0
"#;
    fs::write(&bin, script).expect("write spel stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&bin).expect("stub metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bin, perms).expect("chmod spel stub");
    }
}

/// Recompute the program ID the spel stub emits for a given binary basename.
/// Mirrors the trivial hex-of-basename formula in `write_spel_stub` (`od -tx1`
/// then repeat to 64 chars) so tests can assert exact hex without invoking
/// the stub themselves. No hash table, no polynomial — if the formula ever
/// drifts, both sides of the equation are right next to each other.
fn expected_stub_program_id(bin_name: &str) -> String {
    let mut hex: String = bin_name.bytes().map(|b| format!("{b:02x}")).collect();
    while hex.len() < 64 {
        hex = format!("{hex}{hex}");
    }
    hex.truncate(64);
    hex
}

fn write_wallet_config(project_root: &Path, sequencer_addr: Option<&str>) {
    let wallet_home = project_root.join(".scaffold/wallet");
    fs::create_dir_all(&wallet_home).expect("create wallet home");
    let path = wallet_home.join("wallet_config.json");

    let mut value = serde_json::json!({
        "initial_accounts": [
            { "Public": { "account_id": VALID_ACCOUNT_ID } }
        ]
    });
    if let Some(addr) = sequencer_addr {
        value["sequencer_addr"] = serde_json::Value::String(addr.to_string());
    }

    let text = serde_json::to_string_pretty(&value).expect("wallet config json");
    fs::write(path, text).expect("write wallet config");
}

fn write_wallet_stub(lez_path: &Path) {
    let path = lez_path.join("target/release/wallet");
    fs::create_dir_all(path.parent().expect("parent")).expect("create wallet binary dir");
    let script = r#"#!/bin/sh
set -eu

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

if [ "$#" -ge 2 ] && [ "$1" = "account" ] && [ "$2" = "list" ]; then
  echo "Preconfigured Public/6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV"
  echo "/ Public/8zxWNm1qh6FLsJpVBuDxdxcTm55qHPgFEdqJpPVu1fuy"
  exit 0
fi

if [ "$#" -ge 2 ] && [ "$1" = "account" ] && [ "$2" = "get" ]; then
  if [ "${TOPUP_PREFLIGHT_FAIL:-0}" = "1" ]; then
    echo "simulated account get failure" >&2
    exit 4
  fi
  if [ "${TOPUP_ACCOUNT_STATE:-initialized}" = "uninitialized" ]; then
    echo "Account is Uninitialized"
  else
    echo "Account state: Initialized"
  fi
  exit 0
fi

if [ "$#" -ge 3 ] && [ "$1" = "auth-transfer" ] && [ "$2" = "init" ] && [ "$3" = "--account-id" ]; then
  require_password_if_configured
  if [ "${TOPUP_INIT_FAIL_CONNECT:-0}" = "1" ]; then
    echo "connection refused" >&2
    exit 1
  fi
  if [ "${TOPUP_INIT_FAIL_ALREADY:-0}" = "1" ]; then
    echo "Error: Account must be uninitialized" >&2
    exit 1
  fi
  marker_path="${NSSA_WALLET_HOME_DIR:-.}/.topup-init-ran"
  : > "$marker_path"
  echo "init ok"
  exit 0
fi

if [ "$#" -ge 2 ] && [ "$1" = "pinata" ] && [ "$2" = "claim" ]; then
  require_password_if_configured
  if [ "${TOPUP_GUARD_REQUIRE_INIT:-0}" = "1" ]; then
    marker_path="${NSSA_WALLET_HOME_DIR:-.}/.topup-init-ran"
    if [ ! -f "$marker_path" ]; then
      echo "pinata called before init" >&2
      exit 9
    fi
  fi
  if [ "${TOPUP_FAIL_CONNECT:-0}" = "1" ]; then
    echo "connection refused" >&2
    exit 1
  fi
  if [ "${TOPUP_FAIL_TIMEOUT:-0}" = "1" ]; then
    echo "Error: Transaction not found in preconfigured amount of blocks" >&2
    exit 1
  fi
  echo "tx_hash=pinata-topup-hash"
  exit 0
fi

if [ "$#" -ge 2 ] && [ "$1" = "deploy-program" ]; then
  require_password_if_configured
  bin_path="$2"
  bin_name="$(basename "$bin_path")"
  if [ "${FAIL_PROGRAM:-}" = "$bin_name" ]; then
    echo "simulated deploy failure for $bin_name" >&2
    exit 2
  fi
  # WALLET_NO_TX=1: simulate the wallet not surfacing a tx identifier so
  # tests can assert the deploy JSON omits the `tx` key when None.
  if [ "${WALLET_NO_TX:-0}" = "1" ]; then
    exit 0
  fi
  echo "tx_hash=deploy-$bin_name"
  exit 0
fi

if [ "$#" -ge 1 ] && [ "$1" = "--version" ]; then
  echo "wallet stub 0.1.0"
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

fn write_guest_program(project_root: &Path, name: &str) {
    let dir = project_root.join("methods/guest/src/bin");
    fs::create_dir_all(&dir).expect("create guest program dir");
    fs::write(dir.join(format!("{name}.rs")), "fn main() {}\n").expect("write guest source");
}

fn write_guest_binary(project_root: &Path, name: &str) {
    let dir = project_root.join(GUEST_BIN_REL_PATH);
    fs::create_dir_all(&dir).expect("create guest binary dir");
    fs::write(dir.join(format!("{name}.bin")), b"stub-program-bin").expect("write guest binary");
}

struct RpcStub {
    url: String,
    stop: Arc<AtomicBool>,
    addr: String,
    handle: Option<thread::JoinHandle<()>>,
}

impl RpcStub {
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
            while !stop_flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        respond_last_block(&mut stream);
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

fn respond_last_block(stream: &mut TcpStream) {
    let mut buf = [0_u8; 4096];
    let _ = stream.read(&mut buf);

    let body = r#"{"jsonrpc":"2.0","result":123,"id":1}"#;
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
fn init_refuses_already_at_v0_2_0_scaffold_toml() {
    let temp = tempdir().expect("tempdir");
    let scaffold_path = temp.path().join("scaffold.toml");
    // First, run init to lay down a fresh v0.2.0 file.
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .success();
    let original = fs::read_to_string(&scaffold_path).expect("read scaffold.toml");

    // Second invocation must refuse with the "already migrated" hint.
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("already at schema"));

    let after = fs::read_to_string(&scaffold_path).expect("read scaffold.toml");
    assert_eq!(
        after, original,
        "init must not overwrite an already-migrated scaffold.toml"
    );
}

#[test]
fn init_migrates_pre_v0_2_0_scaffold_toml_in_place() {
    let temp = tempdir().expect("tempdir");
    let scaffold_path = temp.path().join("scaffold.toml");
    let original = "# user comment\n[scaffold]\nversion = \"0.1.0\"\n\n[repos.lez]\nurl = \"u\"\nsource = \"u\"\npath = \"p\"\npin = \"abc\"\n";
    fs::write(&scaffold_path, original).expect("seed scaffold.toml");

    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("migrated to schema v0.2.0"));

    let after = fs::read_to_string(&scaffold_path).expect("read scaffold.toml");
    assert!(
        after.contains("# user comment"),
        "user comments must be preserved; got:\n{after}"
    );
    assert!(
        after.contains("version = \"0.2.0\""),
        "[scaffold].version must be bumped; got:\n{after}"
    );
    assert!(
        after.contains("[repos.spel]"),
        "[repos.spel] must be appended; got:\n{after}"
    );
    assert!(
        !after.contains("url ="),
        "url field must be stripped; got:\n{after}"
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

    // Re-running any non-init command must now succeed at the parse step
    // (we only check parsing here; downstream commands will fail for other
    // reasons in a stub setup).
    Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("already at schema"));
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
    let temp = tempdir().expect("tempdir");
    fs::write(temp.path().join("scaffold.toml"), PRE_V0_2_0_SCAFFOLD_TOML)
        .expect("seed pre-v0.2.0 scaffold.toml");

    let output = Command::new(assert_cmd::cargo::cargo_bin!("lgs"))
        .current_dir(temp.path())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run lgs {args:?}: {e}"));

    assert!(
        !output.status.success(),
        "lgs {args:?} must hard-fail on pre-0.2.0 scaffold.toml; got status {:?}, stdout:\n{}\nstderr:\n{}",
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
        after, PRE_V0_2_0_SCAFFOLD_TOML,
        "lgs {args:?} must not mutate scaffold.toml when it rejects pre-0.2.0",
    );
}

#[test]
fn setup_hard_fails_on_pre_v0_2_0_scaffold_toml() {
    assert_pre_v0_2_0_rejection(&["setup"]);
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
            "Build, start localnet, top up wallet, deploy, and run post-deploy hook",
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
        .stderr(predicate::str::contains("Not a logos-scaffold project"));
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
        .stdout(predicate::str::contains("[1/5] Building..."));
}

#[test]
fn run_with_post_deploy_hook_shows_6_steps_in_output() {
    // When a post_deploy hook is configured, the step counter shows /6.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    append_run_config(temp.path(), &["echo hello"]);

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("run")
        .assert()
        .failure() // still fails at build
        .stdout(predicate::str::contains("[1/6] Building..."));
}

#[test]
fn run_with_multiple_post_deploy_hooks_uses_array_form() {
    // Multiple hooks are configured as a TOML inline array; pipeline still has 6 steps.
    let temp = tempdir().expect("tempdir");
    setup_wallet_project(temp.path(), Some("http://127.0.0.1:3040"));
    append_run_config(temp.path(), &["echo one", "echo two"]);

    Command::new(assert_cmd::cargo::cargo_bin!("logos-scaffold"))
        .current_dir(temp.path())
        .arg("run")
        .assert()
        .failure() // still fails at build
        .stdout(predicate::str::contains("[1/6] Building..."));
}

#[test]
fn run_no_post_deploy_flag_skips_configured_hooks() {
    // --no-post-deploy must collapse total_steps back to 5 even when
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
        .stdout(predicate::str::contains("[1/5] Building..."));
}

#[test]
fn run_post_deploy_flag_overrides_configured_hooks() {
    // --post-deploy replaces config hooks. With one config hook ([1/6]) and
    // two flag overrides, the resulting step count stays at /6 — proving
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
        .stdout(predicate::str::contains("[1/6] Building..."));
}

fn append_run_config(project_root: &Path, post_deploy: &[&str]) {
    let toml_path = project_root.join("scaffold.toml");
    let mut content = fs::read_to_string(&toml_path).expect("read scaffold.toml");
    let quoted: Vec<String> = post_deploy.iter().map(|c| format!("\"{c}\"")).collect();
    content.push_str(&format!("\n[run]\npost_deploy = [{}]\n", quoted.join(", ")));
    fs::write(toml_path, content).expect("write scaffold.toml");
}
