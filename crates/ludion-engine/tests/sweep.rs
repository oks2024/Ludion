//! Integration tests for the sweep harness ([`render_sweep_sheet`]).
//!
//! The CPU-only tests (bad pointers, ill-typed values) always run and prove the harness reports its
//! diagnostics without an adapter. The GPU tests skip gracefully when none is available (an `L2003`
//! in the error list), matching `gpu_render.rs`.

use std::path::PathBuf;

use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{RenderScale, SweepOutcome, render_sweep_sheet};
use ludion_scene::SceneOverride;
use serde_json::{Value, json};

/// The gallery scene the post arc sweeps: it carries a `PostProcess` with both bloom knobs.
const BLOOM_STRENGTH: &str = "/entities/0/components/PostProcess/bloom_strength";

fn gallery(scene: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery")
        .join(scene)
}

fn no_adapter(errors: &[Diagnostic]) -> bool {
    ludion_core::require_gpu::is_no_adapter(errors)
}

fn values(raw: &[f64]) -> Vec<Value> {
    raw.iter().map(|v| json!(v)).collect()
}

/// Renders a sweep, or `None` (after printing a skip reason) when there is no GPU adapter.
fn sweep_or_skip(
    test: &str,
    scene: &str,
    base: &[SceneOverride],
    pointer: &str,
    candidates: &[Value],
    w: u32,
    h: u32,
) -> Option<SweepOutcome> {
    match render_sweep_sheet(
        &gallery(scene),
        base,
        pointer,
        candidates,
        w,
        h,
        RenderScale::NATIVE,
    ) {
        Ok(outcome) => {
            eprintln!("[{test}] {scene} on {}", outcome.adapter);
            Some(outcome)
        }
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[{test}] SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("[{test}] sweep failed: {errors:?}"),
    }
}

// --- CPU-only tests (always run) --------------------------------------------------------------

/// A swept pointer that does not resolve fails with `L4008` — no adapter involved. A misspelled
/// *component* is the case that stays unresolvable; a misspelled final field is created and then
/// caught by the schema (see `a_misspelled_swept_field_is_the_schemas_l1014_without_a_gpu`).
#[test]
fn an_unresolvable_swept_pointer_is_l4008_without_a_gpu() {
    let errors = render_sweep_sheet(
        &gallery("night-sky.yaml"),
        &[],
        "/entities/0/components/PostProces/bloom_strength",
        &values(&[0.0, 0.8]),
        64,
        64,
        RenderScale::NATIVE,
    )
    .unwrap_err();
    assert!(
        errors.iter().any(|d| d.code.as_str() == "L4008"),
        "expected an L4008 for the misspelled pointer: {errors:?}",
    );
    assert!(
        !no_adapter(&errors),
        "CPU validation must fail before the adapter is requested: {errors:?}",
    );
    let help = errors[0].help.as_deref().unwrap_or_default();
    assert!(
        help.contains("PostProcess"),
        "expected a nearest-match suggestion: {help}",
    );
}

/// The instrument's point: a field the scene left at its default is sweepable, so the *only* thing
/// standing between a typo and a rendered sheet is the schema — and it holds. `L1014` at the exact
/// pointer, naming the nearest declared field, before any adapter is requested.
#[test]
fn a_misspelled_swept_field_is_the_schemas_l1014_without_a_gpu() {
    let errors = render_sweep_sheet(
        &gallery("night-sky.yaml"),
        &[],
        "/entities/0/components/PostProcess/bloom_strenth",
        &values(&[0.0, 0.8]),
        64,
        64,
        RenderScale::NATIVE,
    )
    .unwrap_err();
    assert!(
        errors.iter().all(|d| d.code.as_str() == "L1014"),
        "a misspelled field is the schema's business alone: {errors:?}",
    );
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/PostProcess/bloom_strenth"),
    );
    assert!(
        errors[0]
            .help
            .as_deref()
            .unwrap_or_default()
            .contains("bloom_strength"),
        "expected a nearest-match suggestion: {errors:?}",
    );
}

/// An unresolvable *fixed* override fails the same way, before any cell renders.
#[test]
fn an_unresolvable_base_override_is_l4008_without_a_gpu() {
    let base = [SceneOverride::parse("/entities/0/components/Nope/x=1").unwrap()];
    let errors = render_sweep_sheet(
        &gallery("night-sky.yaml"),
        &base,
        BLOOM_STRENGTH,
        &values(&[0.0, 0.8]),
        64,
        64,
        RenderScale::NATIVE,
    )
    .unwrap_err();
    assert!(
        errors.iter().any(|d| d.code.as_str() == "L4008"),
        "expected an L4008 for the unknown component: {errors:?}",
    );
    assert!(!no_adapter(&errors), "no adapter should be requested");
}

/// A swept value the component schema rejects comes back as the ordinary `L1014` — the same
/// diagnostic the same edit to the file would produce, not a second rejection path.
#[test]
fn an_ill_typed_swept_value_is_the_ordinary_schema_error() {
    let errors = render_sweep_sheet(
        &gallery("night-sky.yaml"),
        &[],
        BLOOM_STRENGTH,
        &[json!(0.4), json!("loud")],
        64,
        64,
        RenderScale::NATIVE,
    )
    .unwrap_err();
    assert!(
        errors.iter().any(|d| d.code.as_str() == "L1014"),
        "expected the schema's L1014 for a string in a numeric field: {errors:?}",
    );
    assert!(!no_adapter(&errors), "no adapter should be requested");
}

/// A missing scene file is `L1015`, reported once rather than once per cell.
#[test]
fn a_missing_scene_is_l1015() {
    let errors = render_sweep_sheet(
        &gallery("no-such-scene.yaml"),
        &[],
        BLOOM_STRENGTH,
        &values(&[0.0, 0.8]),
        64,
        64,
        RenderScale::NATIVE,
    )
    .unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code.as_str(), "L1015");
}

// --- GPU tests (skip without an adapter) ------------------------------------------------------

/// Extracts cell `(row, col)`'s pixels from a `sheet_w`-wide RGBA8 sheet of `cell_w`×`cell_h` cells.
fn crop_cell(rgba: &[u8], sheet_w: u32, cell_w: u32, cell_h: u32, row: u32, col: u32) -> Vec<u8> {
    let stride = (sheet_w * 4) as usize;
    let row_bytes = (cell_w * 4) as usize;
    let x0 = (col * cell_w * 4) as usize;
    let mut out = Vec::with_capacity(row_bytes * cell_h as usize);
    for r in 0..cell_h as usize {
        let y = (row * cell_h) as usize + r;
        out.extend_from_slice(&rgba[y * stride + x0..y * stride + x0 + row_bytes]);
    }
    out
}

/// A four-value bloom sweep produces a 2×2 sheet of the requested size, one labelled cell per value,
/// and the cells differ — the sweep really moved the knob.
#[test]
fn needs_gpu_a_bloom_sweep_produces_one_distinct_cell_per_value() {
    let candidates = values(&[0.0, 0.12, 0.4, 1.0]);
    let Some(outcome) = sweep_or_skip(
        "bloom_sweep",
        "night-sky.yaml",
        &[],
        BLOOM_STRENGTH,
        &candidates,
        640,
        480,
    ) else {
        return;
    };

    assert_eq!((outcome.cols, outcome.rows), (2, 2));
    assert_eq!((outcome.image.width, outcome.image.height), (640, 480));
    assert_eq!(outcome.cells.len(), 4);
    assert_eq!(outcome.pointer, BLOOM_STRENGTH);
    assert_eq!(
        outcome.original,
        Some(json!(0.12)),
        "the file's own value is echoed back",
    );
    for (index, cell) in outcome.cells.iter().enumerate() {
        assert_eq!(cell.value, candidates[index]);
        assert!(
            cell.label.ends_with("\nbloom_strength"),
            "cell {index} label: {}",
            cell.label,
        );
        assert_eq!((cell.row, cell.col), (index as u32 / 2, index as u32 % 2));
    }

    let (cw, ch) = (320, 240);
    let quadrants: Vec<Vec<u8>> = outcome
        .cells
        .iter()
        .map(|cell| crop_cell(&outcome.image.rgba, 640, cw, ch, cell.row, cell.col))
        .collect();
    for (index, quadrant) in quadrants.iter().enumerate() {
        let first = &quadrant[0..4];
        assert!(
            quadrant.chunks_exact(4).any(|px| px != first),
            "cell {index} is a flat single color",
        );
    }
    for a in 0..quadrants.len() {
        for b in (a + 1)..quadrants.len() {
            assert_ne!(
                quadrants[a], quadrants[b],
                "cells {a} and {b} are identical"
            );
        }
    }
}

/// A sweep over a grade knob **no scene authors**: `night-sky.yaml` spells only the two bloom
/// scalars, so every cell here rides the created-field path, which is the whole reason the grade's
/// fields are flat on the component rather than nested under a `grade:` map. The echoed original is
/// `None` — the file never mentioned the field — and the cells still differ, so the created value
/// really reached the composite pass.
#[test]
fn needs_gpu_a_grade_sweep_works_on_a_field_no_scene_authors() {
    const SATURATION: &str = "/entities/0/components/PostProcess/saturation";
    let candidates = values(&[0.0, 1.0, 2.0]);
    let Some(outcome) = sweep_or_skip(
        "grade_sweep",
        "night-sky.yaml",
        &[],
        SATURATION,
        &candidates,
        320,
        240,
    ) else {
        return;
    };

    assert_eq!(outcome.original, None, "the file never authored saturation");
    assert_eq!(outcome.cells.len(), 3);
    let cells: Vec<Vec<u8>> = outcome
        .cells
        .iter()
        .map(|cell| crop_cell(&outcome.image.rgba, 320, 160, 120, cell.row, cell.col))
        .collect();
    for a in 0..cells.len() {
        for b in (a + 1)..cells.len() {
            assert_ne!(cells[a], cells[b], "cells {a} and {b} are identical");
        }
    }
}

/// A sweep over a **fog** knob on the one gallery scene with a real depth range (`post-full.yaml`
/// puts a crate at 12 units and a skyline at 84).
///
/// This is the tuning-loop proof for the first effect whose value depends on scene depth: the sheet
/// that picked the committed `fog_density` is one command away from the committed scene, and the
/// echoed original is the file's own number, so a later re-tune starts from what is authored rather
/// than from a default. Zero cells at `0.0` would be a sweep of nothing, so the list starts there
/// deliberately: cell 0 is the unfogged reference the rest are judged against.
///
/// The created-field half of the override contract is proved on `night-sky.yaml` instead
/// (`a_grade_sweep_works_on_a_field_no_scene_authors`) — `post-full.yaml` now authors the whole
/// `PostProcess` roster, so no field of it is left unspelled to create.
#[test]
fn needs_gpu_a_fog_sweep_retunes_a_field_the_scene_authors() {
    const FOG_DENSITY: &str = "/entities/0/components/PostProcess/fog_density";
    let candidates = values(&[0.0, 0.01, 0.04]);
    let Some(outcome) = sweep_or_skip(
        "fog_sweep",
        "post-full.yaml",
        &[],
        FOG_DENSITY,
        &candidates,
        320,
        240,
    ) else {
        return;
    };

    assert_eq!(
        outcome.original,
        Some(json!(0.014)),
        "the file's own fog density is echoed back",
    );
    assert_eq!(outcome.cells.len(), 3);
    let cells: Vec<Vec<u8>> = outcome
        .cells
        .iter()
        .map(|cell| crop_cell(&outcome.image.rgba, 320, 160, 120, cell.row, cell.col))
        .collect();
    for a in 0..cells.len() {
        for b in (a + 1)..cells.len() {
            assert_ne!(cells[a], cells[b], "cells {a} and {b} are identical");
        }
    }
}

/// Determinism: the same sweep twice yields byte-identical images and PNG bytes.
#[test]
fn needs_gpu_the_same_sweep_twice_is_byte_identical() {
    let candidates = values(&[0.0, 0.7, 1.6]);
    let Some(first) = sweep_or_skip(
        "determinism",
        "night-sky.yaml",
        &[],
        BLOOM_STRENGTH,
        &candidates,
        480,
        320,
    ) else {
        return;
    };
    let second = render_sweep_sheet(
        &gallery("night-sky.yaml"),
        &[],
        BLOOM_STRENGTH,
        &candidates,
        480,
        320,
        RenderScale::NATIVE,
    )
    .unwrap();

    assert_eq!(
        first.image, second.image,
        "two renders of the same sweep must be byte-identical",
    );
    assert_eq!(
        first.image.encode_png().unwrap(),
        second.image.encode_png().unwrap(),
        "the PNG bytes must be byte-identical too",
    );
    // N=3 leaves the fourth quadrant on the unused fill, which must be identical too.
    assert_eq!((first.cols, first.rows), (2, 2));
}

/// The fixed `--set` overrides compose with the sweep: changing the *threshold* under a sweep of
/// *strength* moves every cell.
#[test]
fn needs_gpu_fixed_overrides_apply_to_every_cell() {
    let candidates = values(&[0.0, 0.7]);
    let Some(plain) = sweep_or_skip(
        "compose",
        "night-sky.yaml",
        &[],
        BLOOM_STRENGTH,
        &candidates,
        320,
        160,
    ) else {
        return;
    };
    let base = [
        SceneOverride::parse("/entities/0/components/PostProcess/bloom_threshold=0.02").unwrap(),
    ];
    let lowered = render_sweep_sheet(
        &gallery("night-sky.yaml"),
        &base,
        BLOOM_STRENGTH,
        &candidates,
        320,
        160,
        RenderScale::NATIVE,
    )
    .unwrap();

    assert_eq!(lowered.applied_base.len(), 1);
    assert_eq!(lowered.applied_base[0].from, Some(json!(1.0)));
    assert_eq!(lowered.applied_base[0].to, json!(0.02));
    assert_ne!(
        plain.image.rgba, lowered.image.rgba,
        "a fixed override must reach every cell",
    );
}
