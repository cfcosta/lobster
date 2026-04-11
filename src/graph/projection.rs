//! Project redb artifacts into Grafeo nodes and edges.
//!
//! This is the bridge between the canonical store (redb) and the
//! semantic serving layer (Grafeo). All writes use the programmatic
//! CRUD API.

use grafeo::GrafeoDB;

use crate::{
    graph::db::{self, edges},
    store::schema::{Decision, Entity, Episode, Task},
};

/// Project an Episode into Grafeo.
pub fn project_episode(grafeo: &GrafeoDB, episode: &Episode) -> grafeo::NodeId {
    db::create_episode_node(
        grafeo,
        &episode.episode_id.to_string(),
        &episode.repo_id.to_string(),
        &format!("{:?}", episode.processing_state),
        episode.finalized_ts_utc_ms,
    )
}

/// Project a Decision into Grafeo, linking it to its episode.
pub fn project_decision(
    grafeo: &GrafeoDB,
    decision: &Decision,
    episode_node: grafeo::NodeId,
) -> grafeo::NodeId {
    let node = db::create_decision_node(
        grafeo,
        &decision.decision_id.to_string(),
        &decision.statement,
        &decision.rationale,
        &format!("{:?}", decision.confidence),
    );

    db::create_temporal_edge(
        grafeo,
        episode_node,
        node,
        edges::PRODUCED_DECISION,
        decision.valid_from_ts_utc_ms,
        decision.valid_to_ts_utc_ms,
    );

    node
}

/// Project a Task into Grafeo, linking it to its episode.
pub fn project_task(
    grafeo: &GrafeoDB,
    task: &Task,
    episode_node: grafeo::NodeId,
) -> grafeo::NodeId {
    let node = db::create_task_node(
        grafeo,
        &task.task_id.to_string(),
        &task.title,
        &format!("{:?}", task.status),
    );

    db::create_temporal_edge(
        grafeo,
        episode_node,
        node,
        edges::PRODUCED_TASK,
        0, // task creation time not tracked separately
        None,
    );

    node
}

/// Project an Entity into Grafeo.
pub fn project_entity(
    grafeo: &GrafeoDB,
    entity: &Entity,
    episode_node: grafeo::NodeId,
) -> grafeo::NodeId {
    let node = db::create_entity_node(
        grafeo,
        &entity.entity_id.to_string(),
        &format!("{:?}", entity.kind),
        &entity.canonical_name,
    );

    db::create_temporal_edge(
        grafeo,
        episode_node,
        node,
        edges::MENTIONED_ENTITY,
        0,
        None,
    );

    node
}

/// Link a decision to an entity in the graph.
pub fn link_decision_entity(
    grafeo: &GrafeoDB,
    decision_node: grafeo::NodeId,
    entity_node: grafeo::NodeId,
    valid_from_ms: i64,
) {
    db::create_temporal_edge(
        grafeo,
        decision_node,
        entity_node,
        edges::DECISION_ENTITY,
        valid_from_ms,
        None,
    );
}

/// Link a task to a decision in the graph.
pub fn link_task_decision(
    grafeo: &GrafeoDB,
    task_node: grafeo::NodeId,
    decision_node: grafeo::NodeId,
    valid_from_ms: i64,
) {
    db::create_temporal_edge(
        grafeo,
        task_node,
        decision_node,
        edges::TASK_DECISION,
        valid_from_ms,
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        ids::{DecisionId, EntityId, EpisodeId, RepoId, TaskId},
        schema::{
            Confidence,
            EntityKind,
            EvidenceRef,
            ProcessingState,
            TaskStatus,
        },
    };

    fn test_episode() -> Episode {
        Episode {
            episode_id: EpisodeId::derive(b"ep-1"),
            repo_id: RepoId::derive(b"repo"),
            start_seq: 0,
            end_seq: 10,
            task_id: None,
            processing_state: ProcessingState::Ready,
            finalized_ts_utc_ms: 1_700_000_000_000,
        }
    }

    fn test_decision() -> Decision {
        Decision {
            decision_id: DecisionId::derive(b"dec-1"),
            repo_id: RepoId::derive(b"repo"),
            episode_id: EpisodeId::derive(b"ep-1"),
            task_id: None,
            statement: "Use redb for storage".into(),
            rationale: "Embedded, ACID, Rust-native".into(),
            confidence: Confidence::High,
            valid_from_ts_utc_ms: 1_700_000_000_000,
            valid_to_ts_utc_ms: None,
            evidence: vec![EvidenceRef {
                episode_id: EpisodeId::derive(b"ep-1"),
                span_summary: "discussed options".into(),
            }],
        }
    }

    #[test]
    fn test_project_episode() {
        let grafeo = db::new_in_memory();
        let ep = test_episode();
        let node = project_episode(&grafeo, &ep);
        assert_eq!(grafeo.node_count(), 1);
        let n = grafeo.get_node(node).unwrap();
        assert_eq!(
            n.get_property("processing_state")
                .unwrap()
                .as_str()
                .unwrap(),
            "Ready"
        );
    }

    #[test]
    fn test_project_decision_with_edge() {
        let grafeo = db::new_in_memory();
        let ep = test_episode();
        let dec = test_decision();

        let ep_node = project_episode(&grafeo, &ep);
        let _dec_node = project_decision(&grafeo, &dec, ep_node);

        assert_eq!(grafeo.node_count(), 2);
        assert_eq!(grafeo.edge_count(), 1);
    }

    #[test]
    fn test_full_projection() {
        let grafeo = db::new_in_memory();
        let ep = test_episode();
        let dec = test_decision();
        let task = Task {
            task_id: TaskId::derive(b"task-1"),
            repo_id: RepoId::derive(b"repo"),
            title: "Build memory".into(),
            status: TaskStatus::Open,
            opened_in: EpisodeId::derive(b"ep-1"),
            last_seen_in: EpisodeId::derive(b"ep-1"),
        };
        let entity = Entity {
            entity_id: EntityId::derive(b"ent-1"),
            repo_id: RepoId::derive(b"repo"),
            kind: EntityKind::Component,
            canonical_name: "redb".into(),
        };

        let ep_node = project_episode(&grafeo, &ep);
        let dec_node = project_decision(&grafeo, &dec, ep_node);
        let task_node = project_task(&grafeo, &task, ep_node);
        let ent_node = project_entity(&grafeo, &entity, ep_node);

        // Link semantic edges
        link_task_decision(&grafeo, task_node, dec_node, 1_700_000_000_000);
        link_decision_entity(&grafeo, dec_node, ent_node, 1_700_000_000_000);

        // 4 nodes: episode, decision, task, entity
        assert_eq!(grafeo.node_count(), 4);
        // 5 edges: 3 provenance + 2 semantic
        assert_eq!(grafeo.edge_count(), 5);
    }
}
