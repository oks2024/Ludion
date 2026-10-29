//! The GI voxelizer through `extract()`: the opt-in gate, determinism, the rasterization rules, and
//! the cell-count guardrail.
//!
//! These build a `World` directly from the built-in component types (the `extract.rs` house style)
//! and never touch a GPU — the voxelizer is pure CPU, and nothing it produces reaches a pixel.

use std::path::Path;
use std::sync::Arc;

use ludion_ecs::World;
use ludion_engine::components::GiVolume;
use ludion_engine::{Camera, GiDynamic, Mesh, MeshSource, Transform, extract};
use ludion_renderer::WorldVoxelGrid;

/// A one-cell voxel model whose palette entry authors both a color and an emissive strength.
/// Authoring the emissive is what sets the meshed document's `authored_material`, which is what
/// sends the render path — and so the voxelizer — to the palette for the material channels.
const VOXEL_EMISSIVE_CELL: &str = "\
format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
palette:
  A:
    color:
      - 0.5
      - 0.25
      - 0.125
      - 1.0
    emissive: 3.0
slices:
  -
    - \"A\"
";

/// The same one-cell model with a palette that authors **only** a color — no emissive, no
/// roughness, no metallic. That is what makes its meshed form carry `authored_material: false`, and
/// so what makes the render path take all three material channels from the `Mesh` component
/// instead. The common case: almost every committed vigil model authors colors alone.
const VOXEL_PLAIN_CELL: &str = "\
format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
palette:
  A:
    color:
      - 0.5
      - 0.25
      - 0.125
      - 1.0
slices:
  -
    - \"A\"
";

/// A two-voxel row, red at `x = 0` and green at `x = 1` — two authored cells whose flat index order
/// is known, for pinning the within-model half of the last-writer rule.
const VOXEL_TWO_GLYPH_ROW: &str = "\
format: 1
kind: voxel
size:
  - 2
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
  B:
    color:
      - 0.0
      - 1.0
      - 0.0
      - 1.0
slices:
  -
    - \"AB\"
";

/// A solid 2 x 2 x 2 block — every authored cell filled, for measuring how densely the point
/// sampler deposits a model at and past the cell size.
const VOXEL_SOLID_BLOCK: &str = "\
format: 1
kind: voxel
size:
  - 2
  - 2
  - 2
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
slices:
  -
    - \"AA\"
    - \"AA\"
  -
    - \"AA\"
    - \"AA\"
";

/// A scratch project root holding the fixture voxel models: `lamp` (palette authors a material),
/// `plain` (palette authors colors only), `row` (two glyphs), `block` (a solid 2 x 2 x 2).
fn project_with_voxel_model(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-voxelizer-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("voxels")).unwrap();
    std::fs::write(dir.join("voxels/lamp.voxel.yaml"), VOXEL_EMISSIVE_CELL).unwrap();
    std::fs::write(dir.join("voxels/plain.voxel.yaml"), VOXEL_PLAIN_CELL).unwrap();
    std::fs::write(dir.join("voxels/row.voxel.yaml"), VOXEL_TWO_GLYPH_ROW).unwrap();
    std::fs::write(dir.join("voxels/block.voxel.yaml"), VOXEL_SOLID_BLOCK).unwrap();
    dir
}

/// Spawns an entity carrying a transform and a mesh.
fn place(world: &mut World, transform: Transform, mesh: Mesh) {
    let entity = world.spawn();
    world.insert(entity, transform).unwrap();
    world.insert(entity, mesh).unwrap();
}

/// Adds the opt-in `GiVolume` singleton.
fn author_gi_volume(world: &mut World, cell_size: f32) {
    let entity = world.spawn();
    world
        .insert(
            entity,
            GiVolume {
                cell_size,
                ..GiVolume::default()
            },
        )
        .unwrap();
}

/// Adds the opt-in `GiVolume` singleton with both lattice fields authored.
fn author_gi_volume_with(world: &mut World, cell_size: f32, probe_spacing: f32) {
    let entity = world.spawn();
    world
        .insert(
            entity,
            GiVolume {
                cell_size,
                probe_spacing,
                ..GiVolume::default()
            },
        )
        .unwrap();
}

/// The set of filled cells, as `(x, y, z)` triples in ascending flat order.
fn filled_cells(grid: &WorldVoxelGrid) -> Vec<(u32, u32, u32)> {
    let [ex, ey, ez] = grid.extent;
    let mut cells = Vec::new();
    for z in 0..ez {
        for y in 0..ey {
            for x in 0..ex {
                if grid.occupancy[grid.index([x, y, z])] {
                    cells.push((x, y, z));
                }
            }
        }
    }
    cells
}

/// Every byte of a grid's three parallel buffers, as raw bit patterns — what "bit-identical" means
/// for a float buffer (`==` would call two NaNs unequal and two zeros of opposite sign equal).
fn grid_bits(grid: &WorldVoxelGrid) -> Vec<u32> {
    let mut bits = vec![
        grid.origin.x.to_bits(),
        grid.origin.y.to_bits(),
        grid.origin.z.to_bits(),
        grid.cell_size.to_bits(),
        grid.extent[0],
        grid.extent[1],
        grid.extent[2],
    ];
    bits.extend(grid.occupancy.iter().map(|&filled| u32::from(filled)));
    bits.extend(grid.albedo.iter().flatten().map(|c| c.to_bits()));
    bits.extend(grid.emissive.iter().map(|e| e.to_bits()));
    bits
}

/// A scene with a `GiVolume`, a voxel model, and both primitive kinds extracts identically twice —
/// the extract-determinism contract, now covering `RenderScene.voxel` through the same `PartialEq`
/// every other field rides.
#[test]
fn extract_with_a_gi_volume_is_deterministic() {
    let dir = project_with_voxel_model("deterministic");

    let mut world = World::new();
    let camera = world.spawn();
    world.insert(camera, Camera::default()).unwrap();
    author_gi_volume(&mut world, 0.5);
    place(
        &mut world,
        Transform {
            scale: [6.0, 1.0, 6.0],
            ..Transform::default()
        },
        Mesh {
            shape: MeshSource::Plane,
            ..Mesh::default()
        },
    );
    place(
        &mut world,
        Transform {
            translation: [1.5, 0.5, -1.0],
            ..Transform::default()
        },
        Mesh {
            shape: MeshSource::Cube,
            color: [0.2, 0.4, 0.8, 1.0],
            emissive: 2.0,
            ..Mesh::default()
        },
    );
    place(
        &mut world,
        Transform {
            translation: [-2.0, 0.0, 1.0],
            ..Transform::default()
        },
        Mesh {
            shape: MeshSource::Voxel("voxels/lamp.voxel.yaml".to_string()),
            ..Mesh::default()
        },
    );

    let a = extract(&world, (320, 180), &dir).unwrap();
    let b = extract(&world, (320, 180), &dir).unwrap();
    assert_eq!(a, b, "extract must stay deterministic with a GiVolume");

    let grid = a.voxel.as_ref().expect("a GiVolume builds a grid");
    assert!(
        grid.occupancy.iter().any(|&filled| filled),
        "the scene's geometry must fill something"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Two voxelizations of one scene are **bit-identical**, buffer for buffer — the mesher's
/// `double_run_is_bit_identical` for volumes. This is the claim the last-writer combine rule exists
/// to make true: with several instances (and, inside the voxel model, several authored cells)
/// landing in shared world cells, an unordered combine would be free to differ here.
#[test]
fn voxelizing_the_same_scene_twice_is_bit_identical() {
    let dir = project_with_voxel_model("bit-identity");

    let mut world = World::new();
    author_gi_volume(&mut world, 0.5);
    // Two overlapping cubes plus a voxel model, so world cells genuinely receive several writes.
    for x in [-0.25_f32, 0.25] {
        place(
            &mut world,
            Transform {
                translation: [x, 0.0, 0.0],
                scale: [2.0, 2.0, 2.0],
                ..Transform::default()
            },
            Mesh {
                shape: MeshSource::Cube,
                color: [0.9, 0.1, 0.3, 1.0],
                emissive: 0.5,
                ..Mesh::default()
            },
        );
    }
    place(
        &mut world,
        Transform::default(),
        Mesh {
            shape: MeshSource::Voxel("voxels/lamp.voxel.yaml".to_string()),
            ..Mesh::default()
        },
    );

    let first = extract(&world, (320, 180), &dir).unwrap().voxel.unwrap();
    let second = extract(&world, (320, 180), &dir).unwrap().voxel.unwrap();
    assert_eq!(
        grid_bits(&first),
        grid_bits(&second),
        "the voxelizer must be a pure function of the world, bit for bit"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// FNV-1a over a grid's raw bits — a stable fingerprint for pinning a grid value.
fn grid_fingerprint(grid: &WorldVoxelGrid) -> u64 {
    grid_bits(grid)
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, bit| {
            (hash ^ u64::from(*bit)).wrapping_mul(0x0100_0000_01b3)
        })
}

/// A scene with no asset mesh keeps the grid it had before asset meshes were voxelized: the pinned
/// fingerprint is the value the primitive + voxel-model path produced beforehand.
#[test]
fn a_scene_without_asset_meshes_keeps_its_pinned_grid() {
    let dir = project_with_voxel_model("pinned-grid");

    let mut world = World::new();
    author_gi_volume(&mut world, 0.5);
    place(
        &mut world,
        Transform {
            scale: [6.0, 1.0, 6.0],
            ..Transform::default()
        },
        Mesh {
            shape: MeshSource::Plane,
            ..Mesh::default()
        },
    );
    place(
        &mut world,
        Transform {
            translation: [1.5, 0.5, -1.0],
            ..Transform::default()
        },
        Mesh {
            shape: MeshSource::Cube,
            color: [0.2, 0.4, 0.8, 1.0],
            emissive: 2.0,
            ..Mesh::default()
        },
    );
    place(
        &mut world,
        Transform {
            translation: [-2.0, 0.0, 1.0],
            ..Transform::default()
        },
        Mesh {
            shape: MeshSource::Voxel("voxels/lamp.voxel.yaml".to_string()),
            ..Mesh::default()
        },
    );

    let grid = extract(&world, (320, 180), &dir).unwrap().voxel.unwrap();
    assert_eq!(
        grid_fingerprint(&grid),
        16_562_896_631_023_753_224,
        "pinned grid fingerprint"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// One axis-aligned cube fills **exactly** the cells whose centers lie inside its world box — the
/// analytic set, hand-computed, not re-derived by the code under test.
///
/// The cube spans `[0.1, 1.1] × [-0.5, 0.5]²`, so at half-unit cells the grid snaps outward to
/// `3 × 2 × 2` from origin `(0, -0.5, -0.5)`. Cell centers along X are `0.25, 0.75, 1.25`; the third
/// lies past the cube's `1.1` face, so X fills two of three columns while Y and Z fill both.
#[test]
fn an_axis_aligned_cube_fills_exactly_its_analytic_cells() {
    let mut world = World::new();
    place(
        &mut world,
        Transform {
            translation: [0.6, 0.0, 0.0],
            ..Transform::default()
        },
        Mesh {
            shape: MeshSource::Cube,
            ..Mesh::default()
        },
    );
    author_gi_volume(&mut world, 0.5);

    let grid = extract(&world, (320, 180), Path::new("."))
        .unwrap()
        .voxel
        .expect("a GiVolume builds a grid");

    assert_eq!(grid.origin.to_array(), [0.0, -0.5, -0.5]);
    assert_eq!(grid.extent, [3, 2, 2]);
    assert_eq!(grid.cell_size, 0.5);

    let mut expected = Vec::new();
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                expected.push((x, y, z));
            }
        }
    }
    assert_eq!(
        filled_cells(&grid),
        expected,
        "the cube fills exactly the eight cells whose centers it contains"
    );
    // Its albedo and emissive come from the instance's own channels.
    let index = grid.index([0, 0, 0]);
    assert_eq!(grid.albedo[index], [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(grid.emissive[index], 0.0);
}

/// A ground plane has no thickness, so it fills exactly the one cell row its world Y lands in — the
/// floor registers as solid occupancy rather than vanishing between two rows.
#[test]
fn a_ground_plane_fills_one_cell_row() {
    let mut world = World::new();
    place(
        &mut world,
        Transform {
            scale: [4.0, 1.0, 4.0],
            ..Transform::default()
        },
        Mesh {
            shape: MeshSource::Plane,
            ..Mesh::default()
        },
    );
    author_gi_volume(&mut world, 0.5);

    let grid = extract(&world, (320, 180), Path::new("."))
        .unwrap()
        .voxel
        .expect("a GiVolume builds a grid");

    assert_eq!(
        grid.extent,
        [8, 1, 8],
        "a flat axis still gets one cell row"
    );
    assert_eq!(grid.origin.to_array(), [-2.0, 0.0, -2.0]);
    assert_eq!(
        grid.occupancy.iter().filter(|&&filled| filled).count(),
        64,
        "the plane covers the whole row"
    );
}

/// A voxel model whose palette authors a material deposits that palette — the albedo modulated by
/// the instance color (the product the mesh shader forms) and the emissive taken from the palette
/// entry, which is exactly what the render path draws for this model because its
/// `authored_material` is set. The component's own `emissive` is overridden here, not ignored in
/// general (see the plain-palette test below).
#[test]
fn a_voxel_model_deposits_its_palette_under_the_instance_tint() {
    let dir = project_with_voxel_model("palette");

    let mut world = World::new();
    place(
        &mut world,
        Transform::default(),
        Mesh {
            shape: MeshSource::Voxel("voxels/lamp.voxel.yaml".to_string()),
            color: [0.5, 0.5, 0.5, 1.0],
            emissive: 9.0,
            ..Mesh::default()
        },
    );
    author_gi_volume(&mut world, 0.5);

    let grid = extract(&world, (320, 180), &dir)
        .unwrap()
        .voxel
        .expect("a GiVolume builds a grid");

    // The model is one voxel with a bottom-center origin: object box [-0.5, 0.5] × [0, 1] ×
    // [-0.5, 0.5], so the grid is 2 × 2 × 2 and the single authored cell's center lands in (1, 1, 1).
    assert_eq!(grid.extent, [2, 2, 2]);
    assert_eq!(filled_cells(&grid), vec![(1, 1, 1)]);

    let index = grid.index([1, 1, 1]);
    assert_eq!(
        grid.albedo[index],
        [0.25, 0.125, 0.0625, 1.0],
        "palette color modulated by the instance tint"
    );
    assert_eq!(
        grid.emissive[index], 3.0,
        "the palette authors a material, so the palette's emissive is what renders and what bakes"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **Voxelize what renders.** A voxel model whose palette authors *no* material renders its
/// emissive from the `Mesh` component — `extract` sets `use_vertex_material` from the meshed
/// document's `authored_material`, and `mesh.wgsl` then takes the whole material vector from the
/// instance's own scalars. So the grid must deposit the component's value too: a glowing lamp that
/// renders emissive and voxelizes as a non-emitter is a light the bake cannot see.
///
/// The palette-authoring fixture above hid this — it authors `emissive: 3.0`, so palette-always and
/// selection agree there. This is the configuration where they disagree.
#[test]
fn a_voxel_model_with_no_authored_material_deposits_the_components_emissive() {
    let dir = project_with_voxel_model("plain-palette");

    let mut world = World::new();
    place(
        &mut world,
        Transform::default(),
        Mesh {
            shape: MeshSource::Voxel("voxels/plain.voxel.yaml".to_string()),
            color: [0.5, 0.5, 0.5, 1.0],
            emissive: 4.0,
            ..Mesh::default()
        },
    );
    author_gi_volume(&mut world, 0.5);

    let grid = extract(&world, (320, 180), &dir)
        .unwrap()
        .voxel
        .expect("a GiVolume builds a grid");

    assert_eq!(filled_cells(&grid), vec![(1, 1, 1)]);
    let index = grid.index([1, 1, 1]);
    assert_eq!(
        grid.albedo[index],
        [0.25, 0.125, 0.0625, 1.0],
        "albedo takes no selection — the shader always forms vertex_color * instance_color"
    );
    assert_eq!(
        grid.emissive[index], 4.0,
        "with no authored material the component's emissive is what renders, so it is what bakes"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **The last-writer rule, pinned across instances.** Two unit cubes of different colors share every
/// cell; the combine rule says the one later in ascending entity-slot order wins, so the shared cell
/// carries the second-spawned cube's channels. Swapping the spawn order flips the result — which is
/// what rules out first-writer-wins and descending order, neither of which the two-run bit-identity
/// test can tell apart from this.
#[test]
fn overlapping_instances_keep_the_last_writers_channels() {
    const RED: ([f32; 4], f32) = ([1.0, 0.0, 0.0, 1.0], 1.0);
    const GREEN: ([f32; 4], f32) = ([0.0, 1.0, 0.0, 1.0], 2.0);

    /// The shared cell's `(albedo, emissive)` when two coincident unit cubes carrying those channels
    /// are spawned in the given order. Every cell of the 2 x 2 x 2 grid is written by both.
    fn shared_cell(first: ([f32; 4], f32), second: ([f32; 4], f32)) -> ([f32; 4], f32) {
        let mut world = World::new();
        for (color, emissive) in [first, second] {
            place(
                &mut world,
                Transform::default(),
                Mesh {
                    shape: MeshSource::Cube,
                    color,
                    emissive,
                    ..Mesh::default()
                },
            );
        }
        author_gi_volume(&mut world, 0.5);

        let grid = extract(&world, (320, 180), Path::new("."))
            .unwrap()
            .voxel
            .expect("a GiVolume builds a grid");
        assert_eq!(grid.extent, [2, 2, 2]);
        assert_eq!(
            filled_cells(&grid).len(),
            8,
            "both cubes cover the whole grid, so every cell takes two writes"
        );
        let index = grid.index([0, 0, 0]);
        (grid.albedo[index], grid.emissive[index])
    }

    assert_eq!(
        shared_cell(RED, GREEN),
        GREEN,
        "the later entity slot wins the shared cell"
    );
    assert_eq!(
        shared_cell(GREEN, RED),
        RED,
        "and swapping the authoring order flips it — the rule is order, not color"
    );
}

/// **The within-model mirror of the same rule.** Two authored cells of one voxel model land in a
/// single world cell (the model is far finer than `cell_size` here, which is the normal case); the
/// documented order is ascending flat index, so the cell carries the `x = 1` glyph's color, not the
/// `x = 0` one's.
#[test]
fn overlapping_model_cells_keep_the_last_flat_index() {
    let dir = project_with_voxel_model("within-model-order");

    let mut world = World::new();
    place(
        &mut world,
        Transform {
            translation: [4.0, 4.0, 4.0],
            ..Transform::default()
        },
        Mesh {
            shape: MeshSource::Voxel("voxels/row.voxel.yaml".to_string()),
            ..Mesh::default()
        },
    );
    // Coarse enough that the model's whole 2 x 1 x 1 box is one cell.
    author_gi_volume(&mut world, 8.0);

    let grid = extract(&world, (320, 180), &dir)
        .unwrap()
        .voxel
        .expect("a GiVolume builds a grid");

    assert_eq!(grid.extent, [1, 1, 1], "the whole model lands in one cell");
    assert_eq!(
        grid.albedo[grid.index([0, 0, 0])],
        [0.0, 1.0, 0.0, 1.0],
        "the higher flat index (the green glyph at x = 1) is the last writer"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **The point-sampling boundary, pinned as behavior — not endorsed as good.** A voxel model is
/// sampled once per authored voxel center, so it deposits a solid block only while one authored
/// voxel is no larger than one world cell. At exactly that boundary (`Transform.scale` ==
/// `cell_size`) a solid 2 x 2 x 2 model fills every cell of its grid; at twice it, the *same* model
/// fills one cell in eight and reads as a hollow shell.
///
/// The counter-intuitive half is the second case: refining `cell_size` past the model's own voxel
/// size makes occupancy worse, not better. Recorded here so a future resampling fix has a baseline
/// to move.
#[test]
fn a_voxel_model_coarser_than_the_cell_size_deposits_a_sparse_lattice() {
    let dir = project_with_voxel_model("point-sampling");

    let occupancy = |scale: f32, cell_size: f32| {
        let mut world = World::new();
        place(
            &mut world,
            Transform {
                scale: [scale; 3],
                ..Transform::default()
            },
            Mesh {
                shape: MeshSource::Voxel("voxels/block.voxel.yaml".to_string()),
                ..Mesh::default()
            },
        );
        author_gi_volume(&mut world, cell_size);
        let grid = extract(&world, (320, 180), &dir)
            .unwrap()
            .voxel
            .expect("a GiVolume builds a grid");
        (grid.extent, filled_cells(&grid).len())
    };

    assert_eq!(
        occupancy(0.5, 0.5),
        ([2, 2, 2], 8),
        "one authored voxel per world cell: the block is solid"
    );
    assert_eq!(
        occupancy(1.0, 0.5),
        ([4, 4, 4], 8),
        "one authored voxel per two world cells: the same block is 8 of 64 cells, a hollow shell"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An over-fine `cell_size` is `L2103` at the authored field, reported from the computed extent —
/// no multi-gigabyte allocation is attempted, and the help names a size that would fit.
#[test]
fn a_tiny_cell_size_is_l2103_at_the_authored_field() {
    let mut world = World::new();
    place(&mut world, Transform::default(), Mesh::default());
    author_gi_volume(&mut world, 0.001);

    let errors = extract(&world, (320, 180), Path::new(".")).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L2103");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/1/components/GiVolume/cell_size"),
    );
    assert!(
        errors[0].message.contains("0.001") && errors[0].message.contains("1000000000"),
        "the message names the authored cell size and the implied count: {}",
        errors[0].message,
    );
    assert!(
        errors[0]
            .help
            .as_deref()
            .is_some_and(|help| help.contains("cell_size")),
        "the help suggests a workable cell size: {:?}",
        errors[0].help,
    );
}

/// The probe lattice covers the grid's world box, aligned to its origin: a unit cube at half-unit
/// cells is a 1×1×1 world box, and half-unit probe spacing lays three probes along each of its axes
/// spanning exactly that box. The authored knobs ride through verbatim.
#[test]
fn the_probe_lattice_covers_the_grids_world_box() {
    let mut world = World::new();
    place(&mut world, Transform::default(), Mesh::default());
    author_gi_volume_with(&mut world, 0.5, 0.5);

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    let grid = scene.voxel.as_deref().expect("the GiVolume builds a grid");
    let gi = scene.gi.expect("a grid carries its GI parameters");

    assert_eq!(grid.extent, [2, 2, 2], "a unit cube at half-unit cells");
    assert_eq!(
        gi.count,
        [3, 3, 3],
        "ceil(1.0 / 0.5) + 1 probes along each axis"
    );
    assert_eq!(
        gi.origin, grid.origin,
        "probe (0,0,0) sits on the grid's own origin"
    );
    assert_eq!(
        gi.span.to_array(),
        [1.0, 1.0, 1.0],
        "(count - 1) x spacing, which is the grid's own 1.0-unit box"
    );
    assert_eq!(gi.probe_spacing, 0.5);
    assert_eq!(
        gi.normal_bias,
        GiVolume::default().normal_bias,
        "an unauthored normal_bias rides through as its default"
    );
    assert_eq!(gi.sky_visibility, GiVolume::default().sky_visibility);
}

/// A degenerate axis still carries two probes: an unrotated plane has no thickness, so its grid is
/// one cell tall, and a probe spacing far coarser than that cell must not collapse the axis to a
/// single probe — trilinear interpolation needs two on every axis.
#[test]
fn a_flat_axis_still_carries_the_two_probes_trilinear_needs() {
    let mut world = World::new();
    place(
        &mut world,
        Transform::default(),
        Mesh {
            shape: MeshSource::Plane,
            ..Mesh::default()
        },
    );
    author_gi_volume_with(&mut world, 0.5, 8.0);

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    let grid = scene.voxel.as_deref().expect("the GiVolume builds a grid");
    let gi = scene.gi.expect("a grid carries its GI parameters");

    assert_eq!(grid.extent[1], 1, "a flat plane fills one cell row");
    assert_eq!(
        gi.count,
        [2, 2, 2],
        "every axis floors at the two probes trilinear interpolation needs"
    );
    assert_eq!(
        gi.span.to_array(),
        [8.0, 8.0, 8.0],
        "the lattice still spans a whole probe_spacing, which covers the box with room over"
    );
}

/// An over-fine `probe_spacing` is `L2105` at the authored field, reported from the computed
/// lattice — no probe volume is allocated, and the help names a spacing that would fit.
#[test]
fn a_tiny_probe_spacing_is_l2105_at_the_authored_field() {
    let mut world = World::new();
    place(&mut world, Transform::default(), Mesh::default());
    author_gi_volume_with(&mut world, 0.5, 0.0005);

    let errors = extract(&world, (320, 180), Path::new(".")).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L2105");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/1/components/GiVolume/probe_spacing"),
    );
    assert!(
        errors[0].message.contains("0.0005") && errors[0].message.contains("2001"),
        "the message names the authored spacing and the implied lattice: {}",
        errors[0].message,
    );
    assert!(
        errors[0]
            .help
            .as_deref()
            .is_some_and(|help| help.contains("probe_spacing")),
        "the help suggests a workable probe spacing: {:?}",
        errors[0].help,
    );
}

/// A grid whose total fits but whose longest axis is past the 3D-texture cap is `L2103` at the
/// authored `cell_size`, naming the axis. This shape — 4000 x 4 x 2000 cells, *exactly* the total
/// ceiling — is what one stray entity a couple of thousand units from the rest produces, and the
/// product check alone waved it through into a `max_texture_dimension_3d` panic inside
/// `prepare_frame`. Reported from the computed extent: the grid it rejects would be over half a
/// gigabyte of buffers, and this test allocates none of it.
#[test]
fn a_grid_axis_past_the_texture_cap_is_l2103_at_the_authored_field() {
    let mut world = World::new();
    place(
        &mut world,
        Transform {
            scale: [4000.0, 4.0, 2000.0],
            ..Transform::default()
        },
        Mesh::default(),
    );
    author_gi_volume(&mut world, 1.0);

    let errors = extract(&world, (320, 180), Path::new(".")).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L2103");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/1/components/GiVolume/cell_size"),
    );
    assert!(
        errors[0].message.contains("4000 cells along X") && errors[0].message.contains("2048"),
        "the message names the breaching axis, its extent and the limit: {}",
        errors[0].message,
    );
    assert!(
        errors[0]
            .help
            .as_deref()
            .is_some_and(|help| help.contains("cell_size")),
        "the help suggests a workable cell size: {:?}",
        errors[0].help,
    );
}

/// The lattice's half of the same defect: a long, thin grid whose probe *total* is a hundred
/// thousand — nowhere near the ceiling — still lays 4001 probes along X, which is a 3D texture no
/// conformant adapter is required to create. `L2105` at the authored `probe_spacing`, naming the
/// axis.
#[test]
fn a_probe_axis_past_the_texture_cap_is_l2105_at_the_authored_field() {
    let mut world = World::new();
    place(
        &mut world,
        Transform {
            scale: [4000.0, 1.0, 1.0],
            ..Transform::default()
        },
        Mesh::default(),
    );
    author_gi_volume_with(&mut world, 2.0, 1.0);

    let errors = extract(&world, (320, 180), Path::new(".")).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L2105");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/1/components/GiVolume/probe_spacing"),
    );
    assert!(
        errors[0].message.contains("4001 irradiance probes along X")
            && errors[0].message.contains("2048"),
        "the message names the breaching axis, its probe count and the limit: {}",
        errors[0].message,
    );
    assert!(
        errors[0]
            .help
            .as_deref()
            .is_some_and(|help| help.contains("probe_spacing")),
        "the help suggests a workable probe spacing: {:?}",
        errors[0].help,
    );
}

/// A scene whose geometry sits far enough from the origin that the sizing arithmetic once overflowed
/// `f32` — both faces divided to the same infinity, `inf - inf` floored to a one-cell axis, and the
/// voxelizer sealed and cached a grid with an infinite origin that nothing ever filled. It is a
/// readable `L2103` now, not a silently empty grid.
#[test]
fn a_scene_far_from_the_origin_is_l2103_rather_than_a_degenerate_grid() {
    let mut world = World::new();
    place(
        &mut world,
        Transform {
            translation: [1e38, 0.0, 0.0],
            scale: [1e33; 3],
            ..Transform::default()
        },
        Mesh::default(),
    );
    author_gi_volume(&mut world, 0.1);

    let errors = extract(&world, (320, 180), Path::new(".")).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L2103");
}

/// The same scene without a `GiVolume` voxelizes nothing — the opt-in gate, and the reason an
/// unauthored scene takes the pipeline it took before the component existed.
#[test]
fn a_scene_without_a_gi_volume_builds_no_grid() {
    let mut world = World::new();
    place(&mut world, Transform::default(), Mesh::default());

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert!(scene.voxel.is_none(), "no GiVolume ⇒ no grid");
    assert!(scene.gi.is_none(), "and no grid ⇒ no probe lattice");
    assert_eq!(scene.meshes.len(), 1, "the mesh still renders");
}

/// A `GiVolume` over a scene that places no rasterizable geometry has no world box to bound, so it
/// yields no grid rather than a zero-cell one every consumer would have to special-case.
#[test]
fn a_gi_volume_over_an_empty_scene_builds_no_grid() {
    let mut world = World::new();
    let camera = world.spawn();
    world.insert(camera, Camera::default()).unwrap();
    author_gi_volume(&mut world, 0.5);

    let scene = extract(&world, (320, 180), Path::new(".")).unwrap();
    assert!(scene.voxel.is_none(), "nothing to bound ⇒ no grid");
    assert!(
        scene.gi.is_none(),
        "and no grid ⇒ nothing to lay probes around"
    );
}

/// **T13 — a marked mesh neither occludes nor deposits**
/// (`Plan/gi-wave3-dynamic-injection.md` §11). Two flanking plain cubes give the grid its bounds; a
/// third, emissive cube sits between them, either marked `GiDynamic` or not. A *marked* center cube
/// must voxelize as though it were never placed at all — same origin and extent (its own footprint
/// contributes nothing to the scene bounds either), and every occupancy/albedo/emissive byte
/// identical to the flanking-cubes-alone grid, so its cells stay open and its emissive never reaches
/// the bake's input. The unmarked control proves the assertion has teeth: the same cube, unmarked,
/// actually fills and lights those cells.
#[test]
fn a_marked_mesh_neither_occludes_nor_deposits() {
    fn flanking_cubes(world: &mut World) {
        for x in [-3.0_f32, 3.0] {
            let entity = world.spawn();
            world
                .insert(
                    entity,
                    Transform {
                        translation: [x, 0.0, 0.0],
                        ..Transform::default()
                    },
                )
                .unwrap();
            world
                .insert(
                    entity,
                    Mesh {
                        shape: MeshSource::Cube,
                        ..Mesh::default()
                    },
                )
                .unwrap();
        }
    }

    /// `with_center: None` builds the bounds-only baseline; `Some(dynamic)` adds the emissive
    /// center cube, marked or not.
    fn build(with_center: Option<bool>) -> Arc<WorldVoxelGrid> {
        let mut world = World::new();
        flanking_cubes(&mut world);
        if let Some(dynamic) = with_center {
            let entity = world.spawn();
            world.insert(entity, Transform::default()).unwrap();
            world
                .insert(
                    entity,
                    Mesh {
                        shape: MeshSource::Cube,
                        color: [0.2, 0.6, 0.9, 1.0],
                        emissive: 7.0,
                        ..Mesh::default()
                    },
                )
                .unwrap();
            if dynamic {
                world.insert(entity, GiDynamic::default()).unwrap();
            }
        }
        author_gi_volume(&mut world, 0.5);
        extract(&world, (320, 180), Path::new("."))
            .unwrap()
            .voxel
            .expect("the flanking cubes alone build a grid")
    }

    let bounds_only = build(None);
    let marked = build(Some(true));
    let unmarked = build(Some(false));

    assert_eq!(
        grid_bits(&marked),
        grid_bits(&bounds_only),
        "a marked cube must voxelize as if it were absent entirely: no occlusion, no deposit"
    );
    assert_ne!(
        grid_bits(&unmarked),
        grid_bits(&bounds_only),
        "control: the same cube, left unmarked, must actually change the grid"
    );
}

/// The eight corners of a unit cube centred on the origin, then its twelve triangles — the last two
/// (indices 30..36) are the top face, so dropping them leaves a box open at the top.
const CUBE_CORNERS: [f64; 24] = [
    -0.5, -0.5, -0.5, 0.5, -0.5, -0.5, 0.5, 0.5, -0.5, -0.5, 0.5, -0.5, -0.5, -0.5, 0.5, 0.5, -0.5,
    0.5, 0.5, 0.5, 0.5, -0.5, 0.5, 0.5,
];
const CUBE_TRIANGLES: [u32; 36] = [
    0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 4, 7, 0, 7, 3, 1, 2, 6, 1, 6, 5, 0, 1, 5, 0, 5, 4, 3, 7,
    6, 3, 6, 2,
];

/// A `kind: mesh` document; `emissive` authors a per-vertex emissive list (and so a material).
fn mesh_doc(positions: &[f64], indices: &[u32], emissive: Option<f64>) -> String {
    let list = |values: Vec<String>| {
        values
            .iter()
            .map(|v| format!("  - {v}\n"))
            .collect::<String>()
    };
    let mut doc = format!(
        "format: 1\nkind: mesh\npositions:\n{}indices:\n{}",
        list(positions.iter().map(|v| format!("{v:?}")).collect()),
        list(indices.iter().map(u32::to_string).collect()),
    );
    if let Some(value) = emissive {
        doc.push_str(&format!(
            "emissive:\n{}",
            list(vec![format!("{value:?}"); positions.len() / 3])
        ));
    }
    doc
}

/// A scratch project root holding the given `meshes/<name>.mesh.yaml` documents.
fn project_with_meshes(tag: &str, docs: &[(&str, String)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-voxelizer-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    for (name, doc) in docs {
        std::fs::write(dir.join(format!("meshes/{name}.mesh.yaml")), doc).unwrap();
    }
    dir
}

fn place_asset(world: &mut World, name: &str, translation: [f32; 3], scale: f32, mesh: Mesh) {
    place(
        world,
        Transform {
            translation,
            scale: [scale; 3],
            ..Transform::default()
        },
        Mesh {
            shape: MeshSource::Asset(format!("meshes/{name}.mesh.yaml")),
            ..mesh
        },
    );
}

/// Whether `cell` lies on the surface of an `n`-cell cube.
fn on_surface(cell: (u32, u32, u32), n: u32) -> bool {
    [cell.0, cell.1, cell.2]
        .iter()
        .any(|&c| c == 0 || c == n - 1)
}

/// A closed asset cube fills its surface shell — every boundary cell, no interior cell.
#[test]
fn a_closed_asset_cube_fills_a_shell_not_a_solid() {
    let dir = project_with_meshes(
        "shell",
        &[("cube", mesh_doc(&CUBE_CORNERS, &CUBE_TRIANGLES, None))],
    );
    let mut world = World::new();
    author_gi_volume(&mut world, 0.5);
    place_asset(&mut world, "cube", [0.0; 3], 4.0, Mesh::default());

    let grid = extract(&world, (320, 180), &dir).unwrap().voxel.unwrap();
    assert_eq!(grid.extent, [8, 8, 8]);
    let mut expected = 0;
    for z in 0..8 {
        for y in 0..8 {
            for x in 0..8 {
                let filled = grid.occupancy[grid.index([x, y, z])];
                assert_eq!(filled, on_surface((x, y, z), 8), "cell ({x}, {y}, {z})");
                expected += u32::from(filled);
            }
        }
    }
    assert_eq!(expected, 8 * 8 * 8 - 6 * 6 * 6);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A box open at the top stays hollow: the interior is empty and so is the open face's interior.
#[test]
fn an_open_asset_box_stays_hollow() {
    let dir = project_with_meshes(
        "hollow",
        &[("box", mesh_doc(&CUBE_CORNERS, &CUBE_TRIANGLES[..30], None))],
    );
    let mut world = World::new();
    author_gi_volume(&mut world, 0.5);
    place_asset(&mut world, "box", [0.0; 3], 4.0, Mesh::default());

    let grid = extract(&world, (320, 180), &dir).unwrap().voxel.unwrap();
    for (x, y, z) in filled_cells(&grid) {
        assert!(on_surface((x, y, z), 8), "({x}, {y}, {z}) is interior");
        let top_interior = y == 7 && (1..7).contains(&x) && (1..7).contains(&z);
        assert!(
            !top_interior,
            "the open top's interior ({x}, {y}, {z}) is filled"
        );
    }
    assert!(grid.occupancy[grid.index([0, 0, 0])]);
    assert!(grid.occupancy[grid.index([3, 0, 3])], "the floor is filled");
    assert!(!grid.occupancy[grid.index([3, 7, 3])], "the lid is not");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Albedo is the vertex colour (white) times the instance colour; emissive is the document's when it
/// authors a material and the component's when it does not.
#[test]
fn an_asset_mesh_deposits_instance_colour_and_the_render_paths_emissive() {
    let dir = project_with_meshes(
        "asset-channels",
        &[
            ("plain", mesh_doc(&CUBE_CORNERS, &CUBE_TRIANGLES, None)),
            ("lit", mesh_doc(&CUBE_CORNERS, &CUBE_TRIANGLES, Some(5.0))),
        ],
    );
    let mut world = World::new();
    author_gi_volume(&mut world, 0.5);
    let tinted = Mesh {
        color: [0.5, 0.25, 0.125, 1.0],
        emissive: 2.0,
        ..Mesh::default()
    };
    place_asset(&mut world, "plain", [-4.0, 0.0, 0.0], 2.0, tinted.clone());
    place_asset(&mut world, "lit", [4.0, 0.0, 0.0], 2.0, tinted);

    let grid = extract(&world, (320, 180), &dir).unwrap().voxel.unwrap();
    let probe = |x: f32| {
        let index = grid.index([
            ((x - grid.origin.x) / grid.cell_size) as u32,
            ((0.0 - grid.origin.y) / grid.cell_size) as u32,
            ((0.0 - grid.origin.z) / grid.cell_size) as u32,
        ]);
        (grid.albedo[index], grid.emissive[index])
    };
    // The cube's -x face sits one unit from each centre.
    assert_eq!(
        probe(-5.0),
        ([0.5, 0.25, 0.125, 1.0], 2.0),
        "component emissive"
    );
    assert_eq!(
        probe(3.0),
        ([0.5, 0.25, 0.125, 1.0], 5.0),
        "authored emissive"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The asset-mesh grid is bit-identical across runs and thread counts.
#[test]
fn an_asset_mesh_grid_is_bit_identical_across_runs_and_threads() {
    let dir = project_with_meshes(
        "asset-det",
        &[("cube", mesh_doc(&CUBE_CORNERS, &CUBE_TRIANGLES, None))],
    );
    let mut world = World::new();
    author_gi_volume(&mut world, 0.25);
    place_asset(&mut world, "cube", [0.3, 0.1, -0.2], 3.0, Mesh::default());
    place_asset(&mut world, "cube", [1.0, 0.5, 0.2], 2.0, Mesh::default());

    let build = || grid_bits(&extract(&world, (320, 180), &dir).unwrap().voxel.unwrap());
    let first = build();
    assert_eq!(first, build());
    let one = ludion_core::par::with_threads(1, build);
    let eight = ludion_core::par::with_threads(8, build);
    assert_eq!(one, first);
    assert_eq!(eight, first);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A lattice past the derived probe cap but under the old four-million ceiling is `L2105` at the
/// authored `probe_spacing` — refused before the renderer, where its hit cache would have failed the
/// storage-binding limit as `L2010`. A 127-unit cube at unit cells and unit spacing is 128 cells per
/// axis and 129 probes per axis, 2_146_689 in all.
#[test]
fn a_lattice_past_the_derived_probe_cap_is_l2105_not_a_gpu_error() {
    let cap = ludion_renderer::max_gi_probes();
    assert!(
        cap < 129 * 129 * 129 && 129 * 129 * 129 < 4_000_000,
        "cap {cap}"
    );

    let mut world = World::new();
    place(
        &mut world,
        Transform {
            scale: [127.0; 3],
            ..Transform::default()
        },
        Mesh::default(),
    );
    author_gi_volume_with(&mut world, 1.0, 1.0);

    let errors = extract(&world, (320, 180), Path::new(".")).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L2105");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/1/components/GiVolume/probe_spacing"),
    );
    assert!(
        errors[0].message.contains(&cap.to_string()),
        "the message names the limit: {}",
        errors[0].message
    );
}
