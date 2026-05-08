use llmsh_llm::types::ToolCall;
use llmsh_policy::context::{CheckedToolCall, ResolvedPath};
use llmsh_policy::sensitive::matches_sensitive;
use llmsh_policy::types::{PolicyFlag, RiskLevel};
use std::path::{Path, PathBuf};

pub struct EnrichmentInput<'a> {
    pub cwd: &'a Path,
    pub workspace_root: &'a Path,
    pub home: Option<&'a Path>,
    pub sensitive_patterns: &'a [String],
}

pub fn enrich(
    call: &ToolCall,
    declared_risk: RiskLevel,
    input: EnrichmentInput,
) -> CheckedToolCall {
    let mut paths: Vec<ResolvedPath> = Vec::new();
    let mut flags: Vec<PolicyFlag> = Vec::new();

    let collect_path = |raw: &str, paths: &mut Vec<ResolvedPath>| {
        let expanded = expand_tilde(raw, input.home);
        let abs: PathBuf = if expanded.is_absolute() {
            expanded.clone()
        } else {
            input.cwd.join(&expanded)
        };
        let canon = std::fs::canonicalize(&abs).unwrap_or(abs.clone());
        let inside = canon.starts_with(input.workspace_root);
        let sensitive = matches_sensitive(&canon, input.sensitive_patterns, input.home);
        paths.push(ResolvedPath {
            original: raw.to_string(),
            canonical: canon,
            inside_workspace: inside,
            matches_sensitive: sensitive,
        });
    };

    match call.name.as_str() {
        "read_file" | "list_directory" => {
            if let Some(p) = call.args.get("path").and_then(|v| v.as_str()) {
                collect_path(p, &mut paths);
            }
        }
        "run_process" => {
            let program = call
                .args
                .get("program")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args: Vec<String> = call
                .args
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // Privilege escalation
            if matches!(program, "sudo" | "doas" | "su") {
                flags.push(PolicyFlag::UsesPrivilegeEscalation);
            }
            // Large blast radius patterns
            let joined = format!("{} {}", program, args.join(" "));
            let blast_patterns = [("rm", ["-rf", "-fr", "--recursive"])];
            if program == "rm"
                && args.iter().any(|a| {
                    a == "-rf"
                        || a == "-fr"
                        || a == "--recursive"
                        || (a.starts_with('-') && a.contains('r') && a.contains('f'))
                })
            {
                flags.push(PolicyFlag::LargeBlastRadius);
            }
            if matches!(program, "dd" | "mkfs" | "diskutil") {
                flags.push(PolicyFlag::LargeBlastRadius);
            }
            if matches!(program, "chmod" | "chown")
                && args.iter().any(|a| a == "-R" || a == "--recursive")
            {
                flags.push(PolicyFlag::LargeBlastRadius);
            }
            // Path-like args
            for a in &args {
                if a.starts_with('/')
                    || a.starts_with("./")
                    || a.starts_with("../")
                    || a.starts_with("~")
                {
                    collect_path(a, &mut paths);
                }
            }
            let _ = blast_patterns;
            let _ = joined;
        }
        _ => {}
    }

    CheckedToolCall {
        id: call.id.clone(),
        tool_name: call.name.clone(),
        args: call.args.clone(),
        declared_risk,
        resolved_paths: paths,
        flags,
    }
}

fn expand_tilde(raw: &str, home: Option<&Path>) -> PathBuf {
    if let Some(h) = home {
        if let Some(rest) = raw.strip_prefix("~/") {
            return h.join(rest);
        }
        if raw == "~" {
            return h.to_path_buf();
        }
    }
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rm_rf_flagged() {
        let call = ToolCall {
            id: "1".into(),
            name: "run_process".into(),
            args: json!({"program":"rm","args":["-rf","./x"]}),
        };
        let cwd = std::env::temp_dir();
        let out = enrich(
            &call,
            RiskLevel::Unknown,
            EnrichmentInput {
                cwd: &cwd,
                workspace_root: &cwd,
                home: None,
                sensitive_patterns: &[],
            },
        );
        assert!(out.flags.contains(&PolicyFlag::LargeBlastRadius));
    }

    #[test]
    fn sudo_flagged() {
        let call = ToolCall {
            id: "1".into(),
            name: "run_process".into(),
            args: json!({"program":"sudo","args":["ls"]}),
        };
        let cwd = std::env::temp_dir();
        let out = enrich(
            &call,
            RiskLevel::Unknown,
            EnrichmentInput {
                cwd: &cwd,
                workspace_root: &cwd,
                home: None,
                sensitive_patterns: &[],
            },
        );
        assert!(out.flags.contains(&PolicyFlag::UsesPrivilegeEscalation));
    }
}
