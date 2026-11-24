//! Transcript → Markdown, for `clog export` and the viewer's pager action.

use std::fmt::Write as _;

use chrono::{DateTime, Local, Utc};

use crate::model::{Block, Compaction, Item, Message, Role, Transcript};
use crate::sanitize::sanitize;

#[derive(Debug, Clone, Copy, Default)]
pub struct ExportOptions {
    pub include_thinking: bool,
}

pub fn to_markdown(t: &Transcript, opts: ExportOptions) -> String {
    let mut out = String::new();
    let m = &t.meta;
    let _ = writeln!(out, "# {}\n", m.title.as_deref().unwrap_or("(untitled)"));
    let mut facts = Vec::new();
    if let Some(id) = &m.session_id {
        facts.push(format!("session `{id}`"));
    }
    if let Some(cwd) = &m.cwd {
        facts.push(format!("cwd `{cwd}`"));
    }
    if let Some(s) = m.started {
        facts.push(format!("started {}", fmt_time(s)));
    }
    facts.push(format!(
        "{} messages, {} compactions",
        m.messages, m.compactions
    ));
    let _ = writeln!(out, "{}\n", facts.join(" · "));

    for item in &t.items {
        match item {
            Item::Message(msg) => write_message(&mut out, msg, opts),
            Item::Compaction(c) => write_compaction(&mut out, c),
        }
    }
    // Output goes to terminals (stdout, $PAGER): neutralise escape sequences.
    sanitize(&out)
}

fn write_message(out: &mut String, msg: &Message, opts: ExportOptions) {
    let who = match msg.role {
        Role::User => "You",
        Role::Assistant => "Claude",
    };
    let when = msg
        .time
        .map(|t| format!(" · {}", fmt_time(t)))
        .unwrap_or_default();
    let _ = writeln!(out, "---\n\n## #{} {who}{when}\n", msg.no);
    for b in &msg.blocks {
        match b {
            Block::Text(t) => {
                let _ = writeln!(out, "{}\n", t.trim_end());
            }
            Block::Thinking(t) if opts.include_thinking => {
                let quoted: Vec<String> = t.lines().map(|l| format!("> {l}")).collect();
                let _ = writeln!(out, "> *thinking*\n>\n{}\n", quoted.join("\n"));
            }
            Block::Thinking(_) => {}
            Block::Attachment(kind) => {
                let _ = writeln!(out, "*[{kind}]*\n");
            }
            Block::Tool(call) => {
                let _ = writeln!(
                    out,
                    "**▸ {}** `{}`\n",
                    call.name,
                    call.summary().replace('`', "'")
                );
                let input = serde_json::to_string_pretty(&call.input).unwrap_or_default();
                let _ = writeln!(out, "{}", fence("json", &input));
                if let Some(r) = &call.result {
                    let label = if r.is_error { "error" } else { "output" };
                    let _ = writeln!(out, "{}", fence(label, &r.text));
                }
            }
        }
    }
}

fn write_compaction(out: &mut String, c: &Compaction) {
    let when = c
        .time
        .map(|t| format!(" {}", fmt_time(t)))
        .unwrap_or_default();
    let tokens = match (c.pre_tokens, c.post_tokens) {
        (Some(a), Some(b)) => format!(" · {} → {} tokens", kilo(a), kilo(b)),
        _ => String::new(),
    };
    let _ = writeln!(out, "---\n\n## ⧉ Compacted{when}{tokens}\n");
    if let Some(s) = &c.summary {
        let _ = writeln!(
            out,
            "<details><summary>Summary Claude saw after compaction</summary>\n\n{}\n\n</details>\n",
            s.trim_end()
        );
    }
}

/// Fenced code block whose fence is longer than any backtick run inside.
fn fence(lang: &str, body: &str) -> String {
    let longest = body.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let ticks = "`".repeat(longest.max(2) + 1);
    format!("{ticks}{lang}\n{}\n{ticks}\n", body.trim_end())
}

pub fn fmt_time(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()
}

pub fn kilo(n: u64) -> String {
    if n >= 1000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{ParseOptions, parse_reader};

    const FIXTURE: &str = include_str!("../tests/fixtures/compacted.jsonl");

    #[test]
    fn exports_dialog_tools_and_compaction() {
        let t = parse_reader(FIXTURE.as_bytes(), ParseOptions::default()).unwrap();
        let md = to_markdown(&t, ExportOptions::default());
        assert!(md.starts_with("# Fix webhook retry on timeout"));
        assert!(md.contains("## #1 You"));
        assert!(md.contains("**▸ Bash** `cargo test retry`"));
        assert!(md.contains("```error\ntest retry ... FAILED\n```"));
        assert!(md.contains("⧉ Compacted") && md.contains("129k → 13k tokens"));
        assert!(
            !md.contains("Look at the classifier"),
            "thinking excluded by default"
        );
        let with = to_markdown(
            &t,
            ExportOptions {
                include_thinking: true,
            },
        );
        assert!(with.contains("> Look at the classifier first."));
    }

    #[test]
    fn fence_outgrows_inner_backticks() {
        assert_eq!(fence("x", "a ```` b"), "`````x\na ```` b\n`````\n");
        assert_eq!(fence("", "plain"), "```\nplain\n```\n");
    }
}
