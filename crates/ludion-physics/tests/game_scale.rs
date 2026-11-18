//! A gameplay scale (`GameClock::scale`) drives the physics step with no game call: the step runs at
//! `base × scale`, composes by minimum with the game's own `scale_time`, and a hold does not reach it.

#[allow(dead_code)]
// the plain-context `tick` is unused: a scale needs the driver's clocked context
mod common;
#[path = "common/ragdoll.rs"]
mod ragdoll;

use common::pose_of;
use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, GameClock, TickRate};
use ludion_ecs::{Schedule, SystemCtx, SystemPanic, World};
use ludion_physics::{PhysicsSettings, PhysicsSpace};
use ludion_spatial::Shape;
use ragdoll::{physics, rigid, schedule_with, spawn};

fn base_dt() -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    let dt = TickRate::DEFAULT.seconds_per_tick() as f32;
    dt
}

fn drifter() -> (World, Schedule, EntityId) {
    let mut world = World::new();
    world.insert_resource(GameClock::default());
    let ball = spawn(
        &mut world,
        Shape::Sphere { radius: 0.5 },
        rigid(1.0),
        [0.0; 3],
        [2.0, 0.0, 0.0],
    );
    let schedule = schedule_with(PhysicsSettings {
        gravity: [0.0; 3],
        ..PhysicsSettings::new(PhysicsSpace::Free)
    });
    (world, schedule, ball)
}

/// One tick the way the sim driver runs it: the clock folds first, then the schedule sees it.
/// `scale` is requested on the clock before the fold; `hold` marks the frame held for the next.
fn tick(world: &mut World, schedule: &mut Schedule, frame: u64, scale: Option<f32>, hold: bool) {
    let (clock, advanced) = {
        let clock = world.resource_mut::<GameClock>().unwrap();
        if let Some(scale) = scale {
            clock.scale(scale);
        }
        if hold {
            clock.hold();
        }
        let advanced = clock.advance();
        (*clock, advanced)
    };
    let ctx =
        SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT).with_clock(&clock, advanced);
    schedule.run(world, &ctx);
    if let Some(panic) = world.resource::<SystemPanic>() {
        panic!("{} panicked: {}", panic.system, panic.message);
    }
}

fn x_of(world: &World, entity: EntityId) -> f32 {
    pose_of(world, entity).translation[0]
}

#[test]
fn a_scaled_clock_steps_physics_at_the_scaled_timestep_every_frame() {
    let (mut world, mut schedule, ball) = drifter();
    tick(&mut world, &mut schedule, 0, None, false);
    for frame in 1..=8 {
        let before = x_of(&world, ball);
        tick(&mut world, &mut schedule, frame, Some(0.25), false);
        assert_eq!(physics(&mut world).last_step_dt(), base_dt() * 0.25);
        let moved = x_of(&world, ball) - before;
        assert!((moved - 2.0 * base_dt() * 0.25).abs() < 1e-5, "{moved}");
    }
    tick(&mut world, &mut schedule, 9, None, false);
    assert_eq!(physics(&mut world).last_step_dt(), base_dt());
}

#[test]
fn an_unscaled_clock_steps_at_the_base_timestep_bit_exactly() {
    let (mut world, mut schedule, _ball) = drifter();
    for frame in 0..5 {
        tick(&mut world, &mut schedule, frame, None, false);
        assert_eq!(physics(&mut world).last_step_dt(), base_dt());
    }
}

#[test]
fn a_held_tick_still_steps_at_the_base_timestep() {
    let (mut world, mut schedule, ball) = drifter();
    tick(&mut world, &mut schedule, 0, None, false);
    let before = x_of(&world, ball);
    // The hold marks this frame; the held tick runs with a scale request pending too.
    tick(&mut world, &mut schedule, 1, Some(0.25), true);
    assert_eq!(physics(&mut world).last_step_dt(), base_dt());
    assert!((x_of(&world, ball) - before - 2.0 * base_dt()).abs() < 1e-5);
}

#[test]
fn the_games_own_scale_time_wins_when_smaller() {
    let (mut world, mut schedule, _ball) = drifter();
    tick(&mut world, &mut schedule, 0, None, false);
    physics(&mut world).scale_time(0.1).expect("in range");
    tick(&mut world, &mut schedule, 1, Some(0.25), false);
    assert_eq!(physics(&mut world).last_step_dt(), base_dt() * 0.1);

    physics(&mut world).scale_time(0.5).expect("in range");
    tick(&mut world, &mut schedule, 2, Some(0.25), false);
    assert_eq!(physics(&mut world).last_step_dt(), base_dt() * 0.25);
}

#[test]
fn the_smallest_clock_scale_clamps_to_the_solvers_floor() {
    let (mut world, mut schedule, _ball) = drifter();
    tick(&mut world, &mut schedule, 0, None, false);
    tick(
        &mut world,
        &mut schedule,
        1,
        Some(GameClock::MIN_SCALE),
        false,
    );
    assert_eq!(
        physics(&mut world).last_step_dt(),
        base_dt() * ludion_physics::MIN_TIME_SCALE
    );
}
