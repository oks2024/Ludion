//! The reflections pass's probe payload: the ranked probe list, its per-cell bitmask and the
//! fallback cube array (`Plan/reflections.md` decisions 2, 15). The shading itself is
//! `shaders/snippets/probes.wgsl` and the `reflections.wgsl` fill; `probe.rs` holds the CPU twins.

use ludion_core::math::glam::Vec3;

use crate::cluster::probe_masks;
use crate::probe::{MAX_PROBES, ProbeData, ProbeVolume, probe_rank};
use crate::probe_capture::ProbeCubes;
use crate::scene::RenderScene;
use crate::shader_gen::{ProbeList, ShadeProbe};

/// The probe uniform and cell masks for a frame. `cubes` is `None` when nothing was captured,
/// which uploads a zero count so the pass skips the probe path. `grid` is the frame's
/// `LightHeader.grid`.
pub(crate) fn build_probe_list(
    scene: &RenderScene,
    cubes: Option<&ProbeCubes>,
    grid: [f32; 4],
    intensity: f32,
    render_extent: (u32, u32),
) -> (ProbeList, Vec<u32>) {
    let mut list: ProbeList = bytemuck::Zeroable::zeroed();
    list.header[2] = intensity;
    list.grid = grid;
    let Some(cubes) = cubes else {
        return (list, vec![0]);
    };
    let held = &scene.probes[..scene.probes.len().min(MAX_PROBES)];
    let mut order: Vec<usize> = (0..held.len()).collect();
    order.sort_by(|&a, &b| probe_rank(&held[a], &held[b]));
    let ranked: Vec<ProbeData> = order.iter().map(|&i| held[i]).collect();
    for (k, &i) in order.iter().enumerate() {
        list.probes[k] = shade_probe(&held[i], cubes.slots[i]);
    }
    list.header[0] = ranked.len() as f32;
    list.header[1] = cubes.max_lod as f32;
    let masks = probe_masks(&ranked, &scene.camera, render_extent);
    (list, masks)
}

fn shade_probe(probe: &ProbeData, slot: u32) -> ShadeProbe {
    let column = |axis: Vec3| {
        let v = probe.rotation * axis;
        [v.x, v.y, v.z, 0.0]
    };
    let (extent, kind) = match probe.volume {
        ProbeVolume::Box { extent } => ([extent.x, extent.y, extent.z, 0.0], 0.0),
        ProbeVolume::Sphere { radius } => ([radius, 0.0, 0.0, 0.0], 1.0),
    };
    ShadeProbe {
        center: [
            probe.position.x,
            probe.position.y,
            probe.position.z,
            slot as f32,
        ],
        axis_x: column(Vec3::X),
        axis_y: column(Vec3::Y),
        axis_z: column(Vec3::Z),
        extent,
        params: [kind, probe.blend, 0.0, 0.0],
    }
}

/// A 1×1, six-layer cube-array view bound when no probe was captured; never sampled then.
pub(crate) fn fallback_cubes(device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ludion fallback probe cubes"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::CubeArray),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::FrameSize;
    use crate::ibl::tests::test_env;
    use crate::probe_capture::tests::{lit_scene, mesh, probe_at};
    use crate::render::Renderer;
    use crate::scene::{
        EnvironmentData, MeshShape, PostData, RenderCamera, RenderScene, TonemapOperator,
    };
    use ludion_core::ids::EntityId;
    use ludion_core::math::glam::{Mat4, Quat};
    use ludion_render_graph::execute::RenderedImage;

    #[test]
    fn the_shader_and_the_scene_agree_on_the_probe_cap() {
        assert_eq!(crate::shader_gen::MAX_SHADE_PROBES as usize, MAX_PROBES);
    }

    fn renderer_or_skip(what: &str) -> Option<Renderer> {
        match Renderer::new() {
            Ok(r) => Some(r),
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED {what}: no GPU adapter");
                None
            }
            Err(d) => panic!("renderer init failed: {d:?}"),
        }
    }

    fn pixel(img: &RenderedImage, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * img.width + x) * 4) as usize;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
    }

    /// A mirror floor under a blue sky, an emissive red wall behind it and a camera looking at
    /// the floor: the floor reflects the wall, which only a probe can show.
    fn room_scene() -> RenderScene {
        let mut scene = RenderScene::empty((64, 64));
        scene.post = Some(PostData {
            bloom_strength: 0.0,
            exposure: 1.0,
            tonemap: TonemapOperator::None,
            dither: false,
            ..PostData::default()
        });
        scene.environment = Some(EnvironmentData {
            sky_zenith: [0.05, 0.1, 0.9],
            sky_horizon: [0.05, 0.1, 0.9],
            sky_ground: [0.05, 0.1, 0.9],
            ..test_env()
        });
        let eye = Vec3::new(0.0, 2.0, 4.0);
        scene.camera = RenderCamera {
            view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
            projection: Mat4::perspective_rh(60.0_f32.to_radians(), 1.0, 0.1, 100.0),
            position: eye,
        };
        scene.meshes = vec![
            mesh(
                MeshShape::Plane,
                Mat4::from_scale(Vec3::new(30.0, 1.0, 30.0)),
                [0.9, 0.9, 0.9, 1.0],
                [0.05, 1.0, 0.0],
            ),
            mesh(
                MeshShape::Cube,
                Mat4::from_translation(Vec3::new(0.0, 4.0, -6.1))
                    * Mat4::from_scale(Vec3::new(30.0, 8.0, 0.2)),
                [1.0, 0.0, 0.0, 1.0],
                [1.0, 0.0, 4.0],
            ),
        ];
        scene
    }

    /// A box probe fitted to the room puts the wall in the mirror floor where the sky showed.
    #[test]
    fn needs_gpu_a_box_probe_reflects_the_room() {
        let Some(r) = renderer_or_skip("a box probe reflects the room") else {
            return;
        };
        let size = FrameSize::native(64, 64);
        let mut scene = room_scene();
        let bare = r.render(&scene, size).expect("render without a probe");
        scene.probes = vec![ProbeData {
            entity: EntityId::new(1, 0),
            position: Vec3::new(0.0, 4.0, 0.0),
            rotation: Quat::IDENTITY,
            volume: ProbeVolume::Box {
                extent: Vec3::new(15.0, 5.0, 6.0),
            },
            blend: 0.0,
            priority: 0,
        }];
        let probed = r.render(&scene, size).expect("render with a probe");
        // The centre ray meets the floor at the origin and reflects up and back into the wall.
        let [br, _, bb] = pixel(&bare, 32, 32);
        let [pr, _, pb] = pixel(&probed, 32, 32);
        assert!(bb > br + 10, "without the probe the sky shows: {br} {bb}");
        assert!(
            pr > pb + 40,
            "with the probe the wall shows: {pr} vs {pb} (without: {br} {bb})"
        );
    }

    /// A probe whose volume holds no visible pixel takes the fill path nowhere: the frame is
    /// byte-identical to the probe-less one.
    #[test]
    fn needs_gpu_an_unseen_probe_moves_no_byte() {
        let Some(r) = renderer_or_skip("an unseen probe moves no byte") else {
            return;
        };
        let size = FrameSize::native(64, 64);
        let mut scene = lit_scene();
        scene.probes.clear();
        scene.meshes[0].material = [0.1, 1.0, 0.0];
        let bare = r.render(&scene, size).expect("render without a probe");
        let mut far = probe_at(1, Vec3::new(400.0, 0.0, 400.0));
        far.volume = ProbeVolume::Box {
            extent: Vec3::splat(1.0),
        };
        scene.probes = vec![far];
        let probed = r.render(&scene, size).expect("render with a far probe");
        assert!(bare.rgba == probed.rgba, "a probe nobody sees moved a byte");
    }

    /// The render pixel the world point `p` lands on, for a `size`² frame.
    fn pixel_of(scene: &RenderScene, size: u32, p: Vec3) -> (u32, u32) {
        let clip = scene.camera.projection * scene.camera.view * p.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        let extent = size as f32;
        (
            ((ndc.x + 1.0) * 0.5 * extent) as u32,
            ((1.0 - ndc.y) * 0.5 * extent) as u32,
        )
    }

    fn view_pixel(img: &RenderedImage, scene: &RenderScene, p: Vec3) -> [u8; 3] {
        let (x, y) = pixel_of(scene, img.width, p);
        pixel(img, x, y)
    }

    fn box_probe(entity: u32, extent: Vec3, priority: i32) -> ProbeData {
        ProbeData {
            entity: EntityId::new(entity, 0),
            position: Vec3::new(0.0, 4.0, 0.0),
            rotation: Quat::IDENTITY,
            volume: ProbeVolume::Box { extent },
            blend: 0.0,
            priority,
        }
    }

    fn palette_bytes(rank: usize) -> [u8; 3] {
        crate::PROBE_VIEW_PALETTE[rank].map(|c| (c * 255.0).round() as u8)
    }

    fn assert_near(got: [u8; 3], want: [u8; 3], what: &str) {
        let close = got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1);
        assert!(close, "{what}: got {got:?}, want {want:?}");
    }

    /// The view paints each pixel with the winning probe's rank colour, gray where the volumes miss
    /// the surface.
    #[test]
    fn needs_gpu_probe_index_view_colors_the_winner() {
        let Some(r) = renderer_or_skip("the probe index view colours the winner") else {
            return;
        };
        let size = FrameSize::native(64, 64);
        let mut scene = room_scene();
        scene.debug_view = Some(crate::DebugView::ProbeIndex);
        let origin = Vec3::ZERO;

        scene.probes = vec![box_probe(1, Vec3::new(15.0, 5.0, 6.0), 0)];
        let image = r.render(&scene, size).expect("render the view");
        assert_near(
            view_pixel(&image, &scene, origin),
            palette_bytes(0),
            "the lone box is rank 0",
        );

        // A higher priority ranks first: the sphere takes rank 0 over its side of the floor and the
        // box falls to rank 1 elsewhere.
        let mut sphere = box_probe(2, Vec3::ZERO, 1);
        sphere.position = Vec3::new(-1.5, 0.0, 0.0);
        sphere.volume = ProbeVolume::Sphere { radius: 0.8 };
        scene.probes.push(sphere);
        let image = r.render(&scene, size).expect("render the view");
        assert_near(
            view_pixel(&image, &scene, Vec3::new(-1.5, 0.0, 0.0)),
            palette_bytes(0),
            "inside the higher-priority sphere",
        );
        assert_near(
            view_pixel(&image, &scene, origin),
            palette_bytes(1),
            "the box outside the sphere",
        );

        // A box narrowed in z leaves the floor nearer the camera outside every volume.
        scene.probes = vec![box_probe(1, Vec3::new(15.0, 5.0, 1.0), 0)];
        let image = r.render(&scene, size).expect("render the view");
        assert_near(
            view_pixel(&image, &scene, origin),
            palette_bytes(0),
            "inside the narrow box",
        );
        assert_near(
            view_pixel(&image, &scene, Vec3::new(0.0, 0.0, 1.8)),
            [64, 64, 64],
            "floor outside every volume",
        );
    }

    /// A scene with no probes ignores the view: the frame is the one drawn without it.
    #[test]
    fn needs_gpu_a_probe_view_without_probes_draws_the_normal_frame() {
        let Some(r) = renderer_or_skip("a probe view without probes draws the normal frame") else {
            return;
        };
        let size = FrameSize::native(64, 64);
        let mut scene = room_scene();
        let plain = r.render(&scene, size).expect("render without a view");
        scene.debug_view = Some(crate::DebugView::ProbeIndex);
        let viewed = r.render(&scene, size).expect("render with the view");
        assert!(
            plain.rgba == viewed.rgba,
            "the view moved a byte of a probe-free frame"
        );
    }
}
