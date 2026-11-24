use std::collections::HashSet;

use super::*;
use crate::parse::{ParseOptions, parse_reader};

const FIXTURE: &str = include_str!("../../../tests/fixtures/compacted.jsonl");

fn transcript() -> Transcript {
    parse_reader(FIXTURE.as_bytes(), ParseOptions::default()).unwrap()
}

fn texts(lines: &[RLine]) -> Vec<String> {
    lines.iter().map(RLine::text).collect()
}

fn opts(expanded: &HashSet<Fold>) -> RenderOpts<'_> {
    RenderOpts {
        width: 60,
        expanded,
        all_tools: false,
        show_thinking: false,
    }
}

#[test]
fn renders_collapsed_tool_and_banner() {
    let t = transcript();
    let none = HashSet::new();
    let lines = render(&t, &opts(&none));
    let all = texts(&lines);
    assert!(
        all.iter()
            .any(|l| l.starts_with("┃ you") && l.trim_end().ends_with("#1"))
    );
    assert!(
        all.iter().any(|l| l == "▸ Bash cargo test retry ✗ error"),
        "{all:#?}"
    );
    assert!(
        !all.iter().any(|l| l.contains("FAILED")),
        "collapsed tool hides output"
    );
    assert!(
        !all.iter().any(|l| l.contains("Look at the classifier")),
        "thinking hidden"
    );
    let banner = lines.iter().find(|l| l.kind == Kind::Boundary).unwrap();
    assert!(banner.text().contains("⧉ COMPACTED") && banner.text().contains("129k → 13k"));
    assert!(lines.iter().all(|l| l.line.width() <= 60), "fits width");
}

#[test]
fn expanding_tool_and_summary_shows_bodies() {
    let t = transcript();
    let expanded: HashSet<Fold> = [Fold::Tool { item: 1, block: 2 }, Fold::Summary { item: 2 }]
        .into_iter()
        .collect();
    let all = texts(&render(&t, &opts(&expanded)));
    assert!(all.iter().any(|l| l == "  │ $ cargo test retry"));
    assert!(all.iter().any(|l| l == "  │ test retry ... FAILED"));
    assert!(all.iter().any(|l| l.contains("Summary: retry fix.")));
}

#[test]
fn all_tools_inverts_individual_toggles() {
    let t = transcript();
    let one: HashSet<Fold> = [Fold::Tool { item: 1, block: 2 }].into_iter().collect();
    let o = RenderOpts {
        all_tools: true,
        ..opts(&one)
    };
    assert!(
        texts(&render(&t, &o))
            .iter()
            .any(|l| l.starts_with("▸ Bash"))
    );
    let none = HashSet::new();
    let o = RenderOpts {
        all_tools: true,
        ..opts(&none)
    };
    assert!(
        texts(&render(&t, &o))
            .iter()
            .any(|l| l.starts_with("▾ Bash"))
    );
}

#[test]
fn thinking_shown_on_request_and_lines_tagged() {
    let t = transcript();
    let none = HashSet::new();
    let o = RenderOpts {
        show_thinking: true,
        ..opts(&none)
    };
    let lines = render(&t, &o);
    assert!(
        texts(&lines)
            .iter()
            .any(|l| l.contains("Look at the classifier first."))
    );
    let tool = lines
        .iter()
        .find(|l| l.text().starts_with("▸ Bash"))
        .unwrap();
    assert_eq!(tool.fold, Some(Fold::Tool { item: 1, block: 2 }));
    assert_eq!(tool.msg_no, Some(2));
    assert_eq!(lines.iter().filter(|l| l.kind == Kind::Prompt).count(), 2);
}

#[test]
fn markdown_styles_code_and_wraps_prose() {
    let lines = markdown("# Title\nsome `code` here\n```\n  let x = 1;\n```", 40);
    let t: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert_eq!(
        t,
        vec!["# Title", "some code here", "```", "  let x = 1;", "```"]
    );
    assert_eq!(lines[1].spans[1].style.fg, Some(Color::Green));
}

#[test]
fn long_tool_output_is_capped() {
    let mut out = Out {
        lines: Vec::new(),
        msg: None,
        fold: None,
    };
    let text = (0..MAX_TOOL_LINES + 5)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    boxed(&mut out, &text, 40, Style::new());
    assert_eq!(out.lines.len(), MAX_TOOL_LINES + 1);
    assert!(out.lines.last().unwrap().text().contains("5 more lines"));
}
