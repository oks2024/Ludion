//! Validates every `ParentJoint` from a scene *document*: the entity's `Parent` must be a skinned
//! mesh, and `joint` must name one of its joints. One pure pass shared by `scene.load`/a district
//! switch, `ludion check-scene` and prefab spawn (the [`crate::skin_bone`] pattern).
//!
//! Two diagnostics (`L1318`, `L1319`); see [`ParentJoint`](crate::components::ParentJoint)'s doc
//! comment for the rule.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use ludion_ecs::Parent;
use ludion_scene::{MeshJoint, Scene, SceneEntity};

use crate::components::ParentJoint;
use crate::skin_bone::{load_mesh_joints, named, skinned_mesh_path};

/// Every `L1318`/`L1319` a scene's `ParentJoint` entities raise, collected across the whole scene
/// (the [`crate::skin_bone::scene_skin_bone_diagnostics`] contract). Mesh load errors are reported
/// once per mesh.
pub fn scene_parent_joint_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut reported_mesh_errors: BTreeSet<String> = BTreeSet::new();
    let mut mesh_cache: BTreeMap<String, Result<Option<Vec<MeshJoint>>, Vec<Diagnostic>>> =
        BTreeMap::new();

    for (index, entity) in scene.entities.iter().enumerate() {
        let Some((_, value)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "ParentJoint")
        else {
            continue;
        };
        let Ok(parent_joint) = serde_json::from_value::<ParentJoint>(value.clone()) else {
            // Schema-invalid; the generic scene check already reports it (`L1014`).
            continue;
        };
        let pointer = format!("/entities/{index}/components/ParentJoint");

        let Some((_, parent_value)) = entity.components.iter().find(|(name, _)| name == "Parent")
        else {
            diagnostics.push(not_skinned(
                &pointer,
                index,
                entity,
                "has no `Parent`".to_string(),
            ));
            continue;
        };
        let Ok(Parent(parent)) = serde_json::from_value::<Parent>(parent_value.clone()) else {
            continue; // schema-invalid (`L1014`)
        };
        let parent_index = parent.index() as usize;
        if parent_index >= scene.entities.len() || parent_index == index {
            // Out-of-range/self-referencing; the generic entity-ref check reports it (`L1017`).
            continue;
        }
        let parent_entity = &scene.entities[parent_index];

        let mesh_rel = match skinned_mesh_path(project_root, parent_entity) {
            Some(Ok(path)) => path,
            Some(Err((character, errors))) => {
                if reported_mesh_errors.insert(format!("character:{character}")) {
                    diagnostics.extend(errors);
                }
                continue;
            }
            None => {
                diagnostics.push(not_skinned(
                    &pointer,
                    index,
                    entity,
                    format!(
                        "its `Parent` {} carries no `Mesh`, or its `shape` is not `asset` or \
                         `character`",
                        named(parent_entity, parent_index)
                    ),
                ));
                continue;
            }
        };

        let joints = mesh_cache
            .entry(mesh_rel.clone())
            .or_insert_with(|| load_mesh_joints(project_root, &mesh_rel));
        let joints = match joints {
            Ok(joints) => joints,
            Err(errors) => {
                if reported_mesh_errors.insert(mesh_rel.clone()) {
                    diagnostics.extend(errors.clone());
                }
                continue;
            }
        };
        let Some(joints) = joints else {
            diagnostics.push(not_skinned(
                &pointer,
                index,
                entity,
                format!(
                    "its `Parent` {} uses mesh `{mesh_rel}`, which has no `joints:`",
                    named(parent_entity, parent_index)
                ),
            ));
            continue;
        };

        if !joints.iter().any(|j| j.name == parent_joint.joint) {
            diagnostics.push(unknown_joint(index, &parent_joint.joint, joints));
        }
    }
    diagnostics
}

/// `L1318` — the entity's `Parent` is not a skinned mesh, or it has none.
fn not_skinned(pointer: &str, index: usize, entity: &SceneEntity, reason: String) -> Diagnostic {
    Diagnostic::new(
        code::PARENT_JOINT_NOT_SKINNED,
        format!(
            "{} carries a `ParentJoint` but {reason}",
            named(entity, index)
        ),
    )
    .with_path(pointer.to_string())
    .with_help(
        "point `Parent` at an entity carrying a skinned `Mesh` (`shape: { asset: \
         \"meshes/rig.mesh.yaml\" }` exported with its armature, or a `character`), or remove \
         `ParentJoint`",
    )
}

/// `L1319` — `joint` names no joint of the parent's mesh document.
fn unknown_joint(index: usize, joint: &str, joints: &[MeshJoint]) -> Diagnostic {
    let names: Vec<&str> = joints.iter().map(|j| j.name.as_str()).collect();
    let help = match suggest::nearest(joint, names.iter().copied()) {
        Some(candidate) => format!("nearest match is `{candidate}`"),
        None => format!("this skin's joints are: {}", names.join(", ")),
    };
    Diagnostic::new(
        code::PARENT_JOINT_UNKNOWN_JOINT,
        format!("`ParentJoint.joint` `{joint}` is not a joint of its parent's mesh document"),
    )
    .with_path(format!("/entities/{index}/components/ParentJoint/joint"))
    .with_help(help)
}
