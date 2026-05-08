use crate::capabilities::Capabilities;
use crate::types::{LlmRequest, LlmResponse, ModelInfo};
use async_trait::async_trait;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    async fn complete(&self, req: LlmRequest) -> anyhow::Result<LlmResponse>;

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Err(anyhow::anyhow!("provider does not support listing models"))
    }

    async fn set_model(&self, id: &str) -> anyhow::Result<()> {
        let _ = id;
        Err(anyhow::anyhow!(
            "provider does not support runtime model switching"
        ))
    }

    fn current_model(&self) -> String {
        "unknown".to_string()
    }
}
