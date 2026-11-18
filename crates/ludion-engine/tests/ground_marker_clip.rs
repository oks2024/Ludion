//! `GroundMarkerClip`: the visibility profile cast through physics, its derivation from a marker,
//! its packing into the render scene, and its agreement with 0001 sight.

use std::path::Path;

use ludion_assets::AssetRoot;
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::Quat;
use ludion_core::math::{cos, sin};
use ludion_core::time::{SimTime, TickRate};
use ludion_ecs::World;
use ludion_engine::{
    Engine, Name, PerceptionConfig, PerceptionResults, ProfileQuery, QualityParams,
    VisibilityProfile, extract_at, ground_marker_profile, perception_update, visibility_profile,
};
use ludion_physics::{PhysicsWorld, QueryFilter};

const HEAD: &str = "format: 1\nseed: 1\nphysics:\n  space: free\n  layers:\n    - name: sight_blocker\n    - name: low_cover\nentities:\n";

fn seq(indent: usize, v: &[f32]) -> String {
    v.iter()
        .map(|c| format!("{:indent$}- {c:?}\n", ""))
        .collect()
}

/// A static box on `layer`.
fn boxed(name: &str, at: [f32; 3], half: [f32; 3], layer: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n{}      Collider:\n        shape:\n          box:\n            half_extents:\n{}        body: static\n        layer: {layer}\n",
        seq(10, &at),
        seq(14, &half)
    )
}

/// A marker at the origin: `shape` is the block-style shape fields, `clip` the clip fields.
fn marker(shape: &str, clip: &str, extra: &str) -> String {
    format!(
        "  - name: marker\n    components:\n      Transform:\n        translation:\n{}      GroundMarker:\n        shape:\n{shape}      GroundMarkerClip:\n{clip}{extra}",
        seq(10, &[0.0, 0.0, 0.0])
    )
}

const DISC: &str = "          type: disc\n          radius: 8.0\n";
const CLIP: &str = "        bins: 64\n        layers:\n          - sight_blocker\n";

fn engine(entities: &[String]) -> Engine {
    let mut text = HEAD.to_string();
    text += &entities.concat();
    let mut engine = Engine::builder()
        .perception(PerceptionConfig {
            classes: vec!["player".into()],
            ..Default::default()
        })
        .system("perception_update", perception_update)
        .build()
        .expect("boots");
    engine.load_inline_scene(&text).expect("the scene loads");
    engine
}

fn find(engine: &mut Engine, wanted: &str) -> EntityId {
    engine
        .world_mut()
        .query_ids::<(&Name,)>()
        .find(|(_, (name,))| name.0 == wanted)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no entity named {wanted}"))
}

fn profile(engine: &mut Engine) -> VisibilityProfile {
    let id = find(engine, "marker");
    ground_marker_profile(engine.world(), id)
        .expect("a clipped marker")
        .expect("casts")
}

/// The bearing of bin `i` of a full-circle profile of `bins` bins.
fn angle(p: &VisibilityProfile, i: usize) -> f32 {
    p.arc_start + (i as f32 + 0.5) * p.arc_span / p.radii.len() as f32
}

/// A wall whose near face is the plane x = 5, 40 wide, on `sight_blocker`.
fn wall() -> String {
    boxed("wall", [5.5, 0.0, 0.0], [0.5, 2.0, 20.0], "sight_blocker")
}

fn extract(world: &World) -> ludion_renderer::RenderScene {
    extract_at(
        world,
        (64, 64),
        AssetRoot::content(Path::new(".")),
        SimTime::from_ticks(0, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )
    .expect("extracts")
}

#[test]
fn a_wall_clips_each_bin_to_its_distance_and_the_rest_to_range() {
    // No tick has run: the cast must already see the static wall.
    let mut engine = engine(&[wall(), marker(DISC, CLIP, "")]);
    let p = profile(&mut engine);
    assert_eq!(p.radii.len(), 64);
    let (mut clipped, mut free) = (0, 0);
    for (i, &r) in p.radii.iter().enumerate() {
        let a = angle(&p, i);
        let wall_at = 5.0 / cos(a);
        if cos(a) > 0.0 && (wall_at - 8.0).abs() > 0.05 {
            if wall_at < 8.0 {
                assert!((r - wall_at).abs() < 1e-4, "bin {i}: {r} vs {wall_at}");
                clipped += 1;
            } else {
                assert_eq!(r, 8.0, "bin {i}");
                free += 1;
            }
        } else if cos(a) <= 0.0 {
            assert_eq!(r, 8.0, "bin {i} faces away from the wall");
            free += 1;
        }
    }
    assert!(clipped > 10 && free > 10, "{clipped} clipped, {free} free");
    assert_eq!(
        p.partial, p.radii,
        "no partial layers: partial equals radii"
    );
}

#[test]
fn the_profile_is_the_same_after_ticks_and_in_a_fresh_world() {
    let mut a = engine(&[wall(), marker(DISC, CLIP, "")]);
    let at_load = profile(&mut a);
    a.advance_live();
    a.advance_live();
    assert_eq!(profile(&mut a), at_load);
    let mut b = engine(&[wall(), marker(DISC, CLIP, "")]);
    let (x, y) = (profile(&mut b), at_load);
    let bits = |p: &VisibilityProfile| -> Vec<u32> {
        p.radii
            .iter()
            .chain(&p.partial)
            .map(|v| v.to_bits())
            .collect()
    };
    assert_eq!(bits(&x), bits(&y));
}

#[test]
fn a_partial_box_dims_only_its_bins() {
    let cover = boxed("cover", [3.0, 0.0, 0.0], [0.25, 2.0, 1.0], "low_cover");
    let clip = format!("{CLIP}        partial_layers:\n          - low_cover\n");
    let mut engine = engine(&[wall(), cover, marker(DISC, &clip, "")]);
    let p = profile(&mut engine);
    let (mut dimmed, mut clear) = (0, 0);
    for (i, &r) in p.radii.iter().enumerate() {
        let a = angle(&p, i);
        if cos(a) <= 0.0 {
            continue;
        }
        let tan = (sin(a) / cos(a)).abs();
        if tan * 2.75 < 0.9 {
            assert!((p.partial[i] - 2.75 / cos(a)).abs() < 1e-4, "bin {i}");
            dimmed += 1;
        } else if tan * 2.75 > 1.1 {
            assert_eq!(p.partial[i], r, "bin {i} misses the cover");
            clear += 1;
        }
    }
    assert!(dimmed >= 4 && clear > 10, "{dimmed} dimmed, {clear} clear");
}

#[test]
fn exclude_skips_the_markers_own_collider() {
    // A static slab around the marker's ray origin, on the clipped layer.
    let own = format!(
        "  - name: marker\n    components:\n      Transform:\n        translation:\n{}      Collider:\n        shape:\n          box:\n            half_extents:\n{}        body: static\n        layer: sight_blocker\n      GroundMarker:\n        shape:\n{DISC}      GroundMarkerClip:\n{CLIP}",
        seq(10, &[0.0, 0.0, 0.0]),
        seq(14, &[1.0, 2.0, 1.0])
    );
    let mut engine = engine(&[own]);
    let p = profile(&mut engine);
    assert!(
        p.radii.iter().all(|&r| r == 8.0),
        "the root's own box is skipped"
    );

    engine.advance_live();
    let id = find(&mut engine, "marker");
    let physics = engine.world().resource::<PhysicsWorld>().unwrap();
    let blockers = QueryFilter::new().only_fixed();
    let query = |exclude| ProfileQuery {
        origin: [0.0, 1.6, 0.0],
        rotation: Quat::IDENTITY,
        arc_start: -std::f32::consts::PI,
        arc_span: std::f32::consts::TAU,
        range: 8.0,
        bins: 16,
        blockers: &blockers,
        partial: None,
        exclude,
    };
    let unexcluded = visibility_profile(physics, &query(None)).unwrap();
    assert!(
        unexcluded.radii.iter().all(|&r| r < 8.0),
        "inside the slab every ray hits it"
    );
    let excluded = visibility_profile(physics, &query(Some(id))).unwrap();
    assert!(excluded.radii.iter().all(|&r| r == 8.0));
}

#[test]
fn extract_packs_the_header_and_two_bins_worth_of_floats() {
    let wedge =
        "          type: wedge\n          half_angle_deg: 30.0\n          outer_radius: 8.0\n";
    let mut engine = engine(&[wall(), marker(wedge, CLIP, "")]);
    let scene = extract(engine.world());
    assert_eq!(scene.ground_markers.len(), 1);
    assert_eq!(scene.ground_markers[0].clip, Some(0));
    let floats = &scene.ground_marker_profiles;
    assert_eq!(floats.len(), 4 + 2 * 64);
    let half = 30.0_f32.to_radians();
    assert_eq!(floats[0], -half);
    assert!((floats[1] - 2.0 * half).abs() < 1e-6);
    assert_eq!(floats[2], 0.35);
    assert_eq!(floats[3], 64.0);
    let p = profile(&mut engine);
    assert_eq!(&floats[4..68], &p.radii[..]);
    assert_eq!(&floats[68..], &p.partial[..]);
}

#[test]
fn an_unclipped_marker_packs_no_profile() {
    let plain = format!(
        "  - name: marker\n    components:\n      Transform:\n        translation:\n{}      GroundMarker:\n        shape:\n{DISC}",
        seq(10, &[0.0, 0.0, 0.0])
    );
    let engine = engine(&[wall(), plain]);
    let scene = extract(engine.world());
    assert_eq!(scene.ground_markers[0].clip, None);
    assert!(scene.ground_marker_profiles.is_empty());
}

#[test]
fn a_clip_without_physics_draws_unclipped() {
    let mut text = String::from("format: 1\nseed: 1\nentities:\n");
    text += &marker(DISC, "        bins: 64\n", "");
    let mut engine = Engine::builder().build().expect("boots");
    engine.load_inline_scene(&text).expect("loads");
    let scene = extract(engine.world());
    assert_eq!(scene.ground_markers[0].clip, None);
    assert!(scene.ground_marker_profiles.is_empty());
    // `extract_at` has no log to report to, so the code is the engine's own degraded path; the
    // world-level reader answers `None`.
    let id = find(&mut engine, "marker");
    assert!(ground_marker_profile(engine.world(), id).is_none());
    let _ = code::GROUND_MARKER_CLIP_NO_PHYSICS;
}

/// A guard at the origin looks down -Z through a wall at z = -5; a target stands behind it.
/// Sight reports the target unseen, and the clip profile ends at the wall on the target's bearing.
#[test]
fn sight_and_the_clip_agree_behind_a_wall() {
    let guard = "  - name: guard\n    components:\n      Transform:\n        translation:\n"
        .to_string()
        + &seq(10, &[0.0, 0.0, 0.0])
        + "      Perceiver:\n        eye:\n          - 0.0\n          - 1.6\n          - 0.0\n        sight:\n          - name: z\n            range: 20.0\n            half_angle_deg: 60.0\n"
        + "      GroundMarker:\n        shape:\n          type: disc\n          radius: 12.0\n      GroundMarkerClip:\n        bins: 64\n        layers:\n          - sight_blocker\n";
    let target = "  - name: runner\n    components:\n      Transform:\n        translation:\n"
        .to_string()
        + &seq(10, &[0.0, 0.0, -8.0])
        + "      SightTarget:\n        class: player\n        points:\n          - offset:\n              - 0.0\n              - 1.6\n              - 0.0\n            blocked_by:\n              - sight_blocker\n";
    let wall = boxed("wall", [0.0, 0.0, -5.5], [20.0, 2.0, 0.5], "sight_blocker");
    let mut engine = engine(&[guard, target, wall]);
    for _ in 0..4 {
        engine.advance_live();
    }
    let guard = find(&mut engine, "guard");
    let runner = find(&mut engine, "runner");
    let sample = engine
        .world()
        .resource::<PerceptionResults>()
        .unwrap()
        .sight(guard)
        .iter()
        .find(|s| s.target == runner)
        .cloned()
        .expect("the target is inside the zone");
    assert!(!sample.visible, "the wall blocks sight");

    let p = ground_marker_profile(engine.world(), guard)
        .expect("clipped")
        .expect("casts");
    // The target's bearing is local -Z, a = -pi/2: the bins either side of it.
    let at = p
        .radii
        .iter()
        .enumerate()
        .filter(|(i, _)| (angle(&p, *i) + std::f32::consts::FRAC_PI_2).abs() < 0.1)
        .map(|(_, r)| *r)
        .collect::<Vec<_>>();
    assert!(!at.is_empty());
    assert!(
        at.iter().all(|&r| r < 8.0),
        "{at:?} ends before the target at 8"
    );
}
