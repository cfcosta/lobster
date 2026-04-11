//! Grafeo database initialization and schema setup.
//!
//! Grafeo is the semantic serving layer — a materialized index
//! over redb's canonical data. If lost, it can be rebuilt.

use grafeo::GrafeoDB;

/// Node labels used in the Lobster graph schema.
pub mod labels {
    pub const EPISODE: &str = "Episode";
    pub const TASK: &str = "Task";
    pub const DECISION: &str = "Decision";
    pub const ENTITY: &str = "Entity";
}

/// Edge types used in the Lobster graph schema.
pub mod edges {
    // Provenance edges (episode → derived records)
    pub const PRODUCED_TASK: &str = "PRODUCED_TASK";
    pub const PRODUCED_DECISION: &str = "PRODUCED_DECISION";
    pub const MENTIONED_ENTITY: &str = "MENTIONED_ENTITY";

    // Semantic work edges
    pub const TASK_DECISION: &str = "TASK_DECISION";
    pub const TASK_ENTITY: &str = "TASK_ENTITY";
    pub const DECISION_ENTITY: &str = "DECISION_ENTITY";
    pub const ENTITY_ENTITY: &str = "ENTITY_ENTITY";
}

/// Entity kind values.
pub mod entity_kinds {
    pub const CONCEPT: &str = "concept";
    pub const CONSTRAINT: &str = "constraint";
    pub const COMPONENT: &str = "component";
    pub const FILE_LITE: &str = "file-lite";
    pub const REPO: &str = "repo";
}

/// Create an in-memory Grafeo database for testing.
#[must_use]
pub fn new_in_memory() -> GrafeoDB {
    GrafeoDB::new_in_memory()
}

/// Create a graph node for an episode.
pub fn create_episode_node(
    db: &GrafeoDB,
    episode_id: &str,
    repo_id: &str,
    processing_state: &str,
    finalized_ts_ms: i64,
) -> grafeo::NodeId {
    let node = db.create_node(&[labels::EPISODE]);
    db.set_node_property(node, "episode_id", grafeo::Value::from(episode_id));
    db.set_node_property(node, "repo_id", grafeo::Value::from(repo_id));
    db.set_node_property(
        node,
        "processing_state",
        grafeo::Value::from(processing_state),
    );
    db.set_node_property(
        node,
        "finalized_ts_ms",
        grafeo::Value::from(finalized_ts_ms),
    );
    node
}

/// Create a graph node for a decision.
pub fn create_decision_node(
    db: &GrafeoDB,
    decision_id: &str,
    statement: &str,
    rationale: &str,
    confidence: &str,
) -> grafeo::NodeId {
    let node = db.create_node(&[labels::DECISION]);
    db.set_node_property(node, "decision_id", grafeo::Value::from(decision_id));
    db.set_node_property(node, "statement", grafeo::Value::from(statement));
    db.set_node_property(node, "rationale", grafeo::Value::from(rationale));
    db.set_node_property(node, "confidence", grafeo::Value::from(confidence));
    node
}

/// Create a graph node for an entity.
pub fn create_entity_node(
    db: &GrafeoDB,
    entity_id: &str,
    kind: &str,
    canonical_name: &str,
) -> grafeo::NodeId {
    let node = db.create_node(&[labels::ENTITY]);
    db.set_node_property(node, "entity_id", grafeo::Value::from(entity_id));
    db.set_node_property(node, "kind", grafeo::Value::from(kind));
    db.set_node_property(
        node,
        "canonical_name",
        grafeo::Value::from(canonical_name),
    );
    node
}

/// Create a graph node for a task.
pub fn create_task_node(
    db: &GrafeoDB,
    task_id: &str,
    title: &str,
    status: &str,
) -> grafeo::NodeId {
    let node = db.create_node(&[labels::TASK]);
    db.set_node_property(node, "task_id", grafeo::Value::from(task_id));
    db.set_node_property(node, "title", grafeo::Value::from(title));
    db.set_node_property(node, "status", grafeo::Value::from(status));
    node
}

/// Create an edge between two nodes with temporal validity.
pub fn create_temporal_edge(
    db: &GrafeoDB,
    from: grafeo::NodeId,
    to: grafeo::NodeId,
    edge_type: &str,
    valid_from_ms: i64,
    valid_to_ms: Option<i64>,
) -> grafeo::EdgeId {
    let edge = db.create_edge(from, to, edge_type);
    db.set_edge_property(
        edge,
        "valid_from_ts_utc_ms",
        grafeo::Value::from(valid_from_ms),
    );
    if let Some(to_ms) = valid_to_ms {
        db.set_edge_property(
            edge,
            "valid_to_ts_utc_ms",
            grafeo::Value::from(to_ms),
        );
    }
    edge
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_in_memory() {
        let db = new_in_memory();
        assert_eq!(db.node_count(), 0);
        assert_eq!(db.edge_count(), 0);
    }

    #[test]
    fn test_create_episode_node() {
        let db = new_in_memory();
        let node = create_episode_node(
            &db,
            "ep-abc",
            "repo-xyz",
            "Pending",
            1_700_000_000_000,
        );
        let n = db.get_node(node).unwrap();
        assert_eq!(
            n.get_property("episode_id").unwrap().as_str().unwrap(),
            "ep-abc"
        );
        assert_eq!(db.node_count(), 1);
    }

    #[test]
    fn test_create_decision_node() {
        let db = new_in_memory();
        let node = create_decision_node(
            &db,
            "dec-123",
            "Use redb",
            "ACID, embedded",
            "High",
        );
        let n = db.get_node(node).unwrap();
        assert_eq!(
            n.get_property("statement").unwrap().as_str().unwrap(),
            "Use redb"
        );
    }

    #[test]
    fn test_create_entity_node() {
        let db = new_in_memory();
        let node = create_entity_node(
            &db,
            "ent-456",
            entity_kinds::COMPONENT,
            "Grafeo",
        );
        let n = db.get_node(node).unwrap();
        assert_eq!(
            n.get_property("kind").unwrap().as_str().unwrap(),
            "component"
        );
    }

    #[test]
    fn test_create_temporal_edge() {
        let db = new_in_memory();
        let ep = create_episode_node(&db, "ep-1", "repo", "Ready", 1000);
        let dec =
            create_decision_node(&db, "dec-1", "stmt", "rationale", "High");

        let edge = create_temporal_edge(
            &db,
            ep,
            dec,
            edges::PRODUCED_DECISION,
            1000,
            None,
        );

        // Edge exists
        assert_eq!(db.edge_count(), 1);
        // Edge ID was returned (compiler enforces this, but
        // we keep the binding to prove it's used)
        let _ = edge;
    }

    #[test]
    fn test_create_full_graph_structure() {
        let db = new_in_memory();

        // Create nodes
        let ep = create_episode_node(&db, "ep-1", "repo", "Ready", 1000);
        let task = create_task_node(&db, "task-1", "Build memory", "Open");
        let dec =
            create_decision_node(&db, "dec-1", "Use redb", "ACID", "High");
        let entity = create_entity_node(&db, "ent-1", "component", "redb");

        // Create provenance edges
        create_temporal_edge(&db, ep, task, edges::PRODUCED_TASK, 1000, None);
        create_temporal_edge(
            &db,
            ep,
            dec,
            edges::PRODUCED_DECISION,
            1000,
            None,
        );

        // Create semantic edges
        create_temporal_edge(&db, task, dec, edges::TASK_DECISION, 1000, None);
        create_temporal_edge(
            &db,
            dec,
            entity,
            edges::DECISION_ENTITY,
            1000,
            None,
        );

        assert_eq!(db.node_count(), 4);
        assert_eq!(db.edge_count(), 4);
    }
}
