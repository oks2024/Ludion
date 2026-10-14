//! Mip-chain generation for a freshly uploaded texture: a blit chain, one render pass per level.
//!
//! A mesh material map is minified far more often than it is magnified — a textured wall a hundred
//! units away covers a handful of pixels — so it needs a full mip pyramid and trilinear, anisotropic
//! sampling (`Plan/textured-pbr.md` G6). Nothing else in the renderer does: the sprite path samples at
//! its authored scale and every other sampler is single-level.
//!
//! The chain is the classic successive halving: level `n` is rendered by drawing a fullscreen triangle
//! that samples level `n - 1` bilinearly through a one-level view, which at exactly half the extent
//! averages each 2×2 texel quad. The shader is `blit.wgsl` — the renderer's existing passthrough copy,
//! whose `{src_tex, src_sampler}` group is one of the generated texture+sampler layouts — so this
//! module adds no WGSL: only two pipelines, one per target colour space, because a pipeline pins its
//! target format and mesh maps arrive in both ([`super::render`]'s colour-space split). An sRGB target
//! decodes on sample and re-encodes on store, so the averaging happens in linear light either way.
//!
//! A dev shader reload of `blit.wgsl` rebuilds the overlay pass's pipeline, not these two: they are
//! compiled once from the built-in source, like the compute helpers in `ibl.rs` and `gi_bake.rs`.

use ludion_render_graph::Gpu;

/// The mip levels a `width` × `height` texture's full chain holds — down to 1×1, so the coarsest level
/// is a single averaged texel and a distant surface reads its mean colour rather than aliasing.
pub(crate) fn mip_level_count(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

/// The two blit pipelines (one per target colour space) and the bilinear sampler the chain reads the
/// level above with.
pub(crate) struct MipGen {
    srgb: wgpu::RenderPipeline,
    linear: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
}

impl MipGen {
    /// Builds the two pipelines from `blit_src` (the composed `blit.wgsl` source) against `layout`,
    /// the generated `{src_tex, src_sampler}` bind group layout that shader declares.
    pub(crate) fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        blit_src: &str,
    ) -> MipGen {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mip blit"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(blit_src)),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mip blit"),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
        let pipeline = |format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mip blit"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        MipGen {
            srgb: pipeline(wgpu::TextureFormat::Rgba8UnormSrgb),
            linear: pipeline(wgpu::TextureFormat::Rgba8Unorm),
            // ClampToEdge with no mip filtering: every read is of a single-level view at exactly twice
            // the target's extent, so there is no level to blend and no coordinate outside [0, 1].
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("mip blit sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            }),
            layout: layout.clone(),
        }
    }

    /// Fills levels `1..mip_level_count` of `texture` from its level 0, which the caller has already
    /// written. `srgb` selects the pipeline whose target format matches the texture's; one encoder,
    /// one submit, like the IBL prefilter.
    pub(crate) fn generate(&self, gpu: &Gpu, texture: &wgpu::Texture, srgb: bool) {
        let device = gpu.device();
        let levels = texture.mip_level_count();
        if levels < 2 {
            return;
        }
        let view = |level: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("mip level"),
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mip chain"),
        });
        for level in 1..levels {
            let src = view(level - 1);
            let dst = view(level);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mip blit"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mip blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Every texel of the level is written by the fullscreen triangle, so there is
                        // nothing to preserve.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(if srgb { &self.srgb } else { &self.linear });
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        gpu.queue().submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::mip_level_count;

    /// The chain runs down to 1×1 on the longer axis, so a non-square texture keeps halving after its
    /// short axis has bottomed out (wgpu clamps each level's extent at 1).
    #[test]
    fn mip_chains_run_down_to_one_by_one() {
        assert_eq!(mip_level_count(1, 1), 1);
        assert_eq!(mip_level_count(2, 2), 2);
        assert_eq!(mip_level_count(256, 256), 9);
        assert_eq!(mip_level_count(1024, 512), 11);
        assert_eq!(mip_level_count(3, 1), 2);
        // A degenerate zero extent still reports one level rather than none.
        assert_eq!(mip_level_count(0, 0), 1);
    }
}
