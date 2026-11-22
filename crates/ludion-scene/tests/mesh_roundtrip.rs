//! Round-trip and canonical-form properties for mesh asset documents (`meshes/*.mesh.yaml`,
//! Phase 8 Wave 1) — the mesh analog of `roundtrip.rs`:
//!
//! - **value round trip**: mesh → emit → check loses nothing;
//! - **emit idempotence**: `emit_mesh(parse_mesh(emit_mesh(d)))` is *byte-equal* to `emit_mesh(d)`;
//! - the emitted text is LF-only and validates cleanly;
//! - a hand-authored fixture pins the exact canonical text.

use ludion_scene::{
    AlphaMode, MeshDocument, MeshTextures, TextureRef, TextureWrap, check_mesh, emit_mesh,
    parse_mesh, validate_mesh,
};
use proptest::prelude::*;
#[path = "support/deterministic.rs"]
mod deterministic;
use deterministic::deterministic_config;

// --- Strategies -------------------------------------------------------------------------------

/// Finite once narrowed to `f32`, the range the kind readers accept.
fn finite_f64() -> impl Strategy<Value = f64> {
    any::<f64>().prop_filter("finite as f32", |f| (*f as f32).is_finite())
}

/// Valid `roughness`/`metallic` values: perceptual, `[0, 1]`.
fn unit_f64() -> impl Strategy<Value = f64> {
    0.0f64..=1.0f64
}

/// Valid `emissive` values: a non-negative HDR intensity multiplier.
fn non_negative_f64() -> impl Strategy<Value = f64> {
    0.0f64..1_000.0f64
}

/// Valid triangle indices for a mesh with `vertex_count` vertices: empty when there are no
/// vertices to index, otherwise a flat list of 0..6 triangles' worth of in-range indices.
fn indices_for(vertex_count: usize) -> BoxedStrategy<Vec<u32>> {
    if vertex_count == 0 {
        return Just(Vec::new()).boxed();
    }
    let max_index = vertex_count as u32;
    (0usize..6)
        .prop_flat_map(move |triangle_count| {
            proptest::collection::vec(0..max_index, triangle_count * 3)
        })
        .boxed()
}

/// `rows` of `stride` floats with every leading xyz made non-degenerate: the validator refuses a
/// normal or tangent of near-zero length.
fn non_degenerate(mut flat: Vec<f64>, stride: usize) -> Vec<f64> {
    for row in flat.chunks_exact_mut(stride) {
        if row[..3].iter().map(|c| c * c).sum::<f64>() < 1e-9 {
            row[0] = 1.0;
        }
    }
    flat
}

/// Valid tangents for a mesh with `vertex_count` vertices: 4 floats per vertex, the handedness `w`
/// exactly `-1` or `1`.
fn tangents_for(vertex_count: usize) -> impl Strategy<Value = Vec<f64>> {
    let vertex = (
        finite_f64(),
        finite_f64(),
        finite_f64(),
        prop_oneof![Just(-1.0f64), Just(1.0f64)],
    );
    proptest::collection::vec(vertex, vertex_count).prop_map(|rows| {
        rows.into_iter()
            .flat_map(|(x, y, z, w)| [x, y, z, w])
            .collect()
    })
}

/// One texture reference. `with_scalar` says whether the slot has a `scale`/`strength` key: a slot
/// without one can only ever hold the identity factor, since there is nowhere to write another.
fn texture_ref(with_scalar: bool) -> impl Strategy<Value = TextureRef> {
    let factor: BoxedStrategy<f64> = if with_scalar {
        prop_oneof![Just(1.0f64), Just(0.5f64), Just(0.0f64), Just(2.5f64)].boxed()
    } else {
        Just(1.0f64).boxed()
    };
    let wrap = prop_oneof![
        Just(TextureWrap::Repeat),
        Just(TextureWrap::Clamp),
        Just(TextureWrap::Mirror),
    ];
    ("[a-c]{1,4}", wrap, factor).prop_map(|(stem, wrap, factor)| TextureRef {
        path: format!("textures/{stem}.png"),
        wrap,
        factor,
    })
}

fn mesh_textures() -> impl Strategy<Value = MeshTextures> {
    (
        proptest::option::of(texture_ref(false)),
        proptest::option::of(texture_ref(false)),
        proptest::option::of(texture_ref(true)),
        proptest::option::of(texture_ref(true)),
        proptest::option::of(texture_ref(false)),
    )
        .prop_map(
            |(base_color, metallic_roughness, normal, occlusion, emissive)| MeshTextures {
                base_color,
                metallic_roughness,
                normal,
                occlusion,
                emissive,
            },
        )
}

/// A structurally consistent [`MeshDocument`]: `normals`/`uvs`/`tangents`/`emissive`/`metallic`/
/// `roughness`, when present, always agree in count with `positions` and stay in their valid range,
/// a referenced texture always has the UVs (and, for a normal map, the tangents) it needs, and every
/// index is in range — properties [`validate_mesh`] enforces.
fn mesh_doc() -> impl Strategy<Value = MeshDocument> {
    (0usize..8).prop_flat_map(|vertex_count| {
        let positions = proptest::collection::vec(finite_f64(), vertex_count * 3);
        let normals = proptest::option::of(
            proptest::collection::vec(finite_f64(), vertex_count * 3)
                .prop_map(|flat| non_degenerate(flat, 3)),
        );
        let uvs = proptest::option::of(proptest::collection::vec(finite_f64(), vertex_count * 2));
        let tangents = proptest::option::of(
            tangents_for(vertex_count).prop_map(|flat| non_degenerate(flat, 4)),
        );
        let emissive =
            proptest::option::of(proptest::collection::vec(non_negative_f64(), vertex_count));
        let metallic = proptest::option::of(proptest::collection::vec(unit_f64(), vertex_count));
        let roughness = proptest::option::of(proptest::collection::vec(unit_f64(), vertex_count));
        let indices = indices_for(vertex_count);
        let name = proptest::option::of(any::<String>());
        let generated_from = proptest::option::of(any::<String>());
        let alpha_mode = prop_oneof![
            Just(AlphaMode::Opaque),
            Just(AlphaMode::Blend),
            (0.0f64..=1.0).prop_map(|cutoff| AlphaMode::Mask { cutoff }),
        ];
        (
            (name, generated_from, positions, normals, uvs, tangents),
            (emissive, metallic, roughness, mesh_textures(), indices),
            (alpha_mode, any::<bool>()),
        )
            .prop_map(
                move |(
                    (name, generated_from, positions, normals, uvs, tangents),
                    (emissive, metallic, roughness, textures, indices),
                    (alpha_mode, double_sided),
                )| {
                    let mut doc = MeshDocument {
                        name,
                        generated_from,
                        origin: None,
                        positions,
                        normals,
                        uvs,
                        tangents,
                        emissive,
                        metallic,
                        porosity: None,
                        roughness,
                        textures,
                        alpha_mode,
                        double_sided,
                        skin: None,
                        indices,
                        rim: None,
                        lods: Vec::new(),
                        surfaces: None,
                    };
                    // The two cross-key requirements are satisfied here rather than by conditioning
                    // the texture strategy on the vertex lists: a texture needs UVs to sample at,
                    // and a normal map needs a tangent frame.
                    if doc.textures != MeshTextures::default() && doc.uvs.is_none() {
                        doc.uvs = Some(vec![0.0; vertex_count * 2]);
                    }
                    if doc.textures.normal.is_some() && doc.tangents.is_none() {
                        doc.tangents = Some(
                            (0..vertex_count)
                                .flat_map(|_| [1.0, 0.0, 0.0, 1.0])
                                .collect(),
                        );
                    }
                    doc
                },
            )
    })
}

// --- Properties -------------------------------------------------------------------------------

proptest! {
    #![proptest_config(deterministic_config())]
    #[test]
    fn emit_then_check_loses_nothing(doc in mesh_doc()) {
        let text = emit_mesh(&doc);
        let checked = check_mesh(&text).expect("canonical output must check clean");
        prop_assert_eq!(checked, doc);
    }

    #[test]
    fn emit_is_idempotent_byte_for_byte(doc in mesh_doc()) {
        let first = emit_mesh(&doc);
        let reparsed = parse_mesh(&first).expect("canonical output must parse cleanly");
        let second = emit_mesh(&reparsed);
        prop_assert_eq!(&second, &first);
        // And once stable, it stays stable.
        let third = emit_mesh(&parse_mesh(&second).unwrap());
        prop_assert_eq!(&third, &first);
    }

    #[test]
    fn emitted_text_is_lf_only_and_validates(doc in mesh_doc()) {
        let text = emit_mesh(&doc);
        prop_assert!(!text.contains('\r'), "canonical output must use LF only");
        prop_assert!(text.ends_with('\n'), "canonical output ends with a newline");
        let parsed = parse_mesh(&text).expect("canonical output must parse cleanly");
        let diagnostics = validate_mesh(&parsed);
        prop_assert!(
            diagnostics.is_empty(),
            "canonical output of a consistent mesh must validate: {:?}",
            diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }
}

// --- Deterministic golden shape ---------------------------------------------------------------

/// One non-property test pinning the exact canonical text: field order is
/// `format, kind, name, positions, normals, uvs, indices`, each flat list one entry per line,
/// floats are ryu-shortest, `name` quoting follows the same rule as scene strings.
#[test]
fn canonical_form_golden() {
    let source = "\
format: 1
kind: mesh
name: no
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
normals:
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
uvs:
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
indices:
  - 0
  - 1
  - 2
";
    let mesh = parse_mesh(source).expect("mesh should parse");
    assert!(validate_mesh(&mesh).is_empty());
    let canonical = emit_mesh(&mesh);
    assert_eq!(
        canonical,
        "\
format: 1
kind: mesh
name: \"no\"
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
normals:
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
uvs:
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
indices:
  - 0
  - 1
  - 2
"
    );
    // Emission is a fixed point.
    assert_eq!(emit_mesh(&parse_mesh(&canonical).unwrap()), canonical);
}

/// The material lists emit between `uvs` and `indices`, alphabetically (`emissive`, `metallic`,
/// `roughness`), only when authored — a mesh with none of the three emits exactly as
/// [`canonical_form_golden`], so old files stay byte-stable.
#[test]
fn canonical_form_golden_with_material_lists() {
    let source = "\
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
  - 1.0
  - 0.0
uvs:
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
emissive:
  - 2.0
  - 0.0
  - 0.0
metallic:
  - 1.0
  - 0.5
  - 0.0
roughness:
  - 0.1
  - 0.5
  - 1.0
indices:
  - 0
  - 1
  - 2
";
    let mesh = parse_mesh(source).expect("mesh should parse");
    assert!(validate_mesh(&mesh).is_empty());
    let canonical = emit_mesh(&mesh);
    assert_eq!(canonical, source);
    // Emission is a fixed point.
    assert_eq!(emit_mesh(&parse_mesh(&canonical).unwrap()), canonical);
}

/// `tangents` emits after `uvs` (it is geometry) and the five texture references after `roughness`,
/// in the fixed order `base_color`, `metallic_roughness`, `normal`, `occlusion`, `emissive`. Each
/// reference writes `path` always, `wrap` only when it is not glTF's `repeat` default, and the map's
/// own scalar only when it is not `1`.
#[test]
fn canonical_form_golden_with_tangents_and_textures() {
    let source = "\
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
  - 1.0
  - 0.0
uvs:
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
tangents:
  - 1.0
  - 0.0
  - 0.0
  - 1.0
  - 1.0
  - 0.0
  - 0.0
  - -1.0
  - 0.0
  - 0.0
  - 1.0
  - 1.0
roughness:
  - 0.5
  - 0.5
  - 0.5
base_color_texture:
  path: textures/0011223344556677.png
metallic_roughness_texture:
  path: textures/8899aabbccddeeff.png
  wrap: clamp
normal_texture:
  path: textures/00112233445566aa.png
  scale: 0.8
occlusion_texture:
  path: textures/00112233445566bb.png
  strength: 0.5
emissive_texture:
  path: textures/00112233445566cc.png
indices:
  - 0
  - 1
  - 2
";
    let mesh = parse_mesh(source).expect("mesh should parse");
    assert!(validate_mesh(&mesh).is_empty());
    let canonical = emit_mesh(&mesh);
    assert_eq!(canonical, source);
    // Emission is a fixed point.
    assert_eq!(emit_mesh(&parse_mesh(&canonical).unwrap()), canonical);
}

/// `L1111`: `tangents` is 4 floats per vertex, and the handedness `w` is a sign, not a magnitude.
#[test]
fn bad_tangents_are_l1111() {
    let short = MeshDocument {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        tangents: Some(vec![1.0, 0.0, 0.0, 1.0]),
        indices: vec![0, 1, 2],
        ..Default::default()
    };
    let diagnostics = validate_mesh(&short);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1111" && d.path.as_deref() == Some("/tangents"))
    );

    let bad_handedness = MeshDocument {
        positions: vec![0.0, 0.0, 0.0],
        tangents: Some(vec![1.0, 0.0, 0.0, 0.5]),
        indices: vec![0, 0, 0],
        ..Default::default()
    };
    let diagnostics = validate_mesh(&bad_handedness);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1111" && d.path.as_deref() == Some("/tangents/3"))
    );
}

/// `L1111`: a texture has nowhere to sample from without `uvs`, and a tangent-space normal map has
/// no frame without `tangents`.
#[test]
fn a_texture_without_its_geometry_is_l1111() {
    let no_uvs = MeshDocument {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        textures: MeshTextures {
            base_color: Some(TextureRef::new("textures/0011223344556677.png")),
            ..Default::default()
        },
        indices: vec![0, 1, 2],
        ..Default::default()
    };
    let diagnostics = validate_mesh(&no_uvs);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1111"
                && d.path.as_deref() == Some("/base_color_texture"))
    );

    let no_tangents = MeshDocument {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        uvs: Some(vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
        textures: MeshTextures {
            normal: Some(TextureRef::new("textures/0011223344556677.png")),
            ..Default::default()
        },
        indices: vec![0, 1, 2],
        ..Default::default()
    };
    let diagnostics = validate_mesh(&no_tangents);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1111" && d.path.as_deref() == Some("/normal_texture"))
    );
}

/// `L1110`: an unknown `wrap` spelling, and a per-map scalar spelled on a map that has none.
#[test]
fn bad_texture_keys_are_l1110() {
    let bad_wrap = "\
format: 1
kind: mesh
positions: []
uvs: []
base_color_texture:
  path: textures/0011223344556677.png
  wrap: clampp
indices: []
";
    let diagnostics = parse_mesh(bad_wrap).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/base_color_texture/wrap"))
        .expect("expected a /base_color_texture/wrap diagnostic");
    assert_eq!(d.code.as_str(), "L1110");
    assert_eq!(d.help.as_deref(), Some("nearest match is `clamp`"));

    let misplaced_scalar = "\
format: 1
kind: mesh
positions: []
uvs: []
base_color_texture:
  path: textures/0011223344556677.png
  strength: 0.5
indices: []
";
    let diagnostics = parse_mesh(misplaced_scalar).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/base_color_texture/strength"))
        .expect("expected a /base_color_texture/strength diagnostic");
    assert_eq!(d.code.as_str(), "L1110");
    assert_eq!(d.help.as_deref(), Some("valid keys are: path, wrap"));
}

/// A mesh with no optional fields emits the minimal shape (no `name`/`normals`/`uvs` lines, empty
/// `positions`/`indices` as flow `[]`).
#[test]
fn minimal_mesh_omits_optional_fields() {
    let doc = MeshDocument::default();
    assert_eq!(
        emit_mesh(&doc),
        "format: 1\nkind: mesh\npositions: []\nindices: []\n"
    );
}

/// A position past the f32 range reads as infinite, so validation refuses it with `L1111` at its
/// own pointer rather than letting it narrow to `inf` later.
#[test]
fn a_position_past_the_f32_range_is_l1111() {
    let source = "format: 1\nkind: mesh\npositions:\n  - 1e39\n  - 0.0\n  - 0.0\nindices: []\n";
    let diagnostics = check_mesh(source).expect_err("1e39 does not fit a 32-bit float");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1111" && d.path.as_deref() == Some("/positions/0")),
        "{diagnostics:?}"
    );
}

/// `L1111` on inconsistent data: a truncated `uvs` list and an out-of-range index, both reported
/// in one pass with JSON Pointers to the exact offending list.
#[test]
fn bad_indices_and_counts_are_l1111() {
    let source = "\
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
  - 1.0
  - 0.0
uvs:
  - 0.0
  - 0.0
indices:
  - 0
  - 1
  - 9
";
    let mesh = parse_mesh(source).expect("mesh should parse structurally");
    let diagnostics = validate_mesh(&mesh);
    let codes: Vec<&str> = diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.iter().all(|c| *c == "L1111"), "codes: {codes:?}");

    let uv_diag = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/uvs"))
        .expect("expected a /uvs diagnostic");
    assert!(
        uv_diag.message.contains("1 vertices"),
        "{}",
        uv_diag.message
    );

    let index_diag = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/indices/2"))
        .expect("expected an /indices/2 diagnostic");
    assert!(index_diag.message.contains('9'), "{}", index_diag.message);

    assert!(check_mesh(source).is_err());
}

/// `L1111`: a per-vertex material list shorter than the vertex count is rejected with a pointer to
/// the whole list (there is no single offending index).
#[test]
fn wrong_material_list_length_is_l1111() {
    let source = "\
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
  - 1.0
  - 0.0
emissive:
  - 1.0
  - 1.0
indices:
  - 0
  - 1
  - 2
";
    let mesh = parse_mesh(source).expect("mesh should parse structurally");
    let diagnostics = validate_mesh(&mesh);
    let emissive_diag = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/emissive"))
        .expect("expected an /emissive diagnostic");
    assert_eq!(emissive_diag.code.as_str(), "L1111");
    assert!(check_mesh(source).is_err());
}

/// `L1111`: `roughness`/`metallic` above `1` and `emissive` below `0` are rejected per element,
/// each with a JSON Pointer to the offending entry.
#[test]
fn out_of_range_material_values_are_l1111() {
    let source = "\
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
  - 1.0
  - 0.0
emissive:
  - -1.0
  - 0.0
  - 0.0
roughness:
  - 1.5
  - 0.0
  - 0.0
indices:
  - 0
  - 1
  - 2
";
    let mesh = parse_mesh(source).expect("mesh should parse structurally");
    let diagnostics = validate_mesh(&mesh);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1111" && d.path.as_deref() == Some("/emissive/0"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1111" && d.path.as_deref() == Some("/roughness/0"))
    );
    assert!(check_mesh(source).is_err());
}

/// A near-miss key on a material list is `L1110` with a nearest-match suggestion.
#[test]
fn unknown_material_key_suggests_nearest() {
    let diagnostics =
        parse_mesh("format: 1\nkind: mesh\npositions: []\nemissives: []\nindices: []\n")
            .unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/emissives"))
        .expect("unknown key diagnostic");
    assert_eq!(d.code.as_str(), "L1110");
    assert_eq!(d.help.as_deref(), Some("nearest match is `emissive`"));
}

/// `L1112` fires above the vertex budget and is reported by [`ludion_scene::mesh_budget_warning`]
/// without appearing in [`validate_mesh`]'s (fatal) diagnostics — `check_mesh` still succeeds.
#[test]
fn vertex_budget_warning_does_not_fail_validation() {
    let vertex_count = ludion_scene::MESH_VERTEX_BUDGET + 1;
    let doc = MeshDocument {
        name: None,
        generated_from: None,
        origin: None,
        positions: vec![0.0; vertex_count * 3],
        normals: None,
        uvs: None,
        tangents: None,
        emissive: None,
        metallic: None,
        porosity: None,
        roughness: None,
        textures: MeshTextures::default(),
        alpha_mode: AlphaMode::Opaque,
        double_sided: false,
        skin: None,
        indices: vec![0, 1, 2],
        rim: None,
        lods: Vec::new(),
        surfaces: None,
    };
    assert!(validate_mesh(&doc).is_empty());
    let warning = ludion_scene::mesh_budget_warning(&doc).expect("expected a budget warning");
    assert_eq!(warning.code.as_str(), "L1112");

    let text = emit_mesh(&doc);
    let checked = check_mesh(&text).expect("over-budget meshes still check clean");
    assert!(ludion_scene::mesh_budget_warning(&checked).is_some());
}

// --- Surface tags (`surfaces` / `triangle_surfaces`) ------------------------------------------

mod surface_tags {
    use ludion_scene::{MeshSurfaces, check_mesh, emit_mesh};

    const QUAD: &str = "\
format: 1
kind: mesh
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
";
    const INDICES: &str = "\
indices:
  - 0
  - 1
  - 2
  - 0
  - 2
  - 3
";
    const TAGS: &str = "\
surfaces:
  - concrete
  - metal_grate
triangle_surfaces:
  - - 1
    - 0
  - - 1
    - 1
";

    fn doc(tags: &str) -> String {
        format!("{QUAD}{tags}{INDICES}")
    }

    fn codes(source: &str) -> Vec<(String, Option<String>)> {
        check_mesh(source)
            .expect_err("should be invalid")
            .iter()
            .map(|d| (d.code.to_string(), d.path.clone()))
            .collect()
    }

    #[test]
    fn tagged_document_round_trips_byte_identically() {
        let source = doc(TAGS);
        let mesh = check_mesh(&source).expect("valid");
        let surfaces = mesh.surfaces.as_ref().expect("tagged");
        assert_eq!(surfaces.per_triangle(), vec![0, 1]);
        assert_eq!(emit_mesh(&mesh), source);
    }

    #[test]
    fn untagged_document_emits_its_old_bytes() {
        let source = doc("");
        let mesh = check_mesh(&source).expect("valid");
        assert!(mesh.surfaces.is_none());
        assert_eq!(emit_mesh(&mesh), source);
    }

    #[test]
    fn clearing_the_field_drops_the_tags() {
        let source = doc(TAGS);
        let mut mesh = check_mesh(&source).expect("valid");
        mesh.surfaces = None;
        assert_eq!(emit_mesh(&mesh), doc(""));
    }

    #[test]
    fn invalid_tag_lists_are_l1696() {
        let cases: [(&str, &str); 7] = [
            ("surfaces:\n  - a\n", "/surfaces"),
            (
                "surfaces:\n  - a\n  - a\ntriangle_surfaces:\n  - - 2\n    - 0\n",
                "/surfaces/1",
            ),
            (
                "surfaces:\n  - \"\"\ntriangle_surfaces:\n  - - 2\n    - 0\n",
                "/surfaces/0",
            ),
            (
                "surfaces:\n  - a\ntriangle_surfaces:\n  - - 0\n    - 0\n  - - 2\n    - 0\n",
                "/triangle_surfaces/0",
            ),
            (
                "surfaces:\n  - a\ntriangle_surfaces:\n  - - 2\n    - 1\n",
                "/triangle_surfaces/0",
            ),
            (
                "surfaces:\n  - a\ntriangle_surfaces:\n  - - 1\n    - 0\n",
                "/triangle_surfaces",
            ),
            (
                "triangle_surfaces:\n  - - 2\n    - 0\n",
                "/triangle_surfaces",
            ),
        ];
        for (tags, pointer) in cases {
            let source = doc(tags);
            let found = codes(&source);
            assert!(
                found.contains(&("L1696".to_string(), Some(pointer.to_string()))),
                "{tags:?}: expected L1696 at {pointer}, got {found:?}"
            );
            assert!(check_mesh(&source).is_err());
        }
    }

    #[test]
    fn more_than_256_names_is_l1696() {
        let names: String = (0..257).map(|i| format!("  - n{i}\n")).collect();
        let source = doc(&format!(
            "surfaces:\n{names}triangle_surfaces:\n  - - 2\n    - 0\n"
        ));
        assert!(codes(&source).contains(&("L1696".to_string(), Some("/surfaces".to_string()))));
    }

    #[test]
    fn per_triangle_expands_runs() {
        let surfaces = MeshSurfaces {
            names: vec!["a".into(), "b".into()],
            runs: vec![[2, 1], [1, 0]],
        };
        assert_eq!(surfaces.per_triangle(), vec![1, 1, 0]);
    }
}

const THREE_VERTICES: &str = "\
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
  - 1.0
  - 0.0
";

/// `porosity` is a per-vertex list written alphabetically between `metallic` and `roughness`.
#[test]
fn porosity_list_round_trips_between_metallic_and_roughness() {
    let source = format!(
        "{THREE_VERTICES}metallic:\n  - 1.0\n  - 0.5\n  - 0.0\nporosity:\n  - 0.0\n  - 0.5\n  - 1.0\n\
         roughness:\n  - 0.1\n  - 0.5\n  - 1.0\nindices:\n  - 0\n  - 1\n  - 2\n"
    );
    let mesh = parse_mesh(&source).expect("mesh should parse");
    assert_eq!(mesh.porosity, Some(vec![0.0, 0.5, 1.0]));
    assert!(validate_mesh(&mesh).is_empty());
    assert_eq!(emit_mesh(&mesh), source);
}

/// `L1111`: a porosity list of the wrong length or out of range, each at its JSON Pointer.
#[test]
fn bad_porosity_is_l1111() {
    let short = format!("{THREE_VERTICES}porosity:\n  - 0.5\nindices:\n  - 0\n  - 1\n  - 2\n");
    let diagnostics = validate_mesh(&parse_mesh(&short).unwrap());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1111" && d.path.as_deref() == Some("/porosity"))
    );
    let high = format!(
        "{THREE_VERTICES}porosity:\n  - 0.5\n  - 1.5\n  - 0.5\nindices:\n  - 0\n  - 1\n  - 2\n"
    );
    let diagnostics = validate_mesh(&parse_mesh(&high).unwrap());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.as_str() == "L1111" && d.path.as_deref() == Some("/porosity/1"))
    );
}
