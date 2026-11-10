//! Dashboard state and key handling. Every action is a child `lgs`
//! invocation, so the TUI does exactly what the same command does on the
//! command line, and its output is that command's output.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::LocalnetOwnership;
use crate::DynResult;

use super::status::Snapshot;

/// Job output kept for the Output pane; older lines are dropped.
const OUTPUT_CAP: usize = 5_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Run,
    Watch,
    Build,
    Deploy(String),
    LocalnetStart,
    LocalnetStop,
    LocalnetReset,
    Topup,
    Doctor,
}

impl Action {
    pub(crate) fn args(&self) -> Vec<String> {
        let args: &[&str] = match self {
            Action::Run => &["run"],
            Action::Watch => &["run", "--watch"],
            Action::Build => &["build"],
            Action::Deploy(program) => return vec!["deploy".into(), program.clone()],
            Action::LocalnetStart => &["localnet", "start"],
            Action::LocalnetStop => &["localnet", "stop"],
            Action::LocalnetReset => &["localnet", "reset", "--yes"],
            Action::Topup => &["wallet", "topup"],
            Action::Doctor => &["doctor"],
        };
        args.iter().map(|arg| arg.to_string()).collect()
    }
}

/// What the Output pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum View {
    Job,
    SequencerLog,
}

pub(crate) enum Modal {
    None,
    Confirm(String, Confirmed),
}

pub(crate) enum Confirmed {
    Start(Action),
    Quit,
}

pub(crate) struct Job {
    pub(crate) label: String,
    pub(crate) started: Instant,
    child: Child,
    /// The threads copying its stdout and stderr into the Output pane.
    readers: Vec<JoinHandle<()>>,
    exited: Option<Instant>,
}

pub(crate) struct App {
    pub(crate) root: PathBuf,
    pub(crate) exe: PathBuf,
    pub(crate) status: Option<Snapshot>,
    pub(crate) job: Option<Job>,
    /// How the last job ended, e.g. `lgs build: exit 0`.
    pub(crate) last_result: Option<(String, bool)>,
    pub(crate) output: Vec<String>,
    pub(crate) view: View,
    /// Lines scrolled up from the bottom of the Output pane; 0 follows it.
    pub(crate) scroll: usize,
    pub(crate) selected: usize,
    pub(crate) modal: Modal,
    pub(crate) quit: bool,
    lines_tx: Sender<String>,
    lines_rx: Receiver<String>,
}

impl App {
    pub(crate) fn new(root: PathBuf, exe: PathBuf) -> Self {
        let (lines_tx, lines_rx) = channel();
        Self {
            root,
            exe,
            status: None,
            job: None,
            last_result: None,
            output: Vec::new(),
            view: View::Job,
            scroll: 0,
            selected: 0,
            modal: Modal::None,
            quit: false,
            lines_tx,
            lines_rx,
        }
    }

    /// The lines the Output pane is showing.
    pub(crate) fn view_lines(&self) -> &[String] {
        match self.view {
            View::Job => &self.output,
            View::SequencerLog => self.status.as_ref().map_or(&[], |s| &s.sequencer_log),
        }
    }

    pub(crate) fn programs(&self) -> &[super::status::ProgramRow] {
        match self.status.as_ref().map(|s| &s.programs) {
            Some(Ok(programs)) => programs,
            _ => &[],
        }
    }

    /// Pull in job output and notice a job that has exited.
    pub(crate) fn tick(&mut self) {
        // A job is done once it has exited and its output is all in, so the
        // summary line comes last. A daemon left holding its pipes (none of
        // the wrapped commands leave one) gets a second, not forever.
        let finished = self.job.as_mut().and_then(|job| {
            let status = job.child.try_wait().ok().flatten()?;
            let exited = *job.exited.get_or_insert_with(Instant::now);
            let drained = job.readers.iter().all(JoinHandle::is_finished)
                || exited.elapsed() > Duration::from_secs(1);
            drained.then(|| (status, job.label.clone()))
        });
        self.output.extend(self.lines_rx.try_iter());
        if let Some((status, label)) = finished {
            self.job = None;
            let summary = match status.code() {
                Some(code) => format!("{label}: exit {code}"),
                None => format!("{label}: stopped"),
            };
            self.output.push(format!("── {summary} ──"));
            self.last_result = Some((summary, status.success()));
        }
        if self.output.len() > OUTPUT_CAP {
            self.output.drain(..self.output.len() - OUTPUT_CAP);
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.request_quit();
            return;
        }
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => {}
            Modal::Confirm(_, confirmed) => {
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Enter) {
                    match confirmed {
                        Confirmed::Start(action) => self.start(action),
                        Confirmed::Quit => self.quit = true,
                    }
                }
                return;
            }
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.request_quit(),
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.programs().len().saturating_sub(1))
            }
            KeyCode::PageUp => self.scroll = (self.scroll + 10).min(self.view_lines().len()),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::End => self.scroll = 0,
            KeyCode::Char('l') => {
                self.view = match self.view {
                    View::Job => View::SequencerLog,
                    View::SequencerLog => View::Job,
                };
                self.scroll = 0;
            }
            KeyCode::Char('x') => self.stop_job(),
            KeyCode::Char('r') => self.start(Action::Run),
            KeyCode::Char('w') => self.start(Action::Watch),
            KeyCode::Char('b') => self.start(Action::Build),
            KeyCode::Char('d') => {
                if let Some(program) = self.programs().get(self.selected) {
                    self.start(Action::Deploy(program.name.clone()));
                }
            }
            KeyCode::Char('s') => {
                let running = self.status.as_ref().is_some_and(|s| {
                    matches!(
                        s.localnet.ownership,
                        LocalnetOwnership::Managed | LocalnetOwnership::ManagedNotReady
                    )
                });
                self.start(if running {
                    Action::LocalnetStop
                } else {
                    Action::LocalnetStart
                });
            }
            KeyCode::Char('R') => {
                self.modal = Modal::Confirm(
                    "Reset localnet? This wipes the chain state (the wallet is kept).".into(),
                    Confirmed::Start(Action::LocalnetReset),
                )
            }
            KeyCode::Char('t') => self.start(Action::Topup),
            KeyCode::Char('c') => self.start(Action::Doctor),
            _ => {}
        }
    }

    fn request_quit(&mut self) {
        if self.job.is_some() {
            self.modal = Modal::Confirm(
                "A job is still running. Stop it and quit?".into(),
                Confirmed::Quit,
            );
        } else {
            self.quit = true;
        }
    }

    /// Start `action` unless a job is already running: one at a time, like
    /// a terminal, so two commands never race on the same project state.
    fn start(&mut self, action: Action) {
        if let Some(job) = &self.job {
            self.output
                .push(format!("── {} is still running; x stops it ──", job.label));
            self.view = View::Job;
            self.scroll = 0;
            return;
        }
        let args = action.args();
        let label = format!("lgs {}", args.join(" "));
        self.output.push(format!("$ {label}"));
        self.view = View::Job;
        self.scroll = 0;
        match spawn(&self.exe, &self.root, &args, &self.lines_tx) {
            Ok((child, readers)) => {
                self.job = Some(Job {
                    label,
                    started: Instant::now(),
                    child,
                    readers,
                    exited: None,
                })
            }
            Err(err) => self.output.push(format!("error: {err:#}")),
        }
    }

    /// Stop the running job and everything it started, except a localnet,
    /// which `localnet start` daemonizes into its own session.
    pub(crate) fn stop_job(&mut self) {
        if let Some(job) = &self.job {
            // The child leads its own session and group (see `spawn`), so this
            // reaches cargo, docker and every other grandchild too.
            unsafe {
                libc::kill(-(job.child.id() as libc::pid_t), libc::SIGTERM);
            }
        }
    }
}

fn spawn(
    exe: &Path,
    root: &Path,
    args: &[String],
    tx: &Sender<String>,
) -> DynResult<(Child, Vec<JoinHandle<()>>)> {
    let mut command = Command::new(exe);
    command
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A session of its own: no controlling terminal, so a tool that writes
    // to /dev/tty (risc0's guest build does) cannot draw over the screen,
    // and it leads its own process group for `stop_job`.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let readers = vec![
        forward_lines(child.stdout.take(), tx.clone()),
        forward_lines(child.stderr.take(), tx.clone()),
    ];
    Ok((child, readers))
}

fn forward_lines(stream: Option<impl Read + Send + 'static>, tx: Sender<String>) -> JoinHandle<()> {
    thread::spawn(move || {
        let Some(stream) = stream else { return };
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        while reader.read_until(b'\n', &mut buf).unwrap_or(0) > 0 {
            if tx.send(clean_line(&buf)).is_err() {
                return;
            }
            buf.clear();
        }
    })
}

/// A line as a terminal would end up showing it: the text after the last
/// carriage return (progress redraws), without ANSI escape sequences.
pub(crate) fn clean_line(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let text = text.trim_end_matches(['\n', '\r']);
    let text = text.rsplit('\r').next().unwrap_or_default();
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI: ESC [ params... final byte in @..~
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else if c == '\t' {
            out.push_str("    ");
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn clean_line_keeps_what_a_terminal_shows() {
        assert_eq!(clean_line(b"plain\n"), "plain");
        assert_eq!(clean_line(b"10%\r50%\r100%\r\n"), "100%");
        assert_eq!(clean_line(b"\x1b[1;32mok\x1b[0m done\n"), "ok done");
        assert_eq!(clean_line(b"a\tb"), "a    b");
    }

    #[test]
    fn actions_map_to_cli_invocations() {
        assert_eq!(Action::Run.args(), ["run"]);
        assert_eq!(Action::Watch.args(), ["run", "--watch"]);
        assert_eq!(Action::Deploy("hello".into()).args(), ["deploy", "hello"]);
        assert_eq!(Action::LocalnetReset.args(), ["localnet", "reset", "--yes"]);
        assert_eq!(Action::Topup.args(), ["wallet", "topup"]);
    }

    #[test]
    fn reset_asks_first_and_only_y_confirms() {
        let mut app = App::new(PathBuf::from("/nonexistent"), PathBuf::from("/bin/true"));
        app.handle_key(key(KeyCode::Char('R')));
        assert!(matches!(app.modal, Modal::Confirm(..)));
        app.handle_key(key(KeyCode::Char('n')));
        assert!(matches!(app.modal, Modal::None));
        assert!(app.job.is_none());
    }

    #[test]
    fn jobs_run_one_at_a_time_and_report_their_exit() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path().to_path_buf(), PathBuf::from("/bin/echo"));
        app.handle_key(key(KeyCode::Char('b')));
        assert!(app.job.is_some());
        app.handle_key(key(KeyCode::Char('r')));
        assert!(app.output.iter().any(|l| l.contains("still running")));
        while app.job.is_some() {
            app.tick();
        }
        app.tick();
        assert_eq!(app.last_result, Some(("lgs build: exit 0".into(), true)));
        assert!(app.output.contains(&"build".to_string()));
    }

    #[test]
    fn quit_with_a_running_job_asks_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path().to_path_buf(), PathBuf::from("/bin/true"));
        app.job = Some(Job {
            label: "sleep".into(),
            started: Instant::now(),
            child: Command::new("sleep")
                .arg("30")
                .process_group(0)
                .spawn()
                .unwrap(),
            readers: Vec::new(),
            exited: None,
        });
        app.handle_key(key(KeyCode::Char('q')));
        assert!(!app.quit);
        app.handle_key(key(KeyCode::Char('y')));
        assert!(app.quit);
        app.stop_job();
        let status = app.job.as_mut().unwrap().child.wait().unwrap();
        assert!(!status.success());
    }
}
