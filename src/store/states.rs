//! Processing state transition validation.
//!
//! Enforces the legal state machine transitions defined in the
//! architecture. Illegal transitions are rejected.

use crate::store::schema::ProcessingState;

/// Error when attempting an illegal state transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IllegalTransition {
    pub from: ProcessingState,
    pub to: ProcessingState,
}

impl std::fmt::Display for IllegalTransition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "illegal transition: {:?} -> {:?}", self.from, self.to)
    }
}

impl std::error::Error for IllegalTransition {}

/// Validate that a state transition is legal.
///
/// Legal transitions:
/// - `Pending` → `Ready`
/// - `Pending` → `RetryQueued`
/// - `RetryQueued` → `Ready`
/// - `RetryQueued` → `FailedFinal`
///
/// All other transitions are rejected.
/// # Errors
///
/// Returns `IllegalTransition` if the transition is not in the set
/// of legal state changes.
pub const fn validate_transition(
    from: ProcessingState,
    to: ProcessingState,
) -> Result<(), IllegalTransition> {
    use ProcessingState::{FailedFinal, Pending, Ready, RetryQueued};

    let legal = matches!(
        (from, to),
        (Pending | RetryQueued, Ready)
            | (Pending, RetryQueued)
            | (RetryQueued, FailedFinal)
    );

    if legal {
        Ok(())
    } else {
        Err(IllegalTransition { from, to })
    }
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;
    use crate::store::schema::ProcessingState::{
        self,
        FailedFinal,
        Pending,
        Ready,
        RetryQueued,
    };

    const ALL_STATES: [ProcessingState; 4] =
        [Pending, Ready, RetryQueued, FailedFinal];

    // -- Property: legal transitions succeed --
    #[test]
    fn test_legal_transitions() {
        assert!(validate_transition(Pending, Ready).is_ok());
        assert!(validate_transition(Pending, RetryQueued).is_ok());
        assert!(validate_transition(RetryQueued, Ready).is_ok());
        assert!(validate_transition(RetryQueued, FailedFinal).is_ok());
    }

    // -- Property: self-transitions are illegal --
    #[test]
    fn test_self_transitions_illegal() {
        for s in &ALL_STATES {
            assert!(
                validate_transition(*s, *s).is_err(),
                "{s:?} -> {s:?} should be illegal"
            );
        }
    }

    // -- Property: Ready is a terminal state --
    // No outgoing transitions from Ready.
    #[test]
    fn test_ready_terminal() {
        for to in &ALL_STATES {
            if *to != Ready {
                assert!(
                    validate_transition(Ready, *to).is_err(),
                    "Ready -> {to:?} should be illegal"
                );
            }
        }
        // Including self
        assert!(validate_transition(Ready, Ready).is_err());
    }

    // -- Property: FailedFinal is a terminal state --
    #[test]
    fn test_failed_final_terminal() {
        for to in &ALL_STATES {
            assert!(
                validate_transition(FailedFinal, *to).is_err(),
                "FailedFinal -> {to:?} should be illegal"
            );
        }
    }

    // -- PBT: exactly 4 legal transitions exist --
    // Exhaustive check over all 16 possible transitions.
    #[hegel::test(test_cases = 200)]
    fn prop_transition_deterministic(tc: TestCase) {
        let from_idx: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(3));
        let to_idx: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(3));
        let from = ALL_STATES[from_idx];
        let to = ALL_STATES[to_idx];

        let result1 = validate_transition(from, to);
        let result2 = validate_transition(from, to);

        // Determinism: same inputs, same result
        assert_eq!(result1.is_ok(), result2.is_ok());
    }

    // -- Exactly 4 legal transitions --
    #[test]
    fn test_exactly_four_legal_transitions() {
        let mut legal_count = 0;
        for from in &ALL_STATES {
            for to in &ALL_STATES {
                if validate_transition(*from, *to).is_ok() {
                    legal_count += 1;
                }
            }
        }
        assert_eq!(legal_count, 4);
    }
}
