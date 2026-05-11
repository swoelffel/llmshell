//! Envelope that wraps an `AuditEvent` into a chained JSONL line.

use crate::digest::digest_excluding;
use crate::event::AuditEvent;
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const CHAIN_SCHEMA_VERSION: u32 = 6;

#[derive(Debug, Clone, Serialize)]
pub struct ChainedEvent<'a> {
    pub schema_version: u32,
    pub seq: u64,
    pub prev_digest: String,
    #[serde(flatten)]
    pub event: &'a AuditEvent,
    pub digest: String,
}

pub fn session_seed_digest(session_id: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"llmsh-audit-chain-seed/v6:");
    h.update(session_id.as_bytes());
    hex(h.finalize().as_slice())
}

pub fn build_envelope<'a>(
    event: &'a AuditEvent,
    seq: u64,
    prev_digest: &str,
) -> (ChainedEvent<'a>, String) {
    let mut env = ChainedEvent {
        schema_version: CHAIN_SCHEMA_VERSION,
        seq,
        prev_digest: prev_digest.to_string(),
        event,
        digest: String::new(),
    };
    let json_no_digest = serde_json::to_value(&env).expect("envelope serialises");
    let digest = digest_excluding(&json_no_digest, "digest");
    env.digest = digest.clone();
    (env, digest)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::now_iso;

    fn ev() -> AuditEvent {
        AuditEvent::SessionEnded {
            ts: now_iso(),
            reason: "test".into(),
        }
    }

    #[test]
    fn envelope_has_required_fields() {
        let e = ev();
        let (env, _) = build_envelope(&e, 0, "seed");
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["schema_version"], 6);
        assert_eq!(v["seq"], 0);
        assert_eq!(v["prev_digest"], "seed");
        assert_eq!(v["type"], "session_ended");
        assert!(v["digest"].as_str().unwrap().len() == 64);
    }

    #[test]
    fn digest_is_stable_for_identical_input() {
        let e = ev();
        let (_, d1) = build_envelope(&e, 0, "seed");
        let (_, d2) = build_envelope(&e, 0, "seed");
        assert_eq!(d1, d2);
    }

    #[test]
    fn changing_prev_digest_changes_self_digest() {
        let e = ev();
        let (_, d1) = build_envelope(&e, 0, "seed-a");
        let (_, d2) = build_envelope(&e, 0, "seed-b");
        assert_ne!(d1, d2);
    }

    #[test]
    fn changing_seq_changes_self_digest() {
        let e = ev();
        let (_, d1) = build_envelope(&e, 0, "seed");
        let (_, d2) = build_envelope(&e, 1, "seed");
        assert_ne!(d1, d2);
    }

    #[test]
    fn session_seed_digest_is_64_hex_chars() {
        let s = session_seed_digest("sess-abc");
        assert_eq!(s.len(), 64);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn session_seed_digest_is_deterministic_per_session_id() {
        assert_eq!(session_seed_digest("x"), session_seed_digest("x"));
        assert_ne!(session_seed_digest("x"), session_seed_digest("y"));
    }
}
