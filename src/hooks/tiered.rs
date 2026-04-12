//! Tiered output for automatic recall.
//!
//! Usually brief hints. Expand to structured block when confidence
//! is high. Follows the architecture: expandable layered recall.

use serde::Serialize;

use crate::hooks::recall::{RecallItem, RecallPayload};

/// Output tier for the recall payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OutputTier {
    /// No recall items — stay silent.
    Silent,
    /// 1-2 brief hint items.
    Hint,
    /// Structured block with full evidence.
    Structured,
}

/// Determine the output tier from a recall payload.
///
/// - 0 items → Silent
/// - 1-2 items → Hint
/// - 3+ items → Structured
#[must_use]
pub fn classify_tier(payload: &RecallPayload) -> OutputTier {
    match payload.items.len() {
        0 => OutputTier::Silent,
        1 | 2 => OutputTier::Hint,
        _ => OutputTier::Structured,
    }
}

/// Format a recall payload as a minimal hint string.
///
/// Used when the tier is Hint — produces a compact one-liner.
#[must_use]
pub fn format_hint(payload: &RecallPayload) -> String {
    if payload.items.is_empty() {
        return String::new();
    }

    payload
        .items
        .iter()
        .map(|item| match item {
            RecallItem::Decision(d) => {
                format!("Decision: {}", d.statement)
            }
            RecallItem::Summary(s) => {
                let preview: String = s.summary_text.chars().take(80).collect();
                format!("Summary: {preview}")
            }
            RecallItem::Hint { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_payload() -> RecallPayload {
        RecallPayload {
            items: vec![],
            truncated: None,
            latency_ms: 0,
        }
    }

    fn hint_payload() -> RecallPayload {
        RecallPayload {
            items: vec![RecallItem::Hint {
                text: "Related to storage decision".into(),
            }],
            truncated: None,
            latency_ms: 10,
        }
    }

    fn structured_payload() -> RecallPayload {
        RecallPayload {
            items: vec![
                RecallItem::Hint {
                    text: "item 1".into(),
                },
                RecallItem::Hint {
                    text: "item 2".into(),
                },
                RecallItem::Hint {
                    text: "item 3".into(),
                },
            ],
            truncated: None,
            latency_ms: 50,
        }
    }

    #[test]
    fn test_classify_silent() {
        assert_eq!(classify_tier(&empty_payload()), OutputTier::Silent);
    }

    #[test]
    fn test_classify_hint() {
        assert_eq!(classify_tier(&hint_payload()), OutputTier::Hint);
    }

    #[test]
    fn test_classify_structured() {
        assert_eq!(
            classify_tier(&structured_payload()),
            OutputTier::Structured
        );
    }

    #[test]
    fn test_format_hint_empty() {
        assert!(format_hint(&empty_payload()).is_empty());
    }

    #[test]
    fn test_format_hint_single_item() {
        let s = format_hint(&hint_payload());
        assert_eq!(s, "Related to storage decision");
    }

    #[test]
    fn test_format_hint_multiple_items() {
        let s = format_hint(&structured_payload());
        assert!(s.contains("item 1"));
        assert!(s.contains(" | "));
    }
}
