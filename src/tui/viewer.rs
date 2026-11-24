//! Session viewer screen: scrollable dialog with folds, jumps and search.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use super::fmt::{highlight, resume_command, truncate};
use super::lines::{Fold, Kind, RLine, RenderOpts, render};
use super::{Action, footer, header};
use crate::model::Transcript;
use crate::parse::{ParseOptions, one_line, parse_file};
use crate::paths::jsonl_files;

#[derive(Debug, Clone, Copy)]
enum Anchor {
    Fold(Fold),
    Message { no: usize, offset: usize },
}

const TIMELINE_WIDTH: u16 = 3;
const MIN_WIDTH_FOR_TIMELINE: u16 = 60;

pub struct Viewer {
    title: String,
    project: String,
    resume: Option<String>,
    subagents: Vec<PathBuf>,
    transcript: Transcript,
    expanded: HashSet<Fold>,
    all_tools: bool,
    show_thinking: bool,
    timeline: bool,
    lines: Vec<RLine>,
    /// Lowercased plain text per line, for in-session search.
    haystack: Vec<String>,
    /// Indices of compaction banner lines, for the timeline.
    boundaries: Vec<usize>,
    width: usize,
    dirty: bool,
    cursor: usize,
    scroll: usize,
    height: usize,
    jump_to: Option<usize>,
    search: Option<Search>,
    picker: Option<Picker>,
}

struct Search {
    editing: bool,
    query: String,
    matches: Vec<usize>,
}

struct Picker {
    items: Vec<(PathBuf, String)>,
    state: ListState,
}

impl Viewer {
    /// Opens a transcript; subagent files (under `subagents/`) include sidechain records.
    pub fn open(path: PathBuf, project: String, jump_to: Option<usize>) -> Result<Self> {
        let is_subagent = crate::paths::is_subagent_file(&path);
        let transcript = parse_file(
            &path,
            ParseOptions {
                include_sidechain: is_subagent,
                keep_tool_output: true,
            },
        )?;
        let meta = &transcript.meta;
        let resume = match (&meta.session_id, is_subagent) {
            (Some(id), false) => Some(resume_command(meta.cwd.as_deref(), id)),
            _ => None,
        };
        let subagents = if is_subagent {
            Vec::new()
        } else {
            jsonl_files(&path.with_extension("").join("subagents"))
        };
        let title = meta.title.clone().unwrap_or_else(|| "(untitled)".into());
        Ok(Self {
            title,
            project,
            resume,
            subagents,
            transcript,
            expanded: HashSet::new(),
            all_tools: false,
            show_thinking: false,
            timeline: true,
            lines: Vec::new(),
            haystack: Vec::new(),
            boundaries: Vec::new(),
            width: 0,
            dirty: true,
            cursor: 0,
            scroll: 0,
            height: 1,
            jump_to,
            search: None,
            picker: None,
        })
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    fn rebuild(&mut self, width: usize) {
        let anchor = self.anchor();
        let opts = RenderOpts {
            width,
            expanded: &self.expanded,
            all_tools: self.all_tools,
            show_thinking: self.show_thinking,
        };
        self.lines = render(&self.transcript, &opts);
        self.haystack = self.lines.iter().map(|l| l.text().to_lowercase()).collect();
        self.boundaries = (0..self.lines.len())
            .filter(|&i| self.lines[i].kind == Kind::Boundary)
            .collect();
        self.width = width;
        self.dirty = false;
        if let Some(no) = self.jump_to.take() {
            let at = self.first_line_of(no).unwrap_or(0);
            self.jump(at);
        } else if let Some(a) = anchor {
            self.cursor = self.resolve(a).min(self.lines.len().saturating_sub(1));
            self.keep_visible();
        }
        self.refresh_matches();
    }

    /// Where the cursor is, in terms that survive re-wrapping.
    fn anchor(&self) -> Option<Anchor> {
        let line = self.lines.get(self.cursor)?;
        if let Some(fold) = line.fold {
            return Some(Anchor::Fold(fold));
        }
        let no = line.msg_no?;
        let first = self.first_line_of(no)?;
        Some(Anchor::Message {
            no,
            offset: self.cursor - first,
        })
    }

    fn resolve(&self, a: Anchor) -> usize {
        match a {
            Anchor::Fold(f) => self.lines.iter().position(|l| l.fold == Some(f)),
            Anchor::Message { no, offset } => self.first_line_of(no).map(|first| {
                let len = self.lines[first..]
                    .iter()
                    .take_while(|l| l.msg_no == Some(no))
                    .count();
                first + offset.min(len.saturating_sub(1))
            }),
        }
        .unwrap_or(self.cursor)
    }

    fn first_line_of(&self, no: usize) -> Option<usize> {
        self.lines.iter().position(|l| l.msg_no == Some(no))
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Action {
        if self.picker.is_some() {
            return self.on_picker_key(k);
        }
        if self.search.as_ref().is_some_and(|s| s.editing) {
            self.on_search_key(k);
            return Action::None;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.height.max(1);
        match k.code {
            KeyCode::Char('q') => return Action::Back,
            KeyCode::Esc if self.search.is_some() => self.search = None,
            KeyCode::Esc => return Action::Back,
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::Char('d') if ctrl => self.move_by(signed(page / 2)),
            KeyCode::Char('u') if ctrl => self.move_by(-signed(page / 2)),
            KeyCode::PageDown | KeyCode::Char(' ') => self.move_by(signed(page)),
            KeyCode::PageUp | KeyCode::Char('b') => self.move_by(-signed(page)),
            KeyCode::Char('g') | KeyCode::Home => self.jump(0),
            KeyCode::Char('G') | KeyCode::End => self.jump(self.lines.len().saturating_sub(1)),
            KeyCode::Char(']') => self.seek(Kind::Prompt, true),
            KeyCode::Char('[') => self.seek(Kind::Prompt, false),
            KeyCode::Char('}') => self.seek(Kind::Boundary, true),
            KeyCode::Char('{') => self.seek(Kind::Boundary, false),
            KeyCode::Enter => self.toggle_fold(),
            KeyCode::Char('t') => {
                self.all_tools = !self.all_tools;
                self.expanded.retain(|f| !matches!(f, Fold::Tool { .. }));
                self.dirty = true;
            }
            KeyCode::Char('T') => {
                self.show_thinking = !self.show_thinking;
                self.dirty = true;
            }
            KeyCode::Char('|') => self.timeline = !self.timeline,
            KeyCode::Char('/') => {
                self.search = Some(Search {
                    editing: true,
                    query: String::new(),
                    matches: Vec::new(),
                })
            }
            KeyCode::Char('n') => self.next_match(true),
            KeyCode::Char('N') => self.next_match(false),
            KeyCode::Char('y') => {
                return match &self.resume {
                    Some(cmd) => Action::Copy(cmd.clone()),
                    None => Action::Status("subagent transcripts cannot be resumed".into()),
                };
            }
            KeyCode::Char('e') => return Action::Pager,
            KeyCode::Char('s') => return self.open_picker(),
            _ => {}
        }
        Action::None
    }

    fn on_search_key(&mut self, k: KeyEvent) {
        let Some(s) = self.search.as_mut() else {
            return;
        };
        match k.code {
            KeyCode::Esc => self.search = None,
            KeyCode::Enter => {
                s.editing = false;
                if s.query.is_empty() {
                    self.search = None;
                }
            }
            KeyCode::Backspace => {
                s.query.pop();
                self.refresh_matches();
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                s.query.clear();
                self.refresh_matches();
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                s.query.push(c);
                self.refresh_matches();
                self.next_match_from(self.cursor, true);
            }
            _ => {}
        }
    }

    fn refresh_matches(&mut self) {
        let Some(s) = self.search.as_mut() else {
            return;
        };
        let needle = s.query.to_lowercase();
        s.matches = if needle.is_empty() {
            Vec::new()
        } else {
            (0..self.haystack.len())
                .filter(|&i| self.haystack[i].contains(&needle))
                .collect()
        };
    }

    fn next_match(&mut self, forward: bool) {
        let from = if forward {
            self.cursor + 1
        } else {
            self.cursor
        };
        self.next_match_from(from, forward);
    }

    fn next_match_from(&mut self, from: usize, forward: bool) {
        let Some(s) = &self.search else { return };
        let found = if forward {
            s.matches.iter().find(|&&m| m >= from).or(s.matches.first())
        } else {
            s.matches
                .iter()
                .rev()
                .find(|&&m| m < from)
                .or(s.matches.last())
        };
        if let Some(&m) = found {
            self.jump(m);
        }
    }

    fn toggle_fold(&mut self) {
        let Some(fold) = self.lines.get(self.cursor).and_then(|l| l.fold) else {
            return;
        };
        if !self.expanded.remove(&fold) {
            self.expanded.insert(fold);
        }
        // Keep the cursor on the fold's header line after re-rendering.
        if let Some(head) = self.lines.iter().position(|l| l.fold == Some(fold)) {
            self.cursor = head;
        }
        self.dirty = true;
    }

    fn seek(&mut self, kind: Kind, forward: bool) {
        let found = if forward {
            self.lines
                .iter()
                .enumerate()
                .skip(self.cursor + 1)
                .find(|(_, l)| l.kind == kind)
        } else {
            self.lines
                .iter()
                .enumerate()
                .take(self.cursor)
                .rev()
                .find(|(_, l)| l.kind == kind)
        };
        if let Some((i, _)) = found {
            self.jump(i);
        }
    }

    fn move_by(&mut self, delta: isize) {
        let max = self.lines.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(max);
        self.keep_visible();
    }

    /// Moves the cursor to `line` and scrolls it to the top of the view.
    fn jump(&mut self, line: usize) {
        self.cursor = line.min(self.lines.len().saturating_sub(1));
        self.scroll = self
            .cursor
            .min(self.lines.len().saturating_sub(self.height));
    }

    fn keep_visible(&mut self) {
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + self.height {
            self.scroll = self.cursor + 1 - self.height;
        }
    }

    fn open_picker(&mut self) -> Action {
        if self.subagents.is_empty() {
            return Action::Status("no subagent transcripts for this session".into());
        }
        let opts = ParseOptions {
            include_sidechain: true,
            keep_tool_output: false,
        };
        let items = self
            .subagents
            .iter()
            .map(|p| {
                let label = parse_file(p, opts)
                    .ok()
                    .and_then(|t| t.meta.first_prompt.or(t.meta.title))
                    .map_or_else(|| p.display().to_string(), |s| one_line(&s, 200));
                (p.clone(), label)
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(0));
        self.picker = Some(Picker { items, state });
        Action::None
    }

    fn on_picker_key(&mut self, k: KeyEvent) -> Action {
        let Some(p) = self.picker.as_mut() else {
            return Action::None;
        };
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') => self.picker = None,
            KeyCode::Char('j') | KeyCode::Down => p.state.select_next(),
            KeyCode::Char('k') | KeyCode::Up => p.state.select_previous(),
            KeyCode::Enter => {
                let chosen = p
                    .state
                    .selected()
                    .and_then(|i| p.items.get(i))
                    .map(|(path, _)| path.clone());
                self.picker = None;
                if let Some(path) = chosen {
                    return Action::OpenSubagent(path, self.project.clone());
                }
            }
            _ => {}
        }
        Action::None
    }

    pub fn draw(&mut self, f: &mut Frame<'_>, area: Rect) {
        let [head, body, foot] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .areas(area);
        let show_timeline = self.timeline && body.width >= MIN_WIDTH_FOR_TIMELINE;
        let [text_area, timeline_area] = Layout::horizontal([
            Constraint::Min(1),
            Constraint::Length(if show_timeline { TIMELINE_WIDTH } else { 0 }),
        ])
        .areas(body);

        let content_width = usize::from(text_area.width.saturating_sub(2)).max(10);
        if self.dirty || content_width != self.width {
            self.height = usize::from(text_area.height).max(1);
            self.rebuild(content_width);
        }
        self.height = usize::from(text_area.height).max(1);
        self.keep_visible();

        let m = &self.transcript.meta;
        let compactions = if m.compactions > 0 {
            format!(" · ⧉{}", m.compactions)
        } else {
            String::new()
        };
        let agents = if self.subagents.is_empty() {
            String::new()
        } else {
            format!(" · {} subagents", self.subagents.len())
        };
        header(
            f,
            head,
            &format!("{} · {}", self.project, self.title),
            &format!("{} msgs{compactions}{agents}", m.messages),
        );

        self.draw_text(f, text_area);
        if show_timeline {
            self.draw_timeline(f, timeline_area);
        }
        self.draw_footer(f, foot);
        if self.picker.is_some() {
            self.draw_picker(f, area);
        }
    }

    fn draw_text(&self, f: &mut Frame<'_>, area: Rect) {
        let needle = self.search.as_ref().map(|s| s.query.as_str()).unwrap_or("");
        let hl = Style::new().add_modifier(Modifier::REVERSED);
        let cursor_style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
        let visible: Vec<Line<'_>> = self
            .lines
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(area.height.into())
            .map(|(i, l)| {
                let gutter = if i == self.cursor {
                    Span::styled("▌ ", cursor_style)
                } else {
                    Span::raw("  ")
                };
                let body = highlight(&l.line, needle, hl);
                let mut spans = vec![gutter];
                spans.extend(
                    body.spans
                        .into_iter()
                        .map(|s| Span::styled(s.content, body.style.patch(s.style))),
                );
                Line::from(spans)
            })
            .collect();
        f.render_widget(Paragraph::new(visible), area);
    }

    fn draw_timeline(&self, f: &mut Frame<'_>, area: Rect) {
        let total = self.lines.len().max(1);
        let rows = usize::from(area.height).max(1);
        let boundaries = &self.boundaries;
        let view = self.scroll..self.scroll + self.height;
        let lines: Vec<Line<'_>> = (0..rows)
            .map(|r| {
                let span = (r * total / rows)..(((r + 1) * total / rows).max(r * total / rows + 1));
                let (ch, style) = if boundaries.iter().any(|b| span.contains(b)) {
                    ("═", Style::new().fg(Color::Yellow))
                } else if span.start < view.end && view.start < span.end {
                    ("█", Style::new().fg(Color::Cyan))
                } else {
                    ("░", Style::new().add_modifier(Modifier::DIM))
                };
                Line::from(Span::styled(format!(" {ch}"), style))
            })
            .collect();
        f.render_widget(Paragraph::new(lines), area);
    }

    fn draw_footer(&self, f: &mut Frame<'_>, area: Rect) {
        let msg = self
            .lines
            .get(self.cursor)
            .and_then(|l| l.msg_no)
            .map_or_else(String::new, |n| format!("#{n}  "));
        let pct = (self.cursor + 1) * 100 / self.lines.len().max(1);
        let pos = format!("{msg}{}/{} {pct}%", self.cursor + 1, self.lines.len());
        let keys = match &self.search {
            Some(s) if s.editing => format!("/{}█   {} matches · Enter keep · Esc close", s.query, s.matches.len()),
            Some(s) => {
                let at = s.matches.iter().position(|&m| m == self.cursor).map_or("-".into(), |i| (i + 1).to_string());
                format!("/{}  match {at}/{} · n next · N prev · Esc close", s.query, s.matches.len())
            }
            None => "j/k scroll  ]/[ prompt  }/{ ⧉  Enter fold  t tools  T thinking  / search  e pager  s agents  y resume  q back".into(),
        };
        footer(f, area, &keys, &pos);
    }

    fn draw_picker(&mut self, f: &mut Frame<'_>, area: Rect) {
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        let w = area.width.saturating_sub(8).min(100);
        let h = (u16::try_from(p.items.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2))
        .min(area.height.saturating_sub(4));
        let rect = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + (area.height - h) / 2,
            width: w,
            height: h,
        };
        let items: Vec<ListItem<'_>> = p
            .items
            .iter()
            .map(|(_, label)| ListItem::new(truncate(label, usize::from(w.saturating_sub(4)))))
            .collect();
        let list = List::new(items)
            .block(
                ratatui::widgets::Block::bordered().title(" Subagents · Enter open · Esc close "),
            )
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        f.render_widget(Clear, rect);
        f.render_stateful_widget(list, rect, &mut p.state);
    }
}

fn signed(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

#[cfg(test)]
mod tests;
