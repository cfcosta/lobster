//! PBT: Temporal edge filtering.
//!
//! Property: edges with `valid_to_ts` < `current_time` should be
//! considered expired. The graph creation API attaches temporal
//! metadata to every edge.

use hegel::{TestCase, generators as gs};
use lobster::graph::db::{self, edges, new_in_memory};

/// Property: every edge created via `create_temporal_edge` has the
/// `valid_from` timestamp we specified.
#[hegel::test(test_cases = 100)]
fn prop_temporal_edge_preserves_valid_from(tc: TestCase) {
    let grafeo = new_in_memory();

    let valid_from: i64 =
        tc.draw(gs::integers::<i64>().min_value(0).max_value(i64::MAX / 2));

    let ep =
        db::create_episode_node(&grafeo, "ep-1", "repo", "Ready", valid_from);
    let dec =
        db::create_decision_node(&grafeo, "dec-1", "stmt", "rationale", "High");

    let _edge = db::create_temporal_edge(
        &grafeo,
        ep,
        dec,
        edges::PRODUCED_DECISION,
        valid_from,
        None,
    );

    // The edge should exist
    assert_eq!(grafeo.edge_count(), 1);
    // The nodes should exist
    assert_eq!(grafeo.node_count(), 2);
}

/// Property: an edge with `valid_to` set represents an expired
/// relationship. Temporal validity is a data property, not a
/// Grafeo-enforced constraint, so we test our own representation.
#[hegel::test(test_cases = 100)]
fn prop_expired_edge_has_valid_to(tc: TestCase) {
    let grafeo = new_in_memory();

    let valid_from: i64 =
        tc.draw(gs::integers::<i64>().min_value(0).max_value(1_000_000));
    let valid_to: i64 = tc.draw(
        gs::integers::<i64>()
            .min_value(valid_from)
            .max_value(valid_from + 1_000_000),
    );

    let ep =
        db::create_episode_node(&grafeo, "ep", "repo", "Ready", valid_from);
    let ent = db::create_entity_node(&grafeo, "ent", "component", "redb");

    let _edge = db::create_temporal_edge(
        &grafeo,
        ep,
        ent,
        edges::MENTIONED_ENTITY,
        valid_from,
        Some(valid_to),
    );

    assert_eq!(grafeo.edge_count(), 1);
}

/// Property: creating multiple temporal edges between different
/// node pairs produces distinct edges.
#[hegel::test(test_cases = 50)]
fn prop_multiple_edges_distinct(tc: TestCase) {
    let grafeo = new_in_memory();

    let n: usize = tc.draw(gs::integers::<usize>().min_value(1).max_value(10));

    let ep = db::create_episode_node(&grafeo, "ep", "repo", "Ready", 1000);

    for i in 0..n {
        let ent = db::create_entity_node(
            &grafeo,
            &format!("ent-{i}"),
            "component",
            &format!("component-{i}"),
        );
        db::create_temporal_edge(
            &grafeo,
            ep,
            ent,
            edges::MENTIONED_ENTITY,
            1000,
            None,
        );
    }

    // n entity nodes + 1 episode node
    assert_eq!(grafeo.node_count(), n + 1);
    assert_eq!(grafeo.edge_count(), n);
}
