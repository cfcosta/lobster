//! Integration tests for the hook recall pipeline.
//!
//! Tests the full path: finalize episodes → open snapshot →
//! rebuild Grafeo → run recall → format output.

use hegel::{TestCase, generators as gs};
use lobster::{
    episodes::finalize::{FinalizeResult, finalize_episode},
    graph::{db as grafeo_db, indexes},
    hooks::{
        events::HookEvent,
        recall::{self, run_recall},
        tiered::{OutputTier, classify_tier, format_hint},
    },
    store::db,
};

fn make_prompt_event(prompt: &str) -> HookEvent {
    serde_json::from_value(serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "tool_input": {"prompt": prompt},
        "cwd": "/test/repo",
    }))
    .unwrap()
}

fn make_stop_event() -> HookEvent {
    serde_json::from_value(serde_json::json!({
        "hook_event_name": "Stop",
        "reason": "done",
    }))
    .unwrap()
}

// ── construct_query ─────────────────────────────────────────────

/// `construct_query` extracts the prompt from `UserPromptSubmit`.
#[hegel::test(test_cases = 200)]
fn prop_construct_query_extracts_prompt(tc: TestCase) {
    let prompt: String = tc.draw(
        gs::text()
            .min_size(1)
            .max_size(200)
            .alphabet("abcdefghijklmnopqrstuvwxyz "),
    );
    let event = make_prompt_event(&prompt);
    let query = recall::construct_query(&event);
    assert_eq!(query.as_deref(), Some(prompt.as_str()));
}

/// `construct_query` returns None for Stop events.
#[test]
fn test_construct_query_stop_none() {
    assert!(recall::construct_query(&make_stop_event()).is_none());
}

// ── run_recall ──────────────────────────────────────────────────

/// `run_recall` on an empty database returns empty payload.
#[hegel::test(test_cases = 50)]
fn prop_recall_empty_db_returns_empty(tc: TestCase) {
    let prompt: String = tc.draw(
        gs::text()
            .min_size(1)
            .max_size(100)
            .alphabet("abcdefghijklmnopqrstuvwxyz "),
    );
    let (database, _dir) = db::open_in_memory().unwrap();
    let grafeo = grafeo_db::new_in_memory();
    let event = make_prompt_event(&prompt);

    let payload = run_recall(&event, &database, &grafeo);
    assert!(payload.items.is_empty());
}

/// `run_recall` for Stop events returns empty (latency 0).
#[test]
fn test_recall_stop_returns_empty() {
    let (database, _dir) = db::open_in_memory().unwrap();
    let grafeo = grafeo_db::new_in_memory();
    let payload = run_recall(&make_stop_event(), &database, &grafeo);
    assert!(payload.items.is_empty());
    assert_eq!(payload.latency_ms, 0);
}

/// `run_recall` on a populated database with finalized episodes
/// completes within the latency budget.
#[hegel::test(test_cases = 10)]
fn prop_recall_respects_latency_budget(tc: TestCase) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let (database, _dir) = db::open_in_memory().unwrap();
    let grafeo = grafeo_db::new_in_memory();

    // Finalize a few episodes to populate the DB
    let n: usize = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
    for i in 0..n {
        let result = rt.block_on(finalize_episode(
            &database,
            &grafeo,
            "/test/repo",
            b"[]",
            i as u64 * 10,
            i as u64 * 10 + 5,
            None,
        ));
        assert!(matches!(result, FinalizeResult::Ready { .. }));
    }
    indexes::ensure_indexes(&grafeo);

    let event = make_prompt_event("test query about storage");
    let payload = run_recall(&event, &database, &grafeo);

    // Latency should be reported (even if 0) and under 5 seconds
    assert!(
        payload.latency_ms < 5000,
        "recall took {}ms",
        payload.latency_ms
    );
}

// ── tiered output ───────────────────────────────────────────────

/// `classify_tier` is Silent for empty payloads.
#[test]
fn test_tier_silent_for_empty() {
    let payload = recall::RecallPayload {
        items: vec![],
        truncated: None,
        latency_ms: 0,
    };
    assert_eq!(classify_tier(&payload), OutputTier::Silent);
}

/// `format_hint` output is bounded: never exceeds a reasonable
/// size even with maximum items.
#[hegel::test(test_cases = 100)]
fn prop_format_hint_bounded(tc: TestCase) {
    let n: usize = tc.draw(gs::integers::<usize>().min_value(0).max_value(5));
    let items: Vec<recall::RecallItem> = (0..n)
        .map(|_| recall::RecallItem::Hint {
            text: tc.draw(
                gs::text()
                    .min_size(1)
                    .max_size(200)
                    .alphabet("abcdefghijklmnopqrstuvwxyz "),
            ),
        })
        .collect();

    let payload = recall::RecallPayload {
        items,
        truncated: None,
        latency_ms: 0,
    };
    let hint = format_hint(&payload);

    // Each item is at most 200 chars, with " | " separators.
    // 5 items * 200 + 4 * 3 = 1012. Use 2000 as generous bound.
    assert!(
        hint.len() < 2000,
        "format_hint output too large: {} bytes",
        hint.len()
    );
}

/// `format_hint` on empty payload returns empty string.
#[test]
fn test_format_hint_empty() {
    let payload = recall::RecallPayload {
        items: vec![],
        truncated: None,
        latency_ms: 0,
    };
    assert!(format_hint(&payload).is_empty());
}

// ── open_snapshot integration ───────────────────────────────────

/// Full path: finalize → snapshot → rebuild → recall succeeds.
#[test]
fn test_full_snapshot_recall_path() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("lmdb");

    // Finalize an episode into a file-based DB
    let database = db::open(&db_path).unwrap();
    let grafeo = grafeo_db::new_in_memory();
    let result = rt.block_on(finalize_episode(
        &database,
        &grafeo,
        "/test/repo",
        b"[]",
        0,
        5,
        None,
    ));
    assert!(matches!(result, FinalizeResult::Ready { .. }));
    drop(database);

    // Reopen the database (LMDB allows concurrent readers)
    let snap = db::open(&db_path).expect("reopen");
    let snap_grafeo = grafeo_db::new_in_memory();
    lobster::graph::rebuild::rebuild_from_redb(&snap, &snap_grafeo)
        .expect("rebuild");
    indexes::ensure_indexes(&snap_grafeo);

    // Run recall against the snapshot
    let event = make_prompt_event("test query");
    let payload = run_recall(&event, &snap, &snap_grafeo);

    // Should not panic, latency should be reported
    assert!(payload.latency_ms < 5000);
}
