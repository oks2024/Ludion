//! The engine's `pose_blend` system: resolves each ragdoll group's [`PoseBlend`] into its parts'
//! [`JointMotor`]s.
//!
//! A part's group root is its [`JointPoses::group`], else its [`SkinBone::skin`]; a part with
//! neither is inert and its motor is never touched. A part carrying `JointPoses` with a resolvable
//! group has its `JointMotor` owned by this system: `{ pose·w, stiffness·w, damping·w, max_force }`
//! while the root is alive, carries a `PoseBlend` with `w > 0`, and the part has that pose; no motor
//! otherwise. Writes happen only when the result differs from what the part carries, so a steady
//! blend leaves the physics motor path (and body sleep) alone. A world with neither component is
//! untouched. Installed just before the physics triple — after every game system, so a weight a
//! game writes this tick drives this tick's motor.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_spatial::{JointMotor, JointPoses, PoseBlend};
use serde_json::Value;

use crate::components::SkinBone;

pub(crate) const SYSTEM_NAME: &str = "pose_blend";

/// Adds [`pose_blend`] to `schedule`; called immediately before the physics triple.
pub(crate) fn install(schedule: &mut Schedule) {
    schedule.add_system(SYSTEM_NAME, pose_blend);
}

/// The system's `engine.describe` entry, with no call site to attribute; the caller sets its `order`.
pub(crate) fn describe_entry() -> Value {
    serde_json::json!({
        "name": SYSTEM_NAME,
        "order": 0,
        "fn_path": "ludion_engine::pose_blend",
        "registered_at": Value::Null,
        "queries": [{
            "components": [
                {"name": "PoseBlend", "access": "read"},
                {"name": "JointPoses", "access": "read"},
                {"name": "SkinBone", "access": "read"},
                {"name": "JointMotor", "access": "write"},
            ],
        }],
    })
}

/// The blend weight as the system applies it: clamped to `[0, 1]`, and `0` when not finite (a
/// runtime write bypasses the schema range).
fn effective_weight(blend: &PoseBlend) -> f32 {
    if blend.weight.is_finite() {
        blend.weight.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The group root `part` belongs to: `JointPoses.group`, else `SkinBone.skin`.
fn group_of(world: &World, part: EntityId, poses: &JointPoses) -> Option<EntityId> {
    poses
        .group
        .or_else(|| world.component::<SkinBone>(part).map(|bone| bone.skin))
}

/// The motor `poses` asks for under `blend`, or `None` when the part should carry none.
fn desired_motor(poses: &JointPoses, blend: Option<&PoseBlend>) -> Option<JointMotor> {
    let blend = blend?;
    let w = effective_weight(blend);
    if w == 0.0 {
        return None;
    }
    let pose = poses.poses.get(&blend.pose)?;
    let motor = JointMotor {
        target: pose.iter().map(|t| t * w).collect(),
        stiffness: poses.stiffness * w,
        damping: poses.damping * w,
        max_force: poses.max_force,
    };
    // A NaN would compare unequal to itself every tick: rewrite and keep the body awake forever.
    let finite = motor.target.iter().all(|t| t.is_finite())
        && motor.stiffness.is_finite()
        && motor.damping.is_finite()
        && motor.max_force.is_none_or(f32::is_finite);
    finite.then_some(motor)
}

/// Every `JointPoses` part whose desired motor differs from the one it carries, in ascending
/// `EntityId` order. A part with no resolvable group is skipped.
fn plan_writes(world: &mut World) -> Vec<(EntityId, Option<JointMotor>)> {
    let parts: Vec<EntityId> = world
        .query_ids::<(&JointPoses,)>()
        .map(|(id, _)| id)
        .collect();
    let mut writes = Vec::new();
    for part in parts {
        let Some(poses) = world.component::<JointPoses>(part) else {
            continue;
        };
        let Some(group) = group_of(world, part, poses) else {
            continue;
        };
        let blend = if world.is_alive(group) {
            world.component::<PoseBlend>(group)
        } else {
            None
        };
        let desired = desired_motor(poses, blend);
        if world.component::<JointMotor>(part) != desired.as_ref() {
            writes.push((part, desired));
        }
    }
    writes
}

/// Reports `L4035` once per (group, pose) for each live blend at weight above 0 whose group has no
/// part carrying the pose.
fn report_missing_poses(world: &mut World) {
    let blends: Vec<(EntityId, String)> = world
        .query_ids::<(&PoseBlend,)>()
        .filter(|(_, (blend,))| effective_weight(blend) > 0.0)
        .map(|(id, (blend,))| (id, blend.pose.clone()))
        .collect();
    if blends.is_empty() {
        return;
    }
    let parts: Vec<EntityId> = world
        .query_ids::<(&JointPoses,)>()
        .map(|(id, _)| id)
        .collect();
    let mut carried: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
    for part in parts {
        if let Some(poses) = world.component::<JointPoses>(part)
            && let Some(group) = group_of(world, part, poses)
        {
            carried.entry(group).or_default().push(part);
        }
    }
    for (root, pose) in blends {
        let found = carried.get(&root).is_some_and(|parts| {
            parts.iter().any(|part| {
                world
                    .component::<JointPoses>(*part)
                    .is_some_and(|poses| poses.poses.contains_key(&pose))
            })
        });
        if found {
            continue;
        }
        let key = format!("{}/{pose}", crate::diagnostics::offender_key(world, root));
        if world
            .resource::<crate::diagnostics::Diagnostics>()
            .is_some_and(|d| d.is_latched(code::POSE_BLEND_POSE_MISSING, &key))
        {
            continue;
        }
        let diagnostic = Diagnostic::new(
            code::POSE_BLEND_POSE_MISSING,
            format!(
                "{} blends pose `{pose}`, but no part of its group carries that pose, so the \
                 parts stay limp",
                crate::scene_physics::entity_label(world, root)
            ),
        )
        .with_help(
            "add the pose to the parts' `JointPoses.poses`, fix the `PoseBlend.pose` name, or \
             check the parts reach this entity through `JointPoses.group` or `SkinBone.skin`",
        );
        crate::diagnostics::report_full(world, key, diagnostic);
    }
}

/// Resolves every group's [`PoseBlend`] into its parts' [`JointMotor`]s (the module docs have the
/// rules).
pub(crate) fn pose_blend(world: &mut World, _ctx: &SystemCtx) {
    if world.component_count::<JointPoses>() == 0 && world.component_count::<PoseBlend>() == 0 {
        return;
    }
    report_missing_poses(world);
    for (part, motor) in plan_writes(world) {
        let _ = match motor {
            Some(motor) => world.insert(part, motor).map(drop),
            None => world.remove::<JointMotor>(part).map(drop),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Diagnostics;
    use ludion_core::time::{FrameIndex, TickRate};

    fn ctx() -> SystemCtx {
        SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT)
    }

    fn world() -> World {
        let mut world = World::new();
        world.insert_resource(Diagnostics::default());
        world
    }

    fn poses(entries: &[(&str, &[f32])]) -> JointPoses {
        JointPoses {
            group: None,
            stiffness: 40.0,
            damping: 8.0,
            max_force: Some(200.0),
            poses: entries
                .iter()
                .map(|(name, pose)| (name.to_string(), pose.to_vec()))
                .collect(),
        }
    }

    fn blend(pose: &str, weight: f32) -> PoseBlend {
        PoseBlend {
            pose: pose.to_string(),
            weight,
        }
    }

    /// A root with a `PoseBlend` and one part explicitly grouped to it.
    fn rig(pose_set: JointPoses, pose: &str, weight: f32) -> (World, EntityId, EntityId) {
        let mut world = world();
        let root = world.spawn();
        world.insert(root, blend(pose, weight)).unwrap();
        let part = world.spawn();
        world
            .insert(
                part,
                JointPoses {
                    group: Some(root),
                    ..pose_set
                },
            )
            .unwrap();
        (world, root, part)
    }

    fn motor(world: &World, part: EntityId) -> Option<JointMotor> {
        world.component::<JointMotor>(part).cloned()
    }

    fn reported(world: &World) -> Vec<String> {
        world
            .resource::<Diagnostics>()
            .unwrap()
            .reported(code::POSE_BLEND_POSE_MISSING)
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_hinge_pose_resolves_scaled_by_the_weight() {
        let (mut world, _, part) = rig(poses(&[("curl", &[60.0])]), "curl", 0.5);
        pose_blend(&mut world, &ctx());
        assert_eq!(
            motor(&world, part),
            Some(JointMotor {
                target: vec![30.0],
                stiffness: 20.0,
                damping: 4.0,
                max_force: Some(200.0),
            })
        );
    }

    #[test]
    fn a_non_finite_value_resolves_to_no_motor_and_writes_nothing() {
        for bad in ["target", "stiffness", "damping", "max_force"] {
            let mut set = poses(&[("curl", &[60.0])]);
            match bad {
                "target" => set.poses.insert("curl".to_string(), vec![f32::NAN]),
                "stiffness" => {
                    set.stiffness = f32::INFINITY;
                    None
                }
                "damping" => {
                    set.damping = f32::NAN;
                    None
                }
                _ => {
                    set.max_force = Some(f32::NAN);
                    None
                }
            };
            let (mut world, _, part) = rig(set, "curl", 1.0);
            assert!(
                plan_writes(&mut world).is_empty(),
                "{bad}: nothing to write"
            );
            pose_blend(&mut world, &ctx());
            assert_eq!(motor(&world, part), None, "{bad}: no motor");
        }
    }

    #[test]
    fn a_ball_pose_scales_all_three_components() {
        let (mut world, _, part) = rig(poses(&[("lean", &[10.0, 20.0, -30.0])]), "lean", 0.5);
        pose_blend(&mut world, &ctx());
        assert_eq!(motor(&world, part).unwrap().target, vec![5.0, 10.0, -15.0]);
    }

    #[test]
    fn the_group_comes_from_skin_bone_and_explicit_group_wins() {
        let mut world = world();
        let skin = world.spawn();
        world.insert(skin, blend("curl", 1.0)).unwrap();
        let other = world.spawn();
        world.insert(other, blend("curl", 0.0)).unwrap();
        let bone = |skin| SkinBone {
            skin,
            bone: "b".to_string(),
        };

        let via_skin = world.spawn();
        world.insert(via_skin, poses(&[("curl", &[60.0])])).unwrap();
        world.insert(via_skin, bone(skin)).unwrap();

        let explicit = world.spawn();
        let mut explicit_poses = poses(&[("curl", &[60.0])]);
        explicit_poses.group = Some(other);
        world.insert(explicit, explicit_poses).unwrap();
        world.insert(explicit, bone(skin)).unwrap();

        pose_blend(&mut world, &ctx());
        assert_eq!(motor(&world, via_skin).unwrap().target, vec![60.0]);
        assert_eq!(
            motor(&world, explicit),
            None,
            "`group` wins over `SkinBone`"
        );
    }

    #[test]
    fn a_steady_blend_writes_nothing_and_a_change_writes() {
        let (mut world, root, part) = rig(poses(&[("curl", &[60.0])]), "curl", 0.5);
        pose_blend(&mut world, &ctx());
        assert!(plan_writes(&mut world).is_empty());
        world.component_mut::<PoseBlend>(root).unwrap().weight = 0.75;
        let writes = plan_writes(&mut world);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, part);
        assert_eq!(writes[0].1.as_ref().unwrap().target, vec![45.0]);
    }

    #[test]
    fn zero_nan_and_a_missing_blend_leave_no_motor() {
        for weight in [0.0, f32::NAN] {
            let (mut world, root, part) = rig(poses(&[("curl", &[60.0])]), "curl", 1.0);
            pose_blend(&mut world, &ctx());
            assert!(motor(&world, part).is_some());
            world.component_mut::<PoseBlend>(root).unwrap().weight = weight;
            pose_blend(&mut world, &ctx());
            assert_eq!(motor(&world, part), None, "weight {weight}");
        }

        let (mut world, root, part) = rig(poses(&[("curl", &[60.0])]), "curl", 1.0);
        pose_blend(&mut world, &ctx());
        world.remove::<PoseBlend>(root).unwrap();
        pose_blend(&mut world, &ctx());
        assert_eq!(motor(&world, part), None, "a root with no `PoseBlend`");
    }

    #[test]
    fn a_weight_past_one_clamps() {
        let (mut world, _, part) = rig(poses(&[("curl", &[60.0])]), "curl", 3.0);
        pose_blend(&mut world, &ctx());
        assert_eq!(motor(&world, part).unwrap().target, vec![60.0]);
    }

    #[test]
    fn a_part_lacking_the_pose_stays_limp_while_its_sibling_is_driven() {
        let (mut world, root, driven) = rig(poses(&[("curl", &[60.0])]), "curl", 1.0);
        let limp = world.spawn();
        let mut limp_poses = poses(&[("stand", &[0.0])]);
        limp_poses.group = Some(root);
        world.insert(limp, limp_poses).unwrap();
        pose_blend(&mut world, &ctx());
        assert!(motor(&world, driven).is_some());
        assert_eq!(motor(&world, limp), None);
        assert!(reported(&world).is_empty(), "the group carries the pose");
    }

    #[test]
    fn a_missing_pose_warns_once_over_several_ticks_and_only_above_zero_weight() {
        let (mut world, root, _) = rig(poses(&[("stand", &[0.0])]), "curl", 1.0);
        for _ in 0..3 {
            pose_blend(&mut world, &ctx());
        }
        assert_eq!(reported(&world).len(), 1);
        assert!(
            reported(&world)[0].ends_with("/curl"),
            "{:?}",
            reported(&world)
        );

        let (mut quiet, _, _) = rig(poses(&[("stand", &[0.0])]), "curl", 0.0);
        pose_blend(&mut quiet, &ctx());
        assert!(reported(&quiet).is_empty());
        let _ = root;
    }

    #[test]
    fn a_part_with_no_group_is_untouched() {
        let mut world = world();
        let part = world.spawn();
        world.insert(part, poses(&[("curl", &[60.0])])).unwrap();
        let hand_written = JointMotor {
            target: vec![5.0],
            stiffness: 1.0,
            damping: 1.0,
            max_force: None,
        };
        world.insert(part, hand_written.clone()).unwrap();
        pose_blend(&mut world, &ctx());
        assert_eq!(motor(&world, part), Some(hand_written));
    }

    #[test]
    fn a_world_without_the_components_is_untouched() {
        let mut world = world();
        let e = world.spawn();
        pose_blend(&mut world, &ctx());
        assert!(world.is_alive(e));
    }
}
