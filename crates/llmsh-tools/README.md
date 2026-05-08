# llmsh-tools

Built-in tool implementations for LLMShell. Provides the `Tool` async trait, a `ToolRegistry` for dynamic dispatch, and three production tools: `ReadFile` (reads a file and returns its contents), `ListDirectory` (lists directory entries with optional recursion), and `RunProcess` (executes a shell command in a sandboxed environment). Each tool is enriched with JSON-Schema descriptions so the LLM can choose and invoke them correctly, and all tool calls pass through `llmsh-policy` before execution.
