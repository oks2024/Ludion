//! Collision layers reach the solver as groups: a pair collides unless either side ignores the
//! other, an undeclared layer builds no body, and a table or `layer:` edit takes effect live.
//! The determinism half — a layered pile with a jointed figure — traces identically twice, against
//! a committed reference, across a snapshot resume, and whatever order the layers are declared in.

mod common;

use std::path::PathBuf;

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, SystemCtx, World};
use ludion_physics::{
    CollisionLayers, ContactEvent, ContactPhase, NoParent, PhysicsSettings, PhysicsSpace,
    PhysicsWorld, ShapeRejection, install,
};
use ludion_spatial::{Body, Collider, Joint, JointKind, RigidBody, Shape, Velocity};
use xxhash_rust::xxh3::Xxh3;

/// The table a scene declaring `layers` resolves to: `default` at index 0, then declaration order,
/// each `ignores` entry applied symmetrically — `ludion_scene::LayerMatrix`'s rule, restated here
/// because this crate does not depend on the scene.
fn table(layers: &[(&str, &[&str])]) -> CollisionLayers {
    let mut names = vec!["default".to_string()];
    names.extend(layers.iter().map(|(name, _)| name.to_string()));
    let mut masks = vec![0u32; names.len()];
    for (offset, (_, ignores)) in layers.iter().enumerate() {
        let index = offset + 1;
        for ignored in *ignores {
            let other = names
                .iter()
                .position(|name| name == ignored)
                .expect("the test ignores a declared layer");
            masks[index] |= 1 << other;
            masks[other] |= 1 << index;
        }
    }
    CollisionLayers::new(names, masks)
}

fn settings(layers: Option<CollisionLayers>) -> PhysicsSettings {
    PhysicsSettings {
        layers,
        ..PhysicsSettings::new(PhysicsSpace::Free)
    }
}

fn collider(shape: Shape, body: Body, layer: Option<&str>) -> Collider {
    Collider {
        shape,
        body,
        layer: layer.map(str::to_string),
        sensor: false,
    }
}

fn cube(body: Body, layer: Option<&str>) -> Collider {
    collider(
        Shape::Box {
            half_extents: [0.5; 3],
        },
        body,
        layer,
    )
}

fn rigid() -> Body {
    Body::Rigid(RigidBody::default())
}

fn spawn(world: &mut World, translation: [f32; 3], collider: Collider) -> EntityId {
    let entity = world.spawn();
    world
        .insert(entity, TestPose::at(translation))
        .expect("a fresh entity accepts a pose");
    world
        .insert(entity, collider)
        .expect("a fresh entity accepts a collider");
    world
        .insert(entity, Velocity::default())
        .expect("a fresh entity accepts a velocity");
    entity
}

/// A wide static floor whose top face is `y = 0`.
fn floor(world: &mut World, layer: Option<&str>) -> EntityId {
    spawn(
        world,
        [0.0, -0.5, 0.0],
        collider(
            Shape::Box {
                half_extents: [20.0, 0.5, 20.0],
            },
            Body::new_static(),
            layer,
        ),
    )
}

fn set_collider(world: &mut World, entity: EntityId, collider: Collider) {
    *world
        .get_mut::<Collider>(entity)
        .expect("a live entity")
        .expect("its collider") = collider;
}

fn y_of(world: &World, entity: EntityId) -> f32 {
    pose_of(world, entity).translation[1]
}

/// A world, the schedule stepping it, and everything the solver reported while it ran.
struct Sim {
    world: World,
    schedule: Schedule,
    frame: u64,
    contacts: Vec<ContactEvent>,
    rejections: Vec<ShapeRejection>,
}

impl Sim {
    fn new(layers: Option<CollisionLayers>) -> Self {
        let mut sim = Sim {
            world: World::new(),
            schedule: Schedule::new(),
            frame: 0,
            contacts: Vec::new(),
            rejections: Vec::new(),
        };
        sim.reinstall(layers);
        sim
    }

    /// A live matrix edit: a new schedule over the same world, the way the engine reinstalls the
    /// triple. The [`PhysicsWorld`] resource persists.
    fn reinstall(&mut self, layers: Option<CollisionLayers>) {
        self.schedule = Schedule::new();
        install::<TestPose, NoParent>(&mut self.schedule, settings(layers));
    }

    fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            tick(&mut self.world, &mut self.schedule, self.frame);
            self.frame += 1;
            let physics = self
                .world
                .resource_mut::<PhysicsWorld>()
                .expect("a claimed body installs the solver");
            self.rejections.extend(physics.take_rejections());
            self.contacts.extend_from_slice(physics.contact_events());
        }
    }

    fn physics(&self) -> &PhysicsWorld {
        self.world
            .resource::<PhysicsWorld>()
            .expect("a claimed body installs the solver")
    }

    /// Whether `a` and `b` were ever reported touching, in either order.
    fn touched(&self, a: EntityId, b: EntityId) -> bool {
        let pair = (a.min(b), a.max(b));
        self.contacts
            .iter()
            .any(|event| (event.a, event.b) == pair && event.phase == ContactPhase::Started)
    }
}

#[test]
fn stacked_boxes_on_a_self_ignoring_layer_fall_through_each_other() {
    let mut sim = Sim::new(Some(table(&[("props", &["props"])])));
    floor(&mut sim.world, None);
    let lower = spawn(
        &mut sim.world,
        [0.0, 1.0, 0.0],
        cube(rigid(), Some("props")),
    );
    let upper = spawn(
        &mut sim.world,
        [0.0, 2.5, 0.0],
        cube(rigid(), Some("props")),
    );
    sim.run(150);

    assert!((y_of(&sim.world, lower) - 0.5).abs() < 0.02);
    assert!(
        (y_of(&sim.world, upper) - 0.5).abs() < 0.02,
        "the upper box must fall through the lower onto the floor, not rest on it"
    );
    assert!(
        !sim.touched(lower, upper),
        "no ContactEvent may name the pair"
    );
}

#[test]
fn the_same_stack_without_the_ignore_rests_and_touches() {
    let mut sim = Sim::new(Some(table(&[("props", &[])])));
    floor(&mut sim.world, None);
    let lower = spawn(
        &mut sim.world,
        [0.0, 1.0, 0.0],
        cube(rigid(), Some("props")),
    );
    let upper = spawn(
        &mut sim.world,
        [0.0, 2.5, 0.0],
        cube(rigid(), Some("props")),
    );
    sim.run(150);

    assert!((y_of(&sim.world, upper) - 1.5).abs() < 0.05);
    assert!(sim.touched(lower, upper));
}

/// A rigid box on `dynamic_layer` dropped through a static plate on `plate_layer`, above an
/// unlayered floor: where it ends up.
fn dropped_through_plate(
    layers: &CollisionLayers,
    dynamic_layer: Option<&str>,
    plate_layer: Option<&str>,
) -> f32 {
    let mut sim = Sim::new(Some(layers.clone()));
    floor(&mut sim.world, None);
    spawn(
        &mut sim.world,
        [0.0, 2.0, 0.0],
        collider(
            Shape::Box {
                half_extents: [2.0, 0.25, 2.0],
            },
            Body::new_static(),
            plate_layer,
        ),
    );
    let dropped = spawn(
        &mut sim.world,
        [0.0, 4.0, 0.0],
        cube(rigid(), dynamic_layer),
    );
    sim.run(150);
    y_of(&sim.world, dropped)
}

#[test]
fn a_one_sided_ignore_filters_both_directions() {
    let layers = table(&[("a", &["b"]), ("b", &[])]);
    let plate_top_rest = 2.0 + 0.25 + 0.5;

    // Control: two colliders that do not ignore each other rest on the plate.
    let rests = dropped_through_plate(&layers, Some("a"), Some("a"));
    assert!((rests - plate_top_rest).abs() < 0.05, "control: {rests}");

    // A ignores B, so A falls through a static B; and B falls through a static A.
    let a_through_b = dropped_through_plate(&layers, Some("a"), Some("b"));
    assert!(
        (a_through_b - 0.5).abs() < 0.05,
        "a through b: {a_through_b}"
    );
    let b_through_a = dropped_through_plate(&layers, Some("b"), Some("a"));
    assert!(
        (b_through_a - 0.5).abs() < 0.05,
        "b through a: {b_through_a}"
    );
}

#[test]
fn the_default_layer_can_be_ignored() {
    let mut sim = Sim::new(Some(table(&[("ghost", &["default"])])));
    floor(&mut sim.world, None);
    let ghost = spawn(
        &mut sim.world,
        [0.0, 2.0, 0.0],
        cube(rigid(), Some("ghost")),
    );
    sim.run(120);
    assert!(
        y_of(&sim.world, ghost) < -5.0,
        "a ghost falls through an unlayered floor: y = {}",
        y_of(&sim.world, ghost)
    );
}

#[test]
fn a_live_matrix_edit_takes_effect_next_tick_and_reverses() {
    let colliding = table(&[("x", &[])]);
    let ignoring = table(&[("x", &["x"])]);
    let mut sim = Sim::new(Some(colliding.clone()));
    floor(&mut sim.world, None);
    let lower = spawn(&mut sim.world, [0.0, 0.5, 0.0], cube(rigid(), Some("x")));
    let upper = spawn(&mut sim.world, [0.0, 1.5, 0.0], cube(rigid(), Some("x")));
    sim.run(90);
    assert_eq!(
        sim.physics().is_sleeping(upper),
        Some(true),
        "the stack must be asleep so the edit has to wake it"
    );
    let resting = y_of(&sim.world, upper);
    assert!((resting - 1.5).abs() < 0.05);

    sim.reinstall(Some(ignoring.clone()));
    sim.run(2);
    assert!(
        y_of(&sim.world, upper) < resting - 1e-3,
        "the top box must be sinking within two ticks of the edit"
    );
    sim.run(90);
    assert!((y_of(&sim.world, upper) - 0.5).abs() < 0.05);
    assert!((y_of(&sim.world, lower) - 0.5).abs() < 0.05);

    // The reverse edit: a box still falling meets the other again once `x` stops ignoring itself.
    let mut sim = Sim::new(Some(ignoring));
    floor(&mut sim.world, None);
    let lower = spawn(&mut sim.world, [0.0, 0.5, 0.0], cube(rigid(), Some("x")));
    let upper = spawn(&mut sim.world, [0.0, 6.0, 0.0], cube(rigid(), Some("x")));
    sim.run(2);
    sim.reinstall(Some(colliding));
    sim.run(180);
    assert!(
        (y_of(&sim.world, upper) - 1.5).abs() < 0.05,
        "contact is restored: y = {}",
        y_of(&sim.world, upper)
    );
    assert!(sim.touched(lower, upper));
}

type StateBits = ([u32; 3], [u32; 4], [u32; 3], [u32; 3]);

fn state_bits(world: &World, entity: EntityId) -> StateBits {
    let pose = pose_of(world, entity);
    let velocity = *world
        .get::<Velocity>(entity)
        .expect("a live entity")
        .expect("the test gives it a Velocity");
    (
        pose.translation.map(f32::to_bits),
        pose.rotation.map(f32::to_bits),
        velocity.linear.map(f32::to_bits),
        velocity.angular.map(f32::to_bits),
    )
}

#[test]
fn a_layer_edit_rebuilds_at_pose_and_velocity_and_the_body_then_passes_through() {
    let layers = table(&[("ghost", &["default"])]);
    let build = || {
        let mut sim = Sim::new(Some(layers.clone()));
        floor(&mut sim.world, None);
        let falling = spawn(&mut sim.world, [0.0, 6.0, 0.0], cube(rigid(), None));
        (sim, falling)
    };

    let (mut control, falling) = build();
    control.run(11);
    let (mut edited, _) = build();
    edited.run(10);
    set_collider(&mut edited.world, falling, cube(rigid(), Some("ghost")));
    edited.run(1);
    assert_eq!(
        state_bits(&edited.world, falling),
        state_bits(&control.world, falling),
        "the rebuild must carry pose and velocity over exactly"
    );

    edited.run(120);
    assert!(
        y_of(&edited.world, falling) < -5.0,
        "the edited body passes through the floor it would have hit"
    );
    control.run(120);
    assert!((y_of(&control.world, falling) - 0.5).abs() < 0.05);
}

#[test]
fn an_unknown_layer_builds_no_body_and_reports_l1186_once() {
    let mut sim = Sim::new(Some(table(&[("dummy", &["dummy"])])));
    let entity = spawn(&mut sim.world, [0.0, 3.0, 0.0], cube(rigid(), Some("dumy")));
    sim.run(6);

    assert_eq!(sim.physics().body_count(), 0);
    assert_eq!(sim.rejections.len(), 1, "{:?}", sim.rejections);
    assert_eq!(sim.rejections[0].entity, entity);
    assert_eq!(sim.rejections[0].code, code::COLLIDER_LAYER_UNKNOWN);
    assert!(
        sim.rejections[0].reason.contains("`dummy`"),
        "the reason names the nearest match: {}",
        sim.rejections[0].reason
    );

    // Editing the Collider to a declared layer builds it.
    set_collider(&mut sim.world, entity, cube(rigid(), Some("dummy")));
    sim.run(2);
    assert_eq!(sim.physics().body_count(), 1);
    assert_eq!(sim.rejections.len(), 1);
}

#[test]
fn a_table_edit_declaring_the_name_builds_it_and_removing_the_name_drops_the_body() {
    let mut sim = Sim::new(Some(table(&[("dummy", &[])])));
    let entity = spawn(
        &mut sim.world,
        [0.0, 3.0, 0.0],
        cube(rigid(), Some("props")),
    );
    sim.run(4);
    assert_eq!(sim.physics().body_count(), 0);
    assert_eq!(sim.rejections.len(), 1);

    sim.reinstall(Some(table(&[("dummy", &[]), ("props", &[])])));
    sim.run(2);
    assert_eq!(sim.physics().body_count(), 1, "a declared layer builds");
    assert_eq!(sim.rejections.len(), 1);

    let before = y_of(&sim.world, entity);
    sim.reinstall(Some(table(&[("dummy", &[])])));
    sim.run(4);
    assert_eq!(
        sim.physics().body_count(),
        0,
        "an undeclared layer drops it"
    );
    assert_eq!(sim.rejections.len(), 2, "{:?}", sim.rejections);
    assert_eq!(sim.rejections[1].code, code::COLLIDER_LAYER_UNKNOWN);
    assert_eq!(sim.rejections[1].entity, entity);
    assert!(
        y_of(&sim.world, entity) <= before,
        "the pose is left as it was"
    );
}

#[test]
fn a_layer_with_no_table_is_rejected_and_default_is_always_valid() {
    let mut sim = Sim::new(None);
    floor(&mut sim.world, Some("default"));
    let stray = spawn(
        &mut sim.world,
        [0.0, 3.0, 0.0],
        cube(rigid(), Some("props")),
    );
    sim.run(3);
    assert_eq!(sim.physics().body_count(), 1, "only the floor builds");
    assert_eq!(sim.rejections.len(), 1);
    assert_eq!(sim.rejections[0].entity, stray);
    assert!(sim.rejections[0].reason.contains("physics.layers"));
}

/// Two rigid boxes overlapping by 0.4, jointed with `collide: true`.
fn jointed_overlap(
    layers: Option<CollisionLayers>,
    layer: Option<&str>,
) -> (Sim, EntityId, EntityId) {
    let mut sim = Sim::new(layers);
    floor(&mut sim.world, None);
    let a = spawn(&mut sim.world, [0.0, 3.0, 0.0], cube(rigid(), layer));
    let b = spawn(&mut sim.world, [0.0, 3.6, 0.0], cube(rigid(), layer));
    sim.world
        .insert(
            b,
            Joint {
                a,
                b,
                kind: JointKind::Ball { limits: None },
                anchor: [0.0, 0.3, 0.0],
                break_impulse: None,
                collide: true,
            },
        )
        .expect("a live entity accepts a joint");
    (sim, a, b)
}

#[test]
fn a_colliding_joint_between_parts_on_a_self_ignoring_layer_yields_no_contact() {
    let (mut control, a, b) = jointed_overlap(None, None);
    control.run(40);
    assert!(
        control.touched(a, b),
        "control: `collide: true` makes the pair touch"
    );

    let (mut layered, a, b) =
        jointed_overlap(Some(table(&[("figure", &["figure"])])), Some("figure"));
    layered.run(40);
    assert!(
        !layered.touched(a, b),
        "the layer filter wins over `collide: true`"
    );
}

// ---- The determinism gates: a layered pile with a jointed figure -------------------------------

const BOXES: usize = 40;
const TICKS: u64 = 300;
const SNAPSHOT_TICK: u64 = 150;
const KICK_PERIOD: u64 = 25;

fn reference_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/traces/layers.txt")
}

/// The two layers of the trace scene, declared in either order — the resolved rule is the same.
fn trace_layers(figure_first: bool) -> CollisionLayers {
    if figure_first {
        table(&[("figure", &["figure"]), ("props", &[])])
    } else {
        table(&[("props", &[]), ("figure", &["figure"])])
    }
}

/// The pile's lattice with an integer-derived jitter: asymmetric, hence chaotic, with no RNG.
fn drop_position(index: usize) -> [f32; 3] {
    let column = (index % 5) as f32;
    let row = ((index / 5) % 5) as f32;
    let layer = (index / 25) as f32;
    let jitter = ((index * 37) % 13) as f32 * 0.01;
    [
        column * 1.1 - 2.2 + jitter,
        layer * 1.3 + 0.6,
        row * 1.1 - 2.2 - jitter,
    ]
}

/// One figure part: a shape at a position on the `figure` layer.
fn part(world: &mut World, translation: [f32; 3], shape: Shape) -> EntityId {
    spawn(
        world,
        translation,
        collider(
            shape,
            Body::Rigid(RigidBody {
                sleep_after_ticks: Some(30),
                ..RigidBody::default()
            }),
            Some("figure"),
        ),
    )
}

/// A static floor, [`BOXES`] boxes on `props`, and a six-part figure on `figure` whose hips, arms
/// and head overlap the torso, dropped above the pile. Spawned in a fixed order.
fn build(world: &mut World) {
    floor(world, None);
    for index in 0..BOXES {
        spawn(
            world,
            drop_position(index),
            collider(
                Shape::Box {
                    half_extents: [0.5; 3],
                },
                Body::Rigid(RigidBody {
                    mass: 1.0,
                    friction: 0.5,
                    restitution: 0.1,
                    sleep_after_ticks: Some(30),
                    ..RigidBody::default()
                }),
                Some("props"),
            ),
        );
    }

    let torso = part(
        world,
        [0.2, 7.0, 0.1],
        Shape::Box {
            half_extents: [0.3, 0.5, 0.2],
        },
    );
    let limb = |half_height: f32| Shape::Capsule {
        half_height,
        radius: 0.15,
    };
    let joints = [
        (
            part(world, [0.2, 7.7, 0.1], Shape::Sphere { radius: 0.25 }),
            [0.0, 0.7, 0.0],
        ),
        (part(world, [-0.15, 7.2, 0.1], limb(0.3)), [-0.35, 0.2, 0.0]),
        (part(world, [0.55, 7.2, 0.1], limb(0.3)), [0.35, 0.2, 0.0]),
        (
            part(world, [0.05, 6.05, 0.1], limb(0.4)),
            [-0.15, -0.5, 0.0],
        ),
        (part(world, [0.35, 6.05, 0.1], limb(0.4)), [0.15, -0.5, 0.0]),
    ];
    for (child, anchor) in joints {
        world
            .insert(
                child,
                Joint {
                    a: torso,
                    b: child,
                    kind: JointKind::Ball { limits: None },
                    anchor,
                    break_impulse: None,
                    collide: false,
                },
            )
            .expect("a fresh entity accepts a joint");
    }
}

/// Every [`KICK_PERIOD`] ticks, adds an integer-derived impulse to each body's velocity, so the
/// pile never sleeps into one repeated trace line.
fn kick(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64();
    if frame == 0 || !frame.is_multiple_of(KICK_PERIOD) {
        return;
    }
    let round = (frame / KICK_PERIOD) as u32;
    for (nth, (velocity,)) in world.query::<(&mut Velocity,)>().enumerate() {
        let mix = (nth as u32)
            .wrapping_mul(2_654_435_761)
            .wrapping_add(round.wrapping_mul(40_503))
            % 1000;
        velocity.linear[0] += (mix % 7) as f32 - 3.0;
        velocity.linear[1] += (mix % 5) as f32 * 0.8 + 2.5;
        velocity.linear[2] += (mix % 11) as f32 * 0.5 - 2.5;
        velocity.angular[1] += (mix % 3) as f32 - 1.0;
    }
}

fn trace_schedule(figure_first: bool) -> Schedule {
    let mut schedule = Schedule::new();
    schedule.add_system("kick", kick);
    install::<TestPose, NoParent>(&mut schedule, settings(Some(trace_layers(figure_first))));
    schedule
}

/// One trace line: the frame and an XXH3 digest of every body's pose in handle-table order.
fn trace_line(world: &World, frame: u64) -> String {
    let physics = world
        .resource::<PhysicsWorld>()
        .expect("the write system installs the world on the first tick");
    let mut digest = Xxh3::new();
    for entity in physics.entities() {
        let pose = pose_of(world, entity);
        for value in pose.translation.into_iter().chain(pose.rotation) {
            digest.update(&value.to_bits().to_le_bytes());
        }
    }
    format!("{frame:04} {:016x}", digest.digest())
}

fn run(figure_first: bool) -> Vec<String> {
    let mut world = World::new();
    build(&mut world);
    let mut schedule = trace_schedule(figure_first);
    let mut trace = Vec::with_capacity(TICKS as usize);
    for frame in 0..TICKS {
        tick(&mut world, &mut schedule, frame);
        trace.push(trace_line(&world, frame));
    }
    trace
}

/// The ECS half of a session by hand, the solver half as snapshot bytes.
struct Capture {
    entities: Vec<EntityId>,
    colliders: Vec<Collider>,
    poses: Vec<TestPose>,
    velocities: Vec<Velocity>,
    joints: Vec<Option<Joint>>,
    bytes: Vec<u8>,
}

fn capture(world: &World) -> Capture {
    let entities: Vec<EntityId> = world.entities().collect();
    Capture {
        colliders: entities
            .iter()
            .map(|&e| world.get::<Collider>(e).unwrap().unwrap().clone())
            .collect(),
        poses: entities.iter().map(|&e| pose_of(world, e)).collect(),
        velocities: entities
            .iter()
            .map(|&e| *world.get::<Velocity>(e).unwrap().unwrap())
            .collect(),
        joints: entities
            .iter()
            .map(|&e| world.get::<Joint>(e).unwrap().cloned())
            .collect(),
        bytes: world
            .resource::<PhysicsWorld>()
            .expect("the write system installed a world")
            .to_bytes()
            .expect("the solver state encodes"),
        entities,
    }
}

fn restore(capture: &Capture) -> World {
    let mut world = World::new();
    world
        .restore_entities(&capture.entities)
        .expect("captured live set is valid");
    for (index, &entity) in capture.entities.iter().enumerate() {
        world
            .insert(entity, capture.colliders[index].clone())
            .expect("a restored entity accepts a collider");
        world
            .insert(entity, capture.poses[index])
            .expect("a restored entity accepts a pose");
        world
            .insert(entity, capture.velocities[index])
            .expect("a restored entity accepts a velocity");
        if let Some(joint) = &capture.joints[index] {
            world
                .insert(entity, joint.clone())
                .expect("a restored entity accepts a joint");
        }
    }
    world.insert_resource(
        PhysicsWorld::from_bytes(&capture.bytes, TickRate::DEFAULT)
            .expect("the bytes this run just wrote decode"),
    );
    world
}

#[test]
fn the_layered_scene_traces_identically_twice() {
    assert_eq!(run(false), run(false));
}

#[test]
fn declaration_order_does_not_move_the_trace() {
    assert_eq!(run(false), run(true));
}

#[test]
fn the_layered_scene_matches_the_committed_trace() {
    let trace = run(false);
    let path = reference_path();
    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick digest of every body's mirrored pose, in handle-table order (tests/layers.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-physics --test layers\n\
# A layered pile with a jointed figure on a self-ignoring layer; the kicks keep it live, so the\n\
# distinct-line count is the second half of the gate.\n";
        std::fs::write(&path, format!("{header}{}\n", trace.join("\n")))
            .expect("the trace directory is committed");
        return;
    }
    let committed: Vec<String> = std::fs::read_to_string(&path)
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect();
    assert_eq!(committed.len(), TICKS as usize);
    for (frame, (actual, expected)) in trace.iter().zip(&committed).enumerate() {
        assert_eq!(actual, expected, "the trace diverges at tick {frame}");
    }
}

#[test]
fn the_layered_trace_is_varied() {
    let trace = run(false);
    let mut distinct: Vec<&String> = trace.iter().collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(
        distinct.len() > 250,
        "only {} distinct lines of {TICKS}",
        distinct.len()
    );
}

#[test]
fn a_layered_run_resumed_from_a_snapshot_continues_byte_identically() {
    let reference = run(false);

    let mut world = World::new();
    build(&mut world);
    let mut schedule = trace_schedule(false);
    for frame in 0..SNAPSHOT_TICK {
        tick(&mut world, &mut schedule, frame);
    }
    let capture = capture(&world);
    drop(world);
    drop(schedule);

    let mut world = restore(&capture);
    let mut schedule = trace_schedule(false);
    for frame in SNAPSHOT_TICK..TICKS {
        tick(&mut world, &mut schedule, frame);
        assert_eq!(
            trace_line(&world, frame),
            reference[frame as usize],
            "the resumed run diverges at tick {frame}"
        );
    }
}
