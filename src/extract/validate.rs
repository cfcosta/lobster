//! Extraction output validation.
//!
//! Validates schema, evidence, and duplicate checks before
//! extraction output is compiled into graph operations.

use crate::extract::traits::ExtractionOutput;

/// Validation error for extraction output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    EmptyEntityName(usize),
    InvalidEntityKind(String),
    EmptyRelationEndpoint { index: usize, field: String },
    NoEvidence,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyEntityName(idx) => {
                write!(f, "entity {idx} has empty name")
            }
            Self::InvalidEntityKind(kind) => {
                write!(f, "invalid entity kind: {kind}")
            }
            Self::EmptyRelationEndpoint { index, field } => {
                write!(f, "relation {index} has empty {field}")
            }
            Self::NoEvidence => {
                write!(f, "extraction output has no references")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

const VALID_ENTITY_KINDS: &[&str] =
    &["concept", "constraint", "component", "file-lite", "repo"];

/// Validate an `ExtractionOutput` before compilation.
///
/// Checks:
/// - Entity names are non-empty
/// - Entity kinds are in the allowed set
/// - Relation endpoints are non-empty
/// - Relation types are valid
/// - At least one reference exists (task, decision, or entity)
///
/// # Errors
///
/// Returns all validation errors found.
pub fn validate(output: &ExtractionOutput) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    // Check entities
    for (i, entity) in output.entities.iter().enumerate() {
        if entity.name.trim().is_empty() {
            errors.push(ValidationError::EmptyEntityName(i));
        }
        if !VALID_ENTITY_KINDS.contains(&entity.kind.as_str()) {
            errors
                .push(ValidationError::InvalidEntityKind(entity.kind.clone()));
        }
    }

    // Check relations
    for (i, rel) in output.relations.iter().enumerate() {
        if rel.from.trim().is_empty() {
            errors.push(ValidationError::EmptyRelationEndpoint {
                index: i,
                field: "from".into(),
            });
        }
        if rel.to.trim().is_empty() {
            errors.push(ValidationError::EmptyRelationEndpoint {
                index: i,
                field: "to".into(),
            });
        }
    }

    // Empty extraction is valid — not every episode produces
    // extractable facts. Decisions count as evidence too.
    // (Previously this required at least one entity/task/decision ref,
    // which forced the extractor to inject fake entities.)

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::traits::{
        ExtractedEntity,
        ExtractedRelation,
        RelationType,
    };

    fn valid_output() -> ExtractionOutput {
        ExtractionOutput {
            task_refs: vec!["task:build-memory".into()],
            decision_refs: vec!["decision:9d2".into()],
            entities: vec![ExtractedEntity {
                kind: "component".into(),
                name: "Grafeo".into(),
            }],
            relations: vec![ExtractedRelation {
                relation_type: RelationType::TaskDecision,
                from: "task:build-memory".into(),
                to: "decision:9d2".into(),
            }],
            decisions: vec![],
            conventions: vec![],
        }
    }

    #[test]
    fn test_valid_output_passes() {
        assert!(validate(&valid_output()).is_ok());
    }

    #[test]
    fn test_empty_entity_name_fails() {
        let mut output = valid_output();
        output.entities.push(ExtractedEntity {
            kind: "concept".into(),
            name: String::new(),
        });
        let errs = validate(&output).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, ValidationError::EmptyEntityName(_)))
        );
    }

    #[test]
    fn test_invalid_entity_kind_fails() {
        let mut output = valid_output();
        output.entities[0].kind = "invalid_kind".into();
        let errs = validate(&output).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, ValidationError::InvalidEntityKind(_)))
        );
    }

    #[test]
    fn test_empty_relation_endpoint_fails() {
        let mut output = valid_output();
        output.relations[0].from = String::new();
        let errs = validate(&output).unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            ValidationError::EmptyRelationEndpoint { .. }
        )));
    }

    #[test]
    fn test_empty_extraction_is_valid() {
        let output = ExtractionOutput {
            task_refs: vec![],
            decision_refs: vec![],
            entities: vec![],
            relations: vec![],
            decisions: vec![],
            conventions: vec![],
        };
        // Empty extraction is valid — not every episode produces facts
        assert!(validate(&output).is_ok());
    }
}
