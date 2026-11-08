//! The GPU frustum cull (`shaders/cull.wgsl`, `Plan/gpu-driven-culling.md` decisions 11-16): per
//! (view, instance) one visibility bit, the survivors compacted per (view, bucket) in input order, and
//! one indirect-args slot per (view, bucket). The CPU cull (`cull.rs`) is its oracle.
//!
//! Two invariants carry the design. The compaction is a scan, never an atomic append, so survivors
//! keep input order and a frame draws identically however the threads interleave. And the args slot
//! `[elements, visible, 0, 0, 0]` reads as a `DrawIndexedIndirectArgs` and, as its first four words, a
//! `DrawIndirectArgs`; `first_instance` stays 0, so the draw binds `cull_out` at the bucket's offset
//! instead of needing `INDIRECT_FIRST_INSTANCE`.

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

/// What a frame's cull reads.
pub(crate) struct CullInput<'a> {
    /// The packed instances, bucket after bucket (`MeshInstanceRaw`s, `STORAGE`).
    pub(crate) instances: &'a wgpu::Buffer,
    /// One item per instance.
    pub(crate) items: &'a [CullItem],
    /// One entry per bucket: its `[first, first + count)` range and draw element count.
    pub(crate) buckets: &'a [CullBucket],
    pub(crate) views: &'a [Frustum],
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
        debug_assert_eq!(input.views.len(), shape.views as usize);
        let params = CullParams {
            instance_count: input.items.len() as u32,
            bucket_count: input.buckets.len() as u32,
            view_count: shape.views,
            instance_cap: shape.instances,
            bucket_cap: shape.buckets,
            word_cap: shape.words(),
            block_cap: shape.blocks(),
            pad0: 0,
        };
        let views: Vec<CullView> = input
            .views
            .iter()
            .map(|f| CullView {
                planes: f.planes().map(|p| p.to_array()),
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
        }
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
        let views = self.shape.views;
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
