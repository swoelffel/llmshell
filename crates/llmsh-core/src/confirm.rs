use crate::plan::CheckedPlan;
use llmsh_policy::context::CheckedToolCall;
use llmsh_policy::types::{PolicyAction, PolicyFlag};
use serde_json::Value;

pub trait ConfirmationGate: Send + Sync {
    fn ask(&self, plan: &CheckedPlan) -> bool;
}

pub struct StdinConfirmationGate;

impl ConfirmationGate for StdinConfirmationGate {
    fn ask(&self, plan: &CheckedPlan) -> bool {
        for step in &plan.steps {
            match &step.decision.action {
                PolicyAction::Allow => continue,
                PolicyAction::Deny => return false,
                PolicyAction::RequireConfirmation { strong, phrase } => {
                    println!(
                        "⚠ Confirm: {} (risk={:?})",
                        step.call.tool_name, step.decision.effective_risk
                    );
                    for line in summarize_call(&step.call) {
                        println!("{}", line);
                    }
                    if !step.decision.flags.is_empty() {
                        let flags: Vec<&'static str> =
                            step.decision.flags.iter().map(flag_label).collect();
                        println!("  flags: {}", flags.join(", "));
                    }
                    if !step.decision.reasons.is_empty() {
                        println!("  reasons: {}", step.decision.reasons.join("; "));
                    }
                    if *strong {
                        let p = phrase.clone().unwrap_or_else(|| "yes".into());
                        println!("Type exactly: {}", p);
                        let mut line = String::new();
                        if std::io::stdin().read_line(&mut line).is_err() {
                            return false;
                        }
                        if line.trim() != p {
                            return false;
                        }
                    } else {
                        print!("[y/N] ");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                        let mut line = String::new();
                        if std::io::stdin().read_line(&mut line).is_err() {
                            return false;
                        }
                        let s = line.trim().to_lowercase();
                        if !(s == "y" || s == "yes") {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
}

pub struct AlwaysYesGate;
impl ConfirmationGate for AlwaysYesGate {
    fn ask(&self, plan: &CheckedPlan) -> bool {
        !plan.has_deny()
    }
}

pub struct AlwaysNoGate;
impl ConfirmationGate for AlwaysNoGate {
    fn ask(&self, _plan: &CheckedPlan) -> bool {
        false
    }
}

const SUMMARY_MAX: usize = 200;

pub(crate) fn summarize_call(call: &CheckedToolCall) -> Vec<String> {
    let mut out = Vec::new();
    match call.tool_name.as_str() {
        "run_process" => {
            let program = call
                .args
                .get("program")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let args: Vec<String> = call
                .args
                .get("args")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(quote_arg).collect())
                .unwrap_or_default();
            let cmd = if args.is_empty() {
                program.to_string()
            } else {
                format!("{} {}", program, args.join(" "))
            };
            out.push(format!("  $ {}", truncate(&cmd, SUMMARY_MAX)));
            if let Some(cwd) = call.args.get("cwd").and_then(Value::as_str) {
                out.push(format!("  cwd: {}", cwd));
            }
            if let Some(t) = call.args.get("timeout_ms").and_then(Value::as_i64) {
                out.push(format!("  timeout_ms: {}", t));
            }
        }
        "read_file" | "list_directory" | "get_file_metadata" | "write_file" | "edit_file"
        | "search_files" => {
            if let Some(p) = call.args.get("path").and_then(Value::as_str) {
                out.push(format!("  path: {}", p));
            }
            if let Some(q) = call.args.get("query").and_then(Value::as_str) {
                out.push(format!("  query: {}", truncate(q, SUMMARY_MAX)));
            }
        }
        _ => {
            let json = serde_json::to_string(&call.args).unwrap_or_default();
            out.push(format!("  args: {}", truncate(&json, SUMMARY_MAX)));
        }
    }
    out
}

fn quote_arg(s: &str) -> String {
    let needs_quote = s.is_empty()
        || s.chars()
            .any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '$' | '`' | '\\'));
    if needs_quote {
        format!("{:?}", s)
    } else {
        s.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{}…", cut)
}

fn flag_label(f: &PolicyFlag) -> &'static str {
    match f {
        PolicyFlag::SensitivePath => "sensitive_path",
        PolicyFlag::SecretLikeContent => "secret_like_content",
        PolicyFlag::OutsideWorkspace => "outside_workspace",
        PolicyFlag::LargeBlastRadius => "large_blast_radius",
        PolicyFlag::UsesShell => "uses_shell",
        PolicyFlag::UsesPrivilegeEscalation => "privilege_escalation",
        PolicyFlag::KnownReadOnlyCommand => "known_read_only_command",
        PolicyFlag::ModelClaimedRisk => "model_claimed_risk",
        PolicyFlag::ModelDisagreesOnRisk => "model_disagrees_on_risk",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmsh_policy::types::RiskLevel;
    use serde_json::json;

    fn call(name: &str, args: Value) -> CheckedToolCall {
        CheckedToolCall {
            id: "x".into(),
            tool_name: name.into(),
            args,
            declared_risk: RiskLevel::Unknown,
            resolved_paths: vec![],
            flags: vec![],
        }
    }

    #[test]
    fn run_process_shows_program_and_args() {
        let c = call(
            "run_process",
            json!({"program":"rm","args":["-rf","/tmp/x"],"cwd":"/home/u"}),
        );
        let s = summarize_call(&c);
        assert!(s.iter().any(|l| l == "  $ rm -rf /tmp/x"), "{:?}", s);
        assert!(s.iter().any(|l| l == "  cwd: /home/u"));
    }

    #[test]
    fn run_process_quotes_args_with_spaces() {
        let c = call(
            "run_process",
            json!({"program":"echo","args":["hello world","plain"]}),
        );
        let s = summarize_call(&c);
        assert!(
            s.iter().any(|l| l.contains(r#""hello world" plain"#)),
            "{:?}",
            s
        );
    }

    #[test]
    fn run_process_missing_args_ok() {
        let c = call("run_process", json!({"program":"ls"}));
        let s = summarize_call(&c);
        assert_eq!(s, vec!["  $ ls".to_string()]);
    }

    #[test]
    fn read_file_shows_path() {
        let c = call("read_file", json!({"path":"/etc/hosts"}));
        let s = summarize_call(&c);
        assert_eq!(s, vec!["  path: /etc/hosts".to_string()]);
    }

    #[test]
    fn unknown_tool_dumps_args_truncated() {
        let big: String = "x".repeat(300);
        let c = call("custom_tool", json!({"k": big}));
        let s = summarize_call(&c);
        assert!(s[0].starts_with("  args: "));
        assert!(s[0].ends_with('…'));
    }
}
