//! `lgs tui`: a terminal dashboard for one project. Status is read in
//! process; actions run as child `lgs` commands (see [`app`]).

mod app;
mod lambda;
mod status;
mod ui;

use std::io::IsTerminal;
use std::sync::mpsc::channel;
use std::time::Duration;

use anyhow::bail;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::project::load_project;
use crate::DynResult;

use app::App;

pub(crate) fn cmd_tui() -> DynResult<()> {
    let project = load_project()?;
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        bail!("`lgs tui` needs an interactive terminal; run the commands it wraps directly instead (see `lgs --help`)");
    }
    let mut app = App::new(project.root.clone(), std::env::current_exe()?);
    app.splash = Some(std::time::Instant::now());
    let (status_tx, status_rx) = channel();
    status::spawn_poller(project.root, status_tx);

    let mut terminal = ratatui::init();
    let result = (|| -> DynResult<()> {
        while !app.quit {
            if let Some(snapshot) = status_rx.try_iter().last() {
                app.status = Some(snapshot);
                app.selected = app.selected.min(app.programs().len().saturating_sub(1));
            }
            app.tick();
            terminal.draw(|frame| ui::draw(frame, &app))?;
            // ~25 fps while the λ moves, 10 otherwise.
            let wait = if app.animating() { 40 } else { 100 };
            if event::poll(Duration::from_millis(wait))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        app.handle_key(key);
                    }
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    app.stop_job();
    result
}
