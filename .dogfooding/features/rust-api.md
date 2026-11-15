---
feature: rust-api
prefix: API
surface: api
scenarios:
  - id: API-001
    title: Open a project and read its paths and status
    priority: P1
    provenance: documented
    source: DOGFOODING.md@740e5e9 A1
    depends_on: [CREATE-001]
  - id: API-002
    title: API docs build and doctests pass
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 A1
  - id: API-003
    title: Errors are categorized
    priority: P2
    provenance: documented
    source: DOGFOODING.md@740e5e9 A1
    depends_on: [API-001, BUILD-001]
  - id: API-004
    title: Start, query and stop a test node from the library
    priority: P1
    provenance: documented
    source: src/api/testnode.rs
    depends_on: [API-001, TNODE-002]
retired: []
---

# Rust API

`logos_scaffold::api` exposes the same setup, localnet, wallet, deploy, doctor, report and
test-node capabilities as typed functions with typed results and categorized errors, so tools
can embed scaffold without shelling out. Every operation targets an explicit project root and
never depends on the process cwd. Related: [Test-node lifecycle](test-node-lifecycle.md),
[Localnet](localnet.md).

API-001, API-003 and API-004 share one scratch crate outside the repo, run as
`cargo run -- <abs project>` from `/`:

```toml
[dependencies]
logos-scaffold = { path = "/abs/path/to/scaffold" }   # $REPO_ROOT
```

```rust
use logos_scaffold::api::{testnode::{BlockTimingOverrides, TestNode, TestNodeConfig}, Error, Project};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args().nth(1).expect("project root");
    let p = Project::open(&root)?;
    // scenario steps go here
    Ok(())
}
```

## API-001 — Open a project and read its paths and status

### Expected
`Project::open(root)` loads a project from an explicit root, and `Project::discover(dir)`
walks upward to find `scaffold.toml`. `project.paths()` reports the cache root and which layer
supplied it, the pinned checkouts, vendored binaries, wallet home, localnet state and log, and
the circuits dir, whether or not they exist yet. `project.localnet_status()` returns the same
model as `localnet status --json`.

### Preconditions
CREATE-001, and the scratch crate above.

### Steps
1. ```rust
   let paths = p.paths()?;
   println!("rpc_url={} sequencer_binary={:?}", p.localnet_rpc_url(), paths.sequencer_binary);
   println!("cache_root={:?} source={:?}", paths.cache_root, paths.cache_root_source);
   println!("ready={}", p.localnet_status().ready);
   let d = Project::discover(format!("{root}/src/bin"))?;
   assert_eq!(d.root(), p.root());
   std::fs::create_dir_all(std::env::temp_dir().join("lgs-empty"))?;
   match Project::discover(std::env::temp_dir().join("lgs-empty")) { Err(Error::Config { message }) => println!("discover: {message}"), other => panic!("{other:?}") }
   ```
2. `( cd / && cargo run --manifest-path <crate>/Cargo.toml -- "$P" )`, then compare with
   `"$SCAFFOLD_BIN" test-node pins --project "$P" --json | jq -r .sequencer_binary` and
   `( cd "$P" && "$SCAFFOLD_BIN" localnet status --json | jq .ready )`.

### Success criteria
- The sequencer binary, cache root and readiness match the CLI, and `source` names the layer
  that supplied the cache root (for example `LOGOS_SCAFFOLD_CACHE_ROOT`).
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
1. `cd "$REPO_ROOT" && cargo doc --no-deps && cargo test --doc && ls target/doc/logos_scaffold/api/`

### Success criteria
- Both exit 0, and the `api` module has doc pages.

### Notes
A failing `api` doctest means a documented example drifted from the surface. Fix the example
or the doc; do not delete the test.

## API-003 — Errors are categorized

### Expected
Errors are `api::Error::{Config, BlockTimingValidation, MissingTool, RepoState, Process,
Timeout, Transport, Command, Other}` (`#[non_exhaustive]`). Opening a directory without
`scaffold.toml` gives `Config`. A test-node block timing override outside 1 to 3,600,000 ms
gives `BlockTimingValidation` before anything is spawned. A failed external command gives
`Command(CommandFailed)` with the rendered `command`, `label`, `exit_code`, `stdout`,
`stderr` and `log_path`; `stdout` and `stderr` are empty when the output was streamed to the
terminal rather than captured.

### Preconditions
API-001's crate, and the CREATE-001 project after BUILD-001 (so the wallet binary exists).

### Steps
1. ```rust
   match Project::open("/tmp") { Err(e @ Error::Config { .. }) => println!("ok: {e}"), other => panic!("{other:?}") }
   match p.wallet_passthrough(&["no-such-subcommand".to_string()]) { Err(Error::Command(f)) => println!("cmd={} exit={:?} label={}", f.command, f.exit_code, f.label), other => panic!("{other:?}") }
   match TestNode::start_with_block_timing(&p, &TestNodeConfig::default(), BlockTimingOverrides::new().with_block_create_timeout_ms(0)) { Err(Error::BlockTimingValidation { source }) => println!("timing: {}", source.field()), other => panic!("{other:?}") }
   ```

### Success criteria
- It prints `ok:` with a readable message; a `cmd=` that ends in `wallet no-such-subcommand`
  with a non-zero `exit` and the label `wallet passthrough command`; and `timing:
  block_create_timeout_ms`. No node is started.

### Notes
The library streams `$ <command>` echoes and the child's own output to the host process;
`LOGOS_SCAFFOLD_QUIET=1` is the only way to silence the echoes.

## API-004 — Start, query and stop a test node from the library

### Expected
`TestNode::start(&project, &TestNodeConfig::default())` returns once an isolated node is
healthy, on its own port. `node.client()` is a `TestNodeClient` for it, `node.status()`
reports it healthy and running, and `node.stop()` stops it and removes its runtime
directory. Dropping the handle without `detach()` cleans up too.

### Preconditions
API-001's crate and TNODE-002 (sequencer built, r0vm provisioned) for the same project.

### Steps
1. ```rust
   let node = TestNode::start(&p, &TestNodeConfig::default())?;
   let dir = node.state_dir().to_path_buf();
   let st = node.status();
   println!("url={} head={} healthy={} running={} dir_exists={}", node.rpc_url(), node.client().last_block_id()?, st.healthy, st.running, dir.exists());
   assert_ne!(node.rpc_url(), p.localnet_rpc_url());
   node.stop()?;
   println!("after stop: dir_exists={}", dir.exists());
   ```

### Success criteria
- It prints a `127.0.0.1` URL on a port other than the localnet's, `head` ≥ 1, `healthy=true
  running=true dir_exists=true`, then `after stop: dir_exists=false`.
- `pgrep -x sequencer_servi` finds no leftover node afterwards.
