# llmsh-llm

Provider abstraction layer for LLMShell. Defines the `LlmProvider` async trait, the shared `Message`, `ToolCall`, `ToolResult`, and `LlmResponse` types, and the `ProviderCapabilities` struct that advertises what an LLM backend supports (tool use, streaming, etc.). All other crates that need to call an LLM depend only on this crate; concrete provider implementations live in separate crates such as `llmsh-llm-openai`.
