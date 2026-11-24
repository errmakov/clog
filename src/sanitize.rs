//! Neutralising transcript text before it reaches a terminal.
//!
//! Transcript content (tool output especially) may contain escape sequences
//! that would retitle the window, rewrite the clipboard (OSC 52) or move the
//! cursor. Everything shown in the TUI, written by `clog export` or handed to
//! the pager goes through [`sanitize`].

use std::iter::Peekable;
use std::str::Chars;

const TAB: &str = "    ";

/// Removes ANSI CSI/OSC escape sequences, other C0/C1 controls and bidi
/// override characters. Tabs become four spaces; newlines are kept.
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str(TAB),
            '\u{1b}' => skip_escape(&mut chars),
            c if c.is_control() || is_bidi_control(c) => {}
            c => out.push(c),
        }
    }
    out
}

/// Strips everything [`sanitize`] strips and also newlines and tabs, for
/// values that must stay on one line (paths, ids).
pub fn sanitize_inline(s: &str) -> String {
    s.chars()
        .filter(|&c| !c.is_control() && !is_bidi_control(c))
        .collect()
}

/// Bidi embedding/override/isolate controls, which can visually reorder text.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}' | '\u{061c}')
}

/// Consumes the escape sequence after an ESC. Malformed sequences stop at
/// the first character that cannot belong to them, so ordinary text after a
/// stray ESC is not swallowed.
fn skip_escape(chars: &mut Peekable<Chars<'_>>) {
    match chars.peek() {
        Some('[') => {
            chars.next();
            // CSI: parameter/intermediate bytes 0x20–0x3F, then a final byte 0x40–0x7E.
            while chars
                .next_if(|c| ('\u{20}'..='\u{3f}').contains(c))
                .is_some()
            {}
            chars.next_if(|c| ('@'..='~').contains(c));
        }
        Some(']') => {
            chars.next();
            // OSC: until BEL or ESC \ (or end of line, for unterminated ones).
            while let Some(c) = chars.next_if(|&c| c != '\n') {
                if c == '\u{7}' {
                    break;
                }
                if c == '\u{1b}' {
                    chars.next_if_eq(&'\\');
                    break;
                }
            }
        }
        Some(c) if ('\u{20}'..='\u{7e}').contains(c) => {
            chars.next();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_escapes_and_controls() {
        assert_eq!(sanitize("\u{1b}[31mred\u{1b}[0m ok"), "red ok");
        assert_eq!(sanitize("a\u{1b}]52;c;Zm9v\u{7}b"), "ab");
        assert_eq!(sanitize("a\u{1b}]0;title\u{1b}\\b"), "ab");
        assert_eq!(sanitize("x\r\ny\tz\u{0}\u{9b}"), "x\ny    z");
        assert_eq!(sanitize("\u{1b}7saved\u{1b}8"), "saved");
    }

    #[test]
    fn malformed_escapes_do_not_swallow_text() {
        assert_eq!(sanitize("a\u{1b}[ünicode"), "aünicode");
        assert_eq!(sanitize("a\u{1b}]unterminated\nnext line"), "a\nnext line");
        assert_eq!(sanitize("tail\u{1b}"), "tail");
    }

    #[test]
    fn strips_bidi_overrides() {
        assert_eq!(sanitize("abc\u{202e}fed"), "abcfed");
        assert_eq!(sanitize_inline("/home/\u{2066}x\ny\u{1b}"), "/home/xy");
    }
}
