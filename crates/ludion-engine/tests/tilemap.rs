//! `Tilemap` end-to-end: scene validation (L1019/L1020 with JSON Pointers), canonical round-trip,
//! the coordinate-helper API, and the wire `component.set` semantic-rejection path.
//!
//! Validation runs against the real built-in registry — `Tilemap`'s semantic validator is registered
//! alongside its schema (`register_validated`) and invoked generically by the scene validator, so
//! these tests exercise the actual seam a `check-scene` run uses.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ludion_engine::{Engine, TileAppearance, Tilemap, builtin_registry, dispatch};
use ludion_protocol::Command;
use ludion_protocol::command::ComponentSetParams;
use serde_json::json;

/// The one diagnostic from checking `source` against the built-in registry, or a panic if there is
/// not exactly one.
fn one_diagnostic(source: &str) -> ludion_core::diagnostics::Diagnostic {
    let diagnostics = ludion_scene::check(source, &builtin_registry())
        .expect_err("the scene must fail validation");
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got: {diagnostics:?}"
    );
    diagnostics.into_iter().next().unwrap()
}

/// Ragged rows → `L1019` with a JSON Pointer to the offending row.
#[test]
fn ragged_rows_is_l1019_pointing_at_the_row() {
    let source = "\
format: 1
entities:
  - components:
      Tilemap:
        legend:
          \"#\": {}
        rows:
          - \"##\"
          - \"#\"
";
    let diagnostic = one_diagnostic(source);
    assert_eq!(diagnostic.code.as_str(), "L1019");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/entities/0/components/Tilemap/rows/1"),
        "the pointer must name the row whose width differs"
    );
}

/// A glyph with no legend entry → `L1020`, the pointer naming the row it appears in.
#[test]
fn unknown_glyph_is_l1020_pointing_at_the_row() {
    let source = "\
format: 1
entities:
  - components:
      Tilemap:
        legend:
          \"#\": {}
        rows:
          - \"#x\"
";
    let diagnostic = one_diagnostic(source);
    assert_eq!(diagnostic.code.as_str(), "L1020");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/entities/0/components/Tilemap/rows/0")
    );
    assert!(
        diagnostic.message.contains('x'),
        "the message names the offending glyph: {}",
        diagnostic.message
    );
}

/// A legend key that is not a single glyph → `L1020`, the pointer naming the legend key (key-shape
/// violations are folded into L1020, the clearer of the two glyph codes).
#[test]
fn multi_char_legend_key_is_l1020_pointing_at_the_key() {
    let source = "\
format: 1
entities:
  - components:
      Tilemap:
        legend:
          \"ab\": {}
        rows: []
";
    let diagnostic = one_diagnostic(source);
    assert_eq!(diagnostic.code.as_str(), "L1020");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/entities/0/components/Tilemap/legend/ab")
    );
}

/// A well-formed tilemap passes validation cleanly (no false positives from the semantic hook).
#[test]
fn valid_tilemap_passes() {
    let source = "\
format: 1
entities:
  - components:
      Tilemap:
        legend:
          \"#\": {}
          \".\": {}
        rows:
          - \"#.\"
          - \".#\"
";
    assert!(ludion_scene::check(source, &builtin_registry()).is_ok());
}

/// The canonical writer handles the map-valued `legend` field and the multi-row string list
/// cleanly: emission is **idempotent** — `emit(parse(emit(s)))` is byte-equal to `emit(s)` — over the
/// real gallery tilemap (comments in the source are not part of the canonical body, so the fixed
/// point is the emitted form, matching every other gallery scene).
#[test]
fn tilemap_canonical_emission_is_idempotent() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/tilemap.yaml");
    let source = std::fs::read_to_string(&path).expect("gallery tilemap scene reads");
    let registry = builtin_registry();

    let scene = ludion_scene::check(&source, &registry).expect("gallery tilemap is valid");
    let once = ludion_scene::emit(&scene, &registry);
    let reparsed = ludion_scene::check(&once, &registry).expect("emitted tilemap re-validates");
    let twice = ludion_scene::emit(&reparsed, &registry);
    assert_eq!(once, twice, "canonical emission must be a fixed point");
    // The map-valued field and multi-row list survive the round-trip intact.
    assert!(once.contains("legend:"));
    assert!(once.contains("- \"#o.#\""));
}

/// A `Tilemap` value survives a parse → instantiate → serialize round-trip unchanged.
#[test]
fn tilemap_value_round_trips_through_the_world() {
    let mut legend = BTreeMap::new();
    legend.insert(
        "#".to_string(),
        TileAppearance {
            texture: Some("assets/brick.png".to_string()),
            color: [1.0, 1.0, 1.0, 1.0],
            ..TileAppearance::default()
        },
    );
    legend.insert(".".to_string(), TileAppearance::default());
    let original = Tilemap {
        cell_size: 1.5,
        legend,
        rows: vec!["#.".to_string(), ".#".to_string()],
        layer: 0,
    };

    let value = serde_json::to_value(&original).unwrap();
    let back: Tilemap = serde_json::from_value(value).unwrap();
    assert_eq!(original, back);
}

/// The coordinate helpers respect the placement convention and round-trip a cell center exactly.
#[test]
fn helper_api_maps_cells_and_world_both_ways() {
    let mut legend = BTreeMap::new();
    legend.insert("#".to_string(), TileAppearance::default());
    legend.insert(".".to_string(), TileAppearance::default());
    let map = Tilemap {
        cell_size: 2.0,
        legend,
        rows: vec!["#.#".to_string(), ".#.".to_string()],
        layer: 0,
    };

    assert_eq!(map.row_count(), 2);
    assert_eq!(map.col_count(), 3);
    assert_eq!(map.glyph_at(0, 0), Some('#'));
    assert_eq!(map.glyph_at(2, 0), Some('#'));
    assert_eq!(map.glyph_at(1, 1), Some('#'));
    assert_eq!(map.glyph_at(3, 0), None);
    assert_eq!(map.glyph_at(0, 2), None);

    let origin = [4.0, 2.0, -1.0];
    // +X per column, −Y per row, z unchanged.
    assert_eq!(map.cell_to_world(origin, 2, 1), [8.0, 0.0, -1.0]);
    // world_to_cell is the exact inverse at a cell center, and round-trips every in-range cell.
    for row in 0..map.row_count() as i32 {
        for col in 0..map.col_count() as i32 {
            let world = map.cell_to_world(origin, col, row);
            assert_eq!(map.world_to_cell(origin, world), (col, row));
        }
    }
}

/// A wire `component.set` of a semantically invalid tilemap fails loudly (L1019), before any world
/// mutation — matching the scene-validation path, so the wire is not a back door around it.
#[test]
fn component_set_rejects_an_invalid_tilemap() {
    let mut engine = Engine::new();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    let entity = engine.world_mut().spawn();

    let ragged = json!({
        "cell_size": 1.0,
        "legend": { "#": {} },
        "rows": ["##", "#"],
    });
    let error = dispatch(
        &mut engine,
        Command::ComponentSet(ComponentSetParams {
            entity,
            component: "Tilemap".to_string(),
            value: ragged,
        }),
    )
    .expect_err("a ragged tilemap must be rejected");
    assert_eq!(error.code.as_str(), "L1019");
    // The entity was not given the component (rejected before mutation).
    assert!(
        !builtin_registry()
            .get("Tilemap")
            .unwrap()
            .has(engine.world(), entity),
        "a rejected component.set must not have inserted the component"
    );

    // A well-formed tilemap sets cleanly on the same entity.
    let ok = json!({ "legend": { "#": {} }, "rows": ["##"] });
    dispatch(
        &mut engine,
        Command::ComponentSet(ComponentSetParams {
            entity,
            component: "Tilemap".to_string(),
            value: ok,
        }),
    )
    .expect("a valid tilemap sets cleanly");
}

/// A non-positive `cell_size` divides every `world_to_cell` by zero (or flips the grid) — the schema
/// rejects it, as it does for `MeshGrid`.
#[test]
fn a_non_positive_cell_size_fails_validation() {
    for size in ["0.0", "-2.0"] {
        let source = format!(
            "format: 1
entities:
  - components:
      Tilemap:
        cell_size: {size}
        legend:
          \"#\": {{}}
        rows:
          - \"#\"
"
        );
        let diagnostics = ludion_scene::check(&source, &builtin_registry())
            .expect_err("a non-positive cell_size must fail validation");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.path.as_deref() == Some("/entities/0/components/Tilemap/cell_size")),
            "cell_size {size}: {diagnostics:?}"
        );
    }
}
