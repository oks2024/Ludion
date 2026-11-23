//! The clock-free seam through which extract reports where a frame's walk spends its work.
//!
//! `extract_inner` calls an [`ExtractObserver`] at the boundaries of its top-level blocks. The trait
//! has no clock — time is the implementor's to read — and the same rule as `LoadObserver` holds:
//! observation never mutates, so an attached observer cannot change the scene or the world hash. A
//! detached observer costs one `Option` test per boundary. A phase that is split in the walk is
//! bracketed once per piece; the observer sums them. Counters ride beside the brackets
//! (`skin`, `assembly`). A failed extract may leave its last phase open.

/// A contiguous block of the extract walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExtractPhase {
    /// Camera resolve, placement, the non-finite filter and the visibility filter.
    Place,
    /// Prefab part animation poses and `ParentJoint` socket joints, before placement.
    Poses,
    /// Sprite texture resolve, volley bullets and the pixel snap.
    Sprites,
    /// UI elements and world text.
    Ui,
    /// Mesh asset resolve and the viewmodel pass.
    Meshes,
    /// Skin palettes: every skinned instance's bone palette.
    Skin,
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
    pub const ALL: [ExtractPhase; 11] = [
        ExtractPhase::Place,
        ExtractPhase::Poses,
        ExtractPhase::Sprites,
        ExtractPhase::Ui,
        ExtractPhase::Meshes,
        ExtractPhase::Skin,
        ExtractPhase::Decals,
        ExtractPhase::Gi,
        ExtractPhase::Particles,
        ExtractPhase::Lights,
        ExtractPhase::Projectiles,
    ];

    /// The snake_case name a profile report carries.
    pub fn name(self) -> &'static str {
        match self {
            ExtractPhase::Place => "place",
            ExtractPhase::Poses => "poses",
            ExtractPhase::Sprites => "sprites",
            ExtractPhase::Ui => "ui",
            ExtractPhase::Meshes => "meshes",
            ExtractPhase::Skin => "skin",
            ExtractPhase::Decals => "decals",
            ExtractPhase::Gi => "gi",
            ExtractPhase::Particles => "particles",
            ExtractPhase::Lights => "lights",
            ExtractPhase::Projectiles => "projectiles",
        }
    }
}

/// What one extract's `Skin` phase did. A deterministic shape of the frame, not a measurement:
/// `instances == palettes_sampled + palettes_reused + palettes_shared`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SkinCounts {
    /// Skinned mesh instances drawn this extract.
    pub instances: u32,
    /// Palettes posed from scratch (rest, follower or clip sampling).
    pub palettes_sampled: u32,
    /// Palettes copied from an earlier frame's identical pose.
    pub palettes_reused: u32,
    /// Palettes shared with another instance of the same pose.
    pub palettes_shared: u32,
    /// Joint poses evaluated; a crossfade counts both clips, a rest palette none.
    pub joints_sampled: u64,
}

impl SkinCounts {
    /// Adds every field of `other`.
    pub fn add(&mut self, other: SkinCounts) {
        self.instances += other.instances;
        self.palettes_sampled += other.palettes_sampled;
        self.palettes_reused += other.palettes_reused;
        self.palettes_shared += other.palettes_shared;
        self.joints_sampled += other.joints_sampled;
    }
}

/// What one extract's mesh loop did posing voxel assemblies: a deterministic shape of the frame.
/// Counts the drawn assemblies once each; a socket re-posing a figure it rides adds nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssemblyCounts {
    /// Assembly instances posed this extract.
    pub instances: u32,
    /// Parts posed across them.
    pub parts_posed: u32,
    /// Animation layers sampled across them.
    pub layers_sampled: u32,
    /// IK goals applied: look-ats and point chains.
    pub ik_chains_solved: u32,
    /// Ground ray casts the IK feet cast.
    pub ik_ground_casts: u32,
}

impl AssemblyCounts {
    /// Adds every field of `other`.
    pub fn add(&mut self, other: AssemblyCounts) {
        self.instances += other.instances;
        self.parts_posed += other.parts_posed;
        self.layers_sampled += other.layers_sampled;
        self.ik_chains_solved += other.ik_chains_solved;
        self.ik_ground_casts += other.ik_ground_casts;
    }
}

/// Receives the extract walk's phase boundaries.
pub trait ExtractObserver: Send {
    /// A phase starts.
    fn begin(&mut self, phase: ExtractPhase);
    /// The phase last begun ends.
    fn end(&mut self, phase: ExtractPhase);
    /// The skin counters, once per extract, inside the `Skin` bracket (zeros included).
    fn skin(&mut self, _counts: SkinCounts) {}
    /// The assembly posing counters, once per extract, inside the `Meshes` bracket (zeros included).
    fn assembly(&mut self, _counts: AssemblyCounts) {}
}
