//! The derived tree end to end: the conformance hash the two pinned platforms must agree on, the
//! shape of the document the derive writes, and the four ways an edit is refused.
//!
//! Every test builds its own tree in a temp directory — the derive writes beside the edit document,
//! and a fixture directory is not a place to write.

use std::path::{Path, PathBuf};

use ludion_mesh_edit::{Derived, derive};

const PLATE_MESH: &str = include_str!("fixtures/plate.mesh.yaml");
const PLATE_EDIT: &str = include_str!("fixtures/plate-lit.mesh-edit.yaml");
const STRIP_MESH: &str = include_str!("fixtures/strip.mesh.yaml");
const STRIP_FILL_EDIT: &str = include_str!("fixtures/strip-fill.mesh-edit.yaml");

/// The same plate with no UV set and therefore no map slot — a mesh an edit has nowhere to paint.
const NO_UVS_MESH: &str = "\
format: 1
kind: mesh
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
indices:
  - 0
  - 1
  - 2
";

/// XXH3 of the derived emissive map's PNG bytes, landed from the authoring machine (Windows
/// x86-64). The two pinned boxes must agree on it: a mismatch on the other one is a determinism
/// bug in the rasteriser, the region algebra or the blend, never a per-OS constant.
const PLATE_EMISSIVE_HASH: u64 = 0x0c60_5f5c_c6b4_ad21;

/// XXH3 of the derived emissive map of the strip fill, landed from the authoring machine (Windows
/// x86-64). It walks a UV seam by weld and a two-tone base by colour, so a mismatch on the other
/// pinned platform is a determinism bug in the weld, the seam bridge or the flood, never a
/// per-OS constant.
const STRIP_FILL_HASH: u64 = 0xa0c9_c7e0_4974_1c1d;

/// The pristine base-colour map, the one file of the fixture tree that is not text. Written by the
/// engine's own encoder so no binary blob is committed for it.
fn base_png() -> Vec<u8> {
    ludion_assets::encode_png(64, 64, &[128u8, 128, 128, 255].repeat(64 * 64))
        .expect("the encoder takes a 64x64 image")
}

/// A fresh tree laid out the way `gen accept` leaves one: `meshes/` beside `textures/`, the edit
/// document next to the mesh it edits.
fn tree(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-mesh-edit-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).expect("the tree is created");
    std::fs::create_dir_all(dir.join("textures")).expect("the tree is created");
    write(&dir.join("meshes/plate.mesh.yaml"), PLATE_MESH.as_bytes());
    write(
        &dir.join("meshes/plate-lit.mesh-edit.yaml"),
        PLATE_EDIT.as_bytes(),
    );
    write(&dir.join("textures/base.png"), &base_png());
    dir
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}

fn edit_path(dir: &Path) -> PathBuf {
    dir.join("meshes/plate-lit.mesh-edit.yaml")
}

fn derived(dir: &Path) -> Derived {
    derive(&edit_path(dir)).unwrap_or_else(|diagnostics| panic!("{diagnostics:#?}"))
}

/// The one derived map's bytes.
fn emissive_png(derived: &Derived) -> Vec<u8> {
    let files = derived.files();
    assert_eq!(files.len(), 2, "the document and one map");
    files[1].1.to_vec()
}

#[test]
fn the_fixtures_are_canonical() {
    let mesh = ludion_scene::check_mesh(PLATE_MESH).unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(ludion_scene::emit_mesh(&mesh), PLATE_MESH);
    let edit = ludion_scene::check_mesh_edit(PLATE_EDIT).unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(ludion_scene::emit_mesh_edit(&edit), PLATE_EDIT);
    let mesh = ludion_scene::check_mesh(STRIP_MESH).unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(ludion_scene::emit_mesh(&mesh), STRIP_MESH);
    let edit = ludion_scene::check_mesh_edit(STRIP_FILL_EDIT).unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(ludion_scene::emit_mesh_edit(&edit), STRIP_FILL_EDIT);
}

#[test]
fn the_plate_derives_to_the_conformance_hash() {
    let dir = tree("conformance");
    let hash = ludion_assets::content_hash(&emissive_png(&derived(&dir)));
    assert_eq!(
        hash, PLATE_EMISSIVE_HASH,
        "the derived emissive map hashes to {hash:#018x}; a change here re-derives every committed \
         tree, and a difference between the two pinned platforms is a determinism break"
    );
}

#[test]
fn deriving_twice_is_bit_identical() {
    let dir = tree("twice");
    assert_eq!(emissive_png(&derived(&dir)), emissive_png(&derived(&dir)));
}

/// The derived document is the source's text with three differences: it says where it came from,
/// the edited slot points at the new map beside it, and every untouched slot points at the pristine
/// file where it already lives.
#[test]
fn the_derived_document_points_back_and_out() {
    let dir = tree("document");
    let derived = derived(&dir);
    let document =
        ludion_scene::check_mesh(derived.document()).unwrap_or_else(|d| panic!("{d:#?}"));

    assert_eq!(
        document.generated_from.as_deref(),
        Some("../plate-lit.mesh-edit.yaml")
    );
    assert_eq!(
        document
            .textures
            .base_color
            .as_ref()
            .map(|r| r.path.as_str()),
        Some("../../textures/base.png")
    );
    let emissive = document
        .textures
        .emissive
        .as_ref()
        .expect("the edit created an emissive map");
    assert!(
        emissive.path.starts_with("textures/") && emissive.path.ends_with(".png"),
        "{}",
        emissive.path
    );
    assert_eq!(
        document.positions,
        ludion_scene::check_mesh(PLATE_MESH).unwrap().positions
    );
}

/// Decision 9's trap, as a test: writing a per-vertex material list would flip the loader's
/// `authored_material` and silently make the scene's own `metallic`/`roughness`/`emissive` inert.
#[test]
fn the_derive_never_invents_a_material_list() {
    let dir = tree("material");
    let document =
        ludion_scene::check_mesh(derived(&dir).document()).unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(document.emissive, None);
    assert_eq!(document.metallic, None);
    assert_eq!(document.roughness, None);
}

#[test]
fn the_derived_document_round_trips() {
    let dir = tree("roundtrip");
    let derived = derived(&dir);
    let reparsed =
        ludion_scene::check_mesh(derived.document()).unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(ludion_scene::emit_mesh(&reparsed), derived.document());
}

#[test]
fn the_report_names_what_each_op_painted() {
    let dir = tree("report");
    let derived = derived(&dir);
    let report = derived.report();

    assert_eq!(report.source, "plate.mesh.yaml");
    assert_eq!(report.out, "plate-lit");
    assert_eq!(report.ops.len(), 3);
    for (index, op) in report.ops.iter().enumerate() {
        assert_eq!(op.index, index);
        assert_eq!(op.op, "paint");
        assert_eq!(op.map, "emissive");
        assert!(op.texels > 0, "{op:#?}");
        assert!(op.share > 0.0 && op.share < 1.0, "{op:#?}");
        assert!(op.triangles >= 1, "{op:#?}");
    }
    // The sphere sits on the near corner of the plate, so its painted surface stays there.
    let sphere = report.ops[0].bounds.expect("the sphere painted something");
    assert!(sphere.max[0] < 0.5 && sphere.max[2] < 0.5, "{sphere:?}");
    assert_eq!(sphere.min[1], 0.0, "the plate is flat");

    assert_eq!(report.maps.len(), 1);
    let map = &report.maps[0];
    assert_eq!(map.map, "emissive");
    assert_eq!((map.width, map.height), (64, 64));
    assert!(map.created, "the source has no emissive map");
    assert!(map.non_black_texels > 0 && map.non_black_texels < 64 * 64);
    assert!(report.next.contains("Mesh.emissive"), "{}", report.next);
}

/// The report is what `--json` prints, so its field names are a consumer contract.
#[test]
fn the_report_serializes_to_the_documented_json() {
    let dir = tree("json");
    let derived = derived(&dir);
    let json = serde_json::to_value(derived.report()).expect("the report serializes");

    assert_eq!(json["source"], "plate.mesh.yaml");
    assert_eq!(json["out"], "plate-lit");
    assert!(json["next"].is_string());
    let op = &json["ops"][0];
    for key in [
        "index",
        "op",
        "map",
        "texels",
        "share",
        "bounds",
        "triangles",
    ] {
        assert!(!op[key].is_null(), "ops[0].{key} is missing: {op}");
    }
    assert!(
        op["bounds"]["min"].is_array() && op["bounds"]["max"].is_array(),
        "{op}"
    );
    let map = &json["maps"][0];
    for key in [
        "map",
        "path",
        "width",
        "height",
        "non_black_texels",
        "created",
    ] {
        assert!(!map[key].is_null(), "maps[0].{key} is missing: {map}");
    }
}

#[test]
fn a_written_tree_checks_green_and_drifts_on_one_byte() {
    let dir = tree("drift");
    let derived = derived(&dir);
    let written = derived.write().expect("the tree is written");
    assert_eq!(written.len(), 2);
    assert!(derived.check().is_empty(), "{:#?}", derived.check());

    // One byte of the PNG.
    let png = written[1].clone();
    let mut bytes = std::fs::read(&png).expect("the map is there");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    write(&png, &bytes);
    let drifted = derived.check();
    assert_eq!(drifted.len(), 1, "{drifted:#?}");
    assert_eq!(drifted[0].code.as_str(), "L1173");
    assert!(drifted[0].message.starts_with("DRIFT "), "{:?}", drifted[0]);

    // And the document.
    derived.write().expect("the tree is rewritten");
    let document = written[0].clone();
    let text = std::fs::read_to_string(&document).expect("the document is there");
    write(
        &document,
        text.replace("name: plate", "name: plate2").as_bytes(),
    );
    let drifted = derived.check();
    assert_eq!(drifted.len(), 1, "{drifted:#?}");
    assert!(
        drifted[0].message.contains("mesh.mesh.yaml"),
        "{:?}",
        drifted[0]
    );
}

/// A map an earlier apply produced and this one does not is the derive's own to remove; a file it
/// never named is not, and stops the write dead.
#[test]
fn the_write_owns_its_own_names_and_refuses_the_rest() {
    let dir = tree("owned");
    let first = derived(&dir);
    first.write().expect("the tree is written");
    let stale = first.files()[1].0.to_path_buf();

    let edit = edit_path(&dir);
    let text = std::fs::read_to_string(&edit).expect("the edit is there");
    write(
        &edit,
        text.replace("radius: 0.15", "radius: 0.3").as_bytes(),
    );
    let second = derived(&dir);
    second.write().expect("the wider tree is written");
    assert!(!stale.exists(), "the map the first apply wrote is gone");
    assert!(second.check().is_empty(), "{:#?}", second.check());

    write(&second.out_dir().join("notes.md"), b"hand written");
    let refused = second.write().expect_err("a foreign file stops the write");
    assert_eq!(refused[0].code.as_str(), "L1176");
    assert!(refused[0].message.contains("notes.md"), "{:?}", refused[0]);
    // The check reports it too, as drift rather than as a refusal.
    let drifted = second.check();
    assert_eq!(drifted.len(), 1, "{drifted:#?}");
    assert!(drifted[0].message.contains("notes.md"), "{:?}", drifted[0]);
}

/// A crash mid-write can leave a hidden temp beside the maps; the next write sweeps it.
#[test]
fn a_leftover_temp_is_swept_not_refused() {
    let dir = tree("leftover");
    let first = derived(&dir);
    first.write().expect("the tree is written");
    let leftover = first
        .out_dir()
        .join("textures")
        .join(".0123456789abcdef.png.1.0.tmp");
    write(&leftover, b"half");

    let second = derived(&dir);
    second
        .write()
        .expect("a leftover temp does not refuse the write");
    assert!(!leftover.exists(), "the leftover temp is swept");
    assert!(second.check().is_empty(), "{:#?}", second.check());
}

/// Only a temp of a file this edit owns is swept; any other `.*.tmp` is someone's file.
#[test]
fn a_temp_of_a_file_the_edit_does_not_own_is_refused() {
    let dir = tree("foreign-temp");
    let first = derived(&dir);
    first.write().expect("the tree is written");
    let foreign = first.out_dir().join(".notes.md.1.0.tmp");
    write(&foreign, b"mine");

    let refused = derived(&dir)
        .write()
        .expect_err("a foreign temp stops the write");
    assert_eq!(refused[0].code.as_str(), "L1176");
    assert!(foreign.exists(), "the foreign temp is left alone");
}

/// An unreadable derived document is a diagnostic, never a silent overwrite.
#[test]
fn an_unreadable_derived_document_is_not_overwritten() {
    let dir = tree("unreadable-owner");
    let first = derived(&dir);
    std::fs::create_dir_all(first.out_dir().join("mesh.mesh.yaml")).expect("the obstacle exists");
    let refused = first.write().expect_err("the owner cannot be read");
    assert_eq!(refused[0].code.as_str(), "L1015");
}

/// `Self/x` and `self` are one directory on a case-insensitive filesystem.
#[test]
fn a_source_inside_the_out_in_another_case_is_l1174() {
    let dir = tree("source-in-out-case");
    let edit = edit_path(&dir);
    let text = std::fs::read_to_string(&edit).expect("the edit is there");
    write(
        &edit,
        text.replace(
            "source: plate.mesh.yaml",
            "source: PLATE-lit/mesh.mesh.yaml",
        )
        .as_bytes(),
    );
    let refused = derive(&edit).expect_err("the source lies in the out");
    assert_eq!(refused[0].code.as_str(), "L1174");
    assert_eq!(refused[0].path.as_deref(), Some("/source"));
}

#[test]
fn an_out_with_a_drive_segment_is_l1174_and_writes_nothing() {
    let dir = tree("out-drive");
    let edit = edit_path(&dir);
    let text = std::fs::read_to_string(&edit).expect("the edit is there");
    write(
        &edit,
        text.replace(
            "source: plate.mesh.yaml",
            "source: plate.mesh.yaml\nout: lit/C:/x",
        )
        .as_bytes(),
    );
    let refused = derive(&edit).expect_err("the out carries a drive");
    assert_eq!(refused[0].code.as_str(), "L1174");
    assert_eq!(refused[0].path.as_deref(), Some("/out"));
    assert!(!dir.join("meshes/lit").exists());
}

#[test]
fn an_op_that_paints_nothing_is_l1177() {
    let dir = tree("nothing");
    let edit = edit_path(&dir);
    let text = std::fs::read_to_string(&edit).expect("the edit is there");
    write(
        &edit,
        text.replace(
            "          - 0.25\n          - 0.0\n",
            "          - 9.0\n          - 9.0\n",
        )
        .as_bytes(),
    );
    let refused = derive(&edit).expect_err("the sphere is nowhere near the plate");
    assert_eq!(refused[0].code.as_str(), "L1177");
    assert_eq!(refused[0].path.as_deref(), Some("/ops/0"));
    assert!(refused[0].message.contains("op 0"), "{:?}", refused[0]);
    assert!(
        refused[0].message.contains("while the mesh spans"),
        "{:?}",
        refused[0]
    );
}

/// The created map takes the base-colour map's dimensions when the op does not say, so dropping the
/// `size:` from the fixture derives exactly the same tree.
#[test]
fn a_created_map_defaults_to_the_base_colour_dimensions() {
    let dir = tree("default-size");
    let sized = emissive_png(&derived(&dir));

    let edit = edit_path(&dir);
    let text = std::fs::read_to_string(&edit).expect("the edit is there");
    write(&edit, text.replace("    size: 64\n", "").as_bytes());
    assert_eq!(emissive_png(&derived(&dir)), sized);
}

#[test]
fn a_size_on_a_map_that_already_exists_is_l1174() {
    let dir = tree("resize");
    let edit = edit_path(&dir);
    let text = std::fs::read_to_string(&edit).expect("the edit is there");
    write(
        &edit,
        text.replace("    strength: 0.75\n", "    size: 32\n    strength: 0.75\n")
            .as_bytes(),
    );
    let refused = derive(&edit).expect_err("the second op cannot resize the map");
    assert_eq!(refused[0].code.as_str(), "L1174");
    assert_eq!(refused[0].path.as_deref(), Some("/ops/1/size"));
}

#[test]
fn a_source_without_uvs_is_l1176() {
    let dir = tree("no-uvs");
    write(&dir.join("meshes/plate.mesh.yaml"), NO_UVS_MESH.as_bytes());
    let refused = derive(&edit_path(&dir)).expect_err("there is nowhere to paint");
    assert_eq!(refused[0].code.as_str(), "L1176");
    assert!(refused[0].message.contains("`uvs`"), "{:?}", refused[0]);
}

#[test]
fn a_missing_source_names_the_source_key() {
    let dir = tree("missing");
    std::fs::remove_file(dir.join("meshes/plate.mesh.yaml")).expect("the source is removed");
    let refused = derive(&edit_path(&dir)).expect_err("the source is gone");
    assert_eq!(refused[0].code.as_str(), "L1174");
    assert_eq!(refused[0].path.as_deref(), Some("/source"));
}

/// One op painting the plate's whole surface, its region `shape` followed by `extra` lines
/// (a `key:` block, or nothing).
fn whole_plate_edit(extra: &str) -> String {
    format!(
        "\
format: 1
kind: mesh-edit
name: plate-lit
source: plate.mesh.yaml
ops:
  - op: paint
    map: emissive
    size: 64
    color:
      - 1.0
      - 1.0
      - 1.0
    region:
      sphere:
        at:
          - 0.5
          - 0.0
          - 0.5
        radius: 2.0
{extra}"
    )
}

/// A key block reading `map` for `grey` (a byte) within `tolerance`.
fn key_block(map: &str, grey: u8, tolerance: f64) -> String {
    let level = f64::from(grey) / 255.0;
    format!(
        "      key:\n        map: {map}\n        color:\n          - {level}\n          - {level}\n          - {level}\n        tolerance: {tolerance}\n"
    )
}

/// A base map grey on its left half and white on its right, so a key can keep one half of a region.
fn two_tone_base() -> Vec<u8> {
    let mut rgba = Vec::new();
    for _y in 0..64 {
        for x in 0..64 {
            let level = if x < 32 { 128 } else { 255 };
            rgba.extend_from_slice(&[level, level, level, 255]);
        }
    }
    ludion_assets::encode_png(64, 64, &rgba).expect("the encoder takes a 64x64 image")
}

fn write_edit(dir: &Path, text: &str) {
    write(&edit_path(dir), text.as_bytes());
}

#[test]
fn a_key_keeps_only_the_matching_texels_of_its_region() {
    let dir = tree("key-partial");
    write(&dir.join("textures/base.png"), &two_tone_base());

    write_edit(&dir, &whole_plate_edit(""));
    let all = derived(&dir).report().ops[0].texels;

    write_edit(&dir, &whole_plate_edit(&key_block("base_color", 128, 0.05)));
    let kept = derived(&dir).report().ops[0].texels;
    assert!(kept > 0 && kept < all, "{kept} of {all}");
    assert_eq!(kept * 2, all, "the grey half is half the plate");
}

#[test]
fn a_key_that_matches_nothing_is_l1178() {
    let dir = tree("key-none");
    write_edit(&dir, &whole_plate_edit(&key_block("base_color", 0, 0.01)));
    let refused = derive(&edit_path(&dir)).expect_err("the plate has no black texel");
    assert_eq!(refused[0].code.as_str(), "L1178");
    assert_eq!(refused[0].path.as_deref(), Some("/ops/0/region/key"));
    assert!(refused[0].message.contains("op 0"), "{:?}", refused[0]);
    assert!(
        refused[0].message.contains("region reaches"),
        "{:?}",
        refused[0]
    );
    assert!(
        !refused[0].message.contains("  ")
            && !refused[0].help.as_deref().unwrap_or("").contains("  "),
        "{:?}",
        refused[0]
    );
}

#[test]
fn a_key_on_a_map_nothing_provides_is_l1176() {
    let dir = tree("key-missing-map");
    write_edit(&dir, &whole_plate_edit(&key_block("normal", 128, 0.05)));
    let refused = derive(&edit_path(&dir)).expect_err("the source has no normal map");
    assert_eq!(refused[0].code.as_str(), "L1176");
    assert_eq!(refused[0].path.as_deref(), Some("/ops/0/region/key/map"));
}

// --- fill ---------------------------------------------------------------------------------------

/// A base map 32 texels wide, grey up to column 20 and white beyond: the fill's tolerance keeps the
/// grey, which spans the left island and the first columns of the right one.
fn strip_base() -> Vec<u8> {
    let mut rgba = Vec::new();
    for _y in 0..32 {
        for x in 0..32 {
            let level = if x < 20 { 128 } else { 255 };
            rgba.extend_from_slice(&[level, level, level, 255]);
        }
    }
    ludion_assets::encode_png(32, 32, &rgba).expect("the encoder takes a 32x32 image")
}

/// The strip's tree: two quads with duplicated seam vertices and the fill edit that walks across.
fn strip_tree(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-mesh-edit-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).expect("the tree is created");
    std::fs::create_dir_all(dir.join("textures")).expect("the tree is created");
    write(&dir.join("meshes/strip.mesh.yaml"), STRIP_MESH.as_bytes());
    write(
        &dir.join("meshes/strip-fill.mesh-edit.yaml"),
        STRIP_FILL_EDIT.as_bytes(),
    );
    write(&dir.join("textures/base.png"), &strip_base());
    dir
}

fn strip_derived(dir: &Path) -> Derived {
    derive(&dir.join("meshes/strip-fill.mesh-edit.yaml"))
        .unwrap_or_else(|diagnostics| panic!("{diagnostics:#?}"))
}

#[test]
fn the_strip_fill_derives_to_the_conformance_hash() {
    let dir = strip_tree("strip-conformance");
    let hash = ludion_assets::content_hash(&emissive_png(&strip_derived(&dir)));
    assert_eq!(
        hash, STRIP_FILL_HASH,
        "the derived emissive map hashes to {hash:#018x}; a difference between the two pinned \
         platforms is a determinism break in the weld, the seam bridge or the flood"
    );
}

#[test]
fn deriving_a_fill_twice_is_bit_identical() {
    let dir = strip_tree("strip-twice");
    assert_eq!(
        emissive_png(&strip_derived(&dir)),
        emissive_png(&strip_derived(&dir))
    );
}

/// The seed is on the left quad and the base is grey out to column 20: the flood only reaches the
/// right quad by the weld across the seam, and stops at the tone change.
#[test]
fn the_strip_fill_crosses_the_seam_and_stops_at_the_tone() {
    let dir = strip_tree("strip-report");
    let derived = strip_derived(&dir);
    let op = &derived.report().ops[0];
    let bounds = op.bounds.expect("the fill painted something");
    assert!(bounds.min[0] < 1.0, "it painted the left quad: {bounds:?}");
    assert!(
        bounds.max[0] > 1.0,
        "it crossed into the right quad: {bounds:?}"
    );
    // The right quad's grey columns are 18 and 19 of its 18..=28; nothing white is painted, so the
    // painted surface ends where the tone does.
    assert!(
        bounds.max[0] < 1.3,
        "it stopped at the white texels: {bounds:?}"
    );
    assert_eq!(op.triangles, 4);
}

fn plate_fill_edit(fill: &str, key: &str) -> String {
    format!(
        "\
format: 1
kind: mesh-edit
name: plate-lit
source: plate.mesh.yaml
ops:
  - op: paint
    map: emissive
    size: 64
    color:
      - 1.0
      - 1.0
      - 1.0
    region:
      fill:
{fill}{key}"
    )
}

fn fill_at(at: [f64; 3], max_radius: f64, extra: &str) -> String {
    format!(
        "        at:\n          - {}\n          - {}\n          - {}\n        tolerance: 0.1\n        max_radius: {max_radius}\n{extra}",
        at[0], at[1], at[2]
    )
}

/// A hard-edged fill on the plate's uniform base reaches exactly the texels a straight-line radius
/// does, and its report stays inside the radius' box.
#[test]
fn a_hard_fill_reports_the_radius_count() {
    let dir = tree("fill-count");
    write_edit(
        &dir,
        &plate_fill_edit(&fill_at([0.5, 0.0, 0.5], 0.25, ""), ""),
    );
    let report = derived(&dir).report().clone();
    let op = &report.ops[0];

    let mesh = ludion_scene::check_mesh(PLATE_MESH).expect("the plate parses");
    let uvs = mesh.uvs.as_deref().expect("the plate has uvs");
    let coverage = ludion_mesh_edit::raster::rasterize(&mesh.positions, uvs, &mesh.indices, 64, 64)
        .expect("the plate rasterises");
    let inside = (0..coverage.len())
        .filter_map(|index| coverage.at(index))
        .filter(|(_, point)| {
            let offset = [point[0] - 0.5, point[1], point[2] - 0.5];
            let squared = offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2];
            ludion_core::math::f64::sqrt(squared) <= 0.25
        })
        .count();
    assert_eq!(op.texels, inside);
    assert!(op.triangles >= 1);
    let bounds = op.bounds.expect("the fill painted something");
    assert!(bounds.min[0] >= 0.25 && bounds.max[0] <= 0.75, "{bounds:?}");
    assert!(bounds.min[2] >= 0.25 && bounds.max[2] <= 0.75, "{bounds:?}");
}

#[test]
fn a_seed_off_the_surface_is_l1178() {
    let dir = tree("fill-off");
    write_edit(
        &dir,
        &plate_fill_edit(&fill_at([0.5, 0.1, 0.5], 0.25, ""), ""),
    );
    let refused = derive(&edit_path(&dir)).expect_err("the seed floats above the plate");
    assert_eq!(refused[0].code.as_str(), "L1178");
    assert_eq!(refused[0].path.as_deref(), Some("/ops/0/region/fill/at"));
    assert!(refused[0].message.contains("0.1"), "{:?}", refused[0]);
    assert!(!refused[0].message.contains("  "), "{:?}", refused[0]);
    assert!(
        refused[0]
            .help
            .as_deref()
            .is_some_and(|help| help.contains("mesh pick"))
    );
}

#[test]
fn a_seed_outside_its_radius_is_l1178() {
    let dir = tree("fill-radius");
    write_edit(
        &dir,
        &plate_fill_edit(&fill_at([0.5, 0.0, 0.5], 0.001, ""), ""),
    );
    let refused = derive(&edit_path(&dir)).expect_err("no texel centre is within a millimetre");
    assert_eq!(refused[0].code.as_str(), "L1178");
    assert_eq!(
        refused[0].path.as_deref(),
        Some("/ops/0/region/fill/max_radius")
    );
}

#[test]
fn a_fill_reading_a_map_nothing_provides_is_l1176() {
    let dir = tree("fill-missing-map");
    write_edit(
        &dir,
        &plate_fill_edit(&fill_at([0.5, 0.0, 0.5], 0.25, "        map: normal\n"), ""),
    );
    let refused = derive(&edit_path(&dir)).expect_err("the source has no normal map");
    assert_eq!(refused[0].code.as_str(), "L1176");
    assert_eq!(refused[0].path.as_deref(), Some("/ops/0/region/fill/map"));
}

/// The key composes with the walk: it thins the fill's texels, and never gates the walk itself.
#[test]
fn a_key_thins_a_fill() {
    let dir = tree("fill-key");
    write(&dir.join("textures/base.png"), &two_tone_base());
    let fill = fill_at([0.3, 0.0, 0.5], 1.0, "");
    let tolerance = fill.replace("tolerance: 0.1", "tolerance: 1.0");

    write_edit(&dir, &plate_fill_edit(&tolerance, ""));
    let all = derived(&dir).report().ops[0].texels;
    write_edit(
        &dir,
        &plate_fill_edit(&tolerance, &key_block("base_color", 128, 0.05)),
    );
    let kept = derived(&dir).report().ops[0].texels;
    assert!(kept > 0 && kept < all, "{kept} of {all}");
}

// --- ownership and write order ------------------------------------------------------------------

/// A second edit whose `out` names the tree the fixture edit already owns, in a different colour.
fn copy_edit_into_plate_lit(dir: &Path) -> PathBuf {
    let text = std::fs::read_to_string(edit_path(dir)).expect("the edit is there");
    let copy = dir.join("meshes/plate-copy.mesh-edit.yaml");
    write(
        &copy,
        text.replace(
            "source: plate.mesh.yaml\n",
            "source: plate.mesh.yaml\nout: plate-lit\n",
        )
        .replace(
            "      - 0.05\n      - 1.0\n      - 0.35\n",
            "      - 1.0\n      - 0.0\n      - 0.0\n",
        )
        .as_bytes(),
    );
    copy
}

#[test]
fn a_tree_another_edit_owns_is_refused() {
    let dir = tree("foreign-owner");
    let original = derived(&dir);
    original.write().expect("the tree is written");

    let copy = copy_edit_into_plate_lit(&dir);
    let second = derive(&copy).unwrap_or_else(|diagnostics| panic!("{diagnostics:#?}"));
    let refused = second
        .write()
        .expect_err("the tree belongs to the other edit");
    assert_eq!(refused[0].code.as_str(), "L1176");
    assert!(
        refused[0].message.contains("plate-lit.mesh-edit.yaml"),
        "{:?}",
        refused[0]
    );
    assert!(original.check().is_empty(), "{:#?}", original.check());
}

#[test]
fn a_source_inside_the_edits_own_out_is_l1174() {
    let dir = tree("source-in-out");
    derived(&dir).write().expect("the tree is written");
    let edit = edit_path(&dir);
    let text = std::fs::read_to_string(&edit).expect("the edit is there");
    write(
        &edit,
        text.replace(
            "source: plate.mesh.yaml",
            "source: plate-lit/mesh.mesh.yaml",
        )
        .as_bytes(),
    );
    let refused = derive(&edit).expect_err("the source is rewritten by its own derive");
    assert_eq!(refused[0].code.as_str(), "L1174");
    assert_eq!(refused[0].path.as_deref(), Some("/source"));
}

#[test]
fn a_failed_write_leaves_the_previous_tree_whole() {
    let dir = tree("failed-write");
    let first = derived(&dir);
    first.write().expect("the tree is written");

    let edit = edit_path(&dir);
    let text = std::fs::read_to_string(&edit).expect("the edit is there");
    write(
        &edit,
        text.replace("radius: 0.15", "radius: 0.3").as_bytes(),
    );
    let second = derived(&dir);
    // A directory where the new map goes makes its write fail.
    std::fs::create_dir_all(second.files()[1].0).expect("the obstacle is created");
    second.write().expect_err("the map cannot be written");
    assert!(first.check().is_empty(), "{:#?}", first.check());
}

#[test]
fn a_region_that_reaches_texels_at_zero_weight_does_not_blame_the_region() {
    let dir = tree("zero-strength");
    let edit = edit_path(&dir);
    let text = std::fs::read_to_string(&edit).expect("the edit is there");
    write(
        &edit,
        text.replacen("    region:\n", "    strength: 0.0\n    region:\n", 1)
            .as_bytes(),
    );
    let refused = derive(&edit).expect_err("a zero strength paints nothing");
    assert_eq!(refused[0].code.as_str(), "L1177");
    assert!(
        !refused[0].message.contains("while the mesh spans"),
        "{:?}",
        refused[0]
    );
}

#[test]
fn an_oversized_base_colour_map_points_at_map_not_size() {
    let dir = tree("huge-base");
    let rgba = [128u8, 128, 128, 255].repeat(4097);
    write(
        &dir.join("textures/base.png"),
        &ludion_assets::encode_png(4097, 1, &rgba).expect("the encoder takes 4097x1"),
    );
    let edit = edit_path(&dir);
    let text = std::fs::read_to_string(&edit).expect("the edit is there");
    write(&edit, text.replace("    size: 64\n", "").as_bytes());
    let refused = derive(&edit).expect_err("4097 texels are out of range");
    assert_eq!(refused[0].code.as_str(), "L1174");
    assert_eq!(refused[0].path.as_deref(), Some("/ops/0/map"));
}
