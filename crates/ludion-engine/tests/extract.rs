//! `extract()` tests: determinism, camera-count validation, and the default framing.
//!
//! These build a `World` directly from the built-in component types (avoiding YAML flow-collection
//! rules in test strings) and exercise `extract` with no GPU.

use std::path::Path;

use std::collections::BTreeMap;

use ludion_core::math::glam::{Mat4, Quat, Vec3};
use ludion_core::time::{SimTime, TickRate};
use ludion_ecs::World;
use ludion_engine::components::{Anchor, UiList, UiListItem, UiRect, UiText};
use ludion_engine::{
    Camera, Decal, DecalMap, DecalReceiverSet, DropShadow, Environment, Fade, GiDynamic,
    GroundMarker, GroundMarkerShape, MapWrap, Mesh, MeshSource, ParticleEmitter, PointLight,
    PostProcess, QualityParams, SpotLight, Sprite, Text, TileAppearance, Tilemap, Transform,
    Visibility, extract, extract_at,
};
use ludion_renderer::{
    DecalReceivers, GROUND_MARKER_LIFT, GroundMarkerShape as RendererMarkerShape, MapImage,
    MeshShape, RenderCamera, TextureWrap, UiInstance,
};

fn scene_world() -> World {
    let mut world = World::new();

    let camera = world.spawn();
    world
        .insert(
            camera,
            Transform {
                translation: [0.0, 2.0, 8.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world.insert(camera, Camera::default()).unwrap();

    let mesh = world.spawn();
    world.insert(mesh, Transform::default()).unwrap();
    world.insert(mesh, Mesh::default()).unwrap();

    let sprite = world.spawn();
    world
        .insert(
            sprite,
            Transform {
                translation: [1.0, 0.0, 0.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world.insert(sprite, Sprite::default()).unwrap();

    let light = world.spawn();
    world
        .insert(
            light,
            Transform {
                translation: [2.0, 3.0, 4.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world.insert(light, PointLight::default()).unwrap();

    let marker = world.spawn();
    world
        .insert(
            marker,
            Transform {
                translation: [-3.0, 0.0, 1.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world.insert(marker, GroundMarker::default()).unwrap();

    // An endless emitter: in steady state at tick 0, so `extract` sees a full plume with no warm-up.
    let emitter = world.spawn();
    world
        .insert(
            emitter,
            Transform {
                translation: [0.0, 1.0, -2.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world.insert(emitter, ParticleEmitter::default()).unwrap();

    world
}

/// Extracting the same world twice yields byte-equal render scenes (entity order = ascending slot).
#[test]
fn extract_is_deterministic() {
    let world = scene_world();
    let a = extract(&world, (320, 180), Path::new(".")).unwrap();
    let b = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(
        a, b,
        "extract must be a deterministic function of the world"
    );
    assert_eq!(a.meshes.len(), 1);
    assert_eq!(a.sprites.len(), 1);
    assert_eq!(a.lights.len(), 1);
    assert_eq!(a.ground_markers.len(), 1);
    assert!(
        !a.particles.is_empty(),
        "an endless emitter is in steady state at tick 0, so it must draw something"
    );
}

/// A sprite's instance vectors are FULL width/height (`Sprite.size` × `Transform.scale` x/y): a
/// `size: [2, 0.4]` sprite under `scale: [3, 2, 1]` extracts `|right| = 6`, `|up| = 0.8`. Guards
/// against the double-halving bug THE-test's agent found (extract passed half-extents into a shader
/// whose unit quad already spans [-0.5, 0.5], drawing sprites at half their declared size) and
/// against `Transform.scale` being silently ignored for sprites.
#[test]
fn sprite_extents_are_full_size_and_honor_scale() {
    let mut world = World::new();
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
                size: Some([2.0, 0.4]),
                ..Sprite::default()
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    let instance = &scene.sprites[0];
    assert!(
        (instance.right.length() - 6.0).abs() < 1e-6,
        "full width = size.x * scale.x, got {}",
        instance.right.length()
    );
    assert!(
        (instance.up.length() - 0.8).abs() < 1e-6,
        "full height = size.y * scale.y, got {}",
        instance.up.length()
    );
}

/// Two cameras are an `L2101` error naming the second camera's entity.
#[test]
fn multiple_cameras_is_l2101() {
    let mut world = World::new();
    let a = world.spawn();
    world.insert(a, Camera::default()).unwrap();
    let b = world.spawn();
    world.insert(b, Camera::default()).unwrap();

    let errors = extract(&world, (320, 180), Path::new(".")).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L2101");
    // The pointer names the *second* camera (entity index 1).
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/1/components/Camera")
    );
}

/// A `Tilemap` expands into the sprite batcher **row-major**, one sprite per legend-mapped cell, at
/// the documented placement (origin = Transform.translation = center of cell (0,0); +X per column,
/// −Y per row). Asserts count, centers, and colors in exact order.
#[test]
fn tilemap_expands_row_major_into_sprites() {
    let red = [1.0, 0.0, 0.0, 1.0];
    let blue = [0.0, 0.0, 1.0, 1.0];
    let mut legend = BTreeMap::new();
    legend.insert(
        "#".to_string(),
        TileAppearance {
            color: red,
            ..TileAppearance::default()
        },
    );
    legend.insert(
        ".".to_string(),
        TileAppearance {
            color: blue,
            ..TileAppearance::default()
        },
    );

    let mut world = World::new();
    let grid = world.spawn();
    world.insert(grid, Transform::default()).unwrap(); // origin at [0, 0, 0]
    world
        .insert(
            grid,
            Tilemap {
                cell_size: 2.0,
                legend,
                rows: vec!["#.".to_string(), ".#".to_string()],
                layer: 0,
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();

    // 2×2 grid, row-major: (0,0)#, (1,0)., (0,1)., (1,1)#.
    let centers: Vec<[f32; 3]> = scene.sprites.iter().map(|s| s.center.to_array()).collect();
    assert_eq!(
        centers,
        vec![
            [0.0, 0.0, 0.0],  // col 0, row 0
            [2.0, 0.0, 0.0],  // col 1, row 0 (+X)
            [0.0, -2.0, 0.0], // col 0, row 1 (−Y)
            [2.0, -2.0, 0.0], // col 1, row 1
        ]
    );
    let colors: Vec<[f32; 4]> = scene.sprites.iter().map(|s| s.color).collect();
    assert_eq!(colors, vec![red, blue, blue, red]);

    // Each cell is a `cell_size × cell_size` axis-aligned quad.
    for sprite in &scene.sprites {
        assert!((sprite.right.length() - 2.0).abs() < 1e-6);
        assert!((sprite.up.length() - 2.0).abs() < 1e-6);
    }

    // Extraction is a deterministic function of the world.
    let again = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(scene, again);
}

/// An empty tilemap (no rows) renders nothing — the drift-safe `{}` case.
#[test]
fn empty_tilemap_renders_nothing() {
    let mut world = World::new();
    let grid = world.spawn();
    world.insert(grid, Tilemap::default()).unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert!(
        scene.sprites.is_empty(),
        "an empty tilemap draws no sprites"
    );
}

/// A `Mesh.shape: asset` referenced by two entities resolves to **one** `mesh_assets` entry
/// (deduplicated by path), both instances indexing `Asset(0)`; and two extractions of the same world
/// are byte-equal, `mesh_assets` order included — the extract-determinism criterion for asset meshes.
#[test]
fn asset_meshes_dedup_and_extract_deterministically() {
    let dir = std::env::temp_dir().join(format!("ludion-extract-mesh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    std::fs::write(
        dir.join("meshes/tri.mesh.yaml"),
        "format: 1\nkind: mesh\npositions:\n  - 0.0\n  - 0.0\n  - 0.0\n  - 1.0\n  - 0.0\n  - 0.0\n  \
         - 0.0\n  - 1.0\n  - 0.0\nindices:\n  - 0\n  - 1\n  - 2\n",
    )
    .unwrap();

    let mut world = World::new();
    for x in [-1.0_f32, 1.0] {
        let e = world.spawn();
        world
            .insert(
                e,
                Transform {
                    translation: [x, 0.0, 0.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        world
            .insert(
                e,
                Mesh {
                    shape: MeshSource::Asset("meshes/tri.mesh.yaml".to_string()),
                    ..Mesh::default()
                },
            )
            .unwrap();
    }

    let a = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(
        a.mesh_assets.len(),
        1,
        "one document ⇒ one mesh_assets entry"
    );
    assert_eq!(a.meshes.len(), 2);
    assert!(a.meshes.iter().all(|m| m.shape == MeshShape::Asset(0)));
    // Faceted normals were computed at load (the document omitted `normals`).
    assert_eq!(
        a.mesh_assets[0].normals,
        vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]
    );

    let b = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(a, b, "extract of asset meshes must be deterministic");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A skinned asset's instances each get their own copy of its rest palette, offset into the
/// combined `bone_palette` (entry 0 is the renderer's own identity, so an asset's own entries start
/// at 1); an unskinned instance stays at offset 0.
#[test]
fn skinned_instances_extract_with_offset_rest_palettes() {
    use ludion_scene::{JointRest, MeshDocument as SceneMeshDocument, MeshJoint, MeshSkin};

    let dir = std::env::temp_dir().join(format!("ludion-extract-skin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).unwrap();

    let mut identity = [0.0; 16];
    for i in 0..4 {
        identity[i * 5] = 1.0;
    }
    let skinned_doc = SceneMeshDocument {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        skin: Some(MeshSkin {
            joints: vec![
                MeshJoint {
                    name: "root".to_string(),
                    parent: None,
                    rest: JointRest {
                        translation: [0.0, 0.0, 0.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0, 1.0, 1.0],
                    },
                    inverse_bind: identity,
                },
                MeshJoint {
                    name: "child".to_string(),
                    parent: Some(0),
                    rest: JointRest {
                        translation: [0.0, 1.0, 0.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0, 1.0, 1.0],
                    },
                    // Identity, unlike the bind pose: the child's rest palette entry is `T(0,1,0)`,
                    // not identity — distinguishable from the root's in the assertions below.
                    inverse_bind: identity,
                },
            ],
            weights: vec![1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            bone_indices: vec![0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0],
        }),
        indices: vec![0, 1, 2],
        ..SceneMeshDocument::default()
    };
    std::fs::write(
        dir.join("meshes/skinned.mesh.yaml"),
        ludion_scene::emit_mesh(&skinned_doc),
    )
    .unwrap();
    std::fs::write(
        dir.join("meshes/plain.mesh.yaml"),
        "format: 1\nkind: mesh\npositions:\n  - 0.0\n  - 0.0\n  - 0.0\n  - 1.0\n  - 0.0\n  - 0.0\n  \
         - 0.0\n  - 1.0\n  - 0.0\nindices:\n  - 0\n  - 1\n  - 2\n",
    )
    .unwrap();

    let mut world = World::new();
    // Entity order: skinned, unskinned, skinned — so offsets land [1, 0, 3] (entry 0 is the
    // renderer's identity; the doc's own two joints occupy 1..3, then 3..5 for the second instance).
    for source in [
        "meshes/skinned.mesh.yaml",
        "meshes/plain.mesh.yaml",
        "meshes/skinned.mesh.yaml",
    ] {
        let e = world.spawn();
        world.insert(e, Transform::default()).unwrap();
        world
            .insert(
                e,
                Mesh {
                    shape: MeshSource::Asset(source.to_string()),
                    ..Mesh::default()
                },
            )
            .unwrap();
    }

    let a = extract(&world, (320, 180), &dir).unwrap();
    let offsets: Vec<u32> = a.meshes.iter().map(|m| m.bone_offset).collect();
    assert_eq!(offsets, vec![1, 0, 3], "entity order gives these offsets");

    let doc_palette: Vec<Mat4> = a
        .mesh_assets
        .iter()
        .find(|m| !m.skin_joints.is_empty())
        .expect("the skinned asset carries skin joints")
        .skin_joints
        .iter()
        .map(|joint| joint.rest)
        .collect();
    assert_eq!(doc_palette.len(), 2);
    assert_eq!(
        a.bone_palette,
        [doc_palette.clone(), doc_palette].concat(),
        "each skinned instance gets its own copy of the asset's rest palette"
    );

    let b = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(a, b, "extract of skinned meshes must be deterministic");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `Mesh.shape: asset` pointing at a missing document is an `L1101`, re-pathed at the referencing
/// `Mesh` component's asset field (the texture-error style, for meshes).
#[test]
fn missing_asset_mesh_is_l1101_at_the_mesh_component() {
    let dir = std::env::temp_dir().join(format!("ludion-extract-missmesh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            Mesh {
                shape: MeshSource::Asset("meshes/nope.mesh.yaml".to_string()),
                ..Mesh::default()
            },
        )
        .unwrap();

    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1101");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Mesh/shape/asset")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A minimal valid voxel document: a single filled cell (glyph `A`) → a full 24-vertex box.
const VOXEL_ONE_CELL: &str = "\
format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.5
      - 0.2
      - 1.0
slices:
  -
    - \"A\"
";

/// A `Mesh.shape: voxel` referenced by two entities resolves to **one** `mesh_assets` entry
/// (deduplicated by path, so the renderer's hash-keyed GPU cache uploads it once), both instances
/// indexing `Asset(0)`; and two extractions of the world are byte-equal — the voxel analog of
/// `asset_meshes_dedup_and_extract_deterministically`, and the engine-observable "one upload per
/// hash" at the mesh-asset seam voxel models share.
#[test]
fn voxel_meshes_dedup_and_extract_deterministically() {
    let dir = std::env::temp_dir().join(format!("ludion-extract-voxel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("voxels")).unwrap();
    std::fs::write(dir.join("voxels/v.voxel.yaml"), VOXEL_ONE_CELL).unwrap();

    let mut world = World::new();
    for x in [-1.0_f32, 1.0] {
        let e = world.spawn();
        world
            .insert(
                e,
                Transform {
                    translation: [x, 0.0, 0.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        world
            .insert(
                e,
                Mesh {
                    shape: MeshSource::Voxel("voxels/v.voxel.yaml".to_string()),
                    ..Mesh::default()
                },
            )
            .unwrap();
    }

    let a = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(
        a.mesh_assets.len(),
        1,
        "one voxel document ⇒ one mesh_assets entry (one GPU upload)"
    );
    assert_eq!(a.meshes.len(), 2);
    assert!(a.meshes.iter().all(|m| m.shape == MeshShape::Asset(0)));
    // The greedy mesher emitted a full box for the single cell, with per-quad palette colors.
    assert_eq!(a.mesh_assets[0].positions.len(), 24 * 3);
    assert_eq!(a.mesh_assets[0].colors.len(), 24 * 4);

    let b = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(a, b, "extract of voxel meshes must be deterministic");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A broken voxel document surfaces its diagnostic re-pathed at the referencing `Mesh` component's
/// `shape/voxel` field (the voxel analog of the mesh-asset re-pathing), with the message prefixed by
/// the voxel path. Here the row uses glyph `Z`, which names no palette entry → `L1114`.
#[test]
fn broken_voxel_doc_surfaces_at_the_mesh_component() {
    let dir = std::env::temp_dir().join(format!("ludion-extract-badvoxel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("voxels")).unwrap();
    std::fs::write(
        dir.join("voxels/bad.voxel.yaml"),
        "format: 1\nkind: voxel\nsize:\n  - 2\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      - \
         1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - AZ\n",
    )
    .unwrap();

    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            Mesh {
                shape: MeshSource::Voxel("voxels/bad.voxel.yaml".to_string()),
                ..Mesh::default()
            },
        )
        .unwrap();

    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1114");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Mesh/shape/voxel")
    );
    assert!(
        errors[0]
            .message
            .contains("voxel model `voxels/bad.voxel.yaml`"),
        "message names the voxel document: {}",
        errors[0].message
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A broken sculpt document surfaces its diagnostics re-pathed at the referencing `Mesh` component's
/// `shape/sculpt` field, with the message prefixed by the sculpt path. Here the part's `material`
/// names no palette glyph → `L1157`.
#[test]
fn broken_sculpt_doc_surfaces_at_the_mesh_component() {
    let dir = std::env::temp_dir().join(format!("ludion-extract-badsculpt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sculpts")).unwrap();
    std::fs::write(
        dir.join("sculpts/bad.sculpt.yaml"),
        "format: 1\nkind: sculpt\nresolution: 16\ntriangles: 200\npalette:\n  S:\n    color:\n      \
         - 0.5\n      - 0.5\n      - 0.5\n      - 1.0\nbones:\n  - name: root\nparts:\n  - name: \
         ball\n    shape: sphere\n    radius: 0.5\n    material: Z\n",
    )
    .unwrap();

    let mut world = World::new();
    world.spawn();
    let e = world.spawn();
    world
        .insert(
            e,
            Mesh {
                shape: MeshSource::Sculpt("sculpts/bad.sculpt.yaml".to_string()),
                ..Mesh::default()
            },
        )
        .unwrap();

    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert!(!errors.is_empty());
    for error in &errors {
        assert_eq!(error.code.as_str(), "L1157");
        assert_eq!(
            error.path.as_deref(),
            Some("/entities/1/components/Mesh/shape/sculpt")
        );
        assert!(
            error
                .message
                .contains("sculpt model `sculpts/bad.sculpt.yaml`"),
            "message names the sculpt document: {}",
            error.message
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `Mesh.shape: voxel` pointing at a missing document is an `L1101`, re-pathed at the referencing
/// `Mesh` component's `voxel` field.
#[test]
fn missing_voxel_doc_is_l1101_at_the_mesh_component() {
    let dir = std::env::temp_dir().join(format!("ludion-extract-missvoxel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            Mesh {
                shape: MeshSource::Voxel("voxels/nope.voxel.yaml".to_string()),
                ..Mesh::default()
            },
        )
        .unwrap();

    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1101");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Mesh/shape/voxel")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A two-part assembly: a 2×1×1 `body` bar at the grid origin and a single-cell `arm` parented to it,
/// in a 4×2×1 assembled grid. Its rest offsets are hand-computable (see the test): `body` at
/// `[-1, 0, 0]`, `arm` at `[0.5, 1, 0]`.
const TWO_PART_ASSEMBLY: &str = "\
format: 1
kind: assembly
name: fixture
size:
  - 4
  - 2
  - 1
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

/// A `kind: part` document of `size` with one row string (one y layer, one z row).
fn part_source(size: [u32; 3], row: &str) -> String {
    format!(
        "format: 1\nkind: part\nsize:\n  - {}\n  - {}\n  - {}\nslices:\n  -\n    - \"{row}\"\n",
        size[0], size[1], size[2]
    )
}

/// Writes [`TWO_PART_ASSEMBLY`] and its two part files into a fresh directory, returning it.
fn assembly_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-extract-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("models/parts")).unwrap();
    std::fs::write(dir.join("models/fixture.assembly.yaml"), TWO_PART_ASSEMBLY).unwrap();
    std::fs::write(
        dir.join("models/parts/body.part.yaml"),
        part_source([2, 1, 1], "AA"),
    )
    .unwrap();
    std::fs::write(
        dir.join("models/parts/arm.part.yaml"),
        part_source([1, 1, 1], "A"),
    )
    .unwrap();
    dir
}

/// A `Mesh.shape: assembly` on **one** entity expands into one `MeshInstance` per part, each carrying
/// the entity's own transform composed with that part's rest offset inside the assembled grid — so a
/// whole model moves and scales as one. The offsets are the pinned rest-pose contract:
/// `place + part_size/2 − assembly_size/2` on x/z, `place.y` on y.
#[test]
fn assembly_expands_to_one_instance_per_part_at_rest() {
    let dir = assembly_dir("assembly");

    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            Transform {
                translation: [3.0, 0.0, -2.0],
                scale: [2.0, 2.0, 2.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world
        .insert(
            e,
            Mesh {
                shape: MeshSource::Assembly("models/fixture.assembly.yaml".to_string()),
                ..Mesh::default()
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(
        scene.mesh_assets.len(),
        2,
        "one mesh_assets entry per part (each its own GPU upload)"
    );
    assert_ne!(
        scene.mesh_assets[0].hash, scene.mesh_assets[1].hash,
        "two parts of one assembly must not share a GPU cache key"
    );
    assert_eq!(scene.meshes.len(), 2, "one instance per part, one entity");
    assert_eq!(scene.meshes[0].shape, MeshShape::Asset(0));
    assert_eq!(scene.meshes[1].shape, MeshShape::Asset(1));

    // The entity's own transform, exactly as `place` builds it.
    let entity = Mat4::from_scale_rotation_translation(
        Vec3::splat(2.0),
        Quat::IDENTITY,
        Vec3::new(3.0, 0.0, -2.0),
    );
    // `body`: place 0 + 2/2 − 4/2 = −1 on x, place 0 on y, 0 + 1/2 − 1/2 = 0 on z.
    assert_eq!(
        scene.meshes[0].model,
        entity * Mat4::from_translation(Vec3::new(-1.0, 0.0, 0.0))
    );
    // `arm`: place 2 + 1/2 − 2 = 0.5 on x, place 1 on y, 0 on z.
    assert_eq!(
        scene.meshes[1].model,
        entity * Mat4::from_translation(Vec3::new(0.5, 1.0, 0.0))
    );
    // Read as world positions: the part mesh origins land at the entity's translation plus its scale
    // times each rest offset.
    assert_eq!(
        scene.meshes[0].model.transform_point3(Vec3::ZERO),
        Vec3::new(1.0, 0.0, -2.0)
    );
    assert_eq!(
        scene.meshes[1].model.transform_point3(Vec3::ZERO),
        Vec3::new(4.0, 2.0, -2.0)
    );
    // A rest placement is a translation, so every part keeps the entity's own normal matrix, and the
    // assembly's palette authors the material (the `Mesh` scalars stay inert, as for a voxel model).
    assert!(
        scene
            .meshes
            .iter()
            .all(|m| m.normal_matrix == scene.meshes[0].normal_matrix
                && m.use_vertex_material == scene.mesh_assets[0].authored_material)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Two entities drawing the same assembly share one run of `mesh_assets` entries (one GPU upload per
/// part, not per entity), the instances come out in ascending-entity then authored-part order, and two
/// extractions of the world are byte-equal — the assembly analog of
/// `voxel_meshes_dedup_and_extract_deterministically`.
#[test]
fn assembly_parts_dedup_and_extract_deterministically() {
    let dir = assembly_dir("assembly-dedup");

    let mut world = World::new();
    for x in [-4.0_f32, 4.0] {
        let e = world.spawn();
        world
            .insert(
                e,
                Transform {
                    translation: [x, 0.0, 0.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        world
            .insert(
                e,
                Mesh {
                    shape: MeshSource::Assembly("models/fixture.assembly.yaml".to_string()),
                    ..Mesh::default()
                },
            )
            .unwrap();
    }

    let a = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(
        a.mesh_assets.len(),
        2,
        "one assembly document ⇒ one entry per part, however many entities draw it"
    );
    assert_eq!(a.meshes.len(), 4);
    let shapes: Vec<MeshShape> = a.meshes.iter().map(|m| m.shape).collect();
    assert_eq!(
        shapes,
        vec![
            MeshShape::Asset(0),
            MeshShape::Asset(1),
            MeshShape::Asset(0),
            MeshShape::Asset(1)
        ],
        "ascending entity, then authored part order"
    );
    // The first entity sits at x = −4, the second at x = +4; both keep the same part order.
    assert_eq!(a.meshes[0].model.transform_point3(Vec3::ZERO).x, -5.0);
    assert_eq!(a.meshes[2].model.transform_point3(Vec3::ZERO).x, 3.0);

    let b = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(a, b, "extract of assemblies must be deterministic");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A broken *part* document surfaces its own diagnostic re-pathed at the referencing `Mesh`
/// component's `shape/assembly` field, with the message naming both the assembly and the failing part
/// — a part pointer alone (`/slices/0/0`) never says which of an assembly's files it means. Here the
/// body's row is one glyph wide where its `size.x` says two → `L1140`.
#[test]
fn broken_assembly_part_surfaces_at_the_mesh_component() {
    let dir = assembly_dir("badassembly");
    std::fs::write(
        dir.join("models/parts/body.part.yaml"),
        part_source([2, 1, 1], "A"),
    )
    .unwrap();

    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            Mesh {
                shape: MeshSource::Assembly("models/fixture.assembly.yaml".to_string()),
                ..Mesh::default()
            },
        )
        .unwrap();

    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1140");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Mesh/shape/assembly")
    );
    assert!(
        errors[0]
            .message
            .contains("assembly `models/fixture.assembly.yaml`")
            && errors[0].message.contains("part `body`"),
        "message names the assembly and the failing part: {}",
        errors[0].message
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `Mesh.shape: assembly` pointing at a missing document is an `L1101`, re-pathed at the referencing
/// `Mesh` component's `assembly` field.
#[test]
fn missing_assembly_is_l1101_at_the_mesh_component() {
    let dir = std::env::temp_dir().join(format!("ludion-extract-missasm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            Mesh {
                shape: MeshSource::Assembly("models/nope.assembly.yaml".to_string()),
                ..Mesh::default()
            },
        )
        .unwrap();

    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1101");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Mesh/shape/assembly")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `Mesh.shape: {assembly_part: {assembly, part}}` entity draws exactly one instance — the named
/// part's own geometry, indexed into the very same GPU-upload run a whole-assembly `Mesh` shares —
/// posed by the entity's own transform directly, with no rest-offset composed in (unlike a
/// whole-assembly `Mesh`, this entity already *is* one part, so its `Transform` is the part's own
/// world pose, the ragdoll switch's contract).
#[test]
fn assembly_part_draws_one_instance_at_the_entitys_own_transform() {
    let dir = assembly_dir("assembly-part");

    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            Transform {
                translation: [5.0, 1.0, -3.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world
        .insert(
            e,
            Mesh {
                shape: MeshSource::AssemblyPart {
                    assembly: "models/fixture.assembly.yaml".to_string(),
                    part: "arm".to_string(),
                },
                ..Mesh::default()
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(
        scene.mesh_assets.len(),
        2,
        "the whole assembly's part run is still uploaded, in authored order"
    );
    assert_eq!(scene.meshes.len(), 1, "one instance for the one entity");
    assert_eq!(
        scene.meshes[0].shape,
        MeshShape::Asset(1),
        "`arm` is the assembly's second part"
    );
    let entity = Mat4::from_translation(Vec3::new(5.0, 1.0, -3.0));
    assert_eq!(
        scene.meshes[0].model, entity,
        "the entity's own transform, with no rest-offset composed in"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Two `assembly_part` entities naming the same assembly, plus a whole-assembly `Mesh`, all share
/// the one dedup run — the assembly-part analog of `assembly_parts_dedup_and_extract_deterministically`.
#[test]
fn assembly_part_shares_the_whole_assembly_dedup_run() {
    let dir = assembly_dir("assembly-part-dedup");

    let mut world = World::new();
    let body = world.spawn();
    world.insert(body, Transform::default()).unwrap();
    world
        .insert(
            body,
            Mesh {
                shape: MeshSource::AssemblyPart {
                    assembly: "models/fixture.assembly.yaml".to_string(),
                    part: "body".to_string(),
                },
                ..Mesh::default()
            },
        )
        .unwrap();
    let whole = world.spawn();
    world.insert(whole, Transform::default()).unwrap();
    world
        .insert(
            whole,
            Mesh {
                shape: MeshSource::Assembly("models/fixture.assembly.yaml".to_string()),
                ..Mesh::default()
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(
        scene.mesh_assets.len(),
        2,
        "one entry per part — one document, however many entities reference it"
    );
    assert_eq!(
        scene.meshes.len(),
        3,
        "one for `body` alone, two for the whole assembly"
    );
    assert_eq!(scene.meshes[0].shape, MeshShape::Asset(0));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `part` the named assembly does not have is `L1185`/`MESH_PART_UNKNOWN`, pointed at the
/// `assembly_part/part` field; the message names the missing part and the assembly.
#[test]
fn unknown_assembly_part_is_l1185_at_its_pointer() {
    let dir = assembly_dir("assembly-part-unknown");

    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Transform::default()).unwrap();
    world
        .insert(
            e,
            Mesh {
                shape: MeshSource::AssemblyPart {
                    assembly: "models/fixture.assembly.yaml".to_string(),
                    part: "leg".to_string(),
                },
                ..Mesh::default()
            },
        )
        .unwrap();

    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1185");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Mesh/shape/assembly_part/part")
    );
    assert!(
        errors[0].message.contains("`leg`")
            && errors[0].message.contains("models/fixture.assembly.yaml"),
        "{}",
        errors[0].message
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A missing assembly document is the ordinary asset-read diagnostic, re-pathed at the sibling
/// `assembly_part/assembly` field rather than the whole-assembly `assembly` one.
#[test]
fn missing_assembly_part_document_is_l1101_at_its_pointer() {
    let dir = std::env::temp_dir().join(format!(
        "ludion-extract-missassemblypart-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            Mesh {
                shape: MeshSource::AssemblyPart {
                    assembly: "models/nope.assembly.yaml".to_string(),
                    part: "arm".to_string(),
                },
                ..Mesh::default()
            },
        )
        .unwrap();

    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1101");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Mesh/shape/assembly_part/assembly")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A scene with no camera uses the documented default framing.
#[test]
fn zero_cameras_uses_default_framing() {
    let mut world = World::new();
    let mesh = world.spawn();
    world.insert(mesh, Mesh::default()).unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(scene.camera, RenderCamera::default_framing((320, 180)));
}

// --- Phase 14 Wave 2: RenderScene.ui, resolve + extraction ---

/// `RenderScene.ui` is pre-sorted by `(order, EntityId)`: three entities spawned in an order that
/// disagrees with their `order` field, with two sharing `order` (tie broken by ascending EntityId,
/// i.e. spawn order for freshly spawned entities) — the pinned determinism contract.
#[test]
fn ui_instances_are_sorted_by_order_then_entity_id() {
    let mut world = World::new();

    // Spawned first, but the highest `order` — must sort last.
    let last = world.spawn();
    world
        .insert(
            last,
            UiRect {
                order: 5,
                ..UiRect::default()
            },
        )
        .unwrap();

    // Spawned second, `order: 1`, ties with the next entity — lower EntityId (spawned earlier)
    // wins the tie.
    let tie_first = world.spawn();
    world
        .insert(
            tie_first,
            UiText {
                order: 1,
                text: "HI".to_string(),
                ..UiText::default()
            },
        )
        .unwrap();

    // Spawned third, `order: 1` too — ties with `tie_first`, sorts after it (higher EntityId).
    let tie_second = world.spawn();
    world
        .insert(
            tie_second,
            UiRect {
                order: 1,
                ..UiRect::default()
            },
        )
        .unwrap();

    let scene = extract(&world, (1920, 1080), Path::new(".")).unwrap();
    assert_eq!(scene.ui.len(), 3, "one UiRect/UiText each ⇒ 3 instances");

    match &scene.ui[0] {
        UiInstance::Text { text, .. } => assert_eq!(text, "HI"),
        other => panic!("expected the order-1, lower-EntityId Text first, got {other:?}"),
    }
    assert!(
        matches!(scene.ui[1], UiInstance::Rect { .. }),
        "expected the order-1, higher-EntityId Rect second"
    );
    assert!(
        matches!(scene.ui[2], UiInstance::Rect { .. }),
        "expected the order-5 Rect last"
    );

    // Two extractions of a UI-carrying world are byte-equal (the extract-determinism contract
    // extended to UI).
    let again = extract(&world, (1920, 1080), Path::new(".")).unwrap();
    assert_eq!(
        scene, again,
        "extract of UI components must be deterministic"
    );
}

/// A `UiRect`'s resolved `rect_px` matches [`ludion_engine::ui::resolve_ui_rect`] exactly, and its
/// `color`/absent `texture` carry through untouched — the extract seam does no extra transformation.
#[test]
fn ui_rect_resolves_through_extract_exactly_like_the_pure_function() {
    let mut world = World::new();
    let e = world.spawn();
    let rect = UiRect {
        anchor: Anchor::BottomRight,
        offset: [-8.0, -8.0],
        size: [64.0, 32.0],
        texture: None,
        atlas: None,
        frame: None,
        color: [1.0, 0.2, 0.2, 1.0],
        order: 3,
        visible: true,
        fill: false,
    };
    world.insert(e, rect.clone()).unwrap();

    let viewport = (1920, 1080);
    let scene = extract(&world, viewport, Path::new(".")).unwrap();
    assert_eq!(scene.ui.len(), 1);
    let expected_px =
        ludion_engine::ui::resolve_ui_rect(rect.anchor, rect.offset, rect.size, viewport);
    match &scene.ui[0] {
        UiInstance::Rect {
            rect_px,
            texture,
            color,
            uv,
        } => {
            assert_eq!(*rect_px, expected_px);
            assert_eq!(*texture, None);
            assert_eq!(*color, rect.color);
            assert_eq!(
                *uv,
                [0.0, 0.0, 1.0, 1.0],
                "an untextured rect samples the whole texture"
            );
        }
        other => panic!("expected Rect, got {other:?}"),
    }
}

/// A `fill: true` rect extracts as the whole render target at every extent, so it follows a resized
/// window; its anchor, offset and size play no part, and a texture-less fill keeps the full UV.
#[test]
fn a_filled_ui_rect_extracts_as_the_whole_viewport() {
    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            UiRect {
                anchor: Anchor::Center,
                offset: [40.0, 40.0],
                size: [10.0, 10.0],
                color: [0.0, 0.0, 0.0, 0.5],
                fill: true,
                ..UiRect::default()
            },
        )
        .unwrap();
    for viewport in [(1440, 1080), (1920, 1080), (2520, 1080), (2560, 1080)] {
        let scene = extract(&world, viewport, Path::new(".")).unwrap();
        match &scene.ui[..] {
            [UiInstance::Rect { rect_px, uv, .. }] => {
                assert_eq!(*rect_px, [0.0, 0.0, viewport.0 as f32, viewport.1 as f32]);
                assert_eq!(*uv, [0.0, 0.0, 1.0, 1.0]);
            }
            other => panic!("expected one Rect, got {other:?}"),
        }
    }
}

/// A `Transform` on the same entity as a `UiRect`/`UiText` changes nothing — UI is screen-space and
/// the transform is simply ignored.
#[test]
fn transform_on_a_ui_entity_is_ignored() {
    let mut world_plain = World::new();
    let e1 = world_plain.spawn();
    world_plain
        .insert(
            e1,
            UiRect {
                anchor: Anchor::Center,
                ..UiRect::default()
            },
        )
        .unwrap();

    let mut world_with_transform = World::new();
    let e2 = world_with_transform.spawn();
    world_with_transform
        .insert(
            e2,
            Transform {
                translation: [100.0, -50.0, 25.0],
                rotation: [10.0, 20.0, 30.0],
                scale: [2.0, 3.0, 4.0],
            },
        )
        .unwrap();
    world_with_transform
        .insert(
            e2,
            UiRect {
                anchor: Anchor::Center,
                ..UiRect::default()
            },
        )
        .unwrap();

    let viewport = (1920, 1080);
    let plain = extract(&world_plain, viewport, Path::new(".")).unwrap();
    let with_transform = extract(&world_with_transform, viewport, Path::new(".")).unwrap();
    assert_eq!(
        plain.ui, with_transform.ui,
        "a Transform on a UI entity must not change its resolved geometry"
    );
}

/// A `UiRect`'s `texture` resolves through the same dedup as a `Sprite`'s: both referencing the same
/// path land on the same `RenderScene.textures` index — proving the UI texture path reuses the
/// existing texture cache/dedup rather than a parallel one.
#[test]
fn ui_rect_texture_dedups_against_a_sprite_using_the_same_path() {
    let dir = std::env::temp_dir().join(format!("ludion-extract-ui-tex-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("textures")).unwrap();
    let png = ludion_assets::encode_png(1, 1, &[255, 0, 0, 255]).unwrap();
    std::fs::write(dir.join("textures/icon.png"), png).unwrap();

    let mut world = World::new();
    let sprite = world.spawn();
    world
        .insert(
            sprite,
            Sprite {
                texture: Some("textures/icon.png".to_string()),
                ..Sprite::default()
            },
        )
        .unwrap();
    let rect = world.spawn();
    world
        .insert(
            rect,
            UiRect {
                texture: Some("textures/icon.png".to_string()),
                ..UiRect::default()
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(
        scene.textures.len(),
        1,
        "one shared path ⇒ one textures entry"
    );
    assert_eq!(scene.sprites[0].texture, Some(0));
    match &scene.ui[0] {
        UiInstance::Rect { texture, .. } => assert_eq!(*texture, Some(0)),
        other => panic!("expected Rect, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `UiRect` pointing at a missing texture is an `L1101`, re-pathed at the referencing component
/// (the sprite/mesh texture-error style, for UI).
#[test]
fn missing_ui_rect_texture_is_l1101_at_the_ui_rect_component() {
    let dir =
        std::env::temp_dir().join(format!("ludion-extract-ui-misstex-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            UiRect {
                texture: Some("textures/nope.png".to_string()),
                ..UiRect::default()
            },
        )
        .unwrap();

    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1101");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/UiRect/texture")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `UiText`'s resolved `origin_px`/`cell_px` match the measured, anchored block exactly.
#[test]
fn ui_text_resolves_through_extract_exactly_like_the_measured_block() {
    let mut world = World::new();
    let e = world.spawn();
    let text = UiText {
        anchor: Anchor::Center,
        offset: [0.0, 0.0],
        text: "GAME OVER".to_string(),
        size: 32.0,
        color: [1.0, 1.0, 0.0, 1.0],
        order: 0,
        wrap_width: 0.0,
        visible: true,
        font: None,
    };
    world.insert(e, text.clone()).unwrap();

    let viewport = (1920, 1080);
    let scene = extract(&world, viewport, Path::new(".")).unwrap();
    assert_eq!(scene.ui.len(), 1);
    let block = ludion_engine::ui::measure_ui_text(
        ludion_font::FontFace::embedded(),
        &text.text,
        text.size,
    );
    let rect = ludion_engine::ui::resolve_ui_rect(text.anchor, text.offset, block, viewport);
    let (expected_origin, expected_cell) = ([rect[0], rect[1]], text.size);
    match &scene.ui[0] {
        UiInstance::Text {
            color,
            text: drawn_text,
            quads,
            ..
        } => {
            assert_eq!(first_quad(quads), (expected_origin, expected_cell));
            assert_eq!(*color, text.color);
            assert_eq!(drawn_text, &text.text);
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

/// An empty `UiText` (default `text: ""`) still emits its instance (a zero-sized block, harmless —
/// the renderer draws nothing for it) rather than being filtered out.
#[test]
fn empty_ui_text_still_emits_an_instance() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, UiText::default()).unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(scene.ui.len(), 1, "an empty UiText still emits an instance");
}

/// `visible: false` on a `UiRect` skips it entirely — no instance, and (proven by the otherwise
/// missing texture never raising `L1101`) no atlas/texture resolution either. The default
/// (`visible` absent) still emits, as every other `UiRect` test here relies on.
#[test]
fn hidden_ui_rect_emits_no_instance_and_skips_texture_resolution() {
    let mut world = World::new();
    let shown = world.spawn();
    world.insert(shown, UiRect::default()).unwrap();
    let hidden = world.spawn();
    world
        .insert(
            hidden,
            UiRect {
                visible: false,
                texture: Some("textures/does_not_exist.png".to_string()),
                ..UiRect::default()
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(
        scene.ui.len(),
        1,
        "the hidden rect emits nothing; the default-visible one still does"
    );
}

/// `visible: false` on a `UiText` skips it entirely — no instance. The default (`visible` absent)
/// still emits.
#[test]
fn hidden_ui_text_emits_no_instance() {
    let mut world = World::new();
    let shown = world.spawn();
    world.insert(shown, UiText::default()).unwrap();
    let hidden = world.spawn();
    world
        .insert(
            hidden,
            UiText {
                visible: false,
                text: "BOSS HEALTH".to_string(),
                ..UiText::default()
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(
        scene.ui.len(),
        1,
        "the hidden text emits nothing; the default-visible one still does"
    );
}

// --- Environment/IBL Wave 1: Environment + Fade through extract ---

/// An `Environment` resolves onto `RenderScene.environment` and a `PostProcess` onto
/// `RenderScene.post` (authored fields carried verbatim), a per-entity `Fade` rides onto its mesh
/// instance, and two extractions of the world are byte-equal — the extract-determinism contract
/// extended to the render-inert data.
#[test]
fn environment_and_fade_extract_deterministically() {
    let mut world = World::new();

    let env_entity = world.spawn();
    world
        .insert(
            env_entity,
            Environment {
                sky_zenith: [0.1, 0.2, 0.9],
                ibl_intensity: 1.5,
                ..Environment::default()
            },
        )
        .unwrap();
    world
        .insert(
            env_entity,
            PostProcess {
                bloom_strength: 0.4,
                ..PostProcess::default()
            },
        )
        .unwrap();

    let mesh = world.spawn();
    world.insert(mesh, Transform::default()).unwrap();
    world.insert(mesh, Mesh::default()).unwrap();
    world
        .insert(
            mesh,
            Fade {
                fade: 0.25,
                height_min: 3.5,
            },
        )
        .unwrap();

    let a = extract(&world, (320, 180), Path::new(".")).unwrap();
    let b = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(a, b, "extract of Environment/Fade must be deterministic");

    let env = a.environment.expect("an environment is present");
    assert_eq!(env.sky_zenith, [0.1, 0.2, 0.9]);
    assert_eq!(env.ibl_intensity, 1.5);

    let post = a.post.expect("a post-process is present");
    assert_eq!(post.bloom_strength, 0.4);
    assert_eq!(post.bloom_threshold, 1.0);

    assert_eq!(a.meshes.len(), 1);
    assert_eq!(a.meshes[0].fade, 0.25);
    assert_eq!(a.meshes[0].fade_height_min, 3.5);
}

/// A mesh with no `Fade` of its own takes its nearest ancestor's through `Parent`; its own wins; an
/// unparented mesh keeps the default. Instances come out in spawn order.
#[test]
fn a_child_mesh_inherits_its_nearest_ancestors_fade() {
    let mut world = World::new();
    let mut mesh = |parent: Option<ludion_core::ids::EntityId>, fade: Option<Fade>| {
        let e = world.spawn();
        world.insert(e, Transform::default()).unwrap();
        world.insert(e, Mesh::default()).unwrap();
        if let Some(parent) = parent {
            world.insert(e, ludion_ecs::Parent(parent)).unwrap();
        }
        if let Some(fade) = fade {
            world.insert(e, fade).unwrap();
        }
        e
    };
    let root = mesh(
        None,
        Some(Fade {
            fade: 0.25,
            height_min: 1.0,
        }),
    );
    mesh(Some(root), None);
    let own = mesh(
        Some(root),
        Some(Fade {
            fade: 0.5,
            height_min: 2.0,
        }),
    );
    mesh(Some(own), None);
    mesh(None, None);

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    let fades: Vec<(f32, f32)> = scene
        .meshes
        .iter()
        .map(|m| (m.fade, m.fade_height_min))
        .collect();
    assert_eq!(
        fades,
        [
            (0.25, 1.0),
            (0.25, 1.0),
            (0.5, 2.0),
            (0.5, 2.0),
            (1.0, -f32::MAX)
        ]
    );
}

/// A camera, a hidden-able root, and one child of every kind `Visibility` removes. `root` is the
/// `Visibility` the root carries (`None`: the component is absent).
fn visibility_world(root: Option<Visibility>) -> World {
    let mut world = World::new();
    let camera = world.spawn();
    world.insert(camera, Transform::default()).unwrap();
    world.insert(camera, Camera::default()).unwrap();
    let root_id = world.spawn();
    world.insert(root_id, Transform::default()).unwrap();
    if let Some(visibility) = root {
        world.insert(root_id, visibility).unwrap();
    }
    let child = |world: &mut World| {
        let id = world.spawn();
        world.insert(id, Transform::default()).unwrap();
        world.insert(id, ludion_ecs::Parent(root_id)).unwrap();
        id
    };
    let id = child(&mut world);
    world.insert(id, Mesh::default()).unwrap();
    let id = child(&mut world);
    world.insert(id, PointLight::default()).unwrap();
    let id = child(&mut world);
    world.insert(id, SpotLight::default()).unwrap();
    let id = child(&mut world);
    world.insert(id, Sprite::default()).unwrap();
    let id = child(&mut world);
    world.insert(id, Text::default()).unwrap();
    let id = child(&mut world);
    world.insert(id, GroundMarker::default()).unwrap();
    let id = child(&mut world);
    world.insert(id, DropShadow::default()).unwrap();
    let id = child(&mut world);
    world.insert(id, Decal::default()).unwrap();
    let id = child(&mut world);
    world.insert(id, ParticleEmitter::default()).unwrap();
    world
}

fn hidden() -> Visibility {
    Visibility {
        visible: false,
        ..Visibility::default()
    }
}

/// A hidden root removes every kind under it, a part's own `visible: true` notwithstanding, and
/// showing the root again extracts exactly what a world with no `Visibility` extracts.
#[test]
fn a_hidden_root_extracts_none_of_its_tree() {
    let mut world = visibility_world(Some(hidden()));
    let part = world.entities().last().unwrap();
    world.insert(part, Visibility::default()).unwrap();
    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert!(scene.meshes.is_empty());
    assert!(scene.lights.is_empty());
    assert!(scene.spot_lights.is_empty());
    assert!(scene.sprites.is_empty(), "sprite and text quads");
    assert!(scene.ground_markers.is_empty());
    assert!(scene.drop_shadows.is_empty());
    assert!(scene.decals.is_empty());
    assert!(scene.particles.is_empty());

    let root = world.entities().nth(1).unwrap();
    world.insert(root, Visibility::default()).unwrap();
    let shown = extract(&world, (320, 180), Path::new(".")).unwrap();
    let plain = extract(&visibility_world(None), (320, 180), Path::new(".")).unwrap();
    assert_eq!(shown, plain, "an all-default Visibility changes nothing");
    assert!(!shown.meshes.is_empty() && !shown.lights.is_empty());
}

fn shadowless() -> Visibility {
    Visibility {
        cast_shadows: false,
        ..Visibility::default()
    }
}

/// A world with no `Visibility` extracts identically twice (the gate path) with every mesh casting,
/// and a shadowless root alone removes nothing from the scene: it only clears `cast_shadows` on the
/// meshes under it.
#[test]
fn a_visibility_free_world_extracts_unchanged() {
    let world = visibility_world(None);
    let a = extract(&world, (320, 180), Path::new(".")).unwrap();
    let b = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(a, b);
    assert!(a.meshes.iter().all(|m| m.cast_shadows));
    let c = extract(
        &visibility_world(Some(shadowless())),
        (320, 180),
        Path::new("."),
    )
    .unwrap();
    assert_eq!(a.meshes.len(), c.meshes.len());
    assert!(c.meshes.iter().all(|m| !m.cast_shadows));
    let mut recast = c.clone();
    for mesh in &mut recast.meshes {
        mesh.cast_shadows = true;
    }
    assert_eq!(a, recast, "cast_shadows alone does not hide anything");
}

/// `cast_shadows: false` on a root reaches its child's instance and every part of an assembly under
/// it; a hidden ancestor still removes the mesh entirely.
#[test]
fn cast_shadows_inherits_to_children_and_assembly_parts() {
    let dir = assembly_dir("cast-shadows");
    let mut world = World::new();
    let root = world.spawn();
    world.insert(root, Transform::default()).unwrap();
    world.insert(root, shadowless()).unwrap();
    let child = world.spawn();
    world.insert(child, Transform::default()).unwrap();
    world.insert(child, ludion_ecs::Parent(root)).unwrap();
    world
        .insert(
            child,
            Mesh {
                shape: MeshSource::Assembly("models/fixture.assembly.yaml".to_string()),
                ..Mesh::default()
            },
        )
        .unwrap();
    let plain = world.spawn();
    world.insert(plain, Transform::default()).unwrap();
    world.insert(plain, Mesh::default()).unwrap();
    let scene = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(
        scene.meshes.len(),
        3,
        "two assembly parts and the plain cube"
    );
    assert!(!scene.meshes[0].cast_shadows && !scene.meshes[1].cast_shadows);
    assert!(scene.meshes[2].cast_shadows);

    world
        .insert(
            root,
            Visibility {
                visible: false,
                cast_shadows: false,
            },
        )
        .unwrap();
    let scene = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(scene.meshes.len(), 1, "hidden wins over the shadow flag");
    assert!(scene.meshes[0].cast_shadows);
}

/// A hidden `GiDynamic` light is absent from the scene's lights.
#[test]
fn a_hidden_gi_dynamic_light_is_not_extracted() {
    let mut world = World::new();
    let light = world.spawn();
    world.insert(light, Transform::default()).unwrap();
    world.insert(light, PointLight::default()).unwrap();
    world.insert(light, GiDynamic::default()).unwrap();
    let shown = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(shown.lights.len(), 1);
    world.insert(light, hidden()).unwrap();
    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert!(scene.lights.is_empty());
}

fn sim_hash(world: &World) -> ludion_sim::WorldHash {
    ludion_sim::WorldHash::compute(world, &ludion_engine::builtin_registry()).unwrap()
}

/// Presentation entities are never hashed, so toggling `Visibility` on one moves nothing; on a sim
/// entity it does, and an all-default `Visibility` hashes the same as `Visibility: {}`.
#[test]
fn visibility_hashes_on_sim_entities_only() {
    let mut world = World::new();
    let sim = world.spawn();
    world.insert(sim, Transform::default()).unwrap();
    let fx = world.spawn_presentation();
    world.insert(fx, Transform::default()).unwrap();
    let before = sim_hash(&world);

    world.insert(fx, hidden()).unwrap();
    assert_eq!(sim_hash(&world), before, "a presentation toggle is free");

    world.insert(sim, Visibility::default()).unwrap();
    let defaulted = sim_hash(&world);
    let empty: Visibility = serde_json::from_value(serde_json::json!({})).unwrap();
    world.insert(sim, empty).unwrap();
    assert_eq!(sim_hash(&world), defaulted, "`{{}}` is the default");

    world.insert(sim, hidden()).unwrap();
    assert_ne!(sim_hash(&world), defaulted, "a sim toggle moves the hash");
}

/// `{}` deserializes to the default and a default serializes to `{}`.
#[test]
fn visibility_round_trips_through_empty_json() {
    let empty: Visibility = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(empty, Visibility::default());
    assert_eq!(
        serde_json::to_value(Visibility::default()).unwrap(),
        serde_json::json!({})
    );
    assert_eq!(
        serde_json::to_value(hidden()).unwrap(),
        serde_json::json!({ "visible": false })
    );
}

/// A world with no `Environment` extracts `environment: None`, one with no `PostProcess` extracts
/// `post: None`, and a mesh with no `Fade` carries the render-identical default `(1.0, -f32::MAX)` —
/// the "absent ⇒ historical pipeline" contract.
#[test]
fn absent_environment_and_fade_are_the_defaults() {
    let mut world = World::new();
    let mesh = world.spawn();
    world.insert(mesh, Mesh::default()).unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert!(scene.environment.is_none(), "no Environment ⇒ None");
    assert!(scene.post.is_none(), "no PostProcess ⇒ None");
    assert_eq!(scene.meshes[0].fade, 1.0);
    assert_eq!(scene.meshes[0].fade_height_min, -f32::MAX);
}

/// A tilemap cell whose legend texture is missing is `L1101` pointed at that legend entry, with the
/// glyph escaped as one JSON Pointer reference token (`/` → `~1`). The pointer is built lazily on
/// the error path — this pins that it still reads exactly as the eager form did.
#[test]
fn missing_tilemap_cell_texture_is_l1101_at_the_escaped_legend_entry() {
    let dir = std::env::temp_dir().join(format!("ludion-extract-tilecell-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut legend = BTreeMap::new();
    legend.insert(
        "/".to_string(),
        TileAppearance {
            texture: Some("tiles/nope.png".to_string()),
            ..TileAppearance::default()
        },
    );

    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            Tilemap {
                legend,
                rows: vec!["/".to_string()],
                ..Tilemap::default()
            },
        )
        .unwrap();

    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1101");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Tilemap/legend/~1/texture")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The missing-texture suggestion goes through `ludion_core::suggest`, which charges an adjacent
/// transposition 1 rather than 2. `rgass.pgn` is two transpositions from `grass.png` — inside the
/// shared budget under that rule, outside it under plain Levenshtein.
#[test]
fn missing_texture_suggests_a_transposed_filename() {
    let dir = std::env::temp_dir().join(format!("ludion-extract-suggest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tiles")).unwrap();
    std::fs::write(dir.join("tiles/grass.png"), b"not really a png").unwrap();

    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            Sprite {
                texture: Some("tiles/rgass.pgn".to_string()),
                ..Sprite::default()
            },
        )
        .unwrap();

    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Sprite/texture")
    );
    assert_eq!(
        errors[0].help.as_deref(),
        Some("nearest match is `grass.png`")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The `GiDynamic` marker's light half (`Plan/gi-wave3-dynamic-injection.md` §3.2, §11): an
/// extracted marked light carries `dynamic: true`, an unmarked one `dynamic: false`, and marking
/// perturbs neither the content nor the ascending-`EntityId` extract order of `RenderScene.lights`
/// — every light, marked or not, still reaches clustered forward exactly as it did before the
/// marker existed. The split governs GI only.
#[test]
fn a_marked_light_carries_the_dynamic_flag_and_the_light_list_is_unperturbed() {
    let mut world = World::new();

    let unmarked = world.spawn();
    world
        .insert(
            unmarked,
            Transform {
                translation: [-2.0, 0.0, 0.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world.insert(unmarked, PointLight::default()).unwrap();

    let marked = world.spawn();
    world
        .insert(
            marked,
            Transform {
                translation: [2.0, 0.0, 0.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world.insert(marked, PointLight::default()).unwrap();
    world.insert(marked, GiDynamic::default()).unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(scene.lights.len(), 2, "both lights reach the render scene");
    assert_eq!(
        scene.lights[0].position.x, -2.0,
        "extract order is ascending-EntityId, unperturbed by marking"
    );
    assert_eq!(scene.lights[1].position.x, 2.0);
    assert!(
        !scene.lights[0].dynamic,
        "the unmarked light carries dynamic: false"
    );
    assert!(
        scene.lights[1].dynamic,
        "the marked light carries dynamic: true"
    );
}

// --- Telegraphs Wave 1: GroundMarker through extract ---

/// A marker's authored dimensions are its **maximum** extent: the entity's uniform
/// `Transform.scale.x` multiplies them into the final world-space size the renderer draws, for both
/// shapes. The model matrix stays rigid, so the size travels as a length and never as a matrix the
/// signed-distance evaluation would have to undo.
#[test]
fn ground_marker_dimensions_are_scaled_into_world_units() {
    let mut world = World::new();

    let disc = world.spawn();
    world
        .insert(
            disc,
            Transform {
                scale: [0.25, 0.25, 0.25],
                ..Transform::default()
            },
        )
        .unwrap();
    world
        .insert(
            disc,
            GroundMarker {
                shape: GroundMarkerShape::Disc { radius: 4.0 },
                color: [1.0; 4],
            },
        )
        .unwrap();

    let segment = world.spawn();
    world
        .insert(
            segment,
            Transform {
                scale: [2.0, 2.0, 2.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world
        .insert(
            segment,
            GroundMarker {
                shape: GroundMarkerShape::Segment {
                    length: 8.0,
                    width: 1.5,
                },
                color: [1.0; 4],
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(
        scene.ground_markers[0].shape,
        RendererMarkerShape::Disc { radius: 1.0 },
    );
    assert_eq!(
        scene.ground_markers[1].shape,
        RendererMarkerShape::Segment {
            length: 16.0,
            width: 3.0,
        },
    );
}

/// The instance's model matrix is **rigid** — rotation and translation only, with
/// `GROUND_MARKER_LIFT` added to Y — so a scaled marker's matrix is a pure rotation about the lifted
/// position. The lift is the engine's one answer to a coplanar quad; no scene authors it.
#[test]
fn ground_marker_model_is_rigid_and_lifted() {
    let mut world = World::new();
    let id = world.spawn();
    world
        .insert(
            id,
            Transform {
                translation: [4.0, 0.0, -2.0],
                rotation: [0.0, 90.0, 0.0],
                scale: [3.0, 3.0, 3.0],
            },
        )
        .unwrap();
    world.insert(id, GroundMarker::default()).unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    let model = scene.ground_markers[0].model;

    let (scale, _, translation) = model.to_scale_rotation_translation();
    assert!(
        (scale - Vec3::ONE).length() < 1e-6,
        "the model matrix carries no scale: {scale:?}"
    );
    assert_eq!(translation.x, 4.0);
    assert_eq!(translation.z, -2.0);
    assert_eq!(translation.y, GROUND_MARKER_LIFT);

    // Local +X is the segment's length axis; a 90 deg yaw carries it onto world -Z.
    let length_axis = model.transform_vector3(Vec3::X);
    assert!(
        (length_axis - Vec3::NEG_Z).length() < 1e-6,
        "local +X is the length axis: {length_axis:?}"
    );
}

/// Markers reach `RenderScene.ground_markers` in ascending-`EntityId` order — the blend order —
/// with the authored (HDR-capable) color passed through verbatim, and an entity carrying no
/// `GroundMarker` contributes nothing.
#[test]
fn ground_markers_extract_in_ascending_entity_order_with_their_color() {
    let mut world = World::new();

    let first = world.spawn();
    world.insert(first, Transform::default()).unwrap();
    world
        .insert(
            first,
            GroundMarker {
                shape: GroundMarkerShape::Disc { radius: 1.0 },
                color: [2.5, 0.2, 0.1, 0.55],
            },
        )
        .unwrap();

    let plain = world.spawn();
    world.insert(plain, Mesh::default()).unwrap();

    let second = world.spawn();
    world.insert(second, Transform::default()).unwrap();
    world
        .insert(
            second,
            GroundMarker {
                shape: GroundMarkerShape::Disc { radius: 1.0 },
                color: [0.1, 0.9, 0.3, 1.0],
            },
        )
        .unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(scene.ground_markers.len(), 2);
    assert_eq!(scene.ground_markers[0].color, [2.5, 0.2, 0.1, 0.55]);
    assert_eq!(scene.ground_markers[1].color, [0.1, 0.9, 0.3, 1.0]);
}

/// A world that authors no marker extracts an empty list — the conditional pass's precondition, and
/// what keeps a marker-free scene's recorded command stream the pre-marker one.
#[test]
fn a_marker_free_world_extracts_no_ground_markers() {
    let mut world = World::new();
    let mesh = world.spawn();
    world.insert(mesh, Mesh::default()).unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert!(scene.ground_markers.is_empty());
}

// --- UI widgets Wave 1: UiList lowering ---

fn list_item(text: &str, enabled: bool) -> UiListItem {
    UiListItem {
        text: text.to_string(),
        enabled,
    }
}

/// A three-item list with a title, a panel, and one disabled item — the shape every colour and
/// ordering assertion below reads.
fn shop_list() -> UiList {
    UiList {
        title: "WARES".to_string(),
        items: vec![
            list_item("SALVE", true),
            list_item("RELIC", false),
            list_item("LEAVE", true),
        ],
        selected: 2,
        size: 10.0,
        line_spacing: 2.0,
        padding: 4.0,
        color: [1.0, 1.0, 1.0, 1.0],
        selected_color: [1.0, 0.5, 0.0, 1.0],
        disabled_color: [0.3, 0.3, 0.3, 1.0],
        background: [0.0, 0.0, 0.0, 0.75],
        order: 2,
        ..UiList::default()
    }
}

fn world_with(list: UiList) -> World {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, list).unwrap();
    world
}

/// One `UiList` lowers to its background first and then one `Text` per rendered line — title, then
/// items, top to bottom — all at one `(order, EntityId)` key, which the stable sort preserves. The
/// geometry matches `ui::resolve_ui_list` exactly; extract adds no transformation of its own.
#[test]
fn ui_list_emits_its_background_then_its_lines_in_order() {
    let list = shop_list();
    let viewport = (1920, 1080); // scale 1.0
    let world = world_with(list.clone());

    let scene = extract(&world, viewport, Path::new(".")).unwrap();
    assert_eq!(scene.ui.len(), 5, "1 panel + title + 3 items");

    let layout =
        ludion_engine::ui::resolve_ui_list(ludion_font::FontFace::embedded(), &list, viewport);
    match &scene.ui[0] {
        UiInstance::Rect {
            rect_px,
            texture,
            color,
            uv,
        } => {
            assert_eq!(*rect_px, layout.background_px);
            assert_eq!(*texture, None);
            assert_eq!(*color, list.background);
            assert_eq!(*uv, [0.0, 0.0, 1.0, 1.0]);
        }
        other => panic!("expected the background Rect first, got {other:?}"),
    }

    let expected_lines = ["WARES", "SALVE", "RELIC", "LEAVE"];
    for (i, expected) in expected_lines.iter().enumerate() {
        match &scene.ui[i + 1] {
            UiInstance::Text { text, quads, .. } => {
                assert_eq!(text, expected, "line {i}");
                assert_eq!(
                    first_quad(quads),
                    (
                        [
                            layout.origin_px[0],
                            layout.origin_px[1] + i as f32 * layout.line_advance_px
                        ],
                        layout.cell_px
                    ),
                    "line {i} advances by (size + line_spacing)"
                );
            }
            other => panic!("expected Text for line {i}, got {other:?}"),
        }
    }

    let again = extract(&world, viewport, Path::new(".")).unwrap();
    assert_eq!(scene, again, "extract of a UiList must be deterministic");
}

/// The three colours land on the three kinds of line: the cursor's item in `selected_color`, a
/// disabled item in `disabled_color`, everything else — the title included — in `color`.
#[test]
fn ui_list_applies_the_normal_selected_and_disabled_colors() {
    let list = shop_list();
    let scene = extract(&world_with(list.clone()), (1920, 1080), Path::new(".")).unwrap();

    let color_of = |index: usize| match &scene.ui[index] {
        UiInstance::Text { color, .. } => *color,
        other => panic!("expected Text at {index}, got {other:?}"),
    };
    assert_eq!(color_of(1), list.color, "the title is never selectable");
    assert_eq!(color_of(2), list.color, "an ordinary item");
    assert_eq!(color_of(3), list.disabled_color, "the disabled item");
    assert_eq!(color_of(4), list.selected_color, "the cursor's item");
}

/// A disabled item under an authored cursor reads as disabled, not as selected — navigation never
/// parks there, but a scene can.
#[test]
fn a_disabled_item_under_the_cursor_stays_disabled() {
    let list = UiList {
        selected: 1, // the RELIC line, `enabled: false`
        ..shop_list()
    };
    let scene = extract(&world_with(list.clone()), (1920, 1080), Path::new(".")).unwrap();
    match &scene.ui[3] {
        UiInstance::Text { color, .. } => assert_eq!(*color, list.disabled_color),
        other => panic!("expected the RELIC Text, got {other:?}"),
    }
}

/// A `selected` past the end is clamped when drawn — the last item takes the cursor colour, and no
/// diagnostic is raised.
#[test]
fn a_selected_past_the_end_clamps_to_the_last_item() {
    let list = UiList {
        selected: 999,
        ..shop_list()
    };
    let scene = extract(&world_with(list.clone()), (1920, 1080), Path::new(".")).unwrap();
    match &scene.ui[4] {
        UiInstance::Text { color, text, .. } => {
            assert_eq!(text, "LEAVE");
            assert_eq!(*color, list.selected_color);
        }
        other => panic!("expected the last Text, got {other:?}"),
    }
}

/// An empty `items` vec draws **nothing at all** — not even the background, and not the title
/// either. That is the visibility mechanism, so it must hold with a panel authored.
#[test]
fn an_empty_ui_list_emits_nothing() {
    let empty = UiList {
        title: "WARES".to_string(),
        background: [0.0, 0.0, 0.0, 1.0],
        ..UiList::default()
    };
    let scene = extract(&world_with(empty), (320, 180), Path::new(".")).unwrap();
    assert!(
        scene.ui.is_empty(),
        "an empty list draws nothing at all: {:?}",
        scene.ui
    );
}

/// `visible: false` on a non-empty `UiList` — background and items included — skips it entirely,
/// orthogonal to the `items`-emptiness check above. The default (`visible` absent) still emits, as
/// `shop_list()`-based tests elsewhere rely on.
#[test]
fn hidden_ui_list_emits_nothing() {
    let hidden = UiList {
        visible: false,
        ..shop_list()
    };
    let scene = extract(&world_with(hidden), (1920, 1080), Path::new(".")).unwrap();
    assert!(
        scene.ui.is_empty(),
        "a hidden list draws nothing at all: {:?}",
        scene.ui
    );
}

/// A zero-alpha `background` draws no panel; the lines are unaffected.
#[test]
fn a_transparent_background_draws_no_panel() {
    let list = UiList {
        background: [0.2, 0.2, 0.2, 0.0],
        ..shop_list()
    };
    let scene = extract(&world_with(list), (1920, 1080), Path::new(".")).unwrap();
    assert_eq!(scene.ui.len(), 4, "title + 3 items, no panel");
    assert!(
        matches!(scene.ui[0], UiInstance::Text { .. }),
        "the first instance is the title line, not a panel"
    );
}

/// A list with no title starts at its first item — the title contributes a line only when it is
/// non-empty, in draw exactly as in measurement.
#[test]
fn an_untitled_list_starts_at_its_first_item() {
    let list = UiList {
        title: String::new(),
        ..shop_list()
    };
    let scene = extract(&world_with(list), (1920, 1080), Path::new(".")).unwrap();
    assert_eq!(scene.ui.len(), 4, "1 panel + 3 items");
    match &scene.ui[1] {
        UiInstance::Text { text, .. } => assert_eq!(text, "SALVE"),
        other => panic!("expected the first item, got {other:?}"),
    }
}

/// A `UiList` sorts against `UiRect`/`UiText` by the same `(order, EntityId)` key, and its own
/// instances stay contiguous and in emission order inside that key.
#[test]
fn ui_list_instances_sort_with_the_other_ui_components() {
    let mut world = World::new();

    let list_entity = world.spawn();
    world
        .insert(
            list_entity,
            UiList {
                order: 1,
                ..shop_list()
            },
        )
        .unwrap();

    let over = world.spawn();
    world
        .insert(
            over,
            UiText {
                order: 5,
                text: "OVER".to_string(),
                ..UiText::default()
            },
        )
        .unwrap();

    let scene = extract(&world, (1920, 1080), Path::new(".")).unwrap();
    assert_eq!(scene.ui.len(), 6, "5 list instances + 1 UiText");
    assert!(
        matches!(scene.ui[0], UiInstance::Rect { .. }),
        "the list's panel leads its own run"
    );
    match &scene.ui[5] {
        UiInstance::Text { text, .. } => assert_eq!(text, "OVER", "the order-5 text sorts last"),
        other => panic!("expected the order-5 UiText last, got {other:?}"),
    }
}

// --- UI widgets Wave 2: wrapped UiText, windowed UiList ---

/// A wrapped `UiText` still lowers to exactly one `Text` instance, drawn at the origin of the block
/// measured over the wrapped string (rebuilt here from `wrap_ui_text`, `measure_ui_text` and
/// `resolve_ui_rect`, not from `resolve_ui_text`); the instance carries the authored text.
#[test]
fn a_wrapped_ui_text_emits_one_instance_laid_out_at_the_wrapped_blocks_origin() {
    let text = UiText {
        text: "THE OLD KEEPER WAITS".to_string(),
        size: 10.0,
        wrap_width: 100.0,
        ..UiText::default()
    };
    let viewport = (1920, 1080); // scale 1.0
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, text.clone()).unwrap();

    let scene = extract(&world, viewport, Path::new(".")).unwrap();
    assert_eq!(
        scene.ui.len(),
        1,
        "wrapping never splits into more instances"
    );

    let wrapped = ludion_engine::ui::wrap_ui_text(
        ludion_font::FontFace::embedded(),
        &text.text,
        text.size,
        text.wrap_width,
    );
    assert_eq!(wrapped, "THE OLD\nKEEPER\nWAITS");
    let block =
        ludion_engine::ui::measure_ui_text(ludion_font::FontFace::embedded(), &wrapped, text.size);
    assert_eq!(
        block,
        [70.0, 30.0],
        "THE OLD is 7 glyphs wide over three lines"
    );
    let rect = ludion_engine::ui::resolve_ui_rect(text.anchor, text.offset, block, viewport);
    let (expected_origin, expected_cell) = ([rect[0], rect[1]], text.size);
    match &scene.ui[0] {
        UiInstance::Text {
            text: emitted,
            quads,
            ..
        } => {
            assert_eq!(emitted, &text.text);
            assert_eq!(first_quad(quads), (expected_origin, expected_cell));
        }
        other => panic!("expected one Text, got {other:?}"),
    }
}

/// The first glyph quad's top-left corner and height — the origin and cell a text instance was
/// laid out at. Only meaningful for text whose first glyph is mapped.
fn first_quad(quads: &[ludion_renderer::GlyphQuad]) -> ([f32; 2], f32) {
    let pos = quads[0].pos;
    ([pos[0], pos[1]], pos[3] - pos[1])
}

/// The resolved block is the drawn block: the union of a wrapped, multi-line `UiText`'s quads is the
/// rect `resolve_ui_text` returns, in pixels, and that rect is the wrapped string measured and
/// anchored independently.
#[test]
fn ui_text_quads_fill_the_measured_block() {
    // Every line's full width is covered by mapped glyphs, so the union is the whole block.
    let text = UiText {
        anchor: Anchor::Center,
        offset: [11.0, -5.0],
        text: "THE OLD KEEPER WAITS".to_string(),
        size: 10.0,
        wrap_width: 100.0,
        ..UiText::default()
    };
    let viewport = (1920, 1080);
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, text.clone()).unwrap();
    let scene = extract(&world, viewport, Path::new(".")).unwrap();
    let UiInstance::Text { quads, .. } = &scene.ui[0] else {
        panic!("expected Text");
    };
    let rect =
        ludion_engine::ui::resolve_ui_text(ludion_font::FontFace::embedded(), &text, viewport);
    let wrapped = ludion_engine::ui::wrap_ui_text(
        ludion_font::FontFace::embedded(),
        &text.text,
        text.size,
        text.wrap_width,
    );
    let block =
        ludion_engine::ui::measure_ui_text(ludion_font::FontFace::embedded(), &wrapped, text.size);
    assert_eq!(
        rect,
        ludion_engine::ui::resolve_ui_rect(text.anchor, text.offset, block, viewport)
    );
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for q in quads {
        min = [min[0].min(q.pos[0]), min[1].min(q.pos[1])];
        max = [max[0].max(q.pos[2]), max[1].max(q.pos[3])];
    }
    assert_eq!(min, [rect[0], rect[1]]);
    // "THE OLD" is the widest line.
    assert_eq!(max, [rect[0] + rect[2], rect[1] + rect[3]]);
}

/// `wrap_width: 0.0` — the default every authored scene carries — lowers byte-identically to the
/// un-wrapped run: the geometry no existing golden may move.
#[test]
fn an_unwrapped_ui_text_lowers_exactly_as_before() {
    let unwrapped = UiText {
        anchor: Anchor::Center,
        offset: [3.0, -7.0],
        text: "THE OLD KEEPER WAITS".to_string(),
        size: 10.0,
        color: [0.9, 0.8, 0.7, 1.0],
        order: 2,
        wrap_width: 0.0,
        visible: true,
        font: None,
    };
    let viewport = (1920, 1080);
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, unwrapped.clone()).unwrap();

    let scene = extract(&world, viewport, Path::new(".")).unwrap();
    let rect =
        ludion_engine::ui::resolve_ui_text(ludion_font::FontFace::embedded(), &unwrapped, viewport);
    let (origin_px, cell_px) = ([rect[0], rect[1]], unwrapped.size);
    assert_eq!(scene.ui.len(), 1);
    match &scene.ui[0] {
        UiInstance::Text {
            color, text, quads, ..
        } => {
            assert_eq!(*color, unwrapped.color);
            assert_eq!(text, &unwrapped.text);
            assert_eq!(first_quad(quads), (origin_px, cell_px));
        }
        other => panic!("expected one Text, got {other:?}"),
    }
}

/// A list longer than `max_visible` draws exactly the window around its cursor, and the per-line
/// colours key off the **item** index, not the line offset — the whole point of the seam.
#[test]
fn a_windowed_ui_list_draws_its_window_with_item_keyed_colors() {
    let mut items: Vec<UiListItem> = (0..12)
        .map(|i| list_item(&format!("ITEM{i}"), true))
        .collect();
    items[6] = list_item("ITEM6", false); // disabled, inside the window
    items[0] = list_item("ITEM0", false); // disabled, outside it
    let list = UiList {
        title: "WARES".to_string(),
        items,
        selected: 7,
        max_visible: 5,
        size: 10.0,
        line_spacing: 2.0,
        padding: 4.0,
        selected_color: [1.0, 0.5, 0.0, 1.0],
        disabled_color: [0.3, 0.3, 0.3, 1.0],
        background: [0.0, 0.0, 0.0, 0.75],
        ..UiList::default()
    };
    let viewport = (1920, 1080); // scale 1.0
    let scene = extract(&world_with(list.clone()), viewport, Path::new(".")).unwrap();
    assert_eq!(scene.ui.len(), 7, "1 panel + title + 5 windowed items");

    // The cursor sits mid-list, so the window is centred on it: items 5..10.
    let layout =
        ludion_engine::ui::resolve_ui_list(ludion_font::FontFace::embedded(), &list, viewport);
    let expected = [
        ("WARES", list.color),
        ("ITEM5", list.color),
        ("ITEM6", list.disabled_color),
        ("ITEM7", list.selected_color),
        ("ITEM8", list.color),
        ("ITEM9", list.color),
    ];
    for (line, (expected_text, expected_color)) in expected.iter().enumerate() {
        match &scene.ui[line + 1] {
            UiInstance::Text {
                color, text, quads, ..
            } => {
                assert_eq!(text, expected_text, "line {line}");
                assert_eq!(color, expected_color, "line {line} colour");
                assert_eq!(
                    first_quad(quads).0,
                    [
                        layout.origin_px[0],
                        layout.origin_px[1] + line as f32 * layout.line_advance_px
                    ],
                    "line {line} advances by (size + line_spacing)"
                );
            }
            other => panic!("expected Text for line {line}, got {other:?}"),
        }
    }
}

/// A world holding one scrolling, tiled sprite plus the camera that frames it.
fn scroll_world(sprite: Sprite) -> World {
    let mut world = World::new();
    let camera = world.spawn();
    world
        .insert(
            camera,
            Transform {
                translation: [0.0, 0.0, 8.0],
                ..Transform::default()
            },
        )
        .unwrap();
    world.insert(camera, Camera::default()).unwrap();
    let quad = world.spawn();
    world.insert(quad, Transform::default()).unwrap();
    world.insert(quad, sprite).unwrap();
    world
}

/// The one sprite instance `world` extracts at `tick`.
fn only_sprite_at(world: &World, tick: u64) -> ludion_renderer::SpriteInstance {
    let scene = extract_at(
        world,
        (320, 180),
        Path::new("."),
        SimTime::from_ticks(tick, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("the scrolling world extracts");
    assert_eq!(scene.sprites.len(), 1, "one quad in, one quad out");
    scene.sprites[0]
}

/// The default quad neither tiles nor scrolls, so it stays on the shader's clamping path.
#[test]
fn a_default_sprite_carries_no_tiling_and_no_offset() {
    let sprite = only_sprite_at(&scroll_world(Sprite::default()), 0);
    assert_eq!(sprite.uv_repeat, [1.0, 1.0]);
    assert_eq!(sprite.uv_offset, [0.0, 0.0]);
}

/// `uv_scroll` is evaluated against the frame's own `SimTime`: two different `now`s give two
/// different offsets over one unchanged UV rect — the quad samples elsewhere, it does not move.
#[test]
fn a_scrolling_sprite_offsets_with_time_and_leaves_its_rect_alone() {
    let world = scroll_world(Sprite {
        uv_repeat: [4.0, 1.0],
        uv_scroll: [0.25, 0.0],
        ..Sprite::default()
    });
    let rest = only_sprite_at(&world, 0);
    let later = only_sprite_at(&world, 30); // 0.5 s at the default tick rate

    assert_eq!(rest.uv_offset, [0.0, 0.0], "no time, no scroll");
    assert_eq!(later.uv_offset, [0.125, 0.0], "0.25 UV/s for half a second");
    assert_eq!(
        later.uv, rest.uv,
        "scrolling never touches the sampled rect"
    );
    assert_eq!(later.uv_repeat, [4.0, 1.0], "tiling is carried through");
    assert_eq!(later.center, rest.center, "and the quad itself never moves");
}

/// The offset wraps rather than growing without bound: one full frame width per second means every
/// whole second is the offset the session started at, however long it has been running.
#[test]
fn a_scroll_offset_wraps_into_the_unit_interval() {
    let world = scroll_world(Sprite {
        uv_scroll: [1.0, -1.0],
        ..Sprite::default()
    });
    for tick in [60u64, 600, 6000, 60_000] {
        let sprite = only_sprite_at(&world, tick);
        assert_eq!(
            sprite.uv_offset,
            [0.0, 0.0],
            "a whole number of frame widths is no offset at all (tick {tick})"
        );
    }
    // A negative rate wraps the *other* way — `rem_euclid`, not `%`, so the offset is never
    // negative and the shader's `fract` sees the same value a positive rate would land on.
    let quarter = only_sprite_at(&world, 15);
    assert_eq!(quarter.uv_offset, [0.25, 0.75]);
}

/// A wrapping quad mirrors the tiled coordinate, not the rect: `flip_x` walks the tiling backwards
/// so the whole strip reverses, and the sampled sub-rectangle stays in its authored orientation.
#[test]
fn flipping_a_wrapping_sprite_reverses_the_tiling_instead_of_the_rect() {
    let plain = only_sprite_at(&scroll_world(Sprite::default()), 0);
    let world = scroll_world(Sprite {
        uv_repeat: [4.0, 2.0],
        uv_scroll: [0.25, 0.0],
        flip_x: true,
        flip_y: true,
        ..Sprite::default()
    });
    let flipped = only_sprite_at(&world, 30);

    assert_eq!(flipped.uv, plain.uv, "the rect is left as authored");
    assert_eq!(flipped.uv_repeat, [-4.0, -2.0], "both axes walk backwards");
    // x: (4 + 0.125) rem_euclid 1 = 0.125; y: (2 + 0) rem_euclid 1 = 0.
    assert_eq!(flipped.uv_offset, [0.125, 0.0]);
}

/// A *clamping* quad still flips its rect — the path every sprite golden was blessed against is
/// untouched by the wrapping arithmetic beside it.
#[test]
fn flipping_a_clamping_sprite_still_mirrors_its_rect() {
    let world = scroll_world(Sprite {
        flip_x: true,
        ..Sprite::default()
    });
    let sprite = only_sprite_at(&world, 30);
    assert_eq!(sprite.uv, [1.0, 0.0, 0.0, 1.0], "the u endpoints swap");
    assert_eq!(sprite.uv_repeat, [1.0, 1.0]);
    assert_eq!(sprite.uv_offset, [0.0, 0.0]);
}

// --- Fonts by name (Plan/font-assets.md W3) ----------------------------------------------------

/// A scratch content root whose `fonts.yaml` maps `body` to the gallery's Inter (SDF) and `pixel` to
/// Silkscreen (bitmap); the faces are copied at test time.
fn font_root(tag: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-extract-font-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("fonts")).unwrap();
    let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/fonts");
    for file in [
        "inter.font.yaml",
        "inter.png",
        "silkscreen.font.yaml",
        "silkscreen.png",
    ] {
        std::fs::copy(gallery.join(file), dir.join("fonts").join(file)).unwrap();
    }
    std::fs::write(
        dir.join("fonts.yaml"),
        "format: 1\nkind: fonts\nfonts:\n  body: fonts/inter.font.yaml\n  pixel: fonts/silkscreen.font.yaml\n",
    )
    .unwrap();
    dir
}

fn atlas_size(root: &Path, doc: &str) -> (u32, u32) {
    let source = std::fs::read_to_string(root.join(doc)).unwrap();
    let doc = ludion_scene::check_font(&source).unwrap();
    (doc.atlas.width, doc.atlas.height)
}

fn ui_text_world(font: Option<&str>) -> World {
    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            UiText {
                text: "Hello".to_string(),
                size: 20.0,
                font: font.map(str::to_string),
                ..UiText::default()
            },
        )
        .unwrap();
    world
}

fn text_font(scene: &ludion_renderer::RenderScene, at: usize) -> Option<ludion_renderer::FontDraw> {
    match &scene.ui[at] {
        UiInstance::Text { font, .. } => *font,
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn a_named_face_draws_from_its_own_atlas() {
    let root = font_root("named");
    let body = extract(&ui_text_world(Some("body")), (640, 360), &root).unwrap();
    let font = text_font(&body, 0).expect("a table face names its atlas");
    assert!(font.sdf);
    let image = &body.textures[font.texture];
    assert_eq!(
        (image.width, image.height),
        atlas_size(&root, "fonts/inter.font.yaml")
    );

    let pixel = extract(&ui_text_world(Some("pixel")), (640, 360), &root).unwrap();
    let font = text_font(&pixel, 0).expect("a bitmap face names its atlas");
    assert!(!font.sdf);
    let image = &pixel.textures[font.texture];
    assert_eq!(
        (image.width, image.height),
        atlas_size(&root, "fonts/silkscreen.font.yaml")
    );
}

#[test]
fn text_without_a_face_or_with_an_unknown_one_draws_the_embedded_atlas() {
    let root = font_root("embedded");
    let plain = extract(&ui_text_world(None), (640, 360), &root).unwrap();
    let unknown = extract(&ui_text_world(Some("nope")), (640, 360), &root).unwrap();
    for scene in [&plain, &unknown] {
        assert_eq!(text_font(scene, 0), None);
        assert!(scene.textures.is_empty());
        let UiInstance::Text { quads, .. } = &scene.ui[0] else {
            unreachable!()
        };
        let text = UiText {
            text: "Hello".to_string(),
            size: 20.0,
            ..UiText::default()
        };
        let rect = ludion_engine::ui::resolve_ui_text(
            ludion_font::FontFace::embedded(),
            &text,
            (640, 360),
        );
        let expected = ludion_font::layout(ludion_font::FontFace::embedded(), "Hello", 20.0, 0.0)
            .quads(
                [rect[0], rect[1]],
                20.0 * (360.0 / 1080.0),
                ludion_font::YAxis::Down,
            );
        assert_eq!(quads, &expected);
    }
}

#[test]
fn a_named_face_with_a_missing_atlas_is_an_extract_error() {
    let root = font_root("missing");
    std::fs::remove_file(root.join("fonts/inter.png")).unwrap();
    let errors = extract(&ui_text_world(Some("body")), (640, 360), &root).unwrap_err();
    let error = errors
        .iter()
        .find(|d| d.code.as_str() == "L1101")
        .expect("a missing atlas is an L1101");
    assert_eq!(
        error.path.as_deref(),
        Some("/entities/0/components/UiText/font")
    );
}

#[test]
fn an_atlas_of_the_wrong_size_is_l1255_and_not_drawn() {
    let root = font_root("wrong-size");
    std::fs::copy(
        root.join("fonts/silkscreen.png"),
        root.join("fonts/inter.png"),
    )
    .unwrap();
    let errors = extract(&ui_text_world(Some("body")), (640, 360), &root).unwrap_err();
    let error = errors
        .iter()
        .find(|d| d.code.as_str() == "L1255")
        .expect("a mismatched atlas is an L1255");
    assert_eq!(
        error.path.as_deref(),
        Some("/entities/0/components/UiText/font")
    );
    assert!(error.message.contains("declares"), "{}", error.message);
}

#[test]
fn a_ui_list_names_its_face_on_every_line() {
    let root = font_root("list");
    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            UiList {
                items: vec![
                    UiListItem {
                        text: "One".to_string(),
                        ..UiListItem::default()
                    },
                    UiListItem {
                        text: "Two".to_string(),
                        ..UiListItem::default()
                    },
                ],
                font: Some("body".to_string()),
                ..UiList::default()
            },
        )
        .unwrap();
    let scene = extract(&world, (640, 360), &root).unwrap();
    let lines: Vec<_> = scene
        .ui
        .iter()
        .filter(|i| matches!(i, UiInstance::Text { .. }))
        .collect();
    assert_eq!(lines.len(), 2);
    for line in lines {
        let UiInstance::Text { font, .. } = line else {
            unreachable!()
        };
        assert!(font.is_some_and(|f| f.sdf));
    }
}

#[test]
fn world_text_carries_its_face_and_the_laid_out_extent() {
    let root = font_root("world");
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Transform::default()).unwrap();
    world
        .insert(
            e,
            ludion_engine::Text {
                text: "Hello world".to_string(),
                size: 2.0,
                font: Some("body".to_string()),
                ..ludion_engine::Text::default()
            },
        )
        .unwrap();
    let scene = extract(&world, (640, 360), &root).unwrap();
    let run = &scene.texts[0];
    let font = run.font.expect("a table face names its atlas");
    assert!(font.sdf);
    assert!(font.texture < scene.textures.len());
    let library = ludion_engine::FontLibrary::load(&root);
    let expected =
        ludion_font::layout(library.face(Some("body")), "Hello world", 2.0, 0.0).extent();
    assert_eq!(run.extent, expected);
}

// --- Clockout 0006: DropShadow through extract ---

/// Spawns an entity with a `Transform`, an optional parent and an optional `Mesh`.
fn shadow_entity(
    world: &mut World,
    parent: Option<ludion_core::ids::EntityId>,
    transform: Transform,
    mesh: bool,
) -> ludion_core::ids::EntityId {
    let e = world.spawn();
    world.insert(e, transform).unwrap();
    if mesh {
        world.insert(e, Mesh::default()).unwrap();
    }
    if let Some(parent) = parent {
        world.insert(e, ludion_ecs::Parent(parent)).unwrap();
    }
    e
}

/// A scene with no `DropShadow` extracts an empty list.
#[test]
fn a_scene_without_drop_shadows_extracts_none() {
    let scene = extract(&scene_world(), (320, 180), Path::new(".")).unwrap();
    assert!(scene.drop_shadows.is_empty());
}

/// The anchor is the entity's world position through the hierarchy, and the shape is multiplied by
/// the composed uniform scale.
#[test]
fn a_drop_shadow_anchors_at_its_world_position_with_scaled_dimensions() {
    let mut world = World::new();
    let root = shadow_entity(
        &mut world,
        None,
        Transform {
            translation: [10.0, 2.0, -3.0],
            scale: [2.0, 2.0, 2.0],
            ..Transform::default()
        },
        false,
    );
    let soles = shadow_entity(
        &mut world,
        Some(root),
        Transform {
            translation: [1.0, -1.0, 0.0],
            ..Transform::default()
        },
        false,
    );
    world
        .insert(
            soles,
            DropShadow {
                shape: GroundMarkerShape::Segment {
                    length: 2.0,
                    width: 0.5,
                },
                opacity: 0.3,
                softness: 0.2,
                max_distance: 9.0,
                fade_distance: 3.0,
            },
        )
        .unwrap();
    let disc = shadow_entity(&mut world, None, Transform::default(), false);
    world.insert(disc, DropShadow::default()).unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(scene.drop_shadows.len(), 2);
    let shadow = &scene.drop_shadows[0];
    assert!((shadow.anchor - Vec3::new(12.0, 0.0, -3.0)).length() < 1e-5);
    assert_eq!(
        shadow.shape,
        RendererMarkerShape::Segment {
            length: 4.0,
            width: 1.0
        }
    );
    assert_eq!(
        (
            shadow.opacity,
            shadow.softness,
            shadow.max_distance,
            shadow.fade_distance
        ),
        (0.3, 0.2, 9.0, 3.0)
    );
    assert_eq!(
        scene.drop_shadows[1].shape,
        RendererMarkerShape::Disc { radius: 0.5 }
    );
}

/// The segment yaw reads rotation about +Y only: a 45 degree yaw gives pi/4, and a pitch on top of
/// it leaves that yaw alone.
#[test]
fn a_drop_shadow_segment_takes_only_the_yaw_of_its_rotation() {
    let yaw_of = |rotation: [f32; 3]| {
        let mut world = World::new();
        let e = shadow_entity(
            &mut world,
            None,
            Transform {
                rotation,
                ..Transform::default()
            },
            false,
        );
        world.insert(e, DropShadow::default()).unwrap();
        extract(&world, (320, 180), Path::new("."))
            .unwrap()
            .drop_shadows[0]
            .yaw
    };
    // A +Y rotation by +45 degrees carries +X toward -Z; `atan2(-z, x)` reads that back as +pi/4,
    // the angle about +Y that maps +X onto the length axis.
    let quarter = std::f32::consts::FRAC_PI_4;
    assert!((yaw_of([0.0, 45.0, 0.0]) - quarter).abs() < 1e-4);
    assert_eq!(yaw_of([0.0, 0.0, 0.0]), 0.0);
    // Pitch about Z tilts +X off the ground plane but its ground projection still follows the yaw.
    assert!((yaw_of([0.0, 45.0, 30.0]) - quarter).abs() < 1e-4);
}

/// `exclude` lists the meshes of the caster's whole tree (root, children, the shadow's own entity)
/// and no unrelated mesh; an unparented caster excludes itself.
#[test]
fn a_drop_shadow_excludes_its_casters_hierarchy_but_not_other_meshes() {
    let mut world = World::new();
    let other = shadow_entity(&mut world, None, Transform::default(), true); // mesh 0
    let root = shadow_entity(&mut world, None, Transform::default(), true); // mesh 1
    let child = shadow_entity(&mut world, Some(root), Transform::default(), true); // mesh 2
    let soles = shadow_entity(&mut world, Some(child), Transform::default(), false);
    world.insert(soles, DropShadow::default()).unwrap();
    let _ = other;

    let solo = shadow_entity(&mut world, None, Transform::default(), true); // mesh 3
    world.insert(solo, DropShadow::default()).unwrap();

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(scene.meshes.len(), 4);
    assert_eq!(scene.drop_shadows.len(), 2);
    assert_eq!(scene.drop_shadows[0].exclude, [1, 2]);
    assert_eq!(scene.drop_shadows[1].exclude, [3]);
}

// --- Clockout 0012: Decal through extract ---

fn decal_entity(
    world: &mut World,
    parent: Option<ludion_core::ids::EntityId>,
    transform: Transform,
    decal: Decal,
) -> ludion_core::ids::EntityId {
    let e = shadow_entity(world, parent, transform, false);
    world.insert(e, decal).unwrap();
    e
}

/// A scene with no `Decal` extracts an empty list.
#[test]
fn a_scene_without_decals_extracts_none() {
    let scene = extract(&scene_world(), (320, 180), Path::new(".")).unwrap();
    assert!(scene.decals.is_empty());
}

/// The box matrices compose the parent chain, the authored size and the world scale: the unit
/// cube's corners land on the expected world points, and the inverse undoes the forward matrix.
#[test]
fn a_parented_scaled_decal_carries_its_box_matrices() {
    let mut world = World::new();
    let root = shadow_entity(
        &mut world,
        None,
        Transform {
            translation: [10.0, 0.0, 0.0],
            rotation: [0.0, 90.0, 0.0],
            scale: [2.0, 2.0, 2.0],
        },
        false,
    );
    decal_entity(
        &mut world,
        Some(root),
        Transform {
            translation: [1.0, 0.0, 0.0],
            ..Transform::default()
        },
        Decal {
            size: [2.0, 1.0, 4.0],
            ..Decal::default()
        },
    );

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(scene.decals.len(), 1);
    let decal = &scene.decals[0];
    // The child sits at local +X 1, which the root's 90 degree yaw carries to world -Z 2 (scale 2);
    // the box is size x world scale = [4, 2, 8] along the child's rotated axes.
    let centre = Vec3::new(10.0, 0.0, -2.0);
    // Local +X maps to world -Z under a +90 degree yaw, local +Z to world +X.
    let expected = |x: f32, y: f32, z: f32| centre + Vec3::new(z * 8.0, y * 2.0, -x * 4.0);
    for (x, y, z) in [
        (0.5, 0.5, 0.5),
        (-0.5, 0.5, 0.5),
        (0.5, -0.5, -0.5),
        (-0.5, -0.5, 0.5),
    ] {
        let world_point = decal.box_to_world.transform_point3(Vec3::new(x, y, z));
        assert!(
            (world_point - expected(x, y, z)).length() < 1e-4,
            "corner ({x}, {y}, {z}) landed at {world_point:?}"
        );
    }
    let product = decal.world_to_box * decal.box_to_world;
    assert!(product.abs_diff_eq(Mat4::IDENTITY, 1e-5), "{product:?}");
}

/// A decal scaled to nothing covers nothing and is skipped.
#[test]
fn a_zero_scale_decal_is_skipped() {
    let mut world = World::new();
    decal_entity(
        &mut world,
        None,
        Transform {
            scale: [0.0, 1.0, 1.0],
            ..Transform::default()
        },
        Decal::default(),
    );
    decal_entity(&mut world, None, Transform::default(), Decal::default());
    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert_eq!(scene.decals.len(), 1);
}

/// Decals draw in ascending layer, and within a layer the lowest entity id draws last.
#[test]
fn decals_extract_in_layer_then_descending_id_order() {
    let mut world = World::new();
    let mut spawn = |layer: i16, x: f32| {
        decal_entity(
            &mut world,
            None,
            Transform {
                translation: [x, 0.0, 0.0],
                ..Transform::default()
            },
            Decal {
                layer,
                ..Decal::default()
            },
        )
    };
    // a < b < c by id; the x translation tags each.
    let _a = spawn(1, 1.0);
    let _b = spawn(0, 2.0);
    let _c = spawn(0, 3.0);
    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    let order: Vec<f32> = scene
        .decals
        .iter()
        .map(|d| d.box_to_world.transform_point3(Vec3::ZERO).x)
        .collect();
    assert_eq!(order, [3.0, 2.0, 1.0]);
}

/// `all` excludes exactly the skinned instances; `hierarchy` admits the unskinned meshes of the
/// decal's root and descendants and no unrelated mesh.
#[test]
fn decal_receivers_follow_the_skin_and_the_hierarchy() {
    use ludion_scene::{JointRest, MeshDocument as SceneMeshDocument, MeshJoint, MeshSkin};

    let dir = std::env::temp_dir().join(format!("ludion-extract-decal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    let mut identity = [0.0; 16];
    for i in 0..4 {
        identity[i * 5] = 1.0;
    }
    let skinned_doc = SceneMeshDocument {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        skin: Some(MeshSkin {
            joints: vec![MeshJoint {
                name: "root".to_string(),
                parent: None,
                rest: JointRest {
                    translation: [0.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                },
                inverse_bind: identity,
            }],
            weights: vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            bone_indices: vec![0; 12],
        }),
        indices: vec![0, 1, 2],
        ..SceneMeshDocument::default()
    };
    std::fs::write(
        dir.join("meshes/skinned.mesh.yaml"),
        ludion_scene::emit_mesh(&skinned_doc),
    )
    .unwrap();

    let mut world = World::new();
    let other = shadow_entity(&mut world, None, Transform::default(), true); // mesh 0
    let root = shadow_entity(&mut world, None, Transform::default(), true); // mesh 1
    let child = shadow_entity(&mut world, Some(root), Transform::default(), true); // mesh 2
    let skinned = world.spawn(); // mesh 3, a descendant of the root
    world.insert(skinned, Transform::default()).unwrap();
    world
        .insert(
            skinned,
            Mesh {
                shape: MeshSource::Asset("meshes/skinned.mesh.yaml".to_string()),
                ..Mesh::default()
            },
        )
        .unwrap();
    world.insert(skinned, ludion_ecs::Parent(root)).unwrap();
    let _ = (other, child);
    decal_entity(
        &mut world,
        Some(root),
        Transform::default(),
        Decal {
            receivers: DecalReceiverSet::Hierarchy,
            ..Decal::default()
        },
    );
    decal_entity(&mut world, None, Transform::default(), Decal::default());

    let scene = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(scene.meshes.len(), 4);
    assert_eq!(scene.meshes[3].bone_offset, 1, "mesh 3 is the skinned one");
    // Hierarchy decal has the lower id and so draws last.
    assert_eq!(scene.decals.len(), 2);
    assert_eq!(scene.decals[1].receivers, DecalReceivers::Only(vec![1, 2]));
    assert_eq!(scene.decals[0].receivers, DecalReceivers::Exclude(vec![3]));

    let _ = std::fs::remove_dir_all(&dir);
}

fn decal_map(path: &str, wrap: MapWrap, scale: f32) -> Option<DecalMap> {
    Some(DecalMap {
        path: path.to_string(),
        wrap,
        scale,
    })
}

/// A decal's maps resolve into the instance with their wrap and the normal map's scale; an absent
/// map stays empty.
#[test]
fn decal_maps_resolve_with_wrap_and_normal_scale() {
    let dir =
        std::env::temp_dir().join(format!("ludion-extract-decal-maps-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("decals")).unwrap();
    let png = ludion_assets::encode_png(1, 1, &[255, 0, 0, 255]).unwrap();
    std::fs::write(dir.join("decals/c.png"), &png).unwrap();
    std::fs::write(dir.join("decals/n.png"), &png).unwrap();

    let mut world = World::new();
    decal_entity(
        &mut world,
        None,
        Transform::default(),
        Decal {
            base_color_texture: decal_map("decals/c.png", MapWrap::Repeat, 1.0),
            normal_texture: decal_map("decals/n.png", MapWrap::Clamp, 0.5),
            ..Decal::default()
        },
    );
    let scene = extract(&world, (320, 180), &dir).unwrap();
    let maps = &scene.decals[0].maps;
    let base = maps.base_color.as_ref().expect("base colour map");
    assert_eq!(base.wrap, TextureWrap::Repeat);
    assert!(matches!(base.image, MapImage::Source(_)));
    assert_eq!(
        maps.normal.as_ref().expect("normal map").wrap,
        TextureWrap::Clamp
    );
    assert_eq!(maps.normal_scale, 0.5);
    assert!(maps.metallic_roughness.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A decal naming a missing PNG is an `L1101` at the decal's map path.
#[test]
fn a_missing_decal_map_is_l1101_at_the_map_path() {
    let dir =
        std::env::temp_dir().join(format!("ludion-extract-decal-miss-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut world = World::new();
    decal_entity(
        &mut world,
        None,
        Transform::default(),
        Decal {
            base_color_texture: decal_map("decals/nope.png", MapWrap::Repeat, 1.0),
            ..Decal::default()
        },
    );
    let errors = extract(&world, (320, 180), &dir).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1101");
    let pointer = errors[0].path.as_deref().unwrap();
    assert!(
        pointer.starts_with("/entities/")
            && pointer.ends_with("/components/Decal/base_color_texture/path"),
        "{pointer}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A decal written in a prefab in a sub-directory names its map relative to the prefab; the scene
/// load rebases it, so the engine's own extract finds the PNG.
#[test]
fn a_prefab_decal_map_is_rebased_through_the_scene_load() {
    let dir = std::env::temp_dir().join(format!(
        "ludion-extract-decal-prefab-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("props")).unwrap();
    let png = ludion_assets::encode_png(1, 1, &[255, 0, 0, 255]).unwrap();
    std::fs::write(dir.join("props/stain.png"), png).unwrap();
    std::fs::write(
        dir.join("props/stain.prefab.yaml"),
        "format: 1
kind: prefab
name: stain
entities:
  - name: stain
    components:
      Transform: {}
      Decal:
        base_color_texture:
          path: stain.png
",
    )
    .unwrap();
    std::fs::write(
        dir.join("scene.yaml"),
        "format: 1
name: decal
entities:
  - name: spot
    prefab: props/stain.prefab.yaml
",
    )
    .unwrap();

    let engine = ludion_engine::Engine::builder()
        .load_scene_file(&dir.join("scene.yaml"))
        .expect("the scene loads");
    let scene = extract(engine.world(), (320, 180), &dir).unwrap();
    assert_eq!(scene.decals.len(), 1);
    assert!(scene.decals[0].maps.base_color.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}
