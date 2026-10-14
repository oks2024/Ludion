//! Byte-level snapshot test for the `dump-graph` JSON — CPU only, no GPU.
//!
//! The minimal chain's graph is a fixed thirteen-pass pipeline whose structure depends only on the
//! resolution, so its JSON dump is byte-stable and safe to pin. If the graph's passes, resources,
//! scheduling, or aliasing change, this fails and the snapshot must be re-blessed deliberately.
//!
//! The scene-derived companions below cover the other direction: `dump-graph` on a scene that
//! carries UI or a sun must report *that* scene's shape, not the minimal graph's.

use std::path::PathBuf;

use ludion_engine::QualitySetting;
use ludion_renderer::{FrameSize, PostChain};

/// The committed canonical output at 64×64. `.gitattributes` forces LF, matching what the writer
/// produces.
const SNAPSHOT: &str = include_str!("snapshots/frame_graph_64x64.json");

fn gallery(scene: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery")
        .join(scene)
}

/// The scheduled pass names of a `dump-graph` payload, in execution order.
fn scheduled_passes(json: &str) -> Vec<String> {
    let value: serde_json::Value = serde_json::from_str(json).expect("dump-graph emits valid JSON");
    value["schedule"]
        .as_array()
        .expect("the dump carries a schedule array")
        .iter()
        .map(|name| name.as_str().expect("a pass name is a string").to_string())
        .collect()
}

#[test]
fn minimal_chain_dump_graph_matches_committed_snapshot() {
    let json =
        ludion_engine::dump_frame_graph_json(FrameSize::native(64, 64), PostChain::default())
            .expect("the frame graph always schedules");
    assert_eq!(
        json, SNAPSHOT,
        "dump-graph JSON drifted from the committed snapshot; if intended, regenerate \
         crates/ludion-engine/tests/snapshots/frame_graph_64x64.json",
    );
}

/// A UI-free, sunless scene dumps exactly the pinned minimal graph — the scene-derived path and the
/// snapshot above agree by construction, not by coincidence.
#[test]
fn a_bare_scene_dumps_the_minimal_graph() {
    let json = ludion_engine::dump_scene_graph_json(
        &gallery("meshes.yaml"),
        FrameSize::native(64, 64),
        &QualitySetting::full(),
    )
    .expect("the gallery scene validates and schedules");
    assert_eq!(json, SNAPSHOT);
}

/// A scene carrying screen-space UI dumps its real shape: `composite` into the `ldr` transient,
/// then `overlay`.
#[test]
fn a_ui_scene_dumps_the_overlay_pass() {
    let json = ludion_engine::dump_scene_graph_json(
        &gallery("ui-hud.yaml"),
        FrameSize::native(64, 64),
        &QualitySetting::full(),
    )
    .expect("the gallery scene validates and schedules");
    assert_eq!(
        scheduled_passes(&json),
        [
            "scene",
            "bloom_threshold",
            "bloom_down_1",
            "bloom_down_2",
            "bloom_down_3",
            "bloom_down_4",
            "bloom_down_5",
            "bloom_up_4",
            "bloom_up_3",
            "bloom_up_2",
            "bloom_up_1",
            "bloom_up_0",
            "composite",
            "overlay",
        ],
    );
}

/// A scene carrying `GroundMarker` telegraphs dumps the unchanged minimal graph: the marker pipeline
/// draws inside the `scene` pass (an accepted deviation from a dedicated pass — see
/// `ludion_renderer::graph::ground_markers_leave_the_graph_untouched`), so the schedule the CLI
/// surface reports is byte-identical to a marker-free scene, proving golden-neutrality at `--dump-
/// graph`'s own level rather than only inside the renderer crate.
#[test]
fn a_ground_marker_scene_dumps_the_unchanged_minimal_graph() {
    let json = ludion_engine::dump_scene_graph_json(
        &gallery("ground-markers.yaml"),
        FrameSize::native(64, 64),
        &QualitySetting::full(),
    )
    .expect("the gallery scene validates and schedules");
    assert_eq!(
        json, SNAPSHOT,
        "a GroundMarker scene must not change the scheduled pass list",
    );
}

/// A scene carrying `ParticleEmitter`s dumps the unchanged minimal graph, for the ground marker's
/// reason: the particle pipeline draws inside the `scene` pass rather than in a pass of its own, so
/// three live emitters leave the schedule byte-identical to a particle-free scene. The renderer
/// crate proves the empty list issues no draw; this proves the populated one adds no pass.
#[test]
fn a_particle_scene_dumps_the_unchanged_minimal_graph() {
    let json = ludion_engine::dump_scene_graph_json(
        &gallery("particles.yaml"),
        FrameSize::native(64, 64),
        &QualitySetting::full(),
    )
    .expect("the gallery scene validates and schedules");
    assert_eq!(
        json, SNAPSHOT,
        "a ParticleEmitter scene must not change the scheduled pass list",
    );
}

/// A scene carrying a sun dumps its real six-pass shape: the depth-only `shadow` pass first, with
/// the `shadow_map` resource the scene pass samples.
#[test]
fn a_sun_scene_dumps_the_shadow_pass() {
    let json = ludion_engine::dump_scene_graph_json(
        &gallery("sun-shadows.yaml"),
        FrameSize::native(64, 64),
        &QualitySetting::full(),
    )
    .expect("the gallery scene validates and schedules");
    assert_eq!(
        scheduled_passes(&json),
        [
            "shadow",
            "scene",
            "bloom_threshold",
            "bloom_down_1",
            "bloom_down_2",
            "bloom_down_3",
            "bloom_down_4",
            "bloom_down_5",
            "bloom_up_4",
            "bloom_up_3",
            "bloom_up_2",
            "bloom_up_1",
            "bloom_up_0",
            "composite",
        ],
    );
    assert!(json.contains("shadow_map"), "the shadow map is declared");
}
