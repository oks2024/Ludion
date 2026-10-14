#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `hello-paddle` — the smallest complete Ludion game: one custom component, one system, one scene
//! that mixes built-in visuals with that component.
//!
//! It is the Phase 5 proof that a game gets everything the engine provides — scene load/validation
//! through the registry, deterministic sim, `WorldHash`, recording/replay, the wire protocol, and
//! the golden harness — while bringing only its own [`Paddle`] component and [`move_paddle`] system.
//! The binary (`src/main.rs`) is the ~3-line `ludion::run(game())` host; this library exposes the
//! pieces so the tests can drive them directly.

use ludion::prelude::*;

/// How fast the paddle slides, in world units per second (applied as `speed * dt` each tick, so it
/// is frame-rate independent and deterministic).
pub const PADDLE_SPEED: f32 = 6.0;

/// The world-space x of the arena's right edge. Sliding across it plays [`BOUNCE_SOUND`] once — a
/// pure *observation* of the existing motion: the paddle's movement is unchanged, only a sound is
/// emitted. At `PADDLE_SPEED` the paddle advances `0.1`/tick from `x = 0`, so it crosses `2.05` on
/// frame 20 — the anchor the audio event-stream test asserts on.
pub const ARENA_EDGE: f32 = 2.05;

/// The sound played as the paddle reaches [`ARENA_EDGE`], relative to the content root (the scene's
/// directory). A small committed 16-bit PCM WAV blip.
pub const BOUNCE_SOUND: &str = "sounds/bounce.wav";

/// The game's one custom component: a paddle's horizontal position.
///
/// It rides on the same entity as the built-in [`Transform`] and [`Sprite`], and [`move_paddle`]
/// keeps the `Transform` in step so the sprite actually slides. `Paddle` is registered *on top of*
/// the built-in visual components, so a scene freely mixes the two.
#[derive(
    serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Copy, Debug, PartialEq,
)]
pub struct Paddle {
    /// The paddle's horizontal position, in world units.
    #[serde(default)]
    pub x: f32,
}

impl Default for Paddle {
    fn default() -> Self {
        Paddle { x: 0.0 }
    }
}

/// Slides every paddle right while the `D` key is held, keeping its [`Transform`] in step.
///
/// A plain system — `fn(&mut World, &SystemCtx)` — that reads time only through `ctx.dt` and input
/// only through the [`InputState`] resource, so it is fully deterministic (no wall clock, no
/// `thread_rng`). It queries a game component and a built-in component together, the everyday shape
/// of gameplay code.
pub fn move_paddle(world: &mut World, ctx: &SystemCtx) {
    if !world.resource::<InputState>().unwrap().is_held(Key::D) {
        return;
    }
    // Move the paddle, noting whether it crossed the arena edge this tick. Audio must be pushed
    // through a separate `resource_mut` borrow, so the crossing is collected here and emitted below.
    let mut crossed_edge = false;
    for (paddle, transform) in world.query::<(&mut Paddle, &mut Transform)>() {
        let before = paddle.x;
        paddle.x += PADDLE_SPEED * ctx.dt;
        transform.translation[0] = paddle.x;
        if before < ARENA_EDGE && paddle.x >= ARENA_EDGE {
            crossed_edge = true;
        }
    }
    if crossed_edge {
        world
            .resource_mut::<AudioQueue>()
            .expect("the engine installs the AudioQueue resource on every world")
            .play(BOUNCE_SOUND, 1.0);
    }
}

/// Registers the game's components on top of a registry that already holds the built-ins.
///
/// This is the `components` closure [`game`] hands the builder — exposed as a named function so the
/// golden test's registry seam (`GoldenTest::with_registry`) can build the same registry.
///
/// # Errors
/// An `L0005` diagnostic if a name collides with a built-in or another game component.
pub fn register(registry: &mut Registry) -> Result<(), Diagnostic> {
    registry.register::<Paddle>("Paddle")
}

/// The game's full component registry: the built-ins plus [`Paddle`], in registration order.
///
/// A `fn() -> Registry` so it can be handed straight to `GoldenTest::with_registry` (which validates
/// a scene naming game components against exactly this set).
///
/// # Panics
/// If [`register`] fails — which would be a build-time bug (a name colliding with a built-in), not a
/// runtime condition.
pub fn registry() -> Registry {
    let mut registry = builtin_registry();
    register(&mut registry).expect("Paddle registers cleanly on top of the built-ins");
    registry
}

/// Describes the engine that hosts this game: the built-in core plus [`Paddle`] and [`move_paddle`],
/// with `scenes/paddle.yaml` as the default scene `ludion::run` loads when `--scene` is omitted.
///
/// Returns a fresh builder each call (building consumes it), so `main`, the record test, and the
/// replay reproduce it identically.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(register)
        .system("move_paddle", move_paddle)
        .default_scene("scenes/paddle.yaml")
}
