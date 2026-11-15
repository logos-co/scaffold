//! `.dogfooding/features/run-pipeline.md`

use std::fs;

use crate::fixtures::{self, BUILD_TIMEOUT};
use crate::harness::{check, tail, Ctx, Out, Project, Restore, R};
use crate::scenarios::localnet::PORT;

const HEADERS: [&str; 5] = [
    "[1/5] Building",
    "[2/5] Building IDL",
    "[3/5] Ensuring localnet",
    "[4/5] Topping up wallet",
    "[5/5] Deploy",
];

fn in_order(out: &Out, needles: &[&str]) -> R {
    let text = out.all();
    let mut from = 0;
    for n in needles {
        match text[from..].find(n) {
            Some(i) => from += i + n.len(),
            None => crate::harness::fail!(
                "expected {n:?} (in this order: {needles:?}) in:\n{}",
                tail(&text, 2500)
            ),
        }
    }
    Ok(())
}

fn run(cx: &Ctx, p: &Project, args: &[&str]) -> Out {
    let mut argv = vec!["run"];
    argv.extend_from_slice(args);
    cx.lgs(&p.root, &argv).timeout(BUILD_TIMEOUT).run()
}

/// RUN-001 — Bare run takes a fresh project to deployed programs
pub fn bare_run(cx: &Ctx) -> R {
    let (p, out) = fixtures::run_project(cx, true)?;
    in_order(&out, &HEADERS)?;
    in_order(
        &out,
        &[
            "localnet ready (sequencer pid=",
            "[5/5] Deploying programs",
            "program_id:",
            "Deployed programs:",
            "Sequencer:",
        ],
    )?;
    check!(
        p.default_address().is_some(),
        "`run` left the project without a default wallet"
    );
    // From a subdirectory it anchors at the project root.
    let sub = cx
        .lgs(&p.path("src"), &["run", "--no-post-deploy"])
        .timeout(BUILD_TIMEOUT)
        .run();
    sub.ok()?
        .lacks("must be run inside a logos-scaffold project")?;
    in_order(&sub, &HEADERS)
}

/// RUN-002 — Re-run reuses localnet, skips an unchanged deploy and names the wallet's sequencer
pub fn rerun(cx: &Ctx) -> R {
    let (p, _) = fixtures::run_project(cx, false)?;
    let before = cx.localnet_status(&p)?["tracked_pid"].clone();
    let out = run(cx, &p, &["--no-post-deploy"]);
    out.ok()?
        .has("localnet already running (sequencer pid=")?
        .has("Deploy skipped (guest binaries + IDL + config + sequencer unchanged")?;
    check!(
        cx.localnet_status(&p)?["tracked_pid"] == before,
        "a re-run restarted the sequencer"
    );
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(p.wallet_config()).unwrap_or_default())
            .unwrap_or_default();
    let target = config["sequencers"][0]["sequencer_addr"]
        .as_str()
        .or(config["sequencer_addr"].as_str())
        .unwrap_or_default()
        .to_string();
    let line = out
        .stdout
        .lines()
        .find(|l| l.starts_with("Sequencer:"))
        .unwrap_or("")
        .to_string();
    check!(
        !target.is_empty() && line.trim_start_matches("Sequencer:").trim() == target,
        "summary says {line:?} but the wallet targets {target:?}"
    );
    Ok(())
}

/// RUN-003 — Post-deploy hooks get the documented environment and can be overridden
pub fn hooks(cx: &Ctx) -> R {
    let (p, _) = fixtures::run_project(cx, false)?;
    let _restore = Restore::of(&p.path("scaffold.toml"));
    p.append_toml(
        r#"
[run]
post_deploy = [
  "echo cwd=$(pwd)",
  "echo seq=$SEQUENCER_URL idl=$SCAFFOLD_IDL_DIR root=$SCAFFOLD_PROJECT_ROOT",
  "echo nssa=$NSSA_WALLET_HOME_DIR lee=$LEE_WALLET_HOME_DIR",
  "echo topup=$SCAFFOLD_TOPUP_SKIPPED deploy=$SCAFFOLD_DEPLOY_SKIPPED",
  "echo pid=${SCAFFOLD_PROGRAM_ID:-unset} bin=${SCAFFOLD_GUEST_BIN:-unset}",
]
"#,
    );
    let out = run(cx, &p, &[]);
    out.ok()?
        .has("[6/6] Running 5 post-deploy hook(s)")?
        .has("===> post_deploy[1/5]:")?
        .has("<=== post_deploy[5/5] OK")?;
    let root = p.root.to_string_lossy().into_owned();
    let wallet = format!("{root}/.scaffold/wallet");
    let deploy_skipped = out.all().contains("Deploy skipped");
    for line in [
        format!("cwd={root}"),
        format!("seq=http://127.0.0.1:{PORT} idl={root}/"),
        format!("root={root}"),
        format!("nssa={wallet} lee={wallet}"),
        format!("topup=0 deploy={}", u8::from(deploy_skipped)),
        "pid=unset bin=unset".to_string(),
    ] {
        out.has(&line)?;
    }

    // Command-line hooks replace the configured ones; a failing hook aborts.
    let over = run(
        cx,
        &p,
        &["--post-deploy", "echo override", "--post-deploy", "exit 3"],
    );
    over.fails()?
        .has("Running 2 post-deploy hook(s)")?
        .has("override")?
        .has("post-deploy hook exited with status 3")?
        .lacks("cwd=")?;
    // No hooks: the summary instead.
    let none = run(cx, &p, &["--no-post-deploy"]);
    none.ok()?
        .has("Deployed programs:")?
        .has("Sequencer:")?
        .lacks("post-deploy hook")?;
    // Both flags: an argument error, before any work.
    run(cx, &p, &["--post-deploy", "x", "--no-post-deploy"])
        .fails()?
        .within(10.0)?
        .has("cannot be used with")?
        .lacks("[1/")?;
    Ok(())
}

/// RUN-005 — Run profiles skip deploy or topup
pub fn profiles(cx: &Ctx) -> R {
    let (p, _) = fixtures::run_project(cx, false)?;
    let _restore = Restore::of(&p.path("scaffold.toml"));
    let base = p.toml();
    let profiles = r#"
[run.profiles.self-deploy]
deploy = false
post_deploy = ["echo topup=$SCAFFOLD_TOPUP_SKIPPED deploy=$SCAFFOLD_DEPLOY_SKIPPED"]

[run.profiles.self-fund]
topup = false
post_deploy = ["echo topup=$SCAFFOLD_TOPUP_SKIPPED deploy=$SCAFFOLD_DEPLOY_SKIPPED"]
"#;
    let write = |default: Option<&str>| {
        let run_table = match default {
            Some(name) => format!("\n[run]\ndefault_profile = \"{name}\"\n"),
            None => String::new(),
        };
        fs::write(
            p.path("scaffold.toml"),
            format!("{base}{run_table}{profiles}"),
        )
        .expect("write scaffold.toml");
    };

    write(None);
    let out = run(cx, &p, &["--profile", "self-deploy"]);
    out.ok()?
        .has("[4/6] Topping up wallet")?
        .has("[5/6] Deploy skipped (`deploy = false` in the run profile")?
        .has("topup=0 deploy=1")?;

    // No --profile: the default profile applies.
    write(Some("self-fund"));
    let out = run(cx, &p, &[]);
    out.ok()?
        .has("[4/6] Topup skipped (`topup = false` in the run profile")?
        .has("[5/6] Deploy")?
        .has("topup=1 deploy=")?
        .lacks("`deploy = false`")?;

    run(cx, &p, &["--profile", "nope"])
        .fails()?
        .has("scaffold.toml has no [run.profiles.nope] section. Known profiles: [self-deploy, self-fund]")?;
    write(Some("nope"));
    run(cx, &p, &[])
        .fails()?
        .has("[run].default_profile = \"nope\" but no [run.profiles.nope] section")?;
    Ok(())
}

/// RUN-007 — Reset before the run
pub fn reset(cx: &Ctx) -> R {
    let (p, _) = fixtures::run_project(cx, false)?;
    run(cx, &p, &["--reset", "--no-reset"])
        .fails()?
        .within(10.0)?
        .has("cannot be used with")?;
    // The chain is shared with the main project (same cache root and pin):
    // what its fixtures put on chain is gone after this.
    cx.forget("main: wallet topup");
    cx.forget("main: deploy");
    let out = run(cx, &p, &["--reset", "--no-post-deploy"]);
    out.ok()?
        .has("Resetting localnet")?
        .has("[5/5] Deploying programs")?
        .has("Failed: 0")?
        .lacks("Deploy skipped")?;
    Ok(())
}

/// RUN-008 — Watch mode re-runs on matching changes
pub fn watch(cx: &Ctx) -> R {
    let (p, _) = fixtures::run_project(cx, false)?;
    let _restore = Restore::of(&p.path("scaffold.toml"));
    p.append_toml("\n[run.watch]\nexclude = [\"**/*.md\"]\n");
    run(cx, &p, &["--watch-debounce-ms", "100"])
        .fails()?
        .within(10.0)?
        .has("--watch")?;

    // A program no other program calls. Editing a tail-call callee
    // (`hello_world`) changes its id, the build then rewrites the id embedded
    // in the caller's source, and that write is one more change to react to.
    let guest = p.path("methods/guest/src/bin/hello_world_with_move_function.rs");
    let _guest_restore = Restore::of(&guest);
    let mut watcher = cx
        .lgs(&p.root, &["run", "--watch", "--no-post-deploy"])
        .spawn(&cx.out.join("logs/RUN-008-watch.log"))?;
    let pipelines = |log: &str| log.lines().filter(|l| l.starts_with("[1/")).count();
    let watching = |log: &str| log.matches("===> watching").count();
    // Each finished pipeline ends with the deploy summary (no hooks run).
    let finished = |log: &str| log.lines().filter(|l| l.starts_with("Sequencer:")).count();
    const WAIT: u64 = 30 * 60;
    // `===> watching` is printed once, when the first pipeline is done.
    let first = cx.poll(BUILD_TIMEOUT, 1000, || {
        (watching(&watcher.log_text()) >= 1 || !watcher.running()).then_some(())
    });
    check!(
        first.is_some() && watcher.running(),
        "`run --watch` did not reach the watching state:\n{}",
        tail(&watcher.log_text(), 2000)
    );
    check!(
        pipelines(&watcher.log_text()) == 1,
        "the first watch cycle ran more than one pipeline"
    );

    // An excluded file changes nothing.
    fs::write(
        p.path("README.md"),
        format!("{}\n<!-- e2e -->\n", p.read("README.md")),
    )
    .expect("touch README");
    std::thread::sleep(std::time::Duration::from_secs(6));
    check!(
        pipelines(&watcher.log_text()) == 1,
        "a change to an excluded file re-ran the pipeline"
    );
    // A guest source change triggers exactly one re-run.
    let src = fs::read_to_string(&guest).unwrap_or_default();
    fs::write(&guest, format!("// e2e watch edit\n{src}")).expect("edit guest");
    let second = cx.poll(WAIT, 1000, || {
        (finished(&watcher.log_text()) >= 2 || !watcher.running()).then_some(())
    });
    // Long enough for a second, unwanted re-run to start.
    std::thread::sleep(std::time::Duration::from_secs(6));
    let log = watcher.log_text();
    check!(
        second.is_some() && watcher.running(),
        "the watcher did not finish a re-run after a guest edit:\n{}",
        tail(&log, 2000)
    );
    check!(
        pipelines(&log) == 2 && log.matches("change detected, re-running pipeline").count() == 1,
        "expected exactly one re-run after the guest edit:\n{}",
        tail(&log, 2000)
    );
    Ok(())
}

/// RUN-010 — Hooks get the program id and guest path on a single-program project
pub fn single_program(cx: &Ctx) -> R {
    let (src, _) = fixtures::run_project(cx, false)?;
    // A copy reduced to one program. The build output comes along, so only
    // what changed is rebuilt.
    cx.localnet_down();
    let single = Project {
        root: cx.work.join("single"),
    };
    let _ = fs::remove_dir_all(&single.root);
    cx.cmd(
        &cx.work,
        "cp",
        &[
            "-a",
            &src.root.to_string_lossy(),
            &single.root.to_string_lossy(),
        ],
    )
    .timeout(1800)
    .run()
    .ok()?;
    for stale in ["localnet.state", "run_deploy.json"] {
        let _ = fs::remove_file(single.path(".scaffold/state").join(stale));
    }
    for f in crate::scenarios::ls(&single.path("methods/guest/src/bin")) {
        if f != "hello_world.rs" {
            let _ = fs::remove_file(single.path("methods/guest/src/bin").join(f));
        }
    }
    // The other clients import the removed programs' ELF constants.
    for f in crate::scenarios::ls(&single.path("src/bin")) {
        if f != "run_hello_world.rs" && f != "run_hello_world_private.rs" {
            let _ = fs::remove_file(single.path("src/bin").join(f));
        }
    }
    let out = run(
        cx,
        &single,
        &[
            "--post-deploy",
            "echo pid=${SCAFFOLD_PROGRAM_ID:-unset} bin=${SCAFFOLD_GUEST_BIN:-unset}",
        ],
    );
    if out.code == Some(0) {
        cx.localnet_mark(&single, true);
    }
    out.ok()?;
    let line = out
        .stdout
        .lines()
        .find(|l| l.starts_with("pid="))
        .unwrap_or("")
        .to_string();
    let pid = line
        .trim_start_matches("pid=")
        .split(' ')
        .next()
        .unwrap_or("");
    let bin = line.split(" bin=").nth(1).unwrap_or("");
    check!(
        pid.len() == 64 && pid.chars().all(|c| c.is_ascii_hexdigit()),
        "SCAFFOLD_PROGRAM_ID is not a 64-hex image id: {line:?}"
    );
    check!(
        bin.starts_with('/')
            && bin.ends_with("hello_world.bin")
            && std::path::Path::new(bin).is_file(),
        "SCAFFOLD_GUEST_BIN is not an absolute path to an existing hello_world.bin: {line:?}"
    );
    let v = cx
        .lgs(&single.root, &["deploy", "hello_world", "--json"])
        .timeout(600)
        .run()
        .json()?;
    check!(
        v["deploys"][0]["program_id"] == pid,
        "the hook saw program id {pid} but deploy reports {}",
        v["deploys"][0]["program_id"]
    );
    // The copy's build tree is large and nothing else needs it.
    cx.localnet_down();
    let _ = fs::remove_dir_all(single.path("target"));
    Ok(())
}
