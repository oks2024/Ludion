//! The rain draw: one instanced draw of streak quads decoded from the instance index, parameters in one
//! uniform. See `shaders/rain.wgsl` and [`crate::rain`].

use super::{
    CAMERA_GROUP, CasterPipeline, FrameRecorder, HDR_FORMAT, LIGHTS_GROUP, LayoutId, RecordedPass,
    Renderer, SHADOW_VIEW_BINDING, SHADOW_VIEW_GROUP, ShaderId, depth_state, init_buffer, module,
    premultiplied_blend, shader_gen, skin_bind_group,
};
use crate::graph::FrameSize;
use crate::rain::{RainData, occlusion_view, rain_layout};
use crate::scene::RenderScene;
use ludion_core::math::glam::Vec3;

/// Group of the rain's own uniform.
const RAIN_GROUP: u32 = shader_gen::RAIN_GROUP;

/// The frame's rain: the uniform, the occlusion view and the instance count. The bind group is built
/// at record time, once the graph has handed over the occlusion map.
pub(super) struct RainDraw {
    params: wgpu::Buffer,
    occlusion_view: [[f32; 4]; 4],
    occlusion: bool,
    instances: u32,
}

/// What the scene pass needs to wet its surfaces: the occlusion view, the authored wetness and the
/// map's half extent. Present on every frame whose graph schedules the `rain_occlusion` map (the
/// scene pass reads it last), even at `wetness: 0`.
#[derive(Clone, Copy)]
pub(super) struct WetFrame {
    pub(super) view: [[f32; 4]; 4],
    pub(super) wetness: f32,
    pub(super) half_extent: f32,
}

/// The group-3 flavour a shading bind group is built for.
#[derive(Clone, Copy)]
pub(super) enum WetInput<'a> {
    /// The lit decal pass's layout, which has no map slot.
    Decal,
    /// A mesh pipeline's layout: the `rain_occlusion` map and its frame, or none on a frame that is
    /// dry (the fallback depth is bound and the shader's wet branch is never taken).
    Mesh(Option<(&'a wgpu::TextureView, &'a WetFrame)>),
}

impl<'a> WetInput<'a> {
    pub(super) fn frame(self) -> Option<(&'a wgpu::TextureView, &'a WetFrame)> {
        match self {
            WetInput::Mesh(frame) => frame,
            WetInput::Decal => None,
        }
    }
}

impl WetFrame {
    pub(super) fn new(rain: &RainData, camera: Vec3) -> Self {
        Self {
            view: occlusion_view(rain, camera).to_cols_array_2d(),
            wetness: rain.wetness,
            half_extent: rain.radius,
        }
    }
}

impl Renderer {
    /// Uploads the scene's rain, or `None` when it has none or nothing to draw. A wet rain with no
    /// streaks is still built: its occlusion pass must fill the map the surfaces read.
    pub(super) fn build_rain_draw(
        &self,
        scene: &RenderScene,
        size: FrameSize,
        device: &wgpu::Device,
    ) -> Option<RainDraw> {
        let rain = scene.rain.as_ref()?;
        let layout = rain_layout(rain, scene.camera.position);
        if layout.instances == 0 && rain.wetness <= 0.0 {
            return None;
        }
        let occlusion_view = occlusion_view(rain, scene.camera.position).to_cols_array_2d();
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
            occ_view: occlusion_view,
        };
        let buffer = init_buffer(
            device,
            wgpu::BufferUsages::UNIFORM,
            bytemuck::bytes_of(&params),
        );
        Some(RainDraw {
            params: buffer,
            occlusion_view,
            occlusion: rain.occlusion,
            instances: layout.instances,
        })
    }
}

impl FrameRecorder<'_> {
    /// The rain's overhead occlusion pass: every shadow caster (statics included) drawn straight down
    /// into the graph's `rain_occlusion` depth map, cleared to 1.0 and stored for the two rain draws.
    /// With `occlusion: false` (or nothing to draw) the map is only cleared, so nothing occludes.
    pub(super) fn record_rain_occlusion(&self, pass: &mut RecordedPass<'_>) {
        let depth_view = pass
            .depth
            .expect("the rain occlusion pass declares a depth attachment");
        let view_bg = self
            .rain_draw
            .as_ref()
            .filter(|draw| draw.occlusion)
            .map(|draw| {
                let buf = init_buffer(
                    pass.device,
                    wgpu::BufferUsages::UNIFORM,
                    bytemuck::bytes_of(&draw.occlusion_view),
                );
                skin_bind_group(
                    pass.device,
                    self.r.layouts.get(LayoutId::ShadowView),
                    SHADOW_VIEW_BINDING,
                    &buf,
                    &self.bone_palette_buf,
                )
            });
        let mut rp = pass.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rain_occlusion"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: pass.timestamp_writes(),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let Some(view_bg) = view_bg else {
            return;
        };
        let mut bound = CasterPipeline::default();
        rp.set_pipeline(self.caster_pipeline(bound));
        rp.set_bind_group(SHADOW_VIEW_GROUP, &view_bg, &[]);
        self.draw_casters(
            &mut rp,
            |sources| {
                let all = 0..sources.len() as u32;
                std::iter::once(all).collect()
            },
            None,
            &mut bound,
        );
    }

    /// Draws the rain inside a recording pass: the colour pass when `layer` is `None`, else the
    /// transparent layer of `Some(format)`. `inputs` is the pass's reads, whose last is the
    /// `rain_occlusion` map.
    pub(super) fn record_rain(
        &self,
        rp: &mut wgpu::RenderPass<'_>,
        device: &wgpu::Device,
        inputs: &[&wgpu::TextureView],
        layer: Option<ludion_render_graph::TextureFormat>,
    ) {
        let Some(draw) = &self.rain_draw else {
            return;
        };
        let occlusion = inputs
            .last()
            .expect("a rain frame schedules the occlusion map as its last read");
        let params_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rain bind group"),
            layout: self.r.layouts.get(LayoutId::Rain),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: shader_gen::RAIN_BINDING,
                    resource: draw.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: shader_gen::RAIN_OCCLUSION_BINDING,
                    resource: wgpu::BindingResource::TextureView(occlusion),
                },
            ],
        });
        rp.set_pipeline(match layer {
            None => self.r.pipeline(ShaderId::Rain),
            Some(format) => self.r.transparent_depth_pipeline(ShaderId::Rain, format),
        });
        rp.set_bind_group(CAMERA_GROUP, &self.camera_bg, &[]);
        rp.set_bind_group(LIGHTS_GROUP, &self.lights_bg, &[]);
        rp.set_bind_group(RAIN_GROUP, &params_bg, &[]);
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
