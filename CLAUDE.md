# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build                          # Build the project
cargo test                           # Run all tests
cargo test <name>                    # Run a single test by name
cargo clippy                         # Lint
```

## Architecture

This is a graph database library built on top of `highlandcows-isam`, an ISAM (Indexed Sequential Access Method) key-value store. The binary entry point is a placeholder — all meaningful functionality lives in the library and is exercised through the test suite in `graph.rs`.

### Storage model

`GraphDb` owns three ISAM stores on disk:

| Store | Key type | Value | Purpose |
|---|---|---|---|
| `nodes` | `String` | `NodeRecord` | Primary node store |
| `label_index` | `(String, String)` = `(label, node_id)` | `()` | Inverted index for label lookup |
| `edges` | `(String, String)` = `(from, to)` | `()` | Materialized adjacency list |

### Edge semantics

Edges are never inserted explicitly by callers. Two nodes are connected if and only if they share at least one label. `add_node`, `delete_node`, and `update_node` all maintain this invariant by calling `add_labels` / `remove_labels`, which scan the label index for co-members and insert or remove edges accordingly. `remove_edge_if_unconnected` checks `shares_any_label` before deleting an edge, so edges shared via multiple labels survive partial label removal.

### Ordering invariants

Both `delete_node` and `update_node` have non-obvious ordering constraints documented in their `///` doc comments:

- **`delete_node`**: the node record must be removed from the primary store *before* `remove_labels` is called, so that `shares_any_label` (called inside `remove_labels`) does not see the deleted node and incorrectly retain edges.
- **`update_node`**: the node record must be written with the *new* label set *before* `remove_labels` is called, so that labels present in both old and new sets are not incorrectly treated as removed connections.

### Range scans

The label index and edges store use inclusive range scans over `(prefix, "")..=(prefix, MAX_LABEL_VALUE)` to enumerate all entries under a given prefix. `MAX_LABEL_VALUE` (`\u{10FFFF}`, the highest Unicode scalar value) serves as the upper sentinel.

### Transactions

Every read or write wraps an explicit `begin_transaction` / `commit` pair. Transactions are short-lived and never shared across the three stores.

## Known open work

- `NodeInput` is defined in `types.rs` but none of its fields are read — it is likely a stub for a future deserialization / ingestion path.
- `GraphDb::open` is unused at the binary level (dead code warning) — the `create` / `open` split is in place but the open path has no caller yet.
