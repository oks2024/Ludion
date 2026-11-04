//! Atlas resolution at `extract()` (`Plan/sprite-engine.md` Wave 1): a `Sprite`'s or a `Tilemap`
//! legend entry's `atlas` + `frame` pair becomes one `RenderScene::textures` entry shared by every
//! quad of that atlas, a per-instance UV rect, and — for a `Sprite` that authors no `size` — a quad
//! sized by the region's own pixels.
//!
//! These build a `World` directly from the built-in component types (the `tests/extract.rs` idiom)
//! and run with no GPU. The one exception is the schema-refusal test, which has to go through
//! `ludion_scene::check` because refusing a half-written reference is what stops it reaching a world
//! at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_ecs::World;
use ludion_engine::{Sprite, TileAppearance, Tilemap, Transform, UiRect, extract};
use ludion_renderer::{SpriteInstance, UiInstance};
use ludion_scene::{AtlasDocument, AtlasFilter, AtlasRegion};

/// The packed image every fixture shares: 8×8 RGBA, whose first texel is **half-alpha** so a second
/// premultiplication would visibly darken it (the loader's one atlas-specific rule).
const ATLAS_WIDTH: u32 = 8;
const ATLAS_HEIGHT: u32 = 8;

/// How many texels of the fixture atlas span one world unit.
const PIXELS_PER_UNIT: f64 = 4.0;

/// The fixture's colour bytes: one half-alpha texel, then a deterministic ramp.
fn atlas_rgba() -> Vec<u8> {
    let mut rgba = vec![0u8; (ATLAS_WIDTH * ATLAS_HEIGHT * 4) as usize];
    rgba[0..4].copy_from_slice(&[200, 100, 50, 128]);
    for (index, chunk) in rgba.chunks_exact_mut(4).enumerate().skip(1) {
        let value = (index % 251) as u8;
        chunk.copy_from_slice(&[value, 255 - value, 17, 255]);
    }
    rgba
}

/// Writes a content root holding `art/ship.png`, the `art/ship.atlas.yaml` addressing it with two
/// regions, and a loose `art/loose.png`. Returns the root.
fn content_root(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-sprite-atlas-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    let png = ludion_assets::encode_png(ATLAS_WIDTH, ATLAS_HEIGHT, &atlas_rgba()).unwrap();
    std::fs::write(dir.join("art/ship.png"), &png).unwrap();
    std::fs::write(
        dir.join("art/ship.atlas.yaml"),
        ludion_scene::emit_atlas(&doc()),
    )
    .unwrap();

    let loose = ludion_assets::encode_png(1, 1, &[10, 20, 30, 255]).unwrap();
    std::fs::write(dir.join("art/loose.png"), &loose).unwrap();
    dir
}

/// The fixture atlas document: two regions of different shapes, so a wrong UV or a wrong derived
/// size cannot pass by coincidence.
fn doc() -> AtlasDocument {
    let mut regions = BTreeMap::new();
    regions.insert(
        "ship/idle".to_string(),
        AtlasRegion {
            rect: [0, 0, 4, 2],
            pivot: [2.0, 1.0],
        },
    );
    regions.insert(
        "ship/hit".to_string(),
        AtlasRegion {
            rect: [4, 0, 2, 3],
            pivot: [1.0, 1.5],
        },
    );
    AtlasDocument {
        generated_from: "ship.atlas-source.yaml".to_string(),
        pixels_per_unit: PIXELS_PER_UNIT,
        color_map: "ship.png".to_string(),
        filter: AtlasFilter::Nearest,
        regions,
        flipbooks: Vec::new(),
    }
}

/// Spawns an entity at the origin carrying `sprite`.
fn spawn_sprite(world: &mut World, sprite: Sprite) {
    let id = world.spawn();
    world.insert(id, Transform::default()).unwrap();
    world.insert(id, sprite).unwrap();
}

/// A `Sprite` naming `frame` of the fixture atlas.
fn atlas_sprite(frame: &str, size: Option<[f32; 2]>) -> Sprite {
    Sprite {
        atlas: Some("art/ship.atlas.yaml".to_string()),
        frame: Some(frame.to_string()),
        size,
        ..Sprite::default()
    }
}

/// The whole Wave 1 contract in one extract: two atlas sprites and a tilemap of the same atlas share
/// **one** texture index (one batch), a loose PNG gets its own, each region's UV rect is exact, an
/// un-sized sprite is sized `rect / pixels_per_unit`, an authored `size` overrides that, and the
/// packed image arrives byte-identical to the PNG on disk — the no-double-premultiply proof.
#[test]
fn one_atlas_is_one_texture_with_exact_uvs_and_derived_sizes() {
    let dir = content_root("resolve");
    let mut world = World::new();

    spawn_sprite(&mut world, atlas_sprite("ship/idle", None));
    spawn_sprite(&mut world, atlas_sprite("ship/hit", Some([3.0, 2.0])));
    spawn_sprite(
        &mut world,
        Sprite {
            texture: Some("art/loose.png".to_string()),
            ..Sprite::default()
        },
    );

    let tiles = world.spawn();
    world.insert(tiles, Transform::default()).unwrap();
    let mut legend = BTreeMap::new();
    legend.insert(
        "i".to_string(),
        TileAppearance {
            atlas: Some("art/ship.atlas.yaml".to_string()),
            frame: Some("ship/idle".to_string()),
            ..TileAppearance::default()
        },
    );
    world
        .insert(
            tiles,
            Tilemap {
                cell_size: 2.0,
                legend,
                rows: vec!["ii".to_string()],
                layer: 0,
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), ludion_assets::AssetRoot::content(&dir)).unwrap();

    // One entry per *atlas path*, not per quad: four quads of one atlas, one texture, one batch.
    assert_eq!(scene.textures.len(), 2, "the atlas and the loose PNG");
    assert_eq!(scene.sprites.len(), 5, "three sprites plus two cells");
    let atlas_index = scene.sprites[0].texture.unwrap();
    assert!(
        scene.sprites[1].texture == Some(atlas_index)
            && scene.sprites[3].texture == Some(atlas_index)
            && scene.sprites[4].texture == Some(atlas_index),
        "every quad of one atlas shares its texture index"
    );
    assert_ne!(
        scene.sprites[2].texture,
        Some(atlas_index),
        "a loose PNG is its own texture"
    );

    // UV rects: `rect` over the packed image's own extent, top-left origin.
    assert_eq!(scene.sprites[0].uv, [0.0, 0.0, 0.5, 0.25]);
    assert_eq!(scene.sprites[1].uv, [0.5, 0.0, 0.75, 0.375]);
    assert_eq!(scene.sprites[2].uv, SpriteInstance::FULL_UV);
    assert_eq!(scene.sprites[3].uv, [0.0, 0.0, 0.5, 0.25]);

    // Sizes: derived from the region for the un-sized sprite, authored for the second, `[1, 1]` for
    // the loose one, and `cell_size` for every tilemap cell whatever region it draws.
    assert_eq!(scene.sprites[0].right.x, 4.0 / PIXELS_PER_UNIT as f32);
    assert_eq!(scene.sprites[0].up.y, 2.0 / PIXELS_PER_UNIT as f32);
    assert_eq!(scene.sprites[1].right.x, 3.0);
    assert_eq!(scene.sprites[1].up.y, 2.0);
    assert_eq!(scene.sprites[2].right.x, 1.0);
    assert_eq!(scene.sprites[2].up.y, 1.0);
    assert_eq!(scene.sprites[3].right.x, 2.0);
    assert_eq!(scene.sprites[3].up.y, 2.0);

    // The bake writes the packed image premultiplied, so the loader must not premultiply it again —
    // which the half-alpha texel is here to catch.
    let texture = &scene.textures[atlas_index];
    assert_eq!(texture.width, ATLAS_WIDTH);
    assert_eq!(texture.height, ATLAS_HEIGHT);
    assert_eq!(
        texture.rgba,
        atlas_rgba(),
        "an atlas is already premultiplied; premultiplying again darkens every translucent texel"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `frame` naming no region is `L1152` at the component's `frame` field, carrying the nearest
/// region name so the fix is in the message.
#[test]
fn an_unknown_frame_is_l1152_with_a_suggestion() {
    let dir = content_root("frame");
    let mut world = World::new();
    spawn_sprite(&mut world, atlas_sprite("ship/idel", None));

    let errors = extract(&world, (320, 180), ludion_assets::AssetRoot::content(&dir)).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1152");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Sprite/frame")
    );
    let rendered = format!("{}", errors[0]);
    assert!(
        rendered.contains("ship/idel") && rendered.contains("ship/idle"),
        "the report names the frame that failed and the one that was meant: {rendered}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An `atlas` path that names no file is `L1151` at the component's `atlas` field — the
/// texture-error precedent, with the load failure carried as a related diagnostic.
#[test]
fn a_missing_atlas_is_l1151_at_the_atlas_field() {
    let dir = content_root("missing");
    let mut world = World::new();
    spawn_sprite(
        &mut world,
        Sprite {
            atlas: Some("art/absent.atlas.yaml".to_string()),
            frame: Some("ship/idle".to_string()),
            ..Sprite::default()
        },
    );

    let errors = extract(&world, (320, 180), ludion_assets::AssetRoot::content(&dir)).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1151");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Sprite/atlas")
    );
    assert_eq!(errors[0].related[0].code.as_str(), "L1101");
    assert!(
        !errors[0]
            .help
            .as_deref()
            .unwrap()
            .contains("could not read"),
        "the cause is related, not joined into the help: {:?}",
        errors[0].help
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An atlas file that exists but does not parse keeps its causes as related diagnostics and is not
/// offered a nearest-filename suggestion.
#[test]
fn an_unparseable_atlas_carries_its_causes_and_suggests_nothing() {
    let dir = content_root("unparseable");
    std::fs::create_dir_all(dir.join("art")).unwrap();
    std::fs::write(dir.join("art/bad.atlas.yaml"), "format: [unterminated").unwrap();
    std::fs::write(dir.join("art/bad2.atlas.yaml"), "format: 1\n").unwrap();
    let mut world = World::new();
    spawn_sprite(
        &mut world,
        Sprite {
            atlas: Some("art/bad.atlas.yaml".to_string()),
            frame: Some("ship/idle".to_string()),
            ..Sprite::default()
        },
    );

    let errors = extract(&world, (320, 180), ludion_assets::AssetRoot::content(&dir)).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1151");
    assert!(!errors[0].related.is_empty(), "{:?}", errors[0]);
    assert!(
        !errors[0].help.as_deref().unwrap().contains("nearest match"),
        "{:?}",
        errors[0].help
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `frame` with no `atlas` never reaches `extract`: it is `L1151` at scene-check time, because an
/// atlas reference is a pair and half of one has no region to name.
#[test]
fn a_frame_without_an_atlas_is_refused_before_extract() {
    let registry = ludion_engine::builtin_registry();
    let source = "format: 1\nentities:\n  - components:\n      Sprite:\n        frame: ship/idle\n";
    let diagnostics = ludion_scene::check(source, &registry).unwrap_err();
    assert!(
        diagnostics.iter().any(|d| d.code.as_str() == "L1151"),
        "a half-written atlas reference is refused: {diagnostics:?}"
    );
}

/// `check_atlas_reference` is the seam `check-scene` shares with `extract`: it answers for a
/// reference that draws, one that names no region, and one whose atlas is not there — against the
/// same content root, with the same codes.
#[test]
fn the_shared_reference_check_answers_for_a_scene() {
    let dir = content_root("shared");
    let pointer = "/entities/7/components/Sprite";

    assert!(
        ludion_engine::check_atlas_reference(
            ludion_assets::AssetRoot::content(&dir),
            pointer,
            "art/ship.atlas.yaml",
            Some("ship/idle")
        )
        .is_empty(),
        "a resolvable reference reports nothing"
    );

    let unknown = ludion_engine::check_atlas_reference(
        ludion_assets::AssetRoot::content(&dir),
        pointer,
        "art/ship.atlas.yaml",
        Some("ship/idel"),
    );
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0].code.as_str(), "L1152");
    assert_eq!(
        unknown[0].path.as_deref(),
        Some("/entities/7/components/Sprite/frame")
    );

    let missing = ludion_engine::check_atlas_reference(
        ludion_assets::AssetRoot::content(Path::new(".")),
        pointer,
        "art/absent.atlas.yaml",
        Some("ship/idle"),
    );
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].code.as_str(), "L1151");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `UiRect` naming an atlas region resolves through the **same** path a `Sprite` does: it lands on
/// the shared atlas `textures` entry (batching with atlas sprites), and its UI instance carries the
/// region's UV sub-rect — while a loose-`texture:` rect keeps the whole-texture UV, byte-identical to
/// the pre-atlas path. The `ship/idle` region is `[0, 0, 4, 2]` on the 8×8 fixture ⇒ `[0, 0, 0.5,
/// 0.25]`.
#[test]
fn a_ui_rect_atlas_reference_resolves_to_the_region_uv() {
    let dir = content_root("ui-atlas");
    let mut world = World::new();

    // One atlas sprite, so the UI rect's atlas can share its texture index and prove one-batch reuse.
    spawn_sprite(&mut world, atlas_sprite("ship/idle", None));

    let icon = world.spawn();
    world
        .insert(
            icon,
            UiRect {
                atlas: Some("art/ship.atlas.yaml".to_string()),
                frame: Some("ship/idle".to_string()),
                ..UiRect::default()
            },
        )
        .unwrap();
    let plain = world.spawn();
    world
        .insert(
            plain,
            UiRect {
                texture: Some("art/loose.png".to_string()),
                ..UiRect::default()
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), ludion_assets::AssetRoot::content(&dir)).unwrap();

    let atlas_index = scene.sprites[0].texture.unwrap();
    // The atlas sprite and the atlas UI rect share one texture entry; the loose PNG is its own.
    assert_eq!(scene.textures.len(), 2, "the atlas and the loose PNG");
    assert_eq!(scene.ui.len(), 2);

    match &scene.ui[0] {
        UiInstance::Rect { texture, uv, .. } => {
            assert_eq!(
                *texture,
                Some(atlas_index),
                "the UI rect shares the atlas texture"
            );
            assert_eq!(
                *uv,
                [0.0, 0.0, 0.5, 0.25],
                "the region's UV sub-rect, not the whole image"
            );
        }
        other => panic!("expected the atlas Rect first, got {other:?}"),
    }
    match &scene.ui[1] {
        UiInstance::Rect { texture, uv, .. } => {
            assert_ne!(
                *texture,
                Some(atlas_index),
                "a loose PNG is its own texture"
            );
            assert_eq!(
                *uv,
                SpriteInstance::FULL_UV,
                "a loose texture samples the whole image"
            );
        }
        other => panic!("expected the loose Rect second, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

// --- A bullet kind's atlas pair resolves at document load and reload --------------------------

/// A `kind: pattern` document with one bullet kind whose textured fields are `bullet_fields`
/// (block-style YAML lines, indented for the kind's mapping).
fn pattern_doc(bullet_fields: &str) -> String {
    format!(
        "format: 1\nkind: pattern\nname: probe\nbullets:\n  needle:\n    life: 60\n{bullet_fields}\
         timeline:\n  - at: 0\n    bullet: needle\n    speed: 4.0\n"
    )
}

/// Builds a patterns engine over a content root holding `pattern_doc(bullet_fields)`; the
/// diagnostics of a refused build (collapsed head plus `related`), empty on success.
fn bullet_atlas_diagnostics(
    label: &str,
    bullet_fields: &str,
) -> Vec<ludion_core::diagnostics::Diagnostic> {
    let dir = content_root(label);
    std::fs::create_dir_all(dir.join("patterns")).unwrap();
    std::fs::write(dir.join("patterns/probe.yaml"), pattern_doc(bullet_fields)).unwrap();
    let out = match ludion_engine::Engine::builder()
        .patterns()
        .project_root(&dir)
        .build()
    {
        Ok(_) => Vec::new(),
        Err(mut collapsed) => {
            let related = std::mem::take(&mut collapsed.related);
            std::iter::once(collapsed).chain(related).collect()
        }
    };
    let _ = std::fs::remove_dir_all(&dir);
    out
}

fn has(diagnostics: &[ludion_core::diagnostics::Diagnostic], code: &str, path: &str) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code.as_str() == code && d.path.as_deref() == Some(path))
}

const BULLET_PATH: &str = "pattern/patterns/probe.yaml/bullets/needle";

#[test]
fn a_bullet_kind_naming_a_real_region_builds() {
    let diagnostics = bullet_atlas_diagnostics(
        "bullet-ok",
        "    atlas: art/ship.atlas.yaml\n    frame: ship/idle\n",
    );
    assert!(
        diagnostics.is_empty(),
        "a resolving pair builds: {diagnostics:?}"
    );
}

#[test]
fn a_bullet_kind_naming_an_unknown_region_is_l1152_at_load() {
    let diagnostics = bullet_atlas_diagnostics(
        "bullet-region",
        "    atlas: art/ship.atlas.yaml\n    frame: ship/idel\n",
    );
    assert!(
        has(&diagnostics, "L1152", &format!("{BULLET_PATH}/frame")),
        "{diagnostics:?}"
    );
}

#[test]
fn a_bullet_kind_naming_an_absent_atlas_is_l1151_at_load() {
    let diagnostics = bullet_atlas_diagnostics(
        "bullet-absent",
        "    atlas: art/absent.atlas.yaml\n    frame: ship/idle\n",
    );
    assert!(
        has(&diagnostics, "L1151", &format!("{BULLET_PATH}/atlas")),
        "{diagnostics:?}"
    );
}

#[test]
fn a_bullet_frame_without_an_atlas_is_l1151_at_load() {
    let diagnostics = bullet_atlas_diagnostics("bullet-frame-only", "    frame: ship/idle\n");
    assert!(
        has(&diagnostics, "L1151", &format!("{BULLET_PATH}/frame")),
        "{diagnostics:?}"
    );
}
