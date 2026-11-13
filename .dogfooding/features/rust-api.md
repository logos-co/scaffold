---
feature: rust-api
prefix: API
surface: api
scenarios:
  - id: API-001
    title: Open a project and read its paths and status
    priority: P1
    provenance: documented
    source: DOGFOODING.md A1
    depends_on: [CREATE-001]
  - id: API-002
    title: API docs build and doctests pass
    priority: P2
    provenance: documented
    source: DOGFOODING.md A1
  - id: API-003
    title: Errors are categorized
    priority: P2
    provenance: documented
    source: DOGFOODING.md A1
    depends_on: [API-001, BUILD-001]
retired: []
---

# Rust API

`logos_scaffold::api` exposes the same setup, localnet, wallet, deploy, doctor, report and
test-node capabilities as typed functions with typed results and categorized errors, so tools
can embed scaffold without shelling out. Every operation targets an explicit project root and
never depends on the process cwd. Related: [Test-node lifecycle](test-node-lifecycle.md),
[Localnet](localnet.md).

## API-001 — Open a project and read its paths and status

### Expected
`Project::open(root)` loads a project from an explicit root, and `Project::discover(dir)`
walks upward to find `scaffold.toml`. `project.paths()` reports the cache root and which layer
supplied it, the pinned checkouts, vendored binaries, wallet home, localnet state and log, and
the circuits dir, whether or not they exist yet. `project.localnet_status()` returns the same
model as `localnet status --json`.

### Preconditions
CREATE-001 at `$SCRATCH_ROOT/dogfood-default`. A scratch crate with a path dependency on the
repo.

### Steps
1. In the scratch crate's `main.rs`: `let p = Project::open("<abs project>")?;` then print
   `p.localnet_rpc_url()`, `p.paths()?.sequencer_binary`, `paths.cache_root` /
   `paths.cache_root_source` and `p.localnet_status().ready`.
2. Run it with `cargo run` from a different directory (for example `/`).
3. Add `let d = Project::discover("<abs project>/src/bin")?; assert_eq!(d.root(), p.root());`
   and `match Project::discover(std::env::temp_dir().join("lgs-empty")) { Err(Error::Config { message }) => println!("discover: {message}"), other => panic!("{other:?}") }`
   (create that empty dir first), then `cargo run` again.

### Success criteria
- Paths resolve under the project or cache, matching `test-node pins` and
  `localnet status --json`.
- The output is the same whatever directory you run it from.
- `discover` from a subdirectory finds the same root as `open`. From a directory with no
  `scaffold.toml` above it, it returns `Config` with
  `no scaffold.toml found in <dir> or any parent directory`.

## API-002 — API docs build and doctests pass

### Expected
`cargo doc --no-deps` builds rustdoc for `api`. `cargo test --doc` passes the `api` doctests
for setup, the localnet lifecycle, wallet topup, deploy, doctor, report and test-node.

### Preconditions
The repo checkout.

### Steps
1. `cd "$REPO_ROOT" && cargo doc --no-deps && cargo test --doc`

### Success criteria
- Both exit 0, and the `api` module has doc pages.

## API-003 — Errors are categorized

### Expected
Errors are `api::Error::{Config, BlockTimingValidation, MissingTool, RepoState, Process,
Timeout, Transport, Command, Other}` (`#[non_exhaustive]`). Opening a directory without
`scaffold.toml` gives `Config`. A test-node block timing override outside 1 to 3,600,000 ms
gives `BlockTimingValidation`. A failed external command gives `Command(CommandFailed)` with
the rendered `command`, `label`, `exit_code`, `stdout`, `stderr` and `log_path`. `stdout` and
`stderr` are empty when the output was streamed to the terminal rather than captured.

### Preconditions
The scratch crate from API-001, and the CREATE-001 project after BUILD-001 (so the wallet
binary and wallet home exist).

### Steps
1. `match Project::open("/tmp") { Err(e @ Error::Config { .. }) => println!("ok: {e}"), other => panic!("{other:?}") }`
2. `match p.wallet_passthrough(&["no-such-subcommand".to_string()]) { Err(Error::Command(f)) => println!("cmd={} exit={:?} label={}", f.command, f.exit_code, f.label), other => panic!("{other:?}") }`
3. With `use logos_scaffold::api::testnode::{BlockTimingOverrides, TestNode, TestNodeConfig};`:
   `match TestNode::start_with_block_timing(&p, &TestNodeConfig::default(), BlockTimingOverrides::new().with_block_create_timeout_ms(0)) { Err(Error::BlockTimingValidation { source }) => println!("timing: {}", source.field()), other => panic!("{other:?}") }`

### Success criteria
- Step 1 prints `ok:` with a readable message.
- Step 2 prints a `command` that ends in `wallet no-such-subcommand`, a non-zero
  `exit_code` and the label `wallet passthrough command`.
- Step 3 prints `timing: block_create_timeout_ms` and starts no node.
