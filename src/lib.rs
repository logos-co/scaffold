pub(crate) type DynResult<T> = anyhow::Result<T>;

pub mod api;

pub(crate) mod circuits;
pub(crate) mod cli;
pub(crate) mod cli_help;
pub(crate) mod commands;
pub(crate) mod config;
pub(crate) mod constants;
pub(crate) mod doctor_checks;
pub(crate) mod error;
pub(crate) mod lez_layout;
pub(crate) mod migrate;
pub(crate) mod model;
pub(crate) mod process;
pub(crate) mod project;
pub(crate) mod repo;
pub(crate) mod sequencer_config;
pub(crate) mod state;
pub(crate) mod template;
pub(crate) mod testnode;
pub(crate) mod tui;

/// Lowercase hex, two digits per byte.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(data))
}

pub fn run(args: Vec<String>) -> DynResult<()> {
    cli::run(args)
}

pub fn entry_main() {
    install_closed_stdout_hook();
    let args: Vec<String> = std::env::args().collect();
    if let Err(err) = run(args) {
        if is_broken_pipe(&err) && stdout_is_closed() {
            exit_like_sigpipe();
        }
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

// Rust starts every process with SIGPIPE ignored, so `lgs --help | head -1` or
// `lgs doctor | grep -m1 …` turns the reader hanging up into a panic backtrace (exit 101) or
// an `error: Broken pipe` line. Unix tools just die by the signal instead; do the same.
//
// SIGPIPE is deliberately *not* reset to SIG_DFL for the whole process:
// `process::run_with_stdin` relies on getting EPIPE when a child exits without reading its
// stdin (for example a wallet that fails before the password prompt).

/// Die by SIGPIPE, as a C program writing to a closed pipe would (shell status 141).
fn exit_like_sigpipe() -> ! {
    // SAFETY: plain libc calls with no pointers; the process exits right after.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        libc::raise(libc::SIGPIPE);
    }
    std::process::exit(141)
}

/// `print!`/`println!` panic when stdout is a closed pipe; turn that one panic into a
/// quiet SIGPIPE death and leave every other panic to the default hook.
fn install_closed_stdout_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if info
            .to_string()
            .contains("failed printing to stdout: Broken pipe")
        {
            exit_like_sigpipe();
        }
        default(info);
    }));
}

fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

/// True when nobody reads our stdout any more. A `BrokenPipe` can also come from a child's
/// stdin, which is a real error to report, so check the descriptor itself.
fn stdout_is_closed() -> bool {
    let mut fd = libc::pollfd {
        fd: libc::STDOUT_FILENO,
        events: 0,
        revents: 0,
    };
    // SAFETY: `fd` is a valid pollfd and nfds is 1; timeout 0 never blocks.
    let ready = unsafe { libc::poll(&mut fd, 1, 0) };
    ready > 0 && fd.revents & libc::POLLERR != 0
}

#[cfg(test)]
mod tests {
    #[test]
    fn hex_is_lowercase_padded() {
        assert_eq!(super::hex(&[0x0a, 0xff, 0x00]), "0aff00");
    }
}
