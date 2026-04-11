# redb

A simple, portable, high-performance, ACID, embedded key-value store written in pure Rust.
Loosely inspired by LMDB. Data is stored in copy-on-write B-trees in a single file.

- **Repository**: <https://github.com/cberner/redb>
- **Crates.io**: <https://crates.io/crates/redb>
- **Documentation**: <https://docs.rs/redb>
- **Homepage**: <https://www.redb.org>

## Key properties

- Zero-copy, thread-safe, `BTreeMap`-like API
- Fully ACID-compliant transactions
- MVCC: concurrent readers + single writer, without blocking
- Crash-safe by default (no WAL needed)
- Savepoints and rollbacks (ephemeral and persistent)
- No external dependencies (pure Rust, no C bindings)
- Stable file format

## Adding to your project

```toml
[dependencies]
redb = "4.0.0"
```

Optional feature flags:

| Feature         | What it enables                                   |
| --------------- | ------------------------------------------------- |
| `logging`       | `log` crate integration for debug/info/warn       |
| `cache_metrics` | Cache hit/miss/eviction tracking via `CacheStats` |
| `chrono_v0_4`   | `Key`/`Value` impls for `chrono` types            |
| `uuid`          | `Key`/`Value` impls for `uuid::Uuid`              |

For derive macros on custom types:

```toml
[dependencies]
redb-derive = "0.1.0"
```

## Core concepts

### Database

The main entry point. Represents an open database file.

```rust
use redb::Database;

// Create or open a database file
let db = Database::create("my_db.redb")?;

// Open existing only (errors if file doesn't exist)
let db = Database::open("my_db.redb")?;
```

For read-only access (multiple processes can open simultaneously):

```rust
use redb::ReadOnlyDatabase;
let db = ReadOnlyDatabase::open("my_db.redb")?;
```

For advanced configuration, use `Builder`:

```rust
use redb::Builder;

let db = Builder::new()
    .set_cache_size(512 * 1024 * 1024) // 512 MiB (default 1 GiB)
    .create("my_db.redb")?;
```

### TableDefinition

A compile-time constant that names a table and declares its key/value types. Zero-sized
and `Copy`. The actual table is created lazily when first opened in a write transaction.

```rust
use redb::TableDefinition;

const USERS: TableDefinition<&str, u64> = TableDefinition::new("users");
const SCORES: TableDefinition<u64, &[u8]> = TableDefinition::new("scores");
```

For multimap tables (each key maps to multiple values):

```rust
use redb::MultimapTableDefinition;

const TAGS: MultimapTableDefinition<&str, &str> = MultimapTableDefinition::new("tags");
```

### Transactions

**WriteTransaction** -- only one can be active at a time (calls block until the previous
one commits or aborts). Can open tables for both reading and writing. If dropped without
calling `commit()` or `abort()`, it automatically aborts.

```rust
let write_txn = db.begin_write()?;
{
    let mut table = write_txn.open_table(USERS)?;
    table.insert("alice", &42)?;
} // table must be dropped before commit
write_txn.commit()?;
```

**Important**: `begin_read()` comes from the `ReadableDatabase` trait, which must be
imported. `begin_write()` is an inherent method on `Database`.

**ReadTransaction** -- captures a snapshot; unaffected by concurrent writes. Multiple
read transactions can exist simultaneously.

```rust
use redb::ReadableDatabase; // required for begin_read()

let read_txn = db.begin_read()?;
let table = read_txn.open_table(USERS)?;
if let Some(value) = table.get("alice")? {
    println!("alice = {}", value.value());
}
```

### AccessGuard

Zero-copy wrapper around stored data. Call `.value()` to get the deserialized value:

```rust
let guard = table.get("key")?.unwrap();
let val: u64 = guard.value();
```

For `&str` values, `.value()` returns `&str` (zero-copy from the database page).

## Basic usage

### CRUD operations

```rust
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

const TABLE: TableDefinition<&str, u64> = TableDefinition::new("my_data");

fn main() -> Result<(), redb::Error> {
    let db = Database::create("my_db.redb")?;

    // Write
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(TABLE)?;
        table.insert("my_key", &123)?;
        table.insert("another_key", &456)?;
    }
    write_txn.commit()?;

    // Read
    let read_txn = db.begin_read()?;
    let table = read_txn.open_table(TABLE)?;
    assert_eq!(table.get("my_key")?.unwrap().value(), 123);

    // Update (insert returns the old value)
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(TABLE)?;
        let old = table.insert("my_key", &789)?;
        assert_eq!(old.unwrap().value(), 123);
    }
    write_txn.commit()?;

    // Delete
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(TABLE)?;
        let removed = table.remove("my_key")?;
        assert_eq!(removed.unwrap().value(), 789);
    }
    write_txn.commit()?;

    Ok(())
}
```

### Range queries and iteration

```rust
let read_txn = db.begin_read()?;
let table = read_txn.open_table(TABLE)?;

// Range query (half-open interval)
for result in table.range("a".."c")? {
    let (key, value) = result?;
    println!("{} = {}", key.value(), value.value());
}

// Iterate all entries
for result in table.iter()? {
    let (key, value) = result?;
    println!("{} = {}", key.value(), value.value());
}

// Reverse iteration (Range is double-ended)
for result in table.iter()?.rev() {
    let (key, value) = result?;
    println!("{} = {}", key.value(), value.value());
}

// first() and last()
let (first_k, first_v) = table.first()?.unwrap();
let (last_k, last_v) = table.last()?.unwrap();
```

### Multimap tables

```rust
use redb::{Database, MultimapTableDefinition, ReadableMultimapTable};

const TAGS: MultimapTableDefinition<&str, &str> = MultimapTableDefinition::new("tags");

let db = Database::create("multimap.redb")?;

let write_txn = db.begin_write()?;
{
    let mut table = write_txn.open_multimap_table(TAGS)?;
    table.insert("post1", "rust")?;
    table.insert("post1", "database")?;
    table.insert("post1", "embedded")?;
    table.insert("post2", "rust")?;
}
write_txn.commit()?;

let read_txn = db.begin_read()?;
let table = read_txn.open_multimap_table(TAGS)?;

// Get all values for a key (values in ascending order)
for result in table.get("post1")? {
    let guard = result?;
    println!("tag: {}", guard.value());
}
```

## Built-in key/value types

Both `Key` and `Value`: `()`, `bool`, `char`, `u8`-`u128`, `i8`-`i128`, `&str`,
`String`, `&[u8]`, `[T; N]`, `Option<T>`, tuples up to 12 elements.

`Value` only (no ordering): `f32`, `f64`.

With feature flags: `chrono` types (`chrono_v0_4`), `uuid::Uuid` (`uuid`).

### Tuple keys for compound indexes

```rust
const TABLE: TableDefinition<(&str, u64), &[u8]> = TableDefinition::new("compound_keys");
```

## Advanced usage

### Custom key/value types with derive macros

```rust
use redb::{Database, ReadableTable, TableDefinition};
use redb_derive::{Key, Value};

#[derive(Debug, Key, Value, PartialEq, Eq, PartialOrd, Ord, Clone)]
struct SomeKey {
    foo: String,
    bar: i32,
}

const TABLE: TableDefinition<SomeKey, u64> = TableDefinition::new("my_data");

let db = Database::create("derived.redb")?;
let key = SomeKey { foo: "example".to_string(), bar: 42 };

let write_txn = db.begin_write()?;
{
    let mut table = write_txn.open_table(TABLE)?;
    table.insert(key.clone(), 0)?;
}
write_txn.commit()?;
```

### Durability settings

For bulk loading, skip fsync on intermediate transactions:

```rust
use redb::Durability;

// Fast writes without fsync
let mut write_txn = db.begin_write()?;
write_txn.set_durability(Durability::None)?;
{
    let mut table = write_txn.open_table(TABLE)?;
    for i in 0..1000u64 {
        table.insert(i, i * 2)?;
    }
}
write_txn.commit()?; // fast, no fsync

// Flush everything to disk with a durable commit
let write_txn = db.begin_write()?;
// Durability::Immediate is the default
write_txn.commit()?; // fsyncs, ensuring all previous writes are persistent
```

### Savepoints

Ephemeral savepoints exist only as long as the `Savepoint` object:

```rust
let mut write_txn = db.begin_write()?;
let savepoint = write_txn.ephemeral_savepoint()?;
{
    let mut table = write_txn.open_table(TABLE)?;
    table.insert("key", &123)?;
}
// Roll back
write_txn.restore_savepoint(&savepoint)?;
write_txn.commit()?; // commits the pre-savepoint state
```

Persistent savepoints survive across transactions and process restarts:

```rust
let write_txn = db.begin_write()?;
let savepoint_id = write_txn.persistent_savepoint()?;
write_txn.commit()?;

// Later, restore to that point
let mut write_txn = db.begin_write()?;
let savepoint = write_txn.get_persistent_savepoint(savepoint_id)?;
write_txn.restore_savepoint(&savepoint)?;
write_txn.commit()?;
```

### In-memory backend for testing

```rust
use redb::{Builder, backends::InMemoryBackend};

let backend = InMemoryBackend::new();
let db = Builder::new().create_with_backend(backend)?;
```

### Multithreaded access

Wrap the `Database` in `Arc` for sharing across threads:

```rust
use std::sync::Arc;

let db = Arc::new(Database::create("my_db.redb")?);

let db_clone = db.clone();
std::thread::spawn(move || {
    let read_txn = db_clone.begin_read().unwrap();
    let table = read_txn.open_table(TABLE).unwrap();
    // ...
});
```

Multiple readers run concurrently. Writers serialize automatically (`begin_write` blocks
until the previous write transaction completes).

### Compaction

After many writes and deletes, reclaim space. Note: `compact()` takes `&mut self`.

```rust
let mut db = Database::create("my_db.redb")?;
let compacted = db.compact()?;
println!("Compaction performed: {compacted}");
```

### Table management

```rust
let write_txn = db.begin_write()?;

// List all tables
for table_handle in write_txn.list_tables()? {
    println!("Table: {}", table_handle.name());
}

// Delete a table (pass a TableDefinition)
write_txn.delete_table(TABLE)?;

// Rename a table (both arguments must implement the sealed TableHandle trait,
// e.g. TableDefinition or UntypedTableHandle from list_tables())
const OLD_TABLE: TableDefinition<&str, u64> = TableDefinition::new("old_name");
const NEW_TABLE: TableDefinition<&str, u64> = TableDefinition::new("new_name");
write_txn.rename_table(OLD_TABLE, NEW_TABLE)?;

write_txn.commit()?;
```

## Error types

The main error enum is `Error`, which encompasses all error variants. More specific types:

- `StorageError` -- I/O errors, corruption, value too large
- `TableError` -- type mismatches, table doesn't exist, table already open
- `DatabaseError` -- opening/creating databases
- `TransactionError` -- beginning transactions
- `CommitError` -- committing transactions
- `CompactionError` -- compaction failures
- `SavepointError` -- invalid savepoint operations

## Common patterns

1. **Scope-based table lifetime**: Always open tables in a block scope so they drop
   before `commit()`.

2. **`Arc<Database>` for sharing**: Wrap in `Arc` for multi-threaded access.

3. **Snapshot reads**: `begin_read()` captures a snapshot. Concurrent writes don't
   affect it.

4. **Batch writes with `Durability::None`**: Use for bulk loading, do a final
   `Durability::Immediate` commit to flush.

5. **`TableDefinition` as constants**: Always define tables as `const` at module level.

6. **In-memory backend for tests**: Use `InMemoryBackend` for fast unit tests.

7. **Savepoints for atomic multi-step operations**: Create a savepoint before a complex
   sequence, restore it if any step fails.

## Role in Lobster

In Lobster, redb is the **canonical source of truth** for all deterministic data: raw
hook events, episode records, summary artifacts, task records, decision records, accepted
extraction/embedding artifacts, processing jobs, and retry state.

The canonical rule: if Grafeo or any retrieval index is lost, Lobster must be able to
rebuild semantic state from redb alone.

Key redb tables in Lobster will store:

- Raw events (append-only, keyed by sequence number)
- Episode records with processing state (`Pending`, `Ready`, `RetryQueued`, `FailedFinal`)
- Summary, extraction, and embedding artifacts (versioned, checksummed)
- Entity canonicalization metadata
- Provenance/evidence refs
- Retrieval statistics and operational config
