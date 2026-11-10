//! Drawing. Reads [`App`], never changes it.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::model::LocalnetOwnership;

use super::app::{App, Modal, View};

const KEYS: &str = "r run · w run --watch · b build · d deploy selected · \
                    s localnet start/stop · R reset · t topup · c doctor · l output/sequencer log · \
                    x stop job · PgUp/PgDn scroll · q quit";

pub(crate) fn draw(frame: &mut Frame, app: &App) {
    let [header, top, output, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(7),
        Constraint::Min(5),
        Constraint::Length(2),
    ])
    .areas(frame.area());

    frame.render_widget(
        Line::from(vec![
            Span::from(" lgs tui ").bold().reversed(),
            Span::from(format!(" {}", app.root.display())),
        ]),
        header,
    );
    let [status, programs] =
        Layout::horizontal([Constraint::Length(48), Constraint::Min(30)]).areas(top);
    draw_status(frame, status, app);
    draw_programs(frame, programs, app);
    draw_output(frame, output, app);
    frame.render_widget(
        Paragraph::new(KEYS).dark_gray().wrap(Wrap { trim: true }),
        footer,
    );

    if let Modal::Confirm(text, _) = &app.modal {
        let area = centered(frame.area(), 60, 5);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(text.as_str())
                .wrap(Wrap { trim: true })
                .block(
                    Block::bordered()
                        .title(" Confirm ")
                        .title_bottom(" y yes · any other key no ")
                        .border_style(Color::Yellow),
                ),
            area,
        );
    }
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let label = |text: &str| Span::from(format!("{text:<10}")).dark_gray();
    let mut lines = Vec::new();
    match &app.status {
        None => lines.push(Line::from("reading project status…").dark_gray()),
        Some(status) => {
            let localnet = &status.localnet;
            let (state, color) = match localnet.ownership {
                LocalnetOwnership::Managed => ("running", Color::Green),
                LocalnetOwnership::ManagedNotReady => ("starting", Color::Yellow),
                LocalnetOwnership::Stopped => ("stopped", Color::DarkGray),
                LocalnetOwnership::StaleState => ("stale state", Color::Red),
                LocalnetOwnership::Foreign => ("foreign listener", Color::Red),
            };
            let mut spans = vec![
                label("Localnet"),
                Span::from(format!("● {state}")).fg(color),
            ];
            if let Some(pid) = localnet.tracked_pid.or(localnet.listener_pid) {
                spans.push(format!("  pid {pid}").into());
            }
            lines.push(Line::from(spans));
            if let Some(block) = status.block {
                lines.push(Line::from(vec![label(""), format!("block {block}").into()]));
            }
            // Stopped needs no hint: `s` starts it.
            if matches!(
                localnet.ownership,
                LocalnetOwnership::StaleState | LocalnetOwnership::Foreign
            ) {
                if let Some(hint) = localnet.remediation.first() {
                    lines.push(Line::from(hint.clone().yellow()));
                }
            }
            lines.push(Line::from(vec![
                label("Wallet"),
                match &status.wallet {
                    Some(address) => middle_ellipsis(address, 36).into(),
                    None => "no default yet (`lgs run` sets one)".dark_gray(),
                },
            ]));
        }
    }
    lines.push(Line::from(vec![label("Job"), job_line(app)]));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(" Status ")),
        area,
    );
}

fn job_line(app: &App) -> Span<'static> {
    if let Some(job) = &app.job {
        return format!("{} · {}s", job.label, job.started.elapsed().as_secs()).cyan();
    }
    match &app.last_result {
        Some((summary, true)) => format!("✓ {summary}").green(),
        Some((summary, false)) => format!("✗ {summary}").red(),
        None => "idle".dark_gray(),
    }
}

fn draw_programs(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" Programs (build · deployed by last run) ");
    let programs = match app.status.as_ref().map(|s| &s.programs) {
        Some(Ok(programs)) => programs,
        Some(Err(err)) => {
            frame.render_widget(
                Paragraph::new(err.as_str())
                    .dark_gray()
                    .wrap(Wrap { trim: true })
                    .block(block),
                area,
            );
            return;
        }
        None => &Vec::new(),
    };
    let width = programs.iter().map(|p| p.name.len()).max().unwrap_or(0);
    let items: Vec<ListItem> = programs
        .iter()
        .map(|program| {
            let build = match program.build {
                Some(kind) => Span::from(format!("{kind:<8}")),
                None => "not built".dark_gray(),
            };
            let deployed = if program.deployed {
                "  ✓ deployed".green()
            } else {
                "".into()
            };
            ListItem::new(Line::from(vec![
                format!("{:<width$}  ", program.name).into(),
                build,
                deployed,
            ]))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        area,
        &mut ListState::default().with_selected(Some(app.selected)),
    );
}

fn draw_output(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.view {
        View::Job => " Output ",
        View::SequencerLog => " Sequencer log (.scaffold/logs/sequencer.log) ",
    };
    let lines = app.view_lines();
    let height = usize::from(area.height.saturating_sub(2));
    let end = lines
        .len()
        .saturating_sub(app.scroll.min(lines.len().saturating_sub(height)));
    let start = end.saturating_sub(height);
    let mut block = Block::bordered().title(title);
    if end < lines.len() {
        block = block.title_bottom(format!(" {} more below · End follows ", lines.len() - end));
    }
    let text: Vec<Line> = lines[start..end]
        .iter()
        .map(|line| {
            if line.starts_with("── ") || line.starts_with("$ ") {
                Line::from(line.as_str()).cyan()
            } else if line.starts_with("error") {
                Line::from(line.as_str()).red()
            } else {
                Line::from(line.as_str())
            }
        })
        .collect();
    if lines.is_empty() && app.view == View::Job {
        frame.render_widget(
            Paragraph::new(
                "Press r for `lgs run`: build, start localnet, top up the wallet, deploy.",
            )
            .dark_gray()
            .block(block),
            area,
        );
        return;
    }
    frame.render_widget(Paragraph::new(text).block(block), area);
}

/// `text` cut to `max` chars by dropping its middle, so both ends of an
/// address stay recognisable.
fn middle_ellipsis(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let head = (max - 1) / 2;
    let tail = max - 1 - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::model::LocalnetStatusReport;
    use crate::tui::status::{ProgramRow, Snapshot};

    fn render(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn long_addresses_keep_both_ends() {
        assert_eq!(middle_ellipsis("Public/abc", 36), "Public/abc");
        let cut = middle_ellipsis("Public/J8zRSzGerW9WMiho1YaqvAwDjqiJ5QuKNNE96e4qjDvz", 36);
        assert_eq!(cut.chars().count(), 36);
        assert!(cut.starts_with("Public/J8zR") && cut.ends_with("qjDvz") && cut.contains('…'));
    }

    #[test]
    fn dashboard_shows_localnet_wallet_and_programs() {
        let mut app = App::new(PathBuf::from("/work/my-app"), PathBuf::from("lgs"));
        app.status = Some(Snapshot {
            localnet: LocalnetStatusReport {
                tracked_pid: Some(4242),
                tracked_running: true,
                listener_present: true,
                listener_pid: Some(4242),
                ownership: LocalnetOwnership::Managed,
                ready: true,
                log_path: String::new(),
                remediation: Vec::new(),
            },
            block: Some(17),
            wallet: Some("Public/abc".into()),
            programs: Ok(vec![
                ProgramRow {
                    name: "hello_world".into(),
                    build: Some("docker"),
                    deployed: true,
                },
                ProgramRow {
                    name: "counter".into(),
                    build: None,
                    deployed: false,
                },
            ]),
            sequencer_log: Vec::new(),
        });
        app.output.push("$ lgs build".into());
        let screen = render(&app);
        for expected in [
            "/work/my-app",
            "● running  pid 4242",
            "block 17",
            "Public/abc",
            "hello_world  docker    ✓ deployed",
            "counter      not built",
            "$ lgs build",
            "r run",
        ] {
            assert!(
                screen.contains(expected),
                "missing {expected:?} in\n{screen}"
            );
        }
    }
}
