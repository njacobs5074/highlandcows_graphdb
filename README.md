# highlandcows_graphdb

A graph database built on top of [`highlandcows-isam`](https://njacobs5074.github.io/highlandcows/highlandcows_isam/index.html) — an ISAM (Indexed Sequential Access Method) store with ACID transactions.

This project is a **validation exercise**: prove that the ISAM library behaves correctly under the access patterns a production graph database would require, before committing to it in a larger project.

## Graph semantics

Nodes are connected based on **label co-membership**: if two nodes share a label, a bidirectional edge exists between them. Edges are derived automatically — callers never insert them directly. This is structurally a hypergraph projection: labels act as hyperedges, and the pairwise edge store is their projection onto a standard graph.

## Storage model

Three ISAM files are kept in sync for every mutation:

| Store | Key | Purpose |
|---|---|---|
| `nodes.idb` | `node_key` | Primary node records |
| `label_index.idb` | `(label, node_key)` | Inverted index — range-scan to find all nodes with a given label |
| `edges.idb` | `(from, to)` | Materialized adjacency list |

## API

```rust
// Lifecycle
GraphDb::create(path: &Path) -> IsamResult<Self>
GraphDb::open(path: &Path)   -> IsamResult<Self>

// Operations
fn add_node(&mut self, key: String, record: NodeRecord) -> IsamResult<()>
fn delete_node(&mut self, key: &str) -> IsamResult<()>
fn update_node(&mut self, key: &str, record: NodeRecord) -> IsamResult<()>
fn get_node_neighbors(&self, key: &str) -> IsamResult<Vec<String>>
fn is_reachable(&mut self, start: &str, end: &str) -> IsamResult<bool>
```

Nodes are keyed by a unique string. `IsamError::DuplicateKey` is returned on duplicate inserts. `IsamError::KeyNotFound` is returned when operating on a node that does not exist.

## Input format

Nodes can be represented as JSON for deserialization via `NodeInput`:

```json
[
  {
    "key": "Alice",
    "description": "Writes Rust compilers",
    "labels": ["Rust", "Compilers", "PL"]
  }
]
```

## Running the tests

The binary entry point is a placeholder. All functionality is exercised through the test suite:

```bash
cargo test
```

## ISAM API surface exercised

| Scenario | ISAM feature |
|---|---|
| Insert / update / delete nodes and edges | `insert`, `update`, `delete` |
| Find label co-members | `range` scan on inverted index |
| BFS graph traversal | Repeated `range` scans on edge store |
| Duplicate node / edge detection | `IsamError::DuplicateKey` |
| Idempotent edge deletion | `IsamError::KeyNotFound` |

## Known limitations and backlog

These items surfaced during the design session and are not yet addressed:

- **Cross-store atomicity** — logical operations span multiple independent per-store transactions. A `TransactionManager` abstraction is needed to provide atomicity across `Isam` instances.
- **Multi-valued secondary index** — the ISAM `DeriveKey` trait returns a single key, making it unsuitable for the label index where one node record must produce one index entry per label. The return type needs to become `Vec<Self::Key>`.
- **Commit safety** — there is no compile-time guarantee that a transaction is committed. A RAII `TransactionGuard` that warns or panics on drop without commit would prevent silent rollbacks.
- **`GraphDbError` type** — graph-level errors (database already exists, not found) currently reuse `IsamError::Io`, which is a pragmatic workaround. A proper `GraphDbError` wrapping `IsamError` with graph-specific variants would be cleaner.
