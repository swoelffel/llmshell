use crate::context::CheckedToolCall;

pub fn generate_phrase(call: &CheckedToolCall) -> String {
    match call.tool_name.as_str() {
        "run_process" => {
            let program = call.args.get("program").and_then(|v| v.as_str()).unwrap_or("?");
            let args = call.args.get("args").and_then(|v| v.as_array());
            let arg_strs: Vec<String> = args
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            format!("run {} {}", program, arg_strs.join(" ")).trim().to_string()
        }
        "read_file" => {
            let p = call.args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("read {}", p)
        }
        "list_directory" => {
            let p = call.args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("list {}", p)
        }
        other => format!("execute {}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RiskLevel;
    use serde_json::json;

    fn call(name: &str, args: serde_json::Value) -> CheckedToolCall {
        CheckedToolCall {
            id: "t1".into(), tool_name: name.into(), args,
            declared_risk: RiskLevel::Unknown,
            resolved_paths: vec![], flags: vec![],
        }
    }

    #[test]
    fn run_process_phrase_deterministic() {
        let c = call("run_process", json!({"program":"rm","args":["-rf","./x"]}));
        assert_eq!(generate_phrase(&c), "run rm -rf ./x");
    }
}
