//! Public error type for the scaffold API.
//!
//! Every API entry point returns [`Error`], which buckets failures into the
//! categories consumers need to branch on: configuration problems, missing
//! tools, repository state, managed-process failures, timeouts, transport
//! failures, and structured external-command failures. Anything that does not
//! fit a category surfaces as [`Error::Other`] with the full error chain
//! preserved.

use std::path::PathBuf;

pub use crate::error::CommandFailed;
use crate::error::{LocalnetError, ResetError};

/// Result alias used by every API entry point.
pub type Result<T> = std::result::Result<T, Error>;

/// Categorized API error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `scaffold.toml` is missing, unparseable, on an unsupported schema
    /// version, or an option value is invalid.
    #[error("configuration error: {message}")]
    Config { message: String },

    /// A required binary or artifact is missing (sequencer, wallet, spel,
    /// circuits release, …). `path` is the location that was probed, when
    /// known.
    #[error("{message}")]
    MissingTool {
        message: String,
        path: Option<PathBuf>,
    },

    /// A pinned repository checkout is dirty, missing, or not at the
    /// configured pin.
    #[error("repository state error: {message}")]
    RepoState { message: String },

    /// A managed long-lived process failed (sequencer exited before ready,
    /// refused to stop, a foreign process holds its port, …).
    #[error("{message}")]
    Process {
        message: String,
        /// Tail of the process log when one was captured.
        log_tail: Option<String>,
    },

    /// An operation did not complete within its deadline.
    #[error("{message}")]
    Timeout { message: String, timeout_sec: u64 },

    /// An RPC/HTTP transport failure talking to the sequencer or a remote
    /// endpoint. Distinct from business-level rejections.
    #[error("transport error: {message}")]
    Transport { message: String },

    /// An external command exited non-zero. Carries the rendered command,
    /// exit code, and captured diagnostic output.
    #[error(transparent)]
    Command(CommandFailed),

    /// Uncategorized failure; the full `anyhow` chain is preserved.
    #[error(transparent)]
    Other(anyhow::Error),
}

/// Map an internal `anyhow::Error` onto the public categories by downcasting
/// the typed errors the crate produces. Unknown errors pass through as
/// [`Error::Other`] — no string sniffing.
pub(crate) fn classify(err: anyhow::Error) -> Error {
    let err = match err.downcast::<CommandFailed>() {
        Ok(failure) => return Error::Command(failure),
        Err(err) => err,
    };

    let err = match err.downcast::<LocalnetError>() {
        Ok(localnet) => {
            let message = localnet.to_string();
            return match localnet {
                LocalnetError::MissingSequencerBinary { path } => Error::MissingTool {
                    message,
                    path: Some(PathBuf::from(path)),
                },
                LocalnetError::ExitedBeforeReady { log_tail, .. } => Error::Process {
                    message,
                    log_tail: Some(log_tail),
                },
                LocalnetError::StartTimeout { timeout_sec, .. }
                | LocalnetError::StopTimeout { timeout_sec, .. } => Error::Timeout {
                    message,
                    timeout_sec,
                },
                LocalnetError::StopKillFailed { .. } => Error::Process {
                    message,
                    log_tail: None,
                },
            };
        }
        Err(err) => err,
    };

    let err = match err.downcast::<ResetError>() {
        Ok(reset) => {
            let message = reset.to_string();
            return match reset {
                ResetError::ForeignListener { .. } => Error::Process {
                    message,
                    log_tail: None,
                },
                ResetError::BlocksNotProduced { timeout_sec } => Error::Timeout {
                    message,
                    timeout_sec,
                },
                ResetError::VerificationPollFailed(_) => Error::Transport { message },
            };
        }
        Err(err) => err,
    };

    let err = match err.downcast::<crate::commands::wallet_support::RpcReachabilityError>() {
        Ok(rpc) => {
            return Error::Transport {
                message: rpc.to_string(),
            }
        }
        Err(err) => err,
    };

    Error::Other(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_maps_command_failed() {
        let err: anyhow::Error = CommandFailed {
            message: "build wallet failed with exit status: 1".to_string(),
            command: "cargo build".to_string(),
            label: "build wallet".to_string(),
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "boom".to_string(),
            log_path: None,
        }
        .into();
        match classify(err) {
            Error::Command(failure) => {
                assert_eq!(failure.exit_code, Some(1));
                assert_eq!(failure.command, "cargo build");
                assert_eq!(failure.stderr, "boom");
            }
            other => panic!("expected Command, got {other:?}"),
        }
    }

    #[test]
    fn classify_maps_missing_sequencer_to_missing_tool() {
        let err: anyhow::Error = LocalnetError::MissingSequencerBinary {
            path: "/tmp/lez/target/release/sequencer_service".to_string(),
        }
        .into();
        match classify(err) {
            Error::MissingTool { path, .. } => {
                assert_eq!(
                    path.as_deref(),
                    Some(std::path::Path::new(
                        "/tmp/lez/target/release/sequencer_service"
                    ))
                );
            }
            other => panic!("expected MissingTool, got {other:?}"),
        }
    }

    #[test]
    fn classify_maps_start_timeout_to_timeout() {
        let err: anyhow::Error = LocalnetError::StartTimeout {
            timeout_sec: 20,
            pid: 1234,
            log_tail: "tail".to_string(),
        }
        .into();
        match classify(err) {
            Error::Timeout { timeout_sec, .. } => assert_eq!(timeout_sec, 20),
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn classify_passes_unknown_errors_through() {
        let err = anyhow::anyhow!("something else");
        match classify(err) {
            Error::Other(inner) => assert_eq!(inner.to_string(), "something else"),
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
