//! Round-trip and canonical-form properties for voxel model documents (`voxels/*.voxel.yaml`,
//! Phase 10 Wave 1) — the voxel analog of `mesh_roundtrip.rs`:
//!
//! - **value round trip**: model → emit → check loses nothing;
//! - **emit idempotence**: `emit_voxel(parse_voxel(emit_voxel(d)))` is *byte-equal* to
//!   `emit_voxel(d)`;
//! - the emitted text is LF-only and validates cleanly;
//! - a hand-authored fixture pins the exact canonical text;
//! - every `L1114` misshape is reported with a JSON Pointer;
//! - `L1115` (the cell budget) warns without failing `check_voxel`.

use ludion_scene::{
    PaletteEntry, VoxelDocument, check_voxel, emit_voxel, parse_voxel, validate_voxel,
    voxel_budget_warning,
};
use proptest::prelude::*;
#[path = "support/deterministic.rs"]
mod deterministic;
use deterministic::deterministic_config;

// --- Strategies -------------------------------------------------------------------------------

/// The glyph pool, in ascending char order (so a subset taken in order is already sorted — the
/// canonical palette order the emitter writes).
const GLYPH_POOL: [char; 5] = ['#', '1', 'A', 'B', 'C'];

/// Finite once narrowed to `f32`, the range the kind readers accept.
fn finite_f64() -> impl Strategy<Value = f64> {
    any::<f64>().prop_filter("finite as f32", |f| (*f as f32).is_finite())
}

fn color() -> impl Strategy<Value = [f64; 4]> {
    (finite_f64(), finite_f64(), finite_f64(), finite_f64()).prop_map(|(r, g, b, a)| [r, g, b, a])
}

/// Valid `metallic`/`roughness` values: perceptual, `[0, 1]`.
fn unit_f64() -> impl Strategy<Value = f64> {
    0.0f64..=1.0f64
}

/// Valid `emissive` values: a non-negative HDR intensity multiplier.
fn non_negative_f64() -> impl Strategy<Value = f64> {
    0.0f64..1_000.0f64
}

/// A palette entry's full field set: a color plus optional `emissive`/`metallic`/`roughness`
/// material scalars, each independently authored or absent.
fn palette_entry_fields() -> impl Strategy<Value = ([f64; 4], Option<f64>, Option<f64>, Option<f64>)>
{
    (
        color(),
        proptest::option::of(non_negative_f64()),
        proptest::option::of(unit_f64()),
        proptest::option::of(unit_f64()),
    )
}

/// Valid `scale` values: the declared intended world scale, finite and strictly positive.
fn positive_f64() -> impl Strategy<Value = f64> {
    (0.001f64..1_000.0f64).prop_filter("positive and finite", |f| f.is_finite() && *f > 0.0)
}

/// A structurally consistent [`VoxelDocument`]: a sorted, unique, single-glyph palette; `size`
/// dimensions in `1..=3`; an optional positive `scale`; an optional `generated_from` path and
/// `budget_exempt: true`; and slices whose layers/rows/columns match `size`, drawn only from the
/// chosen glyphs plus the empty `.` — exactly the properties [`validate_voxel`] enforces.
fn voxel_doc() -> impl Strategy<Value = VoxelDocument> {
    (
        0u32..32,
        1u32..=3,
        1u32..=3,
        1u32..=3,
        proptest::option::of("[a-zA-Z0-9 ]{0,6}"),
        proptest::option::of(positive_f64()),
        // `generated_from` is a non-empty path; `budget_exempt` is only ever `true` (the sole valid
        // authored value), so the strategy never generates the `L1114` shape.
        proptest::option::of("[a-z][a-z0-9-]{0,8}[.]voxel[.]gen[.]mjs"),
        proptest::option::of(Just(true)),
    )
        .prop_flat_map(
            |(mask, sx, sy, sz, name, scale, generated_from, budget_exempt)| {
                let glyphs: Vec<char> = GLYPH_POOL
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, c)| *c)
                    .collect();
                let mut choices = glyphs.clone();
                choices.push('.');
                let colors = proptest::collection::vec(palette_entry_fields(), glyphs.len());
                let cells = proptest::collection::vec(
                    proptest::sample::select(choices),
                    (sx * sy * sz) as usize,
                );
                (
                    Just(glyphs),
                    Just([sx, sy, sz]),
                    Just(name),
                    Just(scale),
                    Just(generated_from),
                    Just(budget_exempt),
                    colors,
                    cells,
                )
            },
        )
        .prop_map(
            |(glyphs, size, name, scale, generated_from, budget_exempt, colors, cells)| {
                let palette = glyphs
                    .iter()
                    .zip(colors)
                    .map(|(g, (color, emissive, metallic, roughness))| PaletteEntry {
                        glyph: g.to_string(),
                        color,
                        emissive,
                        metallic,
                        porosity: None,
                        roughness,
                    })
                    .collect();
                let [sx, sy, sz] = size;
                let mut cells = cells.into_iter();
                let mut slices = Vec::with_capacity(sy as usize);
                for _ in 0..sy {
                    let mut layer = Vec::with_capacity(sz as usize);
                    for _ in 0..sz {
                        let row: String = (0..sx).map(|_| cells.next().unwrap()).collect();
                        layer.push(row);
                    }
                    slices.push(layer);
                }
                VoxelDocument {
                    name,
                    size,
                    scale,
                    generated_from,
                    budget_exempt,
                    palette,
                    slices,
                    rim: None,
                    lods: Vec::new(),
                }
            },
        )
}

// --- Properties -------------------------------------------------------------------------------

proptest! {
    #![proptest_config(deterministic_config())]
    #[test]
    fn emit_then_check_loses_nothing(doc in voxel_doc()) {
        let text = emit_voxel(&doc);
        let checked = check_voxel(&text).expect("canonical output must check clean");
        prop_assert_eq!(checked, doc);
    }

    #[test]
    fn emit_is_idempotent_byte_for_byte(doc in voxel_doc()) {
        let first = emit_voxel(&doc);
        let reparsed = parse_voxel(&first).expect("canonical output must parse cleanly");
        let second = emit_voxel(&reparsed);
        prop_assert_eq!(&second, &first);
        let third = emit_voxel(&parse_voxel(&second).unwrap());
        prop_assert_eq!(&third, &first);
    }

    #[test]
    fn emitted_text_is_lf_only_and_validates(doc in voxel_doc()) {
        let text = emit_voxel(&doc);
        prop_assert!(!text.contains('\r'), "canonical output must use LF only");
        prop_assert!(text.ends_with('\n'), "canonical output ends with a newline");
        let parsed = parse_voxel(&text).expect("canonical output must parse cleanly");
        let diagnostics = validate_voxel(&parsed);
        prop_assert!(
            diagnostics.is_empty(),
            "canonical output of a consistent model must validate: {:?}",
            diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }
}

// --- Deterministic golden shape ---------------------------------------------------------------

/// One non-property test pinning the exact canonical text: field order is
/// `format, kind, name, size, scale, palette, slices`; palette sorted by glyph; colors and the
/// declared `scale` are ryu-shortest; rows follow the shared `scalar_string` quoting rule (`AB`
/// stays plain, `.A` is quoted because it opens with a non-identifier character).
#[test]
fn canonical_form_golden() {
    let source = "\
format: 1
kind: voxel
name: gem
size:
  - 2
  - 2
  - 1
scale: 0.35
palette:
  A:
    color:
      - 0.8
      - 0.1
      - 0.1
      - 1.0
  B:
    color:
      - 0.1
      - 0.1
      - 0.9
      - 1.0
slices:
  -
    - \"AB\"
  -
    - \".A\"
";
    let model = parse_voxel(source).expect("model should parse");
    assert!(validate_voxel(&model).is_empty());
    assert_eq!(model.size, [2, 2, 1]);
    assert_eq!(model.scale, Some(0.35));
    assert_eq!(model.filled_count(), 3);
    let canonical = emit_voxel(&model);
    assert_eq!(canonical, source);
    // Emission is a fixed point.
    assert_eq!(emit_voxel(&parse_voxel(&canonical).unwrap()), canonical);
}

/// A palette entry's material scalars emit after `color`, alphabetically (`emissive`, `metallic`,
/// `roughness`), only when authored — an entry with no material keys emits exactly as
/// [`canonical_form_golden`], so old files stay byte-stable. This model also pins the **absent**
/// `scale`: a model declaring no intended scale emits no `scale:` line at all.
#[test]
fn canonical_form_golden_with_material_fields() {
    let source = "\
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
      - 0.0
      - 1.0
    emissive: 2.0
    metallic: 0.5
    roughness: 0.25
slices:
  -
    - \"A\"
";
    let model = parse_voxel(source).expect("model should parse");
    assert!(validate_voxel(&model).is_empty());
    assert_eq!(model.scale, None);
    assert_eq!(model.palette[0].emissive, Some(2.0));
    assert_eq!(model.palette[0].metallic, Some(0.5));
    assert_eq!(model.palette[0].roughness, Some(0.25));
    let canonical = emit_voxel(&model);
    assert_eq!(canonical, source);
    // Emission is a fixed point.
    assert_eq!(emit_voxel(&parse_voxel(&canonical).unwrap()), canonical);
}

/// `generated_from` and `budget_exempt` round-trip in canonical position — between `scale` and
/// `palette`, in that order. A document that uses neither emits exactly as before (the two goldens
/// above pin that), so the keys are additive to every committed model.
#[test]
fn canonical_form_golden_with_provenance_keys() {
    let source = "\
format: 1
kind: voxel
name: colossus
size:
  - 2
  - 1
  - 1
scale: 0.5
generated_from: colossus.voxel.gen.mjs
budget_exempt: true
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
";
    let model = parse_voxel(source).expect("model should parse");
    assert!(validate_voxel(&model).is_empty());
    assert_eq!(
        model.generated_from.as_deref(),
        Some("colossus.voxel.gen.mjs")
    );
    assert_eq!(model.budget_exempt, Some(true));
    let canonical = emit_voxel(&model);
    assert_eq!(canonical, source);
    assert_eq!(emit_voxel(&parse_voxel(&canonical).unwrap()), canonical);
}

/// `budget_exempt: true` silences `L1115` for a model that is deliberately over the cell budget —
/// the explicit opt-out. Every other model keeps the warning.
#[test]
fn budget_exempt_suppresses_the_cell_budget_warning() {
    let (sx, sy, sz) = (65u32, 65u32, 65u32);
    let layer = vec![".".repeat(sx as usize); sz as usize];
    let mut doc = VoxelDocument {
        name: None,
        size: [sx, sy, sz],
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette: Vec::new(),
        slices: vec![layer; sy as usize],
        rim: None,
        lods: Vec::new(),
    };
    assert!(voxel_budget_warning(&doc).is_some());
    doc.budget_exempt = Some(true);
    assert!(voxel_budget_warning(&doc).is_none());
    assert!(validate_voxel(&doc).is_empty());
}

/// `budget_exempt: false` is an `L1114` error rather than a silently accepted no-op, and its help
/// says to remove the key.
#[test]
fn budget_exempt_false_is_l1114() {
    let source = "\
format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
budget_exempt: false
palette: {}
slices:
  -
    - \".\"
";
    let diagnostics = errors_for(source);
    assert!(has_l1114_at(&diagnostics, "/budget_exempt"));
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/budget_exempt"))
        .expect("a /budget_exempt diagnostic");
    assert!(
        d.help
            .as_deref()
            .is_some_and(|help| help.contains("remove")),
        "help should say to remove the key: {:?}",
        d.help
    );
}

/// A non-boolean `budget_exempt` and an empty/non-string `generated_from` are `L1113` structure
/// errors, each with its own pointer.
#[test]
fn malformed_provenance_keys_are_l1113() {
    let head = "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\n";
    let tail = "palette: {}\nslices:\n  -\n    - \".\"\n";

    let diagnostics = errors_for(&format!("{head}budget_exempt: yes\n{tail}"));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1113" && d.path.as_deref() == Some("/budget_exempt"))
    );

    let diagnostics = errors_for(&format!("{head}generated_from: \"\"\n{tail}"));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1113" && d.path.as_deref() == Some("/generated_from"))
    );

    let diagnostics = errors_for(&format!("{head}generated_from: 3\n{tail}"));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1113" && d.path.as_deref() == Some("/generated_from"))
    );
}

/// A model with an empty palette and empty slices emits the minimal flow-collection shape.
#[test]
fn minimal_model_uses_flow_empties() {
    let doc = VoxelDocument {
        name: None,
        size: [1, 1, 1],
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette: Vec::new(),
        slices: Vec::new(),
        rim: None,
        lods: Vec::new(),
    };
    assert_eq!(
        emit_voxel(&doc),
        "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\npalette: {}\nslices: []\n"
    );
}

// --- L1113 structure (palette-entry material scalars) ------------------------------------------

fn errors_for(source: &str) -> Vec<ludion_core::diagnostics::Diagnostic> {
    check_voxel(source).expect_err("expected the model to be rejected")
}

fn has_l1114_at(diagnostics: &[ludion_core::diagnostics::Diagnostic], pointer: &str) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code.as_str() == "L1114" && d.path.as_deref() == Some(pointer))
}

#[test]
fn non_number_material_scalar_is_l1113() {
    let source = "\
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
      - 1.0
      - 1.0
      - 1.0
    metallic: \"high\"
slices:
  -
    - A
";
    let diagnostics = errors_for(source);
    assert!(diagnostics.iter().any(
        |d| d.code.as_str() == "L1113" && d.path.as_deref() == Some("/palette/A/metallic")
    ));
}

#[test]
fn unknown_palette_entry_key_suggests_nearest() {
    let source = "\
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
      - 1.0
      - 1.0
      - 1.0
    metalic: 0.5
slices:
  -
    - A
";
    let diagnostics = errors_for(source);
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/palette/A/metalic"))
        .expect("unknown key diagnostic");
    assert_eq!(d.code.as_str(), "L1113");
    assert_eq!(d.help.as_deref(), Some("nearest match is `metallic`"));
}

// --- L1114 misshapes (each with a JSON Pointer assertion) -------------------------------------

#[test]
fn ragged_row_is_l1114() {
    // A 2×1×2 model whose second row is only one character wide.
    let source = "\
format: 1
kind: voxel
size:
  - 2
  - 1
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
    - AA
    - A
";
    assert!(has_l1114_at(&errors_for(source), "/slices/0/1"));
}

#[test]
fn wrong_layer_count_is_l1114() {
    // size.y = 2 but only one layer is supplied.
    let source = "\
format: 1
kind: voxel
size:
  - 1
  - 2
  - 1
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
slices:
  -
    - A
";
    assert!(has_l1114_at(&errors_for(source), "/slices"));
}

#[test]
fn wrong_row_count_is_l1114() {
    // size.z = 2 but the layer has a single row.
    let source = "\
format: 1
kind: voxel
size:
  - 1
  - 1
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
    - A
";
    assert!(has_l1114_at(&errors_for(source), "/slices/0"));
}

#[test]
fn wrong_row_width_is_l1114() {
    // size.x = 3 but the row is two characters wide.
    let source = "\
format: 1
kind: voxel
size:
  - 3
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
slices:
  -
    - AA
";
    assert!(has_l1114_at(&errors_for(source), "/slices/0/0"));
}

#[test]
fn unknown_glyph_is_l1114() {
    let source = "\
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
      - 1.0
      - 1.0
      - 1.0
slices:
  -
    - Z
";
    assert!(has_l1114_at(&errors_for(source), "/slices/0/0"));
}

#[test]
fn multi_char_palette_key_is_l1114() {
    let source = "\
format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
palette:
  AA:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
slices:
  -
    - \".\"
";
    assert!(has_l1114_at(&errors_for(source), "/palette/AA"));
}

#[test]
fn non_ascii_palette_key_is_l1114() {
    let source = "\
format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
palette:
  \"é\":
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
slices:
  -
    - \".\"
";
    assert!(has_l1114_at(&errors_for(source), "/palette/é"));
}

#[test]
fn reserved_dot_palette_key_is_l1114() {
    let source = "\
format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
palette:
  \".\":
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
slices:
  -
    - \".\"
";
    assert!(has_l1114_at(&errors_for(source), "/palette/."));
}

#[test]
fn duplicate_palette_glyph_is_l1114() {
    // YAML rejects exact-duplicate keys (L1007), so the duplicate is reached by constructing the
    // document directly — validate is independent of how the document was built.
    let doc = VoxelDocument {
        name: None,
        size: [1, 1, 1],
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette: vec![
            PaletteEntry {
                glyph: "A".into(),
                color: [1.0; 4],
                emissive: None,
                metallic: None,
                porosity: None,
                roughness: None,
            },
            PaletteEntry {
                glyph: "A".into(),
                color: [1.0; 4],
                emissive: None,
                metallic: None,
                porosity: None,
                roughness: None,
            },
        ],
        slices: vec![vec![".".into()]],
        rim: None,
        lods: Vec::new(),
    };
    assert!(has_l1114_at(&validate_voxel(&doc), "/palette/A"));
}

#[test]
fn non_finite_color_is_l1114() {
    // `.nan` is rejected at the YAML layer (L1009), so a non-finite color is reached by direct
    // construction — the voxel analog of the mesh non-finite-float test.
    let doc = VoxelDocument {
        name: None,
        size: [1, 1, 1],
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette: vec![PaletteEntry {
            glyph: "A".into(),
            color: [f64::NAN, 1.0, 1.0, 1.0],
            emissive: None,
            metallic: None,
            porosity: None,
            roughness: None,
        }],
        slices: vec![vec!["A".into()]],
        rim: None,
        lods: Vec::new(),
    };
    assert!(has_l1114_at(&validate_voxel(&doc), "/palette/A/color/0"));
}

#[test]
fn out_of_range_metallic_is_l1114() {
    let source = "\
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
      - 1.0
      - 1.0
      - 1.0
    metallic: 1.5
slices:
  -
    - A
";
    assert!(has_l1114_at(&errors_for(source), "/palette/A/metallic"));
}

#[test]
fn out_of_range_roughness_is_l1114() {
    let source = "\
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
      - 1.0
      - 1.0
      - 1.0
    roughness: -0.1
slices:
  -
    - A
";
    assert!(has_l1114_at(&errors_for(source), "/palette/A/roughness"));
}

#[test]
fn emissive_past_the_f32_range_is_l1114() {
    let source = "format: 1
kind: voxel
size:
  - 1
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
    emissive: 1e39
slices:
  -
    - A
";
    assert!(has_l1114_at(&errors_for(source), "/palette/A/emissive"));
}

#[test]
fn negative_emissive_is_l1114() {
    let source = "\
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
      - 1.0
      - 1.0
      - 1.0
    emissive: -1.0
slices:
  -
    - A
";
    assert!(has_l1114_at(&errors_for(source), "/palette/A/emissive"));
}

#[test]
fn non_finite_material_scalar_is_l1114() {
    // `.nan` is rejected at the YAML layer (L1009), so a non-finite material scalar is reached by
    // direct construction — the same pattern as `non_finite_color_is_l1114`.
    let doc = VoxelDocument {
        name: None,
        size: [1, 1, 1],
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette: vec![PaletteEntry {
            glyph: "A".into(),
            color: [1.0; 4],
            emissive: Some(f64::NAN),
            metallic: Some(f64::INFINITY),
            porosity: None,
            roughness: Some(f64::NEG_INFINITY),
        }],
        slices: vec![vec!["A".into()]],
        rim: None,
        lods: Vec::new(),
    };
    let diagnostics = validate_voxel(&doc);
    assert!(has_l1114_at(&diagnostics, "/palette/A/emissive"));
    assert!(has_l1114_at(&diagnostics, "/palette/A/metallic"));
    assert!(has_l1114_at(&diagnostics, "/palette/A/roughness"));
}

// --- L1115 cell budget ------------------------------------------------------------------------

/// `L1115` fires above the cell budget and is reported by [`voxel_budget_warning`] without
/// appearing in [`validate_voxel`]'s (fatal) diagnostics — `check_voxel` still succeeds.
#[test]
fn cell_budget_warning_does_not_fail_validation() {
    // A small, consistent, all-empty model just over 64³ = 262,144 cells.
    let (sx, sy, sz) = (65u32, 65u32, 65u32);
    let row = ".".repeat(sx as usize);
    let layer = vec![row; sz as usize];
    let doc = VoxelDocument {
        name: None,
        size: [sx, sy, sz],
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette: Vec::new(),
        slices: vec![layer; sy as usize],
        rim: None,
        lods: Vec::new(),
    };
    assert!(validate_voxel(&doc).is_empty());
    let warning = voxel_budget_warning(&doc).expect("expected a budget warning");
    assert_eq!(warning.code.as_str(), "L1115");

    let text = emit_voxel(&doc);
    let checked = check_voxel(&text).expect("over-budget models still check clean");
    assert!(voxel_budget_warning(&checked).is_some());

    // A 4³ = 64-cell model is well under budget.
    let small = VoxelDocument {
        name: None,
        size: [4, 4, 4],
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette: Vec::new(),
        slices: vec![vec!["....".into(); 4]; 4],
        rim: None,
        lods: Vec::new(),
    };
    assert!(voxel_budget_warning(&small).is_none());
}

// --- `lods` (the generated-at-load LOD chain, `L1395`) -----------------------------------------------

const LODS_BASE: &str = "format: 1\nkind: voxel\nsize:\n  - 2\n  - 1\n  - 1\n";
const LODS_TAIL: &str = "palette:\n  A:\n    color:\n      - 1.0\n      - 0.0\n      - 0.0\n      - 1.0\nslices:\n  -\n    - \"AA\"\n";

fn with_lods(block: &str) -> String {
    format!("{LODS_BASE}{block}{LODS_TAIL}")
}

#[test]
fn lods_round_trip_as_a_fixed_point() {
    let source = with_lods("lods:\n  - screen_size: 0.25\n  - screen_size: 0.08\n");
    let doc = check_voxel(&source).expect("a chain checks clean");
    assert_eq!(doc.lods.len(), 2);
    assert_eq!(doc.lods[1].screen_size, 0.08);
    let text = emit_voxel(&doc);
    assert_eq!(text, source, "the canonical spelling is the authored one");
    assert_eq!(emit_voxel(&check_voxel(&text).unwrap()), text);
}

#[test]
fn a_chainless_document_emits_no_lods_key() {
    let doc = check_voxel(&with_lods("")).unwrap();
    assert!(doc.lods.is_empty());
    assert!(!emit_voxel(&doc).contains("lods"));
}

#[test]
fn a_malformed_chain_reports_l1395_with_a_pointer() {
    let cases: [(&str, &str); 5] = [
        (
            "lods:\n  - screen_size: 0.4\n  - screen_size: 0.3\n  - screen_size: 0.2\n  - screen_size: 0.1\n",
            "/lods",
        ),
        ("lods:\n  - screen_size: 0\n", "/lods/0/screen_size"),
        (
            "lods:\n  - screen_size: 0.2\n  - screen_size: 0.2\n",
            "/lods/1/screen_size",
        ),
        (
            "lods:\n  - screen_size: 0.2\n    mesh: a.mesh.yaml\n",
            "/lods/0/mesh",
        ),
        ("lods:\n  - {}\n", "/lods/0"),
    ];
    for (block, pointer) in cases {
        let diagnostics = check_voxel(&with_lods(block)).expect_err(block);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code.as_str() == "L1395" && d.path.as_deref() == Some(pointer)),
            "{block:?} should report L1395 at {pointer}, got {diagnostics:?}"
        );
    }
}

#[test]
fn a_non_finite_threshold_is_l1395() {
    // The YAML layer refuses `.nan`/`.inf` text, so only a built document reaches the validator.
    let mut doc = check_voxel(&with_lods("")).unwrap();
    for bad in [f64::NAN, f64::INFINITY] {
        doc.lods = vec![ludion_scene::VoxelLod { screen_size: bad }];
        let diagnostics = validate_voxel(&doc);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code.as_str() == "L1395"
                    && d.path.as_deref() == Some("/lods/0/screen_size")),
            "{bad} should report L1395, got {diagnostics:?}"
        );
    }
}
