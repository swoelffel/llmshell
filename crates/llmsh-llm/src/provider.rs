use crate::capabilities::Capabilities;
use crate::types::{LlmRequest, LlmResponse};
use async_trait::async_trait;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    async fn complete(&self, req: LlmRequest) -> anyhow::Result<LlmResponse>;
}
