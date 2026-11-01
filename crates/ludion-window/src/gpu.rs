//! The window's own little wgpu surface: uploads a decoded frame to a texture and presents it
//! letterboxed.
//!
//! This presenter is separate from the engine's offscreen renderer — it never sees engine GPU
//! state (that lives behind the sim thread, out of process). It shares only the window blit
//! (`ludion_render_graph::blit`) and the adapter policy (`Gpu::request_for_surface`) with the
//! in-process presenter. It owns a surface on the window, a sampled texture holding the latest
//! decoded frame, and the blit that draws that texture into an aspect-preserving viewport against a
//! black clear. The texture is (re)created whenever the incoming frame's dimensions change; the
//! surface is reconfigured on resize and on a lost/outdated swapchain.

use std::sync::Arc;

use ludion_core::diagnostics::Diagnostic;
use ludion_core::letterbox::letterbox;
use ludion_render_graph::Gpu;
use ludion_render_graph::blit::{Blit, SurfaceSetup, surface_extent, surface_unavailable};
use winit::window::Window;

use crate::frame::DecodedFrame;

/// What a [`Presenter::render`] call needs the caller to do next when acquiring the swapchain image
/// did not simply succeed.
#[derive(Debug)]
pub(crate) enum RenderError {
    /// The swapchain is lost or outdated; the caller should [`Presenter::reconfigure`] and retry.
    Reconfigure,
    /// An unrecoverable surface failure; the caller should surface this diagnostic and exit.
    Fatal(Diagnostic),
}

/// A texture holding one uploaded frame, plus its bind group and the dimensions it was sized for.
struct Uploaded {
    bind_group: wgpu::BindGroup,
    texture: wgpu::Texture,
    width: u32,
    height: u32,
}

/// The window presenter: surface + device + the shared blit, plus the current uploaded frame.
pub(crate) struct Presenter {
    // The instance is kept alive for as long as the surface derived from it.
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    gpu: Gpu,
    config: wgpu::SurfaceConfiguration,
    blit: Blit,
    /// The format each frame's swapchain view is created with: the sRGB variant of the surface's.
    view_format: wgpu::TextureFormat,
    uploaded: Option<Uploaded>,
    /// The frame last copied into `uploaded`, so an unchanged frame (a plain redraw) is not re-sent.
    last_upload: Option<Arc<DecodedFrame>>,
}

impl Presenter {
    /// Creates a presenter for `window`, configuring a surface at the window's current inner size.
    ///
    /// # Errors
    /// - `L2007` if a GPU surface cannot be created for the window, or it reports no presentable
    ///   format.
    /// - `L2003` if no GPU adapter compatible with the surface is available.
    /// - `L2004` if the adapter is found but a device/queue cannot be created.
    pub fn new(window: Arc<Window>) -> Result<Presenter, Diagnostic> {
        let instance = wgpu::Instance::default();
        let size = window.inner_size();

        let surface = instance.create_surface(window).map_err(|e| {
            surface_unavailable(&format!(
                "could not create a GPU surface for the window: {e}"
            ))
        })?;

        let gpu = Gpu::request_for_surface(&instance, &surface)?;
        let setup = SurfaceSetup::for_surface(&gpu, &surface)?;
        let config = setup.config(size.width, size.height, wgpu::PresentMode::Fifo);
        surface.configure(gpu.device(), &config);
        let blit = Blit::new(gpu.device(), setup.view_format);

        Ok(Presenter {
            _instance: instance,
            surface,
            gpu,
            config,
            blit,
            view_format: setup.view_format,
            uploaded: None,
            last_upload: None,
        })
    }

    /// The surface's current configured size (width, height) in physical pixels — the same
    /// dimensions [`render`](Presenter::render) letterboxes a frame into, so a cursor-position mapper
    /// stays consistent with what is actually drawn.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Reconfigures the surface for a new window size (`width`/`height` in physical pixels),
    /// sized by [`surface_extent`]: a zero dimension (a minimized window) is clamped to 1 and an
    /// oversized one is scaled down to the texture limit, so the swapchain stays valid.
    pub fn resize(&mut self, width: u32, height: u32) {
        (self.config.width, self.config.height) = surface_extent(width, height);
        self.surface.configure(self.gpu.device(), &self.config);
    }

    /// Reconfigures the surface at its current size — the recovery for a lost/outdated swapchain.
    pub fn reconfigure(&mut self) {
        self.surface.configure(self.gpu.device(), &self.config);
    }

    /// Draws `frame` (if any) letterboxed against a black clear and presents. Before the first frame
    /// arrives, `frame` is `None` and the window simply clears to black.
    ///
    /// # Errors
    /// [`RenderError::Reconfigure`] when the swapchain is lost/outdated (the caller reconfigures and
    /// retries) and [`RenderError::Fatal`] on an unrecoverable surface failure or a device fault (a
    /// lost device, a GPU validation error). A timed-out or occluded acquire is not an error — the
    /// frame is silently skipped (`Ok`).
    pub fn render(&mut self, frame: Option<Arc<DecodedFrame>>) -> Result<(), RenderError> {
        if let Some(frame) = frame {
            let unchanged = self
                .last_upload
                .as_ref()
                .is_some_and(|last| Arc::ptr_eq(last, &frame));
            if !unchanged {
                self.upload(&frame);
                self.last_upload = Some(frame);
            }
        }

        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(()); // transient: skip this frame, try again next redraw
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                return Err(RenderError::Reconfigure);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RenderError::Fatal(surface_unavailable(
                    "the window surface raised a validation error while acquiring a frame",
                )));
            }
        };
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                format: Some(self.view_format),
                ..Default::default()
            });
        let mut encoder =
            self.gpu
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("ludion-window frame"),
                });
        self.blit.draw(
            &mut encoder,
            &view,
            self.uploaded.as_ref().map(|up| {
                let rect = letterbox(up.width, up.height, self.config.width, self.config.height);
                (&up.bind_group, rect)
            }),
        );
        self.gpu.queue().submit([encoder.finish()]);
        device_fault(&self.gpu)?;
        self.gpu.queue().present(surface_texture);
        Ok(())
    }

    /// Uploads `frame` to the sampled texture, recreating it (and its bind group) when the frame's
    /// dimensions differ from the currently-held texture.
    fn upload(&mut self, frame: &DecodedFrame) {
        if frame.width == 0 || frame.height == 0 {
            return;
        }
        let needs_new = match &self.uploaded {
            Some(up) => up.width != frame.width || up.height != frame.height,
            None => true,
        };
        if needs_new {
            self.uploaded = Some(self.create_texture(frame.width, frame.height));
        }
        let up = self.uploaded.as_ref().expect("just ensured present");
        self.gpu.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &up.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame.width * 4),
                rows_per_image: Some(frame.height),
            },
            wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Allocates a sampled texture of the given size plus its bind group.
    fn create_texture(&self, width: u32, height: u32) -> Uploaded {
        let texture = self.gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("ludion-window frame texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.blit.bind_group(self.gpu.device(), &view);
        Uploaded {
            bind_group,
            texture,
            width,
            height,
        }
    }
}

/// A device fault (a lost device, a GPU validation error) as an unrecoverable [`RenderError`].
fn device_fault(gpu: &Gpu) -> Result<(), RenderError> {
    gpu.take_fault()
        .map_or(Ok(()), |fault| Err(RenderError::Fatal(fault)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device fault ends the viewer rather than painting a dead window forever.
    #[test]
    fn needs_gpu_a_device_fault_is_fatal_to_the_viewer() {
        let gpu = match Gpu::request() {
            Ok(gpu) => gpu,
            Err(diagnostic) => {
                ludion_core::require_gpu::skip_no_adapter(true);
                eprintln!("SKIPPED: no GPU adapter available:\n{diagnostic}");
                return;
            }
        };
        assert!(device_fault(&gpu).is_ok(), "a fresh device has no fault");

        let _invalid = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("zero-width texture"),
            size: wgpu::Extent3d {
                width: 0,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        match device_fault(&gpu) {
            Err(RenderError::Fatal(diagnostic)) => assert_eq!(diagnostic.code.as_str(), "L2010"),
            other => panic!("expected a fatal fault, got {other:?}"),
        }
    }
}
