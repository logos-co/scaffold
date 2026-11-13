//! Drawing. Reads [`App`] and changes nothing but `App::max_scroll`, the
//! one fact only a drawn frame knows.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::model::LocalnetOwnership;

use super::app::{App, Modal, View, SPLASH_MIN};
use super::lambda::Lambda;

const KEYS: &str = "r run · w run --watch · b build · d deploy selected · \
                    s localnet start/stop · R reset · t topup · c doctor · l output/sequencer log · \
                    x stop job · PgUp/PgDn scroll · q quit";

/// The corner λ needs this much room, so it never crowds the panes.
const BADGE_MIN_WIDTH: u16 = 120;
const BADGE_MIN_HEIGHT: u16 = 30;
const BADGE_WIDTH: u16 = 16;
const STATUS_WIDTH: u16 = 48;
/// The confirm box's key hint. `App::handle_key` confirms on `y` and `Enter`.
const CONFIRM_KEYS: &str = " y / Enter yes · any other key no ";
/// Status and Programs panes are at least this tall, borders included.
const TOP_MIN_HEIGHT: u16 = 7;

pub(crate) fn draw(frame: &mut Frame, app: &App) {
    if let Some(shown) = app.splash {
        draw_splash(frame, shown.elapsed().as_secs_f32());
        return;
    }
    // The top row grows with what the Status pane wraps to, so no row (the
    // Job line above all) is ever pushed out by a hint or a long label.
    let status = status_paragraph(app);
    let top_height = u16::try_from(status.line_count(STATUS_WIDTH - 2))
        .unwrap_or(u16::MAX)
        .max(TOP_MIN_HEIGHT);
    let [header, top, output, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(top_height),
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
    let area = frame.area();
    let badge = area.width >= BADGE_MIN_WIDTH && area.height >= BADGE_MIN_HEIGHT;
    let [status_area, programs, corner] = Layout::horizontal([
        Constraint::Length(STATUS_WIDTH),
        Constraint::Min(30),
        Constraint::Length(if badge { BADGE_WIDTH } else { 0 }),
    ])
    .areas(top);
    frame.render_widget(status, status_area);
    draw_programs(frame, programs, app);
    if badge {
        // Still when idle; spins while a job runs.
        let lambda = match &app.job {
            Some(job) => Lambda::spinning(job.started.elapsed()),
            None => Lambda::still(),
        };
        frame.render_widget(lambda, corner.inner(ratatui::layout::Margin::new(1, 0)));
    }
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
                        .title_bottom(CONFIRM_KEYS)
                        .border_style(Color::Yellow),
                ),
            area,
        );
    }
}

/// The start-up screen: a large λ drawn in from the top left, then a light
/// passing over it, with the product name underneath.
fn draw_splash(frame: &mut Frame, seconds: f32) {
    let area = frame.area();
    let caption = ["L O G O S   S C A F F O L D", "loading project…"];
    let rows = area.height.saturating_sub(6).min(18);
    if rows < 4 {
        frame.render_widget(
            Paragraph::new(caption[0]).centered().bold(),
            centered(area, area.width, 1),
        );
        return;
    }
    let cols = (rows * 2).min(area.width);
    let block = centered(area, cols, rows + 3);
    let [glyph, _, name, hint] = Layout::vertical([
        Constraint::Length(rows),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(Rect {
        x: area.x,
        width: area.width,
        ..block
    });
    let reveal_for = SPLASH_MIN.as_secs_f32() * 0.45;
    let shine_from = reveal_for + 0.1;
    let shine = (seconds - shine_from) / 0.6;
    frame.render_widget(
        Lambda {
            angle: 0.0,
            reveal: seconds / reveal_for,
            shine: (0.0..1.3).contains(&shine).then_some(shine * 1.3 - 0.15),
        },
        Rect {
            x: block.x,
            width: cols,
            ..glyph
        },
    );
    let fade = ((seconds - reveal_for * 0.6) / 0.4).clamp(0.0, 1.0);
    let grey = (90.0 + 140.0 * fade) as u8;
    frame.render_widget(
        Paragraph::new(caption[0])
            .centered()
            .bold()
            .fg(Color::Rgb(grey, grey, grey)),
        name,
    );
    frame.render_widget(Paragraph::new(caption[1]).centered().dark_gray(), hint);
}

fn status_paragraph(app: &App) -> Paragraph<'static> {
    let label = |text: &str| Span::from(format!("{text:<10}")).dark_gray();
    let mut lines: Vec<Line<'static>> = Vec::new();
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
    // `trim: false`: the label padding is what lines `block N` up under the
    // localnet state; trimming would push it flush against the border.
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(" Status "))
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
    let mut block = Block::bordered().title(title);
    if lines.is_empty() && app.view == View::Job {
        app.max_scroll.set(0);
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
    // Wrap at the pane width the way a terminal would, so a long error keeps
    // its remediation on the next row instead of being cut off. Scrolling then
    // counts rendered rows, not source lines.
    let width = usize::from(area.width.saturating_sub(2)).max(1);
    let rows: Vec<Line> = lines
        .iter()
        .flat_map(|line| {
            let style = if line.starts_with("── ") || line.starts_with("$ ") {
                Style::new().cyan()
            } else if line.starts_with("error") {
                Style::new().red()
            } else {
                Style::new()
            };
            wrap_chars(line, width)
                .into_iter()
                .map(move |row| Line::styled(row, style))
        })
        .collect();
    let height = usize::from(area.height.saturating_sub(2));
    let max_scroll = rows.len().saturating_sub(height);
    app.max_scroll.set(max_scroll);
    let end = rows.len() - app.scroll.min(max_scroll);
    let start = end.saturating_sub(height);
    if end < rows.len() {
        block = block.title_bottom(format!(" {} more below · End follows ", rows.len() - end));
    }
    frame.render_widget(Paragraph::new(rows[start..end].to_vec()).block(block), area);
}

/// `line` split into rows of at most `width` columns, breaking anywhere, as a
/// terminal does. An empty line is one empty row.
fn wrap_chars(line: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    for c in line.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        row.push(c);
        used += w;
    }
    rows.push(row);
    rows
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
        render_at(app, 100, 24)
    }

    fn render_at(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
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
    fn splash_shows_a_large_lambda_and_the_name() {
        let mut app = App::new(PathBuf::from("/work/my-app"), PathBuf::from("lgs"));
        app.splash = Some(std::time::Instant::now() - SPLASH_MIN);
        let screen = render_at(&app, 100, 30);
        assert!(screen.contains("L O G O S   S C A F F O L D"), "{screen}");
        assert!(screen.contains('█'), "{screen}");
        assert!(!screen.contains("r run"), "the dashboard waits: {screen}");
        // Too small for a glyph: just the name.
        let screen = render_at(&app, 40, 8);
        assert!(
            screen.contains("L O G O S") && !screen.contains('█'),
            "{screen}"
        );
    }

    #[test]
    fn corner_lambda_only_on_big_screens() {
        let app = App::new(PathBuf::from("/work/my-app"), PathBuf::from("lgs"));
        let quadrant = |screen: &str| screen.contains(['█', '▟', '▙', '▛', '▜']);
        assert!(!quadrant(&render_at(&app, 100, 24)));
        assert!(!quadrant(&render_at(&app, 160, 24)), "too short");
        assert!(quadrant(&render_at(&app, 160, 40)));
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
            // Aligned under the localnet state, not flush with the border.
            "│          block 17",
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

    fn snapshot(ownership: LocalnetOwnership, remediation: Vec<String>) -> Snapshot {
        Snapshot {
            localnet: LocalnetStatusReport {
                tracked_pid: None,
                tracked_running: false,
                listener_present: true,
                listener_pid: Some(1107),
                ownership,
                ready: true,
                log_path: String::new(),
                remediation,
            },
            block: Some(34),
            wallet: None,
            programs: Ok(Vec::new()),
            sequencer_log: Vec::new(),
        }
    }

    /// A two-row remediation hint plus a long deploy label used to push the
    /// Job line out of a fixed-height Status pane.
    #[test]
    fn job_line_survives_a_hint_and_a_long_label() {
        let mut app = App::new(PathBuf::from("/work/my-app"), PathBuf::from("lgs"));
        app.status = Some(snapshot(
            LocalnetOwnership::Foreign,
            vec![
                "Stop the external listener on 127.0.0.1:3040 or choose a clean environment".into(),
            ],
        ));
        app.last_result = Some((
            "lgs deploy hello_world_with_authorization: exit 0".into(),
            true,
        ));
        let screen = render(&app);
        for expected in [
            "foreign listener",
            "choose a clean environment",
            "Job",
            "exit 0",
        ] {
            assert!(
                screen.contains(expected),
                "missing {expected:?} in\n{screen}"
            );
        }
    }

    /// Long output lines wrap instead of being cut at the pane edge.
    #[test]
    fn long_output_lines_wrap() {
        let mut app = App::new(PathBuf::from("/work/my-app"), PathBuf::from("lgs"));
        let tail = "or `cargo install cargo-risczero` END-OF-LINE";
        app.output.push(format!(
            "Install it with `rzup install cargo-risczero` {}{tail}",
            "x".repeat(80)
        ));
        let screen = render(&app);
        assert!(screen.contains("END-OF-LINE"), "{screen}");
    }

    /// Scrolling counts wrapped rows, so PgUp reaches the first line even when
    /// the lines wrap to more rows than there are lines.
    #[test]
    fn page_up_reaches_the_top_of_wrapped_output() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new(PathBuf::from("/work/my-app"), PathBuf::from("lgs"));
        app.output.push("FIRST-LINE".into());
        for i in 0..20 {
            app.output.push(format!("{i:02} {}", "w".repeat(250)));
        }
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        for _ in 0..20 {
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        }
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("FIRST-LINE"), "{screen}");
    }

    /// A shorter line drawn over a longer one leaves nothing of it behind.
    #[test]
    fn a_shorter_line_leaves_no_remnant_of_a_longer_one() {
        let mut app = App::new(PathBuf::from("/work/my-app"), PathBuf::from("lgs"));
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        app.output.push(format!("short{}", "REMNANT".repeat(10)));
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        app.output = vec!["short".into()];
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(!screen.contains("REMNANT"), "{screen}");
    }

    #[test]
    fn wrap_chars_breaks_at_the_width() {
        assert_eq!(wrap_chars("", 4), [""]);
        assert_eq!(wrap_chars("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap_chars("  ab", 4), ["  ab"]);
    }

    /// The footer must name every key that confirms: `y` and `Enter`.
    #[test]
    fn confirm_box_names_both_confirm_keys() {
        use crate::tui::app::Confirmed;
        let mut app = App::new(PathBuf::from("/work/my-app"), PathBuf::from("lgs"));
        app.modal = Modal::Confirm("Quit while a job runs?".into(), Confirmed::Quit);
        let screen = render(&app);
        assert!(screen.contains("y / Enter yes"), "{screen}");
    }
}
