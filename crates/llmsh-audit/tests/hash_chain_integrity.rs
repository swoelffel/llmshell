//! Materialises the tamper-evidence property end-to-end.

use llmsh_audit::event::{now_iso, AuditEvent};
use llmsh_audit::{verify_chain, AuditWriter, ChainError};
use std::fs;

fn write_chain(dir: &std::path::Path, session_id: &str, n: usize) -> String {
    let mut w = AuditWriter::open(dir, session_id).expect("open writer");
    for i in 0..n {
        w.write(&AuditEvent::AssistantMessage {
            ts: now_iso(),
            text_redacted: format!("event-{i}"),
        })
        .unwrap();
    }
    w.write(&AuditEvent::SessionEnded {
        ts: now_iso(),
        reason: "ok".into(),
    })
    .unwrap();
    w.flush().unwrap();
    fs::read_to_string(dir.join(format!("{session_id}.jsonl"))).unwrap()
}

#[test]
fn freshly_written_chain_verifies_and_is_sealed() {
    let tmp = tempfile::tempdir().unwrap();
    let jsonl = write_chain(tmp.path(), "sess-1", 5);
    let v = verify_chain(&jsonl, "sess-1").unwrap();
    assert_eq!(v.events, 6);
    assert!(v.sealed);
}

#[test]
fn one_char_tamper_in_middle_event_is_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let jsonl = write_chain(tmp.path(), "sess-2", 5);
    let mut lines: Vec<String> = jsonl.lines().map(String::from).collect();
    lines[2] = lines[2].replace("event-2", "event-Z");
    let tampered = lines.join("\n") + "\n";
    let err = verify_chain(&tampered, "sess-2").unwrap_err();
    assert!(matches!(err, ChainError::DigestMismatch { line_no } if line_no == 3));
}

#[test]
fn tail_truncation_is_unsealed_but_consistent() {
    let tmp = tempfile::tempdir().unwrap();
    let jsonl = write_chain(tmp.path(), "sess-3", 5);
    let mut lines: Vec<String> = jsonl.lines().map(String::from).collect();
    lines.truncate(3);
    let truncated = lines.join("\n") + "\n";
    let v = verify_chain(&truncated, "sess-3").unwrap();
    assert_eq!(v.events, 3);
    assert!(!v.sealed);
}
