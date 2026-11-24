//! Terminal UI: session list → full-width viewer (screen stack).

mod fmt;
mod lines;
mod list;
mod viewer;

use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};

use crate::export::{ExportOptions, to_markdown};
use crate::index::Index;
use list::ListScreen;
use viewer::Viewer;

const POLL: Duration = Duration::from_millis(50);
const STATUS_TTL: Duration = Duration::from_secs(4);

/// What a screen asks the app to do after handling a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Quit,
    Back,
    Open {
        path: PathBuf,
        project: String,
        jump: Option<usize>,
    },
    OpenSubagent(PathBuf, String),
    Copy(String),
    Pager,
    Status(String),
}

pub fn run(index: Index, query: String) -> Result<()> {
    let sessions = index.sessions()?;
    let mut app = App {
        index,
        list: ListScreen::new(sessions, query),
        viewers: Vec::new(),
        status: None,
    };
    let mut terminal = ratatui::init();
    let result = app.event_loop(&mut terminal);
    ratatui::restore();
    result
}

struct App {
    index: Index,
    list: ListScreen,
    viewers: Vec<Viewer>,
    status: Option<(String, Instant)>,
}

impl App {
    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        // Redraw only when something changed: an event, a search tick or an
        // expiring status message.
        let mut dirty = true;
        loop {
            if self
                .status
                .as_ref()
                .is_some_and(|(_, t)| t.elapsed() > STATUS_TTL)
            {
                self.status = None;
                dirty = true;
            }
            if dirty {
                terminal.draw(|f| self.draw(f))?;
                dirty = false;
            }
            if !event::poll(POLL)? {
                if self.viewers.is_empty() && self.list.wants_tick() {
                    self.list.tick(&self.index);
                    dirty = !self.list.wants_tick();
                }
                continue;
            }
            dirty = true;
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if !self.handle(key, terminal)? {
                return Ok(());
            }
        }
    }

    /// Returns `false` when the app should exit.
    fn handle(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) -> Result<bool> {
        let action = match self.viewers.last_mut() {
            Some(v) => v.on_key(key),
            None => self.list.on_key(key),
        };
        match action {
            Action::None => {}
            Action::Quit => return Ok(false),
            Action::Back => {
                self.viewers.pop();
            }
            Action::Open {
                path,
                project,
                jump,
            } => self.open(terminal, path, project, jump)?,
            Action::OpenSubagent(path, project) => self.open(terminal, path, project, None)?,
            Action::Copy(text) => {
                let mut out = std::io::stdout();
                out.write_all(fmt::osc52(&text).as_bytes())?;
                out.flush()?;
                self.set_status(format!("copied: {text}"));
            }
            Action::Pager => {
                if let Err(e) = self.page(terminal) {
                    self.set_status(format!("pager failed: {e:#}"));
                }
            }
            Action::Status(s) => self.set_status(s),
        }
        Ok(true)
    }

    /// Parses and opens a transcript; large files take a second, so a
    /// loading notice is drawn first.
    fn open(
        &mut self,
        terminal: &mut DefaultTerminal,
        path: PathBuf,
        project: String,
        jump: Option<usize>,
    ) -> Result<()> {
        self.set_status("loading session…".into());
        terminal.draw(|f| self.draw(f))?;
        match Viewer::open(path, project, jump) {
            Ok(v) => {
                self.status = None;
                self.viewers.push(v);
            }
            Err(e) => self.set_status(format!("cannot open session: {e:#}")),
        }
        Ok(())
    }

    /// Suspends the TUI and shows the current session as Markdown in `$PAGER`.
    fn page(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some(v) = self.viewers.last() else {
            return Ok(());
        };
        let md = to_markdown(v.transcript(), ExportOptions::default());
        // Private temp file (0600), removed when dropped.
        let mut file = tempfile::Builder::new()
            .prefix("clog-")
            .suffix(".md")
            .tempfile()?;
        file.write_all(md.as_bytes())?;
        file.flush()?;
        let pager = fmt::pager_command(std::env::var("PAGER").ok().as_deref());
        let mut parts = pager.split_whitespace();
        let Some(program) = parts.next() else {
            return Ok(());
        };
        ratatui::restore();
        let status = Command::new(program).args(parts).arg(file.path()).status();
        *terminal = ratatui::init();
        terminal.clear()?;
        let status = status.with_context(|| format!("run {program}"))?;
        if !status.success() {
            self.set_status(format!("{program} exited with {status}"));
        }
        Ok(())
    }

    fn set_status(&mut self, s: String) {
        self.status = Some((s, Instant::now()));
    }

    fn draw(&mut self, f: &mut Frame<'_>) {
        let area = f.area();
        let status = self.status.as_ref().map(|(s, _)| s.as_str()).unwrap_or("");
        match self.viewers.last_mut() {
            Some(v) => {
                v.draw(f, area);
                if !status.is_empty() {
                    let line = Rect {
                        y: area.bottom().saturating_sub(1),
                        height: 1,
                        ..area
                    };
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            status.to_string(),
                            Style::new().add_modifier(Modifier::BOLD),
                        )),
                        line,
                    );
                }
            }
            None => self.list.draw(f, area, status),
        }
    }
}

/// Title bar: bold left text, dim right text, rule underneath when 2 rows tall.
pub(crate) fn header(f: &mut Frame<'_>, area: Rect, left: &str, right: &str) {
    let [top, rule] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(area.height.saturating_sub(1)),
    ])
    .areas(area);
    let right_w = u16::try_from(unicode_width::UnicodeWidthStr::width(right))
        .unwrap_or(0)
        .min(top.width / 2);
    let [l, r] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(right_w + 1)]).areas(top);
    let left = fmt::truncate(left, usize::from(l.width));
    f.render_widget(
        Paragraph::new(Span::styled(
            left,
            Style::new().add_modifier(Modifier::BOLD),
        )),
        l,
    );
    f.render_widget(
        Paragraph::new(
            Line::from(Span::styled(
                right.to_string(),
                Style::new().add_modifier(Modifier::DIM),
            ))
            .right_aligned(),
        ),
        r,
    );
    if rule.height > 0 {
        f.render_widget(
            Paragraph::new(Span::styled(
                "─".repeat(rule.width.into()),
                Style::new().add_modifier(Modifier::DIM),
            )),
            rule,
        );
    }
}

/// Key-hint bar with a rule above it and an optional right-aligned position.
pub(crate) fn footer(f: &mut Frame<'_>, area: Rect, keys: &str, right: &str) {
    let [rule, text] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    f.render_widget(
        Paragraph::new(Span::styled(
            "─".repeat(rule.width.into()),
            Style::new().add_modifier(Modifier::DIM),
        )),
        rule,
    );
    let right_w = u16::try_from(unicode_width::UnicodeWidthStr::width(right)).unwrap_or(0);
    let [l, r] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(right_w + 1)]).areas(text);
    f.render_widget(
        Paragraph::new(Span::styled(
            fmt::truncate(keys, usize::from(l.width)),
            Style::new().add_modifier(Modifier::DIM),
        )),
        l,
    );
    f.render_widget(
        Paragraph::new(Line::from(right.to_string()).right_aligned()),
        r,
    );
}
