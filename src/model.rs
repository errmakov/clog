//! Parsed, display-oriented view of a session transcript.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// Harness-injected record stored with the user role (task
    /// notification, command output, interruption).
    Event,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Event => "event",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub result: Option<ToolResult>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Thinking(String),
    Tool(ToolCall),
    /// Non-text attachment such as an image or document.
    Attachment(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// 1-based position among the transcript's messages; stable between the
    /// indexer and the viewer because both use the same parser.
    pub no: usize,
    pub role: Role,
    pub time: Option<DateTime<Utc>>,
    pub blocks: Vec<Block>,
}

impl Message {
    /// Concatenated plain dialog text (no thinking, no tool payloads).
    pub fn dialog_text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Compaction {
    pub time: Option<DateTime<Utc>>,
    pub trigger: Option<String>,
    pub pre_tokens: Option<u64>,
    pub post_tokens: Option<u64>,
    /// The summary Claude received after the compaction.
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Message(Message),
    Compaction(Compaction),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
    pub model: Option<String>,
    pub branch: Option<String>,
    pub messages: usize,
    pub compactions: usize,
    /// Lines that were not valid JSON records (e.g. a partially written tail).
    pub bad_lines: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcript {
    pub items: Vec<Item>,
    pub meta: Meta,
}

impl Transcript {
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.items.iter().filter_map(|i| match i {
            Item::Message(m) => Some(m),
            Item::Compaction(_) => None,
        })
    }
}

impl ToolCall {
    /// One-line description of the call, e.g. `cargo test` for Bash.
    pub fn summary(&self) -> String {
        let field = |k: &str| {
            self.input
                .get(k)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let picked = match self.name.as_str() {
            "Bash" => field("command"),
            "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                field("file_path").or_else(|| field("notebook_path"))
            }
            "Grep" => field("pattern").map(|p| match field("path") {
                Some(path) => format!("\"{p}\" in {path}"),
                None => format!("\"{p}\""),
            }),
            "Glob" => field("pattern"),
            "WebFetch" => field("url"),
            "WebSearch" => field("query"),
            "Task" | "Agent" => field("description").or_else(|| field("prompt")),
            "Skill" => field("skill"),
            _ => None,
        };
        let text = picked.unwrap_or_else(|| first_string(&self.input).unwrap_or_default());
        crate::parse::one_line(&text, 200)
    }
}

fn first_string(v: &serde_json::Value) -> Option<String> {
    v.as_object()?
        .values()
        .find_map(|x| x.as_str().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "t".into(),
            name: name.into(),
            input,
            result: None,
        }
    }

    #[test]
    fn summarises_known_tools() {
        assert_eq!(
            call("Bash", json!({"command": "cargo  test\n-q"})).summary(),
            "cargo test -q"
        );
        assert_eq!(
            call("Read", json!({"file_path": "/a/b.rs"})).summary(),
            "/a/b.rs"
        );
        assert_eq!(
            call("Grep", json!({"pattern": "foo", "path": "src"})).summary(),
            "\"foo\" in src"
        );
        assert_eq!(call("Grep", json!({"pattern": "foo"})).summary(), "\"foo\"");
    }

    #[test]
    fn summarises_unknown_tools_by_first_string() {
        assert_eq!(
            call("mcp__x__y", json!({"n": 1, "q": "hello"})).summary(),
            "hello"
        );
        assert_eq!(call("Odd", json!(null)).summary(), "");
    }
}
