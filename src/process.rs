use std::env;
use std::fs::File;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::bail;

use crate::model::Captured;
use crate::DynResult;

static ECHO_COMMANDS: AtomicBool = AtomicBool::new(true);

pub(crate) fn set_command_echo(enabled: bool) {
    ECHO_COMMANDS.store(enabled, Ordering::Relaxed);
}

/// RAII guard that suppresses subprocess command echo for the duration of its
/// scope. On drop, restores `ECHO_COMMANDS` to whatever it was when the guard
/// was constructed — so nesting (or already-suppressed outer scopes) round-trip
/// correctly. Use this instead of paired `set_command_echo(false)` /
/// `set_command_echo(true)` calls so a `?` propagation or panic doesn't leave
/// echo permanently disabled for the rest of the process.
pub(crate) struct EchoGuard {
    restore_to: bool,
}

impl EchoGuard {
    pub(crate) fn suppress() -> Self {
        let restore_to = ECHO_COMMANDS.load(Ordering::Relaxed);
        ECHO_COMMANDS.store(false, Ordering::Relaxed);
        Self { restore_to }
    }
}

impl Drop for EchoGuard {
    fn drop(&mut self) {
        ECHO_COMMANDS.store(self.restore_to, Ordering::Relaxed);
    }
}

fn should_echo() -> bool {
    ECHO_COMMANDS.load(Ordering::Relaxed)
}

pub(crate) fn render_command(cmd: &Command) -> String {
    let mut out = cmd.get_program().to_string_lossy().to_string();
    for arg in cmd.get_args() {
        out.push(' ');
        out.push_str(&arg.to_string_lossy());
    }
    out
}

pub(crate) fn run_checked(cmd: &mut Command, label: &str) -> DynResult<()> {
    run_forwarded(cmd, label)
}

pub(crate) fn run_forwarded(cmd: &mut Command, label: &str) -> DynResult<()> {
    if should_echo() {
        println!("$ {}", render_command(cmd));
    }
    let status = cmd.status()?;
    if !status.success() {
        bail!("{label} failed with {status}");
    }
    Ok(())
}

/// Set to `true` when `--print-output` or `LOGOS_SCAFFOLD_PRINT_OUTPUT=1` is
/// in effect. When true, `run_logged` falls back to streaming subprocess
/// output directly to the terminal — useful for CI pipelines that already
/// capture structured logs, or for debugging a weird build failure.
static PRINT_OUTPUT: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_print_output(enabled: bool) {
    PRINT_OUTPUT.store(enabled, Ordering::Relaxed);
}

pub(crate) fn print_output_enabled() -> bool {
    PRINT_OUTPUT.load(Ordering::Relaxed)
        || std::env::var_os("LOGOS_SCAFFOLD_PRINT_OUTPUT")
            .as_deref()
            .map(is_truthy_env_value)
            .unwrap_or(false)
}

/// Accept only `1` or `true` (case-insensitive). Rejects `0`, empty,
/// `false`, `no`, `yes`, `on`, and anything else — so
/// `LOGOS_SCAFFOLD_PRINT_OUTPUT=false` doesn't surprisingly enable
/// streaming.
fn is_truthy_env_value(v: &std::ffi::OsStr) -> bool {
    match v.to_str() {
        Some("1") => true,
        Some(s) => s.eq_ignore_ascii_case("true"),
        None => false,
    }
}

/// Run a subprocess with captured output and a single progress line. The
/// user sees:
/// - `<step>` on its own line.
/// - `  tip: tail -f <abs-log-path>` so they can watch live.
/// - In TTY mode: an animated `⋯ <step>` spinner line that resolves to
///   `  ✓ <step> (<duration>)` on success or `  ✗ <step> (<duration>)` on
///   failure.
/// - In non-TTY mode: the same `⋯`/`✓`/`✗` lines, no animation.
///
/// Full stdout+stderr is captured to the log file at `log_path`. On failure,
/// the error carries the log path and, when nix's truncated-eval-trace
/// marker is present in the log, a hint to re-run with `--show-trace`.
/// Under `--print-output` / `LOGOS_SCAFFOLD_PRINT_OUTPUT=1`, output streams
/// directly to the terminal and nothing is captured.
///
/// After the step completes, old logs in the same `.scaffold/logs/`
/// directory with the same command-name suffix are rotated down to 10.
pub(crate) fn run_logged(cmd: &mut Command, step: &str, log_path: &Path) -> DynResult<()> {
    if print_output_enabled() {
        return run_forwarded_with_status(cmd, step);
    }

    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Seed the log with a header block. `File::create` is fine here — the
    // timestamp stamp is millisecond-granular (see `timestamp_compact`), so
    // two back-to-back calls don't collide on the same filename.
    if let Ok(mut f) = File::create(log_path) {
        use std::io::Write;
        let _ = writeln!(f, "# step: {step}");
        let _ = writeln!(f, "# started: {}", chrono_like_stamp());
        let _ = writeln!(f, "# command: {}", render_command(cmd));
        let _ = writeln!(f, "---");
    }

    println!("{step}");
    println!("  tip: tail -f {}", log_path.display());

    let start = std::time::Instant::now();
    let bar = make_spinner(step);

    // Append captured output to the log (the header seeded above stays).
    let log = std::fs::OpenOptions::new()
        .append(true)
        .open(log_path)
        .map_err(|e| anyhow::anyhow!("open log {}: {e}", log_path.display()))?;

    cmd.stdout(Stdio::from(
        log.try_clone()
            .map_err(|e| anyhow::anyhow!("clone log handle: {e}"))?,
    ));
    cmd.stderr(Stdio::from(
        log.try_clone()
            .map_err(|e| anyhow::anyhow!("clone log handle: {e}"))?,
    ));

    let status = cmd.status()?;
    let duration = fmt_duration(start.elapsed());

    // Rotate once per call. `rotate_logs` is defensive: if the stem doesn't
    // parse or the logs dir doesn't exist, it's a silent no-op.
    if let Some((project_root, command)) = project_root_and_command_from_log_path(log_path) {
        rotate_logs(&project_root, command, 10);
    }

    if status.success() {
        finish_spinner_ok(bar, step, &duration);
        Ok(())
    } else {
        finish_spinner_err(bar, step, &duration);
        let mut detail = format!("{step} failed with {status}; see {}", log_path.display());
        if log_indicates_truncated_trace(log_path) {
            detail.push_str(&format!(
                "\nhint: nix elided part of the eval trace — re-run with --show-trace for full detail: {} --show-trace",
                render_command(cmd)
            ));
        }
        bail!("{detail}");
    }
}

/// `run_forwarded` but with the same spinner-status UX shape as logged
/// mode: echoes the command, streams live, prints ✓/✗ with duration.
fn run_forwarded_with_status(cmd: &mut Command, step: &str) -> DynResult<()> {
    println!("{step}");
    println!("  running: {}", render_command(cmd));
    let start = std::time::Instant::now();
    let status = cmd.status()?;
    let duration = fmt_duration(start.elapsed());
    if status.success() {
        println!("  ✓ {step} ({duration})");
        Ok(())
    } else {
        println!("  ✗ {step} ({duration})");
        bail!("{step} failed with {status}");
    }
}

fn make_spinner(step: &str) -> Option<indicatif::ProgressBar> {
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal() {
        // Non-TTY: print a static "⋯" line; finalization prints the ✓/✗ line.
        println!("  ⋯ {step}");
        return None;
    }
    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(
        indicatif::ProgressStyle::with_template("  {spinner:.cyan} {wide_msg}")
            .expect("valid progress template")
            .tick_strings(&["⋯", "⋯.", "⋯..", "⋯..."]),
    );
    pb.set_message(step.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(200));
    Some(pb)
}

fn finish_spinner_ok(bar: Option<indicatif::ProgressBar>, step: &str, duration: &str) {
    let msg = format!("✓ {step} ({duration})");
    if let Some(bar) = bar {
        bar.set_style(
            indicatif::ProgressStyle::with_template("  {msg:.green}").expect("valid template"),
        );
        bar.finish_with_message(msg);
    } else {
        println!("  ✓ {step} ({duration})");
    }
}

fn finish_spinner_err(bar: Option<indicatif::ProgressBar>, step: &str, duration: &str) {
    let msg = format!("✗ {step} ({duration})");
    if let Some(bar) = bar {
        bar.set_style(
            indicatif::ProgressStyle::with_template("  {msg:.red}").expect("valid template"),
        );
        bar.finish_with_message(msg);
    } else {
        println!("  ✗ {step} ({duration})");
    }
}

/// Scan a build log for nix's stack-trace-truncation marker. Only appears on
/// evaluation-time failures when the eval stack exceeds nix's default frame
/// limit (~25). Not present for builder-stage failures, so this check gates
/// the `--show-trace` hint to cases where it actually helps.
fn log_indicates_truncated_trace(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|s| s.contains("stack trace truncated"))
        .unwrap_or(false)
}

/// Given a log path of the shape
/// `<project_root>/.scaffold/logs/YYYYMMDD-HHMMSS-mmm-<command>.log`, recover
/// the `(project_root, command)` pair used for rotation. Returns `None` if
/// the path doesn't match that layout; rotation becomes a no-op.
fn project_root_and_command_from_log_path(log_path: &Path) -> Option<(PathBuf, &str)> {
    let project_root = log_path.parent()?.parent()?.parent()?.to_path_buf();
    let stem = log_path.file_stem()?.to_str()?;
    let mut parts = stem.splitn(4, '-');
    parts.next()?; // YYYYMMDD
    parts.next()?; // HHMMSS
    parts.next()?; // mmm
    let command = parts.next()?;
    if command.is_empty() {
        return None;
    }
    Some((project_root, command))
}

/// Build a log path `<project_root>/.scaffold/logs/<stamp>-<command>.log`.
/// Caller is responsible for ensuring `project_root` exists.
pub(crate) fn derive_log_path(project_root: &Path, command: &str) -> PathBuf {
    project_root
        .join(".scaffold/logs")
        .join(format!("{}-{}.log", timestamp_compact(), command))
}

/// Delete all but the most recent `keep` log files for a given command prefix
/// (matches on filename suffix `-<command>.log`). No-op if the logs dir
/// doesn't exist yet.
pub(crate) fn rotate_logs(project_root: &Path, command: &str, keep: usize) {
    use std::fs;
    let logs_dir = project_root.join(".scaffold/logs");
    let Ok(entries) = fs::read_dir(&logs_dir) else {
        return;
    };
    let suffix = format!("-{command}.log");
    let mut matching: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.ends_with(&suffix))
                .unwrap_or(false)
        })
        .filter_map(|e| {
            e.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| (t, e.path()))
        })
        .collect();
    matching.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    for (_, path) in matching.into_iter().skip(keep) {
        let _ = fs::remove_file(path);
    }
}

fn fmt_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        // Integer half-up rounding to deciseconds: 499ms → 0.5s, 949ms →
        // 0.9s, 950ms → 1.0s. (Floating-point `{:.1}` is unreliable here —
        // 0.95f32 actually stores as 0.9499…, formatting to "0.9".)
        let ms = d.subsec_millis();
        let deci_total = secs * 10 + (u64::from(ms) + 50) / 100;
        let whole = deci_total / 10;
        let deci = deci_total % 10;
        format!("{whole}.{deci}s")
    } else {
        let m = secs / 60;
        let s = secs % 60;
        format!("{m}m{s:02}s")
    }
}

fn timestamp_compact() -> String {
    // YYYYMMDD-HHMMSS-mmm using the system clock. Millis granularity so two
    // `run_logged` calls completing in the same wall-clock second (warm nix
    // cache, fast builds) don't produce the same filename and clobber each
    // other's log via the subsequent `File::create`.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let (y, mo, d, h, mi, se) = unix_to_ymdhms(now.as_secs());
    let ms = now.subsec_millis();
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{se:02}-{ms:03}")
}

fn chrono_like_stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let (y, mo, d, h, mi, se) = unix_to_ymdhms(now.as_secs());
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{se:02}Z")
}

/// Minimal unix-epoch → (Y, M, D, h, m, s) conversion (UTC). Handles 1970+
/// for log-naming purposes; we don't need TZ or leap seconds.
fn unix_to_ymdhms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = secs / 86400;
    let tod = secs % 86400;
    let h = (tod / 3600) as u32;
    let mi = ((tod % 3600) / 60) as u32;
    let se = (tod % 60) as u32;

    // Civil-from-days, Howard Hinnant's algorithm.
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (y + if m <= 2 { 1 } else { 0 }) as u32;
    (y, m, d, h, mi, se)
}

#[cfg(test)]
mod logged_tests {
    use super::*;

    #[test]
    fn fmt_duration_short_secs() {
        assert_eq!(fmt_duration(std::time::Duration::from_millis(500)), "0.5s");
        assert_eq!(fmt_duration(std::time::Duration::from_secs(37)), "37.0s");
    }

    #[test]
    fn fmt_duration_rounds_instead_of_truncating() {
        // R-S2: previously we floored via `subsec_millis() / 100`, so 499ms
        // rendered as "0.4s" (floor of 4.99 → 4). Round to nearest decisecond.
        assert_eq!(fmt_duration(std::time::Duration::from_millis(499)), "0.5s");
        assert_eq!(fmt_duration(std::time::Duration::from_millis(949)), "0.9s");
        assert_eq!(fmt_duration(std::time::Duration::from_millis(950)), "1.0s");
    }

    #[test]
    fn fmt_duration_minutes() {
        assert_eq!(fmt_duration(std::time::Duration::from_secs(65)), "1m05s");
        assert_eq!(fmt_duration(std::time::Duration::from_secs(3601)), "60m01s");
    }

    #[test]
    fn is_truthy_env_value_accepts_only_one_and_true() {
        use std::ffi::OsStr;
        // R-S1: previously "any non-empty, non-zero value" was truthy, so
        // `LOGOS_SCAFFOLD_PRINT_OUTPUT=false` surprisingly enabled streaming.
        assert!(is_truthy_env_value(OsStr::new("1")));
        assert!(is_truthy_env_value(OsStr::new("true")));
        assert!(is_truthy_env_value(OsStr::new("TRUE")));
        assert!(is_truthy_env_value(OsStr::new("True")));
        assert!(!is_truthy_env_value(OsStr::new("0")));
        assert!(!is_truthy_env_value(OsStr::new("")));
        assert!(!is_truthy_env_value(OsStr::new("false")));
        assert!(!is_truthy_env_value(OsStr::new("no")));
        assert!(!is_truthy_env_value(OsStr::new("yes")));
        assert!(!is_truthy_env_value(OsStr::new("on")));
    }

    #[test]
    fn echo_guard_restores_previous_state_on_drop() {
        // Verify the RAII guard's contract: capture-then-restore, so a `?`
        // propagation inside a `--json` block never leaves echo permanently
        // suppressed for subsequent commands. The guard captures the *current*
        // state at construction (not a hardcoded `true`), so nesting works.
        // NOTE: `ECHO_COMMANDS` is a process-global; this test cannot be run
        // concurrently with other tests that toggle it. The default is `true`.
        let initial = ECHO_COMMANDS.load(Ordering::Relaxed);
        // Force a known starting state.
        ECHO_COMMANDS.store(true, Ordering::Relaxed);
        {
            let _g = EchoGuard::suppress();
            assert!(!ECHO_COMMANDS.load(Ordering::Relaxed));
        }
        assert!(ECHO_COMMANDS.load(Ordering::Relaxed));
        // Outer-suppressed → guard suppresses → drop restores to suppressed.
        ECHO_COMMANDS.store(false, Ordering::Relaxed);
        {
            let _g = EchoGuard::suppress();
            assert!(!ECHO_COMMANDS.load(Ordering::Relaxed));
        }
        assert!(!ECHO_COMMANDS.load(Ordering::Relaxed));
        // Restore for any tests that follow.
        ECHO_COMMANDS.store(initial, Ordering::Relaxed);
    }

    #[test]
    fn unix_to_ymdhms_spot_checks() {
        // 2020-01-01 00:00:00 UTC
        assert_eq!(unix_to_ymdhms(1577836800), (2020, 1, 1, 0, 0, 0));
        // 2024-06-15 12:34:56 UTC
        assert_eq!(unix_to_ymdhms(1718454896), (2024, 6, 15, 12, 34, 56));
    }

    #[test]
    fn derive_log_path_uses_scaffold_logs_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let p = derive_log_path(tmp.path(), "setup");
        assert!(p.starts_with(tmp.path().join(".scaffold/logs")));
        assert!(p.to_string_lossy().ends_with("-setup.log"));
    }

    #[test]
    fn derive_log_path_includes_millisecond_suffix_in_stamp() {
        // R-C1: two calls within the same second must produce different
        // filenames. Millis granularity gives us 1000x the headroom before
        // two truly-simultaneous calls collide on File::create truncation.
        let tmp = tempfile::tempdir().unwrap();
        let p = derive_log_path(tmp.path(), "install");
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap();
        // Expected shape: YYYYMMDD-HHMMSS-mmm-<command>
        //   parts[0] = YYYYMMDD, parts[1] = HHMMSS,
        //   parts[2] = mmm,      parts[3] = command
        let parts: Vec<&str> = stem.split('-').collect();
        assert!(
            parts.len() >= 4,
            "expected 4 dash-separated segments, got: {stem}"
        );
        assert_eq!(
            parts[1].len(),
            6,
            "HHMMSS should be 6 chars, got {:?} in {stem}",
            parts[1]
        );
        assert_eq!(
            parts[2].len(),
            3,
            "millis should be 3 digits, got {:?} in {stem}",
            parts[2]
        );
        assert!(parts[2].chars().all(|c| c.is_ascii_digit()));
        assert_eq!(parts[3], "install");
    }
}

pub(crate) fn run_capture(cmd: &mut Command, label: &str) -> DynResult<Captured> {
    if should_echo() {
        println!("$ {}", render_command(cmd));
    }
    let Output {
        status,
        stdout,
        stderr,
    } = cmd.output()?;

    let captured = Captured {
        status,
        stdout: String::from_utf8_lossy(&stdout).to_string(),
        stderr: String::from_utf8_lossy(&stderr).to_string(),
    };

    if !captured.status.success() {
        bail!("{label} failed: {}", captured.stderr);
    }

    Ok(captured)
}

pub(crate) fn run_with_stdin(mut cmd: Command, input: String) -> DynResult<Captured> {
    if should_echo() {
        println!("$ {}", render_command(&cmd));
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        if let Err(err) = stdin.write_all(input.as_bytes()) {
            if err.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(err.into());
            }
        }
    }
    let out = child.wait_with_output()?;
    Ok(Captured {
        status: out.status,
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    })
}

pub(crate) fn spawn_to_log(cmd: &mut Command, log_path: &Path) -> DynResult<u32> {
    if should_echo() {
        println!("$ {}", render_command(cmd));
    }
    let file = File::create(log_path)?;
    let err_file = file.try_clone()?;
    cmd.stdout(Stdio::from(file)).stderr(Stdio::from(err_file));
    let child = cmd.spawn()?;
    Ok(child.id())
}

#[cfg(unix)]
pub(crate) fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub(crate) fn pid_alive(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn pid_is_zombie(pid: u32) -> bool {
    let output = Command::new("ps")
        .arg("-o")
        .arg("stat=")
        .arg("-p")
        .arg(pid.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }

    let stat = String::from_utf8_lossy(&output.stdout);
    stat.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.starts_with('Z'))
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn pid_is_zombie(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
pub(crate) fn pid_running(pid: u32) -> bool {
    pid_alive(pid) && !pid_is_zombie(pid)
}

#[cfg(not(unix))]
pub(crate) fn pid_running(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
pub(crate) fn pid_command(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .arg("-o")
        .arg("command=")
        .arg("-p")
        .arg(pid.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToString::to_string)
}

#[cfg(not(unix))]
pub(crate) fn pid_command(_pid: u32) -> Option<String> {
    None
}

pub(crate) fn port_open(addr: &str) -> bool {
    let parsed: SocketAddr = match addr.parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    TcpStream::connect_timeout(&parsed, Duration::from_millis(500)).is_ok()
}

#[cfg(unix)]
pub(crate) fn listener_pid(port: u16) -> Option<u32> {
    let output = Command::new("lsof")
        .arg("-nP")
        .arg(format!("-iTCP:{port}"))
        .arg("-sTCP:LISTEN")
        .arg("-t")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.trim().parse::<u32>().ok())
}

#[cfg(not(unix))]
pub(crate) fn listener_pid(_port: u16) -> Option<u32> {
    None
}

pub(crate) fn which(binary: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    for p in env::split_paths(&paths) {
        let candidate = p.join(binary);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}
