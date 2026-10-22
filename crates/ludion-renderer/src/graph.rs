//! The frame graph: a fifteen-pass pipeline (sixteen with UI, with a sun, or with a render scale
//! off native, two more with light shafts, two more with depth of field, three more with ambient
//! occlusion, and two seed-plus-reduce chains with screen-space reflections) declared through `ludion-render-graph`.
//!
//! The structure depends only on the [`FrameSize`] and the [`PostChain`] (bloom is always on), so the
//! graph is a pure function of `(size, chain)` and needs no GPU to build or dump. That is what makes
//! `ludion dump-graph` CPU-only and byte-stable.
//!
//! ```text
//! scene ─► reflections ─► transparents (HDR) ──► bloom_threshold ──► bloom_down_1 ─► … ─► bloom_down_5
//!    │                   │                  │                   │
//!    │                   ▼                  ▼                   ▼
//!    │             bloom_up_0 ◄──── bloom_up_1 ◄── … ◄─── bloom_up_4   (each adds its level back)
//!    │                   │
//!    └───────────────────┴──────────────────────────────► composite ──► output (sRGB)
//! ```
//!
//! - **scene** — opaque meshes (depth-tested, lit) into a full-resolution HDR (`Rgba16Float`)
//!   target, and their normal+roughness and F0+specular-occlusion into two more.
//! - **reflections** — a fullscreen additive pass over the opaque scene (the sky's indirect specular).
//! - **transparents** — the sky, ground markers, sprites/billboards (alpha, depth-tested), particles
//!   and text, over the same target.
//! - **bloom_threshold / bloom_down_1..5 / bloom_up_4..0** — a six-level bloom pyramid: a bright-pass
//!   writing level 0, five progressive 13-tap downsamples (Karis-averaged on the first, so a lone
//!   bright pixel cannot ride the chain down as a firefly) and five 3×3 tent upsamples adding each
//!   level back. It composites the sum of all six levels, which is why `bloom_strength` ships near
//!   `0.1` rather than near `1`.
//! - **composite** — blends in the defocused scene, mixes scene + blurred bloom, then applies fog, the
//!   light shafts, exposure, the selected tonemap operator, the colour grade, the vignette and the
//!   optional output dither, HDR→sRGB into the read-back output. It also reads `scene_depth` —
//!   unconditionally, so the pass has one bind group layout and one pipeline whatever the scene
//!   authors; fog skips by a uniform branch. Its two *optional* inputs, the blurred shaft term and the
//!   depth-of-field gather, follow the fixed-slot rule in `fullscreen_effect`'s doc comment rather
//!   than a second pipeline.
//!
//! # Which extent sizes which target
//!
//! [`FrameSize`] carries two extents — what the client asked for (`output`) and what the chain draws
//! at (`render`, `output × scale`). The normative rule is that **only the scene pass's colour and
//! depth targets, and `ldr`, are sized from the render extent; every other target is sized from the
//! output extent** — the SSR pyramids (`hiz`, `ssr_color`) among them, at output × `QualityParams::ssr_scale`.
//!
//! That is what makes a render scale a pure quality change. Expressing a filter's footprint in
//! *target*-texel units (as `record_blur` and the pyramid do) only makes it screen-relative if the
//! target is itself a fixed fraction of the **frame** — sizing a bloom level off the render buffer
//! instead would keep the tap offsets identical while halving the fraction of the *image* they
//! cover, and the glow would narrow every time quality went up. Sizing every bloom target from the
//! output extent makes both bloom chains scale-invariant with no shader or uniform change at all, and
//! makes bloom cost the same at any scale — the right trade for a low-frequency effect. One known
//! consequence: the bright pass takes a single bilinear tap, so at a scale above 1 it reduces the
//! scene by more than 2:1 in that tap. That is today's behaviour at a wider ratio, not a new defect.
//!
//! The rule holds unchanged below native, where it reads the other way round: at a scale under 1 the
//! half-extent post targets are *larger* than the scene the chain drew, which is the same statement —
//! a bloom radius and a beam width are fractions of the frame the client asked for, not of the
//! buffer this machine could afford.
//!
//! [`PostChain::ssao`] prepends three passes in front of the scene rather than behind it, because the
//! shading is what consumes the result (`Plan/post-process-pipeline.md` decision 8): a **prepass**
//! rasterizes the opaque meshes into its own depth attachment plus a world-space normal target,
//! **ssao** estimates occlusion from those two, **ssao_blur** cancels the estimator's per-pixel
//! rotation, and the scene pass samples the blurred factor and multiplies it into the ambient + IBL
//! term alone:
//!
//! ```text
//! prepass ─► ssao ─► ssao_blur ─► scene ─► …
//! ```
//!
//! [`PostChain::shafts`] appends the mirror-image pair *behind* the scene pass, because what the
//! volumetric march consumes is the frame the scene already drew: **shafts** ray-marches the sun's
//! shadow map along each eye ray, stopping at whatever surface `scene_depth` recorded, **shafts_blur**
//! cancels the march's per-pixel step offset with the same 4×4 box `ssao_blur` uses, and `composite`
//! tints the result with the sun's colour and adds it:
//!
//! ```text
//! scene ─► shafts ─► shafts_blur ─► composite
//! ```
//!
//! It is the one chain gated on *three* things — the authored strength, the presence of a sun, and a
//! medium with a nonzero extinction. The march estimates the in-scatter *of a medium* along the ray,
//! so all three are the same requirement read from different sides: no shadow map is nothing to
//! march, and no medium is nothing to scatter.
//!
//! [`PostChain::dof`] appends a second pair behind the scene pass: **dof_coc** downsamples the scene
//! into a half-extent target carrying each texel's signed circle of confusion in its alpha, **dof**
//! gathers a disc over that one texture, and `composite` blends the result in by the weight the
//! gather wrote. The prefilter is not an optimisation looking for a home — it turns each of the
//! gather's taps into one filterable fetch instead of a colour, a depth and a matrix multiply, it
//! area-averages the colour once instead of point-sampling it per tap, and it takes the circle of
//! confusion from the *nearest* source texel so a silhouette does not thin:
//!
//! ```text
//! scene ─► dof_coc ─► dof ─► composite
//! ```
//!
//! The prepass owns its depth instead of handing one to the scene pass, so the scene pass's `Less`
//! test and clear are untouched. An AO frame therefore rasterizes its geometry twice — which is
//! exactly the geometry SSAO reads.
//!
//! [`PostChain::ssr`] adds the trace's two input pyramids between the opaque pass and `reflections`,
//! at the trace extent (output × [`QualityParams::ssr_scale`]): `hiz` (`R32Float`, a per-cell min-Z of `scene_depth`) and
//! `ssr_color` (`Rgba16Float`, the opaque `scene` pre-convolved per mip), each a seed pass plus one
//! reduce pass per further mip. The `ssr_trace` pass then marches a ray per trace texel through them
//! into `ssr_trace` (`Rgba16Float`: radiance and confidence), which `reflections` blends. With
//! [`PostChain::ssr_view`] set, [`pass::SSR_DEBUG`] replaces `composite` and draws the trace itself;
//! [`PostChain::probe_view`] does the same with [`pass::PROBE_DEBUG`], drawing the winning probe per pixel.
//!
//! ```text
//! scene ─► hiz_seed ─► hiz_reduce_1 ─► … ─► ssr_color_seed ─► ssr_color_reduce_1 ─► … ─► ssr_trace ─► reflections
//! ```
//!
//! The scene's three passes write `scene` and `scene_depth` between them, which the graph orders by
//! one rule: **passes that write the same subresource run in declaration order**
//! ([`schedule`](fn@schedule): a later writer follows an earlier one, and a reader follows the
//! latest writer declared before it). `scene` rasterizes the opaque meshes, `reflections` reads
//! their depth, normal and specular targets and adds into `scene`, and `transparents` draws the
//! rest into `scene` against `scene_depth`.
//!
//! When the scene carries screen-space UI ([`PostChain::ui`], `Plan/phase-14-ui.md` decision 5), the
//! **composite** pass instead writes an LDR transient (`ldr`, `Rgba8UnormSrgb`) and a trailing
//! **overlay** pass reads it, blits it to the output, and draws the sorted UI rects/text on top.
//!
//! A render extent that differs from the output extent inserts one resampling pass between the two —
//! **resolve** when the chain drew larger than it presents (an area-weighted box filter down),
//! **upscale** when it drew smaller (a bilinear filter up). `composite` writes `ldr` at the render
//! extent, the resampling pass writes the output extent, and only then does the overlay draw. So UI
//! text is rasterized once, at output resolution, and passes through neither filter:
//!
//! ```text
//! composite ─► final                                                     [native, no UI]
//! composite ─► ldr ─► overlay ─► final                                   [native, UI]
//! composite ─► ldr ─► resample ─► final                                  [scaled, no UI]
//! composite ─► ldr ─► resample ─► ldr_resolved ─► overlay ─► final       [scaled, UI]
//! ```
//!
//! where `resample` is [`resolve`](pass::RESOLVE) or [`upscale`](pass::UPSCALE) — one seam, one
//! extra pass, whichever direction the scale went ([`FrameSize::resample`]).
//!
//! All passes are [`PassKind::External`](ludion_render_graph::PassKind::External); the renderer
//! records their GPU work while the graph owns scheduling, transient aliasing, and the JSON dump.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_render_graph::{
    MAX_TEXTURE_DIMENSION_2D, RenderGraph, ResourceId, TextureDesc, schedule,
};

use crate::quality::{Quality, QualityParams};
use crate::scene::RenderScene;

/// The dispatch labels of the frame graph's passes. Shared between graph construction (as the
/// external-pass labels) and the renderer's recorder (as the match arms), so the two never drift.
pub mod pass {
    /// The depth-only sun shadow pass: every mesh drawn into the orthographic shadow map. Present
    /// only when the frame draws one — the scene carries a sun and this machine's quality keeps its
    /// shadows (`Plan/shadow-mapping.md` decision 6).
    pub const SHADOW: &str = "shadow";
    /// The opaque scene pass: every mesh, lit, into the HDR colour target plus the normal+roughness
    /// and F0+occlusion targets the reflections pass reads.
    pub const SCENE: &str = "scene";
    /// The reflections pass: a fullscreen additive draw over the opaque scene. It reads its normal,
    /// specular and depth targets and adds the sky's indirect specular into `scene`.
    pub const REFLECTIONS: &str = "reflections";
    /// The Hi-Z seed: scene depth into `hiz` mip 0 as a per-cell minimum.
    pub const HIZ_SEED: &str = "hiz_seed";
    /// A Hi-Z reduce: one `hiz` mip's per-cell minimum over the mip above. Named `hiz_reduce_{k}`
    /// for the mip it writes.
    pub const HIZ_REDUCE: &str = "hiz_reduce";
    /// The colour-pyramid seed: the opaque `scene` into `ssr_color` mip 0, before `reflections`.
    pub const SSR_COLOR_SEED: &str = "ssr_color_seed";
    /// A colour-pyramid reduce: a 4×4 tent over the mip above. Named `ssr_color_reduce_{k}` for the
    /// mip it writes.
    pub const SSR_COLOR_REDUCE: &str = "ssr_color_reduce";
    /// The SSR trace: one mirror ray per trace texel marched through `hiz`, written to `ssr_trace` as
    /// radiance and confidence. Reads both pyramids whole, the opaque normal and the depth.
    pub const SSR_TRACE: &str = "ssr_trace";
    /// A full-screen view of the trace (`ludion render --debug-view`): replaces `composite`, reads
    /// `ssr_trace` and writes the same target.
    pub const SSR_DEBUG: &str = "ssr_debug";
    /// A full-screen view of the winning reflection probe per pixel (`ludion render --debug-view
    /// probe_index`): replaces `composite`, reads the normal and depth and writes the same target.
    pub const PROBE_DEBUG: &str = "probe_debug";
    /// A probe capture's last pass: the lit face into the capture target, with sky visibility in
    /// alpha. Reads `scene` and `scene_depth`.
    pub const PROBE_FACE: &str = "probe_face";
    /// Everything drawn after the reflections: the sky, ground markers, sprites, particles and
    /// text, depth-tested against the opaque depth and writing none. No mesh is transparent.
    pub const TRANSPARENTS: &str = "transparents";
    /// Bright-pass threshold into the half-res bloom chain: it writes the pyramid's level 0.
    pub const BLOOM_THRESHOLD: &str = "bloom_threshold";
    /// The pyramid's progressive downsamples. **Index `i` is the pass that writes level `i + 1`,
    /// reading level `i`** — so `BLOOM_DOWN[0]` halves the bright-pass target and `BLOOM_DOWN[4]`
    /// writes the coarsest level. They execute in ascending index order.
    pub const BLOOM_DOWN: [&str; 5] = [
        "bloom_down_1",
        "bloom_down_2",
        "bloom_down_3",
        "bloom_down_4",
        "bloom_down_5",
    ];
    /// The pyramid's tent-filter upsamples. **Index `i` is the pass that writes level `i`**, reading
    /// the coarser accumulation and
    /// level `i`'s own downsample result. They execute in *descending* index order (4 → 0), so
    /// `BLOOM_UP[0]` runs last and is what `composite` mixes in.
    pub const BLOOM_UP: [&str; 5] = [
        "bloom_up_0",
        "bloom_up_1",
        "bloom_up_2",
        "bloom_up_3",
        "bloom_up_4",
    ];
    /// The depth+normal prepass: every opaque mesh rasterized into its own depth attachment and a
    /// world-space normal target, ahead of the scene pass. Present only when the scene enables
    /// ambient occlusion ([`PostChain::ssao`](super::PostChain::ssao), `Plan/post-process-pipeline.md` decision 8).
    ///
    /// It owns its depth rather than sharing `scene_depth`, so the scene pass keeps its own `Less`
    /// test and clear.
    pub const PREPASS: &str = "prepass";
    /// The dynamic-light injection: the frame graph's one **compute** pass, replaying the GI bake's
    /// cached hits against a world-space bin of the scene's `GiDynamic`-marked lights into a
    /// transient probe-lattice volume the scene pass samples
    /// (`Plan/gi-wave3-dynamic-injection.md` §5). Present only when
    /// [`PostChain::gi_inject`](super::PostChain::gi_inject) is `Some` — a scene with a `GiVolume`
    /// at a non-zero `bounce`, at least one marked light, and a machine whose
    /// [`Quality::gi_dynamic`](crate::Quality::gi_dynamic) is on.
    pub const GI_INJECT: &str = "gi_inject";
    /// Screen-space ambient occlusion from the prepass's depth and normals, into a half-resolution
    /// factor. [`PostChain::ssao`](super::PostChain::ssao) only.
    pub const SSAO: &str = "ssao";
    /// The 4×4 box blur that cancels [`SSAO`]'s per-pixel rotation — its width is the rotation tile's
    /// period, not a quality dial. [`PostChain::ssao`](super::PostChain::ssao) only.
    pub const SSAO_BLUR: &str = "ssao_blur";
    /// The volumetric light-shaft march: the sun shadow map ray-marched along the eye ray into a
    /// half-resolution in-scatter term. Present only when [`PostChain::shafts`](super::PostChain::shafts) — the scene asks for
    /// shafts *and* carries a sun (`Plan/post-process-pipeline.md` decision 13).
    pub const SHAFTS: &str = "shafts";
    /// The 4×4 box blur that cancels [`SHAFTS`]'s per-pixel step offset. The same shader as
    /// [`SSAO_BLUR`] over a different target — one filter, two pass names, because a budget and
    /// `--dump-timings` attribute cost to a pass. [`PostChain::shafts`](super::PostChain::shafts) only.
    pub const SHAFTS_BLUR: &str = "shafts_blur";
    /// The depth-of-field prefilter: the scene at a fraction of the output extent with each texel's
    /// signed circle of confusion in its alpha channel. [`PostChain::dof`](super::PostChain::dof) only.
    pub const DOF_COC: &str = "dof_coc";
    /// The depth-of-field gather: one disc of taps over [`DOF_COC`]'s colour+CoC target, writing the
    /// defocused image and the weight `composite` blends it in with. [`PostChain::dof`](super::PostChain::dof) only.
    pub const DOF: &str = "dof";
    /// Scene + bloom composited, then exposure, the tonemap operator and the output dither, HDR→sRGB
    /// into the output (or into the `ldr` transient when UI is present or the frame is
    /// supersampled). Everything that is a pure per-pixel function of already-sampled colour folds
    /// into this one pass rather than costing a full-resolution round trip of its own
    /// (`Plan/post-process-pipeline.md` decision 2).
    pub const COMPOSITE: &str = "composite";
    /// The supersampling downsample: an area-weighted box filter from the render extent to the
    /// output extent. Present only when
    /// [`FrameSize::supersampled`](crate::FrameSize::supersampled), and always after `composite` and
    /// before `overlay`, so screen-space UI is never resampled.
    pub const RESOLVE: &str = "resolve";
    /// The sub-native magnification: a bilinear filter from the render extent up to the output
    /// extent, in the seam [`RESOLVE`] takes in the other direction (after `composite`, before
    /// `overlay`, so screen-space UI is drawn at the output extent and never magnified).
    ///
    /// Its own shader rather than a reuse of [`RESOLVE`]'s: under magnification a destination
    /// pixel's footprint falls inside a single source texel, so the area-weighted box degenerates to
    /// nearest-neighbour and the frame arrives as blocks.
    pub const UPSCALE: &str = "upscale";
    /// Screen-space UI overlay: blit the tonemapped LDR image, then draw the sorted UI rects/text on
    /// top, into the output. Only present when the scene carries UI (`Plan/phase-14-ui.md` dec. 5).
    pub const OVERLAY: &str = "overlay";
}

/// A validated render-scale factor: finite, at least `0.25`, at most `4.0`.
///
/// A **client** setting, never scene content (`Plan/post-process-pipeline.md` decision 4) — the same
/// scene is the same scene at any scale, and content must not dictate the client's GPU budget. It is
/// a newtype rather than a bare `f32` so the range is checked once, where the number enters the
/// engine, instead of at every render entry point.
///
/// One range covers both directions, because they are one decision: above `1.0` the chain draws
/// larger than it presents and a box filter resolves it back down, below `1.0` it draws smaller and a
/// bilinear filter magnifies it. Either way the caller gets back the extent it asked for and the UI
/// is drawn at that extent, so a scale is a quality change and never a look change.
///
/// # Example
/// ```
/// use ludion_renderer::RenderScale;
///
/// assert_eq!(RenderScale::new(2.0).unwrap().factor(), 2.0);
/// assert_eq!(RenderScale::new(0.5).unwrap().factor(), 0.5);
/// assert!(RenderScale::NATIVE.is_native());
/// assert_eq!(
///     RenderScale::new(0.1).unwrap_err().code.as_str(),
///     "L4011",
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct RenderScale(f32);

impl RenderScale {
    /// The smallest accepted factor. At a sixteenth of native's pixels one source texel already
    /// spans a 4×4 block of output pixels, and below that what the magnifier reconstructs is its own
    /// filter rather than the frame.
    pub const MIN: f32 = 0.25;

    /// The largest accepted factor. Past 16× the pixels of native the cost grows as the square while
    /// the resolve's box filter has nothing left to average.
    pub const MAX: f32 = 4.0;

    /// Native resolution: the render extent equals the output extent, no resampling pass, and the
    /// output is byte-identical to a pre-render-scale render. Also [`Default`].
    pub const NATIVE: RenderScale = RenderScale(1.0);

    /// Validates `factor` as a render scale.
    ///
    /// # Errors
    /// `L4011` if it is not finite, is below [`MIN`](RenderScale::MIN), or is above
    /// [`MAX`](RenderScale::MAX).
    pub fn new(factor: f32) -> Result<RenderScale, ludion_core::diagnostics::Diagnostic> {
        if factor.is_finite() && (RenderScale::MIN..=RenderScale::MAX).contains(&factor) {
            return Ok(RenderScale(factor));
        }
        Err(ludion_core::diagnostics::Diagnostic::new(
            ludion_core::codes::code::INVALID_RENDER_SCALE,
            format!(
                "render scale `{factor}` is not a finite number between {} and {}",
                RenderScale::MIN,
                RenderScale::MAX
            ),
        )
        .with_help(
            "pass a factor in [0.25, 4.0] — 1.0 is native, 1.5 and 2.0 supersample for quality, \
             0.75 and 0.5 render below native and magnify back up for speed",
        ))
    }

    /// The validated factor.
    pub fn factor(self) -> f32 {
        self.0
    }

    /// Whether this is exactly native resolution (and so adds no resampling pass).
    pub fn is_native(self) -> bool {
        self.0 == 1.0
    }
}

impl Default for RenderScale {
    fn default() -> Self {
        RenderScale::NATIVE
    }
}

/// The two extents one frame is built from: what the client asked for, and what the chain draws at.
///
/// Plain `Copy` data, so [`build_frame_graph`] stays a pure function of its arguments. Collapsing the
/// pair into one value rather than growing every render entry point a third and fourth integer is the
/// same move [`PostChain`] made for the conditional-pass flags: the arity stops exploding.
///
/// See the module docs for the rule about which targets are sized from which extent — it is the whole
/// reason supersampling costs no shader change.
///
/// # Example
/// ```
/// use ludion_renderer::{FrameSize, RenderScale};
///
/// let native = FrameSize::native(1920, 1080);
/// assert_eq!(native.render, native.output);
/// assert!(!native.supersampled());
///
/// let ssaa = FrameSize::scaled(1920, 1080, RenderScale::new(1.5).unwrap());
/// assert_eq!(ssaa.render, (2880, 1620));
/// assert_eq!(ssaa.output, (1920, 1080));
/// assert!(ssaa.supersampled());
///
/// let cheap = FrameSize::scaled(1920, 1080, RenderScale::new(0.5).unwrap());
/// assert_eq!(cheap.render, (960, 540));
/// assert_eq!(cheap.output, (1920, 1080));
/// assert!(!cheap.supersampled());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSize {
    /// The image the caller gets back: the readback target's extent, the extent screen-space UI is
    /// laid out in, and the extent every post target but `ldr` is sized from.
    pub output: (u32, u32),
    /// The extent the scene and the composite are drawn at (`output × scale`, rounded, at least 1).
    pub render: (u32, u32),
}

impl FrameSize {
    /// A frame rendered at its output resolution — no supersampling, no resolve pass.
    pub fn native(width: u32, height: u32) -> FrameSize {
        FrameSize {
            output: (width, height),
            render: (width, height),
        }
    }

    /// A `width` × `height` frame rendered at `scale`, rounded per axis and clamped to at least one
    /// texel.
    pub fn scaled(width: u32, height: u32, scale: RenderScale) -> FrameSize {
        let axis = |n: u32| ((n as f32 * scale.factor()).round() as u32).max(1);
        FrameSize {
            output: (width, height),
            render: (axis(width), axis(height)),
        }
    }

    /// Checks that both extents can be allocated: every output and render axis is at least 1 px
    /// and at most [`MAX_TEXTURE_DIMENSION_2D`], the 2D limit the engine's device is pinned to. The
    /// one gate every render entry point reaches, so a bad `render.size`, `--scale` or
    /// `--width/--height` is a diagnostic rather than a GPU validation error.
    ///
    /// # Errors
    /// `L2012` naming the output extent, the scale-derived render extent and the limit.
    pub fn validate(self) -> Result<FrameSize, Diagnostic> {
        let axes = [self.output.0, self.output.1, self.render.0, self.render.1];
        if axes
            .iter()
            .all(|&axis| (1..=MAX_TEXTURE_DIMENSION_2D).contains(&axis))
        {
            return Ok(self);
        }
        Err(Diagnostic::new(
            code::RENDER_EXTENT_INVALID,
            format!(
                "the frame extent {}x{} px (render {}x{} px after scale) is outside 1..={} px per \
                 axis",
                self.output.0,
                self.output.1,
                self.render.0,
                self.render.1,
                MAX_TEXTURE_DIMENSION_2D,
            ),
        )
        .with_help(
            "lower `render.size`, `--scale`, or `--width/--height` so that `output x scale` fits \
             in 8192 px per axis",
        ))
    }

    /// Whether the chain draws at a **larger** extent than it presents — the condition that adds the
    /// [`resolve`](pass::RESOLVE) pass. A scale that rounds back onto the output extent (1.0, or
    /// 1.0001 on a small frame) is *not* supersampled: there would be nothing to resolve.
    pub fn supersampled(self) -> bool {
        self.resample() == Some(pass::RESOLVE)
    }

    /// The pass that carries the render extent to the output extent, or `None` when they are equal:
    /// [`resolve`](pass::RESOLVE) when the frame drew larger than it presents,
    /// [`upscale`](pass::UPSCALE) when it drew smaller. One value, because the direction is the whole
    /// of what the graph's tail depends on.
    ///
    /// # Panics
    /// A frame that grew on one axis and shrank on the other has no answer here, and none is
    /// derivable: one pass reads one texture through one ratio. [`scaled`](FrameSize::scaled) applies
    /// a single factor to both axes and so cannot build one, but the fields are `pub`, so the mixed
    /// pair stays *representable* and is asserted here instead. Typing it away would mean sealing the
    /// two extents behind accessors, which costs every reader of `output`/`render` its field access
    /// to exclude a case no constructor produces.
    pub fn resample(self) -> Option<&'static str> {
        let ((rw, rh), (ow, oh)) = (self.render, self.output);
        let grew = rw >= ow && rh >= oh;
        let shrank = rw <= ow && rh <= oh;
        assert!(
            grew || shrank,
            "a frame cannot magnify one axis and minify the other: render {:?}, output {:?}",
            self.render,
            self.output,
        );
        // Both at once is the equal case, which resamples through no pass at all.
        match (grew, shrank) {
            (true, true) => None,
            (true, false) => Some(pass::RESOLVE),
            _ => Some(pass::UPSCALE),
        }
    }

    /// The extent every half-resolution post target takes: half the **output** extent, clamped to at
    /// least one texel (the rule in the module docs). One place, so the bloom, ambient-occlusion,
    /// light-shaft and depth-of-field chains cannot disagree about what "half" means. Half of the
    /// *output*, never of the render buffer — so below native it is the larger of the two, which is
    /// the rule working rather than failing.
    pub(crate) fn half_post(self) -> (u32, u32) {
        ((self.output.0 / 2).max(1), (self.output.1 / 2).max(1))
    }
}

/// What a scene's content requires of the graph's *structure* — everything but the resolution.
///
/// Plain `Copy` data, so [`build_frame_graph`] stays a pure function of its arguments: no GPU, no
/// scene reference, a byte-stable dump. [`PostChain::default()`] is the minimal chain (no UI, no shadow map,
/// none of the conditional effects) — the fixed fifteen-pass graph; derive the real one from a scene
/// with [`PostChain::for_scene`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PostChain {
    /// The scene carries screen-space UI: `composite` retargets to the `ldr` transient and a trailing
    /// `overlay` pass is appended (`Plan/phase-14-ui.md` decision 5).
    pub ui: bool,
    /// The edge length of the square sun shadow map this frame draws, or `None` when it draws none
    /// — a depth-only `shadow` pass is prepended and the scene pass samples its map exactly when
    /// this is `Some` (`Plan/shadow-mapping.md` decision 6).
    ///
    /// It carries the extent for the reason [`gi_inject`](PostChain::gi_inject) does: a resource's
    /// extent *is* graph structure, so the transient cannot be declared without it, and keeping it
    /// here leaves [`build_frame_graph`] a pure function of `(size, chain)`.
    ///
    /// **It says nothing about whether the scene has a sun.** A sun lights the mesh pass through the
    /// `Sun` uniform, which every frame uploads and no graph structure depends on, so a machine that
    /// masks shadows off ([`Quality::shadows`]) renders a sunlit shadowless frame rather than a
    /// sunless one.
    pub shadow_map: Option<u32>,
    /// The scene enables ambient occlusion: the [`pass::PREPASS`], [`pass::SSAO`] and
    /// [`pass::SSAO_BLUR`] passes are prepended and the scene pass samples the result
    /// (`Plan/post-process-pipeline.md` decision 8). False leaves the graph with none of the three,
    /// which is what makes an AO-free frame byte-identical by construction rather than by care.
    pub ssao: bool,
    /// The scene enables volumetric light shafts, carries a sun **and** authors a medium for them to
    /// scatter in: the [`pass::SHAFTS`] and [`pass::SHAFTS_BLUR`] passes are added behind the scene
    /// pass and `composite` gains its one optional input.
    ///
    /// All three parts of the condition are load-bearing, because the march estimates one term of a
    /// participating medium: without a shadow map there is nothing to march (which is also why
    /// [`masked`](PostChain::masked) drops the pair on a machine whose shadows are off), and without extinction
    /// there is nothing to scatter — the estimate's coefficient is σ_s = `fog_albedo` · `fog_density`,
    /// so an empty medium returns exactly zero from thirty-two steps and a blur. A scene missing either
    /// derives `false` here and gets none of the passes, which is the component's standing "absence is
    /// meaningful" posture rather than an error.
    pub shafts: bool,
    /// The scene enables depth of field: the [`pass::DOF_COC`] and [`pass::DOF`] passes are added
    /// behind the scene pass and `composite` gains its second optional input. False leaves the graph
    /// with neither, which is what makes an in-focus frame byte-identical by construction rather than
    /// by care.
    pub dof: bool,
    /// The probe lattice the [`pass::GI_INJECT`] compute pass writes, or `None` when this frame
    /// schedules no injection at all — the whole scheduling predicate, in one value
    /// (`Plan/gi-wave3-dynamic-injection.md` §5).
    ///
    /// It carries the lattice rather than a bare flag because the transient volume's extent *is*
    /// graph structure: the pass writes one texel per probe, so the resource cannot be declared
    /// without it. That keeps [`build_frame_graph`] a pure function of `(size, chain)` — no scene
    /// reference, no GPU, a byte-stable dump — exactly as the flags beside it do.
    ///
    /// `None` leaves the graph with neither the pass nor the volume, which is what makes a frame
    /// with no marked light (or a machine with the effect off) byte-identical by construction rather
    /// than by care.
    pub gi_inject: Option<[u32; 3]>,
    /// The scene carries world-space debug lines: the `overlay` pass is appended exactly as for
    /// [`ui`](PostChain::ui), and additionally reads `scene_depth` so the lines can be dimmed where
    /// geometry hides them.
    pub debug_lines: bool,
    /// The trace's scale against the output extent, or `None` when the scene draws no screen-space
    /// reflections: the [`pass::HIZ_SEED`] / [`pass::HIZ_REDUCE`] and
    /// [`pass::SSR_COLOR_SEED`] / [`pass::SSR_COLOR_REDUCE`] pyramid passes are added between
    /// `scene` and `reflections`, building the `hiz` and `ssr_color` transients at the trace extent.
    /// `None` leaves the graph with neither the resources nor the passes, which is what makes an
    /// SSR-free frame byte-identical by construction rather than by care.
    ///
    /// It carries the scale for the reason [`shadow_map`](PostChain::shadow_map) carries its edge: a
    /// resource's extent is graph structure.
    pub ssr: Option<f32>,
    /// A full-screen view of the SSR trace: [`pass::SSR_DEBUG`] replaces `composite`, writing the
    /// same target. Only ever set with [`ssr`](PostChain::ssr): a view never turns the trace on, so
    /// `for_scene` and `masked` drop it whenever SSR is absent.
    pub ssr_view: Option<crate::DebugView>,
    /// A full-screen view of the winning probe: [`pass::PROBE_DEBUG`] replaces `composite`, writing
    /// the same target. Only set when the scene has probes: a view never conjures its subject, so
    /// a probe-free scene draws the normal frame. Probes have no quality toggle, so `masked` keeps it.
    pub probe_view: bool,
}

impl PostChain {
    /// The chain `scene`'s content requires — the one place the "which passes does this scene need"
    /// question is answered, so every render path and the `dump-graph` command cannot disagree about
    /// what a given scene draws.
    pub fn for_scene(scene: &RenderScene) -> Self {
        Self {
            ui: !scene.ui.is_empty(),
            // A sun draws a shadow map, at the extent the scene's own authoring implies. Lowering it
            // — or dropping it — belongs to `masked`: the ceiling is the scene's, the discount is
            // the machine's.
            shadow_map: scene
                .sun
                .is_some()
                .then_some(QualityParams::FULL.shadow_map_size),
            // `ssao` is a strength *and* the gate: at zero the effect contributes nothing, so
            // scheduling three passes to compute it would be work with no output.
            ssao: scene.post.is_some_and(|post| post.ssao > 0.0),
            // `shafts` is a strength *and* one third of the gate. The sun is the second third, because
            // the march reads the shadow map a sun is what builds; the medium's extinction is the
            // last, because the march estimates that medium's in-scatter and scales it by
            // σ_s = albedo · density — with no medium the two passes compute an exact zero.
            shafts: scene
                .post
                .is_some_and(|post| post.shafts > 0.0 && post.fog_density > 0.0)
                && scene.sun.is_some(),
            // `dof` is the far field's circle of confusion *and* the gate: at zero every pixel is in
            // focus, so the gather would return each pixel's own colour and the blend weight would be
            // zero everywhere — two passes to reproduce their own input.
            dof: scene.post.is_some_and(|post| post.dof > 0.0),
            // The injection's three-part predicate lives in `gi_inject.rs`, beside the pass that
            // implements it, so the graph and the recorder read one function rather than two copies
            // of a conjunction.
            gi_inject: crate::gi_inject::scheduled_lattice(scene),
            debug_lines: !scene.debug_lines.is_empty(),
            // `ssr` is a strength *and* the gate: at zero the blend takes the sky fill alone, so
            // tracing would be work with no output. Machines scale it down in `masked`.
            ssr: scene
                .post
                .is_some_and(|post| post.ssr > 0.0)
                .then_some(QualityParams::FULL.ssr_scale),
            ssr_view: scene
                .debug_view
                .filter(|v| v.is_ssr())
                .filter(|_| scene.post.is_some_and(|post| post.ssr > 0.0)),
            probe_view: scene.debug_view == Some(crate::DebugView::ProbeIndex)
                && !scene.probes.is_empty(),
        }
    }

    /// The debug view this chain draws in place of the finished picture, if any.
    pub fn view(&self) -> Option<crate::DebugView> {
        self.ssr_view
            .or_else(|| self.probe_view.then_some(crate::DebugView::ProbeIndex))
    }

    /// The chain the scene requires, minus what this machine's [`Quality`] disables — masking an
    /// effect is indistinguishable from the scene not authoring it: the passes are simply absent, so
    /// the frame is byte-identical to that scene's, never a cheaper version of this one's.
    ///
    /// [`ui`](PostChain::ui) and [`debug_lines`](PostChain::debug_lines) pass through untouched: they
    /// are content, and a HUD that vanished is a different frame rather than a cheaper one. The sun's shadows are the one masked thing that is
    /// not an effect a scene could have left unauthored — see [`Quality::shadows`] for what stays
    /// lit.
    ///
    /// `params` reaches the chain for exactly the three axes that are *resource extents* — the shadow
    /// map's edge, the GI probe lattice's dimensions and the SSR trace's scale — because an extent is graph structure,
    /// while every other count is a uniform the owning pass uploads.
    pub fn masked(self, quality: &Quality, params: &QualityParams) -> PostChain {
        // Off removes the map; on redraws it at this machine's extent rather than the scene-implied
        // ceiling `for_scene` filled in.
        let shadow_map = self
            .shadow_map
            .filter(|_| quality.shadows)
            .map(|_| params.shadow_map_size);
        PostChain {
            shadow_map,
            ssao: self.ssao && quality.ssao,
            // The march reads the shadow map, so the pair cannot outlive it. Keeping the map alive
            // for the shafts alone was the alternative and is worse: "shadows off" would then still
            // schedule the `shadow` pass, and masking would mean weakening rather than absence.
            shafts: self.shafts && quality.shafts && shadow_map.is_some(),
            dof: self.dof && quality.dof,
            // Off removes the trace and its pyramids; on re-sizes them to this machine's extent.
            ssr: self.ssr.filter(|_| quality.ssr).map(|_| params.ssr_scale),
            ssr_view: self.ssr_view.filter(|_| self.ssr.is_some() && quality.ssr),
            probe_view: self.probe_view,
            // Off removes the pass and its volume; on re-lays the lattice at this machine's coarser
            // probe spacing, which only ever removes probes (`gi_bake::scaled_probe_count`). The
            // spacing itself cancels out of that arithmetic, which is what lets a scene-free
            // function answer it.
            gi_inject: self
                .gi_inject
                .filter(|_| quality.gi_dynamic)
                .map(|count| crate::gi_bake::scaled_probe_count(count, params.gi_probe_scale)),
            ..self
        }
    }
}

/// The pyramid's level count below the bright pass: levels 1..=[`PYRAMID_LEVELS`], each written by
/// one [`pass::BLOOM_DOWN`] pass and read back by one [`pass::BLOOM_UP`] pass.
const PYRAMID_LEVELS: usize = 5;

/// The ambient-occlusion chain's transient targets, declared together when [`PostChain::ssao`] is on.
///
/// The split of extents is the extent rule (see the module docs): the prepass is *geometry*,
/// so its two targets are sized from the render extent exactly like `scene`/`scene_depth`, while the
/// AO factor and its blur are post and are sized from the output extent. That is what makes the
/// effect's screen footprint — and its cost — a fixed fraction of the frame at any render scale.
#[derive(Clone, Copy)]
struct AoTargets {
    /// The prepass's world-space normals (`Rgba16Float`, so a normal is stored signed and needs no
    /// encode/decode pair).
    normal: ResourceId,
    /// The prepass's own depth attachment, sampled by `ssao`.
    depth: ResourceId,
    /// The raw per-pixel occlusion factor, before the rotation is blurred out.
    raw: ResourceId,
    /// The blurred factor the scene pass samples.
    blurred: ResourceId,
}

/// The light-shaft chain's transient targets, declared together when [`PostChain::shafts`] is on.
///
/// Both are **post**, so both take a fraction of the *output* extent under the extent rule (see the
/// module docs) — which is what makes a beam land on the same pixels at any render scale, and makes
/// the effect cost the same there too.
#[derive(Clone, Copy)]
struct ShaftTargets {
    /// The raw per-pixel in-scatter term, before the march's step offset is blurred out.
    raw: ResourceId,
    /// The blurred term `composite` tints with the sun's colour and adds.
    blurred: ResourceId,
}

/// The depth-of-field chain's transient targets, declared together when [`PostChain::dof`] is on.
///
/// Both are **post**, so both take a fraction of the *output* extent under the extent rule (see the
/// module docs) — which, with a circle of confusion expressed as a fraction of the frame's height, is
/// what makes the blur cover the same part of the image, and cost the same, at any render scale.
#[derive(Clone, Copy)]
struct DofTargets {
    /// The prefilter's colour (rgb) and signed circle of confusion (a).
    coc: ResourceId,
    /// The gathered image (rgb) and the weight `composite` blends it in with (a).
    gathered: ResourceId,
}

/// The bloom pyramid's transient targets. Declared up front — before `final`, whose position in the
/// resource list is pinned by the frozen graph — and consumed when the chain's passes are added.
#[derive(Clone, Copy)]
struct BloomTargets {
    /// Level `i + 1`'s downsample target.
    down: [ResourceId; PYRAMID_LEVELS],
    /// Level `i`'s upsample accumulation.
    up: [ResourceId; PYRAMID_LEVELS],
}

/// Builds the frame graph for `size` and returns it with the handle of the final output target.
/// `chain` selects the conditional passes (see the module docs): the default chain at a native
/// [`FrameSize`] is the exact fifteen-pass graph — the scene, reflections, transparents, the bloom pyramid's eleven and the
/// composite — while `ui` retargets `composite` to an LDR transient and appends an `overlay` pass,
/// `shadow_map` prepends the depth-only `shadow` pass at its own extent, and a `size` whose two
/// extents differ inserts one resampling pass — `resolve` or `upscale` — between composite and
/// overlay.
///
/// # Example
/// ```
/// use ludion_renderer::{FrameSize, PostChain, RenderScale};
///
/// let native = FrameSize::native(1920, 1080);
/// let (graph, output) = ludion_renderer::build_frame_graph(native, PostChain::default());
/// assert_eq!(graph.passes().len(), 15);
/// assert_eq!(
///     graph.resource(output).kind,
///     ludion_render_graph::ResourceKind::Output
/// );
/// let ui = PostChain {
///     ui: true,
///     ..PostChain::default()
/// };
/// let (ui_graph, _) = ludion_renderer::build_frame_graph(native, ui);
/// assert_eq!(ui_graph.passes().len(), 16);
/// // A shadow map prepends the depth-only shadow pass; the scene pass then samples it.
/// let sun = PostChain {
///     shadow_map: Some(2048),
///     ..PostChain::default()
/// };
/// let (sun_graph, _) = ludion_renderer::build_frame_graph(native, sun);
/// assert_eq!(sun_graph.passes().len(), 16);
/// // A render scale adds exactly one pass in either direction, whatever the rest of the chain is.
/// let ssaa = FrameSize::scaled(1920, 1080, RenderScale::new(2.0).unwrap());
/// let (ssaa_graph, _) = ludion_renderer::build_frame_graph(ssaa, PostChain::default());
/// assert_eq!(ssaa_graph.passes().len(), 16);
/// let cheap = FrameSize::scaled(1920, 1080, RenderScale::new(0.5).unwrap());
/// let (cheap_graph, _) = ludion_renderer::build_frame_graph(cheap, PostChain::default());
/// assert_eq!(cheap_graph.passes().len(), 16);
/// // Ambient occlusion prepends the prepass, the estimate and its blur.
/// let ao = PostChain {
///     ssao: true,
///     ..PostChain::default()
/// };
/// let (ao_graph, _) = ludion_renderer::build_frame_graph(native, ao);
/// assert_eq!(ao_graph.passes().len(), 18);
/// // Light shafts add the march and its blur. `PostChain::for_scene` only derives this from a scene
/// // that has all three of a strength, a sun and a medium — see `PostChain::shafts`.
/// let shafts = PostChain {
///     shadow_map: Some(2048),
///     shafts: true,
///     ..PostChain::default()
/// };
/// let (shafts_graph, _) = ludion_renderer::build_frame_graph(native, shafts);
/// assert_eq!(shafts_graph.passes().len(), 18);
/// // Depth of field adds the prefilter and the gather.
/// let dof = PostChain {
///     dof: true,
///     ..PostChain::default()
/// };
/// let (dof_graph, _) = ludion_renderer::build_frame_graph(native, dof);
/// assert_eq!(dof_graph.passes().len(), 17);
/// ```
pub fn build_frame_graph(size: FrameSize, chain: PostChain) -> (RenderGraph, ResourceId) {
    // The scene and its depth are the only targets drawn at the render extent; every post target
    // below is a fraction of the *output* extent (see the module docs).
    let (render_w, render_h) = size.render;
    let (width, height) = size.output;
    let (half_w, half_h) = size.half_post();

    let mut graph = RenderGraph::new();

    // Full-resolution HDR scene target (declared first, so it is `resources[0]` in dump output).
    // Each pass writes a *distinct* virtual resource — the bloom chain never ping-pongs into a
    // resource it also reads (that would be a cycle). Transient aliasing reclaims the physical
    // memory wherever two levels' lifetimes are disjoint.
    let scene = graph.transient("scene", TextureDesc::hdr_color(render_w, render_h));
    // The scene pass's depth buffer, owned and aliased by the graph (declared next to the color
    // target it pairs with). Sampleable — not attachment-only — because `composite` reads it for
    // fog, and any pass reading scene depth needs the `TextureBinding` usage on this resource.
    let scene_depth = graph.transient(
        "scene_depth",
        TextureDesc::sampled_depth(render_w, render_h),
    );
    // The opaque pass's two extra targets, read by `reflections`: world normal + clamped roughness,
    // and F0 + specular occlusion (sRGB storage gives the dielectric F0 its precision).
    let scene_normal = graph.transient("scene_normal", TextureDesc::hdr_color(render_w, render_h));
    let scene_specular = graph.transient("scene_specular", TextureDesc::color(render_w, render_h));
    // The sun shadow map (only when this frame draws one, so a sunless scene's — or a
    // shadows-masked machine's — resource list is exactly the pre-shadow graph's,
    // `Plan/shadow-mapping.md` decision 6). A square sampled depth texture: written depth-only by
    // the `shadow` pass, sampled by the scene pass.
    let shadow_map = chain
        .shadow_map
        .map(|edge| graph.transient("shadow_map", TextureDesc::sampled_depth(edge, edge)));
    // The ambient-occlusion chain (only when the scene enables it, so an AO-free scene's resource list
    // is exactly the pre-SSAO graph's — the same inversion the shadow map rests on).
    let ao = chain.ssao.then(|| AoTargets {
        normal: graph.transient("prepass_normal", TextureDesc::hdr_color(render_w, render_h)),
        depth: graph.transient(
            "prepass_depth",
            TextureDesc::sampled_depth(render_w, render_h),
        ),
        raw: graph.transient("ssao", TextureDesc::hdr_color(half_w, half_h)),
        blurred: graph.transient("ssao_blur", TextureDesc::hdr_color(half_w, half_h)),
    });
    // The pyramid's level 0, written by the bright pass.
    let bloom_bright = graph.transient("bloom_bright", TextureDesc::hdr_color(half_w, half_h));
    // A pyramid level is always a fixed fraction of the *output* frame (level L is
    // `width >> (L + 1)`), never a fixed pixel count and never a fraction of the render buffer —
    // that is what keeps the glow's radius screen-relative, so raising the render scale raises
    // quality without narrowing the look.
    let level_extent = |level: usize| {
        (
            (width >> (level + 1)).max(1),
            (height >> (level + 1)).max(1),
        )
    };
    let bloom_targets = BloomTargets {
        down: std::array::from_fn(|i| {
            let (w, h) = level_extent(i + 1);
            graph.transient(pass::BLOOM_DOWN[i], TextureDesc::hdr_color(w, h))
        }),
        up: std::array::from_fn(|i| {
            let (w, h) = level_extent(i);
            graph.transient(pass::BLOOM_UP[i], TextureDesc::hdr_color(w, h))
        }),
    };
    // The light-shaft chain (only when the scene asks for shafts *and* has a sun to march), declared
    // after the bloom targets and still before `final`, so no existing resource list moves.
    let shafts = chain.shafts.then(|| ShaftTargets {
        raw: graph.transient(pass::SHAFTS, TextureDesc::hdr_color(half_w, half_h)),
        blurred: graph.transient(pass::SHAFTS_BLUR, TextureDesc::hdr_color(half_w, half_h)),
    });
    // The depth-of-field chain, declared after the shaft targets and still before `final` — the same
    // append-at-the-end discipline every conditional chain follows, so no existing resource list moves.
    let dof = chain.dof.then(|| DofTargets {
        coc: graph.transient(pass::DOF_COC, TextureDesc::hdr_color(half_w, half_h)),
        gathered: graph.transient(pass::DOF, TextureDesc::hdr_color(half_w, half_h)),
    });
    // The dynamic-light injection's output: a probe-lattice 3D volume, written whole by the one
    // compute pass every frame it exists and sampled by the scene pass. A transient like any other —
    // it holds no history, so there is nothing to carry between frames — and declared under the same
    // append-before-`final` discipline every conditional chain follows. It is the only resource in
    // the graph whose extent comes from scene content rather than the frame size, which is why
    // `PostChain` carries the lattice.
    let gi_dynamic = chain.gi_inject.map(|[cx, cy, cz]| {
        graph.transient(pass::GI_INJECT, TextureDesc::hdr_storage_volume(cx, cy, cz))
    });
    // The SSR input pyramids, at the trace extent (a post target: sized from the output). Declared
    // only when SSR is on, after the other conditional chains and before `final`.
    let ssr = chain.ssr.map(|scale| {
        declare_ssr_targets(&mut graph, size, scale, |graph, (tw, th)| {
            graph.transient("ssr_trace", TextureDesc::hdr_color(tw, th))
        })
    });
    // Declared before any post-composite resource so the output's handle (and hence the minimal
    // graph's resource list) is identical to the pre-UI graph.
    let output = graph.output("final", TextureDesc::present_target(width, height));
    // Which resampling pass this frame's two extents call for, if any — asked once, because the
    // three declarations below and the tail must agree about it.
    let resample = size.resample();
    // `ldr` is what `composite` writes whenever a pass still has to run after it — the overlay's
    // seam (the present target excludes `TextureBinding`, so `overlay` cannot read it directly) and
    // the resampling pass's source. It is at the *render* extent: the resampling pass is what brings
    // the frame to the output extent.
    let overlay = chain.ui || chain.debug_lines;
    let ldr = (overlay || resample.is_some())
        .then(|| graph.transient("ldr", TextureDesc::color(render_w, render_h)));
    // Only a resampled frame that also draws UI needs a second LDR seam: the resampling pass must
    // land somewhere the overlay can sample, and that is not the present target. It holds the frame
    // at the output extent whichever pass wrote it.
    let ldr_resolved = (overlay && resample.is_some())
        .then(|| graph.transient("ldr_resolved", TextureDesc::color(width, height)));

    // Everything the scene pass samples, in the order `record_scene` unpacks `pass.inputs`. Every
    // entry is conditional, so the recorder indexes by which flags are set rather than by a fixed
    // slot.
    let mut scene_reads = Vec::new();
    // The depth-only shadow pass writes the shadow map with no reads; the scene pass then samples it
    // (the read edge orders scene after shadow, `depth_only_pass_orders_before_a_later_sampler`).
    if let Some(shadow_map) = shadow_map {
        graph.add_external_pass_with_depth(pass::SHADOW, pass::SHADOW, vec![], vec![], shadow_map);
        scene_reads.push(shadow_map);
    }
    // Prepass → ssao → blur, each ordered by the next one's read, and the scene pass's read of the
    // blurred factor is what puts the whole chain in front of it.
    if let Some(ao) = ao {
        graph.add_external_pass_with_depth(
            pass::PREPASS,
            pass::PREPASS,
            vec![],
            vec![ao.normal],
            ao.depth,
        );
        graph.add_external_pass(
            pass::SSAO,
            pass::SSAO,
            vec![ao.depth, ao.normal],
            vec![ao.raw],
        );
        graph.add_external_pass(
            pass::SSAO_BLUR,
            pass::SSAO_BLUR,
            vec![ao.raw],
            vec![ao.blurred],
        );
        scene_reads.push(ao.blurred);
    }
    // The injection reads nothing the graph owns (its inputs — the bake's hit cache, the voxel
    // albedo, this frame's binned light set — are renderer-owned externals, exactly as the bake's
    // are); the scene pass's read of the volume is the ordinary edge that orders the two.
    if let Some(gi_dynamic) = gi_dynamic {
        graph.add_external_compute_pass(pass::GI_INJECT, pass::GI_INJECT, vec![], gi_dynamic);
        scene_reads.push(gi_dynamic);
    }
    graph.add_external_pass_with_depth(
        pass::SCENE,
        pass::SCENE,
        scene_reads,
        vec![scene, scene_normal, scene_specular],
        scene_depth,
    );
    // The pyramids sit between `scene` and `reflections`: a read follows the latest writer declared
    // before it, so the colour seed sees the opaque scene before `reflections` adds into it, and the
    // Hi-Z seed runs before `transparents` attach the depth.
    if let Some(ssr) = &ssr {
        declare_ssr_passes(&mut graph, ssr, scene, scene_normal, scene_depth);
    }
    // Reads in the shader's binding order; additive into `scene`, after the opaque pass. The trace
    // is the fourth read when SSR is on, and the edge that orders it before this pass.
    let mut reflections_reads = vec![scene_normal, scene_specular, scene_depth];
    if let Some(ssr) = &ssr {
        reflections_reads.push(ssr.trace);
    }
    graph.add_external_pass(
        pass::REFLECTIONS,
        pass::REFLECTIONS,
        reflections_reads,
        vec![scene],
    );
    graph.add_external_pass_with_depth(
        pass::TRANSPARENTS,
        pass::TRANSPARENTS,
        vec![],
        vec![scene],
        scene_depth,
    );
    graph.add_external_pass(
        pass::BLOOM_THRESHOLD,
        pass::BLOOM_THRESHOLD,
        vec![scene],
        vec![bloom_bright],
    );
    // The chain ends in one half-res resource the composite pass mixes in: level 0's accumulation.
    let bloom = {
        let BloomTargets { down, up } = bloom_targets;
        for level in 1..=PYRAMID_LEVELS {
            let source = if level == 1 {
                bloom_bright
            } else {
                down[level - 2]
            };
            let label = pass::BLOOM_DOWN[level - 1];
            graph.add_external_pass(label, label, vec![source], vec![down[level - 1]]);
        }
        // Upsampling walks back down the index, each pass reading [coarser accumulation, this
        // level's own downsample result] — the read order the recorder's bind group relies on.
        for level in (0..PYRAMID_LEVELS).rev() {
            let coarser = if level == PYRAMID_LEVELS - 1 {
                down[PYRAMID_LEVELS - 1]
            } else {
                up[level + 1]
            };
            let same_level = if level == 0 {
                bloom_bright
            } else {
                down[level - 1]
            };
            let label = pass::BLOOM_UP[level];
            graph.add_external_pass(label, label, vec![coarser, same_level], vec![up[level]]);
        }
        up[0]
    };
    // The march reads the scene pass's depth (so foreground geometry occludes the beams behind it)
    // and the shadow pass's map; its blur then feeds `composite`, whose read is what orders the pair
    // in front of it. Both edges are the scheduler's, derived from these reads.
    let shafts_term = shafts.map(|shafts| {
        let shadow_map = shadow_map.expect(
            "the march samples the shadow map: PostChain::for_scene requires a sun for shafts, and \
             PostChain::masked drops them with it",
        );
        graph.add_external_pass(
            pass::SHAFTS,
            pass::SHAFTS,
            vec![scene_depth, shadow_map],
            vec![shafts.raw],
        );
        graph.add_external_pass(
            pass::SHAFTS_BLUR,
            pass::SHAFTS_BLUR,
            vec![shafts.raw],
            vec![shafts.blurred],
        );
        shafts.blurred
    });
    // The prefilter reads the scene pass's colour *and* its depth — a two-edge read the
    // scheduler orders like any other — and the gather then reads the one
    // target it wrote. `composite`'s read of the gather is what orders the pair in front of it.
    let dof_term = dof.map(|dof| {
        graph.add_external_pass(
            pass::DOF_COC,
            pass::DOF_COC,
            vec![scene, scene_depth],
            vec![dof.coc],
        );
        graph.add_external_pass(pass::DOF, pass::DOF, vec![dof.coc], vec![dof.gathered]);
        dof.gathered
    });

    // The tail is a chain of at most three passes, each writing the output directly when it is the
    // last one. Every edge below is derived by the scheduler from the next pass's read.
    let composited = ldr.unwrap_or(output);
    // `scene_depth` is read on **every** frame, not only a fogged one: making it conditional would
    // fork `composite` into two bind group layouts and two pipelines for one shader, which the
    // sunless frame's zeroed uniform plus 1×1 fallback depth already refuses to do. The cost is that
    // the depth buffer now lives to the end of the frame instead of dying at the scene pass.
    //
    // The shaft term and the depth-of-field gather are the two inputs that genuinely cannot be
    // unconditional — neither producer exists on most frames — so both are declared **last**, after
    // every required read and in their binding order. That ordering is the graph half of the
    // optional-slot rule `fullscreen_effect` states: the recorder walks the optional slots in their
    // fixed binding order and consumes a real input for each present one, so the graph must declare
    // the present ones in that same order, behind the required reads.
    let mut composite_reads = vec![scene, bloom, scene_depth];
    composite_reads.extend(shafts_term);
    composite_reads.extend(dof_term);
    // A debug view stands in for the whole composite: the tail after it is unchanged. Bloom, shafts
    // and dof still schedule with unread outputs — acceptable for a debug frame.
    match (chain.ssr_view, &ssr) {
        _ if chain.probe_view => graph.add_external_pass(
            pass::PROBE_DEBUG,
            pass::PROBE_DEBUG,
            vec![scene_normal, scene_depth],
            vec![composited],
        ),
        (Some(_), Some(ssr)) => graph.add_external_pass(
            pass::SSR_DEBUG,
            pass::SSR_DEBUG,
            vec![ssr.trace],
            vec![composited],
        ),
        _ => graph.add_external_pass(
            pass::COMPOSITE,
            pass::COMPOSITE,
            composite_reads,
            vec![composited],
        ),
    };
    let tail = match (ldr, resample) {
        (Some(ldr), Some(resample)) => {
            let target = ldr_resolved.unwrap_or(output);
            graph.add_external_pass(resample, resample, vec![ldr], vec![target]);
            target
        }
        _ => composited,
    };
    if overlay {
        // The lines' x-ray test samples the scene's depth, so the read joins the overlay's inputs
        // behind the frame: `record_overlay` finds it at `inputs[1]` exactly when lines are drawn.
        let mut reads = vec![tail];
        if chain.debug_lines {
            reads.push(scene_depth);
        }
        graph.add_external_pass(pass::OVERLAY, pass::OVERLAY, reads, vec![output]);
    }

    (graph, output)
}

/// The frame graph's `dump-graph` JSON for `size` and `chain` — CPU-only, byte-stable, no GPU.
///
/// This is what `ludion dump-graph --scene …` prints, with the chain derived from the scene it names
/// ([`PostChain::for_scene`]) so the dump is the graph that scene actually renders. Scheduling and
/// transient aliasing run here, so an unschedulable graph would surface its `L2001`/`L2002`
/// diagnostic — but the frame graph is always acyclic and fully written.
///
/// # Example
/// ```
/// use ludion_renderer::{FrameSize, PostChain};
///
/// let size = FrameSize::native(1920, 1080);
/// let json = ludion_renderer::dump_frame_graph_json(size, PostChain::default()).unwrap();
/// assert!(json.contains("\"scene\""));
/// assert!(json.contains("\"composite\""));
/// ```
pub fn dump_frame_graph_json(
    size: FrameSize,
    chain: PostChain,
) -> Result<String, ludion_core::diagnostics::Diagnostic> {
    let (graph, _output) = build_frame_graph(size, chain);
    let scheduled = schedule(&graph)?;
    Ok(scheduled.dump_graph_json())
}

/// The SSR chain's transients: the two input pyramids and the trace's output.
struct SsrTargets {
    hiz: ResourceId,
    color: ResourceId,
    trace: ResourceId,
    levels: u32,
}

/// Declares `hiz`, `ssr_color` and then the trace target at the trace extent; `trace` builds the last
/// (the frame graph wants a transient, the test harness an output it can read back).
fn declare_ssr_targets(
    graph: &mut RenderGraph,
    size: FrameSize,
    scale: f32,
    trace: impl FnOnce(&mut RenderGraph, (u32, u32)) -> ResourceId,
) -> SsrTargets {
    let extent = crate::ssr_pyramid::trace_extent(size, scale);
    let levels = crate::ssr_pyramid::levels(extent);
    let (tw, th) = extent;
    let hiz = graph.transient(
        "hiz",
        TextureDesc::r32_float(tw, th).with_mip_levels(levels),
    );
    let color = graph.transient(
        "ssr_color",
        TextureDesc::hdr_color(tw, th).with_mip_levels(levels),
    );
    let trace = trace(graph, extent);
    SsrTargets {
        hiz,
        color,
        trace,
        levels,
    }
}

/// The pyramids' seed and reduce passes, then the trace. They sit between the opaque `scene` pass and
/// `reflections`: a read follows the latest writer declared before it, so the colour seed sees the
/// opaque scene before `reflections` adds into it, and the Hi-Z seed runs before `transparents`
/// attach the depth. The trace reads each pyramid whole, which orders it after every reduce.
fn declare_ssr_passes(
    graph: &mut RenderGraph,
    ssr: &SsrTargets,
    scene: ResourceId,
    scene_normal: ResourceId,
    scene_depth: ResourceId,
) {
    let SsrTargets {
        hiz,
        color,
        trace,
        levels,
    } = *ssr;
    graph.add_external_pass(
        pass::HIZ_SEED,
        pass::HIZ_SEED,
        vec![scene_depth],
        vec![hiz.mip(0)],
    );
    for k in 1..levels {
        graph.add_external_pass(
            format!("{}_{k}", pass::HIZ_REDUCE),
            pass::HIZ_REDUCE,
            vec![hiz.mip(k - 1)],
            vec![hiz.mip(k)],
        );
    }
    graph.add_external_pass(
        pass::SSR_COLOR_SEED,
        pass::SSR_COLOR_SEED,
        vec![scene],
        vec![color.mip(0)],
    );
    for k in 1..levels {
        graph.add_external_pass(
            format!("{}_{k}", pass::SSR_COLOR_REDUCE),
            pass::SSR_COLOR_REDUCE,
            vec![color.mip(k - 1)],
            vec![color.mip(k)],
        );
    }
    graph.add_external_pass(
        pass::SSR_TRACE,
        pass::SSR_TRACE,
        vec![hiz, color, scene_normal, scene_depth],
        vec![trace],
    );
}

/// The SSR trace and its inputs as a minimal graph whose `ssr_trace` is an **output** — it survives
/// to readback, where a transient could be aliased away and has no `COPY_SRC`. The opaque targets are
/// declared as `build_frame_graph` does, and the scene pass has no reads, as the recorder's has on a
/// sunless, AO-free frame.
#[cfg(test)]
pub(crate) fn build_ssr_trace_graph(size: FrameSize) -> (RenderGraph, ResourceId) {
    let (render_w, render_h) = size.render;
    let mut graph = RenderGraph::new();
    let scene = graph.transient("scene", TextureDesc::hdr_color(render_w, render_h));
    let scene_depth = graph.transient(
        "scene_depth",
        TextureDesc::sampled_depth(render_w, render_h),
    );
    let scene_normal = graph.transient("scene_normal", TextureDesc::hdr_color(render_w, render_h));
    let scene_specular = graph.transient("scene_specular", TextureDesc::color(render_w, render_h));
    graph.add_external_pass_with_depth(
        pass::SCENE,
        pass::SCENE,
        vec![],
        vec![scene, scene_normal, scene_specular],
        scene_depth,
    );
    let ssr = declare_ssr_targets(
        &mut graph,
        size,
        QualityParams::FULL.ssr_scale,
        |graph, (tw, th)| {
            let mut desc = TextureDesc::hdr_color(tw, th);
            desc.usages.push(ludion_render_graph::TextureUsage::CopySrc);
            graph.output("ssr_trace", desc)
        },
    );
    declare_ssr_passes(&mut graph, &ssr, scene, scene_normal, scene_depth);
    (graph, ssr.trace)
}

/// One reflection-probe cube face: the opaque scene lit as an ordinary frame (shadow, GI inject,
/// scene, reflections, transparents), then `probe_face` writes the linear HDR result with sky
/// visibility in alpha to the output. `chain` is the derived face scene's, which schedules no AO,
/// SSR, shafts, DoF, UI or debug lines.
pub(crate) fn build_probe_capture_graph(face: u32, chain: PostChain) -> (RenderGraph, ResourceId) {
    debug_assert!(
        !chain.ssao
            && chain.ssr.is_none()
            && chain.view().is_none()
            && !chain.shafts
            && !chain.dof
            && !chain.ui
            && !chain.debug_lines,
        "a probe face schedules none of the screen-space chains: {chain:?}"
    );
    let mut graph = RenderGraph::new();
    let scene = graph.transient("scene", TextureDesc::hdr_color(face, face));
    let scene_depth = graph.transient("scene_depth", TextureDesc::sampled_depth(face, face));
    let scene_normal = graph.transient("scene_normal", TextureDesc::hdr_color(face, face));
    let scene_specular = graph.transient("scene_specular", TextureDesc::color(face, face));
    let shadow_map = chain
        .shadow_map
        .map(|edge| graph.transient("shadow_map", TextureDesc::sampled_depth(edge, edge)));
    let gi_dynamic = chain.gi_inject.map(|[cx, cy, cz]| {
        graph.transient(pass::GI_INJECT, TextureDesc::hdr_storage_volume(cx, cy, cz))
    });
    let mut desc = TextureDesc::hdr_color(face, face);
    desc.usages.push(ludion_render_graph::TextureUsage::CopySrc);
    let output = graph.output(pass::PROBE_FACE, desc);

    // Reads in `build_frame_graph`'s order: `record_scene` unpacks them by flag.
    let mut scene_reads = Vec::new();
    if let Some(shadow_map) = shadow_map {
        graph.add_external_pass_with_depth(pass::SHADOW, pass::SHADOW, vec![], vec![], shadow_map);
        scene_reads.push(shadow_map);
    }
    if let Some(gi_dynamic) = gi_dynamic {
        graph.add_external_compute_pass(pass::GI_INJECT, pass::GI_INJECT, vec![], gi_dynamic);
        scene_reads.push(gi_dynamic);
    }
    graph.add_external_pass_with_depth(
        pass::SCENE,
        pass::SCENE,
        scene_reads,
        vec![scene, scene_normal, scene_specular],
        scene_depth,
    );
    graph.add_external_pass(
        pass::REFLECTIONS,
        pass::REFLECTIONS,
        vec![scene_normal, scene_specular, scene_depth],
        vec![scene],
    );
    graph.add_external_pass_with_depth(
        pass::TRANSPARENTS,
        pass::TRANSPARENTS,
        vec![],
        vec![scene],
        scene_depth,
    );
    graph.add_external_pass(
        pass::PROBE_FACE,
        pass::PROBE_FACE,
        vec![scene, scene_depth],
        vec![output],
    );
    (graph, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extent gate: 1..=8192 px on every output and render axis, and the message names both
    /// extents so a bad `--scale` reads as the product it is.
    #[test]
    fn validate_bounds_output_and_render_axes() {
        let scale = |f: f32| crate::RenderScale::new(f).unwrap();
        for ok in [
            FrameSize::native(1, 1),
            FrameSize::native(7680, 4320),
            FrameSize::native(8192, 8192),
            FrameSize::scaled(1920, 1080, scale(4.0)),
            FrameSize::scaled(2048, 1024, scale(4.0)),
        ] {
            assert_eq!(ok.validate().unwrap(), ok);
        }
        for bad in [
            FrameSize::native(0, 1080),
            FrameSize::native(1920, 0),
            FrameSize::native(8193, 100),
            FrameSize::scaled(7680, 4320, scale(2.0)),
            FrameSize::scaled(3000, 1000, scale(4.0)),
        ] {
            let error = bad.validate().unwrap_err();
            assert_eq!(error.code.as_str(), "L2012", "{bad:?}");
        }
        let message = FrameSize::scaled(7680, 4320, scale(2.0))
            .validate()
            .unwrap_err()
            .message;
        assert!(message.contains("7680x4320"), "{message}");
        assert!(message.contains("15360x8640"), "{message}");
        assert!(message.contains("8192"), "{message}");
        assert!(!message.contains("  "), "no run of spaces: {message}");
    }

    fn pass_names(graph: &RenderGraph) -> Vec<&str> {
        graph.passes().iter().map(|p| p.name.as_str()).collect()
    }

    /// The capture graph's pass order, with and without a shadow map and a GI injection, and an
    /// output a copy can read.
    #[test]
    fn the_probe_capture_graph_orders_its_passes() {
        let bare = PostChain::default();
        let (graph, output) = build_probe_capture_graph(64, bare);
        assert_eq!(
            pass_names(&graph),
            ["scene", "reflections", "transparents", "probe_face"]
        );
        let lit = PostChain {
            shadow_map: Some(256),
            gi_inject: Some([2, 2, 2]),
            ..bare
        };
        let (graph, output_lit) = build_probe_capture_graph(64, lit);
        assert_eq!(
            pass_names(&graph),
            [
                "shadow",
                "gi_inject",
                "scene",
                "reflections",
                "transparents",
                "probe_face"
            ]
        );
        for (graph, output) in [
            (build_probe_capture_graph(64, bare).0, output),
            (graph, output_lit),
        ] {
            schedule(&graph).expect("the capture graph schedules");
            let desc = &graph.resources()[output.index()].desc;
            assert!(
                desc.usages
                    .contains(&ludion_render_graph::TextureUsage::CopySrc)
            );
            assert_eq!(desc.extent.width, 64);
        }
    }

    fn resource_names(graph: &RenderGraph) -> Vec<&str> {
        graph.resources().iter().map(|r| r.name.as_str()).collect()
    }

    /// A scene carrying neither UI nor a sun, and asking for none of the conditional effects,
    /// derives exactly the default chain — bloom is the one post effect that is always on, so the
    /// default chain is the scene pass, the pyramid's eleven and the composite.
    #[test]
    fn a_bare_scene_derives_the_default_chain() {
        let mut scene = RenderScene::empty((1920, 1080));
        assert_eq!(PostChain::for_scene(&scene), PostChain::default());
        // A fully-default `PostProcess` derives the same chain a scene with none does: every
        // pass-adding field is a strength that doubles as its own gate, and all of them ship at 0.
        scene.post = Some(crate::scene::PostData::default());
        assert_eq!(PostChain::for_scene(&scene), PostChain::default());
    }

    /// **Ground markers never move the frame graph.** They are drawn inside the `scene` pass, between
    /// the opaque meshes and the sprites — draw order is the only thing that keeps a telegraph *under*
    /// the character standing on it, since sprites are alpha-blended and write no depth, so a trailing
    /// pass of its own could not sit there. Golden neutrality is therefore the sky triangle's
    /// inversion rather than the overlay's: the chain, the pass list and the resource list are
    /// identical whether a scene authors markers or not, and an empty `ground_markers` records no
    /// draw at all (`Renderer::build_ground_marker_draw`).
    #[test]
    fn ground_markers_leave_the_graph_untouched() {
        let mut scene = RenderScene::empty((1920, 1080));
        let bare = PostChain::for_scene(&scene);
        scene
            .ground_markers
            .push(crate::scene::GroundMarkerInstance {
                shape: crate::scene::GroundMarkerShape::Disc { radius: 3.0 },
                model: ludion_core::math::glam::Mat4::IDENTITY,
                color: [4.0, 0.4, 0.2, 0.6],
            });
        let marked = PostChain::for_scene(&scene);
        assert_eq!(
            marked, bare,
            "a marker is content the post chain must not notice",
        );

        let size = FrameSize::native(1920, 1080);
        let (bare_graph, _) = build_frame_graph(size, bare);
        let (marked_graph, _) = build_frame_graph(size, marked);
        // (a) the marker-free scene keeps the frozen fifteen-pass graph, verbatim …
        assert_eq!(
            pass_names(&bare_graph),
            [
                "scene",
                "reflections",
                "transparents",
                "bloom_threshold",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_4",
                "bloom_up_3",
                "bloom_up_2",
                "bloom_up_1",
                "bloom_up_0",
                "composite",
            ],
        );
        // … and (b) a scene that authors markers gets exactly the same passes and resources.
        assert_eq!(pass_names(&marked_graph), pass_names(&bare_graph));
        assert_eq!(resource_names(&marked_graph), resource_names(&bare_graph));
    }

    /// **Masking is transparent at `FULL` and surgical at `LOW`**, over every chain a scene can
    /// derive — all forty-eight. The first half is what makes the shipped default byte-identical to a
    /// renderer that never consulted a quality at all; the second is that a machine's switch reaches
    /// exactly the five optional things and never the `ui` flag beside them.
    #[test]
    fn masking_is_the_identity_at_full_and_reaches_only_the_effects_at_low() {
        let mut derivable = 0;
        for bits in 0..64u8 {
            let chain = PostChain {
                ui: bits & 1 != 0,
                shadow_map: (bits & 2 != 0).then_some(QualityParams::FULL.shadow_map_size),
                ssao: bits & 4 != 0,
                shafts: bits & 8 != 0,
                dof: bits & 16 != 0,
                gi_inject: (bits & 32 != 0).then_some([4, 4, 4]),
                // Content like `ui`, so masking leaves it alone (`ui` already varies over the walk).
                debug_lines: bits & 1 != 0,
                // Varies with `ssao` so both gates are walked; `FULL` keeps the same scale.
                ssr: (bits & 4 != 0).then_some(QualityParams::FULL.ssr_scale),
                ssr_view: None,
                // Content like `ui`: masking keeps it.
                probe_view: bits & 1 != 0,
            };
            // The march reads the shadow map, so no scene derives shafts without one. The sixteen
            // combinations that pair them are not chains that exist, and `masked` is free to clear
            // the shafts there rather than to preserve an impossible pairing.
            if chain.shafts && chain.shadow_map.is_none() {
                continue;
            }
            derivable += 1;
            assert_eq!(
                chain.masked(&Quality::FULL, &QualityParams::FULL),
                chain,
                "FULL must leave {chain:?} exactly as the scene derived it",
            );
            assert_eq!(
                chain.masked(&Quality::LOW, &QualityParams::LOW),
                PostChain {
                    ssao: false,
                    shafts: false,
                    dof: false,
                    gi_inject: None,
                    shadow_map: None,
                    ssr: None,
                    ..chain
                },
                "LOW must clear the six effects of {chain:?} and nothing else",
            );
        }
        assert_eq!(derivable, 48, "every chain a scene can derive was walked");
    }

    /// The shadow map is the one masked thing carrying a *value*, so masking it has three outcomes
    /// rather than two: gone at `LOW`, redrawn at the machine's own extent at `MEDIUM`, and the
    /// scene's own ceiling at `FULL`. The march that reads it leaves with it — the dependency
    /// `masked` maintains so `build_frame_graph`'s shafts arm can keep asserting the map exists.
    #[test]
    fn masking_resizes_the_shadow_map_and_takes_the_shafts_with_it() {
        let authored = PostChain {
            shadow_map: Some(QualityParams::FULL.shadow_map_size),
            shafts: true,
            ..PostChain::default()
        };
        assert_eq!(
            authored
                .masked(&Quality::MEDIUM, &QualityParams::MEDIUM)
                .shadow_map,
            Some(QualityParams::MEDIUM.shadow_map_size),
            "a machine that keeps its shadows still draws them at its own extent",
        );
        assert!(
            authored
                .masked(&Quality::MEDIUM, &QualityParams::MEDIUM)
                .shafts,
            "and keeps the march, which has a map to read",
        );
        // Shadows off with shafts explicitly on — the override file's one reachable combination, and
        // the one the graph would otherwise panic on.
        let shadowless = Quality {
            shadows: false,
            ..Quality::FULL
        };
        let masked = authored.masked(&shadowless, &QualityParams::FULL);
        assert_eq!(masked.shadow_map, None);
        assert!(
            !masked.shafts,
            "the march samples the shadow map, so it cannot survive the map",
        );
        // Not vacuous: the same chain with a `MEDIUM` shadow map keeps its shafts (asserted above),
        // and a sunless chain never had either.
        assert_eq!(
            PostChain::default().masked(&Quality::FULL, &QualityParams::FULL),
            PostChain::default(),
        );
    }

    /// `for_scene` derives SSR from the authored strength alone; `masked` drops it at `LOW` and
    /// re-sizes it to the machine's own scale.
    #[test]
    fn ssr_is_gated_by_the_authored_strength_and_the_machine() {
        let with = |ssr: f32| {
            let mut scene = RenderScene::empty((1920, 1080));
            scene.post = Some(crate::scene::PostData {
                ssr,
                ..crate::scene::PostData::default()
            });
            PostChain::for_scene(&scene).ssr
        };
        assert_eq!(
            PostChain::for_scene(&RenderScene::empty((1920, 1080))).ssr,
            None
        );
        assert_eq!(with(0.0), None);
        assert_eq!(with(0.6), Some(0.5));
        let chain = PostChain {
            ssr: Some(QualityParams::FULL.ssr_scale),
            ..PostChain::default()
        };
        assert_eq!(chain.masked(&Quality::LOW, &QualityParams::LOW).ssr, None);
        let quarter = QualityParams {
            ssr_scale: 0.25,
            ..QualityParams::FULL
        };
        let masked = chain.masked(&Quality::FULL, &quarter);
        assert_eq!(masked.ssr, Some(0.25));
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), masked);
        let trace = graph
            .resources()
            .iter()
            .find(|r| r.name == "ssr_trace")
            .unwrap();
        assert_eq!(
            (trace.desc.extent.width, trace.desc.extent.height),
            (480, 270)
        );
    }

    /// The opaque pass writes the extra targets, `reflections` reads them, and the transparents
    /// draw over the same colour and depth — in that declaration order.
    #[test]
    fn the_scene_pass_splits_around_reflections() {
        let size = FrameSize::native(64, 64);
        let (graph, _) = build_frame_graph(size, PostChain::default());
        let order = schedule(&graph).unwrap();
        assert_eq!(
            order.order_names()[..3],
            ["scene", "reflections", "transparents"]
        );
        let names = |ids: &[ResourceId]| -> Vec<String> {
            ids.iter()
                .map(|id| graph.resources()[id.index()].name.clone())
                .collect()
        };
        let find = |name: &str| graph.passes().iter().find(|p| p.name == name).unwrap();
        let scene = find("scene");
        assert_eq!(
            names(scene.color_targets()),
            ["scene", "scene_normal", "scene_specular"]
        );
        assert_eq!(names(&[scene.depth_target().unwrap()]), ["scene_depth"]);
        let reflections = find("reflections");
        assert_eq!(
            names(&reflections.reads()),
            ["scene_normal", "scene_specular", "scene_depth"]
        );
        assert_eq!(names(reflections.color_targets()), ["scene"]);
        let transparents = find("transparents");
        assert_eq!(names(transparents.color_targets()), ["scene"]);
        assert_eq!(
            names(&[transparents.depth_target().unwrap()]),
            ["scene_depth"]
        );

        let ao = PostChain {
            ssao: true,
            ..PostChain::default()
        };
        let (ao_graph, _) = build_frame_graph(size, ao);
        let ao_order = schedule(&ao_graph).unwrap();
        assert_eq!(
            ao_order.order_names()[..5],
            ["prepass", "ssao", "ssao_blur", "scene", "reflections"]
        );
    }

    /// The default graph's exact shape: the opaque scene pass, `reflections`, `transparents`, the
    /// shared bright-pass, five downsamples, five upsamples back down and the composite — the same
    /// resources in the same declaration order, the same passes, and the same scheduled execution
    /// order. These lists ARE the regression detector; if one changes, the shipped chain changed.
    #[test]
    fn default_graph_is_the_fixed_fifteen_pass_chain() {
        let (graph, output) =
            build_frame_graph(FrameSize::native(1920, 1080), PostChain::default());
        assert_eq!(
            resource_names(&graph),
            [
                "scene",
                "scene_depth",
                "scene_normal",
                "scene_specular",
                "bloom_bright",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_0",
                "bloom_up_1",
                "bloom_up_2",
                "bloom_up_3",
                "bloom_up_4",
                "final"
            ],
        );
        assert_eq!(
            pass_names(&graph),
            [
                "scene",
                "reflections",
                "transparents",
                "bloom_threshold",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_4",
                "bloom_up_3",
                "bloom_up_2",
                "bloom_up_1",
                "bloom_up_0",
                "composite",
            ],
        );
        // The scene pass writes the color scene target and the graph-owned depth attachment.
        let scene = graph.passes().iter().find(|p| p.name == "scene").unwrap();
        assert_eq!(
            graph.resources()[scene.depth_target().unwrap().index()].name,
            "scene_depth",
        );
        // composite writes the output directly (no `ldr` indirection), and reads the scene depth on
        // every frame — the unconditional third input that makes fog one pipeline rather than two.
        let composite = graph
            .passes()
            .iter()
            .find(|p| p.name == "composite")
            .unwrap();
        assert_eq!(composite.color_targets(), [output]);
        assert_eq!(
            composite
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>(),
            ["scene", "bloom_up_0", "scene_depth"],
        );

        let scheduled = schedule(&graph).unwrap();
        assert_eq!(
            scheduled.order_names(),
            [
                "scene",
                "reflections",
                "transparents",
                "bloom_threshold",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_4",
                "bloom_up_3",
                "bloom_up_2",
                "bloom_up_1",
                "bloom_up_0",
                "composite",
            ],
        );
    }

    /// The UI graph adds exactly the `ldr` transient and the `overlay` pass: composite retargets to
    /// `ldr`, `overlay` reads `ldr` and writes the output, and the scheduler derives composite→overlay
    /// from that read edge (overlay runs last).
    #[test]
    fn ui_graph_appends_ldr_and_overlay() {
        let chain = PostChain {
            ui: true,
            ..PostChain::default()
        };
        let (graph, output) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        assert_eq!(
            resource_names(&graph),
            [
                "scene",
                "scene_depth",
                "scene_normal",
                "scene_specular",
                "bloom_bright",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_0",
                "bloom_up_1",
                "bloom_up_2",
                "bloom_up_3",
                "bloom_up_4",
                "final",
                "ldr"
            ],
        );
        assert_eq!(
            pass_names(&graph),
            [
                "scene",
                "reflections",
                "transparents",
                "bloom_threshold",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_4",
                "bloom_up_3",
                "bloom_up_2",
                "bloom_up_1",
                "bloom_up_0",
                "composite",
                "overlay",
            ],
        );

        let composite = graph
            .passes()
            .iter()
            .find(|p| p.name == "composite")
            .unwrap();
        let overlay = graph.passes().iter().find(|p| p.name == "overlay").unwrap();
        // `ldr` is the transient composite now writes and overlay reads (same handle), and it is not
        // the output — so the chain is composite → ldr → overlay → final.
        let composite_target = composite.color_targets()[0];
        assert_eq!(
            overlay.reads,
            vec![composite_target],
            "overlay reads composite's ldr target"
        );
        assert_ne!(
            composite_target, output,
            "composite no longer writes the output directly"
        );
        assert_eq!(
            graph.resources()[composite_target.index()].name,
            "ldr",
            "composite retargets to the ldr transient",
        );
        assert_eq!(
            overlay.color_targets(),
            [output],
            "overlay writes the final output"
        );

        let scheduled = schedule(&graph).unwrap();
        let order = scheduled.order_names();
        assert_eq!(order.last(), Some(&"overlay"), "overlay runs last");
        let composite_pos = order.iter().position(|&n| n == "composite").unwrap();
        let overlay_pos = order.iter().position(|&n| n == "overlay").unwrap();
        assert!(
            composite_pos < overlay_pos,
            "the derived edge is composite→overlay"
        );
    }

    /// Debug lines alone take the UI's tail (`ldr` + `overlay`) and add one read: the overlay samples
    /// `scene_depth` for the x-ray dimming, right behind the frame it draws onto.
    #[test]
    fn debug_lines_append_the_overlay_reading_scene_depth() {
        let chain = PostChain {
            debug_lines: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let ui_only = PostChain {
            ui: true,
            ..PostChain::default()
        };
        let (ui_graph, _) = build_frame_graph(FrameSize::native(1920, 1080), ui_only);
        assert_eq!(pass_names(&graph), pass_names(&ui_graph));
        assert_eq!(resource_names(&graph), resource_names(&ui_graph));

        let overlay = graph.passes().iter().find(|p| p.name == "overlay").unwrap();
        assert_eq!(overlay.reads.len(), 2);
        assert_eq!(
            graph.resources()[overlay.reads[1].index()].name,
            "scene_depth"
        );
        assert!(schedule(&graph).is_ok());
    }

    /// A resampled frame with debug lines needs the same second LDR seam the UI does.
    #[test]
    fn debug_lines_with_a_resample_add_ldr_resolved() {
        let chain = PostChain {
            debug_lines: true,
            ..PostChain::default()
        };
        let size = FrameSize::scaled(1920, 1080, RenderScale::new(2.0).unwrap());
        let (graph, _) = build_frame_graph(size, chain);
        assert!(resource_names(&graph).contains(&"ldr_resolved"));
        assert!(pass_names(&graph).contains(&"overlay"));
        assert!(schedule(&graph).is_ok());
    }

    /// No debug lines leaves the default graph exactly as it was.
    #[test]
    fn no_debug_lines_leave_the_graph_untouched() {
        let (plain, _) = build_frame_graph(FrameSize::native(1920, 1080), PostChain::default());
        let (off, _) = build_frame_graph(
            FrameSize::native(1920, 1080),
            PostChain {
                debug_lines: false,
                ..PostChain::default()
            },
        );
        assert_eq!(pass_names(&plain), pass_names(&off));
        assert_eq!(resource_names(&plain), resource_names(&off));
        assert!(!pass_names(&plain).contains(&"overlay"));
    }

    /// The sun graph prepends a depth-only `shadow` pass writing the `shadow_map` resource, and the
    /// scene pass samples it — so the scheduler orders shadow before scene (`Plan/shadow-mapping.md`
    /// decision 6). A sunless scene declares neither the pass nor the resource (proved above), so
    /// existing goldens and the dump-graph snapshot are untouched by construction.
    #[test]
    fn sun_graph_prepends_the_shadow_pass_and_map() {
        let chain = PostChain {
            shadow_map: Some(QualityParams::FULL.shadow_map_size),
            ..PostChain::default()
        };
        let (graph, _output) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        assert_eq!(
            resource_names(&graph),
            [
                "scene",
                "scene_depth",
                "scene_normal",
                "scene_specular",
                "shadow_map",
                "bloom_bright",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_0",
                "bloom_up_1",
                "bloom_up_2",
                "bloom_up_3",
                "bloom_up_4",
                "final",
            ],
        );
        assert_eq!(
            pass_names(&graph),
            [
                "shadow",
                "scene",
                "reflections",
                "transparents",
                "bloom_threshold",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_4",
                "bloom_up_3",
                "bloom_up_2",
                "bloom_up_1",
                "bloom_up_0",
                "composite",
            ],
        );

        // The shadow pass is depth-only (no color target) and writes `shadow_map`.
        let shadow = graph.passes().iter().find(|p| p.name == "shadow").unwrap();
        assert!(shadow.color_targets().is_empty(), "shadow is depth-only");
        assert_eq!(
            graph.resources()[shadow.depth_target().unwrap().index()].name,
            "shadow_map",
        );
        // The scene pass samples `shadow_map`.
        let scene = graph.passes().iter().find(|p| p.name == "scene").unwrap();
        assert_eq!(
            scene
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>(),
            ["shadow_map"],
        );

        let scheduled = schedule(&graph).unwrap();
        let order = scheduled.order_names();
        let shadow_pos = order.iter().position(|&n| n == "shadow").unwrap();
        let scene_pos = order.iter().position(|&n| n == "scene").unwrap();
        assert!(shadow_pos < scene_pos, "shadow renders before the scene");
    }

    /// The map is declared at the extent the chain names, so a machine that lowers
    /// `shadow_map_size` allocates a smaller texture and nothing else about the graph moves.
    #[test]
    fn the_shadow_map_is_declared_at_the_chains_own_extent() {
        for edge in [QualityParams::FULL.shadow_map_size, 1024] {
            let chain = PostChain {
                shadow_map: Some(edge),
                ..PostChain::default()
            };
            let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
            let map = graph
                .resources()
                .iter()
                .find(|r| r.name == "shadow_map")
                .expect("the chain declares a shadow map");
            assert_eq!(
                (map.desc.extent.width, map.desc.extent.height),
                (edge, edge)
            );
            assert_eq!(
                pass_names(&graph).len(),
                16,
                "the extent is not structure: the pass list is the same either way",
            );
        }
    }

    /// Level sizes halve down the pyramid (level 0 is the half-res bright pass), and every upsample
    /// reads `[coarser accumulation, same-level downsample result]` in that order — the read order
    /// the recorder's bind group is built from.
    #[test]
    fn pyramid_levels_halve_and_upsamples_read_coarse_then_fine() {
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), PostChain::default());
        let extent = |name: &str| {
            let r = graph.resources().iter().find(|r| r.name == name).unwrap();
            (r.desc.extent.width, r.desc.extent.height)
        };
        assert_eq!(extent("bloom_bright"), (960, 540));
        for (level, expected) in [
            (1, (480, 270)),
            (2, (240, 135)),
            (3, (120, 67)),
            (4, (60, 33)),
        ] {
            assert_eq!(extent(pass::BLOOM_DOWN[level - 1]), expected);
            // Each upsample writes at its own level's size.
            assert_eq!(extent(pass::BLOOM_UP[level]), expected);
        }
        assert_eq!(extent(pass::BLOOM_DOWN[4]), (30, 16));
        assert_eq!(extent(pass::BLOOM_UP[0]), extent("bloom_bright"));

        let reads = |name: &str| {
            graph
                .passes()
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(reads("bloom_down_1"), ["bloom_bright"]);
        assert_eq!(reads("bloom_down_5"), ["bloom_down_4"]);
        // The coarsest upsample's "coarser accumulation" is the last downsample itself.
        assert_eq!(reads("bloom_up_4"), ["bloom_down_5", "bloom_down_4"]);
        assert_eq!(reads("bloom_up_1"), ["bloom_up_2", "bloom_down_1"]);
        // Level 0's own "downsample result" is the shared bright pass.
        assert_eq!(reads("bloom_up_0"), ["bloom_up_1", "bloom_bright"]);
    }

    fn ssr_chain() -> PostChain {
        PostChain {
            ssr: Some(QualityParams::FULL.ssr_scale),
            ..PostChain::default()
        }
    }

    /// With `ssr` off the graph has neither pyramid; `for_scene` derives `None` until a scene
    /// authors a strength, so every other frame is the pre-SSR graph.
    #[test]
    fn ssr_off_declares_no_pyramids() {
        assert_eq!(
            PostChain::for_scene(&RenderScene::empty((1920, 1080))).ssr,
            None
        );
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), PostChain::default());
        assert!(!resource_names(&graph).contains(&"hiz"));
        assert!(
            pass_names(&graph)
                .iter()
                .all(|n| !n.starts_with("hiz") && !n.starts_with("ssr_color"))
        );
    }

    #[test]
    fn ssr_pyramids_are_full_chains_at_the_trace_extent() {
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), ssr_chain());
        let find = |name: &str| graph.resources().iter().find(|r| r.name == name).unwrap();
        let hiz = find("hiz");
        assert_eq!(hiz.desc.format.as_str(), "r32float");
        assert_eq!(
            (
                hiz.desc.extent.width,
                hiz.desc.extent.height,
                hiz.desc.mip_levels
            ),
            (960, 540, 10)
        );
        let color = find("ssr_color");
        assert_eq!(color.desc.format.as_str(), "rgba16float");
        assert_eq!(
            (
                color.desc.extent.width,
                color.desc.extent.height,
                color.desc.mip_levels
            ),
            (960, 540, 10)
        );
        // A supersampled frame's trace extent still follows the output.
        let (ssaa, _) = build_frame_graph(
            FrameSize::scaled(1920, 1080, RenderScale::new(2.0).unwrap()),
            ssr_chain(),
        );
        let hiz = ssaa.resources().iter().find(|r| r.name == "hiz").unwrap();
        assert_eq!((hiz.desc.extent.width, hiz.desc.extent.height), (960, 540));
    }

    #[test]
    fn ssr_pyramids_schedule_between_scene_and_reflections() {
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), ssr_chain());
        let order = schedule(&graph).unwrap();
        let names = order.order_names();
        let pos = |n: &str| names.iter().position(|p| *p == n).unwrap();
        assert!(pos("scene") < pos("hiz_seed"));
        assert!(pos("scene") < pos("ssr_color_seed"));
        assert!(pos("hiz_seed") < pos("hiz_reduce_1"));
        assert!(pos("hiz_reduce_9") < pos("reflections"));
        assert!(pos("ssr_color_reduce_9") < pos("reflections"));
        assert!(pos("reflections") < pos("transparents"));
        let find = |name: &str| graph.passes().iter().find(|p| p.name == name).unwrap();
        let label = |id: &ResourceId| {
            let name = &graph.resources()[id.index()].name;
            format!("{name}@mip{}", id.mip_level().unwrap())
        };
        let reduce = find("hiz_reduce_3");
        assert_eq!(
            reduce.reads().iter().map(label).collect::<Vec<_>>(),
            ["hiz@mip2"]
        );
        assert_eq!(
            reduce.color_targets().iter().map(label).collect::<Vec<_>>(),
            ["hiz@mip3"]
        );
    }

    #[test]
    fn the_ssr_trace_reads_both_pyramids_and_runs_before_transparents() {
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), ssr_chain());
        let trace = graph
            .resources()
            .iter()
            .find(|r| r.name == "ssr_trace")
            .unwrap();
        assert_eq!(trace.desc.format.as_str(), "rgba16float");
        assert_eq!(
            (
                trace.desc.extent.width,
                trace.desc.extent.height,
                trace.desc.mip_levels
            ),
            (960, 540, 1)
        );
        let reads = |name: &str| {
            graph
                .passes()
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .reads()
                .iter()
                .map(|id| graph.resources()[id.index()].name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            reads("ssr_trace"),
            ["hiz", "ssr_color", "scene_normal", "scene_depth"]
        );
        // `reflections` reads its three surface targets and then the trace.
        assert_eq!(
            reads("reflections"),
            ["scene_normal", "scene_specular", "scene_depth", "ssr_trace"]
        );
        let order = schedule(&graph).unwrap();
        let names = order.order_names();
        let pos = |n: &str| names.iter().position(|p| *p == n).unwrap();
        assert!(pos("hiz_reduce_9") < pos("ssr_trace"));
        assert!(pos("ssr_color_reduce_9") < pos("ssr_trace"));
        assert!(pos("ssr_trace") < pos("reflections"));
        assert!(pos("ssr_trace") < pos("transparents"));
    }

    #[test]
    fn the_ssr_trace_harness_graph_schedules() {
        let (graph, trace) = build_ssr_trace_graph(FrameSize::native(96, 64));
        schedule(&graph).unwrap();
        assert_eq!(graph.resources()[trace.index()].name, "ssr_trace");
    }

    /// A tiny or odd frame clamps the chain to its few levels and still schedules.
    fn ssr_view_chain() -> PostChain {
        PostChain {
            ssr_view: Some(crate::DebugView::SsrConfidence),
            ..ssr_chain()
        }
    }

    /// The view replaces `composite`: it reads the trace alone and writes what composite would have.
    #[test]
    fn an_ssr_view_replaces_composite_and_reads_the_trace() {
        let size = FrameSize::native(1920, 1080);
        let (graph, _) = build_frame_graph(size, ssr_view_chain());
        let (plain, _) = build_frame_graph(size, ssr_chain());
        let find = |graph: &RenderGraph, name: &str| {
            let pass = graph.passes().iter().find(|p| p.name == name)?;
            let names = |ids: &[ResourceId]| {
                ids.iter()
                    .map(|id| graph.resources()[id.index()].name.clone())
                    .collect::<Vec<_>>()
            };
            Some((names(&pass.reads()), names(pass.color_targets())))
        };
        assert!(find(&graph, "composite").is_none());
        let (reads, writes) = find(&graph, "ssr_debug").unwrap();
        assert_eq!(reads, ["ssr_trace"]);
        assert_eq!(writes, find(&plain, "composite").unwrap().1);
        assert!(schedule(&graph).is_ok());
    }

    /// A view with no SSR is not a chain `for_scene` produces, and the graph ignores it anyway.
    #[test]
    fn a_view_without_ssr_is_the_plain_graph() {
        let size = FrameSize::native(1920, 1080);
        let with_view = PostChain {
            ssr_view: Some(crate::DebugView::SsrFootprint),
            ..PostChain::default()
        };
        let (a, _) = build_frame_graph(size, with_view);
        let (b, _) = build_frame_graph(size, PostChain::default());
        assert_eq!(pass_names(&a), pass_names(&b));
        assert_eq!(resource_names(&a), resource_names(&b));
        let mut scene = RenderScene::empty((1920, 1080));
        scene.debug_view = Some(crate::DebugView::SsrFootprint);
        assert_eq!(PostChain::for_scene(&scene).ssr_view, None);
    }

    #[test]
    fn masking_ssr_off_drops_the_view() {
        assert_eq!(
            ssr_view_chain()
                .masked(&Quality::LOW, &QualityParams::LOW)
                .ssr_view,
            None
        );
        assert_eq!(
            ssr_view_chain()
                .masked(&Quality::FULL, &QualityParams::FULL)
                .ssr_view,
            Some(crate::DebugView::SsrConfidence)
        );
    }

    fn probe_view_chain() -> PostChain {
        PostChain {
            probe_view: true,
            ..PostChain::default()
        }
    }

    fn graph_pass(graph: &RenderGraph, name: &str) -> Option<(Vec<String>, Vec<String>)> {
        let pass = graph.passes().iter().find(|p| p.name == name)?;
        let names = |ids: &[ResourceId]| {
            ids.iter()
                .map(|id| graph.resources()[id.index()].name.clone())
                .collect::<Vec<_>>()
        };
        Some((names(&pass.reads()), names(pass.color_targets())))
    }

    /// The probe view replaces `composite` the way the SSR view does, reading normal and depth.
    #[test]
    fn a_probe_view_replaces_composite_and_reads_normal_and_depth() {
        let size = FrameSize::native(1920, 1080);
        let (graph, _) = build_frame_graph(size, probe_view_chain());
        let (plain, _) = build_frame_graph(size, PostChain::default());
        assert!(graph_pass(&graph, "composite").is_none());
        let (reads, writes) = graph_pass(&graph, "probe_debug").unwrap();
        assert_eq!(reads, ["scene_normal", "scene_depth"]);
        assert_eq!(writes, graph_pass(&plain, "composite").unwrap().1);
        assert!(schedule(&graph).is_ok());
    }

    /// `for_scene` sets the probe view only for a scene with probes; without them the graph is the
    /// no-view graph. The probe view never doubles as an SSR view, and `view()` reports what is drawn.
    #[test]
    fn the_probe_view_needs_probes_and_never_sets_the_ssr_view() {
        let mut scene = RenderScene::empty((1920, 1080));
        scene.debug_view = Some(crate::DebugView::ProbeIndex);
        let none = PostChain::for_scene(&scene);
        assert!(!none.probe_view);
        assert_eq!(none.view(), None);
        let size = FrameSize::native(1920, 1080);
        let (viewed, _) = build_frame_graph(size, none);
        let (plain, _) = build_frame_graph(size, PostChain::default());
        assert_eq!(pass_names(&viewed), pass_names(&plain));

        scene.probes = vec![crate::ProbeData {
            entity: ludion_core::ids::EntityId::new(1, 0),
            position: Default::default(),
            rotation: Default::default(),
            volume: crate::ProbeVolume::Sphere { radius: 1.0 },
            blend: 0.0,
            priority: 0,
        }];
        let chain = PostChain::for_scene(&scene);
        assert!(chain.probe_view);
        assert_eq!(chain.ssr_view, None);
        assert_eq!(chain.view(), Some(crate::DebugView::ProbeIndex));
        assert!(chain.masked(&Quality::LOW, &QualityParams::LOW).probe_view);
    }

    /// With SSR on and the probe view asked, the probe pass is scheduled and the SSR one is not.
    #[test]
    fn the_probe_view_wins_over_an_ssr_chain() {
        let chain = PostChain {
            probe_view: true,
            ..ssr_chain()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        assert!(graph_pass(&graph, "probe_debug").is_some());
        assert!(graph_pass(&graph, "ssr_debug").is_none());
        assert!(graph_pass(&graph, "composite").is_none());
        assert!(schedule(&graph).is_ok());
    }

    /// The tail after the swapped pass (resample, overlay) still schedules.
    #[test]
    fn an_ssr_view_with_ui_and_a_resample_still_schedules_the_tail() {
        let chain = PostChain {
            ui: true,
            ..ssr_view_chain()
        };
        let size = FrameSize::scaled(1920, 1080, RenderScale::new(2.0).unwrap());
        let (graph, _) = build_frame_graph(size, chain);
        let order = schedule(&graph).unwrap();
        let names = order.order_names();
        let pos = |n: &str| names.iter().position(|p| *p == n).unwrap();
        assert!(pos("ssr_trace") < pos("ssr_debug"));
        assert!(pos("ssr_debug") < pos("overlay"));
    }

    #[test]
    fn ssr_pyramids_clamp_on_a_tiny_or_odd_frame() {
        for (w, h, levels) in [(1, 1, 1), (3, 1, 2), (67, 45, 6)] {
            let (graph, _) = build_frame_graph(FrameSize::native(w, h), ssr_chain());
            let hiz = graph.resources().iter().find(|r| r.name == "hiz").unwrap();
            assert_eq!(hiz.desc.mip_levels, levels, "{w}x{h}");
            schedule(&graph).unwrap();
        }
    }

    #[test]
    fn ssr_pyramids_appear_in_the_graph_dump() {
        let json = dump_frame_graph_json(FrameSize::native(1920, 1080), ssr_chain()).unwrap();
        for needle in ["r32float", "hiz@mip3", "ssr_color_reduce_2"] {
            assert!(json.contains(needle), "{needle} missing from the dump");
        }
    }

    /// A degenerate resolution cannot produce a zero-extent level: every level clamps to at least
    /// one texel, so the coarsest levels simply collapse onto each other.
    #[test]
    fn tiny_resolutions_clamp_every_pyramid_level_to_one_texel() {
        let (graph, _) = build_frame_graph(FrameSize::native(1, 1), PostChain::default());
        for resource in graph.resources() {
            assert!(
                resource.desc.extent.width >= 1 && resource.desc.extent.height >= 1,
                "{} collapsed to zero: {:?}",
                resource.name,
                resource.desc.extent
            );
        }
        schedule(&graph).expect("the clamped pyramid still schedules");
    }

    // --- Supersampling (Plan/post-process-pipeline.md) ------------------------------------

    fn ssaa(width: u32, height: u32) -> FrameSize {
        FrameSize::scaled(width, height, RenderScale::new(2.0).unwrap())
    }

    /// A supersampled, UI-free frame is the default graph plus exactly one pass and one resource:
    /// `composite` retargets to `ldr` at the **render** extent and `resolve` writes the output at the
    /// output extent. `final` keeps its declaration position, so the minimal graph's resource list is
    /// still a prefix of this one — these lists ARE the regression detector.
    #[test]
    fn supersampled_graph_resolves_into_the_output() {
        let (graph, output) = build_frame_graph(ssaa(960, 540), PostChain::default());
        assert_eq!(
            resource_names(&graph),
            [
                "scene",
                "scene_depth",
                "scene_normal",
                "scene_specular",
                "bloom_bright",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_0",
                "bloom_up_1",
                "bloom_up_2",
                "bloom_up_3",
                "bloom_up_4",
                "final",
                "ldr",
            ],
        );
        let expected_passes = [
            "scene",
            "reflections",
            "transparents",
            "bloom_threshold",
            "bloom_down_1",
            "bloom_down_2",
            "bloom_down_3",
            "bloom_down_4",
            "bloom_down_5",
            "bloom_up_4",
            "bloom_up_3",
            "bloom_up_2",
            "bloom_up_1",
            "bloom_up_0",
            "composite",
            "resolve",
        ];
        assert_eq!(pass_names(&graph), expected_passes);
        assert_eq!(schedule(&graph).unwrap().order_names(), expected_passes);

        let extent = |name: &str| {
            let r = graph.resources().iter().find(|r| r.name == name).unwrap();
            (r.desc.extent.width, r.desc.extent.height)
        };
        assert_eq!(extent("scene"), (1920, 1080), "the scene draws at 2x");
        assert_eq!(extent("scene_depth"), (1920, 1080));
        assert_eq!(extent("ldr"), (1920, 1080), "composite stays at 2x");
        assert_eq!(extent("final"), (960, 540), "the output is what was asked");

        let resolve = graph.passes().iter().find(|p| p.name == "resolve").unwrap();
        assert_eq!(
            resolve
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>(),
            ["ldr"],
        );
        assert_eq!(resolve.color_targets(), [output]);
    }

    /// Supersampling *and* UI: the resolve lands in a second LDR seam (`ldr_resolved`, at the output
    /// extent) so the overlay can sample it, and the overlay still writes `final` last. This is the
    /// ordering the overlay exists for — UI text is rasterized at output resolution, after the
    /// downsample, so it never softens.
    #[test]
    fn supersampled_ui_graph_resolves_before_the_overlay() {
        let chain = PostChain {
            ui: true,
            ..PostChain::default()
        };
        let (graph, output) = build_frame_graph(ssaa(960, 540), chain);
        assert_eq!(
            resource_names(&graph),
            [
                "scene",
                "scene_depth",
                "scene_normal",
                "scene_specular",
                "bloom_bright",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_0",
                "bloom_up_1",
                "bloom_up_2",
                "bloom_up_3",
                "bloom_up_4",
                "final",
                "ldr",
                "ldr_resolved",
            ],
        );
        let expected_passes = [
            "scene",
            "reflections",
            "transparents",
            "bloom_threshold",
            "bloom_down_1",
            "bloom_down_2",
            "bloom_down_3",
            "bloom_down_4",
            "bloom_down_5",
            "bloom_up_4",
            "bloom_up_3",
            "bloom_up_2",
            "bloom_up_1",
            "bloom_up_0",
            "composite",
            "resolve",
            "overlay",
        ];
        assert_eq!(pass_names(&graph), expected_passes);
        assert_eq!(schedule(&graph).unwrap().order_names(), expected_passes);

        let resolved = graph
            .resources()
            .iter()
            .find(|r| r.name == "ldr_resolved")
            .unwrap();
        assert_eq!(
            (resolved.desc.extent.width, resolved.desc.extent.height),
            (960, 540),
            "the overlay draws its UI at the output extent",
        );
        let overlay = graph.passes().iter().find(|p| p.name == "overlay").unwrap();
        assert_eq!(
            overlay
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>(),
            ["ldr_resolved"],
        );
        assert_eq!(overlay.color_targets(), [output]);
    }

    // --- Sub-native rendering (Plan/scalability.md) ---------------------------------------

    fn sub_native(width: u32, height: u32) -> FrameSize {
        FrameSize::scaled(width, height, RenderScale::new(0.5).unwrap())
    }

    /// The mirror image of [`supersampled_graph_resolves_into_the_output`]: a sub-native, UI-free
    /// frame is the default graph plus exactly one pass and one resource, and the pass is `upscale`
    /// rather than `resolve` — one seam, two directions. `composite` writes `ldr` at the **render**
    /// extent, which is now the smaller of the two, and `upscale` magnifies it into the output.
    #[test]
    fn sub_native_graph_upscales_into_the_output() {
        let (graph, output) = build_frame_graph(sub_native(960, 540), PostChain::default());
        assert_eq!(
            resource_names(&graph),
            [
                "scene",
                "scene_depth",
                "scene_normal",
                "scene_specular",
                "bloom_bright",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_0",
                "bloom_up_1",
                "bloom_up_2",
                "bloom_up_3",
                "bloom_up_4",
                "final",
                "ldr",
            ],
        );
        let expected_passes = [
            "scene",
            "reflections",
            "transparents",
            "bloom_threshold",
            "bloom_down_1",
            "bloom_down_2",
            "bloom_down_3",
            "bloom_down_4",
            "bloom_down_5",
            "bloom_up_4",
            "bloom_up_3",
            "bloom_up_2",
            "bloom_up_1",
            "bloom_up_0",
            "composite",
            "upscale",
        ];
        assert_eq!(pass_names(&graph), expected_passes);
        assert_eq!(schedule(&graph).unwrap().order_names(), expected_passes);
        assert!(
            !pass_names(&graph).contains(&pass::RESOLVE),
            "a frame that magnifies never also resolves",
        );

        let extent = |name: &str| {
            let r = graph.resources().iter().find(|r| r.name == name).unwrap();
            (r.desc.extent.width, r.desc.extent.height)
        };
        assert_eq!(extent("scene"), (480, 270), "the scene draws at half");
        assert_eq!(extent("scene_depth"), (480, 270));
        assert_eq!(extent("ldr"), (480, 270), "composite stays at half");
        assert_eq!(extent("final"), (960, 540), "the output is what was asked");
        // The sizing rule below native: `half_post` is half the *output*, so it is larger than the
        // buffer the scene drew into. That is the rule holding, not breaking — a bloom radius is a
        // fraction of the frame the client asked for.
        assert_eq!(extent("bloom_bright"), (480, 270));

        let upscale = graph.passes().iter().find(|p| p.name == "upscale").unwrap();
        assert_eq!(
            upscale
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>(),
            ["ldr"],
        );
        assert_eq!(upscale.color_targets(), [output]);
    }

    /// **UI stays native-crisp below native, and the graph is where that starts.** The upscale lands
    /// in the same `ldr_resolved` seam the resolve uses, and the overlay — which draws every rect and
    /// every glyph — writes `final` at the **output** extent, behind the magnification. So the UI is
    /// rasterized once, at the resolution the client asked for, and no filter touches it.
    #[test]
    fn sub_native_ui_graph_upscales_before_the_overlay() {
        let chain = PostChain {
            ui: true,
            ..PostChain::default()
        };
        let (graph, output) = build_frame_graph(sub_native(960, 540), chain);
        let expected_passes = [
            "scene",
            "reflections",
            "transparents",
            "bloom_threshold",
            "bloom_down_1",
            "bloom_down_2",
            "bloom_down_3",
            "bloom_down_4",
            "bloom_down_5",
            "bloom_up_4",
            "bloom_up_3",
            "bloom_up_2",
            "bloom_up_1",
            "bloom_up_0",
            "composite",
            "upscale",
            "overlay",
        ];
        assert_eq!(pass_names(&graph), expected_passes);
        assert_eq!(schedule(&graph).unwrap().order_names(), expected_passes);

        let extent = |id: ResourceId| {
            let d = graph.resource(id).desc.extent;
            (d.width, d.height)
        };
        let overlay = graph.passes().iter().find(|p| p.name == "overlay").unwrap();
        assert_eq!(
            overlay
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>(),
            ["ldr_resolved"],
        );
        assert_eq!(overlay.color_targets(), [output]);
        assert_eq!(
            extent(output),
            (960, 540),
            "the UI is drawn at the output extent, not at the extent the scene drew",
        );
        assert_eq!(
            extent(overlay.reads()[0]),
            (960, 540),
            "and it composites over an already-magnified image",
        );
    }

    /// **Scale invariance.** Every bloom target is sized from the *output* extent, so raising
    /// the render scale cannot narrow the glow: the extents at scale 1 and scale 2 are identical,
    /// which is why no bloom shader or uniform changed for supersampling.
    #[test]
    fn bloom_target_extents_ignore_the_render_scale() {
        let bloom_extents = |size: FrameSize| {
            let (graph, _) = build_frame_graph(size, PostChain::default());
            graph
                .resources()
                .iter()
                .filter(|r| r.name.starts_with("bloom_"))
                .map(|r| (r.name.clone(), (r.desc.extent.width, r.desc.extent.height)))
                .collect::<Vec<_>>()
        };
        let native = bloom_extents(FrameSize::native(960, 540));
        assert!(!native.is_empty(), "the chain must declare bloom targets");
        assert_eq!(
            native,
            bloom_extents(ssaa(960, 540)),
            "bloom targets are a function of the output extent alone",
        );
    }

    // --- Ambient occlusion (Plan/post-process-pipeline.md) --------------------------------

    fn ssao_chain() -> PostChain {
        PostChain {
            ssao: true,
            ..PostChain::default()
        }
    }

    /// `ssao` is a strength *and* the gate. A scene with no `PostProcess`, and equally one whose
    /// `ssao` is zero, derives the frozen chain; only a positive strength turns it on. The other
    /// imaging knobs are irrelevant to it — the chain is a question about the *graph*, and only this
    /// one field changes the graph.
    #[test]
    fn for_scene_reads_the_ssao_gate() {
        let mut scene = RenderScene::empty((1920, 1080));
        assert!(!PostChain::for_scene(&scene).ssao);
        let post = crate::scene::PostData::default();
        scene.post = Some(post);
        assert!(!PostChain::for_scene(&scene).ssao);
        scene.post = Some(crate::scene::PostData {
            exposure: 2.0,
            vignette: 0.4,
            ..post
        });
        assert!(!PostChain::for_scene(&scene).ssao);
        scene.post = Some(crate::scene::PostData { ssao: 0.6, ..post });
        assert!(PostChain::for_scene(&scene).ssao);
    }

    /// **The by-construction half of the byte-identity claim.** With ambient occlusion off the graph
    /// declares neither the prepass's two targets nor the AO chain's, and none of its three passes —
    /// this is the exact default graph, resource for resource. If AO ever leaked a resource into an
    /// AO-free frame, this list would change before a single golden did.
    #[test]
    fn ao_off_graph_declares_no_prepass_or_ssao_resources() {
        let chain = PostChain {
            ssao: false,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        assert_eq!(
            resource_names(&graph),
            [
                "scene",
                "scene_depth",
                "scene_normal",
                "scene_specular",
                "bloom_bright",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_0",
                "bloom_up_1",
                "bloom_up_2",
                "bloom_up_3",
                "bloom_up_4",
                "final"
            ],
        );
        assert_eq!(
            pass_names(&graph),
            [
                "scene",
                "reflections",
                "transparents",
                "bloom_threshold",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_4",
                "bloom_up_3",
                "bloom_up_2",
                "bloom_up_1",
                "bloom_up_0",
                "composite",
            ],
        );
    }

    /// The AO chain's exact shape: four transients declared together after the shadow map, three
    /// passes ahead of the scene pass, and a scheduled order that runs prepass → ssao → ssao_blur →
    /// scene. These lists ARE the regression detector.
    #[test]
    fn ssao_graph_prepends_the_prepass_and_the_ao_chain() {
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), ssao_chain());
        assert_eq!(
            resource_names(&graph),
            [
                "scene",
                "scene_depth",
                "scene_normal",
                "scene_specular",
                "prepass_normal",
                "prepass_depth",
                "ssao",
                "ssao_blur",
                "bloom_bright",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_0",
                "bloom_up_1",
                "bloom_up_2",
                "bloom_up_3",
                "bloom_up_4",
                "final",
            ],
        );
        let expected_passes = [
            "prepass",
            "ssao",
            "ssao_blur",
            "scene",
            "reflections",
            "transparents",
            "bloom_threshold",
            "bloom_down_1",
            "bloom_down_2",
            "bloom_down_3",
            "bloom_down_4",
            "bloom_down_5",
            "bloom_up_4",
            "bloom_up_3",
            "bloom_up_2",
            "bloom_up_1",
            "bloom_up_0",
            "composite",
        ];
        assert_eq!(pass_names(&graph), expected_passes);
        assert_eq!(schedule(&graph).unwrap().order_names(), expected_passes);

        let reads = |name: &str| {
            graph
                .passes()
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>()
        };
        // The prepass reads nothing and writes its own colour + depth pair.
        let prepass = graph.passes().iter().find(|p| p.name == "prepass").unwrap();
        assert!(reads("prepass").is_empty());
        assert_eq!(
            graph.resources()[prepass.color_targets()[0].index()].name,
            "prepass_normal",
        );
        assert_eq!(
            graph.resources()[prepass.depth_target().unwrap().index()].name,
            "prepass_depth",
            "the prepass owns its depth — the scene pass's is untouched",
        );
        // `[depth, normal]` is the estimator's binding order, derived from this declaration.
        assert_eq!(reads("ssao"), ["prepass_depth", "prepass_normal"]);
        assert_eq!(reads("ssao_blur"), ["ssao"]);
        // The scene pass's read of the blurred factor is what orders the whole chain in front of it.
        assert_eq!(reads("scene"), ["ssao_blur"]);
    }

    /// With a sun *and* ambient occlusion the scene pass samples two textures, and the order it
    /// declares them in is the order `record_scene` counts its input slots in: shadow map first, AO
    /// second.
    #[test]
    fn sun_and_ssao_scene_reads_are_shadow_then_ao() {
        let chain = PostChain {
            shadow_map: Some(QualityParams::FULL.shadow_map_size),
            ssao: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let scene = graph.passes().iter().find(|p| p.name == "scene").unwrap();
        assert_eq!(
            scene
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>(),
            ["shadow_map", "ssao_blur"],
        );
        let scheduled = schedule(&graph).unwrap();
        assert_eq!(
            scheduled.order_names(),
            [
                "shadow",
                "prepass",
                "ssao",
                "ssao_blur",
                "scene",
                "reflections",
                "transparents",
                "bloom_threshold",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_4",
                "bloom_up_3",
                "bloom_up_2",
                "bloom_up_1",
                "bloom_up_0",
                "composite",
            ],
        );
    }

    /// The extent rule, applied to the AO chain: the prepass is **geometry**, so its normal and
    /// depth targets follow the scene at the render extent, while the AO factor and its blur are
    /// **post** and are a fixed fraction of the *output* extent. That is what makes the effect's
    /// screen footprint and cost identical at any render scale — the shading half of the same
    /// invariant `bloom_target_extents_ignore_the_render_scale` pins for the glow.
    #[test]
    fn ao_target_extents_split_geometry_from_post() {
        let extents = |size: FrameSize| {
            let (graph, _) = build_frame_graph(size, ssao_chain());
            graph
                .resources()
                .iter()
                .map(|r| (r.name.clone(), (r.desc.extent.width, r.desc.extent.height)))
                .collect::<Vec<_>>()
        };
        let native: std::collections::BTreeMap<_, _> =
            extents(FrameSize::native(960, 540)).into_iter().collect();
        let scaled: std::collections::BTreeMap<_, _> =
            extents(ssaa(960, 540)).into_iter().collect();

        assert_eq!(native["prepass_normal"], (960, 540));
        assert_eq!(native["prepass_depth"], (960, 540));
        assert_eq!(
            scaled["prepass_normal"],
            (1920, 1080),
            "geometry follows the render extent"
        );
        assert_eq!(scaled["prepass_depth"], (1920, 1080));

        assert_eq!(native["ssao"], (480, 270));
        assert_eq!(native["ssao_blur"], (480, 270));
        assert_eq!(
            (scaled["ssao"], scaled["ssao_blur"]),
            (native["ssao"], native["ssao_blur"]),
            "the AO factor is a fraction of the output extent at any scale",
        );
    }

    // --- Light shafts (Plan/post-process-pipeline.md) --------------------------------------

    fn shafts_chain() -> PostChain {
        PostChain {
            shadow_map: Some(QualityParams::FULL.shadow_map_size),
            shafts: true,
            ..PostChain::default()
        }
    }

    fn sun_scene() -> RenderScene {
        let mut scene = RenderScene::empty((1920, 1080));
        scene.sun = Some(crate::scene::SunData {
            direction: ludion_core::math::glam::Vec3::new(0.0, -1.0, 0.0),
            color: ludion_core::math::glam::Vec3::ONE,
            light_space: ludion_core::math::glam::Mat4::IDENTITY,
            bias: 0.001,
        });
        scene
    }

    /// `shafts` is a strength and **one third** of the gate: the sun is the second third, because the
    /// march reads a shadow map only a `DirectionalLight` builds, and the medium's extinction is the
    /// last, because what the march estimates is that medium's in-scatter. A scene missing any of the
    /// three derives `false` and simply gets none of the passes, which is the component's "absence is
    /// meaningful" posture rather than an error.
    ///
    /// The medium third is the one worth spelling out in a test: an empty medium takes σ_s to exactly
    /// zero, so the two passes would march thirty-two steps and blur them to produce a term the
    /// composite adds as `shafts_color * 0.0`. The arc's rule is that a neutral effect is skipped, not
    /// computed, which is what `ssao` and `dof` already do with their own neutrals.
    #[test]
    fn for_scene_gates_shafts_on_the_strength_the_sun_and_the_medium() {
        let post = crate::scene::PostData::default();
        let medium = crate::scene::PostData {
            shafts: 0.8,
            fog_density: 0.02,
            ..post
        };
        let mut sunless = RenderScene::empty((1920, 1080));
        assert!(!PostChain::for_scene(&sunless).shafts);
        sunless.post = Some(medium);
        assert!(
            !PostChain::for_scene(&sunless).shafts,
            "shafts asked for on a sunless scene are not built",
        );

        let mut sunny = sun_scene();
        assert!(
            !PostChain::for_scene(&sunny).shafts,
            "a sun alone is not it"
        );
        sunny.post = Some(post);
        assert!(!PostChain::for_scene(&sunny).shafts, "zero is off");
        sunny.post = Some(crate::scene::PostData {
            shafts: 0.8,
            ..post
        });
        assert!(
            !PostChain::for_scene(&sunny).shafts,
            "a strength and a sun with no medium to scatter in are not built either",
        );
        sunny.post = Some(crate::scene::PostData {
            fog_density: 0.02,
            ..post
        });
        assert!(
            !PostChain::for_scene(&sunny).shafts,
            "and a medium alone is not it — the directional term is still unasked for",
        );
        sunny.post = Some(medium);
        assert!(PostChain::for_scene(&sunny).shafts);
    }

    /// **The by-construction half of the byte-identity claim.** With shafts off the graph declares
    /// neither target and neither pass, and `composite` reads exactly the three inputs it always did —
    /// so the optional slot costs an off frame nothing but a 1×1 fallback view.
    #[test]
    fn shafts_off_graph_declares_no_shaft_passes_or_resources() {
        for chain in [
            PostChain::default(),
            PostChain {
                shadow_map: Some(QualityParams::FULL.shadow_map_size),
                ..PostChain::default()
            },
        ] {
            let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
            assert!(!resource_names(&graph).contains(&pass::SHAFTS));
            assert!(!resource_names(&graph).contains(&pass::SHAFTS_BLUR));
            assert!(!pass_names(&graph).contains(&pass::SHAFTS));
            assert!(!pass_names(&graph).contains(&pass::SHAFTS_BLUR));
            let composite = graph
                .passes()
                .iter()
                .find(|p| p.name == pass::COMPOSITE)
                .unwrap();
            assert_eq!(composite.reads().len(), 3, "no optional input is declared");
        }
    }

    /// The shaft chain's exact shape: two transients after the bloom targets and still before
    /// `final`, two passes behind the scene pass, and a scheduled order that runs
    /// scene → shafts → shafts_blur → composite. The march reads `[scene_depth, shadow_map]` — its
    /// binding order — and `composite`'s shaft input is declared **last**, behind every required
    /// read, which is the graph half of the optional-slot rule. These lists ARE the regression
    /// detector.
    #[test]
    fn shafts_graph_appends_the_march_and_its_blur() {
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), shafts_chain());
        assert_eq!(
            resource_names(&graph),
            [
                "scene",
                "scene_depth",
                "scene_normal",
                "scene_specular",
                "shadow_map",
                "bloom_bright",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_0",
                "bloom_up_1",
                "bloom_up_2",
                "bloom_up_3",
                "bloom_up_4",
                "shafts",
                "shafts_blur",
                "final",
            ],
        );
        let expected_passes = [
            "shadow",
            "scene",
            "reflections",
            "transparents",
            "bloom_threshold",
            "bloom_down_1",
            "bloom_down_2",
            "bloom_down_3",
            "bloom_down_4",
            "bloom_down_5",
            "bloom_up_4",
            "bloom_up_3",
            "bloom_up_2",
            "bloom_up_1",
            "bloom_up_0",
            "shafts",
            "shafts_blur",
            "composite",
        ];
        assert_eq!(pass_names(&graph), expected_passes);
        assert_eq!(schedule(&graph).unwrap().order_names(), expected_passes);

        let reads = |name: &str| {
            graph
                .passes()
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(reads("shafts"), ["scene_depth", "shadow_map"]);
        assert_eq!(reads("shafts_blur"), ["shafts"]);
        assert_eq!(
            reads("composite"),
            ["scene", "bloom_up_0", "scene_depth", "shafts_blur"],
        );
    }

    /// The extent rule: both shaft targets are **post**, so both are a fixed fraction of the
    /// *output* extent and are the same size at any render scale. That is what makes a beam land on
    /// the same pixels — and cost the same — whatever quality the client asked for.
    #[test]
    fn shaft_target_extents_ignore_the_render_scale() {
        let extents = |size: FrameSize| {
            let (graph, _) = build_frame_graph(size, shafts_chain());
            graph
                .resources()
                .iter()
                .filter(|r| r.name.starts_with("shafts"))
                .map(|r| (r.name.clone(), (r.desc.extent.width, r.desc.extent.height)))
                .collect::<Vec<_>>()
        };
        let native = extents(FrameSize::native(960, 540));
        assert_eq!(
            native,
            [
                ("shafts".to_string(), (480, 270)),
                ("shafts_blur".to_string(), (480, 270)),
            ],
        );
        assert_eq!(
            native,
            extents(ssaa(960, 540)),
            "the shaft chain is a fraction of the output extent at any scale",
        );
    }

    // --- Depth of field (Plan/post-process-pipeline.md) ------------------------------------

    fn dof_chain() -> PostChain {
        PostChain {
            dof: true,
            ..PostChain::default()
        }
    }

    /// `dof` is the far field's circle of confusion *and* the gate. A scene with no `PostProcess`, and
    /// equally one whose `dof` is zero, derives the frozen chain; only a positive value turns it on.
    #[test]
    fn for_scene_reads_the_dof_gate() {
        let mut scene = RenderScene::empty((1920, 1080));
        assert!(!PostChain::for_scene(&scene).dof);
        let post = crate::scene::PostData::default();
        scene.post = Some(post);
        assert!(!PostChain::for_scene(&scene).dof);
        scene.post = Some(crate::scene::PostData {
            focus_distance: 4.0,
            ..post
        });
        assert!(
            !PostChain::for_scene(&scene).dof,
            "a focal plane with no aperture is still in focus everywhere",
        );
        scene.post = Some(crate::scene::PostData { dof: 0.02, ..post });
        assert!(PostChain::for_scene(&scene).dof);
    }

    /// **The by-construction half of the byte-identity claim.** With depth of field off the graph
    /// declares neither target and neither pass, and `composite` reads exactly the three inputs it
    /// always did — so the second optional slot costs an in-focus frame nothing but a 1×1 fallback
    /// view.
    #[test]
    fn dof_off_graph_declares_no_dof_passes_or_resources() {
        for chain in [
            PostChain::default(),
            PostChain {
                shadow_map: Some(QualityParams::FULL.shadow_map_size),
                shafts: true,
                ..PostChain::default()
            },
        ] {
            let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
            assert!(!resource_names(&graph).contains(&pass::DOF_COC));
            assert!(!resource_names(&graph).contains(&pass::DOF));
            assert!(!pass_names(&graph).contains(&pass::DOF_COC));
            assert!(!pass_names(&graph).contains(&pass::DOF));
            let composite = graph
                .passes()
                .iter()
                .find(|p| p.name == pass::COMPOSITE)
                .unwrap();
            let expected = 3 + usize::from(chain.shafts);
            assert_eq!(
                composite.reads().len(),
                expected,
                "no depth-of-field input is declared",
            );
        }
    }

    /// The depth-of-field chain's exact shape: two transients after the shaft targets and still before
    /// `final`, two passes behind the scene pass, and a scheduled order that runs
    /// scene → dof_coc → dof → composite. The prefilter reads `[scene, scene_depth]` — its binding
    /// order — and `composite`'s gather input is declared **last**, behind every required read. These
    /// lists ARE the regression detector.
    #[test]
    fn dof_graph_appends_the_prefilter_and_the_gather() {
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), dof_chain());
        assert_eq!(
            resource_names(&graph),
            [
                "scene",
                "scene_depth",
                "scene_normal",
                "scene_specular",
                "bloom_bright",
                "bloom_down_1",
                "bloom_down_2",
                "bloom_down_3",
                "bloom_down_4",
                "bloom_down_5",
                "bloom_up_0",
                "bloom_up_1",
                "bloom_up_2",
                "bloom_up_3",
                "bloom_up_4",
                "dof_coc",
                "dof",
                "final",
            ],
        );
        let expected_passes = [
            "scene",
            "reflections",
            "transparents",
            "bloom_threshold",
            "bloom_down_1",
            "bloom_down_2",
            "bloom_down_3",
            "bloom_down_4",
            "bloom_down_5",
            "bloom_up_4",
            "bloom_up_3",
            "bloom_up_2",
            "bloom_up_1",
            "bloom_up_0",
            "dof_coc",
            "dof",
            "composite",
        ];
        assert_eq!(pass_names(&graph), expected_passes);
        assert_eq!(schedule(&graph).unwrap().order_names(), expected_passes);

        let reads = |name: &str| {
            graph
                .passes()
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(reads("dof_coc"), ["scene", "scene_depth"]);
        assert_eq!(reads("dof"), ["dof_coc"]);
        assert_eq!(
            reads("composite"),
            ["scene", "bloom_up_0", "scene_depth", "dof"]
        );
    }

    /// Both optional inputs at once: the shaft term is declared before the gather, which is their
    /// binding order and the graph half of the fixed-slot rule. Getting this backwards would bind each
    /// effect's texture to the other's slot with no validation error to catch it.
    #[test]
    fn shafts_and_dof_composite_reads_are_shafts_then_dof() {
        let chain = PostChain {
            shadow_map: Some(QualityParams::FULL.shadow_map_size),
            shafts: true,
            dof: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let composite = graph
            .passes()
            .iter()
            .find(|p| p.name == pass::COMPOSITE)
            .unwrap();
        assert_eq!(
            composite
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.as_str())
                .collect::<Vec<_>>(),
            ["scene", "bloom_up_0", "scene_depth", "shafts_blur", "dof"],
        );
    }

    /// The extent rule: both depth-of-field targets are **post**, so both are a fixed fraction of
    /// the *output* extent and are the same size at any render scale. With the circle of confusion
    /// expressed as a fraction of the frame's height, that is the whole of the effect's scale
    /// invariance.
    #[test]
    fn dof_target_extents_ignore_the_render_scale() {
        let extents = |size: FrameSize| {
            let (graph, _) = build_frame_graph(size, dof_chain());
            graph
                .resources()
                .iter()
                .filter(|r| r.name.starts_with("dof"))
                .map(|r| (r.name.clone(), (r.desc.extent.width, r.desc.extent.height)))
                .collect::<Vec<_>>()
        };
        let native = extents(FrameSize::native(960, 540));
        assert_eq!(
            native,
            [
                ("dof_coc".to_string(), (480, 270)),
                ("dof".to_string(), (480, 270)),
            ],
        );
        assert_eq!(
            native,
            extents(ssaa(960, 540)),
            "the depth-of-field chain is a fraction of the output extent at any scale",
        );
    }

    /// A scale is a validated newtype: `L4011` for anything outside `[0.25, 4.0]` or non-finite, and
    /// a non-integer factor in range is perfectly ordinary in both directions (1.5 is the cheap
    /// quality step, 0.75 the cheap saving).
    #[test]
    fn render_scale_rejects_anything_outside_a_quarter_to_four() {
        for bad in [0.2, 0.0, -1.0, f32::NAN, f32::INFINITY, 5.0] {
            let error = RenderScale::new(bad).expect_err("out of range");
            assert_eq!(error.code.as_str(), "L4011", "scale {bad}");
        }
        assert_eq!(RenderScale::new(1.5).unwrap().factor(), 1.5);
        assert_eq!(RenderScale::new(0.75).unwrap().factor(), 0.75);
        assert_eq!(RenderScale::new(RenderScale::MIN).unwrap().factor(), 0.25);
        assert_eq!(RenderScale::new(RenderScale::MAX).unwrap().factor(), 4.0);
        assert!(RenderScale::new(1.0).unwrap().is_native());
        assert_eq!(RenderScale::default(), RenderScale::NATIVE);
    }

    /// The render extent rounds per axis and never collapses; a scale that rounds back onto the
    /// output extent leaves nothing to resample, so the frame takes neither pass and the graph keeps
    /// its native shape. Rounding is also the only way one axis can sit at the output extent while
    /// the other moves — which is a resample in one direction, never a mixed one.
    #[test]
    fn scaled_frame_sizes_round_per_axis() {
        let one_and_a_half = RenderScale::new(1.5).unwrap();
        assert_eq!(FrameSize::scaled(101, 55, one_and_a_half).render, (152, 83));
        assert_eq!(FrameSize::scaled(1, 1, one_and_a_half).render, (2, 2));
        assert_eq!(
            FrameSize::scaled(101, 55, RenderScale::new(0.5).unwrap()).render,
            (51, 28),
        );
        // 1×1 at a scale that rounds back to 1×1, from either side: no resampling pass, the native
        // graph exactly.
        for factor in [1.4, 0.6] {
            let barely = FrameSize::scaled(1, 1, RenderScale::new(factor).unwrap());
            assert_eq!(barely.render, (1, 1), "scale {factor}");
            assert!(!barely.supersampled());
            assert_eq!(barely.resample(), None);
            let (graph, _) = build_frame_graph(barely, PostChain::default());
            assert_eq!(pass_names(&graph).len(), 15, "scale {factor}");
        }
        // A tall frame at a scale whose short axis rounds back onto the output: one axis equal, one
        // magnified, and the answer is still a single direction.
        let one_axis = FrameSize::scaled(1, 100, RenderScale::new(0.9).unwrap());
        assert_eq!(one_axis.render, (1, 90));
        assert_eq!(one_axis.resample(), Some(pass::UPSCALE));
    }

    /// The one extent pair no constructor produces and no pass could serve: magnified on one axis,
    /// minified on the other. `FrameSize`'s fields are public, so it is representable; the assertion
    /// is what keeps it from silently reaching a shader through a ratio with one component above 1
    /// and one below.
    #[test]
    #[should_panic(expected = "cannot magnify one axis and minify the other")]
    fn a_mixed_extent_pair_is_refused_rather_than_resampled() {
        FrameSize {
            output: (100, 100),
            render: (200, 50),
        }
        .resample();
    }
}
