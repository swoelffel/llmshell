//! Verifier for the chained audit log.

use crate::chain::{session_seed_digest, CHAIN_SCHEMA_VERSION};
use crate::digest::digest_excluding;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedChain {
    pub events: usize,
    pub sealed: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChainError {
    #[error("line {line_no}: not valid JSON")]
    InvalidJson { line_no: usize },
    #[error("line {line_no}: missing field `{field}`")]
    MissingField { line_no: usize, field: &'static str },
    #[error("line {line_no}: schema_version {found} is older than supported v{expected}")]
    SchemaTooOld {
        line_no: usize,
        found: u32,
        expected: u32,
    },
    #[error(
        "line {line_no}: schema_version {found} is newer than this build supports (v{expected})"
    )]
    SchemaTooNew {
        line_no: usize,
        found: u32,
        expected: u32,
    },
    #[error("line {line_no}: seq {found} does not match expected {expected}")]
    SeqMismatch {
        line_no: usize,
        found: u64,
        expected: u64,
    },
    #[error("line {line_no}: prev_digest does not match the previous line's digest")]
    PrevDigestMismatch { line_no: usize },
    #[error(
        "line {line_no}: digest is not the canonical hash of the envelope (tampered or corrupted)"
    )]
    DigestMismatch { line_no: usize },
}

pub fn verify_chain(jsonl: &str, session_id: &str) -> Result<VerifiedChain, ChainError> {
    let mut expected_prev = session_seed_digest(session_id);
    let mut expected_seq: u64 = 0;
    let mut events = 0usize;
    let mut last_type: Option<String> = None;

    for (idx, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line_no = idx + 1;
        let v: Value =
            serde_json::from_str(line).map_err(|_| ChainError::InvalidJson { line_no })?;

        let schema =
            v.get("schema_version")
                .and_then(|x| x.as_u64())
                .ok_or(ChainError::MissingField {
                    line_no,
                    field: "schema_version",
                })?;
        let schema = schema as u32;
        if schema < CHAIN_SCHEMA_VERSION {
            return Err(ChainError::SchemaTooOld {
                line_no,
                found: schema,
                expected: CHAIN_SCHEMA_VERSION,
            });
        }
        if schema > CHAIN_SCHEMA_VERSION {
            return Err(ChainError::SchemaTooNew {
                line_no,
                found: schema,
                expected: CHAIN_SCHEMA_VERSION,
            });
        }

        let seq = v
            .get("seq")
            .and_then(|x| x.as_u64())
            .ok_or(ChainError::MissingField {
                line_no,
                field: "seq",
            })?;
        if seq != expected_seq {
            return Err(ChainError::SeqMismatch {
                line_no,
                found: seq,
                expected: expected_seq,
            });
        }

        let prev =
            v.get("prev_digest")
                .and_then(|x| x.as_str())
                .ok_or(ChainError::MissingField {
                    line_no,
                    field: "prev_digest",
                })?;
        if prev != expected_prev {
            return Err(ChainError::PrevDigestMismatch { line_no });
        }

        let claimed_digest = v
            .get("digest")
            .and_then(|x| x.as_str())
            .ok_or(ChainError::MissingField {
                line_no,
                field: "digest",
            })?
            .to_string();
        let recomputed = digest_excluding(&v, "digest");
        if claimed_digest != recomputed {
            return Err(ChainError::DigestMismatch { line_no });
        }

        last_type = v.get("type").and_then(|x| x.as_str()).map(String::from);
        expected_prev = claimed_digest;
        expected_seq += 1;
        events += 1;
    }

    let sealed = last_type.as_deref() == Some("session_ended");
    Ok(VerifiedChain { events, sealed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{now_iso, AuditEvent};
    use crate::writer::AuditWriter;
    use tempfile::TempDir;

    fn write_n(session_id: &str, n: usize, end_with_session_ended: bool) -> (TempDir, String) {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = AuditWriter::open(tmp.path(), session_id).unwrap();
        for i in 0..n {
            w.write(&AuditEvent::AssistantMessage {
                ts: now_iso(),
                text_redacted: format!("msg-{i}"),
            })
            .unwrap();
        }
        if end_with_session_ended {
            w.write(&AuditEvent::SessionEnded {
                ts: now_iso(),
                reason: "ok".into(),
            })
            .unwrap();
        }
        w.flush().unwrap();
        let s = std::fs::read_to_string(tmp.path().join(format!("{session_id}.jsonl"))).unwrap();
        (tmp, s)
    }

    #[test]
    fn intact_sealed_chain_verifies() {
        let (_t, jsonl) = write_n("sess-A", 3, true);
        let r = verify_chain(&jsonl, "sess-A").unwrap();
        assert_eq!(r.events, 4);
        assert!(r.sealed);
    }

    #[test]
    fn intact_unsealed_chain_verifies_but_unsealed() {
        let (_t, jsonl) = write_n("sess-B", 3, false);
        let r = verify_chain(&jsonl, "sess-B").unwrap();
        assert_eq!(r.events, 3);
        assert!(!r.sealed);
    }

    #[test]
    fn empty_input_is_unsealed_zero_events() {
        let r = verify_chain("", "sess-X").unwrap();
        assert_eq!(r.events, 0);
        assert!(!r.sealed);
    }

    #[test]
    fn tampered_middle_event_detected() {
        let (_t, jsonl) = write_n("sess-C", 5, true);
        let mut lines: Vec<String> = jsonl.lines().map(String::from).collect();
        lines[2] = lines[2].replace("msg-2", "msg-X");
        let tampered = lines.join("\n") + "\n";
        let err = verify_chain(&tampered, "sess-C").unwrap_err();
        assert!(matches!(err, ChainError::DigestMismatch { line_no } if line_no == 3));
    }

    #[test]
    fn dropped_middle_line_detected_via_seq_or_prev() {
        let (_t, jsonl) = write_n("sess-D", 5, true);
        let mut lines: Vec<&str> = jsonl.lines().collect();
        lines.remove(2);
        let truncated = lines.join("\n") + "\n";
        let err = verify_chain(&truncated, "sess-D").unwrap_err();
        match err {
            ChainError::SeqMismatch { line_no, .. }
            | ChainError::PrevDigestMismatch { line_no } => assert_eq!(line_no, 3),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn wrong_session_id_seed_detected_on_first_line() {
        let (_t, jsonl) = write_n("sess-E", 1, false);
        let err = verify_chain(&jsonl, "different-session").unwrap_err();
        assert!(matches!(err, ChainError::PrevDigestMismatch { line_no: 1 }));
    }

    #[test]
    fn truncating_tail_is_internally_consistent_but_unsealed() {
        let (_t, jsonl) = write_n("sess-F", 5, true);
        let mut lines: Vec<String> = jsonl.lines().map(String::from).collect();
        lines.truncate(lines.len() - 2);
        let truncated = lines.join("\n") + "\n";
        let r = verify_chain(&truncated, "sess-F").unwrap();
        assert_eq!(r.events, 4);
        assert!(!r.sealed);
    }

    #[test]
    fn schema_too_old_when_v5_line_seen() {
        let line = r#"{"schema_version":5,"seq":0,"prev_digest":"","type":"session_ended","ts":"x","reason":"y","digest":"zz"}"#;
        let err = verify_chain(line, "anything").unwrap_err();
        assert!(matches!(
            err,
            ChainError::SchemaTooOld {
                line_no: 1,
                found: 5,
                ..
            }
        ));
    }
}
