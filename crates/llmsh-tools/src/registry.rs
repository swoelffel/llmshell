use crate::tool::Tool;
use llmsh_llm::types::{ToolCall, ToolSpec};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(thiserror::Error, Debug)]
pub enum RegistryError {
    #[error("tool not in registry: {0}")]
    NotFound(String),
}

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self { Self { tools: HashMap::new() } }
    pub fn register(&mut self, t: Arc<dyn Tool>) { self.tools.insert(t.name().to_string(), t); }
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| ToolSpec {
            name: t.name().to_string(),
            description: t.description().to_string(),
            input_schema: t.input_schema(),
        }).collect()
    }
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> { self.tools.get(name).cloned() }
    pub fn validate_call(&self, call: &ToolCall) -> Result<Arc<dyn Tool>, RegistryError> {
        self.get(&call.name).ok_or_else(|| RegistryError::NotFound(call.name.clone()))
    }
}

impl Default for ToolRegistry { fn default() -> Self { Self::new() } }
