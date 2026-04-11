//! Episode segmentation rules.
//!
//! Episodes are built from raw events using deterministic rules:
//! idle gaps, repo transitions, task-intent changes, and hook
//! boundaries. Tie-breaks are stable and testable.

use crate::store::schema::RawEvent;

/// Configuration for episode segmentation.
#[derive(Debug, Clone)]
pub struct SegmentationConfig {
    /// Maximum idle gap in milliseconds before starting a new episode.
    pub idle_gap_ms: i64,
}

impl Default for SegmentationConfig {
    fn default() -> Self {
        Self {
            idle_gap_ms: 5 * 60 * 1000, // 5 minutes
        }
    }
}

/// Reason an episode boundary was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundaryReason {
    IdleGap { gap_ms: i64 },
    RepoTransition { from_repo: String, to_repo: String },
    FirstEvent,
}

/// A detected episode boundary between two events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Boundary {
    /// Sequence number of the event AFTER the boundary.
    pub after_seq: u64,
    pub reason: BoundaryReason,
}

/// Detect episode boundaries in a sequence of raw events.
///
/// Events must be sorted by sequence number. Returns boundaries
/// indicating where to split events into episodes.
#[must_use]
pub fn detect_boundaries(
    events: &[RawEvent],
    config: &SegmentationConfig,
) -> Vec<Boundary> {
    if events.is_empty() {
        return vec![];
    }

    let mut boundaries = vec![Boundary {
        after_seq: events[0].seq,
        reason: BoundaryReason::FirstEvent,
    }];

    for window in events.windows(2) {
        let prev = &window[0];
        let curr = &window[1];

        // Check idle gap
        let gap_ms = curr.ts_utc_ms - prev.ts_utc_ms;
        if gap_ms >= config.idle_gap_ms {
            boundaries.push(Boundary {
                after_seq: curr.seq,
                reason: BoundaryReason::IdleGap { gap_ms },
            });
            continue;
        }

        // Check repo transition
        if prev.repo_id != curr.repo_id {
            boundaries.push(Boundary {
                after_seq: curr.seq,
                reason: BoundaryReason::RepoTransition {
                    from_repo: prev.repo_id.to_string(),
                    to_repo: curr.repo_id.to_string(),
                },
            });
        }
    }

    boundaries
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;
    use crate::store::{ids::RepoId, schema::EventKind};

    fn make_event(seq: u64, ts_ms: i64, repo: &[u8]) -> RawEvent {
        RawEvent {
            seq,
            repo_id: RepoId::derive(repo),
            ts_utc_ms: ts_ms,
            event_kind: EventKind::ToolUse,
            payload_hash: [0; 32],
            payload_bytes: vec![],
        }
    }

    #[test]
    fn test_empty_events() {
        let config = SegmentationConfig::default();
        assert!(detect_boundaries(&[], &config).is_empty());
    }

    #[test]
    fn test_single_event_has_first_boundary() {
        let config = SegmentationConfig::default();
        let events = vec![make_event(0, 1000, b"repo")];
        let boundaries = detect_boundaries(&events, &config);
        assert_eq!(boundaries.len(), 1);
        assert_eq!(boundaries[0].reason, BoundaryReason::FirstEvent);
    }

    #[test]
    fn test_idle_gap_creates_boundary() {
        let config = SegmentationConfig { idle_gap_ms: 1000 };
        let events = vec![
            make_event(0, 0, b"repo"),
            make_event(1, 500, b"repo"), // same episode
            make_event(2, 2000, b"repo"), // gap = 1500ms > 1000ms
        ];
        let boundaries = detect_boundaries(&events, &config);
        // FirstEvent + IdleGap
        assert_eq!(boundaries.len(), 2);
        assert!(matches!(
            boundaries[1].reason,
            BoundaryReason::IdleGap { gap_ms: 1500 }
        ));
        assert_eq!(boundaries[1].after_seq, 2);
    }

    #[test]
    fn test_repo_transition_creates_boundary() {
        let config = SegmentationConfig::default();
        let events =
            vec![make_event(0, 0, b"repo-a"), make_event(1, 100, b"repo-b")];
        let boundaries = detect_boundaries(&events, &config);
        assert_eq!(boundaries.len(), 2);
        assert!(matches!(
            boundaries[1].reason,
            BoundaryReason::RepoTransition { .. }
        ));
    }

    // -- PBT: segmentation is deterministic --
    #[hegel::test(test_cases = 100)]
    fn prop_segmentation_deterministic(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(20));
        let mut events = Vec::with_capacity(n);
        let mut ts = 0i64;
        for i in 0..n {
            let gap: i64 =
                tc.draw(gs::integers::<i64>().min_value(0).max_value(600_000));
            ts += gap;
            events.push(make_event(i as u64, ts, b"repo"));
        }

        let config = SegmentationConfig::default();
        let b1 = detect_boundaries(&events, &config);
        let b2 = detect_boundaries(&events, &config);
        assert_eq!(b1, b2, "segmentation must be deterministic");
    }

    // -- PBT: first event always produces a boundary --
    #[hegel::test(test_cases = 100)]
    fn prop_first_event_boundary(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(10));
        let events: Vec<RawEvent> = (0..n)
            .map(|i| make_event(i as u64, i as i64 * 1000, b"repo"))
            .collect();

        let config = SegmentationConfig::default();
        let boundaries = detect_boundaries(&events, &config);
        assert!(!boundaries.is_empty());
        assert_eq!(boundaries[0].reason, BoundaryReason::FirstEvent);
    }
}
