//! `MeshGrid` end-to-end: scene validation (L1019/L1020/L1243 with JSON Pointers), canonical
//! round-trip, extract equivalence with hand-placed parented meshes, legend asset errors, presentation
//! hash-invisibility and the wire `component.set` rejection path.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ludion_ecs::{Parent, World};
use ludion_engine::{
    Engine, Mesh, MeshGrid, MeshSource, Transform, builtin_registry, dispatch, extract,
};
use ludion_protocol::Command;
use ludion_protocol::command::ComponentSetParams;
use serde_json::json;

const TRI: &str = "format: 1\nkind: mesh\npositions:\n  - 0.0\n  - 0.0\n  - 0.0\n  - 1.0\n  - 0.0\n  \
                   - 0.0\n  - 0.0\n  - 1.0\n  - 0.0\nindices:\n  - 0\n  - 1\n  - 2\n";

/// Scratch project root with `meshes/tri.mesh.yaml`, one directory per test name.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-mesh-grid-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    std::fs::write(dir.join("meshes/tri.mesh.yaml"), TRI).unwrap();
    dir
}

fn tri_mesh(color: [f32; 4]) -> Mesh {
    Mesh {
        shape: MeshSource::Asset("meshes/tri.mesh.yaml".to_string()),
        color,
        ..Mesh::default()
    }
}

/// A scene with one `MeshGrid` whose `extra` lines (already indented 8 spaces) follow the legend.
fn grid_scene(legend_key: &str, rows: &[&str], extra: &str) -> String {
    let rows: String = rows
        .iter()
        .map(|r| format!("          - \"{r}\"\n"))
        .collect();
    format!(
        "format: 1\nentities:\n  - components:\n      MeshGrid:\n        legend:\n          \
         \"{legend_key}\":\n            shape:\n              asset: meshes/tri.mesh.yaml\n        \
         rows:\n{rows}{extra}"
    )
}

fn diagnostics(source: &str) -> Vec<ludion_core::diagnostics::Diagnostic> {
    ludion_scene::check(source, &builtin_registry()).expect_err("the scene must fail validation")
}

fn one_diagnostic(source: &str) -> ludion_core::diagnostics::Diagnostic {
    let mut all = diagnostics(source);
    assert_eq!(all.len(), 1, "expected exactly one diagnostic: {all:?}");
    all.remove(0)
}

#[test]
fn ragged_rows_is_l1019_pointing_at_the_row() {
    let diagnostic = one_diagnostic(&grid_scene("a", &["aa", "a"], ""));
    assert_eq!(diagnostic.code.as_str(), "L1019");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/entities/0/components/MeshGrid/rows/1")
    );
}

#[test]
fn unknown_glyph_is_l1020_and_a_space_is_exempt() {
    let diagnostic = one_diagnostic(&grid_scene("a", &["a x"], ""));
    assert_eq!(diagnostic.code.as_str(), "L1020");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/entities/0/components/MeshGrid/rows/0")
    );
    assert!(diagnostic.message.contains('x'), "{}", diagnostic.message);
}

#[test]
fn a_space_legend_key_is_l1020() {
    let diagnostic = one_diagnostic(&grid_scene(" ", &["  "], ""));
    assert_eq!(diagnostic.code.as_str(), "L1020");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/entities/0/components/MeshGrid/legend/ ")
    );
}

#[test]
fn orient_shape_and_glyphs_are_l1243() {
    // Wrong row count, reported once at `/orient`.
    let diagnostic = one_diagnostic(&grid_scene(
        "a",
        &["aa", "aa"],
        "        orient:\n          - \"00\"\n",
    ));
    assert_eq!(diagnostic.code.as_str(), "L1243");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/entities/0/components/MeshGrid/orient")
    );

    // Wrong width, at the offending row.
    let diagnostic = one_diagnostic(&grid_scene(
        "a",
        &["aa", "aa"],
        "        orient:\n          - \"00\"\n          - \"0\"\n",
    ));
    assert_eq!(diagnostic.code.as_str(), "L1243");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/entities/0/components/MeshGrid/orient/1")
    );

    // A glyph outside 0-7, naming the column and glyph.
    let diagnostic = one_diagnostic(&grid_scene(
        "a",
        &["aa"],
        "        orient:\n          - \"08\"\n",
    ));
    assert_eq!(diagnostic.code.as_str(), "L1243");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/entities/0/components/MeshGrid/orient/0")
    );
    assert!(diagnostic.message.contains("`8`"), "{}", diagnostic.message);
    assert!(
        diagnostic.message.contains("column 1"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn a_character_legend_shape_is_l1244() {
    let source = "format: 1\nentities:\n  - components:\n      MeshGrid:\n        legend:\n          \
                  a:\n            shape:\n              character: characters/goblin.character.yaml\n        \
                  rows:\n          - \"a\"\n";
    let diagnostic = one_diagnostic(source);
    assert_eq!(diagnostic.code.as_str(), "L1244");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/entities/0/components/MeshGrid/legend/a")
    );
}

#[test]
fn empty_and_holey_grids_check_clean() {
    let registry = builtin_registry();
    let empty = "format: 1\nentities:\n  - components:\n      MeshGrid: {}\n";
    assert!(ludion_scene::check(empty, &registry).is_ok());
    let holes = grid_scene(
        "a",
        &["a a", "   "],
        "        orient:\n          - \"0 7\"\n          - \"   \"\n",
    );
    assert!(ludion_scene::check(&holes, &registry).is_ok());
}

/// Canonical emission is a fixed point over the gallery scene, and `orient` is omitted when empty.
#[test]
fn mesh_grid_canonical_emission_is_idempotent() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/mesh-grid.yaml");
    let source = std::fs::read_to_string(&path).expect("gallery mesh-grid scene reads");
    let registry = builtin_registry();
    let scene = ludion_scene::check(&source, &registry).expect("gallery mesh-grid is valid");
    let once = ludion_scene::emit(&scene, &registry);
    let reparsed = ludion_scene::check(&once, &registry).expect("emitted scene re-validates");
    assert_eq!(once, ludion_scene::emit(&reparsed, &registry));
    assert!(once.contains("orient:"));

    let bare = ludion_scene::check(&grid_scene("a", &["aa"], ""), &registry).unwrap();
    assert!(!ludion_scene::emit(&bare, &registry).contains("orient"));
}

/// The grid is the same draw as hand-placed parented entities: root carrying the grid's pose, one
/// child per non-empty cell with the cell's `Transform` and the legend `Mesh`.
#[test]
fn a_grid_extracts_like_the_equivalent_parented_entities() {
    let dir = project("equivalence");
    let pose = Transform {
        translation: [3.0, 0.5, -2.0],
        rotation: [0.0, 30.0, 0.0],
        scale: [2.0, 2.0, 2.0],
    };
    let cell_size = 1.5;
    let rows = ["abc", "c b", "abc"];
    // All eight orientation glyphs over the eight cells around a centre hole.
    let orient = ["012", "3 4", "567"];
    let colors = [
        ('a', [1.0, 0.0, 0.0, 1.0]),
        ('b', [0.0, 1.0, 0.0, 1.0]),
        ('c', [0.0, 0.0, 1.0, 1.0]),
    ];
    let color_of = |glyph: char| colors.iter().find(|(g, _)| *g == glyph).unwrap().1;

    let mut grid_world = World::new();
    let grid = grid_world.spawn();
    grid_world.insert(grid, pose).unwrap();
    grid_world
        .insert(
            grid,
            MeshGrid {
                cell_size,
                legend: colors
                    .iter()
                    .map(|&(glyph, color)| (glyph.to_string(), tri_mesh(color)))
                    .collect::<BTreeMap<_, _>>(),
                rows: rows.iter().map(|r| r.to_string()).collect(),
                orient: orient.iter().map(|r| r.to_string()).collect(),
            },
        )
        .unwrap();

    let mut hand_world = World::new();
    let root = hand_world.spawn();
    hand_world.insert(root, pose).unwrap();
    for (row, line) in rows.iter().enumerate() {
        for (col, glyph) in line.chars().enumerate() {
            if glyph == ' ' {
                continue;
            }
            let digit = orient[row].chars().nth(col).unwrap();
            let k = digit.to_digit(8).unwrap() as f32;
            let mirror = if k >= 4.0 { -1.0 } else { 1.0 };
            let child = hand_world.spawn();
            hand_world.insert(child, Parent(root)).unwrap();
            hand_world
                .insert(
                    child,
                    Transform {
                        translation: [col as f32 * cell_size, 0.0, row as f32 * cell_size],
                        rotation: [0.0, 90.0 * (k % 4.0), 0.0],
                        scale: [mirror, 1.0, 1.0],
                    },
                )
                .unwrap();
            hand_world.insert(child, tri_mesh(color_of(glyph))).unwrap();
        }
    }

    let from_grid = extract(
        &grid_world,
        (320, 180),
        ludion_assets::AssetRoot::content(&dir),
    )
    .unwrap();
    let by_hand = extract(
        &hand_world,
        (320, 180),
        ludion_assets::AssetRoot::content(&dir),
    )
    .unwrap();
    assert_eq!(from_grid.mesh_assets.len(), by_hand.mesh_assets.len());
    assert_eq!(from_grid.meshes.len(), 8, "nine cells minus the hole");
    assert_eq!(from_grid.meshes.len(), by_hand.meshes.len());
    for (i, (a, b)) in from_grid.meshes.iter().zip(&by_hand.meshes).enumerate() {
        assert_eq!(a.shape, b.shape, "instance {i}");
        assert_eq!(a.color, b.color, "instance {i}");
        assert_eq!(a.material, b.material, "instance {i}");
        assert!(
            a.model.abs_diff_eq(b.model, 1e-5),
            "instance {i} model\n{:?}\n{:?}",
            a.model,
            b.model
        );
        // A uniform scale keeps the model's 3x3 and a mirrored cell takes the inverse-transpose, which
        // differ by a positive factor the shader normalises away: the column directions must agree.
        for (ca, cb) in [
            (a.normal_matrix.x_axis, b.normal_matrix.x_axis),
            (a.normal_matrix.y_axis, b.normal_matrix.y_axis),
            (a.normal_matrix.z_axis, b.normal_matrix.z_axis),
        ] {
            assert!(
                ca.normalize().abs_diff_eq(cb.normalize(), 1e-5),
                "instance {i} normal matrix\n{:?}\n{:?}",
                a.normal_matrix,
                b.normal_matrix
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_legend_asset_is_l1101_at_the_legend_entry() {
    let dir = std::env::temp_dir().join(format!("ludion-mesh-grid-{}-missing", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut world = World::new();
    let grid = world.spawn();
    world
        .insert(
            grid,
            MeshGrid {
                legend: BTreeMap::from([("a".to_string(), tri_mesh([1.0; 4]))]),
                rows: vec!["aa".to_string()],
                ..MeshGrid::default()
            },
        )
        .unwrap();
    let errors = extract(&world, (320, 180), ludion_assets::AssetRoot::content(&dir)).unwrap_err();
    assert_eq!(
        errors.len(),
        1,
        "a path that fails reports once: {errors:?}"
    );
    assert_eq!(errors[0].code.as_str(), "L1101");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/MeshGrid/legend/a/shape/asset")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `presentation: true` grid loads, draws every cell, and leaves the sim hash where it was.
#[test]
fn a_presentation_grid_draws_and_does_not_move_the_hash() {
    let dir = project("presentation");
    let base = "format: 1\nentities:\n  - name: a\n    components:\n      Transform: {}\n";
    let grid = "  - name: ground\n    presentation: true\n    components:\n      Transform: {}\n      \
                MeshGrid:\n        legend:\n          a:\n            shape:\n              asset: \
                meshes/tri.mesh.yaml\n        rows:\n          - \"aa\"\n          - \"a \"\n";
    let load = |source: &str| {
        let mut engine = Engine::builder().project_root(&dir).build().unwrap();
        engine.load_inline_scene(source).unwrap();
        engine
    };
    let mut with = load(&format!("{base}{grid}"));
    let mut without = load(base);
    assert_eq!(with.world_hash().unwrap(), without.world_hash().unwrap());
    for _ in 0..3 {
        with.advance_live();
        without.advance_live();
    }
    assert_eq!(with.world_hash().unwrap(), without.world_hash().unwrap());

    let scene = extract(
        with.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(&dir),
    )
    .unwrap();
    assert_eq!(scene.meshes.len(), 3, "three non-empty cells draw");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A wire `component.set` of an invalid grid fails before any world change.
#[test]
fn component_set_rejects_an_invalid_mesh_grid() {
    let mut engine = Engine::new();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    let entity = engine.world_mut().spawn();
    let set = |engine: &mut Engine, value| {
        dispatch(
            engine,
            Command::ComponentSet(ComponentSetParams {
                entity,
                component: "MeshGrid".to_string(),
                value,
            }),
        )
    };
    let legend = json!({ "a": { "shape": { "asset": "meshes/tri.mesh.yaml" } } });
    let error = set(
        &mut engine,
        json!({ "legend": legend, "rows": ["aa", "a"] }),
    )
    .expect_err("a ragged grid must be rejected");
    assert_eq!(error.code.as_str(), "L1019");
    assert!(
        !builtin_registry()
            .get("MeshGrid")
            .unwrap()
            .has(engine.world(), entity),
        "a rejected component.set must not insert the component"
    );
    set(&mut engine, json!({ "legend": legend, "rows": ["a "] }))
        .expect("a valid grid sets cleanly");
}
