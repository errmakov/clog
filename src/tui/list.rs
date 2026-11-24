//! Session list screen with fuzzy filter and full-text search modes.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::Local;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use super::fmt::{clock, resume_command, size, snippet_spans, truncate, when};
use super::{Action, footer, header};
use crate::index::{Hit, Index, SessionRow};
use crate::parse::one_line;
use crate::search::Fuzzy;

const FTS_DEBOUNCE: Duration = Duration::from_millis(150);
const FTS_LIMIT: usize = 400;
const HITS_PER_SESSION: usize = 5;
const HIGHLIGHT_W: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Fuzzy,
    FullText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    Recent,
    Size,
    Messages,
}

impl Sort {
    fn next(self) -> Self {
        match self {
            Sort::Recent => Sort::Size,
            Sort::Size => Sort::Messages,
            Sort::Messages => Sort::Recent,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Sort::Recent => "recent",
            Sort::Size => "size",
            Sort::Messages => "messages",
        }
    }
}

/// One selectable row of full-text results.
#[derive(Debug, Clone, Copy)]
enum FtsRow {
    Session { session: usize, hits: usize },
    Hit { session: usize, hit: usize },
    More { session: usize, more: usize },
}

pub struct ListScreen {
    sessions: Vec<SessionRow>,
    hay: Vec<String>,
    by_id: HashMap<i64, usize>,
    pub mode: Mode,
    query: String,
    editing: bool,
    sort: Sort,
    project: Option<String>,
    fuzzy: Fuzzy,
    order: Vec<usize>,
    hits: Vec<Hit>,
    fts_rows: Vec<FtsRow>,
    fts_pending: Option<Instant>,
    fts_info: String,
    table: TableState,
}

impl ListScreen {
    pub fn new(sessions: Vec<SessionRow>, query: String) -> Self {
        let hay = sessions
            .iter()
            .map(|s| {
                format!(
                    "{} {} {}",
                    s.project,
                    s.title,
                    one_line(&s.first_prompt, 300)
                )
            })
            .collect();
        let by_id = sessions
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();
        let mut me = Self {
            sessions,
            hay,
            by_id,
            mode: Mode::Fuzzy,
            editing: !query.is_empty(),
            query,
            sort: Sort::Recent,
            project: None,
            fuzzy: Fuzzy::default(),
            order: Vec::new(),
            hits: Vec::new(),
            fts_rows: Vec::new(),
            fts_pending: None,
            fts_info: String::new(),
            table: TableState::default(),
        };
        me.refilter();
        me
    }

    fn refilter(&mut self) {
        let mut base: Vec<usize> = (0..self.sessions.len())
            .filter(|&i| {
                self.project
                    .as_ref()
                    .is_none_or(|p| &self.sessions[i].project == p)
            })
            .collect();
        let s = &self.sessions;
        match self.sort {
            Sort::Recent => {}
            Sort::Size => base.sort_by_key(|&i| std::cmp::Reverse(s[i].size)),
            Sort::Messages => base.sort_by_key(|&i| std::cmp::Reverse(s[i].messages)),
        }
        let ranked = self
            .fuzzy
            .rank(&self.query, base.iter().map(|&i| self.hay[i].as_str()));
        self.order = ranked.into_iter().map(|r| base[r]).collect();
        self.table.select((!self.order.is_empty()).then_some(0));
    }

    /// Runs a debounced full-text query once typing pauses.
    pub fn tick(&mut self, index: &Index) {
        let Some(since) = self.fts_pending else {
            return;
        };
        if since.elapsed() < FTS_DEBOUNCE {
            return;
        }
        self.fts_pending = None;
        let started = Instant::now();
        match index.search(&self.query, FTS_LIMIT) {
            Ok(hits) => {
                self.hits = hits;
                self.build_fts_rows();
                let sessions = self
                    .fts_rows
                    .iter()
                    .filter(|r| matches!(r, FtsRow::Session { .. }))
                    .count();
                self.fts_info = format!(
                    "{}{} hits in {sessions} sessions · {}ms",
                    if self.hits.len() >= FTS_LIMIT {
                        "≥"
                    } else {
                        ""
                    },
                    self.hits.len(),
                    started.elapsed().as_millis()
                );
            }
            Err(e) => {
                self.hits.clear();
                self.fts_rows.clear();
                self.fts_info = format!("search failed: {e}");
            }
        }
        self.table.select((!self.fts_rows.is_empty()).then_some(0));
    }

    pub fn wants_tick(&self) -> bool {
        self.fts_pending.is_some()
    }

    fn build_fts_rows(&mut self) {
        let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
        let mut slot: HashMap<usize, usize> = HashMap::new();
        for (hi, h) in self.hits.iter().enumerate() {
            let Some(&si) = self.by_id.get(&h.session) else {
                continue;
            };
            if self
                .project
                .as_ref()
                .is_some_and(|p| &self.sessions[si].project != p)
            {
                continue;
            }
            let g = *slot.entry(si).or_insert_with(|| {
                groups.push((si, Vec::new()));
                groups.len() - 1
            });
            groups[g].1.push(hi);
        }
        self.fts_rows.clear();
        for (si, mut hs) in groups {
            hs.sort_by_key(|&h| self.hits[h].msg_no);
            self.fts_rows.push(FtsRow::Session {
                session: si,
                hits: hs.len(),
            });
            for &h in hs.iter().take(HITS_PER_SESSION) {
                self.fts_rows.push(FtsRow::Hit {
                    session: si,
                    hit: h,
                });
            }
            if hs.len() > HITS_PER_SESSION {
                self.fts_rows.push(FtsRow::More {
                    session: si,
                    more: hs.len() - HITS_PER_SESSION,
                });
            }
        }
    }

    fn query_changed(&mut self) {
        match self.mode {
            Mode::Fuzzy => self.refilter(),
            Mode::FullText => self.fts_pending = Some(Instant::now()),
        }
    }

    fn switch_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Fuzzy => Mode::FullText,
            Mode::FullText => Mode::Fuzzy,
        };
        self.editing = true;
        self.query_changed();
        if self.mode == Mode::FullText && self.query.trim().is_empty() {
            self.fts_rows.clear();
            self.fts_info = "type to search all dialog text".into();
        }
    }

    fn rows_len(&self) -> usize {
        match self.mode {
            Mode::Fuzzy => self.order.len(),
            Mode::FullText => self.fts_rows.len(),
        }
    }

    /// Session index and message to jump to for the selected row.
    fn selected(&self) -> Option<(usize, Option<usize>)> {
        let i = self.table.selected()?;
        match self.mode {
            Mode::Fuzzy => self.order.get(i).map(|&s| (s, None)),
            Mode::FullText => self.fts_rows.get(i).map(|r| match *r {
                FtsRow::Session { session, .. } => {
                    let first = self.fts_rows.get(i + 1).and_then(|n| match n {
                        FtsRow::Hit { hit, .. } => Some(self.hits[*hit].msg_no),
                        _ => None,
                    });
                    (session, first)
                }
                FtsRow::Hit { session, hit } => (session, Some(self.hits[hit].msg_no)),
                FtsRow::More { session, .. } => (session, None),
            }),
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Down => self.move_by(1),
            KeyCode::Up => self.move_by(-1),
            KeyCode::Char('n') | KeyCode::Char('j') if ctrl => self.move_by(1),
            KeyCode::Char('p') | KeyCode::Char('k') if ctrl => self.move_by(-1),
            KeyCode::PageDown => self.move_by(10),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::Char('c') if ctrl => return Action::Quit,
            KeyCode::Tab | KeyCode::BackTab => self.switch_mode(),
            KeyCode::Enter => return self.open_selected(),
            _ if self.editing => self.on_edit_key(k),
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('j') => self.move_by(1),
            KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Char('g') | KeyCode::Home => self.table.select(Some(0)),
            KeyCode::Char('G') | KeyCode::End => {
                self.table.select(Some(self.rows_len().saturating_sub(1)))
            }
            KeyCode::Char('/') => self.editing = true,
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.query_changed();
            }
            KeyCode::Esc if self.project.is_some() => {
                self.project = None;
                self.query_changed();
            }
            KeyCode::Char('s') => {
                self.sort = self.sort.next();
                self.refilter();
            }
            KeyCode::Char('p') => self.toggle_project(),
            KeyCode::Char('y') => {
                if let Some((s, _)) = self.selected() {
                    let row = &self.sessions[s];
                    return Action::Copy(resume_command(row.cwd.as_deref(), &row.session_id));
                }
            }
            _ => {}
        }
        Action::None
    }

    fn on_edit_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => self.editing = false,
            KeyCode::Backspace => {
                self.query.pop();
                self.query_changed();
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.clear();
                self.query_changed();
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.push(c);
                self.query_changed();
            }
            _ => {}
        }
    }

    fn toggle_project(&mut self) {
        self.project = match (&self.project, self.selected()) {
            (Some(_), _) => None,
            (None, Some((s, _))) => Some(self.sessions[s].project.clone()),
            (None, None) => None,
        };
        self.refilter();
        if self.mode == Mode::FullText {
            self.build_fts_rows();
        }
    }

    fn move_by(&mut self, d: isize) {
        let len = self.rows_len();
        if len == 0 {
            return;
        }
        let cur = self.table.selected().unwrap_or(0);
        self.table
            .select(Some(cur.saturating_add_signed(d).min(len - 1)));
    }

    fn open_selected(&self) -> Action {
        match self.selected() {
            Some((s, jump)) => {
                let row = &self.sessions[s];
                Action::Open {
                    path: row.path.clone(),
                    project: row.project.clone(),
                    jump,
                }
            }
            None => Action::None,
        }
    }

    pub fn draw(&mut self, f: &mut Frame<'_>, area: Rect, status: &str) {
        let [head, input, body, preview, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .areas(area);
        let filter = self
            .project
            .as_ref()
            .map(|p| format!(" · project: {p}"))
            .unwrap_or_default();
        header_line(
            f,
            head,
            &format!("clog ─ {} sessions{filter}", self.sessions.len()),
            status,
        );
        self.draw_input(f, input);
        match self.mode {
            Mode::Fuzzy => self.draw_sessions(f, body),
            Mode::FullText => self.draw_hits(f, body),
        }
        self.draw_preview(f, preview);
        let keys = if self.editing {
            "type to filter  ↑↓ move  Enter open  Tab fuzzy/full-text  Esc done  Ctrl-u clear"
                .to_string()
        } else {
            format!(
                "j/k move  Enter open  / filter  Tab full-text  s sort:{}  p project  y copy resume  q quit",
                self.sort.label()
            )
        };
        footer(f, foot, &keys, "");
    }

    fn draw_input(&self, f: &mut Frame<'_>, area: Rect) {
        let (label, hint) = match self.mode {
            Mode::Fuzzy => ("Filter", "[Fuzzy]  Tab: full-text"),
            Mode::FullText => ("Search", "[Full-text]  Tab: fuzzy"),
        };
        let cursor = if self.editing { "█" } else { "" };
        let border = if self.editing {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        };
        let line = Line::from(vec![
            Span::styled("> ", Style::new().fg(Color::Cyan)),
            Span::raw(self.query.clone()),
            Span::raw(cursor),
        ]);
        let block = Block::bordered()
            .title(format!(" {label} "))
            .title_bottom(Line::from(format!(" {hint} ")).right_aligned())
            .border_style(border);
        f.render_widget(Paragraph::new(line).block(block), area);
    }

    fn draw_sessions(&mut self, f: &mut Frame<'_>, area: Rect) {
        let now = Local::now();
        // Fixed columns + highlight symbol + one-column gaps between six columns.
        let fixed: u16 = 13 + 22 + 6 + 4 + 7 + HIGHLIGHT_W + 5;
        let title_w = usize::from(area.width.saturating_sub(fixed));
        let rows = self.order.iter().map(|&i| {
            let s = &self.sessions[i];
            let comp = if s.compactions > 0 {
                format!("⧉{}", s.compactions)
            } else {
                String::new()
            };
            let agents = if s.subagents > 0 {
                format!(" +{} agents", s.subagents)
            } else {
                String::new()
            };
            let title = truncate(&format!("{}{agents}", s.title), title_w);
            Row::new(vec![
                Cell::from(when(s.ended.or(s.started), now)),
                Cell::from(truncate(&s.project, 22)).style(Style::new().fg(Color::Blue)),
                Cell::from(title),
                Cell::from(s.messages.to_string()),
                Cell::from(comp).style(Style::new().fg(Color::Yellow)),
                Cell::from(size(s.size)).style(Style::new().add_modifier(Modifier::DIM)),
            ])
        });
        let widths = [
            Constraint::Length(13),
            Constraint::Length(22),
            Constraint::Min(10),
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Length(7),
        ];
        let table = Table::new(rows, widths)
            .header(
                Row::new(["WHEN", "PROJECT", "TITLE", "MSGS", "⧉", "SIZE"])
                    .style(Style::new().add_modifier(Modifier::BOLD | Modifier::DIM)),
            )
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▶ ");
        f.render_stateful_widget(table, area, &mut self.table);
        if self.order.is_empty() {
            let msg = if self.sessions.is_empty() {
                "no sessions found"
            } else {
                "no matches"
            };
            centered_note(f, area, msg);
        }
    }

    fn draw_hits(&mut self, f: &mut Frame<'_>, area: Rect) {
        let [info, list] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
        f.render_widget(
            Paragraph::new(
                Line::from(Span::styled(
                    self.fts_info.clone(),
                    Style::new().add_modifier(Modifier::DIM),
                ))
                .right_aligned(),
            ),
            info,
        );
        let now = Local::now();
        let hl = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        let width = usize::from(list.width.saturating_sub(4));
        let rows = self.fts_rows.iter().map(|r| match *r {
            FtsRow::Session { session, hits } => {
                let s = &self.sessions[session];
                let head = format!(
                    "{} · {} · {}",
                    s.project,
                    s.title,
                    when(s.ended.or(s.started), now)
                );
                Row::new(vec![Cell::from(Line::from(vec![
                    Span::styled(
                        truncate(&head, width.saturating_sub(10)),
                        Style::new().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {hits} hits"),
                        Style::new().add_modifier(Modifier::DIM),
                    ),
                ]))])
            }
            FtsRow::Hit { hit, .. } => {
                let h = &self.hits[hit];
                let who = if h.role == "user" {
                    Span::styled(" you ", Style::new().fg(Color::Cyan))
                } else {
                    Span::styled(" ●   ", Style::new().fg(Color::Magenta))
                };
                let mut spans = vec![
                    Span::styled(
                        format!("  #{:<5}", h.msg_no),
                        Style::new().add_modifier(Modifier::DIM),
                    ),
                    who,
                ];
                spans.extend(snippet_spans(&h.snippet, Style::new(), hl));
                Row::new(vec![Cell::from(Line::from(spans))])
            }
            FtsRow::More { more, .. } => Row::new(vec![Cell::from(Span::styled(
                format!("         … {more} more hits"),
                Style::new().add_modifier(Modifier::DIM),
            ))]),
        });
        let table = Table::new(rows, [Constraint::Min(10)])
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▶ ");
        f.render_stateful_widget(table, list, &mut self.table);
    }

    fn draw_preview(&self, f: &mut Frame<'_>, area: Rect) {
        let block = Block::new()
            .borders(ratatui::widgets::Borders::TOP)
            .border_style(Style::new().add_modifier(Modifier::DIM));
        let lines = match self.selected() {
            Some((i, _)) => {
                let s = &self.sessions[i];
                let mut facts = vec![
                    s.project.clone(),
                    format!("{} → {}", clock(s.started), clock(s.ended)),
                ];
                facts.extend(s.model.clone());
                facts.extend(s.branch.as_ref().map(|b| format!("branch {b}")));
                let width = usize::from(area.width.saturating_sub(2));
                vec![
                    Line::from(Span::styled(
                        truncate(&facts.join(" · "), width),
                        Style::new().add_modifier(Modifier::DIM),
                    )),
                    Line::from(format!(
                        "> {}",
                        truncate(&one_line(&s.first_prompt, 400), width.saturating_sub(2))
                    )),
                ]
            }
            None => Vec::new(),
        };
        f.render_widget(Paragraph::new(lines).block(block), area);
    }
}

fn header_line(f: &mut Frame<'_>, area: Rect, left: &str, right: &str) {
    header(f, area, left, right);
}

fn centered_note(f: &mut Frame<'_>, area: Rect, msg: &str) {
    let y = area.y + area.height / 2;
    let rect = Rect {
        x: area.x,
        y,
        width: area.width,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(
            Line::from(Span::styled(
                msg.to_string(),
                Style::new().add_modifier(Modifier::DIM),
            ))
            .centered(),
        ),
        rect,
    );
}

#[cfg(test)]
mod tests;
