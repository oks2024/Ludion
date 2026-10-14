//! The jointed ragdoll and the small spawn helpers the ragdoll tests share. Included with
//! `#[path = "common/ragdoll.rs"] mod ragdoll;` beside `mod common;`; each test binary uses a subset.
#![allow(dead_code)]

use crate::common::TestPose;
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, World};
use ludion_physics::{ContactPhase, PhysicsSettings, PhysicsSpace, PhysicsWorld, install};
use ludion_spatial::{Body, Collider, Joint, JointKind, RigidBody, Shape, Velocity};

pub fn schedule_with(settings: PhysicsSettings) -> Schedule {
    let mut schedule = Schedule::new();
    install::<TestPose>(&mut schedule, settings);
    schedule
}

pub fn collider(shape: Shape, body: Body) -> Collider {
    Collider {
        shape,
        body,
        layer: None,
    }
}

pub fn rigid(mass: f32) -> Body {
    Body::Rigid(RigidBody {
        mass,
        ..RigidBody::default()
    })
}

pub fn spawn(
    world: &mut World,
    shape: Shape,
    body: Body,
    at: [f32; 3],
    velocity: [f32; 3],
) -> EntityId {
    spawn_moving(world, shape, body, at, velocity, [0.0; 3])
}

/// [`spawn`] with an initial angular velocity too.
pub fn spawn_moving(
    world: &mut World,
    shape: Shape,
    body: Body,
    at: [f32; 3],
    velocity: [f32; 3],
    angular: [f32; 3],
) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(at))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, collider(shape, body))
        .expect("a fresh entity accepts a collider");
    world
        .insert(
            entity,
            Velocity {
                linear: velocity,
                angular,
            },
        )
        .expect("a fresh entity accepts a velocity");
    entity
}

pub fn physics(world: &mut World) -> &mut PhysicsWorld {
    world
        .resource_mut::<PhysicsWorld>()
        .expect("the write system installed a world")
}

/// One ragdoll part: half extents, world centre, and for a child the parent part's index, the joint
/// kind and the joint point in the parent's local frame.
pub type Part = ([f32; 3], [f32; 3], Option<(usize, JointKind, [f32; 3])>);

pub fn ball(limit: f32) -> JointKind {
    ball_axes([limit; 3])
}

/// A ball joint limited to `±limits[axis]` degrees on each axis.
pub fn ball_axes(limits: [f32; 3]) -> JointKind {
    JointKind::Ball {
        limits: Some(limits.map(|limit| [-limit, limit])),
    }
}

pub fn knee(limit: f32) -> JointKind {
    JointKind::Hinge {
        axis: [1.0, 0.0, 0.0],
        limits: Some([-limit, limit]),
    }
}

/// A box torso 4 m up with a ball-jointed head, and an arm and a leg per side, each two box
/// segments joined by a limited hinge — the shape of a game's thrown ragdoll.
pub fn ragdoll_parts() -> Vec<Part> {
    ragdoll_parts_with(ball(35.0), ball(65.0), ball(55.0))
}

/// [`ragdoll_parts`] with the neck, shoulder and hip joints given.
pub fn ragdoll_parts_with(neck: JointKind, shoulder: JointKind, hip: JointKind) -> Vec<Part> {
    let mut parts: Vec<Part> = vec![
        ([0.25, 0.35, 0.15], [0.0, 4.0, 0.0], None),
        (
            [0.15; 3],
            [0.0, 4.525, 0.0],
            Some((0, neck, [0.0, 0.375, 0.0])),
        ),
    ];
    for side in [-1.0_f32, 1.0] {
        let upper_arm = parts.len();
        parts.push((
            [0.06, 0.15, 0.06],
            [0.3 * side, 3.85, 0.0],
            Some((0, shoulder.clone(), [0.3 * side, 0.3, 0.0])),
        ));
        parts.push((
            [0.05, 0.14, 0.05],
            [0.3 * side, 3.56, 0.0],
            Some((upper_arm, knee(70.0), [0.0, -0.15, 0.0])),
        ));
        let thigh = parts.len();
        parts.push((
            [0.08, 0.2, 0.08],
            [0.12 * side, 3.45, 0.0],
            Some((0, hip.clone(), [0.12 * side, -0.35, 0.0])),
        ));
        parts.push((
            [0.06, 0.2, 0.06],
            [0.12 * side, 3.05, 0.0],
            Some((thigh, knee(75.0), [0.0, -0.2, 0.0])),
        ));
    }
    parts
}

pub fn floor(world: &mut World) -> EntityId {
    spawn(
        world,
        Shape::Box {
            half_extents: [50.0, 1.0, 50.0],
        },
        Body::new_static(),
        [0.0, -1.0, 0.0],
        [0.0; 3],
    )
}

/// The ragdoll dropped onto a floor whose top is `y = 0`, launched at `velocity`; the floor is
/// entity 0 and the parts follow in [`ragdoll_parts`] order.
pub fn ragdoll_world(velocity: [f32; 3]) -> (World, Schedule, Vec<EntityId>) {
    let parts = ragdoll_parts();
    let masses: Vec<f32> = parts
        .iter()
        .map(|(half, ..)| 300.0 * 8.0 * half[0] * half[1] * half[2])
        .collect();
    let schedule = schedule_with(PhysicsSettings::new(PhysicsSpace::Free));
    let (world, ids) = build_ragdoll(
        &parts,
        &masses,
        rigid,
        |half| Shape::Box { half_extents: half },
        velocity,
        [0.0; 3],
    );
    (world, schedule, ids)
}

/// A floor and `parts` as bodies of the given `masses`, every part launched at `velocity` and
/// `angular`; `body` turns a mass into the part's rigid body and `shape` its half extents into a collider. The floor is entity 0.
pub fn build_ragdoll(
    parts: &[Part],
    masses: &[f32],
    body: impl Fn(f32) -> Body,
    shape: impl Fn([f32; 3]) -> Shape,
    velocity: [f32; 3],
    angular: [f32; 3],
) -> (World, Vec<EntityId>) {
    let mut world = World::new();
    floor(&mut world);
    let mut ids: Vec<EntityId> = Vec::new();
    for ((half, at, link), &mass) in parts.iter().zip(masses) {
        let part = spawn_moving(&mut world, shape(*half), body(mass), *at, velocity, angular);
        if let Some((parent, kind, anchor)) = link {
            world
                .insert(
                    part,
                    Joint {
                        a: ids[*parent],
                        b: part,
                        kind: kind.clone(),
                        anchor: *anchor,
                        break_impulse: None,
                        collide: false,
                    },
                )
                .expect("a fresh entity accepts a joint");
        }
        ids.push(part);
    }
    (world, ids)
}

/// Whether the last tick started a contact with the floor (entity 0).
pub fn floor_touched(world: &mut World) -> bool {
    physics(world)
        .contact_events()
        .iter()
        .any(|event| event.phase == ContactPhase::Started && event.a.index() == 0)
}
