//! [`switch_to_ragdoll`] and [`switch_to_ragdoll_with`]: promote a posed `kind: assembly` entity into
//! one `rigid` body entity per part, joined by [`Joint`]s — "a ragdoll is the assembly plus a joint
//! list" (`Plan/physics.md` W3's ragdoll-authoring plan, decisions 1-6).
//!
//! Game code decides *when* to call this — a death blow, a stagger, a rope grab — and hands it the
//! meshed [`AssemblyData`] its own asset cache already holds (`ludion_engine::load_assembly`). The
//! switch is otherwise pure ECS surgery: it reads the entity's current pose (rest, or mid-clip at the
//! caller's `game_time`, through the very same `part_placements_sides` extraction draws with, layers
//! included), spawns one entity per part at that pose's world transform, wires a [`Joint`] onto each
//! non-root part (fixed to its parent by default, or whatever `assembly`'s `joints:` binds it to),
//! and strips the root's drawing/physics components so it stops being drawn or simulated as the whole
//! model — the root entity itself, and its other components, survive untouched, so game code that
//! already holds its id keeps a live handle.
//!
//! A ragdoll snapshots rotation and translation only — rigid bodies don't scale, so a clip's `scale`
//! channel is ignored here (`part_placements_sides`'s `scale` flag off). The sampled pose includes
//! the entity's `AnimationLayers`, so a ragdoll spawns at the pose that was drawn; look-at, IK and
//! ground placement are presentation only and are not in it.
//!
//! Where a joint's zero sits is the caller's choice ([`LimitsFrom`]). By default the physics layer
//! captures each joint's zero from the posed bodies, so limits, `JointMotor` targets and
//! `JointPoses` count from the switch pose. With [`LimitsFrom::Rest`] each joint also sets
//! `Joint.zero` at the rig's rest rotation, so they count from rest however the body fell; a
//! switch pose outside a limit measured from rest is pulled back to it by the solver on the first
//! step, a bounded snap, and is not refused. With [`RagdollSwitch::part_velocity`] each part also
//! keeps the velocity of the pose it was drawn in, instead of starting limp.
//!
//! A root `SurfaceTag` is inherited: each spawned part gets a tag with its `parts` entry's surface
//! (matched by part name), else the root's `surface`; a name key matching no part is one `L1695`
//! warning. The root keeps its tag.
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
use ludion_core::math::f64 as f64s;
use ludion_core::math::glam::{DQuat, Mat4, Quat, Vec3};
use ludion_core::math::{quat_to_euler_xyz_degrees, sqrt};
use ludion_core::suggest::nearest;
use ludion_core::time::{GameTime, Q16_PER_TICK};
use ludion_ecs::{SemanticValidate, World};
use ludion_physics::{SurfacePart, SurfaceTag};
use ludion_scene::AssemblyJointKind;
use ludion_spatial::{Body, Collider, Joint, JointKind, RigidBody, Shape, Velocity};

use crate::assembly_mesh::AssemblyData;
use crate::components::{Animation, Mesh, MeshSource, Name, PoseSnapshot, Transform};
use crate::extract::{
    animation_pose, animation_pose_at, assembly_layer_poses, assembly_layer_poses_at,
    part_placements_sides, rotation_quat,
};

/// Where a posed switch measures its joints' zeros from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LimitsFrom {
    /// The posed bodies' relative pose at the switch: limits, motor targets and joint poses count
    /// from the pose the figure was drawn in.
    #[default]
    SwitchPose,
    /// The rig's rest pose: each joint sets `Joint.zero` to the identity, so they count
    /// from rest however the body fell.
    Rest,
}

/// Options for [`switch_to_ragdoll_with`]. Build it from the default and set what you need, so a
/// field added later does not break the call:
///
/// ```
/// use ludion_engine::{LimitsFrom, RagdollSwitch};
///
/// let mut opts = RagdollSwitch::default();
/// opts.limits_from = LimitsFrom::Rest;
/// assert_eq!(opts.limits_from, LimitsFrom::Rest);
/// opts.part_velocity = true;
/// assert!(opts.part_velocity);
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RagdollSwitch {
    /// Where the joints' zeros come from (default: the switch pose).
    pub limits_from: LimitsFrom,
    /// Adds the drawn pose's own motion to each part's `Velocity` (default `false`: the part takes
    /// only the root's rigid velocity). The motion is a two-sample finite difference of the part's
    /// posed world transform at `game_time` and one tick earlier, in radians and metres per gameplay
    /// second, so clips, blends, crossfades and layers all count, and a held or time-scaled clock is
    /// read in gameplay time. A clip cut at the switch tick contributes only its own motion: the
    /// earlier sample clamps to the new clip's first frame. A rest figure, or a switch at tick 0,
    /// adds nothing.
    pub part_velocity: bool,
}

/// Switches `entity` from a posed `kind: assembly` model into one `rigid` [`Collider`] entity per
/// part, joined by [`Joint`]s, and returns the spawned part entities in `assembly`'s authored order.
/// The joints' zeros are captured from the switch pose; [`switch_to_ragdoll_with`] chooses otherwise.
/// See the module docs for the whole contract.
///
/// # Errors
/// As [`switch_to_ragdoll_with`].
pub fn switch_to_ragdoll(
    world: &mut World,
    entity: EntityId,
    assembly: &AssemblyData,
    game_time: impl Into<GameTime>,
) -> Result<Vec<EntityId>, Diagnostic> {
    switch_to_ragdoll_with(world, entity, assembly, game_time, RagdollSwitch::default())
}

/// [`switch_to_ragdoll`] with [`RagdollSwitch`] options: with [`LimitsFrom::Rest`] every joint
/// sets `Joint.zero` to the rig's rest rotation (the identity: an assembly's parts
/// have no rest rotation), so the limits, motor targets and joint poses are measured from rest.
///
/// # Errors
/// [`code::RAGDOLL_SWITCH_INVALID`] when `entity` is not alive, has no [`Transform`], its `Mesh.shape`
/// is not [`MeshSource::Assembly`], its `Transform.scale` is not uniform or not strictly positive and
/// finite, or one of `assembly`'s part meshes has no vertices. Nothing is mutated when this fires; the
/// message names `entity` (and its [`Name`] when it has one, so a game switching several figures in
/// one tick can tell which was refused).
///
/// A root [`SurfaceTag`] is inherited per part by name (else the root's surface); an unmatched name
/// key is an `L1695` warning, not a refusal.
pub fn switch_to_ragdoll_with(
    world: &mut World,
    entity: EntityId,
    assembly: &AssemblyData,
    game_time: impl Into<GameTime>,
    opts: RagdollSwitch,
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

    let now = game_time.into();
    let (pose, fade) = animation_pose(world, entity, assembly, now);
    let layers = assembly_layer_poses(world, entity, assembly, now);
    let placements = part_placements_sides(assembly, pose, fade, false, &layers);
    // Each part's sampled translation, read before the world mutates: the joint anchors below fold
    // it in, and `pose` borrows the world.
    let sampled_translations: Vec<Vec3> = assembly
        .parts
        .iter()
        .map(|part| {
            pose.sample(&part.name).translation.map_or(Vec3::ZERO, |v| {
                Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)
            })
        })
        .collect();
    let q_root = rotation_quat(root_transform.rotation);
    let root_model = Mat4::from_scale_rotation_translation(
        Vec3::from(scale),
        q_root,
        Vec3::from(root_transform.translation),
    );
    // The drawn pose one tick back, only when the caller asked for it: `(placements, seconds)`.
    let earlier = opts.part_velocity.then(|| {
        let step = Q16_PER_TICK.min(now.as_q16());
        let then = GameTime::from_q16(now.as_q16() - step, now.rate());
        let back = step as f64 / Q16_PER_TICK as f64 * now.rate().seconds_per_tick();
        let (pose, fade) = animation_pose_at(world, entity, assembly, now, then, back);
        let layers = assembly_layer_poses_at(world, entity, assembly, now, then, back);
        (
            part_placements_sides(assembly, pose, fade, false, &layers),
            back,
        )
    });
    let root_velocity = world
        .get::<Velocity>(entity)
        .ok()
        .flatten()
        .copied()
        .unwrap_or_default();
    let root_name = world.get::<Name>(entity).ok().flatten().cloned();
    let root_tag = world.get::<SurfaceTag>(entity).ok().flatten().cloned();
    let unmatched = root_tag
        .as_ref()
        .map(|tag| unmatched_part_keys(tag, assembly, world, entity))
        .unwrap_or_default();

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
        let mut linear = Vec3::from(root_velocity.linear)
            + Vec3::from(root_velocity.angular)
                .cross(world_center - Vec3::from(root_transform.translation));
        let mut angular = root_velocity.angular;
        if let Some((then_placements, back)) = &earlier
            && *back > 0.0
        {
            let (then_local, then_rotation) = then_placements[index];
            let then_translation = (root_model * then_local).w_axis.truncate();
            let then_q = q_root * Quat::from_mat3(&then_rotation);
            let then_len = sqrt(
                then_q.x * then_q.x
                    + then_q.y * then_q.y
                    + then_q.z * then_q.z
                    + then_q.w * then_q.w,
            );
            let then_q = Quat::from_xyzw(
                then_q.x / then_len,
                then_q.y / then_len,
                then_q.z / then_len,
                then_q.w / then_len,
            );
            let then_center = then_translation + then_q * local_center;
            let clip_linear = (world_center.as_dvec3() - then_center.as_dvec3()) / *back;
            let clip_angular = rotation_rate(rotation_quat, then_q, *back);
            linear += clip_linear.as_vec3();
            angular = [
                angular[0] + clip_angular[0] as f32,
                angular[1] + clip_angular[1] as f32,
                angular[2] + clip_angular[2] as f32,
            ];
        }

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
                    wrap: root_mesh.wrap,
                    scatter_color: root_mesh.scatter_color,
                    rim: root_mesh.rim,
                    medium: root_mesh.medium,
                    porosity: None,
                    blend: root_mesh.blend,
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
            sensor: false,
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
                    angular,
                },
            )
            .expect("just spawned");
        if let Some(name) = &root_name {
            world
                .insert(id, Name(format!("{}/{}", name.0, part.name)))
                .expect("just spawned");
        }
        if let Some(tag) = &root_tag {
            let surface = tag
                .parts
                .iter()
                .find_map(|entry| match &entry.part {
                    SurfacePart::Name(name) if *name == part.name => Some(entry.surface.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| tag.surface.clone());
            world
                .insert(
                    id,
                    SurfaceTag {
                        surface,
                        parts: vec![],
                        cells: None,
                    },
                )
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
        // offset sits in, ahead of the pivot rotation (`extract.rs`'s `part_placements_sides`
        // doc: `T(offset_i - offset_parent + translation_i) · R_i(pivot_i)`); the anchor must
        // fold it in too, or a mid-clip switch pivots about a point off by `translation_i · s`
        let sampled_translation = sampled_translations[index];
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
            zero: (opts.limits_from == LimitsFrom::Rest).then_some([0.0, 0.0, 0.0, 1.0]),
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
        .remove::<PoseSnapshot>(entity)
        .expect("checked alive above");
    world
        .remove::<Collider>(entity)
        .expect("checked alive above");
    world
        .remove::<Velocity>(entity)
        .expect("checked alive above");
    crate::motor::suspend_motor_of(world, entity);

    for (key, diagnostic) in unmatched {
        crate::diagnostics::report_full(world, key, diagnostic);
    }
    Ok(spawned)
}

/// The `L1695` help: the closest part name, else every part name.
fn part_key_help(key: &str, names: &[&str]) -> String {
    match nearest(key, names.iter().copied()) {
        Some(near) => format!("the closest part is `{near}`"),
        None => format!("the assembly's parts are {}", names.join(", ")),
    }
}

/// One `L1695` warning (with its latch key) per `SurfaceTag.parts` name key that matches no part of
/// `assembly`. Read-only, so it runs before the switch spawns anything.
fn unmatched_part_keys(
    tag: &SurfaceTag,
    assembly: &AssemblyData,
    world: &World,
    entity: EntityId,
) -> Vec<(String, Diagnostic)> {
    let names: Vec<&str> = assembly
        .parts
        .iter()
        .map(|part| part.name.as_str())
        .collect();
    let mut found = Vec::new();
    for (j, entry) in tag.parts.iter().enumerate() {
        let SurfacePart::Name(key) = &entry.part else {
            continue;
        };
        if names.contains(&key.as_str()) {
            continue;
        }
        let help = part_key_help(key, &names);
        let latch = format!("{}:{key}", crate::diagnostics::offender_key(world, entity));
        found.push((
            latch,
            Diagnostic::new(
                code::SURFACE_PART_UNMATCHED,
                format!(
                    "`SurfaceTag` part `{key}` names no part of the assembly; every ragdoll part \
                     took the root surface `{}`",
                    tag.surface
                ),
            )
            .with_path(format!("/parts/{j}/part"))
            .with_help(help),
        ));
    }
    found
}

/// `code::RAGDOLL_SWITCH_INVALID` — a refused switch, prefixed with `named` (the offending entity,
/// see [`entity_label`](crate::scene_physics::entity_label)) so a game switching several figures
/// in one tick can tell which one this refers to, and carrying a one-line `help`.
pub(crate) fn switch_invalid(
    named: &str,
    message: impl Into<String>,
    help: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        code::RAGDOLL_SWITCH_INVALID,
        format!("{named}: {}", message.into()),
    )
    .with_help(help)
}

/// The angular velocity (radians per second, world frame) that turns `then` into `now` over `back`
/// seconds along the shortest arc. `f64` with libm, never glam's `normalize` or `to_axis_angle`.
fn rotation_rate(now: Quat, then: Quat, back: f64) -> [f64; 3] {
    let widen = |q: Quat| DQuat::from_xyzw(q.x.into(), q.y.into(), q.z.into(), q.w.into());
    let (a, b) = (widen(now), widen(then));
    let mut d = a * DQuat::from_xyzw(-b.x, -b.y, -b.z, b.w);
    if d.w < 0.0 {
        d = DQuat::from_xyzw(-d.x, -d.y, -d.z, -d.w);
    }
    let s = f64s::sqrt(d.x * d.x + d.y * d.y + d.z * d.z);
    let scale = if s < 1e-12 {
        2.0 / back
    } else {
        2.0 * f64s::atan2(s, d.w) / s / back
    };
    [d.x * scale, d.y * scale, d.z * scale]
}

/// The axis-aligned bounding box of `positions` (flat xyz triples, the part-mesh frame), or `None`
/// for an empty mesh.
pub(crate) fn mesh_bounds(positions: &[f32]) -> Option<(Vec3, Vec3)> {
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

#[cfg(test)]
mod tests {
    use super::part_key_help;

    #[test]
    fn an_unmatched_key_names_the_closest_part_else_lists_them() {
        let names = ["torso", "helmet", "arm-hinge"];
        assert_eq!(
            part_key_help("helmt", &names),
            "the closest part is `helmet`"
        );
        assert_eq!(
            part_key_help("zzzzzzzz", &names),
            "the assembly's parts are torso, helmet, arm-hinge"
        );
    }
}
