//! The clock-free seam through which extract reports where a frame's walk spends its work.
//!
//! `extract_inner` calls an [`ExtractObserver`] at the boundaries of its top-level blocks. The trait
//! has no clock — time is the implementor's to read — and the same rule as `LoadObserver` holds:
//! observation never mutates, so an attached observer cannot change the scene or the world hash. A
//! detached observer costs one `Option` test per boundary. A phase that is split in the walk is
//! bracketed once per piece; the observer sums them. A failed extract may leave its last phase open.

/// A contiguous block of the extract walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExtractPhase {
    /// Camera resolve, `place`, the non-finite filter and the visibility filter.
    Place,
    /// Sprite texture resolve, volley bullets and the pixel snap.
    Sprites,
    /// UI elements and world text.
    Ui,
    /// Mesh asset resolve, skin palettes and the viewmodel pass.
    Meshes,
    /// Decals.
    Decals,
    /// The GI voxelizer and probe data.
    Gi,
    /// Particle emitters.
    Particles,
    /// Lights, probes, sun, post, ground markers and drop shadows.
    Lights,
    /// Projectile draw items.
    Projectiles,
}

impl ExtractPhase {
    /// Every phase, in walk order.
    pub const ALL: [ExtractPhase; 9] = [
        ExtractPhase::Place,
        ExtractPhase::Sprites,
        ExtractPhase::Ui,
        ExtractPhase::Meshes,
        ExtractPhase::Decals,
        ExtractPhase::Gi,
        ExtractPhase::Particles,
        ExtractPhase::Lights,
        ExtractPhase::Projectiles,
    ];
}

/// Receives the extract walk's phase boundaries.
pub trait ExtractObserver: Send {
    /// A phase starts.
    fn begin(&mut self, phase: ExtractPhase);
    /// The phase last begun ends.
    fn end(&mut self, phase: ExtractPhase);
}
