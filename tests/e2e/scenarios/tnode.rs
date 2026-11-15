//! `.dogfooding/features/test-node-lifecycle.md`

use crate::fixtures;
use crate::harness::{check, Ctx, R};
use crate::scenarios::{toml_section, toml_value};

/// `lgs -q test-node <args…>` from the work dir.
pub fn tn<'a>(cx: &'a Ctx, args: &[&str]) -> crate::harness::Cmd<'a> {
    let mut argv = vec!["-q", "test-node"];
    argv.extend_from_slice(args);
    cx.lgs(&cx.work, &argv)
}

/// TNODE-001 — Report the caller project's pins
pub fn pins(cx: &Ctx) -> R {
    let (p, _) = fixtures::main_created(cx, false)?;
    let root = p.root.to_string_lossy().into_owned();
    let human = tn(cx, &["pins", "--project", &root]).run();
    human
        .ok()?
        .has("lez ref:")?
        .has("lez checkout:")?
        .has("circuits version:")?;
    let v = tn(cx, &["pins", "--project", &root, "--json"])
        .run()
        .json()?;
    for key in [
        "lez_source",
        "lez_source_origin",
        "lez_ref",
        "lez_ref_origin",
        "lez_checkout",
        "checkout_ownership",
        "sequencer_binary",
        "circuits_version",
        "circuits_version_origin",
        "circuits_path",
    ] {
        check!(v[key].is_string(), "`pins --json` has no `{key}`: {v}");
    }
    let toml = p.toml();
    check!(
        v["circuits_version"].as_str()
            == toml_value(&toml_section(&toml, "[circuits]"), "version").as_deref(),
        "pins reports circuits {} but scaffold.toml says otherwise",
        v["circuits_version"]
    );
    check!(
        v["lez_ref"].as_str() == toml_value(&toml_section(&toml, "[repos.lez]"), "pin").as_deref()
            && v["lez_ref_origin"] == "project_config"
            && v["checkout_ownership"] == "managed_cache",
        "pins does not follow the project's LEZ pin: {v}"
    );

    // Step 2: a ref override is reported but nothing is cloned.
    let o = tn(
        cx,
        &[
            "pins",
            "--project",
            &root,
            "--lez-ref",
            "dogfood-unused-ref",
            "--json",
        ],
    )
    .run()
    .json()?;
    let checkout = o["lez_checkout"].as_str().unwrap_or_default();
    check!(
        o["lez_ref"] == "dogfood-unused-ref"
            && o["lez_ref_origin"] == "cli_override"
            && o["checkout_ownership"] == "managed_cache"
            && checkout.ends_with("/repos/lez/dogfood-unused-ref")
            && o["lez_resolved_commit"].is_null(),
        "`--lez-ref` override is not reported as expected: {o}"
    );
    check!(
        !std::path::Path::new(checkout).exists(),
        "`pins` created {checkout}"
    );
    Ok(())
}

use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::harness::{pids_named, Listener, Project};

/// A running test node; stopped (and its state removed) on drop.
pub struct Node<'a> {
    cx: &'a Ctx,
    project: String,
    pub url: String,
    pub dir: String,
    pub json: serde_json::Value,
    preserve: bool,
}

impl<'a> Node<'a> {
    /// `test-node start --project <p> <extra…> --json`.
    pub fn start(cx: &'a Ctx, p: &Project, extra: &[&str]) -> R<Self> {
        let project = p.root.to_string_lossy().into_owned();
        let mut args = vec!["start", "--project", project.as_str()];
        args.extend_from_slice(extra);
        args.push("--json");
        let out = tn(cx, &args).timeout(180).run();
        out.ok()?;
        let json = out.json()?;
        Ok(Self {
            cx,
            url: json["rpc_url"].as_str().unwrap_or_default().to_string(),
            dir: json["state_dir"].as_str().unwrap_or_default().to_string(),
            preserve: extra.contains(&"--preserve-work-dir"),
            project,
            json,
        })
    }

    /// `test-node <args…> --url <this node>`.
    pub fn rpc(&self, args: &[&str]) -> crate::harness::Cmd<'a> {
        let mut argv: Vec<&str> = args.to_vec();
        argv.extend_from_slice(&["--url", &self.url]);
        tn(self.cx, &argv)
    }

    pub fn port(&self) -> u16 {
        self.url
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0)
    }

    fn stop_cmd(&self) -> crate::harness::Out {
        let mut args = vec![
            "stop",
            "--node",
            self.dir.as_str(),
            "--project",
            self.project.as_str(),
        ];
        if self.preserve {
            args.push("--preserve-work-dir");
        }
        tn(self.cx, &args).timeout(60).run()
    }

    /// Stop now and return the command's output.
    pub fn stop(mut self) -> crate::harness::Out {
        let out = self.stop_cmd();
        self.dir.clear();
        out
    }
}

impl Drop for Node<'_> {
    fn drop(&mut self) {
        if !self.dir.is_empty() {
            self.stop_cmd();
        }
    }
}

fn lez_checkout(cx: &Ctx, p: &Project) -> R<PathBuf> {
    let root = p.root.to_string_lossy().into_owned();
    let v = tn(cx, &["pins", "--project", &root, "--json"])
        .run()
        .json()?;
    Ok(PathBuf::from(
        v["lez_checkout"].as_str().unwrap_or_default(),
    ))
}

fn git(cx: &Ctx, dir: &Path, args: &[&str]) -> String {
    cx.cmd(dir, "git", args).run().stdout.trim().to_string()
}

/// TNODE-002 — Prepare and check test-node prerequisites
pub fn prepare_and_doctor(cx: &Ctx) -> R {
    // Step 1: a project that never ran setup, prepare or a node.
    let (fresh, created) = fixtures::create(cx, "tn-fresh", &[]);
    created.ok()?;
    let fresh_root = fresh.root.to_string_lossy().into_owned();
    let cold = tn(cx, &["doctor", "--project", &fresh_root])
        .timeout(300)
        .run();
    cold.fails()?
        .has("FAIL")?
        .has("test-node prepare")?
        .has("test-node doctor reported failing checks")?
        .lacks("panicked")?;

    // Step 2.
    let (p, prepared) = fixtures::tn_prepared(cx, true)?;
    let root = p.root.to_string_lossy().into_owned();
    let pj = prepared.json()?;
    for key in [
        "checkout",
        "checkout_ownership",
        "lez_commit",
        "sequencer_binary",
        "circuits_version",
        "circuits_path",
    ] {
        check!(pj[key].is_string(), "`prepare --json` has no `{key}`: {pj}");
    }
    check!(
        Path::new(pj["sequencer_binary"].as_str().unwrap_or_default()).is_file(),
        "prepare reports a sequencer binary that does not exist: {pj}"
    );
    let doc = tn(cx, &["doctor", "--project", &root, "--json"])
        .timeout(300)
        .run()
        .json()?;
    check!(
        doc["ok"] == true,
        "`test-node doctor --json` after prepare: {doc}"
    );

    // Step 3: the overrides, against a copy of the built checkout so nothing
    // is compiled a second time.
    let pin = pj["lez_commit"].as_str().unwrap_or_default().to_string();
    let src = PathBuf::from(pj["checkout"].as_str().unwrap_or_default());
    let other = cx.empty_dir("tn-cache");
    fs::create_dir_all(other.join("repos/lez")).expect("mkdir");
    let dest = other.join("repos/lez").join(&pin);
    cx.cmd(
        &cx.work,
        "cp",
        &["-a", &src.to_string_lossy(), &dest.to_string_lossy()],
    )
    .timeout(1800)
    .run()
    .ok()?;
    let over = tn(
        cx,
        &[
            "prepare",
            "--project",
            &root,
            "--cache-root",
            &other.to_string_lossy(),
            "--lez-ref",
            &pin,
            "--json",
        ],
    )
    .timeout(fixtures::BUILD_TIMEOUT)
    .run();
    over.ok()?;
    let oj = over.json()?;
    check!(
        oj["checkout"].as_str() == Some(&*dest.to_string_lossy())
            && oj["checkout_ownership"] == "managed_cache"
            && oj["lez_commit"].as_str() == Some(pin.as_str())
            && oj["circuits_path"] == pj["circuits_path"],
        "`prepare --cache-root --lez-ref` reported {oj}"
    );
    check!(
        git(cx, &src, &["status", "--short"]).is_empty(),
        "the project's own checkout was modified by a prepare into another cache root"
    );
    let _ = fs::remove_dir_all(&other);
    Ok(())
}

/// TNODE-003 — Start, inspect and stop an isolated node
pub fn start_inspect_stop(cx: &Ctx) -> R {
    let (p, _) = fixtures::tn_prepared(cx, false)?;
    let root = p.root.to_string_lossy().into_owned();
    let node = Node::start(cx, &p, &[])?;
    for key in ["rpc_url", "state_dir", "config_path", "log_path"] {
        check!(
            node.json[key].is_string(),
            "`start --json` has no `{key}`: {}",
            node.json
        );
    }
    for key in ["pid", "genesis_block_id", "block_height"] {
        check!(
            node.json[key].is_u64(),
            "`start --json` has no numeric `{key}`: {}",
            node.json
        );
    }
    let port = node.port();
    check!(
        port != 0 && port != super::localnet::PORT,
        "the node took port {port}"
    );
    check!(
        node.url == format!("http://127.0.0.1:{port}"),
        "the node's URL is {}",
        node.url
    );
    // Loopback only.
    if let Some(ip) = super::localnet::non_loopback_ip() {
        check!(
            TcpStream::connect_timeout(&SocketAddr::new(ip, port), Duration::from_secs(2)).is_err(),
            "the test node RPC is reachable on {ip}:{port}"
        );
    }
    let name = Path::new(&node.dir)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    check!(
        Path::new(&node.dir) == p.path(".scaffold/test-nodes").join(&name),
        "the node's state dir is not under .scaffold/test-nodes: {}",
        node.dir
    );
    let status = tn(
        cx,
        &["status", "--node", &name, "--project", &root, "--json"],
    )
    .run();
    status.ok()?;
    let sj = status.json()?;
    check!(
        sj["healthy"] == true && sj["rpc_url"] == node.url.as_str(),
        "status of a running node: {sj}"
    );
    let dir = node.dir.clone();
    node.stop().ok()?.has("test node stopped")?;
    check!(
        !Path::new(&dir).exists(),
        "`stop` left the node's state dir behind"
    );
    tn(
        cx,
        &["status", "--node", &name, "--project", &root, "--json"],
    )
    .run()
    .fails()?
    .has("unknown test node")?;
    // The node never writes into the LEZ checkout, and the circuits exist.
    let checkout = lez_checkout(cx, &p)?;
    check!(
        git(cx, &checkout, &["status", "--short"]).is_empty(),
        "the LEZ checkout has new or modified files after a node ran"
    );
    check!(
        p.path(".scaffold/circuits/pol/verification_key.json")
            .is_file(),
        "no circuits install"
    );

    // A relative --project, and a caller-chosen work dir.
    let work = cx.work.join("tn-work");
    let _ = fs::remove_dir_all(&work);
    let work_s = work.to_string_lossy().into_owned();
    let started = cx
        .lgs(
            &p.root,
            &[
                "-q",
                "test-node",
                "start",
                "--project",
                ".",
                "--work-dir",
                &work_s,
                "--json",
            ],
        )
        .timeout(180)
        .run();
    let stop = || {
        cx.lgs(
            &p.root,
            &[
                "-q",
                "test-node",
                "stop",
                "--node",
                &work_s,
                "--project",
                ".",
            ],
        )
        .timeout(60)
        .run()
    };
    if started.code != Some(0) {
        stop();
    }
    started.ok()?;
    let sj = started.json()?;
    let home = fs::read_to_string(sj["config_path"].as_str().unwrap_or_default())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .map(|c| c["home"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    let stopped = stop();
    check!(
        sj["state_dir"].as_str() == Some(work_s.as_str()),
        "`--work-dir` was not used: {sj}"
    );
    check!(
        home.starts_with('/'),
        "with a relative --project the node's `home` is not absolute: {home:?}"
    );
    stopped.ok()?;
    check!(
        !work.exists(),
        "`stop --node <path>` left the work dir behind"
    );
    Ok(())
}

/// TNODE-004 — Wrap a command with a throwaway node
pub fn run_wrapper(cx: &Ctx) -> R {
    let (p, _) = fixtures::tn_prepared(cx, false)?;
    let root = p.root.to_string_lossy().into_owned();
    let before = pids_named("sequencer_servi");
    let run = |extra: &[&str], script: &str| {
        let mut args = vec!["test-node", "run", "--project", root.as_str()];
        args.extend_from_slice(extra);
        args.extend_from_slice(&["--", "sh", "-c", script]);
        // Not `-q`: stdout must be the child's output even with echoes on.
        cx.lgs(&cx.work, &args).timeout(300).run()
    };
    let out = run(
        &["--serial"],
        "echo \"rpc=$LGS_TEST_NODE_RPC_URL port=$LGS_TEST_NODE_PORT dir=$LGS_TEST_NODE_STATE_DIR cfg=$LGS_TEST_NODE_CONFIG_PATH\"",
    );
    out.ok()?;
    let line = out.stdout.trim();
    check!(
        out.stdout.lines().count() == 1
            && line.starts_with("rpc=http://127.0.0.1:")
            && !line.contains("port= ")
            && !line.contains("dir= ")
            && !line.ends_with("cfg="),
        "stdout of `test-node run` is not exactly the child's one line: {:?}",
        out.stdout
    );
    let seven = run(&[], "exit 7");
    check!(
        seven.code == Some(7),
        "the child's exit status 7 came back as {:?}",
        seven.code
    );

    // Three runs, at most two nodes at a time.
    let script = "echo \"$LGS_TEST_NODE_PORT $(date +%s)\"; sleep 6";
    let mut jobs = Vec::new();
    for i in 0..3 {
        let log = cx.out.join(format!("logs/TNODE-004-parallel-{i}.log"));
        let args = [
            "test-node",
            "run",
            "--project",
            root.as_str(),
            "--parallel",
            "2",
            "--",
            "sh",
            "-c",
            script,
        ];
        jobs.push(cx.lgs(&cx.work, &args).spawn(&log)?);
    }
    let mut starts: Vec<(u16, u64)> = Vec::new();
    for job in &mut jobs {
        let code = job.wait(300);
        check!(
            code == Some(0),
            "a parallel `test-node run` ended with {code:?}:\n{}",
            job.log_text()
        );
        let text = job.log_text();
        let found = text.lines().find_map(|l| {
            let mut it = l.split_whitespace();
            Some((
                it.next()?.parse::<u16>().ok()?,
                it.next()?.parse::<u64>().ok()?,
            ))
        });
        match found {
            Some(v) => starts.push(v),
            None => crate::harness::fail!("no `<port> <time>` line from a parallel run:\n{text}"),
        }
    }
    let mut ports: Vec<u16> = starts.iter().map(|s| s.0).collect();
    ports.sort_unstable();
    ports.dedup();
    check!(ports.len() == 3, "parallel nodes shared a port: {starts:?}");
    let mut times: Vec<u64> = starts.iter().map(|s| s.1).collect();
    times.sort_unstable();
    check!(
        times[2] >= times[0] + 5,
        "with --parallel 2 the third node started while two were still running: {starts:?}"
    );
    std::thread::sleep(Duration::from_secs(1));
    let after = pids_named("sequencer_servi");
    check!(
        after.iter().all(|pid| before.contains(pid)),
        "`test-node run` left a sequencer behind: before {before:?}, after {after:?}"
    );
    Ok(())
}

/// TNODE-005 — Start options - a busy port fails fast, block timing overrides apply
pub fn start_options(cx: &Ctx) -> R {
    let (p, _) = fixtures::tn_prepared(cx, false)?;
    let root = p.root.to_string_lossy().into_owned();
    let busy = Listener::bind(0, true)?;
    let port = busy.port.to_string();
    tn(cx, &["start", "--project", &root, "--port", &port])
        .timeout(60)
        .run()
        .fails()?
        .within(15.0)?
        .has(&format!("port {port} is already in use"))?
        .has("--port 0")?;
    drop(busy);
    tn(
        cx,
        &[
            "start",
            "--project",
            &root,
            "--block-create-timeout-ms",
            "0",
        ],
    )
    .timeout(60)
    .run()
    .fails()?
    .has("1..=3600000")?;
    let timing = tn(
        cx,
        &[
            "run", "--project", &root, "--block-create-timeout-ms", "500",
            "--retry-pending-blocks-timeout-ms", "700", "--", "sh", "-c",
            "grep -E 'block_create_timeout|retry_pending_blocks_timeout' \"$LGS_TEST_NODE_CONFIG_PATH\"",
        ],
    )
    .timeout(300)
    .run();
    timing
        .ok()?
        .has("\"block_create_timeout\": \"500ms\"")?
        .has("\"retry_pending_blocks_timeout\": \"700ms\"")?;
    Ok(())
}

/// TNODE-006 — Caller-provided LEZ checkout is never mutated
pub fn caller_checkout(cx: &Ctx) -> R {
    let (p, _) = fixtures::tn_prepared(cx, false)?;
    let root = p.root.to_string_lossy().into_owned();
    let checkout = lez_checkout(cx, &p)?;
    let src = cx.work.join("lez-caller");
    let _ = fs::remove_dir_all(&src);
    let src_s = src.to_string_lossy().into_owned();
    cx.cmd(
        &cx.work,
        "git",
        &["clone", "-q", &checkout.to_string_lossy(), &src_s],
    )
    .timeout(600)
    .run()
    .ok()?;
    let origin = git(cx, &checkout, &["remote", "get-url", "origin"]);
    git(cx, &src, &["remote", "set-url", "origin", &origin]);
    let head = git(cx, &src, &["rev-parse", "HEAD"]);

    let v = tn(
        cx,
        &["pins", "--project", &root, "--lez-source", &src_s, "--json"],
    )
    .run()
    .json()?;
    check!(
        v["checkout_ownership"] == "caller_provided",
        "a local --lez-source is reported as {v}"
    );
    fs::write(src.join("DIRTY"), "x").expect("dirty the checkout");
    tn(cx, &["prepare", "--project", &root, "--lez-source", &src_s])
        .timeout(300)
        .run()
        .fails()?
        .has("uncommitted changes")?
        .has("will not reset")?;
    check!(
        git(cx, &src, &["status", "--short"]) == "?? DIRTY"
            && git(cx, &src, &["rev-parse", "HEAD"]) == head,
        "prepare changed a caller-provided checkout"
    );
    Ok(())
}
