//! The determinism harness (implementation guide, Phase 1 exit criteria).
//!
//! Pins the three claims `ludion-sim` exists to make:
//!
//! 1. **Double run** — the same initial world + seed + input script, built twice in-process,
//!    produces *identical per-frame `WorldHash` sequences* (catches any per-instance
//!    nondeterminism: RandomState leaks, iteration-order drift, wall-clock contamination).
//! 2. **Scripted gameplay** — an input script drives a headless, assertable gameplay outcome
//!    ("the ball crosses `x > 3.0` by frame 30") with no renderer and no human.
//! 3. **Hash sensitivity** — any observable change to world content (one component field by one
//!    ULP, a removed component, a recycled entity generation) changes the hash; anything that must
//!    not matter (pause/step interleaving, speed) does not.

use ludion_core::codes::code;
use ludion_core::rng::DeterministicRng;
use ludion_core::time::FrameIndex;
use ludion_ecs::{Registry, Schedule, SystemCtx, World};
use ludion_sim::{
    GamepadAxis, GamepadButton, InputEvent, InputScript, InputState, Key, MouseButton, RunState,
    SimConfig, SimDriver, Speed, WorldHash,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- Test components ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct Position {
    x: f32,
    y: f32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct Velocity {
    dx: f32,
    dy: f32,
}

/// A field driven purely by the world's `DeterministicRng`, so seed divergence shows in the hash.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct Jitter(f32);

// --- Test systems (explicit registration order: steer, integrate, jitter) -----------------------

/// `D` held drives +10 units/s on x, `A` held −10; neither held stops the entity.
fn steer(world: &mut World, _ctx: &SystemCtx) {
    let (right, left) = {
        let input = world.resource::<InputState>().unwrap();
        (input.is_held(Key::D), input.is_held(Key::A))
    };
    let dx = match (right, left) {
        (true, false) => 10.0,
        (false, true) => -10.0,
        _ => 0.0,
    };
    for (vel,) in world.query::<(&mut Velocity,)>() {
        vel.dx = dx;
    }
}

fn integrate(world: &mut World, ctx: &SystemCtx) {
    for (pos, vel) in world.query::<(&mut Position, &Velocity)>() {
        pos.x += vel.dx * ctx.dt;
        pos.y += vel.dy * ctx.dt;
    }
}

/// Draws from the single seeded RNG resource every tick (`DeterministicRng` is `Copy`, so it is
/// copied out, advanced, and written back — the canonical pattern while a query is borrowing).
fn jitter(world: &mut World, _ctx: &SystemCtx) {
    let mut rng = *world.resource::<DeterministicRng>().unwrap();
    for (j,) in world.query::<(&mut Jitter,)>() {
        j.0 = rng.next_f32();
    }
    world.insert_resource(rng);
}

// --- Fixture -----------------------------------------------------------------------------------

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register::<Position>("Position").unwrap();
    registry.register::<Velocity>("Velocity").unwrap();
    registry.register::<Jitter>("Jitter").unwrap();
    registry
}

/// "Press D at frame 0, release at frame 20."
fn dash_script() -> InputScript {
    let mut script = InputScript::new();
    script.push(FrameIndex::new(0), InputEvent::KeyDown { key: Key::D });
    script.push(FrameIndex::new(20), InputEvent::KeyUp { key: Key::D });
    script
}

/// Builds the identical game twice over: one moving "ball" plus a jittering bystander,
/// steer → integrate → jitter, seeded from `seed`, driven by `script`.
fn fixture(seed: u64, script: InputScript) -> (SimDriver, ludion_core::ids::EntityId) {
    let mut world = World::new();
    let ball = world.spawn();
    world.insert(ball, Position { x: 0.0, y: 0.0 }).unwrap();
    world.insert(ball, Velocity { dx: 0.0, dy: 0.0 }).unwrap();
    let bystander = world.spawn();
    world.insert(bystander, Jitter(0.0)).unwrap();

    let mut schedule = Schedule::new();
    schedule.add_system("steer", steer);
    schedule.add_system("integrate", integrate);
    schedule.add_system("jitter", jitter);

    let config = SimConfig {
        seed,
        input: script,
        ..SimConfig::default()
    };
    (SimDriver::new(world, schedule, config), ball)
}

/// Runs `ticks` ticks, returning the `WorldHash` after every tick.
fn hash_sequence(driver: &mut SimDriver, registry: &Registry, ticks: u64) -> Vec<WorldHash> {
    let mut hashes = Vec::with_capacity(usize::try_from(ticks).unwrap());
    for _ in 0..ticks {
        driver.run_ticks(1);
        hashes.push(WorldHash::compute(driver.world(), registry).unwrap());
    }
    hashes
}

// --- 1. In-process double run ------------------------------------------------------------------

#[test]
fn double_run_produces_identical_per_frame_hash_sequences() {
    let registry = registry();
    let (mut a, _) = fixture(42, dash_script());
    let (mut b, _) = fixture(42, dash_script());

    let hashes_a = hash_sequence(&mut a, &registry, 60);
    let hashes_b = hash_sequence(&mut b, &registry, 60);

    assert_eq!(
        hashes_a, hashes_b,
        "same seed + script must be bit-identical"
    );
    // Sanity: the hash tracks a live, changing world — not a constant.
    assert_ne!(hashes_a.first(), hashes_a.last());
}

#[test]
fn different_seed_diverges_and_different_script_diverges() {
    let registry = registry();

    // Different RNG seed, same script: the Jitter component separates the runs.
    let (mut a, _) = fixture(1, dash_script());
    let (mut b, _) = fixture(2, dash_script());
    assert_ne!(
        hash_sequence(&mut a, &registry, 10),
        hash_sequence(&mut b, &registry, 10),
    );

    // Same seed, different script: inputs are part of the deterministic state evolution.
    let (mut c, _) = fixture(1, dash_script());
    let (mut d, _) = fixture(1, InputScript::new());
    assert_ne!(
        hash_sequence(&mut c, &registry, 10),
        hash_sequence(&mut d, &registry, 10),
    );
}

// --- 2. Scripted gameplay assertion ------------------------------------------------------------

#[test]
fn scripted_dash_crosses_x_3_by_frame_30_then_stops() {
    let registry = registry();
    let (mut driver, ball) = fixture(42, dash_script());

    // Tick one frame at a time, watching for the crossing.
    let mut crossed_at = None;
    while driver.frame().as_u64() < 30 {
        driver.run_ticks(1);
        let x = driver.world().get::<Position>(ball).unwrap().unwrap().x;
        if crossed_at.is_none() && x > 3.0 {
            crossed_at = Some(driver.frame().as_u64());
        }
    }
    let crossed_at = crossed_at.expect("ball must cross x > 3.0 by frame 30");
    assert!(crossed_at <= 30, "crossed at frame {crossed_at}");

    // D was released at frame 20: 20 ticks × 10/60 ≈ 3.33 units, then the ball stops dead.
    let x_at_30 = driver.world().get::<Position>(ball).unwrap().unwrap().x;
    assert!((x_at_30 - 10.0 * 20.0 / 60.0).abs() < 1e-4, "x = {x_at_30}");

    driver.run_ticks(10);
    let x_at_40 = driver.world().get::<Position>(ball).unwrap().unwrap().x;
    assert_eq!(x_at_30, x_at_40, "no drift after release — bitwise equal");

    // The registry round-trip view agrees (this is what `sim.hash` and scenes serialize).
    let hash_30 = WorldHash::compute(driver.world(), &registry).unwrap();
    let hash_40 = WorldHash::compute(driver.world(), &registry).unwrap();
    assert_eq!(hash_30, hash_40);
}

// --- 3. Hash sensitivity -----------------------------------------------------------------------

/// Two-entity world used for pure hash-sensitivity checks (no driver involved).
fn static_world() -> (World, ludion_core::ids::EntityId) {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Position { x: 1.0, y: 2.0 }).unwrap();
    world.insert(e, Velocity { dx: 0.0, dy: 0.0 }).unwrap();
    (world, e)
}

#[test]
fn identical_worlds_hash_identically() {
    let registry = registry();
    let (world_a, _) = static_world();
    let (world_b, _) = static_world();
    assert_eq!(
        WorldHash::compute(&world_a, &registry).unwrap(),
        WorldHash::compute(&world_b, &registry).unwrap(),
    );
}

#[test]
fn one_ulp_component_change_changes_the_hash() {
    let registry = registry();
    let (world_a, _) = static_world();
    let (mut world_b, e) = static_world();

    let pos = world_b.get_mut::<Position>(e).unwrap().unwrap();
    pos.x = f32::from_bits(pos.x.to_bits() + 1); // smallest representable change

    assert_ne!(
        WorldHash::compute(&world_a, &registry).unwrap(),
        WorldHash::compute(&world_b, &registry).unwrap(),
    );
}

#[test]
fn negative_zero_is_distinguished_from_zero() {
    let registry = registry();
    let (world_a, _) = static_world();
    let (mut world_b, e) = static_world();
    world_b.get_mut::<Velocity>(e).unwrap().unwrap().dx = -0.0;

    assert_ne!(
        WorldHash::compute(&world_a, &registry).unwrap(),
        WorldHash::compute(&world_b, &registry).unwrap(),
        "-0.0 and 0.0 are different bit patterns and must hash differently",
    );
}

#[test]
fn removed_component_changes_the_hash() {
    let registry = registry();
    let (world_a, _) = static_world();
    let (mut world_b, e) = static_world();
    world_b.remove::<Velocity>(e).unwrap();

    assert_ne!(
        WorldHash::compute(&world_a, &registry).unwrap(),
        WorldHash::compute(&world_b, &registry).unwrap(),
    );
}

#[test]
fn recycled_entity_generation_changes_the_hash() {
    let registry = registry();

    // Same slot, same component data — but one world despawned and respawned, bumping the
    // generation. Entity identity is world content; the hash must see it.
    let (world_a, _) = static_world();

    let mut world_b = World::new();
    let first = world_b.spawn();
    world_b.despawn(first).unwrap();
    let e = world_b.spawn(); // slot 0, generation 1
    world_b.insert(e, Position { x: 1.0, y: 2.0 }).unwrap();
    world_b.insert(e, Velocity { dx: 0.0, dy: 0.0 }).unwrap();

    assert_ne!(
        WorldHash::compute(&world_a, &registry).unwrap(),
        WorldHash::compute(&world_b, &registry).unwrap(),
    );
}

#[test]
fn unregistered_components_are_invisible_to_the_hash() {
    // A component the registry does not know cannot appear in scenes, snapshots, or the wire —
    // and therefore not in the hash either. (Register everything that is world state.)
    let registry = registry();
    let (world_a, _) = static_world();
    let (mut world_b, e) = static_world();

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct DebugOnly(u32);
    world_b.insert(e, DebugOnly(7)).unwrap();

    assert_eq!(
        WorldHash::compute(&world_a, &registry).unwrap(),
        WorldHash::compute(&world_b, &registry).unwrap(),
    );
}

// --- Run-state and speed must never affect tick semantics ---------------------------------------

#[test]
fn pause_step_interleaving_matches_a_straight_run() {
    let registry = registry();

    let (mut straight, _) = fixture(42, dash_script());
    straight.run_ticks(30);

    let (mut chopped, _) = fixture(42, dash_script());
    chopped.run_ticks(7);
    chopped.pause();
    assert!(!chopped.advance());
    chopped.step(13);
    while chopped.advance() {}
    assert_eq!(chopped.run_state(), RunState::Paused);
    chopped.run_ticks(10); // 7 + 13 + 10 = 30

    assert_eq!(straight.frame(), chopped.frame());
    assert_eq!(
        WorldHash::compute(straight.world(), &registry).unwrap(),
        WorldHash::compute(chopped.world(), &registry).unwrap(),
        "pause/step choreography must not change what ticks compute",
    );
}

#[test]
fn speed_never_affects_the_hash_sequence() {
    let registry = registry();
    let (mut slow, _) = fixture(42, dash_script());
    slow.set_speed(Speed::new(0.25).unwrap());
    let (mut fast, _) = fixture(42, dash_script());
    fast.set_speed(Speed::new(8.0).unwrap());

    assert_eq!(
        hash_sequence(&mut slow, &registry, 30),
        hash_sequence(&mut fast, &registry, 30),
        "speed is wall-clock pacing only; tick semantics are untouchable",
    );
}

#[test]
fn invalid_speed_is_a_structured_l4001() {
    let err = Speed::new(f64::NAN).unwrap_err();
    assert_eq!(err.code, code::INVALID_SIM_SPEED);
    assert_eq!(err.code.as_str(), "L4001");
    assert!(err.help.is_some(), "diagnostics carry the likely fix");
}

// --- 4. Analog input (mouse/gamepad) bit-exact determinism (Phase 13 Wave 2) --------------------
//
// Wave 1 encodes every analog InputEvent field as an IEEE-754 bit pattern rather than a JSON
// number (input.rs's `f32_bits`/`f32x2_bits`); Wave 2 proves it end to end through a real driver
// run: an InputScript carrying mouse-move, mouse-wheel, and gamepad-axis events with awkward exact
// bit patterns (subnormals, a mantissa-precision boundary, MIN_POSITIVE) drives a system that
// copies those analog fields into a hashed component, and two runs of the same script — plus a
// third run built from the script's own re-serialized-then-reloaded form — all produce the
// identical WorldHash trace.

/// Mirrors this frame's analog input state exactly, so the world hash is sensitive to every last
/// bit of mouse/gamepad state that reached it through `InputState::apply`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct Aim {
    mouse_x: f32,
    mouse_y: f32,
    wheel_x: f32,
    wheel_y: f32,
    axis: f32,
}

fn record_aim(world: &mut World, _ctx: &SystemCtx) {
    let (position, wheel, axis) = {
        let input = world.resource::<InputState>().unwrap();
        (
            input.mouse().position(),
            input.mouse().wheel(),
            input.gamepad().left_stick()[0],
        )
    };
    for (aim,) in world.query::<(&mut Aim,)>() {
        aim.mouse_x = position[0];
        aim.mouse_y = position[1];
        aim.wheel_x = wheel[0];
        aim.wheel_y = wheel[1];
        aim.axis = axis;
    }
}

fn aim_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register::<Aim>("Aim").unwrap();
    registry
}

/// Awkward exact f32 bit patterns spread across mouse-move, mouse-wheel, and gamepad-axis events —
/// subnormals, `MIN_POSITIVE`, and a value at the 24-bit mantissa-precision boundary — the kind of
/// bit pattern a naive decimal (JSON-number) encoding could double-round through `f64` (the risk
/// the `f32_bits` encoding closes; this test proves the *driver* path preserves the bits it carries
/// regardless of how "awkward" the value is).
fn awkward_analog_script() -> InputScript {
    let mut script = InputScript::new();
    script.push(
        FrameIndex::new(0),
        InputEvent::MouseMove {
            position: [f32::from_bits(1), -f32::MIN_POSITIVE],
        },
    );
    script.push(
        FrameIndex::new(1),
        InputEvent::MouseWheel {
            delta: [16_777_216.0, f32::from_bits(0x0000_0001)],
        },
    );
    script.push(
        FrameIndex::new(1),
        InputEvent::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            value: f32::from_bits(0x3f7f_ffff), // just under 1.0
        },
    );
    script.push(
        FrameIndex::new(5),
        InputEvent::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            value: -f32::MIN_POSITIVE,
        },
    );
    script
}

fn aim_fixture(script: InputScript) -> (SimDriver, ludion_core::ids::EntityId) {
    let mut world = World::new();
    let aim = world.spawn();
    world
        .insert(
            aim,
            Aim {
                mouse_x: 0.0,
                mouse_y: 0.0,
                wheel_x: 0.0,
                wheel_y: 0.0,
                axis: 0.0,
            },
        )
        .unwrap();

    let mut schedule = Schedule::new();
    schedule.add_system("record_aim", record_aim);

    let config = SimConfig {
        seed: 1,
        input: script,
        ..SimConfig::default()
    };
    (SimDriver::new(world, schedule, config), aim)
}

#[test]
fn analog_bit_patterns_replay_identically_across_two_runs() {
    let registry = aim_registry();
    let (mut a, _) = aim_fixture(awkward_analog_script());
    let (mut b, _) = aim_fixture(awkward_analog_script());

    let hashes_a = hash_sequence(&mut a, &registry, 10);
    let hashes_b = hash_sequence(&mut b, &registry, 10);
    assert_eq!(
        hashes_a, hashes_b,
        "the same awkward-bit-pattern script must drive two runs to an identical hash trace"
    );
    // Sanity: the analog fields actually reach the hashed component (not a vacuous pass).
    assert_ne!(hashes_a.first(), hashes_a.last());
}

/// What a desktop host injects when the window loses focus mid-hold (and the primary pad is
/// unplugged): every held key, mouse button, axis and button released. The right trigger's button
/// is synthesised from its axis, so the host never sends a trigger button.
fn held_then_released_script() -> InputScript {
    let mut script = InputScript::new();
    let at = FrameIndex::new;
    for key in [Key::W, Key::Space] {
        script.push(at(0), InputEvent::KeyDown { key });
        script.push(at(5), InputEvent::KeyUp { key });
    }
    script.push(
        at(0),
        InputEvent::MouseDown {
            button: MouseButton::Left,
        },
    );
    script.push(
        at(5),
        InputEvent::MouseUp {
            button: MouseButton::Left,
        },
    );
    for (axis, down, up) in [
        (GamepadAxis::LeftStickX, 1.0, 0.0),
        (GamepadAxis::RightTrigger, 0.8, 0.0),
    ] {
        script.push(at(0), InputEvent::GamepadAxis { axis, value: down });
        script.push(at(5), InputEvent::GamepadAxis { axis, value: up });
    }
    script.push(
        at(0),
        InputEvent::GamepadButtonDown {
            button: GamepadButton::South,
        },
    );
    script.push(
        at(5),
        InputEvent::GamepadButtonUp {
            button: GamepadButton::South,
        },
    );
    script
}

#[test]
fn host_releases_leave_no_input_held_and_replay_identically() {
    let registry = aim_registry();
    let (mut a, _) = aim_fixture(held_then_released_script());
    let (mut b, _) = aim_fixture(held_then_released_script());

    let hashes_a = hash_sequence(&mut a, &registry, 10);
    assert_eq!(hashes_a, hash_sequence(&mut b, &registry, 10));
    // Non-vacuous: the stick X the release zeroes is hashed, so the trace moves at frame 5.
    assert_ne!(hashes_a[4], hashes_a[5]);

    let (mut c, _) = aim_fixture(held_then_released_script());
    c.run_ticks(1);
    let held = c.world().resource::<InputState>().unwrap();
    assert!(held.is_held(Key::W) && held.is_held(Key::Space));
    assert!(held.mouse().is_held(MouseButton::Left));
    assert!(held.gamepad().is_held(GamepadButton::RightTrigger));

    let input = a.world().resource::<InputState>().unwrap();
    assert!(!input.is_held(Key::W) && !input.is_held(Key::Space));
    assert!(!input.mouse().any_held());
    assert!(!input.gamepad().any_held());
    assert!(!input.gamepad().is_held(GamepadButton::RightTrigger));
    assert_eq!(input.gamepad().left_stick(), [0.0, 0.0]);
    assert_eq!(input.gamepad().right_trigger(), 0.0);
}

#[test]
fn analog_input_script_matches_after_serialize_reload() {
    let registry = aim_registry();
    let script = awkward_analog_script();

    let json = serde_json::to_string(&script).unwrap();
    let reloaded: InputScript = serde_json::from_str(&json).unwrap();
    assert_eq!(
        script, reloaded,
        "the script's own canonical serde form must round-trip exactly"
    );

    let (mut original, _) = aim_fixture(script);
    let (mut from_reload, _) = aim_fixture(reloaded);
    assert_eq!(
        hash_sequence(&mut original, &registry, 10),
        hash_sequence(&mut from_reload, &registry, 10),
        "a script reloaded from its own JSON serde form must replay to the identical hash trace"
    );
}
