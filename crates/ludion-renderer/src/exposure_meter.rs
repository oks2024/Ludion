//! The exposure meter kernel (`shaders/exposure_meter.wgsl`, `Plan/post-film-and-screen-effects.md`
//! decisions 16-17): one workgroup bins a fixed grid of the HDR `scene` into the 65x1 `exposure`
//! transient that `composite` reads as an optional input. A frame that does not meter binds
//! [`ExposureMeter::fallback`] there and never reads it.

use ludion_render_graph::Gpu;
use wgpu::util::DeviceExt;

use crate::compute_shader::ComputeShader;
use crate::scene::{MeterData, MeterWeighting};
use crate::shader_gen::{self, MeterParams};

/// The meter compute pipeline and the 1x1 table a frame without a meter binds to `composite`.
pub(crate) struct ExposureMeter {
    pipeline: wgpu::ComputePipeline,
    fallback: wgpu::TextureView,
}

/// One frame's meter upload: the params uniform.
pub(crate) struct MeterFrame {
    params: wgpu::Buffer,
}

impl ExposureMeter {
    pub(crate) fn new(gpu: &Gpu) -> ExposureMeter {
        let device = gpu.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("exposure meter"),
            source: wgpu::ShaderSource::Wgsl(ComputeShader::ExposureMeter.composed().into()),
        });
        // Auto-derived layout, as `ProbeRelight::new`.
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("exposure meter"),
            layout: None,
            module: &module,
            entry_point: Some("meter_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let fallback = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("exposure meter fallback"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        ExposureMeter { pipeline, fallback }
    }

    /// Uploads the params for one frame's dispatch. `viewmodel_slice` is the frame's viewmodel depth
    /// slice (0 without a viewmodel): pixels inside it weigh nothing.
    pub(crate) fn prepare(
        &self,
        device: &wgpu::Device,
        meter: MeterData,
        viewmodel_slice: f32,
    ) -> MeterFrame {
        let params = MeterParams {
            clip: meter.clip,
            range: meter.range,
            center: u32::from(meter.weighting == MeterWeighting::Center),
            viewmodel_slice,
            pad0: 0,
            pad1: 0,
        };
        MeterFrame {
            params: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("exposure meter params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            }),
        }
    }

    /// The bind group for one dispatch, over the frame's HDR `scene`, `scene_depth` and the table
    /// `out`. Built before the compute pass is opened, because a `RecordedPass` lends its encoder
    /// mutably.
    pub(crate) fn bind_group(
        &self,
        device: &wgpu::Device,
        frame: &MeterFrame,
        scene: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        out: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("exposure meter"),
            layout: &self.pipeline.get_bind_group_layout(shader_gen::METER_GROUP),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: shader_gen::METER_SCENE_BINDING,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::METER_DEPTH_BINDING,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::METER_OUT_BINDING,
                    resource: wgpu::BindingResource::TextureView(out),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::METER_BINDING,
                    resource: frame.params.as_entire_binding(),
                },
            ],
        })
    }

    /// Issues the one workgroup into an already-opened compute pass.
    pub(crate) fn dispatch(&self, pass: &mut wgpu::ComputePass<'_>, bind_group: &wgpu::BindGroup) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(shader_gen::METER_GROUP, bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }

    /// The 1x1 table a frame that does not meter binds to `composite`; its `metered` flag is zero,
    /// so nothing reads it.
    pub(crate) fn fallback(&self) -> &wgpu::TextureView {
        &self.fallback
    }
}
