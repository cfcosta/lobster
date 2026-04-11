//! Evidence-window expansion for surfaced retrieval results.
//!
//! When a decision, summary, or entity is surfaced, expand it
//! with its local evidence window so the result carries enough
//! context to be useful.

use serde::{Deserialize, Serialize};

/// Expanded evidence window for a surfaced decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionEvidence {
    pub statement: String,
    pub rationale: String,
    pub confidence: String,
    pub evidence: Vec<EvidenceItem>,
    pub task_context: Option<TaskContext>,
}

/// Expanded evidence window for a surfaced summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryEvidence {
    pub summary_text: String,
    pub episode_id: String,
    pub decisions_supported: Vec<DecisionRef>,
}

/// Expanded evidence window for a surfaced entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityEvidence {
    pub canonical_name: String,
    pub kind: String,
    pub related_decisions: Vec<DecisionRef>,
    pub related_tasks: Vec<TaskRef>,
}

/// An evidence item pointing back to a source episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub episode_id: String,
    pub span_summary: String,
}

/// Compact reference to a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRef {
    pub decision_id: String,
    pub statement: String,
}

/// Compact reference to a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRef {
    pub task_id: String,
    pub title: String,
}

/// Compact task context attached to a surfaced decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskContext {
    pub task_id: String,
    pub title: String,
    pub status: String,
}

/// Maximum evidence refs per decision.
pub const MAX_EVIDENCE_REFS: usize = 3;

/// Maximum supported decisions per summary.
pub const MAX_DECISIONS_PER_SUMMARY: usize = 2;

/// Build a decision evidence window from stored data.
#[must_use]
pub fn expand_decision(
    statement: &str,
    rationale: &str,
    confidence: &str,
    evidence: &[(String, String)],
    task: Option<(&str, &str, &str)>,
) -> DecisionEvidence {
    DecisionEvidence {
        statement: statement.to_string(),
        rationale: rationale.to_string(),
        confidence: confidence.to_string(),
        evidence: evidence
            .iter()
            .take(MAX_EVIDENCE_REFS)
            .map(|(ep_id, span)| EvidenceItem {
                episode_id: ep_id.clone(),
                span_summary: span.clone(),
            })
            .collect(),
        task_context: task.map(|(id, title, status)| TaskContext {
            task_id: id.to_string(),
            title: title.to_string(),
            status: status.to_string(),
        }),
    }
}

/// Build a summary evidence window.
#[must_use]
pub fn expand_summary(
    summary_text: &str,
    episode_id: &str,
    decisions: &[(String, String)],
) -> SummaryEvidence {
    SummaryEvidence {
        summary_text: summary_text.to_string(),
        episode_id: episode_id.to_string(),
        decisions_supported: decisions
            .iter()
            .take(MAX_DECISIONS_PER_SUMMARY)
            .map(|(id, stmt)| DecisionRef {
                decision_id: id.clone(),
                statement: stmt.clone(),
            })
            .collect(),
    }
}

/// Build an entity evidence window.
#[must_use]
pub fn expand_entity(
    name: &str,
    kind: &str,
    decisions: &[(String, String)],
    tasks: &[(String, String)],
) -> EntityEvidence {
    EntityEvidence {
        canonical_name: name.to_string(),
        kind: kind.to_string(),
        related_decisions: decisions
            .iter()
            .map(|(id, stmt)| DecisionRef {
                decision_id: id.clone(),
                statement: stmt.clone(),
            })
            .collect(),
        related_tasks: tasks
            .iter()
            .map(|(id, title)| TaskRef {
                task_id: id.clone(),
                title: title.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decision_evidence_truncates() {
        let evidence: Vec<_> = (0..10)
            .map(|i| (format!("ep{i}"), format!("span{i}")))
            .collect();
        let expanded = expand_decision(
            "use redb",
            "embedded, ACID",
            "high",
            &evidence,
            None,
        );
        assert_eq!(expanded.evidence.len(), MAX_EVIDENCE_REFS);
    }

    #[test]
    fn test_decision_with_task_context() {
        let expanded = expand_decision(
            "statement",
            "rationale",
            "high",
            &[("ep1".into(), "discussed".into())],
            Some(("task1", "Build memory", "InProgress")),
        );
        assert!(expanded.task_context.is_some());
        let ctx = expanded.task_context.unwrap();
        assert_eq!(ctx.title, "Build memory");
    }

    #[test]
    fn test_summary_evidence_truncates() {
        let decisions: Vec<_> = (0..10)
            .map(|i| (format!("dec{i}"), format!("stmt{i}")))
            .collect();
        let expanded = expand_summary("summary text", "ep1", &decisions);
        assert_eq!(
            expanded.decisions_supported.len(),
            MAX_DECISIONS_PER_SUMMARY
        );
    }

    #[test]
    fn test_entity_evidence() {
        let expanded = expand_entity(
            "Grafeo",
            "component",
            &[("dec1".into(), "use Grafeo".into())],
            &[("task1".into(), "Build graph layer".into())],
        );
        assert_eq!(expanded.canonical_name, "Grafeo");
        assert_eq!(expanded.related_decisions.len(), 1);
        assert_eq!(expanded.related_tasks.len(), 1);
    }

    // -- Property: serde round-trip --
    #[test]
    fn test_decision_evidence_serde() {
        let expanded = expand_decision(
            "use redb",
            "ACID compliance",
            "high",
            &[("ep1".into(), "discussed".into())],
            Some(("t1", "storage", "Open")),
        );
        let json = serde_json::to_string(&expanded).unwrap();
        let parsed: DecisionEvidence = serde_json::from_str(&json).unwrap();
        assert_eq!(expanded, parsed);
    }
}
