//! Byte-level snapshot test for the `dump-graph` JSON — CPU only, no GPU.
//!
//! The minimal chain's graph is a fixed fifteen-pass pipeline whose structure depends only on the
//! resolution, so its JSON dump is byte-stable and safe to pin. If the graph's passes, resources,
//! scheduling, or aliasing change, this fails and the snapshot must be re-blessed deliberately.
//!
//! The scene-derived companions below cover the other direction: `dump-graph` on a scene that
//! carries UI or a sun must report *that* scene's shape, not the minimal graph's.

use std::path::PathBuf;

use ludion_engine::QualitySetting;
use ludion_renderer::{AntiAliasing, FrameSize, PostChain};

/// The committed canonical output at 64×64. `.gitattributes` forces LF, matching what the writer
/// produces.
const SNAPSHOT: &str = include_str!("snapshots/frame_graph_64x64.json");

/// The same graph for a scene with a world mesh: the GPU `cull` pass first, its three buffers at the
/// 256-instance / 16-bucket floor every small scene shares.
const CULL_SNAPSHOT: &str = include_str!("snapshots/frame_graph_64x64_cull.json");

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

/// The SMAA chain at 64×64: the minimal graph with the three AA passes behind `composite`.
const SMAA_SNAPSHOT: &str = include_str!("snapshots/frame_graph_64x64_smaa.json");

#[test]
fn smaa_chain_dump_graph_matches_committed_snapshot() {
    let json = ludion_engine::dump_frame_graph_json(
        FrameSize::native(64, 64),
        PostChain {
            aa: AntiAliasing::Smaa,
            ..PostChain::default()
        },
    )
    .expect("the frame graph always schedules");
    assert_eq!(
        json, SMAA_SNAPSHOT,
        "dump-graph JSON drifted from the committed snapshot; if intended, regenerate \
         crates/ludion-engine/tests/snapshots/frame_graph_64x64_smaa.json",
    );
}

/// A UI-free, sunless scene with meshes dumps exactly the pinned culled graph — the scene-derived path
/// and the snapshot agree by construction, not by coincidence. The same scene without the host's
/// culling dumps the minimal graph.
#[test]
fn a_bare_mesh_scene_dumps_the_culled_minimal_graph() {
    let json = ludion_engine::dump_scene_graph_json(
        &gallery("meshes.yaml"),
        FrameSize::native(64, 64),
        &QualitySetting::full(),
        AntiAliasing::Off,
    )
    .expect("the gallery scene validates and schedules");
    assert_eq!(json, CULL_SNAPSHOT);
}

/// A scene carrying screen-space UI dumps its real shape: `composite` into the `ldr` transient,
/// then `overlay`.
#[test]
fn a_ui_scene_dumps_the_overlay_pass() {
    let json = ludion_engine::dump_scene_graph_json(
        &gallery("ui-hud.yaml"),
        FrameSize::native(64, 64),
        &QualitySetting::full(),
        AntiAliasing::Off,
    )
    .expect("the gallery scene validates and schedules");
    assert_eq!(
        scheduled_passes(&json),
        [
            "cull",
            "scene",
            "reflections",
            "transparents",
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
        AntiAliasing::Off,
    )
    .expect("the gallery scene validates and schedules");
    assert_eq!(
        json, CULL_SNAPSHOT,
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
        AntiAliasing::Off,
    )
    .expect("the gallery scene validates and schedules");
    assert_eq!(
        json, CULL_SNAPSHOT,
        "a ParticleEmitter scene must not change the scheduled pass list",
    );
}

/// A scene carrying a sun dumps its real shape: the `cull` first, then the depth-only `shadow` pass it
/// feeds, with the `shadow_map` resource the scene pass samples.
#[test]
fn a_sun_scene_dumps_the_shadow_pass() {
    let json = ludion_engine::dump_scene_graph_json(
        &gallery("sun-shadows.yaml"),
        FrameSize::native(64, 64),
        &QualitySetting::full(),
        AntiAliasing::Off,
    )
    .expect("the gallery scene validates and schedules");
    assert_eq!(
        scheduled_passes(&json),
        [
            "cull",
            "shadow",
            "scene",
            "reflections",
            "transparents",
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

fn fixture(scene: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(scene)
}

/// A scene with a shadowed local light schedules `local_shadow` before `scene` and declares its
/// atlas; the cheapest quality removes both, and a scene that never opts in keeps the pre-feature
/// graph.
#[test]
fn a_shadowed_local_light_dumps_the_local_shadow_pass() {
    let dump = |path: PathBuf, setting: &QualitySetting| {
        ludion_engine::dump_scene_graph_json(
            &path,
            FrameSize::native(64, 64),
            setting,
            AntiAliasing::Off,
        )
        .expect("the scene validates and schedules")
    };
    let full = dump(fixture("local-shadows.yaml"), &QualitySetting::full());
    let passes = scheduled_passes(&full);
    let position = |name: &str| passes.iter().position(|pass| pass == name).unwrap();
    assert!(position("local_shadow") < position("scene"));
    assert!(full.contains("local_shadow_atlas"), "the atlas is declared");

    let low = dump(
        fixture("local-shadows.yaml"),
        &ludion_engine::resolve_startup_quality(Some("low"), None).unwrap(),
    );
    assert!(!scheduled_passes(&low).contains(&"local_shadow".to_string()));
    assert!(!low.contains("local_shadow_atlas"));

    let plain = dump(gallery("spot-lights.yaml"), &QualitySetting::full());
    assert!(!scheduled_passes(&plain).contains(&"local_shadow".to_string()));
    assert!(!plain.contains("local_shadow_atlas"));
}

/// A sun authoring `cascades` schedules the same passes as a plain sun, with `shadow_map` declared
/// as the cascade atlas: 2×2 tiles at full quality, 2×1 at medium (whose cap is two).
#[test]
fn a_cascaded_sun_dumps_the_atlas() {
    let dump = |setting: &QualitySetting| {
        ludion_engine::dump_scene_graph_json(
            &fixture("sun-cascades.yaml"),
            FrameSize::native(64, 64),
            setting,
            AntiAliasing::Off,
        )
        .expect("the scene validates and schedules")
    };
    let shadow_map_extent = |json: &str| {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let map = value["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|resource| resource["name"] == "shadow_map")
            .expect("the shadow map is declared")
            .clone();
        (
            map["width"].as_u64().unwrap(),
            map["height"].as_u64().unwrap(),
        )
    };
    let plain = ludion_engine::dump_scene_graph_json(
        &gallery("sun-shadows.yaml"),
        FrameSize::native(64, 64),
        &QualitySetting::full(),
        AntiAliasing::Off,
    )
    .unwrap();

    let full = dump(&QualitySetting::full());
    assert_eq!(scheduled_passes(&full), scheduled_passes(&plain));
    assert_eq!(shadow_map_extent(&full), (4096, 4096));

    let medium = dump(&ludion_engine::resolve_startup_quality(Some("medium"), None).unwrap());
    assert_eq!(shadow_map_extent(&medium), (2048, 1024));
}

/// A scene with a decal schedules the projector once, before `transparents`, with no drop-shadow
/// occlusion pass and no receiver pass for an unlisted decal, and the same at the cheapest quality
/// (a decal is content); a decal-free scene keeps the graph without it.
#[test]
fn a_decal_scene_dumps_one_projector_and_nothing_else() {
    let dump = |path: PathBuf, setting: &QualitySetting| {
        ludion_engine::dump_scene_graph_json(
            &path,
            FrameSize::native(64, 64),
            setting,
            AntiAliasing::Off,
        )
        .expect("the scene validates and schedules")
    };
    for setting in [
        QualitySetting::full(),
        ludion_engine::resolve_startup_quality(Some("low"), None).unwrap(),
    ] {
        let json = dump(fixture("decals.yaml"), &setting);
        let passes = scheduled_passes(&json);
        let count = |name: &str| passes.iter().filter(|pass| *pass == name).count();
        assert_eq!(count("projector"), 1, "{passes:?}");
        assert_eq!(count("drop_shadow_occlusion"), 0);
        assert_eq!(count("decal_receivers"), 0);
        let position = |name: &str| passes.iter().position(|pass| pass == name).unwrap();
        assert!(position("projector") < position("transparents"));
    }
    let plain = dump(gallery("spot-lights.yaml"), &QualitySetting::full());
    assert!(!scheduled_passes(&plain).contains(&"projector".to_string()));
}

/// A scene with an alpha decal schedules `decals_lit` once, between `scene` and `reflections`, with
/// no projector (nothing multiplies or adds), and the same at the cheapest quality.
#[test]
fn a_lit_decal_scene_dumps_decals_lit_and_no_projector() {
    for setting in [
        QualitySetting::full(),
        ludion_engine::resolve_startup_quality(Some("low"), None).unwrap(),
    ] {
        let json = ludion_engine::dump_scene_graph_json(
            &fixture("decals-lit.yaml"),
            FrameSize::native(64, 64),
            &setting,
            AntiAliasing::Off,
        )
        .expect("the scene validates and schedules");
        let passes = scheduled_passes(&json);
        let count = |name: &str| passes.iter().filter(|pass| *pass == name).count();
        assert_eq!(count("decals_lit"), 1, "{passes:?}");
        assert_eq!(count("projector"), 0);
        assert_eq!(count("decal_receivers"), 0);
        let position = |name: &str| passes.iter().position(|pass| pass == name).unwrap();
        assert!(position("scene") < position("decals_lit"));
        assert!(position("decals_lit") < position("reflections"));
    }
}

/// A host's anti-aliasing mode schedules the three SMAA passes directly after `composite`, last, at
/// every preset (no quality masks it); `Off` is the pinned graph.
#[test]
fn a_host_aa_mode_dumps_the_smaa_passes_after_composite() {
    for setting in [
        QualitySetting::full(),
        ludion_engine::resolve_startup_quality(Some("low"), None).unwrap(),
    ] {
        let dump = |mode| {
            ludion_engine::dump_scene_graph_json(
                &gallery("meshes.yaml"),
                FrameSize::native(64, 64),
                &setting,
                mode,
            )
            .expect("the gallery scene validates and schedules")
        };
        let passes = scheduled_passes(&dump(AntiAliasing::Smaa));
        let composite = passes.iter().position(|pass| pass == "composite").unwrap();
        assert_eq!(
            passes[composite + 1..],
            ["smaa_edges", "smaa_weights", "smaa_blend"],
            "{passes:?}"
        );
        assert!(!scheduled_passes(&dump(AntiAliasing::Off)).contains(&"smaa_edges".to_string()));
    }
    let off = ludion_engine::dump_scene_graph_json(
        &gallery("meshes.yaml"),
        FrameSize::native(64, 64),
        &QualitySetting::full(),
        AntiAliasing::Off,
    )
    .unwrap();
    assert_eq!(off, CULL_SNAPSHOT);
}

/// `ParticleEmitter.lit` leaves the schedule alone: absent, `lit: false` and `lit: true` dump
/// byte-identical graphs, because the light set is bound inside the existing `scene` pass.
#[test]
fn a_lit_particle_scene_dumps_the_same_graph_as_an_unlit_one() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("lit-particle-graph-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dump = |name: &str, lit_line: &str| {
        let scene = format!(
            "format: 1\nentities:\n  - name: camera\n    components:\n      Camera: {{}}\n  \
             - name: lamp\n    components:\n      PointLight: {{}}\n  - name: motes\n    \
             components:\n      ParticleEmitter:\n        rate: 20.0\n{lit_line}"
        );
        let path = dir.join(format!("{name}.yaml"));
        std::fs::write(&path, scene).unwrap();
        ludion_engine::dump_scene_graph_json(
            &path,
            FrameSize::native(64, 64),
            &QualitySetting::full(),
            AntiAliasing::Off,
        )
        .expect("the scene validates and schedules")
    };
    let absent = dump("absent", "");
    assert_eq!(absent, dump("off", "        lit: false\n"));
    assert_eq!(absent, dump("on", "        lit: true\n"));
    assert_eq!(
        absent, SNAPSHOT,
        "a lit emitter must not change the pass list"
    );
}

/// A fogged scene with a sprite schedules `transparent_depth` right after `transparents`, with its
/// `rg16float` target declared; the same scene without the sprite, and the fog-free gallery scene
/// with sprites, keep the layer-free graph.
#[test]
fn a_fogged_sprite_scene_dumps_the_transparent_layer() {
    let dump = |path: PathBuf| {
        ludion_engine::dump_scene_graph_json(
            &path,
            FrameSize::native(64, 64),
            &QualitySetting::full(),
            AntiAliasing::Off,
        )
        .expect("the scene validates and schedules")
    };
    let fogged = dump(fixture("fog-sprite.yaml"));
    let passes = scheduled_passes(&fogged);
    let position = |name: &str| passes.iter().position(|pass| pass == name).unwrap();
    assert_eq!(
        position("transparent_depth"),
        position("transparents") + 1,
        "the layer is redrawn right after the transparents",
    );
    assert!(position("transparent_depth") < position("composite"));
    assert!(fogged.contains("rg16float"), "the target is declared");

    for plain in [
        dump(gallery("sprites.yaml")),
        dump(gallery("post-full.yaml")),
    ] {
        assert!(!scheduled_passes(&plain).contains(&"transparent_depth".to_string()));
        assert!(!plain.contains("rg16float"));
    }
}
