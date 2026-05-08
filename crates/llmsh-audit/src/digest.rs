use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

pub fn canonical_json_digest(v: &Value) -> String {
    let canonical = canonicalize(v);
    sha256_hex(canonical.as_bytes())
}

fn canonicalize(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap(),
        Value::Array(a) => {
            let parts: Vec<String> = a.iter().map(canonicalize).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(*k).unwrap(),
                        canonicalize(&o[*k])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_order_invariant() {
        let a = json!({"a":1,"b":2});
        let b = json!({"b":2,"a":1});
        assert_eq!(canonical_json_digest(&a), canonical_json_digest(&b));
    }
}
