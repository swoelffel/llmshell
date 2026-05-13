//! Serde types for the Anthropic Messages API (`POST /v1/messages`).
//!
//! Reference: <https://docs.anthropic.com/en/api/messages>
//!
//! Notes:
//! - `max_tokens` is REQUIRED in the request body.
//! - System prompt lives at top-level (`system: string`), not as a message.
//! - Messages alternate `user` / `assistant` only; tool results are encoded as
//!   `tool_result` blocks inside a `user` message; tool calls as `tool_use`
//!   blocks inside an `assistant` message.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Debug)]
pub struct MessagesRequest<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<WireToolChoice>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WireMessage {
    pub role: String, // "user" | "assistant"
    pub content: Vec<ContentBlock>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Serialize, Debug)]
pub struct WireTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireToolChoice {
    Auto,
    Any,
    None,
    Tool { name: String },
}

#[derive(Deserialize, Debug)]
pub struct MessagesResponse {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: Option<u32>,
    #[serde(default)]
    pub output_tokens: Option<u32>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct ModelsResponse {
    pub data: Vec<ModelEntry>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn content_block_text_serializes_with_type_tag() {
        let block = ContentBlock::Text {
            text: "hello".into(),
        };
        let s = serde_json::to_string(&block).unwrap();
        assert_eq!(s, r#"{"type":"text","text":"hello"}"#);
    }

    #[test]
    fn content_block_tool_use_roundtrip() {
        let block = ContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "list_directory".into(),
            input: json!({"path": "."}),
        };
        let s = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&s).unwrap();
        assert_eq!(block, back);
    }

    #[test]
    fn content_block_tool_result_roundtrip() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "toolu_1".into(),
            content: r#"{"status":"success"}"#.into(),
        };
        let s = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&s).unwrap();
        assert_eq!(block, back);
    }

    #[test]
    fn tool_choice_any_serializes_with_type_tag() {
        let c = WireToolChoice::Any;
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(s, r#"{"type":"any"}"#);
    }

    #[test]
    fn messages_response_parses_text_and_tool_use_mix() {
        let raw = r#"{
            "content":[
                {"type":"text","text":"I'll list it."},
                {"type":"tool_use","id":"toolu_42","name":"list_directory","input":{"path":"."}}
            ],
            "stop_reason":"tool_use",
            "usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":50}
        }"#;
        let parsed: MessagesResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.content.len(), 2);
        assert_eq!(parsed.stop_reason.as_deref(), Some("tool_use"));
        let u = parsed.usage.unwrap();
        assert_eq!(u.input_tokens, Some(100));
        assert_eq!(u.cache_read_input_tokens, Some(50));
    }

    #[test]
    fn request_skips_empty_tools_and_none_system() {
        let req = MessagesRequest {
            model: "claude-haiku-4-5",
            max_tokens: 4096,
            system: None,
            messages: vec![WireMessage {
                role: "user".into(),
                content: vec![ContentBlock::Text { text: "hi".into() }],
            }],
            tools: vec![],
            tool_choice: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(!s.contains("\"system\""));
        assert!(!s.contains("\"tools\""));
        assert!(!s.contains("\"tool_choice\""));
        assert!(s.contains("\"max_tokens\":4096"));
    }
}
