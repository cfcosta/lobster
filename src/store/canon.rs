//! Deterministic canonicalization for identity resolution.
//!
//! Repos, tasks, decisions, and file references use **strict**
//! canonicalization: the same logical entity always maps to the
//! same ID regardless of surface-level differences (case, trailing
//! slashes, whitespace). General entities use **conservative**
//! canonicalization: they may be inserted as-is and merged later
//! during dreaming.

use crate::store::ids::{DecisionId, EntityId, RepoId, TaskId};

/// Normalize a string for strict canonicalization:
/// trim whitespace, collapse internal whitespace to single spaces,
/// lowercase.
#[must_use]
pub fn normalize(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Normalize a file path: trim, normalize separators to `/`,
/// collapse repeated slashes, strip trailing slash.
#[must_use]
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    let normalized: String = trimmed.replace('\\', "/");
    let collapsed = collapse_slashes(&normalized);
    collapsed
        .strip_suffix('/')
        .unwrap_or(&collapsed)
        .to_string()
}

fn collapse_slashes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_slash = false;
    for c in s.chars() {
        if c == '/' {
            if !prev_slash {
                result.push(c);
            }
            prev_slash = true;
        } else {
            result.push(c);
            prev_slash = false;
        }
    }
    result
}

/// Derive a deterministic `RepoId` from a repo path or name.
#[must_use]
pub fn repo_id(repo: &str) -> RepoId {
    let canonical = normalize_path(repo);
    RepoId::derive(canonical.as_bytes())
}

/// Derive a deterministic `TaskId` from a repo context and task
/// title.
#[must_use]
pub fn task_id(repo: &str, title: &str) -> TaskId {
    let canonical_repo = normalize_path(repo);
    let canonical_title = normalize(title);
    let input = format!("{canonical_repo}:{canonical_title}");
    TaskId::derive(input.as_bytes())
}

/// Derive a deterministic `DecisionId` from a repo context and
/// decision statement.
#[must_use]
pub fn decision_id(repo: &str, statement: &str) -> DecisionId {
    let canonical_repo = normalize_path(repo);
    let canonical_stmt = normalize(statement);
    let input = format!("{canonical_repo}:{canonical_stmt}");
    DecisionId::derive(input.as_bytes())
}

/// Derive an `EntityId` conservatively: normalize the name but
/// do not merge across different entity kinds. Merging happens
/// later during dreaming when evidence supports it.
#[must_use]
pub fn entity_id(repo: &str, kind: &str, name: &str) -> EntityId {
    let canonical_repo = normalize_path(repo);
    let canonical_kind = normalize(kind);
    let canonical_name = normalize(name);
    let input = format!("{canonical_repo}:{canonical_kind}:{canonical_name}");
    EntityId::derive(input.as_bytes())
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    // -- Property: normalize is idempotent --
    // Algebraic law: normalize(normalize(x)) == normalize(x)
    #[hegel::test(test_cases = 500)]
    fn prop_normalize_idempotent(tc: TestCase) {
        let input: String = tc.draw(gs::text().max_size(200));
        let once = normalize(&input);
        let twice = normalize(&once);
        assert_eq!(once, twice, "normalize must be idempotent");
    }

    // -- Property: normalize_path is idempotent --
    #[hegel::test(test_cases = 500)]
    fn prop_normalize_path_idempotent(tc: TestCase) {
        let input: String = tc.draw(gs::text().max_size(200));
        let once = normalize_path(&input);
        let twice = normalize_path(&once);
        assert_eq!(once, twice, "normalize_path must be idempotent");
    }

    // -- Property: repo_id is deterministic --
    // Same repo string → same RepoId, always.
    #[hegel::test(test_cases = 500)]
    fn prop_repo_id_deterministic(tc: TestCase) {
        let repo: String = tc.draw(gs::text().min_size(1).max_size(100));
        let id1 = repo_id(&repo);
        let id2 = repo_id(&repo);
        assert_eq!(id1, id2);
    }

    // -- Property: repo paths are case-sensitive (Linux) --
    // but whitespace/slash normalization still applies.
    #[hegel::test(test_cases = 200)]
    fn prop_repo_id_path_normalized(tc: TestCase) {
        let base: String = tc.draw(gs::text().min_size(1).max_size(50));
        let clean = normalize_path(&base);
        // Adding trailing slash should not change identity
        let with_slash = format!("{clean}/");
        assert_eq!(repo_id(&clean), repo_id(&with_slash),);
    }

    // -- Property: canonicalization ignores trailing slashes --
    #[hegel::test(test_cases = 200)]
    fn prop_repo_id_trailing_slash(tc: TestCase) {
        let repo: String = tc.draw(gs::text().min_size(1).max_size(50));
        let clean = normalize_path(&repo);
        let with_slash = format!("{clean}/");
        assert_eq!(
            repo_id(&clean),
            repo_id(&with_slash),
            "trailing slash should not affect repo identity"
        );
    }

    // -- Property: task_id is deterministic --
    #[hegel::test(test_cases = 300)]
    fn prop_task_id_deterministic(tc: TestCase) {
        let repo: String = tc.draw(gs::text().min_size(1).max_size(50));
        let title: String = tc.draw(gs::text().min_size(1).max_size(100));
        let id1 = task_id(&repo, &title);
        let id2 = task_id(&repo, &title);
        assert_eq!(id1, id2);
    }

    // -- Property: task_id normalizes whitespace --
    #[hegel::test(test_cases = 200)]
    fn prop_task_id_whitespace_invariant(tc: TestCase) {
        let repo: String = tc.draw(gs::text().min_size(1).max_size(50));
        let word1: String = tc.draw(gs::text().min_size(1).max_size(20));
        let word2: String = tc.draw(gs::text().min_size(1).max_size(20));

        let spaced = format!("  {word1}   {word2}  ");
        let tight = format!("{word1} {word2}");
        assert_eq!(
            task_id(&repo, &spaced),
            task_id(&repo, &tight),
            "whitespace normalization must not affect task identity"
        );
    }

    // -- Property: decision_id deterministic --
    #[hegel::test(test_cases = 200)]
    fn prop_decision_id_deterministic(tc: TestCase) {
        let repo: String = tc.draw(gs::text().min_size(1).max_size(50));
        let stmt: String = tc.draw(gs::text().min_size(1).max_size(200));
        let id1 = decision_id(&repo, &stmt);
        let id2 = decision_id(&repo, &stmt);
        assert_eq!(id1, id2);
    }

    // -- Property: entity_id deterministic --
    #[hegel::test(test_cases = 200)]
    fn prop_entity_id_deterministic(tc: TestCase) {
        let repo: String = tc.draw(gs::text().min_size(1).max_size(50));
        let kind: String = tc.draw(gs::text().min_size(1).max_size(20));
        let name: String = tc.draw(gs::text().min_size(1).max_size(50));
        let id1 = entity_id(&repo, &kind, &name);
        let id2 = entity_id(&repo, &kind, &name);
        assert_eq!(id1, id2);
    }

    // -- Unit tests --

    #[test]
    fn test_normalize_examples() {
        assert_eq!(normalize("  Hello  World  "), "hello world");
        assert_eq!(normalize("UPPER"), "upper");
        assert_eq!(normalize("  "), "");
    }

    #[test]
    fn test_normalize_path_examples() {
        assert_eq!(normalize_path("/home/user/repo/"), "/home/user/repo");
        assert_eq!(normalize_path("path\\to\\file"), "path/to/file");
        assert_eq!(normalize_path("a///b//c/"), "a/b/c");
    }

    #[test]
    fn test_different_repos_different_ids() {
        let id1 = repo_id("/home/alice/project-a");
        let id2 = repo_id("/home/alice/project-b");
        assert_ne!(id1, id2);
    }
}
