//! The voxel ops engine (`Plan/voxel-authoring-tools.md` Wave 1) and `upscale` (Wave 2): comment
//! extraction, the typed ops document, per-op semantics, and — the contract everything else exists
//! to protect — that a cell-level edit rewrites *only* the cells it targets, leaving every other
//! byte of the file (header docstring, layer comments, quoting, float formatting) exactly as it was.
//! `upscale` has no surgical form (every cell moves), so its own section covers the properties that
//! matter for it instead: block correctness, connectivity, and the `size`/`scale`/comment/provenance
//! bookkeeping.

use ludion_scene::{
    MirrorKeep, VoxelAxis, VoxelComments, VoxelDocument, VoxelEdit, VoxelOp, WriteMode,
    apply_voxel_ops, check_voxel, emit_voxel, emit_voxel_with_comments, extract_voxel_comments,
    parse_voxel, parse_voxel_ops, upscale_voxel, voxel_budget_warning, voxel_connectivity_warning,
};

// --- Fixtures ---------------------------------------------------------------------------------

/// A hand-authored model with everything a real vigil model carries: a header docstring, a declared
/// scale, per-layer anatomy comments, and both quoted and unquoted rows.
const TOTEM: &str = "\
format: 1
kind: voxel
name: totem
# Totem — the surgical-edit fixture. Front = row z=0 (meshes at world -Z).
# A second header line, so the block is a block.
size:
  - 4
  - 3
  - 2
scale: 0.25
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
  B:
    color:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
slices:
  # y0 — base
  -
    - AAAA
    - A..A
  # y1 — waist
  -
    - \".AA.\"
    - \"....\"
  # y2 — head
  -
    - \".BB.\"
    - \"....\"
";

/// An empty 5×5×5 model with one palette entry — the sculpting surface the shape ops are measured
/// on.
fn blank(size: u32) -> String {
    let row = ".".repeat(size as usize);
    let mut out = format!("format: 1\nkind: voxel\nsize:\n  - {size}\n  - {size}\n  - {size}\n");
    out.push_str(
        "palette:\n  A:\n    color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\n",
    );
    out.push_str("slices:\n");
    for _ in 0..size {
        out.push_str("  -\n");
        for _ in 0..size {
            out.push_str(&format!("    - \"{row}\"\n"));
        }
    }
    out
}

/// A tiny asymmetric model — no two rows or columns alike — so a block-expansion bug (a transposed
/// axis, an off-by-one on which source cell owns which output block) shows up as a wrong character
/// rather than hiding behind symmetry.
const ASYM: &str = "\
format: 1
kind: voxel
size:
  - 2
  - 1
  - 3
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
  C:
    color:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
slices:
  -
    - AB
    - \".C\"
    - CA
";

fn ops(source: &str) -> Vec<VoxelOp> {
    parse_voxel_ops(source).expect("the ops document should parse")
}

/// Applies a `- op: …` body (the envelope is added here) and returns the edit.
fn apply(model: &str, body: &str) -> VoxelEdit {
    let document = format!("format: 1\nkind: voxel-ops\nops:\n{body}");
    apply_voxel_ops(model, &ops(&document)).expect("the ops should apply")
}

fn errors(model: &str, body: &str) -> Vec<ludion_core::diagnostics::Diagnostic> {
    let document = format!("format: 1\nkind: voxel-ops\nops:\n{body}");
    apply_voxel_ops(model, &ops(&document)).expect_err("the ops should fail")
}

/// How many characters differ between two strings of equal length.
fn char_diff(a: &str, b: &str) -> usize {
    assert_eq!(a.len(), b.len(), "expected equal-length texts");
    a.chars().zip(b.chars()).filter(|(x, y)| x != y).count()
}

// --- The surgical contract ---------------------------------------------------------------------

/// The contract this module exists for: one `set` op changes exactly one character of the file.
/// Everything else — the header docstring, both layer comments, the quoting of each row, the
/// declared scale — is byte-identical.
#[test]
fn one_set_op_changes_exactly_one_character() {
    let edit = apply(TOTEM, "  - op: set\n    at: \"1,0,1\"\n    glyph: B\n");
    assert!(edit.surgical, "a cell edit must not re-emit the document");
    assert_eq!(edit.changed, vec![1]);
    assert_eq!(char_diff(TOTEM, &edit.text), 1);
    assert_eq!(edit.text, TOTEM.replace("- A..A", "- AB.A"));
    assert!(edit.dropped_comments.is_empty());
}

/// A no-op edit (writing the glyph already there) reports zero changed cells and reproduces the
/// file byte for byte.
#[test]
fn a_redundant_write_changes_nothing() {
    let edit = apply(TOTEM, "  - op: set\n    at: \"0,0,0\"\n    glyph: A\n");
    assert_eq!(edit.changed, vec![0]);
    assert_eq!(edit.text, TOTEM);
}

/// A shape op spanning many cells is still surgical: only the covered characters move.
#[test]
fn a_box_op_stays_surgical() {
    let edit = apply(
        TOTEM,
        "  - op: box\n    from: \"0,2,1\"\n    to: \"3,2,1\"\n    glyph: B\n",
    );
    assert!(edit.surgical);
    assert_eq!(edit.changed, vec![4]);
    assert_eq!(char_diff(TOTEM, &edit.text), 4);
    assert!(edit.text.contains("\"BBBB\""));
}

/// Writing a glyph that would change a plain row's quoting is not patched in place — the patched
/// text would not re-parse to the intended model, so the engine falls back to a canonical re-emit
/// (which quotes the row). The fallback is invisible to the caller except through `surgical`.
#[test]
fn a_quoting_hazard_falls_back_to_a_canonical_re_emit() {
    let model = "\
format: 1
kind: voxel
size:
  - 2
  - 1
  - 1
palette:
  \"#\":
    color:
      - 0.5
      - 0.5
      - 0.5
      - 1.0
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
    let edit = apply(model, "  - op: set\n    at: \"0,0,0\"\n    glyph: \"#\"\n");
    assert!(
        !edit.surgical,
        "a row whose quoting changes must re-emit, not be patched"
    );
    assert!(edit.text.contains("- \"#A\""), "text was:\n{}", edit.text);
    assert_eq!(edit.doc.slices[0][0], "#A");
}

// --- Comments ----------------------------------------------------------------------------------

/// The extractor classifies the header block and each layer's anatomy line, and reports nothing
/// stray for a well-formed model.
#[test]
fn the_extractor_finds_the_header_and_layer_comments() {
    let extracted = extract_voxel_comments(TOTEM);
    assert_eq!(
        extracted.comments.header,
        vec![
            "Totem — the surgical-edit fixture. Front = row z=0 (meshes at world -Z).".to_string(),
            "A second header line, so the block is a block.".to_string(),
        ]
    );
    assert_eq!(extracted.comments.layer(0), Some("y0 — base"));
    assert_eq!(extracted.comments.layer(1), Some("y1 — waist"));
    assert_eq!(extracted.comments.layer(2), Some("y2 — head"));
    assert!(extracted.stray.is_empty(), "{:?}", extracted.stray);
}

/// A comment the canonical writer has nowhere to put (inside the palette, or between two rows of a
/// layer) comes back as a stray with its source line — the list a re-emitting edit warns about.
#[test]
fn the_extractor_reports_strays_with_their_line() {
    let model = TOTEM
        .replace("palette:\n", "palette:\n  # the two body tones\n")
        .replace("    - AAAA\n", "    - AAAA\n    # the back row\n");
    let extracted = extract_voxel_comments(&model);
    let texts: Vec<&str> = extracted.stray.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(texts, vec!["# the two body tones", "# the back row"]);
    assert!(extracted.stray[0].line < extracted.stray[1].line);
    // The classified comments are unaffected by the strays around them.
    assert_eq!(extracted.comments.layer(2), Some("y2 — head"));
}

/// Comments replayed into the canonical writer reproduce the authored file, and empty comments
/// reproduce `emit_voxel` byte for byte.
#[test]
fn emit_with_comments_round_trips_and_degrades_to_emit_voxel() {
    // The writer always quotes rows, so the oracle is TOTEM with its bare rows quoted.
    let quoted = TOTEM
        .replace("    - AAAA\n", "    - \"AAAA\"\n")
        .replace("    - A..A\n", "    - \"A..A\"\n");
    let doc = check_voxel(TOTEM).expect("the fixture is valid");
    let extracted = extract_voxel_comments(TOTEM);
    let text = emit_voxel_with_comments(&doc, &extracted.comments);
    assert_eq!(text, quoted);
    assert_eq!(check_voxel(&text).expect("re-emitted text checks"), doc);
    assert_eq!(
        emit_voxel_with_comments(&doc, &VoxelComments::default()),
        emit_voxel(&doc)
    );
    // A comment-carrying emit still parses back to the same document — comments are not data.
    assert_eq!(parse_voxel(&text).expect("parses"), doc);
}

/// A `comment` op replaces the one anatomy line above its layer and touches nothing else.
#[test]
fn a_comment_op_replaces_one_line() {
    let edit = apply(
        TOTEM,
        "  - op: comment\n    y: 1\n    text: \"y1 — hips\"\n",
    );
    assert!(edit.surgical);
    assert_eq!(edit.changed, vec![0]);
    assert_eq!(edit.text, TOTEM.replace("y1 — waist", "y1 — hips"));
}

/// A layer with no comment gains one, inserted above its marker.
#[test]
fn a_comment_op_inserts_where_there_was_none() {
    let model = blank(2);
    let edit = apply(
        &model,
        "  - op: comment\n    y: 1\n    text: \"y1 — top\"\n",
    );
    assert!(edit.surgical);
    assert!(edit.text.contains("  # y1 — top\n  -\n"), "{}", edit.text);
    assert_eq!(
        extract_voxel_comments(&edit.text).comments.layer(1),
        Some("y1 — top")
    );
    // The layer 0 marker is untouched.
    assert_eq!(edit.text.matches("  -\n").count(), 2);
}

// --- Op semantics -------------------------------------------------------------------------------

#[test]
fn set_writes_one_cell_and_the_empty_glyph_carves() {
    let edit = apply(TOTEM, "  - op: set\n    at: \"0,0,0\"\n    glyph: \".\"\n");
    assert_eq!(edit.changed, vec![1]);
    assert_eq!(edit.doc.slices[0][0], ".AAA");
    assert_eq!(edit.doc.palette_index(0, 0, 0), None);
}

/// `set` gains the same `mode: paint` box/ellipsoid/cylinder/line already support: over a filled
/// cell it writes, over an empty one it does nothing (and `changed` says so).
#[test]
fn set_paint_mode_writes_over_filled_and_skips_empty() {
    // TOTEM y0 z0 is "AAAA" — (0,0,0) is filled.
    let over_filled = apply(
        TOTEM,
        "  - op: set\n    at: \"0,0,0\"\n    glyph: B\n    mode: paint\n",
    );
    assert_eq!(over_filled.changed, vec![1]);
    assert_eq!(over_filled.doc.slices[0][0], "BAAA");

    // TOTEM y0 z1 is "A..A" — (1,0,1) is empty.
    let over_empty = apply(
        TOTEM,
        "  - op: set\n    at: \"1,0,1\"\n    glyph: B\n    mode: paint\n",
    );
    assert_eq!(over_empty.changed, vec![0]);
    assert_eq!(over_empty.doc.slices[0][1], "A..A");
}

#[test]
fn an_out_of_range_set_is_l1132() {
    let diagnostics = errors(TOTEM, "  - op: set\n    at: \"9,0,0\"\n    glyph: A\n");
    assert_eq!(diagnostics[0].code.as_str(), "L1132");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/ops/0"));
    assert!(
        diagnostics[0].message.contains("(9, 0, 0)"),
        "{}",
        diagnostics[0].message
    );
}

/// A glyph with no palette entry fails before anything is written, and the help says to add it.
#[test]
fn an_unknown_glyph_is_l1132() {
    let diagnostics = errors(
        TOTEM,
        "  - op: box\n    from: \"0,0,0\"\n    to: \"1,1,1\"\n    glyph: Z\n",
    );
    assert_eq!(diagnostics[0].code.as_str(), "L1132");
    assert!(
        diagnostics[0]
            .help
            .as_deref()
            .is_some_and(|help| help.contains("palette")),
        "{:?}",
        diagnostics[0].help
    );
}

/// Volume ops clip to the grid instead of failing — a shape that pokes out is normal sculpting.
#[test]
fn a_box_clips_to_the_grid() {
    let model = blank(3);
    let edit = apply(
        &model,
        "  - op: box\n    from: \"-5,-5,-5\"\n    to: \"1,0,0\"\n    glyph: A\n",
    );
    assert_eq!(edit.changed, vec![2]);
    assert_eq!(edit.doc.slices[0][0], "AA.");
}

#[test]
fn a_huge_box_returns_promptly_and_changes_only_the_in_grid_cells() {
    let model = blank(3);
    let edit = apply(
        &model,
        "  - op: box
    from: \"-100000,-100000,-100000\"
    to: \"100000,100000,100000\"
    glyph: A
",
    );
    assert_eq!(edit.changed, vec![27]);
    let edit = apply(
        &model,
        "  - op: box
    from: \"2,0,0\"
    to: \"100000,100000,100000\"
    glyph: A
",
    );
    assert_eq!(edit.changed, vec![9]);
    // A box wholly off the grid writes nothing.
    let edit = apply(
        &model,
        "  - op: box
    from: \"5,5,5\"
    to: \"100000,100000,100000\"
    glyph: A
",
    );
    assert_eq!(edit.changed, vec![0]);
}

#[test]
fn paint_mode_writes_only_over_filled_cells() {
    // The whole layer 0 of the totem: row z=0 is solid, row z=1 is `A..A`.
    let edit = apply(
        TOTEM,
        "  - op: box\n    from: \"0,0,0\"\n    to: \"3,0,1\"\n    glyph: B\n    mode: paint\n",
    );
    assert_eq!(edit.changed, vec![6]);
    assert_eq!(edit.doc.slices[0][0], "BBBB");
    assert_eq!(edit.doc.slices[0][1], "B..B");
}

#[test]
fn an_ellipsoid_fills_by_the_analytic_inequality() {
    let model = blank(5);
    let edit = apply(
        &model,
        "  - op: ellipsoid\n    center: \"2,2,2\"\n    radii: \"2,2,2\"\n    glyph: A\n",
    );
    // The centre column reaches the surface; a corner is well outside it.
    assert_eq!(edit.doc.palette_index(0, 2, 2), Some(0));
    assert_eq!(edit.doc.palette_index(0, 0, 0), None);
    assert_eq!(edit.changed[0], edit.doc.filled_count());
}

#[test]
fn a_cylinder_fills_a_disc_per_layer() {
    let model = blank(5);
    let edit = apply(
        &model,
        "  - op: cylinder\n    base: \"2,0,2\"\n    axis: y\n    radius: 1\n    height: 3\n    glyph: A\n",
    );
    // A radius-1 disc is the 5-cell plus; three layers of it, and nothing above.
    assert_eq!(edit.changed, vec![15]);
    assert_eq!(edit.doc.slices[0][2], ".AAA.");
    assert_eq!(edit.doc.slices[3][2], ".....");
}

#[test]
fn a_line_is_a_connected_run_between_its_endpoints() {
    let model = blank(5);
    let edit = apply(
        &model,
        "  - op: line\n    from: \"0,0,0\"\n    to: \"4,4,0\"\n    glyph: A\n",
    );
    assert_eq!(edit.changed, vec![5]);
    assert_eq!(edit.doc.palette_index(0, 0, 0), Some(0));
    assert_eq!(edit.doc.palette_index(4, 4, 0), Some(0));
    // A pure diagonal is a staircase: its cells touch at edges, so it is deliberately not one
    // 6-connected component. `check-scene`'s L1129 is what reports that.
    assert_eq!(edit.doc.filled_count(), 5);
}

#[test]
fn a_line_running_off_the_grid_writes_its_in_grid_cells() {
    let model = blank(5);
    let edit = apply(
        &model,
        "  - op: line
    from: \"-3,0,0\"
    to: \"20,0,0\"
    glyph: A
",
    );
    assert_eq!(edit.changed, vec![5]);
    assert_eq!(edit.doc.slices[0][0], "AAAAA");
}

#[test]
fn a_line_past_the_cell_ceiling_is_l1132() {
    let diagnostics = errors(
        &blank(3),
        "  - op: line
    from: \"0,0,0\"
    to: \"10000000000,0,0\"
    glyph: A
",
    );
    assert_eq!(diagnostics[0].code.as_str(), "L1132");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/ops/0"));
    let diagnostics = errors(
        &blank(3),
        "  - op: line
    from: \"-9223372036854775807,0,0\"
    to: \"9223372036854775807,0,0\"
    glyph: A
",
    );
    assert_eq!(diagnostics[0].code.as_str(), "L1132");
}

#[test]
fn mirror_copies_the_kept_half_across_x() {
    // Layer 2 is `.BB.` / `....` — already symmetric; layer 0 row 1 is `A..A`. Make the low half
    // authoritative after carving one side.
    let edit = apply(
        TOTEM,
        "  - op: set\n    at: \"0,1,0\"\n    glyph: B\n  - op: mirror\n    axis: x\n    keep: low\n",
    );
    assert_eq!(edit.changed, vec![1, 1]);
    assert_eq!(edit.doc.slices[1][0], "BAAB");
}

#[test]
fn mirror_can_keep_the_high_half() {
    let edit = apply(
        TOTEM,
        "  - op: set\n    at: \"3,1,0\"\n    glyph: B\n  - op: mirror\n    axis: x\n    keep: high\n",
    );
    assert_eq!(edit.doc.slices[1][0], "BAAB");
}

/// An odd width leaves the self-mirroring centre column alone.
#[test]
fn mirror_skips_the_centre_column() {
    let model = blank(3);
    let edit = apply(
        &model,
        "  - op: box\n    from: \"0,0,0\"\n    to: \"1,0,0\"\n    glyph: A\n  - op: mirror\n    \
         axis: x\n    keep: low\n",
    );
    // `AA.` mirrored from the low half fills x=2 from x=0; x=1 (the centre) is untouched.
    assert_eq!(edit.doc.slices[0][0], "AAA");
}

// --- Mirror-shadow lint (`L1134`) ----------------------------------------------------------------

/// A `mirror` that discards cells two earlier `set` ops wrote fires one `L1134` warning per writer
/// op, each carrying the mirror's own index, the writer op's index and name, and the discarded-half
/// range.
#[test]
fn mirror_lint_fires_once_per_writer_op_with_counts_and_indices() {
    let model = blank(4);
    let edit = apply(
        &model,
        "  - op: set\n    at: \"3,0,0\"\n    glyph: A\n  - op: set\n    at: \"3,1,0\"\n    glyph: \
         A\n  - op: mirror\n    axis: x\n    keep: low\n",
    );
    assert_eq!(edit.warnings.len(), 2, "{:?}", edit.warnings);
    for warning in &edit.warnings {
        assert_eq!(warning.code.as_str(), "L1134");
        assert_eq!(warning.path.as_deref(), Some("/ops/2"));
    }
    assert!(
        edit.warnings[0].message.contains(
            "op 2 (mirror x keep low) overwrote 1 cell that op 0 (set) wrote in the discarded \
             half (x >= 2)"
        ),
        "{}",
        edit.warnings[0].message
    );
    assert!(
        edit.warnings[1].message.contains(
            "op 2 (mirror x keep low) overwrote 1 cell that op 1 (set) wrote in the discarded \
             half (x >= 2)"
        ),
        "{}",
        edit.warnings[1].message
    );
}

/// A write that stays inside the *kept* half is a mirror *source*, never overwritten — no lint.
#[test]
fn mirror_lint_is_silent_when_the_write_stays_in_the_kept_half() {
    let model = blank(4);
    let edit = apply(
        &model,
        "  - op: set\n    at: \"0,0,0\"\n    glyph: A\n  - op: mirror\n    axis: x\n    keep: \
         low\n",
    );
    assert!(edit.warnings.is_empty(), "{:?}", edit.warnings);
}

/// A plain mirror with nothing written beforehand raises nothing — the lint is never a property of
/// the mirror alone, only of what it overwrites.
#[test]
fn mirror_lint_is_silent_with_no_prior_writes() {
    let edit = apply(TOTEM, "  - op: mirror\n    axis: x\n    keep: high\n");
    assert!(edit.warnings.is_empty(), "{:?}", edit.warnings);
}

/// The write-history a `mirror` reads survives a `shift` between the write and the mirror — the
/// cell moves, its provenance moves with it, so the lint still attributes correctly by op index.
#[test]
fn mirror_lint_survives_a_shift_reshape() {
    let model = blank(4);
    let edit = apply(
        &model,
        "  - op: set\n    at: \"0,0,0\"\n    glyph: A\n  - op: shift\n    by: \"3,0,0\"\n  - op: \
         mirror\n    axis: x\n    keep: low\n",
    );
    assert_eq!(edit.warnings.len(), 1, "{:?}", edit.warnings);
    assert!(
        edit.warnings[0]
            .message
            .contains("op 2 (mirror x keep low) overwrote 1 cell that op 0 (set) wrote"),
        "{}",
        edit.warnings[0].message
    );
}

/// Same proof across a `grow` — the reshape adds cells and moves existing ones to new indices, but
/// a cell's writer travels with it, so a mirror after the grow still names the original writer op.
#[test]
fn mirror_lint_survives_a_grow_reshape() {
    let model = blank(4);
    let edit = apply(
        &model,
        "  - op: set\n    at: \"3,0,0\"\n    glyph: A\n  - op: grow\n    low: \"0,0,0\"\n    \
         high: \"2,0,0\"\n  - op: mirror\n    axis: x\n    keep: low\n",
    );
    // Grown to width 6; the x=3 cell keeps its index (growth padded the high side only) and now
    // sits in the discarded half (x >= 3) of the wider mirror.
    assert_eq!(edit.doc.size[0], 6);
    assert_eq!(edit.warnings.len(), 1, "{:?}", edit.warnings);
    assert!(
        edit.warnings[0].message.contains(
            "op 2 (mirror x keep low) overwrote 1 cell that op 0 (set) wrote in the discarded \
             half (x >= 3)"
        ),
        "{}",
        edit.warnings[0].message
    );
}

#[test]
fn shift_drops_what_leaves_and_empties_what_it_vacates() {
    let edit = apply(TOTEM, "  - op: shift\n    by: \"1,0,0\"\n");
    assert!(edit.surgical, "a shift keeps the grid shape");
    assert_eq!(edit.doc.size, [4, 3, 2]);
    assert_eq!(edit.doc.slices[0][0], ".AAA");
    assert_eq!(edit.doc.slices[0][1], ".A..");
    assert_eq!(edit.doc.slices[2][0], "..BB");
}

#[test]
fn a_shift_past_the_i64_range_empties_the_grid_without_panicking() {
    let edit = apply(
        TOTEM,
        "  - op: shift
    by: \"9223372036854775807,0,0\"
",
    );
    assert_eq!(edit.doc.filled_count(), 0);
    let edit = apply(
        TOTEM,
        "  - op: shift
    by: \"-9223372036854775807,0,-9223372036854775807\"
",
    );
    assert_eq!(edit.doc.filled_count(), 0);
}

// --- Reshapes and comment remapping -------------------------------------------------------------

/// `grow` re-emits (the grid changed shape) and carries every layer comment up by the low-y growth.
#[test]
fn grow_remaps_layer_comments_upward() {
    let edit = apply(
        TOTEM,
        "  - op: grow\n    low: \"1,2,0\"\n    high: \"0,0,1\"\n",
    );
    assert!(!edit.surgical, "a reshape must re-emit");
    assert_eq!(edit.doc.size, [5, 5, 3]);
    let comments = extract_voxel_comments(&edit.text).comments;
    assert_eq!(comments.layer(0), None);
    assert_eq!(comments.layer(1), None);
    assert_eq!(comments.layer(2), Some("y2 — base"));
    assert_eq!(comments.layer(3), Some("y3 — waist"));
    assert_eq!(comments.layer(4), Some("y4 — head"));
    // The header block survives the re-emit, and the model still validates.
    assert_eq!(comments.header.len(), 2);
    assert!(edit.text.contains("scale: 0.25"));
    // The original content moved by the low growth; the new cells are empty.
    assert_eq!(edit.doc.slices[2][0], ".AAAA");
    assert_eq!(edit.doc.slices[0][0], ".....");
}

/// `crop` renumbers the comments of the layers it keeps and reports the ones it removes.
#[test]
fn crop_drops_and_renumbers_layer_comments() {
    let edit = apply(
        TOTEM,
        "  - op: crop\n    from: \"0,1,0\"\n    to: \"3,2,1\"\n",
    );
    assert!(!edit.surgical);
    assert_eq!(edit.doc.size, [4, 2, 2]);
    let comments = extract_voxel_comments(&edit.text).comments;
    assert_eq!(comments.layer(0), Some("y0 — waist"));
    assert_eq!(comments.layer(1), Some("y1 — head"));
    assert_eq!(comments.layers.len(), 2);
    assert_eq!(edit.dropped_comments.len(), 1);
    assert_eq!(edit.dropped_comments[0].line, 25);
    assert_eq!(edit.dropped_comments[0].text, "# y0 — base");
    assert!(!edit.text.contains("base"));
}

/// A y-shift moves layer comments with their cells, renumbers the `y<N>` anchor, and reports the
/// comment whose layer leaves the grid.
#[test]
fn a_y_shift_moves_layer_comments_with_their_cells() {
    let edit = apply(TOTEM, "  - op: shift\n    by: \"0,-1,0\"\n");
    assert!(!edit.surgical, "a y-shift must re-emit");
    let comments = extract_voxel_comments(&edit.text).comments;
    assert_eq!(comments.layer(0), Some("y0 — waist"));
    assert_eq!(comments.layer(1), Some("y1 — head"));
    assert_eq!(comments.layer(2), None);
    assert_eq!(edit.dropped_comments.len(), 1);
    assert_eq!(edit.dropped_comments[0].line, 25);
    assert_eq!(edit.dropped_comments[0].text, "# y0 — base");
    assert!(!edit.text.contains("base"));
}

/// An x-only shift keeps the surgical path, comments byte-identical.
#[test]
fn an_x_only_shift_stays_surgical() {
    let edit = apply(TOTEM, "  - op: shift\n    by: \"1,0,1\"\n");
    assert!(edit.surgical);
    assert!(edit.dropped_comments.is_empty());
    assert_eq!(
        extract_voxel_comments(&edit.text),
        extract_voxel_comments(TOTEM)
    );
}

/// Only a leading `y<old>` token is an anchor: longer words and other text are left alone.
#[test]
fn relabel_leaves_non_anchor_comments_alone() {
    for text in ["yellow trim", "y12x — odd"] {
        let model = TOTEM.replace("y1 — waist", text);
        let edit = apply(
            &model,
            "  - op: grow\n    low: \"0,1,0\"\n    high: \"0,0,0\"\n",
        );
        let comments = extract_voxel_comments(&edit.text).comments;
        assert_eq!(comments.layer(2), Some(text));
    }
}

/// A re-emitting edit reports the comments it could not replay, so the caller can warn.
#[test]
fn a_reshape_reports_dropped_strays() {
    let model = TOTEM.replace("palette:\n", "palette:\n  # the two body tones\n");
    let edit = apply(
        &model,
        "  - op: grow\n    low: \"0,1,0\"\n    high: \"0,0,0\"\n",
    );
    assert!(!edit.surgical);
    assert_eq!(edit.dropped_comments.len(), 1);
    assert_eq!(edit.dropped_comments[0].text, "# the two body tones");
    assert!(!edit.text.contains("the two body tones"));
}

#[test]
fn a_grow_past_the_cell_ceiling_is_l1132() {
    // 32-bit overflow of one extent: must not panic or wrap.
    let diagnostics = errors(
        TOTEM,
        "  - op: grow
    low: \"4294967295,0,0\"
    high: \"0,0,0\"
",
    );
    assert_eq!(diagnostics[0].code.as_str(), "L1132");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/ops/0"));
    // Three extents near u32::MAX: the cell product must not overflow either.
    let diagnostics = errors(
        &blank(3),
        "  - op: grow
    low: \"4294967290,4294967290,4294967290\"
    high: \"0,0,0\"
",
    );
    assert_eq!(diagnostics[0].code.as_str(), "L1132");
    // In range per axis but 257^3 cells overall.
    let diagnostics = errors(
        &blank(3),
        "  - op: grow
    low: \"127,127,127\"
    high: \"127,127,127\"
",
    );
    assert_eq!(diagnostics[0].code.as_str(), "L1132");
    // 256^3 exactly is the ceiling and is not built here; a small grow still works.
    let edit = apply(
        &blank(3),
        "  - op: grow
    low: \"0,0,0\"
    high: \"1,1,1\"
",
    );
    assert_eq!(edit.doc.size, [4, 4, 4]);
}

#[test]
fn an_inverted_crop_is_l1132() {
    let diagnostics = errors(
        TOTEM,
        "  - op: crop\n    from: \"0,2,0\"\n    to: \"3,1,1\"\n",
    );
    assert_eq!(diagnostics[0].code.as_str(), "L1132");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/ops/0"));
}

#[test]
fn a_crop_past_the_grid_is_l1132() {
    let diagnostics = errors(
        TOTEM,
        "  - op: crop\n    from: \"0,0,0\"\n    to: \"9,2,1\"\n",
    );
    assert_eq!(diagnostics[0].code.as_str(), "L1132");
}

// --- Ops document diagnostics --------------------------------------------------------------------

#[test]
fn ops_parse_into_typed_values() {
    let parsed = ops(
        "format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"3,10,4\"\n    glyph: H\n  \
         - op: mirror\n    axis: x\n    keep: high\n  - op: box\n    from: \"0,0,0\"\n    \
         to: \"1,1,1\"\n    glyph: H\n    mode: paint\n",
    );
    assert_eq!(
        parsed[0],
        VoxelOp::Set {
            at: [3, 10, 4],
            glyph: 'H',
            mode: WriteMode::Place,
        }
    );
    assert_eq!(
        parsed[1],
        VoxelOp::Mirror {
            axis: VoxelAxis::X,
            keep: MirrorKeep::High
        }
    );
    assert!(matches!(
        parsed[2],
        VoxelOp::Box {
            mode: WriteMode::Paint,
            ..
        }
    ));
    assert_eq!(parsed[0].name(), "set");
    assert!(!parsed[0].reshapes());
}

#[test]
fn a_two_component_coordinate_is_l1131_with_a_pointer() {
    let diagnostics = parse_voxel_ops(
        "format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"3,10\"\n    glyph: A\n",
    )
    .expect_err("a two-component coordinate is rejected");
    assert_eq!(diagnostics[0].code.as_str(), "L1131");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/ops/0/at"));
}

#[test]
fn an_unknown_op_name_suggests_the_nearest() {
    let diagnostics =
        parse_voxel_ops("format: 1\nkind: voxel-ops\nops:\n  - op: sett\n    at: \"0,0,0\"\n")
            .expect_err("`sett` is not an op");
    assert_eq!(diagnostics[0].code.as_str(), "L1131");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/ops/0/op"));
    assert_eq!(
        diagnostics[0].help.as_deref(),
        Some("nearest match is `set`")
    );
}

#[test]
fn an_unknown_op_key_suggests_the_nearest() {
    let diagnostics = parse_voxel_ops(
        "format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"0,0,0\"\n    gylph: A\n",
    )
    .expect_err("`gylph` is not a key of `set`");
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/ops/0/gylph"))
        .expect("a pointer at the offending key");
    assert_eq!(d.help.as_deref(), Some("nearest match is `glyph`"));
}

#[test]
fn a_missing_required_key_names_the_op_and_the_key() {
    let diagnostics =
        parse_voxel_ops("format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"0,0,0\"\n")
            .expect_err("`set` needs a glyph");
    assert_eq!(diagnostics[0].code.as_str(), "L1131");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/ops/0"));
    assert!(
        diagnostics[0].message.contains("glyph"),
        "{}",
        diagnostics[0].message
    );
}

#[test]
fn a_wrong_kind_is_l1131() {
    let diagnostics = parse_voxel_ops("format: 1\nkind: voxel\nops: []\n")
        .expect_err("an ops document must declare kind: voxel-ops");
    assert_eq!(diagnostics[0].code.as_str(), "L1131");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/kind"));
}

#[test]
fn a_missing_ops_key_is_l1131() {
    let diagnostics =
        parse_voxel_ops("format: 1\nkind: voxel-ops\n").expect_err("`ops` is required");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1131" && d.message.contains("`ops`"))
    );
}

/// A digit glyph resolves as a YAML number; the reader accepts it as the one-character glyph it
/// spells, so `glyph: 1` and `glyph: "1"` mean the same thing.
#[test]
fn a_digit_glyph_is_read_as_a_glyph() {
    let parsed =
        ops("format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"0,0,0\"\n    glyph: 1\n");
    assert_eq!(
        parsed[0],
        VoxelOp::Set {
            at: [0, 0, 0],
            glyph: '1',
            mode: WriteMode::Place,
        }
    );
}

/// An op list applies in order, and the reported per-op counts are the cells each one changed.
#[test]
fn ops_apply_in_order_with_per_op_counts() {
    let model = blank(3);
    let edit = apply(
        &model,
        "  - op: box\n    from: \"0,0,0\"\n    to: \"2,0,2\"\n    glyph: A\n  - op: set\n    \
         at: \"1,0,1\"\n    glyph: \".\"\n",
    );
    assert_eq!(edit.changed, vec![9, 1]);
    assert_eq!(edit.doc.filled_count(), 8);
}

// --- Failure leaves the input alone --------------------------------------------------------------

/// A model that does not validate is rejected before any op runs — the ops engine never edits a
/// broken file into a differently broken one.
#[test]
fn an_invalid_model_is_rejected_with_its_own_code() {
    let broken = TOTEM.replace("    - AAAA", "    - AAA");
    let diagnostics = apply_voxel_ops(
        &broken,
        &ops("format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"0,0,0\"\n    glyph: A\n"),
    )
    .expect_err("the model does not validate");
    assert!(diagnostics.iter().any(|d| d.code.as_str() == "L1114"));
}

/// A failing op yields diagnostics and no text at all — there is nothing a caller could write, which
/// is what keeps a half-applied model off disk.
#[test]
fn a_failing_op_produces_no_text() {
    let result = apply_voxel_ops(
        TOTEM,
        &ops(
            "format: 1\nkind: voxel-ops\nops:\n  - op: set\n    at: \"0,0,0\"\n    glyph: B\n  \
             - op: set\n    at: \"99,0,0\"\n    glyph: A\n",
        ),
    );
    assert!(result.is_err());
    // The source is untouched by construction: `apply` never borrows it mutably.
    assert!(TOTEM.contains("    - AAAA"));
}

/// A document assembled by hand (not parsed) emits with comments just as well — the emitter is
/// independent of how the document was built.
#[test]
fn emit_with_comments_works_on_a_built_document() {
    let doc = VoxelDocument {
        name: Some("stub".into()),
        size: [1, 1, 1],
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette: Vec::new(),
        slices: vec![vec![".".into()]],
    };
    let mut comments = VoxelComments::default();
    comments.header.push("a stub".into());
    comments.set_layer(0, "y0 — nothing");
    let text = emit_voxel_with_comments(&doc, &comments);
    assert_eq!(
        text,
        "format: 1\nkind: voxel\nname: stub\n# a stub\nsize:\n  - 1\n  - 1\n  - 1\npalette: {}\n\
         slices:\n  # y0 — nothing\n  -\n    - \".\"\n"
    );
    assert_eq!(check_voxel(&text).expect("checks"), doc);
}

// --- Upscale (`Plan/voxel-authoring-tools.md` Wave 2) --------------------------------------------

#[test]
fn upscale_past_the_cell_ceiling_is_l1132() {
    // 40^3 x 7 = 280^3 = 21.9M cells, past the 16.7M ceiling; refused before any allocation.
    let diagnostics = upscale_voxel(&blank(40), 7, None).expect_err("past the ceiling");
    assert_eq!(diagnostics[0].code.as_str(), "L1132");
}

/// Each source cell becomes a `factor`^3 block of identical glyphs. `ASYM` has no repeated row or
/// column, so a transposed axis or an off-by-one on block placement would land on the wrong
/// character rather than hiding behind symmetry.
#[test]
fn upscale_expands_each_cell_into_a_block() {
    let up = upscale_voxel(ASYM, 2, None).expect("2 is in range");
    assert_eq!(up.doc.size, [4, 2, 6]);
    let expected: Vec<String> = ["AABB", "AABB", "..CC", "..CC", "CCAA", "CCAA"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(up.doc.slices[0], expected, "y=0 (first copy of source y=0)");
    assert_eq!(
        up.doc.slices[1], expected,
        "y=1 (second copy of source y=0)"
    );
}

/// A fully-connected source model stays one component after upscaling — adjacent source cells stay
/// adjacent (their blocks share a face), so the flood fill never splits.
#[test]
fn upscale_preserves_connectivity() {
    let before = check_voxel(TOTEM).expect("TOTEM is valid");
    assert_eq!(before.component_count(), 1, "fixture assumption");
    let up = upscale_voxel(TOTEM, 3, None).expect("3 is in range");
    assert_eq!(up.doc.component_count(), 1);
}

/// The palette is carried verbatim — upscaling never touches color.
#[test]
fn upscale_palette_is_unchanged() {
    let before = check_voxel(TOTEM).expect("TOTEM is valid");
    let up = upscale_voxel(TOTEM, 4, None).expect("4 is in range");
    assert_eq!(up.doc.palette, before.palette);
}

/// `scale` divides by the factor, so grid height x scale (the declared world footprint) is
/// unchanged; the pre-division value survives on the result so a caller need not invert the math.
#[test]
fn upscale_divides_the_declared_scale() {
    let up = upscale_voxel(TOTEM, 4, None).expect("4 is in range");
    assert_eq!(up.source_scale, Some(0.25));
    assert_eq!(up.doc.scale, Some(0.0625));
}

/// The header carries verbatim plus one appended note, and each layer's anatomy comment (including
/// a multi-line one) moves onto `y * factor` — the first of its `factor` output layers; the other
/// `factor - 1` layers get none.
#[test]
fn upscale_carries_header_and_layer_comments_to_y_times_factor() {
    let model = TOTEM.replace(
        "  # y1 — waist\n",
        "  # y1 — waist\n  # continued anatomy note.\n",
    );
    let up = upscale_voxel(&model, 2, Some("totem.voxel.yaml")).expect("2 is in range");
    let extracted = extract_voxel_comments(&up.text);
    assert_eq!(
        extracted.comments.header,
        vec![
            "Totem — the surgical-edit fixture. Front = row z=0 (meshes at world -Z).".to_string(),
            "A second header line, so the block is a block.".to_string(),
            "upscaled x2 from totem.voxel.yaml".to_string(),
        ]
    );
    assert_eq!(extracted.comments.layer(0), Some("y0 — base"));
    assert!(extracted.comments.layer(1).is_none());
    assert_eq!(
        extracted.comments.layer(2),
        Some("y2 — waist\ncontinued anatomy note.")
    );
    assert!(extracted.comments.layer(3).is_none());
    assert_eq!(extracted.comments.layer(4), Some("y4 — head"));
    assert!(extracted.stray.is_empty(), "{:?}", extracted.stray);
}

/// `generated_from` never survives an upscale (the output is not that script's product) but is
/// reported back so a caller can say what was dropped; `budget_exempt` carries over as-is.
#[test]
fn upscale_drops_generated_from_but_carries_budget_exempt() {
    let model = TOTEM.replace(
        "palette:\n",
        "generated_from: totem.voxel.gen.mjs\nbudget_exempt: true\npalette:\n",
    );
    let up = upscale_voxel(&model, 2, None).expect("2 is in range");
    assert_eq!(
        up.dropped_generated_from.as_deref(),
        Some("totem.voxel.gen.mjs")
    );
    assert!(up.doc.generated_from.is_none());
    assert!(!up.text.contains("generated_from"));
    assert_eq!(up.doc.budget_exempt, Some(true));
    assert!(up.text.contains("budget_exempt: true"));
}

/// 1 is a pointless copy and 9 is past the ceiling; both are the same `L1132` "op inapplicable"
/// family `voxel apply` uses, not a new code.
#[test]
fn a_factor_outside_2_to_8_is_l1132() {
    for bad in [1, 9] {
        let diagnostics = upscale_voxel(TOTEM, bad, None).unwrap_err();
        assert_eq!(diagnostics[0].code.as_str(), "L1132", "factor {bad}");
    }
}

/// An upscale can legitimately cross the cell budget (that is the point of the verb); the warning
/// still fires, because nothing about upscaling asserts `budget_exempt` on the caller's behalf.
#[test]
fn an_over_budget_upscale_still_warns() {
    let model = blank(40); // 40^3 = 64,000, under the 64^3 budget.
    let up = upscale_voxel(&model, 4, None).expect("4 is in range"); // 160^3, well over.
    assert!(voxel_budget_warning(&up.doc).is_some());
}

/// The output is a canonical fixed point: extracting `up.text`'s own comments and re-emitting from
/// `up.doc` reproduces it byte for byte — the same idempotence [`emit_voxel_with_comments`] holds
/// everywhere else.
#[test]
fn upscale_output_is_a_canonical_fixed_point() {
    let up = upscale_voxel(TOTEM, 3, Some("totem.voxel.yaml")).expect("3 is in range");
    let reextracted = extract_voxel_comments(&up.text);
    assert_eq!(
        emit_voxel_with_comments(&up.doc, &reextracted.comments),
        up.text
    );
}

// --- Cross-language parity with scripts/voxel/grid.mjs -------------------------------------------

/// The identical fixture `scripts/voxel/grid.test.mjs`'s `fixtureGrid()` builds cell for cell — a
/// 4x1x4 grid with three 6-connected pieces of distinct size: the 3-cell "main body" L-shape
/// (0,0,0)-(1,0,0)-(1,0,1) (two flood-fill hops, not a straight run, so a fill that only checks
/// immediate neighbours would miss the join), the 2-cell piece (3,0,0)-(3,0,1), and the 1-cell
/// stray (0,0,3). This crate's `L1129` checker and the JS kit's `components()` are two independent
/// implementations of the same 6-connected flood fill; this test and the JS one pin them to the
/// same sizes, samples, and order over the same grid so they cannot silently diverge.
const KIT_FIXTURE: &str = "\
format: 1
kind: voxel
size:
  - 4
  - 1
  - 4
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
slices:
  -
    - \"AA.A\"
    - \".A.A\"
    - \"....\"
    - \"A...\"
";

/// See [`KIT_FIXTURE`]: this crate's connectivity checker must find the same three pieces, in the
/// same smallest-first order, that `scripts/voxel/grid.test.mjs`'s `components()` test pins.
#[test]
fn connectivity_matches_the_js_kit_fixture() {
    let doc = check_voxel(KIT_FIXTURE).expect("the fixture is valid");
    assert_eq!(doc.component_count(), 3);
    let warning = voxel_connectivity_warning(&doc).expect("3 components warns");
    assert!(
        warning
            .message
            .contains("split into 3 disconnected components (main body 3 cells)"),
        "{}",
        warning.message
    );
    // Smallest first, matching components()'s ascending-by-size sort; each sample is that piece's
    // first cell in (y, z, x) scan order, matching components()'s (y, z, x) outer loop.
    let stray_1 = warning.message.find("1 cell, sample (0, 0, 3)");
    let stray_2 = warning.message.find("2 cells, sample (3, 0, 0)");
    assert!(stray_1.is_some(), "{}", warning.message);
    assert!(stray_2.is_some(), "{}", warning.message);
    assert!(
        stray_1 < stray_2,
        "the 1-cell stray must be listed first (smallest first): {}",
        warning.message
    );
}

/// The realism proof: upscaling a frozen, committed model 3x stays clean, connected, and its scale
/// divides exactly as the header arithmetic promises. Read-only — the source file is never written.
#[test]
fn upscaling_a_committed_model_stays_clean_and_connected() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/lantern-x3.voxel.yaml"
    ))
    .expect("lantern-x3.voxel.yaml fixture exists");
    let before = check_voxel(&source).expect("the frozen model is valid");
    let up = upscale_voxel(&source, 3, Some("lantern-x3.voxel.yaml")).expect("3 is in range");
    assert_eq!(
        check_voxel(&up.text).expect("the upscaled model is valid"),
        up.doc
    );
    assert_eq!(
        up.doc.size,
        [before.size[0] * 3, before.size[1] * 3, before.size[2] * 3]
    );
    assert_eq!(
        up.doc.component_count(),
        before.component_count(),
        "upscaling must not change how many pieces the model is in"
    );
    assert_eq!(up.doc.scale, before.scale.map(|s| s / 3.0));
}

// --- Row quoting -------------------------------------------------------------------------------

/// The canonical writer always double-quotes rows, so a re-emit never churns untouched rows.
#[test]
fn a_re_emit_keeps_every_quoted_row_verbatim() {
    let source = "\
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
    - \"A.\"
  -
    - \"..\"
    - \"A.\"
";
    let edit = apply(
        source,
        "  - op: grow\n    high: \"0,1,0\"\n    low: \"0,0,0\"\n",
    );
    assert!(!edit.surgical);
    for line in source.lines().filter(|l| l.starts_with("    - \"")) {
        assert!(
            edit.text.lines().any(|l| l == line),
            "missing {line}\n{}",
            edit.text
        );
    }
}

#[test]
fn emit_voxel_quotes_letter_led_rows() {
    let source = TOTEM.replace("    - AAAA\n", "    - AB..\n");
    let doc = check_voxel(&source).expect("valid");
    assert!(emit_voxel(&doc).contains("    - \"AB..\"\n"));
}
