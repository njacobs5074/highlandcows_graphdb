//! `highlandcows_graphdb` — label-based graph database.
//!
//! The binary entry point is intentionally minimal; all functionality is
//! exposed through the [`graph`] and [`types`] modules and exercised via the
//! test suite in `graph.rs`.

mod graph;
mod types;

fn main() {
    println!("Hello, GraphDb! For now, use the tests");
}
