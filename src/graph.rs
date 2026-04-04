//! Core graph database implementation.
//!
//! [`GraphDb`] stores nodes in three ISAM files: a primary node store, an
//! inverted label index, and a materialized edges store. Edges are never
//! inserted explicitly — they are derived automatically from label
//! co-membership. Two nodes are connected if and only if they share at least
//! one label; the edges store is kept consistent whenever labels change.

use crate::types::{
    EDGES_DB, EDGES_DB_FILE, LABEL_INDEX_DB, LABEL_INDEX_DB_FILE, MAX_LABEL_VALUE, NODES_DB,
    NODES_DB_FILE, NodeRecord,
};
use highlandcows_isam::{Isam, IsamError, IsamResult};
use std::{
    collections::{HashSet, VecDeque},
    path::Path,
};

pub struct GraphDb {
    nodes: Isam<String, NodeRecord>,
    label_index: Isam<(String, String), ()>,
    edges: Isam<(String, String), ()>,
}

// Public methods
impl GraphDb {
    /// Creates a new graph database at `path`.
    ///
    /// Returns [`IsamError::Io`] with `AlreadyExists` if the database files
    /// are already present at that location.
    pub fn create(path: &Path) -> IsamResult<Self> {
        if GraphDb::db_exists(path) {
            return Err(IsamError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Graph database already exists",
            )));
        }

        Ok(Self {
            nodes: Isam::create(path.join(NODES_DB))?,
            label_index: Isam::create(path.join(LABEL_INDEX_DB))?,
            edges: Isam::create(path.join(EDGES_DB))?,
        })
    }

    /// Opens an existing graph database at `path`.
    ///
    /// Returns [`IsamError::Io`] with `NotFound` if the expected database
    /// files are absent.
    pub fn open(path: &Path) -> IsamResult<Self> {
        if !GraphDb::db_exists(path) {
            return Err(IsamError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Graph database files not found",
            )));
        }

        Ok(Self {
            nodes: Isam::open(path.join(NODES_DB))?,
            label_index: Isam::open(path.join(LABEL_INDEX_DB))?,
            edges: Isam::open(path.join(EDGES_DB))?,
        })
    }

    /// Inserts a new node and materializes edges to all nodes that share at
    /// least one label with it.
    pub fn add_node(&mut self, key: String, record: NodeRecord) -> IsamResult<()> {
        let nodes = &self.nodes;
        nodes.write(|txn| nodes.insert(txn, key.clone(), &record))?;
        self.add_labels(&key, &record.labels)
    }

    /// Removes a node and all edges incident to it.
    ///
    /// # Ordering invariant
    ///
    /// The node is removed from the primary store **before** `remove_labels` is
    /// called. `remove_labels` may call `shares_any_label`, which reads node
    /// records to decide whether to keep an edge. If the deleted node were
    /// still present at that point, `shares_any_label` could incorrectly
    /// conclude that a co-member is still connected and leave a dangling edge.
    ///
    /// Do not reorder the node-store deletion and the label-removal steps.
    pub fn delete_node(&mut self, key: &str) -> IsamResult<()> {
        // Get the node's labels before deleting it. We'll need these later.
        let record = {
            let nodes = &self.nodes;
            nodes.read(|txn| nodes.get(txn, &key.to_string())?.ok_or(IsamError::KeyNotFound))?
        };

        // Delete from the node store first — see ordering invariant above.
        let nodes = &self.nodes;
        nodes.write(|txn| nodes.delete(txn, &key.to_string()))?;

        self.remove_labels(key, &record.labels)
    }

    /// Returns the keys of all nodes directly connected to `key`.
    ///
    /// Connectivity is determined by the materialized edges store; a node
    /// appears here if it shares at least one label with `key`. Returns an
    /// empty `Vec` (not an error) if `key` has no neighbors or does not exist.
    pub fn get_node_neighbors(&self, key: &str) -> IsamResult<Vec<String>> {
        let start = (key.to_string(), String::new());
        let end = (key.to_string(), String::from(MAX_LABEL_VALUE));
        let edges = &self.edges;
        edges.read(|txn| {
            Ok(edges
                .range(txn, start..=end)?
                .filter_map(|r| r.ok())
                .map(|(k, _)| k.1)
                .collect())
        })
    }

    /// Replaces the record for an existing node and reconciles edges.
    ///
    /// Labels added by the update cause new edges to be materialized; labels
    /// removed cause edges to be deleted (unless another shared label keeps
    /// the connection alive). Returns [`IsamError::KeyNotFound`] if `key` does
    /// not exist.
    ///
    /// # Ordering invariant
    ///
    /// The node record is written to the primary store **before** label
    /// changes are applied. `remove_labels` calls `shares_any_label`, which
    /// reads the current node record. The record must reflect the new label set
    /// at that point so that edges shared via a label present in *both* the old
    /// and new sets are not incorrectly removed.
    pub fn update_node(&mut self, key: &str, record: NodeRecord) -> IsamResult<()> {
        let old_record = {
            let nodes = &self.nodes;
            nodes.read(|txn| nodes.get(txn, &key.to_string())?.ok_or(IsamError::KeyNotFound))?
        };

        let old_labels: HashSet<&String> = old_record.labels.iter().collect();
        let new_labels: HashSet<&String> = record.labels.iter().collect();

        let removed_labels: Vec<String> = old_labels
            .difference(&new_labels)
            .map(|s| s.to_string())
            .collect();

        let added_labels: Vec<String> = new_labels
            .difference(&old_labels)
            .map(|s| s.to_string())
            .collect();

        // Write new record first — see ordering invariant above.
        let nodes = &self.nodes;
        nodes.write(|txn| nodes.update(txn, key.to_string(), &record))?;

        self.remove_labels(key, &removed_labels)?;
        self.add_labels(key, &added_labels)
    }

    /// Returns `true` if `end` is reachable from `start` by traversing
    /// materialized edges (BFS). A node is always reachable from itself.
    ///
    /// Returns [`IsamError::KeyNotFound`] if either `start` or `end` does not
    /// exist in the database.
    pub fn is_reachable(&self, start: &str, end: &str) -> IsamResult<bool> {
        {
            let nodes = &self.nodes;
            nodes.read(|txn| {
                nodes.get(txn, &start.to_string())?.ok_or(IsamError::KeyNotFound)?;
                nodes.get(txn, &end.to_string())?.ok_or(IsamError::KeyNotFound)?;
                Ok(())
            })?;
        }

        if start == end {
            return Ok(true);
        }

        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        queue.push_back(start.to_string());
        visited.insert(start.to_string());

        while let Some(current) = queue.pop_front() {
            let neighbors = self.get_node_neighbors(&current)?;
            for neighbor in neighbors {
                if neighbor == end {
                    return Ok(true);
                }

                if !visited.contains(&neighbor) {
                    visited.insert(neighbor.clone());
                    queue.push_back(neighbor);
                }
            }
        }

        Ok(false)
    }
}

// Private methods
impl GraphDb {
    fn db_exists(path: &Path) -> bool {
        path.join(NODES_DB_FILE).exists()
            && path.join(LABEL_INDEX_DB_FILE).exists()
            && path.join(EDGES_DB_FILE).exists()
    }

    fn ignore_if<T>(result: IsamResult<T>, is_ignorable: impl Fn(&IsamError) -> bool) -> IsamResult<()> {
        match result {
            Ok(_) => Ok(()),
            Err(e) if is_ignorable(&e) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn add_labels(&mut self, key: &str, labels: &[String]) -> IsamResult<()> {
        let label_index = &self.label_index;
        label_index.write(|txn| {
            for label in labels {
                label_index.insert(txn, (label.clone(), key.to_string()), &())?;
            }
            Ok(())
        })?;

        for label in labels {
            let co_members: Vec<String> = {
                let label_index = &self.label_index;
                let start = (label.clone(), String::new());
                let end = (label.clone(), String::from(MAX_LABEL_VALUE));
                label_index.read(|txn| {
                    Ok(label_index
                        .range(txn, start..=end)?
                        .filter_map(|r| r.ok())
                        .map(|(k, _)| k.1)
                        .filter(|k| *k != key)
                        .collect())
                })?
            };

            let edges = &self.edges;
            edges.write(|txn| {
                for co_member in &co_members {
                    // Insert edges in both directions, ignoring duplicates
                    Self::ignore_if(
                        edges.insert(txn, (key.to_string(), co_member.clone()), &()),
                        |e| matches!(e, IsamError::DuplicateKey),
                    )?;
                    Self::ignore_if(
                        edges.insert(txn, (co_member.clone(), key.to_string()), &()),
                        |e| matches!(e, IsamError::DuplicateKey),
                    )?;
                }
                Ok(())
            })?;
        }

        Ok(())
    }

    fn remove_labels(&mut self, key: &str, labels: &[String]) -> IsamResult<()> {
        let label_index = &self.label_index;
        label_index.write(|txn| {
            for label in labels {
                label_index.delete(txn, &(label.clone(), key.to_string()))?;
            }
            Ok(())
        })?;

        for label in labels {
            let co_members: Vec<String> = {
                let label_index = &self.label_index;
                let start = (label.clone(), String::new());
                let end = (label.clone(), MAX_LABEL_VALUE.to_string());
                label_index.read(|txn| {
                    Ok(label_index
                        .range(txn, start..=end)?
                        .filter_map(|r| r.ok())
                        .map(|(k, _)| k.1)
                        .filter(|k| k.as_str() != key)
                        .collect())
                })?
            };

            for co_member in co_members {
                self.remove_edge_if_unconnected(key, &co_member)?;
            }
        }

        Ok(())
    }

    fn remove_edge_if_unconnected(&mut self, key: &str, co_member: &str) -> IsamResult<()> {
        if self.shares_any_label(key, co_member)? {
            Ok(())
        } else {
            let edges = &self.edges;
            edges.write(|txn| {
                // Delete edges in both directions, ignoring missing keys
                Self::ignore_if(
                    edges.delete(txn, &(key.to_string(), co_member.to_string())),
                    |e| matches!(e, IsamError::KeyNotFound),
                )?;
                Self::ignore_if(
                    edges.delete(txn, &(co_member.to_string(), key.to_string())),
                    |e| matches!(e, IsamError::KeyNotFound),
                )?;
                Ok(())
            })
        }
    }

    fn shares_any_label(&self, key: &str, other: &str) -> IsamResult<bool> {
        let nodes = &self.nodes;
        let (key_record, other_record) = nodes.read(|txn| {
            let key_opt = nodes.get(txn, &key.to_string())?;
            let other = nodes
                .get(txn, &other.to_string())?
                .ok_or(IsamError::KeyNotFound)?;
            Ok((key_opt, other))
        })?;

        let Some(key_record) = key_record else {
            return Ok(false);
        };

        let key_labels: HashSet<&String> = key_record.labels.iter().collect();
        let other_labels: HashSet<&String> = other_record.labels.iter().collect();

        Ok(!key_labels.is_disjoint(&other_labels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_add_node_creates_edges() {
        let dir = TempDir::new().unwrap();
        let mut db = GraphDb::create(dir.path()).unwrap();

        db.add_node(
            "Alice".to_string(),
            NodeRecord {
                description: "Writes Rust compilers".to_string(),
                labels: vec!["Rust".to_string(), "PL".to_string()],
            },
        )
        .unwrap();

        db.add_node(
            "Bob".to_string(),
            NodeRecord {
                description: "Designs programming languages".to_string(),
                labels: vec!["Rust".to_string(), "PL".to_string()],
            },
        )
        .unwrap();

        let alice_neighbors = db.get_node_neighbors("Alice").unwrap();
        assert!(alice_neighbors.contains(&"Bob".to_string()));

        let bob_neighbors = db.get_node_neighbors("Bob").unwrap();
        assert!(bob_neighbors.contains(&"Alice".to_string()));
    }

    #[test]
    fn test_delete_node() {
        let dir = TempDir::new().unwrap();
        let mut db = GraphDb::create(dir.path()).unwrap();

        db.add_node(
            "Alice".to_string(),
            NodeRecord {
                description: "Writes Rust compilers".to_string(),
                labels: vec!["Rust".to_string(), "PL".to_string()],
            },
        )
        .unwrap();

        db.add_node(
            "Bob".to_string(),
            NodeRecord {
                description: "Designs programming languages".to_string(),
                labels: vec!["Rust".to_string(), "PL".to_string()],
            },
        )
        .unwrap();

        db.delete_node("Alice").unwrap();

        let alice_neighbors = db.get_node_neighbors("Alice").unwrap();
        assert!(alice_neighbors.is_empty());

        let bob_neighbors = db.get_node_neighbors("Bob").unwrap();
        assert!(!bob_neighbors.contains(&"Alice".to_string()));

        let result = db.delete_node("Alice");
        assert!(matches!(result, Err(IsamError::KeyNotFound)));
    }

    #[test]
    fn test_update_node() {
        let dir = TempDir::new().unwrap();
        let mut db = GraphDb::create(dir.path()).unwrap();

        db.add_node(
            "Alice".to_string(),
            NodeRecord {
                description: "Writes Rust compilers".to_string(),
                labels: vec!["Rust".to_string(), "PL".to_string()],
            },
        )
        .unwrap();

        db.add_node(
            "Bob".to_string(),
            NodeRecord {
                description: "Designs programming languages".to_string(),
                labels: vec!["Rust".to_string(), "PL".to_string()],
            },
        )
        .unwrap();

        db.add_node(
            "Carol".to_string(),
            NodeRecord {
                description: "Builds distributed systems".to_string(),
                labels: vec!["Systems".to_string()],
            },
        )
        .unwrap();

        db.update_node(
            "Alice",
            NodeRecord {
                description: "Writes Rust compilers".to_string(),
                labels: vec!["PL".to_string(), "Systems".to_string()],
            },
        )
        .unwrap();

        // Alice and Bob still connected via "PL"
        let alice_neighbors = db.get_node_neighbors("Alice").unwrap();
        assert!(alice_neighbors.contains(&"Bob".to_string()));

        // Alice and Carol now connected via "Systems"
        assert!(alice_neighbors.contains(&"Carol".to_string()));

        // Alice and Bob no longer connected via "Rust" only — but still via "PL"
        let bob_neighbors = db.get_node_neighbors("Bob").unwrap();
        assert!(bob_neighbors.contains(&"Alice".to_string()));

        // Carol and Alice now connected
        let carol_neighbors = db.get_node_neighbors("Carol").unwrap();
        assert!(carol_neighbors.contains(&"Alice".to_string()));

        // Updating a non-existent node should return KeyNotFound
        let result = db.update_node(
            "Dave",
            NodeRecord {
                description: "Does not exist".to_string(),
                labels: vec![],
            },
        );
        assert!(matches!(result, Err(IsamError::KeyNotFound)));
    }

    #[test]
    fn test_is_reachable() {
        let dir = TempDir::new().unwrap();
        let mut db = GraphDb::create(dir.path()).unwrap();

        // Alice and Bob share "Rust"
        // Bob and Carol share "Systems"
        // Dave is isolated
        db.add_node(
            "Alice".to_string(),
            NodeRecord {
                description: "Writes Rust compilers".to_string(),
                labels: vec!["Rust".to_string()],
            },
        )
        .unwrap();

        db.add_node(
            "Bob".to_string(),
            NodeRecord {
                description: "Designs programming languages".to_string(),
                labels: vec!["Rust".to_string(), "Systems".to_string()],
            },
        )
        .unwrap();

        db.add_node(
            "Carol".to_string(),
            NodeRecord {
                description: "Builds distributed systems".to_string(),
                labels: vec!["Systems".to_string()],
            },
        )
        .unwrap();

        db.add_node(
            "Dave".to_string(),
            NodeRecord {
                description: "Works alone".to_string(),
                labels: vec!["Isolated".to_string()],
            },
        )
        .unwrap();

        assert!(db.is_reachable("Alice", "Alice").unwrap());
        assert!(db.is_reachable("Alice", "Bob").unwrap());
        assert!(db.is_reachable("Alice", "Carol").unwrap());
        assert!(!db.is_reachable("Alice", "Dave").unwrap());
        assert!(db.is_reachable("Carol", "Alice").unwrap());

        let result = db.is_reachable("Alice", "Nobody");
        assert!(matches!(result, Err(IsamError::KeyNotFound)));
    }
}
