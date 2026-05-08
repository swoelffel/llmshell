use super::types::*;
use tracing::warn;

pub struct MergeWarnings(pub Vec<String>);

pub fn merge_project(base: &mut Config, project: &toml::Value) -> MergeWarnings {
    let mut warns: Vec<String> = Vec::new();

    // ui: free
    if let Some(ui) = project.get("ui").and_then(|v| v.as_table()) {
        if let Some(b) = ui.get("show_plan").and_then(|v| v.as_bool()) {
            base.ui.show_plan = b;
        }
        if let Some(b) = ui.get("show_tool_calls").and_then(|v| v.as_bool()) {
            base.ui.show_tool_calls = b;
        }
        if let Some(b) = ui.get("show_token_usage").and_then(|v| v.as_bool()) {
            base.ui.show_token_usage = b;
        }
    }

    // limits: only if stricter
    if let Some(l) = project.get("limits").and_then(|v| v.as_table()) {
        let mut tighten_u = |key: &str, current: &mut u32| {
            if let Some(v) = l.get(key).and_then(|v| v.as_integer()) {
                let v = v.max(0) as u32;
                if v < *current {
                    *current = v;
                } else if v > *current {
                    warns.push(format!(".llmsh.toml limits.{} ignored (would weaken)", key));
                }
            }
        };
        tighten_u("max_iterations", &mut base.limits.max_iterations);
        tighten_u(
            "max_tool_calls_per_iteration",
            &mut base.limits.max_tool_calls_per_iteration,
        );
        tighten_u(
            "max_schema_repair_attempts",
            &mut base.limits.max_schema_repair_attempts,
        );
        if let Some(v) = l.get("max_llm_output_bytes").and_then(|v| v.as_integer()) {
            let v = v.max(0) as usize;
            if v < base.limits.max_llm_output_bytes {
                base.limits.max_llm_output_bytes = v;
            } else if v > base.limits.max_llm_output_bytes {
                warns.push(".llmsh.toml limits.max_llm_output_bytes ignored (would weaken)".into());
            }
        }
        if let Some(v) = l.get("max_audit_output_bytes").and_then(|v| v.as_integer()) {
            let v = v.max(0) as usize;
            if v < base.limits.max_audit_output_bytes {
                base.limits.max_audit_output_bytes = v;
            } else if v > base.limits.max_audit_output_bytes {
                warns.push(
                    ".llmsh.toml limits.max_audit_output_bytes ignored (would weaken)".into(),
                );
            }
        }
        if let Some(v) = l.get("tool_timeout_ms").and_then(|v| v.as_integer()) {
            let v = v.max(0) as u64;
            if v < base.limits.tool_timeout_ms {
                base.limits.tool_timeout_ms = v;
            } else if v > base.limits.tool_timeout_ms {
                warns.push(".llmsh.toml limits.tool_timeout_ms ignored (would weaken)".into());
            }
        }
    }

    // policy: tighten only (allow → confirm → confirm_strong → deny)
    if let Some(p) = project.get("policy").and_then(|v| v.as_table()) {
        for (k, slot) in [
            ("read_only", &mut base.policy.read_only),
            ("low_risk", &mut base.policy.low_risk),
            ("write", &mut base.policy.write),
            ("destructive", &mut base.policy.destructive),
            ("network", &mut base.policy.network),
            ("privileged", &mut base.policy.privileged),
            ("unknown", &mut base.policy.unknown),
        ] {
            if let Some(v) = p.get(k).and_then(|v| v.as_str()) {
                if action_strictness(v) >= action_strictness(slot) {
                    *slot = v.to_string();
                } else {
                    warns.push(format!(".llmsh.toml policy.{} ignored (would weaken)", k));
                }
            }
        }
    }

    // default_model: only if same provider as user-allowed
    if let Some(m) = project.get("default_model").and_then(|v| v.as_str()) {
        let provider = m.split(':').next().unwrap_or("");
        if base.providers.contains_key(provider) {
            base.default_model = m.to_string();
        } else {
            warns.push(format!(
                ".llmsh.toml default_model={} ignored (provider not in user config)",
                m
            ));
        }
    }

    for w in &warns {
        warn!("{}", w);
    }
    MergeWarnings(warns)
}

fn action_strictness(s: &str) -> u8 {
    match s {
        "allow" => 0,
        "confirm" => 1,
        "confirm_strong" => 2,
        "deny" => 3,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_cannot_relax_destructive() {
        let mut c = Config::defaults(); // destructive = confirm_strong
        let p: toml::Value = toml::from_str(
            r#"
            [policy]
            destructive = "allow"
        "#,
        )
        .unwrap();
        let w = merge_project(&mut c, &p);
        assert_eq!(c.policy.destructive, "confirm_strong");
        assert!(w.0.iter().any(|s| s.contains("destructive")));
    }

    #[test]
    fn project_can_tighten_write() {
        let mut c = Config::defaults();
        let p: toml::Value = toml::from_str(
            r#"
            [policy]
            write = "deny"
        "#,
        )
        .unwrap();
        merge_project(&mut c, &p);
        assert_eq!(c.policy.write, "deny");
    }

    #[test]
    fn project_cannot_increase_iterations() {
        let mut c = Config::defaults();
        let p: toml::Value = toml::from_str(
            r#"
            [limits]
            max_iterations = 100
        "#,
        )
        .unwrap();
        let w = merge_project(&mut c, &p);
        assert_eq!(c.limits.max_iterations, 5);
        assert!(w.0.iter().any(|s| s.contains("max_iterations")));
    }
}
