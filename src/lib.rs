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
    let args: Vec<String> = std::env::args().collect();
    if let Err(err) = run(args) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hex_is_lowercase_padded() {
        assert_eq!(super::hex(&[0x0a, 0xff, 0x00]), "0aff00");
    }
}
