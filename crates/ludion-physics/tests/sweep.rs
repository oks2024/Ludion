//! `first_hit` / `first_hits`: parity with the all-hits queries, extreme speeds, ties, normals, the
//! filter, and thread-count invariance.

#[allow(dead_code)] // shared helpers; each test binary uses a subset
mod common;

use common::{TestPose, tick};
use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::par;
use ludion_core::time::TickRate;
use ludion_ecs::{Schedule, World};
use ludion_physics::{
    CollisionLayers, NoParent, PhysicsError, PhysicsSettings, PhysicsSpace, PhysicsWorld,
    QueryFilter, Sweep, SweepHit, install,
};
use ludion_spatial::{Body, Collider, Shape};

struct BoxSpec {
    at: [f32; 3],
    half: [f32; 3],
    rotation: [f32; 4],
    layer: Option<&'static str>,
    sensor: bool,
}

impl BoxSpec {
    fn at(at: [f32; 3]) -> Self {
        Self {
            at,
            half: [0.5; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            layer: None,
            sensor: false,
        }
    }
}

fn spawn_box(world: &mut World, spec: BoxSpec) -> EntityId {
    let entity = world.spawn();
    world
        .insert(
            entity,
            TestPose {
                translation: spec.at,
                rotation: spec.rotation,
            },
        )
        .expect("a fresh entity accepts a pose");
    world
        .insert(
            entity,
            Collider {
                shape: Shape::Box {
                    half_extents: spec.half,
                },
                body: Body::new_static(),
                layer: spec.layer.map(str::to_string),
                sensor: spec.sensor,
            },
        )
        .expect("a fresh entity accepts a collider");
    entity
}

fn step_free(world: &mut World) {
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(&mut schedule, PhysicsSpace::Free);
    tick(world, &mut schedule, 0);
}

fn physics(world: &World) -> &PhysicsWorld {
    world
        .resource::<PhysicsWorld>()
        .expect("the write system installs the world once a body claims it")
}

fn hit(world: &World, sweep: &Sweep) -> Option<SweepHit> {
    let filter = physics(world)
        .sweep_filter(&QueryFilter::new())
        .expect("an empty filter");
    physics(world).first_hit(sweep, &filter)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn row_world() -> (World, Vec<EntityId>) {
    let mut world = World::new();
    let mut ids = Vec::new();
    // Spawned far-first so entity order is not distance order.
    for x in [8.0, 6.0, 4.0, 2.0] {
        ids.push(spawn_box(&mut world, BoxSpec::at([x, 0.0, 0.0])));
    }
    ids.push(spawn_box(
        &mut world,
        BoxSpec {
            rotation: euler_xyz_degrees_to_quat([0.0, 30.0, 45.0]),
            ..BoxSpec::at([0.0, 5.0, 0.0])
        },
    ));
    step_free(&mut world);
    (world, ids)
}

#[test]
fn first_hit_matches_shape_cast_first_hit() {
    let (world, _) = row_world();
    let p = physics(&world);
    let none = QueryFilter::new();
    let probes: [([f32; 3], [f32; 3]); 4] = [
        ([0.0, 0.0, 0.0], [20.0, 0.0, 0.0]),
        ([0.0, 0.2, 0.1], [20.0, 0.0, 0.0]),
        ([0.2, 10.0, 0.0], [0.0, -20.0, 0.0]),
        ([-3.0, 4.0, 0.3], [6.0, 1.0, 0.0]),
    ];
    for (origin, delta) in probes {
        let length = (dot(delta, delta)).sqrt();
        let ray_hits = p.ray_cast(origin, delta, length, &none).unwrap();
        let got = hit(&world, &Sweep::new(origin, delta, 0.0));
        match ray_hits.first() {
            Some(expected) => {
                let got = got.expect("the ray hits");
                assert_eq!(got.entity, expected.entity);
                assert_eq!(got.point, expected.point);
                assert_eq!(got.normal, expected.normal);
                assert_eq!(got.distance, expected.distance);
            }
            None => assert!(got.is_none(), "{got:?}"),
        }

        let sphere = Shape::Sphere { radius: 0.25 };
        let cast_hits = p
            .shape_cast(&sphere, origin, [0.0, 0.0, 0.0, 1.0], delta, length, &none)
            .unwrap();
        let got = hit(&world, &Sweep::new(origin, delta, 0.25));
        match cast_hits.first() {
            Some(expected) => {
                let got = got.expect("the sphere hits");
                assert_eq!(got.entity, expected.entity);
                assert_eq!(got.point, expected.point);
                assert_eq!(got.normal, expected.normal);
                assert_eq!(got.distance, expected.distance);
            }
            None => assert!(got.is_none(), "{got:?}"),
        }
    }
    // A miss is None where both existing queries are empty.
    let away = Sweep::new([0.0, 20.0, 0.0], [20.0, 0.0, 0.0], 0.25);
    assert!(hit(&world, &away).is_none());
    assert!(
        p.ray_cast(away.origin, [1.0, 0.0, 0.0], 20.0, &none)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_thin_wall_stops_an_extreme_speed_probe() {
    let mut world = World::new();
    let wall = spawn_box(
        &mut world,
        BoxSpec {
            half: [0.005, 50.0, 50.0],
            ..BoxSpec::at([10.0, 0.0, 0.0])
        },
    );
    step_free(&mut world);
    for radius in [0.0f32, 0.1] {
        let got = hit(&world, &Sweep::new([0.0; 3], [1e5, 0.0, 0.0], radius)).expect("hits");
        assert_eq!(got.entity, wall);
        let expected = 10.0 - 0.005 - radius;
        assert!(
            (got.distance - expected).abs() < 1e-3,
            "radius {radius}: {} vs {expected}",
            got.distance
        );
    }
    let past = Sweep::new([10.5, 0.0, 0.0], [1000.0, 0.0, 0.0], 0.1);
    assert!(hit(&world, &past).is_none());
}

#[test]
fn ties_go_to_the_lowest_entity() {
    let mut world = World::new();
    let first = spawn_box(&mut world, BoxSpec::at([5.0, 0.0, 0.0]));
    let second = spawn_box(&mut world, BoxSpec::at([5.0, 0.0, 0.0]));
    step_free(&mut world);
    let (low, high) = if first < second {
        (first, second)
    } else {
        (second, first)
    };
    for radius in [0.0, 0.25] {
        let probe = Sweep::new([0.0; 3], [10.0, 0.0, 0.0], radius);
        assert_eq!(hit(&world, &probe).unwrap().entity, low);
        assert_eq!(hit(&world, &probe.excluding(low)).unwrap().entity, high);
    }
}

#[test]
fn corner_and_edge_normals_are_unit_and_outward() {
    let mut world = World::new();
    spawn_box(&mut world, BoxSpec::at([0.0, 0.0, 0.0]));
    step_free(&mut world);
    let probes = [
        // A sphere into the +x+y+z corner.
        Sweep::new([3.0, 3.0, 3.0], [-6.0, -6.0, -6.0], 0.25),
        // A ray exactly onto the +x+y edge.
        Sweep::new([3.0, 3.0, 0.0], [-6.0, -6.0, 0.0], 0.0),
    ];
    for probe in probes {
        let first = hit(&world, &probe).expect("hits the box");
        let again = hit(&world, &probe).expect("hits the box");
        let n = first.normal;
        assert!(n.iter().all(|c| c.is_finite()), "{n:?}");
        assert!((dot(n, n).sqrt() - 1.0).abs() <= 1e-5, "{n:?}");
        assert!(dot(n, probe.delta) < 0.0, "{n:?}");
        assert_eq!(n.map(f32::to_bits), again.normal.map(f32::to_bits));
    }
}

#[test]
fn a_probe_starting_inside_reports_reversed_travel() {
    let mut world = World::new();
    let target = spawn_box(&mut world, BoxSpec::at([0.0, 0.0, 0.0]));
    step_free(&mut world);
    for radius in [0.0, 0.25] {
        let got = hit(
            &world,
            &Sweep::new([0.1, 0.0, 0.0], [0.0, 4.0, 0.0], radius),
        )
        .unwrap();
        assert_eq!(got.entity, target);
        assert_eq!(got.t, 0.0);
        assert_eq!(got.distance, 0.0);
        assert_eq!(got.normal, [0.0, -1.0, 0.0]);
    }
}

fn layered_row() -> (World, [EntityId; 4]) {
    let mut world = World::new();
    let specs = [
        (Some("props"), false),
        (Some("dummy"), false),
        (None, true),
        (Some("dummy"), false),
    ];
    let mut entities = [EntityId::new(0, 0); 4];
    for (i, (layer, sensor)) in specs.into_iter().enumerate() {
        entities[i] = spawn_box(
            &mut world,
            BoxSpec {
                layer,
                sensor,
                ..BoxSpec::at([2.0 + 2.0 * i as f32, 0.0, 0.0])
            },
        );
    }
    let mut schedule = Schedule::new();
    install::<TestPose, NoParent>(
        &mut schedule,
        PhysicsSettings {
            layers: Some(CollisionLayers::new(
                vec!["default".into(), "dummy".into(), "props".into()],
                vec![0, 0, 0],
            )),
            ..PhysicsSettings::new(PhysicsSpace::Free)
        },
    );
    tick(&mut world, &mut schedule, 0);
    (world, entities)
}

#[test]
fn the_filter_drops_layers_sensors_and_excluded() {
    let (world, [props, dummy_a, sensor, dummy_b]) = layered_row();
    let p = physics(&world);
    let probe = Sweep::new([0.0; 3], [20.0, 0.0, 0.0], 0.0);
    let first = |filter: QueryFilter, probe: &Sweep| {
        let f = p.sweep_filter(&filter).unwrap();
        p.first_hit(probe, &f).map(|h| h.entity)
    };
    assert_eq!(first(QueryFilter::new(), &probe), Some(props));
    assert_eq!(
        first(QueryFilter::new().ignore_layer("props"), &probe),
        Some(dummy_a)
    );
    assert_eq!(
        first(
            QueryFilter::new()
                .ignore_layer("props")
                .ignore_layer("dummy"),
            &probe
        ),
        None,
        "the sensor is dropped by default"
    );
    assert_eq!(
        first(
            QueryFilter::new()
                .ignore_layer("props")
                .ignore_layer("dummy")
                .include_sensors(),
            &probe
        ),
        Some(sensor)
    );
    assert_eq!(
        first(QueryFilter::new().exclude(props), &probe),
        Some(dummy_a)
    );
    assert_eq!(
        first(QueryFilter::new(), &probe.excluding(props)),
        Some(dummy_a)
    );
    assert_eq!(
        first(QueryFilter::new().ignore_layer("default"), &probe),
        Some(props)
    );
    let err = p
        .sweep_filter(&QueryFilter::new().ignore_layer("dumy"))
        .unwrap_err();
    assert!(matches!(err, PhysicsError::QueryInput(_)), "{err}");
    let _ = (dummy_b, TickRate::DEFAULT);
}

/// An integer LCG, so no `rand` and the same probes everywhere.
struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

fn bits(hit: &Option<SweepHit>) -> Option<(EntityId, [u32; 9])> {
    hit.as_ref().map(|h| {
        (
            h.entity,
            [
                h.t.to_bits(),
                h.distance.to_bits(),
                h.point[0].to_bits(),
                h.point[1].to_bits(),
                h.point[2].to_bits(),
                h.normal[0].to_bits(),
                h.normal[1].to_bits(),
                h.normal[2].to_bits(),
                0,
            ],
        )
    })
}

#[test]
fn first_hits_is_thread_count_invariant() {
    let mut world = World::new();
    for gx in 0..8 {
        for gz in 0..5 {
            spawn_box(
                &mut world,
                BoxSpec::at([gx as f32 * 3.0, 0.0, gz as f32 * 3.0]),
            );
        }
    }
    step_free(&mut world);
    let p = physics(&world);
    let filter = p.sweep_filter(&QueryFilter::new()).unwrap();
    let mut rng = Lcg(7);
    let probes: Vec<Sweep> = (0..2000)
        .map(|_| {
            Sweep::new(
                [
                    rng.range(-5.0, 30.0),
                    rng.range(-2.0, 2.0),
                    rng.range(-5.0, 18.0),
                ],
                [
                    rng.range(-12.0, 12.0),
                    rng.range(-1.0, 1.0),
                    rng.range(-12.0, 12.0),
                ],
                if rng.unit() < 0.5 { 0.0 } else { 0.2 },
            )
        })
        .collect();
    let singles: Vec<_> = probes.iter().map(|s| p.first_hit(s, &filter)).collect();
    assert!(singles.iter().any(Option::is_some) && singles.iter().any(Option::is_none));
    for threads in [1, 3, 8] {
        let mut out = vec![None; probes.len()];
        par::with_threads(threads, || p.first_hits(&probes, &filter, &mut out));
        for (i, (a, b)) in out.iter().zip(&singles).enumerate() {
            assert_eq!(bits(a), bits(b), "probe {i} at {threads} threads");
        }
    }
}

#[test]
#[should_panic(expected = "2 sweeps but 1 output slots")]
fn first_hits_panics_on_mismatched_lengths() {
    let (world, _) = row_world();
    let p = physics(&world);
    let filter = p.sweep_filter(&QueryFilter::new()).unwrap();
    let probes = [Sweep::new([0.0; 3], [1.0, 0.0, 0.0], 0.0); 2];
    p.first_hits(&probes, &filter, &mut [None; 1]);
}

#[test]
fn invalid_probes_hit_nothing() {
    let (world, _) = row_world();
    for probe in [
        Sweep::new([f32::NAN, 0.0, 0.0], [10.0, 0.0, 0.0], 0.0),
        Sweep::new([0.0; 3], [10.0, 0.0, 0.0], -0.5),
        Sweep::new([0.0; 3], [0.0; 3], 0.0),
        Sweep::new([0.0; 3], [f32::INFINITY, 0.0, 0.0], 0.0),
        Sweep::new([0.0; 3], [10.0, 0.0, 0.0], f32::NAN),
    ] {
        assert!(hit(&world, &probe).is_none(), "{probe:?}");
    }
}
