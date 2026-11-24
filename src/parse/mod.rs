//! JSONL transcript → [`Transcript`].

mod classify;
mod raw;

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::model::{
    Block, Compaction, Item, Message, Meta, Role, ToolCall, ToolResult, Transcript,
};
use classify::{UserText, classify};
use raw::{RawBlock, RawContent, Record};

const TITLE_FALLBACK_CHARS: usize = 100;

#[derive(Debug, Clone, Copy)]
pub struct ParseOptions {
    /// Keep records flagged `isSidechain` (true for subagent transcript files).
    pub include_sidechain: bool,
    /// Keep tool result bodies; the indexer drops them to save memory.
    pub keep_tool_output: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            include_sidechain: false,
            keep_tool_output: true,
        }
    }
}

pub fn parse_file(path: &Path, opts: ParseOptions) -> Result<Transcript> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    parse_reader(BufReader::new(file), opts).with_context(|| format!("read {}", path.display()))
}

pub fn parse_reader<R: BufRead>(mut reader: R, opts: ParseOptions) -> Result<Transcript> {
    let mut state = State::new(opts);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let line = buf.trim_ascii();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Record>(line) {
            Ok(rec) => state.push(rec),
            Err(_) => state.meta.bad_lines += 1,
        }
    }
    Ok(state.finish())
}

struct State {
    opts: ParseOptions,
    items: Vec<Item>,
    meta: Meta,
    last_summary: Option<String>,
    /// tool_use id → (item index, block index)
    tools: HashMap<String, (usize, usize)>,
}

impl State {
    fn new(opts: ParseOptions) -> Self {
        Self {
            opts,
            items: Vec::new(),
            meta: Meta::default(),
            last_summary: None,
            tools: HashMap::new(),
        }
    }

    fn push(&mut self, rec: Record) {
        self.track_meta(&rec);
        if rec.is_sidechain == Some(true) && !self.opts.include_sidechain {
            return;
        }
        let time = rec.timestamp.as_deref().and_then(parse_time);
        match rec.kind.as_deref() {
            Some("ai-title") => {
                if let Some(t) = rec.ai_title.filter(|t| !t.trim().is_empty()) {
                    self.meta.title = Some(t);
                }
            }
            Some("summary") => {
                if let Some(s) = rec.summary.as_ref().and_then(|s| s.as_str()) {
                    self.last_summary = Some(s.to_string());
                }
            }
            Some("system") if rec.subtype.as_deref() == Some("compact_boundary") => {
                let meta = rec.compact.as_ref();
                self.items.push(Item::Compaction(Compaction {
                    time,
                    trigger: meta.and_then(|m| m.trigger.clone()),
                    pre_tokens: meta.and_then(|m| m.pre_tokens),
                    post_tokens: meta.and_then(|m| m.post_tokens),
                    summary: None,
                }));
            }
            Some("user") => self.push_user(rec, time),
            Some("assistant") => self.push_assistant(rec, time),
            _ => {}
        }
    }

    fn track_meta(&mut self, rec: &Record) {
        if let Some(t) = rec.timestamp.as_deref().and_then(parse_time) {
            self.meta.started = Some(self.meta.started.map_or(t, |s| s.min(t)));
            self.meta.ended = Some(self.meta.ended.map_or(t, |e| e.max(t)));
        }
        if self.meta.session_id.is_none() {
            self.meta.session_id.clone_from(&rec.session_id);
        }
        if self.meta.cwd.is_none() {
            self.meta.cwd.clone_from(&rec.cwd);
        }
        if rec.git_branch.as_deref().is_some_and(|b| !b.is_empty()) {
            self.meta.branch.clone_from(&rec.git_branch);
        }
    }

    fn push_user(&mut self, rec: Record, time: Option<DateTime<Utc>>) {
        if rec.is_meta == Some(true) {
            return;
        }
        let Some(content) = rec.message.and_then(|m| m.content) else {
            return;
        };
        let blocks = match content {
            RawContent::Text(t) if t.trim().is_empty() => Vec::new(),
            RawContent::Text(t) => vec![Block::Text(t)],
            RawContent::Blocks(raw) => self.user_blocks(raw),
            RawContent::Other(_) => return,
        };
        if rec.is_compact_summary == Some(true) {
            self.attach_summary(blocks, time);
            return;
        }
        if blocks.is_empty() {
            return;
        }
        let (role, blocks) = reclassify(blocks);
        self.push_message(role, time, blocks);
    }

    /// Converts user content blocks; tool results are attached to their calls
    /// and do not appear as user blocks.
    fn user_blocks(&mut self, raw: Vec<RawBlock>) -> Vec<Block> {
        let mut out = Vec::new();
        for b in raw {
            match b {
                RawBlock::Text { text: Some(t) } if !t.trim().is_empty() => {
                    out.push(Block::Text(t))
                }
                RawBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let text = if self.opts.keep_tool_output {
                        content.map(|c| c.into_text()).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    self.attach_result(
                        tool_use_id,
                        ToolResult {
                            text,
                            is_error: is_error == Some(true),
                        },
                    );
                }
                RawBlock::Image {} => out.push(Block::Attachment("image".into())),
                RawBlock::Document {} => out.push(Block::Attachment("document".into())),
                _ => {}
            }
        }
        out
    }

    fn attach_result(&mut self, id: Option<String>, result: ToolResult) {
        let Some(&(item, block)) = id.as_ref().and_then(|id| self.tools.get(id)) else {
            return;
        };
        if let Some(Item::Message(m)) = self.items.get_mut(item)
            && let Some(Block::Tool(call)) = m.blocks.get_mut(block)
        {
            call.result = Some(result);
        }
    }

    fn attach_summary(&mut self, blocks: Vec<Block>, time: Option<DateTime<Utc>>) {
        let text = blocks
            .into_iter()
            .filter_map(|b| {
                if let Block::Text(t) = b {
                    Some(t)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        match self.items.last_mut() {
            Some(Item::Compaction(c)) if c.summary.is_none() => c.summary = Some(text),
            _ => self.items.push(Item::Compaction(Compaction {
                time,
                summary: Some(text),
                ..Default::default()
            })),
        }
    }

    fn push_assistant(&mut self, rec: Record, time: Option<DateTime<Utc>>) {
        let Some(msg) = rec.message else { return };
        if msg.model.as_deref().is_some_and(|m| !m.starts_with('<')) {
            self.meta.model.clone_from(&msg.model);
        }
        let raw = match msg.content {
            Some(RawContent::Blocks(b)) => b,
            Some(RawContent::Text(t)) => vec![RawBlock::Text { text: Some(t) }],
            _ => return,
        };
        let blocks: Vec<Block> = raw.into_iter().filter_map(assistant_block).collect();
        if blocks.is_empty() {
            return;
        }
        // Consecutive assistant records (split per content block, interleaved
        // with tool results) form one assistant turn.
        let item = self.items.len().wrapping_sub(1);
        if let Some(Item::Message(last)) = self.items.get_mut(item)
            && last.role == Role::Assistant
        {
            for b in blocks {
                if let Block::Tool(t) = &b {
                    self.tools.insert(t.id.clone(), (item, last.blocks.len()));
                }
                last.blocks.push(b);
            }
            return;
        }
        self.push_message(Role::Assistant, time, blocks);
    }

    fn push_message(&mut self, role: Role, time: Option<DateTime<Utc>>, blocks: Vec<Block>) {
        let item = self.items.len();
        for (i, b) in blocks.iter().enumerate() {
            if let Block::Tool(t) = b {
                self.tools.insert(t.id.clone(), (item, i));
            }
        }
        if role == Role::User && self.meta.first_prompt.is_none() {
            self.meta.first_prompt = blocks.iter().find_map(|b| match b {
                Block::Text(t) if is_real_prompt(t) => Some(t.trim().to_string()),
                _ => None,
            });
        }
        self.meta.messages += 1;
        self.items.push(Item::Message(Message {
            no: self.meta.messages,
            role,
            time,
            blocks,
        }));
    }

    fn finish(mut self) -> Transcript {
        self.meta.compactions = self
            .items
            .iter()
            .filter(|i| matches!(i, Item::Compaction(_)))
            .count();
        if self.meta.title.is_none() {
            self.meta.title = self.last_summary.take().or_else(|| {
                self.meta
                    .first_prompt
                    .as_deref()
                    .map(|p| one_line(p, TITLE_FALLBACK_CHARS))
            });
        }
        Transcript {
            items: self.items,
            meta: self.meta,
        }
    }
}

/// Applies [`classify`] to a user record whose first block is text.
fn reclassify(mut blocks: Vec<Block>) -> (Role, Vec<Block>) {
    let Some(Block::Text(first)) = blocks.first() else {
        return (Role::User, blocks);
    };
    match classify(first) {
        None => (Role::User, blocks),
        Some(UserText::Prompt(p)) => {
            blocks[0] = Block::Text(p);
            (Role::User, blocks)
        }
        Some(UserText::Event(e)) => (Role::Event, vec![Block::Text(e)]),
    }
}

fn assistant_block(b: RawBlock) -> Option<Block> {
    match b {
        RawBlock::Text { text: Some(t) } if !t.trim().is_empty() => Some(Block::Text(t)),
        RawBlock::Thinking { thinking: Some(t) } if !t.trim().is_empty() => {
            Some(Block::Thinking(t))
        }
        RawBlock::ToolUse { id, name, input } => Some(Block::Tool(ToolCall {
            id: id.unwrap_or_default(),
            name: name.unwrap_or_else(|| "tool".into()),
            input: input.unwrap_or(serde_json::Value::Null),
            result: None,
        })),
        _ => None,
    }
}

/// Prompts made of harness markup (slash-command echoes, reminders) are not
/// useful as a title or first-prompt preview.
fn is_real_prompt(t: &str) -> bool {
    let t = t.trim_start();
    let bare_command = t.starts_with('/') && !t.trim_end().contains(' ');
    !t.is_empty() && !bare_command && !t.starts_with('<') && !t.starts_with("[Request interrupted")
}

pub fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Collapses whitespace and truncates to `max` characters with an ellipsis.
pub fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests;
