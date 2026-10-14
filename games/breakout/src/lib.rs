#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `breakout` — a Breakout clone with dynamic lighting, built on Ludion.
//!
//! The game brings six components ([`Paddle`], [`Ball`], [`Brick`], [`BallGlow`], [`PauseMenu`],
//! [`BrickBurst`]) and seven systems (menu input, paddle input, ball physics, brick destruction, reap
//! spent bursts, glow-follows-ball, glow decay). Everything else — scene load, the deterministic sim,
//! `WorldHash`, recording/replay, the golden harness — comes from the engine.
//!
//! **Dynamic lighting is gameplay state here:** a `PointLight` rides the ball ([`BallGlow`]), and
//! breaking a brick spikes its intensity to [`GLOW_FLASH`], which then decays back toward
//! [`GLOW_BASE`]. Because intensity is component state it is hashed, deterministic, and testable.
//!
//! **Breaking a brick also throws a spark burst:** one `ParticleEmitter` entity spawned at the
//! brick's position, in the brick's own `Sprite` colour (or [`BRICK_BURST_FALLBACK_COLOR`] if it had
//! none), tagged [`BrickBurst`] so [`reap_bursts`] despawns it once its longest particle has aged
//! out — proof the particle component is engine-generic, not vigil-only.
//!
//! **Pause is a `UiList`, and being paused is derived, never stored:** the [`PauseMenu`] entity's
//! list holds items while the menu is open and none while it is closed (an empty list draws
//! nothing), so [`is_paused`] simply asks the widget. Every gameplay system returns early while it
//! is open, and `RESTART` re-instantiates the scene through the engine's `SceneSwitch`.

use ludion::prelude::*;

// --- Tunables -----------------------------------------------------------------------------------

/// Half the playfield width: side walls sit at `x = ±ARENA_HALF_WIDTH`.
pub const ARENA_HALF_WIDTH: f32 = 5.0;
/// The ceiling the ball bounces off.
pub const ARENA_TOP: f32 = 4.5;
/// Below this the ball is lost and re-served from [`SERVE_POSITION`].
pub const ARENA_BOTTOM: f32 = -5.5;

/// Paddle slide speed, world units per second.
pub const PADDLE_SPEED: f32 = 8.0;
/// Paddle half-extents (x, y) used for collision and wall clamping.
pub const PADDLE_HALF: [f32; 2] = [1.0, 0.2];
/// How much horizontal "english" a paddle hit imparts, per unit of offset from paddle center.
pub const PADDLE_ENGLISH: f32 = 2.0;

/// Ball half-extent (it collides as a small AABB).
pub const BALL_HALF: f32 = 0.15;
/// Where a lost ball re-serves from.
pub const SERVE_POSITION: [f32; 2] = [0.0, -2.5];
/// The velocity a served ball starts with.
pub const SERVE_VELOCITY: [f32; 2] = [2.5, 5.0];

/// Brick half-extents (x, y) used for collision.
pub const BRICK_HALF: [f32; 2] = [0.8, 0.3];

/// The ball light's resting intensity.
pub const GLOW_BASE: f32 = 1.2;
/// The ball light's intensity the instant a brick breaks.
pub const GLOW_FLASH: f32 = 6.0;
/// Exponential decay rate (per second) pulling the glow back toward [`GLOW_BASE`].
pub const GLOW_DECAY: f32 = 5.0;

/// How many particles a broken brick's spark burst releases at once.
pub const BRICK_BURST_COUNT: u32 = 24;
/// Burst particle lifetime, seconds, drawn uniformly as `[min, max]`.
pub const BRICK_BURST_LIFE: [f32; 2] = [0.3, 0.6];
/// Burst particle speed at spawn, units per second, drawn uniformly as `[min, max]`.
pub const BRICK_BURST_SPEED: [f32; 2] = [2.0, 5.0];
/// World −Y acceleration pulling burst debris down.
pub const BRICK_BURST_GRAVITY: f32 = 6.0;
/// Burst particle disc diameter at age zero (it shrinks to nothing over its life).
pub const BRICK_BURST_START_SIZE: f32 = 0.15;
/// The burst's colour when the broken brick's `Sprite` tint can't be read.
pub const BRICK_BURST_FALLBACK_COLOR: [f32; 4] = [1.0, 0.9, 0.7, 1.0];

/// The pause menu's first row: closes the menu and hands play back.
pub const MENU_RESUME: &str = "RESUME";
/// The pause menu's second row: re-instantiates the scene from disk, board and ball included.
pub const MENU_RESTART: &str = "RESTART";

/// The scene [`MENU_RESTART`] loads, relative to the project root (the started scene's directory).
const RESTART_SCENE: &str = "main.yaml";
/// The entity in [`RESTART_SCENE`] a restart drops its (empty) carry set at.
const RESTART_ANCHOR: &str = "paddle";

// --- Components ---------------------------------------------------------------------------------

/// The player's paddle. Steered with Left/Right; clamped to the arena walls.
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
pub struct Paddle {}

/// The ball: its velocity in world units per second. Position lives in the entity's `Transform`.
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
pub struct Ball {
    /// Current velocity (x, y), world units per second.
    #[serde(default)]
    pub velocity: [f32; 2],
}

/// A destructible brick. Despawned on ball contact.
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
pub struct Brick {}

/// Marks the `PointLight` entity that follows the ball and flashes when a brick breaks.
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
pub struct BallGlow {}

/// Marks the `UiList` entity that is the pause menu. Its items are the whole pause state: present
/// while the menu is open, empty while it is closed — see [`is_paused`].
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
pub struct PauseMenu {}

/// Marks the one-shot `ParticleEmitter` a broken brick throws its spark burst from.
///
/// `expires_at` is the gameplay time ([`SystemCtx::game_time`] in seconds) at which the burst's
/// longest-lived particle has aged out; [`reap_bursts`] despawns the entity once that time passes.
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
pub struct BrickBurst {
    /// The sim time, in seconds, past which this burst is spent and should be despawned.
    #[serde(default)]
    pub expires_at: f32,
}

// --- Systems (registration order is execution order) ---------------------------------------------

/// Whether the game is paused: the [`PauseMenu`] list has items. Derived from the widget, so there
/// is no flag that can disagree with what is on screen.
pub fn is_paused(world: &World) -> bool {
    world.entities().any(|entity| {
        world.has::<PauseMenu>(entity)
            && matches!(world.get::<UiList>(entity), Ok(Some(list)) if !list.items.is_empty())
    })
}

/// Opens, drives and closes the pause menu — the one system that runs while paused.
///
/// `Escape` (or the gamepad's `Start`) toggles; the cursor walks with the engine's default `UiNav`
/// bindings, and a left click activates whatever row it lands on. `items` is written only when the
/// menu opens or closes: a variable-length field rebuilt every frame would churn the `WorldHash`
/// while the menu just sits there.
pub fn menu_input(world: &mut World, _ctx: &SystemCtx) {
    let (toggle, nav, click, cursor) = {
        let input = world.resource::<InputState>().unwrap();
        (
            input.was_pressed(Key::Escape) || input.gamepad().was_pressed(GamepadButton::Start),
            UiNav::from_input(input),
            input.mouse().was_pressed(MouseButton::Left),
            input.mouse().position(),
        )
    };
    // The space the cursor pixels above live in: the session's live render extent, which a host's
    // `--render-size` or a client's `render.size` moves out from under a hard-coded number.
    let viewport = world
        .resource::<RenderExtent>()
        .copied()
        .unwrap_or_default()
        .size();

    let fonts = world.resource::<FontLibrary>().cloned().unwrap_or_default();
    let mut restart = false;
    for (_menu, list) in world.query::<(&PauseMenu, &mut UiList)>() {
        if list.items.is_empty() {
            if toggle {
                list.items = vec![
                    UiListItem {
                        text: MENU_RESUME.to_string(),
                        enabled: true,
                    },
                    UiListItem {
                        text: MENU_RESTART.to_string(),
                        enabled: true,
                    },
                ];
                list.selected = 0;
            }
            continue;
        }
        if toggle {
            list.items.clear();
            continue;
        }

        let mut activated = list.navigate(nav);
        if click {
            // The hit test answers what is under the cursor; whether a disabled row counts is ours.
            if let Some(index) =
                ludion::math::list_hit(fonts.face(list.font.as_deref()), list, viewport, cursor)
                && list.items[index as usize].enabled
            {
                list.selected = index;
                activated = Some(index);
            }
        }
        match activated {
            Some(0) => list.items.clear(),
            Some(1) => {
                list.items.clear();
                restart = true;
            }
            _ => {}
        }
    }

    if restart {
        // The engine rebuilds the world from the scene at the frame boundary: fresh board, fresh
        // ball, fresh menu. Nothing is carried — a restart keeps no state by definition.
        if let Some(switch) = world.resource_mut::<SceneSwitch>() {
            switch.request(RESTART_SCENE, RESTART_ANCHOR, Vec::new());
        }
    }
}

/// Slides the paddle with the Left/Right arrows, clamped so it never leaves the arena.
pub fn move_paddle(world: &mut World, ctx: &SystemCtx) {
    if is_paused(world) {
        return;
    }
    let input = world.resource::<InputState>().unwrap();
    let dir = f32::from(input.is_held(Key::Right)) - f32::from(input.is_held(Key::Left));
    if dir == 0.0 {
        return;
    }
    let limit = ARENA_HALF_WIDTH - PADDLE_HALF[0];
    for (_paddle, transform) in world.query::<(&Paddle, &mut Transform)>() {
        let x = transform.translation[0] + dir * PADDLE_SPEED * ctx.dt;
        transform.translation[0] = x.clamp(-limit, limit);
    }
}

/// Integrates the ball, bounces it off the side walls, the ceiling, and the paddle, and re-serves
/// it when it falls below [`ARENA_BOTTOM`]. Pure `f32` arithmetic — deterministic.
pub fn move_ball(world: &mut World, ctx: &SystemCtx) {
    if is_paused(world) {
        return;
    }
    // Read the paddle's center once (the borrow ends before the ball query starts).
    let paddle = world
        .query::<(&Paddle, &Transform)>()
        .next()
        .map(|(_, t)| [t.translation[0], t.translation[1]]);

    for (ball, transform) in world.query::<(&mut Ball, &mut Transform)>() {
        let mut x = transform.translation[0] + ball.velocity[0] * ctx.dt;
        let mut y = transform.translation[1] + ball.velocity[1] * ctx.dt;

        // Side walls.
        let wall = ARENA_HALF_WIDTH - BALL_HALF;
        if x > wall && ball.velocity[0] > 0.0 {
            x = wall;
            ball.velocity[0] = -ball.velocity[0];
        } else if x < -wall && ball.velocity[0] < 0.0 {
            x = -wall;
            ball.velocity[0] = -ball.velocity[0];
        }

        // Ceiling.
        let ceiling = ARENA_TOP - BALL_HALF;
        if y > ceiling && ball.velocity[1] > 0.0 {
            y = ceiling;
            ball.velocity[1] = -ball.velocity[1];
        }

        // Paddle: only while the ball is descending, so it can't re-collide on the way up.
        if let Some([px, py]) = paddle {
            let over_x = (x - px).abs() < PADDLE_HALF[0] + BALL_HALF;
            let over_y = (y - py).abs() < PADDLE_HALF[1] + BALL_HALF;
            if over_x && over_y && ball.velocity[1] < 0.0 {
                y = py + PADDLE_HALF[1] + BALL_HALF;
                ball.velocity[1] = -ball.velocity[1];
                // Where on the paddle it hit steers the rebound.
                ball.velocity[0] += (x - px) * PADDLE_ENGLISH;
            }
        }

        // Lost below the floor: re-serve.
        if y < ARENA_BOTTOM {
            x = SERVE_POSITION[0];
            y = SERVE_POSITION[1];
            ball.velocity = SERVE_VELOCITY;
        }

        transform.translation[0] = x;
        transform.translation[1] = y;
    }
}

/// Despawns bricks the ball touches, reflects the ball off the axis of least penetration, and — the
/// dynamic-lighting beat — spikes the [`BallGlow`] light to [`GLOW_FLASH`] on every break. Each
/// broken brick also throws a spark burst (see `spawn_brick_burst`).
pub fn break_bricks(world: &mut World, ctx: &SystemCtx) {
    if is_paused(world) {
        return;
    }
    // The ball's AABB, read once.
    let ball = world
        .query::<(&Ball, &Transform)>()
        .next()
        .map(|(_, t)| [t.translation[0], t.translation[1]]);
    let Some([bx, by]) = ball else { return };

    // Walk `entities()` rather than hold a query borrow: `Sprite` is read per hit through `get`
    // (collected: we despawn below).
    // Position and colour are captured here, before the despawn below erases them.
    let mut dead: Vec<(EntityId, [f32; 3], [f32; 4])> = Vec::new();
    let mut reflect_x = false;
    let mut reflect_y = false;
    for e in world.entities().collect::<Vec<_>>() {
        if !world.has::<Brick>(e) {
            continue;
        }
        let Ok(Some(t)) = world.get::<Transform>(e) else {
            continue;
        };
        let dx = bx - t.translation[0];
        let dy = by - t.translation[1];
        let pen_x = BRICK_HALF[0] + BALL_HALF - dx.abs();
        let pen_y = BRICK_HALF[1] + BALL_HALF - dy.abs();
        if pen_x <= 0.0 || pen_y <= 0.0 {
            continue;
        }
        // Overlap: reflect along the axis of least penetration.
        if pen_x < pen_y {
            reflect_x = true;
        } else {
            reflect_y = true;
        }
        let color = world
            .get::<Sprite>(e)
            .ok()
            .flatten()
            .map_or(BRICK_BURST_FALLBACK_COLOR, |sprite| sprite.color);
        dead.push((e, t.translation, color));
    }
    if dead.is_empty() {
        return;
    }

    for (ball, _t) in world.query::<(&mut Ball, &Transform)>() {
        if reflect_x {
            ball.velocity[0] = -ball.velocity[0];
        }
        if reflect_y {
            ball.velocity[1] = -ball.velocity[1];
        }
    }
    // Every burst spawned this tick shares the same stamp: the frame of the break, and the moment
    // its longest-lived particle will have fully aged out.
    let expires_at = ctx.game_time.seconds() as f32 + BRICK_BURST_LIFE[1];
    for (brick, translation, color) in dead {
        let _ = world.despawn(brick);
        spawn_brick_burst(world, translation, color, brick, ctx.game_frame, expires_at);
    }
    // The flash: brick destruction drives the light. Intensity is component state, so it is hashed.
    for (light, _glow) in world.query::<(&mut PointLight, &BallGlow)>() {
        light.intensity = GLOW_FLASH;
    }
}

/// Spawns the one-shot spark burst a broken brick throws: a small analytic particle shower in the
/// brick's own colour, seeded off the brick's own id so two bricks breaking on the same tick don't
/// draw identical sparks. `seed`/`started` are the only per-burst state; every other field is a
/// fixed tunable.
fn spawn_brick_burst(
    world: &mut World,
    translation: [f32; 3],
    color: [f32; 4],
    brick: EntityId,
    started: FrameIndex,
    expires_at: f32,
) {
    let entity = world.spawn();
    let _ = world.insert(
        entity,
        Transform {
            translation,
            ..Default::default()
        },
    );
    let _ = world.insert(
        entity,
        ParticleEmitter {
            shape: EmitShape::Box,
            extent: [BRICK_HALF[0], BRICK_HALF[1], 0.05],
            rate: 0.0,
            burst: BRICK_BURST_COUNT,
            started,
            life: BRICK_BURST_LIFE,
            speed: BRICK_BURST_SPEED,
            spread_deg: 180.0,
            gravity: BRICK_BURST_GRAVITY,
            start_size: BRICK_BURST_START_SIZE,
            end_size: 0.0,
            start_color: color,
            end_color: [color[0], color[1], color[2], 0.0],
            blend: ParticleBlend::Alpha,
            seed: u64::from(brick.index),
            ..Default::default()
        },
    );
    let _ = world.insert(entity, BrickBurst { expires_at });
}

/// Despawns each [`BrickBurst`] once its particles have fully aged out (`expires_at` has passed).
pub fn reap_bursts(world: &mut World, ctx: &SystemCtx) {
    let now = ctx.game_time.seconds() as f32;
    let mut spent = Vec::new();
    for e in world.entities().collect::<Vec<_>>() {
        let Ok(Some(burst)) = world.get::<BrickBurst>(e) else {
            continue;
        };
        if now >= burst.expires_at {
            spent.push(e);
        }
    }
    let _ = world.despawn_many(&spent);
}

/// Keeps the [`BallGlow`] light glued to the ball (x/y; its z offset stays as the scene authored it).
pub fn glow_follows_ball(world: &mut World, _ctx: &SystemCtx) {
    if is_paused(world) {
        return;
    }
    let ball = world
        .query::<(&Ball, &Transform)>()
        .next()
        .map(|(_, t)| [t.translation[0], t.translation[1]]);
    let Some([bx, by]) = ball else { return };
    for (_glow, transform) in world.query::<(&BallGlow, &mut Transform)>() {
        transform.translation[0] = bx;
        transform.translation[1] = by;
    }
}

/// Decays the glow's intensity exponentially back toward [`GLOW_BASE`] after a flash.
pub fn glow_decay(world: &mut World, ctx: &SystemCtx) {
    if is_paused(world) {
        return;
    }
    let keep = 1.0 - GLOW_DECAY * ctx.dt; // per-tick retention; plain arithmetic, deterministic
    for (light, _glow) in world.query::<(&mut PointLight, &BallGlow)>() {
        light.intensity = GLOW_BASE + (light.intensity - GLOW_BASE) * keep;
    }
}

// --- Wiring ---------------------------------------------------------------------------------------

/// Registers this game's components on top of a registry that already holds the built-ins.
///
/// # Errors
/// An `L0005` diagnostic if a name collides with a built-in or another game component.
pub fn register(registry: &mut Registry) -> Result<(), Diagnostic> {
    registry.register::<Paddle>("Paddle")?;
    registry.register::<Ball>("Ball")?;
    registry.register::<Brick>("Brick")?;
    registry.register::<BallGlow>("BallGlow")?;
    registry.register::<PauseMenu>("PauseMenu")?;
    registry.register::<BrickBurst>("BrickBurst")
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

/// Describes the engine that hosts this game. Registration order is execution order: menu input →
/// paddle input → ball physics → brick destruction (flash + spark burst) → reap spent bursts → light
/// follows ball → flash decay. The menu runs first, so the pause it opens takes effect on the very
/// tick it opens.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(register)
        .system("menu_input", menu_input)
        .system("move_paddle", move_paddle)
        .system("move_ball", move_ball)
        .system("break_bricks", break_bricks)
        .system("reap_bursts", reap_bursts)
        .system("glow_follows_ball", glow_follows_ball)
        .system("glow_decay", glow_decay)
        .default_scene("scenes/main.yaml")
}
