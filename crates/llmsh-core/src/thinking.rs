use async_trait::async_trait;
use llmsh_llm::capabilities::Capabilities;
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{LlmRequest, LlmResponse, ModelInfo};
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// Animated "Thinking…" indicator on stderr. No-op when stderr is not a TTY,
/// so test runs and piped invocations stay clean.
pub struct ThinkingIndicator;

impl ThinkingIndicator {
    pub fn start(label: &'static str) -> ThinkingGuard {
        if !std::io::stderr().is_terminal() {
            return ThinkingGuard { notify: None };
        }
        let notify = Arc::new(Notify::new());
        let n2 = notify.clone();
        tokio::spawn(async move {
            const FRAMES: [&str; 4] = ["   ", ".  ", ".. ", "..."];
            let mut i = 0usize;
            loop {
                {
                    let mut err = std::io::stderr().lock();
                    let _ = write!(err, "\r{}{}\x1b[K", label, FRAMES[i % FRAMES.len()]);
                    let _ = err.flush();
                }
                tokio::select! {
                    _ = n2.notified() => break,
                    _ = tokio::time::sleep(Duration::from_millis(400)) => {}
                }
                i += 1;
            }
        });
        ThinkingGuard {
            notify: Some(notify),
        }
    }
}

pub struct ThinkingGuard {
    notify: Option<Arc<Notify>>,
}

impl Drop for ThinkingGuard {
    fn drop(&mut self) {
        if let Some(n) = self.notify.take() {
            n.notify_one();
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r\x1b[2K");
            let _ = err.flush();
        }
    }
}

/// Wraps an `LlmProvider` so the indicator runs while `complete()` is in flight.
/// Other methods pass through unchanged.
pub struct ThinkingProvider {
    inner: Arc<dyn LlmProvider>,
}

impl ThinkingProvider {
    pub fn new(inner: Arc<dyn LlmProvider>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl LlmProvider for ThinkingProvider {
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    async fn complete(&self, req: LlmRequest) -> anyhow::Result<LlmResponse> {
        let _g = ThinkingIndicator::start("Thinking");
        self.inner.complete(req).await
    }
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        self.inner.list_models().await
    }
    async fn set_model(&self, id: &str) -> anyhow::Result<()> {
        self.inner.set_model(id).await
    }
    fn current_model(&self) -> String {
        self.inner.current_model()
    }
}
