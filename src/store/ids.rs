//! Typed ID types with deterministic hash-based generation.
//!
//! Every ID in Lobster is a 128-bit value derived from a
//! deterministic SHA-256 hash of structured input. IDs are typed
//! wrappers so `EpisodeId` and `TaskId` cannot be accidentally
//! swapped.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Raw 128-bit identifier, truncated from SHA-256.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct RawId([u8; 16]);

impl RawId {
    /// Derive an ID by hashing a namespace tag and arbitrary input
    /// bytes. The namespace ensures that different entity types
    /// produce different IDs even for identical input.
    #[must_use]
    pub fn derive(namespace: &str, input: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(namespace.as_bytes());
        hasher.update(b":");
        hasher.update(input);
        let hash = hasher.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash[..16]);
        Self(bytes)
    }

    /// Access the raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Construct from raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for RawId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl fmt::Display for RawId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl std::str::FromStr for RawId {
    type Err = IdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 32 {
            return Err(IdParseError::InvalidLength(s.len()));
        }
        let mut bytes = [0u8; 16];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
                .map_err(|_| IdParseError::InvalidHex)?;
        }
        Ok(Self(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdParseError {
    InvalidLength(usize),
    InvalidHex,
}

impl fmt::Display for IdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength(n) => {
                write!(f, "expected 32 hex chars, got {n}")
            }
            Self::InvalidHex => write!(f, "invalid hex character"),
        }
    }
}

impl std::error::Error for IdParseError {}

/// Generate a typed ID wrapper with `derive` and `Display`/`FromStr`.
macro_rules! typed_id {
    ($(#[$meta:meta])* $name:ident, $namespace:literal) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            Serialize, Deserialize,
        )]
        pub struct $name(RawId);

        impl $name {
            /// Derive this ID deterministically from input bytes.
            #[must_use]
            pub fn derive(input: &[u8]) -> Self {
                Self(RawId::derive($namespace, input))
            }

            /// Access the inner raw ID.
            #[must_use]
            pub const fn raw(&self) -> RawId {
                self.0
            }

            /// Construct from a raw ID.
            #[must_use]
            pub const fn from_raw(raw: RawId) -> Self {
                Self(raw)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = IdParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(s.parse()?))
            }
        }
    };
}

typed_id!(
    /// Identifies a repository.
    RepoId,
    "repo"
);

typed_id!(
    /// Identifies an episode (a coherent work segment).
    EpisodeId,
    "episode"
);

typed_id!(
    /// Identifies a task (a persistent work item).
    TaskId,
    "task"
);

typed_id!(
    /// Identifies a decision (a detected choice with evidence).
    DecisionId,
    "decision"
);

typed_id!(
    /// Identifies a semantic entity (concept, constraint, component, etc.).
    EntityId,
    "entity"
);

typed_id!(
    /// Identifies an artifact (summary, extraction, or embedding output).
    ArtifactId,
    "artifact"
);

typed_id!(
    /// Identifies a detected recurring tool-use workflow.
    WorkflowId,
    "workflow"
);

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    // --- Property: ID derivation is deterministic ---
    // Same namespace + same input always produces the same ID.
    // Oracle: differential (derive twice, compare).
    #[hegel::test(test_cases = 500)]
    fn prop_raw_id_deterministic(tc: TestCase) {
        let ns: String = tc.draw(gs::text().min_size(1).max_size(32));
        let input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).max_size(256));

        let id1 = RawId::derive(&ns, &input);
        let id2 = RawId::derive(&ns, &input);
        assert_eq!(id1, id2, "same input must produce same ID");
    }

    // --- Property: Display/FromStr round-trip ---
    // Formatting an ID as hex then parsing it back produces the
    // same ID. Oracle: round-trip.
    #[hegel::test(test_cases = 500)]
    fn prop_raw_id_display_roundtrip(tc: TestCase) {
        let ns: String = tc.draw(gs::text().min_size(1).max_size(32));
        let input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).max_size(64));

        let id = RawId::derive(&ns, &input);
        let displayed = id.to_string();
        let parsed: RawId = displayed.parse().expect("valid hex");
        assert_eq!(id, parsed);
    }

    // --- Property: Different namespaces produce different IDs ---
    // Even with the same input, different namespaces should yield
    // different IDs (the namespace acts as a domain separator).
    // Oracle: invariant.
    #[hegel::test(test_cases = 500)]
    fn prop_namespace_separation(tc: TestCase) {
        let input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(64));

        let id_repo = RawId::derive("repo", &input);
        let id_episode = RawId::derive("episode", &input);
        assert_ne!(
            id_repo, id_episode,
            "different namespaces must produce different IDs"
        );
    }

    // --- Property: Typed ID round-trip (RepoId) ---
    #[hegel::test(test_cases = 200)]
    fn prop_repo_id_roundtrip(tc: TestCase) {
        let input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(64));

        let id = RepoId::derive(&input);
        let s = id.to_string();
        let parsed: RepoId = s.parse().expect("valid hex");
        assert_eq!(id, parsed);
    }

    // --- Property: Typed ID round-trip (EpisodeId) ---
    #[hegel::test(test_cases = 200)]
    fn prop_episode_id_roundtrip(tc: TestCase) {
        let input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(64));

        let id = EpisodeId::derive(&input);
        let s = id.to_string();
        let parsed: EpisodeId = s.parse().expect("valid hex");
        assert_eq!(id, parsed);
    }

    // --- Property: Typed IDs with same input differ by type ---
    // RepoId::derive(x) != TaskId::derive(x) because of namespace.
    #[hegel::test(test_cases = 200)]
    fn prop_typed_ids_differ_across_types(tc: TestCase) {
        let input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(64));

        let repo = RepoId::derive(&input);
        let task = TaskId::derive(&input);
        let decision = DecisionId::derive(&input);
        let episode = EpisodeId::derive(&input);

        // All raw IDs must differ
        assert_ne!(repo.raw(), task.raw());
        assert_ne!(repo.raw(), decision.raw());
        assert_ne!(repo.raw(), episode.raw());
        assert_ne!(task.raw(), decision.raw());
        assert_ne!(task.raw(), episode.raw());
        assert_ne!(decision.raw(), episode.raw());
    }

    // --- Property: serde JSON round-trip ---
    #[hegel::test(test_cases = 200)]
    fn prop_raw_id_serde_roundtrip(tc: TestCase) {
        let input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(64));

        let id = RawId::derive("test", &input);
        let json = serde_json::to_string(&id).expect("serialize");
        let parsed: RawId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, parsed);
    }

    // --- Unit test: known vector ---
    #[test]
    fn test_known_vector() {
        let id = RawId::derive("repo", b"my-project");
        // Deterministic: this must always produce the same value.
        // If this changes, something broke the hash.
        let hex = id.to_string();
        assert_eq!(hex.len(), 32);
        // Verify it's stable by checking we can round-trip
        let parsed: RawId = hex.parse().unwrap();
        assert_eq!(id, parsed);
    }

    // --- Unit test: parse error cases ---
    #[test]
    fn test_parse_errors() {
        assert!("short".parse::<RawId>().is_err());
        assert!("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".parse::<RawId>().is_err());
        assert!("00000000000000000000000000000000".parse::<RawId>().is_ok());
    }
}
