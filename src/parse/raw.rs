//! Serde shapes for the on-disk JSONL records. Every field is optional so an
//! unexpected or newer record shape degrades to "ignored" instead of failing.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Record {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(rename = "gitBranch", default)]
    pub git_branch: Option<String>,
    #[serde(rename = "isSidechain", default)]
    pub is_sidechain: Option<bool>,
    #[serde(rename = "isMeta", default)]
    pub is_meta: Option<bool>,
    #[serde(rename = "isCompactSummary", default)]
    pub is_compact_summary: Option<bool>,
    #[serde(default)]
    pub message: Option<RawMessage>,
    #[serde(rename = "aiTitle", default)]
    pub ai_title: Option<String>,
    #[serde(default)]
    pub summary: Option<serde_json::Value>,
    #[serde(rename = "compactMetadata", default)]
    pub compact: Option<CompactMeta>,
}

#[derive(Debug, Deserialize)]
pub struct CompactMeta {
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(rename = "preTokens", default)]
    pub pre_tokens: Option<u64>,
    #[serde(rename = "postTokens", default)]
    pub post_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct RawMessage {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub content: Option<RawContent>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum RawContent {
    Text(String),
    Blocks(Vec<RawBlock>),
    Other(serde::de::IgnoredAny),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RawBlock {
    Text {
        #[serde(default)]
        text: Option<String>,
    },
    Thinking {
        #[serde(default)]
        thinking: Option<String>,
    },
    ToolUse {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        input: Option<serde_json::Value>,
    },
    ToolResult {
        #[serde(default)]
        tool_use_id: Option<String>,
        #[serde(default)]
        content: Option<ResultContent>,
        #[serde(default)]
        is_error: Option<bool>,
    },
    Image {},
    Document {},
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ResultContent {
    Text(String),
    Parts(Vec<serde_json::Value>),
    Other(serde_json::Value),
}

impl ResultContent {
    pub fn into_text(self) -> String {
        match self {
            ResultContent::Text(t) => t,
            ResultContent::Parts(parts) => parts
                .iter()
                .map(|p| match p.get("text").and_then(|t| t.as_str()) {
                    Some(t) => t.to_string(),
                    None => format!(
                        "[{}]",
                        p.get("type").and_then(|t| t.as_str()).unwrap_or("?")
                    ),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            ResultContent::Other(v) => v.to_string(),
        }
    }
}
