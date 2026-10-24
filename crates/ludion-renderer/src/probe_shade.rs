//! The reflections pass's probe payload: the ranked probe list, its per-cell bitmask and the
//! fallback cube array (`Plan/reflections.md` decisions 2, 15). The shading itself is
//! `shaders/snippets/probes.wgsl` and the `reflections.wgsl` fill; `probe.rs` holds the CPU twins.

use ludion_core::math::glam::Vec3;

use crate::cluster::probe_masks;
use crate::probe::{MAX_PROBES, ProbeData, ProbeVolume, probe_rank};
use crate::probe_capture::{AxisIrradiance, ProbeCubes};
use crate::scene::RenderScene;
use crate::shader_gen::{ProbeList, ShadeProbe};

/// The probe uniform and cell masks for a frame. `cubes` is `None` when nothing was captured,
/// which uploads a zero count so the pass skips the probe path. `grid` is the frame's
/// `LightHeader.grid`.
pub(crate) fn build_probe_list(
    scene: &RenderScene,
    cubes: Option<&ProbeCubes>,
    grid: [f32; 4],
    render_extent: (u32, u32),
) -> (ProbeList, Vec<u32>) {
    let mut list: ProbeList = bytemuck::Zeroable::zeroed();
    list.grid = grid;
    let Some(cubes) = cubes else {
        return (list, vec![0]);
    };
    let held = &scene.probes[..scene.probes.len().min(MAX_PROBES)];
    let mut order: Vec<usize> = (0..held.len()).collect();
    order.sort_by(|&a, &b| probe_rank(&held[a], &held[b]));
    let ranked: Vec<ProbeData> = order.iter().map(|&i| held[i]).collect();
    for (k, &i) in order.iter().enumerate() {
        list.probes[k] = shade_probe(
            &held[i],
            cubes.slots[i],
            &cubes.irradiance[i],
            cubes.emissive_slots[i],
        );
    }
    list.header[0] = ranked.len() as f32;
    list.header[1] = cubes.max_lod as f32;
    let masks = probe_masks(&ranked, &scene.camera, render_extent);
    (list, masks)
}

fn shade_probe(probe: &ProbeData, slot: u32, baked: &AxisIrradiance, emissive: bool) -> ShadeProbe {
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
        params: [kind, probe.blend, f32::from(emissive), 0.0],
        baked: baked.map(|e| [e[0], e[1], e[2], 0.0]),
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
        EnvironmentData, MeshShape, PostData, ProbeWorld, RenderCamera, RenderScene,
        TonemapOperator,
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

        // A blend band fades toward gray, never toward black.
        let mut soft = box_probe(1, Vec3::new(15.0, 5.0, 1.0), 0);
        soft.blend = 1.0;
        scene.probes = vec![soft];
        let image = r.render(&scene, size).expect("render the view");
        let edge = view_pixel(&image, &scene, Vec3::new(0.0, 0.0, 1.5));
        let winner = palette_bytes(0);
        assert_ne!(edge, [0, 0, 0], "a weak probe is not black");
        assert!(
            edge != [64, 64, 64] && edge != winner,
            "{edge:?} is mid-blend"
        );
        for c in 0..3 {
            let (lo, hi) = (winner[c].min(64), winner[c].max(64));
            assert!((lo..=hi).contains(&edge[c]), "channel {c} of {edge:?}");
        }
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

    /// The probe the relight tests share: a box fitted to the room, no blend.
    fn room_probe() -> ProbeData {
        box_probe(1, Vec3::new(15.0, 5.0, 6.0), 0)
    }

    /// A static scene has live E = baked E, so the relight ratio is exactly 1: two frames agree, and
    /// so do a fresh capture and a store hit.
    #[test]
    fn needs_gpu_a_static_probe_scene_renders_identically_across_frames_and_store_hits() {
        let (Some(mut a), Some(mut b)) = (
            renderer_or_skip("a static probe scene"),
            renderer_or_skip("a static probe scene"),
        ) else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("ludion-probe-relight-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        a.set_probe_store(Some(dir.clone()));
        b.set_probe_store(Some(dir.clone()));
        let size = FrameSize::native(64, 64);
        let mut scene = room_scene();
        scene.probes = vec![room_probe()];
        let first = a.render(&scene, size).expect("first frame");
        let second = a.render(&scene, size).expect("second frame");
        assert!(
            first.rgba == second.rgba,
            "a static scene changed between frames"
        );
        let hit = b.render(&scene, size).expect("a store hit renders");
        assert_eq!(
            b.probe_resolve().probes.iter().filter(|p| p.loaded).count(),
            1
        );
        assert!(
            first.rgba == hit.rgba,
            "a store hit differs from a fresh capture"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The capture is frozen, the live ambient is not: raising it brightens a pixel that reflects only
    /// probe geometry, and leaving it alone changes nothing.
    #[test]
    fn needs_gpu_a_raised_live_ambient_brightens_the_probe_geometry() {
        let Some(r) = renderer_or_skip("the relight follows the live ambient") else {
            return;
        };
        let size = FrameSize::native(64, 64);
        let mut scene = room_scene();
        scene.post = scene.post.map(|post| PostData {
            exposure: 0.05,
            ..post
        });
        // The wall's own light is ambient only: the test follows the geometry, not its emission.
        scene.meshes[1].material[2] = 0.0;
        scene.ambient = [0.2; 3];
        scene.probes = vec![room_probe()];
        scene.probe_world = Some(ProbeWorld::new(scene.clone()));
        let before = r.render(&scene, size).expect("baseline");
        let same = r.render(&scene, size).expect("unchanged ambient");
        assert!(before.rgba == same.rgba);
        scene.ambient = [1.0; 3];
        let after = r.render(&scene, size).expect("raised ambient");
        // The centre pixel reflects the red wall.
        let [r0, _, _] = pixel(&before, 32, 32);
        let [r1, _, _] = pixel(&after, 32, 32);
        assert!(r0 > 0, "no signal to compare: {r0}");
        assert!(r1 > r0 + 4, "the wall did not brighten: {r0} -> {r1}");
        assert_eq!(
            r.probe_resolve().resolve,
            1,
            "the ambient recaptured the probe"
        );
    }

    /// An open Lambertian floor under a dim sky gradient, one probe above it: the relight's ratio
    /// along the floor's facing (+Y) is the floor's own change when the sky's zenith moves, which the
    /// old mean-of-six-axes ratio is not.
    #[test]
    fn needs_gpu_a_zenith_change_relights_a_floor_exactly() {
        use crate::ibl::tests::sky_gradient;
        use crate::probe::{axis_ratio, ratio};
        let Some(r) = renderer_or_skip("a zenith change relights a floor") else {
            return;
        };
        let size = FrameSize::native(64, 64);
        let mut scene = room_scene();
        scene.post = scene.post.map(|post| PostData {
            exposure: 4.0,
            ..post
        });
        let mut env = EnvironmentData {
            sky_zenith: [0.015, 0.028, 0.075],
            sky_horizon: [0.07, 0.1, 0.17],
            sky_ground: [0.008, 0.01, 0.018],
            ..test_env()
        };
        scene.environment = Some(env);
        scene.ambient = [0.0; 3];
        scene.meshes.truncate(1);
        scene.meshes[0].color = [1.0, 1.0, 1.0, 1.0];
        scene.meshes[0].material = [1.0, 0.0, 0.0];
        scene.probes = vec![room_probe()];
        scene.probe_world = Some(ProbeWorld::new(scene.clone()));
        let centre = Vec3::ZERO;

        let before_e = r.read_probe_irradiance(&scene)[0];
        assert_eq!(
            before_e.live, before_e.baked,
            "a fresh capture is its own baseline"
        );
        let before = r.render(&scene, size).expect("baseline");
        env.sky_zenith = [0.12, 0.05, 0.02];
        scene.environment = Some(env);
        let after_e = r.read_probe_irradiance(&scene)[0];
        let after = r.render(&scene, size).expect("moved zenith");
        assert_eq!(
            r.probe_resolve().resolve,
            1,
            "the zenith recaptured the probe"
        );
        assert_eq!(
            after_e.baked, before_e.baked,
            "the bake follows the capture"
        );

        let v = |e: &crate::AxisIrradiance| e.map(Vec3::from_array);
        let (baked, live) = (v(&after_e.baked), v(&after_e.live));
        let facing = axis_ratio(baked, live, Vec3::Y);
        let truth = {
            let lin = |img: &RenderedImage| view_pixel(img, &scene, centre).map(srgb_to_linear);
            let (b, a) = (lin(&before), lin(&after));
            Vec3::new(a[0] / b[0], a[1] / b[1], a[2] / b[2])
        };
        let mean = |e: [Vec3; 6]| e.iter().sum::<Vec3>() / 6.0;
        let old = ratio(mean(baked), mean(live));
        eprintln!(
            "floor truth {truth:?}, per-axis {facing:?}, old mean {old:?}, pixels {:?} -> {:?}",
            view_pixel(&before, &scene, centre),
            view_pixel(&after, &scene, centre)
        );
        let miss = |got: Vec3| ((got - truth) / truth).abs();
        assert!(
            miss(facing).max_element() < 0.02,
            "the per-axis ratio missed the floor: {facing:?} vs {truth:?}"
        );
        assert!(
            miss(old).x.max(miss(old).z) > 0.2,
            "the old mean ratio no longer misses the floor: {old:?} vs {truth:?}"
        );

        // Metals still drift: a mirror along a 45 degrees up ray sees the sky's own change, which no
        // facing's ratio follows (decision 17's limit).
        let ray = [0.0, 0.5_f32.sqrt(), 0.5_f32.sqrt()];
        let (sky_b, sky_a) = (
            sky_gradient(
                &EnvironmentData {
                    sky_zenith: [0.015, 0.028, 0.075],
                    ..env
                },
                ray,
            ),
            sky_gradient(&env, ray),
        );
        let sky_change = Vec3::from_array(sky_a) / Vec3::from_array(sky_b);
        for n in [
            Vec3::Y,
            -Vec3::Y,
            Vec3::X,
            -Vec3::X,
            Vec3::Z,
            -Vec3::Z,
            Vec3::new(0.0, 1.0, 1.0).normalize(),
        ] {
            let got = axis_ratio(baked, live, n);
            let off = ((got - sky_change) / sky_change).abs().max_element();
            assert!(
                off > 0.05,
                "facing {n:?} follows a metal's sky change: {got:?} vs {sky_change:?}"
            );
        }
    }

    fn srgb_to_linear(byte: u8) -> f32 {
        let c = f32::from(byte) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    /// A mirror floor reflecting two wall panels through a fitted box probe: an emissive metal one
    /// (no lit part) at `x = -4` and a lit white dielectric one at `x = 4`.
    fn panel_scene(emissive: f32) -> RenderScene {
        let mut scene = room_scene();
        scene.environment = None;
        scene.post = scene.post.map(|post| PostData {
            exposure: 0.25,
            ..post
        });
        scene.meshes.truncate(1);
        let panel = |x: f32, color: [f32; 4], material: [f32; 3]| {
            mesh(
                MeshShape::Cube,
                Mat4::from_translation(Vec3::new(x, 3.0, -5.9))
                    * Mat4::from_scale(Vec3::new(4.0, 4.0, 0.2)),
                color,
                material,
            )
        };
        scene
            .meshes
            .push(panel(-4.0, [1.0, 0.0, 0.0, 1.0], [1.0, 1.0, emissive]));
        scene
            .meshes
            .push(panel(4.0, [1.0, 1.0, 1.0, 1.0], [1.0, 0.0, 0.0]));
        scene.ambient = [1.0; 3];
        scene.probes = vec![room_probe()];
        scene.probe_world = Some(ProbeWorld::new(scene.clone()));
        scene
    }

    /// The probe relight scales a captured wall's lit part by the live ambient and leaves its emission
    /// alone: dimming the ambient to a quarter dims the lit panel's reflection to a quarter and
    /// leaves the emissive one's where it was.
    #[test]
    fn needs_gpu_probe_emissive_ignores_the_ambient_ratio() {
        let Some(r) = renderer_or_skip("probe emissive ignores the ambient ratio") else {
            return;
        };
        let size = FrameSize::native(64, 64);
        let mut scene = panel_scene(2.0);
        // Each panel's reflection in the floor, at the point where the mirrored ray meets y = 0.
        let (emissive_at, lit_at) = (Vec3::new(-1.6, 0.0, 0.0), Vec3::new(1.6, 0.0, 0.0));
        let before = r.render(&scene, size).expect("frozen ambient");
        assert!(
            r.probes.cubes().expect("a probe").emissive.is_some(),
            "a probe that sees an emissive panel stores its emissive part"
        );
        scene.ambient = [0.25; 3];
        let after = r.render(&scene, size).expect("dimmed ambient");
        assert_eq!(
            r.probe_resolve().resolve,
            1,
            "the ambient recaptured the probe"
        );
        // The same scene with the emission off: the metal panel keeps a lit part, so the emission is
        // what the full scene adds on top of it.
        let mut dark = panel_scene(0.0);
        let dark_before = r.render(&dark, size).expect("no emission");
        assert!(
            r.probes.cubes().expect("a probe").emissive.is_none(),
            "a probe set without emissive allocates no emissive cubes"
        );
        dark.ambient = [0.25; 3];
        let dark_after = r.render(&dark, size).expect("no emission, dimmed");
        let red = |image: &RenderedImage, scene: &RenderScene, at| {
            srgb_to_linear(view_pixel(image, scene, at)[0])
        };
        let emission_before =
            red(&before, &scene, emissive_at) - red(&dark_before, &dark, emissive_at);
        let emission_after =
            red(&after, &scene, emissive_at) - red(&dark_after, &dark, emissive_at);
        eprintln!(
            "emissive panel {:?} -> {:?} (lit part alone {:?} -> {:?}), lit panel {:?} -> {:?}",
            view_pixel(&before, &scene, emissive_at),
            view_pixel(&after, &scene, emissive_at),
            view_pixel(&dark_before, &dark, emissive_at),
            view_pixel(&dark_after, &dark, emissive_at),
            view_pixel(&before, &scene, lit_at),
            view_pixel(&after, &scene, lit_at),
        );
        assert!(
            emission_before > 0.1,
            "no emission to compare: {emission_before}"
        );
        assert!(
            (emission_after / emission_before - 1.0).abs() < 0.03,
            "the emission followed the ambient: {emission_before} -> {emission_after}"
        );
        let scale = red(&after, &scene, lit_at) / red(&before, &scene, lit_at);
        assert!(
            (scale - 0.25).abs() < 0.05,
            "the lit part scaled by {scale}"
        );
    }

    /// A probe that captured nothing but sky shades like no probe: its sky fraction is the live sky.
    #[test]
    fn needs_gpu_a_sky_only_probe_matches_no_probe() {
        let Some(r) = renderer_or_skip("a sky-only probe over a glossy floor") else {
            return;
        };
        let size = FrameSize::native(64, 64);
        let mut scene = room_scene();
        scene.meshes.truncate(1);
        let bare = r.render(&scene, size).expect("render without a probe");
        let mut empty_world = scene.clone();
        empty_world.meshes.clear();
        scene.probes = vec![room_probe()];
        scene.probe_world = Some(ProbeWorld::new(empty_world));
        let probed = r
            .render(&scene, size)
            .expect("render with a sky-only probe");
        let worst = bare
            .rgba
            .iter()
            .zip(&probed.rgba)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 3, "a sky-only probe moved a channel by {worst}");
    }
}
