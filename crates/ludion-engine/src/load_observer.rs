//! The clock-free seam through which scene load reports what it does.
//!
//! Scene instantiation and the asset cache's decodes call a [`LoadObserver`] at their phase
//! boundaries and at each decode or cache hit. The trait has no clock — time is the implementor's
//! to read, and the only implementor that reads one is [`profile::LoadClock`](crate::profile), the
//! sanctioned wall-clock site. Same split as `ScheduleObserver` (ludion-ecs) and `TimingObserver`:
//! the per-crate determinism denylist bans clock reads outside `profile.rs`, and the profiling
//! devdoc's rule is that observation never mutates — an attached observer cannot change the world or
//! its hash.

/// A bracketed span of load work. Phases nest (a mesh decode contains its maps' texture decodes);
/// the observer attributes time to the innermost one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadPhase {
    /// Instantiating the scene document into a world.
    Scene,
    /// Decoding a mesh document.
    Mesh,
    /// Decoding a texture file.
    Texture,
    /// Decoding any other asset kind.
    Other,
}

/// The asset kinds the cache decodes, one counter row each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadKind {
    Texture,
    Mesh,
    Voxel,
    VoxelGrid,
    Sculpt,
    Assembly,
    Character,
    Atlas,
}

impl LoadKind {
    /// Every kind, in the order a report lists them.
    pub(crate) const ALL: [LoadKind; 8] = [
        LoadKind::Texture,
        LoadKind::Mesh,
        LoadKind::Voxel,
        LoadKind::VoxelGrid,
        LoadKind::Sculpt,
        LoadKind::Assembly,
        LoadKind::Character,
        LoadKind::Atlas,
    ];

    /// The kind's wire name.
    pub(crate) fn name(self) -> &'static str {
        match self {
            LoadKind::Texture => "texture",
            LoadKind::Mesh => "mesh",
            LoadKind::Voxel => "voxel",
            LoadKind::VoxelGrid => "voxel_grid",
            LoadKind::Sculpt => "sculpt",
            LoadKind::Assembly => "assembly",
            LoadKind::Character => "character",
            LoadKind::Atlas => "atlas",
        }
    }
}

/// Receives load events. Every `begin` is paired with an `end`, on error paths too.
pub(crate) trait LoadObserver: std::any::Any + Send {
    /// A phase starts.
    fn begin(&mut self, phase: LoadPhase);
    /// The phase last begun ends.
    fn end(&mut self, phase: LoadPhase);
    /// One decode event of `kind` for `key`, reading `bytes` bytes from disk.
    fn decoded(&mut self, kind: LoadKind, key: &str, bytes: u64);
    /// A lookup of `key` found it already cached.
    fn reused(&mut self, kind: LoadKind, key: &str);
}

/// The no-op observer for callers that observe nothing.
pub(crate) struct NoLoadObserver;

impl LoadObserver for NoLoadObserver {
    fn begin(&mut self, _: LoadPhase) {}
    fn end(&mut self, _: LoadPhase) {}
    fn decoded(&mut self, _: LoadKind, _: &str, _: u64) {}
    fn reused(&mut self, _: LoadKind, _: &str) {}
}
