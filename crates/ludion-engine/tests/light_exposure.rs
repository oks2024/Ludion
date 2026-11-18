//! `light_exposure`: the sim-side answer to "how much authored light reaches these points", with
//! physics-ray occlusion, determinism across snapshot/restore and thread counts.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::Vec3;
use ludion_core::par;
use ludion_ecs::{Parent, SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Combine, DirectionalLight, Engine, EngineBuilder, Environment, Exposure, ExposureError,
    ExposureQuery, Facing, LightExposure, LightExposureResults, MAX_EXPOSURE_POINTS, PointLight,
    SpotLight, Transform, Visibility, light_exposure, light_exposure_update, world_affine,
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

// --- the scene-authored path: `LightExposure` read by `light_exposure_update` ---

const LAYERS: &str = "  layers:\n    - name: light_occluder\n";

/// A scene document: free-space physics with one declared layer, then the entities' YAML.
fn doc(entities: &str) -> String {
    format!("format: 1\nseed: 1\nphysics:\n  space: free\n{LAYERS}entities:\n{entities}")
}

/// An entity with a `Transform` at `at` and `rest` (components already indented to 6).
fn entity(name: &str, at: [f32; 3], rest: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {:?}\n          - {:?}\n          - {:?}\n{rest}",
        at[0], at[1], at[2]
    )
}

fn point_lamp(name: &str, at: [f32; 3], range: f32) -> String {
    entity(
        name,
        at,
        &format!("      PointLight:\n        range: {range}\n        cast_shadows: true\n"),
    )
}

fn wall(name: &str, at: [f32; 3], body: &str, extra: &str) -> String {
    let body = if body == "rigid" {
        "\n          rigid:\n            mass: 1.0"
    } else {
        body
    };
    entity(
        name,
        at,
        &format!(
            "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 2.0\n              - 0.1\n              - 2.0\n        body: {body}\n{extra}"
        ),
    )
}

fn subject(name: &str, at: [f32; 3], exposure: &str) -> String {
    entity(name, at, &format!("      LightExposure:\n{exposure}"))
}

const TWO_POINTS: &str = "        points:\n          -\n            - 0.0\n            - 0.3\n            - 0.0\n          -\n            - 0.0\n            - 1.6\n            - 0.0\n";

fn builder() -> EngineBuilder {
    Engine::builder().system("light_exposure_update", light_exposure_update)
}

fn scene_engine(entities: &str) -> Engine {
    let mut e = builder().build().expect("boots");
    e.load_inline_scene(&doc(entities))
        .expect("the scene loads");
    // The solver sees the colliders from the tick after they load.
    e.advance_live();
    e.advance_live();
    e
}

fn named(e: &mut Engine, wanted: &str) -> EntityId {
    e.world_mut()
        .query_ids::<(&ludion_engine::Name,)>()
        .find(|(_, (n,))| n.0 == wanted)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no entity named {wanted}"))
}

fn results(e: &Engine) -> &LightExposureResults {
    e.world().resource::<LightExposureResults>().unwrap()
}

#[test]
fn the_component_reads_like_a_direct_call_through_the_pose() {
    let entities = point_lamp("lamp", [0.0, 4.0, 0.0], 12.0)
        + &wall("roof", [0.0, 2.0, 0.0], "static", "")
        + &subject("plain", [0.0, 0.0, 0.0], TWO_POINTS)
        + &entity(
            "posed",
            [5.0, 0.0, 0.0],
            "        rotation:\n          - 0.0\n          - 0.0\n          - 90.0\n        scale:\n          - 2.0\n          - 2.0\n          - 2.0\n      LightExposure:\n        points:\n          -\n            - 0.0\n            - 1.0\n            - 0.0\n        facing:\n          normal:\n            - 0.0\n            - 1.0\n            - 0.0\n        combine: mean\n",
        );
    let mut e = scene_engine(&entities);
    let (plain, posed) = (named(&mut e, "plain"), named(&mut e, "posed"));
    let direct = |id: EntityId, local: &[[f32; 3]], facing, combine| {
        let affine = world_affine(e.world(), id).unwrap();
        let points: Vec<[f32; 3]> = local.iter().map(|p| affine.transform_point(*p)).collect();
        let filter = QueryFilter::new().only_fixed().exclude(id);
        read(e.world(), &points, facing, &filter, combine)
    };
    let want = direct(
        plain,
        &[[0.0, 0.3, 0.0], [0.0, 1.6, 0.0]],
        Facing::Omni,
        Combine::Max,
    );
    assert_eq!(bits(results(&e).reading(plain).unwrap()), bits(&want));
    assert!(
        want.lights.iter().all(|l| l.occluded),
        "the roof blocks the lamp"
    );
    let want = direct(posed, &[[0.0, 1.0, 0.0]], UP, Combine::Mean);
    assert_eq!(bits(results(&e).reading(posed).unwrap()), bits(&want));
    let moved = affine_point(&e, posed, [0.0, 1.0, 0.0]);
    assert!(
        (moved[0] - 3.0).abs() < 1e-4 && moved[1].abs() < 1e-4,
        "{moved:?}"
    );
    assert_eq!(results(&e).readings().len(), 2);
    assert!(results(&e).reading(EntityId::new(999, 0)).is_none());
}

fn affine_point(e: &Engine, id: EntityId, p: [f32; 3]) -> [f32; 3] {
    world_affine(e.world(), id).unwrap().transform_point(p)
}

#[test]
fn a_rotated_scaled_parent_maps_the_points() {
    let mut e = scene_engine(&point_lamp("lamp", [0.0, 4.0, 0.0], 12.0));
    let parent = e.world_mut().spawn();
    e.world_mut()
        .insert(
            parent,
            Transform {
                translation: [1.0, 0.0, 0.0],
                rotation: [0.0, 90.0, 0.0],
                scale: [2.0, 2.0, 2.0],
            },
        )
        .unwrap();
    let child = e.world_mut().spawn();
    e.world_mut().insert(child, Parent(parent)).unwrap();
    e.world_mut()
        .insert(
            child,
            Transform {
                translation: [0.5, 0.0, 0.0],
                ..Transform::default()
            },
        )
        .unwrap();
    e.world_mut()
        .insert(
            child,
            LightExposure {
                points: vec![[0.0, 0.0, 1.0]],
                ..LightExposure::default()
            },
        )
        .unwrap();
    e.advance_live();
    let world_point = affine_point(&e, child, [0.0, 0.0, 1.0]);
    assert_ne!(world_point, [0.0, 0.0, 1.0]);
    let filter = QueryFilter::new().only_fixed().exclude(child);
    let want = read(
        e.world(),
        &[world_point],
        Facing::Omni,
        &filter,
        Combine::Max,
    );
    assert_eq!(bits(results(&e).reading(child).unwrap()), bits(&want));
}

fn occluded(e: &Engine, id: EntityId) -> bool {
    let x = results(e).reading(id).unwrap();
    assert!(!x.lights.is_empty(), "the lamp reaches the subject");
    x.lights.iter().all(|l| l.occluded)
}

#[test]
fn default_occluders_are_static_colliders_and_the_subject_is_excluded() {
    let lit = |blocker: &str| {
        let entities = point_lamp("lamp", [0.0, 4.0, 0.0], 12.0)
            + blocker
            + &subject(
                "s",
                [0.0; 3],
                "        points:\n          -\n            - 0.0\n            - 0.0\n            - 0.0\n",
            );
        let mut e = scene_engine(&entities);
        let s = named(&mut e, "s");
        occluded(&e, s)
    };
    assert!(
        !lit(&wall("box", [0.0, 2.0, 0.0], "rigid", "")),
        "a rigid body does not occlude"
    );
    assert!(
        lit(&wall("roof", [0.0, 2.0, 0.0], "static", "")),
        "a static wall does"
    );
    let own = entity(
        "s",
        [0.0; 3],
        "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 0.5\n              - 0.5\n              - 0.5\n        body: static\n      LightExposure: {}\n",
    );
    let mut e = scene_engine(&(point_lamp("lamp", [0.0, 4.0, 0.0], 12.0) + &own));
    let s = named(&mut e, "s");
    assert!(
        !occluded(&e, s),
        "the subject's own static collider never occludes"
    );
}

#[test]
fn authored_occluder_layers_replace_the_static_default() {
    let tagged = "      QueryLayers:\n        layers:\n          - light_occluder\n";
    let entities = point_lamp("lamp_a", [0.0, 4.0, 0.0], 6.0)
        + &point_lamp("lamp_b", [10.0, 4.0, 0.0], 6.0)
        + &wall("plain_wall", [0.0, 2.0, 0.0], "static", "")
        + &wall("tagged_wall", [10.0, 2.0, 0.0], "static", tagged)
        + &subject(
            "a",
            [0.0; 3],
            "        occluder_layers:\n          - light_occluder\n",
        )
        + &subject(
            "b",
            [10.0, 0.0, 0.0],
            "        occluder_layers:\n          - light_occluder\n",
        );
    let mut e = scene_engine(&entities);
    let (a, b) = (named(&mut e, "a"), named(&mut e, "b"));
    assert!(!occluded(&e, a), "the untagged wall is not an occluder");
    assert!(occluded(&e, b), "the tagged wall is");
}

#[test]
fn an_undeclared_occluder_layer_gives_no_reading_and_one_latched_l1186() {
    let entities = point_lamp("lamp", [0.0, 4.0, 0.0], 12.0)
        + &wall("far", [20.0, 0.0, 0.0], "static", "")
        + &subject(
            "bad",
            [0.0; 3],
            "        occluder_layers:\n          - nope\n",
        )
        + &subject(
            "good",
            [3.0, 0.0, 0.0],
            "        points:\n          -\n            - 0.0\n            - 0.0\n            - 0.0\n",
        );
    let mut e = scene_engine(&entities);
    for _ in 0..2 {
        e.advance_live();
    }
    let (bad, good) = (named(&mut e, "bad"), named(&mut e, "good"));
    assert!(results(&e).reading(bad).is_none());
    assert!(results(&e).reading(good).is_some());
    let reported = e
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code::COLLIDER_LAYER_UNKNOWN)
        .count();
    assert_eq!(reported, 1);
}

fn hashes(with_system: bool) -> Vec<u64> {
    let entities = point_lamp("lamp", [0.0, 4.0, 0.0], 12.0)
        + &wall("box", [0.0, 2.0, 0.0], "rigid", "")
        + &entity("walker", [3.0, 0.0, 0.0], "");
    let mut b = Engine::builder();
    if with_system {
        b = b.system("light_exposure_update", light_exposure_update);
    }
    let mut e = b.build().expect("boots");
    e.load_inline_scene(&doc(&entities)).unwrap();
    (0..30)
        .map(|_| {
            e.advance_live();
            e.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn a_scene_without_the_component_hashes_the_same_with_the_system_scheduled() {
    assert_eq!(hashes(true), hashes(false));
}

/// Sweeps the casting spot's direction and cuts its intensity at tick 30, from the frame alone.
fn sweep_lamp(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64();
    let a = frame as f32 * 0.1;
    let ids: Vec<EntityId> = world
        .query_ids::<(&SpotLight,)>()
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        let l = world.get_mut::<SpotLight>(id).unwrap().unwrap();
        l.direction = [
            ludion_core::math::sin(a),
            -1.0,
            ludion_core::math::cos(a) * 0.5,
        ];
        l.intensity = if frame >= 30 { 0.0 } else { 1.0 };
    }
}

fn trace_engine() -> Engine {
    let entities = entity(
        "lamp",
        [0.0, 2.0, 0.0],
        "      SpotLight:\n        direction:\n          - 0.0\n          - -1.0\n          - 0.0\n        inner_angle: 15.0\n        outer_angle: 40.0\n        range: 12.0\n        cast_shadows: true\n",
    ) + &wall("post", [3.0, 1.0, 0.0], "static", "")
        + &subject(
            "s",
            [0.0; 3],
            "        points:\n          -\n            - 0.0\n            - 0.0\n            - 0.0\n          -\n            - 2.0\n            - 0.0\n            - 1.0\n          -\n            - -1.0\n            - 0.0\n            - 0.5\n        facing:\n          normal:\n            - 0.0\n            - 1.0\n            - 0.0\n        combine: mean\n",
        );
    let mut e = Engine::builder()
        .system("sweep_lamp", sweep_lamp)
        .system("light_exposure_update", light_exposure_update)
        .build()
        .expect("boots");
    e.load_inline_scene(&doc(&entities)).unwrap();
    e
}

/// `(results tick, world hash, reading bits, lit lights)` for one advanced tick.
fn record(e: &mut Engine) -> (u64, u64, Vec<u32>, usize) {
    e.advance_live();
    let s = named(e, "s");
    let hash = e.world_hash().unwrap().as_u64();
    let x = results(e).reading(s).unwrap();
    (results(e).tick(), hash, bits(x), x.lights.len())
}

fn trace() -> Vec<(u64, u64, Vec<u32>, usize)> {
    let mut e = trace_engine();
    (0..60).map(|_| record(&mut e)).collect()
}

#[test]
fn the_exposure_trace_is_thread_invariant_and_cuts_at_tick_30() {
    let one = par::with_threads(1, trace);
    assert_eq!(one, par::with_threads(2, trace));
    assert_eq!(one, par::with_threads(8, trace));
    assert!(
        one.iter().any(|t| t.3 > 0),
        "the lamp lit the subject at some tick"
    );
    for (tick, _, _, lit) in &one {
        if *tick >= 30 {
            assert_eq!(*lit, 0, "the lamp reads 0 from tick {tick}");
        }
    }
    assert!(
        one.iter().filter(|t| t.0 < 30).any(|t| t.3 > 0),
        "and contributed before the cut"
    );
}

#[test]
fn the_exposure_trace_survives_a_snapshot_and_restore() {
    let mut first = trace_engine();
    let mut tail = Vec::new();
    let mut snapshot = None;
    for tick in 0..60 {
        if tick == 20 {
            snapshot = Some(match first.execute(Command::EngineSnapshot).unwrap().body {
                ResponseBody::Snapshot { snapshot } => snapshot,
                other => panic!("expected a snapshot, got {other:?}"),
            });
        }
        let r = record(&mut first);
        if tick >= 20 {
            tail.push(r);
        }
    }
    let mut resumed = trace_engine();
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: snapshot.unwrap(),
        }))
        .unwrap();
    let replay: Vec<_> = (20..60).map(|_| record(&mut resumed)).collect();
    assert_eq!(replay, tail);
}
