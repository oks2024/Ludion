//! The model-view harness: one model document → one deterministic turntable **contact sheet**.
//!
//! [`render_model_sheet`] takes a single `.voxel.yaml`, `.mesh.yaml`, `.assembly.yaml`, `.sculpt.yaml`,
//! `.character.yaml`, or `.prefab.yaml` document,
//! synthesizes a standard turntable scene around it (a normalized model, a ground plane, a per-view
//! camera, and a neutral key + fill light rig), renders each requested [`ModelView`], and stitches
//! the cells into **one** RGBA8 [`RenderedImage`]. It collapses the voxel/mesh authoring loop to
//! `check-scene` → `render --model` → look at one image (`Plan/roadmap-voxel-mesh-pbr.md`). An
//! assembly renders at its **rest pose** unless [`ModelSheetRequest::clip`] names one of its clips
//! (`Plan/voxel-animation.md` decision 11) — see *Posed sheets and strips* below.
//!
//! Everything here is a deterministic pure function of the inputs: the same document, views, and
//! sheet size produce byte-identical PNG bytes on the same machine. All CPU validation (reading the
//! document, dispatching on its `kind`, validating it, and synthesizing + extracting every view's
//! scene) happens **before** any GPU adapter is requested, matching [`render_scene`](crate::render_scene)'s
//! discipline — a broken model fails with its diagnostics without ever touching the GPU.
//!
//! # Synthesized rig (formulas)
//!
//! **Bounds.** A voxel document's model-space AABB is `[-sx/2, sx/2] × [0, sy] × [-sz/2, sz/2]` (the
//! mesher's bottom-center, 1-voxel-per-unit convention — `crate::voxel_mesh`); a mesh document's is
//! the min/max over its vertex positions, and a sculpt's the min/max over the positions it meshes to
//! (`ludion_sculpt::mesh`, already bottom-centered in world units). The bounding-sphere radius is half the AABB diagonal,
//! clamped to `MIN_RADIUS` so a degenerate (empty / single-point) model still frames.
//!
//! **Normalization.** The model is scaled by `k = 1/radius` so it fits a unit bounding sphere, and
//! translated so its AABB bottom-center sits at the world origin — one hand-tuned camera/light rig
//! then serves every model. The look-at target `C` is the scaled box center `[0, k*(max_y-min_y)/2, 0]`.
//!
//! **Camera.** Per view, the camera sits in a fixed unit direction from `C` (front `−Z`, back `+Z`,
//! left `−X`, right `+X`, top straight up, iso a normalized `(1, 0.75, 1)`, and the opt-in `game` a
//! 58°-down steep top-down over the front) and looks at `C` with a
//! 45° vertical FOV. Its distance `D` fits the unit sphere in the (possibly non-square) cell: with
//! aspect `a = cell_w/cell_h`, the limiting half-angle is `min(fov_y/2, atan(tan(fov_y/2)·a))` and
//! `D = 1.1 / sin(limiting)` (a 10% margin). The camera `Transform.rotation` is the Euler-XYZ
//! decomposition of the `look_at` pose, so the render matches the intended framing exactly. `near`
//! is `NEAR`, `far` is `FAR` — comfortably bracketing `D ± (sphere + ground)`.
//!
//! **Lights.** The engine has only ambient + [`PointLight`], so the two-light rig
//! is placed **per view, relative to the camera** (a world-fixed rig would leave the back view
//! unlit): a neutral-warm key up-and-left of the view direction and a dimmer cool fill low-and-right,
//! both at `LIGHT_RANGE_FACTOR`·D range so attenuation never clips. Offsets and intensities
//! (`KEY_INTENSITY`/`FILL_INTENSITY`) are hand-tuned for obvious face-to-face shading contrast.
//!
//! # Borrowed rigs (`--rig`)
//!
//! The synthesized rig above is a bright neutral one — deliberately, since it must read every model.
//! A game whose content is consumed under a night sky needs the other read, so [`render_model_sheet`]
//! optionally **lifts** the lighting and grade out of an existing scene: every entity carrying any of
//! [`Environment`], [`PostProcess`],
//! [`DirectionalLight`](crate::DirectionalLight), [`PointLight`], or
//! [`SpotLight`](crate::SpotLight) is copied
//! whole (its `Transform` included) into the sheet scene, re-emitted through the canonical writer so
//! no second YAML emitter exists. A rig carrying any light replaces the synthesized key + fill
//! outright — a mixed rig would read as neither. A rig entity named `ground` with a `Mesh` also
//! donates its color to the sheet's ground plane (the plane's own transform is the sheet's), so a
//! palette value is judged against the floor it will sit on.
//!
//! **The scale tension is real and is the price of the feature.** A rig is authored for game-scale
//! subjects while the sheet subject is normalized to a unit sphere. `Environment` and
//! `DirectionalLight` are position-independent, so a sky and a moon read true. A `PointLight` is
//! not: its `range`/`intensity` were tuned at metres from a metres-tall subject, and against a
//! unit-radius one it reads as a much closer or much further lamp than the game shows. `PostProcess`
//! is lifted verbatim for the same reason it is worth lifting — it is the game's grade — but its
//! distance-coupled terms (`fog_*`, `dof`/`focus_distance`) were tuned for game camera distances and
//! land wherever they land on a sheet camera sitting ~3 units out. If borrowed sheets prove
//! unreadable in practice, the recorded fallback is to drop `PostProcess` from the keep-list and
//! lift lighting alone.
//!
//! # Palette overrides (`--set` / `--sweep`) and `--true-size`
//!
//! A model's palette can be tuned for the render alone, without touching the file:
//! [`ModelSheetRequest::set`] writes fixed values, [`ModelSheetRequest::sweep`] lays one cell per
//! candidate value of one pointer. The domain is the palette
//! ([`apply_palette_overrides`](ludion_scene::apply_palette_overrides)), because value calibration
//! is the loop this harness exists to close. A `kind: voxel` document and a `kind: assembly` both
//! own one, and both are reachable; a `kind: sculpt` document widens the domain to every numeric
//! field ([`apply_sculpt_overrides`](ludion_scene::apply_sculpt_overrides) — `/resolution`,
//! `/parts/<i>/blend`, …), and since those move the surface its framing is re-meshed from the
//! overridden document; a `.mesh.yaml` asset carries per-vertex colors and no palette, so it is
//! refused.
//!
//! **Delivery is a staged file, and that is load-bearing.** The extractor resolves a
//! `Mesh.shape: { voxel: … }` by re-reading the document from the scene's project root, so an
//! overridden model is re-emitted through the canonical writer into a temp directory **under its
//! original file name**, and that directory becomes the cell's project root. An assembly's part and
//! clip files are copied beside it at their own relative paths, since the extractor resolves those
//! against the assembly file's directory. A sweep stages one copy per cell. The tree is removed when
//! the render ends; diagnostics always name the model the caller passed, never the copy.
//!
//! **`--true-size`** replaces the unit-sphere framing for the one question it cannot answer: how big
//! is this model *really*. The sheet normalizes every subject to the same bounding sphere, so a
//! 12-unit building and a 2-unit character render identically — a silent failure that argues for
//! deleting good detail. With it on, the *cell* is sized so the subject covers the pixels its
//! declared `scale:` earns under the vertical slice's gameplay camera
//! (`PX_PER_WORLD_UNIT`, measured in `docs/voxel-craft.md`); the requested sheet size then sets
//! only the cell's aspect.
//!
//! # Sculpt parts (`part` / `isolate`)
//!
//! [`ModelSheetRequest::part`] frames a sculpt's sheet on one named part: the fit above runs on the
//! part's meshed bounds (`ludion_sculpt::PartStats::bounds`) instead of the whole model's, while the
//! model keeps standing on the ground plane, so what surrounds the part stays in the picture — one
//! image answers "does the eye sit in the socket". Two rules keep any part framable. The part's box
//! is grown about its center to at least `MIN_PART_FRACTION` of the model's largest extent on every
//! axis, since an eye's meshed shell can be thousandths of a unit thick. And each view's camera
//! distance `D` is raised, when it must be, until the camera sits outside the whole model's box by
//! `CAMERA_CLEARANCE` (measured perpendicular to the face the view ray leaves through), so a camera
//! never starts inside the surface around the part: the part is then seen past whatever of the
//! model lies in front of it, smaller in the cell, which is what `isolate` is for. [`ModelSheetRequest::isolate`] hides the rest:
//! after extraction the cell's model mesh keeps only the triangles with a vertex attributed to the
//! part (`ludion_sculpt::SculptMesh::part_of`), so the part's own rim survives instead of opening up
//! where it blends into its neighbours. The filtered mesh takes its own hash, so the renderer's mesh
//! cache never serves it in place of the whole model.
//!
//! # Posed sheets and strips (`clip` / `time` / `strip`)
//!
//! An **assembly** renders at its rest pose until [`ModelSheetRequest::clip`] names a clip its
//! `clips:` map binds, and a **character** (`*.character.yaml`) or a **prefab** (`*.prefab.yaml`, posed on
//! an instance entry's root part) likewise with the clips it binds — alone, since its parts and clips
//! resolve relative to its own file. A character's clip time is
//! `tick × seconds_per_tick × overlay speed`, and a report's `time` is that clip time.
//! [`ModelSheetRequest::crossfade`] renders the blend of two bound clips as a strip whose phases are
//! the fade's weights. The pose is not a second evaluator: the synthesized scene's model entity
//! carries an [`Animation`] component and the sheet extracts through
//! [`extract_at`], so what a sheet shows is exactly what a game running that
//! clip at that instant shows.
//!
//! **Time is quantized to sim ticks.** Clip time is only ever `(now − started) × seconds_per_tick`,
//! so a requested `time` lands on the nearest [`TickRate::DEFAULT`] tick —
//! `tick = round(time × 60)` — and the sheet reports the tick and the exact seconds it used
//! ([`PhaseReport`]). Asking for `0.4` on the 60 Hz clock gets tick 24, i.e. `0.4` exactly; asking
//! for `0.401` gets the same tick, and the report says so rather than pretending otherwise.
//!
//! **`hold`** ([`ModelSheetRequest::hold`]) snaps each phase's tick down to a multiple of N through
//! extract's own `held_ticks`, so a strip shows the poses a game running `Animation.hold: N` shows
//! and neighbouring phases may repeat one.
//!
//! **`strip`** renders `n` evenly spaced phases into one cell, laid out by the lineup machinery —
//! the same model, `n` times, each entity carrying its own `started` frame so one extract at one
//! `now` produces `n` different poses. Spacing depends on the clip's own `loop:` flag:
//!
//! - **looping**: `t_i = i × duration / n` for `i` in `0..n` — the last phase is *not* the first
//!   repeated, because on a loop it never is.
//! - **one-shot**: `t_i = i × duration / (n − 1)` — both endpoints are shown, because a one-shot's
//!   final pose is the thing it exists to reach.
//!
//! # Posed gap analysis
//!
//! Alongside a posed render, every phase carries a [`PhaseReport`]: the connectivity of the
//! **posed** occupancy grid, so "the arm detaches at t = 0.4" is text rather than an image to squint
//! at. Each filled cell of the rest-pose assembled grid
//! ([`assembly_assembled_grid`](ludion_scene::assembly_assembled_grid)) is carried into its owning
//! part's posed frame — through the very
//! `part_placements` matrices the renderer draws with — and
//! rounded to the nearest cell of a grid grown to fit; the composed result then runs
//! [`VoxelDocument::connectivity_components`](ludion_scene::VoxelDocument::connectivity_components),
//! exactly as `check-scene` runs it over the rest pose.
//!
//! **It is deliberately an over-approximation, and only ever in that direction.** A rotated cell is
//! a rotated *cube*, so every cell its axis-aligned bounding box touches is marked — one cell when
//! the pose is axis-aligned, up to a 2×2×2 box (3×3×3 in the worst diagonal) when it is not.
//! Two consequences, and they are the contract: the posed grid can never *under*-connect, so a
//! rotating limb keeps its own cells face-adjacent instead of staircasing apart and reporting itself
//! detached; and a real gap still reports, because a genuine separation of one full cell or more
//! survives the dilation. What it will not catch is a hairline opening narrower than a cell — which
//! is the trade, and the right way round: a report that cries wolf on every rotation is worth
//! nothing, while one that stays quiet until a joint truly comes apart is worth grepping.
//!
//! At rest it is exact — the transform is the identity, so every cell lands back where it started —
//! which is the property that makes the rest-pose component count the trustworthy baseline a posed
//! count is read against. Use it to find the pose where a joint comes apart, then look at the sheet.
//!
//! # Sheet layout
//!
//! Views are canonicalized to [`ModelView::ALL`] order (dedup, then sort; an empty request means the
//! [`ModelView::DEFAULT`] six — the opt-in `game` view joins only when explicitly asked). With `N`
//! canonical views the grid is `rows = 1 if N ≤ 3 else 2`, `cols = ceil(N/rows)`;
//! `cell_w = sheet_width/cols` and `cell_h = sheet_height/rows` (integer division, so the final sheet
//! may be slightly smaller than requested). Views fill row-major in canonical order; any unused cell
//! (`N = 5` or `N = 7`) is a solid `UNUSED_CELL` fill. The stitch is a plain CPU RGBA8 row-copy — no
//! wall clock, no hashing, no threads — so it is deterministic. Each cell carries a burned-in
//! screen-space orientation label (a `UiText` in the synthesized scene — e.g. `FRONT -Z`, `GAME 58`).
//!
//! A **sweep** lays one cell per swept value instead, over a near-square grid (matching
//! `crate::sweep`, since cell area is what a taste call needs), each labelled with the value it
//! rendered. Under `--true-size` the cell size comes from the subject rather than from dividing the
//! requested sheet, so the sheet is exactly `cell × grid` and can be larger than the request.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::math::{
    self,
    glam::{EulerRot, Mat3, Mat4, Vec3},
};
use ludion_core::time::{FrameIndex, SimTime, TickRate};
use ludion_ecs::Schedule;
use ludion_renderer::{FrameSize, MeshData, QualityParams, RenderScene, RenderedImage, Renderer};
use ludion_scene::{
    AnimationDocument, AppliedOverride, AssembledGrid, AssemblyDocument, MeshDocument,
    PaletteEntry, SceneOverride, SculptDocument, VoxelDocument,
};
use ludion_sculpt::{SculptMesh, SculptSources};
use ludion_sim::{SimConfig, SimDriver};
use serde_json::Value;

use crate::assembly_ik::{IkAim, IkGoal, apply_assembly_ik};
use crate::assembly_mesh::{AssemblyData, AssemblyIkKind};
use crate::builtin_registry;
use crate::character::CharacterData;
use crate::components::{AnimationLayer, AnimationLayers, LayerMode};
use crate::extract::{
    AssemblyLayerPose, AssemblySide, CrossfadePose, assembly_layer_pose, extract_at,
    part_placements, part_placements_sides,
};
use crate::prefab_clips::PrefabClipData;
use crate::sweep::MAX_SWEEP_CELLS;
use crate::{
    Anchor, Animation, Camera, Environment, Mesh, MeshSource, PointLight, PostProcess, Projection,
    Transform, UiText,
};
use ludion_components::animation_layers::MAX_ANIMATION_LAYERS;
use ludion_components::{IkTarget, IkTargets, Look};

/// A turntable view. [`ALL`](ModelView::ALL) is the canonical sheet order;
/// [`DEFAULT`](ModelView::DEFAULT) is the set an empty request expands to.
///
/// **Front/back and the authored convention.** The voxel format's convention is *front = the `z = 0`
/// row* — where authors put the face (eyes, etc.). The mesher lays cell `z` at world `z − sz/2`, so
/// authored row `z = 0` sits at world **−Z**; the [`Front`](ModelView::Front) camera therefore sits
/// on **−Z** (not `+Z`) so its cell shows the authored front, and [`Back`](ModelView::Back) sits on
/// `+Z`. (Before this was fixed, the "front" cell showed the authored *rear*.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelView {
    /// The model's authored front (the `z = 0` row, where authors put the face) — camera on world
    /// `−Z`.
    Front,
    /// The model's authored back (the `z = sz−1` row) — camera on world `+Z`.
    Back,
    /// Looking at the model's left (camera on `−X`).
    Left,
    /// Looking at the model's right (camera on `+X`).
    Right,
    /// Looking straight down from above.
    Top,
    /// A three-quarter view from a normalized `(1, 0.75, 1)`.
    Iso,
    /// The steep top-down "game camera": pitched 58° down, framing the authored front three-quarter
    /// from above — the angle real gameplay sees. Opt-in (`--views game`); not in
    /// [`DEFAULT`](ModelView::DEFAULT).
    Game,
}

impl ModelView {
    /// Every selectable view, in canonical sheet order — the order the stitch lays cells out
    /// row-major and the order [`render_model_sheet`] canonicalizes any explicit request into.
    /// [`Game`](ModelView::Game) is last: available on request but not in
    /// [`DEFAULT`](ModelView::DEFAULT).
    pub const ALL: [ModelView; 7] = [
        ModelView::Front,
        ModelView::Back,
        ModelView::Left,
        ModelView::Right,
        ModelView::Top,
        ModelView::Iso,
        ModelView::Game,
    ];

    /// The default sheet — the six turntable views an **empty** `--views` request expands to. The
    /// steep [`Game`](ModelView::Game) view is deliberately opt-in: adding it to the default would
    /// change the committed contact-sheet layout (7 cells ⇒ a 4×2 grid) and every pinned model
    /// golden with it. Ask for it explicitly with `--views game` (alone or alongside others).
    pub const DEFAULT: [ModelView; 6] = [
        ModelView::Front,
        ModelView::Back,
        ModelView::Left,
        ModelView::Right,
        ModelView::Top,
        ModelView::Iso,
    ];

    /// The view's lowercase name (`"front"`, `"back"`, …) — the `--views` token.
    pub fn name(self) -> &'static str {
        match self {
            ModelView::Front => "front",
            ModelView::Back => "back",
            ModelView::Left => "left",
            ModelView::Right => "right",
            ModelView::Top => "top",
            ModelView::Iso => "iso",
            ModelView::Game => "game",
        }
    }

    /// The view a `--views` token names, or `None` for a token no view answers to. The inverse of
    /// [`name`](Self::name), for a caller that reads a view out of a document or an argument
    /// without a `clap` enum of its own.
    pub fn from_name(name: &str) -> Option<ModelView> {
        ModelView::ALL.into_iter().find(|v| v.name() == name)
    }

    /// The label burned into the view's cell (uppercase, with the **world** axis the camera sits on —
    /// which is also the axis the face it shows points along). Deliberately not the authoring-space
    /// nickname for the authored front (its `z = 0` row): calling that "+Z" reads as a world axis and
    /// has now twice produced a 180°-wrong orientation downstream — flipped sheet cameras, then every
    /// vigil monster walking backwards. Legible at the default sheet size, small enough not to swamp
    /// the model.
    pub fn label(self) -> &'static str {
        match self {
            ModelView::Front => "FRONT -Z",
            ModelView::Back => "BACK +Z",
            ModelView::Left => "LEFT -X",
            ModelView::Right => "RIGHT +X",
            ModelView::Top => "TOP Y",
            ModelView::Iso => "ISO",
            ModelView::Game => "GAME 58",
        }
    }

    /// The unit direction the camera sits in (relative to `C`) and the world up used to frame it.
    ///
    /// The Z axis is flipped relative to the *authoring* convention: authored row `z = 0` (the
    /// "front", where the face is) meshes at world −Z (`crate::voxel_mesh`), so `Front` sits on −Z.
    pub fn direction_and_up(self) -> (Vec3, Vec3) {
        match self {
            ModelView::Front => (Vec3::NEG_Z, Vec3::Y),
            ModelView::Back => (Vec3::Z, Vec3::Y),
            ModelView::Left => (Vec3::NEG_X, Vec3::Y),
            ModelView::Right => (Vec3::X, Vec3::Y),
            // Looking straight down, world up is degenerate; use +Z so the authored front (world −Z)
            // reads toward the bottom of the cell (floor-plan convention).
            ModelView::Top => (Vec3::Y, Vec3::Z),
            ModelView::Iso => (Vec3::new(1.0, 0.75, 1.0).normalize(), Vec3::Y),
            // The vigil district camera's pitch (58° down) — the common tilted top-down game
            // perspective; hardcoded, not a knob. The horizontal aim points at the authored front
            // (world −Z) with a slight +X three-quarter, so gameplay-facing occlusion (e.g. eyes
            // hidden under overhead geometry) shows up exactly as it does in game.
            ModelView::Game => {
                let pitch = (58.0_f32).to_radians();
                let horizontal = Vec3::new(0.45, 0.0, -1.0).normalize();
                let direction = horizontal * math::cos(pitch) + Vec3::Y * math::sin(pitch);
                (direction, Vec3::Y)
            }
        }
    }
}

/// Which contribution a sheet draws instead of the finished picture — a diagnostic framing, and a
/// composition of the stage's own scene rather than a renderer mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelChannel {
    /// Emission alone: every light, the ambient term and the whole sky/IBL environment are zeroed,
    /// the cell's caption is dropped, and the post stack is flattened to a pass-through
    /// (no bloom, no tonemap curve, no exposure, no dither), so a non-black pixel means emissive
    /// and nothing else. Every sheet — this one included — gives a `kind: mesh` document that
    /// references an `emissive_texture` a unit `Mesh.emissive`, since a standalone document carries
    /// no scene to author that intensity in and the map alone emits nothing; the channel's own job
    /// is the mask.
    Emissive,
}

impl ModelChannel {
    /// The channel's `--channel` token.
    pub fn name(self) -> &'static str {
        match self {
            ModelChannel::Emissive => "emissive",
        }
    }
}

/// One palette sweep: the pointer to write, and the value each cell writes there.
#[derive(Clone, Copy, Debug)]
pub struct ModelSweep<'a> {
    /// A palette pointer, e.g. `/palette/G/emissive` (see
    /// [`apply_palette_overrides`](ludion_scene::apply_palette_overrides) for the domain), or on a
    /// sculpt any numeric field, e.g. `/parts/1/blend`.
    pub pointer: &'a str,
    /// The values to try there, one rendered cell each.
    pub values: &'a [Value],
}

/// Everything one [`render_model_sheet`] call needs. Construct it from [`ModelSheetRequest::new`]
/// and override the fields the call actually uses:
///
/// ```no_run
/// # use std::path::Path;
/// # use ludion_engine::{ModelSheetRequest, ModelView};
/// let models = [Path::new("statue.voxel.yaml")];
/// let request = ModelSheetRequest {
///     views: &[ModelView::Game],
///     true_size: true,
///     ..ModelSheetRequest::new(&models, 1920, 1080)
/// };
/// ```
#[derive(Clone, Copy, Debug)]
pub struct ModelSheetRequest<'a> {
    /// The `.voxel.yaml` / `.mesh.yaml` / `.assembly.yaml` / `.sculpt.yaml` documents to render.
    /// **One** is the
    /// turntable sheet; **two or more** is a lineup — every model in one cell, side by side on one
    /// ground plane under one rig, at honest relative sizes.
    pub models: &'a [&'a Path],
    /// The turntable views to include; an **empty** slice means [`ModelView::DEFAULT`], or
    /// [`ModelView::Game`] alone for a lineup, which renders exactly one.
    pub views: &'a [ModelView],
    /// Sheet width in pixels. Under [`true_size`](Self::true_size) it sets the cell *aspect* only.
    pub width: u32,
    /// Sheet height in pixels. Under [`true_size`](Self::true_size) it sets the cell *aspect* only.
    pub height: u32,
    /// A scene whose lighting and grade replace the synthesized neutral rig.
    pub rig: Option<&'a Path>,
    /// Palette overrides applied to every cell, in order (`--set`). Voxel, assembly and sculpt
    /// documents, and a single model — calibrate one model, then line it up.
    pub set: &'a [SceneOverride],
    /// One palette pointer swept across values, one cell each (`--sweep`). Voxel, assembly and
    /// sculpt documents, exactly one view, a single model, and no [`strip`](Self::strip) — a sweep
    /// already spends the sheet's cells.
    pub sweep: Option<ModelSweep<'a>>,
    /// Size the cell so the subject renders at the pixel coverage its declared `scale:` earns under
    /// the vertical slice's gameplay camera, instead of the unit-sphere fit that renders every model
    /// the same size. In a lineup the tallest model earns the pixels and the rest stay honest
    /// against it.
    pub true_size: bool,
    /// Pose the model with the clip its assembly, character or prefab binds under this short name, instead
    /// of rendering the rest pose. Assembly, character and prefab documents only, and a single model.
    pub clip: Option<&'a str>,
    /// The clip instant to pose at, in seconds (`None` means `0.0`). Quantized to the nearest
    /// [`TickRate::DEFAULT`] tick — see the module docs. Needs [`clip`](Self::clip) or
    /// [`crossfade`](Self::crossfade), where it is how far the outgoing clip has played when the
    /// fade begins.
    pub time: Option<f64>,
    /// Render `n` evenly spaced phases of the clip (or of a crossfade) side by side in one cell
    /// instead of one instant. `n` is at least 2 and at most [`MAX_STRIP_PHASES`]. Needs
    /// [`clip`](Self::clip) or [`crossfade`](Self::crossfade), and supersedes [`time`](Self::time)
    /// for a clip.
    pub strip: Option<u32>,
    /// Render the blend of one bound clip into another as a strip of phases, each at its fade
    /// weight — [`DEFAULT_CROSSFADE_PHASES`] of them unless [`strip`](Self::strip) says otherwise.
    /// Assembly, character and prefab documents, a single model, and no [`clip`](Self::clip).
    pub crossfade: Option<CrossfadeSpec<'a>>,
    /// Render a blend space of an assembly or character at swept parameters, one phase per
    /// parameter value — [`DEFAULT_CROSSFADE_PHASES`] of them for a range unless
    /// [`strip`](Self::strip) says otherwise, one phase for plain values. Assembly and character
    /// documents, a single model, and no [`clip`](Self::clip) or [`crossfade`](Self::crossfade).
    pub blend: Option<BlendSweepSpec<'a>>,
    /// Override and additive layers stacked over the base pose of [`clip`](Self::clip),
    /// [`crossfade`](Self::crossfade) or [`blend`](Self::blend): at most
    /// [`MAX_ANIMATION_LAYERS`], one assembly or character at a time (`L1670` past the cap).
    pub layers: &'a [LayerSpec<'a>],
    /// Aim an assembly's `ik:` look-at over the base pose of [`clip`](Self::clip),
    /// [`crossfade`](Self::crossfade) or [`blend`](Self::blend). A `lo..hi` side makes the strip's
    /// phases look phases ([`DEFAULT_CROSSFADE_PHASES`] of them unless [`strip`](Self::strip) says
    /// otherwise), every one at the same [`time`](Self::time). Assembly documents only, and not with a
    /// `--blend` range.
    pub look: Option<LookSpec<'a>>,
    /// Reach `ik:` chains to points of the assembly grid over the same base pose, solved by the one
    /// solver the game extracts with. Assembly documents only.
    pub ik: &'a [IkSpec<'a>],
    /// Show each phase at the pose a game shows with `Animation.hold` = N: the phase tick snaps down
    /// to a multiple of N, the clip or fade taken to begin on a grid tick. `None` and `Some(1)` are
    /// every tick. Needs [`clip`](Self::clip) or [`crossfade`](Self::crossfade); `Some(0)` is `L1317`.
    pub hold: Option<u32>,
    /// Frame every view on this named part of a `kind: sculpt` model instead of the whole model,
    /// keeping the rest of the model in the picture. A single model only.
    pub part: Option<&'a str>,
    /// With [`part`](Self::part), draw only the part's own triangles and hide the rest of the model.
    pub isolate: bool,
    /// Draw one contribution of the stage instead of the finished picture. `None` — the default —
    /// is the ordinary sheet, byte-identical to every sheet rendered before channels existed.
    pub channel: Option<ModelChannel>,
}

impl<'a> ModelSheetRequest<'a> {
    /// A plain sheet of `models` at `width` × `height`: the default views, no rig, no overrides.
    pub fn new(models: &'a [&'a Path], width: u32, height: u32) -> ModelSheetRequest<'a> {
        ModelSheetRequest {
            models,
            views: &[],
            width,
            height,
            rig: None,
            set: &[],
            sweep: None,
            true_size: false,
            clip: None,
            time: None,
            strip: None,
            crossfade: None,
            blend: None,
            layers: &[],
            look: None,
            ik: &[],
            hold: None,
            part: None,
            isolate: false,
            channel: None,
        }
    }
}

/// A parsed `--crossfade <from>:<to>:<seconds>`: blend the clip bound as `from` into the one bound
/// as `to` over `seconds`. `from == to` is legal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrossfadeSpec<'a> {
    /// The outgoing clip's short name.
    pub from: &'a str,
    /// The incoming clip's short name.
    pub to: &'a str,
    /// The fade's length in seconds: finite and greater than zero.
    pub seconds: f32,
}

/// The phases a crossfade strip lays out when the request names no `strip`.
pub const DEFAULT_CROSSFADE_PHASES: u32 = 5;

/// Parses a `from:to:seconds` crossfade spec.
///
/// # Errors
/// `L1117` if the spec is not exactly three `:`-separated fields, a clip name is empty, or
/// `seconds` is not a finite number greater than zero.
pub fn parse_crossfade(spec: &str) -> Result<CrossfadeSpec<'_>, Diagnostic> {
    const EXAMPLE: &str = "e.g. `--crossfade run:idle:0.25` — the outgoing clip, the incoming clip, \
                           and the fade's length in seconds";
    let refuse = |message: String| {
        Diagnostic::new(code::MODEL_SHEET_INPUT, message).with_help(EXAMPLE.to_string())
    };
    let fields: Vec<&str> = spec.split(':').collect();
    let [from, to, seconds] = fields[..] else {
        return Err(refuse(format!(
            "`--crossfade {spec}` has {} `:`-separated field{}, but a crossfade is exactly \
             `<from>:<to>:<seconds>`",
            fields.len(),
            if fields.len() == 1 { "" } else { "s" }
        )));
    };
    if from.is_empty() || to.is_empty() {
        return Err(refuse(format!(
            "`--crossfade {spec}` leaves a clip name empty: both the outgoing and the incoming \
             clip must be named"
        )));
    }
    match seconds.parse::<f32>() {
        Ok(seconds) if seconds.is_finite() && seconds > 0.0 => {
            Ok(CrossfadeSpec { from, to, seconds })
        }
        Ok(_) => Err(refuse(format!(
            "`--crossfade {spec}` has a fade length of `{seconds}`, which is not a finite number \
             of seconds greater than zero"
        ))),
        Err(_) => Err(refuse(format!(
            "`--crossfade {spec}` has a fade length of `{seconds}`, which is not a number of \
             seconds"
        ))),
    }
}

/// One blend parameter of a [`BlendSweepSpec`]: held at a value, or swept linearly across the
/// strip's phases.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sweep {
    /// The same value at every phase.
    At(f32),
    /// `lo` at the first phase and `hi` at the last.
    Range(f32, f32),
}

impl Sweep {
    /// The value at phase `index` of `phases` (`phases` is at least 1).
    fn at(self, index: u32, phases: u32) -> f32 {
        match self {
            Sweep::At(value) => value,
            Sweep::Range(lo, hi) if phases > 1 => {
                lo + (hi - lo) * (index as f32 / (phases - 1) as f32)
            }
            Sweep::Range(lo, _) => lo,
        }
    }
}

/// A parsed `--blend <name>:<p0>[:<p1>]`: a blend space and the parameters to evaluate it at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendSweepSpec<'a> {
    /// The blend's short name, as the assembly or character declares it under `blends:`.
    pub name: &'a str,
    /// The first parameter (a 1D blend's only one).
    pub p0: Sweep,
    /// The second parameter of a 2D blend (`0` when omitted).
    pub p1: Option<Sweep>,
}

/// A parsed `--layer <clip>:<weight>[:<mask>[:<mode>]]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerSpec<'a> {
    /// The layer's clip (or blend) short name.
    pub clip: &'a str,
    /// The layer's weight, in `[0, 1]`.
    pub weight: f32,
    /// The `masks:` entry the layer touches; `None` is every part.
    pub mask: Option<&'a str>,
    /// How the layer combines with the pose beneath it.
    pub mode: LayerMode,
}

/// Parses a `name:p0[:p1]` blend spec, each `p` a number or a `lo..hi` range.
///
/// # Errors
/// `L1117` if the spec has the wrong number of `:`-separated fields, an empty name, a parameter that
/// is not a finite number, or a range that is not `lo..hi` of two finite numbers.
pub fn parse_blend(spec: &str) -> Result<BlendSweepSpec<'_>, Diagnostic> {
    const EXAMPLE: &str = "e.g. `--blend locomotion:0..2` sweeps one parameter across the strip, \
                           `--blend aim:0.5:-1..1` holds the first and sweeps the second";
    let refuse = |message: String| {
        Diagnostic::new(code::MODEL_SHEET_INPUT, message).with_help(EXAMPLE.to_string())
    };
    let fields: Vec<&str> = spec.split(':').collect();
    let (name, p0, p1) = match fields[..] {
        [name, p0] => (name, p0, None),
        [name, p0, p1] => (name, p0, Some(p1)),
        _ => {
            return Err(refuse(format!(
                "`--blend {spec}` has {} `:`-separated field{}, but a blend is \
                 `<name>:<p0>` or `<name>:<p0>:<p1>`",
                fields.len(),
                if fields.len() == 1 { "" } else { "s" }
            )));
        }
    };
    if name.is_empty() {
        return Err(refuse(format!(
            "`--blend {spec}` leaves the blend name empty: name the blend to sweep"
        )));
    }
    let sweep = |text: &str| -> Result<Sweep, Diagnostic> {
        let number = |part: &str| match part.parse::<f32>() {
            Ok(value) if value.is_finite() => Ok(value),
            _ => Err(refuse(format!(
                "`--blend {spec}` has `{part}` where a finite number was expected"
            ))),
        };
        match text.split_once("..") {
            Some((lo, hi)) => Ok(Sweep::Range(number(lo)?, number(hi)?)),
            None => Ok(Sweep::At(number(text)?)),
        }
    };
    Ok(BlendSweepSpec {
        name,
        p0: sweep(p0)?,
        p1: p1.map(sweep).transpose()?,
    })
}

/// Parses a `clip:weight[:mask[:mode]]` layer spec.
///
/// # Errors
/// `L1117` if the spec has more than four `:`-separated fields or fewer than two, an empty clip, a
/// weight that is not a number in `[0, 1]`, or a mode other than `override` or `additive`.
pub fn parse_layer(spec: &str) -> Result<LayerSpec<'_>, Diagnostic> {
    const EXAMPLE: &str = "e.g. `--layer wave:1.0:arm_only` or `--layer breathe:0.5::additive` — the \
                           clip, its weight in 0..1, an optional mask, an optional override|additive";
    let refuse = |message: String| {
        Diagnostic::new(code::MODEL_SHEET_INPUT, message).with_help(EXAMPLE.to_string())
    };
    let fields: Vec<&str> = spec.split(':').collect();
    if !(2..=4).contains(&fields.len()) {
        return Err(refuse(format!(
            "`--layer {spec}` has {} `:`-separated field{}, but a layer is \
             `<clip>:<weight>[:<mask>[:<mode>]]`",
            fields.len(),
            if fields.len() == 1 { "" } else { "s" }
        )));
    }
    let clip = fields[0];
    if clip.is_empty() {
        return Err(refuse(format!(
            "`--layer {spec}` leaves the clip name empty: name the clip the layer plays"
        )));
    }
    let weight = match fields[1].parse::<f32>() {
        Ok(weight) if weight.is_finite() && (0.0..=1.0).contains(&weight) => weight,
        Ok(_) => {
            return Err(refuse(format!(
                "`--layer {spec}` has a weight of `{}`, outside the range 0 to 1",
                fields[1]
            )));
        }
        Err(_) => {
            return Err(refuse(format!(
                "`--layer {spec}` has a weight of `{}`, which is not a number",
                fields[1]
            )));
        }
    };
    let mask = fields.get(2).copied().filter(|mask| !mask.is_empty());
    let mode = match fields.get(3).copied() {
        None | Some("" | "override") => LayerMode::Override,
        Some("additive") => LayerMode::Additive,
        Some(other) => {
            return Err(refuse(format!(
                "`--layer {spec}` has the mode `{other}`, but a layer's mode is `override` or \
                 `additive`"
            )));
        }
    };
    Ok(LayerSpec {
        clip,
        weight,
        mask,
        mode,
    })
}

/// A parsed `--look [NAME=]YAW,PITCH`: a look-at's direction, each side a number or a `lo..hi`
/// range swept across the strip's phases.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookSpec<'a> {
    /// The `ik:` look-at the direction aims; `None` is the assembly's only look-at.
    pub name: Option<&'a str>,
    /// Degrees about +Y; positive turns left (`Look`'s convention).
    pub yaw: Sweep,
    /// Degrees of tilt; positive looks down.
    pub pitch: Sweep,
}

/// A parsed `--ik NAME=X,Y,Z`: a chain's target point, in the assembly's own grid voxels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IkSpec<'a> {
    /// The `ik:` chain whose effector reaches the point.
    pub name: &'a str,
    /// The point in assembly-grid voxels — the frame `place:` is written in.
    pub point: [f32; 3],
}

/// Parses a `[name=]yaw,pitch` look spec, each side a number or a `lo..hi` range.
///
/// # Errors
/// `L1117` if the spec is not two `,`-separated fields, a name before `=` is empty, or a side is not a
/// finite number or a range of two.
pub fn parse_look(spec: &str) -> Result<LookSpec<'_>, Diagnostic> {
    const EXAMPLE: &str = "e.g. `--look 40,0` aims the only look-at 40° to the left, \
                           `--look look=0..100,0` sweeps the yaw of the one named `look` across the \
                           strip — yaw then pitch, in degrees";
    let refuse = |message: String| {
        Diagnostic::new(code::MODEL_SHEET_INPUT, message).with_help(EXAMPLE.to_string())
    };
    let (name, angles) = match spec.split_once('=') {
        Some(("", _)) => {
            return Err(refuse(format!(
                "`--look {spec}` leaves the look-at name empty: write `<name>=<yaw>,<pitch>`, or \
                 drop the `=`"
            )));
        }
        Some((name, angles)) => (Some(name), angles),
        None => (None, spec),
    };
    let fields: Vec<&str> = angles.split(',').collect();
    let [yaw, pitch] = fields[..] else {
        return Err(refuse(format!(
            "`--look {spec}` has {} `,`-separated field{}, but a look is `<yaw>,<pitch>`",
            fields.len(),
            if fields.len() == 1 { "" } else { "s" }
        )));
    };
    let sweep = |text: &str| {
        sweep_of(text).ok_or_else(|| {
            refuse(format!(
                "`--look {spec}` has `{text}` where a finite number or a `lo..hi` range was expected"
            ))
        })
    };
    Ok(LookSpec {
        name,
        yaw: sweep(yaw)?,
        pitch: sweep(pitch)?,
    })
}

/// Parses a `name=x,y,z` chain target, in assembly-grid voxels.
///
/// # Errors
/// `L1117` if there is no `=`, the name is empty, or the point is not three finite numbers.
pub fn parse_ik(spec: &str) -> Result<IkSpec<'_>, Diagnostic> {
    const EXAMPLE: &str = "e.g. `--ik hand_r=6,12,2` reaches the chain `hand_r` for the voxel point \
                           x=6, y=12, z=2 of the assembly grid (`check-scene` lists the chains)";
    let refuse = |message: String| {
        Diagnostic::new(code::MODEL_SHEET_INPUT, message).with_help(EXAMPLE.to_string())
    };
    let Some((name, point)) = spec.split_once('=') else {
        return Err(refuse(format!(
            "`--ik {spec}` has no `=`, but a target is `<name>=<x>,<y>,<z>`"
        )));
    };
    if name.is_empty() {
        return Err(refuse(format!(
            "`--ik {spec}` leaves the chain name empty: name the `ik:` chain to reach with"
        )));
    }
    let fields: Vec<&str> = point.split(',').collect();
    let [x, y, z] = fields[..] else {
        return Err(refuse(format!(
            "`--ik {spec}` has {} `,`-separated field{}, but a point is `<x>,<y>,<z>`",
            fields.len(),
            if fields.len() == 1 { "" } else { "s" }
        )));
    };
    let number = |text: &str| match text.parse::<f32>() {
        Ok(value) if value.is_finite() => Ok(value),
        _ => Err(refuse(format!(
            "`--ik {spec}` has `{text}` where a finite number was expected"
        ))),
    };
    Ok(IkSpec {
        name,
        point: [number(x)?, number(y)?, number(z)?],
    })
}

/// A finite number, or a `lo..hi` range of two.
fn sweep_of(text: &str) -> Option<Sweep> {
    let number = |part: &str| part.parse::<f32>().ok().filter(|value| value.is_finite());
    match text.split_once("..") {
        Some((lo, hi)) => Some(Sweep::Range(number(lo)?, number(hi)?)),
        None => Some(Sweep::At(number(text)?)),
    }
}

/// The most models one lineup may carry. Past eight, each model's slice of the cell is too narrow
/// for the comparison a lineup exists to make — and its caption no longer fits under it.
pub const MAX_LINEUP_MODELS: usize = 8;

/// The most phases one `strip` may lay out — the lineup cap, for the lineup's reason: past eight,
/// each phase's slice of the cell is too narrow to read the pose in.
pub const MAX_STRIP_PHASES: u32 = MAX_LINEUP_MODELS as u32;

/// The fewest phases a strip is worth rendering: one phase is a posed sheet, which is what
/// [`ModelSheetRequest::time`] alone already produces.
pub const MIN_STRIP_PHASES: u32 = 2;

/// What a `--sweep` produced, alongside the sheet.
#[derive(Clone, Debug)]
pub struct ModelSweepOutcome {
    /// The pointer that was swept.
    pub pointer: String,
    /// The value the document held there before each cell wrote over it (after
    /// [`ModelSheetRequest::set`]), or `None` when the palette entry left the field unset.
    pub original: Option<Value>,
    /// The grid's column count.
    pub cols: u32,
    /// The grid's row count.
    pub rows: u32,
}

/// How `--true-size` framed the subject, for the caller to report — a framing this large is
/// surprising enough that stating it is cheaper than having it doubted.
#[derive(Clone, Copy, Debug)]
pub struct TrueSizeFraming {
    /// The subject's world height: its AABB height times the document's declared `scale:`.
    pub world_height: f32,
    /// The vertical pixels that height earns under the gameplay camera.
    pub subject_px: f32,
}

/// What a posed render (`clip`, and optionally `strip`) produced alongside the sheet: the clip it
/// played and one [`PhaseReport`] per rendered phase.
#[derive(Clone, Debug)]
pub struct ClipOutcome {
    /// The clip's short name, as the assembly, character or prefab binds it — the incoming clip of a
    /// crossfade.
    pub name: String,
    /// The clip's authored `duration`, in seconds.
    pub duration: f64,
    /// The clip's authored `loop:` flag — what set the strip's phase spacing.
    pub looped: bool,
    /// The crossfade this render blended, when it was one; `name`, `duration` and `looped` then
    /// describe its incoming clip.
    pub crossfade: Option<ClipCrossfade>,
    /// The blend this render swept, when it was one; `name` is then the blend, `duration` one cycle
    /// (`1 / rate` at the first phase's parameters) and `looped` is `true`.
    pub blend: Option<String>,
    /// The request's layers, each echoed as `clip weight mask mode` (`-` for no mask), bottom first.
    pub layers: Vec<String>,
    /// The request's `--ik` targets, each echoed as `name=[x, y, z]` in assembly-grid voxels.
    pub ik: Vec<String>,
    /// The tick rate clip time was quantized against ([`TickRate::DEFAULT`]), in hertz.
    pub hz: u32,
    /// The pose cadence the phases were snapped to, in ticks (`1` when the request had none).
    pub hold: u32,
    /// One entry per rendered phase, in render (left-to-right) order. A single posed sheet has
    /// exactly one.
    pub phases: Vec<PhaseReport>,
}

/// The crossfade behind a [`ClipOutcome`].
#[derive(Clone, Debug)]
pub struct ClipCrossfade {
    /// The outgoing clip's short name.
    pub from: String,
    /// The fade's length, in seconds.
    pub seconds: f64,
}

/// One rendered phase of a posed sheet: when it is, and whether the model held together there
/// (`Plan/voxel-animation.md` decision 11).
#[derive(Clone, Debug)]
pub struct PhaseReport {
    /// The exact clip time this phase rendered at, in seconds — the *quantized* one
    /// (`tick × seconds_per_tick`, times a character clip's overlay speed), never the requested
    /// one. Under a crossfade it is the incoming clip's time.
    pub time: f64,
    /// The sim tick that time is, on the [`ClipOutcome::hz`] clock. Under a crossfade, the ticks
    /// the fade has run.
    pub tick: u64,
    /// How many 6-connected components the posed occupancy grid has. `1` is whole; more than one
    /// means something came apart at this phase. `None` for a character, which has no voxel grid.
    pub components: Option<usize>,
    /// Every component but the main body, smallest first — empty when
    /// [`components`](Self::components) is `1`, and for a character.
    pub strays: Vec<PhaseStray>,
    /// The incoming clip's share of the blend at this phase, in `[0, 1]`; `None` outside a
    /// crossfade.
    pub weight: Option<f64>,
    /// The blend parameters this phase evaluated; `None` outside a blend sweep.
    pub params: Option<[f32; 2]>,
    /// The `[yaw, pitch]` in degrees this phase aimed its look-at at, before the look-at's limits
    /// clamp it; `None` without `--look`.
    pub look: Option<[f32; 2]>,
    /// The blend's shared phase in unwrapped cycles (`elapsed × rate(params)`); `None` outside a
    /// blend sweep.
    pub phase: Option<f64>,
}

/// One piece of a posed model that is not connected to its main body, attributed to the parts that
/// own it — the posed twin of `check-scene`'s `connectivity owners:` addendum.
#[derive(Clone, Debug)]
pub struct PhaseStray {
    /// The stray component's first cell in `(y, z, x)` scan order, in posed-grid coordinates.
    pub sample: [u32; 3],
    /// The names of the parts owning that cell, in authored order.
    pub owners: Vec<String>,
    /// The main body's cell nearest this component, and the parts owning it — `None` only if the
    /// nearest-pair search found no pair at all.
    pub main: Option<PhaseGap>,
}

/// The main-body end of a [`PhaseStray`]'s nearest-cell pair.
#[derive(Clone, Debug)]
pub struct PhaseGap {
    /// The main-body cell nearest the stray component, in posed-grid coordinates.
    pub cell: [u32; 3],
    /// The names of the parts owning that cell, in authored order.
    pub owners: Vec<String>,
}

/// The result of a model-sheet render: the stitched contact sheet plus the adapter it ran on.
#[derive(Debug)]
pub struct SheetOutcome {
    /// The stitched contact-sheet image (RGBA8).
    pub image: RenderedImage,
    /// A human-readable adapter description, e.g. `"NVIDIA … (Vulkan)"`.
    pub adapter: String,
    /// What each [`ModelSheetRequest::set`] override changed, in the order they were given.
    pub applied: Vec<AppliedOverride>,
    /// The sweep's report, when one was requested.
    pub sweep: Option<ModelSweepOutcome>,
    /// The `--true-size` framing, when it was requested.
    pub true_size: Option<TrueSizeFraming>,
    /// The clip's phase reports, when [`ModelSheetRequest::clip`] named one. `None` for a rest-pose
    /// sheet, which is every sheet this harness rendered before clips existed.
    pub clip: Option<ClipOutcome>,
}

/// Camera vertical field of view, in degrees.
pub(crate) const FOV_Y_DEG: f32 = 45.0;
/// Fraction of extra distance beyond the tight sphere fit (a 10% framing margin).
const FIT_MARGIN: f32 = 1.1;
/// Camera near clip — small enough to never clip the near face of the framed unit sphere.
pub(crate) const NEAR: f32 = 0.05;
/// Camera far clip — comfortably past the camera distance plus the sphere and ground.
pub(crate) const FAR: f32 = 100.0;
/// Smallest bounding-sphere radius, so a degenerate (empty / single-point) model still frames.
const MIN_RADIUS: f32 = 1e-3;
/// The smallest box a part sheet frames on any axis, as a fraction of the whole model's largest
/// extent: tight enough to read an eye, loose enough that a sliver of a part never shrinks the
/// unit-sphere fit to nothing.
const MIN_PART_FRACTION: f32 = 0.125;
/// How far past the model's box a part sheet's camera stops, in normalized sheet units measured
/// perpendicular to the face it leaves through — a margin beyond the near plane.
const CAMERA_CLEARANCE: f32 = 2.0 * NEAR;
/// Point-light range as a multiple of the camera distance `D`, so attenuation never clips the model.
const LIGHT_RANGE_FACTOR: f32 = 5.0;
/// Key-light intensity (premultiplied into its color by the renderer). Tuned for a clear mid-tone
/// read on the gallery-gray ground under the ranged, squared attenuation of `mesh.wgsl`.
pub(crate) const KEY_INTENSITY: f32 = 3.0;
/// Fill-light intensity — dimmer than the key, to soften shadows without flattening the shading.
pub(crate) const FILL_INTENSITY: f32 = 1.5;
/// Solid fill for a grid cell with no view (only `N = 5`), a deliberately dark neutral gray.
pub(crate) const UNUSED_CELL: [u8; 4] = [24, 26, 30, 255];
/// Orientation-label glyph cell height, in the 1080-tall UI canvas units the overlay measures in
/// (`crate::ui`). Scales with the cell (a fixed fraction of cell height), legible at the default
/// sheet size without swamping the model.
pub(crate) const LABEL_SIZE: f32 = 52.0;
/// Orientation-label inset from the cell's top-left corner, in canvas units (both axes).
pub(crate) const LABEL_OFFSET: f32 = 8.0;
/// Vertical pixels one world unit of subject earns under the vertical slice's gameplay camera at
/// 1080p. `docs/voxel-craft.md` is the source of truth for the measurement: the 2.16-unit statue
/// covers 106 px there. `--true-size` sizes a cell so the subject renders at exactly that, instead
/// of the unit-sphere fit under which a 12-unit building and a 2-unit character are the same size.
const PX_PER_WORLD_UNIT: f32 = 106.0 / 2.16;
/// Largest `--true-size` cell edge in pixels, so an outsized declared `scale:` cannot ask for a
/// render target nothing can allocate.
const MAX_TRUE_SIZE_CELL: f32 = 4096.0;
/// The gap between two lineup neighbours, as a fraction of the *widest* model in the row. Wide enough
/// that two models never read as one silhouette, narrow enough that the camera does not pull back and
/// shrink them — and proportional, so a lineup of props and a lineup of buildings both read. Widest
/// rather than mean, because the widest model is what sets the framing: a gap scaled to the mean
/// disappears next to the one model everything else is being compared against.
const LINEUP_GAP_FRACTION: f32 = 0.15;
/// Largest lineup-caption glyph height, in canvas units. Below the orientation label's, since a
/// caption names a model rather than orienting the whole cell.
const CAPTION_MAX_SIZE: f32 = 44.0;
/// Smallest lineup-caption glyph height: past eight models a wide stem would otherwise vanish, and
/// an unreadable caption is worse than a cramped one.
const CAPTION_MIN_SIZE: f32 = 16.0;
/// A lineup caption's gap below the cell's bottom edge, in canvas units.
const CAPTION_MARGIN: f32 = 16.0;
/// How much of the space between two neighbouring captions their glyphs may fill. Below `1.0` so
/// adjacent captions have visible air between them rather than meeting exactly.
const CAPTION_PACKING: f32 = 0.85;
/// The UI overlay's reference canvas height in canvas units, which a caption's `offset` is measured
/// against. `crate::ui` is the source of truth; it is duplicated here because a caption's horizontal
/// placement is computed from a projection, not resolved from an anchor alone.
const UI_CANVAS_HEIGHT: f32 = 1080.0;

/// Renders a contact sheet around the model document named by `request`.
///
/// [`ModelSheetRequest::views`] is canonicalized inside: duplicates are dropped and the remainder
/// sorted into [`ModelView::ALL`] order, so the sheet layout is always canonical regardless of input
/// order. An **empty** slice means "all six views".
///
/// [`ModelSheetRequest::rig`] optionally names a scene file whose lighting and grade replace the
/// synthesized neutral rig — see the module docs' *Borrowed rigs* section for the keep-list and its
/// caveats. With `rig` at `None` the synthesized scene is exactly what it has always been.
///
/// [`ModelSheetRequest::set`] and [`ModelSheetRequest::sweep`] tune the model's palette for the
/// render only; see the module docs' *Palette overrides* section. A `--sweep` lays one cell per
/// value instead of one per view.
///
/// # Errors
/// Every failure is a `Vec<Diagnostic>`:
/// - `L1117` if the model cannot be read, has no `kind:` key, or names a `kind:` that is none of
///   `mesh`, `voxel`, `assembly`, or `sculpt`; if a `rig` scene cannot be read or holds none of the
///   four liftable component types; or if an overridden copy cannot be staged in the temp directory.
/// - `L1110`/`L1111` (mesh), `L1113`/`L1114` (voxel), `L1137`/`L1138` (assembly structure/data,
///   including its part files' own `L1139`/`L1140` and the missing-file `L1101`), or
///   `L1156`/`L1157`/`L1160` (sculpt) if the document is invalid — including a value an override
///   put out of range, which is the document's own diagnostic, not the override's.
/// - `L4008` for an override outside the document's domain, an override against a mesh document, a
///   sweep that does not fit one sheet of one view, or that is paired with a strip; or a `part` that
///   is not a surfaced part of a single sculpt, and an `isolate` without a `part`.
/// - the rig scene's own parse/validation diagnostics (`L1001`–`L1014`) if it is not a valid scene.
/// - `L2003`/`L2004`/`L2005` from the GPU (adapter, device, or readback).
pub fn render_model_sheet(request: &ModelSheetRequest) -> Result<SheetOutcome, Vec<Diagnostic>> {
    let lineup = request.models.len() > 1;
    check_clip_flags(request)?;
    check_part_flags(request)?;
    // A strip spends its one cell on phases exactly as a lineup spends it on models, so the two
    // share every layout decision from here down.
    let fanned = request.strip.is_some()
        || request.crossfade.is_some()
        || sweeps_blend(request)
        || sweeps_look(request);
    let row = lineup || fanned;
    // A row spends its one cell on subjects rather than angles, so it takes one view — and defaults
    // to the gameplay one, the angle the comparison is actually made at.
    let views = if row && request.views.is_empty() {
        vec![ModelView::Game]
    } else {
        canonical_views(request.views)
    };
    if lineup {
        check_lineup_shape(request, &views)?;
    }
    if fanned {
        check_strip_shape(&views)?;
    }
    if let Some(sweep) = request.sweep {
        let phases = request.strip.or_else(|| {
            (request.crossfade.is_some() || sweeps_blend(request) || sweeps_look(request))
                .then_some(DEFAULT_CROSSFADE_PHASES)
        });
        check_sweep_shape(&sweep, &views, phases)?;
    }

    // --- All CPU-side work happens before the GPU is requested. ---
    let mut loaded = load_models(request.models)?;
    check_alone(&loaded)?;
    remesh_overridden_sculpt(request, &mut loaded)?;
    let focus = request
        .part
        .map(|name| resolve_part(&loaded[0], name))
        .transpose()?;
    let posed = match (request.clip, request.crossfade, request.blend) {
        (Some(clip), _, _) => Some(resolve_clip(request, request.models[0], &loaded[0], clip)?),
        (None, Some(spec), _) => Some(resolve_crossfade(
            request,
            request.models[0],
            &loaded[0],
            spec,
        )?),
        (None, None, Some(spec)) => {
            Some(resolve_blend(request, request.models[0], &loaded[0], spec)?)
        }
        (None, None, None) => None,
    };
    let lift = request.rig.map(load_rig).transpose()?;
    // One entry per rendered *entity*: a strip stands the one model up once per phase, so its
    // bounds repeat and the row machinery lays them out with no further special-casing.
    let entities = posed.as_ref().map_or(1, |posed| posed.started.len());
    let bounds: Vec<&ModelBounds> = if fanned {
        vec![&loaded[0].bounds; entities]
    } else {
        loaded.iter().map(|model| &model.bounds).collect()
    };
    let subject = match &focus {
        _ if row => Subject::lineup(&bounds, row_direction(views[0])),
        Some(focus) => Subject::part(bounds[0], focus.bounds),
        None => Subject::single(bounds[0]),
    };

    // One cell per swept value, else one per view — a row being one cell whatever it holds. Not
    // the grid's slot count: `N = 5` or `N = 7` views leave a slot over, and it renders nothing.
    let cell_count = match request.sweep {
        _ if row => 1,
        Some(sweep) => sweep.values.len(),
        None => views.len(),
    };
    let grid = match request.sweep {
        Some(_) if !row => Grid::near_square(cell_count as u32),
        _ => Grid::for_views(cell_count as u32),
    };
    let (layout, true_size) = if request.true_size {
        let framing = TrueSizeFraming::compute(&subject);
        let (cell_w, cell_h) = framing.cell(&subject, grid, request.width, request.height);
        (SheetLayout::from_cell(grid, cell_w, cell_h), Some(framing))
    } else {
        (
            SheetLayout::divide(grid, request.width, request.height),
            None,
        )
    };

    let staged = stage(request, &loaded, cell_count)?;
    // A strip's entities all reference the one staged model; a lineup's each reference their own.
    let names: Vec<String> = if fanned {
        vec![staged.names[0].clone(); entities]
    } else {
        staged.names.clone()
    };
    // A phase's caption is the instant it shows (a crossfade's, its blend weight), which is the only
    // thing that distinguishes it from its neighbours; a lineup's is the model's own name.
    let labels: Vec<String> = match &posed {
        Some(posed) if fanned => posed
            .reports
            .iter()
            .map(|report| match (report.weight, report.params) {
                (Some(weight), _) => format!("w {weight:.2}"),
                (None, Some([x, y])) if posed.two_d => format!("p {x:.2} {y:.2}"),
                (None, Some([x, _])) => format!("p {x:.2}"),
                (None, None) => format!("t {:.2}", report.time),
            })
            .collect(),
        _ => loaded
            .iter()
            .map(|model| model_stem(&model.bounds.file_name))
            .collect(),
    };
    let captions = if row {
        lineup_captions(&labels, &subject, views[0], &layout)
    } else {
        Vec::new()
    };
    // The `Animation` each entity carries: one `started` frame per phase, so a single extract at the
    // shared `now` reads every phase at its own clip time (`crate::extract::clip_elapsed`).
    let animations: Vec<Option<AnimationSpec>> = match &posed {
        None => vec![None; bounds.len()],
        Some(posed) => posed
            .started
            .iter()
            .map(|phase| {
                Some(AnimationSpec {
                    clip: posed.name.as_str(),
                    started: phase.started,
                    params: phase.params,
                    layers: request.layers,
                    ik: Some(&posed.ik).filter(|ik| !ik.is_empty()),
                    look: phase.look,
                    fade: phase.fade.map(|(from_started, fade_started)| FadeSpec {
                        from: posed.from.as_deref().unwrap_or_default(),
                        from_started,
                        fade_started,
                        seconds: posed.fade_seconds,
                    }),
                })
            })
            .collect(),
    };
    let now = SimTime::from_ticks(
        posed.as_ref().map_or(0, |posed| posed.now),
        TickRate::DEFAULT,
    );

    // Synthesize, validate, and extract every cell's scene up front — a `check` failure here is an
    // internal bug (our own YAML), surfaced raw rather than swallowed. Extraction is CPU-only.
    let mut render_scenes = Vec::with_capacity(staged.roots.len());
    // Consecutive cells share a root unless a sweep staged one per value, so the attribution is
    // meshed once per distinct root.
    let mut attribution: Option<(&Path, Vec<Option<u32>>)> = None;
    for (index, root) in staged.roots.iter().enumerate() {
        // A sweep spends its cells on values (one view, N labels); every other sheet spends them on
        // views (one cell each, labelled with the orientation it shows).
        let (view, label) = match request.sweep {
            Some(sweep) => (views[0], sweep_label(sweep.pointer, &sweep.values[index])),
            None => (views[index], views[index].label().to_string()),
        };
        let yaml = synthesize_scene(
            &bounds,
            &names,
            &subject,
            lift.as_ref(),
            view,
            &label,
            &captions,
            &animations,
            layout.cell_w,
            layout.cell_h,
            request.channel,
        );
        let scene = ludion_scene::check(&yaml, &builtin_registry())?;
        // A prefab is an instance entry, not a mesh shape, so its cell expands before it spawns.
        let world = if matches!(bounds[0].kind, ModelKind::Prefab(_)) {
            crate::render::expanded_world(root, &builtin_registry(), &scene)?.1
        } else {
            ludion_scene::instantiate(&scene, &builtin_registry())?
        };
        let mut driver = SimDriver::new(world, Schedule::new(), SimConfig::default());
        driver.run_ticks(0);
        // `now` is tick 0 for every un-posed sheet, which is exactly what `extract()` passes — so a
        // rest-pose sheet extracts byte-identically to what it always did.
        let mut render_scene = extract_at(
            driver.world(),
            (layout.cell_w, layout.cell_h),
            ludion_assets::AssetRoot::local_document_unconfined(root),
            now,
            QualityParams::FULL.particle_density,
        )?;
        if let Some(focus) = focus.as_ref().filter(|_| request.isolate) {
            // Extraction has already validated and meshed this root's document, so re-meshing it
            // here cannot fail on anything the extract did not report first.
            if attribution
                .as_ref()
                .is_none_or(|(at, _)| *at != root.as_path())
            {
                attribution = Some((root, part_attribution(root, &staged.names[0])?));
            }
            if let Some((_, part_of)) = &attribution {
                isolate_part(&mut render_scene, part_of, focus.index);
            }
        }
        render_scenes.push(render_scene);
    }

    // --- GPU work: one renderer, reused across every cell. ---
    let renderer = Renderer::new().map_err(|d| vec![d])?;
    let adapter = format!(
        "{} ({})",
        renderer.adapter_name(),
        renderer.adapter_backend()
    );

    let mut sheet = vec![0u8; layout.sheet_w as usize * layout.sheet_h as usize * 4];
    fill(&mut sheet, UNUSED_CELL);
    for (index, render_scene) in render_scenes.iter().enumerate() {
        let cell = renderer
            .render(
                render_scene,
                FrameSize::native(layout.cell_w, layout.cell_h),
            )
            .map_err(|d| vec![d])?;
        let row = index as u32 / layout.cols;
        let col = index as u32 % layout.cols;
        blit_cell(&mut sheet, layout.sheet_w, &cell, row, col, &layout);
    }

    Ok(SheetOutcome {
        image: RenderedImage {
            width: layout.sheet_w,
            height: layout.sheet_h,
            rgba: sheet,
        },
        adapter,
        applied: staged.applied,
        sweep: request.sweep.map(|sweep| ModelSweepOutcome {
            pointer: sweep.pointer.to_string(),
            original: staged.original,
            cols: grid.cols,
            rows: grid.rows,
        }),
        true_size,
        clip: posed.map(|posed| ClipOutcome {
            crossfade: posed.from.map(|from| ClipCrossfade {
                from,
                seconds: f64::from(posed.fade_seconds),
            }),
            blend: posed.blend,
            layers: request
                .layers
                .iter()
                .map(|layer| {
                    format!(
                        "{} {:?} {} {}",
                        layer.clip,
                        layer.weight,
                        layer.mask.unwrap_or("-"),
                        match layer.mode {
                            LayerMode::Override => "override",
                            LayerMode::Additive => "additive",
                        }
                    )
                })
                .collect(),
            ik: ik_echoes(request),
            name: posed.name,
            duration: posed.duration,
            looped: posed.looped,
            hz: TickRate::DEFAULT.hz(),
            hold: request.hold.unwrap_or(1),
            phases: posed.reports,
        }),
    })
}

/// A lineup answers one question — how do these models read *against each other* — so it spends its
/// one cell on models rather than on angles or on values.
fn check_lineup_shape(
    request: &ModelSheetRequest,
    views: &[ModelView],
) -> Result<(), Vec<Diagnostic>> {
    let refuse = |message: String, help: &str| {
        Err(vec![
            Diagnostic::new(code::SCENE_OVERRIDE_INVALID, message).with_help(help.to_string()),
        ])
    };
    if request.models.len() > MAX_LINEUP_MODELS {
        return refuse(
            format!(
                "a lineup of {} models does not fit one cell; at most {MAX_LINEUP_MODELS} stay wide \
                 enough to compare",
                request.models.len()
            ),
            "line up fewer models at a time — a comparison is between neighbours, not a catalogue",
        );
    }
    if views.len() != 1 {
        return refuse(
            format!(
                "a lineup renders every model in one cell, so it takes one view, but {} were \
                 requested",
                views.len()
            ),
            "pick one view, e.g. `--views game`; omitting `--views` gives a lineup the game view",
        );
    }
    if !request.set.is_empty() || request.sweep.is_some() {
        return refuse(
            "`--set`/`--sweep` tune one model's palette, and a lineup renders several".to_string(),
            "calibrate the one model first (`render --model <one> --sweep …`), write the value into \
             its document, then line it up against its neighbours",
        );
    }
    Ok(())
}

/// `L1117` for a clip request the harness cannot serve, in the wording style the assembly-override
/// deferral uses: what was asked, why it does not apply, and the spelling that does.
fn refuse_clip(message: String, help: &str) -> Vec<Diagnostic> {
    vec![Diagnostic::new(code::MODEL_SHEET_INPUT, message).with_help(help.to_string())]
}

/// The clip flags' shape, checked before anything is read from disk: `time`/`strip` are modifiers of
/// `clip` and mean nothing without it, a clip poses one model rather than a row of them, and a strip
/// of one phase is a posed sheet spelled the long way.
fn check_clip_flags(request: &ModelSheetRequest) -> Result<(), Vec<Diagnostic>> {
    if request.clip.is_some() && request.crossfade.is_some() {
        return Err(refuse_clip(
            "`--clip` and `--crossfade` both pose the model: a crossfade already names the clips \
             it plays"
                .to_string(),
            "use `--clip <name>` for one clip, or `--crossfade <from>:<to>:<seconds>` for the blend \
             of two",
        ));
    }
    if request.blend.is_some() && (request.clip.is_some() || request.crossfade.is_some()) {
        let other = if request.clip.is_some() {
            "`--clip`"
        } else {
            "`--crossfade`"
        };
        return Err(refuse_clip(
            format!(
                "`--blend` and {other} both pose the model: a blend already names the clips it \
                 plays"
            ),
            "use `--blend <name>:<p0>[:<p1>]` for a blend space, `--clip <name>` for one clip, or \
             `--crossfade <from>:<to>:<seconds>` for the fade of two",
        ));
    }
    if request.layers.len() > MAX_ANIMATION_LAYERS {
        return Err(vec![
            Diagnostic::new(
                code::ANIMATION_LAYERS_TOO_MANY,
                format!(
                    "`--layer` was given {} times, but a stack holds at most {MAX_ANIMATION_LAYERS} \
                     layers",
                    request.layers.len()
                ),
            )
            .with_help("drop a `--layer`, or fold two layers' clips into one"),
        ]);
    }
    if !request.layers.is_empty()
        && request.clip.is_none()
        && request.crossfade.is_none()
        && request.blend.is_none()
    {
        return Err(refuse_clip(
            "`--layer` needs a base pose: layers compose over `--clip`, `--crossfade` or `--blend`"
                .to_string(),
            "name the base too, e.g. `--clip walk --layer wave:1.0:arm_only`",
        ));
    }
    let aim = match (request.look, request.ik) {
        (Some(_), _) => Some("`--look`"),
        (None, [_, ..]) => Some("`--ik`"),
        (None, []) => None,
    };
    if let Some(aim) = aim {
        if request.clip.is_none() && request.crossfade.is_none() && request.blend.is_none() {
            return Err(refuse_clip(
                format!(
                    "{aim} needs a base pose: the solver aims over `--clip`, `--crossfade` or \
                     `--blend`"
                ),
                "name the base too, e.g. `--clip idle --look 40,0`",
            ));
        }
        if sweeps_look(request)
            && request.blend.is_some_and(|blend| {
                matches!(blend.p0, Sweep::Range(..)) || matches!(blend.p1, Some(Sweep::Range(..)))
            })
        {
            return Err(refuse_clip(
                "a `--look` range and a `--blend` range both sweep the strip, and a strip sweeps one \
                 thing"
                    .to_string(),
                "hold the blend at a value (`--blend locomotion:1`) while the look sweeps, or give \
                 `--look` plain angles while the blend sweeps",
            ));
        }
    }
    if request.hold == Some(0) {
        return Err(vec![
            Diagnostic::new(
                code::ANIMATION_HOLD_ZERO,
                "`--hold 0` is not a pose cadence: hold is a whole number of ticks ≥ 1",
            )
            .with_help("no flag, or `--hold 1`, plays every tick; 5 is 12 poses/s at 60 Hz"),
        ]);
    }
    if request.clip.is_none() && request.crossfade.is_none() && request.blend.is_none() {
        let orphan = match (request.time, request.strip, request.hold) {
            (Some(_), _, _) => "`--time`",
            (None, Some(_), _) => "`--strip`",
            (None, None, Some(_)) => "`--hold`",
            (None, None, None) => return Ok(()),
        };
        return Err(refuse_clip(
            format!(
                "{orphan} needs `--clip` or `--crossfade`: there is no clip to take an instant of"
            ),
            "name the clip too, e.g. `--clip walk --time 0.4`; `check-scene <model>.assembly.yaml` \
             (or `.character.yaml`) lists the clips a model binds",
        ));
    }
    let flag = if request.crossfade.is_some() {
        "`--crossfade`"
    } else if request.blend.is_some() {
        "`--blend`"
    } else {
        "`--clip`"
    };
    if request.models.len() > 1 {
        return Err(refuse_clip(
            format!(
                "{flag} poses one model, and {} were given",
                request.models.len()
            ),
            "pose each model on its own sheet; a lineup compares rest poses, which is the shape a \
             row of different models reads in",
        ));
    }
    if let Some(phases) = request.strip
        && !(MIN_STRIP_PHASES..=MAX_STRIP_PHASES).contains(&phases)
    {
        return Err(refuse_clip(
            format!(
                "`--strip {phases}` is out of range: a strip lays out {MIN_STRIP_PHASES} to \
                 {MAX_STRIP_PHASES} phases"
            ),
            "ask for at least two phases (one phase is a posed sheet — `--clip <name> --time <t>`) \
             and at most eight, past which a phase is too narrow to read the pose in",
        ));
    }
    Ok(())
}

/// `L4008` for a part request that cannot be served, in the override family's wording: what was
/// asked, why it does not apply, and the spelling that does.
fn refuse_part(message: String, help: String) -> Vec<Diagnostic> {
    vec![Diagnostic::new(code::SCENE_OVERRIDE_INVALID, message).with_help(help)]
}

/// The part flags' shape, checked before anything is read from disk: `isolate` modifies `part`, and a
/// part belongs to one model.
fn check_part_flags(request: &ModelSheetRequest) -> Result<(), Vec<Diagnostic>> {
    match request.part {
        None if request.isolate => Err(refuse_part(
            "`--isolate` needs `--part`: there is no part to isolate".to_string(),
            "name the part too, e.g. `--part head --isolate`; `check-scene <model>.sculpt.yaml \
             --bounds` lists a sculpt's parts"
                .to_string(),
        )),
        Some(name) if request.models.len() > 1 => Err(refuse_part(
            format!(
                "`--part {name}` frames one model's part, and {} models were given",
                request.models.len()
            ),
            "frame the part on its own sheet (`render --model <one>.sculpt.yaml --part <name>`)"
                .to_string(),
        )),
        _ => Ok(()),
    }
}

/// The part a `--part` sheet frames: its index in the document and its meshed bounds.
struct Focus {
    index: u32,
    bounds: ([f32; 3], [f32; 3]),
}

/// Resolves `name` against the model's meshed parts — a sculpt's, the one kind whose parts are named
/// surfaces of one mesh.
fn resolve_part(model: &LoadedModel, name: &str) -> Result<Focus, Vec<Diagnostic>> {
    let LoadedExtra::Sculpt(sculpt) = &model.extra else {
        return Err(refuse_part(
            format!(
                "`--part {name}` frames a named part of a sculpt, but `{}` is a `kind: {}` model",
                model.bounds.file_name,
                model.bounds.kind.document_kind()
            ),
            "`--part` addresses a `kind: sculpt` document's `parts:` by name; render this model \
             without it"
                .to_string(),
        ));
    };
    let parts = &sculpt.mesh.stats.parts;
    let Some(index) = parts.iter().position(|part| part.name == name) else {
        let listed = parts
            .iter()
            .map(|part| format!("`{}`", part.name))
            .collect::<Vec<_>>()
            .join(", ");
        let help = match ludion_core::suggest::nearest(name, parts.iter().map(|p| p.name.as_str()))
        {
            Some(nearest) => {
                format!("nearest match is `{nearest}`; the sculpt's parts are {listed}")
            }
            None if parts.is_empty() => "the sculpt has no parts yet".to_string(),
            None => format!("the sculpt's parts are {listed}"),
        };
        return Err(refuse_part(
            format!(
                "`--part {name}` names no part of `{}`",
                model.bounds.file_name
            ),
            help,
        ));
    };
    let Some(bounds) = parts[index].bounds else {
        return Err(refuse_part(
            format!(
                "`--part {name}` has nothing to frame: the part contributes no surface to the mesh \
                 of `{}`",
                model.bounds.file_name
            ),
            "`check-scene` reports why (`L1158`: hidden inside another part, carved away, or \
             thinner than two grid cells); frame a part that shows, or fix this one first"
                .to_string(),
        ));
    };
    Ok(Focus {
        index: index as u32,
        bounds,
    })
}

/// Re-meshes a single sculpt under the request's `--set` overrides, so its framing (bounds and part
/// boxes) follows the surface the cells will show: a sculpt override can move geometry, which a
/// palette override never does. The document itself stays as authored — [`stage`] applies the same
/// overrides on the way to disk — and a sweep frames on this base, so its cells share one camera.
fn remesh_overridden_sculpt(
    request: &ModelSheetRequest,
    loaded: &mut [LoadedModel],
) -> Result<(), Vec<Diagnostic>> {
    let [model] = loaded else {
        return Ok(());
    };
    let LoadedExtra::Sculpt(sculpt) = &mut model.extra else {
        return Ok(());
    };
    if request.set.is_empty() {
        return Ok(());
    }
    let mut doc = sculpt.doc.clone();
    ludion_scene::apply_sculpt_overrides(&mut doc, request.set)?;
    sculpt.mesh =
        ludion_sculpt::mesh_with(&doc, &sculpt.sources).map_err(|diagnostic| vec![diagnostic])?;
    (model.bounds.min, model.bounds.max) = sculpt_aabb(&sculpt.mesh);
    Ok(())
}

/// The per-vertex part attribution of the sculpt staged at `root`/`file_name` — the document one
/// cell's extract just meshed, so the attribution indexes exactly that cell's vertices.
fn part_attribution(root: &Path, file_name: &str) -> Result<Vec<Option<u32>>, Vec<Diagnostic>> {
    let path = root.join(file_name);
    let source = std::fs::read_to_string(&path).map_err(|error| {
        vec![
            Diagnostic::new(
                code::MODEL_SHEET_INPUT,
                format!("could not re-read sculpt `{}`: {error}", path.display()),
            )
            .with_help("the sheet reads the model twice; check it was not moved mid-render"),
        ]
    })?;
    let doc = ludion_scene::check_sculpt(&source)?;
    let sources = crate::sculpt_mesh::read_sculpt_sources(
        ludion_assets::AssetRoot::local_document_unconfined(root),
        file_name,
        &doc,
    )?;
    let mesh = ludion_sculpt::mesh_with(&doc, &sources).map_err(|diagnostic| vec![diagnostic])?;
    Ok(mesh.part_of)
}

/// Mixed into an isolated mesh's hash so it never shares a renderer cache key with the whole model.
const ISOLATE_HASH_SALT: u64 = 0x9e37_79b9_7f4a_7c15;

/// Keeps only the triangles of `part` in the cell's model mesh: a triangle stays when any of its
/// vertices is attributed to the part. A sheet scene's only mesh asset is its model (the ground is a
/// primitive), so every entry is filtered.
fn isolate_part(scene: &mut RenderScene, part_of: &[Option<u32>], part: u32) {
    for mesh in &mut scene.mesh_assets {
        debug_assert_eq!(mesh.positions.len(), 3 * part_of.len());
        let indices: Vec<u32> = mesh
            .indices
            .chunks_exact(3)
            .filter(|triangle| {
                triangle
                    .iter()
                    .any(|&v| part_of.get(v as usize).copied().flatten() == Some(part))
            })
            .flatten()
            .copied()
            .collect();
        let mut isolated = MeshData::clone(mesh);
        isolated.indices = indices;
        isolated.hash = mesh.hash ^ ISOLATE_HASH_SALT.wrapping_mul(u64::from(part) + 1);
        *mesh = Arc::new(isolated);
    }
}

/// A character or a prefab renders alone: its mesh, parts and clips resolve relative to its own
/// file, and a lineup stages every model into a numbered slot of its own.
fn check_alone(loaded: &[LoadedModel]) -> Result<(), Vec<Diagnostic>> {
    if loaded.len() > 1 {
        for model in loaded {
            let (kind, resolves) = match model.extra {
                LoadedExtra::Character(_) => ("character", "its mesh and clips"),
                LoadedExtra::Prefab(_) => ("prefab", "its parts and clips"),
                _ => continue,
            };
            return Err(refuse_clip(
                format!(
                    "a lineup cannot include a `kind: {kind}` model: a {kind} renders alone, \
                     since {resolves} resolve relative to its own file"
                ),
                &format!(
                    "render the {kind} on its own sheet (`render --model <name>.{kind}.yaml`)"
                ),
            ));
        }
    }
    Ok(())
}

/// A strip lays every phase out in one cell, so — exactly like a lineup — it takes one view.
fn check_strip_shape(views: &[ModelView]) -> Result<(), Vec<Diagnostic>> {
    if views.len() != 1 {
        return Err(refuse_clip(
            format!(
                "a strip renders every phase in one cell, so it takes one view, but {} were \
                 requested",
                views.len()
            ),
            "pick one view, e.g. `--views game`; omitting `--views` gives a strip the game view",
        ));
    }
    Ok(())
}

/// A model sweep must fit one sheet, and must name exactly one variable: N values across N views —
/// or across a strip's N phases — would be a two-variable grid, and a taste call reads one variable
/// at a time.
fn check_sweep_shape(
    sweep: &ModelSweep,
    views: &[ModelView],
    strip: Option<u32>,
) -> Result<(), Vec<Diagnostic>> {
    let refuse = |message: String, help: &str| {
        Err(vec![
            Diagnostic::new(code::SCENE_OVERRIDE_INVALID, message)
                .with_path(sweep.pointer.to_string())
                .with_help(help.to_string()),
        ])
    };
    if sweep.values.is_empty() {
        return refuse(
            "a sweep needs at least one value".to_string(),
            "give `--sweep <pointer>=<v,v,…>` at least one value to try",
        );
    }
    if sweep.values.len() > MAX_SWEEP_CELLS {
        return refuse(
            format!(
                "a sweep of {} values does not fit a contact sheet; at most {MAX_SWEEP_CELLS} \
                 cells stay large enough to judge",
                sweep.values.len()
            ),
            "split the range across several sheets, or sweep fewer candidates at a time",
        );
    }
    if views.len() != 1 {
        return refuse(
            format!(
                "a model sweep renders one view per value, but {} views were requested",
                views.len()
            ),
            "pick one view for the sweep, e.g. `--views game`; without `--views` the sheet takes \
             all six",
        );
    }
    if let Some(phases) = strip {
        return refuse(
            format!(
                "a model sweep renders one cell per value, and `--strip {phases}` renders one cell \
                 per phase"
            ),
            "both multiply cells; pick one — sweep at a single instant (`--clip <name> --time \
             <t>`), or strip the phases of the value the document already holds",
        );
    }
    Ok(())
}

/// One subject's caption in a row: its label, placed under the thing it names.
struct Caption {
    text: String,
    offset: [f32; 2],
    size: f32,
}

/// Places one caption under each subject of a row, so a row of silhouettes is self-describing.
/// `labels` is one string per placement — a lineup's model stems, or a strip's phase instants.
///
/// The horizontal position is the model's footprint center **projected through the cell's own
/// camera**, not an even division of the sheet: a row seen at the `game` camera's angle does not
/// spread evenly across the frame, and a caption that drifts off its model names the wrong one.
///
/// Glyph height then comes from the *measured* spacing between neighbours rather than from the cell
/// width divided by the model count. Two models can project close together — a small model beside a
/// large one is exactly the comparison this feature is for — and an even division would happily
/// overlap them.
fn lineup_captions(
    labels: &[String],
    subject: &Subject,
    view: ModelView,
    layout: &SheetLayout,
) -> Vec<Caption> {
    let stems = labels;
    let aspect = layout.cell_w as f32 / layout.cell_h as f32;
    let canvas_w = UI_CANVAS_HEIGHT * aspect;

    let view_rig = ViewRig::compute(view, subject, layout.cell_w, layout.cell_h);
    let (_, up) = view.direction_and_up();
    let clip = Mat4::perspective_rh(FOV_Y_DEG.to_radians(), aspect, NEAR, FAR)
        * Mat4::look_at_rh(view_rig.position, subject.target, up);
    let centers: Vec<f32> = subject
        .placements
        .iter()
        .map(|placement| {
            let projected = clip * placement.base.extend(1.0);
            // Behind or on the camera plane cannot project; park the caption on the cell's midline
            // rather than at an infinity. A sheet camera always frames its own subject, so this is a
            // guard, not a case.
            let ndc_x = if projected.w.abs() < f32::EPSILON {
                0.0
            } else {
                (projected.x / projected.w).clamp(-1.0, 1.0)
            };
            ndc_x * 0.5 * canvas_w
        })
        .collect();

    // The largest glyph height at which no two neighbouring captions touch: each pair needs half of
    // each caption's width plus a little air, and a caption is `len` square glyph cells wide.
    let mut size = CAPTION_MAX_SIZE;
    for (index, pair) in centers.windows(2).enumerate() {
        let glyphs = (stems[index].len() + stems[index + 1].len()) as f32;
        size = size.min(2.0 * CAPTION_PACKING * (pair[1] - pair[0]).abs() / glyphs.max(1.0));
    }
    let size = size.clamp(CAPTION_MIN_SIZE, CAPTION_MAX_SIZE);

    stems
        .iter()
        .zip(centers)
        .map(|(text, center)| Caption {
            text: text.clone(),
            // `TopCenter` puts the canvas point at the top-center and the *text's* top-center on it,
            // so the offset is the horizontal displacement from the cell's midline.
            offset: [center, UI_CANVAS_HEIGHT - size - CAPTION_MARGIN],
            size,
        })
        .collect()
}

/// A model's caption text: its file name with the extension and the `.voxel`/`.mesh`/`.assembly`/
/// `.sculpt` tag stripped, matching how `render --model` names its default output.
fn model_stem(file_name: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(file_name);
    stem.strip_suffix(".voxel")
        .or_else(|| stem.strip_suffix(".mesh"))
        .or_else(|| stem.strip_suffix(".assembly"))
        .or_else(|| stem.strip_suffix(".sculpt"))
        .unwrap_or(stem)
        .to_string()
}

/// The label burned into one sweep cell: the swept field then its value, e.g. `G/emissive 2.2`. The
/// leading `/palette/` is dropped (it is the same on every cell) and there is no `=` — the label
/// stays short.
fn sweep_label(pointer: &str, value: &Value) -> String {
    let field = pointer.strip_prefix("/palette/").unwrap_or(pointer);
    let value = match value {
        // A JSON string renders without its quotes: they add width and no information.
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    format!("{field} {value}")
}

/// Canonicalizes a view request: an empty slice means the [`ModelView::DEFAULT`] six; otherwise
/// dedup and sort into [`ModelView::ALL`] order (a pure function of the set of requested views, so an
/// explicit request can include the opt-in `Game` view).
fn canonical_views(views: &[ModelView]) -> Vec<ModelView> {
    if views.is_empty() {
        return ModelView::DEFAULT.to_vec();
    }
    ModelView::ALL
        .iter()
        .copied()
        .filter(|v| views.contains(v))
        .collect()
}

/// How many cells the sheet lays out, and in what shape. Split from [`SheetLayout`] because the
/// cell *size* depends on the grid under `--true-size` but never the other way round.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Grid {
    pub(crate) cols: u32,
    pub(crate) rows: u32,
}

impl Grid {
    /// The turntable rule: `rows = 1` for up to three views, else `2`; `cols = ceil(N/rows)`.
    fn for_views(n: u32) -> Grid {
        let n = n.max(1);
        let rows = if n <= 3 { 1 } else { 2 };
        Grid {
            cols: n.div_ceil(rows),
            rows,
        }
    }

    /// The sweep rule: near-square, `cols = ceil(sqrt(N))`, because cell *area* is what a taste call
    /// needs. Matches `crate::sweep`'s grid so the two contact sheets read as one family.
    pub(crate) fn near_square(n: u32) -> Grid {
        let n = n.max(1);
        // Integer ceil(sqrt(n)) without transcendentals: the smallest `c` with `c*c >= n`.
        let mut cols = 1;
        while cols * cols < n {
            cols += 1;
        }
        Grid {
            cols,
            rows: n.div_ceil(cols),
        }
    }
}

/// The grid geometry of the sheet: a [`Grid`] plus the pixel size of one cell.
pub(crate) struct SheetLayout {
    pub(crate) cols: u32,
    pub(crate) cell_w: u32,
    pub(crate) cell_h: u32,
    pub(crate) sheet_w: u32,
    pub(crate) sheet_h: u32,
}

impl SheetLayout {
    /// Cells divide the requested size (integer), and the final sheet is exactly
    /// `cell_w·cols × cell_h·rows` — so it may come back slightly smaller than requested.
    pub(crate) fn divide(grid: Grid, sheet_width: u32, sheet_height: u32) -> SheetLayout {
        SheetLayout::from_cell(
            grid,
            (sheet_width / grid.cols).max(1),
            (sheet_height / grid.rows).max(1),
        )
    }

    /// The sheet a cell size implies — the `--true-size` path, where the cell is computed from the
    /// subject rather than divided out of a requested sheet.
    fn from_cell(grid: Grid, cell_w: u32, cell_h: u32) -> SheetLayout {
        SheetLayout {
            cols: grid.cols,
            cell_w,
            cell_h,
            sheet_w: cell_w * grid.cols,
            sheet_h: cell_h * grid.rows,
        }
    }

    /// The view-grid layout for `n` views at a requested sheet size.
    #[cfg(test)]
    fn compute(n: u32, sheet_width: u32, sheet_height: u32) -> SheetLayout {
        SheetLayout::divide(Grid::for_views(n), sheet_width, sheet_height)
    }
}

impl TrueSizeFraming {
    /// The subject's world height and the pixels it earns under the gameplay camera. In a lineup the
    /// tallest model earns them, and every other model is honest against it by construction.
    fn compute(subject: &Subject) -> TrueSizeFraming {
        TrueSizeFraming {
            world_height: subject.world_height,
            subject_px: subject.world_height * PX_PER_WORLD_UNIT,
        }
    }

    /// The cell that renders the subject at [`subject_px`](TrueSizeFraming::subject_px).
    ///
    /// The sheet frames a *normalized* subject: [`NormalizeRig`] scales the model by `k = 1/radius`
    /// and the camera sits at [`fit_distance`] from the look-at target, where a cell of `cell_h`
    /// pixels spans `2·D·tan(fov_y/2)` world units. The subject's normalized height therefore takes
    /// `h_n / (2·D·tan(fov_y/2))` of the cell, and inverting that for the earned pixels gives the
    /// cell height directly. `D` is a function of the cell's *aspect* alone, so the aspect is fixed
    /// first — from the requested sheet size divided by the grid, which under `--true-size` is all
    /// that size is for — and the cell then solves in one step.
    ///
    /// A flat subject (no finite, positive normalized height) has no earned size, so it gets the
    /// divided cell of the non-true-size layout.
    fn cell(self, subject: &Subject, grid: Grid, width: u32, height: u32) -> (u32, u32) {
        let divided = SheetLayout::divide(grid, width, height);
        if !(subject.normalized_height.is_finite() && subject.normalized_height > 0.0) {
            return (divided.cell_w, divided.cell_h);
        }
        let aspect = divided.cell_w as f32 / divided.cell_h as f32;
        let half_y = (FOV_Y_DEG * 0.5).to_radians();
        let visible = 2.0 * fit_distance_for_aspect(aspect) * math::tan(half_y);

        let cell_h =
            (self.subject_px * visible / subject.normalized_height).clamp(1.0, MAX_TRUE_SIZE_CELL);
        let cell_w = (cell_h * aspect).clamp(1.0, MAX_TRUE_SIZE_CELL);
        (cell_w.round() as u32, cell_h.round() as u32)
    }
}

/// Fills an RGBA8 buffer with one color.
pub(crate) fn fill(buffer: &mut [u8], color: [u8; 4]) {
    for pixel in buffer.chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }
}

/// Row-copies one rendered `cell` (`cell_w` × `cell_h`) into the sheet at grid position `(row, col)`.
pub(crate) fn blit_cell(
    sheet: &mut [u8],
    sheet_w: u32,
    cell: &RenderedImage,
    row: u32,
    col: u32,
    layout: &SheetLayout,
) {
    let row_bytes = (layout.cell_w * 4) as usize;
    let sheet_stride = (sheet_w * 4) as usize;
    let x_offset = (col * layout.cell_w * 4) as usize;
    for r in 0..layout.cell_h as usize {
        let src = &cell.rgba[r * row_bytes..r * row_bytes + row_bytes];
        let dst_y = (row * layout.cell_h) as usize + r;
        let dst = dst_y * sheet_stride + x_offset;
        sheet[dst..dst + row_bytes].copy_from_slice(src);
    }
}

/// Which authoring surface a model document is, and its content-root-relative reference.
enum ModelKind {
    /// A `*.voxel.yaml` voxel model, referenced by a `Mesh.shape: { voxel: … }`.
    Voxel,
    /// A `*.mesh.yaml` mesh asset, referenced by a `Mesh.shape: { asset: … }`.
    Mesh,
    /// A `*.assembly.yaml` multi-part voxel assembly, referenced by a `Mesh.shape: { assembly: … }`
    /// — rendered at its rest pose.
    Assembly,
    /// A `*.sculpt.yaml` implicit-surface model, referenced by a `Mesh.shape: { sculpt: … }`.
    Sculpt,
    /// A `*.character.yaml` skinned mesh with bound clips, referenced by a
    /// `Mesh.shape: { character: … }`.
    Character,
    /// A `*.prefab.yaml` part tree, placed by an instance entry (`prefab: …`). Carries the name of
    /// its root part, the one a clip's `Animation` sits on.
    Prefab(String),
}

impl ModelKind {
    /// The document's `kind:` value.
    fn document_kind(&self) -> &'static str {
        match self {
            ModelKind::Voxel => "voxel",
            ModelKind::Mesh => "mesh",
            ModelKind::Assembly => "assembly",
            ModelKind::Sculpt => "sculpt",
            ModelKind::Character => "character",
            ModelKind::Prefab(_) => "prefab",
        }
    }
}

/// The model's kind, model-space AABB, and file name (used both to frame the rig and to reference the
/// document from the synthesized scene).
struct ModelBounds {
    kind: ModelKind,
    min: [f32; 3],
    max: [f32; 3],
    file_name: String,
    /// The document's declared world scale — `scale:` for a voxel model (`1.0` when it declares
    /// none), and `1.0` for a mesh asset, whose vertices are already in world units. Only
    /// `--true-size` reads it; the ordinary sheet is scale-blind by design, one rig for every model.
    scale: f32,
    /// Whether the document references an emissive map. Only a `kind: mesh` document can, and every
    /// sheet reads it: the shader's emissive term is `base × Mesh.emissive × map`, so a standalone
    /// mesh's map needs an intensity the document has nowhere to author.
    emissive_texture: bool,
}

/// The parsed document behind a [`LoadedModel`], when its kind needs one for later work: a voxel
/// document for palette overrides, an assembly document for the part files a lineup must stage
/// alongside it, or a sculpt with its mesh for overrides and part framing. A mesh document needs
/// none — its vertex data is self-contained.
enum LoadedExtra {
    Voxel(VoxelDocument),
    Mesh,
    Assembly(AssemblyDocument),
    Sculpt(Box<LoadedSculpt>),
    Character(CharacterData),
    Prefab(PrefabClipData),
}

/// A sculpt document and what it meshes to — under the request's `--set` once
/// [`remesh_overridden_sculpt`] has run, while `doc` stays as authored.
struct LoadedSculpt {
    doc: SculptDocument,
    mesh: SculptMesh,
    /// The voxel documents its `shape: voxel` parts read, resolved once at load: an override
    /// re-meshes the same sources, and staging copies exactly the files they name.
    sources: SculptSources,
}

/// A loaded model document: its bounds, plus the parsed document when its kind needs one — palette
/// overrides need a voxel document, a lineup staging an assembly needs its part-file list, and both
/// have already been parsed by the time bounds exist.
struct LoadedModel {
    bounds: ModelBounds,
    extra: LoadedExtra,
}

/// Loads every requested model, collecting the diagnostics of all of them — a lineup with two broken
/// documents should report both, not stop at the first.
fn load_models(models: &[&Path]) -> Result<Vec<LoadedModel>, Vec<Diagnostic>> {
    let mut loaded = Vec::with_capacity(models.len());
    let mut diagnostics = Vec::new();
    for model in models {
        match load_model(model) {
            Ok(one) => loaded.push(one),
            Err(errors) => diagnostics.extend(errors),
        }
    }
    if diagnostics.is_empty() {
        Ok(loaded)
    } else {
        Err(diagnostics)
    }
}

/// Reads the document at `model`, dispatches on its `kind:`, validates it, and computes its bounds.
fn load_model(model: &Path) -> Result<LoadedModel, Vec<Diagnostic>> {
    let source =
        std::fs::read_to_string(model).map_err(|error| {
            vec![
            Diagnostic::new(
                code::MODEL_SHEET_INPUT,
                format!("could not read model document `{}`: {error}", model.display()),
            )
            .with_help(
                "pass a readable `.voxel.yaml`, `.mesh.yaml`, `.assembly.yaml`, `.sculpt.yaml`, \
                 `.character.yaml`, or `.prefab.yaml` model document (check the path and \
                 permissions)",
            ),
        ]
        })?;

    let file_name = model
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| {
            vec![
                Diagnostic::new(
                    code::MODEL_SHEET_INPUT,
                    format!(
                        "`{}` is not a model file (it has no file name)",
                        model.display()
                    ),
                )
                .with_help(
                    "pass a `.voxel.yaml`, `.mesh.yaml`, `.assembly.yaml`, `.sculpt.yaml`, \
                     `.character.yaml`, or `.prefab.yaml` model document",
                ),
            ]
        })?;

    match ludion_scene::peek_kind(&source).as_deref() {
        Some("voxel") => {
            let doc = ludion_scene::check_voxel(&source)?;
            let (min, max) = voxel_aabb(&doc);
            Ok(LoadedModel {
                bounds: ModelBounds {
                    kind: ModelKind::Voxel,
                    min,
                    max,
                    file_name,
                    scale: doc.scale.unwrap_or(1.0) as f32,
                    emissive_texture: false,
                },
                extra: LoadedExtra::Voxel(doc),
            })
        }
        Some("mesh") => {
            let doc = ludion_scene::check_mesh(&source)?;
            let (min, max) = mesh_aabb(&doc);
            Ok(LoadedModel {
                bounds: ModelBounds {
                    kind: ModelKind::Mesh,
                    min,
                    max,
                    file_name,
                    scale: 1.0,
                    emissive_texture: doc.textures.emissive.is_some(),
                },
                extra: LoadedExtra::Mesh,
            })
        }
        // Structure/data-consistency only (`L1137`/`L1138`, single-file, IO-free) — enough for
        // bounds. The cross-file pass (every part file, `L1101`/`L1139`/`L1140`/the unknown-glyph and
        // fit checks) happens where every other model kind's own content is resolved too: inside
        // `extract()`'s `Mesh.shape: { assembly: … }` walk, re-reading from the cell's project root
        // (`crate::extract::resolve_assembly`) — the same double-parse voxel/mesh already accept, so
        // a broken assembly's diagnostics surface with the same pointers `check-scene` reports, still
        // entirely before any GPU adapter is requested.
        Some("assembly") => {
            let doc = ludion_scene::check_assembly(&source)?;
            if doc.mesh.is_some() {
                // A skinned assembly: rooted at the file's own directory, as `check-scene` roots
                // it, so the mesh and clips the document names resolve relative to it. `extract`
                // loads the same assembly again from the cell's root, the harness's established
                // double-parse.
                let root = ludion_assets::AssetRoot::local_document_unconfined(
                    model.parent().unwrap_or_else(|| Path::new(".")),
                );
                let data = crate::character::load(root, &file_name)?;
                let mesh = crate::skin_bone::load_mesh_document(root, &data.mesh)?;
                let (min, max) = mesh_aabb(&mesh);
                return Ok(LoadedModel {
                    bounds: ModelBounds {
                        kind: ModelKind::Character,
                        min,
                        max,
                        file_name,
                        scale: 1.0,
                        emissive_texture: mesh.textures.emissive.is_some(),
                    },
                    extra: LoadedExtra::Character(data),
                });
            }
            let (min, max) = assembly_aabb(&doc);
            Ok(LoadedModel {
                bounds: ModelBounds {
                    kind: ModelKind::Assembly,
                    min,
                    max,
                    file_name,
                    // An assembly declares no `scale:` (`Plan/voxel-animation.md` decision 9) — only
                    // a voxel document carries the world-scale metadata `--true-size` reads.
                    scale: 1.0,
                    emissive_texture: false,
                },
                extra: LoadedExtra::Assembly(doc),
            })
        }
        // Meshed here, before the GPU, because a sculpt's bounds exist only as its mesh; `extract`
        // meshes it again from the cell's project root, the harness's established double-parse.
        Some("sculpt") => {
            let doc = ludion_scene::check_sculpt(&source)?;
            let sources = crate::sculpt_mesh::read_sculpt_sources(
                ludion_assets::AssetRoot::local_document_unconfined(
                    model.parent().unwrap_or_else(|| Path::new(".")),
                ),
                &file_name,
                &doc,
            )?;
            let mesh =
                ludion_sculpt::mesh_with(&doc, &sources).map_err(|diagnostic| vec![diagnostic])?;
            let (min, max) = sculpt_aabb(&mesh);
            Ok(LoadedModel {
                bounds: ModelBounds {
                    kind: ModelKind::Sculpt,
                    min,
                    max,
                    file_name,
                    // The mesher already works in world units.
                    scale: 1.0,
                    emissive_texture: false,
                },
                extra: LoadedExtra::Sculpt(Box::new(LoadedSculpt { doc, mesh, sources })),
            })
        }
        // The retired `kind: character`: `check_assembly` reports `L1246` with the one-line fix.
        Some("character") => Err(ludion_scene::check_assembly(&source).err().unwrap_or_default()),
        // Rooted at the file's own directory, like a character: the parts' meshes and the clips
        // resolve relative to it. Its bounds are the rest-pose box of one expanded instance.
        Some("prefab") => {
            let root = ludion_assets::AssetRoot::local_document_unconfined(
                model.parent().unwrap_or_else(|| Path::new(".")),
            );
            let (data, _) = crate::prefab_clips::load_with_sources(root, &file_name)?;
            let doc = ludion_scene::parse_prefab(&source)?;
            let root_part = doc
                .entities
                .first()
                .and_then(|entity| entity.name.clone())
                .ok_or_else(|| {
                    refuse_clip(
                        format!("prefab `{file_name}` has no named first part to pose"),
                        "a prefab's first entity is its root part: give it a `name:`",
                    )
                })?;
            let (min, max) = prefab_aabb(root, &file_name)?;
            Ok(LoadedModel {
                bounds: ModelBounds {
                    kind: ModelKind::Prefab(root_part),
                    min,
                    max,
                    file_name,
                    scale: 1.0,
                    emissive_texture: false,
                },
                extra: LoadedExtra::Prefab(data),
            })
        }
        Some(other) => Err(vec![
            Diagnostic::new(
                code::MODEL_SHEET_INPUT,
                format!(
                    "model document `{}` has kind `{other}`, but the model view harness renders \
                     only `voxel`, `mesh`, `assembly`, `sculpt`, `character`, or `prefab` documents",
                    model.display()
                ),
            )
            .with_help(
                "point `render --model` at a `.voxel.yaml`, `.mesh.yaml`, `.assembly.yaml`, \
                 `.sculpt.yaml`, `.character.yaml`, or `.prefab.yaml` document",
            ),
        ]),
        None => Err(vec![
            Diagnostic::new(
                code::MODEL_SHEET_INPUT,
                format!(
                    "model document `{}` has no `kind:` key; the model view harness needs a \
                     `voxel`, `mesh`, `assembly`, `sculpt`, `character`, or `prefab` document (a \
                     scene has no `kind:` and cannot be rendered as a model)",
                    model.display()
                ),
            )
            .with_help(
                "add `kind: voxel`, `kind: mesh`, `kind: assembly`, `kind: sculpt`, `kind: \
                 character`, or `kind: prefab`, or render a scene with `render --scene`",
            ),
        ]),
    }
}

/// The bottom-center, 1-voxel-per-unit AABB of an assembly's rest pose — [`voxel_aabb`]'s formula
/// applied to the assembled grid ([`AssemblyDocument::size`]), because the rest pose *is* that grid
/// (`Plan/voxel-animation.md` decision 9).
pub(crate) fn assembly_aabb(doc: &AssemblyDocument) -> ([f32; 3], [f32; 3]) {
    let [sx, sy, sz] = doc.size;
    let (hx, hz) = (sx as f32 * 0.5, sz as f32 * 0.5);
    ([-hx, 0.0, -hz], [hx, sy as f32, hz])
}

/// The rest-pose box of one instance of the prefab at `root`/`rel`: the prefab is expanded and
/// extracted on the CPU, and the box is taken over every mesh the extract drew. A prefab's parts
/// live in other documents, so no one document holds its bounds.
fn prefab_aabb(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<([f32; 3], [f32; 3]), Vec<Diagnostic>> {
    /// The corners of the unit primitives: cube, plane and sphere all span ±0.5.
    const UNIT_CORNERS: [[f32; 3]; 8] = [
        [-0.5, -0.5, -0.5],
        [0.5, -0.5, -0.5],
        [-0.5, 0.5, -0.5],
        [0.5, 0.5, -0.5],
        [-0.5, -0.5, 0.5],
        [0.5, -0.5, 0.5],
        [-0.5, 0.5, 0.5],
        [0.5, 0.5, 0.5],
    ];
    let registry = builtin_registry();
    let probe = emit_entities(vec![ludion_scene::SceneEntity {
        name: Some("subject".to_string()),
        instance: Some(Box::new(ludion_scene::InstanceEntry {
            prefab: rel.to_string(),
            ..Default::default()
        })),
        ..Default::default()
    }]);
    let scene = ludion_scene::check(&probe, &registry)?;
    let (_, world) = crate::render::expanded_world(root.dir(), &registry, &scene)?;
    let rendered = extract_at(
        &world,
        (1, 1),
        root,
        SimTime::from_ticks(0, TickRate::DEFAULT),
        QualityParams::FULL.particle_density,
    )?;
    let mut points: Vec<[f32; 3]> = Vec::new();
    for instance in &rendered.meshes {
        let to_world = |p: Vec3| instance.model.transform_point3(p).to_array();
        match instance.shape {
            ludion_renderer::MeshShape::Asset(index) => {
                if let Some(asset) = rendered.mesh_assets.get(index) {
                    points.extend(
                        asset
                            .positions
                            .chunks_exact(3)
                            .map(|v| to_world(Vec3::new(v[0], v[1], v[2]))),
                    );
                }
            }
            _ => points.extend(
                UNIT_CORNERS
                    .iter()
                    .map(|corner| to_world(Vec3::from_array(*corner))),
            ),
        }
    }
    if points.is_empty() {
        return Err(refuse_clip(
            format!("prefab `{rel}` draws no mesh, so there is nothing to frame"),
            "give a part a `Mesh` component (`ludion docs prefabs`)",
        ));
    }
    Ok(points_aabb(points.into_iter()))
}

// --- Posed sheets, strips, and the per-phase gap analysis (decision 11) ------------------------

/// The `Animation` one model entity of a posed sheet carries.
#[derive(Clone, Copy, Debug)]
struct AnimationSpec<'a> {
    clip: &'a str,
    started: u64,
    fade: Option<FadeSpec<'a>>,
    /// The blend parameters `clip` is evaluated at when it names a blend.
    params: [f32; 2],
    /// The `AnimationLayers` the entity carries, all started at `started`.
    layers: &'a [LayerSpec<'a>],
    /// The `--look`/`--ik` the entity's `IkTargets` aim, when there are any.
    ik: Option<&'a IkPlan>,
    /// The `[yaw, pitch]` this phase's look-at aims at.
    look: Option<[f32; 2]>,
}

/// Whether the request sweeps a blend, so its phases are a strip.
fn sweeps_blend(request: &ModelSheetRequest) -> bool {
    request.blend.is_some_and(|blend| {
        request.strip.is_some()
            || matches!(blend.p0, Sweep::Range(..))
            || matches!(blend.p1, Some(Sweep::Range(..)))
    })
}

/// The crossfade fields of an [`AnimationSpec`]: what `crate::components::crossfade` would have
/// written at `fade_started`, with `from_speed` left at its default.
#[derive(Clone, Copy, Debug)]
struct FadeSpec<'a> {
    from: &'a str,
    from_started: u64,
    fade_started: u64,
    seconds: f32,
}

/// One phase entity's frames: `started`, and under a crossfade its `(from_started, fade_started)`.
#[derive(Debug)]
struct PhaseStart {
    started: u64,
    fade: Option<(u64, u64)>,
    /// The blend parameters under a blend sweep; `[0, 0]` elsewhere.
    params: [f32; 2],
    /// The look-at aim of this phase under `--look`.
    look: Option<[f32; 2]>,
}

/// A resolved clip request: everything the render and the report need, decided entirely on the CPU.
#[derive(Debug)]
struct Posed {
    /// The clip played — a crossfade's incoming one.
    name: String,
    duration: f64,
    looped: bool,
    /// A crossfade's outgoing clip, and its length in seconds (`0.0` outside one).
    from: Option<String>,
    fade_seconds: f32,
    /// The swept blend's name, when the base is one (then `name` is the same).
    blend: Option<String>,
    /// Whether the swept blend is 2D, which sets how a phase's parameters are captioned.
    two_d: bool,
    /// The frames each phase's own entity carries, in render order. All of them are read
    /// at the single [`now`](Self::now) tick, which is what lets one extract produce N poses.
    started: Vec<PhaseStart>,
    /// The sim tick every entity extracts at: the latest phase's, so no `started` goes negative.
    now: u64,
    /// One report per phase, in the same order.
    reports: Vec<PhaseReport>,
    /// What `--look`/`--ik` resolved to; empty without them.
    ik: IkPlan,
}

/// The largest posed occupancy grid the gap analysis will build (256³ cells). A pose is a rigid
/// transform of a bounded grid, so reaching this needs a translation key measured in thousands of
/// voxels — a broken clip, not a big model, and saying so beats allocating for it.
const MAX_POSED_GRID_CELLS: u64 = 256 * 256 * 256;

/// The glyph the posed occupancy grid fills a cell with. Its identity is irrelevant — only
/// occupancy is analysed — so one palette entry serves the whole grid.
const POSED_GLYPH: char = '#';

/// The posed occupancy grid's empty cell, the voxel format's own.
const POSED_EMPTY: char = '.';

/// The kind word and bound clip names of a model that can be posed, or `L1117` for one that cannot.
/// `what` is the flag as the caller spelled it, for the refusal.
fn clip_names<'a>(
    loaded: &'a LoadedModel,
    what: &str,
) -> Result<(&'static str, Vec<&'a str>), Vec<Diagnostic>> {
    match &loaded.extra {
        LoadedExtra::Assembly(document) => Ok((
            "assembly",
            document
                .clips
                .iter()
                .map(|binding| binding.name.as_str())
                .collect(),
        )),
        LoadedExtra::Character(data) => Ok((
            "character",
            data.clips.iter().map(|clip| clip.name.as_str()).collect(),
        )),
        LoadedExtra::Prefab(data) => Ok((
            "prefab",
            data.clips.iter().map(|clip| clip.name.as_str()).collect(),
        )),
        _ => Err(refuse_clip(
            format!(
                "{what} cannot pose this model: only an assembly, a character or a prefab has \
                 parts or bones to pose and clips to name them from"
            ),
            "a `.voxel.yaml`/`.mesh.yaml`/`.sculpt.yaml` model is one rigid piece — render it \
             without `--clip`/`--crossfade`, or point `--model` at the assembly, character or \
             prefab that animates it",
        )),
    }
}

/// `L1117` unless the model binds a clip under `name`.
fn check_bound(noun: &str, names: &[&str], name: &str) -> Result<(), Vec<Diagnostic>> {
    if names.contains(&name) {
        return Ok(());
    }
    let bound = if names.is_empty() {
        "it binds none".to_string()
    } else {
        format!(
            "it binds {}",
            names
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Err(refuse_clip(
        format!("this {noun} binds no clip named `{name}`: {bound}"),
        &format!(
            "name one of the clips the {noun}'s `clips:` map declares, or add the binding to the \
             {noun} file (`check-scene <model>.{noun}.yaml` lists them)"
        ),
    ))
}

/// What a posed render reads its clips and its per-phase report from.
enum ClipSource<'a> {
    Assembly(Box<AssemblyPosing>),
    /// A character: clips only, with no voxel grid to analyse.
    Character(&'a CharacterData),
    /// A prefab: clips only — its parts are meshes, with no voxel grid to analyse.
    Prefab(&'a PrefabClipData),
}

/// What an assembly's posed render reads: its mesh data for the pose, and the assembled grid the
/// gap analysis walks.
struct AssemblyPosing {
    data: AssemblyData,
    assembly: AssemblyDocument,
    grid: AssembledGrid,
    rest: Vec<(Mat4, Mat3)>,
}

impl<'a> ClipSource<'a> {
    /// Loads what `loaded` poses from. An assembly is a third parse of the same files
    /// ([`load_model`] already read the document, and `extract` will read it again from the cell's
    /// project root) — the harness's established double-parse trade, extended by one: what is bought
    /// is that the *analysed* pose and the *rendered* pose come out of one evaluator.
    fn load(model: &Path, loaded: &'a LoadedModel) -> Result<Self, Vec<Diagnostic>> {
        match &loaded.extra {
            LoadedExtra::Character(data) => Ok(ClipSource::Character(data)),
            LoadedExtra::Prefab(data) => Ok(ClipSource::Prefab(data)),
            _ => {
                let root = ludion_assets::AssetRoot::local_document_unconfined(
                    model.parent().unwrap_or_else(|| Path::new(".")),
                );
                let rel = loaded.bounds.file_name.as_str();
                let (data, assembly, parts) =
                    crate::assembly_mesh::load_meshed_documents(root, rel)?;
                let grid = ludion_scene::assembly_assembled_grid(&assembly, &parts);
                let rest = part_placements(&data, None);
                Ok(ClipSource::Assembly(Box::new(AssemblyPosing {
                    data,
                    assembly,
                    grid,
                    rest,
                })))
            }
        }
    }

    /// The clip bound as `name` and the factor its clip time runs at relative to the entity's own
    /// elapsed seconds — a character binding's overlay speed, `1.0` for an assembly.
    fn bound(&self, name: &str) -> (&AnimationDocument, f64) {
        let bound = match self {
            ClipSource::Assembly(posing) => posing.data.clip(name).map(|clip| (clip, 1.0)),
            ClipSource::Character(data) => data.clip(name).map(|clip| (&clip.clip, clip.speed)),
            ClipSource::Prefab(data) => data
                .clips
                .iter()
                .find(|clip| clip.name == name)
                .map(|clip| (&clip.clip, 1.0)),
        };
        bound.expect("the name was checked against the same document `load` resolved")
    }

    /// One phase's report at clip time `time` (the incoming clip's), `tick` ticks in, for the pose
    /// the render will draw.
    ///
    /// An assembly poses through [`part_placements_sides`], the one implementation extract draws
    /// with: `layers` compose over the base `pose` (and its `fade`) exactly as the entity's
    /// `AnimationLayers` do, and `ik` solves over the result as extract's `IkTargets` pass does.
    fn report(
        &self,
        pose: AssemblySide<'_>,
        fade: Option<(AssemblySide<'_>, f64)>,
        layers: &[AssemblyLayerPose<'_>],
        ik: &[IkGoal],
        time: f64,
        tick: u64,
    ) -> Result<PhaseReport, Vec<Diagnostic>> {
        match self {
            ClipSource::Assembly(posing) => {
                let mut posed = part_placements_sides(&posing.data, pose, fade, true, layers);
                if !ik.is_empty() {
                    apply_assembly_ik(&posing.data, ik, &mut posed);
                }
                phase_report(
                    &posing.assembly,
                    &posing.grid,
                    &posing.rest,
                    &posed,
                    time,
                    tick,
                )
            }
            ClipSource::Character(_) | ClipSource::Prefab(_) => Ok(PhaseReport {
                time,
                tick,
                components: None,
                strays: Vec::new(),
                weight: None,
                params: None,
                look: None,
                phase: None,
            }),
        }
    }

    /// The blend space bound as `name`, or `None` (a prefab has none).
    fn blend(&self, name: &str) -> Option<&crate::blend_space::BlendSpaceData> {
        match self {
            ClipSource::Assembly(posing) => posing.data.blend(name),
            ClipSource::Character(data) => data.blend(name),
            ClipSource::Prefab(_) => None,
        }
    }

    /// Every blend name the model declares, for a refusal's suggestions.
    fn blend_names(&self) -> Vec<&str> {
        match self {
            ClipSource::Assembly(posing) => posing
                .data
                .blends
                .iter()
                .map(|blend| blend.name.as_str())
                .collect(),
            ClipSource::Character(data) => data
                .blends
                .iter()
                .map(|blend| blend.name.as_str())
                .collect(),
            ClipSource::Prefab(_) => Vec::new(),
        }
    }

    /// The layers of an assembly posed at `now`, each as the entity's `AnimationLayers` entry would
    /// read — the `started` frame is the phase's own, so a layer's clock is the base clock. Empty for
    /// a character or prefab, which have no gap analysis to pose.
    fn layer_poses<'s>(
        &'s self,
        specs: &[LayerSpec<'_>],
        started: u64,
        now: ludion_core::time::GameTime,
    ) -> Vec<AssemblyLayerPose<'s>> {
        let ClipSource::Assembly(posing) = self else {
            return Vec::new();
        };
        layers_of(specs, started)
            .iter()
            .filter_map(|layer| assembly_layer_pose(&posing.data, layer, now, None))
            .collect()
    }
}

/// The `AnimationLayer`s `specs` stand for, all started at frame `started`. A layer is named for its
/// clip, a repeat of a clip getting its position too, since a layer's name is unique in the stack.
fn layers_of(specs: &[LayerSpec<'_>], started: u64) -> Vec<AnimationLayer> {
    specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let repeats = specs[..index].iter().any(|prior| prior.clip == spec.clip);
            AnimationLayer {
                name: if repeats {
                    format!("{}-{index}", spec.clip)
                } else {
                    spec.clip.to_string()
                },
                clip: spec.clip.to_string(),
                mask: spec.mask.unwrap_or_default().to_string(),
                mode: spec.mode,
                weight: spec.weight,
                started: FrameIndex::new(started),
                ..AnimationLayer::default()
            }
        })
        .collect()
}

/// What `--look` and `--ik` resolved to against one assembly: the entries they drive and the points
/// the chains reach, both in the assembly frame the solver works in.
#[derive(Clone, Debug, Default)]
struct IkPlan {
    /// Each `--ik`: its entry index in [`AssemblyData::ik`], its name, and the target in the assembly
    /// frame (bottom-centre origin, 1 voxel = 1 unit).
    chains: Vec<(usize, String, [f32; 3])>,
    /// The look-at `--look` aims: its entry index and name.
    look: Option<(usize, String)>,
}

impl IkPlan {
    fn is_empty(&self) -> bool {
        self.chains.is_empty() && self.look.is_none()
    }

    /// The solver goals for one phase aiming the look-at at `look` (`[yaw, pitch]` in degrees).
    fn goals(&self, look: Option<[f32; 2]>) -> Vec<IkGoal> {
        let chains = self.chains.iter().map(|(entry, _, point)| IkGoal {
            entry: *entry,
            aim: IkAim::Point(point.map(f64::from)),
            weight: 1.0,
        });
        let look = self
            .look
            .iter()
            .zip(look)
            .map(|((entry, _), [yaw, pitch])| IkGoal {
                entry: *entry,
                aim: IkAim::Direction {
                    yaw_deg: f64::from(yaw),
                    pitch_deg: f64::from(pitch),
                },
                weight: 1.0,
            });
        chains.chain(look).collect()
    }

    /// The `IkTargets` a sheet entity placed by `placement` carries so extract solves the same
    /// goals: chain points move from the assembly frame to the world through the entity's
    /// translation and uniform scale, a look is the direction itself.
    fn targets(&self, look: Option<[f32; 2]>, placement: &Placement) -> IkTargets {
        let mut targets = IkTargets::default();
        for (_, name, point) in &self.chains {
            let world = std::array::from_fn(|axis| {
                placement.translation[axis] + placement.scale * point[axis]
            });
            targets.targets.insert(
                name.clone(),
                IkTarget {
                    point: Some(world),
                    weight: 1.0,
                    ..IkTarget::default()
                },
            );
        }
        if let (Some((_, name)), Some([yaw_deg, pitch_deg])) = (&self.look, look) {
            targets.targets.insert(
                name.clone(),
                IkTarget {
                    direction: Some(Look { yaw_deg, pitch_deg }),
                    weight: 1.0,
                    ..IkTarget::default()
                },
            );
        }
        targets
    }
}

/// Whether a `--look` side is a range, so its phases are a strip of looks.
fn sweeps_look(request: &ModelSheetRequest) -> bool {
    request.look.is_some_and(|look| {
        matches!(look.yaw, Sweep::Range(..)) || matches!(look.pitch, Sweep::Range(..))
    })
}

/// The `[yaw, pitch]` phase `index` of `phases` aims at, or `None` without `--look`.
fn look_at_phase(request: &ModelSheetRequest, index: u32, phases: u32) -> Option<[f32; 2]> {
    request
        .look
        .map(|look| [look.yaw.at(index, phases), look.pitch.at(index, phases)])
}

/// `--ik` as `name=[x, y, z]` echoes, for the report.
fn ik_echoes(request: &ModelSheetRequest) -> Vec<String> {
    request
        .ik
        .iter()
        .map(|spec| format!("{}={:?}", spec.name, spec.point))
        .collect()
}

/// Resolves `--look` and `--ik` against the model, or `L1117` for one that cannot take them. An empty
/// plan when neither flag was given.
fn resolve_ik(
    source: &ClipSource<'_>,
    noun: &str,
    request: &ModelSheetRequest,
) -> Result<IkPlan, Vec<Diagnostic>> {
    if request.look.is_none() && request.ik.is_empty() {
        return Ok(IkPlan::default());
    }
    let what = if request.look.is_some() {
        "`--look`"
    } else {
        "`--ik`"
    };
    let ClipSource::Assembly(posing) = source else {
        return Err(refuse_clip(
            format!(
                "{what} cannot aim this {noun}: only an assembly declares `ik:` chains and \
                 look-ats, and a {noun}'s parts or bones carry none"
            ),
            "point `--model` at the `.assembly.yaml` whose `ik:` names the chain or look-at",
        ));
    };
    let entries = &posing.data.ik;
    let chains: Vec<&str> = entries
        .iter()
        .filter(|entry| matches!(entry.kind, AssemblyIkKind::Chain { .. }))
        .map(|entry| entry.name.as_str())
        .collect();
    let looks: Vec<&str> = entries
        .iter()
        .filter(|entry| matches!(entry.kind, AssemblyIkKind::LookAt { .. }))
        .map(|entry| entry.name.as_str())
        .collect();
    let all: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    let find = |flag: &str, name: &str, wants_chain: bool| -> Result<usize, Vec<Diagnostic>> {
        let (kind, found, mine, instead) = if wants_chain {
            ("chain", "look-at", &chains, "--look")
        } else {
            ("look-at", "chain", &looks, "--ik")
        };
        let Some(index) = entries.iter().position(|entry| entry.name == name) else {
            let help = match ludion_core::suggest::nearest(name, all.iter().copied()) {
                Some(nearest) => format!(
                    "nearest match is `{nearest}`; {} (`check-scene <model>.assembly.yaml` lists them)",
                    declared(&all)
                ),
                None => format!(
                    "name a {kind} of the `ik:` map (`check-scene <model>.assembly.yaml` lists them)"
                ),
            };
            return Err(refuse_clip(
                format!(
                    "`{flag} {name}` names no `ik:` entry of this assembly: {}",
                    declared(&all)
                ),
                &help,
            ));
        };
        if !mine.contains(&name) {
            return Err(refuse_clip(
                format!("`{flag} {name}` names a {found}, but {flag} drives a {kind}"),
                &format!("use `{instead}` for that entry, or name {}", declared(mine)),
            ));
        }
        Ok(index)
    };

    let mut plan = IkPlan::default();
    let [sx, _, sz] = posing.grid.voxel.size;
    for spec in request.ik {
        let entry = find("--ik", spec.name, true)?;
        // The `place:` frame is the grid's own voxels; the assembly frame sits at the bottom-centre.
        let [x, y, z] = spec.point;
        plan.chains.push((
            entry,
            spec.name.to_string(),
            [x - sx as f32 * 0.5, y, z - sz as f32 * 0.5],
        ));
    }
    if let Some(look) = request.look {
        let entry = match look.name {
            Some(name) => find("--look", name, false)?,
            None => match looks[..] {
                [only] => entries
                    .iter()
                    .position(|entry| entry.name == only)
                    .expect("the name came from the same entries"),
                _ => {
                    return Err(refuse_clip(
                        format!(
                            "a bare `--look` needs exactly one look-at, and this assembly declares {}",
                            if looks.is_empty() {
                                "none".to_string()
                            } else {
                                looks.len().to_string()
                            }
                        ),
                        &if looks.is_empty() {
                            "add an `aim:` entry under `ik:` (`ludion docs voxel-animation`)"
                                .to_string()
                        } else {
                            format!(
                                "name one, e.g. `--look {}=0,0` — {}",
                                looks[0],
                                declared(&looks)
                            )
                        },
                    ));
                }
            },
        };
        plan.look = Some((entry, entries[entry].name.clone()));
    }
    Ok(plan)
}

/// Resolves `name` against the model: checks it is an assembly, character or prefab that binds the name,
/// loads what the pose needs, and computes every phase's time, `started` frame, and report.
///
/// A character's clip time is `elapsed × binding.speed`, so the tick that reaches clip time `t` is
/// `t / binding.speed` and the phase reports the clip time actually sampled.
fn resolve_clip(
    request: &ModelSheetRequest,
    model: &Path,
    loaded: &LoadedModel,
    name: &str,
) -> Result<Posed, Vec<Diagnostic>> {
    let (noun, names) = clip_names(loaded, &format!("`--clip {name}`"))?;
    check_bound(noun, &names, name)?;
    let source = ClipSource::load(model, loaded)?;
    check_layers(&source, noun, request.layers)?;
    let ik = resolve_ik(&source, noun, request)?;
    let (clip, factor) = source.bound(name);

    let rate = TickRate::DEFAULT;
    let hold = request.hold.unwrap_or(1);
    // A look sweep spends the strip on looks, so every phase sits at the one `--time`.
    let times = if sweeps_look(request) {
        let phases = request.strip.unwrap_or(DEFAULT_CROSSFADE_PHASES) as usize;
        vec![request.time.unwrap_or(0.0); phases]
    } else {
        phase_times(clip.duration, clip.looped, request.strip, request.time)
    };
    let ticks: Vec<u64> = times
        .into_iter()
        .map(|time| quantize(time / factor, rate))
        .map(|tick| crate::extract::held_ticks(hold, tick))
        .collect();
    let now = ticks.iter().copied().max().unwrap_or(0);
    let mut reports = Vec::with_capacity(ticks.len());
    let mut looks = Vec::with_capacity(ticks.len());
    for (index, &tick) in ticks.iter().enumerate() {
        // The instant the render will reach, derived the way the render derives it: the entity's
        // elapsed seconds are `(now − started) × seconds_per_tick`, and `clip_phase` wraps or
        // clamps them per the clip's own `loop:`.
        let time = tick as f64 * rate.seconds_per_tick() * factor;
        let pose = AssemblySide::single(Some((clip, ludion_scene::clip_phase(clip, time))));
        let layers = source.layer_poses(request.layers, now - tick, game_now(now));
        let look = look_at_phase(request, index as u32, ticks.len() as u32);
        let mut report = source.report(pose, None, &layers, &ik.goals(look), time, tick)?;
        report.look = look;
        reports.push(report);
        looks.push(look);
    }

    Ok(Posed {
        name: name.to_string(),
        duration: clip.duration,
        looped: clip.looped,
        from: None,
        fade_seconds: 0.0,
        blend: None,
        two_d: false,
        started: ticks
            .iter()
            .zip(looks)
            .map(|(tick, look)| PhaseStart {
                started: now - tick,
                fade: None,
                params: [0.0; 2],
                look,
            })
            .collect(),
        now,
        reports,
        ik,
    })
}

/// [`resolve_clip`] for a crossfade: each phase's entity carries the fade fields
/// `crate::components::crossfade` would have written at its `fade_started`, so the one extract at
/// the shared `now` draws the blend at that phase's weight.
///
/// Phase `i` is `k_i` ticks into the fade (`k_i` spaced from zero to the fade's length, both
/// endpoints shown); `--time` is how far the outgoing clip has played when the fade begins, `k0`
/// ticks. The weight is the extractor's own: `k_i × seconds_per_tick / fade`, and at `1` or more the
/// plain incoming pose is drawn.
fn resolve_crossfade(
    request: &ModelSheetRequest,
    model: &Path,
    loaded: &LoadedModel,
    spec: CrossfadeSpec<'_>,
) -> Result<Posed, Vec<Diagnostic>> {
    let what = format!("`--crossfade {}:{}:{}`", spec.from, spec.to, spec.seconds);
    let (noun, names) = clip_names(loaded, &what)?;
    check_bound(noun, &names, spec.from)?;
    check_bound(noun, &names, spec.to)?;
    let source = ClipSource::load(model, loaded)?;
    check_layers(&source, noun, request.layers)?;
    let ik = resolve_ik(&source, noun, request)?;
    let (from_clip, from_factor) = source.bound(spec.from);
    let (to_clip, to_factor) = source.bound(spec.to);

    let rate = TickRate::DEFAULT;
    let seconds = f64::from(spec.seconds);
    let phases = request.strip.unwrap_or(DEFAULT_CROSSFADE_PHASES);
    let hold = request.hold.unwrap_or(1);
    // A look sweep spends the strip on looks, so every phase sits at the fade's start.
    let times = if sweeps_look(request) {
        vec![0.0; phases as usize]
    } else {
        phase_times(seconds, false, Some(phases), None)
    };
    let fade_ticks: Vec<u64> = times
        .into_iter()
        .map(|time| quantize(time, rate))
        .map(|tick| crate::extract::held_ticks(hold, tick))
        .collect();
    let lead = quantize(request.time.unwrap_or(0.0) / from_factor, rate);
    let now = fade_ticks.iter().copied().max().unwrap_or(0) + lead;

    let mut started = Vec::with_capacity(fade_ticks.len());
    let mut reports = Vec::with_capacity(fade_ticks.len());
    for (index, &tick) in fade_ticks.iter().enumerate() {
        let to_time = tick as f64 * rate.seconds_per_tick() * to_factor;
        let from_time = (tick + lead) as f64 * rate.seconds_per_tick() * from_factor;
        // The extractor's own arithmetic, so the report names the weight the render drew.
        let raw = tick as f64 * rate.seconds_per_tick() / seconds;
        let to_pose = Some((to_clip, ludion_scene::clip_phase(to_clip, to_time)));
        let fade = (raw < 1.0).then(|| CrossfadePose {
            from: Some((from_clip, ludion_scene::clip_phase(from_clip, from_time))),
            weight: raw,
        });
        let layers = source.layer_poses(request.layers, now - tick, game_now(now));
        let look = look_at_phase(request, index as u32, phases);
        let mut report = source.report(
            AssemblySide::single(to_pose),
            fade.map(|fade| (AssemblySide::single(fade.from), fade.weight)),
            &layers,
            &ik.goals(look),
            to_time,
            tick,
        )?;
        report.weight = Some(raw.min(1.0));
        report.look = look;
        reports.push(report);
        started.push(PhaseStart {
            started: now - tick,
            fade: Some((now - tick - lead, now - tick)),
            params: [0.0; 2],
            look,
        });
    }

    Ok(Posed {
        name: spec.to.to_string(),
        duration: to_clip.duration,
        looped: to_clip.looped,
        from: Some(spec.from.to_string()),
        fade_seconds: spec.seconds,
        blend: None,
        two_d: false,
        started,
        now,
        reports,
        ik,
    })
}

/// The gameplay instant `tick` ticks in, which is what extract poses at when no `Animation.hold` is set.
fn game_now(tick: u64) -> ludion_core::time::GameTime {
    ludion_core::time::GameTime::from(SimTime::from_ticks(tick, TickRate::DEFAULT))
}

/// `L1117` for a prefab asked to blend or layer: its parts carry neither.
fn refuse_prefab(what: &str) -> Vec<Diagnostic> {
    refuse_clip(
        format!(
            "{what} cannot pose a prefab: a prefab's parts carry `Animation` clips only, with no \
             `blends:` and no `AnimationLayers`"
        ),
        "point `--model` at the assembly or character that declares the blends and masks, or play a \
         prefab clip with `--clip`",
    )
}

/// "it declares `a`, `b`" — or "it declares none" — for a refusal that lists what a model has.
fn declared(names: &[&str]) -> String {
    if names.is_empty() {
        return "it declares none".to_string();
    }
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    format!("it declares {}", quoted.join(", "))
}

/// `L1117` unless every layer names a clip or blend the model binds and a mask it declares.
fn check_layers(
    source: &ClipSource<'_>,
    noun: &str,
    specs: &[LayerSpec<'_>],
) -> Result<(), Vec<Diagnostic>> {
    if specs.is_empty() {
        return Ok(());
    }
    if matches!(source, ClipSource::Prefab(_)) {
        return Err(refuse_prefab("`--layer`"));
    }
    let list = |names: Vec<&str>| declared(&names);
    let (clips, masks): (Vec<&str>, Vec<&str>) = match source {
        ClipSource::Assembly(posing) => (
            posing.data.clips.iter().map(|c| c.name.as_str()).collect(),
            posing.data.masks.iter().map(|m| m.name.as_str()).collect(),
        ),
        ClipSource::Character(data) => (
            data.clips.iter().map(|c| c.name.as_str()).collect(),
            data.masks.iter().map(|m| m.name.as_str()).collect(),
        ),
        ClipSource::Prefab(_) => (Vec::new(), Vec::new()),
    };
    let blends = source.blend_names();
    for spec in specs {
        if !clips.contains(&spec.clip) && !blends.contains(&spec.clip) {
            let known: Vec<&str> = clips.iter().chain(&blends).copied().collect();
            return Err(refuse_clip(
                format!(
                    "`--layer {}:...` names no clip or blend of this {noun}: {}",
                    spec.clip,
                    list(known)
                ),
                &format!(
                    "name a clip or blend the {noun} declares (`check-scene <model>.{noun}.yaml` lists them)"
                ),
            ));
        }
        if let Some(mask) = spec.mask
            && !masks.contains(&mask)
        {
            return Err(refuse_clip(
                format!(
                    "`--layer {}:..:{mask}` names a mask this {noun} does not declare: {}",
                    spec.clip,
                    list(masks)
                ),
                &format!(
                    "name a `masks:` entry of the {noun}, or leave the mask empty for every part \
                     (`--layer {}:{}`)",
                    spec.clip, spec.weight
                ),
            ));
        }
    }
    Ok(())
}

/// [`resolve_clip`] for a blend sweep: every phase plays the blend on the one shared tick at its own
/// parameters, and the report poses through [`AssemblySide::blend`] at the stateless phase
/// `elapsed × rate(params)` — what extract reads for an entity with no `BlendClock`, so the strip
/// and a running game agree.
fn resolve_blend(
    request: &ModelSheetRequest,
    model: &Path,
    loaded: &LoadedModel,
    spec: BlendSweepSpec<'_>,
) -> Result<Posed, Vec<Diagnostic>> {
    let what = format!("`--blend {}`", spec.name);
    let (noun, _) = clip_names(loaded, &what)?;
    let source = ClipSource::load(model, loaded)?;
    if matches!(source, ClipSource::Prefab(_)) {
        return Err(refuse_prefab(&what));
    }
    check_layers(&source, noun, request.layers)?;
    let ik = resolve_ik(&source, noun, request)?;
    let Some(blend) = source.blend(spec.name) else {
        return Err(refuse_clip(
            format!(
                "this {noun} declares no blend named `{}`: {}",
                spec.name,
                declared(&source.blend_names())
            ),
            &format!(
                "name one of the `blends:` the {noun} declares (`check-scene <model>.{noun}.yaml` \
                 lists them)"
            ),
        ));
    };
    if blend.two_d != spec.p1.is_some() {
        let (shape, fix) = if blend.two_d {
            ("2D", "give both parameters, e.g. `--blend NAME:0..1:-1..1`")
        } else {
            ("1D", "give one parameter, e.g. `--blend NAME:0..1`")
        };
        return Err(refuse_clip(
            format!(
                "`--blend {}` is {shape} and takes {} parameter{}",
                spec.name,
                if blend.two_d { "two" } else { "one" },
                if blend.two_d { "s" } else { "" }
            ),
            fix,
        ));
    }

    let rate = TickRate::DEFAULT;
    let ranged = matches!(spec.p0, Sweep::Range(..)) || matches!(spec.p1, Some(Sweep::Range(..)));
    let phases = request.strip.unwrap_or(if ranged || sweeps_look(request) {
        DEFAULT_CROSSFADE_PHASES
    } else {
        1
    });
    let hold = request.hold.unwrap_or(1);
    let tick = crate::extract::held_ticks(hold, quantize(request.time.unwrap_or(0.0), rate));
    let elapsed = tick as f64 * rate.seconds_per_tick();
    let layers = source.layer_poses(request.layers, 0, game_now(tick));

    let mut started = Vec::with_capacity(phases as usize);
    let mut reports = Vec::with_capacity(phases as usize);
    let mut cycle = 0.0;
    for index in 0..phases {
        let params = [
            spec.p0.at(index, phases),
            spec.p1.map_or(0.0, |sweep| sweep.at(index, phases)),
        ];
        let blend_rate = crate::blend_clock::blend_rate(blend, params);
        if index == 0 && blend_rate > 0.0 {
            cycle = 1.0 / blend_rate;
        }
        let phase = elapsed * blend_rate;
        let side = match &source {
            ClipSource::Assembly(posing) => AssemblySide::blend(&posing.data, blend, params, phase),
            _ => AssemblySide::EMPTY,
        };
        let look = look_at_phase(request, index, phases);
        let mut report = source.report(side, None, &layers, &ik.goals(look), elapsed, tick)?;
        report.params = Some(params);
        report.phase = Some(phase);
        report.look = look;
        reports.push(report);
        started.push(PhaseStart {
            started: 0,
            fade: None,
            params,
            look,
        });
    }

    Ok(Posed {
        name: spec.name.to_string(),
        duration: cycle,
        looped: true,
        from: None,
        fade_seconds: 0.0,
        blend: Some(spec.name.to_string()),
        two_d: blend.two_d,
        started,
        now: tick,
        reports,
        ik,
    })
}

/// The clip instants a request renders, in render order.
///
/// Without a strip that is the one requested `time` (`0.0` by default). With one it is `n` evenly
/// spaced phases, spaced by the clip's own `loop:` flag: a looping clip divides by `n` (its last
/// phase is not its first repeated, because on a loop it never is), a one-shot by `n − 1` (both
/// endpoints shown, because a one-shot's final pose is what it exists to reach).
pub(crate) fn phase_times(
    duration: f64,
    looped: bool,
    strip: Option<u32>,
    time: Option<f64>,
) -> Vec<f64> {
    let Some(phases) = strip else {
        return vec![time.unwrap_or(0.0)];
    };
    // `check_clip_flags` has already refused anything below `MIN_STRIP_PHASES`, so `n − 1` is at
    // least one; the `max` is a guard against a caller that skipped the check, never a case.
    let divisor = if looped {
        f64::from(phases)
    } else {
        f64::from(phases - 1).max(1.0)
    };
    (0..phases)
        .map(|index| f64::from(index) * duration / divisor)
        .collect()
}

/// The tick nearest `seconds` on `rate`'s clock — the whole of the harness's time quantization.
/// Negative and non-finite input reads as tick 0, matching [`ludion_scene::clip_phase`]'s own
/// handling rather than producing a saturating cast nobody can explain.
pub(crate) fn quantize(seconds: f64, rate: TickRate) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (seconds * f64::from(rate.hz())).round() as u64
}

/// The inclusive cell box one rest cell conservatively covers once `delta` has moved it.
///
/// `low` is the cell's **low corner** in the assembly frame, so the cell is the unit cube
/// `low ..= low + 1`. All eight of its corners go through `delta` and the axis-aligned bounding box
/// of the results is taken; `shift` turns a world coordinate back into a cell coordinate per axis
/// (see the caller). The box is `floor(min)` to `ceil(max) − 1`, i.e. every cell the transformed
/// cube's AABB touches at all.
///
/// **Conservative by construction, and that is the point.** A rotated unit cube's AABB is at least
/// one unit on every axis and at most √3, so a cell marks a 1×1×1 box when the pose is axis-aligned
/// and up to a 2×2×2 (3×3×3 in the worst diagonal) box when it is not. The posed grid therefore
/// never *under*-connects: a limb that rotates keeps its cells face-adjacent instead of
/// staircasing apart, which a point sample of the cell's center does not.
fn covered_box(delta: Mat4, low: Vec3, shift: [f32; 3]) -> ([i64; 3], [i64; 3]) {
    let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for dx in [0.0, 1.0] {
        for dy in [0.0, 1.0] {
            for dz in [0.0, 1.0] {
                let corner = delta.transform_point3(low + Vec3::new(dx, dy, dz));
                for (axis, value) in [corner.x, corner.y, corner.z].into_iter().enumerate() {
                    min[axis] = min[axis].min(value);
                    max[axis] = max[axis].max(value);
                }
            }
        }
    }
    let mut lo = [0i64; 3];
    let mut hi = [0i64; 3];
    for axis in 0..3 {
        lo[axis] = (min[axis] + shift[axis]).floor() as i64;
        // `ceil() - 1`: the cube's high face lands *on* the boundary of the next cell without
        // entering it, so an axis-aligned unit cube covers exactly one cell per axis.
        hi[axis] = ((max[axis] + shift[axis]).ceil() as i64 - 1).max(lo[axis]);
    }
    (lo, hi)
}

/// One phase's gap analysis: the posed occupancy grid's connectivity, with every stray component
/// attributed to the parts that own it.
///
/// Each filled cell of the **rest** assembled grid is carried into its owning part's posed frame by
/// that part's own rest→posed delta (`posed · rest⁻¹`, both from
/// [`part_placements`](crate::extract::part_placements)) and rasterized *conservatively*: every cell
/// its transformed unit cube's AABB touches is marked filled ([`covered_box`]), over a grid grown to
/// fit. At rest every delta is the identity and every box is exactly one cell, so this reproduces
/// the rest grid exactly.
fn phase_report(
    assembly: &AssemblyDocument,
    grid: &AssembledGrid,
    rest: &[(Mat4, Mat3)],
    posed: &[(Mat4, Mat3)],
    time: f64,
    tick: u64,
) -> Result<PhaseReport, Vec<Diagnostic>> {
    #[allow(clippy::disallowed_methods)] // render-side: GPU matrix, never reaches a hash
    let deltas: Vec<Mat4> = rest
        .iter()
        .zip(posed)
        .map(|((rest, _), (posed, _))| *posed * rest.inverse())
        .collect();

    let [sx, sy, sz] = grid.voxel.size;
    let (hx, hz) = (sx as f32 * 0.5, sz as f32 * 0.5);
    // The world→cell origin shift, per axis: the mesher lays cell `x` over world
    // `[x − sx/2, x − sx/2 + 1]` and cell `y` over `[y, y + 1]` (bottom-center origin, 1 voxel =
    // 1 unit — `crate::voxel_mesh`), which is the frame `part_placements` works in. So a world
    // coordinate is a cell coordinate once these are added back.
    let shift = [hx, 0.0, hz];
    let mut boxes: Vec<([i64; 3], [i64; 3], usize)> = Vec::new();
    for y in 0..sy {
        for z in 0..sz {
            for x in 0..sx {
                let owners = grid.owners_at(x, y, z);
                if owners.is_empty() {
                    continue;
                }
                let low = Vec3::new(x as f32 - hx, y as f32, z as f32 - hz);
                for &owner in owners {
                    let Some(delta) = deltas.get(owner) else {
                        continue;
                    };
                    let (lo, hi) = covered_box(*delta, low, shift);
                    boxes.push((lo, hi, owner));
                }
            }
        }
    }
    if boxes.is_empty() {
        return Ok(PhaseReport {
            time,
            tick,
            components: Some(0),
            strays: Vec::new(),
            weight: None,
            params: None,
            look: None,
            phase: None,
        });
    }

    let (min, max) = boxes.iter().fold(
        ([i64::MAX; 3], [i64::MIN; 3]),
        |(mut min, mut max), (lo, hi, _)| {
            for axis in 0..3 {
                min[axis] = min[axis].min(lo[axis]);
                max[axis] = max[axis].max(hi[axis]);
            }
            (min, max)
        },
    );
    let extent: Vec<u64> = (0..3)
        .map(|axis| (max[axis] - min[axis] + 1) as u64)
        .collect();
    let volume = extent
        .iter()
        .try_fold(1u64, |total, &side| total.checked_mul(side))
        .filter(|&volume| volume <= MAX_POSED_GRID_CELLS)
        .ok_or_else(|| {
            refuse_clip(
                format!(
                    "the pose at t = {time} scatters this model across a {}x{}x{} grid, past the \
                     {MAX_POSED_GRID_CELLS}-cell limit the gap analysis builds",
                    extent[0], extent[1], extent[2]
                ),
                "a rigid pose cannot spread a bounded model this far — check the clip's \
                 `translation` keys, which are in part-local voxel units, not world metres",
            )
        })?;

    let (px, py, pz) = (extent[0] as u32, extent[1] as u32, extent[2] as u32);
    let index = |[x, y, z]: [i64; 3]| -> usize {
        let (x, y, z) = (
            (x - min[0]) as u32,
            (y - min[1]) as u32,
            (z - min[2]) as u32,
        );
        ((y * pz + z) * px + x) as usize
    };
    let mut glyphs = vec![POSED_EMPTY; volume as usize];
    let mut owners: Vec<Vec<usize>> = vec![Vec::new(); volume as usize];
    for (lo, hi, owner) in boxes {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                for x in lo[0]..=hi[0] {
                    let slot = index([x, y, z]);
                    glyphs[slot] = POSED_GLYPH;
                    if !owners[slot].contains(&owner) {
                        owners[slot].push(owner);
                    }
                }
            }
        }
    }

    let mut slices = Vec::with_capacity(py as usize);
    for y in 0..py {
        let mut layer = Vec::with_capacity(pz as usize);
        for z in 0..pz {
            layer.push(
                (0..px)
                    .map(|x| glyphs[((y * pz + z) * px + x) as usize])
                    .collect::<String>(),
            );
        }
        slices.push(layer);
    }
    let voxel = VoxelDocument {
        name: None,
        size: [px, py, pz],
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette: vec![PaletteEntry {
            glyph: POSED_GLYPH.to_string(),
            color: [1.0, 1.0, 1.0, 1.0],
            emissive: None,
            metallic: None,
            porosity: None,
            roughness: None,
        }],
        slices,
        rim: None,
        lods: Vec::new(),
    };

    // The same analysis `check-scene` runs over the rest pose — never a second implementation.
    let components = voxel.connectivity_components();
    let names = |cell: [u32; 3]| -> Vec<String> {
        let slot = ((cell[1] * pz + cell[2]) * px + cell[0]) as usize;
        let mut indices = owners[slot].clone();
        indices.sort_unstable();
        indices
            .into_iter()
            .filter_map(|owner| assembly.parts.get(owner))
            .map(|part| part.name.clone())
            .collect()
    };
    let strays = components
        .iter()
        .take(components.len().saturating_sub(1))
        .map(|component| PhaseStray {
            sample: component.sample,
            owners: names(component.sample),
            main: component.gap.map(|gap| PhaseGap {
                cell: gap.main,
                owners: names(gap.main),
            }),
        })
        .collect();

    Ok(PhaseReport {
        time,
        tick,
        components: Some(components.len()),
        strays,
        weight: None,
        params: None,
        look: None,
        phase: None,
    })
}

/// Where each cell resolves its model reference from, plus what the overrides changed.
struct Staged {
    /// Deletes the staged tree when the render finishes or fails.
    _guard: Option<TempStage>,
    /// One project root per cell, in sheet order — the model's own directory when nothing was
    /// staged, otherwise the directory holding that cell's overridden copy.
    roots: Vec<PathBuf>,
    /// The name each model is referenced by from the synthesized scene, in model order — its own
    /// file name unless the models were staged side by side, where an index prefix keeps two
    /// same-named neighbours apart.
    names: Vec<String>,
    /// What each `--set` override changed.
    applied: Vec<AppliedOverride>,
    /// The value the document held at the swept pointer before any cell wrote over it.
    original: Option<Value>,
}

/// A temp directory removed when the render finishes, however it finishes. Removal is best-effort:
/// a leftover directory in the system temp is a nuisance, never a failed render.
struct TempStage {
    root: PathBuf,
}

impl Drop for TempStage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Distinguishes one render's staging tree from another's within a process. A counter rather than a
/// clock: this crate takes no wall-clock reading, and uniqueness is all that is wanted.
static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Puts every model where the extractor can read it, and applies the request's overrides on the way.
///
/// The extractor resolves a `Mesh.shape: { voxel: … }` by **re-reading the file from the scene's
/// project root**, so an in-memory edit has to land on disk, and a scene naming several models needs
/// them all under one root. Three shapes:
///
/// - **One model, no overrides** — nothing is written and the cell reads it where it lives, so the
///   ordinary sheet touches no temp directory at all.
/// - **Overrides** — the mutated document is re-emitted under its original file name into its own
///   directory (one per sweep cell), which becomes that cell's project root, with an assembly's
///   dependency tree copied beside it — see [`Overridden::write`].
/// - **A lineup** — every model is copied verbatim into its own numbered subdirectory under one
///   root (`lineup/<index>/`), an assembly's dependency tree carried along beside it — see
///   [`stage_lineup`].
fn stage(
    request: &ModelSheetRequest,
    loaded: &[LoadedModel],
    cells: usize,
) -> Result<Staged, Vec<Diagnostic>> {
    let names = |loaded: &[LoadedModel]| -> Vec<String> {
        loaded
            .iter()
            .map(|model| model.bounds.file_name.clone())
            .collect()
    };
    if loaded.len() > 1 {
        return stage_lineup(request.models, loaded);
    }

    let one = &loaded[0];
    let model_dir = request.models[0]
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    if request.set.is_empty() && request.sweep.is_none() {
        return Ok(Staged {
            _guard: None,
            roots: vec![model_dir; cells],
            names: names(loaded),
            applied: Vec::new(),
            original: None,
        });
    }

    let mut base = match &one.extra {
        LoadedExtra::Voxel(doc) => Overridden::Voxel(doc.clone()),
        LoadedExtra::Assembly(doc) => Overridden::Assembly(doc.clone(), request.models[0]),
        LoadedExtra::Sculpt(sculpt) => Overridden::Sculpt(sculpt.doc.clone(), request.models[0]),
        LoadedExtra::Mesh => {
            return Err(vec![
                Diagnostic::new(
                    code::SCENE_OVERRIDE_INVALID,
                    "`--set`/`--sweep` cannot override this model: only a voxel or assembly \
                     document has a palette, and a sculpt its numeric fields"
                        .to_string(),
                )
                .with_help(
                    "a `.mesh.yaml` asset carries per-vertex colors, not a palette — override the \
                     scene that places it instead (`render --scene … --set`)",
                ),
            ]);
        }
        LoadedExtra::Prefab(_) => {
            return Err(vec![
                Diagnostic::new(
                    code::SCENE_OVERRIDE_INVALID,
                    "`--set`/`--sweep` cannot override this model: a prefab has no palette to \
                     tune"
                        .to_string(),
                )
                .with_help(
                    "a `.prefab.yaml` draws its parts' own meshes and materials — change the \
                     part documents, or override the scene that places the instance \
                     (`render --scene … --set`)",
                ),
            ]);
        }
        LoadedExtra::Character(_) => {
            return Err(vec![
                Diagnostic::new(
                    code::SCENE_OVERRIDE_INVALID,
                    "`--set`/`--sweep` cannot override this model: a character has no palette to \
                     tune"
                        .to_string(),
                )
                .with_help(
                    "a `.character.yaml` draws its skinned mesh's own colors and textures — \
                     change the mesh document, or override the scene that places the character \
                     (`render --scene … --set`)",
                ),
            ]);
        }
    };

    let file_name = one.bounds.file_name.as_str();
    let applied = base.apply(request.set)?;

    let guard = temp_stage();

    let (roots, original) = match request.sweep {
        None => {
            let root = guard.root.join("set");
            base.write(&root, file_name)?;
            (vec![root; cells], None)
        }
        Some(sweep) => {
            let mut roots = Vec::with_capacity(sweep.values.len());
            let mut original = None;
            for (index, value) in sweep.values.iter().enumerate() {
                let mut cell = base.clone();
                let over = [SceneOverride {
                    pointer: sweep.pointer.to_string(),
                    value: value.clone(),
                }];
                let change = cell.apply(&over)?;
                if index == 0 {
                    original = change.first().and_then(|change| change.from.clone());
                }
                let root = guard.root.join(format!("cell-{index}"));
                cell.write(&root, file_name)?;
                roots.push(root);
            }
            (roots, original)
        }
    };

    Ok(Staged {
        _guard: Some(guard),
        roots,
        names: names(loaded),
        applied,
        original,
    })
}

/// One model document under override, and whatever a staged copy of it needs on disk beside it.
///
/// A voxel document is self-contained: one file is the whole cell. An assembly's copy only resolves
/// if the part and clip files it names are copied at the same relative paths — and those are
/// relative to the **assembly file's own directory**, which is why the source path travels with the
/// document.
#[derive(Clone)]
enum Overridden<'a> {
    Voxel(VoxelDocument),
    Assembly(AssemblyDocument, &'a Path),
    Sculpt(SculptDocument, &'a Path),
}

impl Overridden<'_> {
    /// Writes the request's overrides into this document — the one palette a voxel or assembly
    /// owns (in an assembly's case the one every part it places resolves its glyphs against), or any
    /// numeric field of a sculpt.
    fn apply(
        &mut self,
        overrides: &[SceneOverride],
    ) -> Result<Vec<AppliedOverride>, Vec<Diagnostic>> {
        match self {
            Self::Voxel(doc) => ludion_scene::apply_palette_overrides(doc, overrides),
            Self::Assembly(doc, _) => {
                ludion_scene::apply_assembly_palette_overrides(doc, overrides)
            }
            Self::Sculpt(doc, _) => ludion_scene::apply_sculpt_overrides(doc, overrides),
        }
    }

    /// Writes one cell's project root: the mutated document under its original file name, plus the
    /// dependency tree an assembly or sculpt reads relative to itself. A dependency path that
    /// climbs out of the model's directory is re-rooted inside `root` and the emitted document
    /// names the staged path.
    fn write(&self, root: &Path, file_name: &str) -> Result<(), Vec<Diagnostic>> {
        match self {
            Self::Voxel(doc) => write_staged(root, file_name, &ludion_scene::emit_voxel(doc)),
            Self::Assembly(doc, source) => {
                let staged = stage_assembly_dependencies(source, doc, root)?;
                let text = ludion_scene::emit_assembly(staged.as_ref().unwrap_or(doc));
                write_staged(root, file_name, &text)
            }
            Self::Sculpt(doc, source) => {
                let staged = stage_sculpt_dependencies(source, doc, root)?;
                let text = ludion_scene::emit_sculpt(staged.as_ref().unwrap_or(doc));
                write_staged(root, file_name, &text)
            }
        }
    }
}

/// Copies a lineup's models side by side under one root — the arrangement a scene naming several
/// documents needs, since one scene resolves every asset against one project root.
///
/// Each model gets its own numbered subdirectory (`lineup/<index>/`) rather than a flat,
/// index-prefixed file name: a voxel or mesh document is one file and either scheme would serve it,
/// but an assembly's part files are read **relative to the assembly file's own directory**
/// (`crate::assembly_mesh`), so it needs a directory of its own to carry that relative structure
/// into — [`stage_assembly_dependencies`] copies exactly the files the assembly names under the
/// assembly's slot, and a path that climbs out of the model's directory (`../parts/arm`) is
/// re-rooted inside the slot, so two models never share or escape a staged file. A document whose
/// paths needed no re-rooting is copied byte for byte.
fn stage_lineup(sources: &[&Path], loaded: &[LoadedModel]) -> Result<Staged, Vec<Diagnostic>> {
    let guard = temp_stage();
    let root = guard.root.join("lineup");
    let mut names = Vec::with_capacity(loaded.len());
    for (index, (source, model)) in sources.iter().zip(loaded).enumerate() {
        let slot = root.join(index.to_string());
        let file_name = &model.bounds.file_name;
        let rewritten = match &model.extra {
            LoadedExtra::Assembly(doc) => stage_assembly_dependencies(source, doc, &slot)?
                .map(|doc| ludion_scene::emit_assembly(&doc)),
            LoadedExtra::Sculpt(sculpt) => stage_sculpt_dependencies(source, &sculpt.doc, &slot)?
                .map(|doc| ludion_scene::emit_sculpt(&doc)),
            // `check_alone` refuses a character or a prefab in a lineup before anything is staged.
            LoadedExtra::Voxel(_)
            | LoadedExtra::Mesh
            | LoadedExtra::Character(_)
            | LoadedExtra::Prefab(_) => None,
        };
        std::fs::create_dir_all(&slot)
            .and_then(|()| match &rewritten {
                Some(text) => std::fs::write(slot.join(file_name), text),
                None => std::fs::copy(source, slot.join(file_name)).map(|_| ()),
            })
            .map_err(|error| stage_io_error(&slot, error))?;
        names.push(format!("{index}/{file_name}"));
    }
    Ok(Staged {
        _guard: Some(guard),
        roots: vec![root],
        names,
        applied: Vec::new(),
        original: None,
    })
}

/// Copies every file `doc` names — its parts and its clips — into `slot`: the dependencies an
/// assembly's top-level document alone does not carry, and a flat root copy would otherwise silently
/// drop, leaving the extractor's own `L1101` (file not found) as the only symptom. Clips travel with
/// the parts because a staged copy is rendered posed as readily as at rest.
///
/// Resolution matches [`crate::assembly_mesh::load`] exactly: each `file` is relative to the
/// **assembly file's own directory** (`source`'s parent). A path with no `..` is written back at
/// that same relative path under `slot`; see [`staged_keys`] for one that climbs out. Returns the
/// document naming the staged paths, or `None` when none changed.
fn stage_assembly_dependencies(
    source: &Path,
    doc: &AssemblyDocument,
    slot: &Path,
) -> Result<Option<AssemblyDocument>, Vec<Diagnostic>> {
    let files: Vec<&str> = doc
        .parts
        .iter()
        .map(|part| part.file.as_str())
        .chain(doc.clips.iter().map(|clip| clip.file.as_str()))
        .collect();
    let keys = copy_dependencies(source, &files, slot, code::ASSEMBLY_DATA_CONSISTENCY)?;
    if !keys_rewritten(&keys) {
        return Ok(None);
    }
    let mut staged = doc.clone();
    for part in &mut staged.parts {
        part.file = staged_key(&keys, &part.file);
    }
    for clip in &mut staged.clips {
        clip.file = staged_key(&keys, &clip.file);
    }
    Ok(Some(staged))
}

/// [`stage_assembly_dependencies`] for a sculpt: the voxel models and the meshes its parts name. A
/// mesh document's own texture maps are not among them — the mesher reads its triangles only.
fn stage_sculpt_dependencies(
    source: &Path,
    doc: &SculptDocument,
    slot: &Path,
) -> Result<Option<SculptDocument>, Vec<Diagnostic>> {
    let mut files = ludion_sculpt::voxel_paths(doc);
    files.extend(ludion_sculpt::mesh_paths(doc));
    let keys = copy_dependencies(source, &files, slot, code::SCULPT_DOCUMENT_DATA)?;
    if !keys_rewritten(&keys) {
        return Ok(None);
    }
    let mut staged = doc.clone();
    for part in &mut staged.parts {
        if let ludion_scene::SculptShape::Voxel { path, .. }
        | ludion_scene::SculptShape::Mesh { path, .. } = &mut part.shape
        {
            *path = staged_key(&keys, path);
        }
    }
    Ok(Some(staged))
}

/// Copies every file a staged document names beside it to its staged key under `slot`. Both kinds
/// read their references relative to their own file, so a flat copy would drop them and leave the
/// extractor's `L1101` as the only symptom.
fn copy_dependencies(
    source: &Path,
    files: &[&str],
    slot: &Path,
    code: DiagnosticCode,
) -> Result<Vec<(String, String)>, Vec<Diagnostic>> {
    let keys = staged_keys(source, files, code)?;
    let source_dir = source.parent().unwrap_or_else(|| Path::new("."));
    for (authored, staged) in &keys {
        let dest = slot.join(staged);
        let dest_dir = dest.parent().unwrap_or(slot);
        std::fs::create_dir_all(dest_dir)
            .and_then(|()| std::fs::copy(source_dir.join(authored), &dest).map(|_| ()))
            .map_err(|error| stage_io_error(dest_dir, error))?;
    }
    Ok(keys)
}

/// The staged path of `authored`, or `authored` itself when it is not among `keys`.
fn staged_key(keys: &[(String, String)], authored: &str) -> String {
    keys.iter()
        .find(|(from, _)| from == authored)
        .map_or_else(|| authored.to_string(), |(_, to)| to.clone())
}

fn keys_rewritten(keys: &[(String, String)]) -> bool {
    keys.iter().any(|(authored, staged)| authored != staged)
}

/// (authored key, staged key) for each dependency of the document at `source`.
///
/// A dependency may climb out of the model's directory (`../parts/arm.part.yaml`), but the staged
/// copy of the document sits at the root of its slot, so the climb has nowhere to go and two models
/// sharing a `../` directory would overwrite each other. Staging therefore re-roots every path
/// under the slot: with `k` the deepest climb among `files`, the last `k` directories of the
/// source's own path become real directories in the slot, and each file keeps its position relative
/// to them — a file `u` levels up lands `k - u` of those directories down. With no `..` anywhere
/// (`k == 0`) every staged key is the authored path, and the document is not rewritten.
///
/// # Errors
/// `code` if a path is absolute, names no file, or climbs higher than the source has ancestors.
fn staged_keys(
    source: &Path,
    files: &[&str],
    code: DiagnosticCode,
) -> Result<Vec<(String, String)>, Vec<Diagnostic>> {
    let refuse = |file: &str, why: &str| {
        vec![
            Diagnostic::new(
                code,
                format!("cannot stage `{file}` for `{}`: {why}", source.display()),
            )
            .with_help(
                "a dependency path must stay under a directory that exists: use a path relative \
                 to the model file, with no more `..` than it has parent directories",
            ),
        ]
    };
    let mut parsed = Vec::with_capacity(files.len());
    for &file in files {
        let mut ups = 0usize;
        let mut rest: Vec<String> = Vec::new();
        for component in Path::new(file).components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    if rest.pop().is_none() {
                        ups += 1;
                    }
                }
                Component::Normal(name) => rest.push(name.to_string_lossy().into_owned()),
                Component::RootDir | Component::Prefix(_) => {
                    return Err(refuse(file, "it is an absolute path"));
                }
            }
        }
        if rest.is_empty() {
            return Err(refuse(file, "it names no file"));
        }
        parsed.push((ups, rest));
    }
    let deepest = parsed.iter().map(|(ups, _)| *ups).max().unwrap_or(0);
    if deepest == 0 {
        return Ok(files
            .iter()
            .map(|file| (file.to_string(), file.to_string()))
            .collect());
    }

    let mut anchors: Vec<String> = Vec::new();
    let absolute = std::path::absolute(source).map_err(|error| stage_io_error(source, error))?;
    for component in absolute.parent().unwrap_or(&absolute).components() {
        match component {
            Component::Normal(name) => anchors.push(name.to_string_lossy().into_owned()),
            Component::ParentDir => {
                anchors.pop();
            }
            _ => {}
        }
    }
    if anchors.len() < deepest {
        let deep = parsed.iter().position(|(ups, _)| *ups == deepest);
        return Err(refuse(
            files[deep.unwrap_or(0)],
            "it climbs out of more directories than the model's path has",
        ));
    }
    Ok(files
        .iter()
        .zip(parsed)
        .map(|(file, (ups, rest))| {
            let kept = &anchors[anchors.len() - deepest..anchors.len() - ups];
            let staged: Vec<&str> = kept.iter().chain(&rest).map(String::as_str).collect();
            (file.to_string(), staged.join("/"))
        })
        .collect())
}

/// A fresh, empty staging directory for one render.
fn temp_stage() -> TempStage {
    let guard = TempStage {
        root: std::env::temp_dir().join(format!(
            "ludion-model-sheet-{}-{}",
            std::process::id(),
            STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )),
    };
    let _ = std::fs::remove_dir_all(&guard.root);
    guard
}

/// Writes one staged copy of a model document, creating its directory.
fn write_staged(root: &Path, file_name: &str, contents: &str) -> Result<(), Vec<Diagnostic>> {
    std::fs::create_dir_all(root)
        .and_then(|()| std::fs::write(root.join(file_name), contents))
        .map_err(|error| stage_io_error(root, error))
}

/// `L1117` for a staging directory that cannot be written.
fn stage_io_error(root: &Path, error: std::io::Error) -> Vec<Diagnostic> {
    vec![
        Diagnostic::new(
            code::MODEL_SHEET_INPUT,
            format!("could not stage the model in `{}`: {error}", root.display()),
        )
        .with_help(
            "`--set`/`--sweep` and a lineup write the model to a temp directory the renderer reads \
             it back from; check that the system temp directory is writable",
        ),
    ]
}

/// The component types `--rig` lifts out of a scene: an entity carrying any of them is copied whole
/// into the sheet scene. None of the four carries an asset path, so a lifted entity needs no path
/// re-rooting against the sheet's project root (the model document's own directory).
const RIG_COMPONENTS: [&str; 5] = [
    "Environment",
    "PostProcess",
    "DirectionalLight",
    "PointLight",
    "SpotLight",
];

/// The lighting and grade lifted out of a `--rig` scene.
#[derive(Debug)]
struct RigLift {
    /// The kept entities, ready to append to the synthesized scene as they are.
    entities: Vec<ludion_scene::SceneEntity>,
    /// Whether any kept entity carries a light. The synthesized key + fill are then skipped
    /// entirely: mixing a borrowed rig with the neutral one reads as neither.
    has_light: bool,
    /// The color of a rig entity named `ground` that carries a `Mesh`, if any — it replaces the
    /// sheet ground plane's gallery gray (the plane keeps the sheet's own transform).
    ground_color: Option<[f32; 4]>,
}

/// Reads and validates the scene at `path`, then lifts its lighting/grade entities.
///
/// An invalid rig reports the scene's own diagnostics verbatim — the rig is an ordinary scene file
/// and its author should read the ordinary scene errors.
fn load_rig(path: &Path) -> Result<RigLift, Vec<Diagnostic>> {
    let source = std::fs::read_to_string(path).map_err(|error| {
        vec![
            Diagnostic::new(
                code::MODEL_SHEET_INPUT,
                format!("could not read rig scene `{}`: {error}", path.display()),
            )
            .with_help(
                "pass a readable scene file to `--rig` (check the path and permissions), e.g. \
                 `examples/gallery/night-street.yaml`",
            ),
        ]
    })?;

    let scene = ludion_scene::check(&source, &builtin_registry())?;
    lift(&scene, path)
}

/// The keep-list walk over an already-validated rig scene. Split from [`load_rig`] so the contract
/// is exercised without touching the filesystem; `path` only names the scene in the diagnostic.
fn lift(scene: &ludion_scene::Scene, path: &Path) -> Result<RigLift, Vec<Diagnostic>> {
    let mut kept = Vec::new();
    let mut has_light = false;
    let mut ground_color = None;
    for entity in &scene.entities {
        if entity.name.as_deref() == Some("ground") {
            ground_color = component(entity, "Mesh").and_then(rgba);
        }
        let carries = |name: &str| component(entity, name).is_some();
        if !RIG_COMPONENTS.iter().any(|name| carries(name)) {
            continue;
        }
        has_light |= carries("DirectionalLight") || carries("PointLight") || carries("SpotLight");
        kept.push(entity.clone());
    }

    if kept.is_empty() {
        return Err(vec![
            Diagnostic::new(
                code::MODEL_SHEET_INPUT,
                format!(
                    "rig scene `{}` has no liftable entity: none of its entities carries an \
                     `Environment`, `PostProcess`, `DirectionalLight`, `PointLight`, or \
                     `SpotLight` component",
                    path.display()
                ),
            )
            .with_help(
                "point `--rig` at a scene that holds the lighting you want borrowed, e.g. \
                 `examples/gallery/night-street.yaml`",
            ),
        ]);
    }

    Ok(RigLift {
        entities: kept,
        has_light,
        ground_color,
    })
}

/// An entity's component value by name, or `None` when it does not carry that component.
fn component<'a>(
    entity: &'a ludion_scene::SceneEntity,
    name: &str,
) -> Option<&'a serde_json::Value> {
    entity
        .components
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

/// A component value's `color` field as RGBA, when it is a four-number array.
fn rgba(value: &serde_json::Value) -> Option<[f32; 4]> {
    let items = value.get("color")?.as_array()?;
    let mut out = [0.0f32; 4];
    if items.len() != out.len() {
        return None;
    }
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = item.as_f64()? as f32;
    }
    Some(out)
}

/// The bottom-center, 1-voxel-per-unit AABB of a voxel model (matches `crate::voxel_mesh`).
fn voxel_aabb(doc: &VoxelDocument) -> ([f32; 3], [f32; 3]) {
    let [sx, sy, sz] = doc.size;
    let (hx, hz) = (sx as f32 * 0.5, sz as f32 * 0.5);
    ([-hx, 0.0, -hz], [hx, sy as f32, hz])
}

/// The min/max AABB over a mesh document's vertex positions; an empty model collapses to the origin
/// (the radius clamp then keeps it framable).
fn mesh_aabb(doc: &MeshDocument) -> ([f32; 3], [f32; 3]) {
    points_aabb(
        doc.positions
            .chunks_exact(3)
            .map(|v| [v[0] as f32, v[1] as f32, v[2] as f32]),
    )
}

/// [`mesh_aabb`] over the positions a sculpt meshed to.
fn sculpt_aabb(mesh: &SculptMesh) -> ([f32; 3], [f32; 3]) {
    points_aabb(mesh.positions.chunks_exact(3).map(|v| [v[0], v[1], v[2]]))
}

/// The min/max box of `points`, or the origin when there are none.
fn points_aabb(points: impl Iterator<Item = [f32; 3]>) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in points {
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    if !min[0].is_finite() {
        return ([0.0; 3], [0.0; 3]);
    }
    (min, max)
}

/// The normalization transform placed on the model entity, plus the scaled look-at target `C`.
struct NormalizeRig {
    translation: [f32; 3],
    scale: f32,
    target: Vec3,
}

impl NormalizeRig {
    /// Scales the model to a unit bounding sphere and sits its AABB bottom-center on the world
    /// origin. Over a bare model-space box, because [`model_camera`] frames a model that has no
    /// [`ModelBounds`] behind it through exactly this arithmetic rather than a second copy.
    fn from_box(min: [f32; 3], max: [f32; 3]) -> NormalizeRig {
        let k = 1.0 / bounding_radius(min, max);
        let cx = (min[0] + max[0]) * 0.5;
        let cz = (min[2] + max[2]) * 0.5;
        NormalizeRig {
            translation: [-cx * k, -min[1] * k, -cz * k],
            scale: k,
            // Scaled box center: x/z centered on the origin, y half the scaled height above ground.
            target: Vec3::new(0.0, k * (max[1] - min[1]) * 0.5, 0.0),
        }
    }
}

/// The box a part sheet frames: the part's meshed bounds with every axis shorter than
/// [`MIN_PART_FRACTION`] of the model's largest extent grown about its center to that length. A
/// thin or tiny part (an eye's shell) would otherwise set a unit sphere so small that the whole
/// sheet is a few pixels of surface seen from a camera pressed against it.
fn framed_part_box(model: &ModelBounds, part: ([f32; 3], [f32; 3])) -> ([f32; 3], [f32; 3]) {
    let largest = (0..3)
        .map(|axis| model.max[axis] - model.min[axis])
        .fold(0.0f32, f32::max);
    let shortest = largest * MIN_PART_FRACTION;
    let (mut min, mut max) = part;
    for axis in 0..3 {
        if max[axis] - min[axis] < shortest {
            let center = 0.5 * (min[axis] + max[axis]);
            min[axis] = center - 0.5 * shortest;
            max[axis] = center + 0.5 * shortest;
        }
    }
    (min, max)
}

/// Half a box's diagonal — its bounding-sphere radius — clamped to [`MIN_RADIUS`] so a degenerate
/// box still frames.
fn bounding_radius(min: [f32; 3], max: [f32; 3]) -> f32 {
    let diag = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let radius = 0.5 * math::sqrt(diag[0] * diag[0] + diag[1] * diag[1] + diag[2] * diag[2]);
    radius.max(MIN_RADIUS)
}

/// Where one model stands in the sheet's normalized space.
pub(crate) struct Placement {
    pub(crate) translation: [f32; 3],
    pub(crate) scale: f32,
    /// The model's footprint center, for projecting its caption position.
    pub(crate) base: Vec3,
}

/// The framing every cell shares: where each model stands, what the camera looks at, and the two
/// heights `--true-size` reasons about.
///
/// One model is normalized on its own, exactly as it always has been — **scale-blind**, so a
/// declared `scale:` never moves a pinned sheet. A lineup normalizes the *union* instead, which is
/// the whole point of one: relative sizes are honest by construction, so a 12-unit house really does
/// stand 5.5× the 2.16-unit statue beside it.
pub(crate) struct Subject {
    /// One entry per model, in the order the models were given.
    pub(crate) placements: Vec<Placement>,
    /// The camera's look-at target.
    pub(crate) target: Vec3,
    /// The subject's height in declared-scale world units — the tallest model in a lineup. What
    /// `--true-size` earns its pixels from.
    world_height: f32,
    /// That height after normalization — what actually lands in the cell.
    normalized_height: f32,
    /// The whole model's box in normalized sheet space when the camera frames something inside it
    /// (a part), so [`camera_distance`](Self::camera_distance) can keep the camera out of the model.
    /// `None` when the framed subject is the whole of every model.
    enclosure: Option<([f32; 3], [f32; 3])>,
}

impl Subject {
    /// One model, framed exactly as [`NormalizeRig`] has always framed it.
    fn single(bounds: &ModelBounds) -> Subject {
        Subject::from_box(bounds.min, bounds.max, bounds.scale)
    }

    /// [`single`](Self::single) over a bare model-space box and a declared world scale — the form
    /// [`model_camera`] frames with, so the public camera and the sheet's own share one derivation.
    pub(crate) fn from_box(min: [f32; 3], max: [f32; 3], world_scale: f32) -> Subject {
        let rig = NormalizeRig::from_box(min, max);
        let height = max[1] - min[1];
        Subject {
            placements: vec![Placement {
                translation: rig.translation,
                scale: rig.scale,
                base: Vec3::ZERO,
            }],
            target: rig.target,
            world_height: height * world_scale,
            normalized_height: height * rig.scale,
            enclosure: None,
        }
    }

    /// One model framed on a box inside it — a sculpt part's meshed bounds, grown by
    /// [`framed_part_box`]. The box is fitted to the unit sphere and centered on the camera target
    /// exactly as [`single`](Self::single) fits a whole model, but the model keeps its own floor on
    /// the ground plane, so the rest of it stands where it always does around the part. That rest can
    /// surround the camera's fit position, so the model's box is kept as the
    /// [`enclosure`](Self::enclosure) every view's camera must stay outside.
    fn part(bounds: &ModelBounds, part: ([f32; 3], [f32; 3])) -> Subject {
        let (min, max) = framed_part_box(bounds, part);
        let k = 1.0 / bounding_radius(min, max);
        let floor = bounds.min[1];
        let height = max[1] - min[1];
        let translation = [
            -(min[0] + max[0]) * 0.5 * k,
            -floor * k,
            -(min[2] + max[2]) * 0.5 * k,
        ];
        let seat = |p: [f32; 3]| [0, 1, 2].map(|axis| p[axis] * k + translation[axis]);
        Subject {
            placements: vec![Placement {
                translation,
                scale: k,
                base: Vec3::ZERO,
            }],
            target: Vec3::new(0.0, k * ((min[1] + max[1]) * 0.5 - floor), 0.0),
            world_height: height * bounds.scale,
            normalized_height: height * k,
            enclosure: Some((seat(bounds.min), seat(bounds.max))),
        }
    }

    /// The camera's distance from [`target`](Self::target) in `view`: the unit-sphere
    /// [`fit_distance`], raised when the subject has an [`enclosure`](Self::enclosure) until the
    /// camera is outside it. The view ray from the target leaves the box through the first face it
    /// crosses; the camera stops [`CAMERA_CLEARANCE`] beyond that face (perpendicular to it). With no
    /// enclosure this is exactly `fit_distance`, which keeps every whole-model sheet unchanged.
    fn camera_distance(&self, view: ModelView, cell_w: u32, cell_h: u32) -> f32 {
        let fit = fit_distance(cell_w, cell_h);
        let Some((min, max)) = self.enclosure else {
            return fit;
        };
        let (direction, _) = view.direction_and_up();
        let exit = (0..3)
            .filter(|&axis| direction[axis].abs() > f32::EPSILON)
            .map(|axis| {
                let step = direction[axis];
                let face = if step > 0.0 { max[axis] } else { min[axis] };
                ((face - self.target[axis]) / step).max(0.0) + CAMERA_CLEARANCE / step.abs()
            })
            .fold(f32::INFINITY, f32::min);
        fit.max(exit)
    }

    /// A row of models standing side by side on one ground plane, the union of their **scaled** AABBs
    /// normalized as a whole.
    ///
    /// Each model's world extent is its raw AABB times its declared `scale:`, so the row's
    /// proportions are the game's, and the single normalization `k` that fits the union in the cell
    /// multiplies every model's own scale — which is what makes the comparison honest rather than a
    /// set of independently-framed thumbnails.
    ///
    /// The row runs along `direction` in the order given, each model taking its own footprint width
    /// along that direction plus a shared gap, the whole row centered on the origin; every model
    /// keeps its own facing and sits bottom-anchored on the plane.
    fn lineup(models: &[&ModelBounds], direction: Vec3) -> Subject {
        let half =
            |bounds: &ModelBounds, i: usize| 0.5 * (bounds.max[i] - bounds.min[i]) * bounds.scale;
        // An axis-aligned box's extent along an arbitrary horizontal direction is its support width:
        // the direction's components weighted by the box's own half-extents.
        let seats: Vec<f32> = models
            .iter()
            .map(|m| 2.0 * (direction.x.abs() * half(m, 0) + direction.z.abs() * half(m, 2)))
            .collect();
        let gap = LINEUP_GAP_FRACTION * seats.iter().copied().fold(0.0f32, f32::max);
        let total: f32 = seats.iter().sum::<f32>() + gap * (models.len() - 1) as f32;

        // Walk the row, recording each model's footprint center, and take the union of the boxes
        // those centers imply. The union is what the camera frames.
        let mut cursor = -0.5 * total;
        let mut centers = Vec::with_capacity(models.len());
        let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut min_z, mut max_z) = (f32::INFINITY, f32::NEG_INFINITY);
        let mut height = 0.0f32;
        for (bounds, seat) in models.iter().zip(&seats) {
            let center = direction * (cursor + 0.5 * seat);
            cursor += seat + gap;
            min_x = min_x.min(center.x - half(bounds, 0));
            max_x = max_x.max(center.x + half(bounds, 0));
            min_z = min_z.min(center.z - half(bounds, 2));
            max_z = max_z.max(center.z + half(bounds, 2));
            height = height.max(2.0 * half(bounds, 1));
            centers.push(center);
        }
        let (union_x, union_z) = (0.5 * (min_x + max_x), 0.5 * (min_z + max_z));
        let (size_x, size_z) = (max_x - min_x, max_z - min_z);
        let radius =
            (0.5 * math::sqrt(size_x * size_x + height * height + size_z * size_z)).max(MIN_RADIUS);
        let k = 1.0 / radius;

        let placements = models
            .iter()
            .zip(&centers)
            .map(|(bounds, center)| {
                let scale = bounds.scale * k;
                let box_center = |i: usize| (bounds.min[i] + bounds.max[i]) * 0.5 * scale;
                let base = Vec3::new((center.x - union_x) * k, 0.0, (center.z - union_z) * k);
                Placement {
                    translation: [
                        base.x - box_center(0),
                        -bounds.min[1] * scale,
                        base.z - box_center(2),
                    ],
                    scale,
                    base,
                }
            })
            .collect();

        Subject {
            placements,
            target: Vec3::new(0.0, k * height * 0.5, 0.0),
            world_height: height,
            normalized_height: height * k,
            enclosure: None,
        }
    }
}

/// The horizontal direction a lineup's row runs in: the camera's **screen-right, flattened onto the
/// ground plane**.
///
/// Two properties come out of that one choice. The row reads left-to-right in the order the models
/// were given, whatever the view — `game` and `front` see world `+X` on the left where `iso` and
/// `back` see it on the right, so a fixed world axis would silently reverse. And the row is broadside
/// to the camera, so neighbours never partly hide one another: a row along `+X` seen from the `game`
/// camera's 24°-off yaw has each model creeping in front of the next, which is exactly what a
/// side-by-side comparison must not do.
///
/// Pure and deterministic — a function of the view alone.
fn row_direction(view: ModelView) -> Vec3 {
    let (direction, up) = view.direction_and_up();
    // The camera's screen-right, exactly as `Mat4::look_at_rh` builds it: `cross(forward, up)`.
    let right = (-direction).cross(up);
    let flat = Vec3::new(right.x, 0.0, right.z);
    // A camera looking straight along the world up would have no horizontal right; none of the seven
    // views does (`top` looks down with `+Z` up, so its right is `−X`), and `+X` is the answer a
    // lineup would want anyway.
    if flat.length_squared() > f32::EPSILON {
        flat.normalize()
    } else {
        Vec3::X
    }
}

/// The camera pose and light positions for one view, all pure functions of `(view, C, D)`.
pub(crate) struct ViewRig {
    pub(crate) position: Vec3,
    pub(crate) rotation_deg: [f32; 3],
    pub(crate) key_position: Vec3,
    pub(crate) fill_position: Vec3,
    pub(crate) range: f32,
}

impl ViewRig {
    pub(crate) fn compute(view: ModelView, subject: &Subject, cell_w: u32, cell_h: u32) -> ViewRig {
        let target = subject.target;
        let distance = subject.camera_distance(view, cell_w, cell_h);
        let (direction, up) = view.direction_and_up();
        let position = target + direction * distance;

        // Euler-XYZ decomposition of the look-at pose, so the extracted camera matches the framing
        // (extract rebuilds the view through the same `ludion_core::math` pair).
        #[allow(clippy::disallowed_methods)] // render-side: GPU matrix, never reaches a hash
        let model = Mat4::look_at_rh(position, target, up).inverse();
        let (_scale, rotation, _translation) = model.to_scale_rotation_translation();
        let (rx, ry, rz) = rotation.to_euler(EulerRot::XYZ);
        let rotation_deg = [rx.to_degrees(), ry.to_degrees(), rz.to_degrees()];

        // Camera basis, for a rig that reads clearly from every view.
        let forward = (target - position).normalize();
        let right = forward.cross(up).normalize();
        let cam_up = right.cross(forward).normalize();
        let key_position = position + (cam_up * 0.9 - right * 0.8 + forward * 0.2) * distance;
        let fill_position = position + (-cam_up * 0.6 + right * 0.9 + forward * 0.2) * distance;

        ViewRig {
            position,
            rotation_deg,
            key_position,
            fill_position,
            range: LIGHT_RANGE_FACTOR * distance,
        }
    }
}

/// The camera one whole-model sheet cell is drawn with, and the normalization that put the model in
/// front of it.
///
/// Every field a screen→world unprojection needs, in the plain-data spelling
/// `ludion::math::camera_ray` takes: pass [`translation`](Self::translation),
/// [`rotation_deg`](Self::rotation_deg), a perspective projection of [`fov_y_deg`](Self::fov_y_deg),
/// [`near`](Self::near), [`far`](Self::far), a pixel and the cell size, and the ray it returns is
/// the ray through that pixel of that cell.
///
/// **The ray is in the sheet's normalized space, not the model's.** The cell draws the model scaled
/// by [`normalize_scale`](Self::normalize_scale) and shifted by
/// [`normalize_translation`](Self::normalize_translation) (`p_normalized = p_model * scale +
/// translation`), so a caller intersecting model geometry either transforms its triangles forward
/// or divides the hit back out. [`label`](Self::label) names the world axis the camera sits on
/// exactly as the cell's burned-in caption does (`FRONT -Z`) — the authoring-space nickname for the
/// authored front has twice produced a 180°-wrong result, so it is never the thing to report.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelCamera {
    /// The camera's world-space position, in normalized sheet space (`Transform.translation`).
    pub translation: [f32; 3],
    /// The camera's Euler-XYZ rotation in degrees (`Transform.rotation`).
    pub rotation_deg: [f32; 3],
    /// The vertical field of view in degrees (`Camera.projection.fov_y_deg`).
    pub fov_y_deg: f32,
    /// The near clip plane (`Camera.near`).
    pub near: f32,
    /// The far clip plane (`Camera.far`).
    pub far: f32,
    /// The point the camera looks at, in normalized sheet space.
    pub target: [f32; 3],
    /// The uniform scale the model is drawn at: `1 / half-diagonal` of its AABB.
    pub normalize_scale: f32,
    /// The translation applied **after** [`normalize_scale`](Self::normalize_scale), which sits the
    /// scaled AABB's bottom-centre on the origin.
    pub normalize_translation: [f32; 3],
    /// The view's cell caption — the world axis the camera sits on, e.g. `FRONT -Z`.
    pub label: &'static str,
}

/// The camera [`render_model_sheet`] draws one whole-model cell with, as a pure function of the
/// view, the model's model-space AABB and the cell's pixel size.
///
/// This is the one definition: the sheet's own cells are framed by exactly this arithmetic, so a
/// caller that rebuilds a pick ray, a projection or an overlay from it is looking at the picture the
/// author is looking at rather than at a second camera that agrees today.
///
/// `min`/`max` are the model's own vertex bounds — what `check-scene <model>.mesh.yaml --bounds`
/// prints. `cell_w`/`cell_h` are the **cell's** pixels, not the sheet's: a one-view sheet
/// (`--views front`) makes the cell the whole image, and a six-view sheet divides it.
///
/// Framing is scale-blind, exactly as the sheet is — a document's declared `scale:` never moves it.
pub fn model_camera(
    view: ModelView,
    min: [f32; 3],
    max: [f32; 3],
    cell_w: u32,
    cell_h: u32,
) -> ModelCamera {
    let subject = Subject::from_box(min, max, 1.0);
    let rig = ViewRig::compute(view, &subject, cell_w, cell_h);
    let placement = &subject.placements[0];
    ModelCamera {
        translation: vec3_array(rig.position),
        rotation_deg: rig.rotation_deg,
        fov_y_deg: FOV_Y_DEG,
        near: NEAR,
        far: FAR,
        target: vec3_array(subject.target),
        normalize_scale: placement.scale,
        normalize_translation: placement.translation,
        label: view.label(),
    }
}

/// The camera distance `D` that fits the unit bounding sphere in a `cell_w` × `cell_h` cell (the
/// limiting half-angle is the smaller of the vertical and horizontal half-FOVs), with a 10% margin.
fn fit_distance(cell_w: u32, cell_h: u32) -> f32 {
    fit_distance_for_aspect(cell_w as f32 / cell_h as f32)
}

/// The camera distance that fits the framed unit sphere in a cell of this **aspect** — which is all
/// the fit depends on, and what lets `--true-size` fix the aspect first and solve for the cell size
/// afterwards. The limiting half-angle is the smaller of the vertical and horizontal half-FOVs, plus
/// a 10% framing margin.
pub fn fit_distance_for_aspect(aspect: f32) -> f32 {
    let half_y = (FOV_Y_DEG * 0.5).to_radians();
    // atan(tan(half_y) * aspect) — the horizontal half-angle.
    let half_x = math::atan2(math::tan(half_y) * aspect, 1.0);
    let limiting = half_y.min(half_x);
    FIT_MARGIN / math::sin(limiting)
}

/// Builds the scene for one cell: every normalized model, a ground plane, the camera, and the key +
/// fill lights, as typed components written by [`ludion_scene::emit`]. A pure function of the
/// inputs.
///
/// `label` is the text burned into the cell's top-left corner — the view's orientation on an
/// ordinary sheet, the swept value on a sweep sheet, where the orientation is the same in every cell
/// and the value is the only thing worth reading. `captions` name the individual models of a lineup
/// and is empty otherwise, which is what keeps a one-model sheet byte-identical.
///
/// A `lift` borrows the ground color, replaces the key + fill when it carries a light, and appends
/// its own entities after the labels.
///
/// `animations` is one entry per model, `Some` only for a posed sheet or a strip: the model entity
/// then carries an `Animation` and the caller extracts at the matching `now`.
///
/// A `channel` replaces the neutral rig with a diagnostic composition of this same scene — see
/// [`ModelChannel`]. `None` is the ordinary sheet, byte for byte.
#[allow(clippy::too_many_arguments)]
fn synthesize_scene(
    models: &[&ModelBounds],
    names: &[String],
    subject: &Subject,
    lift: Option<&RigLift>,
    view: ModelView,
    label: &str,
    captions: &[Caption],
    animations: &[Option<AnimationSpec>],
    cell_w: u32,
    cell_h: u32,
    channel: Option<ModelChannel>,
) -> String {
    let view_rig = ViewRig::compute(view, subject, cell_w, cell_h);

    let mut entities: Vec<ludion_scene::SceneEntity> = Vec::new();

    // The normalized models, in the order they were given — a row's own order.
    for (index, ((bounds, name), placement)) in models
        .iter()
        .zip(names)
        .zip(&subject.placements)
        .enumerate()
    {
        let shape = match &bounds.kind {
            ModelKind::Voxel => MeshSource::Voxel(name.clone()),
            ModelKind::Mesh => MeshSource::Asset(name.clone()),
            ModelKind::Assembly => MeshSource::Assembly(name.clone()),
            ModelKind::Sculpt => MeshSource::Sculpt(name.clone()),
            ModelKind::Character => MeshSource::Character(name.clone()),
            ModelKind::Prefab(root_part) => {
                entities.push(prefab_instance_entity(
                    index,
                    name,
                    root_part,
                    placement,
                    animations.get(index).copied().flatten(),
                ));
                continue;
            }
        };
        let scale = placement.scale;
        // The emissive term is `base × Mesh.emissive × map`, so a mesh document's map emits nothing
        // until something authors the intensity lane — and a standalone document has no scene to
        // author it in. Every sheet supplies a unit intensity, and only where there is a map to
        // show: a model with none is untouched, and stays black under the emissive channel.
        let mesh = Mesh {
            shape,
            emissive: if bounds.emissive_texture { 1.0 } else { 0.0 },
            ..Mesh::default()
        };
        let mut components = vec![
            typed(
                "Transform",
                &Transform {
                    translation: placement.translation,
                    scale: [scale; 3],
                    ..Transform::default()
                },
            ),
            typed("Mesh", &mesh),
        ];
        // `speed` stays at its default 1.0: the phase is chosen with `started`, not by playing the
        // clip faster, so a sheet never depends on a rate the file did not author.
        if let Some(Some(animation)) = animations.get(index) {
            components.push(typed("Animation", &animation_of(*animation)));
            if !animation.layers.is_empty() {
                components.push(typed(
                    "AnimationLayers",
                    &AnimationLayers {
                        layers: layers_of(animation.layers, animation.started),
                    },
                ));
            }
            if let Some(ik) = animation.ik {
                components.push(typed("IkTargets", &ik.targets(animation.look, placement)));
            }
        }
        entities.push(entity(components));
    }

    // The ground plane (gallery gray, or a rig's own floor color).
    let ground_color = lift
        .and_then(|lift| lift.ground_color)
        .unwrap_or([0.5, 0.55, 0.6, 1.0]);
    entities.push(entity(vec![
        typed(
            "Transform",
            &Transform {
                scale: [8.0, 1.0, 8.0],
                ..Transform::default()
            },
        ),
        typed(
            "Mesh",
            &Mesh {
                shape: MeshSource::Plane,
                color: ground_color,
                ..Mesh::default()
            },
        ),
    ]));

    // The camera.
    entities.push(entity(vec![
        typed(
            "Transform",
            &Transform {
                translation: vec3_array(view_rig.position),
                rotation: view_rig.rotation_deg,
                ..Transform::default()
            },
        ),
        typed(
            "Camera",
            &Camera {
                projection: Projection::Perspective {
                    fov_y_deg: FOV_Y_DEG,
                },
                near: NEAR,
                far: FAR,
                ..Camera::default()
            },
        ),
    ]));

    if channel == Some(ModelChannel::Emissive) {
        entities.push(emissive_channel_entity());
        return emit_entities(entities);
    }

    // Key light (neutral-warm) and fill light (dimmer, cool) — skipped whole when a rig brought its
    // own lighting.
    if !lift.is_some_and(|lift| lift.has_light) {
        entities.push(light_entity(
            view_rig.key_position,
            [1.0, 0.97, 0.9],
            KEY_INTENSITY,
            view_rig.range,
        ));
        entities.push(light_entity(
            view_rig.fill_position,
            [0.7, 0.78, 0.95],
            FILL_INTENSITY,
            view_rig.range,
        ));
    }

    // The burned-in label — a screen-space `UiText` in the cell's top-left corner, so an author
    // never again mistakes which face a cell shows. Warm yellow reads over both the gallery-gray
    // ground and a light model; the top-left corner keeps it clear of the centered model.
    entities.push(entity(vec![typed(
        "UiText",
        &UiText {
            anchor: Anchor::TopLeft,
            offset: [LABEL_OFFSET, LABEL_OFFSET],
            text: label.to_string(),
            size: LABEL_SIZE,
            color: [1.0, 0.9, 0.25, 1.0],
            ..UiText::default()
        },
    )]));

    // A lineup's per-model captions, each centered under the model it names.
    for caption in captions {
        entities.push(entity(vec![typed(
            "UiText",
            &UiText {
                anchor: Anchor::TopCenter,
                offset: caption.offset,
                text: caption.text.clone(),
                size: caption.size,
                color: [1.0, 0.9, 0.25, 1.0],
                ..UiText::default()
            },
        )]));
    }

    if let Some(lift) = lift {
        entities.extend(lift.entities.iter().cloned());
    }

    emit_entities(entities)
}

/// The canonical text of a scene holding exactly these entities.
pub(crate) fn emit_entities(entities: Vec<ludion_scene::SceneEntity>) -> String {
    let scene = ludion_scene::Scene {
        entities,
        ..Default::default()
    };
    ludion_scene::emit(&scene, &builtin_registry())
}

/// One component as its `(name, value)` pair — the only place a typed component is serialized.
pub(crate) fn typed<T: serde::Serialize>(name: &str, value: &T) -> (String, Value) {
    let value = serde_json::to_value(value).expect("a component serializes to JSON");
    (name.to_string(), value)
}

/// An unnamed entity carrying these components.
pub(crate) fn entity(components: Vec<(String, Value)>) -> ludion_scene::SceneEntity {
    ludion_scene::SceneEntity {
        components,
        ..Default::default()
    }
}

/// The `Animation` one model entity of a posed sheet carries.
fn animation_of(spec: AnimationSpec) -> Animation {
    let mut animation = Animation {
        clip: spec.clip.to_string(),
        started: FrameIndex::new(spec.started),
        params: spec.params,
        ..Animation::default()
    };
    if let Some(fade) = spec.fade {
        animation.from = fade.from.to_string();
        animation.from_started = FrameIndex::new(fade.from_started);
        animation.fade_started = FrameIndex::new(fade.fade_started);
        animation.fade = fade.seconds;
    }
    animation
}

/// One prefab instance entry: the normalizing placement on the entry, and a posed sheet's
/// `Animation` as an override of the root part, where playback reads it.
fn prefab_instance_entity(
    index: usize,
    file: &str,
    root_part: &str,
    placement: &Placement,
    animation: Option<AnimationSpec>,
) -> ludion_scene::SceneEntity {
    let translation = placement.translation.map(f64::from);
    let scale = f64::from(placement.scale);
    let mut instance = ludion_scene::InstanceEntry {
        prefab: file.to_string(),
        transform: ludion_scene::InstanceTransform {
            translation,
            scale: [scale; 3],
            ..Default::default()
        },
        ..Default::default()
    };
    if let Some(animation) = animation {
        // A partial value, not a typed `Animation`: the override merges over the root part's own
        // component, and a typed one would write `speed: 1.0` over an authored speed.
        let mut value = serde_json::json!({
            "clip": animation.clip,
            "started": animation.started,
        });
        if let Some(fade) = animation.fade {
            value["from"] = fade.from.into();
            value["from_started"] = fade.from_started.into();
            value["fade_started"] = fade.fade_started.into();
            value["fade"] = fade.seconds.into();
        }
        instance.overrides.insert(
            root_part.to_string(),
            vec![("Animation".to_string(), value)],
        );
    }
    ludion_scene::SceneEntity {
        name: Some(format!("phase-{index}")),
        instance: Some(Box::new(instance)),
        ..Default::default()
    }
}

/// [`ModelChannel::Emissive`]'s carrier entity: an `Environment` with no sky, no ambient and no
/// image-based lighting, and a `PostProcess` flattened to a pass-through.
///
/// Every term that could put light on a surface is zeroed here rather than in the renderer, so the
/// channel is a scene an author can read, `--dump-graph` and every other instrument still see the
/// frame graph they always see, and the one thing left that can brighten a pixel is emission.
///
/// The post stack is flattened for the same reason: bloom would bleed the mask past the surface it
/// came from, and a tonemap curve with the shipped `1.3` exposure would make "how bright" a question
/// about the curve. `none` + `1.0` + no dither leaves the scene pass's own linear value, clamped —
/// the mask, not a picture of it. (The other post steps are already neutral at their defaults:
/// vignette, SSAO, shafts and fog all ship at `0.0`.)
fn emissive_channel_entity() -> ludion_scene::SceneEntity {
    entity(vec![
        typed(
            "Environment",
            &Environment {
                sky_zenith: [0.0; 3],
                sky_horizon: [0.0; 3],
                sky_ground: [0.0; 3],
                ambient: [0.0; 3],
                ibl_intensity: 0.0,
                ..Environment::default()
            },
        ),
        typed(
            "PostProcess",
            &PostProcess {
                bloom_strength: 0.0,
                exposure: 1.0,
                tonemap: crate::components::TonemapOperator::None,
                dither: false,
                ..PostProcess::default()
            },
        ),
    ])
}

/// One point-light entity (a `Transform` at `position` plus a `PointLight`).
pub(crate) fn light_entity(
    position: Vec3,
    color: [f32; 3],
    intensity: f32,
    range: f32,
) -> ludion_scene::SceneEntity {
    entity(vec![
        typed(
            "Transform",
            &Transform {
                translation: vec3_array(position),
                ..Transform::default()
            },
        ),
        typed(
            "PointLight",
            &PointLight {
                color,
                intensity,
                range,
                ..PointLight::default()
            },
        ),
    ])
}

/// A `Vec3` as a plain `[x, y, z]` array (glam `Vec3` has no `Into<[f32; 3]>` in this context).
fn vec3_array(v: Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A prefab's bounds are the rest-pose box of its expanded parts: the hinge's lid sits at
    /// z = −0.4 and its latch at z = +0.4, so the box must enclose both.
    #[test]
    fn a_prefabs_bounds_enclose_all_its_parts() {
        let hinge = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/import-node-clips/expected/model.prefab.yaml");
        let loaded = load_model(&hinge).unwrap_or_else(|d| panic!("{d:#?}"));
        assert!(matches!(&loaded.bounds.kind, ModelKind::Prefab(root) if root == "base"));
        assert!(loaded.bounds.min[2] <= -0.4, "{:?}", loaded.bounds.min);
        assert!(loaded.bounds.max[2] >= 0.4, "{:?}", loaded.bounds.max);
    }

    #[test]
    fn a_crossfade_spec_parses_three_fields() {
        let spec = parse_crossfade("run:idle:0.25").expect("a well-formed spec");
        assert_eq!((spec.from, spec.to, spec.seconds), ("run", "idle", 0.25));
        assert_eq!(
            parse_crossfade("snap:snap:1").expect("from == to").to,
            "snap"
        );
    }

    /// Every malformed spec is its own `L1117` that quotes the input and shows the spelling.
    #[test]
    fn a_malformed_crossfade_spec_is_refused_with_its_input() {
        for spec in [
            "run",
            "run:idle",
            "run:idle:0.25:x",
            ":idle:0.25",
            "run::0.25",
            "run:idle:",
            "run:idle:fast",
            "run:idle:0",
            "run:idle:-1",
            "run:idle:inf",
            "run:idle:NaN",
        ] {
            let diagnostic = parse_crossfade(spec).expect_err(spec);
            assert_eq!(diagnostic.code, code::MODEL_SHEET_INPUT, "{spec}");
            assert!(
                diagnostic.message.contains(spec),
                "{spec}: {}",
                diagnostic.message
            );
            assert!(
                diagnostic
                    .help
                    .as_deref()
                    .is_some_and(|help| help.contains("--crossfade run:idle:0.25")),
                "{spec}"
            );
        }
    }

    #[test]
    fn a_blend_spec_parses_values_and_ranges() {
        let one = parse_blend("locomotion:0..2").expect("a range");
        assert_eq!(
            (one.name, one.p0, one.p1),
            ("locomotion", Sweep::Range(0.0, 2.0), None)
        );
        let two = parse_blend("aim:0.5:-1..1").expect("a value and a range");
        assert_eq!(
            (two.p0, two.p1),
            (Sweep::At(0.5), Some(Sweep::Range(-1.0, 1.0)))
        );
    }

    #[test]
    fn a_swept_parameter_is_linear_across_the_phases() {
        let sweep = Sweep::Range(0.0, 2.0);
        assert_eq!(
            (0..3).map(|i| sweep.at(i, 3)).collect::<Vec<_>>(),
            [0.0, 1.0, 2.0]
        );
        assert_eq!(Sweep::At(0.7).at(2, 5), 0.7);
        assert_eq!(sweep.at(0, 1), 0.0);
    }

    #[test]
    fn a_malformed_blend_spec_is_refused_with_its_input() {
        for spec in [
            "locomotion",
            "a:1:2:3",
            ":1",
            "a:",
            "a:x",
            "a:inf",
            "a:NaN..1",
            "a:0..",
            "a:..1",
            "a:0..1..2",
        ] {
            let diagnostic = parse_blend(spec).expect_err(spec);
            assert_eq!(diagnostic.code, code::MODEL_SHEET_INPUT, "{spec}");
            assert!(diagnostic.message.contains(spec), "{spec}");
            assert!(
                diagnostic
                    .help
                    .as_deref()
                    .is_some_and(|help| help.contains("--blend locomotion:0..2")),
                "{spec}"
            );
        }
    }

    #[test]
    fn a_layer_spec_parses_its_optional_fields() {
        let bare = parse_layer("wave:1").expect("clip and weight");
        assert_eq!(
            (bare.clip, bare.weight, bare.mask, bare.mode),
            ("wave", 1.0, None, LayerMode::Override)
        );
        let full = parse_layer("breathe:0.5:torso:additive").expect("every field");
        assert_eq!((full.mask, full.mode), (Some("torso"), LayerMode::Additive));
        let skipped = parse_layer("breathe:0.5::additive").expect("an empty mask");
        assert_eq!((skipped.mask, skipped.mode), (None, LayerMode::Additive));
    }

    #[test]
    fn a_look_spec_parses_names_numbers_and_ranges() {
        let bare = parse_look("40,-10").expect("two numbers");
        assert_eq!(
            bare,
            LookSpec {
                name: None,
                yaw: Sweep::At(40.0),
                pitch: Sweep::At(-10.0)
            }
        );
        let swept = parse_look("look=0..100,5").expect("a name and a range");
        assert_eq!(
            (swept.name, swept.yaw, swept.pitch),
            (Some("look"), Sweep::Range(0.0, 100.0), Sweep::At(5.0))
        );
        let both = parse_look("-20..20,-5..5").expect("two ranges");
        assert_eq!(
            (both.yaw, both.pitch),
            (Sweep::Range(-20.0, 20.0), Sweep::Range(-5.0, 5.0))
        );
    }

    #[test]
    fn a_malformed_look_spec_is_refused_with_its_input() {
        for spec in [
            "", "40", "40,0,1", "=40,0", "left,0", "40,NaN", "0..,0", "..5,0", "0..inf,0",
            "look=40",
        ] {
            let diagnostic = parse_look(spec).expect_err(spec);
            assert_eq!(diagnostic.code, code::MODEL_SHEET_INPUT, "{spec}");
            assert!(diagnostic.message.contains(spec), "{spec}");
            assert!(
                diagnostic
                    .help
                    .as_deref()
                    .is_some_and(|help| help.contains("--look 40,0")),
                "{spec}"
            );
        }
    }

    #[test]
    fn an_ik_spec_parses_a_name_and_a_point() {
        let spec = parse_ik("hand_r=6,12.5,-2").expect("a point");
        assert_eq!((spec.name, spec.point), ("hand_r", [6.0, 12.5, -2.0]));
        assert_eq!(parse_ik("a=0,0,0").expect("the origin").point, [0.0; 3]);
    }

    #[test]
    fn a_malformed_ik_spec_is_refused_with_its_input() {
        for spec in [
            "hand_r",
            "=1,2,3",
            "hand_r=1,2",
            "hand_r=1,2,3,4",
            "hand_r=1,two,3",
            "hand_r=1,2,inf",
            "hand_r=0..1,2,3",
        ] {
            let diagnostic = parse_ik(spec).expect_err(spec);
            assert_eq!(diagnostic.code, code::MODEL_SHEET_INPUT, "{spec}");
            assert!(diagnostic.message.contains(spec), "{spec}");
            assert!(
                diagnostic
                    .help
                    .as_deref()
                    .is_some_and(|help| help.contains("--ik hand_r=6,12,2")),
                "{spec}"
            );
        }
    }

    /// The gallery humanoid: a `look` look-at and a `hand_r` chain, both over a clip it binds.
    fn humanoid_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gallery/voxels/humanoid.assembly.yaml")
    }

    /// Resolves `request` against the gallery humanoid's `idle` clip, CPU only.
    fn humanoid_posed(request: &ModelSheetRequest) -> Result<Posed, Vec<Diagnostic>> {
        let path = humanoid_path();
        let loaded = load_model(&path).unwrap_or_else(|d| panic!("{d:#?}"));
        resolve_clip(request, &path, &loaded, "idle")
    }

    /// A strip sweeping the look past its limit reports each phase's own aim, and every phase sits at
    /// the one `--time`.
    #[test]
    fn a_look_sweep_is_a_strip_of_looks_at_one_time() {
        let path = humanoid_path();
        let models = [path.as_path()];
        let look = parse_look("look=0..100,0").expect("a sweep");
        let request = ModelSheetRequest {
            clip: Some("idle"),
            time: Some(0.5),
            strip: Some(6),
            look: Some(look),
            ..ModelSheetRequest::new(&models, 900, 600)
        };
        assert!(sweeps_look(&request));
        let posed = humanoid_posed(&request).expect("the sweep resolves");
        for (report, yaw) in posed
            .reports
            .iter()
            .zip([0.0, 20.0, 40.0, 60.0, 80.0, 100.0])
        {
            let [got, pitch] = report.look.expect("every phase aims");
            assert!((got - yaw).abs() < 1e-4 && pitch == 0.0, "{got} vs {yaw}");
        }
        assert!(posed.reports.iter().all(|r| r.tick == 30 && r.time == 0.5));
        assert!(posed.reports.iter().all(|r| r.components == Some(1)));
        assert_eq!(posed.started.len(), 6);
    }

    /// Without a range the strip keeps the clip's own phases and aims every one the same way.
    #[test]
    fn a_plain_look_does_not_fan_a_strip_out() {
        let path = humanoid_path();
        let models = [path.as_path()];
        let request = ModelSheetRequest {
            clip: Some("idle"),
            look: Some(parse_look("30,10").expect("a look")),
            ..ModelSheetRequest::new(&models, 900, 600)
        };
        assert!(!sweeps_look(&request));
        let posed = humanoid_posed(&request).expect("a posed sheet");
        assert_eq!(posed.reports.len(), 1);
        assert_eq!(posed.reports[0].look, Some([30.0, 10.0]));
    }

    /// The report's pose is `part_placements_sides` plus the one solver: phases past the yaw limit
    /// pose identically, and a phase inside it does not.
    #[test]
    fn a_look_past_its_limit_poses_like_the_limit() {
        let path = humanoid_path();
        let models = [path.as_path()];
        let request = ModelSheetRequest {
            clip: Some("idle"),
            look: Some(parse_look("0,0").expect("a look")),
            ..ModelSheetRequest::new(&models, 900, 600)
        };
        let loaded = load_model(&path).unwrap_or_else(|d| panic!("{d:#?}"));
        let source = ClipSource::load(&path, &loaded).expect("loads");
        let ClipSource::Assembly(posing) = &source else {
            panic!("the humanoid is an assembly");
        };
        let plan = resolve_ik(&source, "assembly", &request).expect("a bare look resolves");
        assert_eq!(
            plan.look.as_ref().map(|(_, name)| name.as_str()),
            Some("look")
        );
        let rest =
            || part_placements_sides(&posing.data, AssemblySide::single(None), None, true, &[]);
        let pose = |yaw: f32| {
            let mut placed = rest();
            apply_assembly_ik(&posing.data, &plan.goals(Some([yaw, 0.0])), &mut placed);
            placed
        };
        assert_eq!(pose(80.0), pose(100.0), "both clamp at the 70 degree limit");
        assert_eq!(pose(70.0), pose(100.0));
        assert_ne!(pose(40.0), pose(100.0));
        assert_ne!(pose(0.0), pose(40.0));
    }

    /// A chain target is written in assembly-grid voxels and the scene carries it in world space:
    /// `translation + scale * (x - sx/2, y, z - sz/2)`, the same point the report solved.
    #[test]
    fn an_ik_target_reaches_the_scene_in_the_frame_the_report_solved() {
        let path = humanoid_path();
        let models = [path.as_path()];
        let ik = [parse_ik("hand_r=6,12,2").expect("a point")];
        let request = ModelSheetRequest {
            clip: Some("idle"),
            ik: &ik,
            ..ModelSheetRequest::new(&models, 900, 600)
        };
        let posed = humanoid_posed(&request).expect("a posed sheet");
        let [(entry, name, point)] = &posed.ik.chains[..] else {
            panic!("one chain");
        };
        assert_eq!(name, "hand_r");
        // The humanoid is 8 x 16 x 2: the assembly frame sits at the bottom centre.
        assert_eq!(*point, [2.0, 12.0, 1.0]);
        assert_eq!(
            posed.ik.goals(None),
            [IkGoal {
                entry: *entry,
                aim: IkAim::Point([2.0, 12.0, 1.0]),
                weight: 1.0
            }]
        );

        let placement = Placement {
            translation: [0.5, 0.0, -1.0],
            scale: 0.25,
            base: Vec3::ZERO,
        };
        let targets = posed.ik.targets(None, &placement);
        assert_eq!(
            targets.targets["hand_r"].point,
            Some([0.5 + 0.25 * 2.0, 0.25 * 12.0, -1.0 + 0.25 * 1.0])
        );
        assert_eq!(targets.targets["hand_r"].weight, 1.0);
        assert!(!targets.targets.contains_key("look"));
    }

    /// The synthesized scene carries an `IkTargets` only when the request has IK, and the look is a
    /// `direction` in `Look`'s convention.
    #[test]
    fn the_scene_carries_ik_targets_only_when_asked() {
        let bounds = ModelBounds {
            kind: ModelKind::Assembly,
            ..baseline_bounds()
        };
        let subject = Subject::single(&bounds);
        let names = [bounds.file_name.clone()];
        let plan = IkPlan {
            chains: vec![(1, "hand_r".to_string(), [1.0, 2.0, 3.0])],
            look: Some((0, "look".to_string())),
        };
        let scene_of = |ik: Option<&IkPlan>| {
            let animation = AnimationSpec {
                clip: "idle",
                started: 0,
                fade: None,
                params: [0.0; 2],
                layers: &[],
                ik,
                look: Some([40.0, -5.0]),
            };
            synthesize_scene(
                &[&bounds],
                &names,
                &subject,
                None,
                ModelView::Front,
                "front",
                &[],
                &[Some(animation)],
                200,
                200,
                None,
            )
        };
        assert!(!scene_of(None).contains("IkTargets"));
        let yaml = scene_of(Some(&plan));
        let scene = ludion_scene::parse(&yaml).expect("the sheet parses");
        let model = &scene.entities[0];
        let ik = model
            .components
            .iter()
            .find(|(name, _)| name == "IkTargets")
            .map(|(_, value)| value)
            .unwrap_or_else(|| panic!("an IkTargets component in {yaml}"));
        assert_eq!(ik["targets"]["look"]["direction"]["yaw_deg"], 40.0);
        assert_eq!(ik["targets"]["look"]["direction"]["pitch_deg"], -5.0);
        assert_eq!(ik["targets"]["look"]["weight"], 1.0);
        assert!(ik["targets"]["hand_r"]["point"].is_array());
    }

    /// Every way `--look` and `--ik` can misname what they drive is an `L1117` that lists the entries.
    #[test]
    fn look_and_ik_refusals_name_what_the_assembly_declares() {
        let path = humanoid_path();
        let models = [path.as_path()];
        let refused = |look: Option<&str>, ik: &[&str]| -> Diagnostic {
            let looks = look.map(|spec| parse_look(spec).expect("a look"));
            let iks: Vec<IkSpec<'_>> = ik
                .iter()
                .map(|spec| parse_ik(spec).expect("an ik"))
                .collect();
            let request = ModelSheetRequest {
                clip: Some("idle"),
                look: looks,
                ik: &iks,
                ..ModelSheetRequest::new(&models, 900, 600)
            };
            let mut errors = humanoid_posed(&request).expect_err("a refusal");
            assert_eq!(errors.len(), 1, "{errors:?}");
            let error = errors.remove(0);
            assert_eq!(error.code, code::MODEL_SHEET_INPUT, "{error:?}");
            error
        };
        let unknown = refused(None, &["hand_l=0,0,0"]);
        assert!(unknown.message.contains("`hand_r`") && unknown.message.contains("`look`"));
        let help = unknown.help.as_deref().unwrap_or_default();
        assert!(help.contains("nearest match is `hand_r`"), "{help}");
        let wrong = refused(None, &["look=0,0,0"]);
        assert!(
            wrong.message.contains("names a look-at"),
            "{}",
            wrong.message
        );
        let wrong = refused(Some("hand_r=0,0"), &[]);
        assert!(wrong.message.contains("names a chain"), "{}", wrong.message);
        let unknown = refused(Some("gaze=0,0"), &[]);
        assert!(unknown.message.contains("names no `ik:` entry"));
    }

    /// `--look` and `--ik` aim over a base pose, and not alongside a swept blend.
    #[test]
    fn look_and_ik_flag_shapes_are_checked_before_anything_loads() {
        let path = humanoid_path();
        let models = [path.as_path()];
        let look = parse_look("look=0..90,0").expect("a sweep");
        let ik = [parse_ik("hand_r=1,2,3").expect("a point")];
        let bare = ModelSheetRequest {
            look: Some(look),
            ..ModelSheetRequest::new(&models, 900, 600)
        };
        let errors = check_clip_flags(&bare).expect_err("no base pose");
        assert_eq!(errors[0].code, code::MODEL_SHEET_INPUT);
        assert!(errors[0].message.contains("`--look` needs a base pose"));
        let bare = ModelSheetRequest {
            ik: &ik,
            ..ModelSheetRequest::new(&models, 900, 600)
        };
        let errors = check_clip_flags(&bare).expect_err("no base pose");
        assert!(errors[0].message.contains("`--ik` needs a base pose"));

        let both = ModelSheetRequest {
            blend: Some(parse_blend("locomotion:0..2").expect("a sweep")),
            look: Some(look),
            ..ModelSheetRequest::new(&models, 900, 600)
        };
        let errors = check_clip_flags(&both).expect_err("two sweeps");
        assert!(errors[0].message.contains("both sweep the strip"));
        let held = ModelSheetRequest {
            blend: Some(parse_blend("locomotion:1").expect("a value")),
            look: Some(look),
            ..ModelSheetRequest::new(&models, 900, 600)
        };
        assert!(check_clip_flags(&held).is_ok());
    }

    /// A character has no `ik:`, so it refuses `--look`, naming why.
    #[test]
    fn a_character_refuses_look() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gallery/characters/mannequin.character.yaml");
        let loaded = load_model(&path).unwrap_or_else(|d| panic!("{d:#?}"));
        let models = [path.as_path()];
        let look = parse_look("0,0").expect("a look");
        let request = ModelSheetRequest {
            look: Some(look),
            ..ModelSheetRequest::new(&models, 900, 600)
        };
        let errors = resolve_clip(&request, &path, &loaded, "idle").expect_err("a refusal");
        assert_eq!(errors[0].code, code::MODEL_SHEET_INPUT);
        assert!(
            errors[0]
                .message
                .contains("only an assembly declares `ik:`"),
            "{}",
            errors[0].message
        );
    }

    #[test]
    fn a_malformed_layer_spec_is_refused_with_its_input() {
        for spec in [
            "wave",
            ":1",
            "wave:",
            "wave:heavy",
            "wave:1.5",
            "wave:-0.1",
            "wave:NaN",
            "wave:1:m:replace",
            "wave:1:m:override:x",
        ] {
            let diagnostic = parse_layer(spec).expect_err(spec);
            assert_eq!(diagnostic.code, code::MODEL_SHEET_INPUT, "{spec}");
            assert!(diagnostic.message.contains(spec), "{spec}");
            assert!(
                diagnostic
                    .help
                    .as_deref()
                    .is_some_and(|help| help.contains("--layer wave:1.0:arm_only")),
                "{spec}"
            );
        }
    }

    /// A looping clip's strip divides by `n`, so its last phase is **not** its first repeated — on a
    /// loop the two are the same pose, and a strip that showed both would waste a cell.
    #[test]
    fn a_looping_strip_never_repeats_the_first_phase() {
        assert_eq!(
            phase_times(1.2, true, Some(4), None),
            vec![0.0, 0.3, 0.6, 0.8999999999999999]
        );
    }

    /// A one-shot's strip divides by `n − 1`, so both endpoints show: a one-shot's final pose is the
    /// thing it exists to reach, and a strip that stopped short of it would hide the payload.
    #[test]
    fn a_one_shot_strip_shows_both_endpoints() {
        let times = phase_times(1.2, false, Some(4), None);
        assert_eq!(times.first(), Some(&0.0));
        assert_eq!(times.last(), Some(&1.2));
        assert_eq!(times.len(), 4);
    }

    /// Without a strip there is exactly one phase, at the requested instant (`0.0` by default).
    #[test]
    fn a_posed_sheet_is_one_phase_at_the_requested_time() {
        assert_eq!(phase_times(1.2, true, None, Some(0.4)), vec![0.4]);
        assert_eq!(phase_times(1.2, true, None, None), vec![0.0]);
    }

    /// Clip time is only ever a whole number of sim ticks, so a request lands on the nearest one —
    /// and anything that is not a positive finite number lands on tick 0 rather than on a saturating
    /// cast.
    #[test]
    fn clip_time_quantizes_to_the_nearest_tick() {
        let rate = TickRate::DEFAULT;
        assert_eq!(quantize(0.4, rate), 24);
        assert_eq!(quantize(0.401, rate), 24);
        assert_eq!(quantize(0.4084, rate), 25);
        assert_eq!(quantize(0.0, rate), 0);
        assert_eq!(quantize(-1.0, rate), 0);
        assert_eq!(quantize(f64::NAN, rate), 0);
        assert_eq!(quantize(f64::INFINITY, rate), 0);
    }

    /// An empty request expands to the default six views (not the opt-in `game`), in canonical order.
    #[test]
    fn empty_request_is_the_default_six() {
        assert_eq!(canonical_views(&[]), ModelView::DEFAULT.to_vec());
        assert!(
            !canonical_views(&[]).contains(&ModelView::Game),
            "the steep game view is opt-in, never in the default sheet"
        );
    }

    /// The opt-in `game` view renders only when explicitly requested, and sorts last in canonical
    /// order.
    #[test]
    fn game_view_is_opt_in_and_sorts_last() {
        assert_eq!(
            canonical_views(&[ModelView::Game, ModelView::Front]),
            vec![ModelView::Front, ModelView::Game],
        );
    }

    /// A request is deduplicated and sorted into canonical order regardless of input order.
    #[test]
    fn views_are_deduped_and_canonically_ordered() {
        let requested = [
            ModelView::Iso,
            ModelView::Front,
            ModelView::Iso,
            ModelView::Left,
            ModelView::Front,
        ];
        assert_eq!(
            canonical_views(&requested),
            vec![ModelView::Front, ModelView::Left, ModelView::Iso],
        );
    }

    /// `name()` is the lowercase token, in canonical order.
    #[test]
    fn names_are_the_lowercase_tokens() {
        let names: Vec<&str> = ModelView::ALL.iter().map(|v| v.name()).collect();
        assert_eq!(
            names,
            ["front", "back", "left", "right", "top", "iso", "game"]
        );
    }

    /// The grid rule: 1 row for up to three views, 2 rows above, `cols = ceil(N/rows)`.
    #[test]
    fn grid_rule_for_one_through_six() {
        let cases = [
            (1u32, 1u32, 1u32),
            (2, 2, 1),
            (3, 3, 1),
            (4, 2, 2),
            (5, 3, 2),
            (6, 3, 2),
        ];
        for (n, cols, rows) in cases {
            let layout = SheetLayout::compute(n, 600, 400);
            assert_eq!(layout.cols, cols, "cols for N={n}");
            assert_eq!(layout.sheet_h / layout.cell_h, rows, "rows for N={n}");
        }
    }

    /// Cell and sheet dimensions: cells divide the requested size, the sheet is exactly the cell grid.
    #[test]
    fn cell_and_sheet_dimensions() {
        // N=6 → 3×2 grid; 640/3 = 213, 360/2 = 180.
        let layout = SheetLayout::compute(6, 640, 360);
        assert_eq!((layout.cell_w, layout.cell_h), (213, 180));
        assert_eq!(layout.sheet_w, 213 * 3);
        assert_eq!(layout.sheet_h, 180 * 2);
        assert!(layout.sheet_w <= 640 && layout.sheet_h <= 360);

        // N=1 → the whole sheet is one cell.
        let single = SheetLayout::compute(1, 512, 384);
        assert_eq!((single.cell_w, single.cell_h), (512, 384));
        assert_eq!((single.sheet_w, single.sheet_h), (512, 384));
    }

    /// An `env-night`-shaped rig: an `Environment` + `PostProcess` carrier, a `moon`
    /// `DirectionalLight`, a `ground` plane whose color is borrowed, and a `subject` that must not
    /// be lifted.
    const NIGHT_RIG: &str = "\
format: 1
entities:
  - name: environment
    components:
      Environment:
        sky_zenith:
          - 0.01
          - 0.02
          - 0.055
        ibl_intensity: 0.9
      PostProcess:
        exposure: 0.6
        vignette: 0.82
  - name: moon
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - -56.0
      DirectionalLight:
        direction:
          - 0.32
          - -0.45
          - 0.83
        intensity: 0.8
  - name: ground
    components:
      Transform:
        scale:
          - 15.0
          - 1.0
          - 15.0
      Mesh:
        shape: plane
        color:
          - 0.3
          - 0.29
          - 0.27
          - 1.0
  - name: subject
    components:
      Mesh:
        shape:
          voxel: \"voxels/statue.voxel.yaml\"
";

    /// The exact synthesized scene of one view with no rig — pinned so `--rig` cannot silently move
    /// the default sheet. Every existing model golden is a function of this text.
    const BASELINE_FRONT: &str = "\
format: 1
entities:
  - components:
      Transform:
        translation:
          - -0.0
          - -0.0
          - -0.0
        scale:
          - 0.26726124
          - 0.26726124
          - 0.26726124
      Mesh:
        shape:
          voxel: \"l-shape.voxel.yaml\"
  - components:
      Transform:
        scale:
          - 8.0
          - 1.0
          - 8.0
      Mesh:
        shape: plane
        color:
          - 0.5
          - 0.55
          - 0.6
          - 1.0
  - components:
      Transform:
        translation:
          - 0.0
          - 0.8017837
          - -2.8744385
        rotation:
          - -180.0
          - 0.0
          - 180.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 45.0
        near: 0.05
        far: 100.0
  - components:
      Transform:
        translation:
          - 2.2995508
          - 3.3887782
          - -2.2995508
      PointLight:
        color:
          - 1.0
          - 0.97
          - 0.9
        intensity: 3.0
        range: 14.372192
  - components:
      Transform:
        translation:
          - -2.5869946
          - -0.92287946
          - -2.2995508
      PointLight:
        color:
          - 0.7
          - 0.78
          - 0.95
        intensity: 1.5
        range: 14.372192
  - components:
      UiText:
        anchor: top_left
        offset:
          - 8.0
          - 8.0
        text: \"FRONT -Z\"
        size: 52.0
        color:
          - 1.0
          - 0.9
          - 0.25
          - 1.0
";

    /// The 4×6×2 L-shape bounds the pinned baseline was captured from.
    fn baseline_bounds() -> ModelBounds {
        ModelBounds {
            kind: ModelKind::Voxel,
            min: [-2.0, 0.0, -1.0],
            max: [2.0, 6.0, 1.0],
            file_name: "l-shape.voxel.yaml".to_string(),
            scale: 1.0,
            emissive_texture: false,
        }
    }

    /// Lifts `source` as a rig, panicking on any diagnostic.
    fn lift_source(source: &str) -> RigLift {
        let scene = ludion_scene::check(source, &builtin_registry()).expect("valid rig scene");
        lift(&scene, Path::new("rig.yaml")).expect("liftable rig")
    }

    /// Synthesizes the front view of the baseline model, optionally under a rig.
    fn front(lift: Option<&RigLift>) -> String {
        front_of(&baseline_bounds(), lift, None)
    }

    /// [`front`] over an explicit model and channel.
    fn front_of(
        bounds: &ModelBounds,
        lift: Option<&RigLift>,
        channel: Option<ModelChannel>,
    ) -> String {
        let subject = Subject::single(bounds);
        let names = [bounds.file_name.clone()];
        synthesize_scene(
            &[bounds],
            &names,
            &subject,
            lift,
            ModelView::Front,
            ModelView::Front.label(),
            &[],
            &[None],
            200,
            200,
            channel,
        )
    }

    /// The `WorldHash` of the world a scene text instantiates to.
    fn world_hash_of(yaml: &str) -> ludion_sim::WorldHash {
        let registry = builtin_registry();
        let scene = ludion_scene::check(yaml, &registry).expect("a valid scene");
        let world = ludion_scene::instantiate(&scene, &registry).expect("it instantiates");
        let driver = SimDriver::new(world, Schedule::new(), SimConfig::default());
        ludion_sim::WorldHash::compute(driver.world(), &registry).expect("the world hashes")
    }

    /// **The no-rig path is untouched.** With `--rig` absent the synthesized scene is the same world
    /// the hand-written text of [`BASELINE_FRONT`] spelled before the writer was typed: the text may
    /// spell a default or a float differently, the world may not differ.
    #[test]
    fn no_rig_synthesizes_the_pinned_scene() {
        assert_eq!(world_hash_of(&front(None)), world_hash_of(BASELINE_FRONT));
    }

    /// **A prefab override stays partial.** It merges over the root part's own `Animation`, so a
    /// typed one would write `speed: 1.0` over an authored speed and move the render.
    #[test]
    fn a_prefab_override_stays_partial() {
        let bounds = ModelBounds {
            kind: ModelKind::Prefab("base".to_string()),
            ..baseline_bounds()
        };
        let subject = Subject::single(&bounds);
        let names = [bounds.file_name.clone()];
        let keys = |animation: AnimationSpec| -> Vec<String> {
            let yaml = synthesize_scene(
                &[&bounds],
                &names,
                &subject,
                None,
                ModelView::Front,
                "front",
                &[],
                &[Some(animation)],
                200,
                200,
                None,
            );
            let scene = ludion_scene::parse(&yaml).expect("the sheet parses");
            let instance = scene.entities[0].instance.as_ref().expect("a prefab entry");
            let (_, components) = instance.overrides.iter().next().expect("one override");
            let (name, value) = &components[0];
            assert_eq!(name, "Animation");
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        let posed = AnimationSpec {
            clip: "open",
            started: 12,
            fade: None,
            params: [0.0; 2],
            layers: &[],
            ik: None,
            look: None,
        };
        assert_eq!(keys(posed), ["clip", "started"]);
        let fading = AnimationSpec {
            fade: Some(FadeSpec {
                from: "snap",
                from_started: 3,
                fade_started: 9,
                seconds: 0.25,
            }),
            ..posed
        };
        assert_eq!(
            keys(fading),
            [
                "clip",
                "fade",
                "fade_started",
                "from",
                "from_started",
                "started"
            ]
        );
    }

    /// **A non-finite placement names its field.** A typed `f32` NaN serializes to `null`, which the
    /// scene check refuses at the component's own pointer instead of writing text that parses wrong.
    #[test]
    fn a_non_finite_placement_names_its_field() {
        let bounds = baseline_bounds();
        let mut subject = Subject::single(&bounds);
        subject.placements[0].translation[0] = f32::NAN;
        let names = [bounds.file_name.clone()];
        let yaml = synthesize_scene(
            &[&bounds],
            &names,
            &subject,
            None,
            ModelView::Front,
            "front",
            &[],
            &[None],
            200,
            200,
            None,
        );
        let diagnostics = ludion_scene::check(&yaml, &builtin_registry()).unwrap_err();
        assert!(
            diagnostics.iter().any(|d| d
                .path
                .as_deref()
                .is_some_and(|path| path.ends_with("/Transform/translation/0"))),
            "{diagnostics:#?}"
        );
    }

    /// **The loader and the kind table agree.** `KINDS.model_sheet` is what the testkit routes golden
    /// scenes on; `load_model`'s own match is the other half. A row the table calls drawable must not
    /// be refused as an unsupported kind, and one it does not must be.
    #[test]
    fn the_loader_agrees_with_the_kind_table() {
        let dir = stage_scratch("kind-table");
        for entry in ludion_scene::KINDS {
            let path = dir.join(format!("stub.{}.yaml", entry.name));
            std::fs::write(&path, format!("format: 1\nkind: {}\n", entry.name)).unwrap();
            let refused = match load_model(&path) {
                Ok(_) => false,
                // The retired `character` kind is refused with its own `L1246` instead.
                Err(diagnostics) => diagnostics.iter().any(|d| {
                    d.message.contains(&format!("has kind `{}`", entry.name))
                        || d.code == ludion_core::codes::code::CHARACTER_KIND_MERGED
                }),
            };
            assert_eq!(
                refused, !entry.model_sheet,
                "kind `{}`: model_sheet = {}, loader refused as unsupported = {refused}",
                entry.name, entry.model_sheet
            );
        }
    }

    /// A mesh model's bounds, with or without an emissive map.
    fn mesh_bounds(emissive_texture: bool) -> ModelBounds {
        ModelBounds {
            kind: ModelKind::Mesh,
            min: [-1.0, 0.0, -1.0],
            max: [1.0, 2.0, 1.0],
            file_name: "idol.mesh.yaml".to_string(),
            scale: 1.0,
            emissive_texture,
        }
    }

    /// **`--channel emissive` is a mask, not a picture.** No light of any kind, a zeroed sky and
    /// ambient, a pass-through post stack, and no burned-in caption — so every non-black pixel came
    /// out of the emissive term.
    #[test]
    fn the_emissive_channel_zeroes_every_other_contribution() {
        let yaml = front_of(&mesh_bounds(true), None, Some(ModelChannel::Emissive));
        assert!(!yaml.contains("PointLight"), "no lights: {yaml}");
        assert!(!yaml.contains("UiText"), "no caption: {yaml}");
        for zeroed in ["sky_zenith", "sky_horizon", "sky_ground", "ambient"] {
            let block =
                format!("        {zeroed}:\n          - 0.0\n          - 0.0\n          - 0.0\n");
            assert!(yaml.contains(&block), "{zeroed} is zero: {yaml}");
        }
        assert!(yaml.contains("        ibl_intensity: 0.0\n"), "{yaml}");
        assert!(yaml.contains("        bloom_strength: 0.0\n"), "{yaml}");
        assert!(yaml.contains("        exposure: 1.0\n"), "{yaml}");
        assert!(yaml.contains("        tonemap: none\n"), "{yaml}");
        assert!(yaml.contains("        dither: false\n"), "{yaml}");
        ludion_scene::check(&yaml, &builtin_registry()).expect("the channel scene is valid");
    }

    /// **The intensity lane, on every sheet and only where there is a map.** `base × Mesh.emissive ×
    /// map` means a map with no intensity emits nothing, and an intensity with no map would light
    /// the whole model — so every sheet supplies `1.0` exactly when the document references a map.
    #[test]
    fn a_mapped_mesh_gets_unit_emissive_on_every_sheet() {
        for channel in [None, Some(ModelChannel::Emissive)] {
            let mapped = front_of(&mesh_bounds(true), None, channel);
            assert!(
                mapped.contains("        emissive: 1.0\n"),
                "{channel:?}: {mapped}"
            );
            let bare = front_of(&mesh_bounds(false), None, channel);
            assert!(!bare.contains("emissive: 1.0"), "{channel:?}: {bare}");
        }
    }

    /// **The public camera is the sheet's camera.** [`model_camera`] and the cell `render_model_sheet`
    /// actually draws must never be two derivations that merely agree today: a `mesh pick` built on
    /// the first would then report hits for a picture nobody rendered, and the divergence would be
    /// invisible. Checked over every view at two cell aspects.
    #[test]
    fn the_public_camera_is_the_sheet_camera() {
        let bounds = baseline_bounds();
        let names = [bounds.file_name.clone()];
        let subject = Subject::single(&bounds);
        for (cell_w, cell_h) in [(1920u32, 1080u32), (512, 512)] {
            for view in ModelView::ALL {
                let yaml = synthesize_scene(
                    &[&bounds],
                    &names,
                    &subject,
                    None,
                    view,
                    view.label(),
                    &[],
                    &[None],
                    cell_w,
                    cell_h,
                    None,
                );
                let scene =
                    ludion_scene::check(&yaml, &builtin_registry()).expect("a valid cell scene");
                let drawn = scene_camera(&scene);
                let public = model_camera(view, bounds.min, bounds.max, cell_w, cell_h);
                assert_eq!(
                    drawn,
                    (
                        public.translation,
                        public.rotation_deg,
                        public.fov_y_deg,
                        public.near,
                        public.far
                    ),
                    "{} at {cell_w}x{cell_h}",
                    view.name()
                );
                assert_eq!(public.label, view.label());
            }
        }
        // The normalize transform is the one the model entity is actually drawn with.
        let public = model_camera(ModelView::Front, bounds.min, bounds.max, 512, 512);
        assert_eq!(public.normalize_scale, subject.placements[0].scale);
        assert_eq!(
            public.normalize_translation,
            subject.placements[0].translation
        );
    }

    /// Reads the camera entity out of a synthesized cell scene: `(translation, rotation, fov, near,
    /// far)` — the tuple `ludion::math::camera_ray` consumes.
    fn scene_camera(scene: &ludion_scene::Scene) -> ([f32; 3], [f32; 3], f32, f32, f32) {
        let entity = scene
            .entities
            .iter()
            .find(|entity| entity.components.iter().any(|(name, _)| name == "Camera"))
            .expect("the cell has a camera");
        let component = |want: &str| {
            &entity
                .components
                .iter()
                .find(|(name, _)| name == want)
                .expect("component")
                .1
        };
        let vec3 = |value: &Value| -> [f32; 3] {
            std::array::from_fn(|i| value[i].as_f64().expect("a float") as f32)
        };
        let transform = component("Transform");
        let camera = component("Camera");
        (
            vec3(&transform["translation"]),
            vec3(&transform["rotation"]),
            camera["projection"]["fov_y_deg"].as_f64().expect("fov") as f32,
            camera["near"].as_f64().expect("near") as f32,
            camera["far"].as_f64().expect("far") as f32,
        )
    }

    /// A rig with a light replaces the synthesized key + fill and brings its own grade along.
    #[test]
    fn rig_replaces_the_synthesized_lights() {
        let lift = lift_source(NIGHT_RIG);
        assert!(lift.has_light, "a `DirectionalLight` is a light");

        let yaml = front(Some(&lift));
        assert!(yaml.contains("      Environment:\n"), "{yaml}");
        assert!(yaml.contains("      PostProcess:\n"), "{yaml}");
        assert!(yaml.contains("      DirectionalLight:\n"), "{yaml}");
        assert!(
            !yaml.contains("intensity: 3.0") && !yaml.contains("intensity: 1.5"),
            "the synthesized key/fill must be gone: {yaml}"
        );
        assert!(
            !yaml.contains("voxels/statue.voxel.yaml"),
            "a rig entity with no liftable component must not be copied: {yaml}"
        );
        // The rig's own scene checks out as the sheet's does — the appended blocks are canonical.
        ludion_scene::check(&yaml, &builtin_registry()).expect("the rigged sheet scene is valid");
    }

    /// A rig's `ground` color replaces the sheet's gallery gray; the plane keeps the sheet's own
    /// transform.
    #[test]
    fn rig_ground_color_is_adopted() {
        let lift = lift_source(NIGHT_RIG);
        assert_eq!(lift.ground_color, Some([0.3, 0.29, 0.27, 1.0]));

        let yaml = front(Some(&lift));
        assert!(
            yaml.contains("        color:\n          - 0.3\n          - 0.29\n"),
            "{yaml}"
        );
        assert!(
            !yaml.contains("          - 0.55\n          - 0.6\n"),
            "the gallery gray must be gone: {yaml}"
        );
        assert!(
            yaml.contains("        scale:\n          - 8.0\n          - 1.0\n          - 8.0\n"),
            "the ground keeps the sheet's own transform: {yaml}"
        );
    }

    /// A grade-only rig (no light) keeps the synthesized key + fill — there is nothing to replace
    /// them with.
    #[test]
    fn rig_without_lights_keeps_the_synthesized_rig() {
        let lift = lift_source(
            "format: 1\nentities:\n  - name: grade\n    components:\n      PostProcess:\n        \
             exposure: 0.6\n",
        );
        assert!(!lift.has_light);
        let yaml = front(Some(&lift));
        assert!(yaml.contains("intensity: 3.0"), "{yaml}");
        assert!(yaml.contains("      PostProcess:\n"), "{yaml}");
    }

    /// A rig scene holding none of the four component types is an `L1117` naming all four.
    #[test]
    fn rig_without_liftable_entities_is_l1117() {
        let scene = ludion_scene::check(
            "format: 1\nentities:\n  - name: subject\n    components:\n      Mesh:\n        \
             shape: plane\n",
            &builtin_registry(),
        )
        .expect("valid scene");
        let errors = lift(&scene, Path::new("bare.yaml")).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code.as_str(), "L1117");
        for component in RIG_COMPONENTS {
            assert!(
                errors[0].message.contains(component),
                "the message must name `{component}`: {}",
                errors[0].message
            );
        }
    }

    /// The bounds of a voxel model of `size` cells declaring `scale`.
    fn voxel_bounds(size: [u32; 3], scale: f32) -> ModelBounds {
        let doc = VoxelDocument {
            size,
            ..VoxelDocument::default()
        };
        let (min, max) = voxel_aabb(&doc);
        ModelBounds {
            kind: ModelKind::Voxel,
            min,
            max,
            file_name: "subject.voxel.yaml".to_string(),
            scale,
            emissive_texture: false,
        }
    }

    /// The subject's rendered pixel height in a `cell_w`×`cell_h` cell — the projection written out
    /// forwards, so the test checks the framing rather than restating
    /// [`TrueSizeFraming::cell`]'s algebra.
    fn subject_pixels(bounds: &ModelBounds, cell_w: u32, cell_h: u32) -> f32 {
        let half_y = (FOV_Y_DEG * 0.5).to_radians();
        let visible = 2.0 * fit_distance(cell_w, cell_h) * math::tan(half_y);
        let normalized_height =
            (bounds.max[1] - bounds.min[1]) * NormalizeRig::from_box(bounds.min, bounds.max).scale;
        cell_h as f32 * normalized_height / visible
    }

    /// **The contract of `--true-size`:** the subject renders at the pixels its declared scale
    /// earns, for a character and for a building alike — the two cases the unit-sphere fit renders
    /// identically.
    #[test]
    fn true_size_renders_the_earned_coverage() {
        // The shipped statue: 9³ at `scale: 0.24`, 2.16 units tall, 106 px in game.
        let statue = voxel_bounds([9, 9, 9], 0.24);
        // `house-c`: 128×192×112 at `scale: 0.0625`, 12 units tall.
        let house = voxel_bounds([128, 192, 112], 0.0625);

        for (name, bounds, earned) in [("statue", &statue, 106.0f32), ("house", &house, 588.9)] {
            let subject = Subject::single(bounds);
            let framing = TrueSizeFraming::compute(&subject);
            assert!(
                (framing.subject_px - earned).abs() < 0.5,
                "{name} earns {earned} px, got {}",
                framing.subject_px
            );
            let (cell_w, cell_h) = framing.cell(&subject, Grid::for_views(1), 1920, 1080);
            let rendered = subject_pixels(bounds, cell_w, cell_h);
            assert!(
                (rendered - framing.subject_px).abs() / framing.subject_px < 0.01,
                "{name} renders {rendered} px in a {cell_w}x{cell_h} cell, wanted {}",
                framing.subject_px
            );
        }
    }

    /// The cell sizes that contract implies, pinned so a change to the framing constants is a
    /// visible decision. The building's cell is ~4.3× the character's — the ratio the unit-sphere
    /// sheet flattens to 1.0, and the whole point of the flag.
    #[test]
    fn true_size_cells_are_pinned() {
        let statue = voxel_bounds([9, 9, 9], 0.24);
        let house = voxel_bounds([128, 192, 112], 0.0625);
        let grid = Grid::for_views(1);

        let statue_subject = Subject::single(&statue);
        let (gw, gh) =
            TrueSizeFraming::compute(&statue_subject).cell(&statue_subject, grid, 1920, 1080);
        assert!((200..=230).contains(&gh), "statue cell height {gh}");
        assert_eq!(gw, (gh as f32 * 16.0 / 9.0).round() as u32);

        let house_subject = Subject::single(&house);
        let (_, hh) =
            TrueSizeFraming::compute(&house_subject).cell(&house_subject, grid, 1920, 1080);
        assert!((900..=980).contains(&hh), "house cell height {hh}");
    }

    /// An absurd declared scale is clamped rather than allowed to ask for a render nothing can
    /// allocate.
    #[test]
    fn true_size_clamps_an_absurd_scale() {
        let huge = voxel_bounds([8, 8, 8], 10_000.0);
        let subject = Subject::single(&huge);
        let (w, h) =
            TrueSizeFraming::compute(&subject).cell(&subject, Grid::for_views(1), 1920, 1080);
        assert_eq!(
            (w, h),
            (MAX_TRUE_SIZE_CELL as u32, MAX_TRUE_SIZE_CELL as u32)
        );
    }

    /// A subject with no height (a flat model) earns no size: it gets the divided cell rather than
    /// a NaN that truncates to a zero-pixel render.
    #[test]
    fn true_size_gives_a_flat_subject_the_divided_cell() {
        let grid = Grid::for_views(1);
        let mut subject = Subject::single(&voxel_bounds([8, 8, 8], 0.24));
        subject.normalized_height = 0.0;
        let divided = SheetLayout::divide(grid, 1920, 1080);
        for height in [0.0, f32::NAN, f32::INFINITY, -1.0] {
            subject.normalized_height = height;
            subject.world_height = 0.0;
            assert_eq!(
                TrueSizeFraming::compute(&subject).cell(&subject, grid, 1920, 1080),
                (divided.cell_w, divided.cell_h),
                "normalized height {height}",
            );
        }
    }

    /// A mesh document declares no scale, so `--true-size` reads its vertices as world units.
    #[test]
    fn a_mesh_document_is_already_world_units() {
        let bounds = ModelBounds {
            kind: ModelKind::Mesh,
            min: [0.0, 0.0, 0.0],
            max: [1.0, 3.0, 1.0],
            file_name: "prop.mesh.yaml".to_string(),
            scale: 1.0,
            emissive_texture: false,
        };
        let framing = TrueSizeFraming::compute(&Subject::single(&bounds));
        assert_eq!(framing.world_height, 3.0);
        assert!((framing.subject_px - 3.0 * PX_PER_WORLD_UNIT).abs() < 1e-3);
    }

    /// The sweep grid is near-square, matching `crate::sweep`'s, so the two sheets read alike.
    #[test]
    fn the_sweep_grid_is_near_square() {
        for (n, cols, rows) in [
            (1u32, 1u32, 1u32),
            (2, 2, 1),
            (3, 2, 2),
            (5, 3, 2),
            (16, 4, 4),
        ] {
            let grid = Grid::near_square(n);
            assert_eq!((grid.cols, grid.rows), (cols, rows), "N={n}");
        }
    }

    /// A sweep must name exactly one view and fit one sheet; each refusal is an actionable `L4008`.
    #[test]
    fn sweep_shape_is_enforced() {
        let values = [serde_json::json!(1.0), serde_json::json!(2.0)];
        let sweep = ModelSweep {
            pointer: "/palette/G/emissive",
            values: &values,
        };
        let one = [ModelView::Game];
        assert!(check_sweep_shape(&sweep, &one, None).is_ok());

        let errors = check_sweep_shape(&sweep, &ModelView::DEFAULT, None).unwrap_err();
        assert_eq!(errors[0].code.as_str(), "L4008");
        assert!(errors[0].help.as_deref().unwrap().contains("--views game"));

        let empty = ModelSweep {
            pointer: "/palette/G/emissive",
            values: &[],
        };
        assert_eq!(
            check_sweep_shape(&empty, &one, None).unwrap_err()[0]
                .code
                .as_str(),
            "L4008"
        );

        let too_many: Vec<Value> = (0..=MAX_SWEEP_CELLS)
            .map(|i| serde_json::json!(i))
            .collect();
        let over = ModelSweep {
            pointer: "/palette/G/emissive",
            values: &too_many,
        };
        let errors = check_sweep_shape(&over, &one, None).unwrap_err();
        assert!(errors[0].message.contains(&MAX_SWEEP_CELLS.to_string()));

        // A sweep and a strip both spend the sheet's cells, so the sheet cannot hold both.
        let errors = check_sweep_shape(&sweep, &one, Some(4)).unwrap_err();
        assert_eq!(errors[0].code.as_str(), "L4008");
        assert!(
            errors[0].help.as_deref().unwrap().contains("pick one"),
            "{:?}",
            errors[0].help
        );
    }

    /// A sweep cell's label is the palette-relative field then the value — no `=`, to keep it short.
    #[test]
    fn sweep_labels_name_the_field_and_value() {
        assert_eq!(
            sweep_label("/palette/G/emissive", &serde_json::json!(2.2)),
            "G/emissive 2.2"
        );
        assert_eq!(
            sweep_label("/palette/S/color/0", &serde_json::json!(0.12)),
            "S/color/0 0.12"
        );
    }

    // --- Lineup -------------------------------------------------------------------------------

    /// **The point of a lineup:** the union normalization keeps relative sizes honest, so a 12-unit
    /// house really does stand 5.5× the 2.16-unit statue beside it — the ratio the single-model sheet
    /// flattens to 1.0.
    #[test]
    fn a_lineup_normalizes_the_union_so_relative_size_survives() {
        let statue = voxel_bounds([9, 9, 9], 0.24);
        let house = voxel_bounds([128, 192, 112], 0.0625);
        let subject = Subject::lineup(&[&statue, &house], row_direction(ModelView::Game));

        // One normalization `k` multiplies each model's own declared scale.
        let k = subject.placements[0].scale / statue.scale;
        assert!((subject.placements[1].scale / house.scale - k).abs() / k < 1e-5);

        let rendered = |bounds: &ModelBounds, placement: &Placement| {
            (bounds.max[1] - bounds.min[1]) * placement.scale
        };
        let ratio =
            rendered(&house, &subject.placements[1]) / rendered(&statue, &subject.placements[0]);
        assert!(
            (ratio - 12.0 / 2.16).abs() < 1e-3,
            "the house must render 5.56× the statue, got {ratio}",
        );
        assert_eq!(
            subject.world_height, 12.0,
            "the tallest model sets the height"
        );
    }

    /// The same honesty, read off the synthesized document rather than the intermediate — two models
    /// of the same grid but different declared scale get `Transform.scale`s in that same ratio.
    #[test]
    fn the_synthesized_lineup_carries_honest_transform_scales() {
        let small = voxel_bounds([8, 8, 8], 0.5);
        let large = voxel_bounds([8, 8, 8], 2.0);
        let subject = Subject::lineup(&[&small, &large], row_direction(ModelView::Game));
        let names = [
            "0-small.voxel.yaml".to_string(),
            "1-large.voxel.yaml".to_string(),
        ];
        let yaml = synthesize_scene(
            &[&small, &large],
            &names,
            &subject,
            None,
            ModelView::Game,
            ModelView::Game.label(),
            &[],
            &[None, None],
            400,
            400,
            None,
        );
        let scene =
            ludion_scene::check(&yaml, &builtin_registry()).expect("the lineup scene is valid");

        let scale_of = |index: usize| -> f32 {
            scene.entities[index].components[0].1["scale"][0]
                .as_f64()
                .expect("a uniform scale") as f32
        };
        assert!(
            (scale_of(1) / scale_of(0) - 4.0).abs() < 1e-4,
            "four times the declared scale is four times the rendered size",
        );
        assert!(yaml.contains("0-small.voxel.yaml") && yaml.contains("1-large.voxel.yaml"));
    }

    /// Models stand apart along the row and never share a seat, in the order they were given.
    #[test]
    fn a_lineup_seats_models_in_order_without_overlap() {
        let models = [
            voxel_bounds([4, 4, 4], 1.0),
            voxel_bounds([16, 8, 16], 1.0),
            voxel_bounds([2, 20, 2], 1.0),
        ];
        let refs: Vec<&ModelBounds> = models.iter().collect();
        let direction = row_direction(ModelView::Game);
        let subject = Subject::lineup(&refs, direction);

        // Row position is the projection of each footprint center onto the row direction.
        let seats: Vec<f32> = subject
            .placements
            .iter()
            .map(|placement| placement.base.dot(direction))
            .collect();
        assert!(
            seats[0] < seats[1] && seats[1] < seats[2],
            "row order: {seats:?}"
        );
        for placement in &subject.placements {
            assert!(
                placement.base.y == 0.0,
                "every model stands on the shared ground plane",
            );
        }
    }

    /// The row runs across the screen, not into it: the first model given is always the leftmost one
    /// seen, and the row is broadside so neighbours never hide each other.
    #[test]
    fn the_row_direction_reads_left_to_right_in_every_view() {
        for view in ModelView::ALL {
            let direction = row_direction(view);
            let (camera, up) = view.direction_and_up();
            let right = (-camera).cross(up).normalize();
            assert!(
                direction.dot(right) > 0.9,
                "{view:?}: the row must run along screen-right, got {direction:?}",
            );
            assert!(direction.y == 0.0, "{view:?}: the row stays on the ground");
            assert!(
                (direction.length() - 1.0).abs() < 1e-5,
                "{view:?}: the row direction is a unit vector",
            );
        }
    }

    /// A lineup takes one view, at most [`MAX_LINEUP_MODELS`] models, and no palette override — each
    /// refusal an actionable `L4008`.
    #[test]
    fn lineup_shape_is_enforced() {
        let paths: Vec<PathBuf> = (0..=MAX_LINEUP_MODELS)
            .map(|i| PathBuf::from(format!("m{i}.voxel.yaml")))
            .collect();
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let one = [ModelView::Game];

        let pair = ModelSheetRequest::new(&refs[..2], 64, 64);
        assert!(check_lineup_shape(&pair, &one).is_ok());

        let errors = check_lineup_shape(&pair, &ModelView::DEFAULT).unwrap_err();
        assert_eq!(errors[0].code.as_str(), "L4008");
        assert!(errors[0].help.as_deref().unwrap().contains("--views game"));

        let too_many = ModelSheetRequest::new(&refs, 64, 64);
        let errors = check_lineup_shape(&too_many, &one).unwrap_err();
        assert!(errors[0].message.contains(&MAX_LINEUP_MODELS.to_string()));

        let over = SceneOverride::parse("/palette/G/emissive=2.0").unwrap();
        let with_set = ModelSheetRequest {
            set: std::slice::from_ref(&over),
            ..pair
        };
        let errors = check_lineup_shape(&with_set, &one).unwrap_err();
        assert_eq!(errors[0].code.as_str(), "L4008");
        assert!(errors[0].message.contains("palette"));

        let values = [serde_json::json!(1.0)];
        let with_sweep = ModelSheetRequest {
            sweep: Some(ModelSweep {
                pointer: "/palette/G/emissive",
                values: &values,
            }),
            ..pair
        };
        assert_eq!(
            check_lineup_shape(&with_sweep, &one).unwrap_err()[0]
                .code
                .as_str(),
            "L4008"
        );
    }

    /// A caption names the model, not its file: the extension and the `.voxel`/`.mesh` tag go.
    #[test]
    fn a_caption_is_the_model_stem() {
        assert_eq!(model_stem("house-c.voxel.yaml"), "house-c");
        assert_eq!(model_stem("rock.mesh.yaml"), "rock");
        assert_eq!(model_stem("figure.assembly.yaml"), "figure");
        assert_eq!(model_stem("idol.sculpt.yaml"), "idol");
        assert_eq!(model_stem("plain.yaml"), "plain");
    }

    /// Captions shrink until neighbours stop touching, measured against where the models actually
    /// project — a small model beside a large one is the case an even division would overlap.
    #[test]
    fn captions_shrink_to_clear_their_neighbours() {
        let file = |name: &str, size: [u32; 3], scale: f32| LoadedModel {
            bounds: ModelBounds {
                kind: ModelKind::Voxel,
                min: [-(size[0] as f32) * 0.5, 0.0, -(size[2] as f32) * 0.5],
                max: [size[0] as f32 * 0.5, size[1] as f32, size[2] as f32 * 0.5],
                file_name: name.to_string(),
                scale,
                emissive_texture: false,
            },
            extra: LoadedExtra::Mesh,
        };
        let loaded = [
            file("statue.voxel.yaml", [9, 9, 9], 0.24),
            file("cathedral-quarter.voxel.yaml", [128, 192, 112], 0.0625),
        ];
        let bounds: Vec<&ModelBounds> = loaded.iter().map(|m| &m.bounds).collect();
        let subject = Subject::lineup(&bounds, row_direction(ModelView::Game));
        let layout = SheetLayout::divide(Grid::for_views(1), 1920, 1080);
        let labels: Vec<String> = loaded
            .iter()
            .map(|m| model_stem(&m.bounds.file_name))
            .collect();
        let captions = lineup_captions(&labels, &subject, ModelView::Game, &layout);

        assert_eq!(captions.len(), 2);
        assert_eq!(captions[0].text, "statue");
        assert!(captions[0].offset[0] < captions[1].offset[0], "row order");
        let half = |caption: &Caption| 0.5 * caption.text.len() as f32 * caption.size;
        assert!(
            captions[1].offset[0] - captions[0].offset[0] > half(&captions[0]) + half(&captions[1]),
            "captions must not overlap: {:?} / {:?}",
            (captions[0].offset[0], captions[0].size),
            (captions[1].offset[0], captions[1].size),
        );
        assert!(captions[0].size >= CAPTION_MIN_SIZE);
    }

    /// The committed idol sculpt, shared with `ludion-sculpt`'s conformance test.
    fn idol_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../ludion-sculpt/tests/fixtures/idol.sculpt.yaml")
    }

    /// A sculpt loads as `ModelKind::Sculpt`, bounded by the positions it meshes to — bottom-centered
    /// world units, so the floor is `y = 0` and no declared scale applies.
    #[test]
    fn a_sculpt_loads_with_bounds_from_its_mesh() {
        let loaded = load_model(&idol_path()).unwrap_or_else(|d| panic!("{d:#?}"));
        assert!(matches!(loaded.bounds.kind, ModelKind::Sculpt));
        let LoadedExtra::Sculpt(sculpt) = &loaded.extra else {
            panic!("a sculpt loads its document and mesh");
        };
        assert_eq!(
            (loaded.bounds.min, loaded.bounds.max),
            sculpt_aabb(&sculpt.mesh)
        );
        assert_eq!(loaded.bounds.min[1], 0.0);
        assert!(
            loaded.bounds.max[1] > 1.0,
            "the idol stands over a unit tall"
        );
        assert_eq!(loaded.bounds.scale, 1.0);
        assert_eq!(model_stem(&loaded.bounds.file_name), "idol");
    }

    /// A part sheet fits the part's box to the unit sphere around the camera target, while the
    /// model's own floor stays on the ground plane.
    #[test]
    fn a_part_subject_frames_the_part_on_the_models_floor() {
        let loaded = load_model(&idol_path()).unwrap_or_else(|d| panic!("{d:#?}"));
        let focus = resolve_part(&loaded, "head").unwrap_or_else(|d| panic!("{d:#?}"));
        let (min, max) = framed_part_box(&loaded.bounds, focus.bounds);
        let subject = Subject::part(&loaded.bounds, focus.bounds);
        let placement = &subject.placements[0];
        let k = placement.scale;
        assert!((k - 1.0 / bounding_radius(min, max)).abs() < 1e-6);
        let seat = |p: [f32; 3]| {
            Vec3::new(
                p[0] * k + placement.translation[0],
                p[1] * k + placement.translation[1],
                p[2] * k + placement.translation[2],
            )
        };
        assert!(
            seat(loaded.bounds.min).y.abs() < 1e-5,
            "the floor stays at y = 0"
        );
        let center = (seat(min) + seat(max)) * 0.5;
        assert!(
            (center - subject.target).length() < 1e-4,
            "{center} vs {}",
            subject.target
        );
    }

    /// A sliver of a part on a large body still frames: its box grows to the minimum on every axis
    /// about its own center, and every view's camera stops outside the whole model's box.
    #[test]
    fn a_tiny_part_frames_a_padded_box_from_outside_the_model() {
        let body = ModelBounds {
            kind: ModelKind::Sculpt,
            min: [-2.0, 0.0, -1.5],
            max: [2.0, 4.0, 1.5],
            file_name: "body.sculpt.yaml".to_string(),
            scale: 1.0,
            emissive_texture: false,
        };
        // An eye's meshed shell on the body's front face: 0.16 x 0.03 x 0.007 units.
        let eye = ([0.2, 3.0, -1.5], [0.36, 3.03, -1.493]);

        let (min, max) = framed_part_box(&body, eye);
        let shortest = MIN_PART_FRACTION * 4.0;
        for axis in 0..3 {
            assert!(
                max[axis] - min[axis] >= shortest - 1e-5,
                "axis {axis}: {min:?}..{max:?}"
            );
            let centered = (min[axis] + max[axis]) - (eye.0[axis] + eye.1[axis]);
            assert!(centered.abs() < 1e-5, "axis {axis} moved off the part");
        }

        let subject = Subject::part(&body, eye);
        let (lo, hi) = subject
            .enclosure
            .expect("a part subject keeps its model's box");
        for view in ModelView::ALL {
            let (direction, _) = view.direction_and_up();
            let camera = subject.target + direction * subject.camera_distance(view, 320, 240);
            assert!(
                (0..3).any(|axis| camera[axis] < lo[axis] || camera[axis] > hi[axis]),
                "{} camera {camera} is inside the model {lo:?}..{hi:?}",
                view.name()
            );
        }
        // A whole-model subject keeps the plain fit.
        let whole = Subject::single(&body);
        assert_eq!(
            whole.camera_distance(ModelView::Front, 320, 240),
            fit_distance(320, 240)
        );
    }

    /// The voxel AABB is bottom-center, 1 voxel = 1 unit (matches the mesher convention).
    #[test]
    fn voxel_aabb_is_bottom_center() {
        let doc = VoxelDocument {
            name: None,
            size: [4, 6, 2],
            scale: None,
            generated_from: None,
            budget_exempt: None,
            palette: Vec::new(),
            slices: Vec::new(),
            rim: None,
            lods: Vec::new(),
        };
        let (min, max) = voxel_aabb(&doc);
        assert_eq!(min, [-2.0, 0.0, -1.0]);
        assert_eq!(max, [2.0, 6.0, 1.0]);
    }

    /// Writes `text` to `root`/`rel`, creating parent directories.
    fn write_file(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A one-part assembly whose only part lives beside the model's directory, at `../parts/arm`.
    /// `arm_x` is that part's authored width, and the assembly is as wide.
    fn hero_dir(root: &Path, side: &str, arm_x: u32) {
        let assembly = format!(
            "format: 1\nkind: assembly\nname: hero\nsize:\n  - {arm_x}\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      - 1.0\n      - 0.0\n      - 0.0\n      - 1.0\nparts:\n  - name: arm\n    file: ../parts/arm.part.yaml\n    place:\n      - 0\n      - 0\n      - 0\n"
        );
        let part = format!(
            "format: 1\nkind: part\nsize:\n  - {arm_x}\n  - 1\n  - 1\nslices:\n  -\n    - \"{}\"\n",
            "A".repeat(arm_x as usize)
        );
        write_file(root, &format!("{side}/hero/hero.assembly.yaml"), &assembly);
        write_file(root, &format!("{side}/parts/arm.part.yaml"), &part);
    }

    fn stage_scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ludion-stage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Two models whose parts sit at `../parts/arm.part.yaml` used to share one staged
    /// `lineup/parts/arm.part.yaml` — the second write won, and both rendered the same arm. The
    /// oracle is the arm width each fixture authors (1 and 2).
    #[test]
    fn a_lineup_stages_each_models_parent_relative_parts_apart() {
        let dir = stage_scratch("apart");
        hero_dir(&dir, "a", 1);
        hero_dir(&dir, "b", 2);
        let a = dir.join("a/hero/hero.assembly.yaml");
        let b = dir.join("b/hero/hero.assembly.yaml");
        let sources: Vec<&Path> = vec![&a, &b];
        let loaded = load_models(&sources).expect("both load");
        let staged = stage_lineup(&sources, &loaded).expect("a `..` part stages");

        for (index, width) in [(0, 1), (1, 2)] {
            let data = crate::assembly_mesh::load(
                ludion_assets::AssetRoot::local_document_unconfined(&staged.roots[0]),
                &staged.names[index],
            )
            .expect("the staged assembly loads from its slot");
            assert_eq!(data.parts[0].size, [width, 1, 1], "slot {index}");
        }
        let mut slots: Vec<String> = std::fs::read_dir(&staged.roots[0])
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        slots.sort();
        assert_eq!(slots, ["0", "1"], "nothing is written beside the slots");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `--set` on a model with a `..` part stages it inside the cell's root, and it still loads.
    #[test]
    fn a_set_override_stages_a_parent_relative_part_inside_its_root() {
        let dir = stage_scratch("set");
        hero_dir(&dir, "a", 2);
        let a = dir.join("a/hero/hero.assembly.yaml");
        let loaded = load_models(&[a.as_path()]).expect("loads");
        let doc = match &loaded[0].extra {
            LoadedExtra::Assembly(doc) => doc.clone(),
            _ => panic!("an assembly"),
        };
        let root = dir.join("cell");
        Overridden::Assembly(doc, &a)
            .write(&root, "hero.assembly.yaml")
            .expect("stages");
        let data = crate::assembly_mesh::load(
            ludion_assets::AssetRoot::local_document_unconfined(&root),
            "hero.assembly.yaml",
        )
        .expect("loads");
        assert_eq!(data.parts[0].size, [2, 1, 1]);
        assert!(!dir.join("parts").exists(), "nothing escapes the cell root");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A part path with more `..` than the model's directory has ancestors cannot be re-rooted:
    /// `L1138`, staged nowhere.
    #[test]
    fn a_dependency_path_that_cannot_be_rerooted_is_refused() {
        let dir = stage_scratch("escape");
        hero_dir(&dir, "a", 1);
        let a = dir.join("a/hero/hero.assembly.yaml");
        let mut loaded = load_models(&[a.as_path()]).expect("loads");
        let LoadedExtra::Assembly(doc) = &mut loaded[0].extra else {
            panic!("an assembly")
        };
        doc.parts[0].file = format!("{}x.part.yaml", "../".repeat(80));
        let errors = stage_lineup(&[a.as_path()], &loaded)
            .err()
            .expect("an over-deep path is refused");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, code::ASSEMBLY_DATA_CONSISTENCY);

        let absolute = staged_keys(&a, &["/etc/x.part.yaml"], code::ASSEMBLY_DATA_CONSISTENCY);
        assert!(absolute.is_err(), "an absolute path is refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Parts that climb zero, one and two directories stage under their own ancestors: each file
    /// must resolve to the part it authors, whose width is the oracle (1, 2, 3).
    #[test]
    fn mixed_depth_parent_relative_parts_stage_under_their_own_ancestors() {
        let dir = stage_scratch("mixed");
        let part = |width: u32| {
            format!(
                "format: 1\nkind: part\nsize:\n  - {width}\n  - 1\n  - 1\nslices:\n  -\n    - \"{}\"\n",
                "A".repeat(width as usize)
            )
        };
        let part_entry = |name: &str, file: &str, x: u32| {
            let parent = if name == "a" { "" } else { "    parent: a\n" };
            format!(
                "  - name: {name}\n    file: {file}\n{parent}    place:\n      - {x}\n      - 0\n      - 0\n"
            )
        };
        let assembly = format!(
            "format: 1\nkind: assembly\nname: hero\nsize:\n  - 8\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      - 1.0\n      - 0.0\n      - 0.0\n      - 1.0\nparts:\n{}{}{}",
            part_entry("a", "parts/a.part.yaml", 0),
            part_entry("b", "../y.part.yaml", 2),
            part_entry("c", "../../r/y.part.yaml", 4),
        );
        write_file(&dir, "q/r/hero.assembly.yaml", &assembly);
        write_file(&dir, "q/r/parts/a.part.yaml", &part(1));
        write_file(&dir, "q/y.part.yaml", &part(2));
        write_file(&dir, "r/y.part.yaml", &part(3));
        let hero = dir.join("q/r/hero.assembly.yaml");
        let sources: Vec<&Path> = vec![&hero];
        let loaded = load_models(&sources).expect("loads");
        let staged = stage_lineup(&sources, &loaded).expect("stages");
        let data = crate::assembly_mesh::load(
            ludion_assets::AssetRoot::local_document_unconfined(&staged.roots[0]),
            &staged.names[0],
        )
        .expect("the staged assembly loads");
        let widths: Vec<u32> = data.parts.iter().map(|part| part.size[0]).collect();
        assert_eq!(widths, [1, 2, 3]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With no `..` anywhere the staged keys are the authored paths, so no document is rewritten.
    #[test]
    fn staged_keys_are_the_authored_paths_without_a_parent_reference() {
        let keys = staged_keys(
            Path::new("models/hero.assembly.yaml"),
            &["parts/a.part.yaml", "anims/walk.anim.yaml"],
            code::ASSEMBLY_DATA_CONSISTENCY,
        )
        .unwrap();
        assert!(keys.iter().all(|(authored, staged)| authored == staged));
    }
}
