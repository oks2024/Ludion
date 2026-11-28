//! Resolves every `SkinBone` part's spawn pose from a scene *document*, not the live world — one
//! pure pass so `scene.load`/a district switch and `ludion check-scene` share it (the
//! [`crate::scene_check`] pattern).
//!
//! A part carrying [`SkinBone`](crate::components::SkinBone) and no authored
//! [`Transform`](crate::components::Transform) of its own spawns at its skin's world transform times
//! its bone's rest model transform. Four diagnostics (`L1194`–`L1197`) refuse a bad link; see
//! [`SkinBone`](crate::components::SkinBone)'s doc comment for the rule and `Plan/skeletal-animation.md`
//! decisions 5–7 for why.

use ludion_assets::AssetRoot;
use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{DMat4, DQuat, DVec3};
use ludion_core::math::{Trs, euler_xyz_degrees_to_quat, quat_to_euler_xyz_degrees};
use ludion_core::suggest;
use ludion_ecs::{Parent, World};
use ludion_scene::{MeshDocument, MeshJoint, Scene, SceneEntity};

use crate::assembly_body::{AssemblyBody, body_of};
use crate::components::{Mesh, MeshSource, SkinBone, Transform};

/// Resolves and applies every `SkinBone` spawn pose in `scene` onto `world`'s entities — the id
/// lookup plus insert loop every fresh-world instantiate site shares:
/// [`crate::engine::instantiate_named_world`] (`Engine` loads and district switches) and the bare
/// render paths (`crate::render::extract_scene_file`, `crate::sweep`). `instantiate` spawns one
/// entity per scene entity in document order and `ludion_scene::instantiated_ids` maps each document
/// position to its entity (a `presentation: true` entity takes no sim slot) — this must be called on
/// a freshly instantiated world, before any despawn.
///
/// # Errors
/// Every `L1194`–`L1197` `spawn_poses` produces.
pub(crate) fn apply_spawn_poses(
    world: &mut World,
    scene: &Scene,
    project_root: AssetRoot<'_>,
) -> Result<(), Vec<Diagnostic>> {
    let poses = spawn_poses(project_root, scene)?;
    apply_bone_poses(world, &ludion_scene::instantiated_ids(scene), &poses);
    Ok(())
}

/// Localizes and inserts every pose in `poses`; `ids` maps a document position to its spawned
/// entity, and the placement of the group must already be baked (the skin's world pose is read).
pub(crate) fn apply_bone_poses(world: &mut World, ids: &[EntityId], poses: &[BonePose]) {
    // Every world pose is read before any write, so a skin's own pose never sees a half-localized
    // part.
    let mut parts: Vec<_> = poses
        .iter()
        .map(|pose| {
            let part = ids[pose.index];
            let skin = crate::hierarchy::world_pose_transform(world, ids[pose.skin_index])
                .unwrap_or_default();
            (part, spawn_pose(skin, &pose.bone_model))
        })
        .collect();
    // A part under a `Parent` stores its world pose local to that parent's *final* world pose, so
    // ancestors localize first.
    parts.sort_by_key(|(part, _)| crate::hierarchy::ancestor_count(world, *part));
    for (part, mut transform) in parts {
        if let Some(Parent(parent)) = world.get::<Parent>(part).ok().flatten().copied()
            && let Some(parent_pose) = crate::hierarchy::chain_pose(world, parent)
        {
            let world_trs = Trs {
                translation: transform.translation,
                rotation: euler_xyz_degrees_to_quat(transform.rotation),
                scale: transform.scale,
            };
            let local = parent_pose.trs.localize(&world_trs);
            transform = Transform {
                translation: local.translation,
                rotation: quat_to_euler_xyz_degrees(local.rotation),
                scale: local.scale,
            };
        }
        world
            .insert(part, transform)
            .expect("the entity was just instantiated and is alive");
    }
}

/// A `SkinBone` part awaiting its spawn pose: the pose is `skin`'s world transform times `bone`'s
/// rest model transform, resolved in [`apply_spawn_poses`] once the instantiated world can say where
/// the skin sits under its own `Parent`.
pub(crate) struct BonePose {
    /// The part's document position, lining up one-for-one with `scene.entities` (the same mapping
    /// [`crate::engine::instantiate_named_world`]'s `Name` loop uses).
    index: usize,
    /// The skin's document position.
    pub(crate) skin_index: usize,
    /// The bone's rest model transform, column-major.
    bone_model: [f64; 16],
}

/// Every [`BonePose`] a scene's `SkinBone` parts resolve to, for every part that authors no
/// `Transform` of its own.
///
/// Collects every `SkinBone` diagnostic across the whole scene rather than stopping at the first
/// (the [`crate::scene_check::scene_layer_diagnostics`] contract) — `Err` when any part is
/// misconfigured, in which case the returned poses are meaningless and never read.
pub(crate) fn spawn_poses(
    project_root: AssetRoot<'_>,
    scene: &Scene,
) -> Result<Vec<BonePose>, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut poses = Vec::new();
    let mut seen_pairs: BTreeMap<(usize, String), usize> = BTreeMap::new();
    let mut reported_animated: std::collections::BTreeSet<usize> =
        std::collections::BTreeSet::new();
    let mut reported_layered: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let mut reported_mesh_errors: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    let mut mesh_cache: BTreeMap<String, Result<Option<Vec<MeshJoint>>, Vec<Diagnostic>>> =
        BTreeMap::new();

    for (index, entity) in scene.entities.iter().enumerate() {
        let Some((_, value)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "SkinBone")
        else {
            continue;
        };
        let Ok(skin_bone) = serde_json::from_value::<SkinBone>(value.clone()) else {
            // Schema-invalid; the generic scene check already reports it (`L1014`).
            continue;
        };
        let skin_index = skin_bone.skin.index() as usize;
        let pointer = format!("/entities/{index}/components/SkinBone/skin");

        if skin_index >= scene.entities.len() || skin_index == index {
            // Out-of-range/self-referencing; the generic entity-ref check already reports it
            // (`L1017`) on every path except a hand-built `Scene`.
            continue;
        }

        let pair = (skin_index, skin_bone.bone.clone());
        if let Some(&earlier) = seen_pairs.get(&pair) {
            diagnostics.push(duplicate(index, entity, earlier, &scene.entities[earlier]));
            continue;
        }
        seen_pairs.insert(pair, index);

        let skin_entity = &scene.entities[skin_index];
        if skin_entity
            .components
            .iter()
            .any(|(name, _)| name == "Animation")
            && reported_animated.insert(skin_index)
        {
            diagnostics.push(animated_skin(skin_index, skin_entity));
        }
        if skin_entity
            .components
            .iter()
            .any(|(name, _)| name == "AnimationLayers")
            && reported_layered.insert(skin_index)
        {
            diagnostics.push(layered_skin(skin_index, skin_entity));
        }

        let mesh_rel = match skinned_mesh_path(project_root, skin_entity) {
            Some(Ok(path)) => path,
            Some(Err((character, errors))) => {
                // Keyed by the character document, in a namespace a mesh path cannot reach.
                if reported_mesh_errors.insert(format!("character:{character}")) {
                    diagnostics.extend(errors);
                }
                continue;
            }
            None => {
                diagnostics.push(not_skinned(
                    &pointer,
                    skin_index,
                    skin_entity,
                    "carries no `Mesh`, or its `shape` is not `asset` or `character`".to_string(),
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
                skin_index,
                skin_entity,
                format!("mesh `{mesh_rel}` has no `joints:`"),
            ));
            continue;
        };

        let Some(bone_index) = joints.iter().position(|j| j.name == skin_bone.bone) else {
            diagnostics.push(unknown_bone(index, &skin_bone.bone, joints));
            continue;
        };

        if entity
            .components
            .iter()
            .any(|(name, _)| name == "Transform")
        {
            continue; // an authored `Transform` wins
        }

        poses.push(BonePose {
            index,
            skin_index,
            bone_model: ludion_scene::rest_model_matrices(joints)
                .expect("a validated skin is parent-first")[bone_index],
        });
    }

    if diagnostics.is_empty() {
        Ok(poses)
    } else {
        Err(diagnostics)
    }
}

/// Every `L1194`–`L1197` a scene's `SkinBone` parts raise — `spawn_poses`'s `Err`, or empty. The
/// filesystem-reading half of a `SkinBone` check, run by both `scene.load`
/// (via `spawn_poses`) and `ludion check-scene`/a game's `--check-scene`
/// (the [`crate::scene_check::scene_atlas_diagnostics`] precedent).
pub fn scene_skin_bone_diagnostics(project_root: AssetRoot<'_>, scene: &Scene) -> Vec<Diagnostic> {
    spawn_poses(project_root, scene).err().unwrap_or_default()
}

/// The world-space spawn transform for a part whose skin sits at `skin_transform` and whose bone's
/// rest model transform (column-major) is `bone_model`.
///
/// Sim-side arithmetic, so no glam-f32 transcendental or SIMD normalize (the ragdoll cross-arch
/// trap, `ragdoll.rs`): built in `f64` scalar glam throughout, with `skin_transform`'s rotation
/// converted through [`ludion_core::math::euler_xyz_degrees_to_quat`] (pure-Rust libm) and widened, and the composed
/// rotation narrowed back through [`ludion_core::math::quat_to_euler_xyz_degrees`] (which normalizes by hand) only at
/// the very end.
///
/// A non-uniform `skin_transform.scale` under a rotated bone shears, which a TRS `Transform` cannot
/// hold — author a uniform skin scale.
fn spawn_pose(skin_transform: Transform, bone_model: &[f64; 16]) -> Transform {
    let q = euler_xyz_degrees_to_quat(skin_transform.rotation);
    let skin_model = DMat4::from_scale_rotation_translation(
        DVec3::new(
            skin_transform.scale[0] as f64,
            skin_transform.scale[1] as f64,
            skin_transform.scale[2] as f64,
        ),
        DQuat::from_xyzw(q[0] as f64, q[1] as f64, q[2] as f64, q[3] as f64),
        DVec3::new(
            skin_transform.translation[0] as f64,
            skin_transform.translation[1] as f64,
            skin_transform.translation[2] as f64,
        ),
    );
    let world = skin_model * DMat4::from_cols_array(bone_model);
    let (scale, rotation, translation) = world.to_scale_rotation_translation();
    let quat = [
        rotation.x as f32,
        rotation.y as f32,
        rotation.z as f32,
        rotation.w as f32,
    ];
    Transform {
        translation: [
            translation.x as f32,
            translation.y as f32,
            translation.z as f32,
        ],
        rotation: quat_to_euler_xyz_degrees(quat),
        scale: [scale.x as f32, scale.y as f32, scale.z as f32],
    }
}

/// `entity`'s `Mesh.shape: asset` path, or `None` when it carries no `Mesh` or a different shape.
pub(crate) fn skinned_mesh_path(
    project_root: AssetRoot<'_>,
    entity: &SceneEntity,
) -> Option<Result<String, (String, Vec<Diagnostic>)>> {
    let (_, value) = entity.components.iter().find(|(name, _)| name == "Mesh")?;
    let mesh: Mesh = serde_json::from_value(value.clone()).ok()?;
    mesh_document_path(project_root, &mesh)
}

/// `mesh`'s `shape: asset` path, or the mesh document a skinned assembly resolves to; `None` for any
/// other shape, a voxel assembly included. An assembly that cannot be read is `Some(Err)`.
pub(crate) fn mesh_document_path(
    project_root: AssetRoot<'_>,
    mesh: &Mesh,
) -> Option<Result<String, (String, Vec<Diagnostic>)>> {
    match &mesh.shape {
        MeshSource::Asset(path) => Some(Ok(path.clone())),
        MeshSource::Assembly(path) | MeshSource::Character(path) => {
            let resolved = match body_of(project_root, path) {
                Ok(AssemblyBody::Skinned) => crate::character::mesh_path(project_root, path),
                Ok(AssemblyBody::Voxel) => return None,
                Err(errors) => Err(errors),
            };
            Some(resolved.map_err(|errors| (path.clone(), errors)))
        }
        _ => None,
    }
}

/// Reads, decodes and validates the mesh document at `root`/`rel` — [`crate::mesh_asset`]'s
/// `load_with_sources` shape, minus the texture half a bone-pose resolution never needs.
pub(crate) fn load_mesh_document(
    root: AssetRoot<'_>,
    rel: &str,
) -> Result<MeshDocument, Vec<Diagnostic>> {
    crate::mesh_asset::read_document(root, rel).map(|(doc, _)| doc)
}

/// The skin joints of the mesh document at `root`/`rel` (`None` without a skin), without parsing
/// its vertex data — what a bone-pose or joint-name lookup needs.
pub(crate) fn load_mesh_joints(
    root: AssetRoot<'_>,
    rel: &str,
) -> Result<Option<Vec<MeshJoint>>, Vec<Diagnostic>> {
    crate::mesh_asset::with_mesh_source(root, rel, ludion_scene::read_mesh_joints)
        .map(|(joints, _)| joints)
}

/// `entity` (at document position `index`) by its `name:` when it has one, else its bare position —
/// the ragdoll module's `named_entity` habit, at document time rather than world time.
pub(crate) fn named(entity: &SceneEntity, index: usize) -> String {
    match &entity.name {
        Some(name) => format!("`{name}` (entity {index})"),
        None => format!("entity {index}"),
    }
}

/// `L1194` — `skin` names an entity that is not a skinned mesh.
fn not_skinned(
    pointer: &str,
    skin_index: usize,
    skin_entity: &SceneEntity,
    reason: String,
) -> Diagnostic {
    Diagnostic::new(
        code::SKIN_BONE_NOT_SKINNED,
        format!(
            "`SkinBone.skin` names {}, which is not a skinned mesh: it {reason}",
            named(skin_entity, skin_index)
        ),
    )
    .with_path(pointer.to_string())
    .with_help(
        "point `skin` at an entity carrying a skinned `Mesh` (`shape: { asset: \
         \"meshes/rig.mesh.yaml\" }` exported with its armature, e.g. through `ludion import`)",
    )
}

/// `L1195` — `bone` names no joint of its skin's mesh document.
fn unknown_bone(index: usize, bone: &str, joints: &[MeshJoint]) -> Diagnostic {
    let names: Vec<&str> = joints.iter().map(|j| j.name.as_str()).collect();
    let help = match suggest::nearest(bone, names.iter().copied()) {
        Some(candidate) => format!("nearest match is `{candidate}`"),
        None => format!("this skin's joints are: {}", names.join(", ")),
    };
    Diagnostic::new(
        code::SKIN_BONE_UNKNOWN_BONE,
        format!("`SkinBone.bone` `{bone}` is not a joint of its skin's mesh document"),
    )
    .with_path(format!("/entities/{index}/components/SkinBone/bone"))
    .with_help(help)
}

/// `L1196` — two parts name the same `(skin, bone)` pair.
fn duplicate(
    index: usize,
    entity: &SceneEntity,
    earlier_index: usize,
    earlier_entity: &SceneEntity,
) -> Diagnostic {
    Diagnostic::new(
        code::SKIN_BONE_DUPLICATE,
        format!(
            "{} names the same `(skin, bone)` pair as {}",
            named(entity, index),
            named(earlier_entity, earlier_index)
        ),
    )
    .with_path(format!("/entities/{index}/components/SkinBone"))
    .with_help("give this part a different `bone`, or remove one of the two parts")
}

/// `L1197` — a skin also carries an `Animation`, reported once per skin.
fn animated_skin(skin_index: usize, skin_entity: &SceneEntity) -> Diagnostic {
    Diagnostic::new(
        code::SKIN_BONE_ANIMATED_SKIN,
        format!(
            "{} plays an `Animation` but is also named by a `SkinBone` part: a skin has one pose \
             source",
            named(skin_entity, skin_index)
        ),
    )
    .with_path(format!("/entities/{skin_index}/components/Animation"))
    .with_help(
        "remove this skin entity's `Animation` component, or remove every `SkinBone` part that \
         names it",
    )
}

/// `L1672` — a skin also carries an `AnimationLayers`, reported once per skin.
fn layered_skin(skin_index: usize, skin_entity: &SceneEntity) -> Diagnostic {
    Diagnostic::new(
        code::ANIMATION_LAYERS_ON_SKIN_BONE_SKIN,
        format!(
            "{} carries `AnimationLayers` but is also named by a `SkinBone` part: its pose comes \
             from the parts, so layers have nothing to compose over",
            named(skin_entity, skin_index)
        ),
    )
    .with_path(format!("/entities/{skin_index}/components/AnimationLayers"))
    .with_help(
        "remove this skin entity's `AnimationLayers` component, or remove every `SkinBone` part \
         that names it",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn identity_bone(
        name: &str,
        parent: Option<u32>,
        translation: [f64; 3],
    ) -> ludion_scene::MeshJoint {
        ludion_scene::MeshJoint {
            name: name.to_string(),
            parent,
            rest: ludion_scene::JointRest {
                translation,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
            inverse_bind: {
                let mut m = [0.0; 16];
                for i in 0..4 {
                    m[i * 5] = 1.0;
                }
                m
            },
        }
    }

    fn skin() -> ludion_scene::MeshSkin {
        ludion_scene::MeshSkin {
            joints: vec![
                identity_bone("root", None, [0.0, 0.0, 0.0]),
                identity_bone("child", Some(0), [0.0, 2.0, 0.0]),
            ],
            weights: vec![1.0, 0.0, 0.0, 0.0],
            bone_indices: vec![0, 0, 0, 0],
        }
    }

    /// The pure `spawn_pose` arithmetic: a rotated, translated, uniformly scaled skin composed with
    /// a translated child bone, checked against an independently composed expected value.
    #[test]
    fn spawn_pose_composes_skin_and_bone_rest() {
        let skin_transform = Transform {
            translation: [1.0, 0.0, 0.0],
            rotation: [0.0, 90.0, 0.0],
            scale: [2.0, 2.0, 2.0],
        };
        let bone_model = skin().rest_model_matrices().unwrap()[1];
        let pose = spawn_pose(skin_transform, &bone_model);
        // A 90-degree yaw maps local `+Y` translation onto itself (rotation about Y), scaled by 2,
        // then offset by the skin's own translation.
        assert!((pose.translation[0] - 1.0).abs() < 1e-4, "{pose:?}");
        assert!((pose.translation[1] - 4.0).abs() < 1e-4, "{pose:?}");
        assert!((pose.translation[2] - 0.0).abs() < 1e-4, "{pose:?}");
        assert!((pose.rotation[1] - 90.0).abs() < 1e-3, "{pose:?}");
        assert!((pose.scale[0] - 2.0).abs() < 1e-4, "{pose:?}");
    }

    /// Two parts naming the same skin, whose mesh document fails to load, yield exactly one copy
    /// of the load diagnostic — the same once-per-mesh dedupe `reported_animated` gives `L1197`,
    /// applied to `mesh_cache`'s `Err` arm.
    #[test]
    fn a_broken_mesh_document_is_reported_once_per_mesh_not_once_per_part() {
        let scene = Scene {
            entities: vec![
                SceneEntity {
                    presentation: false,
                    name: None,
                    components: vec![(
                        "Mesh".to_string(),
                        serde_json::json!({ "shape": { "asset": "meshes/does-not-exist.mesh.yaml" } }),
                    )],
                    instance: None,
                },
                SceneEntity {
                    presentation: false,
                    name: None,
                    components: vec![(
                        "SkinBone".to_string(),
                        serde_json::json!({ "skin": { "index": 0, "generation": 0 }, "bone": "root" }),
                    )],
                    instance: None,
                },
                SceneEntity {
                    presentation: false,
                    name: None,
                    components: vec![(
                        "SkinBone".to_string(),
                        serde_json::json!({ "skin": { "index": 0, "generation": 0 }, "bone": "child" }),
                    )],
                    instance: None,
                },
            ],
            ..Scene::default()
        };
        let found =
            scene_skin_bone_diagnostics(ludion_assets::AssetRoot::content(Path::new(".")), &scene);
        let matches = found
            .iter()
            .filter(|d| d.code == code::ASSET_FILE_IO)
            .count();
        assert_eq!(matches, 1, "{found:?}");
    }

    /// A skinned mesh whose joints are valid but whose vertex data is not still poses its parts:
    /// pose resolution reads the joints alone.
    #[test]
    fn a_part_poses_from_the_joints_without_parsing_the_vertex_data() {
        let root =
            std::env::temp_dir().join(format!("ludion-skin-bone-joints-{}", std::process::id()));
        std::fs::create_dir_all(root.join("meshes")).unwrap();
        let doc = ludion_scene::MeshDocument {
            positions: vec![0.0; 3],
            skin: Some(skin()),
            ..ludion_scene::MeshDocument::default()
        };
        let text = ludion_scene::emit_mesh(&doc).replace(
            "positions:
",
            "positions: not-a-list
",
        );
        assert!(ludion_scene::check_mesh(&text).is_err());
        std::fs::write(root.join("meshes/skinned.mesh.yaml"), text).unwrap();

        let scene = Scene {
            entities: vec![
                SceneEntity {
                    presentation: false,
                    name: None,
                    components: vec![(
                        "Mesh".to_string(),
                        serde_json::json!({ "shape": { "asset": "meshes/skinned.mesh.yaml" } }),
                    )],
                    instance: None,
                },
                SceneEntity {
                    presentation: false,
                    name: None,
                    components: vec![(
                        "SkinBone".to_string(),
                        serde_json::json!({ "skin": { "index": 0, "generation": 0 }, "bone": "child" }),
                    )],
                    instance: None,
                },
            ],
            ..Scene::default()
        };
        let poses = spawn_poses(ludion_assets::AssetRoot::content(&root), &scene);
        let _ = std::fs::remove_dir_all(&root);
        let poses = poses.expect("joints are valid");
        assert_eq!(poses.len(), 1);
        assert!(
            (poses[0].bone_model[13] - 2.0).abs() < 1e-9,
            "{:?}",
            poses[0].bone_model
        );
    }
}
