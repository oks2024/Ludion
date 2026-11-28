//! Validates every `ParentJoint` from a scene *document*: the entity's `Parent` must be a skinned
//! mesh or an assembly, and `joint` must name one of its joints or parts. One pure pass shared by
//! `scene.load`/a district switch, `ludion check-scene` and prefab spawn (the [`crate::skin_bone`]
//! pattern).
//!
//! Two diagnostics (`L1318`, `L1319`); see [`ParentJoint`](crate::components::ParentJoint)'s doc
//! comment for the rule.

use ludion_assets::AssetRoot;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::Trs;
use ludion_core::math::glam::{DMat4, Mat4, Vec3};
use ludion_core::suggest;
use ludion_core::time::{GameClock, GameTime, SimTime};
use ludion_ecs::{Parent, Phase, World};
use ludion_renderer::SkinJoint;
use ludion_scene::{MeshJoint, Scene, SceneEntity};

use crate::assembly_body::{AssemblyBody, body_of};
use crate::assembly_mesh::AssemblyData;
use crate::character::CharacterData;
use crate::components::{Mesh, MeshSource, ParentJoint, SkinBone};
use crate::hierarchy::{WorldPose, world_affine};
use crate::skin_bone::{load_mesh_joints, named, skinned_mesh_path};

/// Every `L1318`/`L1319` a scene's `ParentJoint` entities raise, collected across the whole scene
/// (the [`crate::skin_bone::scene_skin_bone_diagnostics`] contract). Mesh load errors are reported
/// once per mesh.
pub fn scene_parent_joint_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut reported_mesh_errors: BTreeSet<String> = BTreeSet::new();
    let mut mesh_cache: BTreeMap<String, Result<Option<Vec<MeshJoint>>, Vec<Diagnostic>>> =
        BTreeMap::new();
    let mut assembly_cache: BTreeMap<String, Result<Vec<String>, Vec<Diagnostic>>> =
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

        if let Some(rel) = assembly_path(project_root, parent_entity) {
            let parts = assembly_cache.entry(rel.clone()).or_insert_with(|| {
                crate::assembly_mesh::load_documents(AssetRoot::content(project_root), &rel).map(
                    |(assembly, _, _, _)| {
                        assembly
                            .parts
                            .into_iter()
                            .map(|p| p.name)
                            .collect::<Vec<_>>()
                    },
                )
            });
            match parts {
                Ok(parts) => {
                    if !parts.contains(&parent_joint.joint) {
                        diagnostics.push(unknown_part(index, &parent_joint.joint, parts));
                    }
                }
                Err(errors) => {
                    if reported_mesh_errors.insert(format!("assembly:{rel}")) {
                        diagnostics.extend(errors.clone());
                    }
                }
            }
            continue;
        }

        let mesh_rel = match skinned_mesh_path(
            ludion_assets::AssetRoot::content(project_root),
            parent_entity,
        ) {
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
                        "its `Parent` {} carries no `Mesh`, or its `shape` is not `asset`, \
                         `character` or `assembly`",
                        named(parent_entity, parent_index)
                    ),
                ));
                continue;
            }
        };

        let joints = mesh_cache.entry(mesh_rel.clone()).or_insert_with(|| {
            load_mesh_joints(ludion_assets::AssetRoot::content(project_root), &mesh_rel)
        });
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

/// The assembly path of `entity`'s `Mesh.shape` unless its body is skinned (that one has joints, so
/// it resolves as a skinned mesh); `None` for no `Mesh` or any other shape. A document that cannot
/// be read is returned too, so the part lookup reports it.
fn assembly_path(project_root: &Path, entity: &SceneEntity) -> Option<String> {
    let (_, value) = entity.components.iter().find(|(name, _)| name == "Mesh")?;
    let mesh: Mesh = serde_json::from_value(value.clone()).ok()?;
    match mesh.shape {
        MeshSource::Assembly(path) | MeshSource::Character(path) => {
            let body = body_of(AssetRoot::content(project_root), &path);
            (body != Ok(AssemblyBody::Skinned)).then_some(path)
        }
        _ => None,
    }
}

/// A skin's joint rest model transforms by joint name.
type JointRests = BTreeMap<String, Trs>;

/// The rest model transform of every joint of the skinned meshes a world's `ParentJoint` entities
/// name, so the hierarchy folds (which hold only a `&World`) can compose a joint level.
///
/// Engine configuration derived from committed content, the [`crate::animation_events`]
/// `AnimationLibrary` mold: never hashed or snapshotted, installed fresh at every world (re)build,
/// not invalidated by `asset.reload`. The cache fills lazily behind a lock, so a read is a pure
/// function of the world and the committed files: a `ParentJoint` inserted at runtime is seen the
/// same as an authored one.
pub(crate) struct SkinRests {
    root: PathBuf,
    /// Keyed `asset:<path>` / `assembly:<path>`; `None` is a mesh with no skin or one that failed
    /// to load, cached as such.
    meshes: Mutex<BTreeMap<String, Option<Arc<JointRests>>>>,
    /// The same keys: the decoded joints and the character's clips, for [`posed_joint_world`].
    /// Separate from `meshes`, so a broken clip can never change a sim-side rest read.
    posed: Mutex<BTreeMap<String, Option<Arc<SkinPoseData>>>>,
    /// Meshed assemblies keyed `assembly:<path>`, for [`posed_joint_world`]; separate from `meshes`
    /// for the same reason as `posed`.
    assemblies: Mutex<BTreeMap<String, Option<Arc<AssemblyData>>>>,
    /// Each assembly document's body by path; `None` for one that cannot be read.
    bodies: Mutex<BTreeMap<String, Option<AssemblyBody>>>,
}

/// What the presentation read of a posed joint needs beyond the rest table: the skin's decoded
/// joints (what the asset cache decodes) and its character, when it is one.
struct SkinPoseData {
    joints: Vec<SkinJoint>,
    character: Option<Arc<CharacterData>>,
}

impl SkinRests {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            meshes: Mutex::new(BTreeMap::new()),
            posed: Mutex::new(BTreeMap::new()),
            assemblies: Mutex::new(BTreeMap::new()),
            bodies: Mutex::new(BTreeMap::new()),
        }
    }

    fn body(&self, rel: &str) -> Option<AssemblyBody> {
        let mut bodies = self.bodies.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = bodies.get(rel) {
            return *found;
        }
        let body = body_of(AssetRoot::content(&self.root), rel).ok();
        bodies.insert(rel.to_string(), body);
        body
    }

    fn assembly_data(&self, rel: &str) -> Option<Arc<AssemblyData>> {
        let mut assemblies = self
            .assemblies
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assemblies
            .entry(format!("assembly:{rel}"))
            .or_insert_with(|| {
                crate::assembly_mesh::load(AssetRoot::content(&self.root), rel)
                    .ok()
                    .map(Arc::new)
            })
            .clone()
    }

    fn pose_data(&self, shape: &MeshSource) -> Option<Arc<SkinPoseData>> {
        let key = match shape {
            MeshSource::Asset(path) => format!("asset:{path}"),
            MeshSource::Assembly(path) | MeshSource::Character(path)
                if self.body(path) == Some(AssemblyBody::Skinned) =>
            {
                format!("assembly:{path}")
            }
            _ => return None,
        };
        let mut posed = self.posed.lock().unwrap_or_else(PoisonError::into_inner);
        posed
            .entry(key)
            .or_insert_with(|| self.load_pose_data(shape))
            .clone()
    }

    fn load_pose_data(&self, shape: &MeshSource) -> Option<Arc<SkinPoseData>> {
        let (path, character) = match shape {
            MeshSource::Asset(path) => (path.clone(), None),
            // `pose_data` asks for a skinned assembly only.
            MeshSource::Assembly(rel) | MeshSource::Character(rel) => {
                let data = crate::character::load(AssetRoot::content(&self.root), rel).ok()?;
                (data.mesh.clone(), Some(Arc::new(data)))
            }
            _ => return None,
        };
        let skin = crate::skin_bone::load_mesh_document(AssetRoot::content(&self.root), &path)
            .ok()?
            .skin?;
        Some(Arc::new(SkinPoseData {
            joints: crate::mesh_asset::decode_skin_joints(&skin),
            character,
        }))
    }

    /// `joint`'s rest model transform in the skin `shape` names; `None` for any miss. The rest shear
    /// is dropped (a `Trs` holds none).
    fn rest(&self, shape: &MeshSource, joint: &str) -> Option<Trs> {
        let key = match shape {
            MeshSource::Asset(path) => format!("asset:{path}"),
            MeshSource::Assembly(path) | MeshSource::Character(path) => {
                format!("assembly:{path}")
            }
            _ => return None,
        };
        let skin = {
            let mut meshes = self.meshes.lock().unwrap_or_else(PoisonError::into_inner);
            meshes
                .entry(key)
                .or_insert_with(|| self.load(shape))
                .clone()
        };
        skin?.get(joint).copied()
    }

    fn load(&self, shape: &MeshSource) -> Option<Arc<JointRests>> {
        let path = match shape {
            MeshSource::Asset(path) => path.clone(),
            MeshSource::Assembly(rel) | MeshSource::Character(rel) => match self.body(rel)? {
                AssemblyBody::Voxel => return self.load_assembly(rel),
                AssemblyBody::Skinned => {
                    crate::character::mesh_path(AssetRoot::content(&self.root), rel).ok()?
                }
            },
            _ => return None,
        };
        let joints = load_mesh_joints(AssetRoot::content(&self.root), &path).ok()??;
        let models = ludion_scene::rest_model_matrices(&joints)?;
        let mut rests = BTreeMap::new();
        for (joint, model) in joints.iter().zip(models) {
            // Scalar f64 glam, narrowed only at the end: the cross-arch-safe recipe of
            // `skin_bone::spawn_pose`.
            let (scale, rotation, translation) =
                DMat4::from_cols_array(&model).to_scale_rotation_translation();
            rests.entry(joint.name.clone()).or_insert(Trs {
                translation: [
                    translation.x as f32,
                    translation.y as f32,
                    translation.z as f32,
                ],
                rotation: [
                    rotation.x as f32,
                    rotation.y as f32,
                    rotation.z as f32,
                    rotation.w as f32,
                ],
                scale: [scale.x as f32, scale.y as f32, scale.z as f32],
            });
        }
        Some(Arc::new(rests))
    }

    /// An assembly's part sockets at rest: the committed documents only, never meshed or posed, so a
    /// broken clip file cannot change a sim read.
    fn load_assembly(&self, rel: &str) -> Option<Arc<JointRests>> {
        let (assembly, parts, _, _) =
            crate::assembly_mesh::load_documents(AssetRoot::content(&self.root), rel).ok()?;
        let rests = crate::assembly_mesh::part_socket_rests(&assembly, &parts)
            .into_iter()
            .map(|(name, translation)| {
                (
                    name,
                    Trs {
                        translation,
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0, 1.0, 1.0],
                    },
                )
            })
            .collect();
        Some(Arc::new(rests))
    }
}

/// Installs an empty [`SkinRests`] over `root` into `world`; re-installing replaces it.
pub(crate) fn install(world: &mut World, root: &Path) {
    world.insert_resource(SkinRests::new(root.to_path_buf()));
}

/// The joint rest level `node`'s `ParentJoint` adds between its parent and its own `Transform`, or
/// `None` when `node` has no `ParentJoint` or any link misses (no `Parent`, a parent without a
/// skinned `Mesh`, an unknown joint, a mesh that fails to load, no [`SkinRests`]): a miss composes
/// as a plain `Parent`, silently. Records no access: every read is unrecorded, so a system needs no
/// `ParentJoint`, `Parent` or `Mesh` declaration for the query-drift gate.
pub(crate) fn joint_rest(world: &World, node: EntityId) -> Option<Trs> {
    if !world.has::<ParentJoint>(node) {
        return None;
    }
    let joint = world.get_unrecorded::<ParentJoint>(node).ok().flatten()?;
    let Parent(parent) = world
        .get_unrecorded::<Parent>(node)
        .ok()
        .flatten()
        .copied()?;
    if !world.is_alive(parent) {
        return None;
    }
    let mesh = world.get_unrecorded::<Mesh>(parent).ok().flatten()?;
    world
        .resource::<SkinRests>()?
        .rest(&mesh.shape, &joint.joint)
}

/// `joint`'s world pose on skin `skin` at `now`, as extract draws a `ParentJoint` child's joint
/// level this frame: the skin's world transform times the joint's model transform in the pose the
/// skin's palette shows (its `Animation` clip and crossfade, `hold` included, and any `SkinBone`
/// followers). A skin with no `Animation` and no followers returns the rest pose. Compose the
/// result with a local transform to place a presentation entity on a hand or a socket.
///
/// `skin` may instead be an `assembly` entity and `joint` a part name: the pose is the part's pivot
/// frame in the part's final drawn placement (clips, blends, crossfade, layers and IK).
///
/// `Ok(None)` when `skin` is dead, has no `Transform`, is not a skinned mesh or an assembly, or
/// names no such joint or part. `now` is sim time; the world's `GameClock` maps it as extract does.
/// Only the presentation and host phases may read it: a sim system is refused with `L0028`, because
/// the answer comes from render-side arithmetic that is not part of the hashed state.
///
/// # Errors
/// `L0028` when called from a sim system.
///
/// # Examples
///
/// ```
/// use ludion_core::time::{SimTime, TickRate};
/// use ludion_ecs::World;
/// use ludion_engine::posed_joint_world;
///
/// let mut world = World::new();
/// let not_a_skin = world.spawn();
/// let now = SimTime::from_ticks(0, TickRate::DEFAULT);
/// assert!(posed_joint_world(&world, not_a_skin, "hand", now).unwrap().is_none());
/// ```
#[allow(clippy::disallowed_methods)] // render-side: GPU matrix, never reaches a hash
pub fn posed_joint_world(
    world: &World,
    skin: EntityId,
    joint: &str,
    now: SimTime,
) -> Result<Option<WorldPose>, Diagnostic> {
    if world.phase() == Phase::Sim {
        return Err(Diagnostic::new(
            code::PRESENTATION_BOUNDARY,
            "`posed_joint_world` is a presentation read and cannot be called from a sim system",
        )
        .with_help("call it from a presentation system, or from the host after a step"));
    }
    if !world.is_alive(skin) {
        return Ok(None);
    }
    let Some(mesh) = world.get::<Mesh>(skin).ok().flatten() else {
        return Ok(None);
    };
    let Some(rests) = world.resource::<SkinRests>() else {
        return Ok(None);
    };
    if let MeshSource::Assembly(rel) | MeshSource::Character(rel) = &mesh.shape
        && rests.body(rel) == Some(AssemblyBody::Voxel)
    {
        return Ok(posed_assembly_part(world, rests, skin, rel, joint, now));
    }
    let Some(data) = rests.pose_data(&mesh.shape) else {
        return Ok(None);
    };
    let Some(skin_affine) = world_affine(world, skin) else {
        return Ok(None);
    };
    if !data.joints.iter().any(|j| j.name == joint) {
        return Ok(None);
    }
    let now = world
        .resource::<GameClock>()
        .map_or_else(|| GameTime::from(now), |clock| clock.game_instant(now));
    let skin_model = affine_mat4(&skin_affine);

    let mut parts: Vec<(&str, Mat4)> = Vec::new();
    for id in world.entities().chain(world.presentation_entities()) {
        if !world.has::<SkinBone>(id) {
            continue;
        }
        let Some(part) = world.get::<SkinBone>(id).ok().flatten() else {
            continue;
        };
        if part.skin != skin || parts.iter().any(|(name, _)| *name == part.bone) {
            continue;
        }
        if let Some(affine) = world_affine(world, id) {
            parts.push((part.bone.as_str(), affine_mat4(&affine)));
        }
    }

    let posed = crate::extract::posed_joint_model(
        world,
        skin,
        &data.joints,
        data.character.as_deref(),
        skin_model,
        Some(parts.as_slice()),
        now,
        joint,
    );
    let affine = match posed {
        Some(model) => skin_affine.mul_affine(&crate::extract::mat4_affine(model)),
        None => {
            let Some(rest) = rests.rest(&mesh.shape, joint) else {
                return Ok(None);
            };
            skin_affine.mul_affine(&rest.to_affine())
        }
    };
    Ok(Some(pose_of(&affine)))
}

/// The world pose of `part`'s pivot frame on assembly entity `entity`: its world transform times the
/// part's final drawn placement (clip or blend, crossfade, layers, IK) times its pivot, the level a
/// socket child draws at. `None` for any miss.
fn posed_assembly_part(
    world: &World,
    rests: &SkinRests,
    entity: EntityId,
    rel: &str,
    part: &str,
    now: SimTime,
) -> Option<WorldPose> {
    let data = rests.assembly_data(rel)?;
    let index = data.parts.iter().position(|p| p.name == part)?;
    let entity_affine = world_affine(world, entity)?;
    let now = world
        .resource::<GameClock>()
        .map_or_else(|| GameTime::from(now), |clock| clock.game_instant(now));
    let placed = crate::extract::assembly_part_models(
        world,
        entity,
        &data,
        affine_mat4(&entity_affine),
        now,
    );
    let level = placed[index].0 * Mat4::from_translation(Vec3::from(data.parts[index].pivot));
    let affine = entity_affine.mul_affine(&crate::extract::mat4_affine(level));
    Some(pose_of(&affine))
}

/// An affine as a [`WorldPose`], flagging shear.
#[allow(clippy::disallowed_methods)] // render-side: GPU matrix, never reaches a hash
fn pose_of(affine: &ludion_core::math::Affine3) -> WorldPose {
    let cols = affine.cols.map(Vec3::from);
    let perpendicular = |u: Vec3, v: Vec3| u.dot(v).abs() <= 1e-5 * u.length() * v.length();
    WorldPose {
        trs: affine.to_trs_lossy(),
        sheared: !(perpendicular(cols[0], cols[1])
            && perpendicular(cols[1], cols[2])
            && perpendicular(cols[2], cols[0])),
    }
}

/// An affine as a matrix: [`crate::extract::mat4_affine`]'s inverse.
fn affine_mat4(affine: &ludion_core::math::Affine3) -> Mat4 {
    let [x, y, z, t] = affine.cols.map(Vec3::from);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), t.extend(1.0))
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
         \"meshes/rig.mesh.yaml\" }` exported with its armature, or a `character`) or an \
         `assembly`, or remove `ParentJoint`",
    )
}

/// `L1319` — `joint` names no part of the parent's assembly.
fn unknown_part(index: usize, part: &str, parts: &[String]) -> Diagnostic {
    let help = match suggest::nearest(part, parts.iter().map(String::as_str)) {
        Some(candidate) => format!("nearest match is `{candidate}`"),
        None => format!("this assembly's parts are: {}", parts.join(", ")),
    };
    Diagnostic::new(
        code::PARENT_JOINT_UNKNOWN_JOINT,
        format!("`ParentJoint.joint` `{part}` is not a part of its parent's assembly"),
    )
    .with_path(format!("/entities/{index}/components/ParentJoint/joint"))
    .with_help(help)
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::components::Transform;
    use crate::hierarchy::world_transform;
    use ludion_scene::{JointRest, MeshDocument, MeshSkin};

    /// A temp project root holding `meshes/rig.mesh.yaml`: `root` at the origin, `child` at
    /// `(0, 2, 0)` rotated 45 degrees about `+Z`.
    pub(crate) fn rig_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-skin-rests-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        let mut identity = [0.0; 16];
        for i in 0..4 {
            identity[i * 5] = 1.0;
        }
        let half = 22.5f64.to_radians();
        let joint = |name: &str, parent, translation, rotation| MeshJoint {
            name: name.to_string(),
            parent,
            rest: JointRest {
                translation,
                rotation,
                scale: [1.0, 1.0, 1.0],
            },
            inverse_bind: identity,
        };
        let doc = MeshDocument {
            positions: vec![0.0; 9],
            indices: vec![0, 1, 2],
            skin: Some(MeshSkin {
                joints: vec![
                    joint("root", None, [0.0; 3], [0.0, 0.0, 0.0, 1.0]),
                    joint(
                        "child",
                        Some(0),
                        [0.0, 2.0, 0.0],
                        [
                            0.0,
                            0.0,
                            ludion_core::math::f64::sin(half),
                            ludion_core::math::f64::cos(half),
                        ],
                    ),
                ],
                weights: vec![1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                bone_indices: vec![0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0],
            }),
            ..MeshDocument::default()
        };
        std::fs::write(
            dir.join("meshes/rig.mesh.yaml"),
            ludion_scene::emit_mesh(&doc),
        )
        .unwrap();
        dir
    }

    /// A skin entity and a `ParentJoint` child under it, `joint` naming the child's joint.
    pub(crate) fn skin_and_child(world: &mut World, joint: &str) -> (EntityId, EntityId) {
        let skin = world.spawn();
        world
            .insert(
                skin,
                Transform {
                    translation: [1.0, 0.0, 0.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        let mesh: Mesh = serde_json::from_value(
            serde_json::json!({ "shape": { "asset": "meshes/rig.mesh.yaml" } }),
        )
        .unwrap();
        world.insert(skin, mesh).unwrap();
        let child = world.spawn();
        world
            .insert(
                child,
                Transform {
                    translation: [0.0, 0.5, 0.0],
                    rotation: [0.0, 30.0, 0.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        world.insert(child, Parent(skin)).unwrap();
        world
            .insert(
                child,
                ParentJoint {
                    joint: joint.to_string(),
                },
            )
            .unwrap();
        (skin, child)
    }

    fn plain_parent_pose() -> crate::hierarchy::WorldPose {
        let mut plain = World::new();
        let (_, child) = skin_and_child(&mut plain, "child");
        plain.remove::<ParentJoint>(child).unwrap();
        world_transform(&plain, child).unwrap()
    }

    /// A temp project holding a two-part assembly: `arm` (1×1×1) placed at `(2, 1, 0)` with its
    /// pivot at `(0, 1, 0)` under `body` (2×1×1), in a 4×2×1 assembly. Returns the root.
    fn assembly_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludion-assembly-socket-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("models/parts")).unwrap();
        let part = |size: [u32; 3], row: &str| {
            format!(
                "format: 1\nkind: part\nsize:\n  - {}\n  - {}\n  - {}\nslices:\n  -\n    - \"{row}\"\n",
                size[0], size[1], size[2]
            )
        };
        std::fs::write(
            dir.join("models/parts/body.part.yaml"),
            part([2, 1, 1], "AA"),
        )
        .unwrap();
        std::fs::write(dir.join("models/parts/arm.part.yaml"), part([1, 1, 1], "A")).unwrap();
        std::fs::write(
            dir.join("models/fixture.assembly.yaml"),
            "format: 1\nkind: assembly\nname: fixture\nsize:\n  - 4\n  - 2\n  - 1\npalette:\n  A:\n    \
             color:\n      - 1.0\n      - 0.0\n      - 0.0\n      - 1.0\nparts:\n  - name: body\n    \
             file: parts/body.part.yaml\n    place:\n      - 0\n      - 0\n      - 0\n  - name: arm\n    \
             file: parts/arm.part.yaml\n    parent: body\n    place:\n      - 2\n      - 1\n      - 0\n    \
             pivot:\n      - 0.0\n      - 1.0\n      - 0.0\n",
        )
        .unwrap();
        dir
    }

    fn assembly_and_child(world: &mut World, part: &str) -> EntityId {
        let assembly = world.spawn();
        world.insert(assembly, Transform::default()).unwrap();
        let mesh: Mesh = serde_json::from_value(
            serde_json::json!({ "shape": { "assembly": "models/fixture.assembly.yaml" } }),
        )
        .unwrap();
        world.insert(assembly, mesh).unwrap();
        let child = world.spawn();
        world.insert(child, Transform::default()).unwrap();
        world.insert(child, Parent(assembly)).unwrap();
        world
            .insert(
                child,
                ParentJoint {
                    joint: part.to_string(),
                },
            )
            .unwrap();
        child
    }

    #[test]
    fn an_assembly_part_rests_at_offset_plus_pivot_and_a_miss_composes_as_a_plain_parent() {
        let dir = assembly_dir("rest");
        let mut hit = World::new();
        let child = assembly_and_child(&mut hit, "arm");
        install(&mut hit, &dir);
        // offset = place + size/2 - assembly/2 on x and z: (2 + 0.5 - 2, 1, 0 + 0.5 - 0.5);
        // pivot in the mesh frame = (0 - 0.5, 1, 0 - 0.5).
        assert_eq!(
            world_transform(&hit, child).unwrap().trs.translation,
            [0.0, 2.0, -0.5]
        );

        let mut miss = World::new();
        let child_m = assembly_and_child(&mut miss, "ram");
        install(&mut miss, &dir);
        assert_eq!(
            world_transform(&miss, child_m).unwrap().trs.translation,
            [0.0; 3],
            "an unknown part composes as a plain Parent"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hit_moves_the_child_and_a_miss_composes_as_a_plain_parent() {
        let dir = rig_dir("miss");
        let mut hit = World::new();
        let (_, child) = skin_and_child(&mut hit, "child");
        install(&mut hit, &dir);
        let posed = world_transform(&hit, child).unwrap();

        let mut unknown = World::new();
        let (_, child_u) = skin_and_child(&mut unknown, "chlid");
        install(&mut unknown, &dir);
        let missed = world_transform(&unknown, child_u).unwrap();

        let mut bare = World::new();
        let (_, child_b) = skin_and_child(&mut bare, "child");
        let no_resource = world_transform(&bare, child_b).unwrap();

        let plain = plain_parent_pose();
        assert_ne!(posed.trs, plain.trs, "a resolved joint must move the child");
        assert_eq!(missed, plain, "an unknown joint composes as a plain Parent");
        assert_eq!(
            no_resource, plain,
            "no SkinRests composes as a plain Parent"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
