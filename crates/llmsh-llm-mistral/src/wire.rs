use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
pub(crate) struct ModelsResponse {
    pub data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
pub(crate) struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub owned_by: Option<String>,
    #[serde(default)]
    pub created: Option<i64>,
}

#[derive(Serialize)]
pub struct WireResponseFormat {
    #[serde(rename = "type")]
    pub kind: &'static str,
}

#[derive(Serialize)]
pub struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<WireResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WireMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<WireToolCall>>,
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

#[derive(Serialize, Deserialize, Clone)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: WireFunctionCall,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WireFunctionCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
}

#[derive(Deserialize)]
pub struct Choice {
    pub message: WireMessage,
    pub finish_reason: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct Usage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}
