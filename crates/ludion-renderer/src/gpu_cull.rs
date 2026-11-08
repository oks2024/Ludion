//! The GPU frustum cull (`shaders/cull.wgsl`, `Plan/gpu-driven-culling.md` decisions 11-16): per
//! (view, instance) one visibility bit, the survivors compacted per (view, bucket) in input order, and
//! one indirect-args slot per (view, bucket). The CPU cull (`cull.rs`) is its oracle.
//!
//! Two invariants carry the design. The compaction is a scan, never an atomic append, so survivors
//! keep input order and a frame draws identically however the threads interleave. And the args slot
//! `[elements, visible, 0, 0, 0]` reads as a `DrawIndexedIndirectArgs` and, as its first four words, a
//! `DrawIndirectArgs`; `first_instance` stays 0, so the draw binds `cull_out` at the bucket's offset
//! instead of needing `INDIRECT_FIRST_INSTANCE`.

use ludion_core::math::glam::Mat4;
use wgpu::util::DeviceExt;

use crate::cull::{Aabb, CullBoundsRaw, Frustum, bounds_slot};
use crate::graph::CullShape;
use crate::render::MeshInstanceRaw;
use crate::scene::MeshInstance;
use crate::shader_gen::{self, CullBucket, CullItem, CullParams, CullView};

pub(crate) use crate::shader_gen::{SLOT_ALWAYS, SLOT_OVERRIDE};

const _: () = assert!(
    std::mem::size_of::<MeshInstanceRaw>() == std::mem::size_of::<shader_gen::CullInstance>()
);
const _: () =
    assert!(std::mem::size_of::<CullBoundsRaw>() == std::mem::size_of::<shader_gen::CullBox>());

/// One view of the cull: its frustum, and whether it admits only shadow casters (every shadow view does).
#[derive(Clone, Copy)]
pub(crate) struct CullViewSpec {
    pub(crate) frustum: Frustum,
    pub(crate) casters_only: bool,
}

/// The cull's views in the order the shadow passes address them: the camera, one casters-only view
/// per sun map, then one per granted local view; and how many views past `shape.views` were dropped.
/// A dropped view is not culled and draws every caster directly.
pub(crate) fn cull_views(
    camera: Mat4,
    sun_maps: &[Mat4],
    local: impl Iterator<Item = Frustum>,
    shape: CullShape,
) -> (Vec<CullViewSpec>, u32) {
    let shadow = |frustum| CullViewSpec {
        frustum,
        casters_only: true,
    };
    let mut views = vec![CullViewSpec {
        frustum: Frustum::from_view_proj(camera),
        casters_only: false,
    }];
    views.extend(
        sun_maps
            .iter()
            .map(|&map| shadow(Frustum::from_view_proj(map))),
    );
    views.extend(local.map(shadow));
    let over = (views.len() as u32).saturating_sub(shape.views);
    views.truncate(shape.views as usize);
    (views, over)
}

/// The names of the first `kept` cull views, in `cull_views` order: `camera`; `cascade_i` per sun
/// map when the sun is cascaded, else `sun`; then `local_i` per local view.
pub(crate) fn cull_view_names(
    sun_maps: usize,
    cascaded: bool,
    local: usize,
    kept: usize,
) -> Vec<String> {
    let sun = (0..sun_maps).map(|i| {
        if cascaded {
            format!("cascade_{i}")
        } else {
            "sun".to_string()
        }
    });
    let local = (0..local).map(|i| format!("local_{i}"));
    std::iter::once("camera".to_string())
        .chain(sun)
        .chain(local)
        .take(kept)
        .collect()
}

/// The cull item of `mesh` in `bucket`, from its entry `bound` in `cull::instance_bounds`: the item
/// rule that makes the GPU's visible set the oracle's, "unknown, or admitted".
pub(crate) fn cull_item(mesh: &MeshInstance, bucket: u32, bound: Option<Aabb>) -> CullItem {
    let (min, max, slot) = match bound {
        None => ([0.0; 3], [0.0; 3], SLOT_ALWAYS),
        Some(b) if mesh.bone_offset != 0 => (b.min.to_array(), b.max.to_array(), SLOT_OVERRIDE),
        Some(_) => ([0.0; 3], [0.0; 3], bounds_slot(mesh.shape)),
    };
    CullItem {
        min,
        slot,
        max,
        bucket,
        casts: mesh.cast_shadows as u32,
        pad0: 0,
        pad1: 0,
        pad2: 0,
    }
}

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

/// The five compute pipelines over one shared layout: group 0 the inputs, group 1 the outputs.
pub(crate) struct CullPipelines {
    device: wgpu::Device,
    inputs: wgpu::BindGroupLayout,
    outputs: wgpu::BindGroupLayout,
    test: wgpu::ComputePipeline,
    reduce: wgpu::ComputePipeline,
    scan: wgpu::ComputePipeline,
    scatter: wgpu::ComputePipeline,
    args: wgpu::ComputePipeline,
}

impl CullPipelines {
    pub(crate) fn new(device: &wgpu::Device) -> CullPipelines {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cull"),
            source: wgpu::ShaderSource::Wgsl(
                crate::compute_shader::ComputeShader::Cull.composed().into(),
            ),
        });
        let inputs = shader_gen::create_cull_params_cull_views_cull_in_cull_items_cull_boxes_cull_buckets_bgl(device);
        let outputs = shader_gen::create_cull_scratch_cull_out_cull_args_bgl(device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cull"),
            bind_group_layouts: &[Some(&inputs), Some(&outputs)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };
        CullPipelines {
            device: device.clone(),
            test: pipeline("cs_test"),
            reduce: pipeline("cs_reduce"),
            scan: pipeline("cs_scan"),
            scatter: pipeline("cs_scatter"),
            args: pipeline("cs_args"),
            inputs,
            outputs,
        }
    }
}

/// One world draw as the cull sees it: its range of the packed instance buffer, its draw element
/// count (vertices of a primitive, indices of an asset) and the `scene.meshes` index of each instance.
pub(crate) struct WorldBucket<'a> {
    pub(crate) first: u32,
    pub(crate) count: u32,
    pub(crate) elements: u32,
    pub(crate) sources: &'a [u32],
}

/// What [`world_cull`] reads.
pub(crate) struct WorldCullInput<'a> {
    pub(crate) instances: &'a wgpu::Buffer,
    pub(crate) meshes: &'a [MeshInstance],
    /// `cull::instance_bounds` of `meshes`.
    pub(crate) bounds: &'a [Option<Aabb>],
    pub(crate) buckets: &'a [WorldBucket<'a>],
    /// The views in order: the camera (the one view-projection its uniform uses), then the shadow views.
    pub(crate) views: &'a [CullViewSpec],
    /// Whether the stats ring is armed: names and tested counts are built only then.
    pub(crate) stats: bool,
    /// One name per view in `views`, for the stats readback.
    pub(crate) names: Vec<String>,
    pub(crate) bounds_table: &'a wgpu::Buffer,
    pub(crate) shape: CullShape,
}

/// Prepares the cull over the frame's world draws, one bucket each, in packing order.
pub(crate) fn world_cull(
    device: &wgpu::Device,
    pipelines: &CullPipelines,
    input: WorldCullInput<'_>,
) -> CullFrame {
    let WorldCullInput {
        instances,
        meshes,
        bounds,
        buckets,
        views,
        stats,
        names,
        bounds_table,
        shape,
    } = input;
    let items: Vec<CullItem> = buckets
        .iter()
        .enumerate()
        .flat_map(|(b, bucket)| {
            bucket
                .sources
                .iter()
                .map(move |&src| (b as u32, src as usize))
        })
        .map(|(b, src)| cull_item(&meshes[src], b, bounds[src]))
        .collect();
    let cull_buckets: Vec<CullBucket> = buckets
        .iter()
        .map(|bucket| CullBucket {
            first: bucket.first,
            count: bucket.count,
            elements: bucket.elements,
            pad0: 0,
        })
        .collect();
    // A casters-only view tests only the items that cast; the camera view tests every item.
    let tested = if stats {
        views
            .iter()
            .map(|v| {
                items
                    .iter()
                    .filter(|item| !v.casters_only || item.casts != 0)
                    .count() as u32
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut frame = CullFrame::prepare(
        device,
        pipelines,
        CullInput {
            instances,
            items: &items,
            buckets: &cull_buckets,
            views,
            bounds_table,
            shape,
        },
    );
    frame.names = if stats { names } else { Vec::new() };
    frame.tested = tested;
    frame
}

/// What a frame's cull reads.
pub(crate) struct CullInput<'a> {
    /// The packed instances, bucket after bucket (`MeshInstanceRaw`s, `STORAGE`).
    pub(crate) instances: &'a wgpu::Buffer,
    /// One item per instance.
    pub(crate) items: &'a [CullItem],
    /// One entry per bucket: its `[first, first + count)` range and draw element count.
    pub(crate) buckets: &'a [CullBucket],
    /// At most `shape.views` of them: the live views.
    pub(crate) views: &'a [CullViewSpec],
    /// The persistent `cull::bounds_table` buffer.
    pub(crate) bounds_table: &'a wgpu::Buffer,
    pub(crate) shape: CullShape,
}

/// Where a frame's cull writes: the graph's buffers (sized by [`CullShape`]).
pub(crate) struct CullTargets<'a> {
    pub(crate) scratch: &'a wgpu::Buffer,
    pub(crate) instances: &'a wgpu::Buffer,
    pub(crate) args: &'a wgpu::Buffer,
}

/// One frame's uploaded cull inputs and the group-0 bind group over them.
pub(crate) struct CullFrame {
    inputs: wgpu::BindGroup,
    shape: CullShape,
    instances: u32,
    buckets: u32,
    views: u32,
    names: Vec<String>,
    tested: Vec<u32>,
}

impl CullFrame {
    pub(crate) fn prepare(
        device: &wgpu::Device,
        pipelines: &CullPipelines,
        input: CullInput<'_>,
    ) -> CullFrame {
        let shape = input.shape;
        debug_assert!(input.items.len() <= shape.instances as usize);
        debug_assert!(input.buckets.len() <= shape.buckets as usize);
        debug_assert!(input.views.len() <= shape.views as usize);
        let params = CullParams {
            instance_count: input.items.len() as u32,
            bucket_count: input.buckets.len() as u32,
            view_count: input.views.len() as u32,
            instance_cap: shape.instances,
            bucket_cap: shape.buckets,
            word_cap: shape.words(),
            block_cap: shape.blocks(),
            pad0: 0,
        };
        let views: Vec<CullView> = input
            .views
            .iter()
            .map(|v| CullView {
                planes: v.frustum.planes().map(|p| p.to_array()),
                casters_only: v.casters_only as u32,
                pad0: 0,
                pad1: 0,
                pad2: 0,
            })
            .collect();
        let storage = |label: &str, bytes: &[u8]| {
            // A zero-size binding is invalid: an empty array uploads one zeroed element.
            let mut padded = bytes.to_vec();
            if padded.is_empty() {
                padded.resize(16, 0);
            }
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: &padded,
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cull params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let views_buf = storage("cull views", bytemuck::cast_slice(&views));
        let items_buf = storage("cull items", bytemuck::cast_slice(input.items));
        let buckets_buf = storage("cull buckets", bytemuck::cast_slice(input.buckets));
        let inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cull inputs"),
            layout: &pipelines.inputs,
            entries: &[
                entry(shader_gen::CULL_PARAMS_BINDING, &params_buf),
                entry(shader_gen::CULL_VIEWS_BINDING, &views_buf),
                entry(shader_gen::CULL_IN_BINDING, input.instances),
                entry(shader_gen::CULL_ITEMS_BINDING, &items_buf),
                entry(shader_gen::CULL_BOXES_BINDING, input.bounds_table),
                entry(shader_gen::CULL_BUCKETS_BINDING, &buckets_buf),
            ],
        });
        CullFrame {
            inputs,
            shape,
            instances: input.items.len() as u32,
            buckets: input.buckets.len() as u32,
            views: input.views.len() as u32,
            names: Vec::new(),
            tested: Vec::new(),
        }
    }

    /// The live buckets: the first `buckets()` args slots of each view are written.
    pub(crate) fn buckets(&self) -> u32 {
        self.buckets
    }

    /// Whether the frame culls nothing: [`encode`](CullFrame::encode) writes no args.
    pub(crate) fn is_empty(&self) -> bool {
        self.instances == 0
    }

    /// Each live view's name (`camera`, `sun`, `cascade_N`, `local_N`).
    pub(crate) fn names(&self) -> &[String] {
        &self.names
    }

    /// Each live view's tested instance count.
    pub(crate) fn tested(&self) -> &[u32] {
        &self.tested
    }

    /// The shape whose strides place each view's slices of the output.
    pub(crate) fn shape(&self) -> CullShape {
        self.shape
    }

    /// The live views: the first `views()` slices of the output are written.
    pub(crate) fn views(&self) -> u32 {
        self.views
    }

    /// Runs the five dispatches in order into an open compute pass; nothing when there is no instance.
    pub(crate) fn encode(
        &self,
        pipelines: &CullPipelines,
        pass: &mut wgpu::ComputePass<'_>,
        targets: CullTargets<'_>,
    ) {
        if self.instances == 0 {
            return;
        }
        let outputs = pipelines
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cull outputs"),
                layout: &pipelines.outputs,
                entries: &[
                    entry(shader_gen::CULL_SCRATCH_BINDING, targets.scratch),
                    entry(shader_gen::CULL_OUT_BINDING, targets.instances),
                    entry(shader_gen::CULL_ARGS_BINDING, targets.args),
                ],
            });
        pass.set_bind_group(shader_gen::CULL_PARAMS_GROUP, &self.inputs, &[]);
        pass.set_bind_group(shader_gen::CULL_SCRATCH_GROUP, &outputs, &[]);
        let views = self.views;
        let words = self.instances.div_ceil(32);
        let blocks = self.instances.div_ceil(256);
        let wg = |n: u32, size: [u32; 3]| n.div_ceil(size[0]);
        for (pipeline, groups) in [
            (
                &pipelines.test,
                wg(words, shader_gen::CULL_CS_TEST_WORKGROUP_SIZE),
            ),
            (
                &pipelines.reduce,
                wg(blocks, shader_gen::CULL_CS_REDUCE_WORKGROUP_SIZE),
            ),
            (&pipelines.scan, 1),
            (
                &pipelines.scatter,
                wg(self.instances, shader_gen::CULL_CS_SCATTER_WORKGROUP_SIZE),
            ),
            (
                &pipelines.args,
                wg(self.buckets, shader_gen::CULL_CS_ARGS_WORKGROUP_SIZE),
            ),
        ] {
            pass.set_pipeline(pipeline);
            pass.dispatch_workgroups(groups, views, 1);
        }
    }
}
