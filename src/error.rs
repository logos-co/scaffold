use std::path::PathBuf;

use thiserror::Error;

/// Structured failure of an external command (cargo, git, wallet, nix, …).
///
/// Carries the rendered command line, the step label scaffold attached to the
/// invocation, the exit code (when the process exited normally), any captured
/// stdout/stderr, and the log file when output was redirected to one. The
/// `Display` output is preserved per call site so existing CLI error text is
/// unchanged; API consumers should read the structured fields instead of
/// parsing the message.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct CommandFailed {
    /// Rendered command line (`program arg1 arg2 …`).
    pub command: String,
    /// Human-readable step label (e.g. `build wallet`).
    pub label: String,
    /// Exit code if the process exited normally; `None` when killed by a
    /// signal or the status could not be determined.
    pub exit_code: Option<i32>,
    /// Captured stdout (empty when output was streamed or logged to a file).
    pub stdout: String,
    /// Captured stderr (empty when output was streamed or logged to a file).
    pub stderr: String,
    /// Log file holding the full output, when the step captured to one.
    pub log_path: Option<PathBuf>,
    pub(crate) message: String,
}

/// Invalid test-node block timing override.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{field} must be between {min} and {max} milliseconds")]
pub struct BlockTimingValidationError {
    field: &'static str,
    min: u64,
    max: u64,
}

impl BlockTimingValidationError {
    pub(crate) fn new(field: &'static str, min: u64, max: u64) -> Self {
        Self { field, min, max }
    }

    /// Name of the invalid override field.
    pub fn field(&self) -> &'static str {
        self.field
    }

    /// Minimum accepted value, in milliseconds.
    pub fn min(&self) -> u64 {
        self.min
    }

    /// Maximum accepted value, in milliseconds.
    pub fn max(&self) -> u64 {
        self.max
    }
}

#[derive(Debug, Error)]
pub(crate) enum LocalnetError {
    #[error("missing sequencer binary at {path}; run `logos-scaffold setup`")]
    MissingSequencerBinary { path: String },

    #[error("sequencer process exited before becoming ready (pid={pid})\nlast logs:\n{log_tail}")]
    ExitedBeforeReady { pid: u32, log_tail: String },

    #[error("localnet start timed out after {timeout_sec}s (pid={pid})\nlast logs:\n{log_tail}")]
    StartTimeout {
        timeout_sec: u64,
        pid: u32,
        log_tail: String,
    },

    #[error(
        "failed to send TERM to sequencer (pid={pid}): {stderr}\n\
         localnet state file preserved; resolve the kill failure manually and retry."
    )]
    StopKillFailed { pid: u32, stderr: String },

    #[error(
        "sequencer did not exit within {timeout_sec}s of TERM (pid={pid}).\n\
         localnet state file preserved; the process may be ignoring TERM. \
         Try `kill -9 {pid}` and retry `logos-scaffold localnet stop`."
    )]
    StopTimeout { pid: u32, timeout_sec: u64 },
}

#[derive(Debug, Error)]
pub(crate) enum ResetError {
    #[error(
        "cannot reset: foreign listener on {addr}{}\n\
         Stop the external process before running `logos-scaffold localnet reset`.",
        pid.map(|p| format!(" (pid={p})")).unwrap_or_default()
    )]
    ForeignListener { addr: String, pid: Option<u32> },

    #[error(
        "sequencer started but is not producing blocks after {timeout_sec}s.\n\
         Check `logos-scaffold localnet logs --tail 200` for errors.\n\
         Run `logos-scaffold localnet status` for diagnostics."
    )]
    BlocksNotProduced { timeout_sec: u64 },

    #[error("verification poll failed: {0}")]
    VerificationPollFailed(String),
}
