//! The rain draw: one instanced draw of streak quads decoded from the instance index, parameters in one
//! uniform. See `shaders/rain.wgsl` and [`crate::rain`].

use super::{
    CAMERA_GROUP, FrameRecorder, HDR_FORMAT, LIGHTS_GROUP, LayoutId, Renderer, ShaderId,
    depth_state, init_buffer, module, premultiplied_blend, shader_gen, uniform_bind_group,
};
use crate::graph::FrameSize;
use crate::rain::rain_layout;
use crate::scene::RenderScene;

/// Group of the rain's own uniform.
const RAIN_GROUP: u32 = shader_gen::RAIN_GROUP;

/// The frame's rain: the uniform's bind group and the instance count.
pub(super) struct RainDraw {
    params_bg: wgpu::BindGroup,
    instances: u32,
}

impl Renderer {
    /// Uploads the scene's rain, or `None` when it has none or nothing to draw.
    pub(super) fn build_rain_draw(
        &self,
        scene: &RenderScene,
        size: FrameSize,
        device: &wgpu::Device,
    ) -> Option<RainDraw> {
        let rain = scene.rain.as_ref()?;
        let layout = rain_layout(rain, scene.camera.position);
        if layout.instances == 0 {
            return None;
        }
        let proj_y = scene.camera.projection.y_axis.y.abs().max(1e-6);
        let pixel = 2.0 / (proj_y * size.render.1.max(1) as f32);
        let params = shader_gen::RainParams {
            base: [layout.base.x, layout.base.y, layout.base.z, 0],
            counts: [layout.n_xz, layout.n_y, layout.per_tile, rain.seed],
            shape: [
                rain.cell,
                rain.time_s,
                rain.radius,
                if rain.lit { 1.0 } else { 0.0 },
            ],
            band: [
                rain.height[0],
                rain.height[1],
                rain.fall_speed[0],
                rain.fall_speed[1],
            ],
            wind: [rain.wind[0], rain.wind[1], rain.wind[2], rain.gust],
            look: [rain.drop_width, rain.streak / 60.0, pixel, rain.medium],
            color: rain.color,
        };
        let buffer = init_buffer(
            device,
            wgpu::BufferUsages::UNIFORM,
            bytemuck::bytes_of(&params),
        );
        Some(RainDraw {
            params_bg: uniform_bind_group(device, self.layouts.get(LayoutId::Rain), &buffer),
            instances: layout.instances,
        })
    }
}

impl FrameRecorder<'_> {
    /// Draws the rain inside a recording pass: the colour pass when `layer` is `None`, else the
    /// transparent layer of `Some(format)`.
    pub(super) fn record_rain(
        &self,
        rp: &mut wgpu::RenderPass<'_>,
        layer: Option<ludion_render_graph::TextureFormat>,
    ) {
        let Some(draw) = &self.rain_draw else {
            return;
        };
        rp.set_pipeline(match layer {
            None => self.r.pipeline(ShaderId::Rain),
            Some(format) => self.r.transparent_depth_pipeline(ShaderId::Rain, format),
        });
        rp.set_bind_group(CAMERA_GROUP, &self.camera_bg, &[]);
        rp.set_bind_group(LIGHTS_GROUP, &self.lights_bg, &[]);
        rp.set_bind_group(RAIN_GROUP, &draw.params_bg, &[]);
        rp.draw(0..6, 0..draw.instances);
    }
}

/// `LessEqual` with no depth write, premultiplied: the particle pass's policy. Two-sided.
pub(super) fn build_rain_pipeline(
    device: &wgpu::Device,
    label: &str,
    src: &str,
    layout: &wgpu::PipelineLayout,
) -> wgpu::RenderPipeline {
    let shader = module(device, label, src);
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: Some(premultiplied_blend()),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(depth_state(false, wgpu::CompareFunction::LessEqual)),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}
