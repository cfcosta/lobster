//! Decision detection signal taxonomy and heuristics.
//!
//! Decisions are detected from episode content using deterministic
//! heuristic patterns. The detection pipeline is the only component
//! that creates canonical Decision records.

use serde::{Deserialize, Serialize};

use crate::store::schema::Confidence;

/// A signal type that indicates a potential decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignalKind {
    /// Explicit choice language: "I chose", "we decided", "going with"
    ExplicitChoice,
    /// Plan approval: "yes, do that", "approved"
    PlanApproval,
    /// Implementation commitment: "I will implement X"
    ImplementationCommitment,
    /// Change/fix confirmation: "that fixed it", "the change works"
    ChangeConfirmation,
    /// Test outcome tied to path selection
    TestOutcome,
    /// Stated constraint or non-goal: "must not", "never", "not doing X"
    ConstraintStatement,
}

/// A detected decision signal with its source text and confidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedSignal {
    pub kind: SignalKind,
    pub matched_text: String,
    pub confidence: Confidence,
}

/// Scan text for decision signals.
///
/// Returns all detected signals. Confidence is assigned based on
/// signal strength: explicit choice/constraint are High, plan
/// approval is Medium, others are Low.
#[must_use]
pub fn detect_signals(text: &str) -> Vec<DetectedSignal> {
    let lower = text.to_lowercase();
    let mut signals = Vec::new();

    // Explicit choice patterns
    for pat in &[
        "i chose",
        "we decided",
        "going with",
        "decided to",
        "the decision is",
        "i picked",
    ] {
        if lower.contains(pat) {
            signals.push(DetectedSignal {
                kind: SignalKind::ExplicitChoice,
                matched_text: extract_sentence(&lower, pat),
                confidence: Confidence::High,
            });
        }
    }

    // Plan approval patterns
    for pat in &["yes, do that", "approved", "looks good, proceed"] {
        if lower.contains(pat) {
            signals.push(DetectedSignal {
                kind: SignalKind::PlanApproval,
                matched_text: extract_sentence(&lower, pat),
                confidence: Confidence::Medium,
            });
        }
    }

    // Implementation commitment
    for pat in &[
        "i will implement",
        "let's implement",
        "implementing",
        "i'll build",
    ] {
        if lower.contains(pat) {
            signals.push(DetectedSignal {
                kind: SignalKind::ImplementationCommitment,
                matched_text: extract_sentence(&lower, pat),
                confidence: Confidence::Medium,
            });
        }
    }

    // Change/fix confirmation
    for pat in &[
        "that fixed it",
        "the change works",
        "confirmed working",
        "fix verified",
    ] {
        if lower.contains(pat) {
            signals.push(DetectedSignal {
                kind: SignalKind::ChangeConfirmation,
                matched_text: extract_sentence(&lower, pat),
                confidence: Confidence::Medium,
            });
        }
    }

    // Test outcome
    for pat in &["tests pass", "test passed", "all tests green"] {
        if lower.contains(pat) {
            signals.push(DetectedSignal {
                kind: SignalKind::TestOutcome,
                matched_text: extract_sentence(&lower, pat),
                confidence: Confidence::Low,
            });
        }
    }

    // Constraint/non-goal
    for pat in &[
        "must not",
        "must never",
        "we should never",
        "non-goal",
        "out of scope",
        "not doing",
    ] {
        if lower.contains(pat) {
            signals.push(DetectedSignal {
                kind: SignalKind::ConstraintStatement,
                matched_text: extract_sentence(&lower, pat),
                confidence: Confidence::High,
            });
        }
    }

    signals
}

/// Determine overall confidence from a set of signals.
#[must_use]
#[allow(clippy::option_if_let_else)]
pub fn aggregate_confidence(signals: &[DetectedSignal]) -> Option<Confidence> {
    if signals.is_empty() {
        return None;
    }
    if signals.iter().any(|s| s.confidence == Confidence::High) {
        return Some(Confidence::High);
    }
    if signals.len() >= 2 {
        return Some(Confidence::Medium);
    }
    Some(Confidence::Low)
}

fn extract_sentence(text: &str, pattern: &str) -> String {
    text.find(pattern).map_or_else(String::new, |pos| {
        let start = text[..pos]
            .rfind(['.', '!', '?', '\n'])
            .map_or(0, |p| p + 1);
        let end = text[pos..]
            .find(['.', '!', '?', '\n'])
            .map_or(text.len(), |p| pos + p + 1);
        text[start..end].trim().to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explicit_choice_detected() {
        let signals = detect_signals(
            "After evaluating options, I chose redb for storage.",
        );
        assert!(!signals.is_empty());
        assert_eq!(signals[0].kind, SignalKind::ExplicitChoice);
        assert_eq!(signals[0].confidence, Confidence::High);
    }

    #[test]
    fn test_constraint_detected() {
        let signals = detect_signals("Cloud sync is a non-goal for v1.");
        assert!(!signals.is_empty());
        assert_eq!(signals[0].kind, SignalKind::ConstraintStatement);
    }

    #[test]
    fn test_no_signals_in_plain_text() {
        let signals = detect_signals("The weather is nice today.");
        assert!(signals.is_empty());
    }

    #[test]
    fn test_aggregate_high_confidence() {
        let signals = vec![DetectedSignal {
            kind: SignalKind::ExplicitChoice,
            matched_text: "chose redb".into(),
            confidence: Confidence::High,
        }];
        assert_eq!(aggregate_confidence(&signals), Some(Confidence::High));
    }

    #[test]
    fn test_aggregate_multiple_low_becomes_medium() {
        let signals = vec![
            DetectedSignal {
                kind: SignalKind::TestOutcome,
                matched_text: "tests pass".into(),
                confidence: Confidence::Low,
            },
            DetectedSignal {
                kind: SignalKind::ChangeConfirmation,
                matched_text: "confirmed working".into(),
                confidence: Confidence::Medium,
            },
        ];
        assert_eq!(aggregate_confidence(&signals), Some(Confidence::Medium));
    }

    #[test]
    fn test_aggregate_empty_is_none() {
        assert_eq!(aggregate_confidence(&[]), None);
    }
}
