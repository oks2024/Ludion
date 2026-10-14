//! Skinned mesh documents: the `joints` / `weights` / `bone_indices` keys — the canonical form,
//! the all-or-nothing rule, structural errors (`L1190`) and data errors (`L1191`).

use ludion_scene::{
    JointRest, MESH_JOINT_LIMIT, MeshDocument, MeshJoint, MeshSkin, check_mesh, emit_mesh,
    parse_mesh, read_mesh_joints, validate_mesh,
};

/// `skinned_tri()` in its canonical text. Hand-written so a change to key order, nesting or the
/// root's omitted `parent` fails here.
const SKINNED_TRI: &str = "\
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
joints:
  - name: pelvis
    rest:
      t:
        - 0.0
        - 0.0
        - 0.0
      r:
        - 0.0
        - 0.0
        - 0.0
        - 1.0
      s:
        - 1.0
        - 1.0
        - 1.0
    inverse_bind:
      - 1.0
      - 0.0
      - 0.0
      - 0.0
      - 0.0
      - 1.0
      - 0.0
      - 0.0
      - 0.0
      - 0.0
      - 1.0
      - 0.0
      - 0.0
      - 0.0
      - 0.0
      - 1.0
  - name: torso
    parent: 0
    rest:
      t:
        - 0.0
        - 0.5
        - 0.0
      r:
        - 0.0
        - 0.0
        - 0.6
        - 0.8
      s:
        - 1.0
        - 1.0
        - 1.0
    inverse_bind:
      - 1.0
      - 0.0
      - 0.0
      - 0.0
      - 0.0
      - 1.0
      - 0.0
      - 0.0
      - 0.0
      - 0.0
      - 1.0
      - 0.0
      - 0.0
      - -0.5
      - 0.0
      - 1.0
weights:
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 0.5
  - 0.5
  - 0.0
  - 0.0
  - 0.25
  - 0.75
  - 0.0
  - 0.0
bone_indices:
  - 0
  - 0
  - 0
  - 0
  - 0
  - 1
  - 0
  - 0
  - 0
  - 1
  - 0
  - 0
indices:
  - 0
  - 1
  - 2
";

fn skinned_tri() -> MeshDocument {
    let mut identity = [0.0; 16];
    for i in 0..4 {
        identity[i * 5] = 1.0;
    }
    let mut torso_bind = identity;
    torso_bind[13] = -0.5;
    MeshDocument {
        positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        skin: Some(MeshSkin {
            joints: vec![
                MeshJoint {
                    name: "pelvis".to_string(),
                    parent: None,
                    rest: JointRest {
                        translation: [0.0, 0.0, 0.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0, 1.0, 1.0],
                    },
                    inverse_bind: identity,
                },
                MeshJoint {
                    name: "torso".to_string(),
                    parent: Some(0),
                    rest: JointRest {
                        translation: [0.0, 0.5, 0.0],
                        rotation: [0.0, 0.0, 0.6, 0.8],
                        scale: [1.0, 1.0, 1.0],
                    },
                    inverse_bind: torso_bind,
                },
            ],
            weights: vec![1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.25, 0.75, 0.0, 0.0],
            bone_indices: vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0],
        }),
        indices: vec![0, 1, 2],
        ..MeshDocument::default()
    }
}

fn skin_mut(doc: &mut MeshDocument) -> &mut MeshSkin {
    doc.skin.as_mut().expect("the fixture is skinned")
}

/// Removes the top-level block `key:` (the key line and every indented line under it).
fn strip_key(text: &str, key: &str) -> String {
    let header = format!("{key}:");
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        if line == header {
            skipping = true;
        } else if skipping && line.starts_with(' ') {
            // still inside the removed block
        } else {
            skipping = false;
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The first diagnostic with `code` at `path`, or a panic listing what was reported.
fn expect_at<'a>(
    diagnostics: &'a [ludion_core::diagnostics::Diagnostic],
    code: &str,
    path: &str,
) -> &'a ludion_core::diagnostics::Diagnostic {
    diagnostics
        .iter()
        .find(|d| d.code.as_str() == code && d.path.as_deref() == Some(path))
        .unwrap_or_else(|| {
            let seen: Vec<_> = diagnostics
                .iter()
                .map(|d| format!("{} at {:?}", d.code.as_str(), d.path))
                .collect();
            panic!("expected {code} at {path}, got {seen:?}")
        })
}

#[test]
fn canonical_form_golden_with_skin() {
    let doc = skinned_tri();
    assert!(validate_mesh(&doc).is_empty());
    let text = emit_mesh(&doc);
    assert_eq!(text, SKINNED_TRI);
    let checked = check_mesh(SKINNED_TRI).expect("the canonical text checks clean");
    assert_eq!(checked, doc);
    assert_eq!(emit_mesh(&checked), SKINNED_TRI);
}

#[test]
fn skin_keys_are_all_or_nothing() {
    let keys = ["joints", "weights", "bone_indices"];
    for present in keys {
        let mut text = SKINNED_TRI.to_string();
        for other in keys.into_iter().filter(|k| *k != present) {
            text = strip_key(&text, other);
        }
        let diagnostics = parse_mesh(&text).unwrap_err();
        expect_at(&diagnostics, "L1190", &format!("/{present}"));
    }
    // A pair reports at the first present key, in document order.
    for (absent, first) in [
        ("weights", "joints"),
        ("bone_indices", "joints"),
        ("joints", "weights"),
    ] {
        let diagnostics = parse_mesh(&strip_key(SKINNED_TRI, absent)).unwrap_err();
        let d = expect_at(&diagnostics, "L1190", &format!("/{first}"));
        assert!(d.message.contains(&format!("`{absent}`")), "{}", d.message);
    }
    assert!(parse_mesh(SKINNED_TRI).is_ok());
}

#[test]
fn malformed_joints_are_l1190() {
    let short_bind = SKINNED_TRI.replace("      - 1.0\nweights:", "weights:");
    let diagnostics = parse_mesh(&short_bind).unwrap_err();
    let d = expect_at(&diagnostics, "L1190", "/joints/1/inverse_bind");
    assert!(d.message.contains("15 entries"), "{}", d.message);

    let short_rotation = SKINNED_TRI.replace("        - 0.8\n", "");
    let diagnostics = parse_mesh(&short_rotation).unwrap_err();
    expect_at(&diagnostics, "L1190", "/joints/1/rest/r");

    let no_rest = SKINNED_TRI.replace("    parent: 0\n    rest:\n", "    parent: 0\n    restt:\n");
    let diagnostics = parse_mesh(&no_rest).unwrap_err();
    let d = expect_at(&diagnostics, "L1190", "/joints/1");
    assert!(d.message.contains("`rest`"), "{}", d.message);

    let typo = SKINNED_TRI.replacen("    inverse_bind:", "    inverse_bnd:", 1);
    let diagnostics = parse_mesh(&typo).unwrap_err();
    let d = expect_at(&diagnostics, "L1190", "/joints/0/inverse_bnd");
    assert_eq!(d.help.as_deref(), Some("nearest match is `inverse_bind`"));

    let negative = SKINNED_TRI.replace("    parent: 0\n", "    parent: -1\n");
    let diagnostics = parse_mesh(&negative).unwrap_err();
    expect_at(&diagnostics, "L1190", "/joints/1/parent");

    let (head, tail) = SKINNED_TRI
        .split_once("weights:\n")
        .expect("weights present");
    let after = tail
        .split_once("bone_indices:")
        .expect("bone_indices present")
        .1;
    let mapping = format!("{head}weights:\n  a: 1\nbone_indices:{after}");
    let diagnostics = parse_mesh(&mapping).unwrap_err();
    expect_at(&diagnostics, "L1190", "/weights");

    let element = SKINNED_TRI.replace("      - 0.6\n", "      - fast\n");
    let diagnostics = parse_mesh(&element).unwrap_err();
    expect_at(&diagnostics, "L1190", "/joints/1/rest/r/2");
}

/// A case name, the pointer its diagnostic must carry, and the mutation that provokes it.
type SkinCase = (&'static str, &'static str, fn(&mut MeshSkin));

#[test]
fn inconsistent_skin_is_l1191() {
    let cases: [SkinCase; 12] = [
        ("bone index at the joint count", "/bone_indices/4", |s| {
            s.bone_indices[4] = 2;
        }),
        ("a vertex summing to 0.9", "/weights/4", |s| {
            s.weights[4..8].copy_from_slice(&[0.4, 0.5, 0.0, 0.0]);
        }),
        ("a negative weight", "/weights/1", |s| {
            s.weights[0..4].copy_from_slice(&[1.2, -0.2, 0.0, 0.0]);
        }),
        ("weights one short", "/weights", |s| {
            s.weights.pop();
        }),
        ("bone_indices one short", "/bone_indices", |s| {
            s.bone_indices.pop();
        }),
        ("a parent equal to its own index", "/joints/1/parent", |s| {
            s.joints[1].parent = Some(1);
        }),
        ("a duplicate name", "/joints/1/name", |s| {
            s.joints[1].name = "pelvis".to_string();
        }),
        ("an empty name", "/joints/1/name", |s| {
            s.joints[1].name.clear();
        }),
        ("a non-unit rotation", "/joints/1/rest/r", |s| {
            s.joints[1].rest.rotation = [0.0, 0.0, 0.6, 0.9];
        }),
        ("a NaN in inverse_bind", "/joints/1/inverse_bind/5", |s| {
            s.joints[1].inverse_bind[5] = f64::NAN;
        }),
        ("empty joints", "/joints", |s| {
            s.joints.clear();
        }),
        ("more than the joint limit", "/joints", |s| {
            s.joints = chain(MESH_JOINT_LIMIT + 1);
        }),
    ];
    for (what, path, mutate) in cases {
        let mut doc = skinned_tri();
        mutate(skin_mut(&mut doc));
        let diagnostics = validate_mesh(&doc);
        let d = expect_at(&diagnostics, "L1191", path);
        assert!(d.help.is_some(), "{what}: the diagnostic carries a help");
    }
    assert!(validate_mesh(&skinned_tri()).is_empty());
}

/// `n` joints in a chain, each parented to the one before.
fn chain(n: usize) -> Vec<MeshJoint> {
    let joint = skinned_tri().skin.expect("skinned").joints.remove(0);
    (0..n)
        .map(|i| MeshJoint {
            name: format!("j{i}"),
            parent: i.checked_sub(1).map(|p| p as u32),
            ..joint.clone()
        })
        .collect()
}

#[test]
fn a_skin_at_the_joint_limit_is_valid() {
    let mut doc = skinned_tri();
    let skin = skin_mut(&mut doc);
    skin.joints = chain(MESH_JOINT_LIMIT);
    skin.bone_indices[4] = (MESH_JOINT_LIMIT - 1) as u32;
    assert!(validate_mesh(&doc).is_empty());
    let text = emit_mesh(&doc);
    assert_eq!(check_mesh(&text).expect("checks clean"), doc);
}

#[test]
fn rest_model_matrices_compose_parent_first() {
    let mut doc = skinned_tri();
    let skin = skin_mut(&mut doc);
    let half = std::f64::consts::FRAC_1_SQRT_2;
    skin.joints[0].rest.translation = [0.0, 1.0, 0.0];
    skin.joints[0].rest.rotation = [0.0, 0.0, half, half];
    skin.joints[1].rest.translation = [1.0, 0.0, 0.0];
    skin.joints[1].rest.rotation = [0.0, 0.0, 0.0, 1.0];
    let model = skin.rest_model_matrices().unwrap();
    assert_eq!(model.len(), 2);
    let head = [model[1][12], model[1][13], model[1][14]];
    for (got, want) in head.iter().zip([0.0, 2.0, 0.0]) {
        assert!((got - want).abs() < 1e-12, "child head {head:?}");
    }
    assert_eq!([model[0][12], model[0][13], model[0][14]], [0.0, 1.0, 0.0]);
}

#[test]
fn rest_model_matrices_are_none_when_a_parent_comes_later() {
    let mut doc = skinned_tri();
    let skin = skin_mut(&mut doc);
    skin.joints[0].parent = Some(1);
    skin.joints[1].parent = None;
    assert!(skin.rest_model_matrices().is_none());
    assert!(skin.rest_palette().is_none());
}

/// `text` with the `joints:` block's items moved to column 0 (`joints:\n- name: …`).
fn dedent_joints(text: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line == "joints:" {
            inside = true;
            out.push_str(line);
        } else if inside && line.starts_with("  ") {
            out.push_str(&line[2..]);
        } else {
            inside = false;
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

#[test]
fn read_mesh_joints_matches_the_full_parse() {
    let text = emit_mesh(&skinned_tri());
    let expected = check_mesh(&text).unwrap().skin.unwrap().joints;
    assert_eq!(read_mesh_joints(&text).unwrap(), Some(expected.clone()));

    let column_zero = dedent_joints(&text);
    assert!(column_zero.contains("joints:\n- name: pelvis"));
    assert_eq!(read_mesh_joints(&column_zero).unwrap(), Some(expected));
}

#[test]
fn read_mesh_joints_reports_the_full_parses_errors() {
    let text = emit_mesh(&skinned_tri()).replace("name: torso", "name: pelvis");
    let expected = check_mesh(&text).unwrap_err();
    assert!(!expected.is_empty());
    assert_eq!(read_mesh_joints(&text).unwrap_err(), expected);
}

#[test]
fn read_mesh_joints_without_a_skin_is_none() {
    let mut doc = skinned_tri();
    doc.skin = None;
    assert_eq!(read_mesh_joints(&emit_mesh(&doc)).unwrap(), None);
}

#[test]
fn read_mesh_joints_does_not_parse_the_vertex_data() {
    let text = emit_mesh(&skinned_tri()).replace("positions:\n", "positions: not-a-list\n");
    assert!(check_mesh(&text).is_err(), "the full parse rejects it");
    let joints = read_mesh_joints(&text).unwrap().unwrap();
    assert_eq!(joints.len(), 2);
}
