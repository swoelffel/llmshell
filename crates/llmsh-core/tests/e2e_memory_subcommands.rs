//! End-to-end coverage for the `/memory` subcommand surface.
//!
//! Drives the underlying `Memory` API (no full Repl):
//! - `/memory add`     → `add_manual_fact`
//! - `/memory list`    → `load_active_facts`
//! - `/memory forget`  → `mark_fact_cleared_by_id`

use llmsh_core::memory::{ClearSource, Memory};

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `add_manual_fact` writes rows visible via `load_active_facts`,
/// `mark_fact_cleared_by_id` removes one targeted fact, and a non-existent
/// id returns false without affecting other rows.
#[test]
fn add_list_forget_round_trip() {
    let mem = Memory::open_in_memory().unwrap();

    // /memory add — two facts.
    let id_a = mem
        .add_manual_fact(&now(), "identity", "user is alice")
        .unwrap();
    let id_b = mem
        .add_manual_fact(&now(), "preference", "likes terse output")
        .unwrap();
    assert!(id_a > 0 && id_b > 0 && id_a != id_b);

    // /memory list — both must appear, all marked manual.
    let facts = mem.load_active_facts().unwrap();
    assert_eq!(facts.len(), 2, "both manually-added facts must be visible");
    for f in &facts {
        assert_eq!(
            f.insert_source, "manual",
            "manually-added facts must have insert_source='manual'"
        );
    }
    assert!(facts.iter().any(|f| f.id == id_a));
    assert!(facts.iter().any(|f| f.id == id_b));

    // /memory forget id_a — only that one is removed.
    let ok = mem
        .mark_fact_cleared_by_id(id_a, &now(), ClearSource::MemoryForget)
        .unwrap();
    assert!(ok, "forget on a real id must return true");

    let after = mem.load_active_facts().unwrap();
    assert_eq!(after.len(), 1, "exactly one fact must remain after forget");
    assert_eq!(
        after[0].id, id_b,
        "the surviving fact must be the one we did NOT forget"
    );

    // /memory forget on a non-existent id — false, no-op.
    let ok = mem
        .mark_fact_cleared_by_id(99_999, &now(), ClearSource::MemoryForget)
        .unwrap();
    assert!(!ok, "forget on a non-existent id must return false");

    let still = mem.load_active_facts().unwrap();
    assert_eq!(
        still.len(),
        1,
        "non-existent forget must not change anything"
    );
}

/// Forgetting a fact twice: the first call returns true, the second returns
/// false (the row's `cleared_at` is already non-null so the WHERE filter
/// excludes it).
#[test]
fn forget_is_idempotent_on_already_cleared_id() {
    let mem = Memory::open_in_memory().unwrap();

    let id = mem
        .add_manual_fact(&now(), "identity", "user is alice")
        .unwrap();

    let first = mem
        .mark_fact_cleared_by_id(id, &now(), ClearSource::MemoryForget)
        .unwrap();
    assert!(first, "first forget must succeed");

    let second = mem
        .mark_fact_cleared_by_id(id, &now(), ClearSource::MemoryForget)
        .unwrap();
    assert!(!second, "second forget on the same id must be a no-op");

    assert!(
        mem.load_active_facts().unwrap().is_empty(),
        "no active facts remain after forgetting the only one"
    );
}

/// `add_manual_fact` on an empty DB writes generation 1; subsequent additions
/// stay in the same active generation (don't bump it). They co-exist with the
/// curated facts produced by `replace_facts_generation`.
#[test]
fn manual_facts_and_curated_facts_coexist() {
    let mem = Memory::open_in_memory().unwrap();

    // Curated generation 1.
    mem.replace_facts_generation(&now(), &[("identity".into(), "from compactor".into())])
        .unwrap();

    // Manual addition — stays in the active generation.
    let id_manual = mem
        .add_manual_fact(&now(), "preference", "from /memory add")
        .unwrap();
    assert!(id_manual > 0);

    let facts = mem.load_active_facts().unwrap();
    assert_eq!(
        facts.len(),
        2,
        "both curated + manual facts must show up in load_active_facts"
    );
    assert!(facts
        .iter()
        .any(|f| f.insert_source == "compact" && f.claim.contains("compactor")));
    assert!(facts
        .iter()
        .any(|f| f.insert_source == "manual" && f.claim.contains("/memory add")));
}

/// `/memory list` on an empty DB returns an empty vec (not an error).
#[test]
fn list_on_empty_db_is_empty() {
    let mem = Memory::open_in_memory().unwrap();
    let facts = mem.load_active_facts().unwrap();
    assert!(facts.is_empty());
}
