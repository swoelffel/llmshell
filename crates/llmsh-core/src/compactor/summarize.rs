use anyhow::{Context, Result};
use llmsh_audit::digest::{canonical_json_digest, sha256_hex};
use llmsh_audit::event::{now_iso, AuditEvent};
use llmsh_audit::redact::Redactor;
use llmsh_audit::writer::AuditWriter;
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{LlmRequest, Message, MessageRole, ResponseFormat, ToolPolicyHint};
use serde::Deserialize;
use std::sync::{Arc, Mutex};

/// Returns the index just before the `keep_last_user_turns`-th user message
/// counted from the end. Messages at indices `< cut_index` are eligible for
/// summarization. Returns `None` if the conversation has fewer than
/// `keep_last_user_turns + 1` user messages — nothing to summarize.
///
/// Special case: `keep_last_user_turns == 0` means "summarize everything"
/// → returns `Some(messages.len())`.
///
/// Guarantees: the cut is always immediately before a `user` message, so the
/// summarized prefix is a complete logical history (no half-tool-call
/// dangling).
pub fn find_cut_index(messages: &[Message], keep_last_user_turns: usize) -> Option<usize> {
    if keep_last_user_turns == 0 {
        return Some(messages.len());
    }
    let mut user_count = 0usize;
    for (i, m) in messages.iter().enumerate().rev() {
        if m.role == MessageRole::User {
            user_count += 1;
            if user_count == keep_last_user_turns {
                return if i == 0 { None } else { Some(i) };
            }
        }
    }
    None
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExtractedFact {
    pub category: String,
    pub claim: String,
}

#[derive(Debug, Deserialize)]
struct CompactJson {
    summary: String,
    #[serde(default)]
    facts: Vec<ExtractedFact>,
}

const SUMMARIZE_AND_EXTRACT_SYSTEM: &str = "\
Tu es un agent qui produit (1) un résumé de conversation et (2) une liste \
curée de faits long-terme à mémoriser. Réponds en JSON strict avec les clés \
\"summary\" et \"facts\".";

/// Run the LLM call to produce both a summary message and the new curated
/// facts list. Returns `(summary_message, facts, digest)`.
///
/// Fails (Err) if the LLM call fails or returns unparseable JSON — caller
/// decides how to handle (typically silent fallback: keep prefix as-is).
#[allow(clippy::too_many_arguments)]
pub async fn summarize_and_extract(
    provider: Arc<dyn LlmProvider>,
    summary_model_label: Option<&str>,
    prefix: &[Message],
    current_facts: &[(String, String)], // (category, claim)
    summary_max_tokens: u32,
    max_facts: usize,
    model_label_for_audit: &str,
    audit: Option<&Mutex<AuditWriter>>,
    redactor: Option<&Redactor>,
) -> Result<(Message, Vec<ExtractedFact>, String)> {
    let prefix_body = render_prefix_for_summary(prefix);
    let facts_body = render_current_facts(current_facts);
    let user_msg = format!(
        "Voici les faits long-terme actuellement mémorisés (à fusionner, mettre à jour, supprimer si stale, garder max {max_facts} entrées) :\n\n{facts_body}\n\nVoici la conversation à résumer :\n\n{prefix_body}\n\nProduis un JSON strict avec :\n- \"summary\" : résumé factuel max {summary_max_tokens} tokens. Préserve décisions, actions exécutées, TODOs ouverts.\n- \"facts\" : liste finale curée, max {max_facts} entrées, chaque entrée avec \"category\" (identity|preference|project|todo|other) et \"claim\" (phrase courte)."
    );

    let req = LlmRequest {
        system: Some(SUMMARIZE_AND_EXTRACT_SYSTEM.into()),
        messages: vec![Message {
            role: MessageRole::User,
            content: user_msg,
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }],
        tools: vec![],
        tool_policy: ToolPolicyHint::None,
        response_format: Some(ResponseFormat::JsonObject),
    };
    let _ = summary_model_label; // reserved: future hook to swap provider/model

    // Audit: request side. Distinguish compaction from agent traffic by
    // suffixing the model label.
    let audit_model = format!("{model_label_for_audit}#compact");
    if let Some(audit) = audit {
        let messages_digest = serde_json::to_value(&req.messages)
            .map(|v| canonical_json_digest(&v))
            .unwrap_or_default();
        let context_bytes = serde_json::to_string(&req.messages)
            .map(|s| s.len())
            .unwrap_or(0);
        if let Ok(mut w) = audit.lock() {
            let _ = w.write(&AuditEvent::LlmRequest {
                ts: now_iso(),
                model: audit_model.clone(),
                messages_digest,
                tool_count: 0,
                prompt_token_estimate: None,
                context_bytes,
                redaction_applied: redactor.is_some(),
                redaction_hit_count: 0,
            });
        }
    }

    let resp_result = provider.complete(req).await;
    let resp = match resp_result {
        Ok(r) => r,
        Err(e) => {
            if let Some(audit) = audit {
                if let Ok(mut w) = audit.lock() {
                    let _ = w.write(&AuditEvent::LlmResponse {
                        ts: now_iso(),
                        model: audit_model.clone(),
                        finish_reason: "error".into(),
                        message_redacted: Some(format!("error: {e:#}")),
                        tool_call_count: 0,
                        tool_calls_digest: None,
                        usage: None,
                    });
                }
            }
            return Err(e).context("compaction LLM call");
        }
    };

    // Audit: response side (success path or empty body — both useful).
    if let Some(audit) = audit {
        let msg_red = resp.message.as_deref().map(|m| match redactor {
            Some(r) => r.redact(m).0,
            None => m.to_string(),
        });
        if let Ok(mut w) = audit.lock() {
            let _ = w.write(&AuditEvent::LlmResponse {
                ts: now_iso(),
                model: audit_model.clone(),
                finish_reason: format!("{:?}", resp.finish_reason).to_lowercase(),
                message_redacted: msg_red,
                tool_call_count: 0,
                tool_calls_digest: None,
                usage: resp
                    .usage
                    .as_ref()
                    .and_then(|u| serde_json::to_value(u).ok()),
            });
        }
    }

    let raw = resp.message.unwrap_or_default();
    if raw.trim().is_empty() {
        anyhow::bail!("compaction LLM returned empty message body");
    }
    let parsed: CompactJson = serde_json::from_str(&raw).with_context(|| {
        let preview: String = raw.chars().take(200).collect();
        format!("compaction JSON parse failed; first 200 chars: {preview}")
    })?;

    let facts: Vec<ExtractedFact> = parsed.facts.into_iter().take(max_facts).collect();

    let prefixed = format!(
        "=== compacted ({} messages summarized) ===\n{}",
        prefix.len(),
        parsed.summary.trim()
    );
    let digest = sha256_hex(prefixed.as_bytes());
    let summary_msg = Message {
        role: MessageRole::Assistant,
        content: prefixed,
        tool_call_id: None,
        name: None,
        tool_calls: None,
    };
    Ok((summary_msg, facts, digest))
}

pub fn render_prefix_for_summary(prefix: &[Message]) -> String {
    let mut out = String::new();
    for m in prefix {
        match m.role {
            MessageRole::User => {
                out.push_str("user: ");
                out.push_str(&m.content);
                out.push('\n');
            }
            MessageRole::Assistant => {
                if let Some(calls) = &m.tool_calls {
                    let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
                    out.push_str("assistant (tool_calls: ");
                    out.push_str(&names.join(", "));
                    out.push(')');
                    if !m.content.is_empty() {
                        out.push_str(": ");
                        out.push_str(&m.content);
                    }
                    out.push('\n');
                } else {
                    out.push_str("assistant: ");
                    out.push_str(&m.content);
                    out.push('\n');
                }
            }
            MessageRole::Tool => {
                let name = m.name.as_deref().unwrap_or("?");
                out.push_str("tool[");
                out.push_str(name);
                out.push_str("]: ");
                out.push_str(&m.content);
                out.push('\n');
            }
            MessageRole::System => {
                // Should not appear in prefix (system is rebuilt each turn).
            }
        }
    }
    out
}

fn render_current_facts(facts: &[(String, String)]) -> String {
    if facts.is_empty() {
        return "(aucun fait mémorisé)".into();
    }
    facts
        .iter()
        .map(|(c, claim)| format!("- [{c}] {claim}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
    use llmsh_llm::provider::LlmProvider;
    use llmsh_llm::types::{FinishReason, LlmRequest, LlmResponse, ModelInfo, TokenUsage};
    use std::sync::{Arc as StdArc, Mutex};

    fn user(s: &str) -> Message {
        Message {
            role: MessageRole::User,
            content: s.into(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }
    fn assistant(s: &str) -> Message {
        Message {
            role: MessageRole::Assistant,
            content: s.into(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    #[test]
    fn cut_at_kth_user_from_end() {
        let msgs = vec![
            user("u1"),
            assistant("a1"),
            user("u2"),
            assistant("a2"),
            user("u3"),
            assistant("a3"),
            user("u4"),
            assistant("a4"),
            user("u5"),
            assistant("a5"),
        ];
        // K=2 → keep u4,a4,u5,a5 → cut at index of u4 = 6
        assert_eq!(find_cut_index(&msgs, 2), Some(6));
    }

    #[test]
    fn fewer_user_turns_than_k_returns_none() {
        let msgs = vec![user("u1"), assistant("a1"), user("u2"), assistant("a2")];
        assert_eq!(find_cut_index(&msgs, 4), None);
    }

    #[test]
    fn equal_user_turns_returns_none() {
        // Exactly K user turns means the first user is at index 0 → cutting at 0
        // would summarize nothing. Treat as None.
        let msgs = vec![
            user("u1"),
            assistant("a1"),
            user("u2"),
            assistant("a2"),
            user("u3"),
            assistant("a3"),
            user("u4"),
            assistant("a4"),
        ];
        assert_eq!(find_cut_index(&msgs, 4), None);
    }

    #[test]
    fn keep_zero_means_summarize_everything() {
        let msgs = vec![user("u1"), assistant("a1")];
        assert_eq!(find_cut_index(&msgs, 0), Some(2));
    }

    struct ScriptedProvider {
        responses: Mutex<Vec<LlmResponse>>,
    }
    #[async_trait]
    impl LlmProvider for ScriptedProvider {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tool_calling: ToolCallingMode::Native,
                supports_streaming: false,
                supports_json_mode: true,
                supports_parallel_tool_calls: true,
                supports_tool_choice_required: true,
                max_context_tokens: None,
            }
        }
        async fn complete(&self, _: LlmRequest) -> anyhow::Result<LlmResponse> {
            Ok(self.responses.lock().unwrap().remove(0))
        }
        async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
            Ok(vec![])
        }
        async fn set_model(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn current_model(&self) -> String {
            "mock".into()
        }
    }

    #[tokio::test]
    async fn summarize_and_extract_yields_summary_and_facts() {
        let provider: StdArc<dyn LlmProvider> = StdArc::new(ScriptedProvider {
            responses: Mutex::new(vec![LlmResponse {
                message: Some(
                    r#"{"summary":"résumé court","facts":[{"category":"identity","claim":"alice"}]}"#
                        .into(),
                ),
                tool_calls: vec![],
                finish_reason: FinishReason::Stop,
                usage: Some(TokenUsage::default()),
            }]),
        });
        let prefix = vec![user("hello"), assistant("hi")];
        let (msg, facts, digest) = summarize_and_extract(
            provider,
            None,
            &prefix,
            &[],
            500,
            10,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(msg.role, MessageRole::Assistant);
        assert!(msg.tool_calls.is_none());
        assert!(msg.tool_call_id.is_none());
        assert!(msg
            .content
            .starts_with("=== compacted (2 messages summarized) ==="));
        assert!(msg.content.contains("résumé court"));
        assert_eq!(digest.len(), 64);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].category, "identity");
        assert_eq!(facts[0].claim, "alice");
    }

    #[tokio::test]
    async fn summarize_and_extract_provider_failure_propagates() {
        struct Boom;
        #[async_trait]
        impl LlmProvider for Boom {
            fn capabilities(&self) -> Capabilities {
                Capabilities {
                    tool_calling: ToolCallingMode::Native,
                    supports_streaming: false,
                    supports_json_mode: true,
                    supports_parallel_tool_calls: true,
                    supports_tool_choice_required: true,
                    max_context_tokens: None,
                }
            }
            async fn complete(&self, _: LlmRequest) -> anyhow::Result<LlmResponse> {
                anyhow::bail!("boom")
            }
            async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
                Ok(vec![])
            }
            async fn set_model(&self, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            fn current_model(&self) -> String {
                "mock".into()
            }
        }
        let p: StdArc<dyn LlmProvider> = StdArc::new(Boom);
        let prefix = vec![user("hi"), assistant("ho")];
        assert!(
            summarize_and_extract(p, None, &prefix, &[], 500, 10, "test-model", None, None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn extract_caps_at_max_facts() {
        let provider: StdArc<dyn LlmProvider> = StdArc::new(ScriptedProvider {
            responses: Mutex::new(vec![LlmResponse {
                message: Some(
                    r#"{"summary":"s","facts":[
                        {"category":"identity","claim":"c1"},
                        {"category":"identity","claim":"c2"},
                        {"category":"identity","claim":"c3"},
                        {"category":"identity","claim":"c4"},
                        {"category":"identity","claim":"c5"}
                    ]}"#
                    .into(),
                ),
                tool_calls: vec![],
                finish_reason: FinishReason::Stop,
                usage: None,
            }]),
        });
        let prefix = vec![user("q"), assistant("a")];
        let (_msg, facts, _digest) = summarize_and_extract(
            provider,
            None,
            &prefix,
            &[],
            200,
            2,
            "test-model",
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(facts.len(), 2);
    }

    #[tokio::test]
    async fn invalid_json_returns_err() {
        let provider: StdArc<dyn LlmProvider> = StdArc::new(ScriptedProvider {
            responses: Mutex::new(vec![LlmResponse {
                message: Some("not valid json at all".into()),
                tool_calls: vec![],
                finish_reason: FinishReason::Stop,
                usage: None,
            }]),
        });
        let prefix = vec![user("q"), assistant("a")];
        assert!(summarize_and_extract(
            provider,
            None,
            &prefix,
            &[],
            200,
            10,
            "test-model",
            None,
            None
        )
        .await
        .is_err());
    }
}
