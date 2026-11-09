//! The character motor on the engine's [`Transform`]: `ludion_physics::move_character` bound to
//! the pose component, which is what a game calls. The move wrappers and the ground probe draw
//! the `motor` debug category after each successful call.

use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_physics::{
    CharacterMotor, CharacterMotorState, GroundProbe, MotorError, MoveConstraint, MoveReport,
    ResizeReport, ground_probe_reach,
};

use crate::components::Transform;
use crate::debug_draw::debug_draw;
use crate::hierarchy::TransformHierarchy;

const CATEGORY: &str = "motor";
/// The capsule while the motor stands on walkable ground (linear RGBA).
pub const MOTOR_GROUNDED: [f32; 4] = [0.1, 0.9, 0.2, 1.0];
/// The capsule while the motor is in the air.
pub const MOTOR_AIRBORNE: [f32; 4] = [1.0, 0.5, 0.05, 1.0];
/// The capsule while the motor is crushed (`stuck`); wins over grounded and airborne.
pub const MOTOR_STUCK: [f32; 4] = [1.0, 0.05, 0.05, 1.0];
/// The ground's surface normal at the contact point.
pub const MOTOR_GROUND_NORMAL: [f32; 4] = [0.1, 0.9, 0.9, 1.0];
/// A wall's surface normal at the contact point.
pub const MOTOR_WALL: [f32; 4] = [0.9, 0.2, 0.9, 1.0];
/// The ceiling's contact cross and surface normal.
pub const MOTOR_CEILING: [f32; 4] = [1.0, 0.9, 0.1, 1.0];
/// The platform carry (translation plus surface displacement), from the feet.
pub const MOTOR_CARRY: [f32; 4] = [0.2, 0.3, 1.0, 1.0];
/// The ground probe: the line the snap looks along below the feet; `probe_character_ground` draws
/// its own, from the step height above the probed feet.
pub const MOTOR_PROBE: [f32; 4] = [0.8, 0.8, 0.8, 1.0];
/// Length of the drawn surface normals, in metres.
const NORMAL_LENGTH: f32 = 0.25;
const CEILING_CROSS: f32 = 0.05;

/// Moves `entity`'s feet by `displacement` through the physics world; see
/// [`ludion_physics::move_character`] for the algorithm and the call-ordering rule (call it from
/// a system registered before the physics systems).
///
/// ```no_run
/// # use ludion_core::ids::EntityId;
/// # use ludion_engine::move_character;
/// # use ludion_ecs::{SystemCtx, World};
/// fn walk(world: &mut World, _ctx: &SystemCtx, hero: EntityId) {
///     let _ = move_character(world, hero, [0.05, -0.2, 0.0]);
/// }
/// ```
///
/// An entity with an engaged (or still easing) `LaneConstraint` is held to its lane: horizontal
/// motion is projected onto the lane and a capped pull returns the feet to its centre line, eased by
/// the lane's `blend`; `MoveReport.lane` describes it. With no lane the call is the unconstrained
/// move, bit for bit. A lane call also draws the `lane` debug category: the path polyline (alpha is
/// the weight), the frame's anchor and axes, and an error-coloured line from the feet to the anchor.
///
/// After a successful call it draws the `motor` debug category: the capsule (green grounded, orange
/// airborne, red stuck), the ground, wall and ceiling contact normals, the platform carry and the
/// ground probe. The drawing is never hashed.
pub fn move_character(
    world: &mut World,
    entity: EntityId,
    displacement: [f32; 3],
) -> Result<MoveReport, MotorError> {
    if let Some(result) = crate::lane::move_on_lane(world, entity, displacement) {
        return result;
    }
    move_character_with(world, entity, displacement, None)
}

/// [`move_character`] with a [`MoveConstraint`] shaping every horizontal displacement and adding
/// its correction. The explicit constraint is the only one: a `LaneConstraint` on the entity is
/// ignored here.
pub fn move_character_with(
    world: &mut World,
    entity: EntityId,
    displacement: [f32; 3],
    constraint: Option<&mut dyn MoveConstraint>,
) -> Result<MoveReport, MotorError> {
    let was_grounded = world
        .get::<CharacterMotorState>(entity)
        .ok()
        .flatten()
        .is_some_and(|state| state.grounded);
    let report = ludion_physics::move_character_with::<Transform, TransformHierarchy>(
        world,
        entity,
        displacement,
        constraint,
    )?;
    draw(world, entity, was_grounded, displacement, &report);
    Ok(report)
}

/// Moves many motors in one call, their casts run in parallel; see
/// [`ludion_physics::move_characters`] for the carry rule (every support pose is the tick-start
/// pose, so a motor riding a motor is carried by its support's previous-tick motion), the serial
/// equivalence and the duplicate rule. `out[i]` is the result for `moves[i]`.
///
/// An entity whose `LaneConstraint` is gated active (a path and a non-zero `blend` or `engaged`,
/// even if the path fails to load) is not moved: its slot is [`MotorError::LaneInBatch`]; call [`move_character`] for it. Like
/// `move_character`, each moved motor draws the `motor` debug category, in slice order.
///
/// ```no_run
/// # use ludion_core::ids::EntityId;
/// # use ludion_engine::move_characters;
/// # use ludion_physics::MotorError;
/// # use ludion_ecs::{SystemCtx, World};
/// fn walk(world: &mut World, _ctx: &SystemCtx, ids: &[EntityId]) {
///     let moves: Vec<_> = ids.iter().map(|id| (*id, [0.05, -0.2, 0.0])).collect();
///     let mut out = vec![Err(MotorError::NoPhysicsWorld); moves.len()];
///     move_characters(world, &moves, &mut out);
/// }
/// ```
///
/// # Panics
/// Panics if `moves` and `out` differ in length.
pub fn move_characters(
    world: &mut World,
    moves: &[(EntityId, [f32; 3])],
    out: &mut [Result<MoveReport, MotorError>],
) {
    assert_eq!(
        moves.len(),
        out.len(),
        "move_characters: {} moves but {} output slots",
        moves.len(),
        out.len()
    );
    let grounded: Vec<bool> = moves
        .iter()
        .map(|(entity, _)| {
            world
                .get::<CharacterMotorState>(*entity)
                .ok()
                .flatten()
                .is_some_and(|state| state.grounded)
        })
        .collect();
    let mut index = Vec::with_capacity(moves.len());
    let mut sub = Vec::with_capacity(moves.len());
    for (i, &(entity, displacement)) in moves.iter().enumerate() {
        if crate::lane::lane_active(world, entity) {
            out[i] = Err(MotorError::LaneInBatch(entity));
        } else {
            index.push(i);
            sub.push((entity, displacement));
        }
    }
    let mut results = vec![Err(MotorError::NoPhysicsWorld); sub.len()];
    ludion_physics::move_characters::<Transform, TransformHierarchy>(world, &sub, &mut results);
    for (slot, result) in index.into_iter().zip(results) {
        if let Ok(report) = &result {
            let (entity, displacement) = moves[slot];
            draw(world, entity, grounded[slot], displacement, report);
        }
        out[slot] = result;
    }
}

/// Sets the capsule height in force; see [`ludion_physics::resize_character`].
///
/// ```no_run
/// # use ludion_core::ids::EntityId;
/// # use ludion_engine::resize_character;
/// # use ludion_ecs::{SystemCtx, World};
/// fn crouch(world: &mut World, _ctx: &SystemCtx, hero: EntityId) {
///     let _ = resize_character(world, hero, 0.6);
/// }
/// ```
pub fn resize_character(
    world: &mut World,
    entity: EntityId,
    height: f32,
) -> Result<ResizeReport, MotorError> {
    ludion_physics::resize_character::<Transform>(world, entity, height)
}

/// Whether a capsule of `height` fits at the feet; see [`ludion_physics::fits_character`].
///
/// ```no_run
/// # use ludion_core::ids::EntityId;
/// # use ludion_engine::fits_character;
/// # use ludion_ecs::{SystemCtx, World};
/// fn can_stand(world: &mut World, _ctx: &SystemCtx, hero: EntityId) -> bool {
///     fits_character(world, hero, 0.9).unwrap_or(false)
/// }
/// ```
pub fn fits_character(world: &World, entity: EntityId, height: f32) -> Result<bool, MotorError> {
    ludion_physics::fits_character::<Transform>(world, entity, height)
}

/// The ground the motor would find with its feet at `feet`, without moving anything; see
/// [`ludion_physics::probe_character_ground`]. Draws the probe line in the `motor` category.
///
/// ```no_run
/// # use ludion_core::ids::EntityId;
/// # use ludion_engine::probe_character_ground;
/// # use ludion_ecs::{SystemCtx, World};
/// fn ledge_ahead(world: &mut World, _ctx: &SystemCtx, hero: EntityId) -> bool {
///     matches!(probe_character_ground(world, hero, [1.0, 0.0, 0.0]), Ok(None))
/// }
/// ```
pub fn probe_character_ground(
    world: &mut World,
    entity: EntityId,
    feet: [f32; 3],
) -> Result<Option<GroundProbe>, MotorError> {
    let probe = ludion_physics::probe_character_ground::<Transform>(world, entity, feet)?;
    if let Some(motor) = world.get::<CharacterMotor>(entity).ok().flatten().cloned() {
        let from = [feet[0], feet[1] + motor.step_height, feet[2]];
        let reach = ground_probe_reach(&motor, true, [0.0; 3]);
        let to = [feet[0], feet[1] - reach, feet[2]];
        debug_draw(world, CATEGORY).line(from, to, MOTOR_PROBE);
    }
    Ok(probe)
}

/// Places the feet and yaw without driving the capsule; see [`ludion_physics::teleport_character`].
/// A `LaneConstraint` on the entity re-acquires its place over the whole path (`s` is nulled,
/// `blend` and the rest are kept).
///
/// ```no_run
/// # use ludion_core::ids::EntityId;
/// # use ludion_engine::teleport_character;
/// # use ludion_ecs::{SystemCtx, World};
/// fn respawn(world: &mut World, _ctx: &SystemCtx, hero: EntityId) {
///     let _ = teleport_character(world, hero, [0.0, 0.5, 0.0], 0.0);
/// }
/// ```
pub fn teleport_character(
    world: &mut World,
    entity: EntityId,
    feet: [f32; 3],
    yaw: f32,
) -> Result<(), MotorError> {
    ludion_physics::teleport_character::<Transform>(world, entity, feet, yaw)?;
    if let Ok(Some(lane)) = world.get_mut::<crate::lane::LaneConstraint>(entity) {
        lane.s = None;
    }
    crate::interpolation::cut_presentation(world, entity);
    Ok(())
}

/// Draws one call's `motor` primitives. `was_grounded` is the state before the call: the probe's
/// reach is decided by it.
fn draw(
    world: &mut World,
    entity: EntityId,
    was_grounded: bool,
    displacement: [f32; 3],
    report: &MoveReport,
) {
    let Some(feet) = world
        .get::<Transform>(entity)
        .ok()
        .flatten()
        .map(|pose| pose.translation)
    else {
        return;
    };
    let Some(motor) = world.get::<CharacterMotor>(entity).ok().flatten().cloned() else {
        return;
    };
    let radius = motor.radius;
    let colour = if report.stuck {
        MOTOR_STUCK
    } else if report.grounded {
        MOTOR_GROUNDED
    } else {
        MOTOR_AIRBORNE
    };
    let up = |y: f32| [feet[0], feet[1] + y, feet[2]];
    let reach = ground_probe_reach(&motor, was_grounded, displacement);
    let normal = |n: [f32; 3]| n.map(|c| c * NORMAL_LENGTH);
    let mut pen = debug_draw(world, CATEGORY);
    pen.capsule(up(radius), up(report.height - radius), radius, colour);
    if let Some(ground) = report.ground {
        pen.ray(ground.point, normal(ground.normal), MOTOR_GROUND_NORMAL);
    }
    for wall in &report.walls {
        pen.ray(wall.point, normal(wall.normal), MOTOR_WALL);
    }
    if let Some(ceiling) = report.ceiling {
        pen.cross(ceiling.point, CEILING_CROSS, MOTOR_CEILING).ray(
            ceiling.point,
            normal(ceiling.normal),
            MOTOR_CEILING,
        );
    }
    if report.platform.entity.is_some() {
        let carry = report.platform;
        let to = [0, 1, 2].map(|i| carry.translation[i] + carry.surface[i]);
        pen.ray(feet, to, MOTOR_CARRY);
    }
    pen.line(feet, up(-reach), MOTOR_PROBE);
}

#[cfg(test)]
mod tests {
    use ludion_ecs::SystemCtx;
    use ludion_renderer::DebugSegment;

    use super::*;
    use crate::Engine;
    use crate::debug_draw::{DebugCategories, segments};

    fn block(v: [f32; 3], indent: &str) -> String {
        v.iter()
            .map(|c| {
                format!(
                    "
{indent}- {c:?}"
                )
            })
            .collect::<String>()
    }

    fn boxed(name: &str, centre: [f32; 3], half: [f32; 3]) -> String {
        let (centre, half) = (block(centre, "          "), block(half, "              "));
        format!(
            "  - name: {name}
    components:
      Transform:
        translation:{centre}
      Collider:
        shape:
          box:
            half_extents:{half}
        body: static
"
        )
    }

    fn scene(feet_y: f32, extra: &str) -> String {
        let feet = block([0.0, feet_y, 0.0], "          ");
        format!(
            "format: 1
seed: 5
physics:
  space: free
entities:
{}{extra}  - name: hero
    components:
      Transform:
        translation:{feet}
      CharacterMotor:
        radius: 0.28
        height: 0.9
",
            boxed("floor", [0.0, -0.5, 0.0], [20.0, 0.5, 20.0])
        )
    }

    fn hero(world: &mut World) -> EntityId {
        world
            .query_ids::<(&CharacterMotor,)>()
            .next()
            .map(|(entity, _)| entity)
            .expect("a motor")
    }

    fn fall(world: &mut World, _ctx: &SystemCtx) {
        let hero = hero(world);
        let _ = move_character(world, hero, [0.0, -0.1, 0.0]);
    }

    fn into_wall(world: &mut World, _ctx: &SystemCtx) {
        let hero = hero(world);
        let _ = move_character(world, hero, [0.05, -0.1, 0.0]);
    }

    /// The `motor` segments after `ticks` ticks of `system` on `scene`.
    fn drawn(
        test: &str,
        scene: &str,
        system: fn(&mut World, &SystemCtx),
        ticks: usize,
    ) -> Vec<DebugSegment> {
        let dir =
            std::env::temp_dir().join(format!("ludion-motor-draw-{}-{test}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut engine = Engine::builder()
            .project_root(&dir)
            .system("motor", system)
            .build()
            .expect("boots");
        engine.load_inline_scene(scene).unwrap();
        for _ in 0..ticks {
            engine.advance_live();
        }
        let drawn = segments(engine.world(), &DebugCategories::parse("motor"));
        drop(engine);
        let _ = std::fs::remove_dir_all(&dir);
        drawn
    }

    fn of(drawn: &[DebugSegment], colour: [f32; 4]) -> Vec<&DebugSegment> {
        drawn.iter().filter(|s| s.color == colour).collect()
    }

    fn length(segment: &DebugSegment) -> f32 {
        let d = [0, 1, 2].map(|i| segment.b[i] - segment.a[i]);
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }

    #[test]
    fn an_airborne_tick_draws_an_orange_capsule_and_a_two_skin_probe() {
        let drawn = drawn("airborne", &scene(2.0, ""), fall, 3);
        assert!(!of(&drawn, MOTOR_AIRBORNE).is_empty());
        assert!(of(&drawn, MOTOR_GROUNDED).is_empty());
        assert!(of(&drawn, MOTOR_GROUND_NORMAL).is_empty());
        let probe = of(&drawn, MOTOR_PROBE);
        assert_eq!(probe.len(), 1);
        assert!((length(probe[0]) - 0.02).abs() < 1e-6);
    }

    #[test]
    fn a_grounded_tick_draws_a_green_capsule_a_ground_normal_and_a_snap_probe() {
        let drawn = drawn("grounded", &scene(0.0, ""), fall, 30);
        assert!(!of(&drawn, MOTOR_GROUNDED).is_empty());
        assert!(of(&drawn, MOTOR_AIRBORNE).is_empty());
        let normals = of(&drawn, MOTOR_GROUND_NORMAL);
        assert_eq!(normals.len(), 1);
        assert!(
            (normals[0].b[1] - normals[0].a[1] - 0.25).abs() < 1e-4,
            "points up"
        );
        let probe = of(&drawn, MOTOR_PROBE);
        assert_eq!(probe.len(), 1);
        assert!((length(probe[0]) - 0.26).abs() < 1e-6);
    }

    #[test]
    fn walking_into_a_wall_draws_one_wall_ray() {
        let wall = boxed("wall", [1.5, 1.0, 0.0], [0.5, 2.0, 5.0]);
        let drawn = drawn("wall", &scene(0.0, &wall), into_wall, 40);
        assert_eq!(of(&drawn, MOTOR_WALL).len(), 1);
    }

    fn probe_at_origin(world: &mut World, _ctx: &SystemCtx) {
        let hero = hero(world);
        let _ = probe_character_ground(world, hero, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_ground_probe_draws_one_line_of_step_snap_and_skin() {
        let drawn = drawn("probe-line", &scene(0.0, ""), probe_at_origin, 3);
        let probe = of(&drawn, MOTOR_PROBE);
        assert_eq!(probe.len(), 1);
        let motor = CharacterMotor::default();
        let want = motor.step_height + motor.snap_distance + motor.skin_width;
        assert!((length(probe[0]) - want).abs() < 1e-5);
    }

    /// A `ghost` box under `[0, 5, 0]` with no floor near it; the hero's motor sits on `layer`.
    fn ghost_probe(test: &str, layer: Option<&str>) -> Option<GroundProbe> {
        let ghost = boxed("ghost", [0.0, 4.5, 0.0], [0.5, 0.5, 0.5])
            .replace("body: static\n", "body: static\n        layer: ghost\n");
        let mut text = scene(0.0, &ghost).replace(
            "  space: free\n",
            "  space: free\n  layers:\n    - name: walker\n      ignores:\n        - ghost\n    - name: ghost\n",
        );
        if let Some(layer) = layer {
            text = text.replace(
                "        height: 0.9\n",
                &format!("        height: 0.9\n        layer: {layer}\n"),
            );
        }
        let dir =
            std::env::temp_dir().join(format!("ludion-motor-probe-{}-{test}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut engine = Engine::builder()
            .project_root(&dir)
            .system("motor", fall)
            .build()
            .expect("boots");
        engine.load_inline_scene(&text).unwrap();
        for _ in 0..3 {
            engine.advance_live();
        }
        let world = engine.world_mut();
        let hero = hero(world);
        let probe = probe_character_ground(world, hero, [0.0, 5.0, 0.0]).expect("a motor");
        drop(engine);
        let _ = std::fs::remove_dir_all(&dir);
        probe
    }

    #[test]
    fn the_ground_probe_honours_the_motors_layer_filter() {
        assert!(ghost_probe("layer-filter-walker", Some("walker")).is_none());
        let hit = ghost_probe("layer-filter-none", None)
            .expect("the ghost box is ground without a layer");
        assert!(hit.rest_y.is_some());
    }

    #[test]
    fn a_crushed_motor_draws_a_red_capsule() {
        let walls = boxed("left", [0.7, 1.0, 0.0], [0.5, 2.0, 5.0])
            + &boxed("right", [-0.7, 1.0, 0.0], [0.5, 2.0, 5.0]);
        let drawn = drawn("crushed", &scene(0.0, &walls), fall, 5);
        assert!(!of(&drawn, MOTOR_STUCK).is_empty());
        assert!(of(&drawn, MOTOR_GROUNDED).is_empty());
        assert!(of(&drawn, MOTOR_AIRBORNE).is_empty());
    }

    fn mate(name: &str, x: f32) -> String {
        let feet = block([x, 0.0, 0.0], "          ");
        format!(
            "  - name: {name}
    components:
      Transform:
        translation:{feet}
      CharacterMotor:
        radius: 0.28
        height: 0.9
"
        )
    }

    fn motors(world: &mut World) -> Vec<EntityId> {
        let mut ids: Vec<_> = world
            .query_ids::<(&CharacterMotor,)>()
            .map(|(entity, _)| entity)
            .collect();
        ids.sort();
        ids
    }

    thread_local! {
        static BATCH: std::cell::RefCell<Vec<Result<MoveReport, MotorError>>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Runs a batch of `[ids[0], ids[0], ids[1]]` on the second tick on, keeping the last results.
    fn duplicate_batch(world: &mut World, _ctx: &SystemCtx) {
        let ids = motors(world);
        let moves = [(ids[0], [0.0, -0.1, 0.0]); 2]
            .into_iter()
            .chain([(ids[1], [0.0, -0.1, 0.0])])
            .collect::<Vec<_>>();
        let mut out = vec![Err(MotorError::NoPhysicsWorld); moves.len()];
        move_characters(world, &moves, &mut out);
        BATCH.with(|slot| *slot.borrow_mut() = out);
    }

    fn lane_batch(world: &mut World, _ctx: &SystemCtx) {
        let ids = motors(world);
        let lane = crate::lane::LaneConstraint {
            path: "lane.path.yaml".into(),
            engaged: true,
            ..Default::default()
        };
        world.insert(ids[0], lane).unwrap();
        let moves = ids
            .iter()
            .map(|id| (*id, [0.0, -0.1, 0.0]))
            .collect::<Vec<_>>();
        let mut out = vec![Err(MotorError::NoPhysicsWorld); moves.len()];
        move_characters(world, &moves, &mut out);
        BATCH.with(|slot| *slot.borrow_mut() = out);
    }

    fn batch_scene() -> String {
        scene(0.0, &mate("mate", 3.0))
    }

    #[test]
    fn a_batch_draws_one_capsule_per_moved_motor() {
        BATCH.with(|slot| slot.borrow_mut().clear());
        let drawn = drawn("batch", &batch_scene(), duplicate_batch, 3);
        // Two moved motors; the duplicate entry draws nothing.
        assert_eq!(of(&drawn, MOTOR_PROBE).len(), 2);
    }

    #[test]
    fn a_duplicate_in_a_batch_is_refused_and_the_first_copy_moves() {
        BATCH.with(|slot| slot.borrow_mut().clear());
        drawn("batch-dup", &batch_scene(), duplicate_batch, 3);
        BATCH.with(|slot| {
            let out = slot.borrow();
            assert!(out[0].is_ok(), "{:?}", out[0]);
            assert!(matches!(out[1], Err(MotorError::DuplicateInBatch(_))));
            assert!(out[2].is_ok());
        });
    }

    #[test]
    fn a_lane_entity_is_refused_in_a_batch() {
        BATCH.with(|slot| slot.borrow_mut().clear());
        drawn("batch-lane", &batch_scene(), lane_batch, 3);
        BATCH.with(|slot| {
            let out = slot.borrow();
            assert!(
                matches!(out[0], Err(MotorError::LaneInBatch(_))),
                "{:?}",
                out[0]
            );
            assert!(out[1].is_ok());
        });
    }
}
