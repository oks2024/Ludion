//! A limited-ball-jointed ragdoll resting on flat ground must not have its mechanical energy
//! pumped up by the solver: the angular-limit and motor rows use the exact rate of the angle they
//! constrain, so a multi-axis swing does not feed its correction back as energy.

mod common;
#[path = "common/ragdoll.rs"]
mod ragdoll;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::math::{cos, sin};
use ludion_ecs::World;
use ludion_physics::{PhysicsSettings, PhysicsSpace};
use ludion_spatial::{Body, Joint, JointKind, JointMotor, RigidBody, Shape, Velocity};
use ragdoll::{
    Part, ball_axes, build_ragdoll, floor_touched, ragdoll_parts_with, schedule_with, spawn_moving,
};

const GRAVITY: f32 = 9.81;
const TICKS: u64 = 900;
/// Ticks after the first floor contact before the energy is counted: the impact itself may
/// legitimately redistribute energy between parts.
const SETTLE: u64 = 30;
/// A gain below this per tick is rounding noise, not injected energy.
const NOISE: f32 = 0.01;

/// Launch velocities: the first-, fourth-, fifth-, sixth-, 14th-, 18th-, 19th- and 23rd-indexed
/// vectors of a fan of 24 thrown in a spiral, each faster and steeper than the last few.
const LAUNCHES: [usize; 8] = [0, 3, 4, 5, 13, 17, 18, 22];

fn launch(index: usize) -> [f32; 3] {
    let angle = index as f32 * 0.7;
    let speed = 6.0 + (index % 5) as f32 * 3.0;
    [
        speed * cos(angle),
        -2.0 - (index % 3) as f32 * 3.0,
        speed * sin(angle),
    ]
}

/// Part masses in kilograms, in [`ragdoll_parts_with`] order: torso, head, then per side upper arm,
/// forearm, thigh, shin.
fn masses() -> Vec<f32> {
    let mut masses = vec![6.0, 2.0];
    for _ in 0..2 {
        masses.extend([1.0, 1.0, 2.0, 1.5]);
    }
    masses
}

/// Neck, shoulder and hip limits, each a different range per axis in degrees.
fn parts() -> Vec<Part> {
    ragdoll_parts_with(
        ball_axes([30.0, 20.0, 20.0]),
        ball_axes([80.0, 30.0, 80.0]),
        ball_axes([60.0, 20.0, 30.0]),
    )
}

fn settings() -> PhysicsSettings {
    PhysicsSettings {
        solver_iterations: 8,
        ..PhysicsSettings::new(PhysicsSpace::Free)
    }
}

/// A rigid body with the tunables a thrown game ragdoll uses.
fn body(mass: f32) -> Body {
    Body::Rigid(RigidBody {
        mass,
        friction: 0.6,
        restitution: 0.05,
        linear_damping: 0.1,
        angular_damping: 0.4,
        ..RigidBody::default()
    })
}

/// A part's mass, and its principal moments of inertia in its own frame.
struct Inertia {
    mass: f32,
    local: [f32; 3],
}

/// A part's capsule: radius the smaller horizontal half extent, cylinder half height the rest of
/// the vertical one.
fn capsule_dims(half: [f32; 3]) -> (f32, f32) {
    let radius = half[0].min(half[2]);
    ((half[1] - radius).max(0.01), radius)
}

fn capsule(half: [f32; 3]) -> Shape {
    let (half_height, radius) = capsule_dims(half);
    Shape::Capsule {
        half_height,
        radius,
    }
}

fn capsule_inertia_of(mass: f32, half: [f32; 3]) -> Inertia {
    let (half_height, radius) = capsule_dims(half);
    capsule_inertia(mass, half_height, radius)
}

/// A capsule along y — a cylinder of half height `h` plus two hemispheres of radius `r`, the mass
/// split by volume. With `Hc = 2h`: `I_y = mc·r²/2 + ms·2r²/5` and
/// `I_x = I_z = mc·(r²/4 + Hc²/12) + ms·(2r²/5 + Hc²/4 + 3·Hc·r/8)`.
fn capsule_inertia(mass: f32, h: f32, r: f32) -> Inertia {
    let pi = std::f32::consts::PI;
    let cylinder = pi * r * r * 2.0 * h;
    let sphere = 4.0 / 3.0 * pi * r * r * r;
    let mc = mass * cylinder / (cylinder + sphere);
    let ms = mass - mc;
    let hc = 2.0 * h;
    let transverse = mc * (r * r / 4.0 + hc * hc / 12.0)
        + ms * (2.0 * r * r / 5.0 + hc * hc / 4.0 + 3.0 * hc * r / 8.0);
    Inertia {
        mass,
        local: [
            transverse,
            mc * r * r / 2.0 + ms * 2.0 * r * r / 5.0,
            transverse,
        ],
    }
}

/// Rotates `v` by the inverse of the unit quaternion `q = [x, y, z, w]`.
fn unrotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [qx, qy, qz, w] = q;
    let u = [-qx, -qy, -qz];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(u, v).map(|c| 2.0 * c);
    let ut = cross(u, t);
    [
        v[0] + w * t[0] + ut[0],
        v[1] + w * t[1] + ut[1],
        v[2] + w * t[2] + ut[2],
    ]
}

/// `Σ m·(½|v|² + g·y) + ½ Σ I_k·ω_local_k²` over `parts`.
fn energy(world: &World, parts: &[(EntityId, Inertia)]) -> f32 {
    parts
        .iter()
        .map(|(id, inertia)| {
            let pose: TestPose = pose_of(world, *id);
            let velocity = world
                .get::<Velocity>(*id)
                .expect("a live entity")
                .expect("every body mirrors a velocity");
            let v = velocity.linear;
            let local = unrotate(pose.rotation, velocity.angular);
            let rotational: f32 = (0..3).map(|k| inertia.local[k] * local[k] * local[k]).sum();
            inertia.mass
                * (0.5 * (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) + GRAVITY * pose.translation[1])
                + 0.5 * rotational
        })
        .sum()
}

/// One launch: the sum over ticks after the first floor contact plus [`SETTLE`] of each tick's
/// positive energy gain above [`NOISE`], in joules.
fn injected(launch_index: usize) -> f32 {
    let parts = parts();
    let masses = masses();
    let (mut world, ids) = build_ragdoll(
        &parts,
        &masses,
        body,
        capsule,
        launch(launch_index),
        [1.0, 2.0, 3.0],
    );
    let mut schedule = schedule_with(settings());
    let inertias: Vec<(EntityId, Inertia)> = ids
        .iter()
        .zip(parts.iter().zip(&masses))
        .map(|(&id, ((half, ..), &mass))| (id, capsule_inertia_of(mass, *half)))
        .collect();
    let mut landed: Option<u64> = None;
    let mut previous = f32::NAN;
    let mut gained = 0.0;
    for frame in 0..TICKS {
        tick(&mut world, &mut schedule, frame);
        if landed.is_none() && floor_touched(&mut world) {
            landed = Some(frame);
        }
        let now = energy(&world, &inertias);
        if landed.is_some_and(|first| frame > first + SETTLE) {
            gained += (now - previous - NOISE).max(0.0);
        }
        previous = now;
    }
    assert!(
        landed.is_some(),
        "launch {launch_index} never touched the floor"
    );
    gained
}

/// The most energy the eight launches may inject in total, in joules. Measured on capsule parts:
/// 0.67 J with the plain limit jacobian, 0.044 J with the exact one.
const INJECTED_BOUND: f32 = 0.2;

#[test]
fn a_resting_ragdoll_gains_no_energy_from_its_ball_limits() {
    let runs: Vec<f32> = LAUNCHES.iter().map(|&index| injected(index)).collect();
    let total: f32 = runs.iter().sum();
    assert!(
        total < INJECTED_BOUND,
        "the solver injected {total} J (bound {INJECTED_BOUND} J); per launch {runs:?}"
    );
}

/// A 1 kg capsule hanging from a fixed anchor on a free ball joint, driven to a multi-axis target.
#[test]
fn a_ball_motor_reaches_a_multi_axis_target() {
    let (radius, half_height, mass) = (0.1, 0.4, 1.0);
    let mut world = World::new();
    let anchor = spawn_moving(
        &mut world,
        Shape::Sphere { radius: 0.05 },
        Body::new_static(),
        [0.0; 3],
        [0.0; 3],
        [0.0; 3],
    );
    let bob = spawn_moving(
        &mut world,
        Shape::Capsule {
            half_height,
            radius,
        },
        Body::Rigid(RigidBody {
            mass,
            ..RigidBody::default()
        }),
        [0.0; 3],
        [0.0; 3],
        [0.0; 3],
    );
    world
        .insert(
            bob,
            Joint {
                a: anchor,
                b: bob,
                kind: JointKind::Ball { limits: None },
                anchor: [0.0; 3],
                break_impulse: None,
                collide: false,
                zero: None,
            },
        )
        .expect("a fresh entity accepts a joint");
    world
        .insert(
            bob,
            JointMotor {
                target: vec![60.0, 40.0, -50.0],
                stiffness: 40.0,
                damping: 4.0,
                max_force: Some(30.0),
            },
        )
        .expect("a fresh entity accepts a motor");
    let mut schedule = schedule_with(settings());
    let parts = [(bob, capsule_inertia(mass, half_height, radius))];
    let mut at_300 = 0.0;
    for frame in 0..600 {
        tick(&mut world, &mut schedule, frame);
        if frame == 299 {
            at_300 = energy(&world, &parts);
        }
    }
    let velocity = world
        .get::<Velocity>(bob)
        .expect("a live entity")
        .expect("the bob mirrors a velocity");
    let speed = velocity.angular.iter().map(|w| w * w).sum::<f32>().sqrt();
    let at_600 = energy(&world, &parts);
    let turned = pose_of(&world, bob).rotation[3].abs();
    assert!(
        turned < 0.9,
        "the bob never turned towards its target (w = {turned})"
    );
    assert!(speed < 1e-3, "still turning at {speed} rad/s");
    assert!(
        at_600 <= at_300 + 0.05,
        "energy rose from {at_300} J to {at_600} J over the last 300 ticks"
    );
}
