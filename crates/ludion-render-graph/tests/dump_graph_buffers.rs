//! Byte-level snapshot of `dump-graph` JSON for a graph with buffers — CPU only.
//!
//! The fixture is cull-shaped: one compute pass writes three buffers, a prepass and the scene pass
//! read two of them. Graphs without buffers keep their old bytes (`dump_graph_snapshot.rs`).

use ludion_render_graph::{BufferDesc, BufferUsage, RenderGraph, TextureDesc, schedule};

const SNAPSHOT: &str = include_str!("snapshots/dump_graph_buffers.json");

#[test]
fn dump_graph_with_buffers_matches_committed_snapshot() {
    let mut graph = RenderGraph::new();
    let visibility = graph.buffer(
        "visibility",
        BufferDesc::new(4096, &[BufferUsage::Storage, BufferUsage::CopyDst]),
    );
    let instances = graph.buffer(
        "instances_out",
        BufferDesc::new(65536, &[BufferUsage::Storage, BufferUsage::Vertex]),
    );
    let indirect = graph.buffer(
        "indirect_args",
        BufferDesc::new(20, &[BufferUsage::Storage, BufferUsage::Indirect]),
    );
    let depth = graph.transient("depth", TextureDesc::sampled_depth(64, 64));
    let output = graph.output("final", TextureDesc::present_target(64, 64));

    // The producer is declared last so the snapshot shows the buffer edge ordering it first.
    let prepass = graph.add_external_pass_with_depth("prepass", "prepass", vec![], vec![], depth);
    let scene = graph.add_external_pass_with_depth("scene", "scene", vec![], vec![output], depth);
    graph.read_buffers(prepass, &[instances, indirect]);
    graph.read_buffers(scene, &[instances, indirect]);
    graph.add_external_buffer_compute_pass(
        "cull",
        "cull",
        vec![],
        vec![visibility, instances, indirect],
    );

    let json = schedule(&graph)
        .expect("the buffer graph schedules")
        .dump_graph_json();
    assert_eq!(
        json, SNAPSHOT,
        "dump-graph JSON with buffers drifted; regenerate tests/snapshots/dump_graph_buffers.json \
         if intended",
    );
}
