#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `character-course` — a greybox obstacle course for the character motor, and its reference
//! controller.
//!
//! The course (`scenes/main.yaml`, geometry from `scenes/blender/build.py`) runs along +X. The
//! scripted route is the main lane: a spawn pad, a 0.24 m step, a 20° ramp, a low tunnel that only a
//! crouch clears, a 1.5 m pit to jump, a conveyor, an end pad. Past the end pad a side-on lane section
//! (`scenes/paths/lane.path.yaml`) curves up a ramp, crosses a pit, drops to a belt and ends on an exit
//! pad: a [`LaneSwitch`] sensor engages the hero's `LaneConstraint`, a [`Checkpoint`] sensor inside
//! makes the pit's respawn land on the lane, and a second switch releases it. Beside it, off the lane, are a 0.4 m
//! step, a 55° slope the motor refuses, a path platform over a pit, a turntable and a press.
//!
//! **Play it** (`Left`/`Right`/`Up`/`Down` or `A`/`D`/`W`/`S`, the left stick; `Space` or the south
//! button jumps, release early to cut the jump; hold `C` or the east button to crouch):
//!
//! ```text
//! cargo run -p character-course -- --scene games/character-course/scenes/main.yaml --window
//! ```
//!
//! The controller is the pattern a game builds on the motor: [`controller`] turns input into a
//! horizontal velocity, integrates gravity into [`RunnerState::velocity_y`], and hands one
//! displacement a tick to `move_character` (the stick goes through `lane_relative`, blended by the
//! lane's weight); crouching is `resize_character`, and a fall or a crush
//! respawns with `teleport_character`. Every tunable is scene data on [`Runner`]. The motor's
//! dimensions come from the hero's `CharacterMotor`, never from a constant here.

use ludion::prelude::*;

// --- Components ---------------------------------------------------------------------------------

/// The reference controller's tunables, on the hero entity next to its `CharacterMotor`.
#[derive(
    serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Copy, Debug, PartialEq,
)]
#[serde(default)]
pub struct Runner {
    /// Horizontal speed at full input, metres per second.
    pub run_speed: f32,
    /// Upward speed a jump starts with, metres per second.
    pub jump_speed: f32,
    /// Downward acceleration, metres per second squared.
    pub gravity: f32,
    /// What releasing the jump early leaves of the upward speed (`0..=1`).
    pub jump_cut: f32,
    /// The capsule height while crouched, metres.
    pub crouch_height: f32,
    /// Falling below this height respawns the hero.
    pub kill_y: f32,
    /// Where a respawn puts the feet.
    pub spawn: [f32; 3],
}

impl Default for Runner {
    fn default() -> Self {
        Runner {
            run_speed: 5.0,
            jump_speed: 7.0,
            gravity: 22.0,
            jump_cut: 0.5,
            crouch_height: 0.6,
            kill_y: -6.0,
            spawn: [0.0, 0.0, 0.0],
        }
    }
}

/// The controller's per-tick state: hashed, so a snapshot mid-jump resumes the same arc.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
#[serde(default)]
pub struct RunnerState {
    /// Vertical velocity, metres per second, up positive.
    pub velocity_y: f32,
    /// Whether the capsule is at the crouch height.
    pub crouching: bool,
    /// Where a respawn puts the feet once a [`Checkpoint`] was reached; `None` means
    /// [`Runner::spawn`].
    pub checkpoint: Option<[f32; 3]>,
}

/// A platform turning about +Y at a constant rate (the turntable).
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
#[serde(default)]
pub struct Spin {
    /// Degrees per second about +Y.
    pub deg_per_s: f32,
}

/// A sensor that engages (`engage: true`) or releases the hero's `LaneConstraint` when the hero
/// enters it.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
#[serde(default)]
pub struct LaneSwitch {
    /// Whether entering the sensor engages the lane (`false` releases it).
    pub engage: bool,
}

/// A sensor that makes `feet` the hero's respawn point when the hero enters it.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
#[serde(default)]
pub struct Checkpoint {
    /// The feet position a respawn uses.
    pub feet: [f32; 3],
}

/// Marks the camera that trails the hero: translation is the hero's feet plus `offset`. The
/// rotation stays as authored.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Default,
)]
#[serde(default)]
pub struct FollowCam {
    /// The camera's position relative to the hero's feet, metres.
    pub offset: [f32; 3],
}

// --- Systems ------------------------------------------------------------------------------------

/// Turns every [`Spin`] entity about +Y. Runs before [`controller`] so a rider is carried by this
/// tick's pose.
pub fn spin(world: &mut World, ctx: &SystemCtx) {
    for (spin, transform) in world.query::<(&Spin, &mut Transform)>() {
        transform.rotation[1] = (transform.rotation[1] + spin.deg_per_s * ctx.game_dt) % 360.0;
    }
}

/// Screen-right in world space: the follow camera never yaws.
const SCREEN_RIGHT: [f32; 3] = [1.0, 0.0, 0.0];

/// Applies the [`LaneSwitch`] and [`Checkpoint`] sensors the hero entered in the last physics step.
/// Runs before [`controller`] so the tick's move sees the new lane state.
pub fn triggers(world: &mut World, _ctx: &SystemCtx) {
    let Some(hero) = world
        .query_ids::<(&Runner,)>()
        .next()
        .map(|(entity, _)| entity)
    else {
        return;
    };
    let entered: Vec<EntityId> = match world.resource::<PhysicsWorld>() {
        Some(physics) => physics
            .sensor_events()
            .iter()
            .filter(|event| event.other == hero && event.phase == SensorPhase::Entered)
            .map(|event| event.sensor)
            .collect(),
        None => return,
    };
    for sensor in entered {
        if let Ok(Some(switch)) = world.get::<LaneSwitch>(sensor).map(|s| s.copied())
            && let Ok(Some(lane)) = world.get_mut::<LaneConstraint>(hero)
        {
            lane.engaged = switch.engage;
        }
        if let Ok(Some(checkpoint)) = world.get::<Checkpoint>(sensor).map(|c| c.copied())
            && let Ok(Some(state)) = world.get_mut::<RunnerState>(hero)
        {
            state.checkpoint = Some(checkpoint.feet);
        }
    }
}

/// The horizontal intent in `[-1, 1]²` (x, z): the keys, else the left stick; a diagonal is not
/// faster than an axis.
fn move_intent(input: &InputState) -> [f32; 2] {
    let axis = |plus: &[Key], minus: &[Key]| {
        f32::from(plus.iter().any(|&k| input.is_held(k)))
            - f32::from(minus.iter().any(|&k| input.is_held(k)))
    };
    let mut intent = [
        axis(&[Key::Right, Key::D], &[Key::Left, Key::A]),
        axis(&[Key::Down, Key::S], &[Key::Up, Key::W]),
    ];
    if intent == [0.0, 0.0] {
        let stick = input.gamepad().left_stick();
        intent = [stick[0], -stick[1]];
    }
    let length = (intent[0] * intent[0] + intent[1] * intent[1]).sqrt();
    if length > 1.0 {
        intent = [intent[0] / length, intent[1] / length];
    }
    intent
}

/// Drives the hero: input → velocity, gravity, jump and its cut, crouch, one `move_character`
/// call, and a respawn when the motor is crushed or the hero falls out of the world.
pub fn controller(world: &mut World, ctx: &SystemCtx) {
    let Some((hero, runner, mut state)) = world
        .query_ids::<(&Runner, &RunnerState)>()
        .next()
        .map(|(entity, (runner, state))| (entity, *runner, *state))
    else {
        return;
    };
    let Ok(Some(motor)) = world.get::<CharacterMotor>(hero).map(|m| m.cloned()) else {
        return;
    };
    let grounded = world
        .get::<CharacterMotorState>(hero)
        .ok()
        .flatten()
        .is_some_and(|motor_state| motor_state.grounded);
    let input = world.resource::<InputState>().unwrap();
    let intent = move_intent(input);
    let jump = input.was_pressed(Key::Space) || input.gamepad().was_pressed(GamepadButton::South);
    let jump_released =
        input.was_released(Key::Space) || input.gamepad().was_released(GamepadButton::South);
    let crouch_held = input.is_held(Key::C) || input.gamepad().is_held(GamepadButton::East);

    if crouch_held && !state.crouching {
        if resize_character(world, hero, runner.crouch_height).is_ok() {
            state.crouching = true;
        }
    } else if !crouch_held && state.crouching {
        // Stand up when there is room; under the tunnel the call reports the blocker and the
        // height stays.
        if let Ok(report) = resize_character(world, hero, motor.height) {
            state.crouching = report.blocked_by.is_some();
        }
    }

    if jump && grounded {
        state.velocity_y = runner.jump_speed;
    } else if jump_released && state.velocity_y > 0.0 {
        state.velocity_y *= runner.jump_cut;
    }
    state.velocity_y -= runner.gravity * ctx.dt;

    // The free direction blended toward the lane's by the lane's weight.
    let free = [intent[0], 0.0, intent[1]];
    let direction = match lane_frame(world, hero) {
        Some(frame) => {
            let lane = lane_relative([intent[0], -intent[1]], &frame, SCREEN_RIGHT).direction;
            std::array::from_fn(|i| free[i] + (lane[i] - free[i]) * frame.weight)
        }
        None => free,
    };
    let displacement = [
        direction[0] * runner.run_speed * ctx.dt,
        state.velocity_y * ctx.dt,
        direction[2] * runner.run_speed * ctx.dt,
    ];
    let mut respawn = false;
    if let Ok(report) = move_character(world, hero, displacement) {
        if report.ceiling.is_some() && state.velocity_y > 0.0 {
            state.velocity_y = 0.0;
        }
        if report.grounded && state.velocity_y < 0.0 {
            state.velocity_y = 0.0;
        }
        respawn = report.stuck;
    }
    let below = world
        .get::<Transform>(hero)
        .ok()
        .flatten()
        .is_some_and(|pose| pose.translation[1] < runner.kill_y);
    let feet = state.checkpoint.unwrap_or(runner.spawn);
    if (respawn || below) && teleport_character(world, hero, feet, 0.0).is_ok() {
        state.velocity_y = 0.0;
        // A lane re-acquires its place over the whole path.
        if let Ok(Some(lane)) = world.get_mut::<LaneConstraint>(hero) {
            lane.s = None;
        }
        state.crouching = world
            .get::<CharacterMotorState>(hero)
            .ok()
            .flatten()
            .is_some_and(|motor_state| motor_state.height.is_some_and(|h| h < motor.height));
    }
    if let Ok(Some(slot)) = world.get_mut::<RunnerState>(hero) {
        *slot = state;
    }
}

/// Puts every [`FollowCam`] camera at the hero's feet plus its offset.
pub fn follow_camera(world: &mut World, _ctx: &SystemCtx) {
    let Some(feet) = world
        .query::<(&Runner, &Transform)>()
        .next()
        .map(|(_, pose)| pose.translation)
    else {
        return;
    };
    for (cam, transform) in world.query::<(&FollowCam, &mut Transform)>() {
        transform.translation = [
            feet[0] + cam.offset[0],
            feet[1] + cam.offset[1],
            feet[2] + cam.offset[2],
        ];
    }
}

// --- Registration -------------------------------------------------------------------------------

/// Registers this game's components on top of a registry that already holds the built-ins.
///
/// # Errors
/// An `L0005` diagnostic if a name collides with a built-in or another game component.
pub fn register(registry: &mut Registry) -> Result<(), Diagnostic> {
    registry.register::<Runner>("Runner")?;
    registry.register::<RunnerState>("RunnerState")?;
    registry.register::<Spin>("Spin")?;
    registry.register::<LaneSwitch>("LaneSwitch")?;
    registry.register::<Checkpoint>("Checkpoint")?;
    registry.register::<FollowCam>("FollowCam")
}

/// This game's full component registry: the built-ins plus this game's components.
///
/// # Panics
/// If [`register`] fails — a build-time bug (a name colliding with a built-in), not a runtime state.
pub fn registry() -> Registry {
    let mut registry = builtin_registry();
    register(&mut registry).expect("components register cleanly on top of the built-ins");
    registry
}

/// Describes the engine that hosts this game. Registration order is execution order: turntables
/// turn → the hero moves → the camera follows. The engine runs `path_follow` before these and the
/// physics step after them.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(register)
        .system("spin", spin)
        .system("triggers", triggers)
        .system("controller", controller)
        .system("follow_camera", follow_camera)
        .default_scene("scenes/main.yaml")
}
