//! The GPU cull kernel against its CPU oracle: random instances, buckets and views, read back.

use ludion_core::math::glam::{Quat, Vec4};
use wgpu::util::DeviceExt;

use super::skin_bounds::bounded_skin_quad;
use super::*;
use crate::cull::{
    Aabb, CullVolume, Frustum, LodView, bounds_table, cull, instance_bounds, instance_levels,
    skinned_lod_radius,
};
use crate::gpu_cull::{
    Chain, CullFrame, CullInput, CullPipelines, CullTargets, CullViewSpec, cull_item, upload_items,
};
use crate::graph::CullShape;
use crate::shader_gen::{CullBucket, CullItem};

const INSTANCE_BYTES: usize = std::mem::size_of::<MeshInstanceRaw>();
const VIEWS: usize = 3;
/// The view that admits casters only, as every shadow view does.
const CASTERS_ONLY_VIEW: usize = 2;

struct Lcg(u64);

impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }

    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next_u32() as f32 / (1u64 << 31) as f32) * (hi - lo)
    }

    fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }

    fn vec3(&mut self, lo: f32, hi: f32) -> Vec3 {
        Vec3::new(self.range(lo, hi), self.range(lo, hi), self.range(lo, hi))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Transform,
    Override,
    Always,
}

/// Cube, plane and sphere buckets, then a rigid asset, a skinned asset and an asset of unknown bounds.
const SHAPES: [MeshShape; 6] = [
    MeshShape::Cube,
    MeshShape::Plane,
    MeshShape::Sphere,
    MeshShape::Asset(0),
    MeshShape::Asset(1),
    MeshShape::Asset(2),
];

fn assets() -> Vec<AssetBounds> {
    let rigid = AssetBounds {
        local: Some(Aabb {
            min: Vec3::new(-1.0, -0.25, -2.0),
            max: Vec3::new(1.5, 0.75, 0.5),
        }),
        skin: None,
    };
    let skinned = AssetBounds::of_mesh(&bounded_skin_quad(0xC0_11_01));
    assert!(skinned.skin.is_some());
    vec![rigid, skinned, AssetBounds::default()]
}

fn random_model(rng: &mut Lcg) -> Mat4 {
    let axis = rng.vec3(-1.0, 1.0);
    let rotation = Quat::from_axis_angle(
        if axis.length() > 0.1 {
            axis.normalize()
        } else {
            Vec3::Y
        },
        rng.range(0.0, std::f32::consts::TAU),
    );
    let mut scale = rng.vec3(0.3, 3.0);
    if rng.below(4) == 0 {
        scale.x = -scale.x;
    }
    Mat4::from_scale_rotation_translation(scale, rotation, rng.vec3(-40.0, 40.0))
}

fn random_view(rng: &mut Lcg) -> Frustum {
    loop {
        let eye = rng.vec3(-40.0, 40.0);
        let target = eye + rng.vec3(-20.0, 20.0) + Vec3::new(0.0, 0.0, 0.5);
        let view = Mat4::look_at_rh(eye, target, Vec3::Y);
        let proj = if rng.below(2) == 0 {
            Mat4::perspective_rh(rng.range(0.5, 1.4), rng.range(0.8, 2.0), 0.1, 60.0)
        } else {
            let (w, h) = (rng.range(5.0, 25.0), rng.range(5.0, 25.0));
            Mat4::orthographic_rh(-w, w, -h, h, 0.1, 60.0)
        };
        let frustum = Frustum::from_view_proj(proj * view);
        if frustum.planes().iter().all(|p| p.is_finite()) {
            return frustum;
        }
    }
}

/// Whether a box lies so close to a plane of `frustum` that FMA rounding could flip the test.
fn near_a_plane(frustum: &Frustum, b: &Aabb) -> bool {
    frustum.planes().iter().any(|p: &Vec4| {
        let n = p.truncate();
        let far = Vec3::new(
            if n.x >= 0.0 { b.max.x } else { b.min.x },
            if n.y >= 0.0 { b.max.y } else { b.min.y },
            if n.z >= 0.0 { b.max.z } else { b.min.z },
        );
        let d = n.dot(far) + p.w;
        d.abs() < 1e-3 * (1.0 + d.abs() + p.w.abs())
    })
}

struct Fixture {
    meshes: Vec<MeshInstance>,
    kinds: Vec<Kind>,
    raws: Vec<MeshInstanceRaw>,
    palette: Vec<Mat4>,
    /// `(first, count)` per bucket, in `SHAPES` order.
    buckets: Vec<(u32, u32)>,
    views: Vec<Frustum>,
}

fn fixture(seed: u64, n: usize) -> Fixture {
    let assets = assets();
    let mut rng = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let views: Vec<Frustum> = (0..VIEWS).map(|_| random_view(&mut rng)).collect();
    let mut counts = [0usize; 6];
    for _ in 0..n {
        counts[rng.below(6) as usize] += 1;
    }
    let mut fx = Fixture {
        meshes: Vec::new(),
        kinds: Vec::new(),
        raws: Vec::new(),
        palette: Vec::new(),
        buckets: Vec::new(),
        views,
    };
    for (b, &shape) in SHAPES.iter().enumerate() {
        fx.buckets.push((fx.meshes.len() as u32, counts[b] as u32));
        for _ in 0..counts[b] {
            loop {
                let model = random_model(&mut rng);
                let mut mesh = skin_instance(model, 0);
                mesh.shape = shape;
                mesh.cast_shadows = rng.below(4) != 0;
                let mut pose = None;
                let kind = match shape {
                    MeshShape::Asset(1) if rng.below(8) == 0 => {
                        // A palette slice that does not exist: the oracle cannot bound it.
                        mesh.bone_offset = 100_000;
                        Kind::Always
                    }
                    MeshShape::Asset(1) => {
                        mesh.bone_offset = 1;
                        pose = Some(random_model(&mut rng));
                        Kind::Override
                    }
                    MeshShape::Asset(2) => Kind::Always,
                    _ => Kind::Transform,
                };
                let bound =
                    instance_bounds(std::slice::from_ref(&mesh), &assets, pose.as_slice())[0];
                assert_eq!(bound.is_none(), kind == Kind::Always, "{shape:?}");
                if bound.is_some_and(|b| fx.views.iter().any(|f| near_a_plane(f, &b))) {
                    continue;
                }
                if let Some(joint) = pose {
                    mesh.bone_offset = 1 + fx.palette.len() as u32;
                    fx.palette.push(joint);
                }
                let mut raw: MeshInstanceRaw = bytemuck::Zeroable::zeroed();
                raw.model = model.to_cols_array_2d();
                raw.color = [fx.meshes.len() as f32, b as f32, 0.0, 1.0];
                fx.raws.push(raw);
                fx.meshes.push(mesh);
                fx.kinds.push(kind);
                break;
            }
        }
    }
    fx
}

fn read_back(renderer: &Renderer, buffer: &wgpu::Buffer) -> Vec<u8> {
    let device = renderer.gpu.device();
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: buffer.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
    renderer.gpu.queue().submit(Some(encoder.finish()));
    let (tx, rx) = std::sync::mpsc::channel();
    staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the device polls");
    rx.recv().unwrap().expect("the buffer maps");
    staging
        .slice(..)
        .get_mapped_range()
        .expect("mapped range")
        .to_vec()
}

/// Runs the kernel on fresh buffers and returns `(compacted instances, args words)`.
fn run_cull(
    renderer: &Renderer,
    fx: &Fixture,
    assets: &[AssetBounds],
    live: usize,
) -> (Vec<u8>, Vec<u32>) {
    let device = renderer.gpu.device();
    let bounds = instance_bounds(&fx.meshes, assets, &fx.palette);
    let items: Vec<CullItem> = fx
        .meshes
        .iter()
        .zip(&bounds)
        .zip(&fx.raws)
        .map(|((mesh, bound), raw)| {
            cull_item(
                mesh,
                raw.color[1] as u32,
                *bound,
                skinned_lod_radius(mesh, assets),
            )
        })
        .collect();
    let buckets: Vec<CullBucket> = fx
        .buckets
        .iter()
        .enumerate()
        .map(|(b, &(first, count))| CullBucket {
            first,
            count,
            elements: 100 + b as u32 * 7,
            levels: 1,
            screen: [0.0; 4],
            level_elements: [100 + b as u32 * 7, 0, 0, 0],
        })
        .collect();
    let shape = CullShape::for_counts(fx.meshes.len(), buckets.len(), VIEWS, 1);
    let views: Vec<CullViewSpec> = fx
        .views
        .iter()
        .enumerate()
        .map(|(v, &frustum)| CullViewSpec {
            frustum,
            casters_only: v == CASTERS_ONLY_VIEW,
            test_frustum: true,
            lod: crate::cull::LodView {
                eye: Vec3::ZERO,
                k: 1.0,
                ortho: false,
                offset: 0,
            },
        })
        .take(live)
        .collect();
    let storage = |bytes: &[u8]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytes,
            usage: wgpu::BufferUsages::STORAGE,
        })
    };
    let instances = storage(bytemuck::cast_slice(&fx.raws));
    let table = storage(bytemuck::cast_slice(&bounds_table(assets)));
    let target = |size: u64| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    };
    let (scratch, out, args) = (
        target(shape.scratch_bytes()),
        target(shape.instance_bytes()),
        target(shape.args_bytes()),
    );
    let pipelines = CullPipelines::new(device);
    let frame = CullFrame::prepare(
        device,
        &pipelines,
        CullInput {
            instances: &instances,
            items: &upload_items(device, &items),
            item_count: items.len() as u32,
            buckets: &buckets,
            views: &views,
            bounds_table: &table,
            shape,
        },
    );
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("cull test"),
            timestamp_writes: None,
        });
        frame.encode(
            &pipelines,
            &mut pass,
            CullTargets {
                scratch: &scratch,
                instances: &out,
                args: &args,
            },
        );
    }
    renderer.gpu.queue().submit(Some(encoder.finish()));
    let args = read_back(renderer, &args)
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .collect();
    (read_back(renderer, &out), args)
}

/// **The GPU cull is the CPU oracle, compacted stably.** Per seed and live view: every args slot is
/// `[elements, visible, 0, 0, 0]`, and each bucket's compacted range is the oracle's survivors in
/// input order, byte for byte; a casters-only view also drops every non-caster. A second run on fresh
/// buffers reads back the same bytes, and one seed runs fewer live views than the shape holds.
#[test]
fn needs_gpu_the_cull_pass_matches_the_cpu_oracle_and_compacts_in_input_order() {
    let Some(renderer) = skip_or_renderer("the_cull_pass_matches_the_cpu_oracle") else {
        return;
    };
    let assets = assets();
    let sizes = [1usize, 31, 32, 33, 255, 256, 257, 3000];
    let mut seen = std::collections::BTreeSet::new();
    let mut flag_culled = false;
    for seed in 0..34u64 {
        let n = match seed {
            0..=7 => sizes[seed as usize],
            32 => 70_000,
            _ => 1 + (seed as usize * 211) % 2000,
        };
        let fx = fixture(seed, n);
        let shape = CullShape::for_counts(n, SHAPES.len(), VIEWS, 1);
        let live = if seed == 33 { VIEWS - 1 } else { VIEWS };
        let (out, args) = run_cull(&renderer, &fx, &assets, live);
        let bounds = instance_bounds(&fx.meshes, &assets, &fx.palette);
        for (v, frustum) in fx.views.iter().enumerate().take(live) {
            let mut mask = vec![false; n];
            for i in cull(&bounds, &CullVolume::Frustum(*frustum)) {
                mask[i as usize] = true;
            }
            for (i, kind) in fx.kinds.iter().enumerate() {
                seen.insert((*kind as u8, mask[i]));
                if v == CASTERS_ONLY_VIEW && !fx.meshes[i].cast_shadows {
                    flag_culled |= mask[i];
                    mask[i] = false;
                }
            }
            for (b, &(first, count)) in fx.buckets.iter().enumerate() {
                // The cull stamps each copy with its bucket-local source index + 1 in the medium
                // lane's spare `y`; nothing else about the instance changes.
                let survivors: Vec<MeshInstanceRaw> = (first..first + count)
                    .filter(|&i| mask[i as usize])
                    .map(|i| {
                        let raw = fx.raws[i as usize];
                        MeshInstanceRaw {
                            medium: [raw.medium[0], (i - first + 1) as f32, 0.0, 0.0],
                            ..raw
                        }
                    })
                    .collect();
                let slot = (v * shape.buckets as usize + b) * 5;
                assert_eq!(
                    args[slot..slot + 5],
                    [100 + b as u32 * 7, survivors.len() as u32, 0, 0, 0],
                    "seed {seed} n {n} view {v} bucket {b}: args"
                );
                let start = (v * shape.instances as usize + first as usize) * INSTANCE_BYTES;
                let got = &out[start..start + survivors.len() * INSTANCE_BYTES];
                let want: Vec<u8> = survivors
                    .iter()
                    .flat_map(|r| bytemuck::bytes_of(r).iter().copied())
                    .collect();
                assert!(
                    got == want,
                    "seed {seed} n {n} view {v} bucket {b}: compacted instances differ"
                );
            }
        }
        let (again_out, again_args) = run_cull(&renderer, &fx, &assets, live);
        assert!(
            again_out == out && again_args == args,
            "seed {seed}: a rerun reads back different bytes"
        );
    }
    for kind in [Kind::Transform, Kind::Override] {
        for visible in [true, false] {
            assert!(
                seen.contains(&(kind as u8, visible)),
                "{kind:?} never came out visible={visible}"
            );
        }
    }
    assert!(seen.contains(&(Kind::Always as u8, true)));
    assert!(
        flag_culled,
        "no non-caster the frustum admits was culled by the casters-only view"
    );
}

/// The on/off fixture: primitives, a rigid asset, a skinned asset, mirrored copies, a viewmodel gun,
/// AO and a sun. A row of instances spreads far past the frustum so about half are off-screen.
fn onoff_scene(size: (u32, u32)) -> RenderScene {
    let mut scene = fog_scene(size.0, size.1);
    scene.meshes.clear();
    scene.post = Some(PostData {
        ssao: 1.0,
        dither: false,
        ..PostData::default()
    });
    scene.sun = Some(test_sun(
        Vec3::new(0.4, -1.0, 0.3),
        Vec3::new(3.0, 3.0, 3.0),
        0.002,
    ));
    scene.mesh_assets = vec![
        Arc::new(skin_quad([1.0, 0.0, 0.0, 0.0], [0, 0, 0, 0], 0xC0_11_10)),
        Arc::new(bounded_skin_quad(0xC0_11_11)),
    ];
    scene.bone_palette = vec![Mat4::IDENTITY];
    let shapes = [
        MeshShape::Cube,
        MeshShape::Plane,
        MeshShape::Sphere,
        MeshShape::Asset(0),
        MeshShape::Asset(1),
    ];
    for i in 0..40usize {
        let x = -24.0 + 1.25 * i as f32;
        let z = -10.0 - 1.5 * (i % 4) as f32;
        let shape = shapes[i % shapes.len()];
        let mirror = if i % 7 == 3 { -1.0 } else { 1.0 };
        let mut mesh = slab(Vec3::ZERO, 1.0, [0.4, 0.3 + 0.01 * i as f32, 0.2]);
        mesh.shape = shape;
        mesh.model = Mat4::from_translation(Vec3::new(x, -1.0 + 0.5 * (i % 3) as f32, z))
            * Mat4::from_scale(Vec3::new(mirror, 1.0, 1.0));
        if shape == MeshShape::Asset(1) {
            mesh.bone_offset = 1;
        }
        scene.meshes.push(mesh);
    }
    super::viewmodel::add_gun(&mut scene);
    scene
}

pub(super) fn visible_world(scene: &RenderScene) -> (usize, usize) {
    let assets: Vec<AssetBounds> = scene
        .mesh_assets
        .iter()
        .map(|m| AssetBounds::of_mesh(m))
        .collect();
    let bounds = instance_bounds(&scene.meshes, &assets, &scene.bone_palette);
    let frustum = Frustum::from_view_proj(scene.depth_view_proj());
    let visible = cull(&bounds, &CullVolume::Frustum(frustum));
    let world = scene.meshes.iter().filter(|m| !m.viewmodel).count();
    let seen = visible
        .iter()
        .filter(|&&i| !scene.meshes[i as usize].viewmodel)
        .count();
    (seen, world)
}

fn second_camera(scene: &mut RenderScene) {
    scene.camera.view = Mat4::look_at_rh(
        Vec3::new(-6.0, 0.0, 0.0),
        Vec3::new(2.0, 0.0, -12.0),
        Vec3::Y,
    );
    scene.camera.position = Vec3::new(-6.0, 0.0, 0.0);
}

/// **Culling on and off render the same bytes.** At two cameras, on one renderer, the frame with the
/// GPU cull scheduled equals the direct-draw frame; the oracle confirms each camera really culls.
#[test]
fn needs_gpu_frustum_culling_on_and_off_render_identical_bytes() {
    let Some(mut renderer) = skip_or_renderer("frustum_culling_on_and_off_identical") else {
        return;
    };
    let size = FrameSize::native(96, 96);
    let mut near = onoff_scene((96, 96));
    near.camera.projection = Mat4::perspective_rh(
        60.0_f32.to_radians(),
        crate::scene::aspect_ratio((96, 96)),
        0.1,
        1000.0,
    );
    let mut far = near.clone();
    second_camera(&mut far);
    for (name, scene) in [("origin", &near), ("side", &far)] {
        let (seen, world) = visible_world(scene);
        assert!(
            seen > 0 && seen < world,
            "{name}: {seen} of {world} visible"
        );
        renderer.set_frustum_culling(true);
        assert!(renderer.frame_chain(scene).cull.is_some());
        let on = renderer.render(scene, size).expect("culled render");
        renderer.set_frustum_culling(false);
        assert!(renderer.frame_chain(scene).cull.is_none());
        let off = renderer.render(scene, size).expect("direct render");
        assert_eq!(on.rgba, off.rgba, "{name}: culling changed the image");
    }
}

/// The culled render of the on/off fixture is stable over 20 repeats.
#[test]
fn needs_gpu_the_culled_render_repeats_identically() {
    let Some(renderer) = skip_or_renderer("the_culled_render_repeats_identically") else {
        return;
    };
    let size = FrameSize::native(96, 96);
    let scene = onoff_scene((96, 96));
    let first = renderer.render(&scene, size).expect("first render");
    for run in 1..20 {
        let again = renderer.render(&scene, size).expect("repeat render");
        assert_eq!(first.rgba, again.rgba, "repeat {run} differs");
    }
}

/// The graph follows the switch: the chain schedules the cull exactly when it is on and a world mesh
/// exists, and a viewmodel-only scene has none.
#[test]
fn needs_gpu_the_frame_chain_follows_the_culling_switch() {
    let Some(mut renderer) = skip_or_renderer("the_frame_chain_follows_the_culling_switch") else {
        return;
    };
    let scene = onoff_scene((64, 64));
    assert!(renderer.frustum_culling());
    assert!(renderer.frame_chain(&scene).cull.is_some());
    renderer.set_frustum_culling(false);
    assert!(renderer.frame_chain(&scene).cull.is_none());
    renderer.set_frustum_culling(true);
    let mut gun_only = scene.clone();
    gun_only.meshes.retain(|m| m.viewmodel);
    assert!(renderer.frame_chain(&gun_only).cull.is_none());
}

pub(super) fn casting_light() -> Option<crate::scene::LocalShadowData> {
    Some(crate::scene::LocalShadowData {
        bias: 0.05,
        near: 0.1,
        distance: None,
        fade: 0.0,
    })
}

/// The shadow fixture: the on/off row (primitives, a rigid and a skinned asset, mirrored copies, a
/// gun) over a ground, with a shadowed spot and a shadowed point light, and a sun that is either the
/// one anchored map or three cascades. One caster inside the point light's range casts no shadow.
fn shadow_scene(cascades: bool) -> RenderScene {
    let mut scene = onoff_scene((96, 96));
    scene.camera.projection = Mat4::perspective_rh(
        60.0_f32.to_radians(),
        crate::scene::aspect_ratio((96, 96)),
        0.1,
        1000.0,
    );
    scene.meshes.push(ground(80.0));
    scene.meshes[10].cast_shadows = false;
    let sun = scene.sun.as_mut().unwrap();
    if cascades {
        sun.cascades = Some(crate::scene::SunCascadeConfig {
            count: 3,
            shadow_distance: 40.0,
            splits: None,
            shadow_near: -60.0,
            shadow_far: 60.0,
        });
    }
    let mut spot = spot_light(
        Vec3::new(2.0, 6.0, -12.0),
        Vec3::NEG_Y,
        30.0,
        14.0,
        20.0,
        40.0,
    );
    spot.shadow = casting_light();
    scene.spot_lights.push(spot);
    let mut point = point_light(Vec3::new(-11.0, 2.0, -13.0), 30.0, 9.0);
    point.shadow = casting_light();
    scene.lights.push(point);
    scene
}

/// The oracle's culled counts for a prepared frame: the most casters any cascade drops, and the
/// most any local view drops, so a test can prove it exercises both.
fn shadow_culls(renderer: &Renderer, scene: &RenderScene) -> (usize, usize) {
    let recorder = renderer
        .prepare_frame(scene, FrameSize::native(96, 96), None)
        .expect("prepares");
    let assets: Vec<AssetBounds> = scene
        .mesh_assets
        .iter()
        .map(|m| AssetBounds::of_mesh(m))
        .collect();
    let bounds = instance_bounds(&scene.meshes, &assets, &scene.bone_palette);
    let dropped = |kept: usize| scene.meshes.len() - kept;
    let cascade = recorder.sun_cascades.as_ref().map_or(0, |fit| {
        crate::sun_cascade::sun_cascade_casters(fit, &bounds)
            .iter()
            .map(|kept| dropped(kept.len()))
            .max()
            .unwrap_or(0)
    });
    let local = recorder.local_shadow.as_ref().map_or(0, |plan| {
        plan.views
            .iter()
            .map(|v| dropped(cull(&bounds, &v.cull).len()))
            .max()
            .unwrap_or(0)
    });
    (cascade, local)
}

/// **Shadow views cull on the GPU without changing a byte.** A legacy sun and three cascades, each
/// with a spot and a point light, at two cameras: the frame with the one `cull` node feeding the
/// shadow passes equals the direct-draw frame. The oracle confirms a cascade view and a local view
/// each really drop a caster, and the graph confirms the cull spans the camera, sun and local views.
#[test]
fn needs_gpu_shadow_culling_on_and_off_render_identical_bytes() {
    let Some(mut renderer) = skip_or_renderer("shadow_culling_on_and_off_identical") else {
        return;
    };
    let size = FrameSize::native(96, 96);
    for cascades in [false, true] {
        let near = shadow_scene(cascades);
        let mut far = near.clone();
        second_camera(&mut far);
        for (name, scene) in [("origin", &near), ("side", &far)] {
            let label = format!("cascades {cascades} camera {name}");
            let (cascade_drops, local_drops) = shadow_culls(&renderer, scene);
            assert!(local_drops > 0, "{label}: no local view culls anything");
            assert!(
                !cascades || cascade_drops > 0,
                "{label}: no cascade culls anything"
            );
            renderer.set_frustum_culling(true);
            let shape = renderer.frame_chain(scene).cull.expect("culled");
            let local = renderer
                .prepare_frame(scene, size, None)
                .unwrap()
                .local_shadow
                .map_or(0, |plan| plan.views.len());
            assert_eq!(
                shape.views as usize,
                1 + if cascades { 3 } else { 1 } + local,
                "{label}"
            );
            assert_eq!(renderer.cull_over_cap_views(), 0, "{label}");
            let on = renderer.render(scene, size).expect("culled render");
            renderer.set_frustum_culling(false);
            let off = renderer.render(scene, size).expect("direct render");
            assert_eq!(renderer.cull_over_cap_views(), 0);
            assert_eq!(on.rgba, off.rgba, "{label}: culling changed the image");
        }
    }
}

/// The culled shadow render is stable over 20 repeats, for both sun shapes.
#[test]
fn needs_gpu_the_culled_shadow_render_repeats_identically() {
    let Some(renderer) = skip_or_renderer("the_culled_shadow_render_repeats_identically") else {
        return;
    };
    let size = FrameSize::native(96, 96);
    for cascades in [false, true] {
        let scene = shadow_scene(cascades);
        let first = renderer.render(&scene, size).expect("first render");
        for run in 1..20 {
            let again = renderer.render(&scene, size).expect("repeat render");
            assert_eq!(
                first.rgba, again.rgba,
                "cascades {cascades}: repeat {run} differs"
            );
        }
    }
}

/// **Crossing the capacity cap changes no byte.** More than 16 384 small instances make each view's
/// slice of the compacted buffer large, and ten shadowed point lights ask for sixty local views, so
/// the views requested exceed what one 128 MiB storage binding holds. The shape clamps, the frame
/// reports exactly the views past it, and the views past the cap draw directly: the image equals the
/// direct-draw frame.
#[test]
fn needs_gpu_views_past_the_cull_capacity_draw_directly_and_change_nothing() {
    let Some(mut renderer) = skip_or_renderer("views_past_the_cull_capacity") else {
        return;
    };
    let size = FrameSize::native(96, 96);
    let mut scene = shadow_scene(false);
    scene.meshes.clear();
    scene.meshes.push(ground(80.0));
    for i in 0..16_384usize {
        let (x, z) = (
            (i % 128) as f32 * 0.25 - 16.0,
            -8.0 - (i / 128) as f32 * 0.25,
        );
        scene
            .meshes
            .push(slab(Vec3::new(x, -0.9, z), 0.15, [0.4, 0.3, 0.2]));
    }
    scene.spot_lights.clear();
    scene.lights.clear();
    for i in 0..10usize {
        let mut point = point_light(
            Vec3::new(-9.0 + 2.0 * i as f32, 0.5, -10.0 - (i % 3) as f32),
            20.0,
            4.0,
        );
        point.shadow = casting_light();
        scene.lights.push(point);
    }
    renderer.set_frustum_culling(true);
    let chain = renderer.frame_chain(&scene);
    let shape = chain.cull.expect("culled");
    let requested = 1
        + 1
        + renderer
            .prepare_frame(&scene, size, None)
            .unwrap()
            .local_shadow
            .map_or(0, |plan| plan.views.len());
    assert!(
        requested > shape.views as usize,
        "the scene must ask for more views ({requested}) than the cap holds ({})",
        shape.views
    );
    assert!(shape.instance_bytes() <= 128 << 20);
    let on = renderer.render(&scene, size).expect("capped render");
    assert_eq!(
        renderer.cull_over_cap_views() as usize,
        requested - shape.views as usize
    );
    renderer.set_frustum_culling(false);
    let off = renderer.render(&scene, size).expect("direct render");
    assert_eq!(on.rgba, off.rgba, "the cap changed the image");

    // The same scene with its first half marked static reads the same bytes through the static buffers.
    renderer.set_frustum_culling(true);
    let mut marked = scene.clone();
    marked.statics = crate::scene::StaticRange::of(&scene.meshes[..8192], &[]);
    let with_statics = renderer
        .render(&marked, size)
        .expect("capped static render");
    assert_eq!(
        on.rgba, with_statics.rgba,
        "statics changed the capped image"
    );
}

/// The oracle's `(tested, kept)` for a view that `volume` bounds: the casters-only views test only
/// the world meshes that cast, the camera view every world mesh.
pub(super) fn oracle_counts(
    scene: &RenderScene,
    volume: &CullVolume,
    casters_only: bool,
) -> (u32, u32) {
    let assets: Vec<AssetBounds> = scene
        .mesh_assets
        .iter()
        .map(|m| AssetBounds::of_mesh(m))
        .collect();
    let bounds = instance_bounds(&scene.meshes, &assets, &scene.bone_palette);
    let tested =
        |i: usize| !scene.meshes[i].viewmodel && (!casters_only || scene.meshes[i].cast_shadows);
    let total = (0..scene.meshes.len()).filter(|&i| tested(i)).count() as u32;
    let kept = cull(&bounds, volume)
        .iter()
        .filter(|&&i| tested(i as usize))
        .count() as u32;
    (total, kept)
}

/// **The profile's counts are the oracle's.** With the ring armed, one render yields one sample whose
/// camera entry tests every world mesh and draws what the CPU cull keeps; with a sun and its shadowed
/// lights the entries follow the cull's view order and each shadow view tests the casters and draws
/// what its own volume keeps.
#[test]
fn needs_gpu_cull_stats_match_the_oracle() {
    let Some(renderer) = skip_or_renderer("cull_stats_match_the_oracle") else {
        return;
    };
    let size = FrameSize::native(96, 96);
    renderer.set_cull_stats(true);

    let mut scene = onoff_scene((96, 96));
    scene.camera.projection = Mat4::perspective_rh(
        60.0_f32.to_radians(),
        crate::scene::aspect_ratio((96, 96)),
        0.1,
        1000.0,
    );
    renderer.render(&scene, size).expect("render");
    let samples = renderer.collect_cull_stats(true);
    assert_eq!(samples.len(), 1);
    let camera = &samples[0][0];
    let (seen, world) = visible_world(&scene);
    assert_eq!(camera.name, "camera");
    assert_eq!(
        (camera.instances as usize, camera.drawn as usize),
        (world, seen)
    );

    for cascades in [false, true] {
        let scene = shadow_scene(cascades);
        renderer.render(&scene, size).expect("render");
        let samples = renderer.collect_cull_stats(true);
        assert_eq!(samples.len(), 1, "cascades {cascades}");
        let views = &samples[0];
        let recorder = renderer.prepare_frame(&scene, size, None).unwrap();
        let plan = recorder.local_shadow.as_ref().expect("local views");
        let suns = if cascades { 3 } else { 1 };
        assert_eq!(
            views.len(),
            1 + suns + plan.views.len(),
            "cascades {cascades}"
        );
        let (seen, world) = visible_world(&scene);
        assert_eq!(
            (
                views[0].name.as_str(),
                views[0].instances as usize,
                views[0].drawn as usize
            ),
            ("camera", world, seen)
        );
        for (i, view) in views[1..1 + suns].iter().enumerate() {
            let expected = if cascades {
                format!("cascade_{i}")
            } else {
                "sun".into()
            };
            assert_eq!(view.name, expected);
            let casters = scene
                .meshes
                .iter()
                .filter(|m| !m.viewmodel && m.cast_shadows)
                .count() as u32;
            assert_eq!(view.instances, casters, "{expected} tests the casters");
            assert!(view.drawn <= view.instances, "{expected}");
        }
        for (i, (view, planned)) in views[1 + suns..].iter().zip(&plan.views).enumerate() {
            assert_eq!(view.name, format!("local_{i}"));
            let (tested, kept) = oracle_counts(&scene, &planned.cull, true);
            assert_eq!((view.instances, view.drawn), (tested, kept), "local_{i}");
        }
    }
}

/// **A full ring skips rather than blocks.** Five frames rendered without collecting leave at most
/// three samples, all of the same scene so all equal; a disarmed renderer collects nothing.
#[test]
fn needs_gpu_cull_stats_skip_rather_than_block() {
    let Some(renderer) = skip_or_renderer("cull_stats_skip_rather_than_block") else {
        return;
    };
    let size = FrameSize::native(96, 96);
    let scene = onoff_scene((96, 96));
    renderer.render(&scene, size).expect("render");
    assert!(renderer.collect_cull_stats(true).is_empty(), "disarmed");

    renderer.set_cull_stats(true);
    for _ in 0..5 {
        renderer.render(&scene, size).expect("render");
    }
    let samples = renderer.collect_cull_stats(true);
    assert!(
        (1..=3).contains(&samples.len()),
        "{} samples",
        samples.len()
    );
    assert!(samples.windows(2).all(|pair| pair[0] == pair[1]));
    renderer.set_cull_stats(false);
    assert!(renderer.collect_cull_stats(true).is_empty());
}

/// Screen sizes under which levels 1 and 2 of the test chain take over.
const THRESHOLDS: [f32; 2] = [0.3, 0.08];
/// The index counts of the test chain's three levels.
const LEVEL_ELEMENTS: [u32; 3] = [300, 200, 100];

/// A camera at the origin looking down -Z, 60° vertical field of view.
fn lod_camera() -> crate::scene::RenderCamera {
    crate::scene::RenderCamera {
        view: Mat4::look_at_rh(Vec3::ZERO, -Vec3::Z, Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), 1.0, 0.1, 1000.0),
        position: Vec3::ZERO,
    }
}

/// A fan of unit-box instances of one chained asset receding down -Z, at scales that keep every
/// instance's screen size well away from a threshold and spread sideways so none hides another.
fn lod_row(n: usize) -> Vec<MeshInstance> {
    (0..n)
        .map(|i| {
            let mut mesh = slab(Vec3::ZERO, 1.0, [0.5; 3]);
            mesh.shape = MeshShape::Asset(0);
            let scale = 1.0 + 0.25 * (i % 3) as f32;
            let z = -4.0 - 3.1 * i as f32;
            let x = ((i % 5) as f32 - 2.0) * -0.18 * z;
            mesh.model =
                Mat4::from_translation(Vec3::new(x, 0.0, z)) * Mat4::from_scale(Vec3::splat(scale));
            mesh.cast_shadows = i % 5 != 4;
            mesh
        })
        .collect()
}

/// Runs the cull over `meshes` as one chained bucket seen by two views — the camera and a casters-only
/// shadow view whose level offset is `shadow_levels` — and returns `(compacted instances, args words)`.
fn run_lod_cull(
    renderer: &Renderer,
    meshes: &[MeshInstance],
    assets: &[AssetBounds],
    palette: &[Mat4],
    shadow_levels: u32,
    cull_on: bool,
) -> (Vec<u8>, Vec<u32>) {
    let device = renderer.gpu.device();
    let bounds = instance_bounds(meshes, assets, palette);
    let raws: Vec<MeshInstanceRaw> = meshes
        .iter()
        .enumerate()
        .map(|(i, mesh)| {
            let mut raw: MeshInstanceRaw = bytemuck::Zeroable::zeroed();
            raw.model = mesh.model.to_cols_array_2d();
            raw.color = [i as f32, 0.0, 0.0, 1.0];
            raw
        })
        .collect();
    let items: Vec<CullItem> = meshes
        .iter()
        .zip(&bounds)
        .map(|(mesh, bound)| cull_item(mesh, 0, *bound, skinned_lod_radius(mesh, assets)))
        .collect();
    let chain = Chain::of(&THRESHOLDS, &LEVEL_ELEMENTS, 3);
    let buckets = [CullBucket {
        first: 0,
        count: meshes.len() as u32,
        elements: LEVEL_ELEMENTS[0],
        levels: chain.levels,
        screen: [chain.screen[0], chain.screen[1], chain.screen[2], 0.0],
        level_elements: chain.indices,
    }];
    let shape = CullShape::for_counts(meshes.len(), 1, 2, 3);
    let lod = LodView::of_camera(&lod_camera(), 0.0);
    let frustum = Frustum::from_view_proj(lod_camera().projection * lod_camera().view);
    let views = [
        CullViewSpec {
            frustum,
            casters_only: false,
            test_frustum: cull_on,
            lod,
        },
        CullViewSpec {
            frustum,
            casters_only: true,
            test_frustum: cull_on,
            lod: LodView {
                offset: shadow_levels,
                ..lod
            },
        },
    ];
    let storage = |bytes: &[u8]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytes,
            usage: wgpu::BufferUsages::STORAGE,
        })
    };
    let instances = storage(bytemuck::cast_slice(&raws));
    let table = storage(bytemuck::cast_slice(&bounds_table(assets)));
    let target = |size: u64| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    };
    let (scratch, out, args) = (
        target(shape.scratch_bytes()),
        target(shape.instance_bytes()),
        target(shape.args_bytes()),
    );
    let pipelines = CullPipelines::new(device);
    let frame = CullFrame::prepare(
        device,
        &pipelines,
        CullInput {
            instances: &instances,
            items: &upload_items(device, &items),
            item_count: items.len() as u32,
            buckets: &buckets,
            views: &views,
            bounds_table: &table,
            shape,
        },
    );
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("lod cull test"),
            timestamp_writes: None,
        });
        frame.encode(
            &pipelines,
            &mut pass,
            CullTargets {
                scratch: &scratch,
                instances: &out,
                args: &args,
            },
        );
    }
    renderer.gpu.queue().submit(Some(encoder.finish()));
    let args = read_back(renderer, &args)
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        .collect();
    (read_back(renderer, &out), args)
}

/// **The cull selects levels as the oracle does, level-major.** A row of a chained asset receding from
/// the camera: per view and level, the args slot is `[level index count, count, 0, 0, start of the
/// level's run]`, and the bucket's compacted range is the oracle's survivors ordered level 0, 1, 2 with
/// input order inside a level. A shadow view with `shadow_levels = 1` sits one level coarser than the
/// camera's selection, clamped at the last level; with the frustum test off every instance the casters
/// rule keeps is still placed.
#[test]
fn needs_gpu_the_cull_selects_levels_as_the_oracle_and_compacts_level_major() {
    let Some(renderer) = skip_or_renderer("the_cull_selects_levels_as_the_oracle") else {
        return;
    };
    let meshes = lod_row(24);
    let bucket_cap = 16;
    let assets = [AssetBounds {
        local: Some(Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        }),
        skin: None,
    }];
    let bounds = instance_bounds(&meshes, &assets, &[]);
    let camera = LodView::of_camera(&lod_camera(), 0.0);
    let chains = vec![&THRESHOLDS[..]; meshes.len()];
    let radii = vec![0.0; meshes.len()];
    for cull_on in [true, false] {
        let mut camera_counts = [0usize; 3];
        let mut shadow_counts = [0usize; 3];
        for shadow_levels in [0u32, 1] {
            let (out, args) =
                run_lod_cull(&renderer, &meshes, &assets, &[], shadow_levels, cull_on);
            for (v, (casters_only, offset)) in
                [(false, 0), (true, shadow_levels)].into_iter().enumerate()
            {
                let levels =
                    instance_levels(&bounds, &radii, &chains, &LodView { offset, ..camera });
                let mut start = 0u32;
                for level in 0..3u32 {
                    let survivors: Vec<usize> = (0..meshes.len())
                        .filter(|&i| {
                            levels[i] == level && (!casters_only || meshes[i].cast_shadows)
                        })
                        .collect();
                    let lane = v * 3 + level as usize;
                    let slot = lane * bucket_cap * 5;
                    assert_eq!(
                        args[slot..slot + 5],
                        [
                            LEVEL_ELEMENTS[level as usize],
                            survivors.len() as u32,
                            0,
                            0,
                            start
                        ],
                        "cull {cull_on} view {v} offset {offset} level {level}: args"
                    );
                    let base = (v * 256 + start as usize) * INSTANCE_BYTES;
                    for (k, &i) in survivors.iter().enumerate() {
                        let at = base + k * INSTANCE_BYTES;
                        let color: [f32; 4] = *bytemuck::from_bytes(&out[at + 64..at + 80]);
                        assert_eq!(
                            color[0] as usize, i,
                            "cull {cull_on} view {v} level {level} slot {k}"
                        );
                    }
                    if cull_on {
                        if v == 0 {
                            camera_counts[level as usize] = survivors.len();
                        } else if shadow_levels == 1 {
                            shadow_counts[level as usize] = survivors.len();
                        }
                    }
                    start += survivors.len() as u32;
                }
            }
        }
        if cull_on {
            assert!(
                camera_counts.iter().all(|&n| n > 0),
                "the row must exercise every level: {camera_counts:?}"
            );
            assert!(
                shadow_counts[0] == 0 && shadow_counts[2] > camera_counts[2] / 2,
                "shadow_levels = 1 moves the sun view coarser: {shadow_counts:?} vs {camera_counts:?}"
            );
        }
    }
}

/// **A skinned chain sizes its level from the rest box, on the GPU as in the oracle.** A row of
/// skinned, scaled instances whose palette inflates the posed cull box well past the rest box: the
/// kernel's per-view, per-level survivors are the oracle's, which select `lod_level(r·k/d)` for
/// `r` = the rest half-diagonal times the largest axis scale, with the posed box's radius selecting
/// another level for some instance.
#[test]
fn needs_gpu_a_skinned_chain_sizes_its_level_from_the_rest_box_as_the_oracle_does() {
    let Some(renderer) = skip_or_renderer("a_skinned_chain_sizes_its_level_from_the_rest_box")
    else {
        return;
    };
    let assets = [AssetBounds::of_mesh(&bounded_skin_quad(0xC0_11_02))];
    let rest = assets[0].local.unwrap();
    let rest_radius = ((rest.max - rest.min) * 0.5).length();
    let lod = LodView::of_camera(&lod_camera(), 0.0);
    let targets = [0.5, 0.15, 0.04];
    let (mut meshes, mut palette) = (Vec::new(), Vec::new());
    for i in 0..24usize {
        let scale = Vec3::new(1.0, 1.0 + 0.5 * (i % 2) as f32, 1.0 + 0.25 * (i % 3) as f32);
        let r = rest_radius * scale.max_element();
        // Distance at which the rest radius reaches this instance's target screen size.
        let z = -(r * lod.k / (targets[i % 3] * (1.0 + 0.05 * (i / 3) as f32)));
        let x = ((i % 5) as f32 - 2.0) * -0.18 * z;
        let mut mesh = skin_instance(
            Mat4::from_translation(Vec3::new(x, 0.0, z)) * Mat4::from_scale(scale),
            1 + palette.len() as u32,
        );
        mesh.cast_shadows = i % 5 != 4;
        palette.push(Mat4::from_scale(Vec3::splat(2.0 + (i % 4) as f32)));
        meshes.push(mesh);
    }
    let bounds = instance_bounds(&meshes, &assets, &palette);
    let radii: Vec<f32> = meshes
        .iter()
        .map(|m| skinned_lod_radius(m, &assets))
        .collect();
    let max_scale = |m: &MeshInstance| {
        m.model
            .x_axis
            .truncate()
            .length()
            .max(m.model.y_axis.truncate().length())
            .max(m.model.z_axis.truncate().length())
    };
    for (m, &r) in meshes.iter().zip(&radii) {
        assert!((r - rest_radius * max_scale(m)).abs() < 1e-4, "{r}");
    }
    let chains = vec![&THRESHOLDS[..]; meshes.len()];
    let by_box = vec![0.0; meshes.len()];
    let oracle = instance_levels(&bounds, &radii, &chains, &lod);
    assert_ne!(
        oracle,
        instance_levels(&bounds, &by_box, &chains, &lod),
        "the posed box must pick other levels than the rest radius"
    );
    let bucket_cap = 16;
    for cull_on in [true, false] {
        for shadow_levels in [0u32, 1] {
            let (out, args) = run_lod_cull(
                &renderer,
                &meshes,
                &assets,
                &palette,
                shadow_levels,
                cull_on,
            );
            for (v, (casters_only, offset)) in
                [(false, 0), (true, shadow_levels)].into_iter().enumerate()
            {
                let levels = instance_levels(&bounds, &radii, &chains, &LodView { offset, ..lod });
                let mut start = 0u32;
                for level in 0..3u32 {
                    let survivors: Vec<usize> = (0..meshes.len())
                        .filter(|&i| {
                            levels[i] == level && (!casters_only || meshes[i].cast_shadows)
                        })
                        .collect();
                    let slot = (v * 3 + level as usize) * bucket_cap * 5;
                    assert_eq!(
                        args[slot..slot + 5],
                        [
                            LEVEL_ELEMENTS[level as usize],
                            survivors.len() as u32,
                            0,
                            0,
                            start
                        ],
                        "cull {cull_on} view {v} offset {offset} level {level}: args"
                    );
                    let base = (v * 256 + start as usize) * INSTANCE_BYTES;
                    for (k, &i) in survivors.iter().enumerate() {
                        let at = base + k * INSTANCE_BYTES;
                        let model: [[f32; 4]; 4] = *bytemuck::from_bytes(&out[at..at + 64]);
                        assert_eq!(
                            model,
                            meshes[i].model.to_cols_array_2d(),
                            "cull {cull_on} view {v} level {level} slot {k}"
                        );
                    }
                    start += survivors.len() as u32;
                }
            }
        }
    }
}

/// A level mesh: the unit box scaled to `scale`, a distinct asset hash so it uploads on its own.
fn scaled_box(hash: u64, scale: f32) -> MeshData {
    let mut mesh = box_asset(hash, false);
    for p in &mut mesh.positions {
        *p *= scale;
    }
    mesh
}

/// A chained box asset: the full box, then half-size boxes, switching at the test thresholds.
pub(super) fn chained_asset() -> Arc<MeshData> {
    let mut base = box_asset(0x10D_0001, false);
    base.lods = THRESHOLDS
        .iter()
        .enumerate()
        .map(|(k, &screen_size)| crate::scene::MeshLod {
            mesh: Arc::new(scaled_box(0x10D_0002 + k as u64, 0.5)),
            screen_size,
        })
        .collect();
    base.hash = 0x10D_00FF;
    Arc::new(base)
}

fn lod_scene(chained: bool) -> RenderScene {
    let mut scene = fog_scene(96, 96);
    scene.camera.view = lod_camera().view;
    scene.camera.projection = lod_camera().projection;
    scene.camera.position = Vec3::ZERO;
    scene.meshes = lod_row(24);
    scene.sun = Some(test_sun(
        Vec3::new(0.4, -1.0, 0.3),
        Vec3::new(3.0, 3.0, 3.0),
        0.002,
    ));
    let mut asset = chained_asset();
    if !chained {
        let mut flat = (*asset).clone();
        flat.lods.clear();
        asset = Arc::new(flat);
    }
    scene.mesh_assets = vec![asset];
    scene
}

/// **A chain draws its levels, and culling on or off renders the same bytes.** The row recedes through
/// all three levels: the chained frame differs from the same row without a chain, a frame with a
/// chain keeps the cull pass with the frustum test off (`levels` 3 both ways) and renders the same
/// bytes as with it on, and a frame with no chain is `levels == 1`.
#[test]
fn needs_gpu_a_chain_draws_its_levels_and_cull_on_and_off_render_identical_bytes() {
    let Some(mut renderer) = skip_or_renderer("a_chain_draws_its_levels") else {
        return;
    };
    if !renderer.gpu.supports_indirect_first_instance() {
        return;
    }
    let size = FrameSize::native(96, 96);
    let chained = lod_scene(true);
    renderer.set_frustum_culling(true);
    assert_eq!(renderer.frame_chain(&chained).cull.unwrap().levels, 3);
    let on = renderer.render(&chained, size).expect("culled render");
    renderer.set_frustum_culling(false);
    assert_eq!(
        renderer
            .frame_chain(&chained)
            .cull
            .map(|shape| shape.levels),
        Some(3),
        "a chain keeps the pass with the frustum test off"
    );
    let off = renderer.render(&chained, size).expect("unculled render");
    assert_eq!(on.rgba, off.rgba, "culling changed a chained image");

    let flat = lod_scene(false);
    renderer.set_frustum_culling(true);
    let shape = renderer.frame_chain(&flat).cull.unwrap();
    assert_eq!(shape.levels, 1);
    assert_eq!(
        (shape.args_bytes(), shape.scratch_bytes()),
        (
            u64::from(shape.views) * u64::from(shape.buckets) * 20,
            u64::from(shape.views) * u64::from(shape.words() + shape.blocks() + 1) * 4
        ),
        "no chain, no extra lanes"
    );
    renderer.set_frustum_culling(false);
    assert!(renderer.frame_chain(&flat).cull.is_none());
    let level0 = renderer.render(&flat, size).expect("level 0 render");
    assert_ne!(on.rgba, level0.rgba, "the chain never drew a coarser level");
}

/// **A decal's receiver ids follow the levels the scene pass drew.** A decal listing only the chained
/// row paints the same bytes as one listing nobody out: the id redraw lands on the coarse levels' depth
/// too, where a level-0 redraw would miss the half-size boxes and leave them unpainted.
#[test]
fn needs_gpu_decal_receivers_follow_the_selected_level() {
    let Some(renderer) = skip_or_renderer("decal_receivers_follow_the_selected_level") else {
        return;
    };
    if !renderer.gpu.supports_indirect_first_instance() {
        return;
    }
    let size = FrameSize::native(96, 96);
    let scene = lod_scene(true);
    let decal = |receivers| {
        // The decal projects along its box's Y; rotate that onto the row's -Z so the faces front it.
        let box_to_world = Mat4::from_translation(Vec3::new(0.0, 0.0, -40.0))
            * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2)
            * Mat4::from_scale(Vec3::new(400.0, 200.0, 400.0));
        crate::scene::DecalInstance {
            box_to_world,
            world_to_box: box_to_world.inverse(),
            blend: crate::scene::DecalBlend::Multiply,
            color: [0.0, 0.0, 0.0, 1.0],
            opacity: 1.0,
            roughness: None,
            metallic: 0.0,
            ripples: 1.0,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            angle_fade: [60.0, 80.0],
            layer: 0,
            maps: crate::scene::DecalMaps::default(),
            receivers,
        }
    };
    let render = |receivers| {
        let mut scene = scene.clone();
        scene.decals = vec![decal(receivers)];
        renderer.render(&scene, size).expect("decal render")
    };
    let bare = renderer.render(&scene, size).expect("bare render");
    let everyone = render(crate::scene::DecalReceivers::Exclude(Vec::new()));
    let listed = render(crate::scene::DecalReceivers::Only(
        (0..scene.meshes.len() as u32).collect(),
    ));
    assert_ne!(bare.rgba, everyone.rgba, "the decal paints the row");
    assert_eq!(
        listed.rgba, everyone.rgba,
        "a listed chained mesh must claim every pixel its drawn level covers"
    );
}

/// **Without `INDIRECT_FIRST_INSTANCE` a chained frame draws level 0.** With the hook forcing that
/// path, a chained scene renders the bytes of the same scene with its chain removed, culling on and
/// off, and the debug marks tint everything as level 0.
#[test]
fn needs_gpu_a_device_without_first_instance_draws_level_zero() {
    let Some(mut renderer) = skip_or_renderer("a_device_without_first_instance") else {
        return;
    };
    renderer.force_level_zero = true;
    assert!(!renderer.draws_lod_levels());
    let size = FrameSize::native(96, 96);
    let (chained, flat) = (lod_scene(true), lod_scene(false));
    for cull_on in [true, false] {
        renderer.set_frustum_culling(cull_on);
        let chain = renderer.frame_chain(&chained).cull;
        assert!(
            chain.is_none_or(|shape| shape.levels == 1),
            "cull {cull_on}"
        );
        let a = renderer.render(&chained, size).expect("chained render");
        let b = renderer.render(&flat, size).expect("flat render");
        assert_eq!(a.rgba, b.rgba, "cull {cull_on}: a chain must draw level 0");
    }
    let tints = |levels| -> std::collections::BTreeSet<[u32; 4]> {
        crate::cull::cull_marks(&chained, 0.0, levels)
            .iter()
            .map(|s| s.color.map(f32::to_bits))
            .collect()
    };
    assert!(
        tints(true).len() > tints(false).len(),
        "the tint follows the drawn level"
    );
}

/// The shape of a chain-free frame is today's, and a chain adds lanes but not instance bytes.
#[test]
fn a_chain_adds_lanes_never_instance_bytes() {
    let flat = CullShape::for_counts(16_384, 6, 60, 1);
    let chained = CullShape::for_counts(16_384, 6, 60, 4);
    assert_eq!((flat.views, chained.views), (36, 36));
    assert_eq!(chained.instance_bytes(), flat.instance_bytes());
    assert_eq!(chained.args_bytes(), 4 * flat.args_bytes());
    assert_eq!(chained.scratch_bytes(), 4 * flat.scratch_bytes());
    assert_eq!(
        flat.instance_bytes(),
        36 * 16_384 * 224,
        "the 224-byte stride and the 36-view cap at 16384 instances"
    );
}

/// **A chained frame's stats sum a view's lanes.** The camera's drawn count is the oracle's kept count
/// (every level together) and the sun view reads its own lanes, not the camera's.
#[test]
fn needs_gpu_cull_stats_sum_each_views_levels() {
    let Some(renderer) = skip_or_renderer("cull_stats_sum_each_views_levels") else {
        return;
    };
    if !renderer.gpu.supports_indirect_first_instance() {
        return;
    }
    renderer.set_cull_stats(true);
    let scene = lod_scene(true);
    renderer
        .render(&scene, FrameSize::native(96, 96))
        .expect("render");
    let samples = renderer.collect_cull_stats(true);
    assert_eq!(samples.len(), 1);
    let views = &samples[0];
    assert_eq!(views[0].name, "camera");
    let (seen, world) = visible_world(&scene);
    assert_eq!(
        (views[0].instances as usize, views[0].drawn as usize),
        (world, seen)
    );
    let casters = scene.meshes.iter().filter(|m| m.cast_shadows).count();
    assert!(views.len() > 1 && views[1].name == "sun");
    assert_eq!(views[1].instances as usize, casters);
    let assets: Vec<AssetBounds> = scene
        .mesh_assets
        .iter()
        .map(|m| AssetBounds::of_mesh(m))
        .collect();
    let bounds = instance_bounds(&scene.meshes, &assets, &scene.bone_palette);
    let sun = Frustum::from_view_proj(scene.sun.as_ref().unwrap().light_space);
    let kept = cull(&bounds, &CullVolume::Frustum(sun))
        .into_iter()
        .filter(|&i| scene.meshes[i as usize].cast_shadows)
        .count();
    assert!(kept > 0 && kept < casters + 1);
    assert_eq!(
        views[1].drawn as usize, kept,
        "the sun view reads its own lanes"
    );
}
