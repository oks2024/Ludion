//! `light_exposure`: the sim-side answer to "how much authored light reaches these points", with
//! physics-ray occlusion, determinism across snapshot/restore and thread counts.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::Vec3;
use ludion_core::par;
use ludion_ecs::{Parent, World};
use ludion_engine::{
    Combine, DirectionalLight, Engine, Environment, Exposure, ExposureError, ExposureQuery, Facing,
    MAX_EXPOSURE_POINTS, PointLight, SpotLight, Transform, Visibility, light_exposure,
};
use ludion_physics::QueryFilter;
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use ludion_renderer::DEFAULT_AMBIENT;
use ludion_renderer::light_model::{sky_irradiance, sky_sphere_mean};

const UP: Facing = Facing::Normal([0.0, 1.0, 0.0]);

/// A scene of static boxes: `(centre, half extents)`.
fn engine(walls: &[([f32; 3], [f32; 3])]) -> Engine {
    let seq = |v: [f32; 3], indent: usize| -> String {
        v.iter()
            .map(|c| format!("{:indent$}- {c:?}\n", ""))
            .collect()
    };
    let mut doc = String::from("format: 1\nseed: 1\nphysics:\n  space: free\nentities:\n");
    if walls.is_empty() {
        doc = doc.replace("entities:\n", "entities: []\n");
    }
    for (n, (at, half)) in walls.iter().enumerate() {
        doc +=
            &format!("  - name: w{n}\n    components:\n      Transform:\n        translation:\n");
        doc += &seq(*at, 10);
        doc += "      Collider:\n        shape:\n          box:\n            half_extents:\n";
        doc += &seq(*half, 14);
        doc += "        body: static\n";
    }
    let mut engine = Engine::builder().build().expect("boots");
    engine.load_inline_scene(&doc).unwrap();
    engine.advance_live();
    engine
}

fn spawn<T: ludion_ecs::Component>(engine: &mut Engine, at: [f32; 3], light: T) -> EntityId {
    let world = engine.world_mut();
    let id = world.spawn();
    world
        .insert(
            id,
            Transform {
                translation: at,
                ..Transform::default()
            },
        )
        .unwrap();
    world.insert(id, light).unwrap();
    id
}

fn point(range: f32) -> PointLight {
    PointLight {
        range,
        ..PointLight::default()
    }
}

fn spot(inner: f32, outer: f32, range: f32) -> SpotLight {
    SpotLight {
        direction: [0.0, -1.0, 0.0],
        inner_angle: inner,
        outer_angle: outer,
        range,
        ..SpotLight::default()
    }
}

fn read(
    world: &World,
    points: &[[f32; 3]],
    facing: Facing,
    filter: &QueryFilter,
    combine: Combine,
) -> Exposure {
    light_exposure(
        world,
        &ExposureQuery {
            points,
            facing,
            occluders: filter,
            combine,
        },
    )
    .unwrap()
}

fn at(engine: &Engine, p: [f32; 3], facing: Facing) -> Exposure {
    read(
        engine.world(),
        &[p],
        facing,
        &QueryFilter::new(),
        Combine::Max,
    )
}

fn amb() -> Vec3 {
    Vec3::from(DEFAULT_AMBIENT)
}

#[test]
fn a_point_at_the_range_gets_exact_zero_and_no_entry() {
    let mut e = engine(&[]);
    spawn(&mut e, [0.0, 3.0, 0.0], point(3.0));
    let x = at(&e, [0.0; 3], UP);
    assert_eq!(x.rgb, DEFAULT_AMBIENT);
    assert!(x.lights.is_empty());
}

#[test]
fn half_range_below_a_lamp_is_a_quarter() {
    let mut e = engine(&[]);
    let id = spawn(&mut e, [0.0, 2.0, 0.0], point(4.0));
    let x = at(&e, [0.0; 3], UP);
    assert_eq!(x.rgb, (amb() + Vec3::splat(0.25)).to_array());
    assert_eq!(x.lights.len(), 1);
    assert_eq!(x.lights[0].light, id);
}

#[test]
fn a_hard_edged_spot_is_one_on_axis_and_zero_outside() {
    let mut e = engine(&[]);
    spawn(&mut e, [0.0, 5.0, 0.0], spot(30.0, 30.0, 20.0));
    let inside = at(&e, [2.0, 0.0, 0.0], UP);
    assert_eq!(inside.lights.len(), 1);
    let outside = at(&e, [3.0, 0.0, 0.0], UP);
    assert_eq!(outside.rgb, DEFAULT_AMBIENT);
    assert!(outside.lights.is_empty());
}

#[test]
fn only_the_lowest_sun_counts() {
    let mut e = engine(&[]);
    let sun = |color| DirectionalLight {
        color,
        ..DirectionalLight::default()
    };
    let first = spawn(&mut e, [0.0; 3], sun([1.0, 0.0, 0.0]));
    spawn(&mut e, [0.0; 3], sun([0.0, 0.0, 1.0]));
    let x = at(&e, [0.0; 3], Facing::Omni);
    assert_eq!(x.lights.len(), 1);
    assert_eq!(x.lights[0].light, first);
    assert_eq!(x.rgb, (amb() + Vec3::new(1.0, 0.0, 0.0)).to_array());
}

#[test]
fn hidden_lights_and_zero_intensity_add_nothing_but_a_hidden_sun_counts() {
    let mut e = engine(&[]);
    let a = spawn(&mut e, [0.0, 1.0, 0.0], point(4.0));
    e.world_mut()
        .insert(
            a,
            Visibility {
                visible: false,
                cast_shadows: true,
            },
        )
        .unwrap();
    let parent = e.world_mut().spawn();
    e.world_mut()
        .insert(
            parent,
            Visibility {
                visible: false,
                cast_shadows: true,
            },
        )
        .unwrap();
    let b = spawn(&mut e, [0.0, 1.0, 0.0], point(4.0));
    e.world_mut().insert(b, Parent(parent)).unwrap();
    spawn(
        &mut e,
        [0.0, 1.0, 0.0],
        PointLight {
            intensity: 0.0,
            ..point(4.0)
        },
    );
    let x = at(&e, [0.0; 3], UP);
    assert_eq!(x.rgb, DEFAULT_AMBIENT);
    assert!(x.lights.is_empty());

    let sun = spawn(&mut e, [0.0; 3], DirectionalLight::default());
    e.world_mut()
        .insert(
            sun,
            Visibility {
                visible: false,
                cast_shadows: true,
            },
        )
        .unwrap();
    assert_eq!(at(&e, [0.0; 3], UP).lights.len(), 1);
}

#[test]
fn omni_is_at_least_any_normal() {
    let mut e = engine(&[]);
    spawn(&mut e, [1.0, 2.0, 0.5], point(6.0));
    spawn(&mut e, [-2.0, 1.0, 1.0], spot(20.0, 60.0, 9.0));
    let omni = at(&e, [0.0; 3], Facing::Omni).total;
    for n in [
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        [1.0, 1.0, 1.0],
        [-1.0, 2.0, -0.5],
    ] {
        assert!(omni >= at(&e, [0.0; 3], Facing::Normal(n)).total, "{n:?}");
    }
}

#[test]
fn ambient_follows_the_environment() {
    let mut e = engine(&[]);
    assert_eq!(at(&e, [0.0; 3], UP).ambient, DEFAULT_AMBIENT);
    let env = Environment {
        ibl_intensity: 0.5,
        ..Environment::default()
    };
    spawn(&mut e, [0.0; 3], env);
    let (z, h, g) = (
        Vec3::from(env.sky_zenith),
        Vec3::from(env.sky_horizon),
        Vec3::from(env.sky_ground),
    );
    let want = Vec3::from(env.ambient) + 0.5 * sky_irradiance(Vec3::Y, z, h, g);
    assert_eq!(at(&e, [0.0; 3], UP).ambient, want.to_array());
    let want = Vec3::from(env.ambient) + 0.5 * sky_sphere_mean(z, h, g);
    assert_eq!(at(&e, [0.0; 3], Facing::Omni).ambient, want.to_array());
}

const WALL: ([f32; 3], [f32; 3]) = ([0.0, 2.5, 0.0], [3.0, 0.1, 3.0]);

fn casting_spot(e: &mut Engine, cast: bool) -> EntityId {
    spawn(
        e,
        [0.0, 5.0, 0.0],
        SpotLight {
            cast_shadows: cast,
            ..spot(30.0, 60.0, 20.0)
        },
    )
}

fn only_fixed() -> QueryFilter {
    QueryFilter::new().only_fixed()
}

#[test]
fn a_wall_occludes_a_casting_spot_only() {
    let mut e = engine(&[WALL]);
    let lamp = casting_spot(&mut e, true);
    let x = read(e.world(), &[[0.0; 3]], UP, &only_fixed(), Combine::Max);
    assert_eq!(x.lights.len(), 1);
    assert!(x.lights[0].occluded);
    assert_eq!(x.lights[0].luminance, 0.0);
    assert!(x.lights[0].unoccluded > 0.0);
    assert_eq!(x.rgb, DEFAULT_AMBIENT);

    e.world_mut()
        .get_mut::<SpotLight>(lamp)
        .unwrap()
        .unwrap()
        .cast_shadows = false;
    let x = read(e.world(), &[[0.0; 3]], UP, &only_fixed(), Combine::Max);
    assert!(!x.lights[0].occluded);
    assert_eq!(x.lights[0].luminance, x.lights[0].unoccluded);
}

#[test]
fn a_wall_that_casts_no_shadow_does_not_occlude() {
    for visibility in [
        Visibility {
            visible: true,
            cast_shadows: false,
        },
        Visibility {
            visible: false,
            cast_shadows: true,
        },
    ] {
        let mut e = engine(&[WALL]);
        casting_spot(&mut e, true);
        let wall = e.world().entities().next().unwrap();
        e.world_mut().insert(wall, visibility).unwrap();
        let x = read(e.world(), &[[0.0; 3]], UP, &only_fixed(), Combine::Max);
        assert!(!x.lights[0].occluded, "{visibility:?}");
    }
}

#[test]
fn the_subject_does_not_occlude_itself_once_excluded() {
    let mut e = engine(&[([0.0, 0.0, 0.0], [0.5, 0.5, 0.5])]);
    casting_spot(&mut e, true);
    let subject = e.world().entities().next().unwrap();
    let p = [[0.0, 0.2, 0.0]];
    let hit = read(e.world(), &p, UP, &QueryFilter::new(), Combine::Max);
    assert!(hit.lights[0].occluded);
    let free = read(
        e.world(),
        &p,
        UP,
        &QueryFilter::new().exclude(subject),
        Combine::Max,
    );
    assert!(!free.lights[0].occluded);
}

#[test]
fn the_sun_is_occluded_by_a_roof_and_a_bad_layer_is_a_query_error() {
    let mut e = engine(&[([0.0, 3.0, 0.0], [3.0, 0.1, 3.0])]);
    spawn(&mut e, [0.0; 3], DirectionalLight::default());
    let covered = read(e.world(), &[[0.0; 3]], UP, &only_fixed(), Combine::Max);
    assert!(covered.lights[0].occluded);
    let open = read(
        e.world(),
        &[[10.0, 0.0, 0.0]],
        UP,
        &only_fixed(),
        Combine::Max,
    );
    assert!(!open.lights[0].occluded);
    let bad = QueryFilter::new().include_layer("no_such_layer");
    let err = light_exposure(
        e.world(),
        &ExposureQuery {
            points: &[[0.0; 3]],
            facing: UP,
            occluders: &bad,
            combine: Combine::Max,
        },
    );
    assert!(matches!(err, Err(ExposureError::Query(_))));
}

#[test]
fn combine_max_picks_whole_and_mean_averages() {
    let mut e = engine(&[]);
    spawn(&mut e, [0.0, 2.0, 0.0], point(4.0));
    let (a, b) = ([0.0; 3], [3.5, 0.0, 0.0]);
    let f = QueryFilter::new();
    let one = |p| read(e.world(), &[p], UP, &f, Combine::Max);
    let max = read(e.world(), &[b, a], UP, &f, Combine::Max);
    assert_eq!(max, one(a));
    let mean = read(e.world(), &[a, b], UP, &f, Combine::Mean);
    let want = (Vec3::from(one(a).rgb) + Vec3::from(one(b).rgb)) / 2.0;
    assert!((Vec3::from(mean.rgb) - want).abs().max_element() < 1e-6);
}

#[test]
fn mean_is_occluded_only_when_every_reached_point_is_blocked() {
    let (a, b) = ([0.0; 3], [8.0, 0.0, 0.0]);
    for (half, both) in [(3.0, false), (6.0, true)] {
        let mut e = engine(&[([0.0, 2.5, 0.0], [half, 0.1, half])]);
        spawn(
            &mut e,
            [0.0, 5.0, 0.0],
            SpotLight {
                cast_shadows: true,
                ..spot(60.0, 80.0, 30.0)
            },
        );
        let x = read(e.world(), &[a, b], UP, &only_fixed(), Combine::Mean);
        assert_eq!(x.lights.len(), 1);
        assert_eq!(x.lights[0].occluded, both, "half {half}");
    }
}

#[test]
fn malformed_queries_are_l4250_with_their_paths() {
    let e = engine(&[]);
    let f = QueryFilter::new();
    let check = |points: &[[f32; 3]], facing: Facing, path: &str| {
        let q = ExposureQuery {
            points,
            facing,
            occluders: &f,
            combine: Combine::Max,
        };
        let Err(ExposureError::Invalid(d)) = light_exposure(e.world(), &q) else {
            panic!("expected L4250 for {path}");
        };
        assert_eq!(d.code, code::LIGHT_EXPOSURE_INVALID);
        assert_eq!(d.path.as_deref(), Some(path));
    };
    check(&[], Facing::Omni, "/points");
    check(
        &[[0.0; 3]; MAX_EXPOSURE_POINTS + 1],
        Facing::Omni,
        "/points",
    );
    check(&[[0.0; 3], [0.0, f32::NAN, 0.0]], Facing::Omni, "/points/1");
    check(&[[0.0; 3]], Facing::Normal([0.0; 3]), "/facing");
}

fn bits(x: &Exposure) -> Vec<u32> {
    let mut v: Vec<u32> = x
        .rgb
        .iter()
        .chain(&x.ambient)
        .map(|f| f.to_bits())
        .collect();
    v.push(x.total.to_bits());
    for l in &x.lights {
        v.extend([
            l.luminance.to_bits(),
            l.unoccluded.to_bits(),
            u32::from(l.occluded),
        ]);
    }
    v
}

fn sweep() -> Vec<Vec<u32>> {
    let mut e = engine(&[([3.0, 1.0, 0.0], [0.2, 1.0, 2.0])]);
    let lamp = spawn(
        &mut e,
        [0.0, 2.0, 0.0],
        SpotLight {
            cast_shadows: true,
            ..spot(15.0, 40.0, 12.0)
        },
    );
    let points = [[0.0; 3], [2.0, 0.0, 1.0], [-1.0, 0.0, 0.5]];
    let filter = only_fixed();
    (0..60)
        .map(|tick| {
            let a = tick as f32 * 0.1;
            {
                let l = e.world_mut().get_mut::<SpotLight>(lamp).unwrap().unwrap();
                l.direction = [
                    ludion_core::math::sin(a),
                    -1.0,
                    ludion_core::math::cos(a) * 0.5,
                ];
                if tick == 30 {
                    l.intensity = 0.0;
                }
            }
            e.advance_live();
            let x = read(e.world(), &points, UP, &filter, Combine::Mean);
            if tick == 30 {
                assert!(
                    x.lights.is_empty(),
                    "the lamp reads 0 on the tick it is switched off"
                );
                assert_eq!(x.rgb, DEFAULT_AMBIENT);
            }
            bits(&x)
        })
        .collect()
}

#[test]
fn a_flashlight_sweep_is_bit_identical_at_any_thread_count() {
    let one = par::with_threads(1, sweep);
    assert!(one.iter().any(|b| b.len() > 7), "the lamp lit something");
    assert_eq!(one, par::with_threads(2, sweep));
    assert_eq!(one, par::with_threads(8, sweep));
}

#[test]
fn a_repeat_query_and_a_restored_world_read_the_same_bits() {
    let build = || {
        let mut e = engine(&[WALL]);
        casting_spot(&mut e, true);
        spawn(&mut e, [1.0, 2.0, 0.0], point(6.0));
        e
    };
    let points = [[0.0; 3], [1.0, 0.0, 1.0]];
    let f = only_fixed();
    let q = |e: &Engine| bits(&read(e.world(), &points, UP, &f, Combine::Mean));
    let mut first = build();
    let before = q(&first);
    assert_eq!(before, q(&first));
    let snapshot = match first.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut resumed = engine(&[WALL]);
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    resumed.advance_live();
    first.advance_live();
    assert_eq!(q(&resumed), q(&first));
}

#[test]
fn a_sun_and_a_point_light_on_one_entity_both_count() {
    let mut e = engine(&[]);
    let id = spawn(&mut e, [0.0, 2.0, 0.0], DirectionalLight::default());
    e.world_mut().insert(id, point(4.0)).unwrap();
    let x = at(&e, [0.0; 3], UP);
    let kinds: Vec<_> = x.lights.iter().map(|l| (l.light, l.kind)).collect();
    assert_eq!(
        kinds,
        [
            (id, ludion_engine::LightKind::Point),
            (id, ludion_engine::LightKind::Directional)
        ]
        .to_vec()
    );
}
