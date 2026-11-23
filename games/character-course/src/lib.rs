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
//! step, a 55° slope the motor refuses, a path platform over a pit, a turntable and a press. Past the exit
//! pad a chase section (x 52..68) has a [`CameraGate`] switch the rig to a collision-aware `chase` boom,
//! which a pillar and a low porch pull in.
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
//! displacement a tick to `move_character` (the stick goes through `camera_relative` and `lane_relative`,
//! blended by the lane's weight); crouching is `resize_character`, and a fall or a crush
//! respawns with `teleport_character`. The camera is the engine's `CameraRig`: a rail behind the hero
//! on the main lane, a [`CameraGate`] sensor that blends to a side-on rig along the lane section and
//! back, a snap and a trauma pulse on respawn; the stick is `camera_relative`
//! (`ludion docs camera`). Every tunable is scene data on [`Runner`]. The motor's
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
    /// The camera trauma a respawn adds (`0..=1`).
    pub respawn_trauma: f32,
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
            respawn_trauma: 0.6,
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

/// A sensor that switches the camera to `rig` when the hero enters it; the rig blends over its own
/// `blend_in`.
#[derive(
    serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Debug, PartialEq, Default,
)]
#[serde(default)]
pub struct CameraGate {
    /// The configuration the camera's `CameraRig` takes.
    pub rig: RigConfig,
}

impl SemanticValidate for CameraGate {
    /// Delegates `rig` to the engine's range rules (`L1304`), pointers under `/rig`.
    fn validate_semantics(value: &serde_json::Value) -> Vec<Diagnostic> {
        let rig = value.get("rig").cloned().unwrap_or_default();
        RigConfig::validate_semantics(&rig)
            .into_iter()
            .map(|mut diagnostic| {
                diagnostic.path = diagnostic.path.map(|path| format!("/rig{path}"));
                diagnostic
            })
            .collect()
    }
}

// --- Systems ------------------------------------------------------------------------------------

/// Turns every [`Spin`] entity about +Y. Runs before [`controller`] so a rider is carried by this
/// tick's pose.
pub fn spin(world: &mut World, ctx: &SystemCtx) {
    for (spin, transform) in world.query::<(&Spin, &mut Transform)>() {
        transform.rotation[1] = (transform.rotation[1] + spin.deg_per_s * ctx.game_dt) % 360.0;
    }
}

/// The entity carrying the scene's [`CameraRig`].
fn camera_entity(world: &mut World) -> Option<EntityId> {
    world
        .query_ids::<(&CameraRig,)>()
        .next()
        .map(|(entity, _)| entity)
}

/// Applies the [`LaneSwitch`], [`Checkpoint`] and [`CameraGate`] sensors the hero entered in the last
/// physics step. Runs before [`controller`] so the tick's move sees the new lane state.
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
        if let Ok(Some(zone)) = world.get::<CameraGate>(sensor).map(|z| z.cloned())
            && let Some(camera) = camera_entity(world)
            && let Ok(Some(rig)) = world.get_mut::<CameraRig>(camera)
        {
            rig.rig = zone.rig;
        }
    }
}

/// Jump: Space or the pad's south button.
const JUMP: Action = Action {
    keys: &[Key::Space],
    buttons: &[GamepadButton::South],
    mouse: &[],
};
/// Crouch: C or the pad's east button.
const CROUCH: Action = Action {
    keys: &[Key::C],
    buttons: &[GamepadButton::East],
    mouse: &[],
};

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
    let stick = input.move_vector();
    let jump = input.action_pressed(&JUMP);
    let jump_released = input.action_released(&JUMP);
    let crouch_held = input.action_held(&CROUCH);
    // The yaw the rig posed last tick: the stick is relative to what the player sees.
    let yaw = camera_entity(world)
        .and_then(|camera| world.get::<CameraRig>(camera).ok().flatten())
        .map_or(0.0, CameraRig::yaw_deg);

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
    state.velocity_y -= runner.gravity * ctx.game_dt;

    // The free direction blended toward the lane's by the lane's weight.
    let free = camera_relative_yaw(stick, yaw);
    let direction = match lane_frame(world, hero) {
        Some(frame) => {
            let screen_right = camera_relative_yaw([1.0, 0.0], yaw);
            let lane = lane_relative(stick, &frame, screen_right).direction;
            std::array::from_fn(|i| free[i] + (lane[i] - free[i]) * frame.weight)
        }
        None => free,
    };
    let displacement = [
        direction[0] * runner.run_speed * ctx.game_dt,
        state.velocity_y * ctx.game_dt,
        direction[2] * runner.run_speed * ctx.game_dt,
    ];
    let mut respawn = false;
    let mut grounded_after = grounded;
    if let Ok(report) = move_character(world, hero, displacement) {
        grounded_after = report.grounded;
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
        state.crouching = world
            .get::<CharacterMotorState>(hero)
            .ok()
            .flatten()
            .is_some_and(|motor_state| motor_state.height.is_some_and(|h| h < motor.height));
        if let Some(camera) = camera_entity(world)
            && let Ok(Some(rig)) = world.get_mut::<CameraRig>(camera)
        {
            snap_rig(rig);
            add_trauma(rig, runner.respawn_trauma);
        }
    }
    if let Some(camera) = camera_entity(world)
        && let Ok(Some(rig)) = world.get_mut::<CameraRig>(camera)
    {
        rig.grounded = grounded_after;
    }
    if let Ok(Some(slot)) = world.get_mut::<RunnerState>(hero) {
        *slot = state;
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
    registry.register_validated::<CameraGate>("CameraGate")
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
/// turn → sensors apply → the hero moves. The engine runs `path_follow` before these, then the
/// physics step, then `camera_rig`, which poses the camera for the tick.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(register)
        .system("spin", spin)
        .system("triggers", triggers)
        .system("controller", controller)
        .default_scene("scenes/main.yaml")
        .title("Character Course")
}
