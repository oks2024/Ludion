//! In-process presentation: blitting the graph's offscreen output into a window surface.
//!
//! The offscreen target stays the single source of truth. Goldens, `render.screenshot` and the frame
//! stream all read the same texture the passes drew into, and a [`PresentTarget`] never becomes a
//! render target of its own — it is an *additional* sink that, after the frame's commands are
//! submitted, draws that texture into a swapchain image with one fullscreen triangle, letterboxed
//! against a black clear. A frame rendered with a present target is byte-identical to one rendered
//! without one; a present target of `None` submits exactly the commands it always did.
//!
//! Threading: a surface must be created on the process's main thread on some platforms, so the host
//! builds the target there and moves it (it is `Send`) to the presenter thread, which owns it from
//! then on. The window thread keeps only a [`PresentSize`] handle and writes the surface's physical size
//! into it on every resize; the presenter thread reads that handle before each present, reconfigures
//! when it has moved, and writes back the presented frame's own size for the window thread's cursor
//! mapping. Those two atomics are the whole cross-thread contract — no lock, and a resize can never
//! tear a frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ludion_core::diagnostics::Diagnostic;
use ludion_core::letterbox::{HostRect, letterbox_in};

use crate::blit::{self, Blit, SurfaceSetup};
use crate::execute::Gpu;

/// The three values the window thread and the render thread exchange, shared between them.
///
/// Each is one `u64` (two or four packed 16/32-bit fields) behind an `Arc`: a write is a single relaxed store and
/// a read a single relaxed load. Relaxed is enough because each value is self-contained — it orders
/// nothing else, and a reader one frame stale simply corrects itself on the next one.
///
/// The traffic runs both ways. The window thread stores the surface's physical size, which the
/// render thread reads to reconfigure the swapchain; the render thread stores the presented frame's
/// size, which the window thread reads to map a cursor position through the same letterbox geometry
/// the blit draws with — so "where the frame is drawn" and "where a click lands" cannot disagree.
///
/// The third value is the host rect (a safe area) the frame is fitted into: four `u16` fields in one
/// word so a reader never sees half a rectangle, `0` meaning the whole surface.
#[derive(Clone)]
pub struct PresentSize {
    packed: Arc<AtomicU64>,
    frame: Arc<AtomicU64>,
    rect: Arc<AtomicU64>,
}

impl PresentSize {
    fn new(width: u32, height: u32) -> PresentSize {
        PresentSize {
            packed: Arc::new(AtomicU64::new(pack(width, height))),
            // Zero is the "nothing presented yet" sentinel: a real source texture is never 0 × 0.
            frame: Arc::new(AtomicU64::new(0)),
            rect: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Records the surface's new physical size, in pixels. The window thread calls this on every
    /// resize; the next present reconfigures the swapchain to match. A zero dimension (a minimized
    /// window) is a legal value — the render thread skips presenting until it moves again.
    pub fn set(&self, width: u32, height: u32) {
        self.packed.store(pack(width, height), Ordering::Relaxed);
    }

    /// Records the part of the surface the frame is fitted into (`None`: all of it). Each field
    /// saturates at `u16::MAX`.
    pub fn set_rect(&self, rect: Option<HostRect>) {
        self.rect.store(pack_rect(rect), Ordering::Relaxed);
    }

    /// The rectangle most recently written by [`set_rect`](PresentSize::set_rect); `None` is the
    /// whole surface.
    pub fn rect(&self) -> Option<HostRect> {
        unpack_rect(self.rect.load(Ordering::Relaxed))
    }

    /// The size most recently written by [`set`](PresentSize::set).
    fn get(&self) -> (u32, u32) {
        unpack(self.packed.load(Ordering::Relaxed))
    }

    /// Records the size of the frame about to be blitted. Written by the render thread only.
    fn set_frame(&self, width: u32, height: u32) {
        self.frame.store(pack(width, height), Ordering::Relaxed);
    }

    /// The size of the most recently presented frame, in render-target pixels — the dimensions a
    /// host maps a cursor position against. `None` until the first frame has been presented, which
    /// is exactly when a cursor position has nowhere meaningful to land.
    pub fn frame_size(&self) -> Option<(u32, u32)> {
        match self.frame.load(Ordering::Relaxed) {
            0 => None,
            packed => Some(unpack(packed)),
        }
    }
}

fn pack(width: u32, height: u32) -> u64 {
    (u64::from(width) << 32) | u64::from(height)
}

fn unpack(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

fn pack_rect(rect: Option<HostRect>) -> u64 {
    let Some(r) = rect else { return 0 };
    let field = |v: u32| u64::from(v.min(u32::from(u16::MAX)));
    (field(r.x) << 48) | (field(r.y) << 32) | (field(r.width) << 16) | field(r.height)
}

fn unpack_rect(packed: u64) -> Option<HostRect> {
    (packed != 0).then_some(HostRect {
        x: (packed >> 48) as u32 & 0xFFFF,
        y: (packed >> 32) as u32 & 0xFFFF,
        width: (packed >> 16) as u32 & 0xFFFF,
        height: packed as u32 & 0xFFFF,
    })
}

/// Whether [`PresentTarget::present`] put a frame on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentOutcome {
    /// The frame was blitted and queued for display.
    Presented,
    /// Nothing was presented — a minimized or occluded window, a timed-out or stale swapchain — and
    /// the call did not block, so a caller pacing itself on `present` must wait on its own.
    Skipped,
}

/// A window surface the render thread blits the frame graph's output into once per frame.
///
/// Built on the main thread with [`PresentTarget::new`] over a surface the caller created (the same
/// surface [`Gpu::request_for_surface`] chose the adapter for), then moved to the render thread. See
/// the module docs for why the offscreen texture, not this, is the source of truth.
pub struct PresentTarget {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    blit: Blit,
    /// The format the per-frame swapchain view is created with — always the sRGB variant of the
    /// surface format, so the blit writes through an sRGB view whatever the surface reports.
    view_format: wgpu::TextureFormat,
    /// The size the swapchain is configured for; `(0, 0)` when it has never been configured (the
    /// window was created minimized).
    configured: (u32, u32),
    requested: PresentSize,
}

impl PresentTarget {
    /// Builds a present target over `surface`, configured at `width` × `height` physical pixels.
    ///
    /// The surface format is the first the surface reports as sRGB, else the first it reports at
    /// all; either way the per-frame view is created with that format's sRGB variant, which is also
    /// declared in `view_formats` and is the format the blit pipeline targets. The graph's output is
    /// `Rgba8UnormSrgb`, so sampling decodes and the sRGB view re-encodes: when the surface and the
    /// frame are the same size, the presented bytes are the bytes readback would return.
    ///
    /// A zero dimension leaves the swapchain unconfigured; the first [`PresentSize::set`] with a
    /// real size configures it.
    ///
    /// # Errors
    /// `L2007` if the surface reports no usable format, or the blit pipeline cannot be built.
    pub fn new(
        gpu: &Gpu,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<PresentTarget, Diagnostic> {
        let setup = SurfaceSetup::for_surface(gpu, &surface)?;
        // Fifo is the one mode every backend guarantees, never tears, and blocks in acquire — the
        // presenter thread is off the sim thread, so that block is the display clock pacing it.
        let config = setup.config(width, height, wgpu::PresentMode::Fifo);

        let mut target = PresentTarget {
            surface,
            config,
            blit: Blit::new(&gpu.device, setup.view_format),
            view_format: setup.view_format,
            configured: (0, 0),
            requested: PresentSize::new(width, height),
        };
        if width != 0 && height != 0 {
            target.configure(gpu, width, height);
        }
        Ok(target)
    }

    /// A handle the window thread keeps to report resizes. See [`PresentSize`].
    pub fn size_handle(&self) -> PresentSize {
        self.requested.clone()
    }

    /// The surface size the window thread last requested; `(0, 0)` while minimized.
    pub fn requested_size(&self) -> (u32, u32) {
        self.requested.get()
    }

    /// Configures the swapchain for `width` × `height` physical pixels, capped by
    /// [`blit::surface_extent`]; `configured` keeps the requested size so a capped surface is not
    /// reconfigured every frame.
    fn configure(&mut self, gpu: &Gpu, width: u32, height: u32) {
        (self.config.width, self.config.height) = blit::surface_extent(width, height);
        self.surface.configure(&gpu.device, &self.config);
        self.configured = (width, height);
    }

    /// Blits `source` — the frame graph's resolved output texture — into the window and presents it.
    ///
    /// Called after the frame's own commands are submitted, so the blit reads finished pixels. The
    /// requested size is read first: a zero dimension means a minimized window and returns without
    /// touching the swapchain, and a size that differs from the configured one reconfigures before
    /// acquiring. A lost, outdated, timed-out or occluded swapchain silently skips this frame — the
    /// next one recovers, and the offscreen frame it would have shown was already streamed.
    ///
    /// # Errors
    /// `L2007` if acquiring the swapchain image fails validation. That is a permanent condition, not
    /// a transient one: it must end the host rather than paint black forever. The live loop reports
    /// it through `Engine::arm_render_fault_notify`.
    pub fn present(
        &mut self,
        gpu: &Gpu,
        source: &wgpu::Texture,
    ) -> Result<PresentOutcome, Diagnostic> {
        let (width, height) = self.requested.get();
        if width == 0 || height == 0 {
            return Ok(PresentOutcome::Skipped);
        }
        if (width, height) != self.configured {
            self.configure(gpu, width, height);
        }

        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(PresentOutcome::Skipped);
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&gpu.device, &self.config);
                return Ok(PresentOutcome::Skipped);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(blit::surface_unavailable(
                    "the window surface raised a validation error while acquiring a swapchain image",
                ));
            }
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                label: Some("ludion-render-graph present view"),
                format: Some(self.view_format),
                ..Default::default()
            });
        let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
        // Built per frame: the source view changes whenever the graph re-aliases, and one bind
        // group is cheap next to a swapchain acquire.
        let bind_group = self.blit.bind_group(&gpu.device, &source_view);

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ludion-render-graph present"),
            });
        // Published before the draw, so a host reading it maps against the geometry this very blit
        // uses.
        self.requested.set_frame(source.width(), source.height());
        let rect = letterbox_in(
            source.width(),
            source.height(),
            width,
            height,
            self.requested.rect(),
        );
        self.blit
            .draw(&mut encoder, &view, Some((&bind_group, rect)));
        gpu.queue.submit([encoder.finish()]);
        gpu.queue.present(surface_texture);
        Ok(PresentOutcome::Presented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host builds the target on the main thread and moves it to the render thread; the size
    /// handle is shared with the window thread. Both properties are load-bearing and both are
    /// checked at compile time.
    #[test]
    fn present_types_cross_threads() {
        fn assert_send<T: Send>() {}
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send::<PresentTarget>();
        assert_send_sync::<PresentSize>();
    }

    #[test]
    fn size_handle_round_trips_every_extreme() {
        for (width, height) in [(0, 0), (1, 1), (u32::MAX, u32::MAX), (1920, 1080)] {
            let size = PresentSize::new(0, 0);
            size.set(width, height);
            assert_eq!(size.get(), (width, height));
        }
    }

    #[test]
    fn rect_round_trips_and_saturates() {
        let size = PresentSize::new(0, 0);
        assert_eq!(size.rect(), None);
        let rect = HostRect {
            x: 141,
            y: 0,
            width: 2250,
            height: 1170,
        };
        size.clone().set_rect(Some(rect));
        assert_eq!(size.rect(), Some(rect));
        size.set_rect(Some(HostRect {
            x: 1,
            y: 2,
            width: 70_000,
            height: 3,
        }));
        assert_eq!(
            size.rect(),
            Some(HostRect {
                x: 1,
                y: 2,
                width: u32::from(u16::MAX),
                height: 3
            })
        );
        size.set_rect(None);
        assert_eq!(size.rect(), None);
    }

    #[test]
    fn size_handle_shares_one_cell() {
        let size = PresentSize::new(0, 0);
        let handle = size.clone();
        handle.set(800, 600);
        assert_eq!(size.get(), (800, 600));
    }

    /// A host maps the cursor only once a frame has been presented; before that there is no
    /// geometry to map against and the accessor says so.
    #[test]
    fn frame_size_is_none_until_a_present_records_one() {
        let size = PresentSize::new(1280, 720);
        assert_eq!(size.frame_size(), None);
        let handle = size.clone();
        for (width, height) in [(1, 1), (1920, 1080), (u32::MAX, u32::MAX)] {
            handle.set_frame(width, height);
            assert_eq!(size.frame_size(), Some((width, height)));
        }
    }
}
