# tantivy

A full-text search engine library written in Rust, inspired by Apache Lucene. It is not
an off-the-shelf search server -- it is a crate you use to build one. Maintained by
Quickwit.

- **Repository**: <https://github.com/quickwit-oss/tantivy>
- **Crates.io**: <https://crates.io/crates/tantivy>
- **Documentation**: <https://docs.rs/tantivy>

## Key properties

- Full-text search with BM25 scoring
- Configurable tokenizers with stemming for 17 languages
- Incremental indexing with multithreaded writers
- mmap-backed persistence or in-memory operation
- SIMD integer compression
- Fast fields (columnar doc values) for sorting and aggregations
- Faceted search (hierarchical categories)
- Aggregations (histogram, range, terms, metrics) with Elasticsearch-compatible JSON API
- Range queries, fuzzy search, regex queries, phrase queries
- Snippet generation with highlighting

## Adding to your project

```toml
[dependencies]
tantivy = "0.26.0"
```

### Feature flags

| Feature                     | What it enables                         | Default |
| --------------------------- | --------------------------------------- | ------- |
| `mmap`                      | `MmapDirectory` for disk-backed indexes | yes     |
| `stopwords`                 | Built-in stop word lists                | yes     |
| `lz4-compression`           | LZ4 compression for document store      | yes     |
| `columnar-zstd-compression` | Zstd compression in columnar/sstable    | yes     |
| `stemmer`                   | Stemmer tokenizer via rust-stemmers     | yes     |
| `zstd-compression`          | Zstd compression for document store     | no      |
| `failpoints`                | Fail points for testing                 | no      |
| `quickwit`                  | SSTable + futures support               | no      |

To disable mmap (e.g., for WASM), use `default-features = false` and select features
individually.

## Core types

### Type aliases

```rust
pub type DocId = u32;          // document ID within a segment (max 2^31 per segment)
pub type Opstamp = u64;        // monotonic operation stamp
pub type Score = f32;          // relevance score
pub type SegmentOrdinal = u32;
```

### DocAddress

Uniquely identifies a document within a `Searcher`:

```rust
pub struct DocAddress {
    pub segment_ord: SegmentOrdinal,
    pub doc_id: DocId,
}
```

### Field value types

```rust
pub enum Type {
    Str,    // text
    U64,    // unsigned 64-bit integer
    I64,    // signed 64-bit integer
    F64,    // 64-bit float
    Bool,   // boolean
    Date,   // date/time (i64 timestamp internally, RFC3339 in JSON)
    Facet,  // hierarchical facet
    Bytes,  // raw bytes
    Json,   // JSON object
    IpAddr, // IPv4/IPv6 (stored as IPv6 internally)
}
```

## Schema

Build a schema using `SchemaBuilder`:

```rust
use tantivy::schema::*;

let mut schema_builder = Schema::builder();

// Text field: tokenized, searchable, with term frequencies and positions
let title = schema_builder.add_text_field("title", TEXT | STORED);

// Text field: not tokenized (exact match), good for IDs
let isbn = schema_builder.add_text_field("isbn", STRING | STORED);

// Numeric fields
let year = schema_builder.add_u64_field("year", INDEXED | STORED | FAST);
let rating = schema_builder.add_f64_field("rating", INDEXED | STORED | FAST);
let temperature = schema_builder.add_i64_field("temperature", INDEXED | FAST);

// Boolean
let published = schema_builder.add_bool_field("published", INDEXED | STORED);

// Date
let created = schema_builder.add_date_field("created", INDEXED | STORED | FAST);

// IP address
let ip = schema_builder.add_ip_addr_field("ip", INDEXED | STORED | FAST);

// Facet (hierarchical category)
let category = schema_builder.add_facet_field("category", FacetOptions::default());

// Bytes
let data = schema_builder.add_bytes_field("data", STORED);

// JSON
let metadata = schema_builder.add_json_field("metadata", TEXT | STORED);

let schema = schema_builder.build();
```

### Field option flags

Combinable with `|`:

- **`STORED`** -- field value saved in document store for retrieval
- **`INDEXED`** -- field is searchable via inverted index
- **`FAST`** -- field stored as columnar fast field (like Lucene DocValues), required for
  sorting, aggregations, and `FilterCollector`
- **`COERCE`** -- coerce values to the target type

### Shortcut constants

- **`TEXT`** = indexed with tokenization, term frequencies, and positions. Required for
  phrase queries and BM25 scoring.
- **`STRING`** = indexed but NOT tokenized (the "raw" tokenizer). Good for IDs and exact
  match fields.

### TextOptions for fine-grained control

```rust
let text_opts = TextOptions::default()
    .set_stored()
    .set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("en_stem")  // or "default", "raw", "whitespace", custom
            .set_index_option(IndexRecordOption::WithFreqsAndPositions)
    )
    .set_fast(None);  // also make it a fast field (for aggregations)
```

### IndexRecordOption

Controls what is stored in the inverted index per term:

```rust
pub enum IndexRecordOption {
    Basic,                    // only doc IDs
    WithFreqs,                // doc IDs + term frequencies
    WithFreqsAndPositions,    // doc IDs + TF + positions (needed for phrase queries)
}
```

### Accessing fields from a schema

```rust
let field = schema.get_field("title")?;   // returns Result<Field>
let name = schema.get_field_name(field);   // returns &str
let num = schema.num_fields();
```

## Index creation

```rust
use tantivy::Index;

// In RAM (for tests or small datasets)
let index = Index::create_in_ram(schema.clone());

// On disk with MmapDirectory (requires "mmap" feature)
let index = Index::create_in_dir("/path/to/dir", schema.clone())?;

// Open existing, or create if not present
let index = Index::open_or_create(directory, schema.clone())?;

// Open existing only
let index = Index::open_in_dir("/path/to/dir")?;

// Builder pattern with custom settings
let index = Index::builder()
    .schema(schema.clone())
    .settings(IndexSettings {
        docstore_compression: Compressor::Lz4,
        docstore_blocksize: 16_384,
        docstore_compress_dedicated_thread: true,
    })
    .create_in_dir("/path")?;
```

### Directory types

- **`RamDirectory`** -- in-memory, useful for tests. `RamDirectory::create()`.
- **`MmapDirectory`** -- memory-mapped files on disk. `MmapDirectory::open(path)`.
  Requires `mmap` feature.
- **`ManagedDirectory`** -- wraps any Directory, manages file lifecycle and garbage
  collection. Used internally by Index.

## Indexing documents

### IndexWriter

```rust
// Automatic thread count, total memory budget
let mut index_writer: IndexWriter = index.writer(50_000_000)?; // 50 MB

// Explicit thread count
let mut index_writer = index.writer_with_num_threads(1, 15_000_000)?;

// For tests (1 thread, 15 MB)
let mut index_writer = index.writer_for_tests()?;
```

**Only one `IndexWriter` can exist at a time** for a given index. Attempting to create a
second returns a lock error.

### Creating documents

Using the `doc!` macro:

```rust
use tantivy::doc;

let doc = doc!(
    title => "The Old Man and the Sea",
    body => "He was an old man...",
    year => 1952u64,
);
index_writer.add_document(doc)?;
```

Multi-valued fields -- repeat the field name:

```rust
let doc = doc!(
    title => "Frankenstein",
    title => "The Modern Prometheus",
    body => "...",
);
```

Manually:

```rust
use tantivy::TantivyDocument;

let mut doc = TantivyDocument::default();
doc.add_text(title, "The Old Man and the Sea");
doc.add_u64(year, 1952);
index_writer.add_document(doc)?;
```

From JSON:

```rust
let doc = TantivyDocument::parse_json(&schema, r#"{"title": "Of Mice and Men", "year": 1937}"#)?;
```

### Committing

Documents are not searchable until committed:

```rust
index_writer.commit()?;
```

Two-phase commit:

```rust
let prepared = index_writer.prepare_commit()?;
prepared.commit()?;
```

Rollback to last commit:

```rust
index_writer.rollback()?;
```

### Deleting and updating

There is no update in tantivy. To update, delete then re-insert:

```rust
index_writer.delete_term(Term::from_field_text(isbn, "978-9176370711"));
index_writer.add_document(doc!(isbn => "978-9176370711", title => "Frankenstein"))?;
index_writer.commit()?;
```

The delete and add happen atomically within the commit.

### Multithreaded indexing

`add_document` only requires a read lock (concurrent from multiple threads). `commit`
requires a write lock:

```rust
use std::sync::{Arc, RwLock};

let index_writer = Arc::new(RwLock::new(index.writer(50_000_000)?));

// In worker threads:
index_writer.read().unwrap().add_document(doc)?;

// For commit:
index_writer.write().unwrap().commit()?;
```

## Searching

### IndexReader and Searcher

```rust
// Default reload policy (OnCommitWithDelay)
let reader = index.reader()?;

// Custom configuration
let reader = index
    .reader_builder()
    .reload_policy(ReloadPolicy::OnCommitWithDelay) // or Manual
    .try_into()?;

// Get a searcher (cheap, snapshot-based)
let searcher = reader.searcher();

// Reload after commit (for Manual policy)
reader.reload()?;
```

### QueryParser

```rust
use tantivy::query::QueryParser;

let mut query_parser = QueryParser::for_index(&index, vec![title, body]);
let query = query_parser.parse_query("sea whale")?;

// Lenient parse (best-effort, returns errors separately)
let (query, errors) = query_parser.parse_query_lenient("sea whale");

// Configuration (requires &mut self)
query_parser.set_conjunction_by_default(); // AND instead of OR
query_parser.set_field_boost(title, 2.0);
query_parser.set_field_fuzzy(title, true, 1, true);
```

### Query language syntax

| Syntax                | Meaning                                                |
| --------------------- | ------------------------------------------------------ |
| `barack obama`        | OR by default (AND after `set_conjunction_by_default`) |
| `title:sea`           | Field-targeted search                                  |
| `AND`, `OR`           | Boolean operators (AND has higher precedence)          |
| `+required -excluded` | Must/must-not                                          |
| `"michael jackson"`   | Phrase (requires positions indexed)                    |
| `"big wolf"~1`        | Phrase with slop                                       |
| `"in the su"*`        | Phrase prefix                                          |
| `year:[1960 TO 1970]` | Inclusive range                                        |
| `year:{1960 TO 1970}` | Exclusive range                                        |
| `year:[* TO 2000]`    | Unbounded range                                        |
| `title: IN [a b cd]`  | Set query                                              |
| `"SRE"^2.0`           | Boost                                                  |
| `*`                   | All documents                                          |

### Programmatic query types

```rust
use tantivy::query::*;

// Single term
TermQuery::new(term, IndexRecordOption::Basic)

// Boolean
BooleanQuery::new(vec![
    (Occur::Must, Box::new(term_query1)),
    (Occur::Should, Box::new(term_query2)),
    (Occur::MustNot, Box::new(term_query3)),
])

// Phrase (requires WithFreqsAndPositions)
PhraseQuery::new(vec![term1, term2])

// Range
RangeQuery::new(Bound::Included(term_low), Bound::Excluded(term_high))

// Fuzzy (third arg is transposition_cost_one, not prefix)
FuzzyTermQuery::new(term, 2 /* distance */, true /* transposition_cost_one */)
// Prefix fuzzy (matches terms sharing a prefix within edit distance)
FuzzyTermQuery::new_prefix(term, 2 /* distance */, true /* transposition_cost_one */)

// Regex (takes a pattern string and a Field, not an Index)
RegexQuery::from_pattern("pattern", field)?

// All / Empty
AllQuery
EmptyQuery

// Boost / constant score
BoostQuery::new(Box::new(inner), 2.0)
ConstScoreQuery::new(Box::new(inner), 1.0)

// Exists (docs where field has a value)
ExistsQuery::new_exists_query("field_name".to_string())

// Efficient multi-term OR
TermSetQuery::new(vec![term1, term2, term3])

// Disjunction max
DisjunctionMaxQuery::new(queries, 0.1 /* tie_breaker */)

// More-like-this (.with_document() terminates the builder, no .build() needed)
MoreLikeThisQuery::builder()
    .with_min_term_frequency(1)
    .with_max_query_terms(10)
    .with_document(doc_address)
```

### Term construction

```rust
Term::from_field_text(field, "word")
Term::from_field_u64(field, 42u64)
Term::from_field_i64(field, -10i64)
Term::from_field_f64(field, 3.14f64)
Term::from_field_bool(field, true)
Term::from_field_date(field, datetime)
Term::from_field_bytes(field, &[0u8, 1, 2])
Term::from_field_ip_addr(field, ipv6_addr)
Term::from_facet(field, &Facet::from("/path/to/facet"))
```

### Running a search

```rust
use tantivy::collector::{Count, TopDocs};

let (count, top_docs): (usize, Vec<(Score, DocAddress)>) =
    searcher.search(&query, &(Count, TopDocs::with_limit(10).order_by_score()))?;

for (score, doc_address) in top_docs {
    let doc: TantivyDocument = searcher.doc(doc_address)?;
    println!("{score}: {}", doc.to_json(&schema));
}
```

## Collectors

### Built-in collectors

```rust
use tantivy::collector::*;

// Count matching documents
let count: usize = searcher.search(&query, &Count)?;

// Top N by score (TopDocs requires an ordering method to become a Collector)
let top: Vec<(Score, DocAddress)> =
    searcher.search(&query, &TopDocs::with_limit(10).order_by_score())?;

// Pagination
TopDocs::with_limit(10).and_offset(20).order_by_score()

// Order by fast field (returns Vec<(Option<u64>, DocAddress)>)
TopDocs::with_limit(10).order_by_fast_field::<u64>("price", Order::Desc)

// Custom score tweak
TopDocs::with_limit(10).tweak_score(move |segment_reader: &SegmentReader| {
    move |doc: DocId, original_score: Score| {
        original_score * some_boost
    }
})

// All matching DocAddresses
let doc_set: HashSet<DocAddress> = searcher.search(&query, &DocSetCollector)?;

// Filter by fast field value
FilterCollector::new(
    "price".to_string(),
    |price: u64| price > 100,
    TopDocs::with_limit(10).order_by_score(),
)

// Facet counting
let mut facet_collector = FacetCollector::for_field("category");
facet_collector.add_facet("/Science");
let facet_counts = searcher.search(&AllQuery, &facet_collector)?;
let facets: Vec<(&Facet, u64)> = facet_counts.get("/Science").collect();

// Histogram
HistogramCollector::new("field".to_string(), min_value, bucket_width, num_buckets)
```

### Combining collectors

Tuples of collectors (up to 4):

```rust
let (count, top_docs) =
    searcher.search(&query, &(Count, TopDocs::with_limit(10).order_by_score()))?;
```

For 5+ collectors, use `MultiCollector`:

```rust
let mut multi = MultiCollector::new();
let count_handle = multi.add_collector(Count);
let top_handle = multi.add_collector(TopDocs::with_limit(10).order_by_score());
let mut multi_fruit = searcher.search(&query, &multi)?;
let count: usize = count_handle.extract(&mut multi_fruit);
```

## Tokenizers

### Built-in (registered by default)

| Name         | Behavior                                                           |
| ------------ | ------------------------------------------------------------------ |
| `default`    | Split on punctuation/whitespace, remove >40 char tokens, lowercase |
| `raw`        | No tokenization (entire string is one token)                       |
| `en_stem`    | Like `default` + English stemming                                  |
| `whitespace` | Split on whitespace only, no lowercasing                           |

### Building custom tokenizers

```rust
use tantivy::tokenizer::*;

let custom = TextAnalyzer::builder(SimpleTokenizer::default())
    .filter(RemoveLongFilter::limit(40))
    .filter(LowerCaser)
    .filter(Stemmer::new(Language::English))
    .build();

index.tokenizers().register("my_custom", custom);
```

Available components:

**Tokenizers**: `SimpleTokenizer`, `WhitespaceTokenizer`, `RawTokenizer`,
`NgramTokenizer::new(min_gram, max_gram, prefix_only)`, `RegexTokenizer`.

**Filters**: `LowerCaser`, `RemoveLongFilter::limit(n)`,
`Stemmer::new(Language::English)`, `StopWordFilter::remove(words)`,
`AlphaNumOnlyFilter`, `AsciiFoldingFilter`, `SplitCompoundWords`.

Then reference your tokenizer in field options:

```rust
TextFieldIndexing::default()
    .set_tokenizer("my_custom")
    .set_index_option(IndexRecordOption::WithFreqsAndPositions)
```

## Snippets

Generate search result snippets with highlighted terms. Requires fields to be `STORED`.

```rust
use tantivy::snippet::SnippetGenerator;

let mut snippet_gen = SnippetGenerator::create(&searcher, &*query, body)?;
snippet_gen.set_max_num_chars(100); // default: 150

for (_score, doc_address) in top_docs {
    let doc: TantivyDocument = searcher.doc(doc_address)?;
    let snippet = snippet_gen.snippet_from_doc(&doc);

    // HTML with <b>highlighted</b> terms
    let html: String = snippet.to_html();

    // Or access raw fragment + highlighted ranges
    let fragment: &str = snippet.fragment();
    let ranges: &[Range<usize>] = snippet.highlighted();
}
```

## Facets

Hierarchical categorization, like directory paths:

```rust
use tantivy::schema::Facet;

// Indexing
let doc = doc!(
    name => "Tiger",
    category => Facet::from("/Felidae/Pantherinae/Panthera"),
);

// Searching by facet
let facet_term = Term::from_facet(category, &Facet::from("/Felidae/Pantherinae"));
let query = TermQuery::new(facet_term, IndexRecordOption::Basic);

// Counting facets
let mut collector = FacetCollector::for_field("category");
collector.add_facet("/Felidae");
let counts = searcher.search(&AllQuery, &collector)?;
for (facet, count) in counts.get("/Felidae") {
    println!("{facet}: {count}");
}
```

## Aggregations

Aggregations require `FAST` fields. The API uses Elasticsearch-compatible JSON.

```rust
use tantivy::aggregation::agg_req::Aggregations;
use tantivy::aggregation::AggregationCollector;

let agg_req: Aggregations = serde_json::from_str(r#"{
    "avg_price": { "avg": { "field": "price" } },
    "price_ranges": {
        "range": {
            "field": "price",
            "ranges": [{"to": 50.0}, {"from": 50.0}]
        },
        "aggs": {
            "avg_stock": { "avg": { "field": "stock" } }
        }
    }
}"#)?;

let collector = AggregationCollector::from_aggs(agg_req, Default::default());
let agg_res = searcher.search(&AllQuery, &collector)?;
let json = serde_json::to_value(agg_res)?;
```

Supported bucket aggregations: `range`, `histogram`, `date_histogram`, `terms`, `filter`,
`composite`.

Supported metric aggregations: `avg`, `min`, `max`, `sum`, `stats`, `extended_stats`,
`count`, `value_count`, `percentiles`, `cardinality`, `top_hits`.

For `terms` aggregation on text fields, the field must use the `raw` tokenizer and be
marked `FAST`.

## Parallel search

By default, search runs single-threaded. For parallel segment searching:

```rust
let index = Index::create_in_ram(schema); // index must be mut or freshly created
index.set_multithread_executor(4)?;         // specific thread count
index.set_default_multithread_executor()?;  // num CPUs
```

## Merge policies

```rust
use tantivy::merge_policy::{LogMergePolicy, NoMergePolicy};

// Default is LogMergePolicy
index_writer.set_merge_policy(Box::new(LogMergePolicy::default()));

// Disable merging
index_writer.set_merge_policy(Box::new(NoMergePolicy));
```

## Important constraints

1. **Only one `IndexWriter` per index.** A lock file prevents concurrent writers.

2. **Documents are immutable.** Updates require delete + re-insert (atomic within a
   commit).

3. **Documents are not searchable until `commit()`.** Readers must also reload
   (`OnCommitWithDelay` does this automatically; `Manual` requires `reader.reload()`).

4. **At most 2^31 documents per segment** (`DocId` is `u32`).

5. **Aggregations require `FAST` fields.** Non-fast fields cannot be used in aggregations.

6. **Terms aggregation on text requires `raw` tokenizer.** The fast field shares the
   dictionary with the inverted index.

7. **Phrase queries require `WithFreqsAndPositions`.** Without positions indexed, phrase
   search fails.

8. **`STRING` vs `TEXT`**: `STRING` stores the entire value as one token (exact match).
   `TEXT` tokenizes with frequencies and positions (full-text search).

9. **Snippet generation requires `STORED` fields.** The `SnippetGenerator` reads field
   content from the document store.

10. **Dates use RFC3339 format** when parsed from strings/JSON.

11. **IPv4 stored as IPv6 internally.** All IP addresses are normalized to IPv6.

12. **Index format version is 7.** Can read indexes from format version 4+.
