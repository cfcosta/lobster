# Grafeo

A high-performance, embeddable graph database written in pure Rust. Supports both
Labeled Property Graph (LPG) and RDF data models. Runs embedded as an in-process library
or as a standalone server.

- **Repository**: <https://github.com/GrafeoDB/grafeo>
- **Crates.io**: <https://crates.io/crates/grafeo>
- **Documentation**: <https://docs.rs/grafeo/latest/grafeo/>
- **Website**: <https://grafeo.dev>

## Key properties

- ACID transactions with MVCC snapshot isolation
- In-memory and persistent (WAL-backed) storage
- Push-based vectorized execution with morsel-driven parallelism
- Cost-based query optimizer
- HNSW vector indexes with SIMD acceleration
- BM25 text search
- Hybrid search (vector + text with RRF fusion)
- Multiple query languages: GQL (default), Cypher, SPARQL, Gremlin, GraphQL, SQL/PGQ

## Adding to your project

```toml
[dependencies]
grafeo = "0.5.34"
```

The default feature profile ("embedded") includes GQL, AI features (vector/text/hybrid
search), graph algorithms, and parallel execution.

### Feature flag profiles

```toml
# Everything: all query languages + AI + algorithms + storage
grafeo = { version = "0.5.34", default-features = false, features = ["full"] }

# Minimal: GQL only
grafeo = { version = "0.5.34", default-features = false, features = ["gql"] }

# GQL + vector/text/hybrid search
grafeo = { version = "0.5.34", default-features = false, features = ["gql", "ai"] }

# GQL + graph algorithms
grafeo = { version = "0.5.34", default-features = false, features = ["gql", "algos"] }

# GQL + persistent storage (WAL, .grafeo file format, disk spill, mmap)
grafeo = { version = "0.5.34", default-features = false, features = ["gql", "storage"] }
```

Notable individual features: `gql`, `cypher`, `sparql`, `gremlin`, `graphql`, `sql-pgq`,
`vector-index`, `text-index`, `hybrid-search`, `cdc`, `embed`, `algos`, `wal`,
`grafeo-file`, `spill`, `mmap`, `parallel`, `arrow-export`, `jsonl-import`,
`parquet-import`.

## Key types

| Type          | Description                                                                                                                                             |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GrafeoDB`    | Main database handle. Entry point for everything.                                                                                                       |
| `Session`     | Lightweight query/transaction handle.                                                                                                                   |
| `QueryResult` | Rows + column names returned from queries.                                                                                                              |
| `Value`       | Dynamic property value enum (Null, Bool, Int64, Float64, String, Bytes, Timestamp, Date, Time, Duration, Vector, List, Map, Path, GCounter, OnCounter). |
| `NodeId`      | Opaque node identifier (wraps u64).                                                                                                                     |
| `EdgeId`      | Opaque edge identifier (wraps u64).                                                                                                                     |
| `Config`      | Database configuration.                                                                                                                                 |

## Basic usage

### Creating a database and running queries

```rust
use grafeo::GrafeoDB;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // In-memory database (no persistence, no setup)
    let db = GrafeoDB::new_in_memory();
    let session = db.session();

    // Insert nodes using GQL syntax
    session.execute("INSERT (:Person {name: 'Alix', age: 30, city: 'Utrecht'})")?;
    session.execute("INSERT (:Person {name: 'Gus', age: 28, city: 'Leiden'})")?;

    // Insert edges
    session.execute(
        "MATCH (a:Person {name: 'Alix'}), (b:Person {name: 'Gus'})
         INSERT (a)-[:KNOWS {since: 2020}]->(b)",
    )?;

    // Query
    let result = session.execute(
        "MATCH (p:Person) RETURN p.name, p.age ORDER BY p.name",
    )?;

    for row in result.iter() {
        let name = row[0].as_str().unwrap_or("?");
        let age = row[1].as_int64().unwrap_or(0);
        println!("{} (age {})", name, age);
    }

    // Scalar extraction for aggregates
    let count: i64 = session
        .execute("MATCH (p:Person) RETURN COUNT(p)")?
        .scalar()?;
    println!("Total people: {count}");

    Ok(())
}
```

### Direct execution vs sessions

```rust
// One-off query (creates a temporary session internally)
db.execute("INSERT (:Person {name: 'Alix'})")?;

// Session for multiple queries (more efficient, supports transactions)
let session = db.session();
session.execute("INSERT (:Person {name: 'Alix'})")?;
session.execute("INSERT (:Person {name: 'Gus'})")?;
```

### Parameterized queries

Always use parameters for user-supplied values to avoid injection:

```rust
use std::collections::HashMap;
use grafeo::Value;

let mut params = HashMap::new();
params.insert("name".to_string(), Value::from("Alix"));
params.insert("age".to_string(), Value::from(30_i64));

session.execute_with_params(
    "INSERT (:Person {name: $name, age: $age})",
    params,
)?;
```

## Programmatic CRUD API

Besides the query language, `GrafeoDB` exposes a direct API:

```rust
use grafeo::{GrafeoDB, Value};

let db = GrafeoDB::new_in_memory();

// Create nodes
let alix = db.create_node(&["Person"]);
db.set_node_property(alix, "name", Value::from("Alix"));
db.set_node_property(alix, "age", Value::from(30_i64));

// Create with properties in one call
let gus = db.create_node_with_props(
    &["Person"],
    [("name", Value::from("Gus")), ("age", Value::from(28_i64))],
);

// Create edges
let edge_id = db.create_edge(alix, gus, "KNOWS");
db.set_edge_property(edge_id, "since", Value::from(2020_i64));

// Read
if let Some(node) = db.get_node(alix) {
    let name = node.get_property("name");
}

// Label management
db.add_node_label(alix, "Employee");
db.remove_node_label(alix, "Contractor");
let labels = db.get_node_labels(alix);

// Graph traversal (these methods are on Session, not GrafeoDB)
let session = db.session();
let outgoing = session.get_neighbors_outgoing(alix);   // Vec<(NodeId, EdgeId)>
let incoming = session.get_neighbors_incoming(gus);

// Deletion
db.delete_edge(edge_id);
db.delete_node(alix);

// Counts
let n = db.node_count();
let e = db.edge_count();
```

## Transactions

```rust
let mut session = db.session();

// Begin / commit
session.begin_transaction()?;
session.execute("INSERT (:Person {name: 'Alix'})")?;
session.execute("INSERT (:Person {name: 'Gus'})")?;
session.commit()?;

// Rollback
session.begin_transaction()?;
session.execute("INSERT (:Person {name: 'Temp'})")?;
session.rollback()?; // discards "Temp"

// Savepoints for partial rollback
session.begin_transaction()?;
session.execute("INSERT (:Person {name: 'Mia'})")?;
session.savepoint("after_mia")?;
session.execute("INSERT (:Person {name: 'Butch'})")?;
session.rollback_to_savepoint("after_mia")?; // undoes Butch, keeps Mia
session.commit()?;

// Prepared commit (inspect changes before committing)
session.begin_transaction()?;
session.execute("INSERT (:Person {name: 'Jules'})")?;
let prepared = session.prepare_commit()?;
// Inspect prepared.info(), then:
prepared.commit()?;
// Or: prepared.abort()?;
```

## Persistence

Requires the `storage` feature (`wal` + `grafeo-file` + `spill` + `mmap`):

```rust
// Open/create a persistent database at a path
let db = GrafeoDB::open("./my_database")?;
let session = db.session();
session.execute("INSERT (:Person {name: 'Alix'})")?;
db.close()?; // flushes WAL

// Reopen: data survives
let db2 = GrafeoDB::open("./my_database")?;

// Read-only mode (shared lock, multiple readers)
let ro = GrafeoDB::open_read_only("./my_graph.grafeo")?;

// Snapshot export/import (in-memory serialization)
let snapshot: Vec<u8> = db.export_snapshot()?;
let restored = GrafeoDB::import_snapshot(&snapshot)?;

// Save in-memory database to disk
db.save("/path/to/backup")?;

// Load persistent database into memory
let in_mem = GrafeoDB::open_in_memory("/path/to/backup")?;
```

## Vector similarity search

Requires the `vector-index` feature (included in default profile):

```rust
use grafeo::{GrafeoDB, Value};

let db = GrafeoDB::new_in_memory();

// Create documents with vector embeddings
let doc = db.create_node(&["Document"]);
db.set_node_property(doc, "title", Value::from("Graph Databases"));
db.set_node_property(
    doc, "embedding",
    Value::Vector(vec![0.9, 0.1, 0.2, 0.0].into()),
);

// Build an HNSW index
db.create_vector_index(
    "Document", "embedding",
    Some(4),           // dimensions (or None to infer)
    Some("cosine"),    // "cosine", "euclidean", "dot_product", "manhattan"
    None,              // m (default 16)
    None,              // ef_construction (default 128)
)?;

// k-NN search
let query = [0.85_f32, 0.15, 0.2, 0.1];
let results = db.vector_search(
    "Document", "embedding",
    &query,
    3,     // k nearest neighbors
    None,  // ef search width
    None,  // property filters
)?;

for (node_id, distance) in &results {
    let node = db.get_node(*node_id).unwrap();
    let title = node.get_property("title").unwrap();
    println!("{}: distance {:.4}", title.as_str().unwrap(), distance);
}

// Vector search via GQL
let result = session.execute(
    "MATCH (d:Document)
     WHERE cosine_similarity(d.embedding, vector([0.85, 0.15, 0.2, 0.1])) > 0.9
     RETURN d.title, cosine_similarity(d.embedding, vector([0.85, 0.15, 0.2, 0.1])) AS score
     ORDER BY score DESC",
)?;
```

## Text and hybrid search

```rust
// Create a BM25 text index
db.create_text_index("Document", "content")?;

// Full-text BM25 search
let results = db.text_search("Document", "content", "graph databases", 10)?;

// Hybrid search: combined vector + text with RRF fusion
// Signature: hybrid_search(label, text_property, vector_property, query_text,
//                          query_vector, k, fusion_method)
let results = db.hybrid_search(
    "Document",
    "content",                     // text property
    "embedding",                   // vector property
    "graph databases",             // query text
    Some(&query_vector),           // query vector (Option<&[f32]>)
    10,                            // k
    None,                          // fusion method (None = default RRF)
)?;
```

## Index management

```rust
// Property indexes (O(1) lookups)
db.create_property_index("email");
let nodes = db.find_nodes_by_property("email", &Value::from("alix@example.com"));
db.drop_property_index("email");

// Vector indexes
db.create_vector_index("Doc", "embedding", Some(384), Some("cosine"), None, None)?;
db.rebuild_vector_index("Doc", "embedding")?;
db.drop_vector_index("Doc", "embedding");

// Text indexes
db.create_text_index("Doc", "content")?;
db.rebuild_text_index("Doc", "content")?;
db.drop_text_index("Doc", "content");

// List all indexes
let indexes = db.list_indexes();
```

## Graph algorithms

Requires the `algos` feature:

```rust
// PageRank
let result = session.execute(
    "CALL grafeo.pagerank({damping: 0.85, max_iterations: 20})"
)?;

// Connected Components
let result = session.execute("CALL grafeo.connected_components()")?;

// Louvain Community Detection
let result = session.execute("CALL grafeo.louvain()")?;

// Degree Centrality
let result = session.execute("CALL grafeo.degree_centrality()")?;

// Also: shortest_path, betweenness_centrality, clustering,
// minimum spanning tree, graph isomorphism, network flow
```

## Working with QueryResult

```rust
let result = session.execute("MATCH (p:Person) RETURN p.name, p.age")?;

// Metadata
println!("Columns: {:?}", result.columns);
println!("Row count: {}", result.row_count());
println!("Is empty: {}", result.is_empty());

// Iterate rows (each row is Vec<Value>)
for row in result.iter() {
    let name: &str = row[0].as_str().unwrap_or("?");
    let age: i64 = row[1].as_int64().unwrap_or(0);
}

// Borrow or take ownership
let rows: &[Vec<Value>] = result.rows();
let owned_rows: Vec<Vec<Value>> = result.into_rows();

// Scalar for single-value results
let count: i64 = session
    .execute("MATCH (p:Person) RETURN COUNT(p)")?
    .scalar()?;
```

## The Value enum

```rust
use grafeo::Value;

let s = Value::from("hello");           // String
let i = Value::from(42_i64);            // Int64
let f = Value::from(3.14_f64);          // Float64
let b = Value::from(true);              // Bool
let n = Value::Null;                    // Null
let v = Value::Vector(vec![0.1, 0.2, 0.3].into()); // Vector (Arc<[f32]>)

// Type checking and extraction
assert!(s.as_str().is_some());
assert_eq!(i.as_int64(), Some(42));
assert_eq!(f.as_float64(), Some(3.14));
assert_eq!(b.as_bool(), Some(true));
assert!(n.is_null());
```

## Custom configuration

```rust
use grafeo::{GrafeoDB, Config, DurabilityMode, GraphModel};
use std::time::Duration;

let config = Config::in_memory()
    .with_memory_limit(512 * 1024 * 1024)
    .with_threads(4)
    .with_query_timeout(Duration::from_secs(30));

let db = GrafeoDB::with_config(config)?;

// Persistent with full tuning
let config = Config::persistent("/path/to/db")
    .with_graph_model(GraphModel::Lpg)
    .with_memory_limit(1024 * 1024 * 1024)
    .with_threads(8)
    .with_wal_durability(DurabilityMode::Sync)
    .with_query_timeout(Duration::from_secs(60));
```

## Admin and introspection

```rust
let info = db.info();            // DatabaseInfo: mode, counts, persistence
let stats = db.detailed_stats(); // memory usage, index counts
let schema = db.schema();        // labels, edge types, property keys
let validation = db.validate();  // integrity check
let mem = db.memory_usage();     // memory breakdown

// WAL management (persistent databases)
let wal = db.wal_status();
db.wal_checkpoint()?;

// CDC (Change Data Capture, requires `cdc` feature)
db.set_cdc_enabled(true);
let changes = db.history(10)?;
let since = db.history_since(epoch_id)?;
```

## Multi-language query support

With the appropriate feature flags enabled, you can query in multiple languages:

```rust
// GQL (default, always available with gql feature)
session.execute("MATCH (p:Person) RETURN p.name")?;

// Cypher (requires `cypher` feature)
session.execute_cypher("MATCH (p:Person) RETURN p.name")?;

// Gremlin (requires `gremlin` feature)
session.execute_gremlin("g.V().hasLabel('Person').values('name')")?;

// GraphQL (requires `graphql` feature)
session.execute_graphql("{ Person { name } }")?;

// SQL/PGQ (requires `sql-pgq` feature)
session.execute_sql(
    "SELECT * FROM GRAPH_TABLE (
         MATCH (p:Person)
         COLUMNS (p.name AS name)
     ) AS g"
)?;

// Dynamic dispatch (language chosen at runtime)
session.execute_language("MATCH (p:Person) RETURN p.name", "gql", None)?;
```

These methods are also available directly on `GrafeoDB` for one-off queries.

## Common patterns

1. **Session reuse**: Create one `Session` and reuse it rather than calling
   `db.execute()` repeatedly (which creates a temporary session each time).

2. **Parameterized queries**: Always use `execute_with_params` with `$param`
   placeholders for user-supplied values.

3. **`scalar()` for aggregates**: When a query returns a single value, use
   `.scalar::<i64>()` to extract it directly.

4. **Programmatic API for bulk loading**: Use `create_node`, `create_node_with_props`,
   `create_edge` for bulk data loading rather than string queries.

5. **Vector search workflow**: Create nodes with `Value::Vector(...)` properties,
   build an HNSW index, then search. The index auto-maintains on new inserts.

6. **Transaction isolation**: Each `Session` has independent transaction state.
   Multiple sessions operate concurrently (MVCC snapshot isolation).

## Role in Lobster

In Lobster, Grafeo is the **semantic serving layer**, not the source of truth (that's
redb). It serves:

- Nodes for episodes, tasks, decisions, and entities
- Evidence-backed edges between them
- Searchable distilled artifacts (summaries, decisions)
- Hybrid retrieval (vector search over pooled embedding proxies + BM25 text)
- Graph neighborhood traversal for context expansion and reranking

Embeddings are produced by Lobster's own runtime (pylate-rs) and projected into Grafeo
explicitly. Grafeo's built-in embedding generation is not used, keeping model provenance
under Lobster's control.

If Grafeo's data is lost, it can be rebuilt from the durable artifacts stored in redb.
