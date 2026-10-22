//! The clock-free seam through which a frame reports its upload and bake phases.
//!
//! [`Renderer::render_observed`](crate::Renderer::render_observed) calls a [`FrameObserver`] at the
//! boundaries of the work that first-use costs sit in: resource uploads and the GI bake. The trait has
//! no clock — the implementor owns it, as with every observer in the engine. GPU work is asynchronous,
//! so an observed frame fences (a device poll) after each phase and the GPU time lands in the phase
//! that submitted it rather than in the frame's readback wait. An unobserved frame fences nothing and
//! submits exactly what it always did.

/// A bracketed span of frame preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePhase {
    /// Uploading textures, mesh maps (with their mip chains), mesh buffers and the environment
    /// prefilter.
    Upload,
    /// Uploading the voxel grid and baking the GI probes.
    GiBake,
    /// Capturing and prefiltering one reflection probe the store did not hold.
    ProbeBake,
}

/// Receives frame-preparation events. Every `begin` is paired with an `end`.
pub trait FrameObserver {
    /// A phase starts.
    fn begin(&mut self, phase: FramePhase);
    /// The phase last begun has finished, GPU work included.
    fn end(&mut self, phase: FramePhase);
}
