//! Maps a scene's `physics:` block onto `ludion-physics`'s settings and installs the write/step/
//! mirror triple — the one place this crate couples `ludion_scene::ScenePhysics` to
//! `ludion_physics::PhysicsSettings`, since neither of those crates may name the other's type
//! (`ludion-scene` never learns a solver exists; `ludion-physics` never learns a scene format
//! exists).

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{Parent, Schedule, World};
use ludion_physics::{
    CollisionLayers, MIRROR, PhysicsSettings, PhysicsSpace, PhysicsWorld, STEP, WRITE, WireBody,
};
use ludion_renderer::DebugSegment;
use ludion_scene::{ScenePhysics, ScenePhysicsSpace};
use ludion_sim::{SimConfig, SimDriver};
use ludion_spatial::{Body, Collider, Joint};
use serde_json::Value;

use crate::components::{Name, Transform};

/// The most segments the `colliders` debug category draws in one frame; bodies are drawn in the
/// physics table's creation order and drawing stops at the cap.
pub(crate) const MAX_COLLIDER_SEGMENTS: usize = 200_000;

const STATIC_COLOR: [f32; 4] = [0.55, 0.55, 0.55, 1.0];
const KINEMATIC_COLOR: [f32; 4] = [0.0, 0.85, 0.85, 1.0];
const RIGID_COLOR: [f32; 4] = [1.0, 0.45, 0.05, 1.0];
const SLEEPING_RIGID_COLOR: [f32; 4] = [0.5, 0.225, 0.025, 1.0];

/// Whether the world holds at least one physics body.
pub(crate) fn any_body(world: &World) -> bool {
    world
        .resource::<PhysicsWorld>()
        .is_some_and(|physics| physics.body_count() > 0)
}

/// Every physics body's collider as coloured wireframe segments (`colliders` debug category), up to
/// [`MAX_COLLIDER_SEGMENTS`].
pub(crate) fn collider_debug_lines(world: &World) -> Vec<DebugSegment> {
    let mut lines = Vec::new();
    let Some(physics) = world.resource::<PhysicsWorld>() else {
        return lines;
    };
    physics.collider_wireframe(&mut |body, a, b| {
        if lines.len() >= MAX_COLLIDER_SEGMENTS {
            return;
        }
        let color = match body {
            WireBody::Static => STATIC_COLOR,
            WireBody::Kinematic => KINEMATIC_COLOR,
            WireBody::Rigid { sleeping: false } => RIGID_COLOR,
            WireBody::Rigid { sleeping: true } => SLEEPING_RIGID_COLOR,
        };
        lines.push(DebugSegment { a, b, color });
    });
    lines
}

/// Maps a scene's `physics:` block onto the solver tunables `ludion-physics` reads: the pinned
/// defaults, with the block's present fields overriding them.
pub(crate) fn settings(physics: &ScenePhysics) -> PhysicsSettings {
    let space = match physics.space {
        ScenePhysicsSpace::Free => PhysicsSpace::Free,
        ScenePhysicsSpace::Planar => PhysicsSpace::Planar,
    };
    let mut settings = PhysicsSettings::new(space);
    if let Some(gravity) = physics.gravity {
        // The scene parser already bounds every element to f32's finite range (`L1033`).
        #[allow(clippy::cast_possible_truncation)]
        {
            settings.gravity = gravity.map(|component| component as f32);
        }
    }
    if let Some(solver_iterations) = physics.solver_iterations {
        settings.solver_iterations = solver_iterations;
    }
    if let Some(stabilization_iterations) = physics.stabilization_iterations {
        settings.stabilization_iterations = stabilization_iterations;
    }
    if let Some(sleep_after_ticks) = physics.sleep_after_ticks {
        settings.sleep_after_ticks = sleep_after_ticks;
    }
    if !physics.layers.is_empty() {
        let matrix = physics.layer_matrix();
        let masks = (0..matrix.names().len())
            .map(|index| matrix.ignores_mask(index))
            .collect();
        settings.layers = Some(CollisionLayers::new(matrix.names().to_vec(), masks));
    }
    settings
}

/// Appends the physics triple to `schedule`, after every game system already added, when the
/// loaded scene declared a `physics:` block — the only opt-in there is. Does nothing for `None`.
pub(crate) fn install(schedule: &mut Schedule, physics: Option<&ScenePhysics>) {
    if let Some(physics) = physics {
        ludion_physics::install::<Transform>(schedule, settings(physics));
    }
}

/// The driver every registry-only host (`render`, `sweep`) builds: no game systems, just the
/// scene's `physics:` block installed on an otherwise empty schedule, and the scene's seed —
/// physics is the one exception to "no game schedule is ever installed here", since a scene's
/// `physics:` block is the only opt-in there is, so a bare host steps it too.
pub(crate) fn bare_driver(
    mut world: World,
    scene: &ludion_scene::Scene,
    project_root: &std::path::Path,
) -> SimDriver {
    world.insert_resource(crate::collider_mesh::library(project_root.to_path_buf()));
    world.insert_resource(crate::path_follow::PathLibrary::new(
        project_root.to_path_buf(),
    ));
    world.insert_resource(crate::FontLibrary::load(project_root));
    let mut schedule = Schedule::new();
    crate::path_follow::install(&mut schedule, crate::path_follow::present(&world));
    install(&mut schedule, scene.physics.as_ref());
    SimDriver::new(
        world,
        schedule,
        SimConfig {
            seed: scene.seed.unwrap_or(0),
            ..SimConfig::default()
        },
    )
}

/// The physics triple's `engine.describe` entries — `physics_write`, `physics_step`,
/// `physics_mirror` — shaped like a game system's entry, but `registered_at: null` since there is
/// no call site to attribute (the scene declared it, not a `.system(...)` call). Ordered at
/// `order_base`, `order_base + 1`, `order_base + 2`; the caller passes `self.systems.len()` so
/// physics always describes last, matching where [`install`] actually places it.
pub(crate) fn describe_entries(order_base: usize) -> [Value; 3] {
    [
        serde_json::json!({
            "name": WRITE,
            "order": order_base,
            "fn_path": "ludion_physics::install",
            "registered_at": Value::Null,
            "queries": [{
                "components": [
                    {"name": "Collider", "access": "read"},
                    {"name": "Transform", "access": "read"},
                    {"name": "Velocity", "access": "read"},
                    {"name": "Joint", "access": "read"},
                    {"name": "JointMotor", "access": "read"},
                ],
            }],
        }),
        serde_json::json!({
            "name": STEP,
            "order": order_base + 1,
            "fn_path": "ludion_physics::install",
            "registered_at": Value::Null,
            "queries": [],
        }),
        serde_json::json!({
            "name": MIRROR,
            "order": order_base + 2,
            "fn_path": "ludion_physics::install",
            "registered_at": Value::Null,
            "queries": [{
                "components": [
                    {"name": "Transform", "access": "write"},
                    {"name": "Velocity", "access": "write"},
                    {"name": "Joint", "access": "write"},
                ],
            }],
        }),
    ]
}

/// The diagnostics latch key for a report about `entity`: `"{name}@{id}"` when it has a [`Name`],
/// else its id. The id alone keeps two entities of one world apart (the generation makes it unique),
/// but a district switch carries the latch onto a fresh world that reuses ids from `e0v0` — the name
/// ties a report to the authored entity across that boundary, and keeps two same-named entities
/// distinct.
fn latch_key(world: &World, entity: EntityId) -> String {
    match world.get::<Name>(entity) {
        Ok(Some(name)) => format!("{}@{entity}", name.0),
        _ => entity.to_string(),
    }
}

/// Drains `ludion-physics`'s queued shape rejections and reports each once through
/// [`crate::diagnostics::report`] — the seam this module exists for, since `ludion-physics` cannot
/// see the engine's `Diagnostics` resource (it does not depend on the engine). A no-op on a world
/// with no [`PhysicsWorld`] resource, which is every world until a collider is first claimed.
///
/// The latch key is the entity's [`Name`] plus its id ([`latch_key`]), so two entities sharing a
/// name (or having none) latch independently; the message names the entity by its [`Name`] when it
/// has one, else by that id, so it reads the way a level designer would recognize the offender.
pub(crate) fn report_rejections(world: &mut World) {
    let Some(physics) = world.resource_mut::<PhysicsWorld>() else {
        return;
    };
    let rejections = physics.take_rejections();
    for rejection in rejections {
        let named = entity_label(world, rejection.entity);
        let retried_on = if rejection.code == code::COLLIDER_LAYER_UNKNOWN {
            "its Collider or the scene's physics.layers changes"
        } else if rejection.code == code::COLLIDER_MESH_UNAVAILABLE {
            "its Collider or the mesh document changes (picked up at the next load or restore)"
        } else {
            "its Collider changes"
        };
        let message = format!(
            "{named}: {}; it gets no rigid body until {retried_on}",
            rejection.reason
        );
        let diagnostic = rejection.related.into_iter().fold(
            Diagnostic::new(rejection.code, message),
            Diagnostic::with_related,
        );
        let key = latch_key(world, rejection.entity);
        crate::diagnostics::report_full(world, key, diagnostic);
    }
}

/// Reports a solver-claimed collider under a live `Parent` once: a rigid body (`L1221`) or a static
/// collider (`L1222`) gets no body, and a kinematic body under a sheared chain (`L1223`) is posed from
/// the lossy world pose. Runs whether or not a `physics:` block is installed; costs one empty join
/// when nothing is parented.
pub(crate) fn report_parented_bodies(world: &mut World) {
    let ids: Vec<EntityId> = world
        .query_ids::<(&Collider, &Parent)>()
        .filter(|(_, (collider, _))| ludion_physics::claims(collider))
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        let Ok(Some(collider)) = world.get::<Collider>(id) else {
            continue;
        };
        let diagnostic = match collider.body {
            Body::Rigid(_) => code::HIERARCHY_RIGID_BODY,
            Body::Static(_) => code::HIERARCHY_STATIC_COLLIDER,
            Body::Kinematic(_) => code::HIERARCHY_KINEMATIC_SHEAR,
        };
        let key = latch_key(world, id);
        if crate::diagnostics::is_latched(world, diagnostic, &key) {
            continue;
        }
        let Some(pose) = crate::hierarchy::parented_world_pose(world, id) else {
            continue;
        };
        let named = entity_label(world, id);
        let message = match diagnostic {
            code::HIERARCHY_RIGID_BODY => {
                format!(
                    "{named} is a rigid body under a Parent; it gets no body until it is un-parented"
                )
            }
            code::HIERARCHY_STATIC_COLLIDER => format!(
                "{named} is a static collider under a Parent; it gets no body until it is \
                 un-parented or made kinematic"
            ),
            _ if pose.sheared => format!(
                "{named} is a kinematic body under a non-uniformly scaled, rotated \
                 ancestor; the solver gets the pose with its shear dropped"
            ),
            _ => continue,
        };
        crate::diagnostics::report(world, diagnostic, key, message);
    }
}

/// Reports `L1290` once per solver-owned collider whose world scale (its own times its ancestors') is
/// not `[1, 1, 1]`: collision ignores `Transform.scale`, so the solver sees an unscaled shape.
pub(crate) fn report_scaled_colliders(world: &mut World) {
    const UNIT: [f32; 3] = [1.0; 3];
    let ids: Vec<(EntityId, [f32; 3])> = world
        .query_ids::<(&Collider, &Transform)>()
        .filter(|(_, (collider, _))| ludion_physics::claims(collider))
        .map(|(id, (_, transform))| (id, transform.scale))
        .collect();
    for (id, own) in ids {
        let key = latch_key(world, id);
        if crate::diagnostics::is_latched(world, code::COLLIDER_SCALE_IGNORED, &key) {
            continue;
        }
        let scale = if own != UNIT {
            own
        } else if world.has::<Parent>(id) {
            match crate::hierarchy::world_transform(world, id) {
                Some(pose) if pose.trs.scale != UNIT => pose.trs.scale,
                _ => continue,
            }
        } else {
            continue;
        };
        let named = entity_label(world, id);
        crate::diagnostics::report(
            world,
            code::COLLIDER_SCALE_IGNORED,
            key,
            format!(
                "{named} is a collider with a world scale of {scale:?}; scale is not applied to \
                 collision; bake it into the mesh or shape"
            ),
        );
    }
}

/// The inputs of the last [`report_unrun_colliders`] scan: collider rows, physics step installed,
/// game systems present, `SpatialIndex` present.
struct UnrunScan((usize, bool, bool, bool));

/// Reports a collider nothing will run once, per cause: a solver collider in a world whose schedule
/// has no physics step (`L1261`), and a 2D collider in a world with game systems that never built a
/// `SpatialIndex` (`L1262`). A bare host's empty schedule is not a game that forgot a system, so
/// `game_systems` gates the second. A healthy world never reports, so the scan over every collider
/// reruns only when the `Collider` column's row count or an input of the checks changes; a
/// collider swapped for another in one tick, leaving the count unchanged, is caught at the next
/// change. Reported causes cost a latch probe.
pub(crate) fn report_unrun_colliders(
    world: &mut World,
    physics_installed: bool,
    game_systems: bool,
) {
    let key = (
        world.component_count::<Collider>(),
        physics_installed,
        game_systems,
        world.resource::<ludion_spatial::SpatialIndex>().is_some(),
    );
    if world
        .resource::<UnrunScan>()
        .is_some_and(|last| last.0 == key)
    {
        return;
    }
    world.insert_resource(UnrunScan(key));
    if !physics_installed
        && !crate::diagnostics::is_latched(world, code::COLLIDER_NO_PHYSICS_SCENE, "world")
    {
        let solver = world
            .query_ids::<(&Collider,)>()
            .find(|(_, (collider,))| collider.is_solver_owned())
            .map(|(id, _)| id);
        if let Some(id) = solver {
            let named = entity_label(world, id);
            crate::diagnostics::report(
                world,
                code::COLLIDER_NO_PHYSICS_SCENE,
                "world",
                format!(
                    "{named} has a 3D shape or a rigid body, but this world has no physics step, so \
                     nothing simulates it: add `physics: {{ space: free }}` to the scene"
                ),
            );
        }
    }
    if game_systems
        && world.resource::<ludion_spatial::SpatialIndex>().is_none()
        && !crate::diagnostics::is_latched(world, code::COLLIDER_NO_SPATIAL_INDEX, "world")
    {
        let flat = world
            .query_ids::<(&Collider,)>()
            .find(|(_, (collider,))| !collider.is_solver_owned())
            .map(|(id, _)| id);
        if let Some(id) = flat {
            let named = entity_label(world, id);
            crate::diagnostics::report(
                world,
                code::COLLIDER_NO_SPATIAL_INDEX,
                "world",
                format!(
                    "{named} has a 2D collider, but no system builds the `SpatialIndex`, so queries \
                     and `move_and_slide` never see it: register \
                     `.system(\"spatial_rebuild\", spatial::rebuild::<Transform>)` on the \
                     `EngineBuilder` before the systems that query"
                ),
            );
        }
    }
}

/// `entity \`name\` (id)` when the entity is named, else `entity \`id\``.
pub(crate) fn entity_label(world: &World, id: EntityId) -> String {
    match world.get::<Name>(id) {
        Ok(Some(name)) => format!("entity `{}` ({id})", name.0),
        _ => format!("entity `{id}`"),
    }
}

/// Drains `ludion-physics`'s queued missing-joint carriers and reports each once as `L3035` — the
/// [`report_rejections`] seam, for a joint whose `a` or `b` is dead or has no body instead of a
/// rejected shape. A no-op on a world with no [`PhysicsWorld`] resource.
///
/// The latch key is the carrier's [`latch_key`], the [`report_rejections`] precedent.
pub(crate) fn report_missing_joints(world: &mut World) {
    let Some(physics) = world.resource_mut::<PhysicsWorld>() else {
        return;
    };
    let carriers = physics.take_missing_joints();
    for carrier in carriers {
        let named = entity_label(world, carrier);
        let offender = world
            .get::<Joint>(carrier)
            .ok()
            .flatten()
            .and_then(|joint| missing_body_offender(world, joint));
        let message = match offender {
            Some((side, entity, true)) => format!(
                "{named}: its joint's `{side}` ({entity}) is dead; it stays inert until both bodies \
                 exist — edit this Joint to name a live body, or despawn the carrier too if the \
                 figure is meant to stay dismembered"
            ),
            Some((side, entity, false)) => format!(
                "{named}: its joint's `{side}` ({entity}) is alive but has no rigid, kinematic or \
                 static body yet (a rejected shape, or a `Collider` never claimed); it stays inert \
                 until both bodies exist — give `{side}` a `Collider` the solver can build a body \
                 from, or wait for the pending claim"
            ),
            // Both named bodies look fine right now — a game system repaired one in the same tick
            // this carrier was queued, before this drain ran. Falls back to the old, unspecific
            // wording rather than claiming something that is no longer true.
            None => format!(
                "{named}: its joint's `a` or `b` is dead or has no body; it stays inert until both \
                 bodies exist"
            ),
        };
        let key = latch_key(world, carrier);
        crate::diagnostics::report(world, code::JOINT_BODY_MISSING, key, message);
    }
}

/// Drains `ludion-physics`'s queued motor mismatches and reports each once as `L3037` — the
/// [`report_missing_joints`] seam, for a `JointMotor` that cannot drive its carrier's `Joint`. A
/// no-op on a world with no [`PhysicsWorld`] resource.
///
/// The latch key is the carrier's [`latch_key`], the [`report_rejections`] precedent.
pub(crate) fn report_motor_mismatches(world: &mut World) {
    let Some(physics) = world.resource_mut::<PhysicsWorld>() else {
        return;
    };
    let mismatches = physics.take_motor_mismatches();
    for mismatch in mismatches {
        let named = entity_label(world, mismatch.carrier);
        let message = format!(
            "{named}: {}; the joint stays passive until its JointMotor or Joint changes",
            mismatch.reason
        );
        let key = latch_key(world, mismatch.carrier);
        crate::diagnostics::report(world, code::JOINT_MOTOR_MISMATCH, key, message);
    }
}

/// Drains `ludion-physics`'s dropped at-centre and at-point impulses and reports each entity once
/// as `L3038` — the [`report_motor_mismatches`] seam, for an impulse whose target has no rigid
/// body. A no-op on a world with no [`PhysicsWorld`] resource.
///
/// The latch key is the target entity's [`latch_key`], the [`report_rejections`] precedent.
pub(crate) fn report_impulse_misses(world: &mut World) {
    let Some(physics) = world.resource_mut::<PhysicsWorld>() else {
        return;
    };
    let misses = physics.take_impulse_misses();
    for miss in misses {
        let named = entity_label(world, miss.entity);
        let message = format!("{named}: {}; the impulse was dropped", miss.reason);
        let key = latch_key(world, miss.entity);
        crate::diagnostics::report(world, code::IMPULSE_TARGET_INVALID, key, message);
    }
}

/// Which of `joint`'s `a`/`b` is missing a body, its id, and whether it is dead (`true`) or merely
/// alive with nothing claimed yet (`false`) — `None` if neither is missing, so the caller falls back
/// to the unspecific message. Dead is checked first: a despawned entity can carry no `Collider`
/// either, so the more specific fault wins.
fn missing_body_offender(world: &World, joint: &Joint) -> Option<(&'static str, EntityId, bool)> {
    for (side, entity) in [("a", joint.a), ("b", joint.b)] {
        if !world.is_alive(entity) {
            return Some((side, entity, true));
        }
    }
    for (side, entity) in [("a", joint.a), ("b", joint.b)] {
        if world.get::<Collider>(entity).ok().flatten().is_none() {
            return Some((side, entity, false));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Diagnostics;
    use ludion_core::codes::code;
    use ludion_core::time::{FrameIndex, TickRate};
    use ludion_ecs::SystemCtx;
    use ludion_spatial::{Body, Collider, JointKind, RigidBody, Shape};

    #[test]
    fn the_latch_key_pairs_a_name_with_the_id() {
        let mut world = World::new();
        let named = world.spawn();
        world
            .insert(named, Name("crate".to_string()))
            .expect("a fresh entity accepts a name");
        let anonymous = world.spawn();

        assert_eq!(latch_key(&world, named), format!("crate@{named}"));
        assert_eq!(latch_key(&world, anonymous), anonymous.to_string());

        // A fresh world reuses the first id: only the name tells the two authored entities apart.
        let mut other = World::new();
        let first = other.spawn();
        other
            .insert(first, Name("door".to_string()))
            .expect("a fresh entity accepts a name");
        assert_ne!(latch_key(&world, named), latch_key(&other, first));
    }

    #[test]
    fn a_rejected_shape_reports_once_naming_the_entity() {
        let mut world = World::new();
        world.insert_resource(Diagnostics::default());
        let entity = world.spawn();
        world
            .insert(entity, Transform::default())
            .expect("a fresh entity accepts a pose");
        world
            .insert(entity, Name("bad-crate".to_string()))
            .expect("a fresh entity accepts a name");
        world
            .insert(
                entity,
                // Two points do not bound a volume, so the solver rejects the hull — inserted
                // directly, bypassing `Collider::validate_semantics`, the reachable path.
                Collider {
                    shape: Shape::Convex {
                        points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                    },
                    body: Body::Rigid(RigidBody::default()),
                    layer: None,
                    sensor: false,
                },
            )
            .expect("a fresh entity accepts a collider");

        let mut schedule = Schedule::new();
        ludion_physics::install::<Transform>(&mut schedule, PhysicsSpace::Free);

        for frame in 0..3 {
            schedule.run(
                &mut world,
                &SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT),
            );
            report_rejections(&mut world);
        }

        let reported = world
            .resource_mut::<Diagnostics>()
            .expect("inserted above")
            .take();
        assert_eq!(
            reported.len(),
            1,
            "one rejection over three ticks of the same bad shape: {reported:?}"
        );
        let (reported_code, message) = (&reported[0].code, &reported[0].message);
        assert_eq!(*reported_code, code::COLLIDER_DEGENERATE_HULL);
        assert!(
            message.contains("bad-crate"),
            "the message must name the entity: {message}"
        );
    }

    fn physics_with_layers(layers: Vec<ludion_scene::SceneLayer>) -> ScenePhysics {
        ScenePhysics {
            space: ScenePhysicsSpace::Free,
            gravity: None,
            solver_iterations: None,
            stabilization_iterations: None,
            sleep_after_ticks: None,
            layers,
        }
    }

    fn layer(name: &str, ignores: &[&str]) -> ludion_scene::SceneLayer {
        ludion_scene::SceneLayer {
            name: name.to_string(),
            ignores: ignores.iter().map(|ignored| ignored.to_string()).collect(),
        }
    }

    #[test]
    fn a_scene_without_layers_maps_to_no_table() {
        assert_eq!(settings(&physics_with_layers(Vec::new())).layers, None);
    }

    #[test]
    fn declared_layers_map_to_names_and_symmetric_masks() {
        let mapped = settings(&physics_with_layers(vec![
            layer("dummy", &["dummy"]),
            layer("props", &["default"]),
        ]))
        .layers
        .expect("layers were declared");
        assert_eq!(mapped.names(), ["default", "dummy", "props"]);
        // `dummy` ignores itself; `props` ignores `default`, and so `default` ignores `props`.
        assert_eq!(
            mapped,
            CollisionLayers::new(
                vec!["default".into(), "dummy".into(), "props".into()],
                vec![0b100, 0b010, 0b001],
            )
        );
    }

    #[test]
    fn parented_bodies_report_once_each_by_kind() {
        let mut world = World::new();
        world.insert_resource(Diagnostics::default());
        let parent = world.spawn();
        world
            .insert(
                parent,
                Transform {
                    scale: [2.0, 1.0, 1.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        let mut child = |body: Body, rotation: [f32; 3]| {
            let id = world.spawn();
            world
                .insert(
                    id,
                    Transform {
                        rotation,
                        ..Transform::default()
                    },
                )
                .unwrap();
            world
                .insert(
                    id,
                    Collider {
                        shape: Shape::Box {
                            half_extents: [0.5; 3],
                        },
                        body,
                        layer: None,
                        sensor: false,
                    },
                )
                .unwrap();
            world.insert(id, Parent(parent)).unwrap();
            id
        };
        child(Body::Rigid(RigidBody::default()), [0.0; 3]);
        child(Body::new_static(), [0.0; 3]);
        child(Body::new_kinematic(), [0.0; 3]);
        child(Body::new_kinematic(), [0.0, 0.0, 30.0]);

        for _ in 0..3 {
            report_parented_bodies(&mut world);
        }
        let reported = world.resource_mut::<Diagnostics>().unwrap().take();
        let codes: Vec<_> = reported.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            [
                code::HIERARCHY_RIGID_BODY,
                code::HIERARCHY_STATIC_COLLIDER,
                code::HIERARCHY_KINEMATIC_SHEAR
            ],
            "{reported:?}"
        );
    }

    #[test]
    fn scaled_colliders_report_once_each() {
        let mut world = World::new();
        world.insert_resource(Diagnostics::default());
        let collider = |body: Body| Collider {
            shape: Shape::Box {
                half_extents: [0.5; 3],
            },
            body,
            layer: None,
            sensor: false,
        };
        let scaled = world.spawn();
        world
            .insert(
                scaled,
                Transform {
                    scale: [2.0, 1.0, 1.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        world.insert(scaled, collider(Body::new_static())).unwrap();
        let parent = world.spawn();
        world
            .insert(
                parent,
                Transform {
                    scale: [3.0; 3],
                    ..Transform::default()
                },
            )
            .unwrap();
        let child = world.spawn();
        world.insert(child, Transform::default()).unwrap();
        world
            .insert(child, collider(Body::new_kinematic()))
            .unwrap();
        world.insert(child, Parent(parent)).unwrap();
        let unit = world.spawn();
        world.insert(unit, Transform::default()).unwrap();
        world.insert(unit, collider(Body::new_static())).unwrap();

        for _ in 0..2 {
            report_scaled_colliders(&mut world);
        }
        let reported = world.resource_mut::<Diagnostics>().unwrap().take();
        let codes: Vec<_> = reported.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            [code::COLLIDER_SCALE_IGNORED, code::COLLIDER_SCALE_IGNORED],
            "{reported:?}"
        );
    }

    fn world_with_collider(shape: Shape, body: Body) -> World {
        let mut world = World::new();
        world.insert_resource(Diagnostics::default());
        let id = world.spawn();
        world
            .insert(
                id,
                Collider {
                    shape,
                    body,
                    layer: None,
                    sensor: false,
                },
            )
            .unwrap();
        world
    }

    fn reported_codes(world: &mut World) -> Vec<ludion_core::diagnostics::DiagnosticCode> {
        let reported = world.resource_mut::<Diagnostics>().unwrap().take();
        reported.iter().map(|d| d.code).collect()
    }

    #[test]
    fn a_solver_collider_without_a_physics_step_reports_l1261_once() {
        let mut world = world_with_collider(
            Shape::Sphere { radius: 0.5 },
            Body::Rigid(RigidBody::default()),
        );
        for _ in 0..3 {
            report_unrun_colliders(&mut world, false, false);
        }
        assert_eq!(
            reported_codes(&mut world),
            [code::COLLIDER_NO_PHYSICS_SCENE]
        );
        let mut installed = world_with_collider(
            Shape::Sphere { radius: 0.5 },
            Body::Rigid(RigidBody::default()),
        );
        report_unrun_colliders(&mut installed, true, false);
        assert!(reported_codes(&mut installed).is_empty());
    }

    #[test]
    fn a_2d_collider_without_a_spatial_index_reports_l1262_once() {
        let mut world = world_with_collider(Shape::Circle { radius: 0.5 }, Body::new_static());
        for _ in 0..3 {
            report_unrun_colliders(&mut world, false, true);
        }
        assert_eq!(
            reported_codes(&mut world),
            [code::COLLIDER_NO_SPATIAL_INDEX]
        );

        let mut bare = world_with_collider(Shape::Circle { radius: 0.5 }, Body::new_static());
        report_unrun_colliders(&mut bare, false, false);
        assert!(reported_codes(&mut bare).is_empty(), "no game systems");

        let mut indexed = world_with_collider(Shape::Circle { radius: 0.5 }, Body::new_static());
        indexed.insert_resource(ludion_spatial::SpatialIndex::default());
        report_unrun_colliders(&mut indexed, false, true);
        assert!(reported_codes(&mut indexed).is_empty());
    }

    #[test]
    fn an_undeclared_layer_reports_once_naming_the_entity_and_the_nearest_layer() {
        let mut world = World::new();
        world.insert_resource(Diagnostics::default());
        let entity = world.spawn();
        world
            .insert(entity, Transform::default())
            .expect("a fresh entity accepts a pose");
        world
            .insert(entity, Name("torso".to_string()))
            .expect("a fresh entity accepts a name");
        world
            .insert(
                entity,
                Collider {
                    shape: Shape::Sphere { radius: 0.5 },
                    body: Body::Rigid(RigidBody::default()),
                    layer: Some("dumy".to_string()),
                    sensor: false,
                },
            )
            .expect("a fresh entity accepts a collider");

        let mut schedule = Schedule::new();
        ludion_physics::install::<Transform>(
            &mut schedule,
            settings(&physics_with_layers(vec![layer("dummy", &["dummy"])])),
        );
        for frame in 0..3 {
            schedule.run(
                &mut world,
                &SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT),
            );
            report_rejections(&mut world);
        }

        let reported = world
            .resource_mut::<Diagnostics>()
            .expect("inserted above")
            .take();
        assert_eq!(reported.len(), 1, "{reported:?}");
        let (reported_code, message) = (&reported[0].code, &reported[0].message);
        assert_eq!(*reported_code, code::COLLIDER_LAYER_UNKNOWN);
        assert!(message.contains("torso"), "{message}");
        assert!(message.contains("dummy"), "{message}");
        assert!(
            message.ends_with("until its Collider or the scene's physics.layers changes"),
            "{message}"
        );
    }

    /// The rejection latch is keyed by entity id, not by [`Name`], so two entities sharing a name
    /// each report once instead of one shadowing the other.
    #[test]
    fn two_entities_sharing_a_name_each_report_their_own_rejection() {
        let mut world = World::new();
        world.insert_resource(Diagnostics::default());
        let degenerate = || Collider {
            shape: Shape::Convex {
                points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            },
            body: Body::Rigid(RigidBody::default()),
            layer: None,
            sensor: false,
        };
        let mut entities = Vec::new();
        for _ in 0..2 {
            let entity = world.spawn();
            world
                .insert(entity, Transform::default())
                .expect("a fresh entity accepts a pose");
            world
                .insert(entity, Name("twin".to_string()))
                .expect("a fresh entity accepts a name");
            world
                .insert(entity, degenerate())
                .expect("a fresh entity accepts a collider");
            entities.push(entity);
        }

        let mut schedule = Schedule::new();
        ludion_physics::install::<Transform>(&mut schedule, PhysicsSpace::Free);
        schedule.run(
            &mut world,
            &SystemCtx::new(FrameIndex::new(0), TickRate::DEFAULT),
        );
        report_rejections(&mut world);

        let reported = world
            .resource_mut::<Diagnostics>()
            .expect("inserted above")
            .take();
        assert_eq!(
            reported.len(),
            2,
            "two distinct entities named `twin` must each latch independently: {reported:?}"
        );
        for entity in entities {
            let id = entity.to_string();
            assert!(
                reported
                    .iter()
                    .any(|d| d.message.contains("twin") && d.message.contains(&id)),
                "expected a message naming `twin` and {id}: {reported:?}"
            );
        }
    }

    /// `L3035`'s message names which of `a`/`b` went missing, its id, and whether it is dead: a
    /// despawned `b` reports itself, not the
    /// old unspecific "`a` or `b`" wording.
    #[test]
    fn a_missing_joint_body_names_which_side_and_that_it_is_dead() {
        let mut world = World::new();
        world.insert_resource(Diagnostics::default());

        let anchor = world.spawn();
        world
            .insert(anchor, Transform::default())
            .expect("a fresh entity accepts a pose");
        world
            .insert(
                anchor,
                Collider {
                    shape: Shape::Sphere { radius: 0.1 },
                    body: Body::new_static(),
                    layer: None,
                    sensor: false,
                },
            )
            .expect("a fresh entity accepts a collider");

        let bob = world.spawn();
        world
            .insert(bob, Transform::default())
            .expect("a fresh entity accepts a pose");
        world
            .insert(
                bob,
                Collider {
                    shape: Shape::Sphere { radius: 0.1 },
                    body: Body::Rigid(RigidBody::default()),
                    layer: None,
                    sensor: false,
                },
            )
            .expect("a fresh entity accepts a collider");

        let carrier = world.spawn();
        world
            .insert(
                carrier,
                Joint {
                    a: anchor,
                    b: bob,
                    kind: JointKind::Fixed {},
                    anchor: [0.0; 3],
                    break_impulse: None,
                    collide: false,
                },
            )
            .expect("a fresh entity accepts a joint");

        let mut schedule = Schedule::new();
        ludion_physics::install::<Transform>(&mut schedule, PhysicsSpace::Free);
        schedule.run(
            &mut world,
            &SystemCtx::new(FrameIndex::new(0), TickRate::DEFAULT),
        );

        world.despawn(bob).expect("a live entity despawns");
        schedule.run(
            &mut world,
            &SystemCtx::new(FrameIndex::new(1), TickRate::DEFAULT),
        );
        report_missing_joints(&mut world);

        let reported = world
            .resource_mut::<Diagnostics>()
            .expect("inserted above")
            .take();
        assert_eq!(reported.len(), 1, "{reported:?}");
        let (reported_code, message) = (&reported[0].code, &reported[0].message);
        assert_eq!(*reported_code, code::JOINT_BODY_MISSING);
        assert!(message.contains(&format!("`b` ({bob})")), "{message}");
        assert!(message.contains("is dead"), "{message}");
    }

    /// A `space: planar` scene must simulate the same under a bare host (`bare_driver`,
    /// what `render`/`sweep` build) as under a full `Engine` — the off-plane axis is locked from
    /// [`ludion_spatial::Plane::of`]'s default, not from a host installing a `Plane` resource.
    ///
    /// Compares the rigid box's `Transform` per tick rather than the full `WorldHash`: the engine
    /// projects each entity's `name:` into a `Name` component (`instantiate_named_world`) that a
    /// bare host's raw `ludion_scene::instantiate` never adds, which would move the hash for a
    /// reason that has nothing to do with physics. The box is entity index 1 (the floor is 0) — a
    /// fresh world's slot order is scene document order on both sides, so the same `EntityId`
    /// names the same body in both.
    #[test]
    fn a_planar_scene_simulates_the_same_under_bare_and_engine_hosts() {
        use crate::{Engine, builtin_registry};
        use ludion_core::ids::EntityId;
        use std::collections::BTreeSet;

        const TICKS: usize = 120;
        let mut text = String::from(
            "format: 1\nseed: 7\nphysics:\n  space: planar\n  gravity:\n    - 2.0\n    - -9.81\n    - 0.0\nentities:\n",
        );
        text.push_str("  - name: floor\n    components:\n      Transform:\n");
        text.push_str("        translation:\n          - 0.0\n          - -0.5\n          - 0.0\n");
        text.push_str("        rotation:\n          - 0.0\n          - 0.0\n          - 0.0\n");
        text.push_str(
            "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 20.0\n              - 0.5\n              - 20.0\n        body: static\n",
        );
        text.push_str("  - name: box\n    components:\n      Transform:\n");
        text.push_str("        translation:\n          - 0.0\n          - 2.0\n          - 0.0\n");
        text.push_str("        rotation:\n          - 0.0\n          - 0.0\n          - 0.0\n");
        text.push_str(
            "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 0.5\n              - 0.5\n              - 0.5\n",
        );
        text.push_str(
            "        body:\n          rigid:\n            mass: 1.0\n            friction: 0.5\n            restitution: 0.25\n      Velocity: {}\n",
        );

        let registry = builtin_registry();
        let parsed = ludion_scene::check(&text, &registry).expect("scene checks");
        let box_entity = EntityId::new(1, 0);

        let dir = std::env::temp_dir().join(format!(
            "ludion-scene-physics-bare-vs-engine-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut engine = Engine::builder()
            .project_root(dir)
            .build()
            .expect("engine boots");
        engine.load_inline_scene(&text).unwrap();
        let mut engine_trace = Vec::with_capacity(TICKS);
        for _ in 0..TICKS {
            engine.advance_live();
            let transform = engine
                .world()
                .get::<Transform>(box_entity)
                .expect("a live entity")
                .copied()
                .expect("the box has a Transform");
            engine_trace.push(transform);
        }

        let world = ludion_scene::instantiate(&parsed, &registry).expect("instantiates");
        let mut driver = bare_driver(world, &parsed, std::path::Path::new("."));
        let mut bare_trace = Vec::with_capacity(TICKS);
        for _ in 0..TICKS {
            driver.run_ticks(1);
            let transform = driver
                .world()
                .get::<Transform>(box_entity)
                .expect("a live entity")
                .copied()
                .expect("the box has a Transform");
            bare_trace.push(transform);
        }

        let distinct: BTreeSet<[u32; 3]> = engine_trace
            .iter()
            .map(|t| t.translation.map(f32::to_bits))
            .collect();
        assert!(
            distinct.len() >= 100,
            "trace too flat to prove anything: {} distinct of {TICKS}",
            distinct.len()
        );
        assert!(
            bare_trace
                .iter()
                .all(|t| t.translation[1] == bare_trace[0].translation[1]),
            "a planar scene's off-plane axis must stay locked on a bare host: {:?}",
            bare_trace
                .iter()
                .map(|t| t.translation[1])
                .collect::<Vec<_>>()
        );

        assert_eq!(
            engine_trace, bare_trace,
            "a planar scene's rigid box must simulate the same under bare and engine hosts"
        );
    }
}
