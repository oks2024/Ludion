//! A voxel ragdoll folding back into its assembly (Neon Shroud request 0008 W3,
//! `Plan/paired-animations.md` decisions 10–14): the inverse of
//! [`switch_to_ragdoll`](crate::switch_to_ragdoll).
//!
//! [`switch_ragdoll_to_assembly`] captures the parts' poses into a hashed
//! [`PoseSnapshot`](crate::components::PoseSnapshot) on the root, despawns the parts, re-inserts the
//! root's `Mesh { shape: assembly }`, places the root by the rule below, and starts a clip that
//! crossfades **from the snapshot**: the first drawn frame equals the last ragdoll frame.
//! [`ragdoll_landing`] is the pure query for the same placement, so a game can pick its get-up clip
//! (`face_up`) before it switches.
//!
//! # The placement rule
//!
//! Let `q_r` be the first parentless part's world rotation and `c` its pivot in world space.
//!
//! - `face_up`: the part's chest (`q_r · -Z`) points up (`y > 0`).
//! - `root_yaw`: radians about `+Y` that turn the entity's `-Z` onto the head's (`q_r · +Y`) direction
//!   projected on the ground plane; the chest's, when the head points straight up or down.
//! - `feet`: `[c.x, lowest, c.z]`, where `lowest` is the lowest world `y` over every part's eight
//!   box corners. It is not a raycast: it needs no physics, works for retired parts, and the body
//!   cannot hit itself.
//!
//! The root `Transform` is written exactly as [`resume_character`](crate::resume_character) writes
//! it for the same `feet` and `root_yaw`, so reviving a motor figure afterwards does not move it.
//!
//! # What stays game code
//!
//! The switch does not resume the motor, re-insert a `Collider` or `Velocity`, or touch the root's
//! `PoseBlend`. Call [`resume_character`](crate::resume_character) with the returned `feet` and
//! `root_yaw` (after the get-up, if the motor should wait), or insert the components you need.
//!
//! ```no_run
//! use ludion_engine::{ragdoll_landing, switch_ragdoll_to_assembly, AssemblyData};
//! use ludion_ecs::World;
//! use ludion_core::ids::EntityId;
//! use ludion_core::time::GameTime;
//!
//! fn get_up(world: &mut World, root: EntityId, parts: &[EntityId], model: &AssemblyData, now: GameTime) {
//!     let Ok(landing) = ragdoll_landing(world, root, parts, model) else { return };
//!     let clip = if landing.face_up { "get_up_back" } else { "get_up_front" };
//!     let _ = switch_ragdoll_to_assembly(world, root, parts, model, clip, 0.25, now);
//! }
//! ```

use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::f64 as f64s;
use ludion_core::math::glam::{DQuat, DVec3};
use ludion_core::math::{euler_xyz_degrees_to_quat, quat_to_euler_xyz_degrees, sin_cos};
use ludion_core::suggest::nearest;
use ludion_core::time::{FrameIndex, GameTime};
use ludion_ecs::{Parent, World};

use crate::assembly_mesh::AssemblyData;
use crate::components::{Animation, Mesh, MeshSource, PoseSnapshot, SnapshotPart, Transform};
use crate::ragdoll::{mesh_bounds, switch_invalid};

/// Where a folded ragdoll puts its root, and which way it lies: the placement rule in the
/// module docs.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RagdollToAssembly {
    /// The chest points up: the figure lies on its back.
    pub face_up: bool,
    /// Radians about `+Y` that turn the entity's `-Z` along the head direction on the ground plane.
    /// Feed it to `resume_character(world, e, feet, root_yaw, ..)` as is.
    pub root_yaw: f32,
    /// The root's new translation: the root part's pivot over the lowest part corner.
    pub feet: [f32; 3],
}

/// Everything both entry points read from a validated world.
struct Solved {
    landing: RagdollToAssembly,
    scale: f32,
    assembly_rel: String,
    /// The first part's `Mesh`, whose material fields the re-inserted root mesh takes.
    mesh: Mesh,
    /// Each part's world rotation and translation, in authored order.
    poses: Vec<(DQuat, DVec3)>,
}

fn quat64(rotation_deg: [f32; 3]) -> DQuat {
    let q = euler_xyz_degrees_to_quat(rotation_deg);
    normalize(DQuat::from_xyzw(
        f64::from(q[0]),
        f64::from(q[1]),
        f64::from(q[2]),
        f64::from(q[3]),
    ))
}

/// By hand, never glam's `normalize`: its SSE2 and NEON backends differ in the last bit.
fn normalize(q: DQuat) -> DQuat {
    let len = f64s::sqrt(q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w);
    DQuat::from_xyzw(q.x / len, q.y / len, q.z / len, q.w / len)
}

fn conjugate(q: DQuat) -> DQuat {
    DQuat::from_xyzw(-q.x, -q.y, -q.z, q.w)
}

fn vec64(v: [f32; 3]) -> DVec3 {
    DVec3::new(f64::from(v[0]), f64::from(v[1]), f64::from(v[2]))
}

fn solve(
    world: &World,
    entity: EntityId,
    parts: &[EntityId],
    assembly: &AssemblyData,
) -> Result<Solved, Diagnostic> {
    if !world.is_alive(entity) {
        return Err(switch_invalid(
            &format!("entity `{entity}`"),
            "is not alive",
            "fold back an entity that still exists",
        ));
    }
    let named = crate::scene_physics::entity_label(world, entity);
    let Some(transform) = world.get::<Transform>(entity).ok().flatten() else {
        return Err(switch_invalid(
            &named,
            "has no `Transform`",
            "give the entity a `Transform` before folding the ragdoll back into it",
        ));
    };
    if world.has::<Parent>(entity) {
        return Err(switch_invalid(
            &named,
            "has a `Parent`; the fold writes a world-space `Transform`",
            "unparent the entity first (as `resume_character` requires for a motor)",
        ));
    }
    if world.has::<Mesh>(entity) {
        return Err(switch_invalid(
            &named,
            "still has a `Mesh`, so it is not a root switched to a ragdoll",
            "pass the entity `switch_to_ragdoll` took the `Mesh` from",
        ));
    }
    let scale = transform.scale;
    if scale[0] != scale[1] || scale[1] != scale[2] || !(scale[0] > 0.0 && scale[0].is_finite()) {
        return Err(switch_invalid(
            &named,
            format!("its `Transform.scale` {scale:?} is not uniform, positive and finite"),
            "give the entity a uniform, strictly positive, finite scale",
        ));
    }
    let s = scale[0];
    if parts.len() != assembly.parts.len() {
        return Err(switch_invalid(
            &named,
            format!(
                "was given {} part entities for an assembly of {} parts",
                parts.len(),
                assembly.parts.len()
            ),
            "pass the entities `switch_to_ragdoll` returned, in the same order",
        ));
    }
    let mut assembly_rel: Option<&str> = None;
    let mut template: Option<&Mesh> = None;
    let mut poses = Vec::with_capacity(parts.len());
    for (index, (&id, data)) in parts.iter().zip(&assembly.parts).enumerate() {
        let part_label = format!("part {index} (`{}`)", data.name);
        if !world.is_alive(id) {
            return Err(switch_invalid(
                &named,
                format!("{part_label} entity `{id}` is not alive"),
                "pass the entities `switch_to_ragdoll` returned; a despawned part cannot be folded",
            ));
        }
        let Some(part_transform) = world.get::<Transform>(id).ok().flatten() else {
            return Err(switch_invalid(
                &named,
                format!("{part_label} entity `{id}` has no `Transform`"),
                "pass the entities `switch_to_ragdoll` returned",
            ));
        };
        let mesh = world.get::<Mesh>(id).ok().flatten();
        let Some((
            mesh,
            MeshSource::AssemblyPart {
                assembly: rel,
                part,
            },
        )) = mesh.map(|mesh| (mesh, &mesh.shape))
        else {
            return Err(switch_invalid(
                &named,
                format!("{part_label} entity `{id}` has no assembly-part `Mesh`"),
                "pass the entities `switch_to_ragdoll` returned, in the same order",
            ));
        };
        if *part != data.name {
            return Err(switch_invalid(
                &named,
                format!(
                    "part entity {index} draws `{part}` but the assembly's part {index} is `{}`",
                    data.name
                ),
                "pass the parts in the assembly's authored order, as `switch_to_ragdoll` returned them",
            ));
        }
        if assembly_rel.is_some_and(|first| first != rel) {
            return Err(switch_invalid(
                &named,
                format!(
                    "{part_label} draws from `{rel}`, but an earlier part draws another assembly"
                ),
                "pass the parts of one ragdoll",
            ));
        }
        assembly_rel = Some(rel);
        template.get_or_insert(mesh);
        if data.mesh.positions.is_empty() {
            return Err(switch_invalid(
                &named,
                format!("assembly part `{}` has no vertices", data.name),
                "give every part file the assembly places some geometry",
            ));
        }
        poses.push((
            quat64(part_transform.rotation),
            vec64(part_transform.translation),
        ));
    }
    let (Some(assembly_rel), Some(template)) = (assembly_rel, template) else {
        return Err(switch_invalid(
            &named,
            "the assembly has no parts",
            "fold back a ragdoll of an assembly with at least one part",
        ));
    };

    let root = assembly
        .parts
        .iter()
        .position(|part| part.parent.is_none())
        .unwrap_or(0);
    let (q_r, p_r) = poses[root];
    let s64 = f64::from(s);
    let pivot = vec64(assembly.parts[root].pivot) * s64;
    let c = p_r + q_r * pivot;
    let head = q_r * DVec3::Y;
    let chest = q_r * DVec3::NEG_Z;
    let mut forward = DVec3::new(head.x, 0.0, head.z);
    if f64s::sqrt(forward.x * forward.x + forward.z * forward.z) < 1e-6 {
        forward = DVec3::new(chest.x, 0.0, chest.z);
    }
    let root_yaw = f64s::atan2(-forward.x, -forward.z) as f32;

    let mut lowest = f64::INFINITY;
    for ((q, p), data) in poses.iter().zip(&assembly.parts) {
        let (min, max) = mesh_bounds(&data.mesh.positions).expect("checked non-empty above");
        for corner in 0..8u8 {
            let local = DVec3::new(
                f64::from(if corner & 1 == 0 { min.x } else { max.x }),
                f64::from(if corner & 2 == 0 { min.y } else { max.y }),
                f64::from(if corner & 4 == 0 { min.z } else { max.z }),
            ) * s64;
            lowest = lowest.min((*p + *q * local).y);
        }
    }
    Ok(Solved {
        landing: RagdollToAssembly {
            face_up: chest.y > 0.0,
            root_yaw,
            feet: [c.x as f32, lowest as f32, c.z as f32],
        },
        scale: s,
        assembly_rel: assembly_rel.to_owned(),
        mesh: template.clone(),
        poses,
    })
}

/// Where [`switch_ragdoll_to_assembly`] would put the root, and which way the figure lies — a pure
/// query that changes nothing, so the game can pick `get_up_back` or `get_up_front` before it
/// switches. The result equals the switch's return for the same world.
///
/// # Errors
/// `RAGDOLL_SWITCH_INVALID` (`L3036`) for what [`switch_ragdoll_to_assembly`] refuses
/// before it mutates anything, except the clip name.
pub fn ragdoll_landing(
    world: &World,
    entity: EntityId,
    parts: &[EntityId],
    assembly: &AssemblyData,
) -> Result<RagdollToAssembly, Diagnostic> {
    solve(world, entity, parts, assembly).map(|solved| solved.landing)
}

/// Folds a voxel ragdoll back into its root `entity`: captures the parts' poses into a
/// [`PoseSnapshot`], despawns `parts`, re-inserts the root's `Mesh { shape: assembly }`, places the
/// root by the module's rule, starts `clip`, and returns the placement.
///
/// `parts` are the entities [`switch_to_ragdoll`](crate::switch_to_ragdoll) returned, in order; a
/// part retired by the ragdoll budget (only `Transform` and `Mesh` left) folds back like any other.
/// `clip` is a clip or blend of `assembly`, or `""` for the rest pose. With `fade > 0` the clip
/// crossfades over `fade` seconds from the snapshot, and the first drawn frame is the last ragdoll
/// frame; otherwise it cuts. `game_time` is the gameplay time of the call.
///
/// The root's `Collider`, `Velocity`, motor and `PoseBlend` are left to the game. The despawned parts
/// leave the physics world on the next physics step, so call `resume_character` (or insert a
/// `Collider`) from the next tick on; on the same tick their bodies still block the capsule.
///
/// # Errors
/// `RAGDOLL_SWITCH_INVALID` (`L3036`) when the entity is not alive, has no `Transform`, has a
/// `Parent`, still has a `Mesh`, or has a non-uniform scale; when `parts` do not match `assembly`
/// (count, liveness, `AssemblyPart` meshes, order, one assembly path, vertices); or when `clip`
/// is neither `""` nor a clip or blend of `assembly` (with the nearest name as help). Nothing is
/// mutated when it fires.
pub fn switch_ragdoll_to_assembly(
    world: &mut World,
    entity: EntityId,
    parts: &[EntityId],
    assembly: &AssemblyData,
    clip: &str,
    fade: f32,
    game_time: impl Into<GameTime>,
) -> Result<RagdollToAssembly, Diagnostic> {
    let solved = solve(world, entity, parts, assembly)?;
    let named = crate::scene_physics::entity_label(world, entity);
    if !clip.is_empty() && assembly.clip(clip).is_none() && assembly.blend(clip).is_none() {
        let names = assembly
            .clips
            .iter()
            .map(|c| c.name.as_str())
            .chain(assembly.blends.iter().map(|b| b.name.as_str()));
        let help = match nearest(clip, names.clone()) {
            Some(near) => format!("the closest clip is `{near}`"),
            None => format!(
                "the assembly's clips and blends are {}",
                names.collect::<Vec<_>>().join(", ")
            ),
        };
        return Err(switch_invalid(
            &named,
            format!("the assembly has no clip or blend `{clip}`"),
            help,
        ));
    }
    let now = game_time.into();
    let landing = solved.landing;

    // The root's new transform first: the snapshot is relative to the exact rotation it stores.
    let (sin_half, cos_half) = sin_cos(landing.root_yaw / 2.0);
    let rotation = quat_to_euler_xyz_degrees([0.0, sin_half, 0.0, cos_half]);
    let q_entity = quat64(rotation);
    let p_entity = vec64(landing.feet);
    let s64 = f64::from(solved.scale);
    let snapshot_parts: Vec<SnapshotPart> = assembly
        .parts
        .iter()
        .zip(&solved.poses)
        .map(|(data, &(q_i, p_i))| {
            // The parent frame: the entity for a root-level part, else the parent part's world pose.
            let (q_p, p_p, offset_p) = match data.parent {
                Some(parent) => {
                    let (q, p) = solved.poses[parent];
                    (q, p, vec64(assembly.parts[parent].offset))
                }
                None => (q_entity, p_entity, DVec3::ZERO),
            };
            let inverse = conjugate(q_p);
            let local_rotation = normalize(inverse * q_i);
            let local_position = (inverse * (p_i - p_p)) / s64;
            let pivot = vec64(data.pivot);
            let translation =
                local_position - (vec64(data.offset) - offset_p) - pivot + local_rotation * pivot;
            SnapshotPart {
                part: data.name.clone(),
                rotation: [
                    local_rotation.x as f32,
                    local_rotation.y as f32,
                    local_rotation.z as f32,
                    local_rotation.w as f32,
                ],
                translation: [
                    translation.x as f32,
                    translation.y as f32,
                    translation.z as f32,
                ],
            }
        })
        .collect();

    // --- Validated: every step below mutates the world. ---

    for &id in parts {
        world.despawn(id).expect("checked alive above");
    }
    let mut mesh = solved.mesh;
    mesh.shape = MeshSource::Assembly(solved.assembly_rel);
    world.insert(entity, mesh).expect("checked alive above");
    world
        .insert(
            entity,
            Transform {
                translation: landing.feet,
                rotation,
                scale: [solved.scale; 3],
            },
        )
        .expect("checked alive above");
    let started = FrameIndex::new(now.ticks_floor());
    if fade > 0.0 && fade.is_finite() {
        world
            .insert(
                entity,
                PoseSnapshot {
                    started,
                    parts: snapshot_parts,
                },
            )
            .expect("checked alive above");
        world
            .insert(
                entity,
                Animation {
                    clip: clip.to_owned(),
                    started,
                    fade_started: started,
                    fade,
                    ..Animation::default()
                },
            )
            .expect("checked alive above");
    } else {
        world
            .remove::<PoseSnapshot>(entity)
            .expect("checked alive above");
        world
            .insert(
                entity,
                Animation {
                    clip: clip.to_owned(),
                    started,
                    ..Animation::default()
                },
            )
            .expect("checked alive above");
    }
    crate::interpolation::cut_presentation(world, entity);
    Ok(landing)
}
