//! Convention aggregation for repo identity profiles.
//!
//! Reads LLM-extracted conventions from episode extraction artifacts
//! and aggregates them into a stable `RepoProfile`.

use crate::store::{
    db::LobsterDb,
    ids::{EpisodeId, RepoId},
    schema::{Confidence, EvidenceRef, ProfileFact, RepoProfile},
};

/// Minimum number of episodes supporting a fact before it's promoted.
const MIN_SUPPORT: u32 = 2;

/// Aggregate conventions across episodes into profile facts.
///
/// Groups convention strings by statement, counts episode support,
/// and promotes those appearing in at least `MIN_SUPPORT` episodes.
#[must_use]
pub(crate) fn aggregate_conventions(
    episode_conventions: &[(EpisodeId, Vec<String>)],
    now_ms: i64,
) -> Vec<ProfileFact> {
    use std::collections::HashMap;

    let mut counts: HashMap<String, Vec<EpisodeId>> = HashMap::new();

    for (ep_id, conventions) in episode_conventions {
        let mut seen = std::collections::HashSet::new();
        for convention in conventions {
            if seen.insert(convention) {
                counts.entry(convention.clone()).or_default().push(*ep_id);
            }
        }
    }

    let mut facts: Vec<ProfileFact> = counts
        .into_iter()
        .filter(|(_, eps)| eps.len() >= MIN_SUPPORT as usize)
        .map(|(statement, eps)| {
            let evidence: Vec<EvidenceRef> = eps
                .iter()
                .take(3)
                .map(|ep_id| EvidenceRef {
                    episode_id: *ep_id,
                    span_summary: format!("detected convention: {statement}"),
                })
                .collect();
            #[allow(clippy::cast_possible_truncation)]
            ProfileFact {
                statement,
                evidence,
                first_seen_ts_utc_ms: now_ms,
                last_confirmed_ts_utc_ms: now_ms,
                support_count: eps.len() as u32,
                confidence: match eps.len() {
                    0..=2 => Confidence::Low,
                    3..=5 => Confidence::Medium,
                    _ => Confidence::High,
                },
            }
        })
        .collect();

    facts.sort_by(|a, b| {
        b.support_count
            .cmp(&a.support_count)
            .then_with(|| a.statement.cmp(&b.statement))
    });

    facts
}

/// Build or update a repo profile from LLM-extracted conventions
/// stored in extraction artifacts.
#[must_use]
pub fn build_profile(db: &LobsterDb, repo_id: &RepoId) -> RepoProfile {
    let episode_conventions = read_conventions_from_artifacts(db, repo_id);

    let now_ms = chrono::Utc::now().timestamp_millis();
    let new_conventions = aggregate_conventions(&episode_conventions, now_ms);

    let existing =
        crate::store::crud::get_repo_profile(db, &repo_id.raw()).ok();

    let conventions = merge_facts(
        existing.as_ref().map_or(&[][..], |p| &p.conventions),
        &new_conventions,
    );

    let now_ms = chrono::Utc::now().timestamp_millis();

    RepoProfile {
        repo_id: *repo_id,
        conventions,
        preferences: existing.map_or_else(Vec::new, |p| p.preferences),
        updated_ts_utc_ms: now_ms,
        revision: "v2".into(),
    }
}

/// Read conventions from extraction artifacts for episodes
/// belonging to the given repo.
fn read_conventions_from_artifacts(
    db: &LobsterDb,
    repo_id: &RepoId,
) -> Vec<(EpisodeId, Vec<String>)> {
    use crate::extract::traits::ExtractionOutput;

    let Ok(rtxn) = db.env.read_txn() else {
        return vec![];
    };

    // Collect episodes for this repo
    let Ok(ep_iter) = db.episodes.iter(&rtxn) else {
        return vec![];
    };

    let mut result = Vec::new();
    for entry in ep_iter.flatten() {
        let (_, bytes) = entry;
        let Ok(ep) =
            serde_json::from_slice::<crate::store::schema::Episode>(bytes)
        else {
            continue;
        };

        if ep.repo_id != *repo_id {
            continue;
        }

        // Read extraction artifact for this episode
        let Ok(artifact) = crate::store::crud::get_extraction_artifact(
            db,
            &ep.episode_id.raw(),
        ) else {
            continue;
        };

        let Ok(output) =
            serde_json::from_slice::<ExtractionOutput>(&artifact.output_json)
        else {
            continue;
        };

        if !output.conventions.is_empty() {
            result.push((ep.episode_id, output.conventions));
        }
    }

    result
}

/// Merge new facts with existing ones, preserving `first_seen` timestamps.
fn merge_facts(
    existing: &[ProfileFact],
    new_facts: &[ProfileFact],
) -> Vec<ProfileFact> {
    let mut merged: Vec<ProfileFact> = Vec::new();

    for new in new_facts {
        if let Some(old) =
            existing.iter().find(|f| f.statement == new.statement)
        {
            merged.push(ProfileFact {
                first_seen_ts_utc_ms: old.first_seen_ts_utc_ms,
                last_confirmed_ts_utc_ms: new.last_confirmed_ts_utc_ms,
                support_count: new.support_count,
                confidence: new.confidence,
                statement: new.statement.clone(),
                evidence: new.evidence.clone(),
            });
        } else {
            merged.push(new.clone());
        }
    }

    merged.truncate(crate::store::schema::MAX_PROFILE_FACTS);
    merged
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;
    use crate::store::ids::EpisodeId;

    #[test]
    fn test_aggregate_empty() {
        let facts = aggregate_conventions(&[], 1000);
        assert!(facts.is_empty());
    }

    #[test]
    fn test_aggregate_below_threshold() {
        let ep1 = EpisodeId::derive(b"ep1");
        let conventions =
            vec![(ep1, vec!["uses nix flakes for builds".into()])];
        let facts = aggregate_conventions(&conventions, 1000);
        assert!(facts.is_empty(), "single episode should not promote");
    }

    #[test]
    fn test_aggregate_above_threshold() {
        let ep1 = EpisodeId::derive(b"ep1");
        let ep2 = EpisodeId::derive(b"ep2");
        let conventions = vec![
            (ep1, vec!["uses nix".into()]),
            (ep2, vec!["uses nix".into()]),
        ];
        let facts = aggregate_conventions(&conventions, 1000);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].support_count, 2);
    }

    #[test]
    fn test_aggregate_deduplicates_within_episode() {
        let ep1 = EpisodeId::derive(b"ep1");
        let ep2 = EpisodeId::derive(b"ep2");
        let conventions = vec![
            (ep1, vec!["uses nix".into(), "uses nix".into()]),
            (ep2, vec!["uses nix".into()]),
        ];
        let facts = aggregate_conventions(&conventions, 1000);
        assert_eq!(facts[0].support_count, 2, "should count 2 episodes, not 3");
    }

    #[hegel::test(test_cases = 50)]
    fn prop_aggregate_deterministic(tc: TestCase) {
        let n_episodes: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
        let statements = vec![
            "uses nix",
            "Rust project using Cargo",
            "uses jujutsu (jj) for version control",
        ];

        let mut episode_conventions = Vec::new();
        for i in 0..n_episodes {
            #[allow(clippy::cast_possible_truncation)]
            let ep = EpisodeId::derive(&(i as u32).to_le_bytes());
            let n_convs: usize =
                tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
            let mut convs = Vec::new();
            for _ in 0..n_convs {
                let stmt = tc.draw(gs::sampled_from(statements.clone()));
                convs.push(stmt.to_string());
            }
            episode_conventions.push((ep, convs));
        }

        let result1 = aggregate_conventions(&episode_conventions, 1000);
        let result2 = aggregate_conventions(&episode_conventions, 1000);
        assert_eq!(result1, result2, "aggregation must be deterministic");
    }

    #[hegel::test(test_cases = 50)]
    fn prop_support_count_bounded(tc: TestCase) {
        let n_episodes: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(8));

        let mut episode_conventions = Vec::new();
        for i in 0..n_episodes {
            #[allow(clippy::cast_possible_truncation)]
            let ep = EpisodeId::derive(&(i as u32).to_le_bytes());
            episode_conventions.push((ep, vec!["uses nix".to_string()]));
        }

        let facts = aggregate_conventions(&episode_conventions, 1000);
        for fact in &facts {
            assert!(
                fact.support_count as usize <= n_episodes,
                "support {} exceeds episodes {}",
                fact.support_count,
                n_episodes
            );
        }
    }

    #[hegel::test(test_cases = 50)]
    fn prop_promoted_facts_have_evidence(tc: TestCase) {
        let n_episodes: usize =
            tc.draw(gs::integers::<usize>().min_value(2).max_value(6));

        let mut episode_conventions = Vec::new();
        for i in 0..n_episodes {
            #[allow(clippy::cast_possible_truncation)]
            let ep = EpisodeId::derive(&(i as u32).to_le_bytes());
            episode_conventions.push((ep, vec!["test convention".to_string()]));
        }

        let facts = aggregate_conventions(&episode_conventions, 1000);
        for fact in &facts {
            assert!(
                !fact.evidence.is_empty(),
                "promoted facts must have evidence"
            );
        }
    }

    #[test]
    fn test_merge_preserves_first_seen() {
        let old = vec![ProfileFact {
            statement: "uses nix".into(),
            evidence: vec![],
            first_seen_ts_utc_ms: 1000,
            last_confirmed_ts_utc_ms: 1000,
            support_count: 2,
            confidence: Confidence::Low,
        }];
        let new_facts = vec![ProfileFact {
            statement: "uses nix".into(),
            evidence: vec![],
            first_seen_ts_utc_ms: 5000,
            last_confirmed_ts_utc_ms: 5000,
            support_count: 4,
            confidence: Confidence::Medium,
        }];

        let merged = merge_facts(&old, &new_facts);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].first_seen_ts_utc_ms, 1000,
            "should preserve original first_seen"
        );
        assert_eq!(merged[0].support_count, 4);
    }

    #[test]
    fn test_merge_adds_new_facts() {
        let old = vec![ProfileFact {
            statement: "uses nix".into(),
            evidence: vec![],
            first_seen_ts_utc_ms: 1000,
            last_confirmed_ts_utc_ms: 1000,
            support_count: 2,
            confidence: Confidence::Low,
        }];
        let new_facts = vec![
            ProfileFact {
                statement: "uses nix".into(),
                evidence: vec![],
                first_seen_ts_utc_ms: 5000,
                last_confirmed_ts_utc_ms: 5000,
                support_count: 4,
                confidence: Confidence::Medium,
            },
            ProfileFact {
                statement: "Rust project".into(),
                evidence: vec![],
                first_seen_ts_utc_ms: 5000,
                last_confirmed_ts_utc_ms: 5000,
                support_count: 3,
                confidence: Confidence::Medium,
            },
        ];

        let merged = merge_facts(&old, &new_facts);
        assert_eq!(merged.len(), 2);
    }

    #[hegel::test(test_cases = 30)]
    fn prop_merge_bounded(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(30));
        let mut new_facts = Vec::new();
        for i in 0..n {
            new_facts.push(ProfileFact {
                statement: format!("fact-{i}"),
                evidence: vec![],
                first_seen_ts_utc_ms: 1000,
                last_confirmed_ts_utc_ms: 1000,
                support_count: 2,
                confidence: Confidence::Low,
            });
        }

        let merged = merge_facts(&[], &new_facts);
        assert!(
            merged.len() <= crate::store::schema::MAX_PROFILE_FACTS,
            "got {} facts, max is {}",
            merged.len(),
            crate::store::schema::MAX_PROFILE_FACTS,
        );
    }
}
