//! Byte-level snapshot test for `dump-graph` JSON — CPU only, no GPU required.
//!
//! `dump_graph_json` must be deterministic to the byte, so this pins the exact output of the fixed
//! proving graph at a known size against a committed snapshot. If the graph's structure, scheduling,
//! or aliasing changes, this test fails and the snapshot must be re-blessed deliberately.

use ludion_render_graph::{build_graph, schedule};

/// The committed canonical output. `include_str!` bakes it in with the repository's LF endings
/// (`.gitattributes` forces LF), matching what `dump_graph_json` produces.
const SNAPSHOT: &str = include_str!("snapshots/dump_graph_64x64.json");

#[test]
fn dump_graph_matches_committed_snapshot() {
    let (graph, _output) = build_graph(64, 64);
    let scheduled = schedule(&graph).expect("the proving graph is acyclic and fully written");
    let json = scheduled.dump_graph_json();
    assert_eq!(
        json, SNAPSHOT,
        "dump-graph JSON drifted from the committed snapshot; if this change is intended, \
         regenerate tests/snapshots/dump_graph_64x64.json",
    );
}
