//! The frame's mesh draws: instances grouped by shape (primitives) or asset index, packed into one
//! `VERTEX | STORAGE` buffer, world draws first, so the `cull` pass can read every world instance
//! and the draws bind their own byte range of it.

use ludion_core::math::glam::{Mat3, Mat4, Vec3};

use super::static_batch::{RunKey, StaticCopy, StaticGpu, StaticSlice};
use super::{CasterPipeline, MeshAssetGpu, MeshVariant};
use crate::cull::{Aabb, AssetBounds, local_bounds};
use crate::scene::{MeshInstance, MeshShape, RenderScene};
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct MeshInstanceRaw {
    pub(super) model: [[f32; 4]; 4],
    pub(super) color: [f32; 4],
    /// `(roughness, metallic, emissive, use_vertex_material)`. `w` is the branch-free selection
    /// flag: `1.0` selects the vertex material, `0.0` these per-instance scalars.
    pub(super) material: [f32; 4],
    /// The normal matrix (inverse-transpose of the model's upper-3×3), embedded in a `mat4` via
    /// [`Mat4::from_mat3`] so the shader can apply it with the same `mat4 * vec4(n, 0)` arithmetic
    /// the old upper-3×3 path used — which keeps uniform-scale output bit-identical (the normal
    /// matrix then *is* the model's upper-3×3, and the `w` column is annihilated by the `0`).
    ///
    /// The lanes that product never reads are **repurposed** to carry the wrap-diffuse data, so this
    /// struct needs no extra attribute (see [`instance_normal_lanes`]). Any further per-instance
    /// value gets its own attribute (as `rim` and `medium` do), not more packing here.
    pub(super) normal: [[f32; 4]; 4],
    /// `(fade, height_min, bone_offset, handedness)`. Read at `@location(16)`: the first two lanes by
    /// the mesh shader's Bayer-dither discard (from the entity's `Fade` component, defaults
    /// `(1.0, -f32::MAX)`; the default can never discard, so a fade-free frame is render-identical by
    /// construction), the third by the vertex shader as the bone-palette offset
    /// ([`MeshInstance::bone_offset`], `0` = unskinned, never read), the fourth as the model's
    /// handedness sign (`-1.0` for a reflection; see [`is_mirrored`]).
    pub(super) fade: [f32; 4],
    /// Per-instance hit-flash lane `(flash_color.rgb, amount)`, from the entity's `Mesh` component
    /// (default `(1, 1, 1, 0)`). Read at `@location(17)`; the mesh shader lerps the shaded colour
    /// toward `flash_color` by `amount`, and `amount == 0` can never change a pixel.
    pub(super) flash: [f32; 4],
    /// Per-instance rim lane `(color·strength rgb, power)` at `@location(20)`; all-zero is no rim.
    pub(super) rim: [f32; 4],
    /// Per-instance medium lane at `@location(21)`: `.x` is the scale, `yzw` are spare zeros
    /// (a whole `vec4` keeps the stride a multiple of 16 for `cull.wgsl`).
    pub(super) medium: [f32; 4],
}

pub(super) struct MeshDraw {
    pub(super) shape: MeshShape,
    pub(super) instances: wgpu::Buffer,
    pub(super) count: u32,
    /// Index of this draw's first instance in the frame's packed buffer.
    pub(super) first: u32,
    /// The world draw's sequential index among world draws (its `cull` bucket); `None` for a
    /// viewmodel draw, which is never culled.
    pub(super) bucket: Option<u32>,
    /// Draws through the shader's winding-reversed variant ([`is_mirrored`]).
    pub(super) mirrored: bool,
    /// Every instance is a viewmodel mesh: the draw binds the viewmodel's camera group.
    pub(super) viewmodel: bool,
    /// Ascending indices into `scene.meshes` of this draw's instances, in instance order, so the
    /// drop-shadow occlusion pass can skip a caster's own.
    pub(super) sources: Vec<u32>,
    /// The leading instances that are the persistent static run, GPU-copied into the packed buffer.
    pub(super) statics: Option<StaticSlice>,
}

impl MeshDraw {
    /// A primitive is always single-sided.
    pub(super) fn variant(&self) -> MeshVariant {
        MeshVariant {
            mirrored: self.mirrored,
            double_sided: false,
        }
    }

    /// A primitive has no material, so it always casts through the depth-only pipeline.
    pub(super) fn caster(&self) -> CasterPipeline {
        CasterPipeline {
            variant: self.variant(),
            cutout: false,
        }
    }
}

pub(super) struct MeshAssetDraw {
    pub(super) mesh: MeshAssetGpu,
    pub(super) instances: wgpu::Buffer,
    pub(super) count: u32,
    /// As [`MeshDraw::first`].
    pub(super) first: u32,
    /// As [`MeshDraw::bucket`].
    pub(super) bucket: Option<u32>,
    /// Draws through the shader's winding-reversed variant ([`is_mirrored`]).
    pub(super) mirrored: bool,
    /// As [`MeshDraw::viewmodel`].
    pub(super) viewmodel: bool,
    /// As [`MeshDraw::sources`].
    pub(super) sources: Vec<u32>,
    /// As [`MeshDraw::statics`].
    pub(super) statics: Option<StaticSlice>,
}

impl MeshAssetDraw {
    pub(super) fn variant(&self) -> MeshVariant {
        MeshVariant {
            mirrored: self.mirrored,
            double_sided: self.mesh.double_sided,
        }
    }

    /// The shadow caster pipeline this draw records with.
    pub(super) fn caster(&self) -> CasterPipeline {
        CasterPipeline {
            variant: self.variant(),
            cutout: self.mesh.cutout,
        }
    }
}

/// The per-instance material vec4 `(roughness, metallic, emissive, use_vertex_material)`: the
/// component-driven scalars plus the branch-free selection flag (`1.0` = the geometry's own vertex
/// material wins, `0.0` = these scalars do).
pub(super) fn instance_material(m: &MeshInstance) -> [f32; 4] {
    let [roughness, metallic, emissive] = m.material;
    let flag = if m.use_vertex_material { 1.0 } else { 0.0 };
    [roughness, metallic, emissive, flag]
}

/// Whether `model` is a reflection (negative determinant). The one predicate behind both halves of
/// mirror correctness: the winding-reversed pipeline twin a draw records with and the handedness
/// sign the mesh shader multiplies into the tangent frame, so the two can never disagree.
pub(super) fn is_mirrored(model: &Mat4) -> bool {
    Mat3::from_mat4(*model).determinant() < 0.0
}

/// The per-instance occlusion-fade vec4 `(fade, height_min, bone_offset, handedness)`. The mesh
/// shader's Bayer-dither discard reads the first two lanes (the default `(1.0, -f32::MAX)` never
/// discards, so a fade-free frame is render-identical by construction); the vertex shader reads the
/// third lane as the bone-palette offset (`0` = unskinned, never read) and the fourth as the model's
/// handedness sign (`-1.0` for a reflection).
pub(super) fn instance_fade(m: &MeshInstance) -> [f32; 4] {
    let handedness = if is_mirrored(&m.model) { -1.0 } else { 1.0 };
    [m.fade, m.fade_height_min, m.bone_offset as f32, handedness]
}

/// The per-instance hit-flash vec4 `(flash_color.rgb, amount)` the mesh shader lerps the shaded
/// colour toward. The default `amount == 0.0` mixes nothing, so a flash-free frame is render-identical
/// by construction.
pub(super) fn instance_flash(m: &MeshInstance) -> [f32; 4] {
    let [r, g, b] = m.flash_color;
    [r, g, b, m.flash]
}

/// The instance's normal-matrix lanes, which also carry the wrap-diffuse data. The shader reads the
/// matrix only as `(normal_mat * vec4(n, 0.0)).xyz`, so the `w` row (`n0.w`, `n1.w`, `n2.w`) and the
/// column multiplied by `0.0` (`n3`) reach no result. A wrapped instance (`wrap > 0`) stores there:
/// `n0.w` = wrap, `n3.xyz` = scatter colour, `(n1.w, n2.w, n3.w)` = world half-extents of its bounds
/// (how far the band's shadow test lifts toward the light; zero when unknown). Wrap off is exactly
/// `Mat4::from_mat3`'s bytes. Every write goes through here, every read through `wrap_lanes` in
/// `mesh.wgsl`. Everything written is finite: `n3.xyz` is multiplied by `0.0`, so a NaN there would
/// poison the normal.
pub(super) fn instance_normal_lanes(
    m: &MeshInstance,
    asset_bounds: &[AssetBounds],
) -> [[f32; 4]; 4] {
    let mut lanes = Mat4::from_mat3(m.normal_matrix).to_cols_array_2d();
    if m.wrap.is_nan() || m.wrap <= 0.0 {
        return lanes;
    }
    let scatter = m
        .scatter_color
        .map(|c| if c.is_finite() { c.max(0.0) } else { 1.0 });
    let half = local_bounds(m.shape, asset_bounds)
        .map(|local| local.transformed(&m.model))
        .filter(Aabb::is_finite)
        .map_or(Vec3::ZERO, |b| (b.max - b.min) * 0.5);
    lanes[0][3] = m.wrap.min(1.0);
    lanes[3] = [scatter[0], scatter[1], scatter[2], half.z];
    lanes[1][3] = half.x;
    lanes[2][3] = half.y;
    lanes
}

/// The packed raw of one instance. `lane_bounds` are the asset bounds the wrap lanes size from; a
/// primitive passes none.
pub(super) fn instance_raw(m: &MeshInstance, lane_bounds: &[AssetBounds]) -> MeshInstanceRaw {
    MeshInstanceRaw {
        model: m.model.to_cols_array_2d(),
        color: m.color,
        material: instance_material(m),
        normal: instance_normal_lanes(m, lane_bounds),
        fade: instance_fade(m),
        flash: instance_flash(m),
        rim: m.rim,
        medium: [m.medium, 0.0, 0.0, 0.0],
    }
}

/// Buckets every [`MeshShape::Asset`] instance in `scene` by its asset index, in scene order, into
/// `asset_count` buckets — one linear pass instead of one filter pass per asset — keeping only the
/// instances whose [`is_mirrored`] equals `mirrored` and whose viewmodel flag equals `viewmodel`.
///
/// Instances whose index is out of range are dropped: the caller has no GPU mesh to draw them with.
/// Only instances from scene index `from` on are bucketed: the statics before it come from their
/// persistent run. Draw order is golden-bearing (this repo gates on bit-exact pinned-GPU images), and this is the
/// order that decides it: buckets ascending, scene order within a bucket.
///
/// The second return value is, per bucket, the ascending `scene.meshes` index of each instance.
pub(super) fn bucket_asset_instances(
    scene: &RenderScene,
    from: usize,
    asset_count: usize,
    asset_bounds: &[AssetBounds],
    mirrored: bool,
    viewmodel: bool,
) -> (Vec<Vec<MeshInstanceRaw>>, Vec<Vec<u32>>) {
    let mut buckets: Vec<Vec<MeshInstanceRaw>> = vec![Vec::new(); asset_count];
    let mut sources: Vec<Vec<u32>> = vec![Vec::new(); asset_count];
    for (scene_index, m) in scene.meshes.iter().enumerate().skip(from) {
        let MeshShape::Asset(index) = m.shape else {
            continue;
        };
        if is_mirrored(&m.model) != mirrored || m.viewmodel != viewmodel {
            continue;
        }
        let Some(bucket) = buckets.get_mut(index) else {
            continue;
        };
        sources[index].push(scene_index as u32);
        bucket.push(instance_raw(m, asset_bounds));
    }
    (buckets, sources)
}

/// The byte range of a draw's instances inside the packed buffer.
fn instance_range(first: u64, count: u32) -> std::ops::Range<u64> {
    let stride = std::mem::size_of::<MeshInstanceRaw>() as u64;
    first * stride..(first + u64::from(count)) * stride
}

impl MeshDraw {
    /// This draw's bytes of the packed instance buffer.
    pub(super) fn instance_bytes(&self) -> std::ops::Range<u64> {
        instance_range(u64::from(self.first), self.count)
    }
}

impl MeshAssetDraw {
    /// This draw's bytes of the packed instance buffer.
    pub(super) fn instance_bytes(&self) -> std::ops::Range<u64> {
        instance_range(u64::from(self.first), self.count)
    }
}

/// The frame's mesh draws and the one buffer they share.
pub(super) struct MeshBatch {
    pub(super) draws: Vec<MeshDraw>,
    pub(super) asset_draws: Vec<MeshAssetDraw>,
    /// Every draw's instances: primitive world draws, asset world draws, then the viewmodel's.
    /// `None` when nothing draws.
    pub(super) instances: Option<wgpu::Buffer>,
    /// The frame bucket of each static run, in run order.
    pub(super) static_buckets: Vec<u32>,
}

/// One draw's instances before packing: the static run it leads with (already on the GPU) and the
/// dynamic raws the CPU writes after it. `sources` lists both, ascending.
struct Pending {
    mirrored: bool,
    viewmodel: bool,
    sources: Vec<u32>,
    raws: Vec<MeshInstanceRaw>,
    statics: Option<StaticSlice>,
}

impl Pending {
    fn count(&self) -> u32 {
        self.statics.map_or(0, |s| s.count) + self.raws.len() as u32
    }
}

struct PrimPending {
    shape: MeshShape,
    draw: Pending,
}

struct AssetPending {
    mesh: MeshAssetGpu,
    draw: Pending,
}

/// The static run of `key` as a draw's leading instances: its slice and sources.
fn static_lead(statics: Option<&StaticGpu>, key: RunKey) -> (Option<StaticSlice>, Vec<u32>) {
    match statics.and_then(|s| s.run(key)) {
        Some((i, run)) => (Some(run.slice(i)), run.sources.clone()),
        None => (None, Vec::new()),
    }
}

/// Groups the scene's mesh instances into per-shape and per-asset draws and uploads them once.
/// Draw order is golden-bearing and unchanged from the per-draw buffers this replaced; only the
/// packing differs (world draws contiguous, so a world draw's `bucket` is its `cull` bucket).
///
/// The leading `statics` meshes of the scene come from their persistent run: each world draw's static
/// run is queued as a GPU copy into `copies` and only the dynamic instances are written here, after
/// it. Without `statics` every instance is written, and the buffer holds the same bytes.
pub(super) fn build_mesh_batch(
    scene: &RenderScene,
    device: &wgpu::Device,
    meshes: &[MeshAssetGpu],
    statics: Option<&StaticGpu>,
    copies: &mut Vec<StaticCopy>,
) -> MeshBatch {
    let from = statics.map_or(0, StaticGpu::count);
    let prims = pending_primitives(scene, from, statics);
    let assets = pending_assets(scene, from, meshes, statics);

    let mut total = 0u32;
    let mut bucket = 0u32;
    let mut prim_slot = vec![(0u32, None); prims.len()];
    let mut asset_slot = vec![(0u32, None); assets.len()];
    for viewmodel in [false, true] {
        let mut place = |draw: &Pending| {
            let first = total;
            total += draw.count();
            let b = (!viewmodel).then(|| {
                bucket += 1;
                bucket - 1
            });
            (first, b)
        };
        for (i, p) in prims
            .iter()
            .enumerate()
            .filter(|(_, p)| p.draw.viewmodel == viewmodel)
        {
            prim_slot[i] = place(&p.draw);
        }
        for (i, a) in assets
            .iter()
            .enumerate()
            .filter(|(_, a)| a.draw.viewmodel == viewmodel)
        {
            asset_slot[i] = place(&a.draw);
        }
    }
    let instances = (total > 0).then(|| {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ludion frame buffer"),
            size: u64::from(total) * std::mem::size_of::<MeshInstanceRaw>() as u64,
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        {
            let mut mapped = buffer
                .slice(..)
                .get_mapped_range_mut()
                .expect("a freshly mapped_at_creation buffer is always mappable");
            let stride = std::mem::size_of::<MeshInstanceRaw>();
            let mut write = |draw: &Pending, first: u32| {
                let at = (first + draw.statics.map_or(0, |s| s.count)) as usize * stride;
                let bytes: &[u8] = bytemuck::cast_slice(&draw.raws);
                mapped.slice(at..at + bytes.len()).copy_from_slice(bytes);
            };
            for (p, &(first, _)) in prims.iter().zip(&prim_slot) {
                write(&p.draw, first);
            }
            for (a, &(first, _)) in assets.iter().zip(&asset_slot) {
                write(&a.draw, first);
            }
        }
        buffer.unmap();
        buffer
    });
    let mut static_buckets = vec![0u32; statics.map_or(0, StaticGpu::run_count)];
    if let (Some(gpu), Some(buffer)) = (statics, instances.as_ref()) {
        let draws = prims
            .iter()
            .zip(&prim_slot)
            .map(|(p, slot)| (&p.draw, slot))
            .chain(
                assets
                    .iter()
                    .zip(&asset_slot)
                    .map(|(a, slot)| (&a.draw, slot)),
            );
        for (draw, &(first, b)) in draws {
            let Some(slice) = draw.statics else { continue };
            debug_assert!(!draw.viewmodel, "a static is never a viewmodel mesh");
            let run = gpu.run_by_index(slice.run as usize);
            copies.push(StaticCopy::instances(gpu.instances(), run, buffer, first));
            static_buckets[slice.run as usize] = b.expect("a world draw has a bucket");
        }
    }
    let draws = prims
        .into_iter()
        .zip(prim_slot)
        .map(|(p, (first, bucket))| MeshDraw {
            shape: p.shape,
            instances: instances.clone().expect("a pending draw has instances"),
            count: p.draw.count(),
            first,
            bucket,
            mirrored: p.draw.mirrored,
            viewmodel: p.draw.viewmodel,
            sources: p.draw.sources,
            statics: p.draw.statics,
        })
        .collect();
    let asset_draws = assets
        .into_iter()
        .zip(asset_slot)
        .map(|(a, (first, bucket))| MeshAssetDraw {
            mesh: a.mesh,
            instances: instances.clone().expect("a pending draw has instances"),
            count: a.draw.count(),
            first,
            bucket,
            mirrored: a.draw.mirrored,
            viewmodel: a.draw.viewmodel,
            sources: a.draw.sources,
            statics: a.draw.statics,
        })
        .collect();
    MeshBatch {
        draws,
        asset_draws,
        instances,
        static_buckets,
    }
}

/// Groups mesh instances from scene index `from` on by shape, each led by its static run; world draws
/// first, then the viewmodel's, unmirrored before mirrored within each.
fn pending_primitives(
    scene: &RenderScene,
    from: usize,
    statics: Option<&StaticGpu>,
) -> Vec<PrimPending> {
    let mut out = Vec::new();
    for (viewmodel, (mirrored, shape)) in [false, true].into_iter().flat_map(|viewmodel| {
        [false, true].into_iter().flat_map(move |mirrored| {
            [MeshShape::Cube, MeshShape::Plane, MeshShape::Sphere]
                .map(move |shape| (viewmodel, (mirrored, shape)))
        })
    }) {
        let (lead, mut sources) = if viewmodel {
            (None, Vec::new())
        } else {
            static_lead(statics, RunKey::Prim { mirrored, shape })
        };
        let (dynamic, raws): (Vec<u32>, Vec<MeshInstanceRaw>) = scene
            .meshes
            .iter()
            .enumerate()
            .skip(from)
            .filter(|(_, m)| {
                m.shape == shape && is_mirrored(&m.model) == mirrored && m.viewmodel == viewmodel
            })
            .map(|(i, m)| (i as u32, instance_raw(m, &[])))
            .unzip();
        if raws.is_empty() && lead.is_none() {
            continue;
        }
        sources.extend(dynamic);
        out.push(PrimPending {
            shape,
            draw: Pending {
                mirrored,
                viewmodel,
                sources,
                raws,
                statics: lead,
            },
        });
    }
    out
}

/// Groups asset-mesh instances from scene index `from` on by their `mesh_assets` index, keyed like
/// [`pending_primitives`] and paired with that mesh's cached GPU buffers for an **indexed** instanced
/// draw.
fn pending_assets(
    scene: &RenderScene,
    from: usize,
    meshes: &[MeshAssetGpu],
    statics: Option<&StaticGpu>,
) -> Vec<AssetPending> {
    let mut out = Vec::new();
    let asset_bounds: Vec<AssetBounds> = meshes.iter().map(|asset| asset.bounds.clone()).collect();
    for (viewmodel, mirrored) in [(false, false), (false, true), (true, false), (true, true)] {
        let (buckets, sources) = bucket_asset_instances(
            scene,
            from,
            meshes.len(),
            &asset_bounds,
            mirrored,
            viewmodel,
        );
        for (index, ((mesh, raws), dynamic)) in meshes.iter().zip(buckets).zip(sources).enumerate()
        {
            let (lead, mut sources) = if viewmodel {
                (None, Vec::new())
            } else {
                static_lead(statics, RunKey::Asset { mirrored, index })
            };
            // An empty mesh is valid content (erasing a model's last voxel) with nothing to draw;
            // its zero-size buffers cannot be sliced, so it never reaches a draw loop.
            if (raws.is_empty() && lead.is_none())
                || mesh.vertex_count == 0
                || mesh.index_count == 0
            {
                continue;
            }
            sources.extend(dynamic);
            out.push(AssetPending {
                mesh: mesh.clone(),
                draw: Pending {
                    mirrored,
                    viewmodel,
                    sources,
                    raws,
                    statics: lead,
                },
            });
        }
    }
    out
}

/// What the cull pass wrote and a draw can read: the compacted instances and indirect args of its
/// views, with the shape that places view `v`'s slices.
#[derive(Clone, Copy)]
pub(super) struct Culled<'a> {
    instances: &'a wgpu::Buffer,
    args: &'a wgpu::Buffer,
    shape: crate::graph::CullShape,
    /// The live views: a draw for a later view reads its own buffer directly.
    views: u32,
}

impl<'a> Culled<'a> {
    /// The cull's compacted instances, every view's slice.
    pub(super) fn instances(self) -> &'a wgpu::Buffer {
        self.instances
    }
}

/// The cull pass's outputs from a pass's buffer reads, placed by the frame's `cull`. `None` when the
/// pass declares no read, which is every direct draw.
pub(super) fn culled_buffers<'a>(
    reads: &'a [&'a wgpu::Buffer],
    cull: Option<&crate::gpu_cull::CullFrame>,
) -> Option<Culled<'a>> {
    match (reads, cull) {
        ([instances, args], Some(cull)) => Some(Culled {
            instances,
            args,
            shape: cull.shape(),
            views: cull.views(),
        }),
        _ => None,
    }
}

/// The cull view the sun's first shadow map is: view 0 is the camera.
pub(super) const SUN_CULL_VIEW: u32 = 1;

/// Every instance of a draw: `sources` is parallel to the instance buffer.
pub(super) fn all_ranges(sources: &[u32]) -> Vec<std::ops::Range<u32>> {
    std::iter::once(0..sources.len() as u32).collect()
}

/// Bytes per indirect-args slot (`DrawIndexedIndirectArgs`).
const ARGS_STRIDE: u64 = 20;

/// Where one draw's instances come from: a buffer and the byte range of it, plus the indirect args
/// when the cull wrote them.
pub(super) struct DrawSource<'a> {
    pub(super) instances: &'a wgpu::Buffer,
    pub(super) bytes: std::ops::Range<u64>,
    pub(super) indirect: Option<(&'a wgpu::Buffer, u64)>,
}

/// Bucket `bucket` (instances `[first, first + count)` of the packed buffer) as view `view` sees it:
/// the cull's compacted slice and args when it culled that view, else the draw's own packed range.
///
/// `level` picks the lane of the view's chain: the args slot of (view, level) holds that level's index
/// count and its run's `first_instance`; level 0 of a frame without a chain is the view's one slot.
fn source_of<'a>(
    own: &'a wgpu::Buffer,
    (first, count): (u32, u32),
    bucket: Option<u32>,
    culled: Option<Culled<'a>>,
    view: u32,
    level: u32,
) -> DrawSource<'a> {
    match (culled, bucket) {
        (Some(c), Some(b)) if view < c.views => {
            let base = u64::from(view) * u64::from(c.shape.instances) + u64::from(first);
            let lane = u64::from(view) * u64::from(c.shape.levels) + u64::from(level);
            let slot = lane * u64::from(c.shape.buckets) + u64::from(b);
            DrawSource {
                instances: c.instances,
                bytes: instance_range(base, count),
                indirect: Some((c.args, slot * ARGS_STRIDE)),
            }
        }
        _ => DrawSource {
            instances: own,
            bytes: instance_range(u64::from(first), count),
            indirect: None,
        },
    }
}

impl MeshDraw {
    /// Where this draw's instances come from in `view` (0 is the camera) and, when culled, its indirect args.
    pub(super) fn source<'a>(&'a self, culled: Option<Culled<'a>>, view: u32) -> DrawSource<'a> {
        source_of(
            &self.instances,
            (self.first, self.count),
            self.bucket,
            culled,
            view,
            0,
        )
    }
}

impl MeshAssetDraw {
    /// As [`MeshDraw::source`], for `level` of the chain.
    pub(super) fn source<'a>(
        &'a self,
        culled: Option<Culled<'a>>,
        view: u32,
        level: u32,
    ) -> DrawSource<'a> {
        source_of(
            &self.instances,
            (self.first, self.count),
            self.bucket,
            culled,
            view,
            level,
        )
    }

    /// How many levels the draw issues in `view`: one per chain level the cull wrote a lane for when
    /// the cull places this view's runs, else just level 0 (a direct draw, a viewmodel or a view past
    /// the cull's capacity).
    pub(super) fn level_count(&self, culled: Option<Culled<'_>>, view: u32) -> u32 {
        match (culled, self.bucket) {
            (Some(c), Some(_)) if view < c.views => {
                (1 + self.mesh.levels.len() as u32).min(c.shape.levels)
            }
            _ => 1,
        }
    }

    /// The GPU buffers of `level`: 0 is the mesh itself.
    pub(super) fn level(&self, level: u32) -> &MeshAssetGpu {
        match level.checked_sub(1) {
            None => &self.mesh,
            Some(k) => &self.mesh.levels[k as usize],
        }
    }
}
