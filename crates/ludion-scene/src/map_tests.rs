//! Unit tests for the map format + generator (`map.rs`).

use super::*;
use crate::{emit, parse};
use ludion_ecs::Registry;

// A shared kit exercising every expansion mode: a merged floor (`.`), a per-cell collider wall
// (`#`), a footprint-tiled post (`W`, 4×1), a plain per-cell orb (`O`), and a variant tree (`V`),
// plus a `gate` marker kind. Arrays are block-style — the strict-YAML subset rejects non-empty flow
// collections, so map/map-kit documents always spell arrays one element per line.
const KIT: &str = "\
format: 1
kind: map-kit
name: test-kit
tiles:
  \".\":
    - name: floor
      shape: cube
      color:
        - 0.4
        - 0.4
        - 0.4
        - 1.0
      y: -0.1
      scale:
        - 2.0
        - 0.2
        - 2.0
      merge: true
  \"#\":
    - name: wall
      shape: cube
      y: 1.0
      collider:
        aabb:
          - 1.0
          - 1.0
  W:
    - name: post
      voxel: post.voxel.yaml
      footprint:
        - 4
        - 1
      collider:
        aabb:
          - 4.0
          - 1.0
  O:
    - name: orb
      shape: sphere
      y: 0.5
  V:
    - name: tree
      shape: cube
      variants:
        - shape: sphere
        - shape: cube
      y: 0.0
kinds:
  gate:
    - name: gate
      shape: cube
      color:
        - 1.0
        - 0.0
        - 1.0
        - 1.0
      y: 0.5
";

fn kit() -> MapKit {
    parse_map_kit(KIT).expect("the shared kit parses")
}

/// Parse + generate a map against the shared kit and no fragments.
fn build(map_src: &str) -> Result<Scene, Vec<Diagnostic>> {
    let map = parse_map(map_src).expect("the map parses");
    generate(&map, &kit(), &[], &Registry::new())
}

fn gen_ok(map_src: &str) -> Scene {
    build(map_src).expect("the map generates cleanly")
}

fn names(scene: &Scene) -> Vec<String> {
    scene
        .entities
        .iter()
        .filter_map(|e| e.name.clone())
        .collect()
}

fn entity<'a>(scene: &'a Scene, name: &str) -> &'a SceneEntity {
    scene
        .entities
        .iter()
        .find(|e| e.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no entity named `{name}`"))
}

fn component<'a>(entity: &'a SceneEntity, name: &str) -> &'a Value {
    entity
        .components
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("no `{name}` component"))
}

const FLOOR_3X3: &str = "\
format: 1
kind: map
name: floors
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"...\"
  - \"...\"
  - \"...\"
";

// --- 1. Byte determinism ----------------------------------------------------------------------

#[test]
fn generation_is_byte_deterministic() {
    let registry = Registry::new();
    let a = emit(&gen_ok(FLOOR_3X3), &registry);
    let b = emit(&gen_ok(FLOOR_3X3), &registry);
    assert_eq!(a, b);
}

// --- 2. Diff locality (per-cell props) --------------------------------------------------------

#[test]
fn one_glyph_edit_is_diff_local() {
    // A grid of per-cell walls; flipping one interior cell to a different per-cell glyph must change
    // only that one entity — every other coordinate-named entity stays byte-identical.
    let base = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"###\"
  - \"###\"
  - \"###\"
";
    let edited = base.replace("  - \"###\"\n  - \"###\"", "  - \"###\"\n  - \"#O#\"");
    let a = gen_ok(base);
    let b = gen_ok(&edited);

    // The changed cell: `wall-r001-c001` gone, `orb-r001-c001` gained; nothing else moves.
    let a_names: std::collections::BTreeSet<_> = names(&a).into_iter().collect();
    let b_names: std::collections::BTreeSet<_> = names(&b).into_iter().collect();
    let only_a: Vec<_> = a_names.difference(&b_names).cloned().collect();
    let only_b: Vec<_> = b_names.difference(&a_names).cloned().collect();
    assert_eq!(only_a, vec!["wall-r001-c001".to_string()]);
    assert_eq!(only_b, vec!["orb-r001-c001".to_string()]);

    // Every shared-name entity is byte-identical between the two generations.
    for name in a_names.intersection(&b_names) {
        assert_eq!(
            entity(&a, name),
            entity(&b, name),
            "entity `{name}` drifted"
        );
    }
}

// --- 3. Rect merging --------------------------------------------------------------------------

#[test]
fn solid_floor_merges_to_one_rect() {
    let scene = gen_ok(FLOOR_3X3);
    let floors: Vec<_> = names(&scene)
        .into_iter()
        .filter(|n| n.starts_with("floor-"))
        .collect();
    assert_eq!(floors, vec!["floor-r000-c000-w03-h03".to_string()]);
    // The merged slab is scaled to cover 3×3 cells of 2.0 units: 6 × 6, thickness kept at 0.2.
    let t = component(entity(&scene, "floor-r000-c000-w03-h03"), "Transform");
    // Values are the nearest f32 widened (`0.2`, `-0.1` are not exact in binary).
    let (thin, down) = (f64::from(0.2_f32), f64::from(-0.1_f32));
    assert_eq!(t["scale"], serde_json::json!([6.0, thin, 6.0]));
    assert_eq!(t["translation"], serde_json::json!([3.0, down, 3.0]));
}

#[test]
fn a_floor_with_a_hole_splits_into_rects() {
    // A `#` in the center leaves a ring of `.`; greedy row-major merge yields four rects.
    let src = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"...\"
  - \".#.\"
  - \"...\"
";
    let scene = gen_ok(src);
    let mut floors: Vec<_> = names(&scene)
        .into_iter()
        .filter(|n| n.starts_with("floor-"))
        .collect();
    floors.sort();
    assert_eq!(
        floors,
        vec![
            "floor-r000-c000-w03-h01".to_string(),
            "floor-r001-c000-w01-h02".to_string(),
            "floor-r001-c002-w01-h02".to_string(),
            "floor-r002-c001-w01-h01".to_string(),
        ]
    );
}

// --- 4. Footprint tiling ----------------------------------------------------------------------

#[test]
fn footprint_run_tiles_into_pieces() {
    let src = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - WWWWWWWW
";
    let scene = gen_ok(src);
    let mut posts: Vec<_> = names(&scene)
        .into_iter()
        .filter(|n| n.starts_with("post-"))
        .collect();
    posts.sort();
    assert_eq!(
        posts,
        vec!["post-r000-c000".to_string(), "post-r000-c004".to_string()]
    );
}

#[test]
fn footprint_run_that_does_not_divide_is_l1026() {
    let src = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - WWWWWW
";
    let diagnostics = build(src).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_FOOTPRINT_MISFIT)
        .expect("a footprint-misfit diagnostic");
    assert_eq!(d.path.as_deref(), Some("/rows/0"));
}

// --- 5. Variant stability ---------------------------------------------------------------------

#[test]
fn editing_a_far_cell_leaves_a_cells_variant_unchanged() {
    let base = "\
format: 1
kind: map
seed: 1
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - VVV
  - VVV
  - VVV
";
    // Change a far corner cell (0,0) to void; cell (2,2)'s seeded variant must not move.
    let edited = base.replace("  - VVV\n  - VVV\n  - VVV", "  - \" VV\"\n  - VVV\n  - VVV");
    let a = gen_ok(base);
    let b = gen_ok(&edited);
    let mesh_a = component(entity(&a, "tree-r002-c002"), "Mesh");
    let mesh_b = component(entity(&b, "tree-r002-c002"), "Mesh");
    assert_eq!(mesh_a, mesh_b);
    // And the far cell really did change (its entity is gone in `b`).
    assert!(!names(&b).contains(&"tree-r000-c000".to_string()));
}

#[test]
fn variant_selection_depends_on_seed_and_coords_only() {
    // Two distinct cells with the same glyph draw independently — at least one differs from a third,
    // proving the pick is not a constant. (A weak but sufficient non-triviality check.)
    let src = "\
format: 1
kind: map
seed: 7
kit: kit.yaml
cell_size: 1.0
origin:
  - 0.0
  - 0.0
rows:
  - VVVVVVVV
";
    let scene = gen_ok(src);
    let shapes: std::collections::BTreeSet<String> = (0..8)
        .map(|c| {
            let name = format!("tree-r000-c{c:03}");
            component(entity(&scene, &name), "Mesh")["shape"].to_string()
        })
        .collect();
    assert!(shapes.len() > 1, "all eight cells picked the same variant");
}

// --- 6. Validation codes ----------------------------------------------------------------------

#[test]
fn ragged_rows_is_l1019() {
    let src = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"...\"
  - \"..\"
";
    let diagnostics = build(src).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::TILEMAP_RAGGED_ROWS)
        .expect("a ragged-rows diagnostic");
    assert_eq!(d.path.as_deref(), Some("/rows/1"));
}

#[test]
fn unknown_glyph_is_l1020() {
    let src = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \".X.\"
";
    let diagnostics = build(src).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::TILEMAP_INVALID_GLYPH)
        .expect("an unknown-glyph diagnostic");
    assert_eq!(d.path.as_deref(), Some("/rows/0"));
    assert!(d.message.contains('X'));
}

#[test]
fn marker_out_of_bounds_is_l1023() {
    let src = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"..\"
  - \"..\"
markers:
  - kind: gate
    x: 99.0
    z: 0.0
";
    let diagnostics = build(src).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_MARKER_OUT_OF_BOUNDS)
        .expect("an out-of-bounds diagnostic");
    assert_eq!(d.path.as_deref(), Some("/markers/0"));
}

#[test]
fn unknown_marker_kind_is_l1024_with_suggestion() {
    let src = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"..\"
  - \"..\"
markers:
  - kind: gat
    x: 1.0
    z: 1.0
";
    let diagnostics = build(src).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_UNKNOWN_KIND)
        .expect("an unknown-kind diagnostic");
    assert_eq!(d.path.as_deref(), Some("/markers/0/kind"));
    assert_eq!(d.help.as_deref(), Some("nearest match is `gate`"));
}

#[test]
fn empty_kit_primitive_list_is_l1022() {
    let bad_kit_src = "\
format: 1
kind: map-kit
tiles:
  \".\":
    - shape: cube
  Q: []
";
    let bad_kit = parse_map_kit(bad_kit_src).expect("the kit parses structurally");
    let map = parse_map(FLOOR_3X3).expect("map parses");
    let diagnostics = generate(&map, &bad_kit, &[], &Registry::new()).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_KIT_INVALID)
        .expect("a kit-invalid diagnostic");
    assert_eq!(d.path.as_deref(), Some("/tiles/Q"));
}

#[test]
fn merge_and_footprint_clash_is_l1022() {
    let bad_kit_src = "\
format: 1
kind: map-kit
tiles:
  \".\":
    - shape: cube
      merge: true
      footprint:
        - 2
        - 1
";
    let bad_kit = parse_map_kit(bad_kit_src).expect("the kit parses structurally");
    let map = parse_map(FLOOR_3X3).expect("map parses");
    let diagnostics = generate(&map, &bad_kit, &[], &Registry::new()).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code::MAP_KIT_INVALID && d.path.as_deref() == Some("/tiles/./0"))
    );
}

#[test]
fn a_map_number_that_overflows_f32_is_l1021() {
    let origin = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 1e39
  - 0.0
rows: []
";
    let diagnostics = parse_map(origin).unwrap_err();
    assert!(
        diagnostics.iter().any(|d| d.code == code::MAP_DOCUMENT_STRUCTURE
            && d.path.as_deref() == Some("/origin"))
    );

    let rotation = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows: []
markers:
  - kind: gate
    x: 2.0
    z: 2.0
    rotation: -1e39
";
    let diagnostics = parse_map(rotation).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code::MAP_DOCUMENT_STRUCTURE
                && d.path.as_deref() == Some("/markers/0/rotation"))
    );
}

#[test]
fn a_kit_color_or_collider_that_overflows_f32_is_l1022() {
    for (field, body, pointer) in [
        (
            "color",
            "      color:\n        - 1e39\n        - 0.0\n        - 0.0\n        - 1.0\n",
            "/tiles/./0/color",
        ),
        (
            "aabb",
            "      collider:\n        aabb:\n          - 1e39\n          - 1.0\n",
            "/tiles/./0/collider",
        ),
        (
            "circle",
            "      collider:\n        circle: 1e39\n",
            "/tiles/./0/collider",
        ),
    ] {
        let kit_src =
            format!("format: 1\nkind: map-kit\ntiles:\n  \".\":\n    - shape: cube\n{body}");
        let bad_kit = parse_map_kit(&kit_src).expect("the kit parses structurally");
        let map = parse_map(FLOOR_3X3).expect("map parses");
        let diagnostics = generate(&map, &bad_kit, &[], &Registry::new()).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == code::MAP_KIT_INVALID && d.path.as_deref() == Some(pointer)),
            "{field}: {diagnostics:#?}"
        );
    }
}

#[test]
fn name_collision_across_fragments_is_l1025() {
    let dup = SceneEntity {
        presentation: false,
        name: Some("twin".to_string()),
        components: vec![],
        instance: None,
    };
    let fragment = Scene {
        entities: vec![dup.clone(), dup],
        ..Scene::default()
    };
    let map = parse_map(FLOOR_3X3).expect("map parses");
    let diagnostics = generate(
        &map,
        &kit(),
        &[("frag.yaml".to_string(), fragment)],
        &Registry::new(),
    )
    .unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_NAME_COLLISION)
        .expect("a name-collision diagnostic");
    assert!(d.message.contains("twin"));
}

#[test]
fn malformed_map_structure_is_l1021() {
    // `cell_size` a string, a bogus top-level key, and a missing `kit`.
    let src = "\
format: 1
kind: map
cell_size: big
origin:
  - 0.0
  - 0.0
rows: []
celll_size: 2.0
";
    let diagnostics = parse_map(src).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .all(|d| d.code == code::MAP_DOCUMENT_STRUCTURE)
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.path.as_deref() == Some("/cell_size"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.help.as_deref() == Some("nearest match is `cell_size`"))
    );
}

#[test]
fn bad_kind_is_l1021() {
    let diagnostics =
        parse_map("format: 1\nkind: mapp\nkit: k\ncell_size: 1\norigin:\n  - 0\n  - 0\nrows: []\n")
            .unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/kind"))
        .expect("a kind diagnostic");
    assert_eq!(d.code, code::MAP_DOCUMENT_STRUCTURE);
}

// --- 7. generated_from ------------------------------------------------------------------------

#[test]
fn generate_sets_generated_from_from_the_source() {
    let mut map = parse_map(FLOOR_3X3).expect("map parses");
    map.source = Some("floors.map.yaml".to_string());
    let scene = generate(&map, &kit(), &[], &Registry::new()).expect("generates");
    assert_eq!(scene.generated_from.as_deref(), Some("floors.map.yaml"));

    // And it round-trips through the canonical writer.
    let text = emit(&scene, &Registry::new());
    assert!(text.contains("generated_from: floors.map.yaml\n"));
    let reparsed = parse(&text).expect("re-parses");
    assert_eq!(reparsed.generated_from.as_deref(), Some("floors.map.yaml"));
}

// --- 8. Fragment merge ------------------------------------------------------------------------

#[test]
fn fragments_are_emitted_first_and_verbatim_in_order() {
    let frag_a = Scene {
        name: Some("frag-a".to_string()),
        entities: vec![SceneEntity {
            presentation: false,
            name: Some("camera".to_string()),
            components: vec![("Camera".to_string(), serde_json::json!({}))],
            instance: None,
        }],
        ..Scene::default()
    };
    let frag_b = Scene {
        entities: vec![SceneEntity {
            presentation: false,
            name: Some("hud".to_string()),
            components: vec![],
            instance: None,
        }],
        ..Scene::default()
    };
    let map = parse_map(FLOOR_3X3).expect("map parses");
    let scene = generate(
        &map,
        &kit(),
        &[
            ("a.yaml".to_string(), frag_a),
            ("b.yaml".to_string(), frag_b),
        ],
        &Registry::new(),
    )
    .expect("generates");

    // Fragment entities come first, in fragment-then-entity order, before any grid entity.
    assert_eq!(scene.entities[0].name.as_deref(), Some("camera"));
    assert_eq!(scene.entities[1].name.as_deref(), Some("hud"));
    assert!(
        scene.entities[2]
            .name
            .as_deref()
            .unwrap()
            .starts_with("floor-")
    );
    // Verbatim: the camera entity's components are untouched.
    assert_eq!(
        component(entity(&scene, "camera"), "Camera"),
        &serde_json::json!({})
    );
}

// --- Marker passthrough -----------------------------------------------------------------------

#[test]
fn marker_kind_plus_components_merge_onto_one_entity() {
    let src = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"..\"
  - \"..\"
markers:
  - name: gate-to-market
    kind: gate
    x: 2.0
    z: 2.0
    rotation: 90.0
    components:
      Gate:
        to: district-market.yaml
        anchor: from-plaza
";
    let scene = gen_ok(src);
    let gate = entity(&scene, "gate-to-market");
    // One entity carries the kit mesh AND the passthrough Gate, plus the rotated Transform.
    let component_names: Vec<_> = gate.components.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(component_names, vec!["Transform", "Mesh", "Gate"]);
    assert_eq!(
        component(gate, "Transform")["rotation"],
        serde_json::json!([0.0, 90.0, 0.0])
    );
    assert_eq!(
        component(gate, "Gate")["anchor"],
        serde_json::json!("from-plaza")
    );
}

// --- base underlay ----------------------------------------------------------------------------

#[test]
fn base_glyph_fills_floor_under_every_non_void_cell() {
    // `base: .` puts the floor under the walls too, so the `#` cells need not redeclare it.
    let src = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
base: \".\"
rows:
  - \"##\"
  - \"##\"
";
    let scene = gen_ok(src);
    // One merged floor rect covers the whole 2×2 non-void region; four walls sit on top.
    let floors: Vec<_> = names(&scene)
        .into_iter()
        .filter(|n| n.starts_with("floor-"))
        .collect();
    assert_eq!(floors, vec!["floor-r000-c000-w02-h02".to_string()]);
    let walls = names(&scene)
        .iter()
        .filter(|n| n.starts_with("wall-"))
        .count();
    assert_eq!(walls, 4);
}

/// The pure marker table resolves each marker to its world position, its kind, and the grid cell it
/// falls in — the coordinate arithmetic `map-check` renders, asserted with no CLI, registry, or GPU.
#[test]
fn marker_table_resolves_world_and_cell() {
    let src = "\
format: 1
kind: map
kit: k
cell_size: 2
origin:
  - -4.0
  - 10.0
rows:
  - \"....\"
  - \"....\"
  - \"....\"
markers:
  - name: well
    kind: well
    x: -3.0
    z: 11.0
  - kind: torch
    x: 3.0
    z: 14.5
  - x: 100.0
    z: 100.0
";
    let map = parse_map(src).expect("the map parses");
    let grid = MapGrid::new(&map);
    assert_eq!((grid.width(), grid.height()), (4, 3));

    let table = grid.marker_table(&map);
    // Named marker: cell = ((x-ox)/cs, (z-oz)/cs) floored = ((-3+4)/2, (11-10)/2) = (col 0, row 0).
    assert_eq!(table[0].name, "well");
    assert_eq!(table[0].kind.as_deref(), Some("well"));
    assert_eq!(table[0].world, [-3.0, 11.0]);
    assert_eq!(table[0].cell, Some((0, 0)));
    // Unnamed marker derives `<kind>-<index>`; cell = ((3+4)/2, (14.5-10)/2) = (col 3, row 2).
    assert_eq!(table[1].name, "torch-1");
    assert_eq!(table[1].kind.as_deref(), Some("torch"));
    assert_eq!(table[1].cell, Some((2, 3)));
    // Off-grid marker: no cell, and a `marker-<index>` name (no kind).
    assert_eq!(table[2].name, "marker-2");
    assert_eq!(table[2].kind, None);
    assert_eq!(table[2].cell, None);
}

/// The kit's optional `probe:` block carries the walker `map-check` measures with — the content's own
/// radius and speed rather than a CLI default. It is read by the checker only, never by `generate`.
#[test]
fn kit_probe_parses_radius_and_speed() {
    let src = "\
format: 1
kind: map-kit
tiles:
  \".\":
    - shape: cube
probe:
  radius: 0.9
  speed: 6.0
";
    let kit = parse_map_kit(src).expect("the kit parses");
    assert_eq!(kit.probe.radius, Some(0.9));
    assert_eq!(kit.probe.speed, Some(6.0));

    // Absent by default, and either field may stand alone.
    let bare = parse_map_kit(
        "\
format: 1
kind: map-kit
tiles:
  \".\":
    - shape: cube
",
    )
    .expect("the kit parses");
    assert_eq!(bare.probe, MapProbe::default());
}

#[test]
fn non_positive_kit_probe_is_l1022() {
    let src = "\
format: 1
kind: map-kit
tiles:
  \".\":
    - shape: cube
probe:
  radius: 0
  speed: -6.0
";
    let diagnostics = parse_map_kit(src).expect_err("a non-positive probe is rejected");
    let paths: Vec<&str> = diagnostics
        .iter()
        .filter(|d| d.code == code::MAP_KIT_INVALID)
        .filter_map(|d| d.path.as_deref())
        .collect();
    assert_eq!(paths, ["/probe/radius", "/probe/speed"]);
}

#[test]
fn unknown_kit_probe_key_is_l1021() {
    let src = "\
format: 1
kind: map-kit
tiles:
  \".\":
    - shape: cube
probe:
  radius: 0.9
  stride: 2
";
    let diagnostics = parse_map_kit(src).expect_err("an unknown probe key is rejected");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code::MAP_DOCUMENT_STRUCTURE
                && d.path.as_deref() == Some("/probe/stride"))
    );

    // `gate_radius` once existed; an old kit that still carries it fails loudly.
    let old = src.replace("stride: 2", "gate_radius: 2.5");
    let diagnostics = parse_map_kit(&old).expect_err("the retired key is rejected");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code::MAP_DOCUMENT_STRUCTURE
                && d.path.as_deref() == Some("/probe/gate_radius"))
    );
}

// --- Grid-less maps (`bounds:`) ---------------------------------------------------------------

const BOUNDS_HEAD: &str = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
";

#[test]
fn markers_on_an_empty_grid_are_l1023_at_rows() {
    let src = format!(
        "{BOUNDS_HEAD}rows: []\nmarkers:\n  - x: 1.0\n    z: 1.0\n  - x: 2.0\n    z: 2.0\n"
    );
    let diagnostics = build(&src).unwrap_err();
    let hits: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.code == code::MAP_MARKER_OUT_OF_BOUNDS)
        .collect();
    assert_eq!(hits.len(), 1, "reported once, not per marker");
    assert_eq!(hits[0].path.as_deref(), Some("/rows"));
    assert!(hits[0].message.contains("frame"));
    assert!(!hits[0].message.contains("  "), "{}", hits[0].message);
    assert!(hits[0].help.as_deref().unwrap().contains("bounds"));
}

#[test]
fn an_empty_grid_without_markers_still_generates() {
    let scene = gen_ok(&format!("{BOUNDS_HEAD}rows: []\n"));
    assert!(scene.entities.is_empty());
}

#[test]
fn bounds_frames_a_marker_only_map() {
    let src = format!("{BOUNDS_HEAD}bounds:\n  - 4\n  - 3\nmarkers:\n  - x: 3.0\n    z: 5.0\n");
    let map = parse_map(&src).expect("the map parses");
    let grid = MapGrid::new(&map);
    assert_eq!((grid.width(), grid.height()), (4, 3));
    let scene = generate(&map, &kit(), &[], &Registry::new()).expect("generates");
    assert_eq!(scene.entities.len(), 1);
    let marker = &scene.entities[0];
    assert_eq!(
        marker.name.as_deref(),
        Some(map.markers[0].derived_name(0).as_str())
    );
    assert_eq!(
        component(marker, "Transform")["translation"],
        serde_json::json!([3.0, 0.0, 5.0])
    );
}

#[test]
fn bounds_does_not_emit_a_base_underlay_or_grid_entities() {
    let src = format!("{BOUNDS_HEAD}bounds:\n  - 4\n  - 3\n");
    assert!(gen_ok(&src).entities.is_empty());
}

#[test]
fn a_marker_outside_bounds_is_l1023() {
    let src = format!("{BOUNDS_HEAD}bounds:\n  - 4\n  - 3\nmarkers:\n  - x: 9.0\n    z: 1.0\n");
    let diagnostics = build(&src).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_MARKER_OUT_OF_BOUNDS)
        .expect("an out-of-bounds diagnostic");
    assert_eq!(d.path.as_deref(), Some("/markers/0"));
}

#[test]
fn rows_and_bounds_together_are_l1021_at_bounds() {
    let src = format!("{BOUNDS_HEAD}rows:\n  - \"..\"\nbounds:\n  - 4\n  - 3\n");
    let diagnostics = parse_map(&src).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code::MAP_DOCUMENT_STRUCTURE
                && d.path.as_deref() == Some("/bounds"))
    );
}

#[test]
fn base_with_bounds_is_l1021_at_base() {
    let src = format!("{BOUNDS_HEAD}base: \".\"\nbounds:\n  - 4\n  - 3\n");
    let diagnostics = parse_map(&src).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code::MAP_DOCUMENT_STRUCTURE && d.path.as_deref() == Some("/base"))
    );
}

#[test]
fn neither_rows_nor_bounds_is_a_missing_key_naming_both() {
    let diagnostics = parse_map(BOUNDS_HEAD).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.message.contains("missing"))
        .expect("a missing-key diagnostic");
    let help = d.help.as_deref().unwrap();
    assert!(help.contains("rows") && help.contains("bounds"));
}

#[test]
fn zero_bounds_is_l1021() {
    let src = format!("{BOUNDS_HEAD}bounds:\n  - 0\n  - 3\n");
    let diagnostics = parse_map(&src).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.path.as_deref() == Some("/bounds"))
    );
}

#[test]
fn a_bounds_value_above_u32_is_refused_not_clamped() {
    let src = format!("{BOUNDS_HEAD}bounds:\n  - 5000000000\n  - 1\n");
    let diagnostics = parse_map(&src).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code::MAP_DOCUMENT_STRUCTURE
                && d.path.as_deref() == Some("/bounds/0"))
    );
}

#[test]
fn bounds_above_the_extent_cap_are_l1021_and_the_cap_itself_parses() {
    let src = format!("{BOUNDS_HEAD}bounds:\n  - 1025\n  - 4\n");
    let diagnostics = parse_map(&src).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code::MAP_DOCUMENT_STRUCTURE
                && d.path.as_deref() == Some("/bounds"))
    );
    let src = format!("{BOUNDS_HEAD}bounds:\n  - 1024\n  - 1024\n");
    parse_map(&src).expect("the cap itself is allowed");
}

// --- Merged colliders and the reserved void glyph ---------------------------------------------

/// A kit whose single tile `T` is `merge: true` with the given collider body (block-style YAML).
fn merged_kit(collider: &str) -> MapKit {
    let src = format!(
        "format: 1\nkind: map-kit\ntiles:\n  T:\n    - shape: cube\n      merge: true\n      collider:\n{collider}"
    );
    parse_map_kit(&src).expect("the kit parses")
}

const TWO_BY_TWO_T: &str = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"TT\"
  - \"TT\"
";

fn kit_errors(kit: &MapKit, map_src: &str) -> Vec<Diagnostic> {
    let map = parse_map(map_src).expect("the map parses");
    generate(&map, kit, &[], &Registry::new()).unwrap_err()
}

#[test]
fn a_merged_circle_collider_is_l1022() {
    let kit = merged_kit("        circle: 1.0\n");
    let diagnostics = kit_errors(&kit, TWO_BY_TWO_T);
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_KIT_INVALID)
        .expect("a kit diagnostic");
    assert_eq!(d.path.as_deref(), Some("/tiles/T/0/collider"));
    assert!(d.message.contains("circle"));
    let help = d.help.as_deref().expect("a fix");
    assert!(!help.contains("  "), "{help}");
}

#[test]
fn a_merged_aabb_that_is_not_one_cell_is_l1022_naming_the_expected_size() {
    let kit = merged_kit("        aabb:\n          - 0.25\n          - 0.25\n");
    let diagnostics = kit_errors(&kit, TWO_BY_TWO_T);
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_KIT_INVALID)
        .expect("a kit diagnostic");
    assert_eq!(d.path.as_deref(), Some("/tiles/T/0/collider"));
    assert!(d.message.contains("[1, 1]"), "{}", d.message);
    assert!(d.message.contains("[0.25, 0.25]"), "{}", d.message);
}

#[test]
fn the_base_glyphs_primitives_are_merged_so_an_off_size_aabb_is_l1022() {
    let kit = parse_map_kit(
        "format: 1\nkind: map-kit\ntiles:\n  B:\n    - shape: cube\n      collider:\n        aabb:\n          - 0.5\n          - 0.5\n",
    )
    .expect("the kit parses");
    let src = TWO_BY_TWO_T.replace("rows:", "base: B\nrows:");
    let diagnostics = kit_errors(&kit, &src.replace("TT", ".."));
    assert!(diagnostics.iter().any(
        |d| d.code == code::MAP_KIT_INVALID && d.path.as_deref() == Some("/tiles/B/0/collider")
    ));
}

#[test]
fn a_cell_sized_merged_aabb_grows_to_the_rect() {
    let kit = merged_kit("        aabb:\n          - 1.0\n          - 1.0\n");
    let map = parse_map(TWO_BY_TWO_T).expect("the map parses");
    let scene = generate(&map, &kit, &[], &Registry::new()).expect("generates");
    // One 2×2-cell rect of 2.0-unit cells: 4 × 4 units, half-extents 2 × 2.
    let collider = component(&scene.entities[0], "Collider");
    assert_eq!(
        collider["shape"]["aabb"]["half_extents"],
        serde_json::json!([2.0, 2.0])
    );
}

#[test]
fn a_kit_box_collider_parses_and_emits_a_static_box_on_a_unit_scale_sibling() {
    let kit = parse_map_kit(
        "format: 1\nkind: map-kit\ntiles:\n  \"#\":\n    - shape: cube\n      scale:\n        - 2.0\n        - 2.0\n        - 2.0\n      collider:\n        box:\n          - 1.0\n          - 1.0\n          - 1.0\n",
    )
    .expect("the kit parses");
    let map = parse_map(&TWO_BY_TWO_T.replace("  - \"TT\"\n  - \"TT\"\n", "  - \"#\"\n"))
        .expect("the map parses");
    let scene = generate(&map, &kit, &[], &Registry::new()).expect("generates");
    assert_eq!(scene.entities.len(), 2);
    let (mesh, sibling) = (&scene.entities[0], &scene.entities[1]);
    assert!(mesh.components.iter().all(|(n, _)| n != "Collider"));
    assert_eq!(
        component(mesh, "Transform")["scale"],
        serde_json::json!([2.0, 2.0, 2.0])
    );
    assert_eq!(
        sibling.name.as_deref(),
        Some(format!("{}-collider", mesh.name.as_deref().unwrap()).as_str())
    );
    assert_eq!(translation(sibling), translation(mesh));
    assert_eq!(
        component(sibling, "Transform")["scale"],
        serde_json::json!([1.0, 1.0, 1.0])
    );
    assert!(sibling.components.iter().all(|(n, _)| n != "Mesh"));
    let collider = component(sibling, "Collider");
    assert_eq!(
        collider["shape"]["box"]["half_extents"],
        serde_json::json!([1.0, 1.0, 1.0])
    );
    assert_eq!(collider["body"], "static");
}

#[test]
fn a_cell_sized_merged_box_grows_in_x_z_and_keeps_y() {
    let kit = merged_kit("        box:\n          - 1.0\n          - 0.25\n          - 1.0\n");
    let map = parse_map(TWO_BY_TWO_T).expect("the map parses");
    let scene = generate(&map, &kit, &[], &Registry::new()).expect("generates");
    let collider = component(&scene.entities[1], "Collider");
    assert_eq!(
        collider["shape"]["box"]["half_extents"],
        serde_json::json!([2.0, 0.25, 2.0])
    );
}

#[test]
fn a_merged_box_that_is_not_one_cell_in_x_z_is_l1022() {
    let kit = merged_kit("        box:\n          - 0.25\n          - 1.0\n          - 1.0\n");
    let diagnostics = kit_errors(&kit, TWO_BY_TWO_T);
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_KIT_INVALID)
        .expect("a kit diagnostic");
    assert_eq!(d.path.as_deref(), Some("/tiles/T/0/collider"));
    assert!(d.message.contains("[1, y, 1]"), "{}", d.message);
    assert!(d.message.contains("[0.25, y, 1]"), "{}", d.message);
    assert!(d.help.as_deref().unwrap().contains("box: [1, hy, 1]"));
}

#[test]
fn a_non_finite_box_is_l1022() {
    let kit_src = "format: 1\nkind: map-kit\ntiles:\n  \".\":\n    - shape: cube\n      collider:\n        box:\n          - 1e39\n          - 1.0\n          - 1.0\n";
    let bad_kit = parse_map_kit(kit_src).expect("the kit parses structurally");
    let map = parse_map(FLOOR_3X3).expect("map parses");
    let diagnostics = generate(&map, &bad_kit, &[], &Registry::new()).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code::MAP_KIT_INVALID
                && d.path.as_deref() == Some("/tiles/./0/collider")),
        "{diagnostics:#?}"
    );
}

#[test]
fn a_box_needs_three_half_extents_is_l1021() {
    let kit_src = "format: 1\nkind: map-kit\ntiles:\n  \".\":\n    - shape: cube\n      collider:\n        box:\n          - 1.0\n          - 1.0\n";
    let diagnostics = parse_map_kit(kit_src).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code::MAP_DOCUMENT_STRUCTURE
                && d.path.as_deref() == Some("/tiles/./0/collider/box")),
        "{diagnostics:#?}"
    );
}

#[test]
fn a_mirrored_box_sibling_is_reflected_with_unit_scale() {
    let kit = parse_map_kit(
        "format: 1\nkind: map-kit\ntiles:\n  \"#\":\n    - shape: cube\n      scale:\n        - 2.0\n        - 2.0\n        - 2.0\n      collider:\n        box:\n          - 1.0\n          - 1.0\n          - 1.0\n",
    )
    .expect("the kit parses");
    let src = "format: 1\nkind: map\nkit: kit.yaml\ncell_size: 2.0\norigin:\n  - 0.0\n  - 0.0\nrows:\n  - \"#   \"\n";
    let generate_with = |mirror| {
        let mut map = parse_map(src).unwrap();
        map.mirror = mirror;
        generate(&map, &kit, &[], &Registry::new()).expect("generates")
    };
    let (plain, flipped) = (generate_with(Mirror::None), generate_with(Mirror::X));
    let (a, b) = (&plain.entities[1], &flipped.entities[1]);
    assert_eq!(translation(b)[0], 8.0 - translation(a)[0]);
    assert_eq!(
        component(b, "Transform")["scale"],
        serde_json::json!([1.0, 1.0, 1.0])
    );
}

#[test]
fn a_space_tile_key_is_l1020() {
    let kit = parse_map_kit("format: 1\nkind: map-kit\ntiles:\n  \" \":\n    - shape: cube\n")
        .expect("the kit parses");
    let diagnostics = kit_errors(&kit, TWO_BY_TWO_T.replace("TT", "  ").as_str());
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::TILEMAP_INVALID_GLYPH)
        .expect("an invalid-glyph diagnostic");
    assert_eq!(d.path.as_deref(), Some("/tiles/ "));
    assert!(d.message.contains("void"));
}

// --- Map physics, dropped fragment keys, f32 coordinates, collision origins -------------------

#[test]
fn a_map_physics_block_reaches_the_generated_scene() {
    let src = format!("{BOUNDS_HEAD}bounds:\n  - 2\n  - 2\nphysics:\n  space: free\n");
    let map = parse_map(&src).expect("the map parses");
    assert!(map.physics.is_some());
    let scene = generate(&map, &kit(), &[], &Registry::new()).expect("generates");
    assert_eq!(scene.physics, map.physics);
    assert!(scene.physics.is_some());
}

#[test]
fn a_fragment_with_physics_or_seed_is_l1021_at_the_fragment() {
    let map = parse_map(FLOOR_3X3).expect("map parses");
    let with_physics =
        parse("format: 1\nphysics:\n  space: free\nentities: []\n").expect("the fragment parses");
    let with_seed = Scene {
        seed: Some(3),
        ..Scene::default()
    };
    for (fragment, key) in [(with_physics, "physics"), (with_seed, "seed")] {
        let diagnostics = generate(
            &map,
            &kit(),
            &[("frag.yaml".to_string(), fragment)],
            &Registry::new(),
        )
        .unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.code == code::MAP_DOCUMENT_STRUCTURE)
            .unwrap_or_else(|| panic!("a diagnostic for `{key}`"));
        assert_eq!(d.path.as_deref(), Some("/fragments/0"));
        assert!(
            d.message.contains("frag.yaml") && d.message.contains(key),
            "{}",
            d.message
        );
    }
}

/// A stand-in for the engine `Transform`: three f32 arrays, so the writer is schema-aware.
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
struct TestTransform {
    translation: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
}

#[test]
fn emitted_coordinates_are_f32_exact_and_spelled_short() {
    let src = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 0.7
origin:
  - 0.1
  - 0.0
rows:
  - \".O.\"
";
    let scene = gen_ok(src);
    let mut registry = Registry::new();
    registry
        .register::<TestTransform>("Transform")
        .expect("registers");
    let text = emit(&scene, &registry);
    // Col 1, row 0: x = 0.1 + 1.5 * 0.7 = 1.15, z = 0.5 * 0.7 = 0.35 (hand-derived).
    assert!(text.contains("- 1.15\n"), "{text}");
    assert!(text.contains("- 0.35\n"), "{text}");
    assert!(
        !text.contains("1.149999") && !text.contains("0.3499999"),
        "{text}"
    );
    for e in &scene.entities {
        for v in component(e, "Transform")["translation"].as_array().unwrap() {
            let v = v.as_f64().unwrap();
            assert_eq!(f64::from(v as f32), v);
        }
    }
}

#[test]
fn a_name_collision_names_both_origins() {
    let clash = SceneEntity {
        presentation: false,
        name: Some("marker-0".to_string()),
        components: vec![],
        instance: None,
    };
    let fragment = Scene {
        entities: vec![clash],
        ..Scene::default()
    };
    let src = format!("{BOUNDS_HEAD}bounds:\n  - 4\n  - 3\nmarkers:\n  - x: 1.0\n    z: 1.0\n");
    let map = parse_map(&src).expect("map parses");
    let diagnostics = generate(
        &map,
        &kit(),
        &[("frag.yaml".to_string(), fragment)],
        &Registry::new(),
    )
    .unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_NAME_COLLISION)
        .expect("a name-collision diagnostic");
    assert_eq!(d.path.as_deref(), Some("/markers/0"));
    assert!(
        d.message.contains("entity 0 of `frag.yaml`"),
        "{}",
        d.message
    );
    assert!(!d.message.contains("  "), "{}", d.message);
    assert!(d.message.contains("/fragments/0"), "{}", d.message);
    assert!(d.message.contains("/markers/0"), "{}", d.message);
}

#[test]
fn a_grid_collision_names_the_glyph_primitive_and_cell() {
    let kit = parse_map_kit(
        "format: 1\nkind: map-kit\ntiles:\n  A:\n    - name: dup\n      shape: cube\n  B:\n    - name: dup\n      shape: cube\n",
    )
    .expect("the kit parses");
    let src = BOUNDS_HEAD.to_string() + "rows:\n  - \"A\"\n  - \"B\"\n";
    let map = parse_map(&src).expect("map parses");
    // The two glyphs sit on distinct rows, so collide the first cell's name with a fragment.
    let fragment = Scene {
        entities: vec![SceneEntity {
            presentation: false,
            name: Some("dup-r000-c000".to_string()),
            components: vec![],
            instance: None,
        }],
        ..Scene::default()
    };
    let diagnostics = generate(
        &map,
        &kit,
        &[("f.yaml".to_string(), fragment)],
        &Registry::new(),
    )
    .unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_NAME_COLLISION)
        .expect("a name-collision diagnostic");
    assert_eq!(d.path.as_deref(), Some("/rows/0"));
    assert!(
        d.message.contains("glyph `A` primitive 0 at row 0, col 0"),
        "{}",
        d.message
    );
}

fn parent_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register::<ludion_ecs::Parent>("Parent").unwrap();
    registry
}

fn plain(name: &str) -> SceneEntity {
    SceneEntity {
        presentation: false,
        name: Some(name.to_string()),
        components: vec![],
        instance: None,
    }
}

fn child_of(name: &str, index: u64) -> SceneEntity {
    SceneEntity {
        components: vec![(
            "Parent".to_string(),
            serde_json::json!({"index": index, "generation": 0}),
        )],
        ..plain(name)
    }
}

fn parent_index(scene: &Scene, name: &str) -> u64 {
    component(entity(scene, name), "Parent")["index"]
        .as_u64()
        .unwrap()
}

#[test]
fn a_fragments_parent_index_is_rebased_past_earlier_fragments() {
    let first = Scene {
        entities: vec![plain("a0"), child_of("a1", 0)],
        ..Scene::default()
    };
    let second = Scene {
        entities: vec![plain("b0"), child_of("b1", 0)],
        ..Scene::default()
    };
    let map = parse_map(FLOOR_3X3).expect("map parses");
    let scene = generate(
        &map,
        &kit(),
        &[
            ("a.yaml".to_string(), first),
            ("b.yaml".to_string(), second),
        ],
        &parent_registry(),
    )
    .expect("generates");
    assert_eq!(
        parent_index(&scene, "a1"),
        0,
        "the first fragment is unshifted"
    );
    assert_eq!(
        parent_index(&scene, "b1"),
        2,
        "two entities precede the second fragment"
    );
}

#[test]
fn a_fragment_parent_outside_the_fragment_is_l1017() {
    let first = Scene {
        entities: vec![plain("a0")],
        ..Scene::default()
    };
    let second = Scene {
        entities: vec![plain("b0"), child_of("b1", 2)],
        ..Scene::default()
    };
    let map = parse_map(FLOOR_3X3).expect("map parses");
    let diagnostics = generate(
        &map,
        &kit(),
        &[
            ("a.yaml".to_string(), first),
            ("b.yaml".to_string(), second),
        ],
        &parent_registry(),
    )
    .unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::SCENE_INVALID_ENTITY_REF)
        .expect("an entity-reference diagnostic");
    assert_eq!(d.path.as_deref(), Some("/fragments/1"));
    assert!(
        d.message.contains("b.yaml") && d.message.contains("entity 1"),
        "{}",
        d.message
    );
}

fn instance_with_parent_override(name: &str, index: u64) -> SceneEntity {
    SceneEntity {
        instance: Some(Box::new(crate::InstanceEntry {
            prefab: "prefabs/p.yaml".to_string(),
            presentation: false,
            transform: crate::InstanceTransform::default(),
            overrides: [(
                "part".to_string(),
                vec![(
                    "Parent".to_string(),
                    serde_json::json!({"index": index, "generation": 0}),
                )],
            )]
            .into(),
            removed: vec![],
        })),
        ..plain(name)
    }
}

#[test]
fn a_fragment_instance_override_ref_is_rebased() {
    let first = Scene {
        entities: vec![plain("a0"), plain("a1")],
        ..Scene::default()
    };
    let second = Scene {
        entities: vec![plain("b0"), instance_with_parent_override("b1", 0)],
        ..Scene::default()
    };
    let map = parse_map(FLOOR_3X3).expect("map parses");
    let scene = generate(
        &map,
        &kit(),
        &[
            ("a.yaml".to_string(), first),
            ("b.yaml".to_string(), second),
        ],
        &parent_registry(),
    )
    .expect("generates");
    let instance = entity(&scene, "b1").instance.as_ref().expect("an instance");
    assert_eq!(instance.overrides["part"][0].1["index"], 2);
}

#[test]
fn a_fragment_instance_override_ref_outside_the_fragment_is_l1017() {
    let first = Scene {
        entities: vec![plain("a0")],
        ..Scene::default()
    };
    let second = Scene {
        entities: vec![plain("b0"), instance_with_parent_override("b1", 2)],
        ..Scene::default()
    };
    let map = parse_map(FLOOR_3X3).expect("map parses");
    let diagnostics = generate(
        &map,
        &kit(),
        &[
            ("a.yaml".to_string(), first),
            ("b.yaml".to_string(), second),
        ],
        &parent_registry(),
    )
    .unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::SCENE_INVALID_ENTITY_REF)
        .expect("an entity-reference diagnostic");
    assert_eq!(d.path.as_deref(), Some("/fragments/1"));
    assert!(
        d.message.contains("b.yaml") && d.message.contains("entity 1"),
        "{}",
        d.message
    );
}

// --- Overrides and mirror ---------------------------------------------------------------------

// A dyadic grid with a non-zero origin and an odd width: a merged floor, a per-cell collider wall, a
// footprint post, a variant tree, an orb, and a rotated marker.
const MIRROR_MAP: &str = "\
format: 1
kind: map
kit: kit.yaml
seed: 1
cell_size: 2.0
origin:
  - -8.0
  - -4.0
rows:
  - \"WWWW#\"
  - \"..VO.\"
  - \"#....\"
markers:
  - name: door
    kind: gate
    x: -3.0
    z: 0.0
    rotation: 30.0
";

/// The X and Z coordinate sums of the reflection lines of [`MIRROR_MAP`]'s grid.
const LINE_SUM_X: f64 = -6.0;
const LINE_SUM_Z: f64 = -2.0;

fn mirrored(mirror: Mirror) -> Scene {
    let mut map = parse_map(MIRROR_MAP).expect("the map parses");
    MapOverrides {
        mirror,
        ..MapOverrides::default()
    }
    .apply(&mut map);
    generate(&map, &kit(), &[], &Registry::new()).expect("the map generates cleanly")
}

fn translation(entity: &SceneEntity) -> Vec<f64> {
    component(entity, "Transform")["translation"]
        .as_array()
        .expect("translation array")
        .iter()
        .map(|v| v.as_f64().expect("number"))
        .collect()
}

fn collider_boxes(scene: &Scene) -> Vec<[f64; 4]> {
    let mut boxes: Vec<[f64; 4]> = scene
        .entities
        .iter()
        .filter(|e| e.components.iter().any(|(n, _)| n == "Collider"))
        .map(|e| {
            let t = translation(e);
            let half = &component(e, "Collider")["shape"]["aabb"]["half_extents"];
            let (hx, hz) = (half[0].as_f64().unwrap(), half[1].as_f64().unwrap());
            [t[0] - hx, t[0] + hx, t[2] - hz, t[2] + hz]
        })
        .collect();
    boxes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    boxes
}

#[test]
fn seed_override_equals_a_map_with_that_seed() {
    let registry = Registry::new();
    let mut overridden = parse_map(MIRROR_MAP).unwrap();
    MapOverrides {
        seed: Some(7),
        ..MapOverrides::default()
    }
    .apply(&mut overridden);
    let edited = parse_map(&MIRROR_MAP.replace("seed: 1", "seed: 7")).unwrap();
    assert_eq!(overridden, edited);
    let a = generate(&overridden, &kit(), &[], &registry).unwrap();
    let b = generate(&edited, &kit(), &[], &registry).unwrap();
    assert_eq!(emit(&a, &registry), emit(&b, &registry));
}

#[test]
fn kit_override_equals_a_map_with_that_kit() {
    let mut overridden = parse_map(MIRROR_MAP).unwrap();
    MapOverrides {
        kit: Some("other/k.kit.yaml".to_string()),
        ..MapOverrides::default()
    }
    .apply(&mut overridden);
    let edited = parse_map(&MIRROR_MAP.replace("kit: kit.yaml", "kit: other/k.kit.yaml")).unwrap();
    assert_eq!(overridden, edited);
}

#[test]
fn mirror_reflects_every_transform_exactly() {
    for (mirror, axis, line_sum) in [(Mirror::X, 0, LINE_SUM_X), (Mirror::Z, 2, LINE_SUM_Z)] {
        let plain = mirrored(Mirror::None);
        let flipped = mirrored(mirror);
        assert_eq!(names(&plain), names(&flipped));
        assert_eq!(plain.entities.len(), flipped.entities.len());
        for (a, b) in plain.entities.iter().zip(&flipped.entities) {
            assert_eq!(a.name, b.name);
            let (ta, tb) = (translation(a), translation(b));
            for i in 0..3 {
                if i == axis {
                    assert_eq!(tb[i], line_sum - ta[i], "{:?} axis", a.name);
                } else {
                    assert_eq!(tb[i], ta[i], "{:?} coordinate {i}", a.name);
                }
            }
            let (ra, rb) = (
                component(a, "Transform")["rotation"][1].as_f64().unwrap(),
                component(b, "Transform")["rotation"][1].as_f64().unwrap(),
            );
            let expected = if axis == 0 {
                if ra == 0.0 { 0.0 } else { -ra }
            } else {
                let mut r = 180.0 - ra;
                if r > 180.0 {
                    r -= 360.0;
                }
                if r <= -180.0 {
                    r += 360.0;
                }
                if r == 0.0 { 0.0 } else { r }
            };
            assert_eq!(rb, expected, "{:?} yaw", a.name);
            assert!(rb.is_sign_positive() || rb != 0.0, "no -0.0 yaw");
            assert_eq!(
                component(a, "Transform")["scale"],
                component(b, "Transform")["scale"]
            );
            assert!(
                component(b, "Transform")["scale"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|s| s.as_f64().unwrap() >= 0.0)
            );
            assert_eq!(component(a, "Mesh"), component(b, "Mesh"));
            assert_eq!(
                a.components.iter().find(|(n, _)| n == "Collider"),
                b.components.iter().find(|(n, _)| n == "Collider")
            );
        }
    }
}

/// The XZ forward of a yaw in degrees: local forward is -Z.
fn forward(yaw: f64) -> [f64; 2] {
    let (sin, cos) = ludion_core::math::f64::sin_cos(yaw.to_radians());
    [-sin, -cos]
}

#[test]
fn mirrored_forward_is_the_reflected_forward() {
    for (mirror, axis) in [(Mirror::X, 0), (Mirror::Z, 1)] {
        let plain = mirrored(Mirror::None);
        let flipped = mirrored(mirror);
        for (a, b) in plain.entities.iter().zip(&flipped.entities) {
            let yaw = |e: &SceneEntity| component(e, "Transform")["rotation"][1].as_f64().unwrap();
            let (fa, fb) = (forward(yaw(a)), forward(yaw(b)));
            for i in 0..2 {
                let expected = if i == axis { -fa[i] } else { fa[i] };
                assert!(
                    (fb[i] - expected).abs() < 1e-6,
                    "{:?} {mirror:?} forward {i}: {} vs {expected}",
                    a.name,
                    fb[i]
                );
            }
        }
    }
}

// Dyadic values and a rotated marker; the grid spans 0..4 on both axes, so both line sums are 4.
const VECTOR_MAP: &str = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
rows:
  - \"..\"
  - \"..\"
markers:
  - name: lamp
    kind: gate
    x: 1.0
    z: 3.0
    rotation: -150.0
    components:
      SpotLight:
        direction:
          - 0.5
          - -1.0
          - 0.25
      Velocity:
        linear:
          - 1.0
          - 2.0
          - -4.0
        angular:
          - 0.5
          - -0.25
          - 2.0
      NavAgent:
        target:
          - 1.0
          - 0.0
          - 3.0
  - name: walker
    kind: gate
    x: 3.0
    z: 1.0
    components:
      NavAgent: {}
";

fn mirrored_vectors(mirror: Mirror) -> Scene {
    let mut map = parse_map(VECTOR_MAP).expect("the map parses");
    MapOverrides {
        mirror,
        ..MapOverrides::default()
    }
    .apply(&mut map);
    generate(&map, &kit(), &[], &Registry::new()).expect("the map generates cleanly")
}

fn vec3(entity: &SceneEntity, comp: &str, field: &str) -> Vec<f64> {
    component(entity, comp)[field]
        .as_array()
        .unwrap_or_else(|| panic!("{comp}.{field} array"))
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect()
}

#[test]
fn mirror_reflects_builtin_world_vectors() {
    let line_sum = 4.0;
    for (mirror, axis) in [(Mirror::X, 0), (Mirror::Z, 2)] {
        let scene = mirrored_vectors(mirror);
        let lamp = entity(&scene, "lamp");
        let pick = |v: [f64; 3], f: fn(usize, usize) -> bool| -> Vec<f64> {
            (0..3)
                .map(|i| if f(i, axis) { -v[i] } else { v[i] })
                .collect()
        };
        // A direction negates the mirrored axis; angular velocity is a pseudovector, so it keeps
        // that component and negates the other two.
        assert_eq!(
            vec3(lamp, "SpotLight", "direction"),
            pick([0.5, -1.0, 0.25], |i, a| i == a)
        );
        assert_eq!(
            vec3(lamp, "Velocity", "linear"),
            pick([1.0, 2.0, -4.0], |i, a| i == a)
        );
        assert_eq!(
            vec3(lamp, "Velocity", "angular"),
            pick([0.5, -0.25, 2.0], |i, a| i != a)
        );
        // A point reflects across the grid line like a translation.
        let mut target = [1.0, 0.0, 3.0];
        target[axis] = line_sum - target[axis];
        assert_eq!(vec3(lamp, "NavAgent", "target"), target);
        // A missing target is the origin, so it reflects to the line.
        let walker = entity(&scene, "walker");
        let mut origin = [0.0, 0.0, 0.0];
        origin[axis] = line_sum;
        assert_eq!(vec3(walker, "NavAgent", "target"), origin);
        // -150 degrees yaw: Z gives 180 + 150 = 330, wrapped to -30.
        let yaw = component(lamp, "Transform")["rotation"][1]
            .as_f64()
            .unwrap();
        assert_eq!(yaw, if axis == 0 { 150.0 } else { -30.0 });
    }
}

#[test]
fn mirror_of_world_vectors_is_an_involution() {
    for (mirror, line_sum) in [(Mirror::X, 4.0), (Mirror::Z, 4.0)] {
        let mut scene = mirrored_vectors(mirror);
        for entity in &mut scene.entities {
            mirror_entity(entity, mirror, line_sum);
        }
        let mut plain = mirrored_vectors(Mirror::None);
        // The absent target materialises as the origin on the first mirror and stays.
        for e in &mut plain.entities {
            if let Some((_, v)) = e.components.iter_mut().find(|(n, _)| n == "NavAgent")
                && v.get("target").is_none()
            {
                v["target"] = serde_json::json!([0.0, 0.0, 0.0]);
            }
        }
        assert_eq!(scene, plain);
    }
}

#[test]
fn mirror_is_an_involution() {
    for (mirror, line_sum) in [(Mirror::X, LINE_SUM_X), (Mirror::Z, LINE_SUM_Z)] {
        let mut scene = mirrored(mirror);
        for entity in &mut scene.entities {
            mirror_entity(entity, mirror, line_sum);
        }
        assert_eq!(scene, mirrored(Mirror::None));
    }
}

#[test]
fn mirrored_collider_boxes_are_the_reflected_set() {
    let plain = collider_boxes(&mirrored(Mirror::None));
    assert!(!plain.is_empty());
    let mut reflected_x: Vec<[f64; 4]> = plain
        .iter()
        .map(|b| [LINE_SUM_X - b[1], LINE_SUM_X - b[0], b[2], b[3]])
        .collect();
    reflected_x.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(collider_boxes(&mirrored(Mirror::X)), reflected_x);
    let mut reflected_z: Vec<[f64; 4]> = plain
        .iter()
        .map(|b| [b[0], b[1], LINE_SUM_Z - b[3], LINE_SUM_Z - b[2]])
        .collect();
    reflected_z.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(collider_boxes(&mirrored(Mirror::Z)), reflected_z);
}

#[test]
fn mirror_with_fragments_is_l1399() {
    let fragment = Scene {
        entities: vec![SceneEntity {
            presentation: false,
            name: Some("hand-placed".to_string()),
            components: vec![],
            instance: None,
        }],
        ..Scene::default()
    };
    let mut map = parse_map(MIRROR_MAP).unwrap();
    map.fragments = vec!["frag.yaml".to_string()];
    map.mirror = Mirror::Z;
    let diagnostics = generate(
        &map,
        &kit(),
        &[("frag.yaml".to_string(), fragment.clone())],
        &Registry::new(),
    )
    .unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_MIRROR_FRAGMENTS)
        .expect("an L1399 diagnostic");
    assert_eq!(d.path.as_deref(), Some("/fragments"));
    map.mirror = Mirror::None;
    assert!(
        generate(
            &map,
            &kit(),
            &[("frag.yaml".to_string(), fragment)],
            &Registry::new()
        )
        .is_ok()
    );
}

#[test]
fn mirror_none_is_byte_identical_to_today() {
    let registry = Registry::new();
    let mut map = parse_map(MIRROR_MAP).unwrap();
    MapOverrides::default().apply(&mut map);
    assert!(MapOverrides::default().is_empty());
    assert_eq!(map, parse_map(MIRROR_MAP).unwrap());
    let a = generate(&map, &kit(), &[], &registry).unwrap();
    assert_eq!(emit(&a, &registry), emit(&gen_ok(MIRROR_MAP), &registry));
}

#[test]
fn provenance_spelling() {
    assert_eq!(
        MapOverrides::default().provenance("a.map.yaml"),
        "a.map.yaml"
    );
    let all = MapOverrides {
        kit: Some("k/x.kit.yaml".to_string()),
        seed: Some(7),
        mirror: Mirror::Z,
    };
    assert_eq!(
        all.provenance("a.map.yaml"),
        "a.map.yaml --kit k/x.kit.yaml --seed 7 --mirror z"
    );
}

// --- `static: true` -----------------------------------------------------------------------------

// A kit whose floor (`.`, merged, also the base underlay), box-collider wall (`#`, per cell),
// footprint post (`W`) and orb (`O`) are all `static: true`, beside a plain tile (`P`).
const STATIC_KIT: &str = "\
format: 1
kind: map-kit
tiles:
  \".\":
    - name: floor
      shape: cube
      merge: true
      static: true
  \"#\":
    - name: wall
      shape: cube
      static: true
      collider:
        box:
          - 1.0
          - 1.0
          - 1.0
  W:
    - name: post
      shape: cube
      footprint:
        - 2
        - 1
      static: true
  O:
    - name: orb
      shape: sphere
      static: true
  P:
    - name: plain
      shape: cube
";

const STATIC_MAP: &str = "\
format: 1
kind: map
kit: kit.yaml
cell_size: 2.0
origin:
  - 0.0
  - 0.0
base: \".\"
rows:
  - \"#WWO\"
  - \"P...\"
";

fn static_scene(overrides: MapOverrides) -> Scene {
    let kit = parse_map_kit(STATIC_KIT).expect("the static kit parses");
    let mut map = parse_map(STATIC_MAP).expect("the map parses");
    overrides.apply(&mut map);
    generate(&map, &kit, &[], &Registry::new()).expect("the static map generates")
}

fn has_static(entity: &SceneEntity) -> bool {
    entity.components.iter().any(|(name, _)| name == "Static")
}

#[test]
fn static_kit_primitives_emit_the_marker_on_mesh_entities_only() {
    let scene = static_scene(MapOverrides::default());
    let mut seen = [0usize; 4];
    for entity in &scene.entities {
        let name = entity.name.as_deref().unwrap_or_default();
        if name.ends_with("-collider") {
            assert!(
                !has_static(entity),
                "{name}: a collider sibling is never static"
            );
            continue;
        }
        let slot = ["floor-", "wall-", "post-", "orb-"]
            .iter()
            .position(|prefix| name.starts_with(prefix));
        if let Some(slot) = slot {
            assert_eq!(
                component(entity, "Static"),
                &serde_json::json!({}),
                "{name}"
            );
            seen[slot] += 1;
        } else if name.starts_with("plain-") {
            assert!(!has_static(entity), "{name}: no `static` key, no marker");
        }
    }
    assert!(
        seen.iter().all(|n| *n > 0),
        "every expansion mode emitted: {seen:?}"
    );
    assert!(
        scene
            .entities
            .iter()
            .any(|e| e.name.as_deref().is_some_and(|n| n.ends_with("-collider"))),
        "the box collider sibling exists"
    );
}

#[test]
fn a_mirrored_static_map_keeps_the_marker() {
    let scene = static_scene(MapOverrides {
        mirror: Mirror::X,
        ..MapOverrides::default()
    });
    let marked = scene.entities.iter().filter(|e| has_static(e)).count();
    assert!(marked > 0);
    for entity in &scene.entities {
        let name = entity.name.as_deref().unwrap_or_default();
        let is_mesh = entity.components.iter().any(|(n, _)| n == "Mesh");
        assert_eq!(
            has_static(entity),
            is_mesh && !name.starts_with("plain-"),
            "{name}"
        );
    }
}

#[test]
fn static_on_a_marker_kind_is_l1022() {
    let bad = "\
format: 1
kind: map-kit
tiles:
  \".\":
    - shape: cube
kinds:
  gate:
    - shape: cube
      static: true
";
    let kit = parse_map_kit(bad).expect("the kit parses structurally");
    let map = parse_map(FLOOR_3X3).expect("map parses");
    let diagnostics = generate(&map, &kit, &[], &Registry::new()).unwrap_err();
    assert!(
        diagnostics.iter().any(|d| d.code == code::MAP_KIT_INVALID
            && d.path.as_deref() == Some("/kinds/gate/0/static"))
    );
}

// --- Material scalars, collider-only primitives, sculpt, per-cell base -------------------------

/// A one-tile kit (`T`) whose single primitive is the given block-style YAML lines.
fn tile_kit(primitive: &str) -> MapKit {
    parse_map_kit(&format!(
        "format: 1\nkind: map-kit\ntiles:\n  T:\n    - {primitive}\n"
    ))
    .expect("the kit parses")
}

fn gen_tiles(kit: &MapKit, rows: &[&str]) -> Scene {
    let rows: String = rows.iter().map(|r| format!("  - \"{r}\"\n")).collect();
    let src = format!(
        "format: 1\nkind: map\nkit: kit.yaml\ncell_size: 2.0\norigin:\n  - 0.0\n  - 0.0\nrows:\n{rows}"
    );
    let map = parse_map(&src).expect("the map parses");
    generate(&map, kit, &[], &Registry::new()).expect("generates")
}

fn tile_errors(kit_src: &str) -> Vec<Diagnostic> {
    let kit = parse_map_kit(kit_src);
    match kit {
        Err(d) => d,
        Ok(kit) => {
            let map = parse_map(TWO_BY_TWO_T).expect("the map parses");
            generate(&map, &kit, &[], &Registry::new()).unwrap_err()
        }
    }
}

fn has_collider(e: &SceneEntity) -> bool {
    e.components.iter().any(|(n, _)| n == "Collider")
}

#[test]
fn material_scalars_are_emitted_on_the_mesh() {
    let kit =
        tile_kit("shape: cube\n      roughness: 0.25\n      metallic: 0.5\n      emissive: 2.0");
    let scene = gen_tiles(&kit, &["T"]);
    let mesh = component(&scene.entities[0], "Mesh");
    assert_eq!(mesh["roughness"], serde_json::json!(0.25));
    assert_eq!(mesh["metallic"], serde_json::json!(0.5));
    assert_eq!(mesh["emissive"], serde_json::json!(2.0));
}

#[test]
fn absent_material_scalars_are_not_emitted() {
    let kit = tile_kit("shape: cube\n      roughness: 0.25");
    let scene = gen_tiles(&kit, &["T"]);
    let mesh = component(&scene.entities[0], "Mesh");
    assert!(mesh.get("roughness").is_some());
    assert!(mesh.get("metallic").is_none());
    assert!(mesh.get("emissive").is_none());
    let plain = gen_tiles(&tile_kit("shape: cube"), &["T"]);
    assert_eq!(
        component(&plain.entities[0], "Mesh"),
        &serde_json::json!({ "shape": "cube" })
    );
}

#[test]
fn out_of_range_material_scalars_are_l1022_at_the_key() {
    for (key, value) in [
        ("roughness", "1.5"),
        ("metallic", "-0.1"),
        ("emissive", "-1"),
    ] {
        let kit = format!(
            "format: 1\nkind: map-kit\ntiles:\n  T:\n    - shape: cube\n      {key}: {value}\n"
        );
        let diagnostics = tile_errors(&kit);
        let want = format!("/tiles/T/0/{key}");
        assert!(
            diagnostics.iter().any(
                |d| d.code == code::MAP_KIT_INVALID && d.path.as_deref() == Some(want.as_str())
            ),
            "{key}: {diagnostics:#?}"
        );
    }
}

#[test]
fn a_merge_never_blends_material_scalars() {
    let kit = parse_map_kit(
        "format: 1\nkind: map-kit\ntiles:\n  A:\n    - shape: cube\n      merge: true\n      roughness: 0.2\n  B:\n    - shape: cube\n      merge: true\n      roughness: 0.8\n",
    )
    .expect("the kit parses");
    let scene = gen_tiles(&kit, &["AABB"]);
    assert_eq!(scene.entities.len(), 2);
    assert_eq!(
        component(&scene.entities[0], "Mesh")["roughness"],
        serde_json::json!(0.2f32 as f64)
    );
    assert_eq!(
        component(&scene.entities[1], "Mesh")["roughness"],
        serde_json::json!(0.8f32 as f64)
    );
}

#[test]
fn a_sculpt_source_and_variant_emit_a_sculpt_shape() {
    let kit = tile_kit(
        "sculpt: sculpts/rock.sculpt.yaml\n      variants:\n        - sculpt: sculpts/rock2.sculpt.yaml",
    );
    let scene = gen_tiles(&kit, &["TTTT"]);
    let mut seen = Vec::new();
    for e in &scene.entities {
        seen.push(
            component(e, "Mesh")["shape"]["sculpt"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    assert!(seen.iter().all(|p| p.starts_with("sculpts/rock")));
    assert!(
        seen.iter().any(|p| p == "sculpts/rock.sculpt.yaml")
            || seen.iter().any(|p| p == "sculpts/rock2.sculpt.yaml")
    );
    let single = gen_tiles(&tile_kit("sculpt: sculpts/rock.sculpt.yaml"), &["T"]);
    assert_eq!(
        component(&single.entities[0], "Mesh")["shape"],
        serde_json::json!({ "sculpt": "sculpts/rock.sculpt.yaml" })
    );
}

#[test]
fn a_collider_only_primitive_is_one_unit_scale_entity_with_its_collider() {
    for (collider, key) in [
        ("aabb:\n          - 1.0\n          - 1.0", "aabb"),
        ("circle: 0.5", "circle"),
        (
            "box:\n          - 1.0\n          - 2.0\n          - 1.0",
            "box",
        ),
    ] {
        let kit = tile_kit(&format!("y: 0.5\n      collider:\n        {collider}"));
        let scene = gen_tiles(&kit, &["T"]);
        assert_eq!(scene.entities.len(), 1, "{key}");
        let e = &scene.entities[0];
        assert!(e.components.iter().all(|(n, _)| n != "Mesh"), "{key}");
        assert_eq!(
            component(e, "Transform")["scale"],
            serde_json::json!([1.0, 1.0, 1.0])
        );
        assert!(
            component(e, "Collider")["shape"].get(key).is_some(),
            "{key}"
        );
        assert!(!e.name.as_deref().unwrap().ends_with("-collider"));
    }
}

#[test]
fn a_merged_collider_only_aabb_grows_to_the_rect() {
    let kit =
        tile_kit("merge: true\n      collider:\n        aabb:\n          - 1.0\n          - 1.0");
    let scene = gen_tiles(&kit, &["TT", "TT"]);
    assert_eq!(scene.entities.len(), 1);
    assert_eq!(
        component(&scene.entities[0], "Collider")["shape"]["aabb"]["half_extents"],
        serde_json::json!([2.0, 2.0])
    );
}

#[test]
fn a_collider_only_marker_kind_carries_the_marker_components() {
    let kit = parse_map_kit(
        "format: 1\nkind: map-kit\ntiles:\n  T:\n    - shape: cube\nkinds:\n  trigger:\n    - collider:\n        circle: 1.0\n",
    )
    .expect("the kit parses");
    let src = "format: 1\nkind: map\nkit: kit.yaml\ncell_size: 2.0\norigin:\n  - 0.0\n  - 0.0\nrows:\n  - \"T\"\nmarkers:\n  - name: zone\n    kind: trigger\n    x: 1.0\n    z: 1.0\n    components:\n      Zone:\n        id: 7\n";
    let map = parse_map(src).expect("the map parses");
    let scene = generate(&map, &kit, &[], &Registry::new()).expect("generates");
    let zone = entity(&scene, "zone");
    let names: Vec<_> = zone.components.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["Transform", "Collider", "Zone"]);
}

#[test]
fn a_mirrored_collider_only_entity_reflects() {
    let kit = tile_kit("collider:\n        circle: 0.5");
    let src = "format: 1\nkind: map\nkit: kit.yaml\ncell_size: 2.0\norigin:\n  - 0.0\n  - 0.0\nrows:\n  - \"T   \"\n";
    let generate_with = |mirror| {
        let mut map = parse_map(src).unwrap();
        map.mirror = mirror;
        generate(&map, &kit, &[], &Registry::new()).expect("generates")
    };
    let (a, b) = (generate_with(Mirror::None), generate_with(Mirror::X));
    assert_eq!(
        translation(&b.entities[0])[0],
        8.0 - translation(&a.entities[0])[0]
    );
}

#[test]
fn a_primitive_with_neither_source_nor_collider_is_l1021() {
    let diagnostics = tile_errors("format: 1\nkind: map-kit\ntiles:\n  T:\n    - y: 1.0\n");
    let d = diagnostics
        .iter()
        .find(|d| d.code == code::MAP_DOCUMENT_STRUCTURE)
        .expect("a structure diagnostic");
    assert!(
        d.message.contains("`sculpt`") && d.message.contains("`collider`"),
        "{}",
        d.message
    );
    assert!(d.help.is_some());
}

#[test]
fn mesh_only_keys_on_a_collider_only_primitive_are_l1022() {
    for (key, value) in [
        (
            "color",
            "\n        - 1.0\n        - 1.0\n        - 1.0\n        - 1.0",
        ),
        ("scale", "\n        - 1.0\n        - 1.0\n        - 1.0"),
        ("roughness", " 0.5"),
        ("metallic", " 0.5"),
        ("emissive", " 0.5"),
        ("static", " true"),
        ("variants", "\n        - shape: cube"),
    ] {
        let kit = format!(
            "format: 1\nkind: map-kit\ntiles:\n  T:\n    - collider:\n        circle: 1.0\n      {key}:{value}\n"
        );
        let diagnostics = tile_errors(&kit);
        let want = format!("/tiles/T/0/{key}");
        assert!(
            diagnostics.iter().any(
                |d| d.code == code::MAP_KIT_INVALID && d.path.as_deref() == Some(want.as_str())
            ),
            "{key}: {diagnostics:#?}"
        );
    }
}

const BASE_FLOOR_KIT: &str = "format: 1\nkind: map-kit\ntiles:\n  B:\n    - name: floor\n      shape: cube\n      merge: false\n";

#[test]
fn a_base_primitive_with_merge_false_emits_one_unscaled_piece_per_cell() {
    let kit = parse_map_kit(BASE_FLOOR_KIT).expect("the kit parses");
    let src = "format: 1\nkind: map\nkit: kit.yaml\ncell_size: 2.0\norigin:\n  - 0.0\n  - 0.0\nbase: B\nrows:\n  - \"BB\"\n  - \"B \"\n";
    let map = parse_map(src).expect("the map parses");
    let scene = generate(&map, &kit, &[], &Registry::new()).expect("generates");
    let floors: Vec<_> = names(&scene)
        .into_iter()
        .filter(|n| n.starts_with("floor-"))
        .collect();
    assert_eq!(
        floors,
        vec!["floor-r000-c000", "floor-r000-c001", "floor-r001-c000"]
    );
    for e in &scene.entities {
        assert_eq!(
            component(e, "Transform")["scale"],
            serde_json::json!([1.0, 1.0, 1.0])
        );
    }
}

#[test]
fn a_per_cell_base_primitive_accepts_a_circle_collider() {
    let kit = parse_map_kit(
        "format: 1\nkind: map-kit\ntiles:\n  B:\n    - shape: cube\n      merge: false\n      collider:\n        circle: 0.5\n",
    )
    .expect("the kit parses");
    let src = "format: 1\nkind: map\nkit: kit.yaml\ncell_size: 2.0\norigin:\n  - 0.0\n  - 0.0\nbase: B\nrows:\n  - \"BB\"\n";
    let map = parse_map(src).expect("the map parses");
    let scene =
        generate(&map, &kit, &[], &Registry::new()).expect("a per-cell base takes any collider");
    assert_eq!(scene.entities.len(), 2);
    assert!(scene.entities.iter().all(has_collider));
}

// --- Collider surface tags ----------------------------------------------------------------------

/// The one `#` cell of a one-row map, over a kit whose `#` primitive is `primitive` (block YAML).
fn surface_scene(primitive: &str) -> Scene {
    let kit = parse_map_kit(&format!(
        "format: 1\nkind: map-kit\ntiles:\n  \"#\":\n{primitive}"
    ))
    .expect("the kit parses");
    let map = parse_map(&TWO_BY_TWO_T.replace("  - \"TT\"\n  - \"TT\"\n", "  - \"#\"\n"))
        .expect("the map parses");
    generate(&map, &kit, &[], &Registry::new()).expect("generates")
}

fn surface_of(entity: &SceneEntity) -> Option<&Value> {
    entity
        .components
        .iter()
        .find(|(n, _)| n == "SurfaceTag")
        .map(|(_, v)| v)
}

const BOX_SURFACE: &str = "        box:\n          - 1.0\n          - 1.0\n          - 1.0\n        surface: metal_grate\n";

#[test]
fn a_box_collider_surface_emits_a_surface_tag_on_the_collider_sibling() {
    let scene = surface_scene(&format!(
        "    - shape: cube\n      collider:\n{BOX_SURFACE}"
    ));
    assert_eq!(scene.entities.len(), 2);
    assert!(surface_of(&scene.entities[0]).is_none());
    assert_eq!(
        surface_of(&scene.entities[1]),
        Some(&serde_json::json!({ "surface": "metal_grate" }))
    );
    let names: Vec<_> = scene.entities[1]
        .components
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, ["Transform", "Collider", "SurfaceTag"]);
}

#[test]
fn a_collider_only_box_surface_emits_the_tag_on_the_one_entity() {
    let scene = surface_scene(&format!("    - collider:\n{BOX_SURFACE}"));
    assert_eq!(scene.entities.len(), 1);
    assert_eq!(
        surface_of(&scene.entities[0]),
        Some(&serde_json::json!({ "surface": "metal_grate" }))
    );
}

#[test]
fn a_merged_base_box_floor_tags_each_rects_sibling() {
    let kit = merged_kit(
        "        box:\n          - 1.0\n          - 0.25\n          - 1.0\n        surface: concrete\n",
    );
    let map = parse_map(TWO_BY_TWO_T).expect("the map parses");
    let scene = generate(&map, &kit, &[], &Registry::new()).expect("generates");
    let tagged = scene
        .entities
        .iter()
        .filter(|e| e.name.as_deref().is_some_and(|n| n.ends_with("-collider")))
        .inspect(|e| {
            assert_eq!(
                surface_of(e),
                Some(&serde_json::json!({ "surface": "concrete" }))
            );
        })
        .count();
    assert_eq!(tagged, 1);
    assert!(scene.entities.iter().all(|e| {
        e.name.as_deref().is_some_and(|n| n.ends_with("-collider")) || surface_of(e).is_none()
    }));
}

fn kit_parse_errors(collider: &str) -> Vec<Diagnostic> {
    parse_map_kit(&format!(
        "format: 1\nkind: map-kit\ntiles:\n  \"#\":\n    - shape: cube\n      collider:\n{collider}"
    ))
    .expect_err("the kit is refused")
}

#[test]
fn a_surface_on_an_aabb_or_circle_or_an_empty_name_is_l1022_at_the_surface() {
    for collider in [
        "        aabb:\n          - 1.0\n          - 1.0\n        surface: metal_grate\n",
        "        circle: 1.0\n        surface: metal_grate\n",
        "        box:\n          - 1.0\n          - 1.0\n          - 1.0\n        surface: \"\"\n",
    ] {
        let diagnostics = kit_parse_errors(collider);
        let d = diagnostics
            .iter()
            .find(|d| d.code == code::MAP_KIT_INVALID)
            .unwrap_or_else(|| panic!("no kit diagnostic for {collider}: {diagnostics:?}"));
        assert_eq!(d.path.as_deref(), Some("/tiles/#/0/collider/surface"));
        assert!(d.help.is_some());
    }
}

#[test]
fn a_kit_without_a_surface_emits_the_components_it_always_did() {
    let scene = surface_scene(
        "    - shape: cube\n      collider:\n        box:\n          - 1.0\n          - 1.0\n          - 1.0\n",
    );
    assert!(scene.entities.iter().all(|e| surface_of(e).is_none()));
    let names: Vec<_> = scene.entities[1]
        .components
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, ["Transform", "Collider"]);
}
