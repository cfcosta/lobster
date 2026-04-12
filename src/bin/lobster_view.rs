//! lobster-view: dump all data from redb and Grafeo for inspection.
//!
//! Opens the database in read-only mode so it works even while the
//! MCP server holds the write lock. Embedding vectors are not
//! printed but their presence is indicated.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use lobster::store::{schema, tables};
use redb::{ReadableDatabase, ReadableTable};

/// Extract a string property from a Grafeo node, stripping
/// surrounding quotes that `Value::to_string()` adds.
macro_rules! prop_str {
    ($node:expr, $key:expr) => {
        $node
            .get_property($key)
            .map_or_else(String::new, |v| strip_quotes(&v.to_string()))
    };
}

fn strip_quotes(s: &str) -> String {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s)
        .to_string()
}

/// Dump all Lobster data for a repository.
#[derive(Parser)]
#[command(name = "lobster-view", version, about)]
struct Cli {
    /// Path to the repository (defaults to current directory).
    #[arg(long)]
    repo: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let repo_dir = cli
        .repo
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let storage_dir = lobster::app::config::resolve_storage_path(&repo_dir);
    let db_path = lobster::app::config::db_path(&storage_dir);

    if !db_path.exists() {
        println!("No Lobster database found at {}", db_path.display());
        println!("Run `lobster init` first.");
        return Ok(());
    }

    // Try read-only first; if the MCP server holds an exclusive lock,
    // fall back to copying the file to a temp location and repairing it.
    let tmp_path = std::env::temp_dir().join("lobster-view-snapshot.redb");
    let ro = redb::ReadOnlyDatabase::open(&db_path);
    let rw;
    let db: &dyn ReadableDatabase = if let Ok(db) = &ro {
        db
    } else {
        eprintln!(
            "Database locked (MCP server running?), reading from snapshot copy..."
        );
        std::fs::copy(&db_path, &tmp_path).context("copy database")?;
        rw = redb::Builder::new()
            .set_repair_callback(|_| {})
            .open(&tmp_path)
            .context("open database copy")?;
        &rw
    };

    dump_redb(db)?;
    let grafeo = rebuild_grafeo(db)?;
    dump_grafeo(&grafeo);

    Ok(())
}

// ── redb dump ───────────────────────────────────────────────────

fn dump_redb(db: &dyn ReadableDatabase) -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("  redb canonical store");
    println!("═══════════════════════════════════════════════════════");

    dump_metadata(db)?;
    dump_raw_events(db)?;
    dump_episodes(db)?;
    dump_decisions(db)?;
    dump_tasks(db)?;
    dump_entities(db)?;
    dump_summary_artifacts(db)?;
    dump_extraction_artifacts(db)?;
    dump_embedding_artifacts(db)?;
    dump_processing_jobs(db)?;
    dump_projection_metadata(db)?;
    dump_repo_config(db)?;
    dump_retrieval_stats(db)?;

    Ok(())
}

fn dump_metadata(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn.open_table(tables::METADATA).context("open metadata")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Metadata ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (k, v) = entry.context("read entry")?;
        let val: serde_json::Value = serde_json::from_slice(v.value())
            .unwrap_or_else(|_| {
                serde_json::Value::String(
                    String::from_utf8_lossy(v.value()).into(),
                )
            });
        println!("  {}: {}", k.value(), format_json(&val));
    }
    Ok(())
}

fn dump_raw_events(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn
        .open_table(tables::RAW_EVENTS)
        .context("open raw_events")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Raw Events ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (k, v) = entry.context("read entry")?;
        let event: schema::RawEvent =
            serde_json::from_slice(v.value()).context("parse event")?;
        println!(
            "  seq={} kind={:?} ts={} payload={}B hash={}",
            k.value(),
            event.event_kind,
            format_ts(event.ts_utc_ms),
            event.payload_bytes.len(),
            hex_short(&event.payload_hash),
        );
    }
    Ok(())
}

fn dump_episodes(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn.open_table(tables::EPISODES).context("open episodes")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Episodes ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (_, v) = entry.context("read entry")?;
        let ep: schema::Episode =
            serde_json::from_slice(v.value()).context("parse episode")?;
        println!(
            "  {} state={:?} seqs={}..{} ts={} retries={} noisy={}",
            ep.episode_id,
            ep.processing_state,
            ep.start_seq,
            ep.end_seq,
            format_ts(ep.finalized_ts_utc_ms),
            ep.retry_count,
            ep.is_noisy,
        );
        if let Some(tid) = &ep.task_id {
            println!("    task: {tid}");
        }
    }
    Ok(())
}

fn dump_decisions(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn
        .open_table(tables::DECISIONS)
        .context("open decisions")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Decisions ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (_, v) = entry.context("read entry")?;
        let dec: schema::Decision =
            serde_json::from_slice(v.value()).context("parse decision")?;
        println!("  {} confidence={:?}", dec.decision_id, dec.confidence);
        println!("    statement: {}", dec.statement);
        println!("    rationale: {}", dec.rationale);
        println!(
            "    valid: {} -> {}",
            format_ts(dec.valid_from_ts_utc_ms),
            dec.valid_to_ts_utc_ms
                .map_or_else(|| "current".to_string(), format_ts),
        );
        println!("    episode: {}", dec.episode_id);
        if let Some(tid) = &dec.task_id {
            println!("    task: {tid}");
        }
        for ev in &dec.evidence {
            println!("    evidence: {} \"{}\"", ev.episode_id, ev.span_summary);
        }
    }
    Ok(())
}

fn dump_tasks(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn.open_table(tables::TASKS).context("open tasks")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Tasks ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (_, v) = entry.context("read entry")?;
        let task: schema::Task =
            serde_json::from_slice(v.value()).context("parse task")?;
        println!(
            "  {} status={:?} \"{}\"",
            task.task_id, task.status, task.title,
        );
        println!(
            "    opened_in={} last_seen_in={}",
            task.opened_in, task.last_seen_in,
        );
    }
    Ok(())
}

fn dump_entities(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn.open_table(tables::ENTITIES).context("open entities")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Entities ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (_, v) = entry.context("read entry")?;
        let ent: schema::Entity =
            serde_json::from_slice(v.value()).context("parse entity")?;
        println!(
            "  {} kind={:?} \"{}\"",
            ent.entity_id, ent.kind, ent.canonical_name,
        );
    }
    Ok(())
}

fn dump_summary_artifacts(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn
        .open_table(tables::SUMMARY_ARTIFACTS)
        .context("open summary_artifacts")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Summary Artifacts ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (_, v) = entry.context("read entry")?;
        let art: schema::SummaryArtifact =
            serde_json::from_slice(v.value()).context("parse summary")?;
        println!(
            "  episode={} rev={} checksum={}",
            art.episode_id,
            art.revision,
            hex_short(&art.payload_checksum),
        );
        let text = truncate(&art.summary_text, 200);
        for line in text.lines() {
            println!("    {line}");
        }
    }
    Ok(())
}

fn dump_extraction_artifacts(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn
        .open_table(tables::EXTRACTION_ARTIFACTS)
        .context("open extraction_artifacts")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Extraction Artifacts ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (_, v) = entry.context("read entry")?;
        let art: schema::ExtractionArtifact =
            serde_json::from_slice(v.value()).context("parse extraction")?;
        println!(
            "  episode={} rev={} checksum={}",
            art.episode_id,
            art.revision,
            hex_short(&art.payload_checksum),
        );
        if let Ok(json) =
            serde_json::from_slice::<serde_json::Value>(&art.output_json)
        {
            let pretty = serde_json::to_string_pretty(&json)
                .unwrap_or_else(|_| format!("{json}"));
            for line in pretty.lines() {
                println!("    {line}");
            }
        } else {
            println!("    (raw {} bytes)", art.output_json.len());
        }
    }
    Ok(())
}

fn dump_embedding_artifacts(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn
        .open_table(tables::EMBEDDING_ARTIFACTS)
        .context("open embedding_artifacts")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Embedding Artifacts ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (_, v) = entry.context("read entry")?;
        let art: schema::EmbeddingArtifact =
            serde_json::from_slice(v.value()).context("parse embedding")?;
        let pooled_dims = art.pooled_vector_bytes.len() / 4;
        let late_status = art.late_interaction_bytes.as_ref().map_or_else(
            || "no".to_string(),
            |bytes| format!("yes ({}B)", bytes.len()),
        );
        println!(
            "  {} rev={} backend={:?} quant={} checksum={}",
            art.artifact_id,
            art.revision,
            art.backend,
            art.quantization.as_deref().unwrap_or("none"),
            hex_short(&art.payload_checksum),
        );
        println!(
            "    pooled_vector: {pooled_dims} dims, late_interaction: {late_status}",
        );
    }
    Ok(())
}

fn dump_processing_jobs(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn
        .open_table(tables::PROCESSING_JOBS)
        .context("open processing_jobs")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Processing Jobs ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (k, v) = entry.context("read entry")?;
        let val: serde_json::Value =
            serde_json::from_slice(v.value()).unwrap_or_default();
        println!("  seq={}: {}", k.value(), format_json(&val));
    }
    Ok(())
}

fn dump_projection_metadata(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn
        .open_table(tables::PROJECTION_METADATA)
        .context("open projection_metadata")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Projection Metadata ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (_, v) = entry.context("read entry")?;
        let val: serde_json::Value =
            serde_json::from_slice(v.value()).unwrap_or_default();
        println!("  {}", format_json(&val));
    }
    Ok(())
}

fn dump_repo_config(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn
        .open_table(tables::REPO_CONFIG)
        .context("open repo_config")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Repo Config ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (_, v) = entry.context("read entry")?;
        let val: serde_json::Value =
            serde_json::from_slice(v.value()).unwrap_or_default();
        println!("  {}", format_json(&val));
    }
    Ok(())
}

fn dump_retrieval_stats(db: &dyn ReadableDatabase) -> Result<()> {
    let txn = db.begin_read().context("begin read")?;
    let table = txn
        .open_table(tables::RETRIEVAL_STATS)
        .context("open retrieval_stats")?;
    let count = table.iter().context("iter")?.count();
    println!("\n── Retrieval Stats ({count}) ──");
    for entry in table.iter().context("iter")? {
        let (k, v) = entry.context("read entry")?;
        let val: serde_json::Value =
            serde_json::from_slice(v.value()).unwrap_or_default();
        println!("  {}: {}", k.value(), format_json(&val));
    }
    Ok(())
}

// ── Grafeo rebuild + dump ───────────────────────────────────────

fn rebuild_grafeo(db: &dyn ReadableDatabase) -> Result<grafeo::GrafeoDB> {
    use lobster::{
        episodes::finalize::parse_entity_kind,
        extract::traits::ExtractionOutput,
        graph::{db as gdb, projection},
        store::ids::EntityId,
    };

    let grafeo = gdb::new_in_memory();

    let txn = db.begin_read().context("begin read")?;
    let ep_table = txn.open_table(tables::EPISODES).context("open episodes")?;

    for entry in ep_table.iter().context("iter")? {
        let (_, v) = entry.context("read entry")?;
        let ep: schema::Episode =
            serde_json::from_slice(v.value()).context("parse episode")?;

        if ep.processing_state != schema::ProcessingState::Ready {
            continue;
        }

        let ep_node = projection::project_episode(&grafeo, &ep);

        let sum_table = txn
            .open_table(tables::SUMMARY_ARTIFACTS)
            .context("open summary_artifacts")?;
        if let Some(sum_guard) = sum_table
            .get(ep.episode_id.raw().as_bytes())
            .context("get summary")?
        {
            if let Ok(art) = serde_json::from_slice::<schema::SummaryArtifact>(
                sum_guard.value(),
            ) {
                gdb::set_episode_summary(&grafeo, ep_node, &art.summary_text);
            }
        }

        let dec_table = txn
            .open_table(tables::DECISIONS)
            .context("open decisions")?;
        for dec_entry in dec_table.iter().context("iter")? {
            let (_, dv) = dec_entry.context("read entry")?;
            if let Ok(dec) =
                serde_json::from_slice::<schema::Decision>(dv.value())
            {
                if dec.episode_id == ep.episode_id {
                    projection::project_decision(&grafeo, &dec, ep_node);
                }
            }
        }

        let ext_table = txn
            .open_table(tables::EXTRACTION_ARTIFACTS)
            .context("open extraction_artifacts")?;
        if let Some(ext_guard) = ext_table
            .get(ep.episode_id.raw().as_bytes())
            .context("get extraction")?
        {
            if let Ok(art) = serde_json::from_slice::<schema::ExtractionArtifact>(
                ext_guard.value(),
            ) {
                if let Ok(output) =
                    serde_json::from_slice::<ExtractionOutput>(&art.output_json)
                {
                    for entity_fact in &output.entities {
                        let ent = schema::Entity {
                            entity_id: EntityId::derive(
                                entity_fact.name.as_bytes(),
                            ),
                            repo_id: ep.repo_id,
                            kind: parse_entity_kind(&entity_fact.kind),
                            canonical_name: entity_fact.name.clone(),
                        };
                        projection::project_entity(&grafeo, &ent, ep_node);
                    }
                }
            }
        }
    }

    Ok(grafeo)
}

fn dump_grafeo(grafeo: &grafeo::GrafeoDB) {
    use std::collections::BTreeMap;

    println!();
    println!("═══════════════════════════════════════════════════════");
    println!(
        "  Grafeo semantic layer (nodes={}, edges={})",
        grafeo.node_count(),
        grafeo.edge_count(),
    );
    println!("═══════════════════════════════════════════════════════");

    // Build a lookup: NodeId -> short display name for readable edges
    let mut node_names: BTreeMap<grafeo::NodeId, String> = BTreeMap::new();
    // Also collect edges per source node for inline display
    let mut outgoing: BTreeMap<grafeo::NodeId, Vec<(String, String)>> =
        BTreeMap::new();

    for node in grafeo.iter_nodes() {
        let labels: Vec<_> =
            node.labels.iter().map(AsRef::as_ref).collect::<Vec<&str>>();
        let name =
            node_display_name_from(&labels, node.id, |k| prop_str!(node, k));
        node_names.insert(node.id, name);
    }

    for edge in grafeo.iter_edges() {
        let dst_name = node_names
            .get(&edge.dst)
            .cloned()
            .unwrap_or_else(|| format!("node:{}", edge.dst));
        outgoing
            .entry(edge.src)
            .or_default()
            .push((edge.edge_type.to_string(), dst_name));
    }

    // ── Episodes ────────────────────────────────────────────────
    let episodes: Vec<_> = grafeo
        .iter_nodes()
        .filter(|n| n.has_label("Episode"))
        .collect();
    if !episodes.is_empty() {
        println!("\n── Episodes ({}) ──", episodes.len());
        for node in &episodes {
            let ep_id = prop_str!(node, "episode_id");
            let state = prop_str!(node, "processing_state");
            let ts = node
                .get_property("finalized_ts_ms")
                .and_then(grafeo::Value::as_int64)
                .map_or_else(|| "?".to_string(), format_ts);
            println!("  {ep_id} state={state} ts={ts}");

            if let Some(summary) = node.get_property("summary_text") {
                let text = summary.as_str().map_or_else(
                    || strip_quotes(&summary.to_string()),
                    String::from,
                );
                if !text.is_empty() {
                    println!();
                    for line in text.lines() {
                        println!("    {line}");
                    }
                    println!();
                }
            }

            print_edges(&outgoing, node.id, &node_names);
        }
    }

    // ── Decisions ───────────────────────────────────────────────
    let decisions: Vec<_> = grafeo
        .iter_nodes()
        .filter(|n| n.has_label("Decision"))
        .collect();
    if !decisions.is_empty() {
        println!("\n── Decisions ({}) ──", decisions.len());
        for node in &decisions {
            let confidence = prop_str!(node, "confidence");
            let statement = prop_str!(node, "statement");
            println!("  [{confidence}] {statement}");

            let rationale = prop_str!(node, "rationale");
            if !rationale.is_empty() {
                println!("    rationale: {rationale}");
            }

            print_edges(&outgoing, node.id, &node_names);
        }
    }

    // ── Tasks ───────────────────────────────────────────────────
    let tasks: Vec<_> = grafeo
        .iter_nodes()
        .filter(|n| n.has_label("Task"))
        .collect();
    if !tasks.is_empty() {
        println!("\n── Tasks ({}) ──", tasks.len());
        for node in &tasks {
            let status = prop_str!(node, "status");
            let title = prop_str!(node, "title");
            println!("  [{status}] {title}");
            print_edges(&outgoing, node.id, &node_names);
        }
    }

    dump_grafeo_entities(grafeo, &node_names);
    dump_grafeo_edge_summary(grafeo);
}

fn dump_grafeo_entities(
    grafeo: &grafeo::GrafeoDB,
    node_names: &std::collections::BTreeMap<grafeo::NodeId, String>,
) {
    use std::collections::BTreeMap;

    let entities: Vec<(String, String, grafeo::NodeId)> = grafeo
        .iter_nodes()
        .filter(|n| n.has_label("Entity"))
        .map(|n| {
            let kind = prop_str!(n, "kind");
            let name = prop_str!(n, "canonical_name");
            (kind, name, n.id)
        })
        .collect();
    if entities.is_empty() {
        return;
    }

    let mut by_kind: BTreeMap<&str, Vec<(&str, grafeo::NodeId)>> =
        BTreeMap::new();
    for (kind, name, nid) in &entities {
        by_kind.entry(kind).or_default().push((name, *nid));
    }

    println!("\n── Entities ({}) ──", entities.len());
    for (kind, nodes) in &by_kind {
        println!("\n  {kind} ({}):", nodes.len());
        for (name, nid) in nodes {
            print!("    - {name}");

            let refs: Vec<_> = grafeo
                .iter_edges()
                .filter(|e| e.dst == *nid)
                .filter_map(|e| node_names.get(&e.src).map(String::as_str))
                .collect();
            if !refs.is_empty() {
                print!("  (from: {})", refs.join(", "));
            }

            println!();
        }
    }
}

fn dump_grafeo_edge_summary(grafeo: &grafeo::GrafeoDB) {
    use std::collections::BTreeMap;

    let mut edge_type_counts: BTreeMap<String, usize> = BTreeMap::new();
    for edge in grafeo.iter_edges() {
        *edge_type_counts
            .entry(edge.edge_type.to_string())
            .or_default() += 1;
    }
    if !edge_type_counts.is_empty() {
        println!("\n── Edge types ──");
        for (etype, count) in &edge_type_counts {
            println!("  {etype}: {count}");
        }
    }
}

fn node_display_name_from(
    labels: &[impl AsRef<str>],
    id: grafeo::NodeId,
    get: impl Fn(&str) -> String,
) -> String {
    let has = |l: &str| labels.iter().any(|x| x.as_ref() == l);
    if has("Episode") {
        format!("episode:{}", truncate(&get("episode_id"), 12))
    } else if has("Decision") {
        format!("decision:\"{}\"", truncate(&get("statement"), 40))
    } else if has("Task") {
        format!("task:\"{}\"", truncate(&get("title"), 40))
    } else if has("Entity") {
        let name = get("canonical_name");
        let kind = get("kind");
        format!("{kind}:{name}")
    } else {
        format!("node:{id}")
    }
}

fn print_edges(
    outgoing: &std::collections::BTreeMap<
        grafeo::NodeId,
        Vec<(String, String)>,
    >,
    node_id: grafeo::NodeId,
    _names: &std::collections::BTreeMap<grafeo::NodeId, String>,
) {
    if let Some(edges) = outgoing.get(&node_id) {
        for (etype, dst_name) in edges {
            println!("    -> [{etype}] {dst_name}");
        }
    }
}

// ── Formatting helpers ──────────────────────────────────────────

fn format_ts(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map_or_else(
        || format!("{ms}ms"),
        |dt| dt.format("%Y-%m-%d %H:%M:%S").to_string(),
    )
}

fn hex_short(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(11);
    for b in bytes.iter().take(4) {
        write!(s, "{b:02x}").unwrap();
    }
    s.push_str("...");
    s
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Find the last char boundary at or before `max` to avoid
        // slicing inside a multi-byte UTF-8 sequence.
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

fn format_json(val: &serde_json::Value) -> String {
    serde_json::to_string(val).unwrap_or_else(|_| format!("{val}"))
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    // ── format_ts ───────────────────────────────────────────────

    /// Any i64 produces a non-empty string (never panics).
    #[hegel::test(test_cases = 500)]
    fn prop_format_ts_never_panics(tc: TestCase) {
        let ms: i64 = tc.draw(gs::integers());
        let out = format_ts(ms);
        assert!(!out.is_empty());
    }

    /// Valid timestamps (post-epoch, pre-year-9999) produce the
    /// expected `YYYY-MM-DD HH:MM:SS` format (19 chars).
    #[hegel::test(test_cases = 500)]
    fn prop_format_ts_valid_range_format(tc: TestCase) {
        // 1970-01-01 to 2100-01-01 in millis
        let ms: i64 = tc
            .draw(gs::integers().min_value(0_i64).max_value(4_102_444_800_000));
        let out = format_ts(ms);
        assert_eq!(
            out.len(),
            19,
            "expected YYYY-MM-DD HH:MM:SS (19 chars), got {out:?}"
        );
        assert_eq!(&out[4..5], "-");
        assert_eq!(&out[7..8], "-");
        assert_eq!(&out[10..11], " ");
        assert_eq!(&out[13..14], ":");
        assert_eq!(&out[16..17], ":");
    }

    /// Out-of-range timestamps fall back to `{ms}ms`.
    #[test]
    fn test_format_ts_fallback() {
        // chrono cannot represent i64::MIN as millis
        let out = format_ts(i64::MIN);
        assert!(out.ends_with("ms"), "expected fallback, got {out:?}");
    }

    // ── hex_short ───────────────────────────────────────────────

    /// Output is always exactly 11 bytes: 8 hex chars + "...".
    #[hegel::test(test_cases = 500)]
    fn prop_hex_short_length(tc: TestCase) {
        let bytes: [u8; 32] = tc.draw(gs::arrays(gs::integers()));
        let out = hex_short(&bytes);
        assert_eq!(out.len(), 11, "got {out:?}");
    }

    /// Output matches manual formatting of the first 4 bytes.
    #[hegel::test(test_cases = 500)]
    fn prop_hex_short_matches_reference(tc: TestCase) {
        let bytes: [u8; 32] = tc.draw(gs::arrays(gs::integers()));
        let expected = format!(
            "{:02x}{:02x}{:02x}{:02x}...",
            bytes[0], bytes[1], bytes[2], bytes[3]
        );
        assert_eq!(hex_short(&bytes), expected);
    }

    // ── truncate ────────────────────────────────────────────────

    /// Output length never exceeds max + 3 (for the "..." suffix).
    #[hegel::test(test_cases = 500)]
    fn prop_truncate_bounded(tc: TestCase) {
        let s: String = tc.draw(gs::text().max_size(500));
        let max: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(500));
        let out = truncate(&s, max);
        assert!(
            out.len() <= max + 3,
            "truncate({:?}, {max}) = {:?} (len {})",
            s,
            out,
            out.len()
        );
    }

    /// Strings at or below max are returned unchanged.
    #[hegel::test(test_cases = 500)]
    fn prop_truncate_identity_when_short(tc: TestCase) {
        let s: String = tc.draw(gs::text().max_size(200));
        let max: usize = tc.draw(
            gs::integers::<usize>()
                .min_value(s.len())
                .max_value(s.len() + 100),
        );
        assert_eq!(truncate(&s, max), s);
    }

    /// Strings longer than max get the "..." suffix.
    #[hegel::test(test_cases = 500)]
    fn prop_truncate_adds_ellipsis(tc: TestCase) {
        let s: String = tc.draw(gs::text().min_size(2).max_size(500));
        let max: usize = tc.draw(
            gs::integers::<usize>()
                .min_value(0)
                .max_value(s.len().saturating_sub(1)),
        );
        let out = truncate(&s, max);
        assert!(out.ends_with("..."), "expected '...' suffix, got {out:?}");
    }

    // ── strip_quotes ────────────────────────────────────────────

    /// Wrapping a string in quotes then stripping recovers original.
    #[hegel::test(test_cases = 500)]
    fn prop_strip_quotes_round_trip(tc: TestCase) {
        let s: String = tc.draw(gs::text().max_size(200));
        let quoted = format!("\"{s}\"");
        assert_eq!(strip_quotes(&quoted), s);
    }

    /// Applying `strip_quotes` twice to an unquoted string is the
    /// same as applying it once (idempotent on unquoted input).
    #[hegel::test(test_cases = 500)]
    fn prop_strip_quotes_idempotent_unquoted(tc: TestCase) {
        // Generate strings that don't start AND end with quotes
        let s: String = tc.draw(gs::text().max_size(200));
        let once = strip_quotes(&s);
        let twice = strip_quotes(&once);
        assert_eq!(once, twice);
    }

    /// Strings without matching quotes are returned unchanged.
    #[test]
    fn test_strip_quotes_partial() {
        assert_eq!(strip_quotes("\"hello"), "\"hello");
        assert_eq!(strip_quotes("hello\""), "hello\"");
        assert_eq!(strip_quotes("hello"), "hello");
        assert_eq!(strip_quotes(""), "");
    }

    // ── format_json ─────────────────────────────────────────────

    /// `format_json` round-trips with `serde_json::from_str` for any
    /// valid JSON value.
    #[hegel::test(test_cases = 200)]
    fn prop_format_json_roundtrip(tc: TestCase) {
        // Generate simple JSON values
        let n: i64 = tc.draw(gs::integers());
        let val = serde_json::Value::Number(n.into());
        let out = format_json(&val);
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("valid json");
        assert_eq!(val, parsed);
    }

    // ── redb dump round-trip ────────────────────────────────────

    use lobster::store::{
        crud,
        db,
        ids::{EpisodeId, RepoId},
        schema::{Episode, EventKind, ProcessingState, RawEvent},
    };

    #[hegel::composite]
    fn gen_raw_event(tc: hegel::TestCase, seq: u64) -> RawEvent {
        let repo_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let payload: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).max_size(128));
        let mut hash = [0u8; 32];
        hash[..payload.len().min(32)]
            .copy_from_slice(&payload[..payload.len().min(32)]);
        RawEvent {
            seq,
            repo_id: RepoId::derive(&repo_input),
            ts_utc_ms: tc.draw(
                gs::integers::<i64>()
                    .min_value(0)
                    .max_value(4_102_444_800_000),
            ),
            event_kind: EventKind::UserPromptSubmit,
            payload_hash: hash,
            payload_bytes: payload,
        }
    }

    #[hegel::composite]
    fn gen_episode(tc: hegel::TestCase) -> Episode {
        let ep_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(32));
        let repo_input: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(16));
        let start: u64 =
            tc.draw(gs::integers::<u64>().min_value(0).max_value(1_000_000));
        let end: u64 = tc.draw(
            gs::integers::<u64>()
                .min_value(start)
                .max_value(start + 10_000),
        );
        Episode {
            episode_id: EpisodeId::derive(&ep_input),
            repo_id: RepoId::derive(&repo_input),
            start_seq: start,
            end_seq: end,
            task_id: None,
            processing_state: ProcessingState::Pending,
            finalized_ts_utc_ms: tc.draw(
                gs::integers::<i64>()
                    .min_value(0)
                    .max_value(4_102_444_800_000),
            ),
            retry_count: 0,
            is_noisy: false,
        }
    }

    /// Helper: count entries in a redb table via the same iteration
    /// pattern that the dump functions use.
    fn count_table_entries<K: redb::Key + 'static, V: redb::Value + 'static>(
        db: &impl ReadableDatabase,
        table_def: redb::TableDefinition<K, V>,
    ) -> usize {
        let txn = db.begin_read().unwrap();
        let table = txn.open_table(table_def).unwrap();
        table.iter().unwrap().count()
    }

    /// Writing N raw events via CRUD produces exactly N entries
    /// when iterated with the dump pattern.
    #[hegel::test(test_cases = 50)]
    fn prop_raw_events_dump_count(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(20));
        let database = db::open_in_memory().unwrap();

        for seq in 0..n {
            let event = tc.draw(gen_raw_event(seq as u64));
            crud::append_raw_event(&database, &event).unwrap();
        }

        assert_eq!(count_table_entries(&database, tables::RAW_EVENTS), n);
    }

    /// Writing N episodes via CRUD produces exactly N entries
    /// when iterated with the dump pattern. Uses unique episode
    /// IDs to avoid overwrites.
    #[hegel::test(test_cases = 50)]
    fn prop_episodes_dump_count(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(0).max_value(20));
        let database = db::open_in_memory().unwrap();

        for i in 0..n {
            let mut ep = tc.draw(gen_episode());
            // Ensure unique episode IDs by mixing in the index
            ep.episode_id = EpisodeId::derive(format!("ep-{i}").as_bytes());
            crud::put_episode(&database, &ep).unwrap();
        }

        assert_eq!(count_table_entries(&database, tables::EPISODES), n);
    }

    /// Every raw event written can be deserialized back through
    /// the table iteration pattern (same as dump functions).
    #[hegel::test(test_cases = 50)]
    fn prop_raw_events_deserialize_roundtrip(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(10));
        let database = db::open_in_memory().unwrap();
        let mut written = Vec::new();

        for seq in 0..n {
            let event = tc.draw(gen_raw_event(seq as u64));
            crud::append_raw_event(&database, &event).unwrap();
            written.push(event);
        }

        // Read back via the same pattern as dump_raw_events
        let txn = database.begin_read().unwrap();
        let table = txn.open_table(tables::RAW_EVENTS).unwrap();
        let mut read_back = Vec::new();
        for entry in table.iter().unwrap() {
            let (_, v) = entry.unwrap();
            let event: RawEvent = serde_json::from_slice(v.value()).unwrap();
            read_back.push(event);
        }

        assert_eq!(written.len(), read_back.len());
        for (w, r) in written.iter().zip(read_back.iter()) {
            assert_eq!(w, r);
        }
    }

    /// Every episode written can be deserialized back through
    /// table iteration.
    #[hegel::test(test_cases = 50)]
    fn prop_episodes_deserialize_roundtrip(tc: TestCase) {
        let n: usize =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(10));
        let database = db::open_in_memory().unwrap();
        let mut written = Vec::new();

        for i in 0..n {
            let mut ep = tc.draw(gen_episode());
            ep.episode_id = EpisodeId::derive(format!("ep-{i}").as_bytes());
            crud::put_episode(&database, &ep).unwrap();
            written.push(ep);
        }

        let txn = database.begin_read().unwrap();
        let table = txn.open_table(tables::EPISODES).unwrap();
        let mut read_back: Vec<Episode> = Vec::new();
        for entry in table.iter().unwrap() {
            let (_, v) = entry.unwrap();
            read_back.push(serde_json::from_slice(v.value()).unwrap());
        }

        assert_eq!(written.len(), read_back.len());
        // Sort both by episode_id for stable comparison
        written.sort_by_key(|e| e.episode_id);
        read_back.sort_by_key(|e| e.episode_id);
        for (w, r) in written.iter().zip(read_back.iter()) {
            assert_eq!(w, r);
        }
    }
}
