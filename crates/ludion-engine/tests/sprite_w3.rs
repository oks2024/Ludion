//! Runtime proof for the Wave 3 sprite/camera capabilities (`Plan/sprite-engine.md` decisions 3–7):
//! `layer`, `blend`, `flip_x`/`flip_y`, pivot-aware rotation, hit-flash, and `pixel_snap`.
//!
//! Two families, pulling in opposite directions (the `sprite_flipbook.rs` shape):
//!
//! - **Determinism boundary.** The *authored* fields on `Sprite` (`flip_x`, `layer`, `flash`,
//!   `blend`) are ordinary component data, so toggling one moves the `WorldHash` exactly as any
//!   component-field change does. The *derived* work (the flipped UV rect, the pivot offset, the
//!   snapped centre and view, the sampler choice) all happens at extract and is written back
//!   nowhere — so a region's pivot lives entirely outside the hash, and extracting under `pixel_snap`
//!   never perturbs a single world hash.
//! - **The extract actually runs.** Flip mirrors the UV rect, a non-centre pivot offsets the quad
//!   centre off the entity translation, `pixel_snap` snaps both the centre and the projection, and
//!   `blend`/`flash` reach the `SpriteInstance`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, drive_hashes, extract_at};
use ludion_protocol::{Command, command};
use ludion_renderer::SpriteInstance;
use ludion_scene::{AtlasDocument, AtlasFilter, AtlasRegion};

const ATLAS_WIDTH: u32 = 8;
const ATLAS_HEIGHT: u32 = 8;
/// Four texels per world unit, so each 4×4 region is exactly one world unit square.
const PIXELS_PER_UNIT: f64 = 4.0;

/// The fixture atlas: a centre-pivoted region and a top-left-corner-pivoted one of the same size, so
/// the pivot is the only thing that differs between them.
fn doc(corner_pivot: [f64; 2], filter: AtlasFilter) -> AtlasDocument {
    let mut regions = BTreeMap::new();
    regions.insert(
        "mid".to_string(),
        AtlasRegion {
            rect: [0, 0, 4, 4],
            pivot: [2.0, 2.0],
        },
    );
    regions.insert(
        "corner".to_string(),
        AtlasRegion {
            rect: [0, 4, 4, 4],
            pivot: corner_pivot,
        },
    );
    AtlasDocument {
        generated_from: "fixture.atlas-source.yaml".to_string(),
        pixels_per_unit: PIXELS_PER_UNIT,
        color_map: "fixture.png".to_string(),
        filter,
        regions,
        flipbooks: Vec::new(),
    }
}

/// A fresh project directory carrying the fixture atlas pair, with the given corner pivot and filter.
fn fixture_project(label: &str, corner_pivot: [f64; 2], filter: AtlasFilter) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-sprite-w3-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    let rgba: Vec<u8> = (0..ATLAS_WIDTH * ATLAS_HEIGHT * 4)
        .map(|index| (index % 251) as u8)
        .collect();
    let png = ludion_assets::encode_png(ATLAS_WIDTH, ATLAS_HEIGHT, &rgba).unwrap();
    std::fs::write(dir.join("art/fixture.png"), &png).unwrap();
    std::fs::write(
        dir.join("art/fixture.atlas.yaml"),
        ludion_scene::emit_atlas(&doc(corner_pivot, filter)),
    )
    .unwrap();
    dir
}

fn step(ticks: u64) -> Command {
    Command::SimStep(command::SimStepParams { ticks, hash: false })
}

/// The current world hash of a fresh engine loaded with `scene`, rooted at `root` (any directory —
/// hashing reads only the world, never the atlas).
fn world_hash(root: &Path, scene: &str) -> ludion_sim::WorldHash {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(scene).expect("the scene loads");
    // No game systems in these scenes, so tick 1's hash equals tick 0's — a component-data digest.
    drive_hashes(&mut engine, vec![step(1)])
        .expect("the run drives")
        .last()
        .expect("at least one frame")
        .1
}

/// The sprite instances of `scene` (rooted at `root`) at `tick`, through the public extract entry.
fn sprites_at(root: &Path, scene: &str, tick: u64) -> Vec<SpriteInstance> {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(scene).expect("the scene loads");
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the world extracts")
    .sprites
}

/// A plain white sprite (no atlas needed — the fields are component data hashed at instantiate), with
/// `extra` sprite-body lines (each already indented eight spaces) spliced after the opaque colour.
fn plain_sprite_scene(extra: &str) -> String {
    format!(
        "format: 1\n\
         seed: 1\n\
         entities:\n\
        \x20 - name: s\n\
        \x20   components:\n\
        \x20     Sprite:\n\
        \x20       color:\n\
        \x20         - 1.0\n\
        \x20         - 1.0\n\
        \x20         - 1.0\n\
        \x20         - 1.0\n\
         {extra}"
    )
}

/// Every authored `Sprite` field the sprite effects added is ordinary component data: toggling one moves the
/// world hash exactly as any component-field change does, and two identical scenes hash the same.
#[test]
fn authored_sprite_fields_move_the_world_hash() {
    let root = std::env::temp_dir();
    let base_hash = world_hash(&root, &plain_sprite_scene(""));

    // A second, identical scene is the control: same bytes, same hash.
    assert_eq!(
        base_hash,
        world_hash(&root, &plain_sprite_scene("")),
        "an identical scene must hash identically",
    );

    for (label, extra) in [
        ("flip_x", "        flip_x: true\n"),
        ("flip_y", "        flip_y: true\n"),
        ("layer", "        layer: 5\n"),
        ("blend", "        blend: additive\n"),
        ("flash", "        flash: 0.5\n"),
        (
            "flash_color",
            "        flash_color:\n          - 1.0\n          - 0.0\n          - 0.0\n",
        ),
    ] {
        assert_ne!(
            base_hash,
            world_hash(&root, &plain_sprite_scene(extra)),
            "toggling `{label}` must move the world hash (it is authored component data)",
        );
    }
}

/// A scene naming a `pixel_snap` camera plus a sprite drawn from an atlas at `x = 0.1`.
const CAMERA_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 10.0
      Camera:
        projection:
          type: orthographic
          height: 8.0
          pixel_snap: true
  - name: sprite
    components:
      Transform:
        translation:
          - 0.1
          - 0.0
          - 0.0
      Sprite:
        atlas: art/fixture.atlas.yaml
        frame: mid
";

/// A sprite drawn from an atlas at the origin, showing `frame`. No camera, so `pixel_snap` is inert —
/// this isolates the pivot offset.
fn origin_atlas_scene(frame: &str) -> String {
    format!(
        "format: 1\n\
         seed: 1\n\
         entities:\n\
        \x20 - name: s\n\
        \x20   components:\n\
        \x20     Transform:\n\
        \x20       translation:\n\
        \x20         - 0.0\n\
        \x20         - 0.0\n\
        \x20         - 0.0\n\
        \x20     Sprite:\n\
        \x20       atlas: art/fixture.atlas.yaml\n\
        \x20       frame: {frame}\n"
    )
}

/// The pivot lives entirely outside the world hash: two atlases identical but for a region's pivot,
/// under one scene, hash the same. The pivot is a render-time geometry input, never component data.
#[test]
fn pivot_lives_outside_the_world_hash() {
    let centre = fixture_project("pivot-centre", [2.0, 2.0], AtlasFilter::Nearest);
    let corner = fixture_project("pivot-corner", [0.0, 0.0], AtlasFilter::Nearest);
    assert_eq!(
        world_hash(&centre, CAMERA_SCENE),
        world_hash(&corner, CAMERA_SCENE),
        "changing a region's pivot must not move the world hash",
    );
    let _ = std::fs::remove_dir_all(&centre);
    let _ = std::fs::remove_dir_all(&corner);
}

/// `pixel_snap`'s snapping is a read: extracting each tick under a `pixel_snap` camera produces the
/// same per-frame `WorldHash` trace as never extracting at all.
#[test]
fn pixel_snap_extract_writes_nothing_to_the_world() {
    let root = fixture_project("snap-neutral", [0.0, 0.0], AtlasFilter::Nearest);

    let run = |extract_each_tick: bool| {
        let mut engine = Engine::with_project_root(&root);
        engine.load_inline_scene(CAMERA_SCENE).expect("scene loads");
        let mut trace = Vec::new();
        for _ in 0..20 {
            trace.extend(
                drive_hashes(&mut engine, vec![step(1)])
                    .expect("drives")
                    .into_iter()
                    .skip(1),
            );
            if extract_each_tick {
                let _ = extract_at(
                    engine.world(),
                    (320, 180),
                    ludion_assets::AssetRoot::content(engine.project_root()),
                    SimTime::from_ticks(engine.frame().as_u64(), TickRate::DEFAULT),
                    QualityParams::FULL.particle_density,
                );
            }
        }
        trace
    };

    assert_eq!(
        run(false),
        run(true),
        "extracting under `pixel_snap` must not move a single world hash",
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Flip mirrors the sampled UV rect in place: `flip_x` swaps the u endpoints, `flip_y` the v.
#[test]
fn flip_mirrors_the_uv_rect() {
    let root = fixture_project("flip", [2.0, 2.0], AtlasFilter::Nearest);
    let scene = |flip: &str| {
        origin_atlas_scene("mid").replace(
            "        frame: mid\n",
            &format!("        frame: mid\n{flip}"),
        )
    };
    let plain = sprites_at(&root, &origin_atlas_scene("mid"), 0)[0].uv;
    let fx = sprites_at(&root, &scene("        flip_x: true\n"), 0)[0].uv;
    let fy = sprites_at(&root, &scene("        flip_y: true\n"), 0)[0].uv;

    assert_eq!(fx, [plain[2], plain[1], plain[0], plain[3]]);
    assert_eq!(fy, [plain[0], plain[3], plain[2], plain[1]]);
    let _ = std::fs::remove_dir_all(&root);
}

/// A non-centre pivot offsets the quad centre off the entity translation so rotation is about the
/// pivot; the centre pivot leaves the centre exactly at the translation (identical to before W3).
#[test]
fn pivot_offsets_the_quad_centre() {
    let root = fixture_project("pivot-offset", [0.0, 0.0], AtlasFilter::Nearest);
    // `mid` is centre-pivoted: the quad centre sits exactly at the translation.
    let centred = sprites_at(&root, &origin_atlas_scene("mid"), 0)[0].center;
    assert_eq!(centred.to_array(), [0.0, 0.0, 0.0]);

    // `corner`'s pivot is the top-left texel [0, 0]: fx = 0, fy = 0. The region is 1×1 world unit, so
    // the centre shifts to translation - ((0-0.5)*right + (0.5-0)*up) = (0.5, -0.5, 0).
    let cornered = sprites_at(&root, &origin_atlas_scene("corner"), 0)[0].center;
    assert_eq!(cornered.to_array(), [0.5, -0.5, 0.0]);
    let _ = std::fs::remove_dir_all(&root);
}

/// `pixel_snap` snaps each sprite centre onto the atlas texel grid; without it the sub-texel centre
/// survives.
#[test]
fn pixel_snap_snaps_the_centre() {
    let root = fixture_project("snap", [2.0, 2.0], AtlasFilter::Nearest);
    // The sprite sits at x = 0.1; the texel grid is 1/4 = 0.25 wide, so it snaps to 0.0.
    let snapped = sprites_at(&root, CAMERA_SCENE, 0);
    assert_eq!(
        snapped[0].center.x, 0.0,
        "a sub-texel centre snaps onto the 0.25-world-unit texel grid",
    );

    // The same scene with `pixel_snap: false` keeps the authored 0.1 centre.
    let unsnapped_scene = CAMERA_SCENE.replace("pixel_snap: true", "pixel_snap: false");
    let unsnapped = sprites_at(&root, &unsnapped_scene, 0);
    assert!(
        (unsnapped[0].center.x - 0.1).abs() < 1e-6,
        "without `pixel_snap` the authored centre survives, got {}",
        unsnapped[0].center.x,
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `blend: additive` and `flash`/`flash_color` reach the `SpriteInstance` the renderer batches, and
/// the default sprite carries neither — render-identical to a sprite without them.
#[test]
fn blend_and_flash_reach_the_instance() {
    let root = std::env::temp_dir();
    let scene = plain_sprite_scene(
        "        blend: additive\n        flash: 0.75\n        flash_color:\n          - 1.0\n     \
         \x20    - 0.0\n          - 0.0\n",
    );
    let s = sprites_at(&root, &scene, 0);
    assert_eq!(s.len(), 1);
    assert!(s[0].additive, "additive blend reaches the instance");
    assert_eq!(s[0].flash, 0.75);
    assert_eq!(s[0].flash_color, [1.0, 0.0, 0.0]);

    let plain = sprites_at(&root, &plain_sprite_scene(""), 0);
    assert!(!plain[0].additive);
    assert_eq!(plain[0].flash, 0.0);
}

/// A flip mirrors the pivot with the art: the pivot texel, found from the drawn instance's own UV
/// rect, sits at the entity translation in all four flip states.
#[test]
fn a_flip_keeps_the_pivot_texel_at_the_translation() {
    for pivot in [[0.0, 0.0], [1.0, 3.0]] {
        let root = fixture_project("flip-pivot", pivot, AtlasFilter::Nearest);
        // `corner` is rect [0, 4, 4, 4] in the 8x8 image.
        let pu = pivot[0] / 8.0;
        let pv = (4.0 + pivot[1]) / 8.0;
        for (flip_x, flip_y) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut lines = String::new();
            if flip_x {
                lines.push_str("        flip_x: true\n");
            }
            if flip_y {
                lines.push_str("        flip_y: true\n");
            }
            let scene = origin_atlas_scene("corner")
                .replace("frame: corner\n", &format!("frame: corner\n{lines}"));
            let sprite = sprites_at(&root, &scene, 0)[0];
            let uv = sprite.uv.map(f64::from);
            let s = (pu - uv[0]) / (uv[2] - uv[0]) - 0.5;
            let t = 0.5 - (pv - uv[1]) / (uv[3] - uv[1]);
            let at =
                sprite.center.as_dvec3() + s * sprite.right.as_dvec3() + t * sprite.up.as_dvec3();
            assert!(
                at.length() < 1e-6,
                "pivot {pivot:?} flip_x={flip_x} flip_y={flip_y}: the pivot texel is at {at:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
