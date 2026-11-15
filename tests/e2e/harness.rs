//! Runner plumbing for the end-to-end suite: outcomes, command execution with
//! timeouts and logs, shared fixtures, and small helpers the scenarios use.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

/// Why a scenario stopped before passing.
#[derive(Debug, Clone)]
pub enum Stop {
    /// The product did not do what the scenario expects.
    Fail(String),
    /// Something the scenario needs first did not work (a fixture, a
    /// prerequisite scenario).
    Blocked(String),
    /// The host cannot run this scenario (no Docker, no tmux, …).
    Skip(String),
}

pub type R<T = ()> = Result<T, Stop>;

macro_rules! check {
    ($cond:expr, $($arg:tt)*) => {
        if !$cond {
            return Err($crate::harness::Stop::Fail(format!($($arg)*)));
        }
    };
}
macro_rules! fail {
    ($($arg:tt)*) => { return Err($crate::harness::Stop::Fail(format!($($arg)*))) };
}
macro_rules! skip {
    ($($arg:tt)*) => { return Err($crate::harness::Stop::Skip(format!($($arg)*))) };
}
macro_rules! blocked {
    ($($arg:tt)*) => { return Err($crate::harness::Stop::Blocked(format!($($arg)*))) };
}
pub(crate) use {blocked, check, fail, skip};

/// The last `n` bytes of `s`, on a char boundary.
pub fn tail(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut start = s.len() - n;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// A finished command.
#[derive(Debug, Clone, Default)]
pub struct Out {
    pub cmd: String,
    pub code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub secs: f64,
}

impl Out {
    /// stdout followed by stderr.
    pub fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    fn describe(&self) -> String {
        format!(
            "`{}` (exit {:?}{}, {:.1}s)\n--- stdout (tail)\n{}\n--- stderr (tail)\n{}",
            self.cmd,
            self.code,
            if self.timed_out { ", TIMED OUT" } else { "" },
            self.secs,
            tail(&self.stdout, 1500),
            tail(&self.stderr, 1500)
        )
    }

    /// Exit status 0.
    pub fn ok(&self) -> R<&Self> {
        check!(
            self.code == Some(0),
            "expected success: {}",
            self.describe()
        );
        Ok(self)
    }

    /// A non-zero exit that is not a timeout or a signal.
    pub fn fails(&self) -> R<&Self> {
        check!(
            !self.timed_out && matches!(self.code, Some(c) if c != 0),
            "expected a non-zero exit: {}",
            self.describe()
        );
        Ok(self)
    }

    /// `needle` appears in stdout or stderr.
    pub fn has(&self, needle: &str) -> R<&Self> {
        check!(
            self.all().contains(needle),
            "expected output to contain {needle:?}: {}",
            self.describe()
        );
        Ok(self)
    }

    /// `needle` appears in neither stdout nor stderr.
    pub fn lacks(&self, needle: &str) -> R<&Self> {
        check!(
            !self.all().contains(needle),
            "expected output not to contain {needle:?}: {}",
            self.describe()
        );
        Ok(self)
    }

    /// stdout is one JSON value and nothing else.
    pub fn json(&self) -> R<Value> {
        match serde_json::from_str(self.stdout.trim()) {
            Ok(v) => Ok(v),
            Err(e) => fail!("expected pure JSON on stdout ({e}): {}", self.describe()),
        }
    }

    /// Finished in under `secs`.
    pub fn within(&self, secs: f64) -> R<&Self> {
        check!(
            !self.timed_out && self.secs < secs,
            "expected to finish within {secs}s: {}",
            self.describe()
        );
        Ok(self)
    }

    /// Lines of stdout and stderr that contain `needle`.
    pub fn lines_with(&self, needle: &str) -> Vec<String> {
        self.all()
            .lines()
            .filter(|l| l.contains(needle))
            .map(str::to_string)
            .collect()
    }
}

/// A command about to run. Every command has a timeout, runs in its own
/// process group (so a timeout kills its children too) and is written to the
/// scenario log.
pub struct Cmd<'a> {
    cx: &'a Ctx,
    program: OsString,
    args: Vec<OsString>,
    cwd: PathBuf,
    envs: Vec<(OsString, OsString)>,
    env_rm: Vec<OsString>,
    timeout: Duration,
    stdin_null: bool,
}

impl<'a> Cmd<'a> {
    pub fn env(mut self, k: &str, v: impl Into<OsString>) -> Self {
        self.envs.push((k.into(), v.into()));
        self
    }
    pub fn env_remove(mut self, k: &str) -> Self {
        self.env_rm.push(k.into());
        self
    }
    /// Seconds before the command and its process group are killed.
    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout = Duration::from_secs(secs);
        self
    }

    fn render(&self) -> String {
        let mut s = self.program.to_string_lossy().into_owned();
        if s == self.cx.lgs.to_string_lossy() {
            s = "lgs".into();
        }
        for a in &self.args {
            let a = a.to_string_lossy();
            if a.is_empty() || a.contains([' ', '\t', '\n', '"', '\'', '$', '|', ';', '&']) {
                s.push_str(&format!(" '{}'", a.replace('\'', "'\\''")));
            } else {
                s.push(' ');
                s.push_str(&a);
            }
        }
        s
    }

    fn build(&self) -> Command {
        let mut c = Command::new(&self.program);
        c.args(&self.args).current_dir(&self.cwd);
        self.cx.base_env(&mut c);
        for k in &self.env_rm {
            c.env_remove(k);
        }
        for (k, v) in &self.envs {
            c.env(k, v);
        }
        c.process_group(0);
        if self.stdin_null {
            c.stdin(Stdio::null());
        }
        c
    }

    pub fn run(self) -> Out {
        let rendered = self.render();
        let id = self.cx.cmd_seq.get();
        self.cx.cmd_seq.set(id + 1);
        let out_path = self.cx.tmp.join(format!("cmd-{id}.out"));
        let err_path = self.cx.tmp.join(format!("cmd-{id}.err"));
        let mut c = self.build();
        c.stdout(File::create(&out_path).expect("create stdout capture"));
        c.stderr(File::create(&err_path).expect("create stderr capture"));
        let started = Instant::now();
        let mut out = Out {
            cmd: rendered.clone(),
            ..Out::default()
        };
        match c.spawn() {
            Err(e) => {
                out.stderr = format!("could not start: {e}");
                out.code = Some(127);
            }
            Ok(mut child) => loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        out.code = status.code();
                        break;
                    }
                    Ok(None) if started.elapsed() > self.timeout => {
                        kill_group(child.id(), libc::SIGKILL);
                        let _ = child.wait();
                        out.timed_out = true;
                        break;
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(20)),
                    Err(e) => {
                        out.stderr = format!("wait failed: {e}");
                        break;
                    }
                }
            },
        }
        out.secs = started.elapsed().as_secs_f64();
        out.stdout = read_lossy(&out_path);
        out.stderr.push_str(&read_lossy(&err_path));
        let _ = fs::remove_file(&out_path);
        let _ = fs::remove_file(&err_path);
        self.cx.log(&format!(
            "$ {rendered}\n  (cwd {}; exit {:?}{}; {:.1}s)\n{}{}",
            self.cwd.display(),
            out.code,
            if out.timed_out { "; TIMED OUT" } else { "" },
            out.secs,
            indent("out", tail(&out.stdout, 6000)),
            indent("err", tail(&out.stderr, 6000)),
        ));
        out
    }

    /// Start the command in the background, with stdout and stderr going to
    /// `log`. The returned guard kills its process group when dropped.
    pub fn spawn(self, log: &Path) -> R<Bg> {
        let rendered = self.render();
        let mut c = self.build();
        let f = File::create(log).expect("create background log");
        c.stdout(f.try_clone().expect("clone log handle")).stderr(f);
        self.cx
            .log(&format!("$ {rendered} &   (log {})\n", log.display()));
        match c.spawn() {
            Ok(child) => Ok(Bg {
                child,
                log: log.to_path_buf(),
            }),
            Err(e) => blocked!("could not start `{rendered}`: {e}"),
        }
    }
}

fn indent(label: &str, text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let mut s = String::new();
    for line in text.lines() {
        s.push_str(&format!("  {label}| {line}\n"));
    }
    s
}

fn read_lossy(path: &Path) -> String {
    let mut bytes = Vec::new();
    if let Ok(mut f) = File::open(path) {
        let _ = f.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn kill_group(pid: u32, signal: i32) {
    // SAFETY: plain kill(2) on a process group this suite created.
    unsafe {
        libc::kill(-(pid as libc::pid_t), signal);
    }
}

/// A background command; killed (with its process group) on drop.
pub struct Bg {
    child: Child,
    log: PathBuf,
}

impl Bg {
    pub fn log_text(&self) -> String {
        read_lossy(&self.log)
    }
    /// Send `signal` to the whole process group.
    pub fn signal(&self, signal: i32) {
        kill_group(self.child.id(), signal);
    }
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
    /// Wait up to `secs` for the process to exit by itself.
    pub fn wait(&mut self, secs: u64) -> Option<i32> {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status.code().unwrap_or(-1));
            }
            thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

impl Drop for Bg {
    fn drop(&mut self) {
        kill_group(self.child.id(), libc::SIGKILL);
        let _ = self.child.wait();
    }
}

/// Restores a file's content (or its absence) when dropped.
pub struct Restore {
    path: PathBuf,
    original: Option<Vec<u8>>,
}

impl Restore {
    pub fn of(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            original: fs::read(path).ok(),
        }
    }
    pub fn original_text(&self) -> String {
        String::from_utf8_lossy(self.original.as_deref().unwrap_or_default()).into_owned()
    }
    /// Put the original back now (also happens on drop).
    pub fn restore(&self) {
        match &self.original {
            Some(bytes) => {
                let _ = fs::write(&self.path, bytes);
            }
            None => {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

impl Drop for Restore {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Renames a path away and puts it back when dropped.
pub struct MovedAway {
    from: PathBuf,
    to: PathBuf,
}

impl MovedAway {
    pub fn new(from: &Path) -> Option<Self> {
        let to = PathBuf::from(format!("{}.e2e-off", from.display()));
        fs::rename(from, &to).ok()?;
        Some(Self {
            from: from.to_path_buf(),
            to,
        })
    }
    pub fn moved_to(&self) -> &Path {
        &self.to
    }
}

impl Drop for MovedAway {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.from);
        let _ = fs::rename(&self.to, &self.from);
    }
}

/// A listener on a local port, held until dropped. With `answer` it replies
/// `501` to every request, like `python3 -m http.server` does to a POST (a
/// "foreign process on the port"); without, it accepts connections and never
/// answers (a peer that hangs).
pub struct Listener {
    pub port: u16,
    stop: Arc<AtomicBool>,
}

impl Listener {
    pub fn bind(port: u16, answer: bool) -> R<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => l,
            Err(e) => blocked!("cannot listen on 127.0.0.1:{port}: {e}"),
        };
        let port = listener.local_addr().expect("local addr").port();
        listener.set_nonblocking(true).expect("nonblocking");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        thread::spawn(move || {
            let mut held: Vec<TcpStream> = Vec::new();
            while !stop_thread.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        if answer {
                            let _ = s.set_read_timeout(Some(Duration::from_millis(300)));
                            let mut buf = [0u8; 4096];
                            let _ = s.read(&mut buf);
                            let _ = s.write_all(
                                b"HTTP/1.0 501 Unsupported method\r\ncontent-type: text/plain\r\ncontent-length: 19\r\nconnection: close\r\n\r\nUnsupported method\n",
                            );
                        } else {
                            held.push(s);
                        }
                    }
                    Err(_) => thread::sleep(Duration::from_millis(20)),
                }
            }
        });
        Ok(Self { port, stop })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        thread::sleep(Duration::from_millis(60));
    }
}

/// `PATH` without the directories that hold an executable called `tool`.
///
/// Used to make a tool unavailable to one command. In particular a `setup`
/// run without `cargo` clones and checks out, then stops by itself at its
/// first build. Never get that effect by killing a `setup` that is compiling:
/// where the Rust toolchain LEZ pins is not installed yet, the kill can land
/// in the middle of rustup's download and leave that toolchain broken.
pub fn path_without(tool: &str) -> OsString {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let kept: Vec<PathBuf> = std::env::split_paths(&path)
        .filter(|d| !d.as_os_str().is_empty() && !d.join(tool).exists())
        .collect();
    std::env::join_paths(kept).unwrap_or(path)
}

/// True when nothing accepts connections on `127.0.0.1:port`.
pub fn port_free(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(300),
    )
    .is_err()
}

/// What the host offers. Probed once at start.
#[derive(Debug, Clone, Default)]
pub struct Caps {
    pub docker: bool,
    pub cargo_risczero: bool,
    pub tmux: bool,
    /// `10.255.255.1` swallows packets (a connect times out instead of being
    /// refused), so it can stand in for an address that never answers.
    pub blackhole: bool,
}

/// An address that never answers, when [`Caps::blackhole`] holds.
pub const BLACKHOLE: &str = "10.255.255.1";

/// One generated project.
#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
}

impl Project {
    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root.join(rel)).unwrap_or_default()
    }
    pub fn toml(&self) -> String {
        self.read("scaffold.toml")
    }
    pub fn wallet_config(&self) -> PathBuf {
        self.root.join(".scaffold/wallet/wallet_config.json")
    }
    /// The project default address without its `Public/` prefix.
    pub fn default_address(&self) -> Option<String> {
        self.read(".scaffold/state/wallet.state")
            .lines()
            .find_map(|l| l.strip_prefix("default_address="))
            .map(|a| a.trim().trim_start_matches("Public/").to_string())
    }
    /// Append text to `scaffold.toml`.
    pub fn append_toml(&self, text: &str) {
        let mut t = self.toml();
        if !t.ends_with('\n') {
            t.push('\n');
        }
        t.push_str(text);
        fs::write(self.root.join("scaffold.toml"), t).expect("write scaffold.toml");
    }
}

type Memo = RefCell<HashMap<String, Result<Out, String>>>;

/// Everything a scenario needs: paths, the binary under test, host
/// capabilities, the scenario log and the shared fixtures.
pub struct Ctx {
    /// The `logos-scaffold` binary under test.
    pub lgs: PathBuf,
    /// The `lgs` alias binary.
    pub lgs_alias: PathBuf,
    /// The scaffold repository this suite was built from.
    pub repo: PathBuf,
    /// Scratch directory for generated projects.
    pub work: PathBuf,
    /// Reports and logs.
    pub out: PathBuf,
    /// The scaffold cache root every command uses.
    pub cache: PathBuf,
    pub caps: Caps,
    tmp: PathBuf,
    cmd_seq: Cell<u64>,
    log: RefCell<Option<File>>,
    notes: RefCell<Vec<String>>,
    memo: Memo,
    /// Root of the project whose localnet is believed to be running.
    localnet_owner: RefCell<Option<PathBuf>>,
}

impl Ctx {
    pub fn new(
        lgs: PathBuf,
        lgs_alias: PathBuf,
        repo: PathBuf,
        work: PathBuf,
        out: PathBuf,
        cache: PathBuf,
    ) -> Self {
        let tmp = out.join("tmp");
        fs::create_dir_all(&tmp).expect("create tmp dir");
        fs::create_dir_all(out.join("logs")).expect("create logs dir");
        fs::create_dir_all(&work).expect("create work dir");
        let mut cx = Self {
            lgs,
            lgs_alias,
            repo,
            work,
            out,
            cache,
            caps: Caps::default(),
            tmp,
            cmd_seq: Cell::new(0),
            log: RefCell::new(None),
            notes: RefCell::new(Vec::new()),
            memo: RefCell::new(HashMap::new()),
            localnet_owner: RefCell::new(None),
        };
        cx.caps = cx.probe_caps();
        cx
    }

    fn probe_caps(&self) -> Caps {
        let have = |tool: &str| {
            self.sh(&self.work, &format!("command -v {tool}"))
                .timeout(10)
                .run()
                .code
                == Some(0)
        };
        let docker =
            have("docker") && self.sh(&self.work, "docker info").timeout(20).run().code == Some(0);
        let blackhole = matches!(
            TcpStream::connect_timeout(
                &format!("{BLACKHOLE}:3040").parse().expect("addr"),
                Duration::from_secs(3)
            ),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut
        );
        Caps {
            docker,
            cargo_risczero: have("cargo-risczero"),
            tmux: have("tmux"),
            blackhole,
        }
    }

    /// The environment every command starts from: the caller's, minus the
    /// scaffold variables that would change behaviour under test, plus the
    /// suite's cache root.
    fn base_env(&self, c: &mut Command) {
        for k in [
            "LOGOS_SCAFFOLD_QUIET",
            "LOGOS_SCAFFOLD_WALLET_PASSWORD",
            "LOGOS_SCAFFOLD_PRINT_OUTPUT",
            "LOGOS_SCAFFOLD_DEPLOY_PACING_TIMEOUT_MS",
            "NSSA_WALLET_HOME_DIR",
            "LEE_WALLET_HOME_DIR",
            "DOCKER_HOST",
            "CONTAINER_HOST",
            // Would replace every project's own `.scaffold/circuits`.
            "LOGOS_BLOCKCHAIN_CIRCUITS",
        ] {
            c.env_remove(k);
        }
        c.env("LOGOS_SCAFFOLD_CACHE_ROOT", &self.cache);
        // `cargo test` exports its own toolchain to this process: a pinned
        // `RUSTUP_TOOLCHAIN`, `CARGO_*`/`RUSTC*` variables, and a `PATH` that
        // starts with that toolchain's `bin`. A child that inherits them
        // builds LEZ and the generated projects with the wrong Rust instead of
        // the one their `rust-toolchain.toml` names, which recompiles
        // everything. Give children the environment a user's shell has.
        let toolchain_bin = std::env::var_os("CARGO")
            .map(PathBuf::from)
            .and_then(|cargo| cargo.parent().map(Path::to_path_buf));
        for (key, _) in std::env::vars_os() {
            let k = key.to_string_lossy();
            let cargo_internal = k.starts_with("CARGO_")
                && !["CARGO_HOME", "CARGO_BUILD_JOBS", "CARGO_TERM_COLOR"].contains(&k.as_ref())
                && !k.starts_with("CARGO_NET_")
                && !k.starts_with("CARGO_HTTP_")
                && !k.starts_with("CARGO_REGISTRIES_");
            if cargo_internal
                || k == "CARGO"
                || k == "RUSTUP_TOOLCHAIN"
                || k == "RUST_RECURSION_COUNT"
                || k == "RUSTC"
                || k == "RUSTDOC"
                || k.starts_with("RUSTC_")
            {
                c.env_remove(&key);
            }
        }
        if let (Some(bin), Some(path)) = (toolchain_bin, std::env::var_os("PATH")) {
            let kept: Vec<PathBuf> = std::env::split_paths(&path).filter(|d| d != &bin).collect();
            if let Ok(joined) = std::env::join_paths(kept) {
                c.env("PATH", joined);
            }
        }
    }

    /// Open the log of scenario `id` and clear its notes.
    pub fn begin(&self, id: &str) {
        let path = self.out.join("logs").join(format!("{id}.log"));
        *self.log.borrow_mut() = File::create(path).ok();
        self.notes.borrow_mut().clear();
    }

    pub fn log(&self, text: &str) {
        if let Some(f) = self.log.borrow_mut().as_mut() {
            let _ = writeln!(f, "{}", text.trim_end());
        }
    }

    /// Record something the report should say next to a pass (a step that
    /// could not run on this host, a substitution).
    pub fn note(&self, text: impl Into<String>) {
        let text = text.into();
        self.log(&format!("note: {text}"));
        self.notes.borrow_mut().push(text);
    }

    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut *self.notes.borrow_mut())
    }

    fn cmd_in(&self, cwd: &Path, program: OsString, args: Vec<OsString>) -> Cmd<'_> {
        Cmd {
            cx: self,
            program,
            args,
            cwd: cwd.to_path_buf(),
            envs: Vec::new(),
            env_rm: Vec::new(),
            timeout: Duration::from_secs(120),
            stdin_null: true,
        }
    }

    /// `logos-scaffold <args>` in `cwd`.
    pub fn lgs(&self, cwd: &Path, args: &[&str]) -> Cmd<'_> {
        self.cmd_in(
            cwd,
            self.lgs.clone().into(),
            args.iter().map(OsString::from).collect(),
        )
    }

    /// `bash -c <line>` in `cwd`, with `$LGS` set to the binary under test.
    pub fn sh(&self, cwd: &Path, line: &str) -> Cmd<'_> {
        self.cmd_in(cwd, "bash".into(), vec!["-c".into(), line.into()])
            .env("LGS", self.lgs.clone())
    }

    /// Any program.
    pub fn cmd(&self, cwd: &Path, program: &str, args: &[&str]) -> Cmd<'_> {
        self.cmd_in(
            cwd,
            program.into(),
            args.iter().map(OsString::from).collect(),
        )
    }

    /// A fresh, empty directory under the work dir.
    pub fn empty_dir(&self, name: &str) -> PathBuf {
        let dir = self.work.join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create empty dir");
        dir
    }

    /// Remove directories under the work dir that only the finished scenario
    /// used. Build trees and clones add up to tens of gigabytes otherwise.
    pub fn discard(&self, names: &[&str]) {
        for name in names {
            let _ = fs::remove_dir_all(self.work.join(name));
        }
    }

    /// Retry `f` every `every_ms` until it returns `Some`, for up to `secs`.
    pub fn poll<T>(&self, secs: u64, every_ms: u64, mut f: impl FnMut() -> Option<T>) -> Option<T> {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some(v) = f() {
                return Some(v);
            }
            if Instant::now() >= end {
                return None;
            }
            thread::sleep(Duration::from_millis(every_ms));
        }
    }

    /// Run `f` once per suite run under `key` and remember its result.
    ///
    /// The scenario that is *about* this step passes `owner = true` and gets
    /// the step's own verdict. Everyone else gets `Blocked` when it failed:
    /// their own subject was never reached.
    pub fn once(&self, key: &str, owner: bool, f: impl FnOnce() -> R<Out>) -> R<Out> {
        let cached = self.memo.borrow().get(key).cloned();
        if let Some(prev) = cached {
            return match prev {
                Ok(out) => Ok(out),
                Err(msg) => blocked!("prerequisite `{key}` failed earlier: {msg}"),
            };
        }
        self.log(&format!("--- fixture `{key}`"));
        let result = f();
        let stored = match &result {
            Ok(out) => Ok(out.clone()),
            Err(Stop::Fail(m)) | Err(Stop::Blocked(m)) | Err(Stop::Skip(m)) => {
                Err(tail(m, 600).to_string())
            }
        };
        self.memo.borrow_mut().insert(key.to_string(), stored);
        match result {
            Err(Stop::Fail(m)) if !owner => blocked!("prerequisite `{key}` failed: {m}"),
            other => other,
        }
    }

    /// Forget a fixture's result, so the next scenario that needs it runs it
    /// again (after something undid its effect, such as a chain reset).
    pub fn forget(&self, key: &str) {
        self.memo.borrow_mut().remove(key);
    }

    // ---- localnet bookkeeping -------------------------------------------

    /// Make `p`'s localnet the one that is running (stopping another
    /// project's first: there is one port).
    pub fn localnet_up(&self, p: &Project) -> R {
        let owner = self.localnet_owner.borrow().clone();
        if let Some(other) = owner.filter(|o| o != &p.root) {
            self.lgs(&other, &["localnet", "stop"]).timeout(60).run();
            *self.localnet_owner.borrow_mut() = None;
        }
        let status = self
            .lgs(&p.root, &["-q", "localnet", "status", "--json"])
            .run();
        let ready = status
            .json()
            .ok()
            .and_then(|v| v["ready"].as_bool())
            .unwrap_or(false);
        if !ready {
            if !port_free(3040) {
                blocked!("port 3040 is held by a process this suite did not start");
            }
            let start = self.lgs(&p.root, &["localnet", "start"]).timeout(180).run();
            if start.code != Some(0) {
                blocked!(
                    "localnet did not start: {}",
                    tail(&start.all(), 800).to_string()
                );
            }
        }
        *self.localnet_owner.borrow_mut() = Some(p.root.clone());
        Ok(())
    }

    /// Stop localnets that a previous, interrupted run left behind in the work
    /// dir: they hold the one sequencer port.
    pub fn stop_stale_localnets(&self) {
        let Ok(entries) = fs::read_dir(&self.work) else {
            return;
        };
        for dir in entries.flatten().map(|e| e.path()) {
            if dir.join(".scaffold/state/localnet.state").is_file() {
                self.lgs(&dir, &["localnet", "stop"]).timeout(60).run();
            }
        }
    }

    /// Stop whatever localnet the suite started.
    pub fn localnet_down(&self) {
        if let Some(owner) = self.localnet_owner.borrow_mut().take() {
            self.lgs(&owner, &["localnet", "stop"]).timeout(60).run();
        }
    }

    /// Tell the bookkeeping that `p`'s localnet is (not) running after a
    /// scenario started or stopped it itself.
    pub fn localnet_mark(&self, p: &Project, running: bool) {
        *self.localnet_owner.borrow_mut() = running.then(|| p.root.clone());
    }

    /// `localnet status --json` of `p`.
    pub fn localnet_status(&self, p: &Project) -> R<Value> {
        self.lgs(&p.root, &["-q", "localnet", "status", "--json"])
            .run()
            .json()
    }
}

/// PIDs of live processes whose executable name is exactly `name` (as
/// `pgrep -x`). Exited processes nobody has waited for yet are left out.
pub fn pids_named(name: &str) -> Vec<u32> {
    let out = Command::new("pgrep").args(["-x", name]).output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .filter(|pid| !is_zombie(*pid))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// True for a process that has exited but has not been reaped by its parent.
pub fn is_zombie(pid: u32) -> bool {
    let state = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    state.starts_with('Z')
}

/// True while a process with this pid exists.
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}
