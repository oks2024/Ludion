//! The drag-carry recipe (`docs/motor.md` §Carrying): a ragdoll part `Joint`ed to a kinematic
//! anchor follows it, a sleeping part wakes when the anchor moves, a carrier-side layer swap frees
//! the carrier from the carried parts without touching any joint, and a yanked anchor breaks its
//! joint. A system registered before `install` plays the hierarchy and drives the anchor's pose.

mod common;

use std::sync::{Arc, Mutex};

use common::{TestPose, pose_of, tick};
use ludion_core::ids::EntityId;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    CharacterMotor, CollisionLayers, JointBroken, NoParent, PhysicsSettings, PhysicsSpace,
    PhysicsWorld, install, move_character,
};
use ludion_spatial::{Body, Collider, Joint, JointKind, RigidBody, Shape, Velocity};

const STEP: f32 = 0.02;

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn collider(shape: Shape, body: Body, layer: Option<&str>) -> Collider {
    Collider {
        shape,
        body,
        layer: layer.map(str::to_string),
        sensor: false,
    }
}

fn ball(radius: f32) -> Shape {
    Shape::Sphere { radius }
}

fn spawn(world: &mut World, at: [f32; 3], collider: Collider) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(at)).unwrap();
    world.insert(entity, collider).unwrap();
    entity
}

fn kinematic_anchor(world: &mut World, at: [f32; 3], layer: Option<&str>) -> EntityId {
    spawn(
        world,
        at,
        collider(ball(0.05), Body::new_kinematic(), layer),
    )
}

fn part(
    world: &mut World,
    at: [f32; 3],
    sleep_after_ticks: Option<u32>,
    layer: Option<&str>,
) -> EntityId {
    let body = Body::Rigid(RigidBody {
        sleep_after_ticks,
        ..RigidBody::default()
    });
    let entity = spawn(world, at, collider(ball(0.1), body, layer));
    world.insert(entity, Velocity::default()).unwrap();
    entity
}

/// Joins `part` to `to`, pivoting about the part's own centre so the formation offset is the
/// held pose: gravity exerts no torque and the pair rests exactly where it was authored.
fn join(
    world: &mut World,
    part: EntityId,
    to: EntityId,
    kind: JointKind,
    break_impulse: Option<f32>,
) {
    let a = world.get::<TestPose>(to).unwrap().unwrap().translation;
    let b = world.get::<TestPose>(part).unwrap().unwrap().translation;
    world
        .insert(
            part,
            Joint {
                a: to,
                b: part,
                kind,
                anchor: [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                break_impulse,
                collide: false,
                zero: None,
            },
        )
        .unwrap();
}

fn ball_joint() -> JointKind {
    JointKind::Ball { limits: None }
}

/// `anchor`'s pose is `script(frame)`; the system runs before physics, as the hierarchy does.
fn rig(
    world: World,
    layers: Option<CollisionLayers>,
    script: impl Fn(&mut World, u64) + Send + Sync + 'static,
) -> (World, Schedule) {
    let mut schedule = Schedule::new();
    schedule.add_system_shared(
        "test-script",
        Arc::new(move |world: &mut World, ctx: &SystemCtx| script(world, ctx.frame.as_u64())),
    );
    let settings = PhysicsSettings {
        layers,
        ..PhysicsSettings::new(PhysicsSpace::Free)
    };
    install::<TestPose, NoParent>(&mut schedule, settings);
    (world, schedule)
}

fn set_x(world: &mut World, entity: EntityId, x: f32) {
    world
        .get_mut::<TestPose>(entity)
        .unwrap()
        .unwrap()
        .translation[0] = x;
}

fn offset(world: &World, from: EntityId, to: EntityId) -> [f32; 3] {
    let (a, b) = (
        pose_of(world, from).translation,
        pose_of(world, to).translation,
    );
    [b[0] - a[0], b[1] - a[1], b[2] - a[2]]
}

fn sleeping(world: &World, entity: EntityId) -> bool {
    world
        .resource::<PhysicsWorld>()
        .unwrap()
        .is_sleeping(entity)
        .unwrap()
}

#[test]
fn a_rigid_body_follows_a_moving_kinematic_anchor() {
    let mut world = World::new();
    let anchor = kinematic_anchor(&mut world, [0.0, 2.0, 0.0], None);
    let bob = part(&mut world, [0.0, 1.0, 0.0], None, None);
    join(&mut world, bob, anchor, ball_joint(), None);
    let (mut world, mut schedule) = rig(world, None, move |world, frame| {
        set_x(world, anchor, STEP * frame as f32);
    });
    for frame in 0..300 {
        tick(&mut world, &mut schedule, frame);
        let rest = [0.0, -1.0, 0.0];
        let now = offset(&world, anchor, bob);
        assert!(
            distance(now, rest) <= 0.05,
            "tick {frame}: bob lags its anchor, offset {now:?}"
        );
    }
    let physics = world.resource::<PhysicsWorld>().unwrap();
    assert_eq!(physics.is_joint_active(bob), Some(true));
    assert!(pose_of(&world, bob).translation[0] > 5.0);
}

#[test]
fn a_sleeping_part_wakes_when_its_anchor_moves() {
    let mut world = World::new();
    let anchor = kinematic_anchor(&mut world, [0.0, 2.0, 0.0], None);
    let bob = part(&mut world, [0.0, 1.0, 0.0], Some(10), None);
    join(&mut world, bob, anchor, ball_joint(), None);
    const START: u64 = 60;
    let (mut world, mut schedule) = rig(world, None, move |world, frame| {
        if frame >= START {
            set_x(world, anchor, STEP * (frame - START) as f32);
        }
    });
    for frame in 0..START {
        tick(&mut world, &mut schedule, frame);
    }
    assert!(sleeping(&world, bob), "the bob never fell asleep");
    for frame in START..START + 120 {
        tick(&mut world, &mut schedule, frame);
        let now = offset(&world, anchor, bob);
        assert!(
            distance(now, [0.0, -1.0, 0.0]) <= 0.05,
            "tick {frame}: a sleeping bob was left behind, offset {now:?}"
        );
    }
}

#[test]
fn a_break_impulse_on_an_anchor_joint_reports() {
    let mut world = World::new();
    let anchor = kinematic_anchor(&mut world, [0.0, 2.0, 0.0], None);
    let bob = part(&mut world, [0.0, 1.0, 0.0], None, None);
    join(&mut world, bob, anchor, ball_joint(), Some(0.01));
    let (mut world, mut schedule) = rig(world, None, move |world, frame| {
        if frame >= 20 {
            set_x(world, anchor, 5.0);
        }
    });
    let mut broken: Vec<JointBroken> = Vec::new();
    for frame in 0..60 {
        tick(&mut world, &mut schedule, frame);
        broken.extend_from_slice(world.resource::<PhysicsWorld>().unwrap().joint_events());
    }
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert_eq!(
        (broken[0].a, broken[0].b, broken[0].joint),
        (anchor, bob, bob)
    );
}

/// The layer table the recipe declares, in `ludion_scene::LayerMatrix`'s symmetric form.
fn carry_layers() -> CollisionLayers {
    // default 0, body 1, carrying 2, anchor 3.
    let names = ["default", "body", "carrying", "anchor"]
        .map(str::to_string)
        .to_vec();
    let mut masks = [0u32; 4];
    let mut ignore = |a: usize, b: usize| {
        masks[a] |= 1 << b;
        masks[b] |= 1 << a;
    };
    ignore(2, 1);
    for other in 0..4 {
        ignore(3, other);
    }
    CollisionLayers::new(names, masks.to_vec())
}

fn motor() -> CharacterMotor {
    CharacterMotor {
        radius: 0.28,
        height: 0.9,
        step_height: 0.25,
        skin_width: 0.01,
        ..CharacterMotor::default()
    }
}

struct Carry {
    world: World,
    schedule: Schedule,
    hero: EntityId,
    torso: EntityId,
    limb: EntityId,
    blockers: Arc<Mutex<Vec<EntityId>>>,
    swap_at: Arc<Mutex<Option<u64>>>,
}

/// A motor at x = -2 walking +x through a body hanging at x = 0: a torso off a shoulder anchor and
/// a limb off the torso, in the carrier's path. The motor swaps to `carrying` at the tick in
/// `swap_at`.
fn carry_scene() -> Carry {
    let mut world = World::new();
    spawn(
        &mut world,
        [0.0, -0.5, 0.0],
        collider(
            Shape::Box {
                half_extents: [30.0, 0.5, 30.0],
            },
            Body::new_static(),
            None,
        ),
    );
    let hero = world.spawn();
    world.insert(hero, TestPose::at([-2.0, 0.0, 0.0])).unwrap();
    world.insert(hero, motor()).unwrap();
    let anchor = kinematic_anchor(&mut world, [0.0, 1.5, 0.0], Some("anchor"));
    let torso = part(&mut world, [0.0, 1.2, 0.0], Some(5), Some("body"));
    let limb = part(&mut world, [0.0, 0.5, 0.0], Some(5), Some("body"));
    join(&mut world, torso, anchor, ball_joint(), None);
    join(&mut world, limb, torso, ball_joint(), None);

    let blockers: Arc<Mutex<Vec<EntityId>>> = Arc::default();
    let swap_at: Arc<Mutex<Option<u64>>> = Arc::default();
    let (sink, swap) = (blockers.clone(), swap_at.clone());
    let (world, schedule) = rig(world, Some(carry_layers()), move |world, frame| {
        if *swap.lock().unwrap() == Some(frame) {
            world
                .get_mut::<CharacterMotor>(hero)
                .unwrap()
                .unwrap()
                .layer = Some("carrying".into());
        }
        if frame >= 60 {
            let report = move_character::<TestPose, NoParent>(world, hero, [0.05, -0.1, 0.0])
                .expect("the hero walks");
            let mut sink = sink.lock().unwrap();
            sink.extend(report.walls.iter().map(|hit| hit.entity));
            sink.extend(report.ground.iter().map(|g| g.entity));
        }
    });
    Carry {
        world,
        schedule,
        hero,
        torso,
        limb,
        blockers,
        swap_at,
    }
}

fn run(sim: &mut Carry, from: u64, to: u64) {
    for frame in from..to {
        tick(&mut sim.world, &mut sim.schedule, frame);
    }
}

#[test]
fn the_carrier_layer_swap_frees_the_pair_without_touching_the_joint() {
    // Control: no swap — the capsule meets the limb and pushes it off its spot.
    let mut control = carry_scene();
    let limb_rest = pose_of(&control.world, control.limb).translation;
    run(&mut control, 0, 160);
    let pushed = distance(pose_of(&control.world, control.limb).translation, limb_rest);
    assert!(
        pushed > 0.02 || control.blockers.lock().unwrap().contains(&control.limb),
        "the control run never touched the limb (moved {pushed})"
    );

    // Swap at frame 50, while the parts are asleep and untouched.
    let mut swap = carry_scene();
    *swap.swap_at.lock().unwrap() = Some(50);
    run(&mut swap, 0, 50);
    assert!(sleeping(&swap.world, swap.limb) && sleeping(&swap.world, swap.torso));
    let before = [
        pose_of(&swap.world, swap.torso),
        pose_of(&swap.world, swap.limb),
    ];
    run(&mut swap, 50, 51);
    let after = [
        pose_of(&swap.world, swap.torso),
        pose_of(&swap.world, swap.limb),
    ];
    for (b, a) in before.iter().zip(&after) {
        assert_eq!(
            b.rotation.map(f32::to_bits),
            a.rotation.map(f32::to_bits),
            "the swap tick turned a jointed part"
        );
    }
    assert!(
        sleeping(&swap.world, swap.limb) && sleeping(&swap.world, swap.torso),
        "the swap woke a part"
    );
    run(&mut swap, 51, 160);
    let blockers = swap.blockers.lock().unwrap();
    assert!(
        !blockers.contains(&swap.limb) && !blockers.contains(&swap.torso),
        "the carrying motor still names a body part: {blockers:?}"
    );
    let limb = pose_of(&swap.world, swap.limb).translation;
    assert!(
        distance(limb, limb_rest) < 1e-3,
        "the carrying capsule pushed the limb to {limb:?}"
    );
    let hero_x = pose_of(&swap.world, swap.hero).translation[0];
    assert!(hero_x > 0.5, "the carrier did not walk through: {hero_x}");
}
