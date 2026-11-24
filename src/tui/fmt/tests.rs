use chrono::TimeZone;
use ratatui::style::{Modifier, Style};

use super::*;

#[test]
fn sanitize_strips_escapes_and_controls() {
    assert_eq!(sanitize("\u{1b}[31mred\u{1b}[0m ok"), "red ok");
    assert_eq!(sanitize("a\u{1b}]52;c;Zm9v\u{7}b"), "ab");
    assert_eq!(sanitize("a\u{1b}]0;title\u{1b}\\b"), "ab");
    assert_eq!(sanitize("x\r\ny\tz\u{0}\u{9b}"), "x\ny    z");
}

#[test]
fn wraps_hard_and_soft() {
    assert_eq!(hard_wrap("  abcdef", 4), vec!["  ab", "cdef"]);
    assert_eq!(hard_wrap("", 4), vec![""]);
    assert_eq!(soft_wrap("one two three", 7), vec!["one two", "three"]);
    assert_eq!(soft_wrap("   ", 5), vec![""]);
}

#[test]
fn truncates_by_display_width() {
    assert_eq!(truncate("hello", 5), "hello");
    assert_eq!(truncate("hello world", 6), "hello…");
    assert_eq!(truncate("日本語テキスト", 5), "日本…");
}

#[test]
fn formats_relative_dates() {
    let now = Local.with_ymd_and_hms(2026, 11, 24, 18, 0, 0).unwrap();
    let ms = |y, m, d, h| {
        Some(
            Local
                .with_ymd_and_hms(y, m, d, h, 5, 0)
                .unwrap()
                .timestamp_millis(),
        )
    };
    assert_eq!(when(ms(2026, 11, 24, 9), now), "today 09:05");
    assert_eq!(when(ms(2026, 11, 23, 9), now), "yesterday 09:05");
    assert_eq!(when(ms(2026, 3, 2, 9), now), "Mar 02 09:05");
    assert_eq!(when(ms(2025, 3, 2, 9), now), "2025-03-02 09:05");
    assert_eq!(when(None, now), "—");
}

#[test]
fn formats_sizes() {
    assert_eq!(size(310), "310B");
    assert_eq!(size(48 * 1024), "48K");
    assert_eq!(size(18 * 1024 * 1024 + 400 * 1024), "18.4M");
}

#[test]
fn quotes_shell_arguments() {
    assert_eq!(shell_quote("/home/a/www"), "/home/a/www");
    assert_eq!(shell_quote("it's here"), "'it'\\''s here'");
    assert_eq!(
        resume_command_for(Shell::Posix, Some("/a b"), "s-1"),
        "cd -- '/a b' && claude --resume s-1"
    );
    assert_eq!(
        resume_command_for(Shell::Posix, None, "s-1"),
        "claude --resume s-1"
    );
    assert_eq!(shell_quote("~/x"), "'~/x'", "no tilde expansion");
}

#[test]
fn powershell_resume_command_quotes_literally() {
    assert_eq!(
        resume_command_for(Shell::PowerShell, Some(r"C:\Users\a\it's $here"), "s-1"),
        r"Set-Location -LiteralPath 'C:\Users\a\it''s $here'; claude --resume 's-1'"
    );
    assert_eq!(
        resume_command_for(Shell::PowerShell, None, "s-1"),
        "claude --resume 's-1'"
    );
    assert_eq!(powershell_quote("a\u{2019}b"), "'a\u{2019}\u{2019}b'");
}

#[test]
fn native_shell_matches_platform() {
    let expected = if cfg!(windows) {
        Shell::PowerShell
    } else {
        Shell::Posix
    };
    assert_eq!(Shell::native(), expected);
    assert_eq!(
        resume_command(Some("/a"), "s-1"),
        resume_command_for(expected, Some("/a"), "s-1")
    );
}

#[test]
fn resume_command_strips_controls_and_option_like_ids() {
    assert_eq!(
        resume_command_for(Shell::Posix, Some("/a\n\u{1b}b"), "--evil\r"),
        "cd -- /ab && claude --resume evil"
    );
    assert_eq!(
        resume_command_for(Shell::PowerShell, Some("C:\\a\r\nb"), "-x"),
        "Set-Location -LiteralPath 'C:\\ab'; claude --resume 'x'"
    );
}

#[test]
fn pager_prefers_env_then_platform_default() {
    assert_eq!(pager_command(Some("bat -p")), "bat -p");
    let default = if cfg!(windows) { "more" } else { "less" };
    assert_eq!(pager_command(Some("  ")), default);
    assert_eq!(pager_command(None), default);
}

#[test]
fn osc52_encodes_base64() {
    assert_eq!(osc52("hi"), "\u{1b}]52;c;aGk=\u{7}");
}

#[test]
fn snippet_marks_become_styled_spans() {
    let hl = Style::new().add_modifier(Modifier::BOLD);
    let spans = snippet_spans("a \u{1}hit\u{2} b\n   c", Style::new(), hl);
    let text: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, vec!["a ", "hit", " b c"]);
    assert_eq!(spans[1].style, hl);
}

#[test]
fn highlight_splits_across_spans() {
    let hl = Style::new().add_modifier(Modifier::REVERSED);
    let line = Line::from(vec![Span::raw("foo Ba"), Span::raw("r baz")]);
    let out = highlight(&line, "bar", hl);
    let pieces: Vec<(&str, bool)> = out
        .spans
        .iter()
        .map(|s| (s.content.as_ref(), s.style == hl))
        .collect();
    assert_eq!(
        pieces,
        vec![("foo ", false), ("Ba", true), ("r", true), (" baz", false)]
    );
}
