//! The physics time scale: `scale_time` runs the step at a fraction of the fixed timestep, `freeze`
//! skips it, both per frame and reset by the step; a frozen frame still applies queued impulses,
//! reports no step events, and a pending scale or freeze keeps the world from being snapshotted.

mod common;
#[path = "common/ragdoll.rs"]
mod ragdoll;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    Blast, ContactPhase, Falloff, MIN_TIME_SCALE, PhysicsError, PhysicsSettings, PhysicsSpace,
    PhysicsWorld, QueryFilter,
};
use ludion_spatial::{Body, Collider, Joint, Shape, Velocity};
use ragdoll::{floor, floor_touched, physics, ragdoll_world, rigid, schedule_with, spawn};

fn free_schedule() -> Schedule {
    schedule_with(PhysicsSettings {
        gravity: [0.0; 3],
        ..PhysicsSettings::new(PhysicsSpace::Free)
    })
}

fn base_dt() -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    let dt = TickRate::DEFAULT.seconds_per_tick() as f32;
    dt
}

fn sphere() -> Shape {
    Shape::Sphere { radius: 0.5 }
}

fn x_of(world: &World, entity: EntityId) -> f32 {
    pose_of(world, entity).translation[0]
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-5,
        "expected {expected}, got {actual}"
    );
}

/// A world with one sphere drifting at `2 x/s` in zero gravity, warmed by tick 0 so the physics
/// resource exists; the next tick is frame 1.
fn drifter() -> (World, Schedule, EntityId) {
    let mut world = World::new();
    let ball = spawn(&mut world, sphere(), rigid(1.0), [0.0; 3], [2.0, 0.0, 0.0]);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);
    (world, schedule, ball)
}

#[test]
fn a_scaled_step_moves_a_body_by_the_scaled_distance_and_the_next_step_is_full() {
    let (mut world, mut schedule, ball) = drifter();
    assert_eq!(physics(&mut world).last_step_dt(), base_dt());

    let before = x_of(&world, ball);
    physics(&mut world)
        .scale_time(0.25)
        .expect("0.25 is in range");
    tick(&mut world, &mut schedule, 1);
    let scaled = x_of(&world, ball);
    close(scaled - before, 2.0 * base_dt() * 0.25);
    assert_eq!(physics(&mut world).last_step_dt(), base_dt() * 0.25);

    tick(&mut world, &mut schedule, 2);
    close(x_of(&world, ball) - scaled, 2.0 * base_dt());
    assert_eq!(physics(&mut world).last_step_dt(), base_dt());
}

#[test]
fn scales_in_one_frame_compose_by_minimum_in_any_order() {
    for order in [[0.5, 0.25], [0.25, 0.5]] {
        let (mut world, mut schedule, _ball) = drifter();
        for scale in order {
            physics(&mut world).scale_time(scale).expect("in range");
        }
        tick(&mut world, &mut schedule, 1);
        assert_eq!(physics(&mut world).last_step_dt(), base_dt() * 0.25);
    }
}

#[test]
fn a_freeze_skips_the_step_and_wins_over_a_scale() {
    let (mut world, mut schedule, ball) = drifter();
    let before = x_of(&world, ball);

    let physics_world = physics(&mut world);
    physics_world.scale_time(0.5).expect("in range");
    physics_world.freeze();
    tick(&mut world, &mut schedule, 1);
    assert_eq!(x_of(&world, ball), before);
    assert_eq!(physics(&mut world).last_step_dt(), 0.0);

    tick(&mut world, &mut schedule, 2);
    close(x_of(&world, ball) - before, 2.0 * base_dt());
    assert_eq!(physics(&mut world).last_step_dt(), base_dt());
}

#[test]
fn a_scale_outside_the_range_is_an_error_that_changes_nothing() {
    let (mut world, mut schedule, ball) = drifter();
    for bad in [0.0, -0.1, 1.0001, f32::NAN, f32::INFINITY] {
        assert!(
            matches!(
                physics(&mut world).scale_time(bad),
                Err(PhysicsError::QueryInput(_))
            ),
            "{bad} should be refused"
        );
    }
    let before = x_of(&world, ball);
    tick(&mut world, &mut schedule, 1);
    close(x_of(&world, ball) - before, 2.0 * base_dt());
    assert_eq!(physics(&mut world).last_step_dt(), base_dt());

    assert!(physics(&mut world).scale_time(1.0).is_ok());
}

#[test]
fn the_floor_is_accepted_and_steps_with_finite_motion_and_anything_below_is_refused() {
    let (mut world, mut schedule, ball) = drifter();
    let before = x_of(&world, ball);
    physics(&mut world)
        .scale_time(MIN_TIME_SCALE)
        .expect("the floor is in range");
    tick(&mut world, &mut schedule, 1);
    let moved = x_of(&world, ball) - before;
    assert!(moved.is_finite() && moved > 0.0, "moved {moved}");
    close(moved, 2.0 * base_dt() * MIN_TIME_SCALE);
    assert_eq!(
        physics(&mut world).last_step_dt(),
        base_dt() * MIN_TIME_SCALE
    );

    for below in [
        MIN_TIME_SCALE * 0.99,
        1e-6,
        f32::MIN_POSITIVE,
        f32::from_bits(1),
    ] {
        let refused = physics(&mut world).scale_time(below);
        let Err(PhysicsError::QueryInput(message)) = refused else {
            panic!("{below} should be refused, got {refused:?}");
        };
        assert!(message.contains("freeze()"), "{message}");
    }
}

/// The ball's `x` speed after a position-driven kinematic box, touching it, is advanced by
/// `2 x/s × base_dt × target_factor` every tick while the step runs at `scale`.
fn pushed_speed(scale: f32, target_factor: f32) -> f32 {
    let mut world = World::new();
    let ball = spawn(&mut world, sphere(), rigid(1.0), [0.0; 3], [0.0; 3]);
    let box_shape = Shape::Box {
        half_extents: [0.5, 0.5, 0.5],
    };
    let pusher = spawn(
        &mut world,
        box_shape,
        Body::new_kinematic(),
        [-1.0, 0.0, 0.0],
        [0.0; 3],
    );
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);

    let per_tick = 2.0 * base_dt() * target_factor;
    for frame in 1..=6 {
        let x = x_of(&world, pusher) + per_tick;
        world
            .insert(pusher, TestPose::at([x, 0.0, 0.0]))
            .expect("a live entity takes a pose");
        physics(&mut world).scale_time(scale).expect("in range");
        tick(&mut world, &mut schedule, frame);
    }
    world
        .get::<Velocity>(ball)
        .expect("a live entity")
        .expect("the test gives it a Velocity")
        .linear[0]
}

#[test]
fn a_kinematic_pusher_is_not_scaled_so_the_game_scales_its_own_motion() {
    let unscaled = pushed_speed(1.0, 1.0);
    assert!(
        (unscaled - 2.0).abs() < 0.3,
        "the unscaled push should carry the pusher's 2 m/s, got {unscaled}"
    );

    let scale = 0.25;
    let unscaled_target = pushed_speed(scale, 1.0);
    assert!(
        (unscaled_target - unscaled / scale).abs() < 0.15 * unscaled / scale,
        "a full target per tick inside a scaled step pushes at 1 / scale, got {unscaled_target}"
    );

    let scaled_target = pushed_speed(scale, scale);
    assert!(
        (scaled_target - unscaled).abs() < 0.3,
        "scaling the kinematic motion by the same factor restores the push, got {scaled_target}"
    );
}

/// Two spheres apart after tick 0, then the second teleported into touching the first.
fn touching_pair() -> (World, Schedule, EntityId, EntityId) {
    let mut world = World::new();
    let a = spawn(&mut world, sphere(), rigid(1.0), [0.0; 3], [0.0; 3]);
    let b = spawn(&mut world, sphere(), rigid(1.0), [5.0, 0.0, 0.0], [0.0; 3]);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);
    assert!(physics(&mut world).contact_events().is_empty());
    world
        .insert(b, TestPose::at([0.99, 0.0, 0.0]))
        .expect("a live entity takes a pose");
    (world, schedule, a, b)
}

#[test]
fn a_frozen_frame_has_no_contact_events_and_the_start_arrives_on_the_next_step() {
    let (mut world, mut schedule, a, b) = touching_pair();
    tick(&mut world, &mut schedule, 1);
    let control = physics(&mut world).contact_events().to_vec();
    assert_eq!(control.len(), 1, "{control:?}");
    assert_eq!((control[0].a, control[0].b), (a, b));
    assert_eq!(control[0].phase, ContactPhase::Started);

    let (mut world, mut schedule, _a, _b) = touching_pair();
    physics(&mut world).freeze();
    tick(&mut world, &mut schedule, 1);
    assert!(physics(&mut world).contact_events().is_empty());
    tick(&mut world, &mut schedule, 2);
    let events = physics(&mut world).contact_events().to_vec();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].phase, ContactPhase::Started);
}

#[test]
fn a_frozen_frame_still_applies_an_impulse_to_the_velocity_but_moves_nothing() {
    let (mut world, mut schedule, _ball) = drifter();
    let pushed = spawn(&mut world, sphere(), rigid(2.0), [0.0, 5.0, 0.0], [0.0; 3]);
    tick(&mut world, &mut schedule, 1);
    let before = x_of(&world, pushed);

    let physics_world = physics(&mut world);
    physics_world
        .apply_impulse(pushed, [4.0, 0.0, 0.0])
        .expect("finite");
    physics_world.freeze();
    tick(&mut world, &mut schedule, 2);

    assert_eq!(x_of(&world, pushed), before);
    let velocity = world
        .get::<Velocity>(pushed)
        .expect("a live entity")
        .expect("the test gives it a Velocity");
    close(velocity.linear[0], 2.0);

    tick(&mut world, &mut schedule, 3);
    close(x_of(&world, pushed) - before, 2.0 * base_dt());
}

#[test]
fn a_blast_on_a_frozen_frame_still_reports_its_hits() {
    let mut world = World::new();
    let target = spawn(&mut world, sphere(), rigid(1.0), [1.5, 0.0, 0.0], [0.0; 3]);
    let mut schedule = free_schedule();
    tick(&mut world, &mut schedule, 0);

    let physics_world = physics(&mut world);
    physics_world
        .radial_impulse(Blast {
            center: [0.0; 3],
            radius: 5.0,
            impulse: 4.0,
            falloff: Falloff::Constant,
            filter: QueryFilter::new(),
        })
        .expect("a valid blast queues");
    physics_world.freeze();
    tick(&mut world, &mut schedule, 1);

    let hits = physics(&mut world).blast_hits().to_vec();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].entity, target);
    assert_eq!(x_of(&world, target), 1.5);
}

fn velocity_of(world: &World, entity: EntityId) -> [f32; 3] {
    world
        .get::<Velocity>(entity)
        .expect("a live entity")
        .expect("spawn gives it a Velocity")
        .linear
}

#[test]
fn a_world_restored_mid_freeze_after_a_rebuild_blasts_like_the_uninterrupted_one() {
    let build = || {
        let mut world = World::new();
        let ball = spawn(&mut world, sphere(), rigid(1.0), [1.0, 0.0, 0.0], [0.0; 3]);
        (world, free_schedule(), ball)
    };
    let (mut a, mut a_schedule, a_ball) = build();
    let (mut b, mut b_schedule, b_ball) = build();
    for frame in 0..=4 {
        tick(&mut a, &mut a_schedule, frame);
        tick(&mut b, &mut b_schedule, frame);
    }
    // A frozen frame rebuilds the edited collider into its reused slot without a step.
    for (world, schedule, ball) in [
        (&mut a, &mut a_schedule, a_ball),
        (&mut b, &mut b_schedule, b_ball),
    ] {
        physics(world).freeze();
        *world
            .get_mut::<Collider>(ball)
            .expect("a live entity")
            .expect("its collider") = ragdoll::collider(sphere(), rigid(2.0));
        tick(world, schedule, 5);
    }
    let bytes = physics(&mut b)
        .to_bytes()
        .expect("the solver state encodes");
    b.insert_resource(
        PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).expect("the bytes just written decode"),
    );
    let (a_dt, b_dt) = (
        physics(&mut a).last_step_dt(),
        physics(&mut b).last_step_dt(),
    );

    for frame in 6..=9 {
        if frame == 6 {
            for world in [&mut a, &mut b] {
                physics(world)
                    .radial_impulse(Blast {
                        center: [0.0; 3],
                        radius: 3.0,
                        impulse: 3.0,
                        falloff: Falloff::Constant,
                        filter: QueryFilter::new(),
                    })
                    .expect("a valid blast queues");
            }
        }
        tick(&mut a, &mut a_schedule, frame);
        tick(&mut b, &mut b_schedule, frame);
        if frame == 6 {
            assert!(
                physics(&mut a).blast_hits().is_empty(),
                "a rebuilt body is blast-immune until the next real step"
            );
            assert_eq!(
                physics(&mut b).blast_hits(),
                physics(&mut a).blast_hits(),
                "the restored world hits a different set"
            );
        }
        assert_eq!(
            pose_of(&b, b_ball),
            pose_of(&a, a_ball),
            "pose diverges at tick {frame}"
        );
        assert_eq!(
            velocity_of(&b, b_ball),
            velocity_of(&a, a_ball),
            "velocity diverges at tick {frame}"
        );
    }
    assert_eq!(a_dt, 0.0);
    assert_eq!(
        b_dt, a_dt,
        "a restored frozen frame reports the uninterrupted step dt"
    );
}

/// The impulse of the first `Started` naming the ball and what it strikes, with the step scaled by
/// `scale` every tick. A 1 kg ball falls 1 m at 8 m/s onto a 6 kg box resting on the floor
/// (`target_is_box`) or onto the static floor itself.
fn first_strike_impulse(scale: f32, target_is_box: bool) -> f32 {
    let mut world = World::new();
    let floor_id = floor(&mut world);
    let (target, ball_y) = if target_is_box {
        let half = [0.15, 0.12, 0.15];
        let target = spawn(
            &mut world,
            Shape::Box { half_extents: half },
            rigid(6.0),
            [0.0, half[1], 0.0],
            [0.0; 3],
        );
        (target, 2.0 * half[1] + 1.0)
    } else {
        (floor_id, 1.0)
    };
    let ball = spawn(
        &mut world,
        Shape::Sphere { radius: 0.1 },
        Body::Rigid(ludion_spatial::RigidBody {
            mass: 1.0,
            gravity_scale: 0.0,
            ..ludion_spatial::RigidBody::default()
        }),
        [0.0, ball_y, 0.0],
        [0.0, -8.0, 0.0],
    );
    let mut schedule = schedule_with(PhysicsSettings::new(PhysicsSpace::Free));
    for frame in 0..3000 {
        if frame > 0 {
            physics(&mut world).scale_time(scale).expect("in range");
        }
        tick(&mut world, &mut schedule, frame);
        let strike = physics(&mut world)
            .contact_events()
            .iter()
            .find(|e| {
                e.phase == ContactPhase::Started
                    && [e.a, e.b].contains(&ball)
                    && [e.a, e.b].contains(&target)
            })
            .map(|e| e.impulse);
        if let Some(impulse) = strike {
            return impulse;
        }
    }
    panic!("the ball never struck at scale {scale}");
}

fn assert_impulse_survives(target_is_box: bool, scales: &[f32]) {
    let full = first_strike_impulse(1.0, target_is_box);
    assert!(full > 0.0, "the full-speed impact carried {full}");
    for &scale in scales {
        let scaled = first_strike_impulse(scale, target_is_box);
        assert!(
            (scaled - full).abs() <= 0.1 * full,
            "at scale {scale} the impact carried {scaled}, {full} at full speed"
        );
    }
}

#[test]
fn an_impact_impulse_survives_time_scale() {
    assert_impulse_survives(true, &[0.25, 0.05]);
}

#[test]
fn a_static_floor_impact_impulse_survives_time_scale() {
    assert_impulse_survives(false, &[0.05]);
}

/// The oracle is momentum: a 1 kg ball stopped dead from 8 m/s by a static floor (restitution 0 on
/// both sides) receives Δp = m·Δv = 8 N·s. The relative tests above scale every event alike, so only
/// an absolute value pins the reported magnitude.
#[test]
fn a_static_floor_impact_reports_the_balls_momentum() {
    let impulse = first_strike_impulse(1.0, false);
    assert!(
        (impulse - 8.0).abs() <= 0.4,
        "a 1 kg ball stopped from 8 m/s must report an impulse of 8 N*s within 5%: reported {impulse}"
    );
}

/// What a window run saw: the `(frame, phase, impulse)` of every contact event, frame by frame.
type Seen = Vec<(u64, ContactPhase, f32)>;

fn record(world: &mut World, frame: u64, seen: &mut Seen) {
    for event in physics(world).contact_events() {
        seen.push((frame, event.phase, event.impulse));
    }
}

/// The overlapping pair of [`touching_pair`], ticked at `scale(frame)` from frame 1 to `last`, calling
/// `each` with the frame before it ticks.
fn overlap_run(
    scale: impl Fn(u64) -> f32,
    last: u64,
    mut each: impl FnMut(u64, &mut World, EntityId),
) -> Seen {
    let (mut world, mut schedule, _a, b) = touching_pair();
    world
        .insert(
            b,
            Velocity {
                linear: [-1.0, 0.0, 0.0],
                ..Velocity::default()
            },
        )
        .expect("a live entity takes a velocity");
    let mut seen = Seen::new();
    for frame in 1..=last {
        each(frame, &mut world, b);
        physics(&mut world)
            .scale_time(scale(frame))
            .expect("in range");
        tick(&mut world, &mut schedule, frame);
        record(&mut world, frame, &mut seen);
    }
    seen
}

#[test]
fn a_started_arrives_one_base_tick_of_simulated_time_after_the_touch() {
    let seen = overlap_run(|_| 0.05, 40, |_, _, _| {});
    let started: Vec<_> = seen
        .iter()
        .filter(|(_, phase, _)| *phase == ContactPhase::Started)
        .collect();
    assert_eq!(started.len(), 1, "{seen:?}");
    // Engaged on frame 1; twenty steps of 0.05 make one base tick.
    assert_eq!(started[0].0, 20, "{seen:?}");
    assert!(started[0].2 > 0.0, "{seen:?}");
}

#[test]
fn a_window_closes_early_when_the_scale_returns_to_full_speed() {
    let full = overlap_run(|_| 1.0, 3, |_, _, _| {});
    let ramped = overlap_run(|frame| if frame <= 5 { 0.05 } else { 1.0 }, 8, |_, _, _| {});
    assert_eq!(full.len(), 1, "{full:?}");
    assert_eq!(ramped.len(), 1, "{ramped:?}");
    assert_eq!(
        ramped[0].0, 6,
        "the first full step closes the window: {ramped:?}"
    );
    assert!(
        (ramped[0].2 - full[0].2).abs() <= 0.1 * full[0].2,
        "{} vs {} at full speed",
        ramped[0].2,
        full[0].2
    );
}

#[test]
fn a_pair_that_separates_inside_the_window_starts_then_stops_on_the_next_step() {
    let seen = overlap_run(
        |_| 0.05,
        40,
        |frame, world, b| {
            if frame == 4 {
                world
                    .insert(b, TestPose::at([5.0, 0.0, 0.0]))
                    .expect("a live entity takes a pose");
            }
        },
    );
    let phases: Vec<_> = seen
        .iter()
        .map(|(frame, phase, _)| (*frame, *phase))
        .collect();
    // The teleport lands on frame 4's step, so the pair is gone on that step: its window flushes
    // there, and the next step reports the stop.
    assert_eq!(
        phases,
        [(4, ContactPhase::Started), (5, ContactPhase::Stopped)],
        "{seen:?}"
    );
    assert!(seen[0].2 > 0.0, "{seen:?}");
}

#[test]
fn a_despawn_mid_window_reports_started_then_stopped() {
    let seen = overlap_run(
        |_| 0.05,
        40,
        |frame, world, b| {
            if frame == 4 {
                world.despawn(b).expect("a live entity despawns");
            }
        },
    );
    let phases: Vec<_> = seen
        .iter()
        .map(|(frame, phase, _)| (*frame, *phase))
        .collect();
    assert_eq!(
        phases,
        [(4, ContactPhase::Started), (5, ContactPhase::Stopped)],
        "{seen:?}"
    );
    assert!(seen[0].2 > 0.0, "{seen:?}");
}

#[test]
fn a_snapshot_taken_mid_window_resumes_with_the_same_events() {
    let (mut world, mut schedule, _a, b) = touching_pair();
    world
        .insert(
            b,
            Velocity {
                linear: [-1.0, 0.0, 0.0],
                ..Velocity::default()
            },
        )
        .expect("a live entity takes a velocity");
    let mut seen = Seen::new();
    for frame in 1..=5 {
        physics(&mut world).scale_time(0.05).expect("in range");
        tick(&mut world, &mut schedule, frame);
        record(&mut world, frame, &mut seen);
    }
    assert!(seen.is_empty(), "the window should still be open: {seen:?}");
    let mut resumed = restore(&capture(&world));
    let mut resumed_schedule = free_schedule();
    let mut resumed_seen = Seen::new();
    for frame in 6..=40 {
        physics(&mut world).scale_time(0.05).expect("in range");
        tick(&mut world, &mut schedule, frame);
        record(&mut world, frame, &mut seen);
        physics(&mut resumed).scale_time(0.05).expect("in range");
        tick(&mut resumed, &mut resumed_schedule, frame);
        record(&mut resumed, frame, &mut resumed_seen);
    }
    assert!(
        seen.iter().any(|(_, _, impulse)| *impulse > 0.0),
        "the run should carry an impact: {seen:?}"
    );
    assert_eq!(seen.len(), resumed_seen.len());
    for (live, again) in seen.iter().zip(&resumed_seen) {
        assert_eq!((live.0, live.1), (again.0, again.1));
        assert_eq!(live.2.to_bits(), again.2.to_bits());
    }
}

/// Ticks a ball resting on a static box until it sleeps, scaling every frame by `scale`.
fn ticks_to_sleep(scale: f32) -> u64 {
    let mut world = World::new();
    let ball = spawn(&mut world, sphere(), rigid(1.0), [0.0, 0.5, 0.0], [0.0; 3]);
    spawn(
        &mut world,
        Shape::Box {
            half_extents: [5.0, 5.0, 5.0],
        },
        Body::new_static(),
        [0.0, -5.0, 0.0],
        [0.0; 3],
    );
    let mut schedule = schedule_with(PhysicsSettings::new(PhysicsSpace::Free));
    tick(&mut world, &mut schedule, 0);
    for frame in 1..2000 {
        physics(&mut world).scale_time(scale).expect("in range");
        tick(&mut world, &mut schedule, frame);
        if physics(&mut world).is_sleeping(ball) == Some(true) {
            return frame;
        }
    }
    panic!("the ball never fell asleep at scale {scale}");
}

#[test]
fn a_body_sleeps_after_simulated_seconds_not_ticks() {
    let full = ticks_to_sleep(1.0);
    let slowed = ticks_to_sleep(0.25);
    assert!(
        slowed >= 3 * full && slowed <= 5 * full,
        "asleep after {full} ticks at 1.0 but {slowed} at 0.25"
    );
}

#[test]
fn a_pending_scale_or_freeze_refuses_a_snapshot_until_the_step_ran() {
    let (mut world, mut schedule, _ball) = drifter();
    assert!(physics(&mut world).to_bytes().is_ok());

    physics(&mut world).scale_time(0.5).expect("in range");
    assert!(matches!(
        physics(&mut world).to_bytes(),
        Err(PhysicsError::Encode(_))
    ));
    tick(&mut world, &mut schedule, 1);
    assert!(physics(&mut world).to_bytes().is_ok());

    physics(&mut world).freeze();
    assert!(matches!(
        physics(&mut world).to_bytes(),
        Err(PhysicsError::Encode(_))
    ));
    tick(&mut world, &mut schedule, 2);
    assert!(physics(&mut world).to_bytes().is_ok());
}

/// Ticks scaled after the ragdoll's first floor contact.
const WINDOW: u64 = 5;
const RAGDOLL_TICKS: u64 = 300;

/// Runs the ragdoll to its first floor contact, scales the next [`WINDOW`] ticks by `scale`, runs to
/// [`RAGDOLL_TICKS`], and returns the highest part centre after that contact.
fn ragdoll_peak(scale: f32, velocity: [f32; 3]) -> f32 {
    let (mut world, mut schedule, ids) = ragdoll_world(velocity);
    let mut landed: Option<u64> = None;
    let mut peak = f32::MIN;
    for frame in 0..RAGDOLL_TICKS {
        if landed.is_some_and(|first| frame <= first + WINDOW) {
            physics(&mut world).scale_time(scale).expect("in range");
        }
        tick(&mut world, &mut schedule, frame);
        if landed.is_none() && floor_touched(&mut world) {
            landed = Some(frame);
        }
        if landed.is_some() {
            for &part in &ids {
                peak = peak.max(pose_of(&world, part).translation[1]);
            }
        }
    }
    assert!(landed.is_some(), "the ragdoll never touched the floor");
    peak
}

#[test]
fn a_ragdoll_landing_into_a_small_scale_is_not_launched() {
    let mut launched = Vec::new();
    for velocity in [[0.0; 3], [12.0, -6.0, 3.0]] {
        let unscaled = ragdoll_peak(1.0, velocity);
        for scale in [0.2, 0.05, 0.01] {
            let peak = ragdoll_peak(scale, velocity);
            if peak >= unscaled + 1.0 {
                launched.push(format!(
                    "{velocity:?} at scale {scale}: peak {peak} m, {unscaled} m unscaled"
                ));
            }
        }
    }
    assert!(launched.is_empty(), "launched: {launched:#?}");
}

/// A 1 m box starting at height `y`, scaled by 0.05 for [`WINDOW`] ticks from tick `settle`, run 60
/// ticks past that; returns the largest speed seen from `settle` on and the final height.
fn box_through_small_scale(y: f32, settle: u64) -> (f32, f32) {
    let mut world = World::new();
    let cube = spawn(
        &mut world,
        Shape::Box {
            half_extents: [0.5; 3],
        },
        rigid(1.0),
        [0.0, y, 0.0],
        [0.0; 3],
    );
    floor(&mut world);
    let mut schedule = schedule_with(PhysicsSettings::new(PhysicsSpace::Free));
    let mut fastest = 0.0_f32;
    for frame in 0..settle + WINDOW + 60 {
        if (settle..settle + WINDOW).contains(&frame) {
            physics(&mut world).scale_time(0.05).expect("in range");
        }
        tick(&mut world, &mut schedule, frame);
        if frame >= settle {
            let linear = world
                .get::<Velocity>(cube)
                .expect("a live entity")
                .expect("spawn gives it a Velocity")
                .linear;
            fastest = fastest.max(linear.iter().map(|v| v * v).sum::<f32>().sqrt());
        }
    }
    (fastest, pose_of(&world, cube).translation[1])
}

#[test]
fn a_resting_box_stays_at_rest_through_a_small_scale() {
    let (_, settled) = box_through_small_scale(0.5, 1000);
    let (fastest, end) = box_through_small_scale(0.5, 60);
    assert!(fastest < 0.05, "a resting box reached {fastest} m/s");
    assert!(
        (end - settled).abs() < 0.01,
        "rested at {settled}, ended at {end}"
    );
}

#[test]
fn a_shallowly_penetrating_box_is_not_launched_by_a_small_scale() {
    let (fastest, end) = box_through_small_scale(0.49, 1);
    assert!(
        fastest < 0.5,
        "a box 1 cm in the floor reached {fastest} m/s"
    );
    assert!((end - 0.5).abs() < 0.02, "ended at {end}");
}

/// Everything a session needs to be rebuilt from nothing — the `joints.rs` `Capture` precedent.
struct Capture {
    entities: Vec<EntityId>,
    colliders: Vec<Option<Collider>>,
    poses: Vec<TestPose>,
    velocities: Vec<Option<Velocity>>,
    joints: Vec<Option<Joint>>,
    bytes: Vec<u8>,
}

fn capture(world: &World) -> Capture {
    let entities: Vec<EntityId> = world.entities().collect();
    Capture {
        colliders: entities
            .iter()
            .map(|&e| world.get::<Collider>(e).expect("a live entity").cloned())
            .collect(),
        poses: entities.iter().map(|&e| pose_of(world, e)).collect(),
        velocities: entities
            .iter()
            .map(|&e| world.get::<Velocity>(e).expect("a live entity").copied())
            .collect(),
        joints: entities
            .iter()
            .map(|&e| world.get::<Joint>(e).expect("a live entity").cloned())
            .collect(),
        bytes: world
            .resource::<PhysicsWorld>()
            .expect("the write system installed a world")
            .to_bytes()
            .expect("the solver state encodes"),
        entities,
    }
}

fn restore(captured: &Capture) -> World {
    let mut world = World::new();
    world
        .restore_entities(&captured.entities)
        .expect("captured live set is valid");
    for (index, &entity) in captured.entities.iter().enumerate() {
        if let Some(collider) = &captured.colliders[index] {
            world
                .insert(entity, collider.clone())
                .expect("a restored entity accepts a collider");
        }
        world
            .insert(entity, captured.poses[index])
            .expect("a restored entity accepts a pose");
        if let Some(velocity) = captured.velocities[index] {
            world
                .insert(entity, velocity)
                .expect("a restored entity accepts a velocity");
        }
        if let Some(joint) = &captured.joints[index] {
            world
                .insert(entity, joint.clone())
                .expect("a restored entity accepts a joint");
        }
    }
    world.insert_resource(
        PhysicsWorld::from_bytes(&captured.bytes, TickRate::DEFAULT)
            .expect("the bytes this run just wrote decode"),
    );
    world
}

fn scaled_tick(world: &mut World, schedule: &mut Schedule, frame: u64) {
    physics(world).scale_time(0.05).expect("in range");
    tick(world, schedule, frame);
}

#[test]
fn a_ragdoll_snapshot_between_scaled_ticks_resumes_bit_identically() {
    let (mut world, mut schedule, ids) = ragdoll_world([0.0; 3]);
    let mut frame = 0;
    loop {
        tick(&mut world, &mut schedule, frame);
        if floor_touched(&mut world) {
            break;
        }
        frame += 1;
        assert!(frame < RAGDOLL_TICKS, "the ragdoll never touched the floor");
    }
    let landed = frame;
    // The snapshot follows the second scaled tick, once its step ran and no scale is pending.
    for frame in landed + 1..=landed + 2 {
        scaled_tick(&mut world, &mut schedule, frame);
    }
    let mut resumed = restore(&capture(&world));
    let mut resumed_schedule = schedule_with(PhysicsSettings::new(PhysicsSpace::Free));
    for frame in landed + 3..=landed + WINDOW {
        scaled_tick(&mut world, &mut schedule, frame);
        scaled_tick(&mut resumed, &mut resumed_schedule, frame);
    }
    for frame in landed + WINDOW + 1..landed + WINDOW + 61 {
        tick(&mut world, &mut schedule, frame);
        tick(&mut resumed, &mut resumed_schedule, frame);
        for &part in &ids {
            assert_eq!(
                pose_of(&world, part),
                pose_of(&resumed, part),
                "the resumed ragdoll diverges at tick {frame}"
            );
        }
    }
}
