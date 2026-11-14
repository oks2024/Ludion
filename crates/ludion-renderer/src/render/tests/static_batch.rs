//! The persistent static buffers (`render/static_batch.rs`): a scene whose leading meshes are marked
//! static renders the same bytes as the scene without the marker, and the buffers are built once,
//! patched when a static moves and rebuilt when the layout changes.

use super::gpu_cull::{casting_light, chained_asset, oracle_counts, visible_world};
use super::*;
use crate::render::static_batch::{RunKey, StaticUpdate, changed_spans};
use crate::scene::StaticRange;

fn perspective() -> Mat4 {
    Mat4::perspective_rh(
        60.0_f32.to_radians(),
        crate::scene::aspect_ratio((96, 96)),
        0.1,
        1000.0,
    )
}

fn placed(mut mesh: MeshInstance, shape: MeshShape, x: f32, mirrored: bool) -> MeshInstance {
    mesh.shape = shape;
    mesh.model = Mat4::from_translation(Vec3::new(x, -0.5, -9.0))
        * Mat4::from_scale(Vec3::new(if mirrored { -1.0 } else { 1.0 }, 1.0, 1.0));
    mesh
}

/// Marks the leading `k` meshes static, over the leading `a` assets.
fn statics(scene: &mut RenderScene, k: usize, a: usize) {
    scene.statics = StaticRange::of(&scene.meshes[..k], &scene.mesh_assets[..a]);
}

/// The same scene with no static marker.
fn zeroed(scene: &RenderScene) -> RenderScene {
    let mut plain = scene.clone();
    plain.statics = StaticRange::default();
    plain
}

/// Statics first: a ground (static-only bucket), a non-caster sphere and a chained asset (both
/// shared with a dynamic), a mirrored asset (static-only mirrored bucket) and an off-screen copy;
/// `cubes` adds a static cube and a mirrored one beside a dynamic cube. Dynamics: a sphere and a
/// chained asset sharing the static buckets, a second asset only they draw, and an off-screen sphere.
/// A cascaded sun and one shadowed point light.
fn static_scene(cubes: bool) -> (RenderScene, usize) {
    let mut scene = fog_scene(96, 96);
    scene.camera.projection = perspective();
    scene.sun = Some(test_sun(
        Vec3::new(0.4, -1.0, 0.3),
        Vec3::new(3.0, 3.0, 3.0),
        0.002,
    ));
    scene.sun.as_mut().unwrap().cascades = Some(crate::scene::SunCascadeConfig {
        count: 3,
        shadow_distance: 40.0,
        splits: None,
        shadow_near: -60.0,
        shadow_far: 60.0,
    });
    let mut point = point_light(Vec3::new(-2.0, 1.5, -7.0), 30.0, 9.0);
    point.shadow = casting_light();
    scene.lights.push(point);
    scene.mesh_assets = vec![chained_asset(), Arc::new(box_asset(0x57_02, false))];
    let base = slab(Vec3::ZERO, 1.0, [0.4, 0.3, 0.2]);
    let mut ground = ground(40.0);
    ground.model = Mat4::from_translation(Vec3::new(0.0, -1.5, 0.0)) * ground.model;
    let mut dim = placed(base, MeshShape::Sphere, -3.0, false);
    dim.cast_shadows = false;
    let mut meshes = vec![
        ground,
        dim,
        placed(base, MeshShape::Asset(0), 0.0, false),
        placed(base, MeshShape::Asset(0), 3.0, true),
        placed(base, MeshShape::Asset(0), 40.0, false),
    ];
    if cubes {
        meshes.push(placed(base, MeshShape::Cube, -6.0, false));
        meshes.push(placed(base, MeshShape::Cube, 6.0, true));
    }
    let k = meshes.len();
    meshes.push(placed(base, MeshShape::Sphere, -1.5, false));
    meshes.push(placed(base, MeshShape::Asset(1), 1.5, false));
    meshes.push(placed(base, MeshShape::Asset(0), 4.5, false));
    meshes.push(placed(base, MeshShape::Sphere, -40.0, false));
    if cubes {
        meshes.push(placed(base, MeshShape::Cube, -4.5, false));
    }
    scene.meshes = meshes;
    statics(&mut scene, k, 1);
    (scene, k)
}

fn render(renderer: &Renderer, scene: &RenderScene) -> Vec<u8> {
    renderer
        .render(scene, FrameSize::native(96, 96))
        .expect("render")
        .rgba
}

#[test]
fn changed_spans_coalesce_and_are_empty_on_equal_input() {
    let old = [1u32, 2, 3, 4, 5, 6];
    assert!(changed_spans(&old, &old).is_empty());
    assert_eq!(changed_spans(&old, &[1, 9, 9, 4, 5, 7]), vec![1..3, 5..6]);
    assert_eq!(changed_spans(&old, &[0; 6]), vec![0..6]);
}

#[test]
fn runs_follow_the_frame_packing_order() {
    let prim = |mirrored, shape| RunKey::Prim { mirrored, shape };
    let asset = |mirrored, index| RunKey::Asset { mirrored, index };
    let canonical = [
        prim(false, MeshShape::Cube),
        prim(false, MeshShape::Plane),
        prim(false, MeshShape::Sphere),
        prim(true, MeshShape::Cube),
        prim(true, MeshShape::Plane),
        prim(true, MeshShape::Sphere),
        asset(false, 0),
        asset(false, 3),
        asset(true, 0),
        asset(true, 2),
    ];
    let mut shuffled = [
        canonical[8],
        canonical[3],
        canonical[6],
        canonical[9],
        canonical[0],
        canonical[5],
        canonical[7],
        canonical[1],
        canonical[4],
        canonical[2],
    ];
    shuffled.sort_by_key(|key| key.rank());
    assert_eq!(shuffled, canonical);
}

/// **Statics change no byte.** The mixed scene renders what the same scene without the marker renders,
/// with culling on and off, and the first frame builds the buffers.
#[test]
fn needs_gpu_statics_render_the_same_bytes_as_dynamics() {
    let Some(mut renderer) = skip_or_renderer("statics_render_the_same_bytes") else {
        return;
    };
    let (scene, _) = static_scene(true);
    let plain = zeroed(&scene);
    for cull in [true, false] {
        renderer.set_frustum_culling(cull);
        let want = render(&renderer, &plain);
        assert!(!renderer.statics_held());
        let got = render(&renderer, &scene);
        assert!(renderer.statics_held());
        assert_eq!(got, want, "culling {cull}: statics changed the image");
    }
    renderer.set_frustum_culling(true);
    assert_ne!(
        render(&renderer, &scene),
        render(&renderer, &{
            let mut empty = scene.clone();
            empty.meshes.truncate(0);
            empty.statics = StaticRange::default();
            empty
        }),
        "the fixture draws something"
    );
}

#[test]
fn needs_gpu_the_static_render_repeats_identically() {
    let Some(renderer) = skip_or_renderer("the_static_render_repeats_identically") else {
        return;
    };
    let (scene, _) = static_scene(true);
    let first = render(&renderer, &scene);
    for run in 1..20 {
        assert_eq!(first, render(&renderer, &scene), "repeat {run} differs");
    }
}

/// **The buffers update only as much as changed.** One renderer walks a scene through a quiet frame, a
/// moved static, a new static bucket, a moved bucket numbering and a scene without statics; each frame
/// equals a fresh renderer's render of the scene without the marker.
#[test]
fn needs_gpu_the_static_buffers_patch_rebuild_and_remap() {
    let Some(renderer) = skip_or_renderer("the_static_buffers_patch_rebuild_and_remap") else {
        return;
    };
    let fresh = |scene: &RenderScene| {
        let other = Renderer::new().expect("a second renderer");
        render(&other, &zeroed(scene))
    };
    let check = |scene: &RenderScene, kind: StaticUpdate, what: &str| {
        let got = render(&renderer, scene);
        assert_eq!(renderer.last_static_update(), kind, "{what}");
        assert_eq!(got, fresh(scene), "{what}: bytes differ");
    };
    let (mut scene, mut k) = static_scene(false);
    check(&scene, StaticUpdate::Rebuilt, "first frame");
    check(&scene, StaticUpdate::Quiet, "same scene");

    scene.meshes[2].model.w_axis.x += 0.5;
    statics(&mut scene, k, 1);
    check(
        &scene,
        StaticUpdate::Patched { slots: 1 },
        "one static moved",
    );

    let plane = placed(
        slab(Vec3::ZERO, 1.0, [0.2, 0.5, 0.3]),
        MeshShape::Plane,
        2.0,
        true,
    );
    scene.meshes.insert(k, plane);
    k += 1;
    statics(&mut scene, k, 1);
    check(&scene, StaticUpdate::Rebuilt, "a static in a new bucket");

    // The static buckets are a plane and assets; a dynamic cube takes bucket 0 and moves them all.
    scene.meshes.push(placed(
        slab(Vec3::ZERO, 1.0, [0.5, 0.2, 0.2]),
        MeshShape::Cube,
        -4.5,
        false,
    ));
    check(
        &scene,
        StaticUpdate::Remapped,
        "a dynamic cube shifts the buckets",
    );
    check(&scene, StaticUpdate::Quiet, "after the remap");

    let plain = zeroed(&scene);
    assert!(renderer.statics_held());
    assert_eq!(render(&renderer, &plain), fresh(&scene), "no statics");
    assert!(
        !renderer.statics_held(),
        "a frame without statics drops them"
    );
}

/// **The profile's counts are the oracle's with a static prefix**: the camera view tests every world
/// mesh and keeps what the CPU cull keeps; each shadow view tests the casters.
#[test]
fn needs_gpu_cull_stats_match_the_oracle_with_statics() {
    let Some(renderer) = skip_or_renderer("cull_stats_match_the_oracle_with_statics") else {
        return;
    };
    renderer.set_cull_stats(true);
    let (scene, _) = static_scene(true);
    let size = FrameSize::native(96, 96);
    renderer.render(&scene, size).expect("render");
    let samples = renderer.collect_cull_stats(true);
    assert_eq!(samples.len(), 1);
    let views = &samples[0];
    let recorder = renderer.prepare_frame(&scene, size, None).unwrap();
    let plan = recorder.local_shadow.as_ref().expect("a local view");
    let (seen, world) = visible_world(&scene);
    assert_eq!(
        (
            views[0].name.as_str(),
            views[0].instances as usize,
            views[0].drawn as usize
        ),
        ("camera", world, seen)
    );
    let casters = scene
        .meshes
        .iter()
        .filter(|m| !m.viewmodel && m.cast_shadows)
        .count() as u32;
    for view in &views[1..4] {
        assert_eq!(view.instances, casters, "{} tests the casters", view.name);
        assert!(view.drawn <= view.instances);
    }
    for (i, (view, planned)) in views[4..].iter().zip(&plan.views).enumerate() {
        let (tested, kept) = oracle_counts(&scene, &planned.cull, true);
        assert_eq!((view.instances, view.drawn), (tested, kept), "local_{i}");
    }
    assert!(
        seen > 0 && seen < world,
        "the camera culls some: {seen} of {world}"
    );
}
