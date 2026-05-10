use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<&'static str>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WireMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<WireToolCall>>,
    /// Optional tool name (for role = "tool"). Ollama ignores this on input
    /// but we emit it for symmetry; OpenAI-style tool_call_id is not used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Serialize)]
pub struct WireTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireFunction,
}

#[derive(Serialize)]
pub struct WireFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WireToolCall {
    pub function: WireFunctionCall,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WireFunctionCall {
    pub name: String,
    /// Ollama returns a JSON object directly (not a stringified JSON like OpenAI).
    pub arguments: Value,
}

#[derive(Deserialize)]
pub struct ChatResponse {
    pub message: WireMessage,
    #[serde(default)]
    pub done_reason: Option<String>,
    #[serde(default)]
    pub prompt_eval_count: Option<u32>,
    #[serde(default)]
    pub eval_count: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct TagsResponse {
    pub models: Vec<TagEntry>,
}

#[derive(Deserialize)]
pub(crate) struct TagEntry {
    pub name: String,
    #[serde(default)]
    pub modified_at: Option<String>,
}
