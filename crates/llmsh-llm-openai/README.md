# llmsh-llm-openai

OpenAI-compatible HTTP provider for LLMShell. Implements the `LlmProvider` trait from `llmsh-llm` by sending requests to any OpenAI-format chat-completions endpoint (OpenAI, Azure, local inference servers, etc.). Handles JSON serialisation of the wire format, tool-call mapping between the internal `llmsh-llm` types and the OpenAI schema, and bearer-token authentication via an API key read from the environment.
