//! The viewmodel's depth slice: GPU tests over the remapped depth buffer.

use super::*;
use crate::viewmodel::{ViewmodelData, ViewmodelLocal, ViewmodelParticle, compose_viewmodel};
use ludion_core::math::glam::Mat4;

const PX: u32 = 64;
/// A bright green no world slab or fog colour shares.
const GUN_COLOR: [f32; 3] = [0.0625, 0.5, 0.0625];
/// The reserved slice, the lens default.
const SLICE: f32 = 0.01;

/// An empty frame under the exact post, the camera at the origin looking down −Z.
fn gun_scene() -> RenderScene {
    let mut scene = fog_scene(PX, PX);
    scene.meshes.clear();
    scene.post = Some(exact_post());
    scene
}

/// Pushes a 0.1 cube flagged as a viewmodel at camera-local z −0.45 and the lens data that goes
/// with it, composed onto the scene's camera.
pub(super) fn add_gun(scene: &mut RenderScene) {
    add_gun_at(
        scene,
        Mat4::from_translation(Vec3::new(0.0, 0.0, -0.45)) * Mat4::from_scale(Vec3::splat(0.1)),
    );
}

/// [`add_gun`] at a chosen camera-local pose.
fn add_gun_at(scene: &mut RenderScene, local: Mat4) {
    let mut gun = slab(Vec3::ZERO, 1.0, GUN_COLOR);
    gun.viewmodel = true;
    gun.cast_shadows = false;
    scene.meshes.push(gun);
    scene.viewmodel = Some(ViewmodelData {
        fov_y_deg: 55.0,
        near: 0.01,
        depth_slice: SLICE,
        projection: Mat4::perspective_rh(
            55.0_f32.to_radians(),
            crate::scene::aspect_ratio((PX, PX)),
            0.01,
            1000.0,
        ),
        locals: vec![ViewmodelLocal {
            mesh: scene.meshes.len() as u32 - 1,
            owner: ludion_core::ids::EntityId::new(1, 0),
            model: local,
            normal_matrix: Mat3::IDENTITY,
        }],
        particles: Vec::new(),
    });
    compose_viewmodel(scene, scene.camera.view.inverse());
}

/// The scene with the gun removed entirely.
fn without_gun(scene: &RenderScene) -> RenderScene {
    let mut off = scene.clone();
    off.meshes.pop();
    off.viewmodel = None;
    off
}

fn is_gun(rgb: [u8; 3]) -> bool {
    rgb[1] > 150 && rgb[0] < 100
}

/// A wall 0.2 in front of the eye, with the gun behind it in world space: the gun draws whole.
/// With the flag off the same mesh is an ordinary world mesh and the wall hides it. Skips with no
/// GPU.
#[test]
fn needs_gpu_a_gun_inside_a_wall_draws_whole() {
    let Some(renderer) = skip_or_renderer("a_gun_inside_a_wall_draws_whole") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene
        .meshes
        .push(slab(Vec3::new(0.0, 0.0, -1.7), 3.0, SLAB_COLOR));
    add_gun(&mut scene);
    let gun = renderer.render(&scene, size).expect("gun");
    let centre = rgb_at(&gun, PX / 2, PX / 2);
    assert!(is_gun(centre), "the gun is clipped by the wall: {centre:?}");

    let mut world = scene.clone();
    world.meshes.last_mut().unwrap().viewmodel = false;
    world.viewmodel = None;
    let world = renderer.render(&world, size).expect("world");
    let centre = rgb_at(&world, PX / 2, PX / 2);
    assert!(
        !is_gun(centre),
        "a world mesh behind the wall shows: {centre:?}"
    );
}

/// A cube 0.3 from the eye covers the gun's whole screen rect in world depth, and the gun still
/// draws over its middle while the cube shows around it. Skips with no GPU.
#[test]
fn needs_gpu_the_gun_draws_in_front_of_a_closer_mesh() {
    let Some(renderer) = skip_or_renderer("the_gun_draws_in_front_of_a_closer_mesh") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene
        .meshes
        .push(slab(Vec3::new(0.0, 0.0, -0.4), 0.2, SLAB_COLOR));
    add_gun(&mut scene);
    let image = renderer.render(&scene, size).expect("gun");
    let centre = rgb_at(&image, PX / 2, PX / 2);
    assert!(
        is_gun(centre),
        "the gun is hidden by the closer cube: {centre:?}"
    );
    let beside = rgb_at(&image, PX / 2 + 8, PX / 2);
    assert!(
        !is_gun(beside) && beside[0] > beside[1],
        "the cube shows beside the gun: {beside:?}"
    );
}

/// Fog and depth of field decode the remapped buffer correctly: outside the gun's footprint the
/// fogged, blurred frame is the one without the gun. The tolerance is the `[s, 1]` remap's depth
/// quantization of the world's depth, two LSB. Skips with no GPU.
#[test]
fn needs_gpu_fog_and_dof_on_the_world_are_unchanged_by_the_gun() {
    let Some(renderer) = skip_or_renderer("fog_and_dof_on_the_world_are_unchanged_by_the_gun")
    else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene.meshes = fog_scene(PX, PX).meshes;
    add_gun(&mut scene);
    let bare = without_gun(&scene);

    let on = renderer.render(&scene, size).expect("gun, exact");
    let off = renderer.render(&bare, size).expect("bare, exact");
    let mut mask = vec![false; (PX * PX) as usize];
    for y in 0..PX {
        for x in 0..PX {
            mask[(y * PX + x) as usize] = rgb_at(&on, x, y) != rgb_at(&off, x, y);
        }
    }
    let footprint = mask.iter().filter(|&&m| m).count();
    assert!(footprint > 20, "the gun covers {footprint} pixels");
    // The blur's reach is under a pixel; three more keep its rim out of the comparison.
    let near_gun = |x: u32, y: u32| {
        (y.saturating_sub(3)..=(y + 3).min(PX - 1)).any(|ny| {
            (x.saturating_sub(3)..=(x + 3).min(PX - 1)).any(|nx| mask[(ny * PX + nx) as usize])
        })
    };

    let post = PostData {
        fog_density: 0.03,
        fog_color: FOG_COLOR,
        dof: 0.01,
        focus_distance: 8.0,
        ..exact_post()
    };
    scene.post = Some(post);
    let mut bare = bare;
    bare.post = Some(post);
    let on = renderer.render(&scene, size).expect("gun, fog + dof");
    let off = renderer.render(&bare, size).expect("bare, fog + dof");
    let mut worst = 0_i32;
    let mut checked = 0;
    for y in 0..PX {
        for x in 0..PX {
            if near_gun(x, y) {
                continue;
            }
            checked += 1;
            let (a, b) = (rgb_at(&on, x, y), rgb_at(&off, x, y));
            worst = worst.max(
                (0..3)
                    .map(|c| (i32::from(a[c]) - i32::from(b[c])).abs())
                    .max()
                    .unwrap(),
            );
        }
    }
    eprintln!("fog + dof outside the gun: {checked} pixels, worst {worst} LSB");
    assert!(checked > 3000, "checked {checked}");
    assert!(
        worst <= 2,
        "the gun moved the world's fog or blur: {worst} LSB"
    );
}

/// The gun made a lit surface: no emissive, so only light reaches it.
fn lit_gun_at(scene: &mut RenderScene, local: Mat4) {
    add_gun_at(scene, local);
    scene.ambient = [0.0; 3];
    scene.meshes.last_mut().unwrap().material = [1.0, 0.0, 0.0];
}

fn lit_gun(scene: &mut RenderScene) {
    lit_gun_at(
        scene,
        Mat4::from_translation(Vec3::new(0.0, 0.0, -0.45)) * Mat4::from_scale(Vec3::splat(0.1)),
    );
}

fn world_point(scene: &RenderScene, local: Vec3) -> Vec3 {
    scene.camera.view.inverse().transform_point3(local)
}

/// The pixels where `a` and `b` differ.
fn differing(a: &RenderedImage, b: &RenderedImage) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for y in 0..PX {
        for x in 0..PX {
            if rgb_at(a, x, y) != rgb_at(b, x, y) {
                out.push((x, y));
            }
        }
    }
    out
}

fn brightest(image: &RenderedImage, pixels: &[(u32, u32)]) -> u8 {
    pixels
        .iter()
        .flat_map(|&(x, y)| rgb_at(image, x, y))
        .max()
        .unwrap_or(0)
}

fn point(position: Vec3, intensity: f32, range: f32) -> crate::scene::PointLightData {
    crate::scene::PointLightData {
        position,
        color: Vec3::ONE,
        intensity,
        range,
        dynamic: false,
        shadow: None,
        scatter: 0.0,
    }
}

/// The viewmodel, the `sun_cascades` view and the blended twins are pipeline constants, not uniform
/// data: the composed mesh shader declares `VIEWMODEL`, `BLENDED`, `SUN_CASCADE_VIEW`,
/// `IRRADIANCE_VIEW`, `BLEND_UNLIT` and `BLEND_ADDITIVE` overrides, each defaulting to false, so a pipeline built without constants shades
/// the world opaque with no debug tint, and neither the `Camera` nor the `Sun` uniform carries a field
/// for them.
#[test]
fn the_mesh_overrides_default_to_the_world() {
    let module = naga::front::wgsl::parse_str(&builtin_composed(ShaderId::Mesh))
        .expect("the composed mesh shader parses");
    let names: Vec<_> = module
        .overrides
        .iter()
        .map(|(_, o)| o.name.as_deref())
        .collect();
    assert_eq!(
        names,
        [
            Some("VIEWMODEL"),
            Some("BLENDED"),
            Some("SUN_CASCADE_VIEW"),
            Some("IRRADIANCE_VIEW"),
            Some("BLEND_UNLIT"),
            Some("BLEND_ADDITIVE")
        ]
    );
    for (_, o) in module.overrides.iter() {
        let init = o.init.expect("each override has a default");
        assert!(
            matches!(
                module.global_expressions[init],
                naga::Expression::Literal(naga::Literal::Bool(false))
            ),
            "{:?} defaults to false",
            o.name
        );
    }
    let sun = module
        .types
        .iter()
        .find(|(_, t)| t.name.as_deref() == Some("Sun"))
        .expect("the mesh shader declares Sun");
    let naga::TypeInner::Struct { members, .. } = &sun.1.inner else {
        panic!("Sun is a struct");
    };
    assert!(
        members
            .iter()
            .all(|m| m.name.as_deref() != Some("cascade_view")),
        "the cascade view is not uniform data"
    );
    let camera = module
        .types
        .iter()
        .find(|(_, t)| t.name.as_deref() == Some("Camera"))
        .expect("the mesh shader declares Camera");
    let naga::TypeInner::Struct { members, .. } = &camera.1.inner else {
        panic!("Camera is a struct");
    };
    let names: Vec<_> = members.iter().filter_map(|m| m.name.as_deref()).collect();
    assert_eq!(names, ["view_proj", "cam_pos", "view_forward"]);
}

/// A gun draw records with the mesh shader's `VIEWMODEL` twin and a world draw with the default
/// pipeline, on a plain frame and on a medium-mask frame alike; a flagged mesh with no lens camera
/// draws as the world does. Skips with no GPU.
#[test]
fn needs_gpu_gun_draws_select_the_viewmodel_twin() {
    let Some(renderer) = skip_or_renderer("gun_draws_select_the_viewmodel_twin") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    lit_gun(&mut scene);
    let recorder = renderer
        .prepare_frame(&scene, size, None)
        .expect("gun frame");
    for variant in MeshVariant::ALL {
        for medium in [false, true] {
            let twin = renderer.viewmodel_pipeline(variant, medium);
            let world = if medium {
                renderer.medium_pipeline(ShaderId::Mesh, variant)
            } else {
                renderer.variant_pipeline(ShaderId::Mesh, variant)
            };
            assert!(twin != world, "the twin is its own pipeline");
            assert!(recorder.scene_mesh_pipeline(variant, true, medium) == twin);
            assert!(recorder.scene_mesh_pipeline(variant, false, medium) == world);
        }
        assert!(
            renderer.viewmodel_pipeline(variant, true)
                != renderer.viewmodel_pipeline(variant, false),
            "the medium twin writes a fourth target"
        );
    }
    drop(recorder);
    scene.viewmodel = None;
    let recorder = renderer
        .prepare_frame(&scene, size, None)
        .expect("lens-less frame");
    assert!(
        recorder.scene_mesh_pipeline(MeshVariant::PLAIN, true, false)
            == renderer.variant_pipeline(ShaderId::Mesh, MeshVariant::PLAIN)
    );
}

/// A point light behind the eye lights a gun whose lens position is nowhere near that light's world
/// tile. Skips with no GPU.
#[test]
fn needs_gpu_a_point_light_behind_the_eye_lights_the_gun() {
    let Some(renderer) = skip_or_renderer("a_point_light_behind_the_eye_lights_the_gun") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    lit_gun(&mut scene);
    let dark = renderer.render(&scene, size).expect("dark gun");
    assert!(
        rgb_at(&dark, PX / 2, PX / 2).iter().all(|&c| c < 4),
        "the gun is lit with no light: {:?}",
        rgb_at(&dark, PX / 2, PX / 2)
    );
    scene.lights = vec![point(Vec3::new(0.0, 0.0, 0.5), 4.0, 5.0)];
    let lit = renderer.render(&scene, size).expect("lit gun");
    let centre = rgb_at(&lit, PX / 2, PX / 2);
    assert!(
        centre[1] > 40,
        "the light does not reach the gun: {centre:?}"
    );
}

/// A gun drawn outside the world frustum still reads the small light beside it, and the culled
/// cluster build renders it byte-identically to the all-lights oracle. Skips with no GPU.
#[test]
fn needs_gpu_the_gun_reads_every_light_that_reaches_it() {
    let Some(renderer) = skip_or_renderer("the_gun_reads_every_light_that_reaches_it") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene.camera.projection = Mat4::perspective_rh(
        10.0_f32.to_radians(),
        crate::scene::aspect_ratio((PX, PX)),
        0.1,
        1000.0,
    );
    lit_gun_at(
        &mut scene,
        Mat4::from_translation(Vec3::new(0.15, -0.12, -0.45)) * Mat4::from_scale(Vec3::splat(0.06)),
    );
    let bare = without_gun(&scene);
    let mask = differing(
        &renderer.render(&scene, size).expect("gun"),
        &renderer.render(&bare, size).expect("bare"),
    );
    assert!(mask.len() > 20, "the gun covers {} pixels", mask.len());

    scene.lights = vec![point(
        world_point(&scene, Vec3::new(0.15, -0.12, -0.40)),
        4.0,
        0.05,
    )];
    renderer.cluster_build.set(ClusterBuild::Culled);
    let culled = renderer.render(&scene, size).expect("culled");
    renderer.cluster_build.set(ClusterBuild::Degenerate);
    let oracle = renderer.render(&scene, size).expect("oracle");
    renderer.cluster_build.set(ClusterBuild::Culled);
    let lit = brightest(&culled, &mask);
    assert!(
        lit > 40,
        "the light beside the gun does not reach it: {lit}"
    );
    assert_eq!(culled.rgba, oracle.rgba, "the viewmodel cell lost a light");
}

/// A sun shining through a wall 0.25 ahead of the eye: the gun, poking through it, takes the sun at
/// the eye, so every gun pixel equals the wall-free frame's. Skips with no GPU.
#[test]
fn needs_gpu_a_wall_does_not_shadow_the_gun() {
    let Some(renderer) = skip_or_renderer("a_wall_does_not_shadow_the_gun") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene.sun = Some(test_sun(Vec3::NEG_Z, Vec3::splat(3.0), 0.002));
    lit_gun(&mut scene);
    let mut walled = scene.clone();
    let mut wall = slab(Vec3::ZERO, 1.0, SLAB_COLOR);
    wall.model = Mat4::from_translation(Vec3::new(0.0, 0.0, -0.25))
        * Mat4::from_scale(Vec3::new(4.0, 4.0, 0.05));
    wall.material = [1.0, 0.0, 0.0];
    walled.meshes.insert(0, wall);
    walled.viewmodel.as_mut().unwrap().locals[0].mesh += 1;

    let with_wall = renderer.render(&walled, size).expect("wall");
    let clear = renderer.render(&scene, size).expect("clear");
    let mut bare = without_gun(&walled);
    bare.viewmodel = None;
    let mask = differing(&with_wall, &renderer.render(&bare, size).expect("bare"));
    assert!(mask.len() > 20, "the gun covers {} pixels", mask.len());
    assert!(
        brightest(&clear, &mask) > 40,
        "the sun does not light the gun"
    );
    for &(x, y) in &mask {
        assert_eq!(rgb_at(&with_wall, x, y), rgb_at(&clear, x, y), "({x}, {y})");
    }
}

/// An occluder between the sun and the eye alone darkens the gun, uniformly. Skips with no GPU.
#[test]
fn needs_gpu_an_eye_in_shadow_darkens_the_whole_gun() {
    let Some(renderer) = skip_or_renderer("an_eye_in_shadow_darkens_the_whole_gun") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene.sun = Some(test_sun(
        Vec3::new(-1.0, 0.0, -1.0),
        Vec3::splat(3.0),
        0.002,
    ));
    lit_gun(&mut scene);
    let sun_side = Vec3::new(1.0, 0.0, 1.0).normalize();
    let mut blocked = scene.clone();
    let mut occluder = slab(sun_side * 0.3, 0.2, SLAB_COLOR);
    occluder.material = [1.0, 0.0, 0.0];
    blocked.meshes.insert(0, occluder);
    blocked.viewmodel.as_mut().unwrap().locals[0].mesh += 1;

    let lit = renderer.render(&scene, size).expect("lit");
    let dark = renderer.render(&blocked, size).expect("blocked");
    let mask = differing(
        &lit,
        &renderer.render(&without_gun(&scene), size).expect("bare"),
    );
    assert!(mask.len() > 20, "the gun covers {} pixels", mask.len());
    assert!(
        brightest(&lit, &mask) > 40,
        "the sun does not light the gun"
    );
    for &(x, y) in &mask {
        let (l, d) = (rgb_at(&lit, x, y), rgb_at(&dark, x, y));
        assert!(
            d.iter().map(|&c| u32::from(c)).sum::<u32>()
                < l.iter().map(|&c| u32::from(c)).sum::<u32>()
                && d.iter().all(|&c| c <= 2),
            "({x}, {y}): {l:?} -> {d:?}"
        );
    }
}

/// Sun cascades and a shadowed spot light on a floor and a caster: outside the gun's own footprint
/// the frame is the one without the gun. Skips with no GPU.
#[test]
fn needs_gpu_world_shadows_are_unchanged_by_the_gun() {
    let Some(renderer) = sun_renderer("world_shadows_are_unchanged_by_the_gun", 512, 3) else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    let mut sun = test_sun(Vec3::new(-0.4, -1.0, -0.6), Vec3::splat(2.0), 0.002);
    sun.cascades = Some(crate::scene::SunCascadeConfig {
        count: 3,
        shadow_distance: 20.0,
        splits: None,
        shadow_near: -100.0,
        shadow_far: 100.0,
    });
    scene.sun = Some(sun);
    let mut floor = ground(12.0);
    floor.model = Mat4::from_translation(Vec3::new(0.0, -1.0, -3.0)) * floor.model;
    let caster = cube(
        Vec3::new(0.3, -0.5, -2.5),
        Vec3::splat(0.8),
        [0.8, 0.3, 0.2, 1.0],
    );
    scene.meshes = vec![floor, caster];
    let mut spot = spot_light(
        Vec3::new(-1.5, 1.5, -1.0),
        Vec3::new(0.5, -1.0, -1.0),
        30.0,
        12.0,
        20.0,
        45.0,
    );
    spot.shadow = Some(crate::scene::LocalShadowData {
        bias: 0.02,
        near: 0.1,
        distance: None,
        fade: 0.0,
    });
    scene.spot_lights = vec![spot];

    let mut footprint_scene = gun_scene();
    add_gun(&mut footprint_scene);
    let footprint = differing(
        &renderer.render(&footprint_scene, size).expect("footprint"),
        &renderer
            .render(&without_gun(&footprint_scene), size)
            .expect("empty"),
    );
    assert!(
        footprint.len() > 20,
        "the gun covers {} pixels",
        footprint.len()
    );

    add_gun(&mut scene);
    let on = renderer.render(&scene, size).expect("gun");
    let off = renderer.render(&without_gun(&scene), size).expect("bare");
    let lit_world = differing(
        &off,
        &renderer.render(&without_floor(&scene), size).expect("dark"),
    );
    assert!(
        lit_world.len() > 500,
        "the world is lit over {} pixels",
        lit_world.len()
    );
    let covered = |x: u32, y: u32| {
        footprint
            .iter()
            .any(|&(fx, fy)| fx.abs_diff(x) <= 1 && fy.abs_diff(y) <= 1)
    };
    for y in 0..PX {
        for x in 0..PX {
            if !covered(x, y) {
                assert_eq!(rgb_at(&on, x, y), rgb_at(&off, x, y), "({x}, {y})");
            }
        }
    }
}

fn without_floor(scene: &RenderScene) -> RenderScene {
    let mut out = scene.clone();
    out.sun = None;
    out.spot_lights.clear();
    out
}

// --- Post exclusion: SSAO, SSR, DoF (Plan/viewmodel.md decision 12) ---------------------------

/// A lit wall 1.2 behind the eye's gun and a floor under it, so the gun sits in front of a crease.
/// Ambient is on and nothing is emissive, so the AO factor is visible in the frame.
fn creased_world(scene: &mut RenderScene) {
    scene.ambient = [0.6; 3];
    let mut wall = slab(Vec3::new(0.0, 0.0, -1.2), 1.0, [0.8, 0.8, 0.8]);
    wall.model = Mat4::from_translation(Vec3::new(0.0, 0.0, -1.2))
        * Mat4::from_scale(Vec3::new(8.0, 8.0, 0.1));
    wall.material = [1.0, 0.0, 0.0];
    let mut floor = slab(Vec3::new(0.0, -0.5, -1.0), 1.0, [0.8, 0.8, 0.8]);
    floor.model = Mat4::from_translation(Vec3::new(0.0, -0.5, -1.0))
        * Mat4::from_scale(Vec3::new(8.0, 0.1, 8.0));
    floor.material = [1.0, 0.0, 0.0];
    scene.meshes.insert(0, wall);
    scene.meshes.insert(1, floor);
    if let Some(vm) = scene.viewmodel.as_mut() {
        for l in &mut vm.locals {
            l.mesh += 2;
        }
    }
}

/// The scene with a gun, the lit creased world and `post`, and its gun-free twin.
fn creased_pair(post: PostData) -> (RenderScene, RenderScene) {
    let mut scene = gun_scene();
    add_gun(&mut scene);
    creased_world(&mut scene);
    scene.meshes.last_mut().unwrap().material = [1.0, 0.0, 0.0];
    let mut off = without_gun(&scene);
    scene.post = Some(post);
    off.post = Some(post);
    (scene, off)
}

/// The pixels the gun owns: where the gun-on frame differs from the gun-off frame under `exact_post`
/// (no SSAO, SSR or DoF), so the mask is the gun's silhouette and nothing else.
fn gun_mask(renderer: &Renderer, scene: &RenderScene) -> Vec<(u32, u32)> {
    let size = FrameSize::native(PX, PX);
    let mut on = scene.clone();
    on.post = Some(exact_post());
    let mut off = without_gun(&on);
    off.post = Some(exact_post());
    let a = renderer.render(&on, size).expect("gun mask on");
    let b = renderer.render(&off, size).expect("gun mask off");
    let mask = differing(&a, &b);
    assert!(mask.len() > 20, "the gun covers {} pixels", mask.len());
    mask
}

fn worst_lsb(a: [u8; 3], b: [u8; 3]) -> i32 {
    (0..3)
        .map(|c| (i32::from(a[c]) - i32::from(b[c])).abs())
        .max()
        .unwrap()
}

/// The gun leaves no ambient-occlusion halo: the prepass never draws it, so outside its silhouette
/// the frame is **byte-equal** to the frame without it. Skips with no GPU.
#[test]
fn needs_gpu_ssao_draws_no_halo_around_the_gun() {
    let Some(renderer) = skip_or_renderer("ssao_draws_no_halo_around_the_gun") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let post = PostData {
        ssao: 1.0,
        ssao_radius: 0.6,
        ..exact_post()
    };
    let (scene, off) = creased_pair(post);
    let mask: std::collections::HashSet<_> = gun_mask(&renderer, &scene).into_iter().collect();
    let on = renderer.render(&scene, size).expect("gun, ssao");
    let bare = renderer.render(&off, size).expect("bare, ssao");
    let mut checked = 0;
    for y in 0..PX {
        for x in 0..PX {
            if mask.contains(&(x, y)) {
                continue;
            }
            checked += 1;
            assert_eq!(
                rgb_at(&on, x, y),
                rgb_at(&bare, x, y),
                "a halo at ({x}, {y})"
            );
        }
    }
    assert!(checked > 3000, "checked {checked}");
}

/// A smooth metal floor under a bright wall, the gun high in the frame.
fn mirror_world(scene: &mut RenderScene, gun_y: f32) {
    let mut wall = slab(Vec3::ZERO, 1.0, [1.0, 0.5, 0.2]);
    wall.model = Mat4::from_translation(Vec3::new(0.0, 2.0, -4.0))
        * Mat4::from_scale(Vec3::new(10.0, 6.0, 0.1));
    let mut floor = slab(Vec3::ZERO, 1.0, [0.05, 0.05, 0.05]);
    floor.model = Mat4::from_translation(Vec3::new(0.0, -1.1, -3.0))
        * Mat4::from_scale(Vec3::new(10.0, 0.2, 10.0));
    floor.material = [0.05, 1.0, 0.0];
    scene.meshes.insert(0, wall);
    scene.meshes.insert(1, floor);
    let vm = scene.viewmodel.as_mut().expect("a gun");
    for l in &mut vm.locals {
        l.mesh += 2;
        l.model = Mat4::from_translation(Vec3::new(0.0, gun_y, -0.45))
            * Mat4::from_scale(Vec3::splat(0.1));
    }
    compose_viewmodel(scene, scene.camera.view.inverse());
}

/// SSR never reflects the gun, and the world around it is unmoved. Outside the gun's silhouette the
/// frame with the gun equals the one without within 2 LSB: the gun's only effect on the world is the
/// reflection rays that cross its screen footprint, which sit above the floor rays compared here, and
/// the 2 LSB are the `[s, 1]` depth remap's quantization. A second case puts the gun where the floor
/// would mirror it and asserts the floor below it is not gun-coloured. Skips with no GPU.
#[test]
fn needs_gpu_ssr_never_reflects_the_gun() {
    let Some(renderer) = skip_or_renderer("ssr_never_reflects_the_gun") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let ssr = PostData {
        ssr: 1.0,
        ssr_max_distance: 50.0,
        ssr_thickness: 1.0,
        ..exact_post()
    };

    // Gun high in the frame: the floor rays under test do not cross it.
    let mut scene = gun_scene();
    add_gun(&mut scene);
    mirror_world(&mut scene, 0.12);
    let mask = gun_mask(&renderer, &scene);
    let mut off = without_gun(&scene);
    scene.post = Some(ssr);
    off.post = Some(ssr);
    let on = renderer.render(&scene, size).expect("gun, ssr");
    let bare = renderer.render(&off, size).expect("bare, ssr");
    let mut worst = 0;
    let mut checked = 0;
    for y in PX / 2..PX {
        for x in 0..PX {
            if mask.contains(&(x, y)) {
                continue;
            }
            checked += 1;
            worst = worst.max(worst_lsb(rgb_at(&on, x, y), rgb_at(&bare, x, y)));
        }
    }
    eprintln!("ssr below the horizon: {checked} pixels, worst {worst} LSB");
    assert!(checked > 1000, "checked {checked}");
    assert!(
        worst <= 2,
        "the gun moved the floor's reflection: {worst} LSB"
    );

    // Gun low, over the floor's mirror: the floor under it must not show the gun.
    let mut scene = gun_scene();
    add_gun(&mut scene);
    mirror_world(&mut scene, -0.12);
    scene.post = Some(ssr);
    let image = renderer.render(&scene, size).expect("low gun, ssr");
    let gun_pixels = gun_mask(&renderer, &scene);
    let lowest = gun_pixels.iter().map(|p| p.1).max().unwrap();
    for y in lowest + 1..PX {
        for x in 0..PX {
            assert!(
                !is_gun(rgb_at(&image, x, y)),
                "the floor at ({x}, {y}) reflects the gun"
            );
        }
    }
}

/// The scene for the DoF test: the gun, a near-world cube off to one side and the creased world
/// behind, focus far away so both the cube and the gun are in the near field.
fn dof_pair() -> (RenderScene, RenderScene) {
    let post = PostData {
        dof: 0.04,
        focus_distance: 8.0,
        ..exact_post()
    };
    let (mut scene, _) = creased_pair(post);
    let mut cube = slab(Vec3::new(-0.2, 0.0, -0.6), 0.2, [0.9, 0.1, 0.1]);
    cube.cast_shadows = false;
    scene.meshes.insert(0, cube);
    if let Some(vm) = scene.viewmodel.as_mut() {
        for l in &mut vm.locals {
            l.mesh += 1;
        }
    }
    let mut off = without_gun(&scene);
    off.post = scene.post;
    (scene, off)
}

/// Depth of field neither blurs the gun nor bleeds it into the world. Gun pixels equal the DoF-off
/// frame's within 1 LSB; outside the gun's silhouette dilated by 4 px (one half-resolution
/// prefilter block plus the bilinear upsample) the frame equals the gun-free one within 2 LSB.
/// Skips with no GPU.
#[test]
fn needs_gpu_dof_neither_blurs_the_gun_nor_bleeds_it() {
    let Some(renderer) = skip_or_renderer("dof_neither_blurs_the_gun_nor_bleeds_it") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let (scene, off) = dof_pair();
    let mask = gun_mask(&renderer, &scene);
    let mut sharp = scene.clone();
    sharp.post = Some(exact_post());
    let sharp_img = renderer.render(&sharp, size).expect("gun, dof off");
    let on = renderer.render(&scene, size).expect("gun, dof");
    let bare = renderer.render(&off, size).expect("bare, dof");
    for &(x, y) in &mask {
        let worst = worst_lsb(rgb_at(&on, x, y), rgb_at(&sharp_img, x, y));
        assert!(worst <= 1, "the gun is blurred at ({x}, {y}): {worst} LSB");
    }
    let set: std::collections::HashSet<_> = mask.iter().copied().collect();
    let near_gun = |x: u32, y: u32| {
        (y.saturating_sub(4)..=(y + 4).min(PX - 1))
            .any(|ny| (x.saturating_sub(4)..=(x + 4).min(PX - 1)).any(|nx| set.contains(&(nx, ny))))
    };
    let mut worst = 0;
    let mut checked = 0;
    for y in 0..PX {
        for x in 0..PX {
            if near_gun(x, y) {
                continue;
            }
            checked += 1;
            worst = worst.max(worst_lsb(rgb_at(&on, x, y), rgb_at(&bare, x, y)));
        }
    }
    eprintln!("dof outside the gun: {checked} pixels, worst {worst} LSB");
    assert!(checked > 2000, "checked {checked}");
    assert!(worst <= 2, "the gun bled into the world: {worst} LSB");
}

/// With SSAO, SSR and DoF all on, gun pixels equal the all-off frame's within 2 LSB. Skips with no
/// GPU.
#[test]
fn needs_gpu_gun_pixels_take_no_ao_ssr_or_dof() {
    let Some(renderer) = skip_or_renderer("gun_pixels_take_no_ao_ssr_or_dof") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let post = PostData {
        ssao: 1.0,
        ssao_radius: 0.6,
        ssr: 1.0,
        dof: 0.04,
        focus_distance: 8.0,
        ..exact_post()
    };
    let (scene, _) = creased_pair(post);
    let mask = gun_mask(&renderer, &scene);
    let mut plain = scene.clone();
    plain.post = Some(exact_post());
    let all_off = renderer.render(&plain, size).expect("all off");
    let all_on = renderer.render(&scene, size).expect("all on");
    for &(x, y) in &mask {
        let worst = worst_lsb(rgb_at(&all_on, x, y), rgb_at(&all_off, x, y));
        assert!(
            worst <= 2,
            "post touched the gun at ({x}, {y}): {worst} LSB"
        );
    }
}

/// A close wall with a bright additive flash at camera-local z −0.5, behind the wall's face in
/// world depth. The flash is listed in the lens data, so it draws through the gun's depth slice.
fn flash_scene() -> RenderScene {
    let mut scene = gun_scene();
    scene
        .meshes
        .push(slab(Vec3::new(0.0, 0.0, -0.35), 0.2, SLAB_COLOR));
    let local = Vec3::new(0.0, 0.0, -0.5);
    scene.particles.push(ParticleInstance {
        additive: true,
        viewmodel: true,
        ..particle(local, 0.2, [0.0, 1.0, 0.0, 1.0], 0.0)
    });
    scene.viewmodel = Some(ViewmodelData {
        fov_y_deg: 55.0,
        near: 0.01,
        depth_slice: SLICE,
        projection: Mat4::perspective_rh(
            55.0_f32.to_radians(),
            crate::scene::aspect_ratio((PX, PX)),
            0.01,
            1000.0,
        ),
        locals: Vec::new(),
        particles: vec![ViewmodelParticle {
            particle: 0,
            center: local,
            right: Vec3::X,
            up: Vec3::Y,
        }],
    });
    let model = scene.camera.view.inverse();
    compose_viewmodel(&mut scene, model);
    scene
}

/// [`flash_scene`] with the flash removed but the lens kept, so the world's depth remap matches.
fn without_flash(scene: &RenderScene) -> RenderScene {
    let mut off = scene.clone();
    off.particles.clear();
    off.viewmodel.as_mut().unwrap().particles.clear();
    off
}

/// A flash behind a close wall in world depth still shows: it draws in the gun's slice. Flagged off
/// it is an ordinary world particle and the wall hides it. Skips with no GPU.
#[test]
fn needs_gpu_a_viewmodel_particle_draws_in_front_of_a_close_wall() {
    let Some(renderer) = skip_or_renderer("a_viewmodel_particle_draws_in_front_of_a_close_wall")
    else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let scene = flash_scene();
    let wall = renderer.render(&without_flash(&scene), size).expect("wall");
    let flash = renderer.render(&scene, size).expect("flash");
    let (wall_px, flash_px) = (
        rgb_at(&wall, PX / 2, PX / 2),
        rgb_at(&flash, PX / 2, PX / 2),
    );
    assert!(
        flash_px[1] > wall_px[1] + 100,
        "the flash is hidden by the wall: {flash_px:?} vs {wall_px:?}"
    );

    let mut world = scene.clone();
    world.particles[0].viewmodel = false;
    world.viewmodel = None;
    let hidden = renderer.render(&world, size).expect("world particle");
    let bare = rgb_at(&hidden, PX / 2, PX / 2);
    assert!(
        bare[1] < wall_px[1] + 10,
        "a world particle behind the wall shows: {bare:?}"
    );
}

/// The flash writes nothing to the transparent layer: it matches the frame without the flash.
/// Skips with no GPU.
#[test]
fn needs_gpu_a_viewmodel_particle_writes_no_transparent_layer() {
    let Some(renderer) = skip_or_renderer("a_viewmodel_particle_writes_no_transparent_layer")
    else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let scene = flash_scene();
    let with = layer_texels(&renderer, &scene, size);
    let without = layer_texels(&renderer, &without_flash(&scene), size);
    assert!(
        with.iter().zip(&without).all(|(a, b)| a == b),
        "the flash wrote into the transparent layer"
    );
}

/// The gun is decoded as a world surface just under the near plane, so fog sees it at about
/// `near`, not its true distance. Under dense fog the mean gun-pixel change stays within what fog
/// at `near` does, plus two LSB. Skips with no GPU.
#[test]
fn needs_gpu_gun_fog_is_bounded_by_the_near_plane() {
    let Some(renderer) = skip_or_renderer("gun_fog_is_bounded_by_the_near_plane") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let sigma = 0.5_f32;
    let near = 0.1_f32;
    let mut scene = gun_scene();
    add_gun(&mut scene);
    let mask = gun_mask(&renderer, &scene);
    let dense = PostData {
        fog_density: sigma,
        fog_color: FOG_COLOR,
        ..exact_post()
    };
    let clear = renderer.render(&scene, size).expect("gun, no fog");
    scene.post = Some(dense);
    let fogged = renderer.render(&scene, size).expect("gun, fog");

    let amount = 1.0 - (-sigma * near).exp();
    for channel in 0..3 {
        let mean = |image: &RenderedImage| {
            mask.iter()
                .map(|&(x, y)| f32::from(rgb_at(image, x, y)[channel]))
                .sum::<f32>()
                / mask.len() as f32
        };
        let measured = (mean(&fogged) - mean(&clear)).abs();
        let c = GUN_COLOR[channel];
        let at_near =
            (srgb8_from_linear(c + amount * (FOG_COLOR[channel] - c)) - srgb8_from_linear(c)).abs();
        eprintln!("gun fog, channel {channel}: measured {measured:.2} LSB, bound {at_near:.2}");
        assert!(
            measured <= at_near + 2.0,
            "channel {channel}: fog moved the gun {measured} LSB, over {at_near} + 2"
        );
    }
}
