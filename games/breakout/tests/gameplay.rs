//! Headless gameplay + determinism tests for the Breakout clone, driven through the real `Engine`
//! built from this game's own `EngineBuilder`. No GPU needed.
//!
//! Covers the core mechanics: paddle input (and wall clamping), ball bounces (wall, ceiling,
//! paddle-with-english), brick destruction, the gameplay-driven dynamic lighting (flash + decay),
//! ball re-serve, and WorldHash determinism over an input-driven session.

use breakout::{
    ARENA_BOTTOM, ARENA_HALF_WIDTH, BRICK_BURST_LIFE, Ball, BallGlow, Brick, BrickBurst, GLOW_BASE,
    PADDLE_HALF, PADDLE_SPEED, Paddle, SERVE_VELOCITY,
};
use ludion::prelude::*;
use ludion_testkit::GameplayDriver;

/// The game scene, embedded so these tests are self-contained.
const MAIN_SCENE: &str = include_str!("../scenes/main.yaml");

/// A fresh driver over this game's config with `scene` loaded inline.
fn driver_with(scene: &str) -> GameplayDriver {
    GameplayDriver::new(breakout::game(), scene)
}

/// The paddle's x position.
fn paddle_x(d: &mut GameplayDriver) -> f32 {
    let mut x = None;
    for (_p, t) in d.engine_mut().world_mut().query::<(&Paddle, &Transform)>() {
        x = Some(t.translation[0]);
    }
    x.expect("the scene has a Paddle entity")
}

/// The ball's (position, velocity).
fn ball_state(d: &mut GameplayDriver) -> ([f32; 2], [f32; 2]) {
    let mut state = None;
    for (b, t) in d.engine_mut().world_mut().query::<(&Ball, &Transform)>() {
        state = Some(([t.translation[0], t.translation[1]], b.velocity));
    }
    state.expect("the scene has a Ball entity")
}

/// How many bricks are still alive.
fn brick_count(d: &mut GameplayDriver) -> usize {
    d.engine_mut().world_mut().query::<(&Brick,)>().count()
}

/// The BallGlow light's intensity.
fn glow_intensity(d: &mut GameplayDriver) -> f32 {
    let mut i = None;
    for (light, _g) in d
        .engine_mut()
        .world_mut()
        .query::<(&PointLight, &BallGlow)>()
    {
        i = Some(light.intensity);
    }
    i.expect("the scene has a BallGlow light")
}

/// How many `BrickBurst` entities are currently live.
fn burst_count(d: &mut GameplayDriver) -> usize {
    d.engine_mut().world_mut().query::<(&BrickBurst,)>().count()
}

/// The one live burst's `ParticleEmitter.started` frame.
fn burst_started(d: &mut GameplayDriver) -> u64 {
    let mut started = None;
    for (_burst, emitter) in d
        .engine_mut()
        .world_mut()
        .query::<(&BrickBurst, &ParticleEmitter)>()
    {
        started = Some(emitter.started.as_u64());
    }
    started.expect("the scene has a BrickBurst entity with a ParticleEmitter")
}

// --- Paddle input ---------------------------------------------------------------------------------

/// Holding Right slides the paddle right at `PADDLE_SPEED`; switching to Left brings it back.
#[test]
fn paddle_follows_left_and_right_input() {
    let mut d = driver_with(MAIN_SCENE);
    d.inject("key_down", "Right");
    d.step(20);
    let expected = PADDLE_SPEED * 20.0 / 60.0;
    let x = paddle_x(&mut d);
    assert!(
        (x - expected).abs() < 1e-3,
        "paddle should be at ~{expected} after 20 ticks of Right, got {x}"
    );

    d.inject("key_up", "Right");
    d.inject("key_down", "Left");
    d.step(20);
    let x = paddle_x(&mut d);
    assert!(
        x.abs() < 1e-3,
        "paddle should be back at ~0 after 20 ticks of Left, got {x}"
    );
}

/// The paddle clamps at the arena wall instead of leaving the playfield.
#[test]
fn paddle_clamps_at_the_wall() {
    let mut d = driver_with(MAIN_SCENE);
    d.inject("key_down", "Left");
    d.step(240); // far more than enough to reach the wall
    let limit = -(ARENA_HALF_WIDTH - PADDLE_HALF[0]);
    assert_eq!(
        paddle_x(&mut d),
        limit,
        "paddle must clamp exactly at the wall"
    );
}

// --- Ball physics ---------------------------------------------------------------------------------

/// A ball heading right reflects off the right wall and stays inside the arena.
#[test]
fn ball_bounces_off_the_side_wall() {
    const SCENE: &str = "format: 1
seed: 0
entities:
  - name: ball
    components:
      Transform:
        translation:
          - 4.0
          - 0.0
          - 0.0
      Ball:
        velocity:
          - 4.0
          - 0.0
";
    let mut d = driver_with(SCENE);
    d.step(30);
    let ([x, _y], [vx, _vy]) = ball_state(&mut d);
    assert!(
        vx < 0.0,
        "ball should have reflected off the right wall, vx = {vx}"
    );
    assert!(
        x <= ARENA_HALF_WIDTH,
        "ball must stay inside the arena, x = {x}"
    );
}

/// A ball heading up reflects off the ceiling.
#[test]
fn ball_bounces_off_the_ceiling() {
    const SCENE: &str = "format: 1
seed: 0
entities:
  - name: ball
    components:
      Transform:
        translation:
          - 0.0
          - 3.5
          - 0.0
      Ball:
        velocity:
          - 0.0
          - 4.0
";
    let mut d = driver_with(SCENE);
    d.step(30);
    let (_pos, [_vx, vy]) = ball_state(&mut d);
    assert!(
        vy < 0.0,
        "ball should have reflected off the ceiling, vy = {vy}"
    );
}

/// A descending ball rebounds off the paddle, and an off-center hit imparts english (steers vx).
#[test]
fn ball_bounces_off_the_paddle_with_english() {
    const SCENE: &str = "format: 1
seed: 0
entities:
  - name: paddle
    components:
      Transform:
        translation:
          - 0.0
          - -4.0
          - 0.0
      Paddle: {}
  - name: ball
    components:
      Transform:
        translation:
          - 0.5
          - -3.3
          - 0.0
      Ball:
        velocity:
          - 0.0
          - -4.0
";
    let mut d = driver_with(SCENE);
    d.step(30);
    let (_pos, [vx, vy]) = ball_state(&mut d);
    assert!(
        vy > 0.0,
        "ball should rebound upward off the paddle, vy = {vy}"
    );
    assert!(
        vx > 0.0,
        "a hit right of paddle center must steer the ball right, vx = {vx}"
    );
}

/// A ball that falls below the arena re-serves from the serve point with the serve velocity.
#[test]
fn lost_ball_reserves() {
    const SCENE: &str = "format: 1
seed: 0
entities:
  - name: ball
    components:
      Transform:
        translation:
          - 3.0
          - -5.0
          - 0.0
      Ball:
        velocity:
          - 0.0
          - -6.0
";
    let mut d = driver_with(SCENE);
    d.step(30);
    let ([_x, y], velocity) = ball_state(&mut d);
    assert_eq!(
        velocity, SERVE_VELOCITY,
        "a lost ball must re-serve with the serve velocity"
    );
    assert!(
        y > ARENA_BOTTOM,
        "the re-served ball is back in play, y = {y}"
    );
}

// --- Bricks + dynamic lighting ----------------------------------------------------------------------

/// The ball destroys a brick it hits, rebounds, and the destruction spikes the BallGlow light —
/// which then decays back to its resting intensity. Gameplay drives the lighting; both are hashed
/// component state.
#[test]
fn brick_breaks_flash_lights_and_decays() {
    const SCENE: &str = "format: 1
seed: 0
entities:
  - name: brick
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - 0.0
      Brick: {}
  - name: ball
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Ball:
        velocity:
          - 0.0
          - 4.0
  - name: ball-glow
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 1.2
      PointLight:
        intensity: 1.2
        range: 8.0
      BallGlow: {}
";
    let mut d = driver_with(SCENE);
    assert_eq!(brick_count(&mut d), 1);
    assert_eq!(glow_intensity(&mut d), GLOW_BASE);

    // The ball needs ~9 ticks to reach the brick; 15 leaves the flash only a few decay ticks old.
    d.step(15);
    assert_eq!(brick_count(&mut d), 0, "the brick must be destroyed");
    let (_pos, [_vx, vy]) = ball_state(&mut d);
    assert!(vy < 0.0, "the ball should rebound off the brick, vy = {vy}");
    let flashed = glow_intensity(&mut d);
    assert!(
        flashed > GLOW_BASE * 2.0,
        "brick destruction must spike the glow light, intensity = {flashed}"
    );

    // The flash decays back toward the resting intensity.
    d.step(120);
    let settled = glow_intensity(&mut d);
    assert!(
        (GLOW_BASE..GLOW_BASE + 0.05).contains(&settled),
        "the flash must decay back toward GLOW_BASE, intensity = {settled}"
    );
}

/// Breaking a brick spawns exactly one `BrickBurst` particle-emitter entity, stamped `started` at
/// the exact frame the break happened — and it is reaped once its particles finish aging out.
#[test]
fn brick_break_spawns_a_burst_that_reaps_after_its_lifetime() {
    const SCENE: &str = "format: 1
seed: 0
entities:
  - name: brick
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - 0.0
      Sprite:
        size:
          - 1.6
          - 0.6
        color:
          - 1.0
          - 0.3
          - 0.3
          - 1.0
      Brick: {}
  - name: ball
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Ball:
        velocity:
          - 0.0
          - 4.0
";
    let mut d = driver_with(SCENE);
    assert_eq!(burst_count(&mut d), 0, "no burst before the break");

    // Step one tick at a time so the exact frame the brick breaks on is known. `d.frame()` counts
    // ticks *executed so far*, while the `SystemCtx.frame` a system reads during the Nth tick is
    // `N - 1` (`ludion_sim::driver`'s tick contract) — the break system stamps the latter.
    let mut break_frame = None;
    for _ in 0..15 {
        d.step(1);
        if brick_count(&mut d) == 0 {
            break_frame = Some(d.frame() - 1);
            break;
        }
    }
    let break_frame = break_frame.expect("the brick must break within 15 ticks");

    assert_eq!(
        burst_count(&mut d),
        1,
        "the break must spawn exactly one BrickBurst"
    );
    assert_eq!(
        burst_started(&mut d),
        break_frame,
        "the burst's ParticleEmitter must stamp `started` at the break frame"
    );

    // Past the longest particle's lifetime the burst must be gone.
    let reap_ticks = (BRICK_BURST_LIFE[1] * 60.0).ceil() as u64 + 5;
    d.step(reap_ticks);
    assert_eq!(
        burst_count(&mut d),
        0,
        "the burst must be reaped once its particles have aged out"
    );
}

/// Playing the real scene long enough destroys bricks — the systems compose on the shipped board.
#[test]
fn main_scene_play_destroys_bricks() {
    let mut d = driver_with(MAIN_SCENE);
    assert_eq!(brick_count(&mut d), 15, "the board starts with 15 bricks");
    d.step(600); // 10 seconds of free play (serve aims at the bricks)
    assert!(
        brick_count(&mut d) < 15,
        "ten seconds of play must break at least one brick"
    );
}

// --- Determinism ------------------------------------------------------------------------------------

/// The command script the determinism test drives: an input-heavy session over the real board.
fn session_commands() -> Vec<Command> {
    let inject = |kind: &str, key: &str| {
        Command::InputInject(command::InputInjectParams {
            frame: None,
            event: serde_json::json!({"kind": kind, "key": key}),
        })
    };
    vec![
        inject("key_down", "Right"),
        Command::SimStep(command::SimStepParams {
            ticks: 90,
            hash: false,
        }),
        inject("key_up", "Right"),
        inject("key_down", "Left"),
        Command::SimStep(command::SimStepParams {
            ticks: 90,
            hash: false,
        }),
        inject("key_up", "Left"),
        Command::SimStep(command::SimStepParams {
            ticks: 180,
            hash: false,
        }),
    ]
}

/// Two identical runs produce identical per-frame `WorldHash` sequences: the whole game —
/// paddle, ball, bricks, and the dynamic lighting — is deterministic.
#[test]
fn two_runs_produce_identical_worldhash_sequences() {
    let run = || {
        let mut engine = ludion_testkit::loaded_engine(breakout::game(), MAIN_SCENE);
        ludion_engine::drive_hashes(&mut engine, session_commands()).expect("drive succeeds")
    };
    assert_eq!(run(), run(), "the simulation must be deterministic");
}
