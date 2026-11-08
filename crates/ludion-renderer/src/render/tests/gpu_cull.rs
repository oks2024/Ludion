//! The GPU cull kernel against its CPU oracle: random instances, buckets and views, read back.

use ludion_core::math::glam::{Quat, Vec4};
use wgpu::util::DeviceExt;

use super::skin_bounds::bounded_skin_quad;
use super::*;
use crate::cull::{Aabb, CullVolume, Frustum, bounds_table, cull, instance_bounds};
use crate::gpu_cull::{CullFrame, CullInput, CullPipelines, CullTargets, cull_item};
use crate::graph::CullShape;
use crate::shader_gen::{CullBucket, CullItem};

const INSTANCE_BYTES: usize = std::mem::size_of::<MeshInstanceRaw>();
const VIEWS: usize = 3;

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
fn run_cull(renderer: &Renderer, fx: &Fixture, assets: &[AssetBounds]) -> (Vec<u8>, Vec<u32>) {
    let device = renderer.gpu.device();
    let bounds = instance_bounds(&fx.meshes, assets, &fx.palette);
    let items: Vec<CullItem> = fx
        .meshes
        .iter()
        .zip(&bounds)
        .zip(&fx.raws)
        .map(|((mesh, bound), raw)| cull_item(mesh, raw.color[1] as u32, *bound))
        .collect();
    let buckets: Vec<CullBucket> = fx
        .buckets
        .iter()
        .enumerate()
        .map(|(b, &(first, count))| CullBucket {
            first,
            count,
            elements: 100 + b as u32 * 7,
            pad0: 0,
        })
        .collect();
    let shape = CullShape::for_counts(fx.meshes.len(), buckets.len(), VIEWS);
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
            items: &items,
            buckets: &buckets,
            views: &fx.views,
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

/// **The GPU cull is the CPU oracle, compacted stably.** Per seed and view: every args slot is
/// `[elements, visible, 0, 0, 0]`, and each bucket's compacted range is the oracle's survivors in
/// input order, byte for byte; a second run on fresh buffers reads back the same bytes.
#[test]
fn needs_gpu_the_cull_pass_matches_the_cpu_oracle_and_compacts_in_input_order() {
    let Some(renderer) = skip_or_renderer("the_cull_pass_matches_the_cpu_oracle") else {
        return;
    };
    let assets = assets();
    let sizes = [1usize, 31, 32, 33, 255, 256, 257, 3000];
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..33u64 {
        let n = match seed {
            0..=7 => sizes[seed as usize],
            32 => 70_000,
            _ => 1 + (seed as usize * 211) % 2000,
        };
        let fx = fixture(seed, n);
        let shape = CullShape::for_counts(n, SHAPES.len(), VIEWS);
        let (out, args) = run_cull(&renderer, &fx, &assets);
        let bounds = instance_bounds(&fx.meshes, &assets, &fx.palette);
        for (v, frustum) in fx.views.iter().enumerate() {
            let mut mask = vec![false; n];
            for i in cull(&bounds, &CullVolume::Frustum(*frustum)) {
                mask[i as usize] = true;
            }
            for (i, kind) in fx.kinds.iter().enumerate() {
                seen.insert((*kind as u8, mask[i]));
            }
            for (b, &(first, count)) in fx.buckets.iter().enumerate() {
                let survivors: Vec<&MeshInstanceRaw> = (first..first + count)
                    .filter(|&i| mask[i as usize])
                    .map(|i| &fx.raws[i as usize])
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
                    .flat_map(|r| bytemuck::bytes_of(*r).iter().copied())
                    .collect();
                assert!(
                    got == want,
                    "seed {seed} n {n} view {v} bucket {b}: compacted instances differ"
                );
            }
        }
        let (again_out, again_args) = run_cull(&renderer, &fx, &assets);
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

fn visible_world(scene: &RenderScene) -> (usize, usize) {
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
