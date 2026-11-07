//! The per-frame dynamic-light injection (`shaders/gi_inject.wgsl`) — the half of the one-bounce
//! term that belongs to lights the simulation moves.
//!
//! One compute dispatch over the probe lattice replays the GI bake's cached hits against a
//! world-space bin of the scene's `GiDynamic`-marked lights and writes each probe's RGB DC
//! irradiance into a **transient** volume the scene pass samples beside the baked trio
//! (`Plan/gi-wave3-dynamic-injection.md` §5). Zero history: the volume is a pure function of this
//! frame's world state, recomputed whole, allocated by the frame graph and forgotten.
//!
//! **What is renderer-owned here, and what is not.** [`GiInject`] owns exactly two things — the
//! compute pipeline and the 1×1×1 fallback volume a frame with no injection binds, the `ibl.rs`
//! shape every GI stage in this crate follows. The *output* volume is the graph's, because a
//! recomputed-every-frame resource is an ordinary transient; the *inputs* are the bake's hit cache,
//! `voxel_bake.rs`'s albedo texture and this frame's binned light set, none of which the graph
//! knows about.
//!
//! **The scheduling predicate is one function** ([`scheduled_lattice`]), read by
//! [`PostChain::for_scene`](crate::PostChain::for_scene) to decide whether the pass exists and by
//! `render.rs` to decide whether to upload for it. One conjunction, not two copies of it: a graph
//! that scheduled the pass while the recorder had nothing to dispatch would leave a probe volume
//! nobody wrote, and a recorder that uploaded for a pass the graph never declared would be silent
//! waste. Masking is the same seam — [`PostChain::masked`](crate::PostChain::masked) clears the
//! lattice when the machine's [`Quality::gi_dynamic`](crate::Quality::gi_dynamic) is off, so a
//! masked frame is byte-for-byte the no-marked-content frame rather than a cheaper variant of the
//! marked one.
//!
//! **Determinism.** The bin build is sequential and fixed-order (`gi_bin.rs`), `k` ascends, each bin
//! list ascends by construction, and each probe is one invocation with no atomics — but the
//! accumulation is GPU float arithmetic, so this is deterministic *per machine*, exactly where the
//! bake and the IBL prefilter sit.

use bytemuck::Zeroable;
use ludion_core::diagnostics::Diagnostic;
use ludion_render_graph::Gpu;

use crate::compute_shader::ComputeShader;
use crate::gi_bake::{
    HIT_FACE_BOUNDARY, HIT_FACE_ESCAPED, HIT_FACE_SHIFT, premultiplied, read_probe_texture,
};
use crate::gi_bin::{BinBuild, build_bin};
use crate::scene::{GiData, PointLightData, RenderScene, WorldVoxelGrid};
use crate::shader_gen::{self, GiInjectParams, Light};

/// The compute shader's workgroup edge: one workgroup injects a 4×4×4 block of probes — the bake's
/// own shape, over the bake's own lattice.
const WORKGROUP: [u32; 3] = shader_gen::GI_INJECT_INJECT_WORKGROUP_SIZE;

/// The hit cache's packing, checked at compile time because `gi_inject.wgsl` **unpacks** it: the
/// face codes must fit in the bits above [`HIT_FACE_SHIFT`] and the cell index in the bits below,
/// with room for every cell the `L2103` 32 M-cell budget admits. `gi_bake.rs` declares the layout,
/// this module is its reader, and both mirror the WGSL that writes and reads the words — which is
/// what `the_shader_and_the_cpu_agree_on_the_hit_cache_layout` compares.
const _: () = {
    assert!(HIT_FACE_ESCAPED < 8 && HIT_FACE_BOUNDARY < HIT_FACE_ESCAPED);
    assert!(HIT_FACE_SHIFT + 3 <= 32 && 1u64 << HIT_FACE_SHIFT >= 32_000_000);
};

/// The dynamic volume's format. [`gi_bake`](crate::gi_bake)'s `PROBE_FORMAT`, for the same two
/// reasons: the consumer needs hardware trilinear filtering between probes with no device feature
/// requested, and the term is HDR radiance.
const VOLUME_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The probe lattice the `gi_inject` pass writes for `scene`, or `None` when the scene schedules no
/// injection at all.
///
/// **This is the scheduling predicate**, in the one place both the graph and the recorder read it.
/// All three parts are load-bearing:
///
/// - a `GiVolume` resolved a real lattice — there is nowhere to write otherwise;
/// - its authored `bounce` is non-zero — the injected term *is* the bounce term, scaled by the same
///   authored strength (there is deliberately no second `dynamic_bounce` knob), so a scene at
///   `bounce: 0.0` gets no injection however much it marks;
/// - at least one light carries the marker — with none, the pass would write an all-zero volume for
///   the shader to add exactly `+0.0` from.
///
/// The machine's quality switch is *not* here: it masks the result
/// ([`PostChain::masked`](crate::PostChain::masked)), because content and host settings are
/// different questions and only the former belongs to a scene.
pub(crate) fn scheduled_lattice(scene: &RenderScene) -> Option<[u32; 3]> {
    let gi = scene.gi.as_ref()?;
    let marked = scene.lights.iter().any(|light| light.dynamic);
    (gi.bounce > 0.0 && marked).then_some(gi.count)
}

/// The dynamic light subset the injection gathers: every light whose entity carries the `GiDynamic`
/// marker, in the extract order it arrived in, premultiplied into the shader's `Light`.
///
/// The exact complement of [`static_lights`](crate::gi_bake::static_lights) — one walk each over the
/// same pinned order, so a light lands in exactly one of the two terms and neither consumer re-reads
/// the tag. The premultiply is the bake's own function, shared rather than restated.
fn dynamic_lights(lights: &[PointLightData]) -> Vec<Light> {
    lights
        .iter()
        .filter(|light| light.dynamic)
        .map(premultiplied)
        .collect()
}

/// Everything one dispatch needs, gathered so the absent case is a single `Option` rather than
/// several that could disagree.
pub(crate) struct InjectInput<'a> {
    /// The voxelization the cached hits index into — its origin, cell size and extent.
    pub grid: &'a WorldVoxelGrid,
    /// The authored knobs and the probe lattice `ludion-engine` sized from `grid`.
    pub gi: &'a GiData,
    /// `voxel_bake.rs`'s per-cell albedo texture for `grid` — the `albedo` of the outgoing radiance.
    pub albedo: &'a wgpu::Texture,
    /// The bake's hit cache for this lattice ([`ResolvedGi::hits`](crate::gi_bake::ResolvedGi)).
    pub hits: &'a wgpu::Buffer,
    /// Every extracted light, marked and unmarked, in extract order. The partition happens here.
    pub lights: &'a [PointLightData],
    /// How many rays the bake behind [`hits`](InjectInput::hits) traced —
    /// [`ResolvedGi::rays`](crate::gi_bake::ResolvedGi::rays), never the machine's setting read a
    /// second time.
    ///
    /// The gather replays `hits` and recovers each slot's direction by dividing by this, and the ray
    /// set is re-derived at every count rather than truncated, so a count that is not the one the
    /// march ran at reconstructs directions it never took — silently, with no binding to fail.
    pub rays: u32,
}

/// One frame's uploaded injection inputs — the buffers and views the dispatch binds, built in
/// `prepare_frame` and consumed once by the recorder.
pub(crate) struct GiInjectFrame {
    params: wgpu::Buffer,
    lights: wgpu::Buffer,
    bin_table: wgpu::Buffer,
    bin_indices: wgpu::Buffer,
    hits: wgpu::Buffer,
    albedo: wgpu::TextureView,
    /// Probes per axis — the dispatch's own extent.
    count: [u32; 3],
}

/// The dynamic-light injection: the compute pipeline and the 1×1×1 fallback volume.
pub(crate) struct GiInject {
    pipeline: wgpu::ComputePipeline,
    /// Bound in the scene pass's dynamic slot on every frame that schedules no injection — valid to
    /// bind, never sampled (the shader's flag is zero then).
    fallback: wgpu::TextureView,
    /// **Test-only.** Which light bin the next upload builds — the seam the conformance oracle needs
    /// to run one scene through this exact shader both binned and degenerate. It does not exist
    /// outside `cfg(test)`: the degenerate bin is an oracle, never a shipping mode.
    #[cfg(test)]
    bin_build: std::cell::Cell<BinBuild>,
}

impl GiInject {
    /// Builds the compute pipeline and the fallback volume.
    pub(crate) fn new(gpu: &Gpu) -> GiInject {
        let device = gpu.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gi inject"),
            source: wgpu::ShaderSource::Wgsl(ComputeShader::GiInject.composed().into()),
        });
        // The auto-derived layout is what makes the albedo binding legal: wgpu infers a sampled float
        // texture's `filterable` from whether the entry point pairs it with a sampler, and this
        // shader only ever `textureLoad`s — so the non-filterable `Rgba32Float` the voxel upload uses
        // satisfies the layout with no device feature and no hand-written BGL.
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gi inject"),
            layout: None,
            module: &module,
            entry_point: Some("inject"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        GiInject {
            pipeline,
            fallback: fallback_volume(device),
            #[cfg(test)]
            bin_build: std::cell::Cell::new(BinBuild::Culled),
        }
    }

    /// The 1×1×1 volume a frame with no injection binds in the scene pass's dynamic slot.
    pub(crate) fn fallback(&self) -> &wgpu::TextureView {
        &self.fallback
    }

    /// Partitions, bins and uploads this frame's injection inputs.
    ///
    /// Sequential and allocation-shaped rather than cached: the marked lights are the ones that move,
    /// so there is nothing here a frame could reuse — which is the whole point of splitting them out
    /// of the bake's cache key.
    pub(crate) fn prepare(&self, gpu: &Gpu, input: InjectInput<'_>) -> GiInjectFrame {
        use wgpu::util::DeviceExt;
        let device = gpu.device();
        // The stride the shader derives from this is `rays / K`, so a count that is not a whole
        // multiple of the cache's slot budget files them into a polar cap instead of over the
        // sphere. `ludion-engine`'s `L0016` refuses one before a host can set it; the bake asserts
        // the same thing where it writes the cache.
        debug_assert!(
            input.rays >= crate::gi_bake::HIT_RAYS
                && input.rays.is_multiple_of(crate::gi_bake::HIT_RAYS),
            "gi_rays {} must be a positive whole multiple of the {}-slot hit cache",
            input.rays,
            crate::gi_bake::HIT_RAYS,
        );

        let mut lights = dynamic_lights(input.lights);
        let light_count = lights.len() as u32;

        // The world-space bin over exactly that array, built before the padding below so a bin index
        // can only ever address a real light.
        #[cfg(not(test))]
        let bin_build = BinBuild::Culled;
        #[cfg(test)]
        let bin_build = self.bin_build.get();
        let bin = build_bin(input.grid, &lights, bin_build);

        // wgpu rejects a zero-sized buffer binding, and naga sizes a runtime array as exactly one
        // element; the predicate guarantees at least one marked light, but a padded zero costs one
        // word and removes the whole class of "what if it were empty".
        if lights.is_empty() {
            lights.push(Light::zeroed());
        }
        let mut bin_indices = bin.indices;
        if bin_indices.is_empty() {
            bin_indices.push(0);
        }

        let params = GiInjectParams {
            grid: [
                input.grid.origin.x,
                input.grid.origin.y,
                input.grid.origin.z,
                input.grid.cell_size,
            ],
            counts: [
                input.gi.count[0],
                input.gi.count[1],
                input.gi.count[2],
                light_count,
            ],
            bin: bin.params,
            rays: [input.rays, 0, 0, 0],
        };
        let storage = |label: &str, contents: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::STORAGE,
            })
        };

        GiInjectFrame {
            params: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gi inject params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            }),
            lights: storage("gi inject lights", bytemuck::cast_slice(&lights)),
            bin_table: storage("gi inject bin table", bytemuck::cast_slice(&bin.table)),
            bin_indices: storage("gi inject bin indices", bytemuck::cast_slice(&bin_indices)),
            hits: input.hits.clone(),
            albedo: input.albedo.create_view(&wgpu::TextureViewDescriptor {
                label: Some("gi inject albedo"),
                dimension: Some(wgpu::TextureViewDimension::D3),
                ..Default::default()
            }),
            count: input.gi.count,
        }
    }

    /// The bind group for one dispatch, over `frame`'s uploads and the volume `out`.
    ///
    /// Split from [`dispatch`](GiInject::dispatch) because a `RecordedPass` lends its encoder
    /// mutably: the bind group must be built before the compute pass is opened, exactly as
    /// `record_scene` builds its groups before opening its render pass.
    pub(crate) fn bind_group(
        &self,
        device: &wgpu::Device,
        frame: &GiInjectFrame,
        out: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gi inject"),
            layout: &self
                .pipeline
                .get_bind_group_layout(shader_gen::INJECT_PARAMS_GROUP),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: shader_gen::INJECT_PARAMS_BINDING,
                    resource: frame.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::INJECT_ALBEDO_BINDING,
                    resource: wgpu::BindingResource::TextureView(&frame.albedo),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::INJECT_HIT_CACHE_BINDING,
                    resource: frame.hits.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::INJECT_LIGHT_LIST_BINDING,
                    resource: frame.lights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::INJECT_BIN_TABLE_BINDING,
                    resource: frame.bin_table.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::INJECT_BIN_INDICES_BINDING,
                    resource: frame.bin_indices.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::DYN_OUT_BINDING,
                    resource: wgpu::BindingResource::TextureView(out),
                },
            ],
        })
    }

    /// Issues the dispatch over `frame`'s probe lattice into an already-opened compute pass.
    pub(crate) fn dispatch(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        frame: &GiInjectFrame,
        bind_group: &wgpu::BindGroup,
    ) {
        let [cx, cy, cz] = frame.count;
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(shader_gen::INJECT_PARAMS_GROUP, bind_group, &[]);
        pass.dispatch_workgroups(
            cx.div_ceil(WORKGROUP[0]),
            cy.div_ceil(WORKGROUP[1]),
            cz.div_ceil(WORKGROUP[2]),
        );
    }

    /// **Test-only.** Points the next upload at a different light bin — the oracle's seam.
    #[cfg(test)]
    pub(crate) fn set_bin_build(&self, build: BinBuild) {
        self.bin_build.set(build);
    }

    /// **Debug readback** (`ludion render --dump-gi`): prepares and dispatches the injection for
    /// `input` into a scratch volume — the same [`prepare`](GiInject::prepare)/[`bind_group`]/
    /// [`dispatch`](GiInject::dispatch) sequence a real frame's recorder runs, just into a volume
    /// this call owns instead of one the frame graph hands back — then reads it back to the CPU,
    /// flat-indexed `x + count.x·(y + count.y·z)` ([`gi_bake`](crate::gi_bake)'s own flat order,
    /// via [`read_probe_texture`], since [`VOLUME_FORMAT`] is that module's `PROBE_FORMAT`).
    ///
    /// Unlike [`gi_bake::GiBake::resolve_and_read`](crate::gi_bake::GiBake::resolve_and_read), there
    /// is no cache to report a hit or miss for: the dynamic volume is a pure per-dispatch transient
    /// by design (§4.1 — "recomputed whole, allocated by the frame graph and forgotten"), so this
    /// call always does the same work a real frame's dispatch does. Blocking, like its sibling: a
    /// debug dump has no non-stalling path to be on.
    ///
    /// # Errors
    /// `L2005` if the staging buffer cannot be mapped.
    pub(crate) fn resolve_and_read(
        &self,
        gpu: &Gpu,
        input: InjectInput<'_>,
    ) -> Result<Vec<[f32; 4]>, Diagnostic> {
        let device = gpu.device();
        let frame = self.prepare(gpu, input);
        let [cx, cy, cz] = frame.count;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gi dynamic readback"),
            size: wgpu::Extent3d {
                width: cx,
                height: cy,
                depth_or_array_layers: cz,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: VOLUME_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("gi dynamic readback"),
            dimension: Some(wgpu::TextureViewDimension::D3),
            ..Default::default()
        });
        let bind = self.bind_group(device, &frame, &view);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gi inject readback"),
        });
        {
            let mut compute = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("gi inject readback"),
                timestamp_writes: None,
            });
            self.dispatch(&mut compute, &frame, &bind);
        }
        gpu.queue().submit(Some(encoder.finish()));

        read_probe_texture(gpu, &texture)
    }
}

/// The 1×1×1 volume bound on frames that schedule no injection — valid to bind, never sampled (the
/// shader's `gi_bounce.y` flag is zero then). Sampled-only: the graph owns every volume this pass
/// ever writes, so this one needs no storage usage.
fn fallback_volume(device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gi dynamic fallback"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: VOLUME_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("gi dynamic fallback"),
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gi_bake::{BakeInput, GiBake, HIT_RAYS, static_lights};
    use crate::graph::{FrameSize, PostChain, build_frame_graph, pass};
    use crate::quality::{Quality, QualityParams};
    use crate::render::Renderer;
    use crate::scene::{MeshInstance, MeshShape};
    use crate::voxel_bake::VoxelBake;
    use ludion_core::math::glam::{Mat3, Mat4, Vec3};
    use std::sync::Arc;

    /// The ray count the fixtures below bake at — full quality, named rather than defaulted, for the
    /// reason `gi_bake.rs`'s own `FULL_RAYS` states: the ray set is re-derived at every count, so a
    /// measured direction or irradiance is a statement about one count.
    const FULL_RAYS: u32 = QualityParams::FULL.gi_rays;

    fn skip_or_gpu(name: &str) -> Option<Gpu> {
        match Gpu::request() {
            Ok(gpu) => Some(gpu),
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED {name}: no GPU adapter");
                None
            }
            Err(d) => panic!("gpu request failed: {d:?}"),
        }
    }

    fn skip_or_renderer(name: &str) -> Option<Renderer> {
        match Renderer::new() {
            Ok(r) => Some(r),
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED {name}: no GPU adapter");
                None
            }
            Err(d) => panic!("renderer init failed: {d:?}"),
        }
    }

    /// A closed `16 × 12 × 16` unit-cell room centred on the origin — `x, z ∈ [-8, 8]`,
    /// `y ∈ [-1, 11]` — with a one-cell solid skin and a hollow, white-walled interior.
    ///
    /// It is placed so the scenes below stand **inside** it: a probe volume the content sits outside
    /// of would make every "these pixels changed" assertion a test of the sampler's edge clamp
    /// instead of the injection. The enclosure is what makes the bin's culling interesting — six
    /// short-ranged movers in a closed box leave most cells seeing only some of them.
    fn room_grid() -> WorldVoxelGrid {
        let extent = [16u32, 12, 16];
        let total = (extent[0] * extent[1] * extent[2]) as usize;
        let mut occupancy = vec![false; total];
        for z in 0..extent[2] {
            for y in 0..extent[1] {
                for x in 0..extent[0] {
                    let skin = x == 0
                        || y == 0
                        || z == 0
                        || x == extent[0] - 1
                        || y == extent[1] - 1
                        || z == extent[2] - 1;
                    let flat = x + extent[0] * (y + extent[1] * z);
                    occupancy[flat as usize] = skin;
                }
            }
        }
        WorldVoxelGrid {
            origin: Vec3::new(-8.0, -1.0, -8.0),
            cell_size: 1.0,
            extent,
            occupancy,
            albedo: vec![[1.0, 1.0, 1.0, 1.0]; total],
            emissive: vec![0.0; total],
            hash: 0,
        }
        .sealed()
    }

    /// The same box with **only its floor** solid and open sky above — the vigil case: a light over
    /// open ground, with nothing but the ground beneath it to bounce off.
    ///
    /// It is the fixture the cached ray set's shape is decided by. A contiguous 16-ray prefix of the
    /// bake's 64 directions is a cone about `+Y`, so this scene injected exactly zero at every probe;
    /// the strided cache is what makes the ground visible, and
    /// `a_marked_light_over_open_ground_lights_the_probes_beneath_it` is the pin that keeps it so.
    fn open_slab_grid() -> WorldVoxelGrid {
        let extent = [16u32, 12, 16];
        let total = (extent[0] * extent[1] * extent[2]) as usize;
        let mut occupancy = vec![false; total];
        for z in 0..extent[2] {
            for x in 0..extent[0] {
                // The `y = 0` row: flat is `x + ex·(0 + ey·z)`.
                occupancy[(x + extent[0] * extent[1] * z) as usize] = true;
            }
        }
        WorldVoxelGrid {
            origin: Vec3::new(-8.0, -1.0, -8.0),
            cell_size: 1.0,
            extent,
            occupancy,
            albedo: vec![[1.0, 1.0, 1.0, 1.0]; total],
            emissive: vec![0.0; total],
            hash: 0,
        }
        .sealed()
    }

    /// The lattice `ludion-engine` would size over `grid` at `probe_spacing`, restated here so this
    /// module's tests need no engine dependency (`gi_bake.rs`'s test fixture, verbatim).
    fn gi_data(grid: &WorldVoxelGrid, probe_spacing: f32, bounce: f32) -> GiData {
        let count = std::array::from_fn(|axis| {
            let span = grid.extent[axis] as f32 * grid.cell_size;
            (span / probe_spacing).ceil().max(1.0) as u32 + 1
        });
        GiData {
            probe_spacing,
            normal_bias: 0.25,
            sky_visibility: 1.0,
            bounce,
            count,
            origin: grid.origin,
            span: Vec3::new(
                (count[0] - 1) as f32 * probe_spacing,
                (count[1] - 1) as f32 * probe_spacing,
                (count[2] - 1) as f32 * probe_spacing,
            ),
        }
    }

    fn light(position: Vec3, color: Vec3, range: f32, dynamic: bool) -> PointLightData {
        PointLightData {
            position,
            color,
            intensity: 2.0,
            range,
            dynamic,
            shadow: None,
        }
    }

    /// A small lit scene over `grid`: a ground plane, a cube, and whatever lights the caller passes.
    /// Everything the injection's predicate reads is a field here, so one fixture serves the marked
    /// and unmarked cases both.
    fn scene_over(
        grid: &WorldVoxelGrid,
        gi: Option<GiData>,
        lights: Vec<PointLightData>,
    ) -> RenderScene {
        let mut scene = RenderScene::empty((64, 64));
        let instance = |shape, model, color: [f32; 4]| MeshInstance {
            shape,
            model,
            normal_matrix: Mat3::IDENTITY,
            color,
            material: [1.0, 0.0, 0.0],
            use_vertex_material: false,
            fade: 1.0,
            fade_height_min: -f32::MAX,
            flash: 0.0,
            flash_color: [1.0; 3],
            wrap: 0.0,
            scatter_color: [1.0; 3],
            bone_offset: 0,
            cast_shadows: true,
            viewmodel: false,
        };
        scene.meshes.push(instance(
            MeshShape::Plane,
            Mat4::from_scale(Vec3::new(8.0, 1.0, 8.0)),
            [0.6, 0.6, 0.6, 1.0],
        ));
        scene.meshes.push(instance(
            MeshShape::Cube,
            Mat4::from_translation(Vec3::new(0.0, 0.5, 0.0)),
            [0.8, 0.3, 0.2, 1.0],
        ));
        scene.lights = lights;
        if gi.is_some() {
            scene.voxel = Some(Arc::new(grid.clone()));
        }
        scene.gi = gi;
        scene
    }

    /// Whether a scene's graph, at `quality`, schedules the injection pass — the `dump-graph`
    /// surface, which is what an agent reads to answer the same question.
    fn schedules_injection(scene: &RenderScene, quality: &Quality) -> bool {
        let chain = PostChain::for_scene(scene).masked(quality, &QualityParams::FULL);
        let (graph, _) = build_frame_graph(FrameSize::native(64, 64), chain);
        graph
            .passes()
            .iter()
            .any(|declared| declared.name == pass::GI_INJECT)
    }

    /// The quality that differs from `FULL` in the injection switch **alone** — what isolates the
    /// injection toggle from the three effects `LOW` also turns off.
    const NO_INJECTION: Quality = Quality {
        gi_dynamic: false,
        ..Quality::FULL
    };

    /// **Gate A / §11 T1 — unused content is neutral, across both toggle states.** A scene that marks
    /// nothing schedules no `gi_inject` pass and renders byte-identically whether the machine's
    /// switch is on or off. Both halves matter: the pass-count half is what makes the neutrality
    /// structural, and the byte half is what catches the hazard a new group-3 binding and a new
    /// nested branch actually pose — codegen moving an untaken path's pixels.
    ///
    /// The second fixture is the sharper one: it authors a `GiVolume` at a non-zero `bounce`, so the
    /// bounce branch the new condition nests inside is *taken*, and only the marker is missing.
    #[test]
    fn needs_gpu_an_unmarked_scene_schedules_no_injection_and_renders_identically_at_both_qualities()
     {
        let Some(mut renderer) =
            skip_or_renderer("an_unmarked_scene_schedules_no_injection_and_renders_identically")
        else {
            return;
        };
        let grid = room_grid();
        let lights = vec![light(Vec3::new(0.0, 3.0, 2.0), Vec3::ONE, 12.0, false)];
        let cases = [
            (
                "no GiVolume at all",
                scene_over(&grid, None, lights.clone()),
            ),
            (
                "a bouncing GiVolume with nothing marked",
                scene_over(&grid, Some(gi_data(&grid, 4.0, 1.0)), lights),
            ),
        ];

        for (what, scene) in cases {
            for quality in [Quality::FULL, NO_INJECTION] {
                assert!(
                    !schedules_injection(&scene, &quality),
                    "{what}: nothing is marked, so no graph may carry the pass ({quality:?})",
                );
            }
            renderer.set_quality(Quality::FULL, QualityParams::FULL);
            let on = renderer
                .render(&scene, FrameSize::native(64, 64))
                .expect("render succeeds");
            renderer.set_quality(NO_INJECTION, QualityParams::FULL);
            let off = renderer
                .render(&scene, FrameSize::native(64, 64))
                .expect("render succeeds");
            assert_eq!(
                on.rgba, off.rgba,
                "{what}: the injection switch must not move one byte of a scene that marks nothing",
            );
        }
    }

    /// **§11 T2 — the pass exists exactly when it should.** Present when the scene marks a light
    /// under a bouncing `GiVolume` and the machine allows it; absent when the machine says no, when
    /// the volume is authored at `bounce: 0.0`, and when nothing is marked. `dump-graph` is the
    /// assertion surface, and it needs no GPU: the predicate is pure scene data.
    ///
    /// **Explicit non-goal: masked-marked and unmarked are NOT asserted byte-equal.** The marker does
    /// two things — it injects the light *and* removes it from the static bake — and only the first
    /// is a machine setting. The second is world-state semantics the quality knob must not reach
    /// (`Plan/gi-wave3-dynamic-injection.md` §2 Gate C), so a masked marked scene legitimately
    /// differs from the same scene unmarked. What must hold instead is the additive identity the
    /// test below measures.
    #[test]
    fn the_pass_exists_exactly_when_the_scene_and_the_machine_both_ask() {
        let grid = room_grid();
        let marked = vec![light(Vec3::new(8.0, 7.0, 8.0), Vec3::ONE, 5.0, true)];

        let bouncing = scene_over(&grid, Some(gi_data(&grid, 4.0, 1.0)), marked.clone());
        assert!(schedules_injection(&bouncing, &Quality::FULL));
        assert!(!schedules_injection(&bouncing, &NO_INJECTION));
        assert!(
            !schedules_injection(&bouncing, &Quality::LOW),
            "`LOW` is the shipped preset that turns the effect off",
        );

        let unlit = scene_over(&grid, Some(gi_data(&grid, 4.0, 0.0)), marked);
        assert!(
            !schedules_injection(&unlit, &Quality::FULL),
            "the injected term IS the bounce term, so `bounce: 0.0` gets none of it",
        );

        let unmarked = scene_over(
            &grid,
            Some(gi_data(&grid, 4.0, 1.0)),
            vec![light(Vec3::new(8.0, 7.0, 8.0), Vec3::ONE, 5.0, false)],
        );
        assert!(!schedules_injection(&unmarked, &Quality::FULL));

        // And the volume the pass writes is the probe lattice, not a resolution-derived size.
        let chain = PostChain::for_scene(&bouncing);
        assert_eq!(chain.gi_inject, Some(gi_data(&grid, 4.0, 1.0).count));
    }

    /// **Gate B / §11 T3 — the additive identity, through live bindings.** A marked light placed
    /// where no cached hit is within its range makes the pass run, the volume allocate, and every
    /// probe be written — with all zeros. The shader then takes the branch and adds exactly `+0.0`,
    /// so the frame is byte-identical to the same scene with the machine's switch off.
    ///
    /// The non-vacuity guard is the first half: without it, "identical" would also be satisfied by a
    /// pass that never ran.
    #[test]
    fn needs_gpu_a_marked_light_out_of_every_hits_range_writes_zeros_and_moves_no_byte() {
        let Some(gpu) = skip_or_gpu("a_marked_light_out_of_every_hits_range_writes_zeros") else {
            return;
        };
        let grid = room_grid();
        let gi = gi_data(&grid, 4.0, 1.0);
        // High above the slab and short-ranged: every cell the bake's rays hit is at least six units
        // below it, so every per-light term is the hard zero at range.
        let far = light(Vec3::new(8.0, 40.0, 8.0), Vec3::ONE, 1.5, true);

        let volume = inject_and_read(&gpu, &grid, &gi, &[far], BinBuild::Culled);
        assert_eq!(
            volume.len(),
            (gi.count[0] * gi.count[1] * gi.count[2]) as usize,
            "the pass writes one texel per probe — every probe, every frame",
        );
        assert!(
            volume.iter().all(|probe| *probe == [0.0; 4]),
            "a light out of range of every cached hit deposits exactly nothing",
        );

        let Some(mut renderer) = skip_or_renderer("a_marked_light_out_of_every_hits_range") else {
            return;
        };
        let scene = scene_over(&grid, Some(gi), vec![far]);
        assert!(
            schedules_injection(&scene, &Quality::FULL),
            "the pass must actually run, or this is Gate A wearing Gate B's name",
        );
        renderer.set_quality(Quality::FULL, QualityParams::FULL);
        let on = renderer
            .render(&scene, FrameSize::native(64, 64))
            .expect("render succeeds");
        renderer.set_quality(NO_INJECTION, QualityParams::FULL);
        let off = renderer
            .render(&scene, FrameSize::native(64, 64))
            .expect("render succeeds");
        assert_eq!(
            on.rgba, off.rgba,
            "an all-zero injection must add exactly +0.0 — through live bindings and a taken branch",
        );
    }

    /// **The bin oracle (§11 T8, dynamic half).** The same frame injected through the same shader
    /// with the culled bin and with the degenerate all-lights-in-every-cell one must produce
    /// byte-identical volumes — the conservatism claim, measured rather than argued: a light the bin
    /// drops was out of range of the cell being shaded, so its term was exactly `+0.0`.
    #[test]
    fn needs_gpu_a_binned_injection_and_a_degenerate_bin_write_the_same_bytes() {
        let Some(gpu) = skip_or_gpu("a_binned_injection_and_a_degenerate_bin_write_the_same_bytes")
        else {
            return;
        };
        let grid = room_grid();
        let gi = gi_data(&grid, 2.0, 1.0);

        // Six short-ranged movers spread through the room — the regime the bin exists for, where
        // most cells see only some of them.
        let lights: Vec<PointLightData> = [
            (Vec3::new(-5.0, 6.0, -5.0), Vec3::new(1.0, 0.6, 0.3)),
            (Vec3::new(5.0, 6.0, -5.0), Vec3::new(0.3, 1.0, 0.5)),
            (Vec3::new(-5.0, 6.0, 5.0), Vec3::new(0.4, 0.5, 1.0)),
            (Vec3::new(5.0, 6.0, 5.0), Vec3::new(1.0, 1.0, 0.4)),
            (Vec3::new(0.0, 7.0, 0.0), Vec3::new(0.8, 0.8, 0.8)),
            (Vec3::new(0.0, 6.0, -5.0), Vec3::new(1.0, 0.2, 0.2)),
        ]
        .into_iter()
        .map(|(position, color)| light(position, color, 4.0, true))
        .collect();

        // Non-vacuity, stated before the comparison: over the cells the replay actually shades, the
        // culled bin must both light some and drop lights at some, or the two runs below are the
        // same loop wearing different names.
        let premultiplied_lights: Vec<Light> = lights.iter().map(premultiplied).collect();
        let bin = build_bin(&grid, &premultiplied_lights, BinBuild::Culled);
        let mut lit = 0usize;
        let mut culled_somewhere = 0usize;
        for z in 0..grid.extent[2] {
            for y in 0..grid.extent[1] {
                for x in 0..grid.extent[0] {
                    let centre = grid.origin
                        + Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5)
                            * grid.cell_size;
                    let held = bin.table[crate::gi_bin::bin_cell_of(&bin.params, centre)][1];
                    lit += usize::from(held > 0);
                    culled_somewhere += usize::from((held as usize) < lights.len());
                }
            }
        }
        assert!(
            lit > 0 && culled_somewhere > 0,
            "of {} shaded cells, {lit} see a light and {culled_somewhere} see fewer than all {} — \
             the oracle needs both to be non-zero",
            grid.cell_count(),
            lights.len(),
        );

        let culled = inject_and_read(&gpu, &grid, &gi, &lights, BinBuild::Culled);
        let degenerate = inject_and_read(&gpu, &grid, &gi, &lights, BinBuild::Degenerate);
        assert!(
            culled.iter().any(|probe| probe[0] > 0.0),
            "the movers must actually deposit light, or the comparison is two black volumes agreeing",
        );
        assert_eq!(
            culled, degenerate,
            "the binned injection must be byte-identical to the all-lights-in-every-cell oracle — a \
             difference is a conservatism hole in the bin",
        );
    }

    /// **The plumbing is live.** A marked light inside the room deposits bounce into the volume, and
    /// the frame that samples it differs from the same frame with the machine's switch off — end to
    /// end: the graph's compute pass, the transient volume, the group-3 binding, the `gi_bounce.y`
    /// flag and the shader's nested add.
    ///
    /// It is the guard the two identity gates cannot be: every assertion they make would also pass
    /// on a build whose injection was wired to nothing.
    #[test]
    fn needs_gpu_a_marked_light_in_the_room_deposits_bounce_and_changes_the_frame() {
        let Some(gpu) = skip_or_gpu("a_marked_light_in_the_room_deposits_bounce") else {
            return;
        };
        let grid = room_grid();
        let gi = gi_data(&grid, 4.0, 1.0);
        let mover = light(Vec3::new(0.0, 5.0, 0.0), Vec3::ONE, 8.0, true);

        let volume = inject_and_read(&gpu, &grid, &gi, &[mover], BinBuild::Culled);
        let peak = volume
            .iter()
            .map(|probe| probe[0])
            .fold(0.0f32, |a, b| a.max(b));
        assert!(
            peak > 0.01,
            "a mover inside a white room must light some probe; the peak DC was {peak}",
        );

        let Some(mut renderer) = skip_or_renderer("a_marked_light_in_the_room_deposits_bounce")
        else {
            return;
        };
        let scene = scene_over(&grid, Some(gi), vec![mover]);
        renderer.set_quality(Quality::FULL, QualityParams::FULL);
        let on = renderer
            .render(&scene, FrameSize::native(64, 64))
            .expect("render succeeds");
        renderer.set_quality(NO_INJECTION, QualityParams::FULL);
        let off = renderer
            .render(&scene, FrameSize::native(64, 64))
            .expect("render succeeds");
        assert_ne!(
            on.rgba, off.rgba,
            "the injected term must reach pixels — otherwise the identity gates prove nothing",
        );
        // And it *adds*: the term is non-negative everywhere, so no channel may darken.
        assert!(
            on.rgba
                .iter()
                .zip(off.rgba.iter())
                .all(|(lit, dark)| lit >= dark),
            "one bounce of dynamic light is an addition; no channel may lose value to it",
        );
    }

    /// **The two GI quality axes each move a `GiVolume` frame, and neither reaches a scene without
    /// one.** `a_lowered_parameter_makes_each_effect_cheaper_and_reaches_nothing_else` is this
    /// test's sibling for the post axes; the GI pair needs its own fixture because it is the only one
    /// whose subject is a *bake* rather than a screen-space pass.
    ///
    /// Each axis is moved **alone**, off `FULL`, so a moved frame can only be that axis's doing. The
    /// control is the same scene with no `GiVolume`: nothing there reads either count, so both must
    /// leave it byte-identical — which is what catches a scale or a ray count leaking into a code
    /// path that has no probes in it.
    ///
    /// The injection is deliberately *not* the subject: the fixture leaves `Quality::FULL` alone, so
    /// both frames run the same passes and the difference is inside the bake and its lattice. That is
    /// also why these axes stay observable at `LOW`, where `gi_dynamic: false` removes the injection
    /// and leaves the bake running.
    #[test]
    fn needs_gpu_a_lowered_gi_ray_count_and_probe_scale_each_move_a_gi_frame() {
        let Some(mut renderer) =
            skip_or_renderer("a_lowered_gi_ray_count_and_probe_scale_each_move_a_gi_frame")
        else {
            return;
        };
        let grid = room_grid();
        let size = FrameSize::native(64, 64);
        let lit = scene_over(
            &grid,
            Some(gi_data(&grid, 2.0, 1.0)),
            vec![light(Vec3::new(0.0, 5.0, 0.0), Vec3::ONE, 8.0, true)],
        );
        let control = scene_over(
            &grid,
            None,
            vec![light(Vec3::new(0.0, 5.0, 0.0), Vec3::ONE, 8.0, true)],
        );

        renderer.set_quality(Quality::FULL, QualityParams::FULL);
        let full = renderer.render(&lit, size).expect("a full-quality render");
        let no_volume = renderer
            .render(&control, size)
            .expect("a volume-free render");

        for (axis, params) in [
            (
                "gi_rays",
                QualityParams {
                    gi_rays: QualityParams::LOW.gi_rays,
                    ..QualityParams::FULL
                },
            ),
            (
                "gi_probe_scale",
                QualityParams {
                    gi_probe_scale: QualityParams::LOW.gi_probe_scale,
                    ..QualityParams::FULL
                },
            ),
        ] {
            renderer.set_quality(Quality::FULL, params);
            assert_ne!(
                renderer.render(&lit, size).expect("a cheaper render").rgba,
                full.rgba,
                "a lowered `{axis}` must move a frame whose scene authors a GiVolume",
            );
            assert_eq!(
                renderer
                    .render(&control, size)
                    .expect("a volume-free render")
                    .rgba,
                no_volume.rgba,
                "`{axis}` must not reach a scene with no probes to bake",
            );
        }
    }

    /// **The replayed directions span the sphere**, which is the whole reason the cache strides.
    ///
    /// The bake's ray set walks elevation monotonically — `y = 1 − 2(i + ½)/RAYS` — so a *contiguous*
    /// sixteen of sixty-four would be a 59°-half-angle cone about `+Y` (ray 0 at `+0.984`, ray 15 at
    /// `+0.516`), and a probe would replay its ceiling and never its floor. Only the golden-angle
    /// azimuth is well-distributed at any prefix length; the elevation sweep beside it is not.
    ///
    /// Taking every `gi_rays / HIT_RAYS`'th ray instead costs nothing and fixes it, and this measures
    /// the fix rather than asserting it: the sixteen slots must reach both poles and average to
    /// nothing.
    #[test]
    fn the_replayed_ray_set_spans_the_sphere() {
        // `gi_rays.wgsl`'s `ray_direction`, mirrored — the same formula over the same slot→ray
        // mapping, so this measures the shader's own directions rather than a paraphrase of them.
        // At full quality, which is the count the fixtures bake at; the stride is derived from it
        // exactly as both shaders derive theirs.
        let elevation = |i: u32| 1.0 - 2.0 * (i as f32 + 0.5) / FULL_RAYS as f32;
        let stride = FULL_RAYS / HIT_RAYS;
        let cached: Vec<f32> = (0..HIT_RAYS).map(|k| elevation(k * stride)).collect();

        let (lowest, highest) = cached
            .iter()
            .fold((1.0f32, -1.0f32), |(lo, hi), &y| (lo.min(y), hi.max(y)));
        assert!(
            lowest < -0.5 && highest > 0.5,
            "the cached directions must reach both poles, not crowd one; they spanned \
             [{lowest}, {highest}]",
        );
        let mean: f32 = cached.iter().sum::<f32>() / cached.len() as f32;
        assert!(
            mean.abs() < 0.05,
            "and must not lean: the mean elevation was {mean}",
        );
        // The contiguous alternative, stated so the regression this guards against is legible: it
        // reaches only the upper cap.
        let contiguous_lowest = (0..HIT_RAYS).map(elevation).fold(1.0f32, f32::min);
        assert!(
            contiguous_lowest > 0.5,
            "a contiguous prefix is a polar cap — that is why the cache strides",
        );
    }

    /// **The product case, pinned: a light over open ground lights the probes beneath it.**
    ///
    /// A white floor under open sky and one marked mover above it — vigil's tracer bolt over the
    /// cobbles, in miniature. Under a contiguous ray cache this injected **exactly zero at every one
    /// of the 100 probes** (measured, at every range tried); under the strided cache the probes near
    /// the floor carry real irradiance. It is the same scene that found the defect, kept as the gate
    /// that stops it coming back silently.
    #[test]
    fn needs_gpu_a_marked_light_over_open_ground_lights_the_probes_beneath_it() {
        let Some(gpu) = skip_or_gpu("a_marked_light_over_open_ground_lights_the_probes_beneath_it")
        else {
            return;
        };
        let grid = open_slab_grid();
        let gi = gi_data(&grid, 4.0, 1.0);
        let mover = light(Vec3::new(0.0, 1.5, 2.5), Vec3::ONE, 6.0, true);

        let volume = inject_and_read(&gpu, &grid, &gi, &[mover], BinBuild::Culled);
        let peak = volume
            .iter()
            .map(|probe| probe[0])
            .fold(0.0f32, |a, b| a.max(b));
        let carrying = volume.iter().filter(|probe| probe[0] > 0.0).count();
        eprintln!(
            "open-ground injection: {carrying}/{} probes carry light, peak DC {peak:.5} \
             (a contiguous ray cache measured 0/{} and 0.0)",
            volume.len(),
            volume.len(),
        );
        assert!(
            peak > 0.02 && carrying >= 4,
            "a mover 2.5 units over a white floor must light the probes around it; {carrying} of \
             {} probes carried anything and the peak DC was {peak}",
            volume.len(),
        );

        // The light is the only source: unmarked, the same scene injects nothing at all.
        let quiet = inject_and_read(
            &gpu,
            &grid,
            &gi,
            &[light(Vec3::new(0.0, 1.5, 2.5), Vec3::ONE, 6.0, false)],
            BinBuild::Culled,
        );
        assert!(
            quiet.iter().all(|probe| *probe == [0.0; 4]),
            "an unmarked light is the bake's business, and must deposit nothing here",
        );
    }

    /// **The ray count is one value reaching two shaders, and it is no longer a constant in either.**
    ///
    /// It rides the bake's `params.flags.y` and the injection's `params.rays.x`, both filled from
    /// [`ResolvedGi::rays`](crate::gi_bake::ResolvedGi::rays) — the count the bake behind the hit
    /// cache actually traced. A constant left in either file would be a second answer, and the
    /// injection's is the dangerous one: it divides by the count to recover a cached slot's
    /// direction, so a stale literal reconstructs directions the march never took, silently, with no
    /// binding to fail. The stride both derive from it is the same thing one level down.
    #[test]
    fn neither_gi_shader_hardcodes_the_ray_count_or_its_cache_stride() {
        for (file, source) in [
            ("gi_bake.wgsl", include_str!("shaders/gi_bake.wgsl")),
            ("gi_inject.wgsl", include_str!("shaders/gi_inject.wgsl")),
            (
                "gi_rays.wgsl",
                include_str!("shaders/snippets/gi_rays.wgsl"),
            ),
        ] {
            for name in ["RAYS", "CACHE_STRIDE"] {
                assert!(
                    !source.lines().any(|line| line
                        .trim_start()
                        .starts_with(&format!("const {name}: u32 = "))),
                    "`{file}` must not declare `const {name}`: the ray count is a quality axis the \
                     host supplies and the stride is derived from it",
                );
            }
        }
        // The uniform each one reads it through, so the pair cannot drift into two sources.
        assert!(include_str!("shaders/gi_bake.wgsl").contains("params.flags.y"));
        assert!(include_str!("shaders/gi_inject.wgsl").contains("params.rays.x"));
        // The relation the stride is derived under, on the CPU side: the cache's slot budget divides
        // the count wholly, or the slots stop spanning the sphere.
        assert_eq!(QualityParams::FULL.gi_rays % HIT_RAYS, 0);
    }

    /// The two GI shaders share the hit cache's slot budget and its packed-word layout, and neither
    /// can move alone: the injection unpacks words the bake wrote. Shader-bindgen unifies the
    /// constants both declare (a disagreement fails the regeneration), so what is left to hold is
    /// the CPU's own declarations against the reflected ones.
    #[test]
    fn the_two_gi_shaders_agree_on_the_ray_set_and_the_hit_layout() {
        assert_eq!(crate::shader_gen::HIT_RAYS, HIT_RAYS);
        for (reflected, cpu) in [
            (crate::shader_gen::HIT_FACE_SHIFT, HIT_FACE_SHIFT),
            (crate::shader_gen::HIT_FACE_BOUNDARY, HIT_FACE_BOUNDARY),
            (crate::shader_gen::HIT_FACE_ESCAPED, HIT_FACE_ESCAPED),
        ] {
            assert_eq!(reflected, cpu);
        }
    }

    /// Bakes `grid` (so the hit cache exists), then injects `lights` and reads the volume back
    /// through [`GiInject::resolve_and_read`] — the production readback this module ships for
    /// `ludion render --dump-gi`, exercised here rather than duplicated, so a fixture and the real
    /// dump path can never quietly diverge.
    fn inject_and_read(
        gpu: &Gpu,
        grid: &WorldVoxelGrid,
        gi: &GiData,
        lights: &[PointLightData],
        build: BinBuild,
    ) -> Vec<[f32; 4]> {
        let voxel = VoxelBake::new()
            .resolve(gpu, Some(grid))
            .expect("a Some grid uploads its textures");
        let bake = GiBake::new(gpu);
        let statics = static_lights(lights);
        let resolved = bake.resolve(
            gpu,
            Some(BakeInput {
                grid,
                gi,
                albedo: &voxel.albedo,
                occupancy: &voxel.emissive_occupancy,
                lights: &statics,
                sun: None,
                rays: FULL_RAYS,
            }),
        );
        let hits = resolved
            .hits
            .as_ref()
            .expect("a bouncing bake writes its hit cache");

        let inject = GiInject::new(gpu);
        inject.set_bin_build(build);
        inject
            .resolve_and_read(
                gpu,
                InjectInput {
                    grid,
                    gi,
                    albedo: &voxel.albedo,
                    hits,
                    lights,
                    // The bake's own count, not the fixture's — the replay divides by what the march
                    // traced, and taking it from the resolved volumes is what makes that structural.
                    rays: resolved.rays,
                },
            )
            .expect("the readback succeeds")
    }
}
