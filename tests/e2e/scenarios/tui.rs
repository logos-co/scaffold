//! `.dogfooding/features/tui.md`: the dashboard, driven through tmux.

use std::time::Duration;

use crate::fixtures::{self, BUILD_TIMEOUT};
use crate::harness::{check, pids_named, port_free, skip, Ctx, Listener, Project, R};
use crate::scenarios::build::programs;
use crate::scenarios::localnet::PORT;

/// A private tmux server, so the dashboard gets this suite's environment.
const SOCKET: &str = "lgs-e2e";
const SESSION: &str = "dash";

#[derive(Debug, Clone, PartialEq)]
enum Job {
    Idle,
    /// `<label> · <n>s`
    Running(String),
    /// `✓ <label>: exit 0` / `✗ <label>: …`
    Done(bool, String),
}

/// A `lgs tui` session in a detached tmux window; the server dies on drop.
struct Tui<'a> {
    cx: &'a Ctx,
}

impl<'a> Tui<'a> {
    fn tmux(cx: &Ctx, args: &[&str]) -> crate::harness::Out {
        let mut argv = vec!["-L", SOCKET];
        argv.extend_from_slice(args);
        cx.cmd(&cx.work, "tmux", &argv).timeout(20).run()
    }

    /// Start the dashboard for `p` and return at once (the splash is showing).
    fn launch(cx: &'a Ctx, p: &Project) -> R<Self> {
        if !cx.caps.tmux {
            skip!("needs tmux to drive and capture the dashboard");
        }
        Self::tmux(cx, &["kill-server"]);
        let lgs = cx.lgs.to_string_lossy().into_owned();
        let root = p.root.to_string_lossy().into_owned();
        let started = Self::tmux(
            cx,
            &[
                "new-session",
                "-d",
                "-s",
                SESSION,
                "-x",
                "120",
                "-y",
                "32",
                "-c",
                &root,
                &lgs,
                "tui",
            ],
        );
        if started.code != Some(0) {
            crate::harness::blocked!("tmux could not start the dashboard: {}", started.all());
        }
        Ok(Self { cx })
    }

    /// Start the dashboard and wait until it has drawn its panes.
    fn start(cx: &'a Ctx, p: &Project) -> R<Self> {
        let tui = Self::launch(cx, p)?;
        tui.wait("the dashboard panes", 30, |s| {
            s.contains("┌ Status") && s.contains("│Job")
        })?;
        Ok(tui)
    }

    fn screen(&self) -> String {
        Self::tmux(self.cx, &["capture-pane", "-p", "-t", SESSION]).stdout
    }

    fn key(&self, key: &str) {
        Self::tmux(self.cx, &["send-keys", "-t", SESSION, key]);
    }

    fn alive(&self) -> bool {
        Self::tmux(self.cx, &["has-session", "-t", SESSION]).code == Some(0)
    }

    fn resize(&self, cols: u16, rows: u16) {
        Self::tmux(
            self.cx,
            &[
                "resize-window",
                "-t",
                SESSION,
                "-x",
                &cols.to_string(),
                "-y",
                &rows.to_string(),
            ],
        );
        std::thread::sleep(Duration::from_millis(700));
    }

    /// Poll the screen until `ok` holds; fail with the last screen otherwise.
    fn wait(&self, what: &str, secs: u64, ok: impl Fn(&str) -> bool) -> R<String> {
        let mut last = String::new();
        let found = self.cx.poll(secs, 200, || {
            last = self.screen();
            ok(&last).then_some(())
        });
        if found.is_none() {
            self.cx
                .log(&format!("--- screen while waiting for {what}:\n{last}"));
            crate::harness::fail!(
                "the dashboard never showed {what} within {secs}s. Last screen:\n{last}"
            );
        }
        Ok(last)
    }

    /// The text of the Status pane, unwrapped into one line.
    fn status(screen: &str) -> String {
        let lines: Vec<Vec<char>> = screen.lines().map(|l| l.chars().collect()).collect();
        let Some(top) = lines
            .iter()
            .position(|l| l.iter().collect::<String>().starts_with("┌ Status"))
        else {
            return String::new();
        };
        let width = lines[top].iter().position(|&c| c == '┐').unwrap_or(0);
        let mut text = String::new();
        for l in &lines[top + 1..] {
            if l.first() == Some(&'└') || width < 2 || l.len() <= width {
                break;
            }
            let inner: String = l[1..width].iter().collect();
            text.push_str(inner.trim());
            text.push(' ');
        }
        text
    }

    fn job(screen: &str) -> Job {
        let status = Self::status(screen);
        let Some(text) = status.split("Job ").nth(1).map(str::trim) else {
            return Job::Idle;
        };
        if let Some(rest) = text.strip_prefix('✓') {
            Job::Done(true, rest.trim().to_string())
        } else if let Some(rest) = text.strip_prefix('✗') {
            Job::Done(false, rest.trim().to_string())
        } else if text.starts_with("idle") {
            Job::Idle
        } else {
            Job::Running(text.to_string())
        }
    }

    /// Press `keys` (which start a job), wait for that job to end and return
    /// whether it succeeded, its summary and the final screen. Never use it
    /// for two jobs in a row with the same label: the second summary would be
    /// indistinguishable from the first.
    fn run_job(&self, keys: &[&str], secs: u64) -> R<(bool, String, String)> {
        let before = Self::job(&self.screen());
        for k in keys {
            self.key(k);
        }
        let mut last = String::new();
        let done = self.cx.poll(secs, 200, || {
            last = self.screen();
            match Self::job(&last) {
                Job::Done(ok, summary) if Job::Done(ok, summary.clone()) != before => {
                    Some((ok, summary))
                }
                _ => None,
            }
        });
        match done {
            Some((ok, summary)) => {
                self.cx.log(&format!(
                    "tui: {keys:?} -> {} {summary}",
                    if ok { "✓" } else { "✗" }
                ));
                // Let the panes catch up with the finished job.
                std::thread::sleep(Duration::from_millis(1500));
                Ok((ok, summary, self.screen()))
            }
            None => crate::harness::fail!(
                "the job started by {keys:?} did not finish within {secs}s. Last screen:\n{last}"
            ),
        }
    }

    /// Like [`Self::run_job`], for a job expected to end with `<label>: exit 0`.
    fn run_ok(&self, keys: &[&str], label: &str, secs: u64) -> R<String> {
        let (ok, summary, screen) = self.run_job(keys, secs)?;
        check!(
            ok && summary == format!("lgs {label}: exit 0"),
            "{keys:?} should run `lgs {label}` successfully; the Job line says {summary:?}:\n{screen}"
        );
        check!(
            screen.contains(&format!("── lgs {label}: exit 0 ──")),
            "the Output pane does not end with `lgs {label}`'s exit line:\n{screen}"
        );
        Ok(screen)
    }
}

impl Drop for Tui<'_> {
    fn drop(&mut self) {
        Self::tmux(self.cx, &["kill-server"]);
    }
}

fn deployed_marks(screen: &str) -> usize {
    screen.matches("✓ deployed").count()
}

fn block_height(screen: &str) -> Option<u64> {
    Tui::status(screen)
        .split("block ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn stop_localnet(cx: &Ctx, p: &Project) -> R {
    cx.localnet_down();
    cx.lgs(&p.root, &["localnet", "stop"]).run();
    check!(
        port_free(PORT),
        "port {PORT} is still busy after stopping localnet"
    );
    Ok(())
}

/// TUI-001 — Dashboard status agrees with the CLI
pub fn status_agrees(cx: &Ctx) -> R {
    let (p, _) = fixtures::run_project(cx, false)?;
    stop_localnet(cx, &p)?;
    let tui = Tui::start(cx, &p)?;
    let screen = tui.wait("the stopped localnet", 20, |s| {
        Tui::status(s).contains("stopped")
    })?;
    let status = Tui::status(&screen);
    let addr = p.default_address().unwrap_or_default();
    check!(addr.len() > 20, "the project has no default wallet address");
    check!(
        status.contains(&format!("Public/{}", &addr[..10]))
            && status.contains(&addr[addr.len() - 10..]),
        "the Wallet line does not show the default address {addr}: {status}"
    );
    for program in programs(&p) {
        check!(
            screen.contains(&program),
            "the Programs pane does not list {program}:\n{screen}"
        );
    }
    check!(
        screen.contains("Press r for `lgs run`"),
        "no empty-Output hint naming `r`:\n{screen}"
    );
    check!(
        deployed_marks(&screen) == 0 || cx.localnet_status(&p)?["ready"] == false,
        "unexpected state"
    );
    Ok(())
}

/// TUI-002 — Each key runs the command it names, and run marks programs deployed
pub fn keys_run_commands(cx: &Ctx) -> R {
    let (p, _) = fixtures::run_project(cx, false)?;
    stop_localnet(cx, &p)?;
    // The chain is shared with the main project; the reset below wipes it.
    cx.forget("main: wallet topup");
    cx.forget("main: deploy");
    let n = programs(&p).len();
    let tui = Tui::start(cx, &p)?;
    let cli_ready = || -> R<bool> { Ok(cx.localnet_status(&p)?["ready"] == true) };

    // s: localnet start, and the dashboard agrees with the CLI.
    let screen = tui.run_ok(&["s"], "localnet start", 240)?;
    cx.localnet_mark(&p, true);
    check!(
        screen.contains("$ lgs localnet start"),
        "the job is not framed by `$ lgs localnet start`:\n{screen}"
    );
    check!(
        cli_ready()?,
        "the dashboard started localnet but the CLI says it is not ready"
    );
    tui.wait("a running localnet", 20, |s| {
        Tui::status(s).contains("running")
    })?;
    // t and c.
    tui.run_ok(&["t"], "wallet topup", 600)?;
    let (_, summary, _) = tui.run_job(&["c"], 300)?;
    check!(
        summary.starts_with("lgs doctor: exit"),
        "`c` ran {summary:?}, expected `lgs doctor`"
    );

    // r: every program is built and marked deployed; blocks advance.
    let screen = tui.run_ok(&["r"], "run", BUILD_TIMEOUT)?;
    let marked = tui
        .wait("the deployed marks", 20, |s| deployed_marks(s) == n)
        .unwrap_or(screen);
    check!(
        deployed_marks(&marked) == n,
        "after `r`, {} of {n} programs are marked deployed:\n{marked}",
        deployed_marks(&marked)
    );
    check!(
        marked.contains("release"),
        "the Programs pane does not show the build kind:\n{marked}"
    );
    let h1 = block_height(&marked);
    let grew = cx.poll(90, 1000, || {
        (block_height(&tui.screen()) > h1).then_some(())
    });
    check!(
        h1.is_some() && grew.is_some(),
        "the block height does not advance (was {h1:?})"
    );

    // j d: deploy the second program. Only `run` records deploys.
    tui.run_ok(&["j", "d"], "deploy hello_world_with_authorization", 600)?;

    // s twice: stop, then start again.
    tui.run_ok(&["s"], "localnet stop", 120)?;
    check!(
        !cli_ready()?,
        "the dashboard stopped localnet but the CLI still says ready"
    );
    tui.wait("the stopped localnet", 20, |s| {
        Tui::status(s).contains("stopped")
    })?;
    tui.run_ok(&["s"], "localnet start", 240)?;
    check!(cli_ready()?, "the second `s` did not start localnet again");

    // A second r skips the deploy and keeps the marks.
    let screen = tui.run_ok(&["r"], "run", BUILD_TIMEOUT)?;
    check!(
        deployed_marks(&screen) == n,
        "the marks did not survive a second `r`:\n{screen}"
    );

    // R + y: a new chain. Marks clear, the wallet stays.
    let addr = p.default_address().unwrap_or_default();
    tui.key("R");
    tui.wait("the reset confirmation", 10, |s| s.contains(" Confirm "))?;
    let screen = tui.run_ok(&["y"], "localnet reset --yes", 600)?;
    let cleared = tui
        .wait("cleared marks", 30, |s| deployed_marks(s) == 0)
        .unwrap_or(screen);
    check!(
        deployed_marks(&cleared) == 0,
        "the deployed marks survived a chain reset:\n{cleared}"
    );
    check!(
        Tui::status(&cleared).contains(&format!("Public/{}", &addr[..10])),
        "the Wallet line changed after a reset"
    );
    // r deploys for real again and the marks return.
    let screen = tui.run_ok(&["r"], "run", BUILD_TIMEOUT)?;
    let back = tui
        .wait("the deployed marks", 20, |s| deployed_marks(s) == n)
        .unwrap_or(screen);
    check!(
        deployed_marks(&back) == n,
        "the marks did not return after the post-reset `r`:\n{back}"
    );
    Ok(())
}

/// TUI-004 — One job at a time, x stops its process tree, destructive keys ask first
pub fn one_job_and_confirmations(cx: &Ctx) -> R {
    let (p, _) = fixtures::run_project(cx, false)?;
    cx.localnet_up(&p)?;
    let pid = cx.localnet_status(&p)?["tracked_pid"].clone();
    let cargo_before = pids_named("cargo");
    let tui = Tui::start(cx, &p)?;
    let confirm =
        |s: &str| s.contains(" Confirm ") && s.contains(" y / Enter yes · any other key no ");

    // R asks first; any other key cancels.
    tui.key("R");
    tui.wait("the reset confirmation", 10, confirm)?;
    tui.key("n");
    tui.wait("the confirmation to close", 10, |s| {
        !s.contains(" Confirm ")
    })?;
    check!(
        cx.localnet_status(&p)?["tracked_pid"] == pid,
        "a cancelled reset restarted the sequencer"
    );

    // A long job (`w`: run --watch never ends by itself; the scenario's `b`
    // can finish before the keys below are pressed).
    tui.key("w");
    tui.wait(
        "the running job",
        30,
        |s| matches!(Tui::job(s), Job::Running(t) if t.contains("lgs run --watch")),
    )?;
    cx.note("`w` (run --watch) stands in for `b` as the long-running job");
    // Another action key starts nothing.
    tui.key("c");
    let screen = tui.wait("the still-running notice", 10, |s| {
        s.contains("is still running; x stops it")
    })?;
    check!(
        !screen.contains("$ lgs doctor"),
        "a second job started while one was running:\n{screen}"
    );
    // q and Esc ask while a job runs.
    for key in ["q", "Escape"] {
        tui.key(key);
        tui.wait("the quit confirmation", 10, confirm)?;
        tui.key("n");
        tui.wait("the confirmation to close", 10, |s| {
            !s.contains(" Confirm ")
        })?;
        check!(tui.alive(), "the dashboard quit after a cancelled `{key}`");
        check!(
            matches!(Tui::job(&tui.screen()), Job::Running(_)),
            "the job stopped after a cancelled `{key}`"
        );
    }
    // x stops the job and everything it started; the sequencer is not its child.
    let (ok, summary, screen) = tui.run_job(&["x"], 60)?;
    check!(
        !ok && summary.starts_with("lgs run --watch: "),
        "`x` should end the job as stopped; the Job line says {summary:?}:\n{screen}"
    );
    let leftover = cx.poll(15, 500, || {
        pids_named("cargo")
            .iter()
            .all(|c| cargo_before.contains(c))
            .then_some(())
    });
    check!(
        leftover.is_some(),
        "a cargo process outlived the job that `x` stopped"
    );
    check!(
        cx.localnet_status(&p)?["tracked_pid"] == pid && cx.localnet_status(&p)?["ready"] == true,
        "`x` took the sequencer down with the job"
    );
    Ok(())
}

/// TUI-006 — Localnet outlives the dashboard; no terminal is refused
pub fn quit_and_no_terminal(cx: &Ctx) -> R {
    let (p, _) = fixtures::run_project(cx, false)?;
    cx.localnet_up(&p)?;
    let pid = cx.localnet_status(&p)?["tracked_pid"].clone();
    let tui = Tui::start(cx, &p)?;
    tui.key("q");
    let gone = cx.poll(10, 200, || (!tui.alive()).then_some(()));
    check!(gone.is_some(), "`q` did not close the dashboard");
    let status = cx.localnet_status(&p)?;
    check!(
        status["tracked_pid"] == pid && status["ready"] == true,
        "quitting the dashboard stopped or restarted localnet: {status}"
    );
    // Commands run with stdin from /dev/null: not a terminal.
    cx.lgs(&p.root, &["tui"])
        .timeout(20)
        .run()
        .fails()?
        .has("needs an interactive terminal")?;
    Ok(())
}

/// The top-right corner of the screen, where the small λ sits.
fn corner(screen: &str) -> String {
    screen
        .lines()
        .take(8)
        .map(|l| {
            let chars: Vec<char> = l.chars().collect();
            chars[chars.len().saturating_sub(13)..]
                .iter()
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn has_lambda(screen: &str) -> bool {
    corner(screen).chars().any(|c| "█▄▀▙▟▛▜▝▘▗▖▌▐".contains(c))
}

/// TUI-007 — Rendering holds up - start-up λ, log pane, wrapping, job line
pub fn rendering(cx: &Ctx) -> R {
    let (p, _) = fixtures::run_project(cx, false)?;

    // The Job line stays visible next to a foreign-listener hint.
    stop_localnet(cx, &p)?;
    {
        let _foreign = Listener::bind(PORT, true)?;
        let tui = Tui::start(cx, &p)?;
        tui.wait("the foreign listener", 20, |s| {
            Tui::status(s).contains("foreign listener")
        })?;
        let (ok, summary, screen) = tui.run_job(&["j", "d"], 300)?;
        let status = Tui::status(&screen);
        check!(
            !ok && summary.starts_with("lgs deploy hello_world_with_authorization: exit "),
            "a deploy against a foreign listener should fail; the Job line says {summary:?}"
        );
        check!(
            status.contains("foreign listener")
                && status.contains(&format!("pid {}", std::process::id())),
            "the Status pane lost the foreign-listener state next to a long job label: {status}"
        );
    }
    cx.localnet_up(&p)?;

    // Start-up: the large λ and caption, replaced within three seconds.
    let tui = Tui::launch(cx, &p)?;
    let splash = cx.poll(3, 30, || {
        tui.screen()
            .contains("L O G O S   S C A F F O L D")
            .then_some(())
    });
    check!(splash.is_some(), "the start-up λ caption was never shown");
    tui.wait("the dashboard after the splash", 10, |s| {
        s.contains("┌ Status") && !s.contains("S C A F F O L D")
    })?;

    // The corner λ: present at 120x32, still when idle, gone when smaller.
    let idle = tui.wait("a running localnet", 20, |s| {
        Tui::status(s).contains("running")
    })?;
    check!(has_lambda(&idle), "no corner λ at 120x32:\n{idle}");
    std::thread::sleep(Duration::from_millis(600));
    check!(
        corner(&tui.screen()) == corner(&idle),
        "the corner λ moves while idle"
    );
    for (cols, rows) in [(119, 32), (120, 29)] {
        tui.resize(cols, rows);
        let small = tui.screen();
        check!(
            !has_lambda(&small),
            "the corner λ is still drawn at {cols}x{rows}:\n{small}"
        );
        check!(
            small.contains("┌ Status") && small.contains("Programs") && small.contains("│Job"),
            "the panes changed at {cols}x{rows}:\n{small}"
        );
    }
    tui.resize(120, 32);
    check!(
        has_lambda(&tui.screen()),
        "the corner λ did not come back at 120x32"
    );

    // It spins while a job runs.
    tui.key("w");
    tui.wait("the running job", 30, |s| {
        matches!(Tui::job(s), Job::Running(_))
    })?;
    let mut shapes = std::collections::BTreeSet::new();
    for _ in 0..12 {
        shapes.insert(corner(&tui.screen()));
        std::thread::sleep(Duration::from_millis(150));
    }
    tui.run_job(&["x"], 60)?;
    check!(
        shapes.len() > 1,
        "the corner λ does not spin while a job runs"
    );

    // l: the sequencer log as plain text, although the file holds colour codes.
    tui.key("l");
    let log = tui.wait("the sequencer log pane", 10, |s| {
        s.contains("Sequencer log")
    })?;
    std::thread::sleep(Duration::from_secs(2));
    let later = tui.screen();
    for screen in [&log, &later] {
        check!(
            !screen.contains("[2m") && !screen.contains("[0m") && !screen.contains('\u{1b}'),
            "colour codes reached the log pane:\n{screen}"
        );
    }
    let file = std::fs::read(p.path(".scaffold/logs/sequencer.log")).unwrap_or_default();
    if !file.contains(&0x1b) {
        cx.note("the sequencer log holds no colour codes on this host, so the pane had nothing to clean");
    }
    tui.key("l");
    tui.wait("the Output pane again", 10, |s| {
        !s.contains("Sequencer log")
    })?;
    cx.note("step 3 (wrapping and PgUp on a long failing build) is not automated");
    Ok(())
}
