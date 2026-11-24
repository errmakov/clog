//! Formatting helpers shared by the TUI screens.

use base64::Engine as _;
use chrono::{DateTime, Local, TimeZone};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub use crate::sanitize::{sanitize, sanitize_inline};

/// Splits `s` into pieces of at most `width` display columns, preserving
/// whitespace exactly (used for code and tool output).
pub fn hard_wrap(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push(c);
        w += cw;
    }
    out.push(cur);
    out
}

/// Word-wraps prose to `width` columns.
pub fn soft_wrap(s: &str, width: usize) -> Vec<String> {
    if s.trim().is_empty() {
        return vec![String::new()];
    }
    textwrap::wrap(s, width.max(1))
        .into_iter()
        .map(|c| c.into_owned())
        .collect()
}

/// Truncates to `width` columns, adding `…` when shortened.
pub fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// "today 17:15", "yesterday", "Nov 21", "2025-03-02".
pub fn when(ms: Option<i64>, now: DateTime<Local>) -> String {
    let Some(t) = ms.and_then(|ms| Local.timestamp_millis_opt(ms).single()) else {
        return "—".into();
    };
    let days = (now.date_naive() - t.date_naive()).num_days();
    match days {
        0 => t.format("today %H:%M").to_string(),
        1 => "yesterday".into(),
        _ if t.format("%Y").to_string() == now.format("%Y").to_string() => {
            t.format("%b %d").to_string()
        }
        _ => t.format("%Y-%m-%d").to_string(),
    }
}

pub fn clock(ms: Option<i64>) -> String {
    ms.and_then(|ms| Local.timestamp_millis_opt(ms).single())
        .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M").to_string())
}

/// 310B, 48K, 18.4M, 1.2G.
pub fn size(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    const K: f64 = 1024.0;
    if b < K {
        format!("{bytes}B")
    } else if b < K * K {
        format!("{:.0}K", b / K)
    } else if b < K * K * K {
        format!("{:.1}M", b / (K * K))
    } else {
        format!("{:.1}G", b / (K * K * K))
    }
}

/// OSC 52 sequence that asks the terminal to put `text` on the clipboard.
pub fn osc52(text: &str) -> String {
    format!(
        "\u{1b}]52;c;{}\u{7}",
        base64::engine::general_purpose::STANDARD.encode(text)
    )
}

/// POSIX single-quote shell quoting.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Shell command that resumes a session from its working directory.
/// Values come from transcript files, so control characters are removed and
/// the id cannot be mistaken for an option.
pub fn resume_command(cwd: Option<&str>, session_id: &str) -> String {
    let id = sanitize_inline(session_id);
    let id = shell_quote(id.trim_start_matches('-'));
    match cwd.map(sanitize_inline).filter(|d| !d.is_empty()) {
        Some(dir) => format!("cd -- {} && claude --resume {id}", shell_quote(&dir)),
        None => format!("claude --resume {id}"),
    }
}

/// Renders an FTS snippet whose matches are wrapped in `\u{1}` … `\u{2}`.
pub fn snippet_spans(snippet: &str, base: Style, hl: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut on = false;
    let mut cur = String::new();
    for c in snippet.chars().chain(std::iter::once('\u{2}')) {
        match c {
            '\u{1}' | '\u{2}' => {
                let text = collapse_ws(&sanitize(&cur));
                if !text.is_empty() {
                    spans.push(Span::styled(text, if on { hl } else { base }));
                }
                cur.clear();
                on = c == '\u{1}';
            }
            c => cur.push(c),
        }
    }
    spans
}

/// Replaces each whitespace run (including newlines) with one space.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

/// Re-styles every case-insensitive occurrence of `needle` (already
/// lowercase) in `line` with `hl` patched on top of the span style.
pub fn highlight(line: &Line<'static>, needle: &str, hl: Style) -> Line<'static> {
    if needle.is_empty() {
        return line.clone();
    }
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let ranges = find_all(&text, needle);
    if ranges.is_empty() {
        return line.clone();
    }
    let mut spans = Vec::new();
    let mut offset = 0;
    for span in &line.spans {
        let content = span.content.as_ref();
        let (start, end) = (offset, offset + content.len());
        let mut cuts = vec![start, end];
        for &(a, b) in &ranges {
            for p in [a, b] {
                if p > start && p < end && content.is_char_boundary(p - start) {
                    cuts.push(p);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let piece = &content[w[0] - start..w[1] - start];
            let inside = ranges.iter().any(|&(a, b)| w[0] >= a && w[1] <= b);
            let style = if inside {
                span.style.patch(hl)
            } else {
                span.style
            };
            spans.push(Span::styled(piece.to_string(), style));
        }
        offset = end;
    }
    Line::from(spans).style(line.style)
}

/// Byte ranges of case-insensitive matches; only ASCII-case folding keeps
/// byte offsets stable.
fn find_all(hay: &str, needle: &str) -> Vec<(usize, usize)> {
    let lower = hay.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    lower
        .match_indices(&needle)
        .map(|(i, m)| (i, i + m.len()))
        .collect()
}

#[cfg(test)]
mod tests;
