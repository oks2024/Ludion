//! The clock-free seam through which scene load reports what it does.
//!
//! Scene instantiation and the asset cache's decodes call a [`LoadObserver`] at their phase
//! boundaries and at each decode or cache hit. The trait has no clock — time is the implementor's
//! to read, and the only implementor that reads one is [`profile::LoadClock`](crate::profile), the
//! sanctioned wall-clock site. Same split as `ScheduleObserver` (ludion-ecs) and `TimingObserver`:
//! the per-crate determinism denylist bans clock reads outside the sanctioned sites, and the profiling
//! devdoc's rule is that observation never mutates — an attached observer cannot change the world or
//! its hash.

use ludion_renderer::{FrameObserver, FramePhase};

/// A bracketed span of load work. Phases nest (a mesh decode contains its maps' texture decodes);
/// the observer attributes time to the innermost one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadPhase {
    /// Instantiating the scene document into a world.
    Scene,
    /// Walking the content root for data-docs and validating them.
    Docs,
    /// Decoding a mesh document.
    Mesh,
    /// Decoding a texture file.
    Texture,
    /// Decoding any other asset kind.
    Other,
    /// The first frame's own work: the extract walk, frame record, draw and readback wait.
    FirstFrame,
    /// Voxelizing the world for GI.
    Voxelize,
    /// Creating the renderer's device and pipelines.
    RendererInit,
    /// Uploading textures, mesh maps, mesh buffers and the environment to the GPU.
    GpuUpload,
    /// Uploading the voxel grid and baking the GI probes.
    GiBake,
    /// Capturing and prefiltering reflection probes the store did not hold.
    ProbeBake,
}

/// Adapts a [`LoadObserver`] to the renderer's [`FrameObserver`], mapping the renderer's phases onto
/// the load phases.
pub(crate) struct FrameLoadObserver<'a>(pub(crate) &'a mut dyn LoadObserver);

impl FrameLoadObserver<'_> {
    fn map(phase: FramePhase) -> LoadPhase {
        match phase {
            FramePhase::Upload => LoadPhase::GpuUpload,
            FramePhase::GiBake => LoadPhase::GiBake,
            FramePhase::ProbeBake => LoadPhase::ProbeBake,
        }
    }
}

impl FrameObserver for FrameLoadObserver<'_> {
    fn begin(&mut self, phase: FramePhase) {
        self.0.begin(Self::map(phase));
    }

    fn end(&mut self, phase: FramePhase) {
        self.0.end(Self::map(phase));
    }
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
    PrefabClips,
    Atlas,
}

impl LoadKind {
    /// Every kind, in the order a report lists them.
    pub(crate) const ALL: [LoadKind; 9] = [
        LoadKind::Texture,
        LoadKind::Mesh,
        LoadKind::Voxel,
        LoadKind::VoxelGrid,
        LoadKind::Sculpt,
        LoadKind::Assembly,
        LoadKind::Character,
        LoadKind::PrefabClips,
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
            LoadKind::PrefabClips => "prefab_clips",
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
    /// One cooked map of `kind` for `key` was read from the store instead of decoded, reading
    /// `bytes` bytes of block data.
    fn cooked(&mut self, kind: LoadKind, key: &str, bytes: u64);
}
