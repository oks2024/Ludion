//! `render.bounds` tests: exact world AABBs + screen rects, camera semantics, and determinism.
//!
//! All are **headless** — `render.bounds` is pure extract + projection math, so no renderer is ever
//! constructed. Worlds are built directly from the built-in component types and driven through
//! `dispatch(Command::RenderBounds(RenderBoundsParams::default()))`.
//!
//! The camera and viewport are chosen so every projected pixel value is exact in `f32`: a
//! 256×256 viewport with an orthographic camera of world height 4 (half-extent 2) at `z = 10` gives
//! `screen_x = 64·world_x + 128` and `screen_y = 128 − 64·world_y` — all the constants
//! (`0.25`, `0.5`, `64`, `128`, `256`) are exact, so the assertions below can demand exact equality.

use std::collections::BTreeMap;

use ludion_ecs::World;
use ludion_engine::{
    Camera, CameraOffset, Engine, Mesh, MeshGrid, MeshSource, PointLight, Projection, RenderView,
    Sprite, Text, TileAppearance, Tilemap, Transform, dispatch,
};
use ludion_protocol::command::{CameraChoice, RenderBoundsParams, RenderViewParams, ViewPose};
use ludion_protocol::response::{EntityBounds, ScreenRect, WorldAabb};
use ludion_protocol::{Command, ResponseBody};

/// A 256×256 engine with an empty world, ready for direct entity construction.
fn engine() -> Engine {
    Engine::new().with_render_size(256, 256)
}

/// Inserts the exact-math orthographic camera (height 4, at `z = 10`, looking down −Z) as the next
/// entity in `world`. Returns its id.
fn add_camera(world: &mut World) -> ludion_core::ids::EntityId {
    let cam = world.spawn();
    world
        .insert(
            cam,
            Transform {
                translation: [0.0, 0.0, 10.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world
        .insert(
            cam,
            Camera {
                projection: Projection::Orthographic {
                    height: 4.0,
                    pixel_snap: false,
                },
                ..Camera::default()
            },
        )
        .unwrap();
    cam
}

/// Dispatches `render.bounds`, expecting success, and returns `(render_size, entities)`.
fn bounds(engine: &mut Engine) -> ([u32; 2], Vec<EntityBounds>) {
    match dispatch(engine, Command::RenderBounds(RenderBoundsParams::default()))
        .expect("render.bounds should succeed")
        .body
    {
        ResponseBody::RenderBounds {
            render_size,
            entities,
        } => (render_size, entities),
        other => panic!("expected RenderBounds, got {other:?}"),
    }
}

/// A sprite, a mesh, and a text run get exact bounds; a light-only entity does not appear at all; the
/// entries come back in ascending entity-slot order (the order `extract` walks).
#[test]
fn mixed_scene_has_exact_bounds_and_excludes_lights() {
    let mut engine = engine();
    let world = engine.world_mut();
    let _camera = add_camera(world); // slot 0 — not renderable, absent from the result.

    // slot 1: a 2×1 sprite centered at the origin.
    let sprite = world.spawn();
    world.insert(sprite, Transform::default()).unwrap();
    world
        .insert(
            sprite,
            Sprite {
                size: Some([2.0, 1.0]),
                ..Sprite::default()
            },
        )
        .unwrap();

    // slot 2: a light only — no visible geometry, must be excluded.
    let light = world.spawn();
    world.insert(light, Transform::default()).unwrap();
    world.insert(light, PointLight::default()).unwrap();

    // slot 3: a unit cube at the origin.
    let mesh = world.spawn();
    world.insert(mesh, Transform::default()).unwrap();
    world.insert(mesh, Mesh::default()).unwrap();

    // slot 4: the two-character text "AB" at the origin, glyph size 1.
    let text = world.spawn();
    world.insert(text, Transform::default()).unwrap();
    world
        .insert(
            text,
            Text {
                text: "AB".to_string(),
                size: 1.0,
                ..Text::default()
            },
        )
        .unwrap();

    let (render_size, entities) = bounds(&mut engine);
    assert_eq!(render_size, [256, 256]);

    // Exactly the three renderables, in slot order: sprite, mesh, text. Neither camera nor light.
    assert_eq!(entities.len(), 3);
    assert_eq!(entities[0].entity, sprite);
    assert_eq!(entities[1].entity, mesh);
    assert_eq!(entities[2].entity, text);

    // Sprite: quad corners `center ± right/2 ± up/2` = x ∈ [-1, 1], y ∈ [-0.5, 0.5].
    assert_eq!(
        entities[0],
        EntityBounds {
            entity: sprite,
            world_aabb: WorldAabb {
                min: [-1.0, -0.5, 0.0],
                max: [1.0, 0.5, 0.0],
            },
            // sx = 64·x + 128 → [64, 192]; sy = 128 − 64·y → [96, 160].
            screen_rect: Some(ScreenRect {
                x: 64.0,
                y: 96.0,
                w: 128.0,
                h: 64.0,
            }),
            on_screen: true,
            hidden: false,
        }
    );

    // Mesh: unit cube, model = identity, AABB = ±0.5 on every axis.
    assert_eq!(
        entities[1],
        EntityBounds {
            entity: mesh,
            world_aabb: WorldAabb {
                min: [-0.5, -0.5, -0.5],
                max: [0.5, 0.5, 0.5],
            },
            screen_rect: Some(ScreenRect {
                x: 96.0,
                y: 96.0,
                w: 64.0,
                h: 64.0,
            }),
            on_screen: true,
            hidden: false,
        }
    );

    // Text: "AB" is 2 cols × 1 row, laid out down-and-right from the origin → x ∈ [0, 2], y ∈ [-1, 0].
    assert_eq!(
        entities[2],
        EntityBounds {
            entity: text,
            world_aabb: WorldAabb {
                min: [0.0, -1.0, 0.0],
                max: [2.0, 0.0, 0.0],
            },
            screen_rect: Some(ScreenRect {
                x: 128.0,
                y: 128.0,
                w: 128.0,
                h: 64.0,
            }),
            on_screen: true,
            hidden: false,
        }
    );
}

/// A hidden entity (or one under a hidden ancestor) is still reported, marked `hidden: true`; a visible
/// one serializes with no `hidden` key.
#[test]
fn a_hidden_entity_is_reported_with_hidden_true() {
    let mut engine = engine();
    let world = engine.world_mut();
    add_camera(world);
    let root = world.spawn();
    world.insert(root, Transform::default()).unwrap();
    world
        .insert(
            root,
            ludion_engine::Visibility {
                visible: false,
                ..Default::default()
            },
        )
        .unwrap();
    let child = world.spawn();
    world.insert(child, Transform::default()).unwrap();
    world.insert(child, ludion_ecs::Parent(root)).unwrap();
    world.insert(child, Mesh::default()).unwrap();
    let shown = world.spawn();
    world.insert(shown, Transform::default()).unwrap();
    world.insert(shown, Mesh::default()).unwrap();

    let (_, entities) = bounds(&mut engine);
    assert_eq!(entities.len(), 2);
    assert_eq!((entities[0].entity, entities[0].hidden), (child, true));
    assert_eq!((entities[1].entity, entities[1].hidden), (shown, false));
    let json = serde_json::to_value(&entities[1]).unwrap();
    assert!(json.get("hidden").is_none(), "omitted when false: {json}");
    let json = serde_json::to_value(&entities[0]).unwrap();
    assert_eq!(json["hidden"], true);
}

/// A fresh per-process scratch project root holding `files` (rel path → text).
fn scratch(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-bounds-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (rel, text) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

/// A camera plus one mesh entity of `shape` at `scale`; returns that entity's bounds.
fn one_mesh(engine: &mut Engine, shape: MeshSource, scale: f32) -> EntityBounds {
    let world = engine.world_mut();
    add_camera(world);
    let model = world.spawn();
    world
        .insert(
            model,
            Transform {
                scale: [scale; 3],
                ..Transform::default()
            },
        )
        .unwrap();
    world
        .insert(
            model,
            Mesh {
                shape,
                ..Mesh::default()
            },
        )
        .unwrap();
    let (_, mut entities) = bounds(engine);
    assert_eq!(entities.len(), 1);
    entities.remove(0)
}

/// An assembly authored 4 × 6 × 2 with a 1-voxel `body` and a 2 × 3 × 1 `arm`.
const ASSEMBLY: &str = "\
format: 1
kind: assembly
name: hero
size:
  - 4
  - 6
  - 2
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: arm
    file: parts/arm.part.yaml
    parent: body
    place:
      - 2
      - 1
      - 0
";
const BODY_PART: &str =
    "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"A\"\n";
const ARM_PART: &str = "format: 1\nkind: part\nsize:\n  - 2\n  - 3\n  - 1\nslices:\n  -\n    - \"AA\"\n  -\n    - \"AA\"\n  -\n    - \"AA\"\n";

fn assembly_root(name: &str) -> std::path::PathBuf {
    scratch(
        name,
        &[
            ("models/hero.assembly.yaml", ASSEMBLY),
            ("models/parts/body.part.yaml", BODY_PART),
            ("models/parts/arm.part.yaml", ARM_PART),
        ],
    )
}

/// An assembly entity is bounded by its assembled grid — bottom-centered, 1 voxel = 1 unit, scaled
/// and placed by its `Transform`. The oracle is the `size: [4, 6, 2]` the fixture text authors, at
/// scale 2: x ∈ ±4, y ∈ [0, 12], z ∈ ±2.
#[test]
fn assembly_bounds_are_its_assembled_grid() {
    let dir = assembly_root("assembly");
    let mut engine = Engine::with_project_root(&dir).with_render_size(256, 256);
    let got = one_mesh(
        &mut engine,
        MeshSource::Assembly("models/hero.assembly.yaml".to_string()),
        2.0,
    );
    assert_eq!(
        got.world_aabb,
        WorldAabb {
            min: [-4.0, 0.0, -2.0],
            max: [4.0, 12.0, 2.0],
        }
    );
    // sx = 64·x + 128 → [-128, 384]; sy = 128 − 64·y → y ∈ [0, 12] → [-640, 128].
    assert_eq!(
        got.screen_rect,
        Some(ScreenRect {
            x: -128.0,
            y: -640.0,
            w: 512.0,
            h: 768.0,
        })
    );
    assert!(got.on_screen);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A single `AssemblyPart` entity is bounded by that part's own grid (`arm` is 2 × 3 × 1).
#[test]
fn an_assembly_part_is_bounded_by_its_own_grid() {
    let dir = assembly_root("part");
    let mut engine = Engine::with_project_root(&dir).with_render_size(256, 256);
    let got = one_mesh(
        &mut engine,
        MeshSource::AssemblyPart {
            assembly: "models/hero.assembly.yaml".to_string(),
            part: "arm".to_string(),
        },
        1.0,
    );
    assert_eq!(
        got.world_aabb,
        WorldAabb {
            min: [-1.0, 0.0, -0.5],
            max: [1.0, 3.0, 0.5],
        }
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A 3-wide, 2-deep voxel document of `layers` layers.
fn voxel_doc(layers: usize) -> String {
    let layer = "  -\n    - \"AAA\"\n    - \"AAA\"\n";
    format!(
        "format: 1\nkind: voxel\nsize:\n  - 3\n  - {layers}\n  - 2\npalette:\n  A:\n    color:\n      - 1.0\n      - 0.5\n      - 0.2\n      - 1.0\nslices:\n{}",
        layer.repeat(layers)
    )
}

/// Loading the same scene again re-validates the cached assets before `render.bounds` reads them, even
/// though no render ever runs: an edited voxel document is sized afresh, not served stale.
#[test]
fn a_reloaded_scene_bounds_an_edited_voxel_model_afresh() {
    let path = "voxels/v.voxel.yaml";
    let dir = scratch("reload", &[(path, &voxel_doc(5))]);
    let scene = "format: 1\nentities:\n  - components:\n      Camera: {}\n  - components:\n      Mesh:\n        shape:\n          voxel: \"voxels/v.voxel.yaml\"\n";
    let mut engine = Engine::with_project_root(&dir).with_render_size(256, 256);

    engine.load_inline_scene(scene).expect("the scene loads");
    let (_, entities) = bounds(&mut engine);
    assert_eq!(entities[0].world_aabb.max[1], 5.0);

    std::fs::write(dir.join(path), voxel_doc(3)).unwrap();
    engine.load_inline_scene(scene).expect("the scene reloads");
    let (_, entities) = bounds(&mut engine);
    assert_eq!(entities[0].world_aabb.max[1], 3.0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A voxel model is bounded by its grid: 3 × 5 × 2 → x ∈ ±1.5, y ∈ [0, 5], z ∈ ±1.
#[test]
fn a_voxel_model_is_bounded_by_its_grid() {
    let layer = "  -\n    - \"AAA\"\n    - \"AAA\"\n";
    let doc = format!(
        "format: 1\nkind: voxel\nsize:\n  - 3\n  - 5\n  - 2\npalette:\n  A:\n    color:\n      - 1.0\n      - 0.5\n      - 0.2\n      - 1.0\nslices:\n{}",
        layer.repeat(5)
    );
    let dir = scratch("voxel", &[("voxels/v.voxel.yaml", &doc)]);
    let mut engine = Engine::with_project_root(&dir).with_render_size(256, 256);
    let got = one_mesh(
        &mut engine,
        MeshSource::Voxel("voxels/v.voxel.yaml".to_string()),
        1.0,
    );
    assert_eq!(
        got.world_aabb,
        WorldAabb {
            min: [-1.5, 0.0, -1.0],
            max: [1.5, 5.0, 1.0],
        }
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// An asset mesh is bounded by its vertex extent: the three authored vertices span
/// x ∈ [-1, 3], y ∈ [-2, 1], z ∈ [2, 5].
#[test]
fn an_asset_mesh_is_bounded_by_its_vertex_extent() {
    let doc = "format: 1\nkind: mesh\npositions:\n  - -1.0\n  - 0.0\n  - 2.0\n  - 3.0\n  - 1.0\n  - 2.0\n  - 0.0\n  - -2.0\n  - 5.0\nindices:\n  - 0\n  - 1\n  - 2\n";
    let dir = scratch("asset", &[("meshes/m.mesh.yaml", doc)]);
    let mut engine = Engine::with_project_root(&dir).with_render_size(256, 256);
    let got = one_mesh(
        &mut engine,
        MeshSource::Asset("meshes/m.mesh.yaml".to_string()),
        1.0,
    );
    assert_eq!(
        got.world_aabb,
        WorldAabb {
            min: [-1.0, -2.0, 2.0],
            max: [3.0, 1.0, 5.0],
        }
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `render.bounds` answers for a broken scene: a model whose document is missing keeps the
/// bottom-centered unit box (at scale 2 → x,z ∈ ±1, y ∈ [0, 2]), a mesh asset the ±0.5 box.
#[test]
fn a_missing_document_falls_back_to_the_unit_box() {
    for (shape, min, max) in [
        (
            MeshSource::Assembly("models/absent.assembly.yaml".to_string()),
            [-1.0, 0.0, -1.0],
            [1.0, 2.0, 1.0],
        ),
        (
            MeshSource::Voxel("voxels/absent.voxel.yaml".to_string()),
            [-1.0, 0.0, -1.0],
            [1.0, 2.0, 1.0],
        ),
        (
            MeshSource::Asset("meshes/absent.mesh.yaml".to_string()),
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
        ),
    ] {
        let mut engine = engine();
        let got = one_mesh(&mut engine, shape, 2.0);
        assert_eq!(got.world_aabb, WorldAabb { min, max });
    }
}

/// A sprite under a non-unit `Transform.scale` has a world AABB of `size × scale` — the regression
/// lock for the half-size bug (extract once passed half-extents into a shader whose unit quad already
/// spanned [-0.5, 0.5]). Because `render.bounds` and a real render both size through `extract`'s
/// `place`, this asserts the drawn size *is* the reported size: a 2×1 sprite at scale [3, 2] spans
/// 6×2 world units, not 3×1.
#[test]
fn sprite_bounds_honor_transform_scale() {
    let mut engine = engine();
    let world = engine.world_mut();
    add_camera(world);

    let sprite = world.spawn();
    world
        .insert(
            sprite,
            Transform {
                scale: [3.0, 2.0, 1.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world
        .insert(
            sprite,
            Sprite {
                size: Some([2.0, 1.0]),
                ..Sprite::default()
            },
        )
        .unwrap();

    let (_, entities) = bounds(&mut engine);
    assert_eq!(entities.len(), 1);
    // Full size = size × scale = 6 × 2, centered at origin → x ∈ [-3, 3], y ∈ [-1, 1].
    assert_eq!(
        entities[0].world_aabb,
        WorldAabb {
            min: [-3.0, -1.0, 0.0],
            max: [3.0, 1.0, 0.0],
        }
    );
    // sx = 64·x + 128 → [-64, 320]; sy = 128 − 64·y → [64, 192]. Extends off both sides but is still
    // partly visible, so on_screen holds.
    assert_eq!(
        entities[0].screen_rect,
        Some(ScreenRect {
            x: -64.0,
            y: 64.0,
            w: 384.0,
            h: 128.0,
        })
    );
    assert!(entities[0].on_screen);
}

/// A `Tilemap` reports as **one** renderable whose world AABB is the union of every cell (the honest
/// bound — a tilemap is a single entity). A 2×2 grid of `cell_size = 2` at the origin spans the four
/// cell quads, each `±1` around its center: x ∈ [−1, 3], y ∈ [−3, 1].
#[test]
fn tilemap_reports_one_entry_with_the_whole_grid_aabb() {
    let mut engine = engine();
    let world = engine.world_mut();
    add_camera(world); // slot 0

    let mut legend = BTreeMap::new();
    legend.insert("#".to_string(), TileAppearance::default());
    let grid = world.spawn(); // slot 1
    world.insert(grid, Transform::default()).unwrap();
    world
        .insert(
            grid,
            Tilemap {
                cell_size: 2.0,
                legend,
                rows: vec!["##".to_string(), "##".to_string()],
                layer: 0,
            },
        )
        .unwrap();

    let (_, entities) = bounds(&mut engine);
    assert_eq!(
        entities.len(),
        1,
        "a tilemap is one renderable, not one per cell"
    );
    assert_eq!(entities[0].entity, grid);
    assert_eq!(
        entities[0].world_aabb,
        WorldAabb {
            min: [-1.0, -3.0, 0.0],
            max: [3.0, 1.0, 0.0],
        }
    );
}

/// A `MeshGrid` reports as **one** renderable whose world AABB is the union of its cells: two unit
/// cubes at `cell_size = 2` along +X span x ∈ [−0.5, 2.5]; the hole in the second row adds nothing.
#[test]
fn mesh_grid_reports_one_entry_with_the_whole_grid_aabb() {
    let mut engine = engine();
    let world = engine.world_mut();
    add_camera(world); // slot 0

    let grid = world.spawn(); // slot 1
    world.insert(grid, Transform::default()).unwrap();
    world
        .insert(
            grid,
            MeshGrid {
                cell_size: 2.0,
                legend: BTreeMap::from([("#".to_string(), Mesh::default())]),
                rows: vec!["##".to_string(), "  ".to_string()],
                ..MeshGrid::default()
            },
        )
        .unwrap();

    let (_, entities) = bounds(&mut engine);
    assert_eq!(entities.len(), 1, "a mesh grid is one renderable");
    assert_eq!(entities[0].entity, grid);
    assert_eq!(
        entities[0].world_aabb,
        WorldAabb {
            min: [-0.5, -0.5, -0.5],
            max: [2.5, 0.5, 0.5],
        }
    );
}

/// An entity entirely behind the camera keeps its (well-defined) world AABB but reports `screen_rect:
/// null` and `on_screen: false` — an agent must never mistake it for something on screen.
#[test]
fn behind_camera_entity_has_no_screen_rect() {
    let mut engine = engine();
    let world = engine.world_mut();

    // A *perspective* camera at the origin looking down −Z (an orthographic box behind its near
    // plane is clipped away too, but a perspective camera is the natural framing for "behind").
    let cam = world.spawn();
    world.insert(cam, Transform::default()).unwrap();
    world.insert(cam, Camera::default()).unwrap();

    // A sprite at z = +5 is behind a camera that looks toward −z.
    let sprite = world.spawn();
    world
        .insert(
            sprite,
            Transform {
                translation: [0.0, 0.0, 5.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world.insert(sprite, Sprite::default()).unwrap();

    let (_, entities) = bounds(&mut engine);
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].entity, sprite);
    assert_eq!(entities[0].screen_rect, None);
    assert!(!entities[0].on_screen);
    // The world AABB is still reported — geometry is camera-independent.
    assert_eq!(entities[0].world_aabb.max[2], 5.0);
    assert_eq!(entities[0].world_aabb.min[2], 5.0);
}

/// A scene with no camera is `L2102` (a projected rect needs a real viewpoint — unlike a render,
/// which would fall back to a default framing).
#[test]
fn no_camera_is_l2102() {
    let mut engine = engine();
    let world = engine.world_mut();
    let sprite = world.spawn();
    world.insert(sprite, Sprite::default()).unwrap();

    let error = dispatch(
        &mut engine,
        Command::RenderBounds(RenderBoundsParams::default()),
    )
    .expect_err("no camera must fail");
    assert_eq!(error.code.as_str(), "L2102");
}

/// More than one camera is `L2101` — identical to a render's camera rule, naming the second camera.
#[test]
fn multiple_cameras_is_l2101() {
    let mut engine = engine();
    let world = engine.world_mut();
    let a = world.spawn();
    world.insert(a, Camera::default()).unwrap();
    let b = world.spawn();
    world.insert(b, Camera::default()).unwrap();

    let error = dispatch(
        &mut engine,
        Command::RenderBounds(RenderBoundsParams::default()),
    )
    .expect_err("two cameras must fail");
    assert_eq!(error.code.as_str(), "L2101");
    assert_eq!(error.path.as_deref(), Some("/entities/1/components/Camera"));
}

/// Same world → byte-identical serialized response (the determinism guarantee).
#[test]
fn render_bounds_is_deterministic() {
    let mut engine = engine();
    let world = engine.world_mut();
    add_camera(world);
    let sprite = world.spawn();
    world.insert(sprite, Transform::default()).unwrap();
    world.insert(sprite, Sprite::default()).unwrap();
    let mesh = world.spawn();
    world
        .insert(
            mesh,
            Transform {
                translation: [1.0, 0.5, 0.0],
                rotation: [10.0, 20.0, 30.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world.insert(mesh, Mesh::default()).unwrap();

    let first = dispatch(
        &mut engine,
        Command::RenderBounds(RenderBoundsParams::default()),
    )
    .unwrap()
    .body;
    let second = dispatch(
        &mut engine,
        Command::RenderBounds(RenderBoundsParams::default()),
    )
    .unwrap()
    .body;
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
        "render.bounds must be a deterministic function of the world",
    );
}

/// An atlas `Sprite` that authors no `size` is bounded by its region's own pixels —
/// `rect / pixels_per_unit` — not by the untextured quad's `[1, 1]`. `place` resolves the atlas, so
/// the reported bound is the drawn quad exactly (the sizing rule), and the second entity locks the
/// other half: an atlas that does not load costs that quad its derived size and nothing else, so
/// `render.bounds` still answers for a broken scene.
#[test]
fn an_un_sized_atlas_sprite_is_bounded_by_its_region() {
    let dir = std::env::temp_dir().join(format!("ludion-bounds-atlas-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let png = ludion_assets::encode_png(8, 8, &[255u8; 8 * 8 * 4]).unwrap();
    std::fs::write(dir.join("ship.png"), &png).unwrap();

    let mut regions = BTreeMap::new();
    regions.insert(
        "ship/idle".to_string(),
        ludion_scene::AtlasRegion {
            // 8 × 2 texels at 4 texels per unit ⇒ a 2.0 × 0.5 quad.
            rect: [0, 0, 8, 2],
            pivot: [4.0, 1.0],
        },
    );
    let atlas = ludion_scene::AtlasDocument {
        generated_from: "ship.atlas-source.yaml".to_string(),
        pixels_per_unit: 4.0,
        color_map: "ship.png".to_string(),
        filter: ludion_scene::AtlasFilter::Nearest,
        regions,
        flipbooks: Vec::new(),
    };
    std::fs::write(
        dir.join("ship.atlas.yaml"),
        ludion_scene::emit_atlas(&atlas),
    )
    .unwrap();

    let mut engine = Engine::with_project_root(&dir).with_render_size(256, 256);
    let world = engine.world_mut();
    add_camera(world);

    let sprite = world.spawn();
    world.insert(sprite, Transform::default()).unwrap();
    world
        .insert(
            sprite,
            Sprite {
                atlas: Some("ship.atlas.yaml".to_string()),
                frame: Some("ship/idle".to_string()),
                ..Sprite::default()
            },
        )
        .unwrap();

    let broken = world.spawn();
    world
        .insert(
            broken,
            Transform {
                translation: [0.0, 1.0, 0.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world
        .insert(
            broken,
            Sprite {
                atlas: Some("absent.atlas.yaml".to_string()),
                frame: Some("ship/idle".to_string()),
                ..Sprite::default()
            },
        )
        .unwrap();

    let (_, entities) = bounds(&mut engine);
    assert_eq!(entities.len(), 2);

    // 2.0 × 0.5 centered at the origin → x ∈ [-1, 1], y ∈ [-0.25, 0.25].
    assert_eq!(
        entities[0].world_aabb,
        WorldAabb {
            min: [-1.0, -0.25, 0.0],
            max: [1.0, 0.25, 0.0],
        }
    );
    // sx = 64·x + 128 → [64, 192]; sy = 128 − 64·y → [112, 144].
    assert_eq!(
        entities[0].screen_rect,
        Some(ScreenRect {
            x: 64.0,
            y: 112.0,
            w: 128.0,
            h: 32.0,
        })
    );

    // The unresolvable atlas falls back to the untextured quad's 1 × 1, still reported.
    assert_eq!(
        entities[1].world_aabb,
        WorldAabb {
            min: [-0.5, 0.5, 0.0],
            max: [0.5, 1.5, 0.0],
        }
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A 2×1 sprite at the origin under the exact-math camera, plus a presentation `CameraOffset` that
/// shifts the camera 1 m right, in a fresh engine. Returns the engine and the sprite.
fn offset_world(offset: bool) -> (Engine, ludion_core::ids::EntityId) {
    let mut engine = engine();
    let world = engine.world_mut();
    add_camera(world);
    let sprite = world.spawn();
    world.insert(sprite, Transform::default()).unwrap();
    world
        .insert(
            sprite,
            Sprite {
                size: Some([2.0, 1.0]),
                ..Sprite::default()
            },
        )
        .unwrap();
    if offset {
        let intro = world.spawn_presentation();
        world
            .insert(
                intro,
                CameraOffset {
                    translation: [1.0, 0.0, 0.0],
                    ..CameraOffset::default()
                },
            )
            .unwrap();
    }
    (engine, sprite)
}

fn sprite_rect(engine: &mut Engine, camera: CameraChoice) -> ScreenRect {
    match dispatch(
        engine,
        Command::RenderBounds(RenderBoundsParams { camera, view: None }),
    )
    .expect("render.bounds should succeed")
    .body
    {
        ResponseBody::RenderBounds { entities, .. } => entities[0].screen_rect.unwrap(),
        other => panic!("expected RenderBounds, got {other:?}"),
    }
}

/// T5: the scene camera's bounds project through the offset (the rect the image draws); the
/// `render.view` override's do not; the hash is unchanged.
#[test]
fn bounds_project_through_the_offset_but_not_through_the_view_override() {
    let (mut plain, _) = offset_world(false);
    let (mut moved, _) = offset_world(true);
    let before = plain.world_hash().unwrap();
    assert_eq!(
        moved.world_hash().unwrap(),
        before,
        "a presentation entity is not hashed"
    );

    let base = sprite_rect(&mut plain, CameraChoice::Scene);
    let shifted = sprite_rect(&mut moved, CameraChoice::Scene);
    assert_eq!(base.x, 64.0);
    assert_eq!(shifted.x, 0.0, "the camera moved 1 m right: 64 px left");
    assert_eq!((shifted.w, shifted.h), (base.w, base.h));

    let pose = ViewPose {
        translation: [0.0, 0.0, 10.0],
        rotation: [0.0, 0.0, 0.0],
        fov_y_deg: None,
    };
    for engine in [&mut plain, &mut moved] {
        dispatch(
            engine,
            Command::RenderView(RenderViewParams {
                pose: Some(Some(pose)),
                ..Default::default()
            }),
        )
        .unwrap();
    }
    assert_eq!(
        sprite_rect(&mut moved, CameraChoice::View),
        sprite_rect(&mut plain, CameraChoice::View)
    );
    assert_eq!(moved.world_hash().unwrap(), before);
}

/// A `radar` view of `size` at the origin, looking down −Z from `z = 10` (the exact-math camera),
/// returned as an entity carrying both `Camera` and `RenderView`.
fn add_view(world: &mut World, size: [u32; 2], enabled: bool) {
    let cam = add_camera(world);
    world
        .insert(
            cam,
            RenderView {
                name: "radar".to_string(),
                size,
                enabled,
                ..serde_json::from_str(r#"{"name":"radar"}"#).unwrap()
            },
        )
        .unwrap();
}

fn view_bounds(
    engine: &mut Engine,
    view: &str,
) -> Result<([u32; 2], Vec<EntityBounds>), ludion_core::diagnostics::Diagnostic> {
    let reply = dispatch(
        engine,
        Command::RenderBounds(RenderBoundsParams {
            view: Some(view.to_string()),
            ..Default::default()
        }),
    )?;
    match reply.body {
        ResponseBody::RenderBounds {
            render_size,
            entities,
        } => Ok((render_size, entities)),
        other => panic!("expected RenderBounds, got {other:?}"),
    }
}

/// A 2×1 sprite at the origin, a main camera 100 m to the side (the sprite is off-screen through it)
/// and a `radar` view looking at the origin: the view's bounds project at the view's own size.
#[test]
fn bounds_through_a_named_view_project_at_the_views_size() {
    let mut engine = engine();
    let world = engine.world_mut();
    let main = add_camera(world);
    world
        .insert(
            main,
            Transform {
                translation: [100.0, 0.0, 10.0],
                ..Transform::default()
            },
        )
        .unwrap();
    add_view(world, [128, 64], true);
    let sprite = world.spawn();
    world.insert(sprite, Transform::default()).unwrap();
    world
        .insert(
            sprite,
            Sprite {
                size: Some([2.0, 1.0]),
                ..Sprite::default()
            },
        )
        .unwrap();

    let (scene_size, scene) = bounds(&mut engine);
    assert_eq!(scene_size, [256, 256]);
    assert!(!scene[0].on_screen, "off-screen through the scene camera");

    let (size, entities) = view_bounds(&mut engine, "radar").expect("the view answers");
    assert_eq!(size, [128, 64]);
    let entity = &entities[0];
    assert!(entity.on_screen);
    let rect = entity.screen_rect.expect("a rect");
    assert!(
        rect.x >= 0.0 && rect.y >= 0.0 && rect.x + rect.w <= 128.0 && rect.y + rect.h <= 64.0,
        "{rect:?} is inside 128x64"
    );
}

/// An unknown name is `L2379` and the help names the views that do render.
#[test]
fn bounds_through_an_unknown_view_is_l2379() {
    let mut engine = engine();
    let world = engine.world_mut();
    add_camera(world);
    add_view(world, [128, 64], true);
    let error = view_bounds(&mut engine, "minimap").expect_err("no such view");
    assert_eq!(error.code.as_str(), "L2379");
    let text = format!("{error:?}");
    assert!(text.contains("radar"), "{text}");
}

/// A disabled view does not render, so it is `L2379` too.
#[test]
fn bounds_through_a_disabled_view_is_l2379() {
    let mut engine = engine();
    let world = engine.world_mut();
    add_camera(world);
    add_view(world, [128, 64], false);
    let error = view_bounds(&mut engine, "radar").expect_err("disabled");
    assert_eq!(error.code.as_str(), "L2379");
}

/// A view names its own viewpoint, so a world with only a view camera answers instead of `L2102`.
#[test]
fn bounds_through_a_view_need_no_main_camera() {
    let mut engine = engine();
    let world = engine.world_mut();
    add_view(world, [128, 64], true);
    let sprite = world.spawn();
    world.insert(sprite, Sprite::default()).unwrap();
    let (size, entities) = view_bounds(&mut engine, "radar").expect("answers, not L2102");
    assert_eq!(size, [128, 64]);
    assert_eq!(entities.len(), 1);
}
