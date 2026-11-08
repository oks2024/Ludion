//! `MeshInstance.rim`: GPU tests over the unlit Fresnel edge in `mesh.wgsl`.

use super::*;
use std::sync::Arc;

use super::skin_bounds::bounded_skin_quad;

const PX: u32 = 128;

fn size() -> FrameSize {
    FrameSize::native(PX, PX)
}

/// A dark sphere of the same angular size at distance `d` (radius `d / 4`, so `sin a = 1/4`),
/// carrying `emissive` of `color` and `rim`; the exact-output post, no lights, a black sky.
fn sphere_scene(d: f32, color: [f32; 3], emissive: f32, rim: [f32; 4]) -> RenderScene {
    let mut scene = RenderScene::empty((PX, PX));
    scene.camera = origin_camera(PX, PX);
    scene.ambient = [0.0; 3];
    scene.post = Some(exact_post());
    let mut sphere = slab(Vec3::new(0.0, 0.0, -d), d / 2.0, color);
    sphere.shape = MeshShape::Sphere;
    sphere.material = [1.0, 0.0, emissive];
    sphere.rim = rim;
    scene.meshes = vec![sphere];
    scene
}

fn px(frame: &RenderedImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * frame.width + x) * 4) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

fn sum(p: [u8; 3]) -> i32 {
    p.iter().map(|&c| i32::from(c)).sum()
}

/// A pixel on the sphere's rim ring, 3/4 of the way from the centre to the silhouette.
const RING: (u32, u32) = (PX / 2 + 20, PX / 2);
const CENTRE: (u32, u32) = (PX / 2, PX / 2);

/// Strength 0 (a zero rgb lane) renders byte-identically to no rim at all. Skips with no GPU.
#[test]
fn needs_gpu_a_zero_strength_rim_is_byte_identical_to_none() {
    let Some(renderer) = skip_or_renderer("a_zero_strength_rim_is_byte_identical_to_none") else {
        return;
    };
    let none = renderer
        .render(&sphere_scene(4.0, [0.4; 3], 1.0, [0.0; 4]), size())
        .expect("none");
    let zero = renderer
        .render(
            &sphere_scene(4.0, [0.4; 3], 1.0, [0.0, 0.0, 0.0, 3.0]),
            size(),
        )
        .expect("zero");
    assert_eq!(none.rgba, zero.rgba);
}

/// A rim lights the silhouette ring and leaves the facing centre alone. Skips with no GPU.
#[test]
fn needs_gpu_a_rim_brightens_the_ring_and_not_the_centre() {
    let Some(renderer) = skip_or_renderer("a_rim_brightens_the_ring_and_not_the_centre") else {
        return;
    };
    let off = renderer
        .render(&sphere_scene(4.0, [0.25; 3], 1.0, [0.0; 4]), size())
        .expect("off");
    let on = renderer
        .render(
            &sphere_scene(4.0, [0.25; 3], 1.0, [0.5, 0.5, 0.5, 1.0]),
            size(),
        )
        .expect("on");
    let (ring_on, ring_off) = (sum(px(&on, RING.0, RING.1)), sum(px(&off, RING.0, RING.1)));
    assert!(ring_on > ring_off + 30, "ring {ring_on} vs {ring_off}");
    let (c_on, c_off) = (px(&on, CENTRE.0, CENTRE.1), px(&off, CENTRE.0, CENTRE.1));
    for ch in 0..3 {
        assert!(
            (i32::from(c_on[ch]) - i32::from(c_off[ch])).abs() <= 1,
            "centre channel {ch}: {c_on:?} vs {c_off:?}"
        );
    }
}

/// A skinned instance rims like its unskinned twin placed by a model matrix: the skinned normal
/// is what the Fresnel term reads. Skips with no GPU.
#[test]
fn needs_gpu_a_skinned_instance_rims_like_its_unskinned_twin() {
    let Some(renderer) = skip_or_renderer("a_skinned_instance_rims_like_its_unskinned_twin") else {
        return;
    };
    let turn = Mat4::from_rotation_y(60.0_f32.to_radians());
    let place = Mat4::from_translation(Vec3::new(0.0, 0.0, -3.0));
    let base = |rim: [f32; 4], skinned: bool| {
        let mut scene = RenderScene::empty((PX, PX));
        scene.camera = origin_camera(PX, PX);
        scene.ambient = [0.0; 3];
        scene.post = Some(exact_post());
        scene
            .mesh_assets
            .push(Arc::new(bounded_skin_quad(0x71_0001)));
        let mut item = if skinned {
            scene.bone_palette = vec![turn];
            skin_instance(place, 1)
        } else {
            let mut twin = skin_instance(place * turn, 0);
            twin.normal_matrix = Mat3::from_mat4(turn);
            twin
        };
        item.color = [0.25, 0.25, 0.25, 1.0];
        item.rim = rim;
        scene.meshes = vec![item];
        scene
    };
    let rim = [0.5, 0.0, 0.0, 1.0];
    let plain = renderer
        .render(&base([0.0; 4], true), size())
        .expect("plain");
    let skinned = renderer.render(&base(rim, true), size()).expect("skinned");
    let twin = renderer.render(&base(rim, false), size()).expect("twin");
    let at = (PX / 2, PX / 2);
    let (p, s, t) = (
        px(&plain, at.0, at.1),
        px(&skinned, at.0, at.1),
        px(&twin, at.0, at.1),
    );
    assert!(
        s[0] > p[0] + 20,
        "the tilted quad takes a rim: {s:?} vs {p:?}"
    );
    for ch in 0..3 {
        assert!(
            (i32::from(s[ch]) - i32::from(t[ch])).abs() <= 2,
            "skinned {s:?} vs twin {t:?}"
        );
    }
}

fn linear(byte: u8) -> f32 {
    let c = f32::from(byte) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Rim rides the colour target like emissive, so the medium attenuates it the same way: the
/// far/near brightness ratio of a rimmed mesh equals an emissive twin's. Skips with no GPU.
#[test]
fn needs_gpu_a_distant_rim_is_fogged_like_emissive() {
    let Some(renderer) = skip_or_renderer("a_distant_rim_is_fogged_like_emissive") else {
        return;
    };
    let render = |d: f32, color: [f32; 3], emissive: f32, rim: [f32; 4]| {
        let mut scene = sphere_scene(d, color, emissive, rim);
        scene.post = Some(PostData {
            fog_density: 0.05,
            fog_color: [0.0; 3],
            ..exact_post()
        });
        let frame = renderer.render(&scene, size()).expect("render");
        linear(px(&frame, RING.0, RING.1)[0])
    };
    let rim = [0.5, 0.0, 0.0, 1.0];
    let (rim_near, rim_far) = (
        render(4.0, [0.0; 3], 0.0, rim),
        render(20.0, [0.0; 3], 0.0, rim),
    );
    let (em_near, em_far) = (
        render(4.0, [0.4, 0.0, 0.0], 1.0, [0.0; 4]),
        render(20.0, [0.4, 0.0, 0.0], 1.0, [0.0; 4]),
    );
    let (rim_ratio, em_ratio) = (rim_far / rim_near, em_far / em_near);
    eprintln!("rim {rim_near}->{rim_far} ({rim_ratio}), emissive {em_near}->{em_far} ({em_ratio})");
    assert!(rim_near > 0.02, "the rim is visible: {rim_near}");
    assert!(rim_ratio < 0.8, "fog attenuates the far rim: {rim_ratio}");
    assert!(
        (rim_ratio - em_ratio).abs() < 0.05,
        "rim {rim_ratio} vs emissive {em_ratio}"
    );
}
