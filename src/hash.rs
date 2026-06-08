//! Small hashing helpers shared by the deploy / IDL caches and the prebuilt
//! download verifier.

use std::fmt::Write;

use sha2::{Digest, Sha256};

/// Lowercase, zero-padded hex encoding of a byte slice.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// SHA-256 of `data`, hex-encoded.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex_encode(&Sha256::digest(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encode_is_lowercase_padded() {
        assert_eq!(hex_encode(&[0x0a, 0xff, 0x00]), "0aff00");
    }
}
