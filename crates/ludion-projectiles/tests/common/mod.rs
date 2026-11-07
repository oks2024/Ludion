//! Shared fixtures: static physics boxes, a hand-built index, a tick driver.

#![allow(dead_code)] // each test binary uses a subset

use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{Schedule, SystemCtx, SystemPanic, World};
use ludion_physics::{NoParent, PhysicsPose, PhysicsSpace, PhysicsWorld, QueryFilter, install};
use ludion_projectiles::StepConfig;
use ludion_spatial::spatial3d::{Handle, Index3d, Item, Volume};
use ludion_spatial::{Body, Collider, Shape};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const DT: f32 = 1.0 / 60.0;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct TestPose {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
}

impl TestPose {
    pub fn at(translation: [f32; 3]) -> Self {
        Self {
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

impl Default for TestPose {
    fn default() -> Self {
        Self::at([0.0; 3])
    }
}

impl PhysicsPose for TestPose {
    fn translation(&self) -> [f32; 3] {
        self.translation
    }
    fn set_translation(&mut self, translation: [f32; 3]) {
        self.translation = translation;
    }
    fn rotation_quat(&self) -> [f32; 4] {
        self.rotation
    }
    fn set_rotation_quat(&mut self, rotation: [f32; 4]) {
        self.rotation = rotation;
    }
}

/// A world of static boxes `(centre, half_extents)`, stepped once so the physics world exists.
/// Returns the world and the box entities in order.
pub fn box_world(boxes: &[([f32; 3], [f32; 3])]) -> (World, Vec<EntityId>) {
    let mut world = World::new();
    let mut ids = Vec::new();
    for &(at, half) in boxes {
        let e = world.spawn();
        world.insert(e, TestPose::at(at)).unwrap();
        world
            .insert(
                e,
                Collider {
                    shape: Shape::Box { half_extents: half },
                    body: Body::new_static(),
                    layer: None,
                    sensor: false,
                },
            )
            .unwrap();
        ids.push(e);
    }
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, PhysicsSpace::Free);
    schedule.run(
        &mut world,
        &SystemCtx::new(FrameIndex::new(0), TickRate::DEFAULT),
    );
    if let Some(p) = world.resource::<SystemPanic>() {
        panic!("{} panicked: {}", p.system, p.message);
    }
    (world, ids)
}

pub fn physics_ref(world: &World) -> &PhysicsWorld {
    world.resource::<PhysicsWorld>().expect("a box was claimed")
}

pub fn empty_physics() -> PhysicsWorld {
    PhysicsWorld::new(TickRate::DEFAULT)
}

pub fn config(physics: &PhysicsWorld) -> StepConfig {
    StepConfig::new(physics.sweep_filter(&QueryFilter::new()).unwrap())
}

pub fn sphere(handle: Handle, centre: [f32; 3], radius: f32, layers: u32) -> Item {
    Item {
        handle,
        volume: Volume::Sphere {
            center: centre,
            radius,
        },
        layers,
    }
}

pub fn aabb(handle: Handle, min: [f32; 3], max: [f32; 3], layers: u32) -> Item {
    Item {
        handle,
        volume: Volume::Aabb { min, max },
        layers,
    }
}

pub fn index_of(items: &[Item]) -> Index3d {
    let mut index = Index3d::new();
    for item in items {
        index.push(*item);
    }
    index.build();
    index
}

pub fn near(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= eps)
}
