//! The performance-report wire types: what a frame costs, in one shape both hosts speak.
//!
//! Two producers fill the same structures. The **offline** driver (`ludion profile`) loads a scene,
//! drives N frames, and emits a [`ProfileReport`]; the **live** engine measures the frames a running
//! process is already producing and emits a [`ProfileWindowData`] per rolling window — and, on
//! request, one final window converted to the very same [`ProfileReport`]
//! ([`ProfileWindowData::into_report`]). One report shape, so a budget file, a chart, and a trace
//! exporter never have to know which side produced their input.
//!
//! # Why these types live in the protocol crate
//!
//! Every layer a report carries is measured outside this crate (sim in `ludion-engine`, GPU in
//! `ludion-render-graph`, transport in `ludion-server`, cluster binning in `ludion-renderer`), and
//! all of them reach a client only through the wire. Putting the shapes here keeps the boundary
//! rule intact — nothing below names a concrete component type; every number is a name and a plain
//! scalar — and makes the generated JSON Schema (and the TypeScript client built from it) the single
//! description of a report, whether it arrived as an event, as a command reply, or as a committed
//! JSON artifact.
//!
//! # Medians and samples
//!
//! Every `*_median_ms` is the median over the window's frames — the noise-resistant statistic for
//! wall-clock timing on a busy machine. Medians answer *what does a frame cost*; they cannot answer
//! *when did it spike*, so a window also carries [`ProfileSamples`]: every name once and one
//! parallel array per name, indexed by the window's frame position. That columnar form is what a
//! timeline is drawn from.
//!
//! # Example
//! ```
//! use ludion_protocol::perf::ProfileCaptureParams;
//!
//! // Defaults: one second of frames at the default 60 Hz tick, medians only.
//! let params: ProfileCaptureParams = serde_json::from_str("{}").unwrap();
//! assert_eq!(params.frames, 60);
//! assert!(!params.include_samples);
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Frames a `profile.capture` measures when the request does not say.
///
/// One second at the default 60 Hz tick rate — the same span a rolling
/// [`ProfileWindowData`] covers, so a capture and the stream around it are comparable by
/// construction.
pub const DEFAULT_CAPTURE_FRAMES: u64 = 60;

/// The absolute cap on `frames` for a `profile.capture` (`L4012`), and what the server's transport
/// rings are sized from.
///
/// A capture's reply is deferred until its window closes, so the request occupies a wire request id
/// for however long those frames take — a minute of frames at the default tick rate here. The
/// wall-clock bound at the current speed and tick rate is [`MAX_CAPTURE_WALL_SECONDS`] (`L4023`).
pub const MAX_CAPTURE_FRAMES: u64 = 3_600;

/// The longest wall-clock window a `profile.capture` may take.
///
/// Half the server's 120 s reply budget, so an accepted capture always answers well before the
/// transport would call the sim stalled. A window that cannot close inside it at the current run
/// state, speed and tick rate is refused or cancelled with `L4023`.
pub const MAX_CAPTURE_WALL_SECONDS: f64 = 60.0;

/// Parameters for `profile.capture`.
///
/// The capture measures the next `frames` **live** frames — the ones the host is already producing —
/// and answers with a [`ProfileReport`] once that window has closed. The rolling
/// [`ProfileWindowData`] stream, if anyone is subscribed, keeps flowing underneath it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileCaptureParams {
    /// How many live frames to measure. Must be at least 1 and at most
    /// [`MAX_CAPTURE_FRAMES`] (`L4012`).
    #[serde(default = "default_capture_frames")]
    pub frames: u64,
    /// Whether the reply carries the window's raw per-frame [`ProfileSamples`] as well as its
    /// medians. Off by default: medians answer "what does a frame cost", and the samples are several
    /// times the payload — ask for them when a timeline or a trace export is the point.
    #[serde(default)]
    pub include_samples: bool,
}

impl Default for ProfileCaptureParams {
    fn default() -> Self {
        ProfileCaptureParams {
            frames: DEFAULT_CAPTURE_FRAMES,
            include_samples: false,
        }
    }
}

fn default_capture_frames() -> u64 {
    DEFAULT_CAPTURE_FRAMES
}

/// One profiling run's result: the input, the frame counts, and the timing sections.
///
/// [`Deserialize`] as well as [`Serialize`]: a report is an artifact tooling reads back — a
/// committed baseline JSON compared against a later run, an Amdahl split computed off it — not only
/// something a host prints once. Every optional section defaults to `None`, so a report written
/// before that section existed still parses; which of them a given report carries depends on which
/// host produced it, and each field below says so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileReport {
    /// The scene that was profiled — the path as given on the command line offline, the loaded
    /// scene's name for a live capture.
    pub scene: String,
    /// Warmup frames driven and discarded before measuring (JIT/cache settling). Always `0` for a
    /// live capture: the frames it measures are the ones the host was already producing.
    pub warmup_frames: u64,
    /// Frames measured; every median below is over this many samples.
    pub measured_frames: u64,
    /// The render quality this measurement was taken at — the setting the host chose and the
    /// parameter axes every GPU number below depends on. Filled by the host that owns the setting
    /// (the offline driver directly, the live host when it answers a capture), so `None` means only
    /// a report predating the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<QualityProfile>,
    /// Sim-side timings: per-system medians and the whole-frame median.
    pub sim: SimProfile,
    /// GPU-side timings: per-pass medians from the render pipeline.
    pub gpu: GpuProfile,
    /// Frame-delivery timings, filled by the server for a live capture and absent otherwise (the
    /// offline driver delivers no frames, so it has no transport to measure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportProfile>,
    /// Clustered-forward light binning: what the frame's cluster tables cost to build and how
    /// crowded they came out. Filled by the **offline** driver and absent for a live capture — the
    /// mirror image of `transport`, and for the mirror-image reason: the live host builds its
    /// cluster tables inside a frame a profiler must neither stall nor make pay twice, while the
    /// offline driver owns its own loop and can time the build as its own span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<ClusterProfile>,
    /// Local-light shadow planning: how many shadowed lights the frame requested, was granted and
    /// skipped. Filled by the **offline** driver and absent for a live capture, for `cluster`'s
    /// reason: the live host plans inside a frame a profiler must not make pay twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_shadows: Option<LocalShadowProfile>,
    /// Sun cascade planning: how many cascades the sun authored and the frame fitted, and each
    /// one's depth range, texel size and caster count. Filled by the **offline** driver and absent
    /// for a live capture, for `cluster`'s reason: the live host fits inside a frame a profiler
    /// must not make pay twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sun_cascades: Option<SunCascadeProfile>,
    /// Projected decals of the frame: how many of each blend mode and how many draws and receiver
    /// meshes the projector pass plans. Filled by the **offline** driver for a scene with a decal;
    /// absent otherwise and for a live capture, for `cluster`'s reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decals: Option<DecalProfile>,
    /// What loading the scene cost, phase by phase, and how many assets each kind decoded or reused.
    /// Filled by the **offline** driver and absent for a live capture: a load is a one-off event the
    /// host has already run by the time a capture asks, and a disarmed host reads no clock for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<LoadProfile>,
    /// Reflection probes of the last resolve: loaded from the store versus captured. Absent for a
    /// scene without probes. Filled offline and by a live capture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probes: Option<ProbeProfile>,
    /// The GPU frustum cull's per-view counts, the median over the measured frames. Absent for a
    /// scene without a world mesh, with culling off, and for a live capture, for `cluster`'s reason:
    /// the counts come back through a readback ring only the **offline** driver arms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub culling: Option<CullingProfile>,
    /// What posing skinned meshes cost inside extract, and the shape of that work. Filled by the
    /// **offline** driver for a scene with skinned instances; absent otherwise and for a live
    /// capture, for `cluster`'s reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin: Option<SkinProfile>,
    /// Median wall time of every extract phase, and the extract time outside all of them. Filled by
    /// the **offline** driver and absent for a live capture, for `cluster`'s reason. Read beside
    /// `sim.threads`; the phases are CPU wall time and do not sum with GPU time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<ExtractProfile>,
    /// The measured frames' raw per-frame columns, present when the request asked for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples: Option<ProfileSamples>,
}

/// Skin posing inside extract: wall time of the `Skin` phase and the per-extract work counters.
///
/// The counters are deterministic shape, as `ClusterProfile`'s occupancy is; only `pose_median_ms`
/// is a measurement. `instances = sampled + reused + shared`. `joints_sampled` counts both clips of
/// a crossfade and nothing for a rest palette. Posing is parallel, so read the time beside
/// `sim.threads`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SkinProfile {
    /// Median wall time of the extract `Skin` phase.
    pub pose_median_ms: f64,
    /// Measured extracts the medians are over.
    pub sampled_frames: u64,
    /// Median skinned instances per extract.
    pub instances_median: f64,
    /// Median palettes sampled from clips per extract.
    pub palettes_sampled_median: f64,
    /// Median palettes reused from a previous frame per extract.
    pub palettes_reused_median: f64,
    /// Median palettes shared with another instance per extract.
    pub palettes_shared_median: f64,
    /// Median joints sampled per extract.
    pub joints_sampled_median: f64,
}

/// Where extract spends its wall time: one median per phase of the walk, in walk order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExtractProfile {
    /// Measured extracts the medians are over.
    pub sampled_frames: u64,
    /// Every phase in walk order, zeros included.
    pub phases: Vec<ExtractPhaseTiming>,
    /// Median over extracts of the extract time no phase covers (cache refresh, debug lines, scene
    /// drop): the extract span minus that extract's phase sum, floored at 0.
    pub outside_median_ms: f64,
}

/// One extract phase's median wall time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExtractPhaseTiming {
    /// The phase name: `place`, `poses`, `sprites`, `ui`, `meshes`, `skin`, `decals`, `gi`,
    /// `particles`, `lights`, `projectiles`.
    pub phase: String,
    /// Median wall time of the phase per extract, in milliseconds.
    pub median_ms: f64,
}

/// What the GPU frustum cull kept, per view, read back from its indirect args.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CullingProfile {
    /// Frames whose counts were read back; a full readback ring skips a frame rather than wait.
    pub sampled_frames: u64,
    /// Views past the 128 MiB cull-output cap: they draw every caster and are not in `views`.
    pub over_cap_views: u64,
    /// One entry per cull view, in cull order.
    pub views: Vec<CullViewProfile>,
    /// Per chained mesh asset: the radius its LOD selection sizes instances from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lod_radii: Vec<LodRadiusProfile>,
}

/// The LOD radius one chained mesh asset's instances size from, in metres, world space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LodRadiusProfile {
    /// The asset's path (`meshes/lod/skin-mannequin.mesh.yaml`), or `mesh_assets[N]` when unnamed.
    pub mesh: String,
    /// Whether the asset is skinned: its radius is the posed-joint box's half-diagonal, about 2× the
    /// mesh or more (`ludion docs culling`).
    pub skinned: bool,
    /// Instances of the asset with known bounds.
    pub instances: u64,
    /// Smallest radius over those instances.
    pub radius_min: f64,
    /// Median radius over those instances.
    pub radius_median: f64,
    /// Largest radius over those instances.
    pub radius_max: f64,
}

/// One cull view's counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CullViewProfile {
    /// `camera | sun | cascade_N | local_N`.
    pub name: String,
    /// Median over the sampled frames of the instances the view tested.
    pub instances_median: f64,
    /// Median over the sampled frames of the instances the view kept to draw.
    pub drawn_median: f64,
    /// Median over the sampled frames of the instances drawn at each mesh LOD level, level 0
    /// first; `drawn_median` is their sum. Empty when the frame drew no `lods:` chain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels_drawn_median: Vec<f64>,
}

/// The last reflection-probe resolve: probes loaded from the store versus captured.
///
/// `hits` stays 0 on the Fresh paths (`ludion profile`, `--profile`), which never read the store, by
/// design: committed images and measurements never depend on a cache file existing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProbeProfile {
    /// Probes loaded from the probe store.
    pub hits: u64,
    /// Probes captured and prefiltered.
    pub captures: u64,
}

/// The render quality a measurement was taken at: what the machine chose, which optional effects
/// that resolved to, and how hard each one worked.
///
/// It travels with a report for the reason [`SimProfile::threads`] does — two measurements taken
/// under different machine settings are not comparable, and only the report can say so. Quality is a
/// per-machine host switch (`--quality`, `LUDION_QUALITY`), never scene content and never sim state,
/// so nothing here affects a `WorldHash`: a report at `low` and one at `full` describe the same
/// simulation costing different pixels.
///
/// The `preset` label and the six toggles are the *choice*; the parameter axes below them are the
/// *build* — a shader's sample count or a texture descriptor's size, reported as the renderer
/// currently runs them. A false toggle leaves that effect's passes out of the frame graph entirely,
/// so its axis describes work that was not done: `shadows: false` beside a `shadow_map_size` of 1024
/// means no map of any size was drawn, and reading the axis without the toggle is the misreading
/// every toggle here exists to prevent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QualityProfile {
    /// The name of the setting this machine chose: a preset name, or `custom` for a local override
    /// file. Not derivable from the toggles — several presets currently resolve to the same ones.
    pub preset: String,
    /// Whether ambient occlusion ran on a scene that authored it.
    pub ssao: bool,
    /// Whether volumetric light shafts ran on a scene that authored them.
    pub shafts: bool,
    /// Whether depth of field ran on a scene that authored it.
    pub dof: bool,
    /// Whether per-frame GI dynamic-light injection ran. Masked off, a marked light contributes its
    /// direct lighting only and no injection pass is scheduled.
    pub gi_dynamic: bool,
    /// Whether the sun's shadow map was drawn on a scene carrying a sun. Masked off, the `shadow`
    /// pass is not scheduled, the light shafts that march the map go with it, and the sun still
    /// lights the scene.
    ///
    /// Not derivable from anything else in this block, which is why it is here: `shafts: false` says
    /// the same thing for a scene that never authored shafts, and an override file reports `custom`
    /// for `preset` — so without this field a capture could report `shadow_map_size` for a frame
    /// that drew no shadow map.
    pub shadows: bool,
    /// Whether this machine's quality allows screen-space reflections (a scene still opts in via
    /// `PostProcess.ssr`).
    pub ssr: bool,
    /// Hemisphere samples per ambient-occlusion pixel.
    pub ssao_samples: u32,
    /// Ray-march steps per light-shaft pixel.
    pub shaft_steps: u32,
    /// Taps in the depth-of-field spiral.
    pub dof_taps: u32,
    /// Edge length of the square sun shadow map, in texels.
    pub shadow_map_size: u32,
    /// Edge length of the square local-light shadow atlas, in texels; the caster budget is a fixed
    /// view count, so only resolution follows this.
    pub local_shadow_atlas_size: u32,
    /// The most cascades a sun fits on this machine (`min(authored, this)`); a sun without
    /// `cascades` draws the anchored box.
    pub sun_cascades: u32,
    /// Edge length of the shadow map's PCF kernel, in taps (`3` is a 3×3 average).
    pub pcf: u32,
    /// Rays cast per probe by the GI bake.
    pub gi_rays: u32,
    /// Multiplier on the scene's authored GI probe spacing; `1.0` bakes at exactly what the scene
    /// asked for.
    pub gi_probe_scale: f32,
    /// Fraction of the output extent the frame was drawn at before being resampled up to it; `1.0`
    /// is native.
    ///
    /// **Every GPU number in this report is a per-pixel cost**, and this is the only field that says
    /// how many pixels they were paid over. A capture at `low` draws 56% of them, so a report naming
    /// its sample counts and its shadow map but not its resolution would show a smaller frame as a
    /// faster one — the same class of lie [`shadows`](QualityProfile::shadows) is here to prevent,
    /// applied to every pass at once rather than to one.
    pub render_scale: f32,
    /// The SSR trace's extent as a fraction of the **output** extent, independent of `render_scale`.
    pub ssr_scale: f32,
    /// Octaves added to every camera's mesh LOD bias; `0.0` leaves a chain's own thresholds, `1.0`
    /// halves every switch distance. A coarser pick draws fewer triangles, so two reports at
    /// different biases are not comparable.
    #[serde(default)]
    pub lod_bias: f32,
    /// Edge length of a face of the sky's specular cube, in texels.
    pub ibl_specular_size: u32,
    /// The anti-aliasing mode the frames were drawn with (`off` or `smaa`), as the host resolved it.
    /// SMAA adds three passes after composite, so two reports at different modes are not comparable.
    #[serde(default = "default_anti_aliasing")]
    pub anti_aliasing: String,
}

fn default_anti_aliasing() -> String {
    "off".into()
}

/// Clustered-forward light binning — the CPU build's cost, and the occupancy of the tables it built.
///
/// The occupancy fields are not measurements: they are the *shape* of the tables, reduced out of the
/// binning's own counting sort, so two runs of the same frame produce identical numbers and any
/// change in them is a change in the binning rather than noise. Only `build_median_ms` is a
/// wall-clock median like the rest of the report.
///
/// They travel together because that pairing is the whole point: a `scene`-pass regression with the
/// occupancy unmoved means per-fragment shading got more expensive, while the same regression *with*
/// the mean or max list length climbing means the binning got looser. One `ludion profile` run tells
/// them apart, which a GPU capture previously had to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClusterProfile {
    /// Median wall time of one cluster build, in milliseconds, over the measured frames.
    pub build_median_ms: f64,
    /// Lights in the frame's global light array.
    pub lights: u64,
    /// Cells in the cluster grid.
    pub cells: u64,
    /// Cells holding at least one light. The complement is free frame — cells a fragment can read
    /// and pay nothing for.
    pub occupied_cells: u64,
    /// Total index references across every cell. This is the number that grows when conservatism
    /// gets looser, whether or not any single list gets longer.
    pub total_refs: u64,
    /// The longest single cell's list — the worst-case loop a fragment can pay.
    pub max_list_len: u64,
    /// The mean list length **over occupied cells** — the loop length a lit fragment pays where the
    /// loop runs at all. The all-cells mean is `total_refs / cells`, one division away, which is why
    /// this one is the occupied mean rather than a second spelling of a derivable number.
    pub mean_list_len: f64,
}

/// Local-light shadow planning — which shadowed lights the frame wanted and which it got.
///
/// Not measurements: the counts are the shape of the frame's plan, reduced from the same planner the
/// renderer runs, so two runs of the same frame produce identical numbers. `skipped > 0` means
/// opted-in lights the camera sees went unshadowed (the game's `skipped == 0` check). A quality that
/// masks shadows grants nothing: `granted` is 0, `skipped` equals `requested` and `atlas_edge` is 0,
/// which is honest for a frame that drew no local shadow — the report's `quality.shadows` names why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LocalShadowProfile {
    /// Casting lights inside their `shadow_distance` whose footprint touches the camera frustum; a
    /// light past it is not requested.
    pub requested: u64,
    /// Lights granted a set of atlas tiles.
    pub granted: u64,
    /// `requested − granted`: candidates that did not fit the caster budget.
    pub skipped: u64,
    /// Shadow views drawn: a spot takes one, a point light (or a spot wider than 60°) six.
    pub views: u64,
    /// The atlas edge the plan used, in texels; 0 when the quality masks shadows.
    pub atlas_edge: u64,
}

/// Projected decals — the frame's decals by blend mode and what the projector pass plans for them.
///
/// Not measurements: the counts are the shape of the frame's plan, reduced from the same packing the
/// renderer runs, so two runs of the same frame produce identical numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DecalProfile {
    /// Decals blended as `alpha`.
    pub alpha: u64,
    /// Decals blended as `multiply`.
    pub multiply: u64,
    /// Decals blended as `add`.
    pub add: u64,
    /// Decals blended as `coat`.
    pub coat: u64,
    /// Decal draws in the projector pass: runs of consecutive decals sharing a blend and a map.
    pub draws: u64,
    /// Meshes the receiver-id pass redraws; 0 means the pass is not scheduled.
    pub receiver_meshes: u64,
}

/// Sun cascade planning — what the sun asked for and what the frame fitted.
///
/// Not measurements: the numbers are the shape of the frame's plan, reduced from the same fit and
/// per-cascade cull the renderer runs, so two runs of the same frame produce identical numbers.
/// `count < authored` means the quality capped the cascades; `count` 0 with `authored > 0` means
/// shadows were masked or the camera was degenerate, so neither the cascades nor the anchored box
/// drew — the report's `quality.shadows` names the first cause.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SunCascadeProfile {
    /// The cascade count the sun authors; 0 for a sun without `cascades` or a sunless scene.
    pub authored: u64,
    /// Cascades fitted this frame; 0 when none ran.
    pub count: u64,
    /// Edge of one cascade's shadow tile in texels; 0 when none ran.
    pub tile_edge: u64,
    /// One entry per fitted cascade, nearest first.
    pub cascades: Vec<SunCascadeSliceProfile>,
}

/// One fitted sun cascade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SunCascadeSliceProfile {
    /// View depth, in metres, where the slice starts.
    pub near: f64,
    /// View depth, in metres, where the slice ends.
    pub far: f64,
    /// World metres per shadow texel.
    pub texel: f64,
    /// Instances the cascade's box keeps for the shadow pass.
    pub casters: u64,
}

/// What loading a scene cost — wall time per phase, and deterministic per-kind asset counters.
///
/// The phases are **exclusive** self-times: a mesh's map decode counts in `texture_ms`, not
/// `mesh_ms`, so the phases never overlap. `total_ms` runs from the first phase's start to the end
/// of the measured load, and `total_ms` minus the sum of the phases is the unattributed share.
/// The counters are not measurements: two loads of the same scene produce identical numbers, so a
/// change in them is a change in what gets decoded rather than noise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LoadProfile {
    /// Time spent instantiating the scene document into a world, outside the data-doc walk
    /// and any asset decode.
    pub scene_ms: f64,
    /// Time spent walking the content root for data-docs and validating them, on the measured scene load.
    pub docs_ms: f64,
    /// Time spent decoding mesh documents (their maps count in `texture_ms`).
    pub mesh_ms: f64,
    /// Time spent decoding textures: loose sprite textures and mesh maps alike.
    pub texture_ms: f64,
    /// Time spent decoding every other asset kind.
    pub other_ms: f64,
    /// Time the renderer spent uploading the scene's assets to the GPU; absent until the renderer
    /// reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_upload_ms: Option<f64>,
    /// Time to the first rendered frame; absent until the renderer reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_frame_ms: Option<f64>,
    /// Time spent voxelizing the scene for GI; absent until the renderer reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voxelize_ms: Option<f64>,
    /// Time spent creating the renderer's device and pipelines; absent until the renderer reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer_init_ms: Option<f64>,
    /// Time spent baking GI probes; absent until the renderer reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gi_bake_ms: Option<f64>,
    /// Time spent capturing and prefiltering reflection probes the store did not hold; absent when
    /// none was captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_bake_ms: Option<f64>,
    /// First phase start to end of the measured load.
    pub total_ms: f64,
    /// One entry per asset kind, always all of them, in a fixed order.
    pub assets: Vec<AssetLoadCounters>,
}

/// How many assets of one kind a load decoded or reused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AssetLoadCounters {
    /// The asset kind's name (`texture`, `mesh`, `voxel`, `voxel_grid`, `sculpt`, `assembly`,
    /// `character`, `prefab_clips`, `atlas`).
    pub kind: String,
    /// Decode events: one per decoded file, so a map shared by two meshes decodes once.
    pub decoded: u64,
    /// Cooked-store reads (KTX2), counted apart from `decoded`; only `texture` is ever non-zero.
    #[serde(default)]
    pub cooked: u64,
    /// Distinct keys whose first lookup in the window was a cache hit.
    pub reused: u64,
    /// On-disk size of the files those decodes read.
    pub bytes_read: u64,
}

/// Sim-side timings — the schedule's per-system medians, the sequential per-tick costs, and the
/// frame total.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SimProfile {
    /// Per-system median wall time, in registration order. Empty when the profiled scene runs no
    /// game systems (a scene loaded through the built-in registry has none — the CLI cannot link a
    /// game's own systems, so its `systems` list is the built-in schedule's, which is empty).
    pub systems: Vec<SystemTiming>,
    /// The tick's **sequential** costs — the per-tick work that is per-*world*, not per-entity, and
    /// so cannot be split across threads. One [`SystemTiming`] each, same median statistic as
    /// `systems`, always present in a fixed order. Measured beside the schedule, never inside it: none
    /// of them is part of `frame_total_median_ms`, so a parallelizable share is
    /// `Σ systems / (frame_total + Σ sequential)` — the Amdahl split. The offline driver reports
    /// three (`world_hash`, `extract`, `command_drain`); a live capture reports the two a streaming
    /// tick actually pays (`extract`, `submit`).
    pub sequential: Vec<SystemTiming>,
    /// Median wall time of one full sim frame (the observed schedule run plus per-frame overhead).
    pub frame_total_median_ms: f64,
    /// Worst-frame statistics of the frame total. Filled by the offline driver and a replay
    /// profile; `None` for a live capture and for a report predating the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_total_tail: Option<TimingTail>,
    /// How many `ludion_core::par` workers the measured sim ran on.
    ///
    /// Results never depend on this — the sim is bit-identical at any worker count — but every
    /// duration above does, so a report is not comparable to another one taken at a different count.
    /// It varies per machine by design (`LUDION_THREADS`, a host's `--threads`, else the machine's
    /// capped available parallelism), which is exactly why it travels with the measurement instead of
    /// being something a reader has to remember.
    ///
    /// `None` only in a report predating the field — historical committed JSON, whose worker count
    /// lives in its filename if anywhere. Any live or offline capture fills it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<usize>,
}

/// One system's median wall time across the measured frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SystemTiming {
    /// The system's registration name.
    pub name: String,
    /// Median wall time, in milliseconds.
    pub median_ms: f64,
    /// The worst-frame statistics beside the median — what a median hides. `None` for a live
    /// capture and for a report predating the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<TimingTail>,
}

/// The tail of one timing's per-frame distribution: the worst frame and the 99th percentile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TimingTail {
    /// The largest single-frame wall time, in milliseconds.
    pub max_ms: f64,
    /// The produced-frame index (`engine.frame()` after the tick) the maximum occurred at; the
    /// first one if several tie.
    pub max_frame: u64,
    /// The nearest-rank 99th percentile, in milliseconds: the `ceil(0.99·n)`-th smallest sample.
    pub p99_ms: f64,
}

/// GPU-side timings — per-pass medians from the frame graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GpuProfile {
    /// Whether a GPU adapter was available at all. `false` (with empty `passes`) on a headless host
    /// with no adapter — the sim section is still fully measured. Also `false` for a replay
    /// profile, which measures the sim only.
    pub available: bool,
    /// Whether the adapter supports `TIMESTAMP_QUERY`. When `false`, each pass's `median_ms` is
    /// `null` (the shape is stable; the numbers are simply unavailable on that adapter).
    pub timestamps_supported: bool,
    /// Per-pass median GPU time, in scheduled order.
    pub passes: Vec<PassTiming>,
    /// Median over the measured frames of the frame-spanning GPU time: the span from the earliest
    /// to the latest pass stamp of one frame, first timed pass start to last timed pass end. This is
    /// the number to read for frame cost; per-pass medians may overlap and must not be summed.
    /// Uploads before the graph and present after it are outside the span, and on a serial GPU it
    /// can exceed the pass sum (inter-pass gaps). `null` without timestamps or where one queue
    /// timeline cannot be proved; a frame whose stamps fail the per-frame guard (counter reset,
    /// unwritten end) is left out of the median.
    #[serde(default)]
    pub frame_median_ms: Option<f64>,
    /// The worst frame and 99th percentile of the frame-spanning GPU time.
    /// Offline, `max_frame` is the worst render's position among the measured renders, because the
    /// driver re-renders one extracted frame repeatedly; in a live capture's report it is the
    /// produced-frame index. Omitted when `frame_median_ms` is `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_tail: Option<TimingTail>,
}

/// One frame-graph pass's median GPU time across the measured frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PassTiming {
    /// The pass name (e.g. `scene`, `bloom_threshold`, `composite`).
    pub name: String,
    /// Median GPU time in milliseconds, or `null` when the adapter lacks timestamp support.
    pub median_ms: Option<f64>,
}

/// Frame-delivery timings — what happens to a rendered frame between the sim thread and the socket.
///
/// Filled by the server, which owns both spans: `encode` is the frame's image codec run, `send` is
/// the write into the connection's sink (queueing for the wire, not the kernel's delivery of it).
/// There is deliberately no readback segment — the engine's pipelined stream ring folds readback
/// into its own `submit` span in [`SimProfile::sequential`], where it belongs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TransportProfile {
    /// Median encode time in milliseconds over the window's delivered frames.
    pub encode_median_ms: f64,
    /// Median sink-write time in milliseconds over the window's delivered frames.
    pub send_median_ms: f64,
    /// How many of the window's frames were delivered, and so timed. Below the window's frame count
    /// whenever a subscriber's latest-wins slot dropped a frame, or nobody was subscribed.
    pub sampled_frames: u64,
    /// Per-frame encode times, aligned to [`ProfileSamples::frames`] (`null` for a frame that was
    /// not delivered). Empty when only medians were recorded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub encode_ms: Vec<Option<f64>>,
    /// Per-frame sink-write times, aligned the same way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub send_ms: Vec<Option<f64>>,
}

/// One closed window of live frames: the medians, and the raw samples they were taken over.
///
/// The payload of a `profile` subscription event — one per window, so a stream of them reads as
/// roughly 1 Hz at the default tick rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileWindowData {
    /// The frame the window's first sample ticked as.
    pub first_frame: u64,
    /// The frame the window's last sample ticked as.
    pub last_frame: u64,
    /// Frame samples in the window.
    pub frames: u64,
    /// Whether this window answers a capture request rather than the rolling stream.
    pub capture: bool,
    /// Per-system median wall time, in schedule order.
    pub systems: Vec<SystemTiming>,
    /// The live tick's sequential-span medians (`extract`, `submit`), in that fixed order — over the
    /// frames that were rendered, so an unrendered stretch lowers no median (`0.0` when none was).
    pub sequential: Vec<SystemTiming>,
    /// Median wall time of one full sim tick.
    pub frame_total_median_ms: f64,
    /// How many `ludion_core::par` workers the measured tick ran on — see
    /// [`SimProfile::threads`]. Required here: a live window always knows its host's worker count.
    #[serde(default = "one_worker")]
    pub threads: usize,
    /// The GPU half: per-pass medians and how much of the window they cover.
    pub gpu: LiveGpuProfile,
    /// Frame-delivery timings, filled by the server before the event goes out; absent when the
    /// window's frames reached no subscriber.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportProfile>,
    /// The window's raw per-frame samples, columnar (names once, parallel arrays).
    pub samples: ProfileSamples,
}

/// The worker count assumed for a [`ProfileWindowData`] that predates the field: one, the count
/// every host ran at before the sim's parallelism was switched on by default.
fn one_worker() -> usize {
    1
}

/// The GPU half of a [`ProfileWindowData`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LiveGpuProfile {
    /// Whether the adapter supports `TIMESTAMP_QUERY`. When `false`, every `median_ms` is `null` —
    /// the same degradation the offline [`GpuProfile`] reports. Also `false` before the first armed
    /// render, when no timing ring exists yet to ask.
    pub timestamps_supported: bool,
    /// How many of the window's frames carried a GPU sample. Below `frames` whenever the timing ring
    /// skipped a frame or the host rendered fewer frames than it ticked — normal, not an error.
    pub sampled_frames: u64,
    /// Per-pass median GPU time over the sampled frames, in scheduled order.
    pub passes: Vec<PassTiming>,
    /// Median over the sampled frames of the frame-spanning GPU time: the span from the earliest
    /// to the latest pass stamp of one frame, first timed pass start to last timed pass end. This is
    /// the number to read for frame cost; per-pass medians may overlap and must not be summed.
    /// Uploads before the graph and present after it are outside the span, and on a serial GPU it
    /// can exceed the pass sum (inter-pass gaps). `null` without timestamps or where one queue
    /// timeline cannot be proved; a frame whose stamps fail the per-frame guard (counter reset,
    /// unwritten end) is left out of the median.
    #[serde(default)]
    pub frame_median_ms: Option<f64>,
    /// The worst frame and 99th percentile of the frame-spanning GPU time.
    /// `max_frame` is the produced-frame index of the worst frame. Omitted when `frame_median_ms`
    /// is `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_tail: Option<TimingTail>,
}

/// A window's raw per-frame samples, stored columnar: every name appears once and each measurement
/// is a parallel array indexed by the window's frame position.
///
/// This is what a timeline is drawn from — a median cannot say *when* a spike happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileSamples {
    /// The frame each column entry belongs to, ascending.
    pub frames: Vec<u64>,
    /// System names, in schedule order.
    pub system_names: Vec<String>,
    /// One row per entry in [`system_names`](ProfileSamples::system_names), each a per-frame array.
    pub system_ms: Vec<Vec<f64>>,
    /// Per-frame tick totals.
    pub frame_total_ms: Vec<f64>,
    /// Per-frame render-scene extraction times (`null` for an unrendered frame).
    pub extract_ms: Vec<Option<f64>>,
    /// Per-frame frame-submit times (`null` for an unrendered frame).
    pub submit_ms: Vec<Option<f64>>,
    /// GPU pass names, in scheduled order.
    pub pass_names: Vec<String>,
    /// One row per entry in [`pass_names`](ProfileSamples::pass_names), each a per-frame array
    /// (`null` where that frame carried no sample, or the adapter cannot time passes).
    pub pass_ms: Vec<Vec<Option<f64>>>,
}

impl ProfileWindowData {
    /// Reshapes a closed window into the report shape both hosts share.
    ///
    /// `scene` labels the report the way the offline driver's path argument does. `include_samples`
    /// decides whether the window's columns ride along; its medians always do.
    ///
    /// [`GpuProfile::available`] is derived rather than carried: a window whose frames produced GPU
    /// samples had an adapter, and one that produced none did not (a headless host never creates a
    /// timing ring). An adapter without `TIMESTAMP_QUERY` still samples every frame — it just
    /// reports `null` per pass — so it reads as available with timestamps unsupported, exactly as
    /// the offline driver reports it.
    ///
    /// [`ProfileReport::cluster`] is always `None` here: a live window measures the frames the host
    /// was already producing, and their cluster tables were built inside a render a profiler must
    /// not stall or duplicate. Offline is where that section comes from.
    ///
    /// [`ProfileReport::quality`] is `None` here for a different reason: a window is a measurement,
    /// and the setting it was measured under belongs to the host, not to the frames. The live host
    /// fills it in before the report goes out, exactly as the server fills `transport`.
    ///
    /// # Example
    /// ```
    /// # use ludion_protocol::perf::*;
    /// # let window = ProfileWindowData {
    /// #     first_frame: 0, last_frame: 59, frames: 60, capture: true,
    /// #     systems: vec![], sequential: vec![], frame_total_median_ms: 1.0, threads: 8,
    /// #     gpu: LiveGpuProfile { timestamps_supported: false, sampled_frames: 0, passes: vec![], frame_median_ms: None, frame_tail: None },
    /// #     transport: None,
    /// #     samples: ProfileSamples {
    /// #         frames: vec![], system_names: vec![], system_ms: vec![], frame_total_ms: vec![],
    /// #         extract_ms: vec![], submit_ms: vec![], pass_names: vec![], pass_ms: vec![],
    /// #     },
    /// # };
    /// let report = window.into_report("live".to_string(), false);
    /// assert_eq!(report.measured_frames, 60);
    /// assert_eq!(report.warmup_frames, 0);
    /// assert!(report.samples.is_none());
    /// ```
    pub fn into_report(self, scene: String, include_samples: bool) -> ProfileReport {
        ProfileReport {
            scene,
            warmup_frames: 0,
            measured_frames: self.frames,
            quality: None,
            sim: SimProfile {
                systems: self.systems,
                sequential: self.sequential,
                frame_total_median_ms: self.frame_total_median_ms,
                frame_total_tail: None,
                threads: Some(self.threads),
            },
            gpu: GpuProfile {
                available: self.gpu.sampled_frames > 0 || !self.gpu.passes.is_empty(),
                timestamps_supported: self.gpu.timestamps_supported,
                passes: self.gpu.passes,
                frame_median_ms: self.gpu.frame_median_ms,
                frame_tail: self.gpu.frame_tail,
            },
            transport: self.transport,
            cluster: None,
            local_shadows: None,
            sun_cascades: None,
            decals: None,
            load: None,
            probes: None,
            culling: None,
            skin: None,
            extract: None,
            samples: include_samples.then_some(self.samples),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn culling_section_round_trips_and_is_optional() {
        let mut report = window().into_report("scene".to_string(), false);
        assert!(report.culling.is_none());
        report.culling = Some(CullingProfile {
            sampled_frames: 3,
            over_cap_views: 1,
            views: vec![CullViewProfile {
                name: "camera".into(),
                instances_median: 10.0,
                drawn_median: 4.0,
                levels_drawn_median: vec![3.0, 1.0],
            }],
            lod_radii: Vec::new(),
        });
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["culling"]["views"][0]["drawn_median"], 4.0);
        assert_eq!(
            serde_json::from_value::<ProfileReport>(json).unwrap(),
            report
        );
        report.culling = None;
        let json = serde_json::to_value(&report).unwrap();
        assert!(json.get("culling").is_none());
        assert!(
            serde_json::from_value::<ProfileReport>(json)
                .unwrap()
                .culling
                .is_none()
        );
    }

    #[test]
    fn skin_section_round_trips_and_is_optional() {
        let mut report = window().into_report("scene".to_string(), false);
        assert!(report.skin.is_none());
        report.skin = Some(SkinProfile {
            pose_median_ms: 0.5,
            sampled_frames: 3,
            instances_median: 3.0,
            palettes_sampled_median: 2.0,
            palettes_reused_median: 1.0,
            palettes_shared_median: 0.0,
            joints_sampled_median: 22.0,
        });
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["skin"]["joints_sampled_median"], 22.0);
        assert_eq!(
            serde_json::from_value::<ProfileReport>(json).unwrap(),
            report
        );
        report.skin = None;
        let json = serde_json::to_value(&report).unwrap();
        assert!(json.get("skin").is_none());
        assert!(
            serde_json::from_value::<ProfileReport>(json)
                .unwrap()
                .skin
                .is_none()
        );
    }

    #[test]
    fn extract_section_round_trips_and_is_optional() {
        let mut report = window().into_report("scene".to_string(), false);
        assert!(report.extract.is_none());
        report.extract = Some(ExtractProfile {
            sampled_frames: 3,
            phases: vec![
                ExtractPhaseTiming {
                    phase: "place".into(),
                    median_ms: 1.5,
                },
                ExtractPhaseTiming {
                    phase: "poses".into(),
                    median_ms: 0.0,
                },
            ],
            outside_median_ms: 0.25,
        });
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["extract"]["phases"][0]["phase"], "place");
        assert_eq!(
            serde_json::from_value::<ProfileReport>(json).unwrap(),
            report
        );
        report.extract = None;
        let json = serde_json::to_value(&report).unwrap();
        assert!(json.get("extract").is_none());
        assert!(
            serde_json::from_value::<ProfileReport>(json)
                .unwrap()
                .extract
                .is_none()
        );
    }

    #[test]
    fn load_section_round_trips_and_is_optional() {
        let mut report = window().into_report("scene".to_string(), false);
        report.load = Some(LoadProfile {
            scene_ms: 1.0,
            docs_ms: 0.25,
            mesh_ms: 2.0,
            texture_ms: 3.0,
            other_ms: 0.5,
            gpu_upload_ms: None,
            first_frame_ms: None,
            voxelize_ms: None,
            renderer_init_ms: None,
            gi_bake_ms: None,
            probe_bake_ms: None,
            total_ms: 7.0,
            assets: vec![AssetLoadCounters {
                kind: "mesh".into(),
                decoded: 2,
                cooked: 0,
                reused: 1,
                bytes_read: 100,
            }],
        });
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["load"]["docs_ms"], 0.25);
        assert!(json["load"]["gpu_upload_ms"].is_null());
        assert!(json["load"].get("probe_bake_ms").is_none());
        assert_eq!(
            serde_json::from_value::<ProfileReport>(json).unwrap(),
            report
        );

        // A report written before the section existed still parses.
        report.load = None;
        let json = serde_json::to_value(&report).unwrap();
        assert!(json.get("load").is_none());
        assert!(json.get("probes").is_none());
        assert_eq!(
            serde_json::from_value::<ProfileReport>(json).unwrap(),
            report
        );

        report.probes = Some(ProbeProfile {
            hits: 1,
            captures: 2,
        });
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(
            json["probes"],
            serde_json::json!({ "hits": 1, "captures": 2 })
        );
        assert_eq!(
            serde_json::from_value::<ProfileReport>(json).unwrap(),
            report
        );
    }

    fn window() -> ProfileWindowData {
        ProfileWindowData {
            first_frame: 10,
            last_frame: 12,
            frames: 3,
            capture: false,
            threads: 4,
            systems: vec![SystemTiming {
                name: "bump".into(),
                median_ms: 0.5,
                tail: None,
            }],
            sequential: vec![
                SystemTiming {
                    name: "extract".into(),
                    median_ms: 0.2,
                    tail: None,
                },
                SystemTiming {
                    name: "submit".into(),
                    median_ms: 0.3,
                    tail: None,
                },
            ],
            frame_total_median_ms: 1.0,
            gpu: LiveGpuProfile {
                timestamps_supported: true,
                sampled_frames: 2,
                passes: vec![PassTiming {
                    name: "scene".into(),
                    median_ms: Some(1.5),
                }],
                frame_median_ms: Some(1.6),
                frame_tail: None,
            },
            transport: None,
            samples: ProfileSamples {
                frames: vec![10, 11, 12],
                system_names: vec!["bump".into()],
                system_ms: vec![vec![0.4, 0.5, 0.6]],
                frame_total_ms: vec![1.0, 1.0, 1.1],
                extract_ms: vec![Some(0.2), Some(0.2), None],
                submit_ms: vec![Some(0.3), Some(0.3), None],
                pass_names: vec!["scene".into()],
                pass_ms: vec![vec![Some(1.5), Some(1.6), None]],
            },
        }
    }

    #[test]
    fn capture_params_round_trip_and_default() {
        let params = ProfileCaptureParams {
            frames: 120,
            include_samples: true,
        };
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "frames": 120, "include_samples": true })
        );
        assert_eq!(
            serde_json::from_value::<ProfileCaptureParams>(value).unwrap(),
            params
        );
        assert_eq!(
            ProfileCaptureParams::default(),
            serde_json::from_str("{}").unwrap()
        );
    }

    #[test]
    fn an_unknown_frame_time_is_an_explicit_null_and_an_old_body_still_parses() {
        let mut report = window().into_report("scene".to_string(), false);
        report.gpu.frame_median_ms = None;
        report.gpu.frame_tail = None;
        let mut json = serde_json::to_value(&report).unwrap();
        assert!(json["gpu"]["frame_median_ms"].is_null());
        assert!(json["gpu"].get("frame_tail").is_none());
        assert_eq!(
            serde_json::from_value::<ProfileReport>(json.clone()).unwrap(),
            report
        );

        // A body written before the fields existed (the frozen stress reports) parses as unknown.
        json["gpu"]
            .as_object_mut()
            .unwrap()
            .remove("frame_median_ms");
        assert_eq!(
            serde_json::from_value::<ProfileReport>(json).unwrap(),
            report
        );

        report.gpu.frame_median_ms = Some(4.5);
        report.gpu.frame_tail = Some(TimingTail {
            max_ms: 6.0,
            max_frame: 3,
            p99_ms: 6.0,
        });
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["gpu"]["frame_tail"]["max_frame"], 3);
        assert_eq!(
            serde_json::from_value::<ProfileReport>(json).unwrap(),
            report
        );
    }

    #[test]
    fn window_round_trips() {
        let text = serde_json::to_string(&window()).unwrap();
        let parsed: ProfileWindowData = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), text);
        // The absent server-filled section is omitted, not `null`.
        assert!(!text.contains("transport"));
    }

    #[test]
    fn transport_round_trips_with_and_without_columns() {
        let medians_only = TransportProfile {
            encode_median_ms: 11.5,
            send_median_ms: 0.4,
            sampled_frames: 58,
            encode_ms: Vec::new(),
            send_ms: Vec::new(),
        };
        let value = serde_json::to_value(&medians_only).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "encode_median_ms": 11.5,
                "send_median_ms": 0.4,
                "sampled_frames": 58,
            })
        );
        let parsed: TransportProfile = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.encode_ms, Vec::<Option<f64>>::new());

        let with_columns = TransportProfile {
            encode_ms: vec![Some(11.0), None],
            send_ms: vec![Some(0.4), None],
            ..medians_only
        };
        let text = serde_json::to_string(&with_columns).unwrap();
        let back: TransportProfile = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), text);
    }

    #[test]
    fn a_window_becomes_a_report() {
        let report = window().into_report("scenes/plaza.yaml".into(), true);
        assert_eq!(report.scene, "scenes/plaza.yaml");
        assert_eq!(report.warmup_frames, 0);
        assert_eq!(report.measured_frames, 3);
        assert_eq!(report.sim.systems[0].name, "bump");
        assert_eq!(report.sim.sequential.len(), 2);
        assert!(report.gpu.available, "sampled frames imply an adapter");
        assert!(report.gpu.timestamps_supported);
        assert_eq!(report.samples.unwrap().frames, [10, 11, 12]);

        // No GPU sample at all reads as no adapter — the offline report's `available: false`.
        let mut headless = window();
        headless.gpu.sampled_frames = 0;
        headless.gpu.passes.clear();
        let report = headless.into_report("live".into(), false);
        assert!(!report.gpu.available);
        assert!(report.samples.is_none());

        // Named passes with no timed frame still prove an adapter.
        let mut untimed = window();
        untimed.gpu.sampled_frames = 0;
        assert!(!untimed.gpu.passes.is_empty());
        let report = untimed.into_report("live".into(), false);
        assert!(report.gpu.available);
    }

    /// The descriptor round-trips whole, and its label is carried rather than derived — three of the
    /// four presets resolve to the same toggles, so the toggles alone cannot say what was chosen.
    #[test]
    fn the_quality_descriptor_round_trips_and_keeps_a_label_the_toggles_cannot_imply() {
        let full = QualityProfile {
            preset: "full".into(),
            ssao: true,
            shafts: true,
            dof: true,
            gi_dynamic: true,
            shadows: true,
            ssr: true,
            ssao_samples: 16,
            shaft_steps: 32,
            dof_taps: 32,
            shadow_map_size: 2048,
            local_shadow_atlas_size: 4096,
            sun_cascades: 4,
            pcf: 3,
            gi_rays: 64,
            gi_probe_scale: 1.0,
            render_scale: 1.0,
            ssr_scale: 0.5,
            lod_bias: 0.0,
            ibl_specular_size: 256,
            anti_aliasing: "smaa".into(),
        };
        let text = serde_json::to_string(&full).unwrap();
        let parsed: QualityProfile = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, full);

        let mut old = serde_json::to_value(&full).unwrap();
        old.as_object_mut().unwrap().remove("anti_aliasing");
        let parsed: QualityProfile = serde_json::from_value(old).unwrap();
        assert_eq!(
            parsed.anti_aliasing, "off",
            "a report without the key was drawn with no AA"
        );

        let medium = QualityProfile {
            preset: "medium".into(),
            ..full.clone()
        };
        assert_ne!(medium, full, "the label is part of the descriptor");
    }

    /// A live window carries no quality of its own — the host fills the report's descriptor after
    /// the conversion, the way the server fills `transport`.
    #[test]
    fn a_converted_window_leaves_the_quality_for_its_host_to_fill() {
        assert!(window().into_report("live".into(), false).quality.is_none());
    }

    #[test]
    fn an_old_report_without_the_optional_sections_still_parses() {
        // The exact shape the offline driver wrote before `transport`/`samples` existed: a committed
        // baseline artifact must keep deserializing.
        let old = serde_json::json!({
            "scene": "scenes/main.yaml",
            "warmup_frames": 5,
            "measured_frames": 60,
            "sim": {
                "systems": [{ "name": "bump", "median_ms": 0.01 }],
                "sequential": [
                    { "name": "world_hash", "median_ms": 0.1 },
                    { "name": "extract", "median_ms": 0.2 },
                    { "name": "command_drain", "median_ms": 0.3 }
                ],
                "frame_total_median_ms": 0.02
            },
            "gpu": {
                "available": true,
                "timestamps_supported": true,
                "passes": [{ "name": "scene", "median_ms": 1.5 }]
            }
        });
        let report: ProfileReport = serde_json::from_value(old.clone()).unwrap();
        assert!(report.transport.is_none());
        assert!(report.samples.is_none());
        // And it serializes back to the old shape — the optional sections add no keys when absent; the
        // unknown GPU frame time is the one explicit `null`.
        let mut expected = old;
        expected["gpu"]["frame_median_ms"] = serde_json::Value::Null;
        assert_eq!(serde_json::to_value(&report).unwrap(), expected);
    }
}
