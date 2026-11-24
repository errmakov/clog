//! Transcript → wrapped, styled screen lines for the viewer.

use std::collections::HashSet;

use chrono::{DateTime, Local, Utc};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::fmt::{hard_wrap, sanitize, soft_wrap, truncate};
use crate::export::kilo;
use crate::model::{Block, Compaction, Item, Message, Role, ToolCall, Transcript};

/// Lines shown per expanded tool section before eliding the rest.
pub const MAX_TOOL_LINES: usize = 400;
const USER_BAR: &str = "┃ ";
const TOOL_BAR: &str = "  │ ";

/// A collapsible region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fold {
    Tool { item: usize, block: usize },
    Summary { item: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Plain,
    /// Header line of a user message (target of `]` / `[`).
    Prompt,
    /// Compaction banner (target of `}` / `{`).
    Boundary,
}

#[derive(Debug, Clone)]
pub struct RLine {
    pub line: Line<'static>,
    pub msg_no: Option<usize>,
    pub fold: Option<Fold>,
    pub kind: Kind,
}

impl RLine {
    pub fn text(&self) -> String {
        self.line.spans.iter().map(|s| s.content.as_ref()).collect()
    }
}

pub struct RenderOpts<'a> {
    pub width: usize,
    pub expanded: &'a HashSet<Fold>,
    pub all_tools: bool,
    pub show_thinking: bool,
}

impl RenderOpts<'_> {
    fn is_open(&self, f: Fold) -> bool {
        self.expanded.contains(&f) != (self.all_tools && matches!(f, Fold::Tool { .. }))
    }
}

pub fn render(t: &Transcript, o: &RenderOpts<'_>) -> Vec<RLine> {
    let mut out = Out {
        lines: Vec::new(),
        msg: None,
        fold: None,
    };
    for (i, item) in t.items.iter().enumerate() {
        match item {
            Item::Message(m) => message(&mut out, i, m, o),
            Item::Compaction(c) => compaction(&mut out, i, c, o),
        }
    }
    out.lines
}

struct Out {
    lines: Vec<RLine>,
    msg: Option<usize>,
    fold: Option<Fold>,
}

impl Out {
    fn push(&mut self, line: Line<'static>, kind: Kind) {
        self.lines.push(RLine {
            line,
            msg_no: self.msg,
            fold: self.fold,
            kind,
        });
    }
    fn blank(&mut self) {
        self.push(Line::default(), Kind::Plain);
    }
}

fn message(out: &mut Out, item: usize, m: &Message, o: &RenderOpts<'_>) {
    out.msg = Some(m.no);
    out.fold = None;
    let user = m.role == Role::User;
    let (bar, body_width) = if user {
        (USER_BAR, o.width.saturating_sub(2))
    } else {
        ("", o.width)
    };
    let bar_style = Style::new().fg(Color::Cyan);
    let who = if user {
        Span::styled(
            "you",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            "● claude",
            Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        )
    };
    let left = vec![
        Span::styled(bar, bar_style),
        who,
        dim(format!(" · {}", hhmm(m.time))),
    ];
    out.push(
        right_align(left, dim(format!("#{}", m.no)), o.width),
        if user { Kind::Prompt } else { Kind::Plain },
    );

    for (bi, b) in m.blocks.iter().enumerate() {
        match b {
            Block::Text(t) => {
                for l in markdown(t, body_width) {
                    out.push(prefixed(bar, bar_style, l), Kind::Plain);
                }
            }
            Block::Thinking(t) if o.show_thinking => {
                let style = Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC);
                out.push(Line::styled("✻ thinking", style), Kind::Plain);
                for l in soft_wrap_all(&sanitize(t), body_width.saturating_sub(2)) {
                    out.push(Line::styled(format!("  {l}"), style), Kind::Plain);
                }
            }
            Block::Thinking(_) => {}
            Block::Attachment(kind) => out.push(
                prefixed(bar, bar_style, Line::from(dim(format!("[{kind}]")))),
                Kind::Plain,
            ),
            Block::Tool(call) => tool(out, Fold::Tool { item, block: bi }, call, o),
        }
    }
    out.fold = None;
    out.blank();
}

fn tool(out: &mut Out, fold: Fold, call: &ToolCall, o: &RenderOpts<'_>) {
    out.fold = Some(fold);
    let open = o.is_open(fold);
    let status = match &call.result {
        Some(r) if r.is_error => Span::styled(" ✗ error", Style::new().fg(Color::Red)),
        Some(r) => dim(format!(" ({} lines)", r.text.lines().count())),
        None => dim(" …"),
    };
    let marker = if open { "▾ " } else { "▸ " };
    let head = format!("{marker}{} ", call.name);
    let room = o
        .width
        .saturating_sub(head.width() + status.content.width());
    let line = Line::from(vec![
        Span::styled(
            head,
            Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
        ),
        Span::raw(truncate(&sanitize(&call.summary()), room)),
        status,
    ]);
    out.push(line, Kind::Plain);
    if open {
        let input = match call.input.get("command").and_then(|c| c.as_str()) {
            Some(cmd) if call.name == "Bash" => format!("$ {cmd}"),
            _ => serde_json::to_string_pretty(&call.input).unwrap_or_default(),
        };
        boxed(
            out,
            &input,
            o.width,
            Style::new().add_modifier(Modifier::DIM),
        );
        if let Some(r) = &call.result {
            let style = if r.is_error {
                Style::new().fg(Color::Red)
            } else {
                Style::new()
            };
            out.push(Line::from(dim("  ├─ output")), Kind::Plain);
            boxed(out, &r.text, o.width, style);
        }
    }
    out.fold = None;
}

/// Tool payload lines with a left rule, hard-wrapped and capped.
fn boxed(out: &mut Out, text: &str, width: usize, style: Style) {
    let clean = sanitize(text);
    let inner = width.saturating_sub(TOOL_BAR.width()).max(1);
    let total = clean.lines().count();
    for (n, raw) in clean.lines().enumerate() {
        if n >= MAX_TOOL_LINES {
            let rest = total - n;
            out.push(
                Line::from(dim(format!(
                    "{TOOL_BAR}… {rest} more lines (e: open in pager)"
                ))),
                Kind::Plain,
            );
            return;
        }
        for piece in hard_wrap(raw, inner) {
            out.push(
                Line::from(vec![dim(TOOL_BAR), Span::styled(piece, style)]),
                Kind::Plain,
            );
        }
    }
}

fn compaction(out: &mut Out, item: usize, c: &Compaction, o: &RenderOpts<'_>) {
    out.msg = None;
    out.fold = None;
    let tokens = match (c.pre_tokens, c.post_tokens) {
        (Some(a), Some(b)) => format!(" · {} → {} tokens", kilo(a), kilo(b)),
        _ => String::new(),
    };
    let label = format!(" ⧉ COMPACTED {}{tokens} ", hhmm(c.time));
    let fill = o.width.saturating_sub(label.width());
    let banner = format!(
        "{}{label}{}",
        "═".repeat(fill / 2),
        "═".repeat(fill - fill / 2)
    );
    let style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    out.push(Line::styled(banner, style), Kind::Boundary);
    if let Some(summary) = &c.summary {
        let fold = Fold::Summary { item };
        out.fold = Some(fold);
        let open = o.is_open(fold);
        let marker = if open { "▾" } else { "▸" };
        out.push(
            Line::from(dim(format!("{marker} Summary Claude saw after compaction"))),
            Kind::Plain,
        );
        if open {
            for l in markdown(summary, o.width.saturating_sub(2)) {
                out.push(prefixed("  ", Style::new(), l), Kind::Plain);
            }
        }
        out.fold = None;
    }
    out.blank();
}

/// Minimal markdown: fenced code (unwrapped style, hard wrap), headings bold,
/// `inline code` coloured, prose soft-wrapped.
fn markdown(text: &str, width: usize) -> Vec<Line<'static>> {
    let code = Style::new().fg(Color::Green);
    let mut out = Vec::new();
    let mut in_fence = false;
    for raw in sanitize(text).lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            out.push(Line::from(dim(truncate(raw, width))));
            continue;
        }
        if in_fence {
            out.extend(
                hard_wrap(raw, width)
                    .into_iter()
                    .map(|p| Line::styled(p, code)),
            );
            continue;
        }
        let heading = trimmed.starts_with('#');
        for piece in soft_wrap(raw, width) {
            let line = inline_code(&piece, code);
            out.push(if heading {
                line.style(Style::new().add_modifier(Modifier::BOLD))
            } else {
                line
            });
        }
    }
    out
}

fn inline_code(s: &str, code: Style) -> Line<'static> {
    if !s.contains('`') {
        return Line::raw(s.to_string());
    }
    let spans = s
        .split('`')
        .enumerate()
        .filter(|(_, p)| !p.is_empty())
        .map(|(i, p)| {
            if i % 2 == 1 {
                Span::styled(p.to_string(), code)
            } else {
                Span::raw(p.to_string())
            }
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn soft_wrap_all(text: &str, width: usize) -> Vec<String> {
    text.lines().flat_map(|l| soft_wrap(l, width)).collect()
}

fn prefixed(bar: &'static str, style: Style, line: Line<'static>) -> Line<'static> {
    if bar.is_empty() {
        return line;
    }
    let line_style = line.style;
    let mut spans = vec![Span::styled(bar, style)];
    spans.extend(
        line.spans
            .into_iter()
            .map(|s| Span::styled(s.content, line_style.patch(s.style))),
    );
    Line::from(spans)
}

fn right_align(mut left: Vec<Span<'static>>, right: Span<'static>, width: usize) -> Line<'static> {
    let used: usize = left.iter().map(|s| s.content.width()).sum::<usize>() + right.content.width();
    left.push(Span::raw(" ".repeat(width.saturating_sub(used).max(1))));
    left.push(right);
    Line::from(left)
}

fn dim(s: impl Into<std::borrow::Cow<'static, str>>) -> Span<'static> {
    Span::styled(s, Style::new().add_modifier(Modifier::DIM))
}

fn hhmm(t: Option<DateTime<Utc>>) -> String {
    t.map(|t| t.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
