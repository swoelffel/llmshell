use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallingMode {
    Native,
    JsonContent,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub tool_calling: ToolCallingMode,
    pub supports_streaming: bool,
    pub supports_json_mode: bool,
    pub supports_parallel_tool_calls: bool,
    pub supports_tool_choice_required: bool,
    pub max_context_tokens: Option<u32>,
}
