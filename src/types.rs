//! Shared types and constants for the graph database storage layer.
//!
//! This module defines the on-disk store names, sentinel values used in range
//! scans, and the serializable record types stored in each ISAM database.

use serde::{Deserialize, Serialize};

// Constants for our database names
pub const NODES_DB: &str = "nodes";
pub const NODES_DB_FILE: &str = "nodes.idb";
pub const LABEL_INDEX_DB: &str = "label_index";
pub const LABEL_INDEX_DB_FILE: &str = "label_index.idb";
pub const EDGES_DB: &str = "edges";
pub const EDGES_DB_FILE: &str = "edges.idb";

// Stub for a future JSON ingestion path — fields not yet consumed.
#[allow(dead_code)]
#[derive(Deserialize)]
pub struct NodeInput {
    key: String,
    description: String,
    labels: Vec<String>,
}

/// The largest valid Unicode scalar value, used as an inclusive upper bound for
/// range scans over composite `(prefix, key)` ISAM entries.
///
/// Because ISAM keys are sorted lexicographically, scanning
/// `(label, "")..=(label, MAX_LABEL_VALUE)` returns every entry whose first
/// component equals `label`, regardless of what the second component is.
pub const MAX_LABEL_VALUE: &str = "\u{10FFFF}";

#[derive(Serialize, Deserialize, Clone)]
pub struct NodeRecord {
    pub description: String,
    pub labels: Vec<String>,
}
