use llmsh_core::init::run_autoinit_if_needed;
use llmsh_core::memory::{InitAudit, Memory};

fn make_existing_audit() -> InitAudit {
    InitAudit {
        written_at: "2026-01-01T00:00:00Z".into(),
        host: "existing-host".into(),
        os: "Linux 6.1 x86_64".into(),
        kernel: "6.1.0".into(),
        user: "alice".into(),
        home: "/home/alice".into(),
        shell: None,
        summary_md: "existing audit".into(),
    }
}

/// Empty DB + no_autoinit=false → autoinit runs and writes an audit.
#[tokio::test]
async fn autoinit_runs_on_empty_db() {
    let memory = Memory::open_in_memory().unwrap();
    assert!(memory.read_init_audit().unwrap().is_none());

    let ran = run_autoinit_if_needed(&memory, false).await.unwrap();
    assert!(ran, "should have run autoinit");
    assert!(
        memory.read_init_audit().unwrap().is_some(),
        "audit should be written"
    );
}

/// Empty DB + no_autoinit=true → autoinit is skipped.
#[tokio::test]
async fn autoinit_skipped_when_disabled() {
    let memory = Memory::open_in_memory().unwrap();
    let ran = run_autoinit_if_needed(&memory, true).await.unwrap();
    assert!(!ran, "should not have run autoinit");
    assert!(
        memory.read_init_audit().unwrap().is_none(),
        "audit should not be written"
    );
}

/// DB with existing audit + no_autoinit=false → autoinit is skipped, existing data unchanged.
#[tokio::test]
async fn autoinit_skipped_when_audit_already_exists() {
    let memory = Memory::open_in_memory().unwrap();
    memory.write_init_audit(&make_existing_audit()).unwrap();

    let ran = run_autoinit_if_needed(&memory, false).await.unwrap();
    assert!(!ran, "should not overwrite existing audit");

    let got = memory.read_init_audit().unwrap().unwrap();
    assert_eq!(
        got.host, "existing-host",
        "existing audit must be preserved"
    );
    assert_eq!(got.summary_md, "existing audit");
}
