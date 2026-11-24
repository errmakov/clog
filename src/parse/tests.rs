use super::*;
use crate::model::{Block, Item, Role};

const FIXTURE: &str = include_str!("../../tests/fixtures/compacted.jsonl");

fn parse(opts: ParseOptions) -> Transcript {
    parse_reader(FIXTURE.as_bytes(), opts).expect("fixture parses")
}

#[test]
fn extracts_session_metadata() {
    let t = parse(ParseOptions::default());
    let m = &t.meta;
    assert_eq!(m.session_id.as_deref(), Some("s-1"));
    assert_eq!(m.cwd.as_deref(), Some("/home/alice/www/demo"));
    assert_eq!(m.branch.as_deref(), Some("main"));
    assert_eq!(m.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(m.title.as_deref(), Some("Fix webhook retry on timeout"));
    assert_eq!(
        m.first_prompt.as_deref(),
        Some("Fix the retry loop in the webhook worker")
    );
    assert_eq!(m.compactions, 1);
    assert_eq!(m.messages, 4);
    assert_eq!(m.bad_lines, 1, "truncated tail line is counted, not fatal");
    assert_eq!(m.started, parse_time("2026-11-20T10:00:00Z"));
    assert_eq!(m.ended, parse_time("2026-11-20T11:05:10Z"));
}

#[test]
fn groups_assistant_turn_and_attaches_tool_result() {
    let t = parse(ParseOptions::default());
    let Item::Message(turn) = &t.items[1] else {
        panic!("expected message")
    };
    assert_eq!(turn.role, Role::Assistant);
    assert_eq!(turn.no, 2);
    // thinking, text, tool, text — split records and msg_2 merged into one turn
    assert_eq!(turn.blocks.len(), 4);
    let Block::Tool(call) = &turn.blocks[2] else {
        panic!("expected tool")
    };
    assert_eq!(call.name, "Bash");
    let result = call.result.as_ref().expect("result attached");
    assert!(result.is_error);
    assert_eq!(result.text, "test retry ... FAILED");
}

#[test]
fn compaction_boundary_carries_summary_and_tokens() {
    let t = parse(ParseOptions::default());
    let Item::Compaction(c) = &t.items[2] else {
        panic!("expected compaction at index 2")
    };
    assert_eq!(c.trigger.as_deref(), Some("auto"));
    assert_eq!(c.pre_tokens, Some(129_905));
    assert_eq!(c.post_tokens, Some(13_447));
    assert!(
        c.summary
            .as_deref()
            .unwrap()
            .contains("Summary: retry fix.")
    );
}

#[test]
fn skips_meta_and_sidechain_records_by_default() {
    let t = parse(ParseOptions::default());
    let texts: Vec<String> = t.messages().map(|m| m.dialog_text()).collect();
    assert!(
        !texts
            .iter()
            .any(|s| s.contains("sidechain") || s.contains("caveat"))
    );
}

#[test]
fn includes_sidechain_when_requested() {
    let t = parse(ParseOptions {
        include_sidechain: true,
        ..Default::default()
    });
    assert!(t.messages().any(|m| m.dialog_text() == "sidechain prompt"));
}

#[test]
fn drops_tool_output_when_not_kept() {
    let t = parse(ParseOptions {
        keep_tool_output: false,
        ..Default::default()
    });
    let Item::Message(turn) = &t.items[1] else {
        panic!()
    };
    let Block::Tool(call) = &turn.blocks[2] else {
        panic!()
    };
    assert_eq!(call.result.as_ref().unwrap().text, "");
}

#[test]
fn keeps_attachments_and_ignores_unknown_blocks() {
    let t = parse(ParseOptions::default());
    let msgs: Vec<_> = t.messages().collect();
    assert_eq!(msgs[2].blocks[1], Block::Attachment("image".into()));
    assert_eq!(msgs[3].blocks.len(), 1);
}

#[test]
fn title_falls_back_to_first_prompt() {
    let jsonl = r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}
{"type":"user","message":{"content":"Explain   the\nindexer"}}"#;
    let t = parse_reader(jsonl.as_bytes(), ParseOptions::default()).unwrap();
    assert_eq!(t.meta.title.as_deref(), Some("Explain the indexer"));
}

#[test]
fn one_line_truncates_with_ellipsis() {
    assert_eq!(one_line("a  b\nc", 10), "a b c");
    assert_eq!(one_line("abcdefghij", 5), "abcd…");
}
