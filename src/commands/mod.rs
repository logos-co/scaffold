pub(crate) mod basecamp;
pub(crate) mod build;
pub(crate) mod client;
pub(crate) mod completions;
pub(crate) mod deploy;
pub(crate) mod doctor;
pub(crate) mod idl;
pub(crate) mod init;
pub(crate) mod localnet;
pub(crate) mod new;
pub(crate) mod report;
pub(crate) mod run;
pub(crate) mod run_state;
pub(crate) mod self_test;
pub(crate) mod setup;
pub(crate) mod spel;
pub(crate) mod wallet;
pub(crate) mod wallet_support;

/// Lowercase ASCII alphanumerics, collapse every run of other characters into a
/// single `separator`, trim leading/trailing separators, and return `fallback`
/// when nothing is left.
pub(crate) fn sanitize_separated(input: &str, separator: char, fallback: &str) -> String {
    let mut out = String::new();
    let mut prev_sep = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_sep = false;
        } else if !prev_sep {
            out.push(separator);
            prev_sep = true;
        }
    }
    let trimmed = out.trim_matches(separator);
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}
