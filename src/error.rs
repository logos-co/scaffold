use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum LocalnetError {
    #[error("missing or invalid sequencer binary at {path}; run `logos-scaffold setup`")]
    MissingSequencerBinary { path: String },

    #[error("sequencer process exited before becoming ready (pid={pid})\nlast logs:\n{log_tail}")]
    ExitedBeforeReady { pid: u32, log_tail: String },

    #[error("localnet start timed out after {timeout_sec}s (pid={pid})\nlast logs:\n{log_tail}")]
    StartTimeout {
        timeout_sec: u64,
        pid: u32,
        log_tail: String,
    },
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
