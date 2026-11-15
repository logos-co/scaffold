//! End-to-end suite: the dogfooding scenarios of `.dogfooding/features/` as a
//! deterministic program. See `tests/e2e/README.md`.
//!
//! This target has its own runner (`harness = false`): scenarios share one
//! sequencer port and one chain, so they run one at a time in a fixed order.
//! A plain `cargo test` only checks that this suite and `.dogfooding/` name the
//! same scenarios; set `LGS_E2E=1` to run them.

#[cfg(unix)]
mod fixtures;
#[cfg(unix)]
mod harness;
#[cfg(unix)]
mod registry;
#[cfg(unix)]
mod scenarios;

#[cfg(not(unix))]
fn main() {
    println!("e2e: logos-scaffold is Unix-only; nothing to run");
}

#[cfg(unix)]
fn main() {
    std::process::exit(registry::main());
}
