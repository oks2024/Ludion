//! Plan-level tests for `ludion_import::import` — the mapping, the exhaustive `L1202` surface, and
//! compiler determinism — all without a component registry (the CLI owns canonical emit). The full
//! CLI pipeline (check-scene, byte-identical re-import, the GPU golden exit gate) lives in
//! `ludion-cli/tests/import.rs`.

use std::path::{Path, PathBuf};

use ludion_import::{ExternalFiles, Import, ImportOptions, import};
use ludion_scene::TextureWrap;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import")
}

fn fixture_bytes() -> Vec<u8> {
    std::fs::read(fixture_dir().join("scene.gltf")).expect("committed fixture exists")
}

/// Imports the committed fixture in `--allow-lossy` mode (it has a textured material).
fn import_fixture_lossy() -> Import {
    import(
        &fixture_bytes(),
        ExternalFiles::Beside(&fixture_dir()),
        &ImportOptions::lossy(true),
    )
    .expect("lossy import of the fixture succeeds")
}

/// A component value on an entity by component name.
fn component<'a>(import: &'a Import, entity: usize, name: &str) -> Option<&'a serde_json::Value> {
    import.prefab.entities[entity]
        .components
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v)
}

#[test]
fn default_mode_fails_on_the_blend_material_naming_it() {
    let errors = import(
        &fixture_bytes(),
        ExternalFiles::Beside(&fixture_dir()),
        &ImportOptions::lossy(false),
    )
    .expect_err("the fixture's `stone` is alpha-blended, so a non-lossy import must fail");
    assert_eq!(
        errors.len(),
        2,
        "the blend material (a shared mesh reports once) and the camera node"
    );
    assert!(errors.iter().all(|e| e.code.as_str() == "L1202"));
    assert!(
        errors[0].message.contains("BLEND") && errors[0].message.contains("pedestal-mesh"),
        "the L1202 must name the offender: {}",
        errors[0].message
    );
    assert!(
        errors[1].message.contains("a camera"),
        "{}",
        errors[1].message
    );
}

#[test]
fn allow_lossy_compiles_the_supported_subset_with_a_warning() {
    let import = import_fixture_lossy();
    assert_eq!(import.prefab.entities.len(), 5, "5 nodes → 5 parts");
    assert_eq!(
        import.meshes.len(),
        2,
        "two distinct meshes (A is shared, emitted once)"
    );
    assert_eq!(import.textures.len(), 1, "one PNG re-encoded");
    assert_eq!(
        import.textures[0].rel_path, "textures/38e03d43f345ff3e.png",
        "a texture is named by the content hash of its canonical PNG bytes"
    );
    assert_eq!(
        import.warnings.len(),
        2,
        "the blend material and the camera"
    );
    assert!(import.warnings.iter().all(|w| w.code.as_str() == "L1202"));
}

#[test]
fn the_fixtures_base_color_texture_lands_on_its_mesh_document() {
    let import = import_fixture_lossy();
    let gem = import
        .meshes
        .iter()
        .find(|m| m.rel_path.contains("gem-mesh"))
        .unwrap();
    let reference = gem.doc.textures.base_color.as_ref().unwrap();
    assert_eq!(
        reference.path, "../textures/38e03d43f345ff3e.png",
        "a texture path is relative to the mesh document's own directory"
    );
    assert_eq!(reference.wrap, TextureWrap::Repeat);
}

#[test]
fn hierarchy_maps_to_parent_by_document_position() {
    let import = import_fixture_lossy();
    // gem (node 1) is a child of pedestal (node 0); satellite (node 2) a child of gem (node 1).
    assert_eq!(component(&import, 1, "Parent").unwrap()["index"], 0);
    assert_eq!(component(&import, 2, "Parent").unwrap()["index"], 1);
    // The two root-level extras (camera node, light) have no Parent.
    assert!(component(&import, 3, "Parent").is_none());
    assert!(component(&import, 4, "Parent").is_none());
}

#[test]
fn a_shared_mesh_is_emitted_once_and_referenced_twice() {
    let import = import_fixture_lossy();
    let pedestal = component(&import, 0, "Mesh").unwrap()["shape"]["asset"].clone();
    let satellite = component(&import, 2, "Mesh").unwrap()["shape"]["asset"].clone();
    assert_eq!(pedestal, satellite, "pedestal and satellite share mesh A");
    assert_eq!(
        pedestal,
        serde_json::json!("meshes/pedestal-mesh.mesh.yaml")
    );
    // Distinct base-color factors survived onto the two materials.
    let ped_color = &component(&import, 0, "Mesh").unwrap()["color"];
    let gem_color = &component(&import, 1, "Mesh").unwrap()["color"];
    assert_ne!(ped_color, gem_color);
}

#[test]
fn quaternion_becomes_euler_xyz_degrees() {
    let import = import_fixture_lossy();
    // gem carries a 45°-about-Y quaternion in the fixture.
    let rotation = &component(&import, 1, "Transform").unwrap()["rotation"];
    let y = rotation[1].as_f64().unwrap();
    assert!((y - 45.0).abs() < 1e-3, "expected ~45° about Y, got {y}");
}

#[test]
fn material_factors_map_onto_mesh_scalars() {
    let import = import_fixture_lossy();
    // `stone` (pedestal/satellite): metallicFactor 0, roughnessFactor 0.7, no emissiveFactor.
    let pedestal = component(&import, 0, "Mesh").unwrap();
    assert_eq!(pedestal["roughness"], f64::from(0.7_f32));
    assert_eq!(pedestal["metallic"], 0.0);
    assert_eq!(pedestal["emissive"], 0.0);
    // `gemstone` (gem): metallicFactor 0.6, roughnessFactor 0.15, emissiveFactor = 0.5 ×
    // baseColorFactor.rgb (exact on the f32 values — see the `proportional_emissive` tests below for
    // why that scalar multiple round-trips exactly).
    let gem = component(&import, 1, "Mesh").unwrap();
    assert_eq!(gem["roughness"], f64::from(0.15_f32));
    assert_eq!(gem["metallic"], f64::from(0.6_f32));
    assert_eq!(gem["emissive"], f64::from(0.5_f32));
}

#[test]
fn camera_node_stays_a_part_without_a_camera_and_point_light_maps() {
    let import = import_fixture_lossy();
    assert!(component(&import, 3, "Camera").is_none());
    assert!(component(&import, 3, "Transform").is_some());
    let light = component(&import, 4, "PointLight").unwrap();
    assert_eq!(light["intensity"], 6.0);
    assert_eq!(light["range"], 20.0);
    for key in ["cast_shadows", "shadow_bias", "shadow_near"] {
        assert!(light.get(key).is_none(), "{key} is not emitted");
    }
}

#[test]
fn the_shared_mesh_omits_normals_and_the_other_carries_them() {
    let import = import_fixture_lossy();
    let by_name = |n: &str| {
        import
            .meshes
            .iter()
            .find(|m| m.rel_path.contains(n))
            .unwrap()
    };
    let pedestal = &by_name("pedestal-mesh").doc;
    let gem = &by_name("gem-mesh").doc;
    assert!(pedestal.normals.is_none(), "no NORMAL → faceted at load");
    assert_eq!(pedestal.vertex_count(), 4);
    assert!(
        gem.normals.is_some() && gem.uvs.is_some(),
        "gem has NORMAL + TEXCOORD_0"
    );
    assert_eq!(gem.vertex_count(), 6);
}

#[test]
fn import_is_deterministic_byte_for_byte() {
    let a = import_fixture_lossy();
    let b = import_fixture_lossy();
    assert_eq!(a.prefab, b.prefab, "the prefab struct is deterministic");
    assert_eq!(a.meshes.len(), b.meshes.len());
    for (ma, mb) in a.meshes.iter().zip(&b.meshes) {
        assert_eq!(ma.rel_path, mb.rel_path);
        assert_eq!(ma.doc, mb.doc);
    }
    assert_eq!(
        a.textures[0].png, b.textures[0].png,
        "PNG re-encode is deterministic"
    );
}

// --- Targeted bad-gltf fixtures for the L1202 classes the main fixture does not exercise ----------

/// Imports inline glTF `json` and returns the L1202 messages it reports (default, non-lossy mode).
fn lossy_messages(json: &str) -> Vec<String> {
    match import(
        json.as_bytes(),
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(false),
    ) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics
            .into_iter()
            .filter(|d| d.code.as_str() == "L1202")
            .map(|d| d.message)
            .collect(),
    }
}

#[test]
fn any_camera_is_l1202_naming_the_scene() {
    for camera in [
        r#"{"type":"orthographic","orthographic":{"xmag":1.0,"ymag":1.0,"znear":0.1,"zfar":10.0}}"#,
        r#"{"type":"perspective","perspective":{"yfov":0.8,"znear":0.1}}"#,
    ] {
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"cameras":[{camera}],"nodes":[{{"name":"cam","camera":0}}]}}"#
        );
        let messages = lossy_messages(&json);
        assert!(
            messages.iter().any(|m| m.contains("a camera")
                && m.contains("cam")
                && m.contains("scene that instances it")),
            "{messages:?}"
        );
    }
}

const SKEW_MATRIX: &str = "[1,0,0,0, 0.5,1,0,0, 0,0,1,0, 0,0,0,1]";

fn matrix_node(matrix: &str) -> String {
    format!(r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"name":"skewed","matrix":{matrix}}}]}}"#)
}

#[test]
fn a_node_matrix_that_shears_is_l1202_naming_the_node() {
    let json = matrix_node(SKEW_MATRIX);
    let messages = lossy_messages(&json);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains("shear") && messages[0].contains("skewed"),
        "{messages:?}"
    );

    let import = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(true),
    )
    .expect("lossy imports");
    assert_eq!(import.warnings.len(), 1, "{:?}", import.warnings);
    assert_eq!(import.warnings[0].code.as_str(), "L1202");
}

#[test]
fn a_node_matrix_without_shear_imports_clean() {
    // Column-major: uniform scale 2, translation (1, 2, 3) in the last column.
    let json = matrix_node("[2,0,0,0, 0,2,0,0, 0,0,2,0, 1,2,3,1]");
    let import = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(false),
    )
    .expect("imports strict");
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
    let transform = component(&import, 0, "Transform").unwrap();
    assert_eq!(transform["translation"], serde_json::json!([1.0, 2.0, 3.0]));
    assert_eq!(transform["scale"], serde_json::json!([2.0, 2.0, 2.0]));
}

#[test]
fn a_slash_in_a_node_name_never_reaches_a_part_name() {
    let json = r#"{"asset":{"version":"2.0"},"nodes":[{"name":"a/b"},{"name":"a_b"}]}"#;
    let import = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(false),
    )
    .unwrap();
    let names: Vec<&str> = import
        .prefab
        .entities
        .iter()
        .map(|e| e.name.as_deref().unwrap())
        .collect();
    assert_eq!(names[0], "a_b");
    assert_ne!(names[1], names[0], "same-named nodes get distinct names");
    assert!(names.iter().all(|n| !n.contains('/')));
}

#[test]
fn directional_light_is_l1202() {
    let json = r#"{"asset":{"version":"2.0"},"extensionsUsed":["KHR_lights_punctual"],"extensions":{"KHR_lights_punctual":{"lights":[{"type":"directional"}]}},"nodes":[{"name":"sun","extensions":{"KHR_lights_punctual":{"light":0}}}]}"#;
    let messages = lossy_messages(json);
    assert!(
        messages.iter().any(|m| m.contains("directional light")),
        "{messages:?}"
    );
}

/// A glTF with one spot light (`spot` is the light's `spot` object body) on node 1, whose
/// optional parent is node 0. `parent_rotation`/`rotation` are node quaternions, `[x,y,z,w]`.
fn spot_gltf(spot: &str, parent_rotation: Option<&str>, rotation: Option<&str>) -> String {
    let rot = |r: Option<&str>| r.map(|r| format!(r#","rotation":{r}"#)).unwrap_or_default();
    let nodes = match parent_rotation {
        Some(p) => format!(
            r#"{{"name":"arm","children":[1]{}}},{{"name":"lamp"{},"extensions":{{"KHR_lights_punctual":{{"light":0}}}}}}"#,
            rot(Some(p)),
            rot(rotation)
        ),
        None => format!(
            r#"{{"name":"lamp"{},"extensions":{{"KHR_lights_punctual":{{"light":0}}}}}}"#,
            rot(rotation)
        ),
    };
    format!(
        r#"{{"asset":{{"version":"2.0"}},"extensionsUsed":["KHR_lights_punctual"],"extensions":{{"KHR_lights_punctual":{{"lights":[{{"type":"spot","color":[1,0.5,0.25],"intensity":3,"range":15,"spot":{spot}}}]}}}},"nodes":[{nodes}]}}"#
    )
}

const Y_90: &str = "[0,0.7071067811865476,0,0.7071067811865476]";

fn assert_direction(light: &serde_json::Value, expected: [f64; 3]) {
    for (axis, want) in expected.iter().enumerate() {
        let got = light["direction"][axis].as_f64().unwrap();
        assert!((got - want).abs() < 1e-6, "axis {axis}: {got} vs {want}");
    }
}

#[test]
fn a_spot_light_maps_to_spot_light_with_degrees_and_a_world_direction() {
    let json = spot_gltf(
        r#"{"innerConeAngle":0.25,"outerConeAngle":0.5}"#,
        None,
        Some(Y_90),
    );
    let import = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(false),
    )
    .expect("imports strict");
    assert!(component(&import, 0, "PointLight").is_none());
    let light = component(&import, 0, "SpotLight").unwrap();
    assert_eq!(light["color"], serde_json::json!([1.0, 0.5, 0.25]));
    assert_eq!(light["intensity"], 3.0);
    assert_eq!(light["range"], 15.0);
    for key in ["cast_shadows", "shadow_bias", "shadow_near"] {
        assert!(light.get(key).is_none(), "{key} is not emitted");
    }
    let inner = light["inner_angle"].as_f64().unwrap();
    let outer = light["outer_angle"].as_f64().unwrap();
    assert!((inner - 0.25_f64.to_degrees()).abs() < 1e-4, "{inner}");
    assert!((outer - 0.5_f64.to_degrees()).abs() < 1e-4, "{outer}");
    // A +90 degree turn about Y takes local -Z to world -X.
    assert_direction(light, [-1.0, 0.0, 0.0]);
}

#[test]
fn a_parented_spot_light_direction_composes_the_whole_chain() {
    let json = spot_gltf(r#"{"outerConeAngle":0.5}"#, Some(Y_90), Some(Y_90));
    let import = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(false),
    )
    .expect("imports strict");
    let light = component(&import, 1, "SpotLight").unwrap();
    // Two quarter turns about Y: local -Z becomes world +Z.
    assert_direction(light, [0.0, 0.0, 1.0]);
}

#[test]
fn an_unrotated_spot_light_points_down_local_minus_z_with_gltf_default_cone() {
    let json = spot_gltf("{}", None, None);
    let import = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(false),
    )
    .expect("imports strict");
    let light = component(&import, 0, "SpotLight").unwrap();
    assert_direction(light, [0.0, 0.0, -1.0]);
    assert_eq!(light["inner_angle"], 0.0);
    let outer = light["outer_angle"].as_f64().unwrap();
    assert!(
        (outer - 45.0).abs() < 1e-4,
        "default pi/4 is 45 degrees: {outer}"
    );
}

#[test]
fn a_spot_cone_gltf_allows_but_the_engine_refuses_is_clamped_valid() {
    // outer = pi/2 (90 degrees, exclusive in SpotLight) with a 1.0 rad inner.
    let json = spot_gltf(
        r#"{"innerConeAngle":1.0,"outerConeAngle":1.5707963267948966}"#,
        None,
        None,
    );
    let import = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(false),
    )
    .expect("imports strict");
    let light = component(&import, 0, "SpotLight").unwrap();
    let inner = light["inner_angle"].as_f64().unwrap();
    let outer = light["outer_angle"].as_f64().unwrap();
    assert!(outer < 90.0 && outer > 89.0, "{outer}");
    assert!(inner <= outer && inner >= 0.0, "{inner}");
}

#[test]
fn an_animation_with_no_channels_is_l1202() {
    let json = r#"{"asset":{"version":"2.0"},"nodes":[{}],"animations":[{"name":"wiggle","channels":[],"samplers":[]}]}"#;
    let messages = lossy_messages(json);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("animation") && m.contains("wiggle")),
        "{messages:?}"
    );
}

#[test]
fn jpeg_image_is_l1202() {
    // A `data:` URI whose payload begins with the JPEG magic (FF D8 FF) — not a PNG. Only a
    // referenced image is read at all, so the material points a supported slot at it.
    let json = r#"{"asset":{"version":"2.0"},"nodes":[{"name":"tri","mesh":0}],"meshes":[{"name":"tri-mesh","primitives":[{"attributes":{"POSITION":0},"material":0}]}],"materials":[{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}],"textures":[{"source":0}],"images":[{"name":"photo","mimeType":"image/jpeg","uri":"data:image/jpeg;base64,/9j/4AAQ"}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36}],"buffers":[{"byteLength":36,"uri":"data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA"}]}"#;
    let messages = lossy_messages(json);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("JPEG image") && m.contains("photo")),
        "{messages:?}"
    );
}

// --- Wave 3: the material-surface audit (decision 7 of `Plan/phase-11-pbr.md`) --------------------

/// The main fixture's tiny embedded "checker" PNG, so a texture reference resolves to real pixels.
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAG0lEQVR42mO4FGXzX8NmwX8YzYDMAdEMBFUAAKPYI1E2QLuEAAAAAElFTkSuQmCC";

/// The primitive attribute maps [`mesh_gltf_with`] offers: POSITION plus, as the test needs them,
/// TEXCOORD_0 (accessor 1) and TANGENT (accessor 2).
const POSITION_ONLY: &str = r#""POSITION":0"#;
const WITH_UVS: &str = r#""POSITION":0,"TEXCOORD_0":1"#;
const WITH_UVS_AND_TANGENT: &str = r#""POSITION":0,"TEXCOORD_0":1,"TANGENT":2"#;

/// A one-triangle, non-indexed glTF document whose sole primitive carries `attributes` and
/// `material_json` verbatim as material 0 — the minimal document that reaches `mesh_reference`'s
/// material checks. `texture_json` is the trailing `textures`/`samplers`/`images` block a material
/// referencing a texture needs (empty when it references none).
///
/// The one triangle is `(0,0,0) (1,0,0) (0,1,0)` with UVs `(0,0) (1,0) (0,1)` — so a generated
/// tangent frame is exactly `(+X, w = 1)` — and an authored TANGENT of `(0, 1, 0, -1)` per vertex,
/// which the generator would never produce, so "imported verbatim" is distinguishable from
/// "computed".
fn mesh_gltf_with(material_json: &str, texture_json: &str, attributes: &str) -> String {
    let mut bytes = Vec::new();
    for value in [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0_f32, 0.0, 1.0, 0.0, 0.0, 1.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0_f32, 1.0, 0.0, -1.0].repeat(3) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let buffer_b64 = base64::encode(&bytes);
    format!(
        r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"name":"tri","mesh":0}}],"meshes":[{{"name":"tri-mesh","primitives":[{{"attributes":{{{attributes}}},"material":0}}]}}],"materials":[{material_json}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}},{{"bufferView":1,"componentType":5126,"count":3,"type":"VEC2"}},{{"bufferView":2,"componentType":5126,"count":3,"type":"VEC4"}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":24}},{{"buffer":0,"byteOffset":60,"byteLength":48}}],"buffers":[{{"byteLength":108,"uri":"data:application/octet-stream;base64,{buffer_b64}"}}]{texture_json}}}"#
    )
}

/// [`mesh_gltf_with`] with UVs and, when `texture`, one default-sampler PNG at texture index 0.
fn mesh_gltf(material_json: &str, texture: bool) -> String {
    let texture_json = if texture {
        one_texture("{}")
    } else {
        String::new()
    };
    mesh_gltf_with(material_json, &texture_json, WITH_UVS)
}

/// A `textures`/`samplers`/`images` block for one PNG at texture index 0, sampled through
/// `sampler_json` (`{}` for glTF's defaults: repeat on both axes).
fn one_texture(sampler_json: &str) -> String {
    format!(
        r#","textures":[{{"source":0,"sampler":0}}],"samplers":[{sampler_json}],"images":[{{"name":"map","mimeType":"image/png","uri":"data:image/png;base64,{PNG_B64}"}}]"#
    )
}

/// The one mesh document a `mesh_gltf*` import produces.
fn only_mesh(import: &Import) -> &ludion_scene::MeshDocument {
    &import.meshes[0].doc
}

/// Imports `json` in default (non-lossy) mode, expecting it to compile cleanly.
fn import_clean(json: &str) -> Import {
    import(
        json.as_bytes(),
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(false),
    )
    .expect("this document has a Ludion mapping")
}

/// Two meshes whose names differ only in case, each at its own z.
fn case_clash_gltf() -> String {
    let mut bytes = Vec::new();
    for z in [0.0_f32, 5.0] {
        for value in [0.0_f32, 0.0, z, 1.0, 0.0, z, 0.0, 1.0, z] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    let buffer_b64 = base64::encode(&bytes);
    format!(
        r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0,1]}}],"nodes":[{{"name":"a","mesh":0}},{{"name":"b","mesh":1}}],"meshes":[{{"name":"Wall","primitives":[{{"attributes":{{"POSITION":0}}}}]}},{{"name":"wall","primitives":[{{"attributes":{{"POSITION":1}}}}]}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}},{{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,5.0],"max":[1.0,1.0,5.0]}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":36}}],"buffers":[{{"byteLength":72,"uri":"data:application/octet-stream;base64,{buffer_b64}"}}]}}"#
    )
}

/// Mesh files whose names differ only in case would be one file on Windows and macOS: the second
/// gets a suffix, and each part still names the mesh carrying its own geometry.
#[test]
fn mesh_names_differing_only_in_case_get_distinct_files() {
    let import = import_clean(&case_clash_gltf());
    let paths: Vec<&str> = import.meshes.iter().map(|m| m.rel_path.as_str()).collect();
    assert_eq!(paths, ["meshes/Wall.mesh.yaml", "meshes/wall-1.mesh.yaml"]);
    assert_eq!(import.meshes[1].doc.name.as_deref(), Some("wall-1"));
    for (index, mesh) in import.meshes.iter().enumerate() {
        let want_z = if index == 0 { 0.0 } else { 5.0 };
        assert!(
            mesh.doc.positions.chunks(3).all(|p| p[2] == want_z),
            "{}: {:?}",
            mesh.rel_path,
            mesh.doc.positions
        );
        let reference = component(&import, index, "Mesh").unwrap()["shape"]["asset"].clone();
        assert_eq!(
            reference,
            serde_json::json!(mesh.rel_path),
            "entity {index}"
        );
    }
}

// --- The five supported metallic-roughness maps (`Plan/textured-pbr.md` G3) ------------------------

/// The hash-named path the tests' one PNG re-encodes to, as a mesh document spells it (relative to
/// its own directory, one level under the output root).
const MAP_PATH: &str = "../textures/38e03d43f345ff3e.png";

#[test]
fn base_color_texture_lands_in_the_mesh_document() {
    let json = mesh_gltf(
        r#"{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}"#,
        true,
    );
    let import = import_clean(&json);
    let reference = only_mesh(&import).textures.base_color.as_ref().unwrap();
    assert_eq!(reference.path, MAP_PATH);
    assert_eq!(reference.wrap, TextureWrap::Repeat, "glTF's default");
    assert_eq!(reference.factor, 1.0, "base color carries no scalar");
    assert_eq!(import.textures.len(), 1, "the referenced PNG is emitted");
    assert_eq!(import.textures[0].rel_path, "textures/38e03d43f345ff3e.png");
}

#[test]
fn metallic_roughness_texture_lands_in_the_mesh_document() {
    let json = mesh_gltf(
        r#"{"name":"m","pbrMetallicRoughness":{"metallicRoughnessTexture":{"index":0}}}"#,
        true,
    );
    let import = import_clean(&json);
    let reference = only_mesh(&import)
        .textures
        .metallic_roughness
        .as_ref()
        .unwrap();
    assert_eq!(reference.path, MAP_PATH);
    assert_eq!(reference.factor, 1.0);
}

#[test]
fn normal_texture_lands_in_the_mesh_document_with_its_scale() {
    let json = mesh_gltf(
        r#"{"name":"m","normalTexture":{"index":0,"scale":0.5}}"#,
        true,
    );
    let import = import_clean(&json);
    let reference = only_mesh(&import).textures.normal.as_ref().unwrap();
    assert_eq!(reference.path, MAP_PATH);
    assert_eq!(
        reference.factor,
        f64::from(0.5_f32),
        "`normalTexture.scale` is the reference's factor"
    );
}

#[test]
fn occlusion_texture_lands_in_the_mesh_document_with_its_strength() {
    let json = mesh_gltf(
        r#"{"name":"m","occlusionTexture":{"index":0,"strength":0.25}}"#,
        true,
    );
    let import = import_clean(&json);
    let reference = only_mesh(&import).textures.occlusion.as_ref().unwrap();
    assert_eq!(reference.path, MAP_PATH);
    assert_eq!(
        reference.factor,
        f64::from(0.25_f32),
        "`occlusionTexture.strength` is the reference's factor"
    );
}

#[test]
fn emissive_texture_lands_in_the_mesh_document() {
    let json = mesh_gltf(r#"{"name":"m","emissiveTexture":{"index":0}}"#, true);
    let import = import_clean(&json);
    let reference = only_mesh(&import).textures.emissive.as_ref().unwrap();
    assert_eq!(reference.path, MAP_PATH);
}

#[test]
fn sampler_wrap_modes_map_onto_the_reference() {
    // GL_CLAMP_TO_EDGE, GL_MIRRORED_REPEAT, GL_REPEAT.
    for (mode, expected) in [
        (33071, TextureWrap::Clamp),
        (33648, TextureWrap::Mirror),
        (10497, TextureWrap::Repeat),
    ] {
        let texture_json = one_texture(&format!(r#"{{"wrapS":{mode},"wrapT":{mode}}}"#));
        let json = mesh_gltf_with(
            r#"{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}"#,
            &texture_json,
            WITH_UVS,
        );
        let import = import_clean(&json);
        let reference = only_mesh(&import).textures.base_color.clone().unwrap();
        assert_eq!(reference.wrap, expected, "wrap mode {mode}");
    }
}

#[test]
fn tex_coord_1_is_l1202() {
    let json = mesh_gltf(
        r#"{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0,"texCoord":1}}}"#,
        true,
    );
    let messages = lossy_messages(&json);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("a texture on a second UV set") && m.contains("tri-mesh")),
        "{messages:?}"
    );
}

#[test]
fn unequal_wrap_modes_are_l1202() {
    // A mesh document's reference holds one wrap mode for both axes, so repeat-on-s/clamp-on-t has
    // nowhere to land.
    let texture_json = one_texture(r#"{"wrapS":10497,"wrapT":33071}"#);
    let json = mesh_gltf_with(
        r#"{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}"#,
        &texture_json,
        WITH_UVS,
    );
    let messages = lossy_messages(&json);
    assert!(
        messages.iter().any(
            |m| m.contains("a sampler with different wrap modes per axis")
                && m.contains("tri-mesh")
        ),
        "{messages:?}"
    );
}

#[test]
fn a_textured_primitive_without_texcoord_0_is_l1202() {
    let json = mesh_gltf_with(
        r#"{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}"#,
        &one_texture("{}"),
        POSITION_ONLY,
    );
    let messages = lossy_messages(&json);
    assert!(
        messages.iter().any(|m| m
            .contains("a textured material on a primitive without TEXCOORD_0")
            && m.contains("tri-mesh")),
        "{messages:?}"
    );
    // The geometry still imports; only the maps drop (`--allow-lossy` is per-feature).
    let import = import(
        json.as_bytes(),
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(true),
    )
    .expect("lossy mode imports the rest");
    assert_eq!(only_mesh(&import).textures.base_color, None);
    assert_eq!(only_mesh(&import).vertex_count(), 3);
}

#[test]
fn identical_images_dedup_to_one_texture_artifact() {
    // Two glTF images with byte-identical payloads under different names, on two slots: one
    // committed file, both references pointing at it.
    let texture_json = format!(
        r#","textures":[{{"source":0}},{{"source":1}}],"images":[{{"name":"first","mimeType":"image/png","uri":"data:image/png;base64,{PNG_B64}"}},{{"name":"second","mimeType":"image/png","uri":"data:image/png;base64,{PNG_B64}"}}]"#
    );
    let json = mesh_gltf_with(
        r#"{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0}},"emissiveTexture":{"index":1}}"#,
        &texture_json,
        WITH_UVS,
    );
    let import = import_clean(&json);
    assert_eq!(import.textures.len(), 1, "identical pixels commit once");
    let textures = &only_mesh(&import).textures;
    assert_eq!(textures.base_color.as_ref().unwrap().path, MAP_PATH);
    assert_eq!(textures.emissive.as_ref().unwrap().path, MAP_PATH);
}

#[test]
fn an_unreferenced_image_is_not_written() {
    // The material references no texture, so the document's image is dead weight — not an `L1202`,
    // just absent from the plan.
    let json = mesh_gltf(r#"{"name":"m"}"#, true);
    let import = import_clean(&json);
    assert!(import.textures.is_empty());
    assert!(import.warnings.is_empty());
}

// --- Authored normals -----------------------------------------------------------------------------

/// A one-triangle glTF in the XZ plane (faceted normal `-y`) whose NORMAL accessor is `(0,0,1)`,
/// zero, `(0,0,1)`.
#[test]
fn a_zero_length_authored_normal_takes_the_faceted_normal_and_the_rest_stay_bit_identical() {
    let mut bytes = Vec::new();
    for value in [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0_f32, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let b64 = base64::encode(&bytes);
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"name":"tri","mesh":0}}],"meshes":[{{"name":"tri-mesh","primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1}}}}]}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,0.0,1.0]}},{{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":36}}],"buffers":[{{"byteLength":72,"uri":"data:application/octet-stream;base64,{b64}"}}]}}"#
    );
    let import = import_clean(&json);
    let doc = only_mesh(&import);
    assert_eq!(
        doc.normals.as_deref(),
        Some([0.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0].as_slice())
    );
    assert!(
        ludion_scene::validate_mesh(doc).is_empty(),
        "the importer writes nothing the validator refuses"
    );
}

// --- Tangents (`Plan/textured-pbr.md` G1) ---------------------------------------------------------

#[test]
fn tangent_accessor_is_imported_verbatim() {
    // With and without a normal map: an authored frame is fidelity, not a normal-map fixup.
    for material in [
        r#"{"name":"m","normalTexture":{"index":0}}"#,
        r#"{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}"#,
    ] {
        let json = mesh_gltf_with(material, &one_texture("{}"), WITH_UVS_AND_TANGENT);
        let import = import_clean(&json);
        assert_eq!(
            only_mesh(&import).tangents.as_deref(),
            Some([0.0, 1.0, 0.0, -1.0].repeat(3).as_slice()),
            "material {material}"
        );
    }
}

#[test]
fn normal_texture_without_tangent_computes_them() {
    let json = mesh_gltf(r#"{"name":"m","normalTexture":{"index":0}}"#, true);
    let import = import_clean(&json);
    let doc = only_mesh(&import);
    assert_eq!(
        doc.tangents.as_deref(),
        Some([1.0, 0.0, 0.0, -1.0].repeat(3).as_slice()),
        "the UV mapping puts +u along +x with negative handedness (the bitangent is image-up)"
    );
    // The primitive carries no NORMAL, so the faceted normals the frame is orthogonal to are
    // written out beside it rather than left to the loader.
    assert_eq!(
        doc.normals.as_deref(),
        Some([0.0, 0.0, 1.0].repeat(3).as_slice())
    );
}

#[test]
fn the_generated_frame_matches_the_fixtures_authored_tangent() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-textured");
    let bytes = std::fs::read(dir.join("scene.gltf")).unwrap();
    let import = import(
        &bytes,
        ExternalFiles::Beside(&dir),
        &ImportOptions::lossy(false),
    )
    .unwrap();
    let tangents_of = |name: &str| {
        import
            .meshes
            .iter()
            .find(|m| m.rel_path.contains(name))
            .unwrap_or_else(|| panic!("no mesh named {name}"))
            .doc
            .tangents
            .clone()
            .expect("tangents")
    };
    assert_eq!(
        tangents_of("computed-cube"),
        tangents_of("tangent-cube"),
        "the generated frame is the exporter-convention one the fixture authors"
    );
}

#[test]
fn no_normal_texture_emits_no_tangents() {
    let json = mesh_gltf(
        r#"{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}"#,
        true,
    );
    let import = import_clean(&json);
    let doc = only_mesh(&import);
    assert_eq!(doc.tangents, None, "no normal map asks for no tangents");
    assert_eq!(doc.normals, None, "nor for normals the loader can compute");
}

#[test]
fn a_mask_material_maps_its_cutoff() {
    let json = mesh_gltf(
        r#"{"name":"m","alphaMode":"MASK","alphaCutoff":0.25}"#,
        false,
    );
    let import = import_clean(&json);
    assert_eq!(
        only_mesh(&import).alpha_mode,
        ludion_scene::AlphaMode::Mask { cutoff: 0.25 }
    );
}

#[test]
fn a_mask_material_without_a_cutoff_takes_the_default_and_writes_none() {
    let json = mesh_gltf(r#"{"name":"m","alphaMode":"MASK"}"#, false);
    let import = import_clean(&json);
    let doc = only_mesh(&import);
    assert_eq!(
        doc.alpha_mode,
        ludion_scene::AlphaMode::Mask { cutoff: 0.5 }
    );
    let text = ludion_scene::emit_mesh(doc);
    assert!(text.contains("alpha_mode: mask\n"), "{text}");
    assert!(!text.contains("alpha_cutoff"), "{text}");
}

#[test]
fn a_double_sided_material_maps_to_double_sided() {
    let json = mesh_gltf(r#"{"name":"m","doubleSided":true}"#, false);
    let import = import_clean(&json);
    assert!(only_mesh(&import).double_sided);
}

#[test]
fn a_blend_material_is_l1202_naming_blend_and_pointing_at_mask() {
    let json = mesh_gltf(r#"{"name":"m","alphaMode":"BLEND"}"#, false);
    let errors = import(
        json.as_bytes(),
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(false),
    )
    .expect_err("BLEND has no mapping");
    let blend = errors
        .iter()
        .find(|d| d.message.contains("BLEND"))
        .expect("the BLEND material is named");
    assert_eq!(blend.code.as_str(), "L1202");
    assert!(blend.message.contains("tri-mesh"), "{}", blend.message);
    assert!(
        blend.help.as_deref().is_some_and(|h| h.contains("MASK")),
        "{:?}",
        blend.help
    );
}

#[test]
fn an_opaque_single_sided_material_emits_none_of_the_keys() {
    let json = mesh_gltf(r#"{"name":"m"}"#, false);
    let import = import_clean(&json);
    let text = ludion_scene::emit_mesh(only_mesh(&import));
    assert!(
        !text.contains("alpha_") && !text.contains("double_sided"),
        "{text}"
    );
}

#[test]
fn emissive_factor_exactly_proportional_to_base_color_maps_cleanly() {
    let json = mesh_gltf(
        r#"{"name":"glow","pbrMetallicRoughness":{"baseColorFactor":[0.3,0.55,0.9,1]},"emissiveFactor":[0.15,0.275,0.45]}"#,
        false,
    );
    let import = import(
        json.as_bytes(),
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(false),
    )
    .expect("an exact scalar multiple of baseColorFactor must import cleanly");
    let mesh = component(&import, 0, "Mesh").unwrap();
    assert_eq!(mesh["emissive"], f64::from(0.5_f32));
}

/// Declares `extensions_used` (and, when `required`, the same names as required) on `mesh_gltf` JSON.
fn with_extensions(json: &str, used: &[&str], required: bool) -> String {
    let list = |names: &[&str]| {
        names
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut declared = format!(r#""extensionsUsed":[{}]"#, list(used));
    if required {
        declared.push_str(&format!(r#","extensionsRequired":[{}]"#, list(used)));
    }
    json.replacen(r#"{"asset":"#, &format!(r#"{{{declared},"asset":"#), 1)
}

const STRENGTH: &str = "KHR_materials_emissive_strength";

#[test]
fn emissive_strength_multiplies_the_emissive_factor() {
    let json = mesh_gltf(
        &format!(
            r#"{{"name":"glow","emissiveFactor":[0.5,0.5,0.5],"extensions":{{"{STRENGTH}":{{"emissiveStrength":4.0}}}}}}"#
        ),
        false,
    );
    let import = import_clean(&with_extensions(&json, &[STRENGTH], false));
    let mesh = component(&import, 0, "Mesh").unwrap();
    assert_eq!(mesh["emissive"], 2.0);
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
}

#[test]
fn emissive_strength_with_a_coloured_base_stays_proportional() {
    let json = mesh_gltf(
        &format!(
            r#"{{"name":"glow","pbrMetallicRoughness":{{"baseColorFactor":[0.25,0.5,1,1]}},"emissiveFactor":[0.125,0.25,0.5],"extensions":{{"{STRENGTH}":{{"emissiveStrength":2.0}}}}}}"#
        ),
        false,
    );
    let import = import_clean(&with_extensions(&json, &[STRENGTH], false));
    let mesh = component(&import, 0, "Mesh").unwrap();
    assert_eq!(mesh["emissive"], 1.0);
}

#[test]
fn an_unmapped_optional_extension_warns_l1239_naming_it() {
    let json = mesh_gltf(r#"{"name":"m"}"#, false);
    let import = import_clean(&with_extensions(&json, &["KHR_materials_specular"], false));
    assert_eq!(import.warnings.len(), 1, "{:?}", import.warnings);
    assert_eq!(import.warnings[0].code.as_str(), "L1239");
    assert!(
        import.warnings[0]
            .message
            .contains("KHR_materials_specular"),
        "{}",
        import.warnings[0].message
    );
}

#[test]
fn an_unmapped_required_extension_aborts_once_without_l1239() {
    let json = mesh_gltf(r#"{"name":"m"}"#, false);
    let json = with_extensions(&json, &["KHR_materials_specular"], true);
    let errors = import(
        json.as_bytes(),
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(false),
    )
    .expect_err("a required extension with no mapping aborts the import");
    assert_eq!(errors.len(), 1, "{errors:?}");
    // The gltf parser refuses an unknown required extension itself, as an L1201 naming it.
    assert_eq!(errors[0].code.as_str(), "L1201", "{}", errors[0].message);
    assert!(errors[0].message.contains("KHR_materials_specular"));
}

// --- Tinted emissive: baked into a map (`Plan/phase-11-pbr.md` decision 1, request 0014) ---------

/// A dyadic tint: `base` × `[2, 1, 0.5]` is `EMISSIVE`, so `Mesh.emissive` is 2 and the hue
/// `[1, 0.5, 0.25]`.
const TINT_BASE: [f32; 3] = [0.5, 0.25, 1.0];
const TINT_EMISSIVE: [f32; 3] = [1.0, 0.25, 0.5];
const TINT_HUE: [f32; 3] = [1.0, 0.5, 0.25];

fn tinted_material(extra: &str) -> String {
    format!(
        r#"{{"name":"tinted","pbrMetallicRoughness":{{"baseColorFactor":[{},{},{},1]}},"emissiveFactor":[{},{},{}]{extra}}}"#,
        TINT_BASE[0],
        TINT_BASE[1],
        TINT_BASE[2],
        TINT_EMISSIVE[0],
        TINT_EMISSIVE[1],
        TINT_EMISSIVE[2],
    )
}

fn texture_at<'a>(import: &'a Import, rel_path: &str) -> &'a [u8] {
    &import
        .textures
        .iter()
        .find(|t| t.rel_path == rel_path.trim_start_matches("../"))
        .unwrap_or_else(|| panic!("no texture {rel_path}"))
        .png
}

#[test]
fn a_tinted_emissive_without_a_texture_bakes_a_one_texel_map() {
    let import = import_clean(&mesh_gltf(&tinted_material(""), false));
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
    assert_eq!(component(&import, 0, "Mesh").unwrap()["emissive"], 2.0);
    assert_eq!(import.textures.len(), 1, "only the baked map is written");
    let reference = only_mesh(&import).textures.emissive.as_ref().unwrap();
    assert_eq!(reference.wrap, TextureWrap::Repeat);
    assert_eq!(reference.factor, 1.0);
    let (width, height, rgba) =
        ludion_assets::decode_png(texture_at(&import, &reference.path), "baked").unwrap();
    assert_eq!((width, height), (1, 1));
    let want: Vec<u8> = TINT_HUE
        .iter()
        .map(|&h| ludion_assets::encode_srgb(h))
        .chain([255])
        .collect();
    assert_eq!(rgba, want);
}

#[test]
fn a_tinted_emissive_texture_is_baked_at_its_size_and_the_source_is_not_written() {
    let json = mesh_gltf(&tinted_material(r#","emissiveTexture":{"index":0}"#), true);
    let import = import_clean(&json);
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
    assert_eq!(component(&import, 0, "Mesh").unwrap()["emissive"], 2.0);
    assert_eq!(
        import.textures.len(),
        1,
        "the untinted source is not written"
    );
    assert_ne!(import.textures[0].rel_path, "textures/38e03d43f345ff3e.png");
    let reference = only_mesh(&import).textures.emissive.as_ref().unwrap();
    let (width, height, _) = ludion_assets::decode_png(&import.textures[0].png, "baked").unwrap();
    assert_eq!((width, height), (4, 4), "the source PNG is 4×4");
    assert_eq!(
        reference.path,
        format!("../{}", import.textures[0].rel_path)
    );
}

/// The shader renders `base × E × decode(map)`; the glTF wants `factor × decode(source)`.
#[test]
fn the_baked_map_reproduces_factor_times_texture_within_half_a_step() {
    let json = mesh_gltf(&tinted_material(r#","emissiveTexture":{"index":0}"#), true);
    let import = import_clean(&json);
    let intensity = 2.0_f32;
    let (_, _, source) =
        ludion_assets::decode_png(&base64::decode(PNG_B64).unwrap(), "source").unwrap();
    let (_, _, baked) = ludion_assets::decode_png(&import.textures[0].png, "baked").unwrap();
    assert_eq!(source.len(), baked.len());
    for (texel, (src, map)) in source.chunks(4).zip(baked.chunks(4)).enumerate() {
        assert_eq!(src[3], map[3], "alpha is copied (texel {texel})");
        for c in 0..3 {
            let rendered = TINT_BASE[c] * intensity * ludion_assets::decode_srgb(map[c]);
            let wanted = TINT_EMISSIVE[c] * ludion_assets::decode_srgb(src[c]);
            let step = ludion_assets::decode_srgb(map[c].saturating_add(1))
                - ludion_assets::decode_srgb(map[c].saturating_sub(1));
            let bound = TINT_BASE[c] * intensity * step / 2.0 + 1e-6;
            assert!(
                (rendered - wanted).abs() <= bound,
                "texel {texel} channel {c}: rendered {rendered}, wanted {wanted}, bound {bound}"
            );
        }
    }
}

#[test]
fn one_baked_map_is_shared_by_materials_with_the_same_tint() {
    let texture = one_texture("{}");
    let slot = r#","emissiveTexture":{"index":0}"#;
    let materials = format!("{},{}", tinted_material(slot), tinted_material(slot));
    let json = mesh_gltf_with(&materials, &texture, WITH_UVS).replace(
        r#""material":0}]"#,
        r#""material":0},{"attributes":{"POSITION":0,"TEXCOORD_0":1},"material":1}]"#,
    );
    let import = import_clean(&json);
    assert_eq!(import.meshes.len(), 2);
    assert_eq!(import.textures.len(), 1, "same texture and hue: one file");
}

#[test]
fn proportional_and_zero_emissive_add_no_texture() {
    for emissive in ["[0.25,0.125,0.5]", "[0,0,0]"] {
        let json = mesh_gltf(
            &format!(
                r#"{{"name":"m","pbrMetallicRoughness":{{"baseColorFactor":[0.5,0.25,1,1]}},"emissiveFactor":{emissive}}}"#
            ),
            false,
        );
        let import = import_clean(&json);
        assert!(import.textures.is_empty(), "{emissive}");
        assert_eq!(only_mesh(&import).textures.emissive, None, "{emissive}");
    }
}

#[test]
fn an_emissive_channel_over_a_zero_base_channel_is_l1202_naming_the_material() {
    let json = mesh_gltf(
        r#"{"name":"mismatched","pbrMetallicRoughness":{"baseColorFactor":[0.0,0.5,0.5,1]},"emissiveFactor":[0.5,0.5,0.5]}"#,
        false,
    );
    let messages = lossy_messages(&json);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("emissiveFactor red") && m.contains("mismatched")),
        "{messages:?}"
    );
    let import = import(
        json.as_bytes(),
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(true),
    )
    .expect("lossy mode imports the rest of the material");
    assert_eq!(import.warnings.len(), 1);
    let mesh = component(&import, 0, "Mesh").unwrap();
    assert_eq!(mesh["emissive"], 0.0);
    assert_eq!(mesh["color"][1], 0.5);
    assert!(import.textures.is_empty());
}

#[test]
fn a_near_zero_base_channel_is_refused_but_one_step_bakes() {
    let material = |base: f32| {
        format!(
            r#"{{"name":"dim","pbrMetallicRoughness":{{"baseColorFactor":[{base},0.5,0.5,1]}},"emissiveFactor":[0.5,0.25,0.5]}}"#
        )
    };
    let messages = lossy_messages(&mesh_gltf(&material(0.003), false));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("emissiveFactor red") && m.contains("dim")),
        "{messages:?}"
    );
    let import = import_clean(&mesh_gltf(&material(1.0 / 255.0), false));
    let emissive = component(&import, 0, "Mesh").unwrap()["emissive"]
        .as_f64()
        .unwrap();
    assert!(emissive.is_finite() && emissive > 100.0, "{emissive}");
    assert_eq!(import.textures.len(), 1);
}

#[test]
fn a_tinted_emissive_without_texcoord_0_is_l1202_naming_the_material() {
    let json = mesh_gltf_with(&tinted_material(""), "", POSITION_ONLY);
    let messages = lossy_messages(&json);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("TEXCOORD_0") && m.contains("tinted")),
        "{messages:?}"
    );
    let import = import(
        json.as_bytes(),
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(true),
    )
    .expect("lossy mode imports the rest of the material");
    assert_eq!(import.warnings.len(), 1, "{:?}", import.warnings);
    assert_eq!(component(&import, 0, "Mesh").unwrap()["emissive"], 0.0);
    assert!(import.textures.is_empty());
}

#[test]
fn unreadable_gltf_is_l1201() {
    let errors = import(
        b"this is not glTF at all",
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(false),
    )
    .unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1201");
}

// --- External sources: confined to the `.gltf`'s directory (L1204) -----------------------------

/// A fresh `<tmp>/<name>/` with an empty `model/` inside, the directory a `.gltf` would sit in.
fn uri_sandbox(name: &str) -> (PathBuf, PathBuf) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("gltf-uri")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    let model = root.join("model");
    std::fs::create_dir_all(&model).expect("the model directory");
    (root, model)
}

/// The 36 bytes of the one triangle `(0,0,0) (1,0,0) (0,1,0)`.
fn triangle_bytes() -> Vec<u8> {
    [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// A one-triangle glTF whose buffer is the external `buffer_uri`.
fn external_buffer_gltf(buffer_uri: &str) -> String {
    format!(
        r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"name":"tri","mesh":0}}],"meshes":[{{"name":"tri-mesh","primitives":[{{"attributes":{{"POSITION":0}}}}]}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}}],"buffers":[{{"byteLength":36,"uri":"{buffer_uri}"}}]}}"#
    )
}

fn import_beside(
    json: &str,
    dir: &Path,
) -> Result<Import, Vec<ludion_core::diagnostics::Diagnostic>> {
    import(
        json.as_bytes(),
        ExternalFiles::Beside(dir),
        &ImportOptions::lossy(false),
    )
}

fn assert_refused(result: Result<Import, Vec<ludion_core::diagnostics::Diagnostic>>, what: &str) {
    let errors = result
        .err()
        .unwrap_or_else(|| panic!("{what} must be refused"));
    assert_eq!(errors.len(), 1, "{what}: {errors:?}");
    assert_eq!(
        errors[0].code.as_str(),
        "L1204",
        "{what}: {}",
        errors[0].message
    );
}

#[test]
fn a_buffer_in_a_subdirectory_imports() {
    let (_root, model) = uri_sandbox("buffer-below");
    std::fs::create_dir_all(model.join("bin")).unwrap();
    std::fs::write(model.join("bin/tri.bin"), triangle_bytes()).unwrap();
    let import = import_beside(&external_buffer_gltf("bin/tri.bin"), &model)
        .expect("a buffer below the .gltf's directory imports");
    assert_eq!(only_mesh(&import).vertex_count(), 3);
}

#[test]
fn a_percent_encoded_image_path_below_the_directory_imports() {
    let (_root, model) = uri_sandbox("image-below");
    std::fs::create_dir_all(model.join("tex")).unwrap();
    std::fs::write(model.join("tex/a b.png"), base64::decode(PNG_B64).unwrap()).unwrap();
    let texture_json = r#","textures":[{"source":0,"sampler":0}],"samplers":[{}],"images":[{"mimeType":"image/png","uri":"tex/a%20b.png"}]"#;
    let json = mesh_gltf_with(
        r#"{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}"#,
        texture_json,
        WITH_UVS,
    );
    let import = import_beside(&json, &model).expect("`%20` decodes to the space in `a b.png`");
    assert_eq!(import.textures.len(), 1);
}

#[test]
fn an_external_buffer_that_leaves_the_directory_is_l1204() {
    let (root, model) = uri_sandbox("buffer-escape");
    std::fs::write(root.join("tri.bin"), triangle_bytes()).unwrap();
    std::fs::create_dir_all(model.join("bin")).unwrap();
    std::fs::write(model.join("bin/tri.bin"), triangle_bytes()).unwrap();
    let absolute = root.join("tri.bin").to_string_lossy().replace('\\', "/");
    for uri in [
        "../tri.bin",
        "bin/../../tri.bin",
        absolute.as_str(),
        "file:///tri.bin",
        "//host/share/x.bin",
        "%2e%2e/tri.bin",
        "%2E%2E%2Ftri.bin",
        "..%5Ctri.bin",
        "C%3A/tri.bin",
        "bin/tri.bin%00.png",
    ] {
        assert_refused(import_beside(&external_buffer_gltf(uri), &model), uri);
    }
}

#[test]
fn an_external_image_that_leaves_the_directory_is_l1204() {
    let (root, model) = uri_sandbox("image-escape");
    std::fs::write(root.join("a.png"), base64::decode(PNG_B64).unwrap()).unwrap();
    let texture_json = r#","textures":[{"source":0,"sampler":0}],"samplers":[{}],"images":[{"mimeType":"image/png","uri":"../a.png"}]"#;
    let json = mesh_gltf_with(
        r#"{"name":"m","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}"#,
        texture_json,
        WITH_UVS,
    );
    assert_refused(import_beside(&json, &model), "an image at ../a.png");
}

#[test]
fn a_directory_is_not_a_regular_file() {
    let (_root, model) = uri_sandbox("buffer-directory");
    std::fs::create_dir_all(model.join("bin")).unwrap();
    assert_refused(
        import_beside(&external_buffer_gltf("bin"), &model),
        "a directory",
    );
}

#[test]
fn a_missing_external_file_stays_l1201() {
    let (_root, model) = uri_sandbox("buffer-missing");
    let errors = import_beside(&external_buffer_gltf("nope.bin"), &model)
        .expect_err("a missing file cannot import");
    assert_eq!(errors[0].code.as_str(), "L1201");
}

#[test]
fn refused_files_accepts_no_external_source_even_when_it_is_there() {
    let (_root, model) = uri_sandbox("refused");
    std::fs::create_dir_all(model.join("bin")).unwrap();
    std::fs::write(model.join("bin/tri.bin"), triangle_bytes()).unwrap();
    let result = import(
        external_buffer_gltf("bin/tri.bin").as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(false),
    );
    assert_refused(result, "an external buffer under `Refused`");
}

// --- Malformed data is L1201, never a panic ---------------------------------------------------------

/// The one triangle's POSITION accessor over view 0 (the first 36 bytes).
const POSITION_ACCESSOR: &str = r#"{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}"#;
const POSITION_VIEW: &str = r#"{"buffer":0,"byteOffset":0,"byteLength":36}"#;

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// A one-node, one-mesh glTF over `bytes` as its only (embedded) buffer, with `accessors`,
/// `views` and `primitive` (the primitive's JSON object) verbatim and `extra` appended as further
/// top-level members (materials, textures).
fn raw_gltf(bytes: &[u8], accessors: &str, views: &str, primitive: &str, extra: &str) -> String {
    let length = bytes.len();
    let data = base64::encode(bytes);
    format!(
        r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"name":"tri","mesh":0}}],"meshes":[{{"name":"tri-mesh","primitives":[{primitive}]}}],"accessors":[{accessors}],"bufferViews":[{views}],"buffers":[{{"byteLength":{length},"uri":"data:application/octet-stream;base64,{data}"}}]{extra}}}"#
    )
}

/// [`raw_gltf`] with the triangle's POSITION as accessor 0 in view 0, plus `more_accessors` /
/// `more_views` (each starting with a comma).
fn triangle_gltf(
    bytes: &[u8],
    more_accessors: &str,
    more_views: &str,
    primitive: &str,
    extra: &str,
) -> String {
    raw_gltf(
        bytes,
        &format!("{POSITION_ACCESSOR}{more_accessors}"),
        &format!("{POSITION_VIEW}{more_views}"),
        primitive,
        extra,
    )
}

/// Imports `json` in default mode, expecting an `L1201` first, and returns its message.
fn malformed_message(json: &str) -> String {
    let errors = import(
        json.as_bytes(),
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(false),
    )
    .expect_err("malformed data must be refused");
    assert_eq!(errors[0].code.as_str(), "L1201", "{errors:?}");
    errors[0].message.clone()
}

/// Imports `json` in default mode, expecting a compiled mesh to fail `validate_mesh` (`L1111`)
/// with its pointer, the original help and the re-export context, and returns its message.
fn mesh_problem_message(json: &str) -> String {
    let errors = import(
        json.as_bytes(),
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(false),
    )
    .expect_err("an inconsistent mesh must be refused");
    let first = &errors[0];
    assert_eq!(first.code.as_str(), "L1111", "{errors:?}");
    assert!(first.path.is_some(), "{first:?}");
    let help = first.help.as_deref().unwrap_or_default();
    assert!(help.contains("re-export"), "{help}");
    assert!(help.contains("; this mesh document"), "{help}");
    first.message.clone()
}

/// A normal-map material on texture 0, and the texture block it needs.
fn normal_map_extra() -> String {
    format!(
        r#","materials":[{{"normalTexture":{{"index":0}}}}]{}"#,
        one_texture("{}")
    )
}

#[test]
fn a_zero_rotation_quaternion_is_l1201_naming_the_node() {
    let json =
        r#"{"asset":{"version":"2.0"},"nodes":[{"name":"spin","rotation":[0.0,0.0,0.0,0.0]}]}"#;
    assert!(malformed_message(json).contains("spin"));
}

#[test]
fn a_stride_shorter_than_the_element_is_l1201() {
    let json = raw_gltf(
        &triangle_bytes(),
        POSITION_ACCESSOR,
        r#"{"buffer":0,"byteOffset":0,"byteLength":36,"byteStride":4}"#,
        r#"{"attributes":{"POSITION":0}}"#,
        "",
    );
    let message = malformed_message(&json);
    assert!(
        message.contains("POSITION") && message.contains("stride"),
        "{message}"
    );
}

#[test]
fn an_accessor_that_overruns_its_view_is_l1201() {
    let json = raw_gltf(
        &triangle_bytes(),
        r#"{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}"#,
        POSITION_VIEW,
        r#"{"attributes":{"POSITION":0}}"#,
        "",
    );
    let message = malformed_message(&json);
    assert!(
        message.contains("POSITION") && message.contains("accessor 0"),
        "{message}"
    );
}

#[test]
fn a_zero_count_accessor_is_l1201() {
    let json = raw_gltf(
        &triangle_bytes(),
        r#"{"bufferView":0,"componentType":5126,"count":0,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}"#,
        POSITION_VIEW,
        r#"{"attributes":{"POSITION":0}}"#,
        "",
    );
    malformed_message(&json);
}

#[test]
fn a_view_past_the_end_of_its_buffer_is_l1201() {
    let json = raw_gltf(
        &triangle_bytes(),
        POSITION_ACCESSOR,
        r#"{"buffer":0,"byteOffset":0,"byteLength":72}"#,
        r#"{"attributes":{"POSITION":0}}"#,
        "",
    );
    malformed_message(&json);
}

#[test]
fn a_nan_position_is_l1111() {
    let mut positions = vec![0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    positions[4] = f32::NAN;
    let json = raw_gltf(
        &f32_bytes(&positions),
        POSITION_ACCESSOR,
        POSITION_VIEW,
        r#"{"attributes":{"POSITION":0}}"#,
        "",
    );
    assert!(mesh_problem_message(&json).contains("tri-mesh"));
}

#[test]
fn an_index_past_the_vertex_count_is_l1111_before_the_tangent_pass() {
    // Positions, UVs, then three u16 indices `[0, 1, 7]` (padded to a 4-byte multiple).
    let mut bytes = triangle_bytes();
    bytes.extend(f32_bytes(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
    for index in [0_u16, 1, 7, 0] {
        bytes.extend(index.to_le_bytes());
    }
    let json = triangle_gltf(
        &bytes,
        r#",{"bufferView":1,"componentType":5126,"count":3,"type":"VEC2"},{"bufferView":2,"componentType":5123,"count":3,"type":"SCALAR"}"#,
        r#",{"buffer":0,"byteOffset":36,"byteLength":24},{"buffer":0,"byteOffset":60,"byteLength":8}"#,
        r#"{"attributes":{"POSITION":0,"TEXCOORD_0":1},"indices":2,"material":0}"#,
        &normal_map_extra(),
    );
    assert!(mesh_problem_message(&json).contains("tri-mesh"));
}

#[test]
fn uvs_shorter_than_the_positions_are_l1111_before_the_tangent_pass() {
    let mut bytes = triangle_bytes();
    bytes.extend(f32_bytes(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
    let json = triangle_gltf(
        &bytes,
        r#",{"bufferView":1,"componentType":5126,"count":2,"type":"VEC2"}"#,
        r#",{"buffer":0,"byteOffset":36,"byteLength":24}"#,
        r#"{"attributes":{"POSITION":0,"TEXCOORD_0":1},"material":0}"#,
        &normal_map_extra(),
    );
    assert!(mesh_problem_message(&json).contains("tri-mesh"));
}

#[test]
fn a_position_accessor_of_the_wrong_type_is_l1201() {
    let json = raw_gltf(
        &triangle_bytes(),
        r#"{"bufferView":0,"componentType":5126,"count":3,"type":"VEC2","min":[0.0,0.0],"max":[1.0,1.0]}"#,
        POSITION_VIEW,
        r#"{"attributes":{"POSITION":0}}"#,
        "",
    );
    let message = malformed_message(&json);
    assert!(message.contains("POSITION"), "{message}");
}

#[test]
fn float_indices_are_l1201() {
    let mut bytes = triangle_bytes();
    bytes.extend(f32_bytes(&[0.0, 1.0, 2.0]));
    let json = triangle_gltf(
        &bytes,
        r#",{"bufferView":1,"componentType":5126,"count":3,"type":"SCALAR"}"#,
        r#",{"buffer":0,"byteOffset":36,"byteLength":12}"#,
        r#"{"attributes":{"POSITION":0},"indices":1}"#,
        "",
    );
    malformed_message(&json);
}

#[test]
fn an_overflowing_light_intensity_is_l1201() {
    let json = r#"{"asset":{"version":"2.0"},"extensionsUsed":["KHR_lights_punctual"],"extensions":{"KHR_lights_punctual":{"lights":[{"type":"point","intensity":1e39}]}},"nodes":[{"name":"lamp","extensions":{"KHR_lights_punctual":{"light":0}}}]}"#;
    malformed_message(json);
}

#[test]
fn an_overflowing_base_color_is_l1201() {
    let json = mesh_gltf_with(
        r#"{"name":"m","pbrMetallicRoughness":{"baseColorFactor":[1e39,0.0,0.0,1.0]}}"#,
        "",
        POSITION_ONLY,
    );
    malformed_message(&json);
}

#[test]
fn a_sparse_normal_accessor_is_l1202() {
    // Accessor 1 is a NORMAL with no base view, patched by one sparse substitution.
    let mut bytes = triangle_bytes();
    bytes.extend([0_u8, 0, 0, 0]);
    bytes.extend(f32_bytes(&[0.0, 0.0, 1.0]));
    let json = triangle_gltf(
        &bytes,
        r#",{"componentType":5126,"count":3,"type":"VEC3","sparse":{"count":1,"indices":{"bufferView":1,"componentType":5123},"values":{"bufferView":2}}}"#,
        r#",{"buffer":0,"byteOffset":36,"byteLength":2},{"buffer":0,"byteOffset":40,"byteLength":12}"#,
        r#"{"attributes":{"POSITION":0,"NORMAL":1}}"#,
        "",
    );
    let messages = lossy_messages(&json);
    assert!(
        messages.iter().any(|m| m.contains("sparse accessor")),
        "{messages:?}"
    );
}

// --- Structurally incomplete documents: L1201, never a panic (fuzz/findings/gltf_import) ---------

/// Imports `bytes`, asserts the first diagnostic is an `L1201`, and returns its message.
fn l1201_message(bytes: &[u8]) -> String {
    let diagnostics = import(
        bytes,
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(false),
    )
    .expect_err("a structurally incomplete document must be refused");
    let first = diagnostics.into_iter().next().expect("one diagnostic");
    assert_eq!(first.code.as_str(), "L1201", "{}", first.message);
    first.message
}

#[test]
fn an_image_without_a_source_is_l1201() {
    let text = String::from_utf8(fixture_bytes()).expect("utf-8 fixture");
    let broken = text.replace(r#""uri": "data:image/png"#, r#""ur": "data:image/png"#);
    assert_ne!(broken, text);
    let message = l1201_message(broken.as_bytes());
    assert!(
        message.contains("neither a uri nor a bufferView"),
        "{message}"
    );
}

#[test]
fn a_missing_accessors_array_is_l1201() {
    let text = String::from_utf8(fixture_bytes()).expect("utf-8 fixture");
    let broken = text.replace(r#""accessors""#, r#""accesors""#);
    assert_ne!(broken, text);
    let message = l1201_message(broken.as_bytes());
    assert!(message.contains("POSITION"), "{message}");
}

#[test]
fn an_image_view_without_a_mime_type_is_l1201() {
    let json = r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4,"uri":"data:application/octet-stream;base64,AAAAAA=="}],"bufferViews":[{"buffer":0,"byteLength":4}],"images":[{"bufferView":0}]}"#;
    let message = l1201_message(json.as_bytes());
    assert!(message.contains("has no mimeType"), "{message}");
}

#[test]
fn a_camera_missing_its_typed_projection_is_l1201() {
    let json = r#"{"asset":{"version":"2.0"},"cameras":[{"type":"perspective","orthographic":{"xmag":1.0,"ymag":1.0,"znear":0.1,"zfar":10.0}}],"nodes":[{"camera":0}]}"#;
    let message = l1201_message(json.as_bytes());
    assert!(message.contains("has no perspective object"), "{message}");
}

#[test]
fn an_out_of_range_index_is_l1201() {
    let json = r#"{"asset":{"version":"2.0"},"nodes":[{"mesh":7}]}"#;
    let message = l1201_message(json.as_bytes());
    assert!(message.contains("invalid glTF"), "{message}");
}

// --- The node graph is a forest: L1201 with a pointer, never a hang --------------------------------

/// Imports `bytes`, asserts the first diagnostic is an `L1201` at `pointer` whose help names the
/// forest rule, and returns its message.
fn forest_refusal(bytes: &[u8], pointer: &str) -> String {
    let diagnostics = import(
        bytes,
        ExternalFiles::Beside(Path::new(".")),
        &ImportOptions::lossy(false),
    )
    .expect_err("a node graph that is not a forest must be refused");
    let first = diagnostics.into_iter().next().expect("one diagnostic");
    assert_eq!(first.code.as_str(), "L1201", "{}", first.message);
    assert_eq!(first.path.as_deref(), Some(pointer), "{}", first.message);
    assert!(first.help.as_deref().unwrap_or_default().contains("forest"));
    first.message
}

fn forest_seed(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus/gltf_import")
            .join(name),
    )
    .expect("fuzz seed")
}

fn nodes_gltf(nodes: &str) -> String {
    format!(r#"{{"asset":{{"version":"2.0"}},"nodes":[{nodes}]}}"#)
}

#[test]
fn a_node_that_lists_itself_is_refused() {
    let message = forest_refusal(&forest_seed("node-self-loop.gltf"), "/nodes/0/children/0");
    assert!(message.contains("lists itself"), "{message}");
}

#[test]
fn a_two_node_cycle_is_refused() {
    let message = forest_refusal(&forest_seed("node-two-cycle.gltf"), "/nodes/1/children/0");
    assert!(message.contains("form a cycle"), "{message}");
}

#[test]
fn a_node_listed_by_two_parents_is_refused_at_the_second_listing() {
    let message = forest_refusal(&forest_seed("node-diamond.gltf"), "/nodes/2/children/0");
    assert!(
        message.contains("node 3") && message.contains("node 1") && message.contains("node 2"),
        "{message}"
    );
}

#[test]
fn a_node_listed_twice_by_one_parent_is_refused() {
    let json = nodes_gltf(r#"{"children":[1,1]},{}"#);
    let message = forest_refusal(json.as_bytes(), "/nodes/0/children/1");
    assert!(message.contains("twice"), "{message}");
}

#[test]
fn a_node_listing_itself_under_another_parent_is_refused() {
    let json = nodes_gltf(r#"{"children":[1]},{"children":[1]}"#);
    forest_refusal(json.as_bytes(), "/nodes/1/children/0");
}

#[test]
fn a_rootless_three_cycle_is_refused() {
    let json = nodes_gltf(r#"{"children":[1]},{"children":[2]},{"children":[0]}"#);
    forest_refusal(json.as_bytes(), "/nodes/2/children/0");
}

#[test]
fn a_cycle_below_a_root_is_a_node_listed_twice() {
    let json = nodes_gltf(r#"{"children":[1]},{"children":[2]},{"children":[1]}"#);
    let message = forest_refusal(json.as_bytes(), "/nodes/2/children/0");
    assert!(message.contains("at most one parent"), "{message}");
}

#[test]
fn a_spot_light_on_a_self_listing_node_is_refused_not_hung() {
    let json = spot_gltf("{}", None, None)
        .replace(r#"{"name":"lamp""#, r#"{"name":"lamp","children":[0]"#);
    assert_ne!(json, spot_gltf("{}", None, None));
    forest_refusal(json.as_bytes(), "/nodes/0/children/0");
}

#[test]
fn a_named_node_appears_by_name_in_the_refusal() {
    let json = nodes_gltf(r#"{"name":"hand","children":[0]}"#);
    let message = forest_refusal(json.as_bytes(), "/nodes/0/children/0");
    assert!(message.contains(r#"node 0 ("hand")"#), "{message}");
}

#[test]
fn a_fibonacci_diamond_chain_is_refused_promptly() {
    let nodes: Vec<String> = (0..64)
        .map(|i| match i {
            62 => r#"{"children":[63]}"#.to_owned(),
            63 => "{}".to_owned(),
            i => format!(r#"{{"children":[{},{}]}}"#, i + 1, i + 2),
        })
        .collect();
    let json = nodes_gltf(&nodes.join(","));
    forest_refusal(json.as_bytes(), "/nodes/1/children/0");
}

#[test]
fn an_out_of_range_child_stays_an_invalid_gltf_refusal() {
    let json = nodes_gltf(r#"{"children":[9]}"#);
    let message = l1201_message(json.as_bytes());
    assert!(message.contains("invalid glTF"), "{message}");
}

#[test]
fn a_three_level_tree_imports_with_its_parent_chain() {
    let json = nodes_gltf(r#"{"children":[1]},{"children":[2]},{}"#);
    let plan = import_clean(&json);
    assert!(component(&plan, 0, "Parent").is_none());
    assert!(component(&plan, 1, "Parent").is_some());
    assert!(component(&plan, 2, "Parent").is_some());
}

// --- Wrap diffuse: `extras.ludion_wrap` / `extras.ludion_scatter_color` ------------------------------

#[test]
fn material_extras_land_on_mesh_wrap_and_scatter_color() {
    let json = mesh_gltf(
        r#"{"name":"skin","extras":{"ludion_wrap":0.5,"ludion_scatter_color":[1.0,0.25,0.125],"other":"ignored"}}"#,
        false,
    );
    let import = import_clean(&json);
    let mesh = component(&import, 0, "Mesh").unwrap();
    assert_eq!(mesh["wrap"], 0.5);
    assert_eq!(mesh["scatter_color"], serde_json::json!([1.0, 0.25, 0.125]));
}

#[test]
fn wrap_alone_leaves_scatter_color_unemitted() {
    let json = mesh_gltf(r#"{"name":"leaf","extras":{"ludion_wrap":1.0}}"#, false);
    let import = import_clean(&json);
    let mesh = component(&import, 0, "Mesh").unwrap();
    assert_eq!(mesh["wrap"], 1.0);
    assert!(mesh.get("scatter_color").is_none());
}

#[test]
fn a_material_without_wrap_extras_emits_neither_key() {
    for material in [
        r#"{"name":"m"}"#,
        r#"{"name":"m","extras":{"blender_note":3}}"#,
        r#"{"name":"m","extras":[1,2]}"#,
    ] {
        let import = import_clean(&mesh_gltf(material, false));
        let mesh = component(&import, 0, "Mesh").unwrap();
        assert!(mesh.get("wrap").is_none(), "{material}");
        assert!(mesh.get("scatter_color").is_none(), "{material}");
    }
}

#[test]
fn a_bad_wrap_extra_is_refused_naming_the_material() {
    for extras in [
        r#""ludion_wrap":1.5"#,
        r#""ludion_wrap":-0.1"#,
        r#""ludion_wrap":"high""#,
        r#""ludion_scatter_color":[1.0,-0.5,1.0]"#,
        r#""ludion_scatter_color":[1.0,1.0]"#,
        r#""ludion_scatter_color":"red""#,
    ] {
        let json = mesh_gltf(
            &format!(r#"{{"name":"skin","extras":{{{extras}}}}}"#),
            false,
        );
        let errors = import(
            json.as_bytes(),
            ExternalFiles::Beside(Path::new(".")),
            &ImportOptions::lossy(false),
        )
        .unwrap_err();
        assert!(
            errors[0].message.contains("skin") && errors[0].message.contains("ludion_"),
            "{extras}: {}",
            errors[0].message
        );
    }
}

// --- Surface tags: `extras.ludion_surface` ---------------------------------------------------------

fn tagged_import(material: &str) -> Import {
    import_clean(&mesh_gltf(material, false))
}

#[test]
fn a_surface_extra_tags_the_document_with_one_run() {
    let import = tagged_import(r#"{"name":"Grate","extras":{"ludion_surface":"metal_grate"}}"#);
    let expected = ludion_scene::MeshSurfaces {
        names: vec!["metal_grate".to_owned()],
        runs: vec![[1, 0]],
    };
    let mesh = &import.meshes[0];
    assert_eq!(mesh.surfaces.as_ref(), Some(&expected));
    let text = ludion_scene::emit_mesh_with_surfaces(&mesh.doc, mesh.surfaces.as_ref());
    let (doc, surfaces) = ludion_scene::check_mesh_surfaces(&text).unwrap();
    assert_eq!(doc, mesh.doc);
    assert_eq!(surfaces, Some(expected));
}

#[test]
fn no_surface_extra_leaves_the_bytes_as_before() {
    for material in [
        r#"{"name":"concrete"}"#,
        r#"{"name":"skin","extras":{"ludion_wrap":0.5}}"#,
    ] {
        let import = tagged_import(material);
        let mesh = &import.meshes[0];
        assert_eq!(mesh.surfaces, None, "{material}");
        assert_eq!(
            ludion_scene::emit_mesh_with_surfaces(&mesh.doc, mesh.surfaces.as_ref()),
            ludion_scene::emit_mesh(&mesh.doc),
            "{material}"
        );
    }
}

#[test]
fn a_bad_surface_extra_is_l1201_naming_the_material() {
    for value in ["3", r#""""#, r#"["a"]"#] {
        let json = mesh_gltf(
            &format!(r#"{{"name":"Grate","extras":{{"ludion_surface":{value}}}}}"#),
            false,
        );
        let errors = import(
            json.as_bytes(),
            ExternalFiles::Beside(Path::new(".")),
            &ImportOptions::lossy(false),
        )
        .unwrap_err();
        assert_eq!(errors[0].code.as_str(), "L1201", "{value}");
        assert!(
            errors[0].message.contains("Grate") && errors[0].message.contains("ludion_surface"),
            "{value}: {}",
            errors[0].message
        );
    }
}

/// Two meshes over one triangle, each on its own material.
fn two_material_gltf(first: &str, second: &str) -> String {
    let mut bytes = Vec::new();
    for value in [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let buffer_b64 = base64::encode(&bytes);
    format!(
        r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"name":"a","mesh":0}},{{"name":"b","mesh":1}}],"meshes":[{{"name":"ma","primitives":[{{"attributes":{{"POSITION":0}},"material":0}}]}},{{"name":"mb","primitives":[{{"attributes":{{"POSITION":0}},"material":1}}]}}],"materials":[{first},{second}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}}],"buffers":[{{"byteLength":36,"uri":"data:application/octet-stream;base64,{buffer_b64}"}}]}}"#
    )
}

#[test]
fn two_tagged_materials_import_as_two_documents_with_one_tag_each() {
    let json = two_material_gltf(
        r#"{"extras":{"ludion_surface":"metal_grate"}}"#,
        r#"{"extras":{"ludion_surface":"dirt"}}"#,
    );
    let import = import_clean(&json);
    assert_eq!(import.meshes.len(), 2);
    let names: Vec<&[String]> = import
        .meshes
        .iter()
        .map(|m| m.surfaces.as_ref().unwrap().names.as_slice())
        .collect();
    assert_eq!(names, [["metal_grate"], ["dirt"]]);
}

#[test]
fn import_gltf_writes_the_tag_pair_and_a_second_import_is_identical() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("import_surface_tags_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("floor.gltf");
    std::fs::write(
        &input,
        mesh_gltf(
            r#"{"name":"Grate","extras":{"ludion_surface":"metal_grate"}}"#,
            false,
        ),
    )
    .unwrap();
    let out = dir.join("out");
    let run = || ludion_import::import_gltf(&input, &out, &ImportOptions::lossy(false)).unwrap();
    run();
    let meshes: Vec<PathBuf> = std::fs::read_dir(out.join("meshes"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(meshes.len(), 1);
    let first = std::fs::read_to_string(&meshes[0]).unwrap();
    assert!(
        first.contains(
            "surfaces:
  - metal_grate"
        ),
        "{first}"
    );
    assert!(
        first.contains(
            "triangle_surfaces:
  - - 1
    - 0"
        ),
        "{first}"
    );
    let (_, surfaces) = ludion_scene::check_mesh_surfaces(&first).unwrap();
    assert_eq!(surfaces.unwrap().runs, [[1, 0]]);
    run();
    assert_eq!(std::fs::read_to_string(&meshes[0]).unwrap(), first);
    let _ = std::fs::remove_dir_all(&dir);
}
