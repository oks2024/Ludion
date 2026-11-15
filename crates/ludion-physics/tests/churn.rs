//! Physics is independent of unrelated entity churn: two worlds that differ only by bare "pad"
//! entities spawned and despawned around the claimed spawns (so the LIFO free list hands the
//! claimed bodies reversed slots) step bit-identically, per spawn index. The write system feeds
//! rapier in ECS spawn-rank order, never by slot value. Blasts and partner wakes follow spawn rank
//! too. The last test observes the partner-wake order through rapier's awake-body order (table
//! order would differ after a rebuild); a blast's per-body impulses are commutative in rapier, so
//! its order has no observable effect to assert on.

mod common;
#[path = "common/ragdoll.rs"]
mod ragdoll;

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    Blast, CharacterMotor, Falloff, MotorError, NoParent, PhysicsSettings, PhysicsSpace,
    PhysicsWorld, QueryFilter, move_characters, teleport_kinematic,
};
use ludion_spatial::{Body, Collider, Joint, RigidBody, Shape, Velocity};
use ragdoll::{collider, physics, ragdoll_parts, rigid, schedule_with, spawn, spawn_moving};

const TICKS: u64 = 80;
const SNAPSHOT_AT: u64 = 40;

/// One tick's pose and velocity bits of one claimed entity: translation, rotation, linear.
type Row = [u32; 10];

/// A scenario event on top of the base run (a rebuild at 20, a blast at 30, a motor walking into
/// the stack throughout).
#[derive(Clone, Copy, Debug)]
enum Extra {
    None,
    /// A collider removed at 45 (the body retires) and put back at 50 (it is claimed afresh).
    RetireReclaim,
    /// A claimed body despawned and a replacement spawned in the same tick, at 45.
    DespawnSpawn,
    /// A limb's joint removed at 45 and added again at 50.
    JointReadd,
    /// A kinematic platform teleported at 45.
    Teleport,
}

struct Scene {
    world: World,
    schedule: Schedule,
    /// Claimed entities in spawn order; the trace is indexed by position here, never by id.
    spawned: Vec<EntityId>,
    stack: Vec<EntityId>,
    motor: EntityId,
    platform: EntityId,
    spare: EntityId,
    limb: EntityId,
    held: Option<(Collider, Joint)>,
}

/// Spawns `count` bare entities, then frees them in ascending order: the next spawns pop the
/// slots back in reverse.
fn churn(world: &mut World, count: usize) {
    let pads: Vec<EntityId> = (0..count).map(|_| world.spawn()).collect();
    for pad in pads {
        world.despawn(pad).expect("a pad is alive");
    }
}

fn box_shape(half: f32) -> Shape {
    Shape::Box {
        half_extents: [half; 3],
    }
}

impl Scene {
    fn build(churned: bool) -> Scene {
        let mut world = World::new();
        let pads = |world: &mut World, count| {
            if churned {
                churn(world, count);
            }
        };
        let mut spawned = Vec::new();
        pads(&mut world, 7);
        spawned.push(ragdoll::floor(&mut world));

        pads(&mut world, 11);
        let parts = ragdoll_parts();
        let mut limbs: Vec<EntityId> = Vec::new();
        for (half, at, link) in &parts {
            let mass = 300.0 * 8.0 * half[0] * half[1] * half[2];
            let part = spawn_moving(
                &mut world,
                Shape::Box {
                    half_extents: *half,
                },
                rigid(mass),
                *at,
                [0.5, 0.0, 0.0],
                [0.0; 3],
            );
            if let Some((parent, kind, anchor)) = link {
                world
                    .insert(
                        part,
                        Joint {
                            a: limbs[*parent],
                            b: part,
                            kind: kind.clone(),
                            anchor: *anchor,
                            break_impulse: None,
                            collide: false,
                        },
                    )
                    .unwrap();
            }
            limbs.push(part);
            spawned.push(part);
            pads(&mut world, 2);
        }

        pads(&mut world, 5);
        let mut stack = Vec::new();
        for level in 0..3 {
            let entity = spawn(
                &mut world,
                box_shape(0.5),
                rigid(10.0),
                [3.0, 0.5 + level as f32, 0.0],
                [0.0; 3],
            );
            stack.push(entity);
            spawned.push(entity);
            pads(&mut world, 3);
        }

        let motor = world.spawn();
        world.insert(motor, TestPose::at([6.0, 0.0, 0.0])).unwrap();
        world.insert(motor, CharacterMotor::default()).unwrap();
        spawned.push(motor);
        pads(&mut world, 4);

        let platform = spawn(
            &mut world,
            Shape::Box {
                half_extents: [1.0, 0.25, 1.0],
            },
            Body::new_kinematic(),
            [-4.0, 0.25, 0.0],
            [0.0; 3],
        );
        spawned.push(platform);
        pads(&mut world, 6);

        let spare = spawn(
            &mut world,
            box_shape(0.4),
            rigid(5.0),
            [-8.0, 0.4, 0.0],
            [0.0; 3],
        );
        spawned.push(spare);
        pads(&mut world, 3);

        Scene {
            world,
            schedule: schedule_with(PhysicsSettings::new(PhysicsSpace::Free)),
            spawned,
            stack,
            motor,
            platform,
            spare,
            limb: limbs[3],
            held: None,
        }
    }

    fn step(&mut self, frame: u64, churned: bool, extra: Extra) {
        if frame == 20 {
            // A collider change the body cannot retune in place: a rebuild.
            let bigger = collider(box_shape(0.55), rigid(10.0));
            self.world.insert(self.stack[1], bigger).unwrap();
        }
        if frame == 25 && churned {
            churn(&mut self.world, 5);
        }
        if frame == 30 {
            physics(&mut self.world)
                .radial_impulse(Blast {
                    center: [3.0, 1.0, 0.5],
                    radius: 6.0,
                    impulse: 6.0,
                    falloff: Falloff::Linear,
                    filter: QueryFilter::new(),
                })
                .expect("a valid blast queues");
        }
        self.event(frame, extra, churned);
        if frame >= 1 {
            let moves = [(self.motor, [-0.07, 0.0, 0.0])];
            let mut out = vec![Err(MotorError::NoPhysicsWorld); 1];
            move_characters::<TestPose, NoParent>(&mut self.world, &moves, &mut out);
        }
        tick(&mut self.world, &mut self.schedule, frame);
        if frame >= 60 && churned {
            churn(&mut self.world, 3);
        }
    }

    fn event(&mut self, frame: u64, extra: Extra, churned: bool) {
        match (extra, frame) {
            (Extra::RetireReclaim, 45) => {
                let held = self.world.remove::<Collider>(self.stack[2]).unwrap();
                self.held = held.map(|collider| (collider, dummy_joint(self.stack[2])));
            }
            (Extra::RetireReclaim, 50) => {
                let (collider, _) = self.held.take().expect("retired at 45");
                self.world.insert(self.stack[2], collider).unwrap();
            }
            (Extra::DespawnSpawn, 45) => {
                self.world.despawn(self.spare).unwrap();
                if churned {
                    churn(&mut self.world, 4);
                }
                let again = spawn(
                    &mut self.world,
                    box_shape(0.4),
                    rigid(5.0),
                    [-8.0, 2.0, 0.0],
                    [0.0; 3],
                );
                self.spared(again);
            }
            (Extra::JointReadd, 45) => {
                let joint = self.world.remove::<Joint>(self.limb).unwrap().unwrap();
                self.held = Some((collider(box_shape(0.1), rigid(1.0)), joint));
            }
            (Extra::JointReadd, 50) => {
                let (_, joint) = self.held.take().expect("removed at 45");
                self.world.insert(self.limb, joint).unwrap();
            }
            (Extra::Teleport, 45) => {
                self.world
                    .insert(self.platform, TestPose::at([-4.0, 3.0, 2.0]))
                    .unwrap();
                teleport_kinematic(&mut self.world, self.platform);
            }
            _ => {}
        }
    }

    fn spared(&mut self, again: EntityId) {
        self.spare = again;
        self.spawned.push(again);
    }

    fn rows(&self) -> Vec<Row> {
        self.spawned
            .iter()
            .map(|&entity| {
                if !self.world.is_alive(entity) {
                    return [0; 10];
                }
                let pose = pose_of(&self.world, entity);
                let linear = match self.world.get::<Velocity>(entity) {
                    Ok(Some(velocity)) => velocity.linear,
                    _ => [0.0; 3],
                };
                let mut row = [0; 10];
                for (slot, value) in row.iter_mut().zip(
                    pose.translation
                        .into_iter()
                        .chain(pose.rotation)
                        .chain(linear),
                ) {
                    *slot = value.to_bits();
                }
                row
            })
            .collect()
    }
}

/// A placeholder carrying nothing: only the collider half of `held` matters for a retire.
fn dummy_joint(entity: EntityId) -> Joint {
    Joint {
        a: entity,
        b: entity,
        kind: ragdoll::ball(1.0),
        anchor: [0.0; 3],
        break_impulse: None,
        collide: false,
    }
}

fn run(churned: bool, extra: Extra) -> Vec<Vec<Row>> {
    let mut scene = Scene::build(churned);
    (0..TICKS)
        .map(|frame| {
            scene.step(frame, churned, extra);
            scene.rows()
        })
        .collect()
}

fn assert_churn_free(extra: Extra) {
    let clean = run(false, extra);
    let churned = run(true, extra);
    assert_eq!(clean.len(), churned.len());
    for (tick, (a, b)) in clean.iter().zip(&churned).enumerate() {
        assert_eq!(a, b, "{extra:?}: tick {tick} differs under churn");
    }
}

#[test]
fn churn_does_not_move_a_ragdoll_a_stack_a_rebuild_a_blast_or_a_motor() {
    assert_churn_free(Extra::None);
}

#[test]
fn a_retire_then_reclaim_is_churn_free() {
    assert_churn_free(Extra::RetireReclaim);
}

#[test]
fn a_same_tick_despawn_and_spawn_is_churn_free() {
    assert_churn_free(Extra::DespawnSpawn);
}

#[test]
fn a_joint_readd_is_churn_free() {
    assert_churn_free(Extra::JointReadd);
}

#[test]
fn a_kinematic_teleport_is_churn_free() {
    assert_churn_free(Extra::Teleport);
}

#[test]
fn the_churned_world_has_a_permuted_slot_order() {
    let scene = Scene::build(true);
    assert!(
        scene.world.spawn_order().is_some(),
        "the scenario must permute slots, or the tests above prove nothing"
    );
}

#[test]
fn a_snapshot_taken_mid_run_keeps_the_order_and_stays_churn_free() {
    let reference = run(false, Extra::None);

    let mut scene = Scene::build(true);
    for frame in 0..SNAPSHOT_AT {
        scene.step(frame, true, Extra::None);
    }
    let bytes = physics(&mut scene.world)
        .to_bytes()
        .expect("the solver state encodes");
    let order = scene
        .world
        .spawn_order()
        .expect("the scenario permutes slots");

    // What `engine.restore` does: the allocator, the ECS state, the spawn order, the solver blob.
    let mut restored = World::new();
    match scene.world.allocator_state() {
        Some(state) => restored.restore_allocator(&state).unwrap(),
        None => {
            let live: Vec<EntityId> = scene.world.entities().collect();
            restored.restore_entities(&live).unwrap();
        }
    }
    for entity in scene.world.entities().collect::<Vec<_>>() {
        copy::<TestPose>(&scene.world, &mut restored, entity);
        copy::<Collider>(&scene.world, &mut restored, entity);
        copy::<Velocity>(&scene.world, &mut restored, entity);
        copy::<Joint>(&scene.world, &mut restored, entity);
        copy::<CharacterMotor>(&scene.world, &mut restored, entity);
        copy::<ludion_physics::CharacterMotorState>(&scene.world, &mut restored, entity);
    }
    restored.restore_spawn_order(&order).unwrap();
    restored.insert_resource(
        PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).expect("the captured bytes decode"),
    );
    scene.world = restored;
    scene.schedule = schedule_with(PhysicsSettings::new(PhysicsSpace::Free));

    for frame in SNAPSHOT_AT..TICKS {
        scene.step(frame, true, Extra::None);
        assert_eq!(
            scene.rows(),
            reference[frame as usize],
            "tick {frame} differs after the restore"
        );
    }
}

fn copy<T: ludion_ecs::Component + Clone>(from: &World, to: &mut World, entity: EntityId) {
    if let Ok(Some(value)) = from.get::<T>(entity) {
        to.insert(entity, value.clone()).unwrap();
    }
}

/// A floor and three separate sleepy boxes A, B, C (spawn order, so ranks A < B < C); A is rebuilt
/// early, so the handle table reads B, C, A. After all three sleep, `wake` fires and the awake
/// order, read right after the write, is returned.
fn wake_order(churned: bool, wake: impl Fn(&mut World, EntityId)) -> Vec<EntityId> {
    let mut world = World::new();
    if churned {
        churn(&mut world, 9);
    }
    let floor = ragdoll::floor(&mut world);
    let mut boxes = [EntityId::new(0, 0); 3];
    for (i, slot) in boxes.iter_mut().enumerate() {
        if churned {
            churn(&mut world, 3 + i);
        }
        let body = Body::Rigid(RigidBody {
            mass: 10.0,
            sleep_after_ticks: Some(5),
            ..RigidBody::default()
        });
        *slot = spawn(
            &mut world,
            box_shape(0.5),
            body,
            [3.0 * i as f32, 0.5, 0.0],
            [0.0; 3],
        );
    }
    let mut schedule = schedule_with(PhysicsSettings::new(PhysicsSpace::Free));
    for frame in 0..60 {
        if frame == 3 {
            let body = Body::Rigid(RigidBody {
                mass: 10.0,
                sleep_after_ticks: Some(5),
                ..RigidBody::default()
            });
            world
                .insert(boxes[0], collider(box_shape(0.55), body))
                .unwrap();
        }
        tick(&mut world, &mut schedule, frame);
    }
    let asleep = physics(&mut world).awake_order();
    assert!(
        boxes.iter().all(|b| !asleep.contains(b)),
        "the boxes must be asleep before the wake: {asleep:?}"
    );
    wake(&mut world, floor);
    // Read right after the write: the step rebuilds the awake island and forgets the wake order.
    let mut order = Vec::new();
    schedule.run_probed(
        &mut world,
        &SystemCtx::new(FrameIndex::new(60), TickRate::DEFAULT),
        &mut |name, world| {
            if name == ludion_physics::WRITE {
                order = world.resource::<PhysicsWorld>().unwrap().awake_order();
            }
        },
    );
    // Spawn order of the boxes, whatever slots they got.
    order
        .into_iter()
        .map(|e| EntityId::new(boxes.iter().position(|b| *b == e).unwrap() as u32, 0))
        .collect()
}

fn index(order: &[EntityId]) -> Vec<u32> {
    order.iter().map(|e| e.index()).collect()
}

#[test]
fn partner_wakes_follow_spawn_rank_even_after_a_rebuild() {
    // The floor is rebuilt, so its resting partners are woken. Rank order is A, B, C; the handle
    // table reads B, C, A (A was rebuilt at tick 3), so table order would wake B first.
    let rebuild_floor = |world: &mut World, floor: EntityId| {
        let bigger = Shape::Box {
            half_extents: [50.0, 1.0, 51.0],
        };
        world
            .insert(floor, collider(bigger, Body::new_static()))
            .unwrap();
    };
    assert_eq!(index(&wake_order(false, rebuild_floor)), [0, 1, 2]);
    assert_eq!(index(&wake_order(true, rebuild_floor)), [0, 1, 2]);
}

#[test]
fn a_restore_refills_the_rank_table_in_the_same_relative_order() {
    let mut scene = Scene::build(true);
    for frame in 0..SNAPSHOT_AT {
        scene.step(frame, true, Extra::None);
    }
    let by_rank = |ranks: &[(EntityId, u64)]| {
        let mut ranks = ranks.to_vec();
        ranks.sort_by_key(|&(_, rank)| rank);
        ranks.into_iter().map(|(e, _)| e).collect::<Vec<_>>()
    };
    let source = by_rank(physics(&mut scene.world).spawn_ranks_view());
    assert!(!source.is_empty(), "the write fills the table");
    let bytes = physics(&mut scene.world).to_bytes().unwrap();
    let order = scene.world.spawn_order().expect("slots are permuted");

    let mut restored = World::new();
    restored
        .restore_allocator(
            &scene
                .world
                .allocator_state()
                .expect("an irreducible allocator"),
        )
        .unwrap();
    restored.restore_spawn_order(&order).unwrap();
    let mut fresh = PhysicsWorld::from_bytes(&bytes, TickRate::DEFAULT).unwrap();
    assert!(fresh.spawn_ranks_view().is_empty());
    fresh.sync_spawn_ranks(&restored);
    assert_eq!(by_rank(fresh.spawn_ranks_view()), source);
}
