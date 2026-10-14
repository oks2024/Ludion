//! The window blit both presenters share: the in-process [`PresentTarget`](crate::PresentTarget)
//! and the out-of-process viewer in `ludion-window`.
//!
//! One shader, one pipeline, one surface-format choice and one `L2007` builder, so the two windows
//! resample and colour-convert the same way. The frame is an `Rgba8UnormSrgb` texture; the
//! swapchain is configured with the sRGB variant of its format as a view format, so sampling
//! decodes and the sRGB view re-encodes and the frame's bytes reach the window unchanged.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::letterbox::Viewport;

use crate::execute::{Gpu, MAX_TEXTURE_DIMENSION_2D};

/// WGSL for the blit: a fullscreen triangle whose UVs sample the frame texture upright (UV origin
/// top-left maps to the frame's top row, the orientation readback also packs).
const BLIT_WGSL: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    out.uv = vec2<f32>(x, y);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

@group(0) @binding(0) var frame_tex: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(frame_tex, frame_sampler, in.uv);
}
"#;

/// The surface format, per-frame view format and alpha mode a window surface is configured with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceSetup {
    /// The format the swapchain is configured with.
    pub format: wgpu::TextureFormat,
    /// The sRGB variant of `format`: declared in `view_formats`, used for each frame's swapchain
    /// view and as the blit pipeline's target, so the blit writes through an sRGB view whatever
    /// the surface reports.
    pub view_format: wgpu::TextureFormat,
    /// The composite alpha mode.
    pub alpha_mode: wgpu::CompositeAlphaMode,
}

impl SurfaceSetup {
    /// Picks the setup from what a surface reports: a format it already reports as sRGB, else the
    /// first with an sRGB variant to view it through, else the first it reports; the first
    /// reported alpha mode, else `Auto`.
    ///
    /// # Errors
    /// `L2007` if the surface reports no texture format — nothing can be presented.
    pub fn choose(caps: &wgpu::SurfaceCapabilities) -> Result<SurfaceSetup, Diagnostic> {
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .or_else(|| {
                caps.formats
                    .iter()
                    .copied()
                    .find(|f| f.add_srgb_suffix().is_srgb())
            })
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| {
                surface_unavailable(
                    "the window surface reports no texture format this adapter can present",
                )
            })?;
        Ok(SurfaceSetup {
            format,
            view_format: format.add_srgb_suffix(),
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
        })
    }

    /// [`choose`](SurfaceSetup::choose) over `surface`'s capabilities on `gpu`'s adapter.
    ///
    /// # Errors
    /// `L2007` if the surface reports no texture format.
    pub fn for_surface(gpu: &Gpu, surface: &wgpu::Surface<'_>) -> Result<SurfaceSetup, Diagnostic> {
        SurfaceSetup::choose(&surface.get_capabilities(&gpu.adapter))
    }

    /// The swapchain configuration at `width` × `height`, sized by [`surface_extent`].
    pub fn config(
        &self,
        width: u32,
        height: u32,
        present_mode: wgpu::PresentMode,
    ) -> wgpu::SurfaceConfiguration {
        let (width, height) = surface_extent(width, height);
        wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: self.format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: self.alpha_mode,
            view_formats: vec![self.view_format],
        }
    }
}

/// The swapchain size for a window of `width` × `height`: a zero dimension becomes 1, and a side
/// past the engine's 2D texture limit scales both down by one factor, so a surface the device
/// cannot create is never configured and the aspect holds.
pub fn surface_extent(width: u32, height: u32) -> (u32, u32) {
    let (width, height) = (width.max(1), height.max(1));
    let limit = MAX_TEXTURE_DIMENSION_2D;
    let longest = width.max(height);
    if longest <= limit {
        return (width, height);
    }
    let scale = |side: u32| {
        let scaled =
            (u64::from(side) * u64::from(limit) + u64::from(longest) / 2) / u64::from(longest);
        (scaled as u32).max(1)
    };
    (scale(width), scale(height))
}

/// The blit pipeline, its texture+sampler bind group layout, and the sampler.
///
/// Linear filtering with `ClampToEdge`: the frame is scaled to an arbitrary window size, so both
/// presenters resample the same way. At a 1:1 size the sample lands on texel centers and the filter
/// is a no-op.
pub struct Blit {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Blit {
    /// Builds the blit for a target viewed as `view_format`.
    pub fn new(device: &wgpu::Device, view_format: wgpu::TextureFormat) -> Blit {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ludion-render-graph window blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ludion-render-graph window blit layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ludion-render-graph window blit pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ludion-render-graph window blit pipeline"),
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
                    format: view_format,
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
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ludion-render-graph window blit sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Blit {
            pipeline,
            bind_group_layout,
            sampler,
        }
    }

    /// A bind group sampling `source` with this blit's sampler.
    pub fn bind_group(&self, device: &wgpu::Device, source: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ludion-render-graph window blit bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Records the one render pass: clears `target` to black, then draws `source` into its
    /// letterboxed viewport when there is one and the viewport is drawable.
    pub fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        source: Option<(&wgpu::BindGroup, Viewport)>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ludion-render-graph window blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                // Clears the whole attachment, so the letterbox margin is opaque black.
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some((bind_group, rect)) = source
            && rect.is_drawable()
        {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.set_viewport(rect.x, rect.y, rect.width, rect.height, 0.0, 1.0);
            pass.draw(0..3, 0..1);
        }
    }
}

/// `L2007` — a window cannot present. Both hosts raise it, for the same reasons.
pub fn surface_unavailable(detail: &str) -> Diagnostic {
    Diagnostic::new(
        code::WINDOW_PRESENTATION_UNAVAILABLE,
        format!("window presentation is unavailable: {detail}"),
    )
    .with_help(
        "run from an interactive desktop session with working GPU drivers; a headless host \
         streams frames over the wire protocol instead",
    )
}

#[cfg(test)]
mod tests {
    use ludion_core::letterbox::letterbox;
    use wgpu::TextureFormat as F;

    use super::*;
    use crate::readback::read_texture_to_image;
    use crate::resource::TextureDesc;

    fn caps(formats: Vec<F>) -> wgpu::SurfaceCapabilities {
        wgpu::SurfaceCapabilities {
            formats,
            ..Default::default()
        }
    }

    #[test]
    fn format_choice_prefers_srgb() {
        let setup = SurfaceSetup::choose(&caps(vec![F::Bgra8Unorm, F::Bgra8UnormSrgb])).unwrap();
        assert_eq!(setup.format, F::Bgra8UnormSrgb);
        assert_eq!(setup.view_format, F::Bgra8UnormSrgb);
    }

    #[test]
    fn format_choice_falls_back_to_the_srgb_view_of_the_first() {
        let setup = SurfaceSetup::choose(&caps(vec![F::Bgra8Unorm, F::Rgba8Unorm])).unwrap();
        assert_eq!(setup.format, F::Bgra8Unorm);
        assert_eq!(setup.view_format, F::Bgra8UnormSrgb);
    }

    #[test]
    fn format_fallback_prefers_a_format_with_an_srgb_variant() {
        let setup = SurfaceSetup::choose(&caps(vec![F::R8Unorm, F::Rgba8Unorm])).unwrap();
        assert_eq!(setup.format, F::Rgba8Unorm);
        assert_eq!(setup.view_format, F::Rgba8UnormSrgb);
    }

    #[test]
    fn a_surface_with_no_srgb_variant_keeps_its_own_format() {
        let setup = SurfaceSetup::choose(&caps(vec![F::Rgb10a2Unorm])).unwrap();
        assert_eq!(setup.format, F::Rgb10a2Unorm);
        assert_eq!(setup.view_format, setup.format);
    }

    #[test]
    fn surface_extent_caps_the_larger_side_keeping_the_aspect() {
        assert_eq!(surface_extent(20000, 5000), (8192, 2048));
        assert_eq!(surface_extent(9000, 9000), (8192, 8192));
        assert_eq!(surface_extent(100_000, 1), (8192, 1));
        assert_eq!(surface_extent(0, 0), (1, 1));
        assert_eq!(surface_extent(1920, 1080), (1920, 1080));
        assert_eq!(surface_extent(8192, 1), (8192, 1));
    }

    #[test]
    fn no_reported_format_is_l2007_not_a_panic() {
        let error = SurfaceSetup::choose(&caps(vec![])).unwrap_err();
        assert_eq!(error.code, code::WINDOW_PRESENTATION_UNAVAILABLE);
    }

    #[test]
    fn alpha_falls_back_to_auto_when_none_is_reported() {
        let setup = SurfaceSetup::choose(&wgpu::SurfaceCapabilities {
            formats: vec![F::Rgba8Unorm],
            alpha_modes: vec![],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(setup.alpha_mode, wgpu::CompositeAlphaMode::Auto);
    }

    const SIDE: u32 = 16;

    /// A 16 × 16 source whose channels cover 0, 1, 127, 128, 254 and 255.
    fn source_bytes() -> Vec<u8> {
        let mut rgba = Vec::new();
        for y in 0..SIDE {
            for x in 0..SIDE {
                rgba.extend([
                    (x * 17) as u8,
                    (y * 17) as u8,
                    ((x * 31 + y * 7) % 256) as u8,
                    255,
                ]);
            }
        }
        rgba
    }

    /// Blits the source 1:1 into an offscreen target standing in for a swapchain and returns what
    /// the target holds. `source_format` is the uploaded texture's format; the target texture is
    /// `target_format`, viewed (and drawn) as `view_format`.
    fn blit_once(gpu: &Gpu, source_format: F, target_format: F, view_format: F) -> Vec<u8> {
        let device = gpu.device();
        let extent = wgpu::Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        };
        let source = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("blit parity source"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: source_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &source_bytes(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SIDE * 4),
                rows_per_image: Some(SIDE),
            },
            extent,
        );
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("blit parity target"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[view_format],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor {
            format: Some(view_format),
            ..Default::default()
        });

        let blit = Blit::new(device, view_format);
        let bind_group = blit.bind_group(
            device,
            &source.create_view(&wgpu::TextureViewDescriptor::default()),
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        blit.draw(
            &mut encoder,
            &target_view,
            Some((&bind_group, letterbox(SIDE, SIDE, SIDE, SIDE))),
        );
        gpu.queue().submit([encoder.finish()]);
        read_texture_to_image(gpu, &target, &TextureDesc::color(SIDE, SIDE))
            .unwrap()
            .rgba
    }

    /// The shared blit shows the frame's bytes unchanged on an sRGB surface, on a non-sRGB surface
    /// (the new viewer path), and matches the old viewer's pass-through on a non-sRGB surface.
    #[test]
    fn needs_gpu_the_shared_blit_shows_the_frame_bytes_unchanged() {
        let gpu = match Gpu::request() {
            Ok(gpu) => gpu,
            Err(diagnostic) => {
                ludion_core::require_gpu::skip_no_adapter(true);
                eprintln!("SKIPPED: no GPU adapter available:\n{diagnostic}");
                return;
            }
        };
        let input = source_bytes();
        let srgb = SurfaceSetup::choose(&caps(vec![F::Rgba8UnormSrgb])).unwrap();
        let plain = SurfaceSetup::choose(&caps(vec![F::Rgba8Unorm])).unwrap();
        assert_eq!(plain.view_format, F::Rgba8UnormSrgb);

        let cases = [
            (
                "srgb surface",
                blit_once(&gpu, F::Rgba8UnormSrgb, srgb.format, srgb.view_format),
            ),
            (
                "non-srgb surface, srgb view",
                blit_once(&gpu, F::Rgba8UnormSrgb, plain.format, plain.view_format),
            ),
            (
                "old viewer, non-srgb pass-through",
                blit_once(&gpu, F::Rgba8Unorm, F::Rgba8Unorm, F::Rgba8Unorm),
            ),
        ];
        let mut report = String::new();
        for (name, output) in &cases {
            let delta = output
                .iter()
                .zip(&input)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            report.push_str(&format!("{name}: max per-channel delta {delta}\n"));
        }
        for (name, output) in &cases {
            assert_eq!(output, &input, "{name} changed the frame's bytes\n{report}");
        }
    }
}
