//! A minimal pose component and the tick driver both integration tests run on.

use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{Schedule, SystemCtx, SystemPanic, World};
use ludion_physics::PhysicsPose;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The smallest thing that can stand in for the engine's `Transform`: a translation and a
/// quaternion, so the tests exercise the solver mirror rather than the Euler conversion.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct TestPose {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
}

impl Default for TestPose {
    fn default() -> Self {
        TestPose {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

impl TestPose {
    pub fn at(translation: [f32; 3]) -> Self {
        TestPose {
            translation,
            ..TestPose::default()
        }
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

/// Runs one tick of `schedule` at `frame` and fails loudly if a system panicked — the unwind
/// boundary otherwise turns a broken system into a silently short tick.
pub fn tick(world: &mut World, schedule: &mut Schedule, frame: u64) {
    schedule.run(
        world,
        &SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT),
    );
    if let Some(panic) = world.resource::<SystemPanic>() {
        panic!("{} panicked: {}", panic.system, panic.message);
    }
}

/// The pose of `entity`, which every body in these scenes has.
pub fn pose_of(world: &World, entity: EntityId) -> TestPose {
    *world
        .get::<TestPose>(entity)
        .expect("a live entity")
        .expect("every body carries a pose")
}
