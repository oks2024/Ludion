//! `QueryLayers` and the include clause: query-only membership reaches every query and blast but
//! never the solver, an unknown name degrades to mask 0 with one `L1186`, and a restore answers an
//! include query before any write.

mod common;

use common::{TestPose, pose_of, tick};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    Blast, CollisionLayers, ContactEvent, Falloff, LayerSet, NoParent, PhysicsError,
    PhysicsSettings, PhysicsSpace, PhysicsWorld, QueryFilter, QueryLayers, ShapeRejection, Sweep,
    install,
};
use ludion_spatial::{Body, Collider, RigidBody, Shape, Velocity};

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn table() -> CollisionLayers {
    let names = [
        "default",
        "cover_low",
        "cover_high",
        "light_occluder",
        "wall",
        "tag",
    ];
    CollisionLayers::new(
        names.iter().map(|n| n.to_string()).collect(),
        vec![0; names.len()],
    )
}

fn settings() -> PhysicsSettings {
    PhysicsSettings {
        layers: Some(table()),
        ..PhysicsSettings::new(PhysicsSpace::Free)
    }
}

fn set(names: &[&str]) -> LayerSet {
    LayerSet::from(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
}

fn query_layers(names: &[&str]) -> QueryLayers {
    QueryLayers { layers: set(names) }
}

fn spawn(
    world: &mut World,
    at: [f32; 3],
    body: Body,
    layer: &str,
    extra: Option<QueryLayers>,
) -> EntityId {
    let entity = world.spawn();
    world.insert(entity, TestPose::at(at)).unwrap();
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Box {
                    half_extents: [0.5; 3],
                },
                body,
                layer: Some(layer.to_string()),
                sensor: false,
            },
        )
        .unwrap();
    world.insert(entity, Velocity::default()).unwrap();
    if let Some(extra) = extra {
        world.insert(entity, extra).unwrap();
    }
    entity
}

struct Sim {
    world: World,
    schedule: Schedule,
    frame: u64,
    rejections: Vec<ShapeRejection>,
    contacts: Vec<ContactEvent>,
}

impl Sim {
    fn new() -> Self {
        let mut schedule = Schedule::new();
        install::<TestPose, NoParent>(&mut schedule, settings());
        Sim {
            world: World::new(),
            schedule,
            frame: 0,
            rejections: Vec::new(),
            contacts: Vec::new(),
        }
    }

    fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            tick(&mut self.world, &mut self.schedule, self.frame);
            self.frame += 1;
            let physics = self.world.resource_mut::<PhysicsWorld>().unwrap();
            self.rejections.extend(physics.take_rejections());
            self.contacts.extend_from_slice(physics.contact_events());
        }
    }

    fn physics(&self) -> &PhysicsWorld {
        self.world.resource::<PhysicsWorld>().unwrap()
    }
}

/// Three statics on the +x ray from `x = -5`: `cover_low`, `cover_high` (also `light_occluder`),
/// `wall`.
fn cover_row() -> (Sim, [EntityId; 3]) {
    let mut sim = Sim::new();
    let low = spawn(
        &mut sim.world,
        [0.0, 0.0, 0.0],
        Body::new_static(),
        "cover_low",
        None,
    );
    let high = spawn(
        &mut sim.world,
        [3.0, 0.0, 0.0],
        Body::new_static(),
        "cover_high",
        Some(query_layers(&["light_occluder"])),
    );
    let wall = spawn(
        &mut sim.world,
        [6.0, 0.0, 0.0],
        Body::new_static(),
        "wall",
        None,
    );
    sim.run(2);
    (sim, [low, high, wall])
}

/// Every query kind over the row with `filter`, as the entities each returned.
fn answers(physics: &PhysicsWorld, filter: &QueryFilter) -> [Vec<EntityId>; 4] {
    let ray: Vec<EntityId> = physics
        .ray_cast([-5.0, 0.0, 0.0], [1.0, 0.0, 0.0], 20.0, filter)
        .unwrap()
        .iter()
        .map(|h| h.entity)
        .collect();
    let cast: Vec<EntityId> = physics
        .shape_cast(
            &Shape::Sphere { radius: 0.1 },
            [-5.0, 0.0, 0.0],
            IDENTITY,
            [1.0, 0.0, 0.0],
            20.0,
            filter,
        )
        .unwrap()
        .iter()
        .map(|h| h.entity)
        .collect();
    let overlap = physics
        .overlap(
            &Shape::Box {
                half_extents: [10.0, 1.0, 1.0],
            },
            [3.0, 0.0, 0.0],
            IDENTITY,
            filter,
        )
        .unwrap();
    let resolved = physics.sweep_filter(filter).unwrap();
    let first: Vec<EntityId> = physics
        .first_hit(
            &Sweep::new([-5.0, 0.0, 0.0], [20.0, 0.0, 0.0], 0.0),
            &resolved,
        )
        .map(|h| h.entity)
        .into_iter()
        .collect();
    [ray, cast, overlap, first]
}

fn check(physics: &PhysicsWorld, filter: &QueryFilter, all: &[EntityId], first: &[EntityId]) {
    let [ray, cast, overlap, nearest] = answers(physics, filter);
    assert_eq!(ray, all, "ray_cast");
    assert_eq!(cast, all, "shape_cast");
    assert_eq!(overlap, all, "overlap");
    assert_eq!(nearest, first, "first_hit");
}

#[test]
fn include_keeps_only_the_named_layers_for_every_query() {
    let (sim, [low, high, wall]) = cover_row();
    let p = sim.physics();
    check(p, &QueryFilter::new(), &[low, high, wall], &[low]);
    check(
        p,
        &QueryFilter::new().include_layer("cover_high"),
        &[high],
        &[high],
    );
    check(
        p,
        &QueryFilter::new().include_layer("light_occluder"),
        &[high],
        &[high],
    );
    check(
        p,
        &QueryFilter::new()
            .include_layer("cover_low")
            .include_layer("wall"),
        &[low, wall],
        &[low],
    );
    check(
        p,
        &QueryFilter::new().include_set(&set(&["cover_low", "wall"])),
        &[low, wall],
        &[low],
    );
}

#[test]
fn ignore_wins_over_include_and_sees_query_only_layers() {
    let (sim, [low, _high, wall]) = cover_row();
    let p = sim.physics();
    check(
        p,
        &QueryFilter::new()
            .include_layer("light_occluder")
            .ignore_layer("light_occluder"),
        &[],
        &[],
    );
    check(
        p,
        &QueryFilter::new().ignore_layer("light_occluder"),
        &[low, wall],
        &[low],
    );
    check(
        p,
        &QueryFilter::new().ignore_set(&set(&["cover_low", "light_occluder"])),
        &[wall],
        &[wall],
    );
    // The body layer still counts beside the query-only one.
    check(
        p,
        &QueryFilter::new()
            .include_layer("cover_high")
            .ignore_layer("light_occluder"),
        &[],
        &[],
    );
}

#[test]
fn an_undeclared_include_name_is_a_query_input_error() {
    let (sim, _) = cover_row();
    let err = sim
        .physics()
        .ray_cast(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            5.0,
            &QueryFilter::new().include_layer("wal"),
        )
        .unwrap_err();
    let PhysicsError::QueryInput(reason) = err else {
        panic!("expected QueryInput");
    };
    assert!(
        reason.contains("`wal`") && reason.contains("wall"),
        "{reason}"
    );
}

#[test]
fn first_hits_with_an_include_mask_is_thread_count_invariant() {
    let mut sim = Sim::new();
    for gx in 0..8 {
        for gz in 0..5 {
            let extra = (gx + gz) % 2 == 0;
            spawn(
                &mut sim.world,
                [gx as f32 * 3.0, 0.0, gz as f32 * 3.0],
                Body::new_static(),
                "cover_low",
                extra.then(|| query_layers(&["tag"])),
            );
        }
    }
    sim.run(2);
    let p = sim.physics();
    let filter = p
        .sweep_filter(&QueryFilter::new().include_layer("tag"))
        .unwrap();
    let probes: Vec<Sweep> = (0..400)
        .map(|i| {
            let f = i as f32;
            Sweep::new(
                [(f * 0.37) % 24.0 - 2.0, 0.0, (f * 0.53) % 12.0 - 2.0],
                [((f * 1.7) % 20.0) - 10.0, 0.0, ((f * 2.3) % 20.0) - 10.0],
                0.0,
            )
        })
        .collect();
    let bits = |h: &Option<ludion_physics::SweepHit>| h.map(|h| (h.entity, h.distance.to_bits()));
    let singles: Vec<_> = probes
        .iter()
        .map(|s| bits(&p.first_hit(s, &filter)))
        .collect();
    assert!(singles.iter().any(Option::is_some));
    for threads in [1, 2, 8] {
        let mut out = vec![None; probes.len()];
        par::with_threads(threads, || p.first_hits(&probes, &filter, &mut out));
        let got: Vec<_> = out.iter().map(bits).collect();
        assert_eq!(got, singles, "{threads} threads");
    }
}

#[test]
fn an_undeclared_query_layer_keeps_the_body_and_reports_once_until_fixed() {
    let mut sim = Sim::new();
    let target = spawn(
        &mut sim.world,
        [0.0, 0.0, 0.0],
        Body::new_static(),
        "wall",
        Some(query_layers(&["light_occlder"])),
    );
    sim.run(3);
    assert_eq!(sim.rejections.len(), 1, "one L1186 for a standing fault");
    assert_eq!(sim.rejections[0].code, code::COLLIDER_LAYER_UNKNOWN);
    assert_eq!(sim.rejections[0].entity, target);
    assert!(sim.rejections[0].reason.contains("light_occluder"));
    assert_eq!(sim.physics().body_count(), 1, "the body is still built");
    let ray = |sim: &Sim, include: &str| {
        sim.physics()
            .ray_cast(
                [-5.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                20.0,
                &QueryFilter::new().include_layer(include),
            )
            .unwrap()
            .len()
    };
    assert_eq!(ray(&sim, "wall"), 1, "its body layer still answers");
    assert_eq!(ray(&sim, "tag"), 0);

    *sim.world.get_mut::<QueryLayers>(target).unwrap().unwrap() = query_layers(&["tag"]);
    sim.run(1);
    assert_eq!(sim.rejections.len(), 1, "the fix reports nothing");
    assert_eq!(ray(&sim, "tag"), 1);
}

/// A 24-box pile on a floor, half of it tagged when `tagged`; returns the poses, contacts and blob.
fn pile(tagged: bool, edit_at: Option<u64>) -> (Vec<[u32; 7]>, Vec<ContactEvent>, Vec<u8>) {
    let mut sim = Sim::new();
    spawn(
        &mut sim.world,
        [0.0, -0.5, 0.0],
        Body::new_static(),
        "default",
        None,
    );
    let mut boxes = Vec::new();
    for i in 0..24 {
        let at = [
            (i % 3) as f32 * 1.1 - 1.0,
            0.6 + (i / 9) as f32 * 1.05 + (i % 2) as f32 * 0.1,
            ((i / 3) % 3) as f32 * 1.1 - 1.0,
        ];
        let extra = (tagged && i % 2 == 0).then(|| query_layers(&["tag"]));
        boxes.push(spawn(
            &mut sim.world,
            at,
            Body::Rigid(RigidBody::default()),
            "default",
            extra,
        ));
    }
    for t in 0..300 {
        if Some(t) == edit_at {
            for (i, entity) in boxes.iter().enumerate() {
                if i % 3 == 0 {
                    sim.world
                        .insert(*entity, query_layers(&["cover_low", "tag"]))
                        .unwrap();
                }
            }
        }
        sim.run(1);
    }
    let poses = boxes
        .iter()
        .map(|e| {
            let p = pose_of(&sim.world, *e);
            let [x, y, z] = p.translation.map(f32::to_bits);
            let [qx, qy, qz, qw] = p.rotation.map(f32::to_bits);
            [x, y, z, qx, qy, qz, qw]
        })
        .collect();
    let blob = sim.physics().to_bytes().unwrap();
    (poses, sim.contacts, blob)
}

#[test]
fn query_layers_never_change_the_simulation() {
    let plain = pile(false, None);
    assert!(!plain.1.is_empty(), "the pile must touch");
    let tagged = pile(true, None);
    assert_eq!(plain.0, tagged.0, "poses");
    assert_eq!(plain.1, tagged.1, "contact events");
    assert_eq!(plain.2, tagged.2, "physics blob");
    let edited = pile(true, Some(150));
    assert_eq!(plain.0, edited.0, "poses after a mid-run edit");
    assert_eq!(plain.1, edited.1, "contacts after a mid-run edit");
    assert_eq!(plain.2, edited.2, "blob after a mid-run edit");
}

#[test]
fn a_restore_answers_an_include_query_before_any_write() {
    let (sim, [_, high, _]) = cover_row();
    let blob = sim.physics().to_bytes().unwrap();
    let mut restored = PhysicsWorld::from_bytes(&blob, TickRate::DEFAULT).unwrap();
    restored.sync_query_layers(&sim.world);
    restored.apply_settings(&settings());
    let hits = restored
        .ray_cast(
            [-5.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            20.0,
            &QueryFilter::new().include_layer("light_occluder"),
        )
        .unwrap();
    assert_eq!(hits.iter().map(|h| h.entity).collect::<Vec<_>>(), [high]);
    assert!(restored.take_rejections().is_empty());
}

#[test]
fn a_blast_filters_by_include_and_query_only_layers() {
    let mut sim = Sim::new();
    let plain = spawn(
        &mut sim.world,
        [1.0, 0.0, 0.0],
        Body::Rigid(RigidBody::default()),
        "default",
        None,
    );
    let tagged = spawn(
        &mut sim.world,
        [-1.0, 0.0, 0.0],
        Body::Rigid(RigidBody::default()),
        "default",
        Some(query_layers(&["tag"])),
    );
    sim.run(1);
    let physics = sim.world.resource_mut::<PhysicsWorld>().unwrap();
    physics
        .radial_impulse(Blast {
            center: [0.0; 3],
            radius: 5.0,
            impulse: 10.0,
            falloff: Falloff::Constant,
            filter: QueryFilter::new().include_layer("tag"),
        })
        .unwrap();
    sim.run(1);
    let hits: Vec<EntityId> = sim
        .physics()
        .blast_hits()
        .iter()
        .map(|h| h.entity)
        .collect();
    assert_eq!(hits, [tagged]);
    assert_ne!(tagged, plain);
}

/// The motor emulates the solver's matrix, so a wall's query-only `actors` tag must not let an
/// `actors`-ignoring motor walk through it.
#[test]
fn a_motor_still_collides_with_a_wall_tagged_with_its_ignored_layer() {
    use std::sync::Arc;

    use ludion_ecs::SystemCtx;
    use ludion_physics::{CharacterMotor, move_character};

    let mut world = World::new();
    let floor = spawn(
        &mut world,
        [0.0, -0.5, 0.0],
        Body::new_static(),
        "default",
        None,
    );
    world.get_mut::<Collider>(floor).unwrap().unwrap().shape = Shape::Box {
        half_extents: [30.0, 0.5, 30.0],
    };
    let wall = spawn(
        &mut world,
        [5.0, 1.0, 0.0],
        Body::new_static(),
        "default",
        Some(query_layers(&["actors"])),
    );
    world.get_mut::<Collider>(wall).unwrap().unwrap().shape = Shape::Box {
        half_extents: [0.5, 1.0, 3.0],
    };
    let hero = world.spawn();
    world.insert(hero, TestPose::at([2.0, 0.0, 0.0])).unwrap();
    world
        .insert(
            hero,
            CharacterMotor {
                radius: 0.28,
                height: 0.9,
                slope_limit_deg: 45.0,
                step_height: 0.25,
                skin_width: 0.01,
                layer: Some("actors".to_string()),
                ..CharacterMotor::default()
            },
        )
        .unwrap();

    let mut schedule = Schedule::new();
    schedule.add_system_shared(
        "test-script",
        Arc::new(move |world: &mut World, _: &SystemCtx| {
            let _ = move_character::<TestPose, NoParent>(world, hero, [0.1, -0.1, 0.0]);
        }),
    );
    let layers = CollisionLayers::new(
        vec!["default".to_string(), "actors".to_string()],
        vec![0, 0b10],
    );
    install::<TestPose, NoParent>(
        &mut schedule,
        PhysicsSettings {
            layers: Some(layers),
            ..PhysicsSettings::new(PhysicsSpace::Free)
        },
    );
    for frame in 0..60 {
        tick(&mut world, &mut schedule, frame);
    }
    let x = pose_of(&world, hero).translation[0];
    assert!(x > 3.0, "the motor must walk toward the wall, x = {x}");
    assert!(
        x < 4.5,
        "the wall face is at 4.5; the motor reached x = {x}"
    );
}
