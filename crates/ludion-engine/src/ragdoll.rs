//! [`switch_to_ragdoll`]: promotes a posed `kind: assembly` entity into one `rigid` body entity per
//! part, joined by [`Joint`]s — "a ragdoll is the assembly plus a joint list"
//! (`Plan/physics.md` W3's ragdoll-authoring plan, decisions 1-6).
//!
//! Game code decides *when* to call this — a death blow, a stagger, a rope grab — and hands it the
//! meshed [`AssemblyData`] its own asset cache already holds (`ludion_engine::load_assembly`). The
//! switch is otherwise pure ECS surgery: it reads the entity's current pose (rest, or mid-clip at the
//! caller's `game_time`, through the very same `part_placements_with` extraction draws with), spawns
//! one entity per part at that pose's world transform, wires a [`Joint`] onto each non-root part
//! (fixed to its parent by default, or whatever `assembly`'s `joints:` binds it to), and strips the
//! root's drawing/physics components so it stops being drawn or simulated as the whole model — the
//! root entity itself, and its other components, survive untouched, so game code that already holds
//! its id keeps a live handle.
//!
//! A ragdoll snapshots rotation and translation only — rigid bodies don't scale, so a clip's `scale`
//! channel is ignored here (`part_placements_with`'s `scale` flag off).
//!
//! All-or-nothing: [`switch_to_ragdoll`] validates every precondition before it spawns or mutates
//! anything, so a rejected switch leaves the world exactly as it was.
//!
//! Sim-side arithmetic stays deterministic: a part's pose rotation is the libm quaternion of the
//! shared clip sampler (`ludion_scene::sample_rotation`), the very one the render path uses, and the
//! root's Euler-degrees↔quaternion conversions go through
//! [`ludion_core::math::euler_xyz_degrees_to_quat`]/[`quat_to_euler_xyz_degrees`] (pure-Rust libm),
//! the one pair every crate shares.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat4, Quat, Vec3};
use ludion_core::math::{quat_to_euler_xyz_degrees, sqrt};
use ludion_core::time::SimTime;
use ludion_ecs::{SemanticValidate, World};
use ludion_scene::AssemblyJointKind;
use ludion_spatial::{Body, Collider, Joint, JointKind, RigidBody, Shape, Velocity};

use crate::assembly_mesh::AssemblyData;
use crate::components::{Animation, Mesh, MeshSource, Name, Transform};
use crate::extract::{animation_pose, part_placements_with, rotation_quat};

/// Switches `entity` from a posed `kind: assembly` model into one `rigid` [`Collider`] entity per
/// part, joined by [`Joint`]s, and returns the spawned part entities in `assembly`'s authored order.
/// See the module docs for the whole contract.
///
/// # Errors
/// [`code::RAGDOLL_SWITCH_INVALID`] when `entity` is not alive, has no [`Transform`], its `Mesh.shape`
/// is not [`MeshSource::Assembly`], its `Transform.scale` is not uniform or not strictly positive and
/// finite, or one of `assembly`'s part meshes has no vertices. Nothing is mutated when this fires; the
/// message names `entity` (and its [`Name`] when it has one, so a game switching several figures in
/// one tick can tell which was refused).
pub fn switch_to_ragdoll(
    world: &mut World,
    entity: EntityId,
    assembly: &AssemblyData,
    game_time: SimTime,
) -> Result<Vec<EntityId>, Diagnostic> {
    if !world.is_alive(entity) {
        return Err(switch_invalid(
            &format!("entity `{entity}`"),
            "is not alive",
            "switch an entity that still exists",
        ));
    }
    let named = crate::scene_physics::entity_label(world, entity);
    let Some(root_transform) = crate::hierarchy::world_pose_transform(world, entity) else {
        return Err(switch_invalid(
            &named,
            "has no `Transform`",
            "give the entity a `Transform` before switching it",
        ));
    };
    let Some(root_mesh) = world.get::<Mesh>(entity).ok().flatten().cloned() else {
        return Err(switch_invalid(
            &named,
            "has no `Mesh`",
            "give the entity a `Mesh` whose `shape` is `assembly` before switching it",
        ));
    };
    let MeshSource::Assembly(assembly_rel) = &root_mesh.shape else {
        return Err(switch_invalid(
            &named,
            "its `Mesh.shape` is not `assembly` — it already draws a single part, or something \
             else entirely",
            "switch an entity that still draws the whole assembly, not a single part or a \
             different kind of mesh",
        ));
    };
    let scale = root_transform.scale;
    if scale[0] != scale[1] || scale[1] != scale[2] {
        return Err(switch_invalid(
            &named,
            format!(
                "its `Transform.scale` {scale:?} is not uniform; the switch has no single \
                 factor to size each part's collider and mass by"
            ),
            "give the entity a uniform scale (equal x, y and z) before switching it",
        ));
    }
    let s = scale[0];
    if !(s > 0.0 && s.is_finite()) {
        return Err(switch_invalid(
            &named,
            format!(
                "its `Transform.scale` {scale:?} is not strictly positive and finite; the switch \
                 cannot size a part's collider or mass from it"
            ),
            "give the entity a strictly positive, finite scale before switching it",
        ));
    }
    if let Some(part) = assembly
        .parts
        .iter()
        .find(|part| part.mesh.positions.is_empty())
    {
        return Err(switch_invalid(
            &named,
            format!("assembly part `{}` has no vertices", part.name),
            "give every part file the assembly places some geometry before switching it",
        ));
    }
    // Each part's bounds and mass, before anything spawns: the switch is all-or-nothing.
    let mut part_bounds = Vec::with_capacity(assembly.parts.len());
    for part in &assembly.parts {
        let (min, max) = mesh_bounds(&part.mesh.positions).expect("checked non-empty above");
        let extent = max - min;
        let volume = f64::from(extent.x) * f64::from(extent.y) * f64::from(extent.z);
        let s64 = f64::from(s);
        let mass = (volume * (s64 * s64 * s64)) as f32;
        if !(mass.is_finite() && mass > 0.0) {
            return Err(switch_invalid(
                &named,
                format!(
                    "assembly part `{}` has mass {mass} at scale {s}; a rigid body needs a \
                     strictly positive, finite mass",
                    part.name
                ),
                "give the part volume on all three axes, or switch at a smaller scale",
            ));
        }
        part_bounds.push((min, max, mass));
    }

    // --- Validated: every step below mutates the world. ---

    let (pose, fade) = animation_pose(world, entity, assembly, game_time);
    let placements = part_placements_with(assembly, pose, fade.as_ref(), false);
    let q_root = rotation_quat(root_transform.rotation);
    let root_model = Mat4::from_scale_rotation_translation(
        Vec3::from(scale),
        q_root,
        Vec3::from(root_transform.translation),
    );
    let root_velocity = world
        .get::<Velocity>(entity)
        .ok()
        .flatten()
        .copied()
        .unwrap_or_default();
    let root_name = world.get::<Name>(entity).ok().flatten().cloned();

    let mut spawned = Vec::with_capacity(assembly.parts.len());
    for (index, part) in assembly.parts.iter().enumerate() {
        let (local, local_rotation) = placements[index];
        // `W_i = root_model · placement_i` — the exact composition `extract` draws each part with.
        let world_matrix = root_model * local;
        let translation = world_matrix.w_axis.truncate();
        // Normalized by hand, as `quat_to_euler_xyz_degrees` does: glam's SSE2 `normalize` divides
        // by the length where its NEON backend multiplies by the reciprocal, which split the
        // generated scene across x86-64 and arm64.
        let q = q_root * Quat::from_mat3(&local_rotation);
        let len = sqrt(q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w);
        let rotation_quat = Quat::from_xyzw(q.x / len, q.y / len, q.z / len, q.w / len);
        let rotation = quat_to_euler_xyz_degrees([
            rotation_quat.x,
            rotation_quat.y,
            rotation_quat.z,
            rotation_quat.w,
        ]);

        let (min, max, mass) = part_bounds[index];
        let scaled_min = min * s;
        let scaled_max = max * s;
        let local_center = (scaled_min + scaled_max) * 0.5;
        let world_center = translation + rotation_quat * local_center;
        let linear = Vec3::from(root_velocity.linear)
            + Vec3::from(root_velocity.angular)
                .cross(world_center - Vec3::from(root_transform.translation));

        let id = world.spawn();
        world
            .insert(
                id,
                Transform {
                    translation: translation.into(),
                    rotation,
                    scale,
                },
            )
            .expect("just spawned");
        world
            .insert(
                id,
                Mesh {
                    shape: MeshSource::AssemblyPart {
                        assembly: assembly_rel.clone(),
                        part: part.name.clone(),
                    },
                    color: root_mesh.color,
                    roughness: root_mesh.roughness,
                    metallic: root_mesh.metallic,
                    emissive: root_mesh.emissive,
                    flash: root_mesh.flash,
                    flash_color: root_mesh.flash_color,
                },
            )
            .expect("just spawned");
        let part_collider = Collider {
            shape: Shape::Convex {
                points: box_corners(scaled_min, scaled_max),
            },
            body: Body::Rigid(RigidBody {
                mass,
                ..RigidBody::default()
            }),
            layer: None,
        };
        debug_assert!(
            Collider::validate_semantics(
                &serde_json::to_value(&part_collider).expect("Collider serializes")
            )
            .is_empty(),
            "switch_to_ragdoll built an invalid Collider for part `{}`",
            part.name
        );
        world.insert(id, part_collider).expect("just spawned");
        world
            .insert(
                id,
                Velocity {
                    linear: linear.into(),
                    angular: root_velocity.angular,
                },
            )
            .expect("just spawned");
        if let Some(name) = &root_name {
            world
                .insert(id, Name(format!("{}/{}", name.0, part.name)))
                .expect("just spawned");
        }
        spawned.push(id);
    }

    // One `Joint` per non-root part, carried by the child (decision 4): fixed to its parent unless
    // `assembly.joints` binds it to something else.
    for (index, part) in assembly.parts.iter().enumerate() {
        let Some(parent_index) = part.parent else {
            continue;
        };
        let parent = &assembly.parts[parent_index];
        // The posed placement folds a sampled translation key into the same rest-frame `T` the
        // offset sits in, ahead of the pivot rotation (`extract.rs`'s `part_placements_with`
        // doc: `T(offset_i - offset_parent + translation_i) · R_i(pivot_i)`); the anchor must
        // fold it in too, or a mid-clip switch pivots about a point off by `translation_i · s`
        let sampled_translation = pose
            .and_then(|(clip, time)| ludion_scene::part_pose(clip, &part.name, time).translation)
            .map_or(Vec3::ZERO, |v| {
                Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)
            });
        let anchor = [
            (part.offset[0] + part.pivot[0] - parent.offset[0] + sampled_translation.x) * s,
            (part.offset[1] + part.pivot[1] - parent.offset[1] + sampled_translation.y) * s,
            (part.offset[2] + part.pivot[2] - parent.offset[2] + sampled_translation.z) * s,
        ];
        let binding = assembly.joints.iter().find(|joint| joint.part == part.name);
        let kind = binding.map_or(JointKind::Fixed {}, |joint| joint_kind_of(&joint.kind));
        let break_impulse = binding
            .and_then(|joint| joint.break_impulse)
            .map(|value| value as f32);
        let part_joint = Joint {
            a: spawned[parent_index],
            b: spawned[index],
            kind,
            anchor,
            break_impulse,
            collide: false,
        };
        debug_assert!(
            Joint::validate_semantics(
                &serde_json::to_value(&part_joint).expect("Joint serializes")
            )
            .is_empty(),
            "switch_to_ragdoll built an invalid Joint for part `{}`",
            part.name
        );
        world
            .insert(spawned[index], part_joint)
            .expect("just spawned");
    }

    world.remove::<Mesh>(entity).expect("checked alive above");
    world
        .remove::<Animation>(entity)
        .expect("checked alive above");
    world
        .remove::<Collider>(entity)
        .expect("checked alive above");
    world
        .remove::<Velocity>(entity)
        .expect("checked alive above");

    Ok(spawned)
}

/// `code::RAGDOLL_SWITCH_INVALID` — a refused switch, prefixed with `named` (the offending entity,
/// see [`entity_label`](crate::scene_physics::entity_label)) so a game switching several figures
/// in one tick can tell which one this refers to, and carrying a one-line `help`.
fn switch_invalid(named: &str, message: impl Into<String>, help: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        code::RAGDOLL_SWITCH_INVALID,
        format!("{named}: {}", message.into()),
    )
    .with_help(help)
}

/// The axis-aligned bounding box of `positions` (flat xyz triples, the part-mesh frame), or `None`
/// for an empty mesh.
fn mesh_bounds(positions: &[f32]) -> Option<(Vec3, Vec3)> {
    let mut chunks = positions.chunks_exact(3);
    let first = chunks.next()?;
    let mut min = Vec3::new(first[0], first[1], first[2]);
    let mut max = min;
    for chunk in chunks {
        let p = Vec3::new(chunk[0], chunk[1], chunk[2]);
        min = min.min(p);
        max = max.max(p);
    }
    Some((min, max))
}

/// The 8 corners of the box `[min, max]`. A convex hull's points have no meaningful order, but a
/// fixed one keeps two switches of the same pose byte-identical.
fn box_corners(min: Vec3, max: Vec3) -> Vec<[f32; 3]> {
    vec![
        [min.x, min.y, min.z],
        [max.x, min.y, min.z],
        [min.x, max.y, min.z],
        [max.x, max.y, min.z],
        [min.x, min.y, max.z],
        [max.x, min.y, max.z],
        [min.x, max.y, max.z],
        [max.x, max.y, max.z],
    ]
}

/// Maps an assembly document's `joints:` entry kind onto the live [`JointKind`] `ludion-physics`
/// builds a body from — `f64` (the assembly format's convention) narrowed to `f32` (the component's).
fn joint_kind_of(kind: &AssemblyJointKind) -> JointKind {
    match kind {
        AssemblyJointKind::Hinge { axis, limits } => JointKind::Hinge {
            axis: to_f32_3(*axis),
            limits: limits.map(to_f32_2),
        },
        AssemblyJointKind::Ball { limits } => JointKind::Ball {
            limits: limits.map(|pairs| pairs.map(to_f32_2)),
        },
        AssemblyJointKind::Slider { axis, limits } => JointKind::Slider {
            axis: to_f32_3(*axis),
            limits: limits.map(to_f32_2),
        },
        AssemblyJointKind::Fixed => JointKind::Fixed {},
    }
}

fn to_f32_3(v: [f64; 3]) -> [f32; 3] {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}

fn to_f32_2(v: [f64; 2]) -> [f32; 2] {
    [v[0] as f32, v[1] as f32]
}
