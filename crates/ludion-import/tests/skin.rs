//! The importer's skin mapping: the committed skinned fixtures, then hand-built one-triangle
//! documents for the weight rules and every refusal.

use ludion_import::{ExternalFiles, Import, import};
use ludion_scene::{MESH_JOINT_LIMIT, MeshDocument, check_mesh, emit_mesh};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}.glb", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).expect("the committed fixture exists")
}

fn import_fixture(name: &str) -> Import {
    import(&fixture(name), ExternalFiles::Refused, false).expect("the fixture imports clean")
}

fn skin_of(import: &Import) -> &ludion_scene::MeshSkin {
    import.meshes[0]
        .doc
        .skin
        .as_ref()
        .expect("the mesh is skinned")
}

fn entity_names(import: &Import) -> Vec<&str> {
    import
        .prefab
        .entities
        .iter()
        .map(|e| e.name.as_deref().expect("imported entities are named"))
        .collect()
}

fn parent_index(import: &Import, entity: usize) -> Option<u64> {
    import.prefab.entities[entity]
        .components
        .iter()
        .find(|(name, _)| name == "Parent")
        .and_then(|(_, value)| value["index"].as_u64())
}

#[test]
fn the_tube_imports_clean_with_its_joints_folded() {
    let import = import_fixture("skin-tube");
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
    assert_eq!(entity_names(&import), ["skin-tube", "skin-tube-rig"]);
    assert_eq!(parent_index(&import, 0), Some(1));

    let skin = skin_of(&import);
    let names: Vec<&str> = skin.joints.iter().map(|j| j.name.as_str()).collect();
    assert_eq!(names, ["lower", "upper"]);
    assert_eq!(skin.joints[0].parent, None);
    assert_eq!(skin.joints[1].parent, Some(0));
    assert_eq!(skin.joints[1].rest.translation, [0.0, 1.0, 0.0]);

    let mut blended = false;
    for (weights, bones) in skin.weights.chunks(4).zip(skin.bone_indices.chunks(4)) {
        let sum: f64 = weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9, "weights {weights:?} sum to {sum}");
        assert!(weights.windows(2).all(|w| w[0] >= w[1]), "{weights:?}");
        for (weight, bone) in weights.iter().zip(bones) {
            assert!(*weight > 0.0 || *bone == 0, "{weights:?} {bones:?}");
        }
        blended |= weights[1] > 0.0;
    }
    assert!(blended, "some vertex is influenced by two joints");
}

#[test]
fn the_mannequin_imports_clean_with_its_joints_folded() {
    let import = import_fixture("skin-mannequin");
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
    let skin = skin_of(&import);
    let expected = [
        ("pelvis", None),
        ("torso", Some(0)),
        ("head", Some(1)),
        ("upper_arm_l", Some(1)),
        ("lower_arm_l", Some(3)),
        ("upper_arm_r", Some(1)),
        ("lower_arm_r", Some(5)),
        ("upper_leg_l", Some(0)),
        ("lower_leg_l", Some(7)),
        ("upper_leg_r", Some(0)),
        ("lower_leg_r", Some(9)),
    ];
    let actual: Vec<(&str, Option<u32>)> = skin
        .joints
        .iter()
        .map(|j| (j.name.as_str(), j.parent))
        .collect();
    assert_eq!(actual, expected);
    for joint in &skin.joints {
        assert!(
            !entity_names(&import).contains(&joint.name.as_str()),
            "{} is folded, not an entity",
            joint.name
        );
    }

    let model = skin.rest_model_matrices().unwrap();
    for (joint, want) in [
        ("pelvis", [0.0, 0.52, 0.0]),
        ("head", [0.0, 0.86, 0.0]),
        ("upper_leg_l", [-0.08, 0.52, 0.0]),
    ] {
        let index = skin.joints.iter().position(|j| j.name == joint).unwrap();
        for axis in 0..3 {
            let got = model[index][12 + axis];
            assert!(
                (got - want[axis]).abs() < 1e-5,
                "{joint} head {got} vs {want:?}"
            );
        }
    }
}

#[test]
fn the_mannequin_rest_palette_is_identity() {
    use ludion_core::math::glam::DMat4;

    let import = import_fixture("skin-mannequin");
    let skin = skin_of(&import);
    for (joint, matrix) in skin.joints.iter().zip(skin.rest_palette().unwrap()) {
        for (element, want) in matrix.iter().zip(DMat4::IDENTITY.to_cols_array()) {
            assert!(
                (element - want).abs() < 1e-5,
                "{}: {matrix:?} is not identity",
                joint.name
            );
        }
    }
}

#[test]
fn a_skinned_import_is_deterministic_and_round_trips() {
    for name in ["skin-tube", "skin-mannequin"] {
        let first = import_fixture(name);
        let second = import_fixture(name);
        let text = emit_mesh(&first.meshes[0].doc);
        assert_eq!(text, emit_mesh(&second.meshes[0].doc), "{name}");
        let parsed: MeshDocument = check_mesh(&text).expect("the emitted mesh checks clean");
        assert_eq!(parsed, first.meshes[0].doc, "{name}");
    }
}

// --- Hand-built documents --------------------------------------------------------------------

/// The pieces of a one-triangle skinned glTF a test varies. Accessor 0 is POSITION, 1 `JOINTS_0`
/// (`VEC4` u8), 2 `WEIGHTS_0` (`VEC4` f32).
struct Spec {
    joints: [[u8; 4]; 3],
    weights: [[f32; 4]; 3],
    nodes: String,
    skins: String,
    attributes: &'static str,
    extra_meshes: &'static str,
}

impl Default for Spec {
    fn default() -> Self {
        Spec {
            joints: [[0, 1, 0, 0]; 3],
            weights: [[0.2, 0.6, 0.0, 0.0]; 3],
            nodes: r#"{"name":"body","mesh":0,"skin":0},{"name":"j0","translation":[0,1,0],"children":[2]},{"name":"j1","translation":[1,0,0]}"#.into(),
            skins: r#"{"joints":[1,2]}"#.into(),
            attributes: r#""POSITION":0,"JOINTS_0":1,"WEIGHTS_0":2"#,
            extra_meshes: "",
        }
    }
}

impl Spec {
    fn gltf(&self) -> String {
        let mut bytes = Vec::new();
        for value in [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend(self.joints.iter().flatten());
        for value in self.weights.iter().flatten() {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let buffer = base64::encode(&bytes);
        let skins = if self.skins.is_empty() {
            String::new()
        } else {
            format!(r#""skins":[{}],"#, self.skins)
        };
        format!(
            r#"{{"asset":{{"version":"2.0"}},"nodes":[{nodes}],{skins}"meshes":[{{"name":"tri","primitives":[{{"attributes":{{{attributes}}}}}]}}{extra_meshes}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}},{{"bufferView":1,"componentType":5121,"count":3,"type":"VEC4"}},{{"bufferView":2,"componentType":5126,"count":3,"type":"VEC4"}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":12}},{{"buffer":0,"byteOffset":48,"byteLength":48}}],"buffers":[{{"byteLength":96,"uri":"data:application/octet-stream;base64,{buffer}"}}]}}"#,
            nodes = self.nodes,
            attributes = self.attributes,
            extra_meshes = self.extra_meshes,
        )
    }

    fn import(&self, lossy: bool) -> Result<Import, Vec<ludion_core::diagnostics::Diagnostic>> {
        import(self.gltf().as_bytes(), ExternalFiles::Refused, lossy)
    }

    /// The single `L1202` a strict import fails with: its message.
    fn refusal(&self, code: &str) -> String {
        let errors = self.import(false).expect_err("the import is refused");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].code.as_str(), code, "{errors:?}");
        errors[0].message.clone()
    }

    /// The sole refusal's help text.
    fn refusal_help(&self, code: &str) -> String {
        let errors = self.import(false).expect_err("the import is refused");
        assert_eq!(errors[0].code.as_str(), code, "{errors:?}");
        errors[0].help.clone().unwrap_or_default()
    }
}

/// f32 weights (0.2, 0.6) are not exact in f64, so compare within the f32 rounding.
fn assert_close(got: &[f64], want: &[f64]) {
    assert!(
        got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-6),
        "{got:?} vs {want:?}"
    );
}

#[test]
fn weights_are_renormalized_and_sorted_heaviest_first() {
    let import = Spec::default().import(false).expect("imports clean");
    assert!(import.warnings.is_empty());
    let skin = skin_of(&import);
    assert_close(&skin.weights[..4], &[0.75, 0.25, 0.0, 0.0]);
    assert_eq!(&skin.bone_indices[..4], &[1, 0, 0, 0]);
}

#[test]
fn a_child_first_joint_list_is_reordered_parent_first() {
    // `skin.joints` lists j1 (the child) before j0 (its parent); vertices name glTF slots.
    let spec = Spec {
        skins: r#"{"joints":[2,1]}"#.into(),
        // slot 0 = j1 (weight 0.6), slot 1 = j0 (weight 0.2).
        weights: [[0.6, 0.2, 0.0, 0.0]; 3],
        joints: [[0, 1, 0, 0]; 3],
        ..Spec::default()
    };
    let import = spec.import(false).expect("imports clean");
    let skin = skin_of(&import);
    let names: Vec<&str> = skin.joints.iter().map(|j| j.name.as_str()).collect();
    assert_eq!(names, ["j0", "j1"]);
    assert_eq!(skin.joints[1].parent, Some(0));
    // j1 (0.75) is index 1 after the reorder, j0 (0.25) index 0.
    assert_close(&skin.weights[..4], &[0.75, 0.25, 0.0, 0.0]);
    assert_eq!(&skin.bone_indices[..4], &[1, 0, 0, 0]);
}

#[test]
fn a_vertex_with_no_weight_is_bound_to_the_root_with_a_warning() {
    let mut spec = Spec::default();
    spec.weights[1] = [0.0; 4];
    let import = spec.import(false).expect("still imports, non-lossy");
    assert_eq!(import.warnings.len(), 1, "{:?}", import.warnings);
    assert_eq!(import.warnings[0].code.as_str(), "L1205");
    assert!(import.warnings[0].message.contains("1 vertices"));
    assert!(import.warnings[0].message.contains("`j0`"));
    let skin = skin_of(&import);
    assert_eq!(&skin.weights[4..8], &[1.0, 0.0, 0.0, 0.0]);
    assert_eq!(&skin.bone_indices[4..8], &[0, 0, 0, 0]);
}

#[test]
fn a_second_joint_set_is_l1202() {
    let spec = Spec {
        attributes: r#""POSITION":0,"JOINTS_0":1,"WEIGHTS_0":2,"JOINTS_1":1,"WEIGHTS_1":2"#,
        ..Spec::default()
    };
    let message = spec.refusal("L1202");
    assert!(message.contains("second joint/weight set"), "{message}");
    assert!(message.contains("mesh 0"), "{message}");
    assert!(
        spec.import(true).expect("lossy imports").meshes[0]
            .doc
            .skin
            .is_some()
    );
}

#[test]
fn a_skinned_primitive_without_weights_is_l1201() {
    let spec = Spec {
        attributes: r#""POSITION":0,"JOINTS_0":1"#,
        ..Spec::default()
    };
    let message = spec.refusal("L1201");
    assert!(message.contains("WEIGHTS_0"), "{message}");
}

#[test]
fn a_mesh_worn_by_two_skinned_nodes_is_l1202() {
    let spec = Spec {
        nodes: r#"{"name":"body","mesh":0,"skin":0},{"name":"j0","children":[2]},{"name":"j1"},{"name":"twin","mesh":0,"skin":1}"#.into(),
        skins: r#"{"joints":[1,2]},{"joints":[1,2]}"#.into(),
        ..Spec::default()
    };
    let message = spec.refusal("L1202");
    assert!(message.contains("twin"), "{message}");
    assert!(message.contains("shared"), "{message}");
}

#[test]
fn a_skin_over_the_joint_cap_is_l1202() {
    let joints = MESH_JOINT_LIMIT + 1;
    let mut nodes = String::from(r#"{"name":"body","mesh":0,"skin":0}"#);
    for i in 0..joints {
        nodes.push_str(&format!(r#",{{"name":"b{i}"}}"#));
    }
    let list: Vec<String> = (1..=joints).map(|n| n.to_string()).collect();
    let spec = Spec {
        nodes,
        skins: format!(r#"{{"name":"big","joints":[{}]}}"#, list.join(",")),
        ..Spec::default()
    };
    let message = spec.refusal("L1202");
    assert!(message.contains("257 joints"), "{message}");
    assert!(message.contains("big"), "{message}");
    let lossy = spec.import(true).expect("lossy imports");
    assert!(
        lossy.meshes[0].doc.skin.is_none(),
        "the mesh imports unskinned"
    );
}

#[test]
fn a_node_under_a_joint_is_l1202() {
    let spec = Spec {
        nodes: r#"{"name":"body","mesh":0,"skin":0},{"name":"j0","children":[2,3]},{"name":"j1"},{"name":"prop"}"#.into(),
        ..Spec::default()
    };
    let message = spec.refusal("L1202");
    assert!(message.contains("parented to a skin joint"), "{message}");
    assert!(message.contains("prop"), "{message}");
    let help = spec.refusal_help("L1202");
    assert!(help.contains("out of the joint chain"), "{help}");
    let lossy = spec.import(true).expect("lossy imports");
    assert_eq!(entity_names(&lossy), ["body"]);
}

const Z90: &str = "[0,0,0.70710678,0.70710678]";
const X30: &str = "[0.25881905,0,0,0.96592583]";

/// `body`, a root joint `j0`, a non-joint `helper` under it, then `j1` and `j2` below the helper.
/// `helper_scale` is the JSON scale of the helper.
fn helper_rig(helper_scale: &str) -> Spec {
    Spec {
        nodes: format!(
            r#"{{"name":"body","mesh":0,"skin":0}},{{"name":"j0","translation":[0,1,0],"children":[2]}},{{"name":"helper","translation":[0.5,0,0],"rotation":{Z90},"scale":{helper_scale},"children":[3]}},{{"name":"j1","translation":[1,0,0],"rotation":{X30},"children":[4]}},{{"name":"j2","translation":[0,1,0]}}"#
        ),
        skins: r#"{"joints":[1,3,4]}"#.into(),
        ..Spec::default()
    }
}

fn dmat(
    translation: [f64; 3],
    rotation: [f32; 4],
    scale: [f64; 3],
) -> ludion_core::math::glam::DMat4 {
    use ludion_core::math::glam::{DMat4, DQuat, DVec3};
    let r = rotation.map(f64::from);
    DMat4::from_scale_rotation_translation(
        DVec3::from_array(scale),
        DQuat::from_xyzw(r[0], r[1], r[2], r[3]).normalize(),
        DVec3::from_array(translation),
    )
}

#[test]
fn a_helper_node_between_joints_folds_into_the_rest_of_the_joints_below() {
    let spec = helper_rig("[2,2,2]");
    let message = spec.refusal("L1202");
    assert!(message.contains("helper"), "{message}");
    assert!(message.contains("inside a skin's joint chain"), "{message}");
    let errors = spec.import(false).expect_err("strict refuses");
    let help = errors[0].help.as_deref().unwrap_or_default();
    assert!(help.contains("parent the joints"), "{help}");

    let import = spec.import(true).expect("lossy imports");
    let skin = skin_of(&import);
    let names: Vec<&str> = skin.joints.iter().map(|j| j.name.as_str()).collect();
    assert_eq!(names, ["j0", "j1", "j2"]);
    assert_eq!(skin.joints[1].parent, Some(0));
    assert_eq!(skin.joints[2].parent, Some(1));

    let id = [0.0, 0.0, 0.0, 1.0];
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let z90 = [0.0, 0.0, half, half];
    let x30 = [0.258_819_05_f32, 0.0, 0.0, 0.965_925_8];
    let j0 = dmat([0.0, 1.0, 0.0], id, [1.0; 3]);
    let helper = dmat([0.5, 0.0, 0.0], z90, [2.0; 3]);
    let j1 = j0 * helper * dmat([1.0, 0.0, 0.0], x30, [1.0; 3]);
    let j2 = j1 * dmat([0.0, 1.0, 0.0], id, [1.0; 3]);
    let model = skin.rest_model_matrices().unwrap();
    for (joint, want) in [j0, j1, j2].iter().enumerate() {
        for (element, want) in model[joint].iter().zip(want.to_cols_array()) {
            assert!(
                (element - want).abs() < 1e-6,
                "{}: {:?} vs {:?}",
                skin.joints[joint].name,
                model[joint],
                want
            );
        }
    }
}

#[test]
fn a_folded_rest_that_shears_is_l1202() {
    let spec = helper_rig("[1,2,1]");
    let import = spec.import(true).expect("lossy imports");
    let shear = import
        .warnings
        .iter()
        .find(|w| w.message.contains("shear"))
        .unwrap_or_else(|| panic!("no shear warning in {:?}", import.warnings));
    assert_eq!(shear.code.as_str(), "L1202");
    assert!(shear.message.contains("j1"), "{}", shear.message);
    assert!(!shear.message.contains("  "), "{}", shear.message);
    let errors = spec.import(false).expect_err("strict refuses");
    let error = errors
        .iter()
        .find(|e| e.code.as_str() == "L1202" && e.message.contains("shear"))
        .unwrap_or_else(|| panic!("no shear refusal in {errors:?}"));
    assert!(error.message.contains("j1"), "{}", error.message);
    let help = error.help.as_deref().unwrap_or_default();
    assert!(help.contains("Ctrl+A"), "{help}");
}

const SKEW_MATRIX: &str = "[1,0,0,0, 0.5,1,0,0, 0,0,1,0, 0,0,0,1]";

/// A helper between two joints whose own `matrix` skews: `local()` keeps the skew, so the fold
/// reports it.
#[test]
fn a_skewed_helper_matrix_between_joints_is_a_shear_naming_the_joint_below() {
    // No rotation below the helper, so only the helper's own skew can shear the fold.
    let mut spec = helper_rig("[1,1,1]");
    spec.nodes = spec
        .nodes
        .replace(
            &format!(r#""translation":[0.5,0,0],"rotation":{Z90},"scale":[1,1,1]"#),
            &format!(r#""matrix":{SKEW_MATRIX}"#),
        )
        .replace(&format!(r#","rotation":{X30}"#), "");
    assert!(spec.nodes.contains("matrix") && !spec.nodes.contains("0.2588"));
    let import = spec.import(true).expect("lossy imports");
    let shears: Vec<_> = import
        .warnings
        .iter()
        .filter(|w| w.message.contains("shear"))
        .collect();
    assert_eq!(shears.len(), 1, "{:?}", import.warnings);
    assert_eq!(shears[0].code.as_str(), "L1202");
    assert!(shears[0].message.contains("j1"), "{}", shears[0].message);
}

/// A joint's own `matrix` that skews is checked as its rest, not read as bare translation/
/// rotation/scale.
#[test]
fn a_joint_whose_own_matrix_shears_is_l1202_naming_it() {
    let spec = Spec {
        nodes: format!(
            r#"{{"name":"body","mesh":0,"skin":0}},{{"name":"j0","matrix":{SKEW_MATRIX},"children":[2]}},{{"name":"j1","translation":[1,0,0]}}"#
        ),
        ..Spec::default()
    };
    let message = spec.refusal("L1202");
    assert!(
        message.contains("shear") && message.contains("j0"),
        "{message}"
    );
    let import = spec.import(true).expect("lossy imports");
    assert_eq!(import.warnings.len(), 1, "{:?}", import.warnings);
}

/// A root whose rest shears against the mesh's frame: the mesh node is scaled non-uniformly and
/// the root rotated, so the mesh's inverse times the root is not a translation/rotation/scale.
#[test]
fn a_root_rest_that_shears_against_the_mesh_frame_is_l1202() {
    let nodes = |scale: &str| {
        format!(
            r#"{{"name":"body","mesh":0,"skin":0,"scale":{scale}}},{{"name":"j0","rotation":{X30},"children":[2]}},{{"name":"j1","translation":[1,0,0]}}"#
        )
    };
    let spec = Spec {
        nodes: nodes("[1,2,1]"),
        ..Spec::default()
    };
    let message = spec.refusal("L1202");
    assert!(message.contains("shear"), "{message}");
    assert!(message.contains("j0"), "{message}");
    let lossy = spec.import(true).expect("lossy imports");
    assert_eq!(lossy.warnings.len(), 1, "{:?}", lossy.warnings);

    let control = Spec {
        nodes: nodes("[2,2,2]"),
        ..Spec::default()
    };
    let import = control
        .import(false)
        .expect("a uniform scale imports strict");
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);
}

fn assert_rests_finite(import: &Import) {
    for joint in &skin_of(import).joints {
        let rest = &joint.rest;
        let all = [&rest.translation[..], &rest.rotation[..], &rest.scale[..]].concat();
        assert!(
            all.iter().all(|v| v.is_finite()),
            "{}: {rest:?}",
            joint.name
        );
    }
}

/// A zero-scale axis cannot be decomposed (the columns have no direction): the rest imports with
/// scale 0, an identity rotation and an `L1202`, never NaN.
#[test]
fn a_zero_scale_rest_collapses_with_an_l1202() {
    // A folded rest.
    let spec = helper_rig("[0,0,0]");
    let errors = spec.import(false).expect_err("strict refuses");
    assert!(
        errors.iter().all(|e| e.code.as_str() == "L1202"),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("j1") && e.message.contains("zero scale")),
        "{errors:?}"
    );
    let import = spec.import(true).expect("lossy imports");
    assert!(
        import
            .warnings
            .iter()
            .any(|w| w.message.contains("j1") && w.message.contains("zero scale")),
        "{:?}",
        import.warnings
    );
    let skin = skin_of(&import);
    assert_close(&skin.joints[1].rest.translation, &[0.5, 0.0, 0.0]);
    assert_close(&skin.joints[1].rest.rotation, &[0.0, 0.0, 0.0, 1.0]);
    assert_close(&skin.joints[1].rest.scale, &[0.0; 3]);
    let id = [0.0, 0.0, 0.0, 1.0];
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let x30 = [0.258_819_05_f32, 0.0, 0.0, 0.965_925_8];
    let j0 = dmat([0.0, 1.0, 0.0], id, [1.0; 3]);
    let helper = dmat([0.5, 0.0, 0.0], [0.0, 0.0, half, half], [0.0; 3]);
    let j1 = j0 * helper * dmat([1.0, 0.0, 0.0], x30, [1.0; 3]);
    let j2 = j1 * dmat([0.0, 1.0, 0.0], id, [1.0; 3]);
    let model = skin.rest_model_matrices().unwrap();
    for (joint, want) in [j0, j1, j2].iter().enumerate() {
        for (element, want) in model[joint].iter().zip(want.to_cols_array()) {
            assert!(
                element.is_finite() && (element - want).abs() < 1e-6,
                "joint {joint}: {:?} vs {want}",
                model[joint]
            );
        }
    }

    // A root under a zero-scale node, and a zero-scale mesh node.
    let under_zero_node = Spec {
        nodes: r#"{"name":"body","mesh":0,"skin":0},{"name":"rig","scale":[0,0,0],"children":[2]},{"name":"j0","children":[3]},{"name":"j1","translation":[1,0,0]}"#.into(),
        skins: r#"{"joints":[2,3]}"#.into(),
        ..Spec::default()
    };
    let zero_mesh = Spec {
        nodes: r#"{"name":"body","mesh":0,"skin":0,"scale":[0,0,0]},{"name":"j0","translation":[0,1,0],"children":[2]},{"name":"j1","translation":[1,0,0]}"#.into(),
        ..Spec::default()
    };
    for spec in [under_zero_node, zero_mesh] {
        let message = spec.refusal("L1202");
        assert!(message.contains("j0"), "{message}");
        assert!(message.contains("zero scale"), "{message}");
        let help = spec.refusal_help("L1202");
        assert!(help.contains("flattens"), "{help}");
        assert_rests_finite(&spec.import(true).expect("lossy imports"));
    }
}

#[test]
fn a_mesh_on_a_joint_is_l1202() {
    let spec = Spec {
        nodes: r#"{"name":"body","mesh":0,"skin":0},{"name":"j0","children":[2]},{"name":"j1","mesh":1}"#.into(),
        extra_meshes: r#",{"name":"hat","primitives":[{"attributes":{"POSITION":0}}]}"#,
        ..Spec::default()
    };
    let message = spec.refusal("L1202");
    assert!(message.contains("a mesh on a skin joint"), "{message}");
    assert!(message.contains("j1"), "{message}");
}

#[test]
fn an_unskinned_mesh_ignores_joint_attributes() {
    let spec = Spec {
        nodes: r#"{"name":"body","mesh":0}"#.into(),
        skins: String::new(),
        ..Spec::default()
    };
    let import = spec.import(false).expect("imports clean");
    assert!(import.warnings.is_empty());
    assert!(import.meshes[0].doc.skin.is_none());
}
