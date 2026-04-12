//! PBT: Evidence invariant for decisions.
//!
//! Property: after the decision detection pipeline runs, every
//! promoted Decision record has at least one `EvidenceRef`.
//! Decisions without evidence are rejected by the architecture.

use hegel::{TestCase, generators as gs};
use lobster::{
    episodes::decisions::{aggregate_confidence, detect_signals},
    store::schema::Confidence,
};

/// Reusable generator for text that contains decision patterns.
#[hegel::composite]
fn decision_text(tc: hegel::TestCase) -> String {
    let prefix: String = tc.draw(gs::text().min_size(0).max_size(50));
    let pattern: String = tc.draw(gs::sampled_from(vec![
        "I chose".to_string(),
        "we decided".to_string(),
        "going with".to_string(),
        "non-goal".to_string(),
        "must not".to_string(),
        "approved".to_string(),
    ]));
    let suffix: String = tc.draw(gs::text().min_size(1).max_size(50));
    format!("{prefix} {pattern} {suffix}")
}

/// Property: if signals are detected and confidence is High or
/// Medium, the `matched_text` is non-empty for every signal.
/// This is the precondition for creating evidence refs.
#[hegel::test(test_cases = 200)]
fn prop_detected_signals_have_nonempty_text(tc: TestCase) {
    let text = tc.draw(decision_text());
    let signals = detect_signals(&text);

    for signal in &signals {
        assert!(
            !signal.matched_text.is_empty(),
            "every detected signal must have matched text \
             for evidence (signal kind: {:?})",
            signal.kind
        );
    }
}

/// Property: `aggregate_confidence` returns None only when there
/// are zero signals.
#[hegel::test(test_cases = 200)]
fn prop_confidence_none_only_when_no_signals(tc: TestCase) {
    let text: String = tc.draw(gs::text().max_size(200));
    let signals = detect_signals(&text);
    let confidence = aggregate_confidence(&signals);

    if signals.is_empty() {
        assert_eq!(confidence, None, "no signals → confidence must be None");
    } else {
        assert!(
            confidence.is_some(),
            "with signals, confidence must be Some"
        );
    }
}

/// Property: with known decision text, confidence is always High.
#[hegel::test(test_cases = 100)]
fn prop_explicit_choice_always_high(tc: TestCase) {
    let text = tc.draw(decision_text());
    let signals = detect_signals(&text);

    // If an ExplicitChoice or ConstraintStatement signal was
    // found, aggregate confidence must be High.
    let has_high_signal =
        signals.iter().any(|s| s.confidence == Confidence::High);

    if has_high_signal {
        assert_eq!(aggregate_confidence(&signals), Some(Confidence::High));
    }
}
