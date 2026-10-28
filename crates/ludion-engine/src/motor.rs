//! The character motor on the engine's [`Transform`]: `ludion_physics::move_character` bound to
//! the pose component, which is what a game calls. The move wrappers draw the `motor` debug
//! category after each successful call.

use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_physics::{
    CharacterMotor, CharacterMotorState, MotorError, MoveConstraint, MoveReport, ResizeReport,
    ground_probe_reach,
};

use crate::components::Transform;
use crate::debug_draw::debug_draw;

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
/// The ground probe: the line the snap looks along below the feet.
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
/// move, bit for bit.
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
    let report =
        ludion_physics::move_character_with::<Transform>(world, entity, displacement, constraint)?;
    draw(world, entity, was_grounded, displacement, &report);
    Ok(report)
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

/// Places the feet and yaw without driving the capsule; see [`ludion_physics::teleport_character`].
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
    ludion_physics::teleport_character::<Transform>(world, entity, feet, yaw)
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
    fn drawn(scene: &str, system: fn(&mut World, &SystemCtx), ticks: usize) -> Vec<DebugSegment> {
        let dir = std::env::temp_dir().join(format!("ludion-motor-draw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut engine = Engine::builder()
            .project_root(dir)
            .system("motor", system)
            .build()
            .expect("boots");
        engine.load_inline_scene(scene).unwrap();
        for _ in 0..ticks {
            engine.advance_live();
        }
        segments(engine.world(), &DebugCategories::parse("motor"))
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
        let drawn = drawn(&scene(2.0, ""), fall, 3);
        assert!(!of(&drawn, MOTOR_AIRBORNE).is_empty());
        assert!(of(&drawn, MOTOR_GROUNDED).is_empty());
        assert!(of(&drawn, MOTOR_GROUND_NORMAL).is_empty());
        let probe = of(&drawn, MOTOR_PROBE);
        assert_eq!(probe.len(), 1);
        assert!((length(probe[0]) - 0.02).abs() < 1e-6);
    }

    #[test]
    fn a_grounded_tick_draws_a_green_capsule_a_ground_normal_and_a_snap_probe() {
        let drawn = drawn(&scene(0.0, ""), fall, 30);
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
        let drawn = drawn(&scene(0.0, &wall), into_wall, 40);
        assert_eq!(of(&drawn, MOTOR_WALL).len(), 1);
    }

    #[test]
    fn a_crushed_motor_draws_a_red_capsule() {
        let walls = boxed("left", [0.7, 1.0, 0.0], [0.5, 2.0, 5.0])
            + &boxed("right", [-0.7, 1.0, 0.0], [0.5, 2.0, 5.0]);
        let drawn = drawn(&scene(0.0, &walls), fall, 5);
        assert!(!of(&drawn, MOTOR_STUCK).is_empty());
        assert!(of(&drawn, MOTOR_GROUNDED).is_empty());
        assert!(of(&drawn, MOTOR_AIRBORNE).is_empty());
    }
}
