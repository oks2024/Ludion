//! Round-trip and canonical-form properties for assembly documents (`*.assembly.yaml`,
//! `Plan/voxel-animation.md` decisions 3, 4 and 9) — the assembly analog of `voxel_roundtrip.rs`:
//!
//! - **value round trip**: doc → emit → check loses nothing;
//! - **emit idempotence**: `emit_assembly(parse_assembly(emit_assembly(d)))` is *byte-equal* to
//!   `emit_assembly(d)`;
//! - the emitted text is LF-only and validates cleanly;
//! - a hand-authored fixture pins the exact canonical text (the plan's `player.assembly.yaml`
//!   sketch, translated into the strict subset's block form);
//! - every `L1137`/`L1138` misshape is reported with a JSON Pointer, the cross-file ones included;
//! - the composition helpers (`assembly_part_voxel`, `assembly_parent_indices`) do what the
//!   hierarchy says.

use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{
    AssemblyClip, AssemblyDocument, AssemblyJoint, AssemblyJointKind, AssemblyPartEntry,
    PaletteEntry, PartDocument, assembly_assembled_grid, assembly_overlap_lint,
    assembly_parent_indices, assembly_part_voxel, check_assembly, check_assembly_parts,
    emit_assembly, parse_assembly, validate_assembly,
};
use proptest::prelude::*;
#[path = "support/deterministic.rs"]
mod deterministic;
use deterministic::deterministic_config;

// --- Strategies -------------------------------------------------------------------------------

/// The glyph pool, in ascending char order (so a subset taken in order is already sorted — the
/// canonical palette order the emitter writes).
const GLYPH_POOL: [char; 3] = ['#', 'A', 'B'];

/// Finite once narrowed to `f32`, the range the kind readers accept.
fn finite_f64() -> impl Strategy<Value = f64> {
    any::<f64>().prop_filter("finite as f32", |f| (*f as f32).is_finite())
}

fn color() -> impl Strategy<Value = [f64; 4]> {
    (finite_f64(), finite_f64(), finite_f64(), finite_f64()).prop_map(|(r, g, b, a)| [r, g, b, a])
}

/// A non-empty palette of unique, sorted, single-glyph entries with optional material scalars —
/// exactly a voxel document's palette, which is what an assembly's is.
fn palette() -> impl Strategy<Value = Vec<PaletteEntry>> {
    (1u32..8u32)
        .prop_flat_map(|mask| {
            let glyphs: Vec<char> = GLYPH_POOL
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, glyph)| *glyph)
                .collect();
            let fields = proptest::collection::vec(
                (
                    color(),
                    proptest::option::of(0.0f64..1_000.0f64),
                    proptest::option::of(0.0f64..=1.0f64),
                    proptest::option::of(0.0f64..=1.0f64),
                ),
                glyphs.len(),
            );
            (Just(glyphs), fields)
        })
        .prop_map(|(glyphs, fields)| {
            glyphs
                .iter()
                .zip(fields)
                .map(
                    |(glyph, (color, emissive, metallic, roughness))| PaletteEntry {
                        glyph: glyph.to_string(),
                        color,
                        emissive,
                        metallic,
                        roughness,
                    },
                )
                .collect()
        })
}

/// A [`validate_assembly`]-clean [`AssemblyDocument`]: a valid palette, `size` dimensions in
/// `4..=8`, one to four uniquely named parts placed inside the grid, part `i > 0` parented to some
/// part before it (so the hierarchy is a single-rooted tree by construction), half-step pivots, and
/// a deduplicated clip map.
fn assembly_doc() -> impl Strategy<Value = AssemblyDocument> {
    (
        proptest::option::of("[a-zA-Z0-9 ]{0,6}"),
        4u32..=8,
        4u32..=8,
        4u32..=8,
        palette(),
        1usize..=4,
        proptest::collection::vec(proptest::sample::select(vec!["attack", "walk"]), 0..=2),
    )
        .prop_flat_map(|(name, sx, sy, sz, palette, count, clip_names)| {
            let entries = proptest::collection::vec(
                (
                    (0..sx, 0..sy, 0..sz),
                    // Halves, never `-0.0`: a negative zero would emit as `-0.0` yet compare equal
                    // to the origin pivot the writer omits, which is a strategy bug, not a format
                    // one.
                    (-16i32..=16, -16i32..=16, -16i32..=16),
                    any::<bool>(),
                    0usize..4,
                ),
                count,
            );
            (
                Just(name),
                Just([sx, sy, sz]),
                Just(palette),
                entries,
                Just(clip_names),
            )
        })
        .prop_map(|(name, size, palette, entries, clip_names)| {
            let parts = entries
                .iter()
                .enumerate()
                .map(
                    |(index, (place, pivot, mirror_x, parent))| AssemblyPartEntry {
                        name: format!("part-{index}"),
                        file: format!("parts/part-{index}.part.yaml"),
                        parent: (index > 0).then(|| format!("part-{}", parent % index)),
                        place: [place.0, place.1, place.2],
                        pivot: [
                            f64::from(pivot.0) / 2.0,
                            f64::from(pivot.1) / 2.0,
                            f64::from(pivot.2) / 2.0,
                        ],
                        mirror_x: *mirror_x,
                    },
                )
                .collect();
            let mut names = clip_names;
            names.sort_unstable();
            names.dedup();
            let clips = names
                .into_iter()
                .map(|name| AssemblyClip {
                    name: name.to_string(),
                    file: format!("anims/{name}.anim.yaml"),
                })
                .collect();
            AssemblyDocument {
                name,
                size,
                palette,
                parts,
                clips,
                joints: Vec::new(),
            }
        })
}

// --- Properties -------------------------------------------------------------------------------

proptest! {
    #![proptest_config(deterministic_config())]
    #[test]
    fn emit_then_check_loses_nothing(doc in assembly_doc()) {
        let text = emit_assembly(&doc);
        let checked = check_assembly(&text).expect("canonical output must check clean");
        prop_assert_eq!(checked, doc);
    }

    #[test]
    fn emit_is_idempotent_byte_for_byte(doc in assembly_doc()) {
        let first = emit_assembly(&doc);
        let reparsed = parse_assembly(&first).expect("canonical output must parse cleanly");
        let second = emit_assembly(&reparsed);
        prop_assert_eq!(&second, &first);
        let third = emit_assembly(&parse_assembly(&second).unwrap());
        prop_assert_eq!(&third, &first);
    }

    #[test]
    fn emitted_text_is_lf_only_and_validates(doc in assembly_doc()) {
        let text = emit_assembly(&doc);
        prop_assert!(!text.contains('\r'), "canonical output must use LF only");
        prop_assert!(text.ends_with('\n'), "canonical output ends with a newline");
        let parsed = parse_assembly(&text).expect("canonical output must parse cleanly");
        let diagnostics = validate_assembly(&parsed);
        prop_assert!(
            diagnostics.is_empty(),
            "canonical output of a consistent assembly must validate: {:?}",
            diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }
}

// --- Deterministic golden shape ---------------------------------------------------------------

/// `Plan/voxel-animation.md`'s `player.assembly.yaml` sketch, translated into the strict subset:
/// the sketch's flow collections (`place: [4, 10, 2]`, `S: { color: [...] }`) are `L1005` here, so
/// block form is the canonical spelling, and `0.30` canonicalizes to the shortest round-trip `0.3`.
const PLAYER: &str = "\
format: 1
kind: assembly
name: player
size:
  - 12
  - 24
  - 8
palette:
  S:
    color:
      - 0.3
      - 0.22
      - 0.18
      - 1.0
parts:
  - name: torso
    file: parts/torso.part.yaml
    place:
      - 4
      - 10
      - 2
  - name: upper-arm-l
    file: parts/arm.part.yaml
    parent: torso
    place:
      - 1
      - 14
      - 3
    pivot:
      - 1.0
      - 5.0
      - 1.0
  - name: upper-arm-r
    file: parts/arm.part.yaml
    parent: torso
    place:
      - 9
      - 14
      - 3
    pivot:
      - 1.0
      - 5.0
      - 1.0
    mirror: x
clips:
  attack: anims/player-slash.anim.yaml
  walk: anims/humanoid-walk.anim.yaml
";

#[test]
fn canonical_form_golden() {
    let doc = check_assembly(PLAYER).expect("the sketch translation is a valid assembly");
    assert_eq!(doc.size, [12, 24, 8]);
    assert_eq!(doc.parts.len(), 3);
    // Parts keep their authored order (the hierarchy reads top-down), unlike the sorted clips.
    let order: Vec<&str> = doc.parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(order, ["torso", "upper-arm-l", "upper-arm-r"]);
    assert_eq!(doc.parts[0].parent, None);
    assert_eq!(doc.parts[2].parent.as_deref(), Some("torso"));
    assert!(doc.parts[2].mirror_x);
    assert_eq!(doc.clips.len(), 2);
    let canonical = emit_assembly(&doc);
    assert_eq!(canonical, PLAYER);
    // Emission is a fixed point.
    assert_eq!(
        emit_assembly(&parse_assembly(&canonical).unwrap()),
        canonical
    );
}

/// Clips are written sorted by short name whatever order they were authored in.
#[test]
fn clips_emit_sorted_by_short_name() {
    let source = PLAYER.replace(
        "clips:\n  attack: anims/player-slash.anim.yaml\n  walk: anims/humanoid-walk.anim.yaml\n",
        "clips:\n  walk: anims/humanoid-walk.anim.yaml\n  attack: anims/player-slash.anim.yaml\n",
    );
    let doc = check_assembly(&source).expect("clip order is not a validity question");
    assert_eq!(emit_assembly(&doc), PLAYER);
}

/// The origin pivot, an unset mirror, and an empty clip map are each the *absence* of their key —
/// one spelling per meaning. An authored `clips: {}` reads exactly as no `clips` at all.
#[test]
fn defaults_canonicalize_to_absent_keys() {
    let source = "\
format: 1
kind: assembly
size:
  - 4
  - 4
  - 4
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
    pivot:
      - 0.0
      - 0.0
      - 0.0
clips: {}
";
    let doc = check_assembly(source).expect("an origin pivot and an empty clip map are valid");
    assert_eq!(doc.parts[0].pivot, [0.0, 0.0, 0.0]);
    assert!(doc.clips.is_empty());
    let canonical = emit_assembly(&doc);
    assert!(
        !canonical.contains("pivot"),
        "an origin pivot is the key's absence:\n{canonical}"
    );
    assert!(
        !canonical.contains("mirror"),
        "an unmirrored part is the key's absence:\n{canonical}"
    );
    assert!(
        !canonical.contains("clips"),
        "an empty clip map is the key's absence:\n{canonical}"
    );
    assert_eq!(
        emit_assembly(&parse_assembly(&canonical).unwrap()),
        canonical
    );
}

/// An assembly with no palette entries and no parts still emits a re-parsable document (the flow
/// empties `{}`/`[]`), even though [`validate_assembly`] rejects the empty `parts`.
#[test]
fn empty_collections_use_flow_empties() {
    let doc = AssemblyDocument {
        name: None,
        size: [1, 1, 1],
        palette: Vec::new(),
        parts: Vec::new(),
        clips: Vec::new(),
        joints: Vec::new(),
    };
    let text = emit_assembly(&doc);
    assert_eq!(
        text,
        "format: 1\nkind: assembly\nsize:\n  - 1\n  - 1\n  - 1\npalette: {}\nparts: []\n"
    );
    assert_eq!(parse_assembly(&text).expect("re-parses"), doc);
}

// --- L1137 structure --------------------------------------------------------------------------

fn errors_for(source: &str) -> Vec<Diagnostic> {
    check_assembly(source).expect_err("expected the assembly to be rejected")
}

fn has_code_at(diagnostics: &[Diagnostic], code: &str, pointer: &str) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code.as_str() == code && d.path.as_deref() == Some(pointer))
}

/// A minimal valid document, as a template every misshape test edits one line of.
const MINIMAL: &str = "\
format: 1
kind: assembly
size:
  - 4
  - 4
  - 4
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
";

#[test]
fn minimal_document_is_valid() {
    check_assembly(MINIMAL).expect("the template must be valid");
}

#[test]
fn empty_document_is_l1137() {
    let diagnostics = errors_for("");
    assert!(diagnostics.iter().any(|d| d.code.as_str() == "L1137"));
}

#[test]
fn wrong_kind_is_l1137() {
    let source = MINIMAL.replace("kind: assembly", "kind: voxel");
    assert!(has_code_at(&errors_for(&source), "L1137", "/kind"));
}

#[test]
fn wrong_format_version_is_l1137() {
    let source = MINIMAL.replace("format: 1", "format: 2");
    assert!(has_code_at(&errors_for(&source), "L1137", "/format"));
}

#[test]
fn unknown_top_level_key_is_l1137() {
    let source = MINIMAL.replace("size:", "slices: []\nsize:");
    assert!(has_code_at(&errors_for(&source), "L1137", "/slices"));
}

#[test]
fn missing_required_keys_are_l1137() {
    for missing in ["size", "palette", "parts"] {
        // Everything from the key onward is dropped, so the key itself is certainly absent.
        let source: String = MINIMAL
            .lines()
            .take_while(|line| !line.starts_with(missing))
            .map(|line| format!("{line}\n"))
            .collect();
        let diagnostics = errors_for(&source);
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1137"
                && d.path.as_deref() == Some("")
                && d.message.contains(missing)),
            "a missing `{missing}` must be an L1137 at the document root: {diagnostics:?}",
        );
    }
}

#[test]
fn unknown_part_entry_key_is_l1137() {
    let source = MINIMAL.replace(
        "    file: parts/body.part.yaml",
        "    fil: parts/body.part.yaml",
    );
    let diagnostics = errors_for(&source);
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/parts/0/fil"))
        .expect("the unknown key is pointed at");
    assert_eq!(diagnostic.code.as_str(), "L1137");
    assert_eq!(diagnostic.help.as_deref(), Some("nearest match is `file`"));
}

#[test]
fn wrong_size_arity_is_l1137() {
    let source = MINIMAL.replace("size:\n  - 4\n  - 4\n  - 4\n", "size:\n  - 4\n  - 4\n");
    assert!(has_code_at(&errors_for(&source), "L1137", "/size"));
}

#[test]
fn wrong_place_arity_is_l1137() {
    let source = MINIMAL.replace(
        "    place:\n      - 0\n      - 0\n      - 0\n",
        "    place:\n      - 0\n      - 0\n",
    );
    assert!(has_code_at(&errors_for(&source), "L1137", "/parts/0/place"));
}

#[test]
fn negative_place_component_is_l1137() {
    let source = MINIMAL.replace(
        "    place:\n      - 0\n      - 0\n      - 0\n",
        "    place:\n      - -1\n      - 0\n      - 0\n",
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1137",
        "/parts/0/place/0"
    ));
}

#[test]
fn wrong_pivot_arity_is_l1137() {
    let source = format!("{MINIMAL}    pivot:\n      - 1.0\n      - 2.0\n");
    assert!(has_code_at(&errors_for(&source), "L1137", "/parts/0/pivot"));
}

#[test]
fn non_numeric_pivot_component_is_l1137() {
    let source = format!("{MINIMAL}    pivot:\n      - middle\n      - 2.0\n      - 0.0\n");
    assert!(has_code_at(
        &errors_for(&source),
        "L1137",
        "/parts/0/pivot/0"
    ));
}

#[test]
fn unknown_mirror_axis_is_l1137() {
    let source = format!("{MINIMAL}    mirror: y\n");
    let diagnostics = errors_for(&source);
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/parts/0/mirror"))
        .expect("the mirror value is pointed at");
    assert_eq!(diagnostic.code.as_str(), "L1137");
    assert!(
        diagnostic
            .help
            .as_deref()
            .is_some_and(|h| h.contains("`x`")),
        "the help must name `x` as the only axis: {:?}",
        diagnostic.help,
    );
}

#[test]
fn non_string_mirror_is_l1137() {
    let source = format!("{MINIMAL}    mirror: true\n");
    assert!(has_code_at(
        &errors_for(&source),
        "L1137",
        "/parts/0/mirror"
    ));
}

/// A clip path that is not a string. (A non-string clip *name* cannot reach this module: a
/// non-string mapping key is `L1008` in the shared strict-YAML layer, before any family sees it.)
#[test]
fn non_string_clip_path_is_l1137() {
    let source = format!("{MINIMAL}clips:\n  walk: 3\n");
    assert!(has_code_at(&errors_for(&source), "L1137", "/clips/walk"));
}

/// [`MINIMAL`] with its `parts:` block cut off — the base every hierarchy test rebuilds on.
fn head() -> String {
    MINIMAL
        .split("parts:")
        .next()
        .expect("the template has a parts block")
        .to_string()
}

/// One part entry's canonical block, at the indentation a `parts:` sequence item takes.
fn part_entry(name: &str, parent: Option<&str>, place: [u32; 3]) -> String {
    let mut out = format!("  - name: {name}\n    file: parts/{name}.part.yaml\n");
    if let Some(parent) = parent {
        out.push_str(&format!("    parent: {parent}\n"));
    }
    out.push_str("    place:\n");
    for component in place {
        out.push_str(&format!("      - {component}\n"));
    }
    out
}

#[test]
fn non_mapping_part_entry_is_l1137() {
    let source = format!("{}parts:\n  - body\n", head());
    assert!(has_code_at(&errors_for(&source), "L1137", "/parts/0"));
}

#[test]
fn part_entry_missing_required_key_is_l1137() {
    let source = MINIMAL.replace("    file: parts/body.part.yaml\n", "");
    assert!(has_code_at(&errors_for(&source), "L1137", "/parts/0"));
}

// --- L1138 data consistency (each with a JSON Pointer assertion) -------------------------------

#[test]
fn empty_clip_name_or_path_is_l1138() {
    let empty_name = format!("{MINIMAL}clips:\n  \"\": anims/walk.anim.yaml\n");
    assert!(has_code_at(&errors_for(&empty_name), "L1138", "/clips/"));
    let empty_path = format!("{MINIMAL}clips:\n  walk: \"\"\n");
    assert!(has_code_at(
        &errors_for(&empty_path),
        "L1138",
        "/clips/walk"
    ));
}

#[test]
fn absolute_drive_or_device_sibling_paths_are_l1138_and_a_parent_is_fine() {
    let part = MINIMAL.replace("parts/body.part.yaml", "C:/parts/x.part.yaml");
    assert!(has_code_at(&errors_for(&part), "L1138", "/parts/0/file"));
    let part = MINIMAL.replace("parts/body.part.yaml", "parts/nul.part.yaml");
    assert!(has_code_at(&errors_for(&part), "L1138", "/parts/0/file"));
    let clip = format!(
        "{MINIMAL}clips:
  walk: /anims/walk.anim.yaml
"
    );
    assert!(has_code_at(&errors_for(&clip), "L1138", "/clips/walk"));
    let clip = format!(
        "{MINIMAL}clips:
  walk: ../nul.anim.yaml
"
    );
    assert!(has_code_at(&errors_for(&clip), "L1138", "/clips/walk"));
    let fine = MINIMAL.replace("parts/body.part.yaml", "../parts/x.part.yaml");
    check_assembly(&fine).expect("a `..` part file is legal");
}

#[test]
fn multi_character_palette_key_is_l1138() {
    let source = MINIMAL.replace("  A:\n", "  AB:\n");
    assert!(has_code_at(&errors_for(&source), "L1138", "/palette/AB"));
}

#[test]
fn reserved_palette_glyph_is_l1138() {
    let source = MINIMAL.replace("  A:\n", "  \".\":\n");
    assert!(has_code_at(&errors_for(&source), "L1138", "/palette/."));
}

#[test]
fn out_of_range_material_scalar_is_l1138() {
    let source = MINIMAL.replace(
        "      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\n",
        "      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\n    metallic: 4.0\n",
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1138",
        "/palette/A/metallic"
    ));
}

#[test]
fn non_finite_palette_color_is_l1138() {
    // `.inf` cannot be authored (the strict subset rejects it), so this is the programmatic path.
    let mut doc = check_assembly(MINIMAL).unwrap();
    doc.palette[0].color[2] = f64::NAN;
    assert!(has_code_at(
        &validate_assembly(&doc),
        "L1138",
        "/palette/A/color/2"
    ));
}

#[test]
fn a_size_past_the_cell_ceiling_is_l1138() {
    let source = MINIMAL.replace(
        "size:
  - 4
  - 4
  - 4
",
        "size:
  - 12
  - 24000000
  - 8
",
    );
    assert!(has_code_at(&errors_for(&source), "L1138", "/size"));
    let source = MINIMAL.replace(
        "size:
  - 4
  - 4
  - 4
",
        "size:
  - 4294967295
  - 4294967295
  - 4294967295
",
    );
    assert!(has_code_at(&errors_for(&source), "L1138", "/size"));
}

#[test]
fn empty_parts_is_l1138() {
    let source = format!("{}parts: []\n", head());
    assert!(has_code_at(&errors_for(&source), "L1138", "/parts"));
}

#[test]
fn duplicate_part_name_is_l1138() {
    let source = format!("{MINIMAL}{}", part_entry("body", Some("body"), [1, 0, 0]));
    assert!(has_code_at(&errors_for(&source), "L1138", "/parts/1/name"));
}

#[test]
fn empty_part_name_is_l1138() {
    let source = MINIMAL.replace("- name: body", "- name: \"\"");
    assert!(has_code_at(&errors_for(&source), "L1138", "/parts/0/name"));
}

#[test]
fn empty_part_file_is_l1138() {
    let source = MINIMAL.replace("    file: parts/body.part.yaml\n", "    file: \"\"\n");
    assert!(has_code_at(&errors_for(&source), "L1138", "/parts/0/file"));
}

#[test]
fn unknown_parent_is_l1138_with_suggestion() {
    let source = format!("{MINIMAL}{}", part_entry("arm", Some("bod"), [1, 0, 0]));
    let diagnostics = errors_for(&source);
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/parts/1/parent"))
        .expect("the parent is pointed at");
    assert_eq!(diagnostic.code.as_str(), "L1138");
    assert_eq!(diagnostic.help.as_deref(), Some("nearest match is `body`"));
}

#[test]
fn self_parenting_is_l1138() {
    let source = MINIMAL.replace(
        "    file: parts/body.part.yaml\n",
        "    file: parts/body.part.yaml\n    parent: body\n",
    );
    let diagnostics = errors_for(&source);
    assert!(has_code_at(&diagnostics, "L1138", "/parts/0/parent"));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("names itself")),
        "self-parenting gets its own message, not the generic cycle one: {diagnostics:?}"
    );
}

#[test]
fn no_root_is_l1138() {
    // Two parts, each the other's parent: no part omits `parent`, and the chain also loops.
    let source = format!(
        "{}parts:\n{}{}",
        head(),
        part_entry("body", Some("arm"), [0, 0, 0]),
        part_entry("arm", Some("body"), [1, 0, 0]),
    );
    let diagnostics = errors_for(&source);
    assert!(has_code_at(&diagnostics, "L1138", "/parts"));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("no part is the root")),
        "{diagnostics:?}"
    );
    // The same document names the loop, once, from its lowest member.
    let cycles: Vec<&Diagnostic> = diagnostics
        .iter()
        .filter(|d| d.message.contains("`parent` chain loops"))
        .collect();
    assert_eq!(cycles.len(), 1, "one cycle, one diagnostic: {cycles:?}");
    assert_eq!(cycles[0].path.as_deref(), Some("/parts/0/parent"));
    assert!(
        cycles[0].message.contains("`body` -> `arm` -> `body`"),
        "the cycle is named: {}",
        cycles[0].message
    );
}

#[test]
fn several_roots_is_l1138() {
    let source = format!("{MINIMAL}{}", part_entry("arm", None, [1, 0, 0]));
    let diagnostics = errors_for(&source);
    assert!(has_code_at(&diagnostics, "L1138", "/parts"));
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/parts"))
        .unwrap();
    assert!(
        diagnostic.message.contains("`body`") && diagnostic.message.contains("`arm`"),
        "the roots found are named: {}",
        diagnostic.message
    );
}

#[test]
fn non_finite_pivot_is_l1138() {
    let mut doc = check_assembly(MINIMAL).unwrap();
    doc.parts[0].pivot[1] = f64::INFINITY;
    assert!(has_code_at(
        &validate_assembly(&doc),
        "L1138",
        "/parts/0/pivot/1"
    ));
}

#[test]
fn place_outside_size_is_l1138() {
    let source = MINIMAL.replace(
        "    place:\n      - 0\n      - 0\n      - 0\n",
        "    place:\n      - 0\n      - 9\n      - 0\n",
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1138",
        "/parts/0/place/1"
    ));
}

// --- L1138 cross-file composition ---------------------------------------------------------------

/// A hand-built part of `size` filled with `glyph` (one layer per y, one row per z).
fn part(size: [u32; 3], glyph: char) -> PartDocument {
    let row: String = std::iter::repeat_n(glyph, size[0] as usize).collect();
    PartDocument {
        name: None,
        size,
        slices: vec![vec![row; size[2] as usize]; size[1] as usize],
    }
}

#[test]
fn part_glyph_outside_the_palette_is_l1138() {
    let doc = check_assembly(MINIMAL).unwrap();
    // `Q` names no entry in the assembly's palette — the decision-4 lint (parts have no palette).
    let diagnostics = check_assembly_parts(&doc, &[part([2, 2, 2], 'Q')]);
    assert!(has_code_at(&diagnostics, "L1138", "/parts/0/file"));
    assert_eq!(
        diagnostics.len(),
        1,
        "the glyph is reported once per part, not once per cell: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("`Q`")
            && diagnostics[0].message.contains("body")
            && diagnostics[0].message.contains("parts/body.part.yaml"),
        "the glyph, the part, and its file are all named: {}",
        diagnostics[0].message
    );
    // A glyph the palette does define is silent, and so is the reserved empty cell.
    let mut mixed = part([2, 2, 2], 'A');
    mixed.slices[0][0] = "A.".to_string();
    assert!(check_assembly_parts(&doc, &[mixed]).is_empty());
}

#[test]
fn part_overflowing_the_grid_is_l1138() {
    let doc = check_assembly(MINIMAL).unwrap(); // size 4x4x4, one part placed at the origin
    let diagnostics = check_assembly_parts(&doc, &[part([2, 5, 2], 'A')]);
    assert!(has_code_at(&diagnostics, "L1138", "/parts/0/place/1"));
    assert!(
        diagnostics[0].message.contains("place 0 + part size 5 = 5"),
        "the message shows the arithmetic: {}",
        diagnostics[0].message
    );
    // Exactly filling the grid is legal.
    assert!(check_assembly_parts(&doc, &[part([4, 4, 4], 'A')]).is_empty());
}

#[test]
fn part_count_mismatch_is_one_l1138() {
    let doc = check_assembly(MINIMAL).unwrap();
    let diagnostics = check_assembly_parts(&doc, &[]);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(has_code_at(&diagnostics, "L1138", "/parts"));
}

// --- Composition helpers ------------------------------------------------------------------------

#[test]
fn part_voxel_carries_the_palette_and_mirrors_slices() {
    let source = MINIMAL.replace(
        "    place:\n      - 0\n      - 0\n      - 0\n",
        "    place:\n      - 0\n      - 0\n      - 0\n    mirror: x\n",
    );
    let doc = check_assembly(&source).unwrap();
    // An asymmetric part: two layers, one row each, three cells wide.
    let asymmetric = PartDocument {
        name: None,
        size: [3, 2, 1],
        slices: vec![vec!["AB.".to_string()], vec![".BA".to_string()]],
    };
    let voxel = assembly_part_voxel(&doc, 0, &asymmetric);
    assert_eq!(voxel.name.as_deref(), Some("body"));
    assert_eq!(voxel.size, [3, 2, 1]);
    assert_eq!(voxel.palette, doc.palette);
    assert_eq!(
        voxel.slices,
        vec![vec![".BA".to_string()], vec!["AB.".to_string()]]
    );

    // Without `mirror: x` the slices are carried through untouched.
    let plain = check_assembly(MINIMAL).unwrap();
    assert_eq!(
        assembly_part_voxel(&plain, 0, &asymmetric).slices,
        asymmetric.slices
    );
}

#[test]
fn pivot_in_mesh_frame_mirrors_only_x_and_only_when_mirrored() {
    let mut entry = AssemblyPartEntry {
        name: "arm".to_string(),
        file: "parts/arm.part.yaml".to_string(),
        parent: None,
        place: [0, 0, 0],
        pivot: [1.0, 5.0, 1.0],
        mirror_x: false,
    };
    assert_eq!(entry.pivot_in_mesh_frame(2), [1.0, 5.0, 1.0]);
    entry.mirror_x = true;
    assert_eq!(entry.pivot_in_mesh_frame(2), [1.0, 5.0, 1.0]); // the center of an even-width limb
    entry.pivot = [0.5, 5.0, 1.0];
    assert_eq!(entry.pivot_in_mesh_frame(3), [2.5, 5.0, 1.0]);
}

#[test]
fn parent_indices_resolve_a_three_deep_chain() {
    // Authored leaf-first, so the test proves resolution is by name and not by position.
    let source = "\
format: 1
kind: assembly
size:
  - 4
  - 4
  - 4
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
parts:
  - name: hand
    file: parts/hand.part.yaml
    parent: forearm
    place:
      - 0
      - 2
      - 0
  - name: forearm
    file: parts/forearm.part.yaml
    parent: torso
    place:
      - 0
      - 1
      - 0
  - name: torso
    file: parts/torso.part.yaml
    place:
      - 0
      - 0
      - 0
";
    let doc = check_assembly(source).expect("a chain is a legal hierarchy");
    assert_eq!(
        assembly_parent_indices(&doc),
        vec![Some(1), Some(2), None],
        "each part resolves to its parent's index; the root is None"
    );
}

// --- Grid-level composition (`assembly_assembled_grid`, decisions 10-11) ------------------------

/// Two parts, placed side by side with no overlap: the composed grid is an exact concatenation of
/// their (already mirror-consumed) cells at each one's `place` offset, and every cell's owner is
/// exactly the one part that filled it.
#[test]
fn assembled_grid_composes_a_two_part_fixture_exactly() {
    let source = "\
format: 1
kind: assembly
size:
  - 3
  - 1
  - 2
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
parts:
  - name: left
    file: parts/left.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: right
    file: parts/right.part.yaml
    parent: left
    place:
      - 2
      - 0
      - 0
";
    let doc = check_assembly(source).expect("valid two-part assembly");
    let left = PartDocument {
        name: None,
        size: [2, 1, 2],
        slices: vec![vec!["AB".to_string(), "BA".to_string()]],
    };
    let right = PartDocument {
        name: None,
        size: [1, 1, 2],
        slices: vec![vec!["B".to_string(), "A".to_string()]],
    };
    let grid = assembly_assembled_grid(&doc, &[left, right]);

    assert_eq!(grid.voxel.size, [3, 1, 2]);
    assert_eq!(grid.voxel.palette, doc.palette);
    assert_eq!(
        grid.voxel.slices,
        vec![vec!["ABB".to_string(), "BAA".to_string()]],
        "right's local x=0 lands on global x=2 (place [2,0,0]), right beside left untouched"
    );
    assert_eq!(grid.owners_at(0, 0, 0), &[0]);
    assert_eq!(grid.owners_at(1, 0, 0), &[0]);
    assert_eq!(grid.owners_at(2, 0, 0), &[1]);
    assert_eq!(grid.owners_at(0, 0, 1), &[0]);
    assert_eq!(grid.owners_at(1, 0, 1), &[0]);
    assert_eq!(grid.owners_at(2, 0, 1), &[1]);
    assert!(grid.overlaps().is_empty());
}

/// A `place` near `u32::MAX` degrades to an empty grid; the composition never overflows.
#[test]
fn assembled_grid_survives_a_place_that_overflows_u32() {
    let source = "\
format: 1
kind: assembly
size:
  - 3
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: far
    file: parts/far.part.yaml
    place:
      - 0
      - 0
      - 0
";
    // `check_assembly` refuses an out-of-range `place`, so move it after validation.
    let mut doc = check_assembly(source).expect("valid assembly");
    doc.parts[0].place = [u32::MAX, 0, 0];
    let far = PartDocument {
        name: None,
        size: [2, 1, 1],
        slices: vec![vec!["AA".to_string()]],
    };
    let grid = assembly_assembled_grid(&doc, &[far]);
    assert_eq!(grid.voxel.slices, vec![vec!["...".to_string()]]);
}

/// A `mirror: x` part's cells are already reversed by [`assembly_part_voxel`] before composition —
/// `assembled_grid` must compose exactly that mirrored grid, not the part's raw, unmirrored one.
#[test]
fn assembled_grid_mirrors_a_part_before_composing() {
    let source = "\
format: 1
kind: assembly
size:
  - 3
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
parts:
  - name: arm
    file: parts/arm.part.yaml
    place:
      - 0
      - 0
      - 0
    mirror: x
";
    let doc = check_assembly(source).expect("valid mirrored assembly");
    let arm = PartDocument {
        name: None,
        size: [3, 1, 1],
        slices: vec![vec!["A.B".to_string()]],
    };
    let grid = assembly_assembled_grid(&doc, &[arm]);
    assert_eq!(
        grid.voxel.slices,
        vec![vec!["B.A".to_string()]],
        "the part's row is reversed before it is painted into the assembly grid"
    );
    assert_eq!(grid.owners_at(0, 0, 0), &[0]);
    assert_eq!(grid.owners_at(2, 0, 0), &[0]);
}

/// Two parts placed so they share one cell (the joint-plug technique, decision 10): the shared cell
/// counts as filled exactly once, the later-authored part's glyph wins, and both parts' indices are
/// recorded as owners, ascending.
#[test]
fn assembled_grid_counts_overlap_once_with_attribution() {
    let source = "\
format: 1
kind: assembly
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
parts:
  - name: first
    file: parts/first.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: second
    file: parts/second.part.yaml
    parent: first
    place:
      - 0
      - 0
      - 0
";
    let doc = check_assembly(source).expect("valid overlapping assembly");
    let first = PartDocument {
        name: None,
        size: [2, 1, 1],
        slices: vec![vec!["AA".to_string()]],
    };
    let second = PartDocument {
        name: None,
        size: [1, 1, 1],
        slices: vec![vec!["B".to_string()]],
    };
    let grid = assembly_assembled_grid(&doc, &[first, second]);

    // The overlapped cell (0,0,0) is filled once, not twice.
    assert_eq!(grid.voxel.filled_count(), 2);
    // `second` is later in authored order, so its glyph wins the shared cell.
    assert_eq!(grid.voxel.slices, vec![vec!["BA".to_string()]]);
    assert_eq!(
        grid.owners_at(0, 0, 0),
        &[0, 1],
        "both parts own the shared cell, ascending by authored index"
    );
    assert_eq!(grid.owners_at(1, 0, 0), &[0]);
    assert_eq!(grid.overlaps(), vec![([0, 0, 0], &[0, 1][..])]);
}

// --- The overlap / joint-plug lint (`assembly_overlap_lint`, L1143, decision 10) ----------------

/// A `trunk` (3x6x1, the root) with a 1x6x1 `limb` child sunk one cell into its top — the shared
/// cell (2,5,0) is the joint plug. The limb's pivot y is spliced in as authored text, because
/// *where the pivot sits* is the entire subject of these tests, and `extra` appends further parts.
fn plugged_source(pivot_y: &str, extra: &str) -> String {
    format!(
        "\
format: 1
kind: assembly
size:
  - 3
  - 11
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
parts:
  - name: trunk
    file: parts/trunk.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: limb
    file: parts/limb.part.yaml
    parent: trunk
    place:
      - 2
      - 5
      - 0
    pivot:
      - 0.5
      - {pivot_y}
      - 0.5
{extra}"
    )
}

/// The lint's diagnostics for a source built by [`plugged_source`] and its hand-built parts.
fn plugged_lint(source: &str, parts: Vec<PartDocument>) -> Vec<Diagnostic> {
    let doc = check_assembly(source).expect("the fixture is a valid assembly");
    let grid = assembly_assembled_grid(&doc, &parts);
    assembly_overlap_lint(&doc, &parts, &grid)
}

/// A limb plugged into its own parent *at its declared pivot* is the joint-plug technique working as
/// intended: legal, expected, and silent.
#[test]
fn a_plug_at_the_pivot_is_silent() {
    let source = plugged_source("0.5", "");
    let diagnostics = plugged_lint(&source, vec![part([3, 6, 1], 'A'), part([1, 6, 1], 'B')]);
    assert!(
        diagnostics.is_empty(),
        "an overlap around the pivot is a joint plug: {diagnostics:?}"
    );
}

/// The valuable catch: the same geometry with the pivot authored at the limb's far end. The overlap
/// is still parent/child, so the verdict rests entirely on the distance — which the warning names,
/// alongside the pair, the cluster's bbox and cell count, and the radius it was measured against.
#[test]
fn a_parent_child_overlap_far_from_the_pivot_warns() {
    let source = plugged_source("5.5", "");
    let diagnostics = plugged_lint(&source, vec![part([3, 6, 1], 'A'), part([1, 6, 1], 'B')]);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let warning = &diagnostics[0];
    assert_eq!(warning.code.as_str(), "L1143");
    assert_eq!(
        warning.severity,
        ludion_core::diagnostics::Severity::Warning,
        "the lint never fails the check"
    );
    assert_eq!(
        warning.path.as_deref(),
        Some("/parts/1"),
        "the pointer names the child — the part whose pivot is suspect"
    );
    assert!(
        warning
            .message
            .contains("`limb` overlaps its parent `trunk`"),
        "{}",
        warning.message
    );
    assert!(
        warning
            .message
            .contains("a cluster of 1 cell spanning (2,5,0)-(2,5,0)"),
        "{}",
        warning.message
    );
    assert!(
        // The plug sits at (2,5,0), five cells below the pivot authored at the limb's far end.
        warning
            .message
            .contains("centered 5.00 cells from `limb`'s pivot at (2.5,10.5,0.5)"),
        "{}",
        warning.message
    );
    assert!(
        // Half the child's largest extent (6), so 3.
        warning.message.contains("plug radius 3.00"),
        "{}",
        warning.message
    );
}

/// Two parts with no parent/child relation between them (here two children of one trunk) have no
/// joint to plug, so any shared cell warns — with no pivot distance to report.
#[test]
fn an_overlap_between_unrelated_parts_warns() {
    let source = plugged_source(
        "0.5",
        "  - name: cap
    file: parts/cap.part.yaml
    parent: trunk
    place:
      - 2
      - 7
      - 0
",
    );
    let diagnostics = plugged_lint(
        &source,
        vec![
            part([3, 6, 1], 'A'),
            part([1, 6, 1], 'B'),
            part([1, 1, 1], 'B'),
        ],
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "the trunk/limb plug stays silent; only the sibling pair warns: {diagnostics:?}"
    );
    let warning = &diagnostics[0];
    assert_eq!(warning.code.as_str(), "L1143");
    assert_eq!(
        warning.path.as_deref(),
        Some("/parts/2"),
        "with no child to blame, the pointer names the later-authored part"
    );
    assert!(
        warning
            .message
            .contains("parts `limb` and `cap` overlap in a cluster of 1 cell"),
        "{}",
        warning.message
    );
    assert!(
        warning.message.contains("neither is the other's parent"),
        "{}",
        warning.message
    );
    assert!(
        !warning.message.contains("pivot"),
        "an unrelated pair has no joint, so no pivot distance is reported: {}",
        warning.message
    );
}

/// One pair of parts can overlap in several disconnected places, and each 6-connected cluster is
/// judged on its own: a limb with a gap in it plugs its parent at the pivot *and* pokes through it a
/// cell higher — the first is silent, the second warns, and the warning names only its own cluster.
#[test]
fn each_cluster_of_a_pair_is_judged_on_its_own() {
    let source = "\
format: 1
kind: assembly
size:
  - 3
  - 3
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
parts:
  - name: trunk
    file: parts/trunk.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: limb
    file: parts/limb.part.yaml
    parent: trunk
    place:
      - 2
      - 0
      - 0
";
    let doc = check_assembly(source).expect("the fixture is a valid assembly");
    // A limb with a hole in its middle overlaps the solid trunk in two disconnected clusters.
    let limb = PartDocument {
        name: None,
        size: [1, 3, 1],
        slices: vec![
            vec!["B".to_string()],
            vec![".".to_string()],
            vec!["B".to_string()],
        ],
    };
    let parts = vec![part([3, 3, 1], 'A'), limb];
    let grid = assembly_assembled_grid(&doc, &parts);
    let diagnostics = assembly_overlap_lint(&doc, &parts, &grid);
    assert_eq!(
        diagnostics.len(),
        1,
        "the cluster at the pivot is silent, the far one is not: {diagnostics:?}"
    );
    assert!(
        diagnostics[0]
            .message
            .contains("a cluster of 1 cell spanning (2,2,0)-(2,2,0)"),
        "only the far cluster's own cells are reported: {}",
        diagnostics[0].message
    );
}

// --- Ragdoll joints (`joints:`, `Plan/physics.md` W3 plan 20) -----------------------------------

/// [`MINIMAL`] plus a second part `arm`, parented to `body` — the base every joint test edits. Not
/// jointed itself, so every case below authors its own `joints:` block.
fn two_part() -> String {
    format!("{MINIMAL}{}", part_entry("arm", Some("body"), [1, 0, 0]))
}

#[test]
fn hinge_joint_round_trips_byte_identically() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: hinge\n    axis:\n      - 1.0\n      - 0.0\n      - 0.0\n    \
         limits:\n      - 0.0\n      - 120.0\n    break_impulse: 80.0\n",
        two_part()
    );
    let doc = check_assembly(&source).expect("a valid hinge joint");
    assert_eq!(doc.joints.len(), 1);
    assert_eq!(doc.joints[0].part, "arm");
    assert_eq!(
        doc.joints[0].kind,
        AssemblyJointKind::Hinge {
            axis: [1.0, 0.0, 0.0],
            limits: Some([0.0, 120.0]),
        }
    );
    assert_eq!(doc.joints[0].break_impulse, Some(80.0));
    let canonical = emit_assembly(&doc);
    assert_eq!(canonical, source);
    assert_eq!(
        emit_assembly(&parse_assembly(&canonical).unwrap()),
        canonical
    );
}

#[test]
fn slider_joint_round_trips_byte_identically() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: slider\n    axis:\n      - 0.0\n      - 0.0\n      - 1.0\n    \
         limits:\n      - -1.0\n      - 1.0\n",
        two_part()
    );
    let doc = check_assembly(&source).expect("a valid slider joint");
    assert_eq!(
        doc.joints[0].kind,
        AssemblyJointKind::Slider {
            axis: [0.0, 0.0, 1.0],
            limits: Some([-1.0, 1.0]),
        }
    );
    assert_eq!(emit_assembly(&doc), source);
}

#[test]
fn ball_joint_round_trips_byte_identically() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: ball\n    limits:\n      -\n        - -10.0\n        - \
         10.0\n      -\n        - -10.0\n        - 10.0\n      -\n        - -5.0\n        - 5.0\n",
        two_part()
    );
    let doc = check_assembly(&source).expect("a valid ball joint");
    assert_eq!(
        doc.joints[0].kind,
        AssemblyJointKind::Ball {
            limits: Some([[-10.0, 10.0], [-10.0, 10.0], [-5.0, 5.0]]),
        }
    );
    assert_eq!(emit_assembly(&doc), source);
}

#[test]
fn fixed_joint_round_trips_byte_identically() {
    let source = format!("{}joints:\n  arm:\n    kind: fixed\n", two_part());
    let doc = check_assembly(&source).expect("a valid fixed joint");
    assert_eq!(doc.joints[0].kind, AssemblyJointKind::Fixed);
    assert_eq!(emit_assembly(&doc), source);
}

#[test]
fn a_joint_with_a_break_impulse_and_no_limits_round_trips() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: fixed\n    break_impulse: 12.5\n",
        two_part()
    );
    let doc = check_assembly(&source).expect("a valid fixed joint with a break impulse");
    assert_eq!(doc.joints[0].break_impulse, Some(12.5));
    assert_eq!(emit_assembly(&doc), source);
}

#[test]
fn default_hinge_axis_is_omitted_on_emit() {
    let with_default_axis = format!(
        "{}joints:\n  arm:\n    kind: hinge\n    axis:\n      - 0.0\n      - 1.0\n      - 0.0\n",
        two_part()
    );
    let doc = check_assembly(&with_default_axis).expect("a hinge joint with the default axis");
    assert_eq!(
        doc.joints[0].kind,
        AssemblyJointKind::Hinge {
            axis: [0.0, 1.0, 0.0],
            limits: None,
        }
    );
    let canonical = emit_assembly(&doc);
    assert_eq!(
        canonical,
        format!("{}joints:\n  arm:\n    kind: hinge\n", two_part()),
        "the default up axis is the key's absence:\n{canonical}"
    );
}

#[test]
fn no_joints_emits_exactly_as_a_plain_two_part_assembly() {
    let source = two_part();
    let doc = check_assembly(&source).expect("a two-part assembly with no joints");
    assert!(doc.joints.is_empty());
    let canonical = emit_assembly(&doc);
    assert_eq!(canonical, source);
    assert!(!canonical.contains("joints"));
}

// --- `joints` L1137 structure ---------------------------------------------------------------

#[test]
fn unknown_joint_kind_is_l1137() {
    let source = format!("{}joints:\n  arm:\n    kind: pivot\n", two_part());
    let diagnostics = errors_for(&source);
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/joints/arm/kind"))
        .expect("the bad kind is pointed at");
    assert_eq!(diagnostic.code.as_str(), "L1137");
}

#[test]
fn missing_joint_kind_is_l1137() {
    let source = format!("{}joints:\n  arm: {{}}\n", two_part());
    assert!(has_code_at(&errors_for(&source), "L1137", "/joints/arm"));
}

#[test]
fn axis_on_ball_joint_is_l1137() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: ball\n    axis:\n      - 1.0\n      - 0.0\n      - 0.0\n",
        two_part()
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1137",
        "/joints/arm/axis"
    ));
}

#[test]
fn axis_on_fixed_joint_is_l1137() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: fixed\n    axis:\n      - 1.0\n      - 0.0\n      - 0.0\n",
        two_part()
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1137",
        "/joints/arm/axis"
    ));
}

#[test]
fn limits_on_fixed_joint_is_l1137() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: fixed\n    limits:\n      - 0.0\n      - 1.0\n",
        two_part()
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1137",
        "/joints/arm/limits"
    ));
}

#[test]
fn wrong_axis_arity_is_l1137() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: hinge\n    axis:\n      - 1.0\n      - 0.0\n",
        two_part()
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1137",
        "/joints/arm/axis"
    ));
}

#[test]
fn non_numeric_limits_component_is_l1137() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: hinge\n    limits:\n      - low\n      - 1.0\n",
        two_part()
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1137",
        "/joints/arm/limits/0"
    ));
}

#[test]
fn wrong_ball_limits_count_is_l1137() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: ball\n    limits:\n      -\n        - -1.0\n        - \
         1.0\n",
        two_part()
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1137",
        "/joints/arm/limits"
    ));
}

#[test]
fn unknown_joint_entry_key_is_l1137() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: fixed\n    weld: true\n",
        two_part()
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1137",
        "/joints/arm/weld"
    ));
}

#[test]
fn non_mapping_joints_is_l1137() {
    let source = format!("{}joints: []\n", two_part());
    assert!(has_code_at(&errors_for(&source), "L1137", "/joints"));
}

// --- `joints` L1184 data consistency ----------------------------------------------------------

#[test]
fn joint_naming_unknown_part_is_l1184_with_suggestion() {
    let source = format!("{}joints:\n  arn:\n    kind: fixed\n", two_part());
    let diagnostics = errors_for(&source);
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/joints/arn"))
        .expect("the unknown part is pointed at");
    assert_eq!(diagnostic.code.as_str(), "L1184");
    assert_eq!(diagnostic.help.as_deref(), Some("nearest match is `arm`"));
}

#[test]
fn joint_naming_root_part_is_l1184() {
    let source = format!("{}joints:\n  body:\n    kind: fixed\n", two_part());
    assert!(has_code_at(&errors_for(&source), "L1184", "/joints/body"));
}

#[test]
fn non_finite_joint_axis_is_l1184() {
    let mut doc = check_assembly(&two_part()).unwrap();
    doc.joints.push(AssemblyJoint {
        part: "arm".to_string(),
        kind: AssemblyJointKind::Hinge {
            axis: [f64::NAN, 1.0, 0.0],
            limits: None,
        },
        break_impulse: None,
    });
    assert!(has_code_at(
        &validate_assembly(&doc),
        "L1184",
        "/joints/arm/axis"
    ));
}

#[test]
fn joint_axis_past_the_f32_range_is_l1184() {
    let source = format!(
        "{}joints:
  arm:
    kind: hinge
    axis:
      - 1e39
      - 0.0
      - 0.0
",
        two_part()
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1184",
        "/joints/arm/axis"
    ));
}

#[test]
fn zero_joint_axis_is_l1184() {
    let mut doc = check_assembly(&two_part()).unwrap();
    doc.joints.push(AssemblyJoint {
        part: "arm".to_string(),
        kind: AssemblyJointKind::Slider {
            axis: [0.0, 0.0, 0.0],
            limits: None,
        },
        break_impulse: None,
    });
    let diagnostics = validate_assembly(&doc);
    assert!(has_code_at(&diagnostics, "L1184", "/joints/arm/axis"));
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("zero vector")),
        "{diagnostics:?}"
    );
}

/// An axis this short survives an exactly-zero check on its authored `f64`s but rounds to exactly
/// `0.0` once narrowed to `f32` — the cast `ludion-engine`'s `ragdoll.rs` (`joint_kind_of`)
/// performs — and would poison the solver with `NaN` if it slipped through.
#[test]
fn a_near_zero_joint_axis_is_l1184() {
    let mut doc = check_assembly(&two_part()).unwrap();
    doc.joints.push(AssemblyJoint {
        part: "arm".to_string(),
        kind: AssemblyJointKind::Hinge {
            axis: [1e-50, 0.0, 0.0],
            limits: None,
        },
        break_impulse: None,
    });
    assert!(has_code_at(
        &validate_assembly(&doc),
        "L1184",
        "/joints/arm/axis"
    ));
}

#[test]
fn inverted_joint_limit_is_l1184() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: hinge\n    limits:\n      - 45.0\n      - -45.0\n",
        two_part()
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1184",
        "/joints/arm/limits"
    ));
}

#[test]
fn inverted_ball_joint_limit_is_l1184() {
    let source = format!(
        "{}joints:\n  arm:\n    kind: ball\n    limits:\n      -\n        - -10.0\n        - \
         10.0\n      -\n        - 10.0\n        - -10.0\n      -\n        - -5.0\n        - \
         5.0\n",
        two_part()
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1184",
        "/joints/arm/limits/1"
    ));
}

#[test]
fn non_positive_break_impulse_is_l1184() {
    for bad in ["0.0", "-1.0"] {
        let source = format!(
            "{}joints:\n  arm:\n    kind: fixed\n    break_impulse: {bad}\n",
            two_part()
        );
        assert!(
            has_code_at(&errors_for(&source), "L1184", "/joints/arm/break_impulse"),
            "break_impulse {bad} must be rejected"
        );
    }
}
