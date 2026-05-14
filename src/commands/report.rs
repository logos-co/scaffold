use std::collections::VecDeque;
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::Serialize;
use serde_json::Value;

use super::doctor::build_doctor_report;
use super::localnet::build_localnet_status_for_project;
use crate::model::{
    CollectedItem, RedactionSummary, ReportManifest, SkippedItem, ToolCommandResult,
};
use crate::process::{set_command_echo, which};
use crate::project::{load_project, resolve_repo_path};
use crate::state::write_text;
use crate::DynResult;

const REPORT_WARNING: &str = "WARNING: This diagnostics bundle is sanitized on a best-effort basis and may still contain sensitive data. Inspect every file before sharing it publicly.";

pub(crate) fn cmd_report(out: Option<PathBuf>, tail: usize) -> DynResult<()> {
    let project = load_project().context(
        "This command must be run inside a logos-scaffold project.\nNext step: cd into your scaffolded project directory and retry.",
    )?;

    let now = unix_timestamp_now()?;
    let output_path = resolve_output_path(&project.root, out, now)?;

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let reports_dir = project.root.join(".scaffold/reports");
    fs::create_dir_all(&reports_dir)?;

    let staging_dir = reports_dir.join(format!(".tmp-report-{now}-{}", std::process::id()));
    fs::create_dir_all(&staging_dir)?;

    let collection = collect_report_artifacts(&project, tail, now, &staging_dir, &output_path);

    let manifest = match collection {
        Ok(manifest) => manifest,
        Err(err) => {
            bail!(
                "report generation failed: {err}\nstaging directory retained at {}",
                staging_dir.display()
            )
        }
    };

    if let Err(err) = pack_staging_dir(&staging_dir, &output_path) {
        bail!(
            "failed to create archive at {}: {err}\nstaging directory retained at {}",
            output_path.display(),
            staging_dir.display()
        );
    }

    if let Err(err) = fs::remove_dir_all(&staging_dir) {
        println!(
            "warning: failed to remove temporary staging directory {}: {err}",
            staging_dir.display()
        );
    }

    println!("report complete");
    println!("  archive: {}", output_path.display());
    println!("  included items: {}", manifest.include_count);
    println!("  skipped items: {}", manifest.skip_count);
    println!("{REPORT_WARNING}");

    Ok(())
}

fn resolve_home_dir_for_scrubbing() -> Option<PathBuf> {
    resolve_home_dir_from_env_like(|key| env::var(key).ok()).map(PathBuf::from)
}

fn resolve_home_dir_from_env_like<F>(mut get: F) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    if let Some(home) = non_empty_env_value(get("HOME")) {
        return Some(home);
    }
    if let Some(user_profile) = non_empty_env_value(get("USERPROFILE")) {
        return Some(user_profile);
    }

    match (
        non_empty_env_value(get("HOMEDRIVE")),
        non_empty_env_value(get("HOMEPATH")),
    ) {
        (Some(home_drive), Some(home_path)) => Some(format!("{home_drive}{home_path}")),
        _ => None,
    }
}

fn non_empty_env_value(raw: Option<String>) -> Option<String> {
    raw.and_then(|value| {
        if value.trim().is_empty() {
            None
        } else {
            Some(value)
        }
    })
}

fn collect_report_artifacts(
    project: &crate::model::Project,
    tail: usize,
    generated_at_unix: u64,
    staging_dir: &Path,
    output_path: &Path,
) -> DynResult<ReportManifest> {
    let mut collected = Vec::new();
    let mut skipped = Vec::new();
    let mut warnings = Vec::new();
    let mut redaction = RedactionSummary::default();
    let home = resolve_home_dir_for_scrubbing();
    if home.is_none() {
        warnings.push(
            "home directory path scrubbing disabled: no HOME/USERPROFILE/HOMEDRIVE+HOMEPATH found"
                .to_string(),
        );
    }
    let sanitize_ctx = SanitizeContext {
        project_root: project.root.display().to_string(),
        home_dir: home.map(|path| path.display().to_string()),
    };

    skipped.push(SkippedItem {
        path: ".scaffold/wallet/**".to_string(),
        reason: "excluded by policy: wallet key material is never included".to_string(),
    });
    skipped.push(SkippedItem {
        path: ".scaffold/state/wallet.state".to_string(),
        reason: "excluded by policy: project default wallet state is never included raw"
            .to_string(),
    });
    skipped.push(SkippedItem {
        path: ".git/**".to_string(),
        reason: "excluded by policy".to_string(),
    });
    skipped.push(SkippedItem {
        path: "target/**".to_string(),
        reason: "excluded by policy".to_string(),
    });
    skipped.push(SkippedItem {
        path: ".env* (raw values)".to_string(),
        reason: "excluded by policy: only key-only redacted environment summaries are included"
            .to_string(),
    });

    write_text(
        &staging_dir.join("README.txt"),
        &format!(
            "logos-scaffold diagnostics report\n\n{REPORT_WARNING}\n\nGenerated at unix timestamp: {generated_at_unix}\n"
        ),
    )?;
    collected.push(CollectedItem {
        path: "README.txt".to_string(),
        source: "generated".to_string(),
        notes: Some("bundle safety warning".to_string()),
    });

    set_command_echo(false);
    let doctor_result = build_doctor_report();
    set_command_echo(true);
    match doctor_result {
        Ok(report) => {
            collect_sanitized_json_artifact(
                staging_dir,
                "diagnostics/doctor.json",
                &report,
                "logos-scaffold doctor report model",
                &sanitize_ctx,
                &mut collected,
                &mut skipped,
                &mut warnings,
                &mut redaction,
            )?;
        }
        Err(err) => {
            warnings.push(format!("doctor diagnostics unavailable: {err}"));
            skipped.push(SkippedItem {
                path: "diagnostics/doctor.json".to_string(),
                reason: format!("failed to collect doctor report: {err}"),
            });
        }
    }

    let localnet_status = build_localnet_status_for_project(project);
    collect_sanitized_json_artifact(
        staging_dir,
        "diagnostics/localnet-status.json",
        &localnet_status,
        "logos-scaffold localnet status model",
        &sanitize_ctx,
        &mut collected,
        &mut skipped,
        &mut warnings,
        &mut redaction,
    )?;

    let scaffold_path = project.root.join("scaffold.toml");
    match fs::read_to_string(&scaffold_path) {
        Ok(raw) => {
            let sanitize = sanitize_text(&raw, &sanitize_ctx);
            register_redaction(&mut redaction, sanitize.replacements);
            if contains_high_risk_content(&sanitize.text) {
                skipped.push(SkippedItem {
                    path: "project/scaffold.toml".to_string(),
                    reason: "file still contains high-risk secret markers after sanitization"
                        .to_string(),
                });
            } else {
                write_text(&staging_dir.join("project/scaffold.toml"), &sanitize.text)?;
                collected.push(CollectedItem {
                    path: "project/scaffold.toml".to_string(),
                    source: rel_path(&scaffold_path, &project.root),
                    notes: Some("paths scrubbed".to_string()),
                });
            }
        }
        Err(err) => {
            skipped.push(SkippedItem {
                path: "project/scaffold.toml".to_string(),
                reason: format!("failed to read required file: {err}"),
            });
            warnings.push(format!(
                "scaffold.toml could not be included: {err}. report may be incomplete"
            ));
        }
    }

    collect_env_summary(
        project,
        staging_dir,
        &sanitize_ctx,
        &mut collected,
        &mut skipped,
        &mut redaction,
    )?;

    collect_log_files(
        project,
        staging_dir,
        tail,
        &sanitize_ctx,
        &mut collected,
        &mut skipped,
        &mut redaction,
        &mut warnings,
    )?;

    let build_evidence = collect_build_evidence(project, &sanitize_ctx);
    write_json_artifact(
        staging_dir,
        "summaries/build-evidence.json",
        &build_evidence,
    )?;
    collected.push(CollectedItem {
        path: "summaries/build-evidence.json".to_string(),
        source: "filesystem scan".to_string(),
        notes: Some("no build commands executed".to_string()),
    });

    let git_metadata = collect_git_metadata(project, &sanitize_ctx);
    write_json_artifact(staging_dir, "summaries/git-metadata.json", &git_metadata)?;
    collected.push(CollectedItem {
        path: "summaries/git-metadata.json".to_string(),
        source: "git CLI".to_string(),
        notes: None,
    });

    let (tool_versions, tool_redactions, tool_warnings) =
        collect_tool_versions(project, &sanitize_ctx);
    register_redaction(&mut redaction, tool_redactions);
    warnings.extend(tool_warnings);
    write_json_artifact(staging_dir, "summaries/tool-versions.json", &tool_versions)?;
    collected.push(CollectedItem {
        path: "summaries/tool-versions.json".to_string(),
        source: "tool --version probes".to_string(),
        notes: None,
    });

    collected.push(CollectedItem {
        path: "manifest.json".to_string(),
        source: "generated".to_string(),
        notes: None,
    });

    scrub_manifest_entries(&mut collected, &mut skipped, &mut warnings, &sanitize_ctx);

    let output_archive_scrubbed =
        scrub_path_string(&output_path.display().to_string(), &sanitize_ctx);
    let manifest = ReportManifest {
        generated_at_unix,
        project_root: "<PROJECT_ROOT>".to_string(),
        output_archive: output_archive_scrubbed,
        include_count: collected.len(),
        skip_count: skipped.len(),
        redaction,
        collected,
        skipped,
        warnings,
    };

    write_json_artifact(staging_dir, "manifest.json", &manifest)?;

    Ok(manifest)
}

fn resolve_output_path(project_root: &Path, out: Option<PathBuf>, now: u64) -> DynResult<PathBuf> {
    match out {
        Some(path) if path.is_absolute() => Ok(path),
        Some(path) => Ok(env::current_dir()?.join(path)),
        None => Ok(default_report_output_path(project_root, now)),
    }
}

fn default_report_output_path(project_root: &Path, now: u64) -> PathBuf {
    let reports_dir = project_root.join(".scaffold/reports");
    let base_stem = format!("report-{now}");
    let default_path = reports_dir.join(format!("{base_stem}.tar.gz"));
    if !default_path.exists() {
        return default_path;
    }

    let pid_stem = format!("{base_stem}-{}", std::process::id());
    let pid_path = reports_dir.join(format!("{pid_stem}.tar.gz"));
    if !pid_path.exists() {
        return pid_path;
    }

    let mut suffix = 1_u64;
    loop {
        let candidate = reports_dir.join(format!("{pid_stem}-{suffix}.tar.gz"));
        if !candidate.exists() {
            return candidate;
        }
        suffix += 1;
    }
}

fn write_json_artifact<T: Serialize>(
    staging_dir: &Path,
    rel_path: &str,
    value: &T,
) -> DynResult<()> {
    let content = serde_json::to_string_pretty(value)?;
    write_text(&staging_dir.join(rel_path), &content)
}

#[allow(clippy::too_many_arguments)]
fn collect_sanitized_json_artifact<T: Serialize>(
    staging_dir: &Path,
    rel_path: &str,
    value: &T,
    source: &str,
    sanitize_ctx: &SanitizeContext,
    collected: &mut Vec<CollectedItem>,
    skipped: &mut Vec<SkippedItem>,
    warnings: &mut Vec<String>,
    redaction: &mut RedactionSummary,
) -> DynResult<()> {
    let sanitized = match render_sanitized_json_artifact(value, sanitize_ctx) {
        Ok(sanitized) => sanitized,
        Err(err) => {
            skipped.push(SkippedItem {
                path: rel_path.to_string(),
                reason: format!("failed to sanitize json artifact: {err}"),
            });
            warnings.push(format!(
                "could not sanitize optional artifact {rel_path}: {err}"
            ));
            return Ok(());
        }
    };

    register_redaction(redaction, sanitized.replacements);
    if contains_high_risk_content(&sanitized.text) {
        skipped.push(SkippedItem {
            path: rel_path.to_string(),
            reason: "artifact still contains high-risk secret markers after sanitization"
                .to_string(),
        });
        warnings.push(format!(
            "skipped {rel_path} because high-risk markers remained after sanitization"
        ));
        return Ok(());
    }

    if let Err(err) = write_text(&staging_dir.join(rel_path), &sanitized.text) {
        skipped.push(SkippedItem {
            path: rel_path.to_string(),
            reason: format!("failed to write sanitized artifact: {err}"),
        });
        warnings.push(format!(
            "could not write optional sanitized artifact {rel_path}: {err}"
        ));
        return Ok(());
    }

    collected.push(CollectedItem {
        path: rel_path.to_string(),
        source: source.to_string(),
        notes: None,
    });
    Ok(())
}

fn render_sanitized_json_artifact<T: Serialize>(
    value: &T,
    sanitize_ctx: &SanitizeContext,
) -> DynResult<SanitizedText> {
    let mut json = serde_json::to_value(value)?;
    let mut replacements = 0;
    sanitize_json_value_strings(&mut json, sanitize_ctx, &mut replacements);

    Ok(SanitizedText {
        text: serde_json::to_string_pretty(&json)?,
        replacements,
    })
}

fn sanitize_json_value_strings(
    value: &mut Value,
    sanitize_ctx: &SanitizeContext,
    replacements: &mut usize,
) {
    match value {
        Value::String(text) => {
            let sanitized = sanitize_text(text, sanitize_ctx);
            *replacements += sanitized.replacements;
            *text = sanitized.text;
        }
        Value::Array(items) => {
            for item in items {
                sanitize_json_value_strings(item, sanitize_ctx, replacements);
            }
        }
        Value::Object(map) => {
            for nested in map.values_mut() {
                sanitize_json_value_strings(nested, sanitize_ctx, replacements);
            }
        }
        _ => {}
    }
}

fn collect_env_summary(
    project: &crate::model::Project,
    staging_dir: &Path,
    sanitize_ctx: &SanitizeContext,
    collected: &mut Vec<CollectedItem>,
    skipped: &mut Vec<SkippedItem>,
    redaction: &mut RedactionSummary,
) -> DynResult<()> {
    let env_local_path = project.root.join(".env.local");
    if !env_local_path.exists() {
        skipped.push(SkippedItem {
            path: "project/env.local".to_string(),
            reason: "optional file missing: .env.local".to_string(),
        });
        return Ok(());
    }

    let raw = match fs::read_to_string(&env_local_path) {
        Ok(raw) => raw,
        Err(err) => {
            skipped.push(SkippedItem {
                path: "project/env.local".to_string(),
                reason: format!("failed to read .env.local: {err}"),
            });
            return Ok(());
        }
    };

    let mut rendered = String::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            rendered.push_str(line);
            rendered.push('\n');
            continue;
        }

        if let Some(eq_idx) = line.find('=') {
            let key = line[..eq_idx].trim();
            if key.is_empty() {
                rendered.push_str("# <redacted malformed env entry>\n");
            } else {
                rendered.push_str(key);
                rendered.push_str("=<REDACTED>\n");
            }
        } else {
            rendered.push_str("# <redacted non key-value env entry>\n");
        }
    }

    let sanitize = sanitize_text(&rendered, sanitize_ctx);
    register_redaction(redaction, sanitize.replacements);

    if contains_high_risk_content(&sanitize.text) {
        skipped.push(SkippedItem {
            path: "project/env.local".to_string(),
            reason: "sanitized env summary still contains high-risk markers".to_string(),
        });
        return Ok(());
    }

    write_text(&staging_dir.join("project/env.local"), &sanitize.text)?;
    collected.push(CollectedItem {
        path: "project/env.local".to_string(),
        source: ".env.local".to_string(),
        notes: Some("key-only redacted representation".to_string()),
    });

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn collect_log_files(
    project: &crate::model::Project,
    staging_dir: &Path,
    tail: usize,
    sanitize_ctx: &SanitizeContext,
    collected: &mut Vec<CollectedItem>,
    skipped: &mut Vec<SkippedItem>,
    redaction: &mut RedactionSummary,
    warnings: &mut Vec<String>,
) -> DynResult<()> {
    let logs_dir = project.root.join(".scaffold/logs");
    if !logs_dir.exists() {
        skipped.push(SkippedItem {
            path: "logs/*.log".to_string(),
            reason: "optional directory missing: .scaffold/logs".to_string(),
        });
        return Ok(());
    }

    let entries = match fs::read_dir(&logs_dir) {
        Ok(entries) => entries,
        Err(err) => {
            skipped.push(SkippedItem {
                path: "logs/*.log".to_string(),
                reason: format!("failed to read .scaffold/logs directory: {err}"),
            });
            warnings.push(format!(
                "could not inspect optional logs directory .scaffold/logs: {err}",
            ));
            return Ok(());
        }
    };

    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
            continue;
        }
        paths.push(path);
    }
    paths.sort();

    if paths.is_empty() {
        skipped.push(SkippedItem {
            path: "logs/*.log".to_string(),
            reason: "no .log files found under .scaffold/logs".to_string(),
        });
        return Ok(());
    }

    for path in paths {
        let raw = match tail_file_lines_lossy(&path, tail) {
            Ok(raw) => raw,
            Err(err) => {
                skipped.push(SkippedItem {
                    path: format!("logs/{}", file_name_or_unknown(&path)),
                    reason: format!("failed to read log file bytes: {err}"),
                });
                continue;
            }
        };

        let sanitize = sanitize_text(&raw, sanitize_ctx);
        register_redaction(redaction, sanitize.replacements);

        if contains_high_risk_content(&sanitize.text) {
            let rel = rel_path(&path, &project.root);
            skipped.push(SkippedItem {
                path: format!("logs/{}", file_name_or_unknown(&path)),
                reason: "log still contains high-risk secret markers after sanitization"
                    .to_string(),
            });
            warnings.push(format!(
                "skipped {} because high-risk markers remained after sanitization",
                rel
            ));
            continue;
        }

        let out_rel = format!("logs/{}", file_name_or_unknown(&path));
        write_text(&staging_dir.join(&out_rel), &sanitize.text)?;
        collected.push(CollectedItem {
            path: out_rel,
            source: rel_path(&path, &project.root),
            notes: Some(format!("tail={tail} lines, sanitized")),
        });
    }

    Ok(())
}

fn collect_build_evidence(
    project: &crate::model::Project,
    sanitize_ctx: &SanitizeContext,
) -> BuildEvidenceReport {
    let guest_src_dir = project.root.join("methods/guest/src/bin");
    let workspace_target = project.root.join("target");

    let mut guest_programs = Vec::new();
    if let Ok(entries) = fs::read_dir(&guest_src_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                guest_programs.push(stem.to_string());
            }
        }
    }
    guest_programs.sort();

    let discovered = super::deploy::discover_program_binaries(&project.root, &guest_programs);

    let mut expected_binaries = Vec::new();
    for program in &guest_programs {
        let path = discovered
            .get(program)
            .cloned()
            .unwrap_or_else(|| project.root.join(format!("{program}.bin")));
        let exists = discovered.contains_key(program);
        expected_binaries.push(BinaryArtifactSummary {
            program: program.clone(),
            relative_path: scrub_path_string(&rel_path(&path, &project.root), sanitize_ctx),
            exists,
            modified_unix: if exists { file_mtime_unix(&path) } else { None },
            size_bytes: if exists { file_size_bytes(&path) } else { None },
        });
    }

    let mut discovered_binaries: Vec<BinaryArtifactSummary> = discovered
        .iter()
        .map(|(program, path)| BinaryArtifactSummary {
            program: program.clone(),
            relative_path: scrub_path_string(&rel_path(path, &project.root), sanitize_ctx),
            exists: true,
            modified_unix: file_mtime_unix(path),
            size_bytes: file_size_bytes(path),
        })
        .collect();
    discovered_binaries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    BuildEvidenceReport {
        note: "No build commands were executed by `logos-scaffold report`; this is metadata only."
            .to_string(),
        workspace_target_exists: workspace_target.exists(),
        workspace_target_modified_unix: file_mtime_unix(&workspace_target),
        guest_programs,
        expected_binaries,
        discovered_binaries,
    }
}

fn collect_git_metadata(
    project: &crate::model::Project,
    sanitize_ctx: &SanitizeContext,
) -> GitMetadataReport {
    let mut errors = Vec::new();

    let head = run_simple_command("git", &["rev-parse", "HEAD"], Some(&project.root))
        .ok()
        .map(|s| scrub_path_string(s.trim(), sanitize_ctx));
    if head.is_none() {
        errors.push("failed to resolve git HEAD".to_string());
    }

    let branch = run_simple_command(
        "git",
        &["rev-parse", "--abbrev-ref", "HEAD"],
        Some(&project.root),
    )
    .ok()
    .map(|s| scrub_path_string(s.trim(), sanitize_ctx));
    if branch.is_none() {
        errors.push("failed to resolve git branch".to_string());
    }

    let clean = run_simple_command("git", &["status", "--porcelain"], Some(&project.root))
        .ok()
        .map(|stdout| stdout.trim().is_empty());
    if clean.is_none() {
        errors.push("failed to determine git working tree cleanliness".to_string());
    }

    GitMetadataReport {
        head,
        branch,
        clean,
        errors,
    }
}

fn collect_tool_versions(
    project: &crate::model::Project,
    sanitize_ctx: &SanitizeContext,
) -> (Vec<ToolCommandResult>, usize, Vec<String>) {
    let mut results = Vec::new();
    let mut warnings = Vec::new();
    let mut redaction_replacements = 0;

    for (name, program, args, cwd) in [
        ("git", "git", vec!["--version"], None::<&Path>),
        ("rustc", "rustc", vec!["--version"], None::<&Path>),
        ("cargo", "cargo", vec!["--version"], None::<&Path>),
    ] {
        let (result, replacements) = collect_tool_command(name, program, &args, cwd, sanitize_ctx);
        redaction_replacements += replacements;
        if result.error.is_some() || result.status.unwrap_or(1) != 0 {
            warnings.push(format!("tool probe `{name}` did not succeed"));
        }
        results.push(result);
    }

    let lez = match resolve_repo_path(project, &project.config.lez, "lez") {
        Ok(p) => p,
        Err(err) => {
            warnings.push(format!("could not resolve lez repo path: {err}"));
            return (results, redaction_replacements, warnings);
        }
    };
    let wallet_binary = lez.join(crate::constants::WALLET_BIN_REL_PATH);
    let wallet_binary_str = wallet_binary.display().to_string();
    let (wallet_result, wallet_replacements) = collect_tool_command(
        "wallet",
        &wallet_binary_str,
        &["--version"],
        None,
        sanitize_ctx,
    );
    redaction_replacements += wallet_replacements;
    if wallet_result.error.is_some() || wallet_result.status.unwrap_or(1) != 0 {
        warnings.push("tool probe `wallet` did not succeed".to_string());
    }
    results.push(wallet_result);

    if which("docker").is_some() {
        let (docker_result, replacements) =
            collect_tool_command("docker", "docker", &["--version"], None, sanitize_ctx);
        redaction_replacements += replacements;
        if docker_result.error.is_some() || docker_result.status.unwrap_or(1) != 0 {
            warnings.push("tool probe `docker` did not succeed".to_string());
        }
        results.push(docker_result);
    }

    if which("podman").is_some() {
        let (podman_result, replacements) =
            collect_tool_command("podman", "podman", &["--version"], None, sanitize_ctx);
        redaction_replacements += replacements;
        if podman_result.error.is_some() || podman_result.status.unwrap_or(1) != 0 {
            warnings.push("tool probe `podman` did not succeed".to_string());
        }
        results.push(podman_result);
    }

    (results, redaction_replacements, warnings)
}

fn collect_tool_command(
    name: &str,
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    sanitize_ctx: &SanitizeContext,
) -> (ToolCommandResult, usize) {
    let mut cmd = Command::new(program);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    for arg in args {
        cmd.arg(arg);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let command_rendered = format!("{} {}", program, args.join(" ")).trim().to_string();
    let sanitized_command = sanitize_text(&command_rendered, sanitize_ctx);

    match cmd.output() {
        Ok(output) => {
            let stdout_raw = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr_raw = String::from_utf8_lossy(&output.stderr).to_string();
            let sanitized_stdout = sanitize_text(&stdout_raw, sanitize_ctx);
            let sanitized_stderr = sanitize_text(&stderr_raw, sanitize_ctx);
            let replacements = sanitized_command.replacements
                + sanitized_stdout.replacements
                + sanitized_stderr.replacements;

            (
                ToolCommandResult {
                    name: name.to_string(),
                    command: sanitized_command.text,
                    status: output.status.code(),
                    stdout: sanitized_stdout.text,
                    stderr: sanitized_stderr.text,
                    error: None,
                },
                replacements,
            )
        }
        Err(err) => {
            let sanitized_error = sanitize_text(&err.to_string(), sanitize_ctx);
            (
                ToolCommandResult {
                    name: name.to_string(),
                    command: sanitized_command.text,
                    status: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    error: Some(sanitized_error.text),
                },
                sanitized_command.replacements + sanitized_error.replacements,
            )
        }
    }
}

fn run_simple_command(program: &str, args: &[&str], cwd: Option<&Path>) -> DynResult<String> {
    let mut cmd = Command::new(program);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output()?;
    if !output.status.success() {
        bail!(
            "{} {} failed with status {}",
            program,
            args.join(" "),
            output.status
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn tail_file_lines_lossy(path: &Path, tail: usize) -> DynResult<String> {
    if tail == 0 {
        return Ok(String::new());
    }

    // TODO: For very large logs, optimize this by seeking backwards from EOF.
    let file =
        File::open(path).with_context(|| format!("failed to open log file {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    let mut lines: VecDeque<Vec<u8>> = VecDeque::new();

    loop {
        buf.clear();
        let read = reader
            .read_until(b'\n', &mut buf)
            .with_context(|| format!("failed to read bytes from {}", path.display()))?;
        if read == 0 {
            break;
        }

        if lines.len() == tail {
            lines.pop_front();
        }
        lines.push_back(buf.clone());
    }

    let mut out = Vec::new();
    for line in lines {
        out.extend_from_slice(&line);
    }

    Ok(String::from_utf8_lossy(&out).to_string())
}

fn register_redaction(summary: &mut RedactionSummary, replacements: usize) {
    if replacements == 0 {
        return;
    }
    summary.files_redacted += 1;
    summary.replacements += replacements;
}

fn rel_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn file_name_or_unknown(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_string()
}

fn scrub_path_string(raw: &str, sanitize_ctx: &SanitizeContext) -> String {
    let (scrubbed, _) = scrub_paths(raw, sanitize_ctx);
    scrubbed
}

fn scrub_manifest_text(raw: &str, sanitize_ctx: &SanitizeContext) -> String {
    scrub_path_string(raw, sanitize_ctx)
}

fn scrub_manifest_entries(
    collected: &mut [CollectedItem],
    skipped: &mut [SkippedItem],
    warnings: &mut [String],
    sanitize_ctx: &SanitizeContext,
) {
    for item in collected {
        item.path = scrub_manifest_text(&item.path, sanitize_ctx);
        item.source = scrub_manifest_text(&item.source, sanitize_ctx);
        if let Some(notes) = &item.notes {
            item.notes = Some(scrub_manifest_text(notes, sanitize_ctx));
        }
    }

    for item in skipped {
        item.path = scrub_manifest_text(&item.path, sanitize_ctx);
        item.reason = scrub_manifest_text(&item.reason, sanitize_ctx);
    }

    for warning in warnings {
        *warning = scrub_manifest_text(warning, sanitize_ctx);
    }
}

fn scrub_paths(line: &str, sanitize_ctx: &SanitizeContext) -> (String, usize) {
    let mut current = line.to_string();
    let mut replacements = 0;

    let mut targets = Vec::new();
    collect_scrub_targets(&mut targets, &sanitize_ctx.project_root, "<PROJECT_ROOT>");
    if let Some(home_dir) = &sanitize_ctx.home_dir {
        collect_scrub_targets(&mut targets, home_dir, "<HOME>");
    }

    for (target, placeholder) in targets {
        if target.is_empty() {
            continue;
        }

        let count = current.matches(&target).count();
        if count > 0 {
            current = current.replace(&target, placeholder);
            replacements += count;
        }
    }

    (current, replacements)
}

fn collect_scrub_targets<'a>(out: &mut Vec<(String, &'a str)>, target: &str, placeholder: &'a str) {
    push_scrub_target(out, target, placeholder);
    if let Some(stripped) = target.strip_prefix("/private") {
        push_scrub_target(out, stripped, placeholder);
    } else if target.starts_with('/') {
        push_scrub_target(out, &format!("/private{target}"), placeholder);
    }
}

fn push_scrub_target<'a>(out: &mut Vec<(String, &'a str)>, target: &str, placeholder: &'a str) {
    if target.is_empty() {
        return;
    }
    if out.iter().any(|(existing, _)| existing == target) {
        return;
    }
    out.push((target.to_string(), placeholder));
}

fn sanitize_text(raw: &str, sanitize_ctx: &SanitizeContext) -> SanitizedText {
    let mut replacements = 0;
    let mut output_lines = Vec::new();
    let mut inside_private_key_block = false;

    for line in raw.lines() {
        let (scrubbed_paths, path_replacements) = scrub_paths(line, sanitize_ctx);
        replacements += path_replacements;

        if inside_private_key_block {
            let is_end = is_private_key_block_end_line(&scrubbed_paths);
            output_lines.push(redacted_line_marker(&scrubbed_paths));
            replacements += 1;
            if is_end {
                inside_private_key_block = false;
            }
            continue;
        }

        if is_private_key_block_begin_line(&scrubbed_paths) {
            let is_end = is_private_key_block_end_line(&scrubbed_paths);
            output_lines.push(redacted_line_marker(&scrubbed_paths));
            replacements += 1;
            if !is_end {
                inside_private_key_block = true;
            }
            continue;
        }

        let (redacted_kv, kv_replacements) = redact_sensitive_line(&scrubbed_paths);
        replacements += kv_replacements;

        let (redacted_urls, url_replacements) = redact_url_credentials(&redacted_kv);
        replacements += url_replacements;

        output_lines.push(redacted_urls);
    }

    let mut text = output_lines.join("\n");
    if raw.ends_with('\n') {
        text.push('\n');
    }

    SanitizedText { text, replacements }
}

fn redacted_line_marker(line: &str) -> String {
    let indentation: String = line
        .chars()
        .take_while(|ch| ch.is_ascii_whitespace())
        .collect();
    format!("{indentation}[REDACTED SENSITIVE LINE]")
}

fn is_private_key_block_begin_line(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    upper.contains("-----BEGIN") && upper.contains("PRIVATE KEY-----")
}

fn is_private_key_block_end_line(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    upper.contains("-----END") && upper.contains("PRIVATE KEY-----")
}

fn redact_sensitive_line(line: &str) -> (String, usize) {
    let lower = line.to_ascii_lowercase();

    let hard_line_keywords = [
        "secret_spending_key",
        "nullifier_secret_key",
        "viewing_secret_key",
        "private_key_holder",
        "-----begin private key",
    ];
    if hard_line_keywords
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return (redacted_line_marker(line), 1);
    }

    let key_value_keywords = [
        "password",
        "secret",
        "token",
        "api_key",
        "private_key",
        "mnemonic",
        "seed",
    ];
    if !key_value_keywords
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return (line.to_string(), 0);
    }

    let Some(eq_idx) = line.find('=') else {
        // Let URL-specific redaction handle embedded credentials in URL-like lines.
        if line.contains("://") {
            return (line.to_string(), 0);
        }

        let Some(colon_idx) = line.find(':') else {
            return ("[REDACTED SENSITIVE LINE]".to_string(), 1);
        };

        let trailing_comma = line[colon_idx + 1..].trim_end().ends_with(',');
        let replacement = if trailing_comma {
            " \"[REDACTED]\","
        } else {
            " \"[REDACTED]\""
        };
        return (format!("{}{}", &line[..=colon_idx], replacement), 1);
    };

    let trailing_comment = line[eq_idx + 1..]
        .find('#')
        .map(|idx| line[eq_idx + 1 + idx..].to_string());

    let mut out = format!("{}=\"[REDACTED]\"", &line[..eq_idx]);
    if let Some(comment) = trailing_comment {
        out.push(' ');
        out.push_str(comment.trim_start());
    }

    (out, 1)
}

fn redact_url_credentials(line: &str) -> (String, usize) {
    let mut rest = line;
    let mut output = String::new();
    let mut replacements = 0;

    while let Some(found) = rest.find("://") {
        let split_at = found + 3;
        let (prefix_with_scheme, after_scheme) = rest.split_at(split_at);
        output.push_str(prefix_with_scheme);

        let end = after_scheme
            .char_indices()
            .find_map(|(idx, ch)| {
                if ch.is_ascii_whitespace() || matches!(ch, '"' | '\'' | ')' | ']' | '>') {
                    Some(idx)
                } else {
                    None
                }
            })
            .unwrap_or(after_scheme.len());

        let url_slice = &after_scheme[..end];

        if let Some(at_idx) = url_slice.find('@') {
            output.push_str("[REDACTED]@");
            output.push_str(&url_slice[at_idx + 1..]);
            replacements += 1;
        } else {
            output.push_str(url_slice);
        }

        rest = &after_scheme[end..];
    }

    output.push_str(rest);

    if replacements == 0 {
        (line.to_string(), 0)
    } else {
        (output, replacements)
    }
}

fn contains_high_risk_content(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "secret_spending_key",
        "nullifier_secret_key",
        "viewing_secret_key",
        "private_key_holder",
        "-----begin private key",
        "-----end private key",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn file_mtime_unix(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(duration.as_secs())
}

fn file_size_bytes(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|metadata| metadata.len())
}

fn unix_timestamp_now() -> DynResult<u64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow!("system clock is before unix epoch: {err}"))?;
    Ok(duration.as_secs())
}

fn pack_staging_dir(staging_dir: &Path, output_path: &Path) -> DynResult<()> {
    let output_file = File::create(output_path)
        .with_context(|| format!("failed to create output archive {}", output_path.display()))?;
    let encoder = GzEncoder::new(output_file, Compression::default());
    let mut archive = tar::Builder::new(encoder);

    archive
        .append_dir_all("report", staging_dir)
        .with_context(|| {
            format!(
                "failed to package staging directory {}",
                staging_dir.display()
            )
        })?;

    let encoder = archive.into_inner()?;
    let _ = encoder.finish()?;
    Ok(())
}

#[derive(Clone)]
struct SanitizeContext {
    project_root: String,
    home_dir: Option<String>,
}

struct SanitizedText {
    text: String,
    replacements: usize,
}

#[derive(Serialize)]
struct GitMetadataReport {
    head: Option<String>,
    branch: Option<String>,
    clean: Option<bool>,
    errors: Vec<String>,
}

#[derive(Serialize)]
struct BinaryArtifactSummary {
    program: String,
    relative_path: String,
    exists: bool,
    modified_unix: Option<u64>,
    size_bytes: Option<u64>,
}

#[derive(Serialize)]
struct BuildEvidenceReport {
    note: String,
    workspace_target_exists: bool,
    workspace_target_modified_unix: Option<u64>,
    guest_programs: Vec<String>,
    expected_binaries: Vec<BinaryArtifactSummary>,
    discovered_binaries: Vec<BinaryArtifactSummary>,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::resolve_home_dir_from_env_like;

    #[test]
    fn resolve_home_dir_prefers_home() {
        let vars = HashMap::from([
            ("HOME".to_string(), "/home/alice".to_string()),
            ("USERPROFILE".to_string(), "C:\\Users\\Alice".to_string()),
        ]);

        let home = resolve_home_dir_from_env_like(|key| vars.get(key).cloned());
        assert_eq!(home.as_deref(), Some("/home/alice"));
    }

    #[test]
    fn resolve_home_dir_uses_userprofile_when_home_missing() {
        let vars = HashMap::from([("USERPROFILE".to_string(), "C:\\Users\\Alice".to_string())]);

        let home = resolve_home_dir_from_env_like(|key| vars.get(key).cloned());
        assert_eq!(home.as_deref(), Some("C:\\Users\\Alice"));
    }

    #[test]
    fn resolve_home_dir_uses_homedrive_and_homepath_when_needed() {
        let vars = HashMap::from([
            ("HOMEDRIVE".to_string(), "C:".to_string()),
            ("HOMEPATH".to_string(), "\\Users\\Alice".to_string()),
        ]);

        let home = resolve_home_dir_from_env_like(|key| vars.get(key).cloned());
        assert_eq!(home.as_deref(), Some("C:\\Users\\Alice"));
    }

    #[test]
    fn resolve_home_dir_returns_none_when_env_vars_missing() {
        let vars = HashMap::<String, String>::new();

        let home = resolve_home_dir_from_env_like(|key| vars.get(key).cloned());
        assert!(home.is_none());
    }
}
