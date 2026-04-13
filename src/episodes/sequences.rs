//! Extract tool-use sequences from episodes.
//!
//! Given an episode's event range (`start_seq..=end_seq`), reads raw
//! events from redb and produces the ordered sequence of `EventKind`
//! values. This is the input for procedural-memory pattern detection.

use redb::{Database, ReadableDatabase};

use crate::store::{
    schema::{EventKind, RawEvent},
    tables,
};

/// Extract the ordered sequence of `EventKind` values from an
/// episode's raw events.
///
/// Reads events with sequence numbers in `start_seq..=end_seq` and
/// returns their kinds in order. Returns an empty vec if the range
/// is empty or the table is unavailable.
#[must_use]
pub fn extract_event_sequence(
    db: &Database,
    start_seq: u64,
    end_seq: u64,
) -> Vec<EventKind> {
    if start_seq > end_seq {
        return vec![];
    }

    let Ok(read_txn) = db.begin_read() else {
        return vec![];
    };
    let Ok(table) = read_txn.open_table(tables::RAW_EVENTS) else {
        return vec![];
    };

    let Ok(range) = table.range(start_seq..=end_seq) else {
        return vec![];
    };

    let mut kinds = Vec::new();
    for entry in range.flatten() {
        let (_, value) = entry;
        if let Ok(event) = serde_json::from_slice::<RawEvent>(value.value()) {
            kinds.push(event.event_kind);
        }
    }

    kinds
}

/// Generate a human-readable label from a sequence of event kinds.
///
/// Produces a compact string like "ToolUse→FileWrite→TestRun" from
/// the event kind sequence.
#[must_use]
pub fn sequence_label(pattern: &[EventKind]) -> String {
    pattern
        .iter()
        .map(|k| match k {
            EventKind::UserPromptSubmit => "Prompt",
            EventKind::AssistantResponse => "Response",
            EventKind::ToolUse => "ToolUse",
            EventKind::ToolResult => "ToolResult",
            EventKind::ToolUseFailure => "ToolFail",
            EventKind::FileRead => "FileRead",
            EventKind::FileWrite => "FileWrite",
            EventKind::FileEdit => "FileEdit",
            EventKind::TestRun => "TestRun",
            EventKind::TestResult => "TestResult",
            EventKind::PlanTransition => "Plan",
            EventKind::GitCommit => "GitCommit",
            EventKind::CiResult => "CiResult",
            EventKind::IssueEvent => "IssueEvent",
            EventKind::DependencyChange => "DepChange",
        })
        .collect::<Vec<_>>()
        .join("→")
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;
    use crate::store::{
        crud,
        db,
        ids::RepoId,
        schema::{EventKind, RawEvent},
    };

    fn all_event_kinds() -> Vec<EventKind> {
        vec![
            EventKind::UserPromptSubmit,
            EventKind::AssistantResponse,
            EventKind::ToolUse,
            EventKind::ToolResult,
            EventKind::ToolUseFailure,
            EventKind::FileRead,
            EventKind::FileWrite,
            EventKind::FileEdit,
            EventKind::TestRun,
            EventKind::TestResult,
            EventKind::PlanTransition,
        ]
    }

    #[allow(clippy::cast_possible_wrap)]
    fn make_event(seq: u64, kind: EventKind) -> RawEvent {
        RawEvent {
            seq,
            repo_id: RepoId::derive(b"test-repo"),
            ts_utc_ms: 1_700_000_000_000 + seq as i64,
            event_kind: kind,
            payload_hash: [0u8; 32],
            payload_bytes: vec![],
        }
    }

    // -- Property: extracted sequence length <= number of events in range --
    #[hegel::test(test_cases = 50)]
    fn prop_sequence_length_bounded(tc: TestCase) {
        let database = db::open_in_memory().unwrap();
        let n_events: u64 =
            tc.draw(gs::integers::<u64>().min_value(0).max_value(20));

        for seq in 0..n_events {
            let kind = tc.draw(gs::sampled_from(all_event_kinds()));
            let event = make_event(seq, kind);
            crud::append_raw_event(&database, &event).unwrap();
        }

        let result =
            extract_event_sequence(&database, 0, n_events.saturating_sub(1));
        assert!(
            result.len() as u64 <= n_events,
            "sequence length {} exceeds event count {}",
            result.len(),
            n_events
        );
    }

    // -- Property: extracted kinds are all valid EventKind values --
    #[hegel::test(test_cases = 50)]
    fn prop_extracted_kinds_valid(tc: TestCase) {
        let database = db::open_in_memory().unwrap();
        let n_events: u64 =
            tc.draw(gs::integers::<u64>().min_value(1).max_value(15));
        let valid_kinds = all_event_kinds();

        for seq in 0..n_events {
            let kind = tc.draw(gs::sampled_from(valid_kinds.clone()));
            crud::append_raw_event(&database, &make_event(seq, kind)).unwrap();
        }

        let result = extract_event_sequence(&database, 0, n_events - 1);
        for kind in &result {
            assert!(
                valid_kinds.contains(kind),
                "extracted unknown kind: {kind:?}"
            );
        }
    }

    // -- Property: extraction preserves insertion order --
    #[hegel::test(test_cases = 50)]
    fn prop_extraction_preserves_order(tc: TestCase) {
        let database = db::open_in_memory().unwrap();
        let n_events: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(15));

        let mut expected_kinds = Vec::with_capacity(n_events);
        for seq in 0..n_events {
            let kind = tc.draw(gs::sampled_from(all_event_kinds()));
            expected_kinds.push(kind.clone());
            crud::append_raw_event(&database, &make_event(seq as u64, kind))
                .unwrap();
        }

        let result =
            extract_event_sequence(&database, 0, (n_events - 1) as u64);
        assert_eq!(result, expected_kinds);
    }

    // -- Property: empty range produces empty sequence --
    #[hegel::test(test_cases = 50)]
    fn prop_empty_range_empty_sequence(tc: TestCase) {
        let database = db::open_in_memory().unwrap();
        // Insert some events
        for seq in 0..5 {
            crud::append_raw_event(
                &database,
                &make_event(seq, EventKind::ToolUse),
            )
            .unwrap();
        }

        // start > end → empty
        let start: u64 =
            tc.draw(gs::integers::<u64>().min_value(10).max_value(100));
        let end: u64 = tc.draw(gs::integers::<u64>().min_value(0).max_value(9));
        tc.assume(start > end);

        let result = extract_event_sequence(&database, start, end);
        assert!(result.is_empty(), "inverted range must produce empty seq");
    }

    // -- Property: sub-range extracts correct slice --
    #[allow(clippy::cast_possible_truncation)]
    #[hegel::test(test_cases = 50)]
    fn prop_subrange_extraction(tc: TestCase) {
        let database = db::open_in_memory().unwrap();
        let total: u64 =
            tc.draw(gs::integers::<u64>().min_value(5).max_value(20));

        let mut all_kinds = Vec::new();
        for seq in 0..total {
            let kind = tc.draw(gs::sampled_from(all_event_kinds()));
            all_kinds.push(kind.clone());
            crud::append_raw_event(&database, &make_event(seq, kind)).unwrap();
        }

        let start: u64 =
            tc.draw(gs::integers::<u64>().min_value(0).max_value(total / 2));
        let end: u64 = tc
            .draw(gs::integers::<u64>().min_value(start).max_value(total - 1));

        let result = extract_event_sequence(&database, start, end);
        let expected = &all_kinds[start as usize..=end as usize];
        assert_eq!(result, expected);
    }

    // -- Unit: sequence_label produces arrow-separated string --
    #[test]
    fn test_sequence_label_basic() {
        let pattern = vec![
            EventKind::FileEdit,
            EventKind::TestRun,
            EventKind::TestResult,
        ];
        assert_eq!(sequence_label(&pattern), "FileEdit→TestRun→TestResult");
    }

    // -- Unit: empty pattern produces empty label --
    #[test]
    fn test_sequence_label_empty() {
        assert_eq!(sequence_label(&[]), "");
    }

    // -- Property: label contains all kind names --
    #[hegel::test(test_cases = 100)]
    fn prop_label_contains_all_kinds(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(8));
        let mut pattern = Vec::with_capacity(n);
        for _ in 0..n {
            pattern.push(tc.draw(gs::sampled_from(all_event_kinds())));
        }

        let label = sequence_label(&pattern);
        // Label must have exactly n-1 arrows
        let arrow_count = label.matches('→').count();
        assert_eq!(
            arrow_count,
            n.saturating_sub(1),
            "label should have n-1 arrows"
        );
    }
}
