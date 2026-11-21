//! `UiRect.uv_rect` and `UiRect.slice` resolved in extract: a nine-slice is nine ordinary rects that
//! share edges, sample inside their region's uv, and move with a tween and a world anchor like any
//! HUD element. Every case extracts through `Engine::extract_present_scene`; none needs a GPU.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ludion_components::ui::{
    UiAnchorBehind, UiAnchorShow, UiClock, UiEase, UiSlice, UiSliceCenter, UiTween, UiTweenField,
    UiTweenRepeat, UiTweenTrack, UiWorldAnchor,
};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_engine::components::{Anchor, UiRect};
use ludion_engine::ui::resolve_ui_rect_of;
use ludion_engine::{Diagnostics, Engine, Name};
use ludion_renderer::{RenderScene, SpriteInstance, UiInstance};
use ludion_scene::{AtlasDocument, AtlasFilter, AtlasRegion};

const SCENE: &str = "\
format: 1
seed: 3
entities:
  - name: cam
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 10.0
      Camera: {}
  - name: target
    components:
      Transform:
        translation:
          - 2.0
          - 0.0
          - 0.0
";

/// Writes a content root: a 12x12 `tex.png`, an 8x8 `ship.png` with its atlas (region `ship/idle` is
/// the 4x2 texels at the top-left), and the scene. Returns the engine loaded from it.
fn engine(label: &str) -> (Engine, PathBuf) {
    let dir = std::env::temp_dir().join(format!("ludion-ui-slice-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let png = |w: u32, h: u32| ludion_assets::encode_png(w, h, &vec![200; (w * h * 4) as usize]);
    std::fs::write(dir.join("tex.png"), png(12, 12).unwrap()).unwrap();
    std::fs::write(dir.join("ship.png"), png(8, 8).unwrap()).unwrap();
    let mut regions = BTreeMap::new();
    regions.insert(
        "ship/idle".to_string(),
        AtlasRegion {
            rect: [0, 0, 4, 2],
            pivot: [2.0, 1.0],
        },
    );
    let atlas = AtlasDocument {
        generated_from: "ship.atlas-source.yaml".to_string(),
        pixels_per_unit: 4.0,
        color_map: "ship.png".to_string(),
        filter: AtlasFilter::Nearest,
        regions,
        flipbooks: Vec::new(),
    };
    std::fs::write(
        dir.join("ship.atlas.yaml"),
        ludion_scene::emit_atlas(&atlas),
    )
    .unwrap();
    let scene = dir.join("scene.yaml");
    std::fs::write(&scene, SCENE).unwrap();
    let engine = Engine::builder()
        .load_scene_file(&scene)
        .expect("the scene loads");
    (engine, dir)
}

fn extract(engine: &mut Engine) -> RenderScene {
    engine.extract_present_scene().expect("extracts").0
}

fn named(engine: &Engine, wanted: &str) -> EntityId {
    let world = engine.world();
    world
        .entities()
        .find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == wanted)
        })
        .unwrap_or_else(|| panic!("the scene has a `{wanted}`"))
}

fn spawn(engine: &mut Engine, rect: UiRect) -> EntityId {
    let id = engine.world_mut().spawn_presentation();
    engine.world_mut().insert(id, rect).unwrap();
    id
}

fn textured(slice: Option<UiSlice>) -> UiRect {
    UiRect {
        offset: [10.0, 20.0],
        size: [120.0, 60.0],
        texture: Some("tex.png".to_string()),
        slice,
        ..UiRect::default()
    }
}

fn border(texels: f32) -> UiSlice {
    UiSlice {
        border: [texels; 4],
        scale: 1.0,
        center: UiSliceCenter::Stretch,
    }
}

/// `(rect_px, uv)` of every rect in the scene UI list.
fn rows(scene: &RenderScene) -> Vec<([f32; 4], [f32; 4])> {
    scene
        .ui
        .iter()
        .filter_map(|instance| match instance {
            UiInstance::Rect { rect_px, uv, .. } => Some((*rect_px, *uv)),
            UiInstance::Text { .. } | UiInstance::Shape { .. } => None,
        })
        .collect()
}

/// The bounding box `[x, y, w, h]` of some rects.
fn union(rects: impl Iterator<Item = [f32; 4]>) -> [f32; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for [x, y, w, h] in rects {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x + w);
        y1 = y1.max(y + h);
    }
    [x0, y0, x1 - x0, y1 - y0]
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-2
}

fn all_close(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| close(*a, *b))
}

#[test]
fn a_slice_is_nine_rects_covering_the_resolved_rect() {
    let (mut engine, _dir) = engine("nine");
    let rect = textured(Some(border(4.0)));
    spawn(&mut engine, rect.clone());
    let drawn = rows(&extract(&mut engine));
    assert_eq!(drawn.len(), 9);
    let ps = engine.render_size().1 as f32 / 1080.0;
    let whole = resolve_ui_rect_of(&rect, engine.render_size());
    assert!(all_close(&union(drawn.iter().map(|d| d.0)), &whole));
    assert!(
        close(drawn[0].0[2], 4.0 * ps) && close(drawn[0].0[3], 4.0 * ps),
        "corner"
    );
    let third = 4.0 / 12.0;
    assert!(all_close(&drawn[0].1, &[0.0, 0.0, third, third]));
    assert!(all_close(
        &drawn[4].1,
        &[third, third, 1.0 - third, 1.0 - third]
    ));
    assert!(all_close(
        &drawn[8].1,
        &[1.0 - third, 1.0 - third, 1.0, 1.0]
    ));
}

#[test]
fn a_slice_through_an_atlas_region_splits_inside_its_uv() {
    let (mut engine, _dir) = engine("atlas");
    spawn(
        &mut engine,
        UiRect {
            size: [120.0, 60.0],
            atlas: Some("ship.atlas.yaml".to_string()),
            frame: Some("ship/idle".to_string()),
            slice: Some(UiSlice {
                border: [1.0, 0.5, 1.0, 0.5],
                scale: 1.0,
                center: UiSliceCenter::Stretch,
            }),
            ..UiRect::default()
        },
    );
    let drawn = rows(&extract(&mut engine));
    assert_eq!(drawn.len(), 9);
    // The region uv is [0, 0, 0.5, 0.25]: 1 of 4 texels is 0.125 of u, 0.5 of 2 texels is 0.0625 of v.
    assert!(all_close(&drawn[0].1, &[0.0, 0.0, 0.125, 0.0625]));
    assert!(all_close(&drawn[8].1, &[0.375, 0.1875, 0.5, 0.25]));
}

#[test]
fn uv_rect_narrows_one_rect() {
    let (mut engine, _dir) = engine("uvrect");
    let mut rect = textured(None);
    rect.uv_rect = Some([0.0, 0.0, 0.5, 1.0]);
    spawn(&mut engine, rect);
    let drawn = rows(&extract(&mut engine));
    assert_eq!(drawn.len(), 1);
    assert_eq!(drawn[0].1, [0.0, 0.0, 0.5, 1.0]);
}

#[test]
fn a_rect_without_the_fields_draws_one_full_uv_rect() {
    let (mut engine, _dir) = engine("plain");
    spawn(&mut engine, textured(None));
    let drawn = rows(&extract(&mut engine));
    assert_eq!(drawn.len(), 1);
    assert_eq!(drawn[0].1, SpriteInstance::FULL_UV);
}

#[test]
fn a_tween_and_a_world_anchor_move_all_nine_pieces() {
    let (mut engine, _dir) = engine("motion");
    let target = named(&engine, "target");
    let plain = UiRect {
        anchor: Anchor::Center,
        offset: [0.0, 0.0],
        order: 0,
        ..textured(None)
    };
    let sliced = UiRect {
        order: 1,
        slice: Some(border(4.0)),
        ..plain.clone()
    };
    let pop = UiTween {
        tracks: vec![UiTweenTrack {
            field: UiTweenField::Scale,
            from: vec![2.0],
            to: vec![2.0],
            start: 0,
            duration: 0.0,
            ease: UiEase::Linear,
            repeat: UiTweenRepeat::Once,
            clock: UiClock::Real,
        }],
    };
    let pin = UiWorldAnchor {
        target,
        world_offset: [0.0; 3],
        show: UiAnchorShow::Always,
        behind: UiAnchorBehind::Clamp,
        margin: 48.0,
        fade: None,
    };
    for rect in [plain, sliced] {
        let id = spawn(&mut engine, rect);
        engine.world_mut().insert(id, pop.clone()).unwrap();
        engine.world_mut().insert(id, pin.clone()).unwrap();
    }
    let drawn = rows(&extract(&mut engine));
    assert_eq!(drawn.len(), 10);
    let twin = drawn[0].0;
    let pieces = union(drawn[1..].iter().map(|d| d.0));
    assert!(all_close(&pieces, &twin), "{pieces:?} vs {twin:?}");
    assert!(
        twin[2] > 120.0 * 1.5 * (engine.render_size().1 as f32 / 1080.0),
        "the scale-2 tween applied: {twin:?}"
    );
}

#[test]
fn an_oversized_border_is_reported_as_l2301() {
    let (mut engine, _dir) = engine("clamp");
    spawn(&mut engine, textured(Some(border(8.0))));
    assert!(!rows(&extract(&mut engine)).is_empty());
    extract(&mut engine);
    let reported = engine
        .world()
        .resource::<Diagnostics>()
        .map(|d| d.reported(code::UI_SLICE_CLAMPED).count())
        .unwrap_or_default();
    assert_eq!(reported, 1, "once per entity, however many frames");
}
