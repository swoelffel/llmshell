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

/// Like `canonical_json_digest`, but with the top-level `field` removed first.
/// Used by the chain to compute a line's `digest` over the envelope minus the
/// `digest` field itself.
pub fn digest_excluding(v: &serde_json::Value, field: &str) -> String {
    let mut clone = v.clone();
    if let Some(obj) = clone.as_object_mut() {
        obj.remove(field);
    }
    canonical_json_digest(&clone)
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

    #[test]
    fn digest_excluding_skips_named_field() {
        let with = serde_json::json!({ "a": 1, "b": 2, "skip_me": "anything" });
        let without = serde_json::json!({ "a": 1, "b": 2 });
        assert_eq!(
            digest_excluding(&with, "skip_me"),
            canonical_json_digest(&without),
        );
    }

    #[test]
    fn digest_excluding_noop_when_field_absent() {
        let v = serde_json::json!({ "a": 1, "b": 2 });
        assert_eq!(digest_excluding(&v, "missing"), canonical_json_digest(&v),);
    }
}
