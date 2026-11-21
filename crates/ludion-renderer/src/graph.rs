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
//! - **local_shadow** — present only with a shadowed spot or point light ([`PostChain::local_shadow_atlas`]):
//!   a depth-only atlas of per-light views, culled per view, drawn after the sun's `shadow` pass.
//! - **drop_shadow_occlusion / projector** — present only with a drop shadow ([`PostChain::drop_shadow_atlas`])
//!   or a decal ([`PostChain::decals`]): a depth-only atlas of straight-down tiles, one per caster,
//!   then the generic projector pass that paints each decal and multiplies each shadow's footprint
//!   into `scene`, after `reflections` and before `transparents`.
//! - **rain_occlusion** — present only with rain ([`PostChain::rain`]): a depth-only straight-down map of
//!   the shadow casters over the rain volume, scheduled ahead of `scene`, which reads it last to wet the
//!   surfaces the rain reaches, and read by `transparents` and `transparent_depth` as their last input,
//!   so the streaks stop under a roof.
//! - **decal_receivers** — present only when a decal lists receiver meshes ([`PostChain::decal_receivers`]):
//!   the listed meshes redrawn into an id target, depth-matched against `scene_depth`, read by both decal
//!   passes. It sits right after `scene`.
//! - **decals_lit** — present only with an `alpha` or `coat` decal ([`PostChain::lit_decals`]): the lit
//!   decals shaded like a mesh into `scene`, after `scene` and before the reflections, so the reflections
//!   see the painted colour. The projector then paints the `multiply`/`add` decals.
//! - **transparents** — the sky, ground markers, sprites/billboards (alpha, depth-tested), particles
//!   and text, over the same target.
//! - **bloom_threshold / bloom_down_1..5 / bloom_up_4..0** — a six-level bloom pyramid: a bright-pass
//!   writing level 0, five progressive 13-tap downsamples (Karis-averaged on the first, so a lone
//!   bright pixel cannot ride the chain down as a firefly) and five 3×3 tent upsamples adding each
//!   level back. It composites the sum of all six levels, which is why `bloom_strength` ships near
//!   `0.1` rather than near `1`.
//! - **lens_down_1..5 / lens_up_4..0** — the anamorphic streak chain, present only when
//!   `PostProcess.anamorphic` is above zero: five x-only downsamples off `bloom_down_1` (extent
//!   `width >> (2 + k)` by `height >> 2`) and five horizontal tent upsamples. `lens_up_0` is
//!   `composite`'s last optional read.
//! - **exposure_meter** — present only with `exposure_mode: metered` ([`PostChain::exposure_meter`]) and no
//!   debug view: one compute workgroup bins the HDR `scene`'s log2 luminance on a fixed sample grid into
//!   the 65x1 `exposure` table, whose last texel `composite` reads (after `lens_up_0`) to scale its
//!   exposure the same frame.
//!   With [`PostChain::exposure_meter_view`] set, [`pass::EXPOSURE_METER_DEBUG`] replaces `composite`, reads the
//!   `scene` and the table, and draws the metered frame with its histogram; the meter stays scheduled.
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
//! [`PostChain::ssr`] adds the trace's two input pyramids between `reflections` and `transparents`,
//! at the trace extent (output × [`QualityParams::ssr_scale`]): `hiz` (`R32Float`, a per-cell min-Z of `scene_depth`) and
//! `ssr_color` (`Rgba16Float`, the lit `scene` after the fill, pre-convolved per mip), each a seed pass plus one
//! reduce pass per further mip. The `ssr_trace` pass then marches a ray per trace texel through them
//! into `ssr_trace` (`Rgba16Float`: radiance and confidence), which `ssr_blend` mixes over the fill. With
//! [`PostChain::ssr_view`] set, [`pass::SSR_DEBUG`] replaces `composite` and draws the trace itself;
//! [`PostChain::probe_view`] does the same with [`pass::PROBE_DEBUG`], drawing the winning probe per pixel,
//! and [`PostChain::local_shadow_view`] with [`pass::LOCAL_SHADOW_DEBUG`], drawing the local-light shadow atlas.
//!
//! ```text
//! scene ─► reflections ─► hiz_seed ─► hiz_reduce_1 ─► … ─► ssr_color_seed ─► ssr_color_reduce_1 ─► … ─► ssr_trace ─► ssr_blend
//! ```
//!
//! The scene's three passes write `scene` and `scene_depth` between them, which the graph orders by
//! one rule: **passes that write the same subresource run in declaration order**
//! ([`schedule`](fn@schedule): a later writer follows an earlier one, and a reader follows the
//! latest writer declared before it). `scene` rasterizes the opaque meshes, `reflections` reads
//! their depth, normal and specular targets and adds into `scene` (and, with SSR on, writes the fill
//! to `refl_fill` too), `ssr_blend` adds the trace's difference from that fill, and `transparents`
//! draws the rest into `scene` against `scene_depth`.
//!
//! When the scene carries screen-space UI ([`PostChain::ui`], `Plan/phase-14-ui.md` decision 5), the
//! **composite** pass instead writes an LDR transient (`ldr`, `Rgba8UnormSrgb`, or `Rgba16Float` when the
//! resample owns the output dither: [`PostChain::resample_owns_output`]) and a trailing
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
//! composite ─► ldr ─► overlay ─► screen_fx_0 ─► screen_fx_1 ─► final     [native, two screen effects]
//! ```
//!
//! With screen effects the overlay writes the whole-frame transient `screen_fx_layer_0` instead of
//! the output, and each `screen_fx_k` pass blits its layer, draws effect `k` over its rect and the next
//! slice of UI on top, writing the next layer, or the output for the last.
//!
//! With [`AntiAliasing::Smaa`] the composite writes `aa_input` instead, and three passes stand
//! between it and the seam it would have written (`ldr`, or `final` when nothing follows):
//!
//! ```text
//! composite ─► aa_input ─► smaa_edges ─► smaa_weights ─► smaa_blend ─► (ldr ─► …) | final
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
    BufferDesc, BufferUsage, MAX_TEXTURE_DIMENSION_2D, RenderGraph, ResourceId, TextureDesc,
    schedule,
};

use crate::quality::{Quality, QualityParams};
use crate::scene::{DecalBlend, RenderScene};

/// The dispatch labels of the frame graph's passes. Shared between graph construction (as the
/// external-pass labels) and the renderer's recorder (as the match arms), so the two never drift.
pub mod pass {
    /// The depth-only sun shadow pass: every mesh drawn into the orthographic shadow map. Present
    /// only when the frame draws one — the scene carries a sun and this machine's quality keeps its
    /// shadows (`Plan/shadow-mapping.md` decision 6).
    pub const SHADOW: &str = "shadow";
    /// The depth-only local-light shadow pass: each granted spot or point-light view drawn, culled
    /// to that view, into its tile of the `local_shadow_atlas`. Present only when at least one
    /// casting light is granted a view and this machine's quality keeps shadows.
    pub const LOCAL_SHADOW: &str = "local_shadow";
    /// The opaque scene pass: every mesh, lit, into the HDR colour target plus the normal+roughness
    /// and F0+occlusion targets the reflections pass reads.
    pub const SCENE: &str = "scene";
    /// The reflections pass: a fullscreen additive draw over the opaque scene. It reads its normal,
    /// specular and depth targets and adds the probe and sky fill into `scene`; with SSR on it also
    /// writes the fill to the `refl_fill` transient.
    pub const REFLECTIONS: &str = "reflections";
    /// The Hi-Z seed: scene depth into `hiz` mip 0 as a per-cell minimum.
    pub const HIZ_SEED: &str = "hiz_seed";
    /// A Hi-Z reduce: one `hiz` mip's per-cell minimum over the mip above. Named `hiz_reduce_{k}`
    /// for the mip it writes.
    pub const HIZ_REDUCE: &str = "hiz_reduce";
    /// The colour-pyramid seed: the lit `scene` into `ssr_color` mip 0, after `reflections`' fill.
    pub const SSR_COLOR_SEED: &str = "ssr_color_seed";
    /// A colour-pyramid reduce: a 4×4 tent over the mip above. Named `ssr_color_reduce_{k}` for the
    /// mip it writes.
    pub const SSR_COLOR_REDUCE: &str = "ssr_color_reduce";
    /// The SSR trace: one mirror ray per trace texel marched through `hiz`, written to `ssr_trace` as
    /// radiance and confidence. Reads both pyramids whole, the opaque normal and the depth.
    pub const SSR_TRACE: &str = "ssr_trace";
    /// The SSR blend: mixes the upsampled trace over the fill by its confidence, additive into
    /// `scene`. Reads `refl_fill`, the opaque normal, specular and depth, and the trace.
    pub const SSR_BLEND: &str = "ssr_blend";
    /// A full-screen view of the trace (`ludion render --debug-view`): replaces `composite`, reads
    /// `ssr_trace` and writes the same target.
    pub const SSR_DEBUG: &str = "ssr_debug";
    /// A full-screen view of the winning reflection probe per pixel (`ludion render --debug-view
    /// probe_index`): replaces `composite`, reads the normal and depth and writes the same target.
    pub const PROBE_DEBUG: &str = "probe_debug";
    /// A full-screen view of the local-light shadow atlas (`ludion render --debug-view
    /// local_shadow_atlas`): replaces `composite`, reads the atlas and writes the same target.
    pub const LOCAL_SHADOW_DEBUG: &str = "local_shadow_debug";
    /// The `irradiance` debug view (`ludion render --debug-view irradiance`): replaces `composite`,
    /// blitting the `scene` target the mesh pass wrote as `E / IRRADIANCE_VIEW_SCALE`.
    pub const IRRADIANCE_DEBUG: &str = "irradiance_debug";
    /// The `exposure_meter` debug view (`ludion render --debug-view exposure_meter`): replaces
    /// `composite`, reading the `scene` and the `exposure` table to draw the metered frame, dimmed,
    /// with the histogram over its bottom quarter.
    pub const EXPOSURE_METER_DEBUG: &str = "exposure_meter_debug";
    /// A probe capture's last pass: the lit face into the capture target, with sky visibility in
    /// alpha. Reads `scene` and `scene_depth`.
    pub const PROBE_FACE: &str = "probe_face";
    /// The depth-only drop-shadow occlusion pass: one straight-down orthographic depth tile per
    /// caster in a single atlas, drawing every mesh except that caster's own. Present only when the
    /// scene carries a drop shadow, at every quality (`Plan/shadow-mapping.md` decision 8).
    pub const DROP_SHADOW_OCCLUSION: &str = "drop_shadow_occlusion";
    /// The generic projector pass: one box volume per instance, the world position rebuilt from
    /// `scene_depth`, the instance's analytic footprint evaluated and multiplied into `scene`. It is
    /// present exactly when the scene carries a drop shadow (each one a projector instance) or a
    /// decal, so a drop shadow's occlusion atlas and a decal's receiver ids both feed it. Reads
    /// `[scene_depth, surface_normal, scene_normal?, drop_shadow_occlusion?, decal_receivers?]`;
    /// `scene_normal` is listed only when the lit decals painted a copy (the `surface_normal`).
    pub const PROJECTOR: &str = "projector";
    /// The lit decal pass: the `alpha` and `coat` decals shaded like a mesh into `scene`, right after
    /// the opaque pass and before the probe relight and the reflections. Reads `[scene_depth,
    /// scene_normal, decal_receivers?]` then everything the scene pass reads. Draw order across the
    /// decal passes is "every lit decal, then every multiply/add decal, then every drop shadow",
    /// `(layer, EntityId)` within each; present exactly when a scene carries a lit decal.
    pub const DECALS_LIT: &str = "decals_lit";
    /// The decal receiver-id pass: redraws the meshes some decal lists, writing `mesh index + 1` into
    /// an `R32Float` target where the redraw's depth matches `scene_depth` (0 elsewhere), so the
    /// projector can honour a decal's `Exclude`/`Only` list per pixel. Present only when a decal's
    /// receiver list names a mesh, right after [`SCENE`] and so before [`DECALS_LIT`] and [`PROJECTOR`].
    pub const DECAL_RECEIVERS: &str = "decal_receivers";
    /// Everything drawn after the reflections: the sky, ground markers, sprites, particles and
    /// text, depth-tested against the opaque depth and writing none. No mesh is transparent.
    pub const TRANSPARENTS: &str = "transparents";
    /// The transparent layer: sprites, particles and world text redrawn right after
    /// [`TRANSPARENTS`] into an `Rg16Float` target holding weighted view distance and coverage,
    /// so a later reader can place a transparent's own distance where `scene_depth` only has the
    /// opaque one. Present only when the scene has a transparent and fog or depth of field
    /// ([`PostChain::transparent_depth_scheduled`](super::PostChain::transparent_depth_scheduled)).
    pub const TRANSPARENT_DEPTH: &str = "transparent_depth";
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
    /// The anamorphic streak chain's progressive x-only downsamples. **Index `i` is the pass that
    /// writes level `i + 1`**, reading level `i` (`BLOOM_DOWN[0]`'s target is level 0 here), so
    /// `LENS_DOWN[0]` halves `bloom_down_1` along x and `LENS_DOWN[4]` writes the coarsest level. They
    /// execute in ascending index order.
    pub const LENS_DOWN: [&str; 5] = [
        "lens_down_1",
        "lens_down_2",
        "lens_down_3",
        "lens_down_4",
        "lens_down_5",
    ];
    /// The streak chain's horizontal tent upsamples. **Index `i` is the pass that writes level `i`**,
    /// reading the coarser accumulation and level `i`'s own downsample result. They execute in
    /// *descending* index order (4 → 0), so `LENS_UP[0]` runs last and is what `composite` mixes in.
    pub const LENS_UP: [&str; 5] = [
        "lens_up_0",
        "lens_up_1",
        "lens_up_2",
        "lens_up_3",
        "lens_up_4",
    ];
    /// The depth+normal prepass: every opaque mesh rasterized into its own depth attachment and a
    /// world-space normal target, ahead of the scene pass. Present only when the scene enables
    /// ambient occlusion ([`PostChain::ssao`](super::PostChain::ssao), `Plan/post-process-pipeline.md` decision 8).
    ///
    /// It owns its depth rather than sharing `scene_depth`, so the scene pass keeps its own `Less`
    /// test and clear.
    pub const PREPASS: &str = "prepass";
    /// The occluded-silhouette mask: only the meshes that resolve an `OccludedSilhouette`, drawn into
    /// an `R8Unorm` style-index target and their own sampled depth, which `composite` reads to tint
    /// the hidden pixels. Present only when
    /// [`PostChain::silhouette`](super::PostChain::silhouette) is set.
    pub const SILHOUETTE_MASK: &str = "silhouette_mask";
    /// The rain's overhead occlusion: every shadow caster over the rain volume drawn straight down
    /// into a depth map the rain's fragments read to hide streaks under a roof. Present only when
    /// [`PostChain::rain`](super::PostChain::rain) is set.
    pub const RAIN_OCCLUSION: &str = "rain_occlusion";
    /// The GPU frustum cull: a **compute** pass testing every world mesh instance against the
    /// camera the frame draws with and compacting the survivors into the buffers `prepass` and
    /// `scene` draw indirectly from. Present only when
    /// [`PostChain::cull`](super::PostChain::cull) is `Some`.
    pub const CULL: &str = "cull";
    /// The dynamic-light injection: the frame graph's one **compute** pass, replaying the GI bake's
    /// cached hits against a world-space bin of the scene's `GiDynamic`-marked lights into a
    /// transient probe-lattice volume the scene pass samples
    /// (`Plan/gi-wave3-dynamic-injection.md` §5). Present only when
    /// [`PostChain::gi_inject`](super::PostChain::gi_inject) is `Some` — a scene with a `GiVolume`
    /// at a non-zero `bounce`, at least one marked light, and a machine whose
    /// [`Quality::gi_dynamic`](crate::Quality::gi_dynamic) is on.
    pub const GI_INJECT: &str = "gi_inject";
    /// The probe relight (`Plan/reflections.md` decision 17): a **compute** pass evaluating the
    /// diffuse irradiance at each ranked probe's capture point into the `probe_live` transient, one
    /// texel per probe, which `reflections` reads last. Present only when
    /// [`PostChain::probe_relight`](super::PostChain::probe_relight) is set — a scene with probes.
    pub const PROBE_RELIGHT: &str = "probe_relight";
    /// The metered-exposure compute pass: a fixed sample grid of the HDR `scene` and `scene_depth`
    /// binned into the 65x1 `exposure` table that `composite` reads last. Present only when
    /// [`PostChain::exposure_meter_scheduled`](super::PostChain::exposure_meter_scheduled).
    pub const EXPOSURE_METER: &str = "exposure_meter";
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
    /// The local-light in-scatter march: each selected scattering light's bounding sphere marched
    /// along the eye ray into a half-resolution term. Present only when
    /// [`PostChain::medium_scatter`](super::PostChain::medium_scatter).
    pub const MEDIUM_SCATTER: &str = "medium_scatter";
    /// The 4×4 box blur that cancels [`MEDIUM_SCATTER`]'s per-pixel step offset, over all four
    /// channels. [`PostChain::medium_scatter`](super::PostChain::medium_scatter) only.
    pub const MEDIUM_SCATTER_BLUR: &str = "medium_scatter_blur";
    /// The depth-of-field prefilter: the scene at a fraction of the output extent with each texel's
    /// signed circle of confusion in its alpha channel. [`PostChain::dof`](super::PostChain::dof) only.
    pub const DOF_COC: &str = "dof_coc";
    /// The depth-of-field gather: one disc of taps over [`DOF_COC`]'s colour+CoC target, writing the
    /// defocused image and the weight `composite` blends it in with. [`PostChain::dof`](super::PostChain::dof) only.
    pub const DOF: &str = "dof";
    /// Scene + bloom composited, then exposure, the tonemap operator, grain and the output dither (the
    /// last output pass owns the two: not here under SMAA or a resample), HDR→sRGB
    /// into the output (or into the `ldr` transient when UI is present or the frame is
    /// supersampled), or into `aa_input` when SMAA runs. Everything that is a pure per-pixel
    /// function of already-sampled colour folds into this one pass rather than costing a
    /// full-resolution round trip of its own (`Plan/post-process-pipeline.md` decision 2).
    pub const COMPOSITE: &str = "composite";
    /// SMAA luma edge detection over `aa_input` into `smaa_edges`. Only with
    /// [`PostChain::smaa_scheduled`](super::PostChain::smaa_scheduled).
    pub const SMAA_EDGES: &str = "smaa_edges";
    /// SMAA blending weights from `smaa_edges`, looked up in the AreaTex and SearchTex tables.
    pub const SMAA_WEIGHTS: &str = "smaa_weights";
    /// SMAA neighbourhood blend of `aa_input` by `smaa_weights`, then grain and dither unless a
    /// resample follows, into the target `composite` would have written.
    pub const SMAA_BLEND: &str = "smaa_blend";
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
    /// The screen-effect passes, in UI order: `SCREEN_FX[k]` blits the layer the overlay (or effect
    /// `k - 1`) wrote, draws effect `k` over its rect, then the next slice of UI, writing the next
    /// layer — or `output` for the last. Present exactly as many as the frame applies
    /// ([`crate::MAX_SCREEN_EFFECTS`] at most).
    pub const SCREEN_FX: [&str; 4] = ["screen_fx_0", "screen_fx_1", "screen_fx_2", "screen_fx_3"];
    /// The whole-frame transients between the overlay and the screen-effect passes: `SCREEN_FX_LAYER[k]`
    /// is what `SCREEN_FX[k]` reads.
    pub const SCREEN_FX_LAYER: [&str; 4] = [
        "screen_fx_layer_0",
        "screen_fx_layer_1",
        "screen_fx_layer_2",
        "screen_fx_layer_3",
    ];
}

/// The anti-aliasing a frame applies after `composite`: SMAA 1x with luma edge detection and the
/// fixed HIGH constants, at the render extent and before any resample or overlay. A client setting
/// like [`RenderScale`]: not scene content, not hashed, not on the wire.
///
/// # Example
/// ```
/// use ludion_renderer::AntiAliasing;
///
/// assert_eq!(AntiAliasing::default(), AntiAliasing::Off);
/// assert_eq!(AntiAliasing::Smaa.name(), "smaa");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AntiAliasing {
    /// No anti-aliasing: the graph is exactly the one without it.
    #[default]
    Off,
    /// Subpixel morphological anti-aliasing, one sample per pixel.
    Smaa,
}

impl AntiAliasing {
    /// Every mode, in the order a client lists them.
    pub const ALL: [AntiAliasing; 2] = [AntiAliasing::Off, AntiAliasing::Smaa];

    /// The spelling used in summaries and by clients.
    pub fn name(self) -> &'static str {
        match self {
            AntiAliasing::Off => "off",
            AntiAliasing::Smaa => "smaa",
        }
    }

    /// The mode a [`name`](Self::name) spells; `None` for anything else.
    ///
    /// ```
    /// use ludion_renderer::AntiAliasing;
    ///
    /// for mode in AntiAliasing::ALL {
    ///     assert_eq!(AntiAliasing::from_name(mode.name()), Some(mode));
    /// }
    /// assert_eq!(AntiAliasing::from_name("fxaa"), None);
    /// ```
    pub fn from_name(name: &str) -> Option<AntiAliasing> {
        AntiAliasing::ALL
            .into_iter()
            .find(|mode| mode.name() == name)
    }
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

/// The capacities of one GPU frustum cull (`cull.wgsl`): how many instances, draw buckets and views
/// its buffers hold. Capacities, not live counts, so the graph's buffer cache rarely reallocates;
/// plain `Copy` data, so it can ride in [`PostChain`].
///
/// ```
/// use ludion_renderer::CullShape;
///
/// let shape = CullShape::for_counts(1000, 5, 1, 1);
/// assert_eq!((shape.instances, shape.buckets, shape.views, shape.levels), (1024, 16, 1, 1));
/// assert_eq!(shape.instance_bytes(), 1024 * 224);
/// assert_eq!(shape.args_bytes(), 16 * 20);
/// assert_eq!(shape.scratch_bytes(), (32 + 4 + 1) * 4);
/// // A frame drawing a two-level chain doubles the lanes, never the instance buffer.
/// let chained = CullShape::for_counts(1000, 5, 1, 2);
/// assert_eq!(chained.args_bytes(), 2 * shape.args_bytes());
/// assert_eq!(chained.instance_bytes(), shape.instance_bytes());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CullShape {
    /// The instance capacity, a power of two of at least 256.
    pub instances: u32,
    /// The bucket (indirect draw) capacity, a power of two of at least 16.
    pub buckets: u32,
    /// The view count.
    pub views: u32,
    /// The levels per view (1 + the longest LOD chain drawn, 1 with none): the cull runs `views ×
    /// levels` lanes.
    pub levels: u32,
}

/// The largest storage binding the compacted-instance buffer may need: WebGPU's guaranteed
/// `max_storage_buffer_binding_size`, which `engine_limits()` keeps.
const CULL_INSTANCE_BINDING_MAX: u64 = 128 << 20;

impl CullShape {
    /// The smallest shape holding `instances` and `buckets` for `views` views: each capacity rounds up
    /// to a power of two, with floors of 256 instances and 16 buckets so small scenes share a shape.
    /// `views` is clamped so [`instance_bytes`](CullShape::instance_bytes) fits one storage binding; a
    /// view past the clamp is not culled and draws every caster directly. When not even one view fits,
    /// `views` is zero and the caller must not cull at all.
    pub fn for_counts(instances: usize, buckets: usize, views: usize, levels: usize) -> CullShape {
        let cap = |n: usize, floor: u32| (n.max(floor as usize).next_power_of_two()) as u32;
        let instances = cap(instances, 256);
        let fit = CULL_INSTANCE_BINDING_MAX / (u64::from(instances) * 224);
        CullShape {
            instances,
            buckets: cap(buckets, 16),
            views: u64::from(views as u32).min(fit) as u32,
            levels: levels.clamp(1, 4) as u32,
        }
    }

    /// Visibility words per view: one bit per instance.
    pub fn words(&self) -> u32 {
        self.instances / 32
    }

    /// Scan blocks per view: 256 instances each.
    pub fn blocks(&self) -> u32 {
        self.instances / 256
    }

    /// The lanes the cull dispatches: one per (view, level).
    pub fn lanes(&self) -> u32 {
        self.views * self.levels
    }

    /// The scratch buffer: per lane the visibility words, the block counts and the total.
    pub fn scratch_bytes(&self) -> u64 {
        u64::from(self.lanes()) * u64::from(self.words() + self.blocks() + 1) * 4
    }

    /// The compacted-instance buffer: `views` copies of `instances` 224-byte instances.
    pub fn instance_bytes(&self) -> u64 {
        u64::from(self.views) * u64::from(self.instances) * 224
    }

    /// The indirect-args buffer: one 20-byte slot per (lane, bucket).
    pub fn args_bytes(&self) -> u64 {
        u64::from(self.lanes()) * u64::from(self.buckets) * 20
    }
}

/// The cull's capacities for `scene`: the camera view plus `shadow_views` (the sun's maps, then the
/// granted local views), a bucket per world draw at most (a primitive shape and a mirror twin, or an
/// asset and its twin, each unmirrored and mirrored), and an instance per world mesh; its levels are 1
/// plus the longest LOD chain among the world instances drawn. `None` without a world mesh.
fn cull_shape(scene: &RenderScene, shadow_views: usize) -> Option<CullShape> {
    let world = scene.meshes.iter().filter(|m| !m.viewmodel).count();
    let chain = scene
        .meshes
        .iter()
        .filter(|m| !m.viewmodel)
        .filter_map(|m| match m.shape {
            crate::scene::MeshShape::Asset(i) => scene.mesh_assets.get(i),
            _ => None,
        })
        .map(|asset| asset.lods.len())
        .max()
        .unwrap_or(0);
    let shape = CullShape::for_counts(
        world,
        2 * (3 + scene.mesh_assets.len()),
        1 + shadow_views,
        1 + chain,
    );
    // A scene too big for even the camera view under the binding cap draws everything direct.
    (world > 0 && shape.views > 0).then_some(shape)
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
    /// The cascade count when the sun authors `cascades` and a map is drawn: the map is then a
    /// [`sun_atlas_grid`](crate::sun_atlas_grid) atlas of [`shadow_map`](PostChain::shadow_map)-edge
    /// tiles. `None` keeps the single square map exactly.
    pub sun_cascades: Option<u32>,
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
    /// The scene authors a medium **and** at least one local light selected to scatter into it
    /// (`scatter::select_scatter`): the [`pass::MEDIUM_SCATTER`] and
    /// [`pass::MEDIUM_SCATTER_BLUR`] passes are added behind the scene pass and `composite` gains its
    /// fourth optional input.
    ///
    /// Independent of [`shafts`](PostChain::shafts): it needs no sun and no shadow map, and a scene
    /// with a scattering light but no shaft strength still gets it. The shafts/medium quality switch
    /// masks it.
    ///
    /// This is the pass gate: it is on when either reason below holds.
    pub medium_scatter: bool,
    /// Reason one: the medium **and** a selected scattering local light.
    pub local_scatter: bool,
    /// Reason two: the medium **and** a `GiVolume` that authors a nonzero `sky_visibility` or `bounce`
    /// — the pass then also carries the GI-scaled ambient (alpha) and the bounce in-scatter. A
    /// volume at zero strength derives `false`, so such a scene's graph is the one without it.
    pub medium_gi: bool,
    /// The scene enables depth of field: the [`pass::DOF_COC`] and [`pass::DOF`] passes are added
    /// behind the scene pass and `composite` gains its second optional input. False leaves the graph
    /// with neither, which is what makes an in-focus frame byte-identical by construction rather than
    /// by care.
    pub dof: bool,
    /// The scene enables the anamorphic streaks: the ten [`pass::LENS_DOWN`]/[`pass::LENS_UP`] passes
    /// are added behind the bloom chain and `composite` gains its eighth optional input. False leaves
    /// the graph with neither, so a frame that authors no streak is graph-identical to one from before
    /// the chain existed.
    pub anamorphic: bool,
    /// The scene meters its exposure (`exposure_mode: metered`): the [`pass::EXPOSURE_METER`] compute
    /// pass is added behind bloom's bright pass and `composite` gains its ninth optional input.
    /// Content like `ui`: no quality switch masks it. False leaves the graph with neither.
    pub exposure_meter: bool,
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
    /// The GPU cull's capacities, or `None` when the frame draws no world mesh (or culling is
    /// off): the [`pass::CULL`] pass and its three buffers are declared exactly when it is `Some`.
    /// Like [`gi_inject`](PostChain::gi_inject) it carries the extent because a buffer's size is
    /// graph structure; the scene-derived counts round up to a shape the buffer cache reuses.
    /// Content, so `masked` keeps it.
    pub cull: Option<CullShape>,
    /// The scene carries world-space debug lines: the `overlay` pass is appended exactly as for
    /// [`ui`](PostChain::ui), and additionally reads `scene_depth` so the lines can be dimmed where
    /// geometry hides them.
    pub debug_lines: bool,
    /// How many screen effects the frame applies (at most [`crate::MAX_SCREEN_EFFECTS`]): the
    /// `overlay` pass exists for them alone and writes a whole-frame transient, and one
    /// [`pass::SCREEN_FX`] pass per effect follows it, the last writing the output. Content, so
    /// `masked` keeps it.
    pub screen_effects: u8,
    /// The trace's scale against the output extent, or `None` when the scene draws no screen-space
    /// reflections: the [`pass::HIZ_SEED`] / [`pass::HIZ_REDUCE`] and
    /// [`pass::SSR_COLOR_SEED`] / [`pass::SSR_COLOR_REDUCE`] pyramid passes, the trace and
    /// [`pass::SSR_BLEND`] are added between `reflections` and `transparents`, building the `hiz` and
    /// `ssr_color` transients at the trace extent and the `refl_fill` transient at the render extent.
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
    /// A full-screen view of the local-light shadow atlas: [`pass::LOCAL_SHADOW_DEBUG`] replaces
    /// `composite`, writing the same target. Set only when the scene asked for it **and** the frame has
    /// an atlas ([`local_shadow_atlas`](PostChain::local_shadow_atlas)): a view never conjures its subject.
    pub local_shadow_view: bool,
    /// The `sun_cascades` view: the mesh pass tints each fragment by its cascade
    /// ([`crate::DebugView::SunCascades`]). It tints the `scene` pass and does not replace
    /// `composite`, so it adds no pass. Set only when the scene asked for it **and** the frame fits
    /// cascades ([`sun_cascades`](PostChain::sun_cascades)): a view never conjures its subject.
    pub sun_cascade_view: bool,
    /// The `irradiance` view: the mesh pass writes the diffuse irradiance ([`crate::DebugView::Irradiance`])
    /// and [`pass::IRRADIANCE_DEBUG`] replaces `composite`. It needs no subject, so it is always set when
    /// asked for and no quality mask removes it.
    pub irradiance_view: bool,
    /// The `exposure_meter` view: [`pass::EXPOSURE_METER_DEBUG`] replaces `composite` and draws the
    /// histogram the meter binned. Set only when the scene asked for it **and** meters
    /// ([`exposure_meter`](PostChain::exposure_meter)): a view never turns its effect on.
    pub exposure_meter_view: bool,
    /// The anti-aliasing the host asked for. A client setting like [`RenderScale`]: `for_scene` leaves
    /// it [`Off`](AntiAliasing::Off), [`masked`](PostChain::masked) never changes it, and a debug view
    /// suppresses it ([`smaa_scheduled`](PostChain::smaa_scheduled)).
    pub aa: AntiAliasing,
    /// A mesh resolves an `OccludedSilhouette`: the [`pass::SILHOUETTE_MASK`] pass is added before
    /// `composite`, which gains its two optional inputs (the mask, then the mask's depth) as its last
    /// reads. `false` leaves the graph with neither the pass nor the resources, which is what makes a
    /// silhouette-free frame byte-identical by construction. It is content, so `masked` keeps it.
    pub silhouette: bool,
    /// The scene has [`Rain`](crate::RainData): the [`pass::RAIN_OCCLUSION`] pass is added before
    /// `transparents`, writing the `rain_occlusion` transient (a straight-down depth map of the shadow
    /// casters over the rain volume), which `transparents` and `transparent_depth` read as their last
    /// input. `false` leaves the graph with neither the pass nor the resource, which is what makes a
    /// rain-free frame byte-identical by construction. It is content, so `masked` keeps it.
    pub rain: bool,
    /// The scene has probes: the [`pass::PROBE_RELIGHT`] compute pass is added before `reflections`,
    /// writing the `probe_live` transient (`MAX_PROBES`×6, `Rgba32Float`: one row per cube axis) that `reflections` reads
    /// as its last input. `false` leaves the graph with neither the pass nor the resource, which is
    /// what makes a probe-free frame byte-identical by construction rather than by care. Probes have
    /// no quality toggle, so `masked` keeps it.
    pub probe_relight: bool,
    /// The edge of the drop-shadow occlusion atlas, or `None` when the scene carries no drop
    /// shadow: the [`pass::DROP_SHADOW_OCCLUSION`] and [`pass::PROJECTOR`] passes are added between
    /// `reflections` (and the SSR blend) and `transparents`, with the atlas as a transient. `None`
    /// leaves the graph with neither, which is what makes a drop-shadow-free frame byte-identical by
    /// construction. The atlas has no quality toggle, so `masked` keeps it.
    ///
    /// It carries the extent for the reason [`shadow_map`](PostChain::shadow_map) carries its edge.
    pub drop_shadow_atlas: Option<u32>,
    /// The edge of the local-light shadow atlas, or `None` when no light is granted a view: the
    /// [`pass::LOCAL_SHADOW`] pass is added after the sun's `shadow` pass, with the atlas as a
    /// transient the `scene` pass reads as its last input. `Some` only when at least one casting
    /// light is granted (`local_shadow::plan_local_shadows`); `None` leaves the graph exactly as it
    /// was before local lights could cast, which is what makes an unshadowed frame byte-identical by
    /// construction. The sun's `shadows` toggle masks it too.
    ///
    /// It carries the extent for the reason [`shadow_map`](PostChain::shadow_map) carries its edge.
    pub local_shadow_atlas: Option<u32>,
    /// The scene carries a `multiply` or `add` decal: the [`pass::PROJECTOR`] is scheduled even without
    /// a drop shadow. `false` leaves the graph with no such work, which is what makes a decal-free
    /// frame byte-identical by construction. A decal is content, so `masked` keeps it.
    pub decals: bool,
    /// The scene carries an `alpha` or `coat` decal: the [`pass::DECALS_LIT`] pass is scheduled.
    /// Content too, so `masked` keeps it.
    pub lit_decals: bool,
    /// Some decal's receiver list names a mesh: the [`pass::DECAL_RECEIVERS`] pass runs right after
    /// `scene` and writes the `decal_receivers` transient (`R32Float`, the render extent) both decal
    /// passes read. Only ever set with [`decals`](PostChain::decals) or
    /// [`lit_decals`](PostChain::lit_decals). Content too, so `masked` keeps it.
    pub decal_receivers: bool,
    /// The scene carries a sprite, a particle or a world text run: content, so `masked` keeps it.
    /// With [`fog`](PostChain::fog) or [`dof`](PostChain::dof) it schedules [`pass::TRANSPARENT_DEPTH`].
    pub transparent_cover: bool,
    /// The scene's medium has a density (`fog_density > 0`). Content too, so `masked` keeps it.
    pub fog: bool,
    /// The medium is on **and** some mesh scales it below 1 (`Mesh.medium`): the scene pass gains a
    /// fourth colour write, the [`scene_medium`](PostChain::mesh_medium_scheduled) mask, and `composite`
    /// its fifth optional input. Content, so `masked` keeps it. A frame with no such mesh keeps the
    /// graph without either.
    pub mesh_medium: bool,
    /// The medium is on **and** some sprite or non-viewmodel particle scales it below 1 (`Sprite.medium`,
    /// `ParticleEmitter.medium`): with the transparent layer scheduled
    /// ([`transparent_medium_scheduled`](PostChain::transparent_medium_scheduled)) the layer target is
    /// `Rgba16Float` and carries the weighted medium scale in B. Content, so `masked` keeps it. Split from
    /// [`mesh_medium`](PostChain::mesh_medium) because the two gate different resources.
    pub transparent_medium: bool,
    /// The scene carries a blended mesh (`RenderScene::blended`): `transparents` and `transparent_depth`
    /// declare the scene pass's lighting reads (the shadow map, the dynamic GI volume and the local
    /// shadow atlas; never ambient occlusion) in its order. Content, so `masked` keeps it. `false`
    /// leaves both passes with no reads, which is what keeps a blend-free frame's graph unchanged.
    pub blended: bool,
}

impl PostChain {
    /// Whether the `scene_medium` target is declared: [`mesh_medium`](PostChain::mesh_medium), unless a
    /// debug view replaces the composite that would read it.
    pub fn mesh_medium_scheduled(self) -> bool {
        self.mesh_medium && self.view().is_none()
    }

    /// Whether the transparent layer carries the weighted medium in B: [`transparent_medium`](PostChain::transparent_medium)
    /// on a frame that schedules the layer.
    pub fn transparent_medium_scheduled(self) -> bool {
        self.transparent_medium && self.transparent_depth_scheduled()
    }

    /// The chain with the medium's in-scatter pass dropped: a probe face has no composite to add it to.
    pub(crate) fn without_medium(self) -> Self {
        Self {
            medium_scatter: false,
            local_scatter: false,
            medium_gi: false,
            mesh_medium: false,
            transparent_medium: false,
            ..self
        }
    }

    /// The chain `scene`'s content requires — the one place the "which passes does this scene need"
    /// question is answered, so every render path and the `dump-graph` command cannot disagree about
    /// what a given scene draws.
    pub fn for_scene(scene: &RenderScene) -> Self {
        // Grants do not depend on the edge, so the scene-implied ceiling answers "any granted?".
        let local_plan = crate::local_shadow::plan_local_shadows(
            scene,
            QualityParams::FULL.local_shadow_atlas_size,
        );
        let local_shadow_atlas =
            (local_plan.stats.granted > 0).then_some(QualityParams::FULL.local_shadow_atlas_size);
        let sun_cascades = scene
            .sun
            .as_ref()
            .and_then(|sun| sun.cascades)
            .and_then(|config| QualityParams::FULL.sun_cascade_count(Some(config.count)))
            // The fit refuses a degenerate camera; the chain must say no cascades there too.
            .filter(|_| crate::sun_cascade::cascade_camera(&scene.camera).is_some());
        // The sun's shadow views: a map per cascade, else the one legacy map, else none.
        let sun_views = match (&scene.sun, sun_cascades) {
            (Some(_), Some(count)) => count as usize,
            (Some(_), None) => 1,
            (None, _) => 0,
        };
        // The medium is the first half of each reason; `select_scatter` already returns nothing
        // without extinction, so a lit-but-off-screen light derives false.
        let medium = scene.post.is_some_and(|post| post.fog_density > 0.0);
        let local_scatter = medium && !crate::scatter::select_scatter(scene).is_empty();
        let medium_gi = medium
            && scene
                .gi
                .as_ref()
                .is_some_and(|gi| gi.sky_visibility > 0.0 || gi.bounce > 0.0);
        Self {
            ui: !scene.ui.is_empty(),
            // A sun draws a shadow map, at the extent the scene's own authoring implies. Lowering it
            // — or dropping it — belongs to `masked`: the ceiling is the scene's, the discount is
            // the machine's.
            shadow_map: scene
                .sun
                .is_some()
                .then_some(QualityParams::FULL.shadow_map_size),
            sun_cascades,
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
            // The pass runs for either reason; the recorder branches on each.
            medium_scatter: local_scatter || medium_gi,
            local_scatter,
            medium_gi,
            // `dof` is the far field's circle of confusion *and* the gate: at zero every pixel is in
            // focus, so the gather would return each pixel's own colour and the blend weight would be
            // zero everywhere — two passes to reproduce their own input.
            dof: scene.post.is_some_and(|post| post.dof > 0.0),
            anamorphic: scene.post.is_some_and(|post| post.anamorphic > 0.0),
            exposure_meter: scene.post.is_some_and(|post| post.meter.is_some()),
            // The injection's three-part predicate lives in `gi_inject.rs`, beside the pass that
            // implements it, so the graph and the recorder read one function rather than two copies
            // of a conjunction.
            gi_inject: crate::gi_inject::scheduled_lattice(scene),
            cull: cull_shape(scene, sun_views + local_plan.views.len()),
            debug_lines: !scene.debug_lines.is_empty(),
            screen_effects: scene
                .screen_effects
                .len()
                .min(crate::screen_effect::MAX_SCREEN_EFFECTS) as u8,
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
            local_shadow_view: scene.debug_view == Some(crate::DebugView::LocalShadowAtlas)
                && local_shadow_atlas.is_some(),
            sun_cascade_view: scene.debug_view == Some(crate::DebugView::SunCascades)
                && sun_cascades.is_some(),
            irradiance_view: scene.debug_view == Some(crate::DebugView::Irradiance),
            exposure_meter_view: scene.debug_view == Some(crate::DebugView::ExposureMeter)
                && scene.post.is_some_and(|post| post.meter.is_some()),
            probe_relight: !scene.probes.is_empty(),
            drop_shadow_atlas: crate::projector::occlusion_atlas(scene.drop_shadows.len())
                .map(|atlas| atlas.edge),
            local_shadow_atlas,
            decals: scene
                .decals
                .iter()
                .any(|d| matches!(d.blend, DecalBlend::Multiply | DecalBlend::Add)),
            lit_decals: scene
                .decals
                .iter()
                .any(|d| matches!(d.blend, DecalBlend::Alpha | DecalBlend::Coat)),
            decal_receivers: !crate::projector::receiver_union(scene).is_empty(),
            transparent_cover: !scene.sprites.is_empty()
                || !scene.particles.is_empty()
                || !scene.texts.is_empty()
                || !scene.blended.is_empty()
                || scene.rain.is_some(),
            fog: scene.post.is_some_and(|post| post.fog_density > 0.0),
            mesh_medium: medium && scene.meshes.iter().any(|m| m.medium < 1.0),
            transparent_medium: medium
                && (scene.sprites.iter().any(|s| s.medium < 1.0)
                    || scene
                        .particles
                        .iter()
                        .any(|p| !p.viewmodel && p.medium < 1.0)
                    || scene.blended.iter().any(|b| b.instance.medium < 1.0)
                    || scene.rain.is_some_and(|r| r.medium < 1.0)),
            blended: !scene.blended.is_empty(),
            aa: AntiAliasing::Off,
            silhouette: !scene.silhouettes.meshes.is_empty(),
            rain: scene.rain.is_some(),
        }
    }

    /// Whether this frame runs the SMAA passes: SMAA is selected and no debug view stands in for the
    /// finished picture. The one predicate the graph and the recorder share. A view flag is only set
    /// when its subject exists, so a view with nothing to show still draws an anti-aliased frame.
    pub fn smaa_scheduled(self) -> bool {
        self.aa == AntiAliasing::Smaa
            && self.ssr_view.is_none()
            && !self.probe_view
            && !self.local_shadow_view
            && !self.sun_cascade_view
            && !self.irradiance_view
            && !self.exposure_meter_view
    }

    /// Whether this frame runs the `transparent_depth` pass: a transparent is drawn, a reader needs
    /// its distance (the medium has a density, or depth of field is on after the machine's mask),
    /// and no debug view stands in for the finished picture. The one predicate the graph and the
    /// recorder share.
    pub fn transparent_depth_scheduled(self) -> bool {
        self.transparent_cover && (self.fog || self.dof) && self.view().is_none()
    }

    /// Whether this frame runs the `exposure_meter` pass: the scene meters and no debug view stands in
    /// for the composite that would read the table (the `irradiance` view stays unmetered; the
    /// `exposure_meter` view draws the table, so it keeps the meter). The one predicate the graph
    /// and the recorder share.
    pub fn exposure_meter_scheduled(self) -> bool {
        self.exposure_meter && (self.view().is_none() || self.exposure_meter_view)
    }

    /// Whether the resample is the last output pass: the frame is resampled and no debug view stands in
    /// for the finished picture, so `resolve`/`upscale` apply the grain and dither at the output extent
    /// and `composite` hands them a half-float `ldr`. The one predicate the graph and the recorder share.
    pub fn resample_owns_output(self, size: FrameSize) -> bool {
        size.resample().is_some() && self.view().is_none()
    }

    /// The debug view this chain draws in place of the finished picture, if any.
    pub fn view(&self) -> Option<crate::DebugView> {
        self.ssr_view
            .or_else(|| self.probe_view.then_some(crate::DebugView::ProbeIndex))
            .or_else(|| {
                self.local_shadow_view
                    .then_some(crate::DebugView::LocalShadowAtlas)
            })
            .or_else(|| {
                self.sun_cascade_view
                    .then_some(crate::DebugView::SunCascades)
            })
            .or_else(|| self.irradiance_view.then_some(crate::DebugView::Irradiance))
            .or_else(|| {
                self.exposure_meter_view
                    .then_some(crate::DebugView::ExposureMeter)
            })
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
        // One shadows toggle covers the sun and the local lights.
        let masked_atlas = self
            .local_shadow_atlas
            .filter(|_| {
                quality.shadows
                    && crate::local_shadow::is_valid_atlas_edge(params.local_shadow_atlas_size)
            })
            .map(|_| params.local_shadow_atlas_size);
        // Re-capping the FULL count with this machine's cap is `min(authored, cap)`, the count the fit uses.
        let sun_cascades = self
            .sun_cascades
            .filter(|_| {
                shadow_map.is_some() && params.shadow_map_size >= crate::SUN_CASCADE_MIN_TILE
            })
            .and_then(|count| params.sun_cascade_count(Some(count)));
        PostChain {
            shadow_map,
            sun_cascades,
            // Masked cascades leave nothing to tint by: the normal frame is drawn.
            sun_cascade_view: self.sun_cascade_view && sun_cascades.is_some(),
            irradiance_view: self.irradiance_view,
            exposure_meter_view: self.exposure_meter_view,
            ssao: self.ssao && quality.ssao,
            // The march reads the shadow map, so the pair cannot outlive it. Keeping the map alive
            // for the shafts alone was the alternative and is worse: "shadows off" would then still
            // schedule the `shadow` pass, and masking would mean weakening rather than absence.
            shafts: self.shafts && quality.shafts && shadow_map.is_some(),
            // Needs no shadow map: the unshadowed march reads only depth.
            medium_scatter: self.medium_scatter && quality.shafts,
            local_scatter: self.local_scatter && quality.shafts,
            medium_gi: self.medium_gi && quality.shafts,
            dof: self.dof && quality.dof,
            // Off removes the trace and its pyramids; on re-sizes them to this machine's extent.
            ssr: self.ssr.filter(|_| quality.ssr).map(|_| params.ssr_scale),
            ssr_view: self.ssr_view.filter(|_| self.ssr.is_some() && quality.ssr),
            probe_view: self.probe_view,
            // Masked shadows leave no atlas to show: the normal frame is drawn.
            local_shadow_view: self.local_shadow_view && masked_atlas.is_some(),
            probe_relight: self.probe_relight,
            // Off removes the pass and its volume; on re-lays the lattice at this machine's coarser
            // probe spacing, which only ever removes probes (`gi_bake::scaled_probe_count`). The
            // spacing itself cancels out of that arithmetic, which is what lets a scene-free
            // function answer it.
            gi_inject: self
                .gi_inject
                .filter(|_| quality.gi_dynamic)
                .map(|count| crate::gi_bake::scaled_probe_count(count, params.gi_probe_scale)),
            local_shadow_atlas: masked_atlas,
            // Without any shadow pass only the camera view remains; otherwise the ceiling stays and a
            // lower cascade cap leaves some slices unused.
            cull: self.cull.map(|shape| {
                if shadow_map.is_none() && masked_atlas.is_none() {
                    CullShape { views: 1, ..shape }
                } else {
                    shape
                }
            }),
            ..self
        }
    }
}

/// The `shadow_map` transient's descriptor: the square map, or the cascade atlas of
/// `shadow_map`-edge tiles (readable back for tests, hence `CopySrc`).
fn shadow_map_desc(chain: &PostChain) -> Option<TextureDesc> {
    let edge = chain.shadow_map?;
    let Some(count) = chain.sun_cascades else {
        return Some(TextureDesc::sampled_depth(edge, edge));
    };
    let (columns, rows) = crate::sun_atlas_grid(count);
    Some(TextureDesc {
        usages: vec![
            ludion_render_graph::TextureUsage::RenderAttachment,
            ludion_render_graph::TextureUsage::TextureBinding,
            ludion_render_graph::TextureUsage::CopySrc,
        ],
        ..TextureDesc::sampled_depth(edge * columns, edge * rows)
    })
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

/// The SMAA chain's transient targets, declared together when [`PostChain::smaa_scheduled`].
#[derive(Clone, Copy)]
struct SmaaTargets {
    /// The tonemapped frame `composite` writes in place of `ldr`/`final`.
    input: ResourceId,
    /// The luma edge mask.
    edges: ResourceId,
    /// The blending weights.
    weights: ResourceId,
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

/// The local-light in-scatter targets, declared together when [`PostChain::medium_scatter`] is on.
/// Both are **post**, at the shafts' half extent.
#[derive(Clone, Copy)]
struct MediumScatterTargets {
    /// The raw per-pixel term, before the march's step offset is blurred out.
    raw: ResourceId,
    /// The blurred term `composite` adds untinted (the colour is already in it).
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

/// The anamorphic streak chain's transient targets, declared with the bloom pyramid's discipline.
#[derive(Clone, Copy)]
struct LensTargets {
    /// Level `i + 1`'s x-only downsample target.
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
    let shadow_map = shadow_map_desc(&chain).map(|desc| graph.transient("shadow_map", desc));
    // The transparent layer (only when a transparent is drawn under fog or depth of field, so every other frame's
    // resource list is exactly the layer-free graph's).
    let transparent_layer = chain.transparent_depth_scheduled().then(|| {
        // A fourth channel only when a transparent scales the medium: B holds the weighted scale.
        let desc = if chain.transparent_medium_scheduled() {
            TextureDesc::hdr_color(render_w, render_h)
        } else {
            TextureDesc::layer_color(render_w, render_h)
        };
        graph.transient(pass::TRANSPARENT_DEPTH, desc)
    });
    // The mesh medium mask (only when a mesh scales the medium below 1, so every other frame's resource
    // list is exactly the mask-free graph's).
    let scene_medium = chain
        .mesh_medium_scheduled()
        .then(|| graph.transient("scene_medium", TextureDesc::mask_color(render_w, render_h)));
    // The silhouette mask and its depth (only when a mesh resolves one, so every other frame's resource
    // list is exactly the silhouette-free graph's).
    let silhouette = chain.silhouette.then(|| {
        (
            graph.transient(
                pass::SILHOUETTE_MASK,
                TextureDesc::mask_color(render_w, render_h),
            ),
            graph.transient(
                "silhouette_depth",
                TextureDesc::sampled_depth(render_w, render_h),
            ),
        )
    });
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
    // The local-light in-scatter pair, declared after the DoF targets and still before `final`.
    let medium_scatter = chain.medium_scatter.then(|| MediumScatterTargets {
        raw: graph.transient(pass::MEDIUM_SCATTER, TextureDesc::hdr_color(half_w, half_h)),
        blurred: graph.transient(
            pass::MEDIUM_SCATTER_BLUR,
            TextureDesc::hdr_color(half_w, half_h),
        ),
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
    // The cull's three buffers: the scan scratch, the compacted instances (also the vertex source
    // the draws bind) and the indirect args. Graph buffers live one frame, so the pass rewrites all
    // three every frame it exists.
    let cull = chain.cull.map(|shape| {
        let scratch = graph.buffer(
            "cull_scratch",
            BufferDesc::new(shape.scratch_bytes(), &[BufferUsage::Storage]),
        );
        let instances = graph.buffer(
            "cull_instances",
            BufferDesc::new(
                shape.instance_bytes(),
                &[BufferUsage::Storage, BufferUsage::Vertex],
            ),
        );
        let args = graph.buffer(
            "cull_args",
            BufferDesc::new(
                shape.args_bytes(),
                &[
                    BufferUsage::Storage,
                    BufferUsage::Indirect,
                    BufferUsage::CopySrc,
                ],
            ),
        );
        (scratch, instances, args)
    });
    // The SSR input pyramids, at the trace extent (a post target: sized from the output). Declared
    // only when SSR is on, after the other conditional chains and before `final`.
    let ssr = chain.ssr.map(|scale| {
        declare_ssr_targets(&mut graph, size, scale, |graph, (tw, th)| {
            graph.transient("ssr_trace", TextureDesc::hdr_color(tw, th))
        })
    });
    // The relight's per-probe, per-axis irradiance table, written whole by one compute pass every frame it
    // exists and read by `reflections`. A transient: nothing carries between frames. 32-bit so the
    // live value compares exactly against the baked one.
    let probe_live = chain.probe_relight.then(|| {
        graph.transient(
            "probe_live",
            TextureDesc::rgba32_storage(crate::probe::MAX_PROBES as u32, 6),
        )
    });
    // The drop-shadow occlusion atlas, appended after the last conditional resource and before
    // `final` like every conditional chain, so no existing resource list moves.
    let drop_shadow_atlas = chain.drop_shadow_atlas.map(|edge| {
        graph.transient(
            pass::DROP_SHADOW_OCCLUSION,
            TextureDesc::sampled_depth(edge, edge),
        )
    });
    // The rain's overhead occlusion map, on the same terms.
    let rain_occlusion = chain.rain.then(|| {
        graph.transient(
            pass::RAIN_OCCLUSION,
            TextureDesc::sampled_depth(
                crate::rain::RAIN_OCCLUSION_SIZE,
                crate::rain::RAIN_OCCLUSION_SIZE,
            ),
        )
    });
    // The local-light shadow atlas, declared on the same terms. `CopySrc` lets a test read it back.
    let local_shadow_atlas = chain.local_shadow_atlas.map(|edge| {
        graph.transient(
            "local_shadow_atlas",
            TextureDesc {
                usages: vec![
                    ludion_render_graph::TextureUsage::RenderAttachment,
                    ludion_render_graph::TextureUsage::TextureBinding,
                    ludion_render_graph::TextureUsage::CopySrc,
                ],
                ..TextureDesc::sampled_depth(edge, edge)
            },
        )
    });
    // The decal receiver ids, on the same append-before-`final` terms. Never in a probe face: a face
    // schedules no decals.
    let decal_receivers = chain.decal_receivers.then(|| {
        graph.transient(
            pass::DECAL_RECEIVERS,
            TextureDesc::r32_float(render_w, render_h),
        )
    });
    // The lit decals' copy of `scene_normal` with the paint blended in, which every later reader of
    // the surface normal takes instead (a pass cannot sample the target it blends into).
    let lit_normal = chain
        .lit_decals
        .then(|| graph.transient("lit_normal", TextureDesc::hdr_color(render_w, render_h)));
    // The anamorphic streak chain: x-only levels off `bloom_down_1`, declared last before `final` so
    // no existing resource handle moves.
    let lens_targets = chain.anamorphic.then(|| {
        let lens_extent = |k: usize| ((width >> (2 + k)).max(1), (height >> 2).max(1));
        let (w1, h1) = level_extent(1);
        LensTargets {
            down: std::array::from_fn(|i| {
                let (w, h) = lens_extent(i + 1);
                graph.transient(pass::LENS_DOWN[i], TextureDesc::hdr_color(w, h))
            }),
            up: std::array::from_fn(|i| {
                let (w, h) = if i == 0 { (w1, h1) } else { lens_extent(i) };
                graph.transient(pass::LENS_UP[i], TextureDesc::hdr_color(w, h))
            }),
        }
    });
    // The metered-exposure table: one texel per histogram bin plus the verdict in the last. Declared
    // after the streak chain so no existing resource handle moves.
    let exposure = chain.exposure_meter_scheduled().then(|| {
        // `CopySrc`: the verdict texel is read back to the CPU.
        let mut desc = TextureDesc::rgba32_storage(65, 1);
        desc.usages.push(ludion_render_graph::TextureUsage::CopySrc);
        graph.transient("exposure", desc)
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
    // the frame to the output extent. When the resample owns the output dither it is half-float, so the
    // sub-LSB offsets have something finer than 8 bits to land on; under a debug view it stays 8-bit,
    // because those passes have no half-float twin.
    let overlay = chain.ui || chain.debug_lines || chain.screen_effects > 0;
    let ldr_desc = if chain.resample_owns_output(size) {
        TextureDesc::hdr_color(render_w, render_h)
    } else {
        TextureDesc::color(render_w, render_h)
    };
    let ldr = (overlay || resample.is_some()).then(|| graph.transient("ldr", ldr_desc));
    // Only a resampled frame that also draws UI needs a second LDR seam: the resampling pass must
    // land somewhere the overlay can sample, and that is not the present target. It holds the frame
    // at the output extent whichever pass wrote it.
    let ldr_resolved = (overlay && resample.is_some())
        .then(|| graph.transient("ldr_resolved", TextureDesc::color(width, height)));
    // The SMAA transients, declared last so every AA-off resource list is unchanged. All three are
    // linear `Rgba16Float` at the render extent: the input must keep more than 8 bits (grain and
    // dither move into the last output pass, the blend or the resample) and the edge and weight
    // values must not go through an sRGB target.
    let smaa = chain.smaa_scheduled().then(|| SmaaTargets {
        input: graph.transient("aa_input", TextureDesc::hdr_color(render_w, render_h)),
        edges: graph.transient("smaa_edges", TextureDesc::hdr_color(render_w, render_h)),
        weights: graph.transient("smaa_weights", TextureDesc::hdr_color(render_w, render_h)),
    });

    // Everything the scene pass samples, in the order `record_scene` unpacks `pass.inputs`. Every
    // entry is conditional, so the recorder indexes by which flags are set rather than by a fixed
    // slot.
    let mut scene_reads = Vec::new();
    // The cull has no texture edges: it is declared first so its buffer edges schedule it ahead of
    // every reader: the shadow passes, the prepass and the scene.
    if let Some((scratch, instances, args)) = cull {
        graph.add_external_buffer_compute_pass(
            pass::CULL,
            pass::CULL,
            vec![],
            vec![scratch, instances, args],
        );
    }
    // The depth-only shadow pass writes the shadow map with no reads; the scene pass then samples it
    // (the read edge orders scene after shadow, `depth_only_pass_orders_before_a_later_sampler`).
    // The casters are drawn from the cull's compacted slices, so both shadow passes read its buffers
    // and the cull schedules ahead of them.
    if let Some(shadow_map) = shadow_map {
        let shadow = graph.add_external_pass_with_depth(
            pass::SHADOW,
            pass::SHADOW,
            vec![],
            vec![],
            shadow_map,
        );
        if let Some((_, instances, args)) = cull {
            graph.read_buffers(shadow, &[instances, args]);
        }
        scene_reads.push(shadow_map);
    }
    // The local-light atlas is drawn beside the sun's map; the scene pass reads it last, so every
    // other scene input keeps its index.
    if let Some(atlas) = local_shadow_atlas {
        let local = graph.add_external_pass_with_depth(
            pass::LOCAL_SHADOW,
            pass::LOCAL_SHADOW,
            vec![],
            vec![],
            atlas,
        );
        if let Some((_, instances, args)) = cull {
            graph.read_buffers(local, &[instances, args]);
        }
    }
    // Prepass → ssao → blur, each ordered by the next one's read, and the scene pass's read of the
    // blurred factor is what puts the whole chain in front of it.
    if let Some(ao) = ao {
        let prepass = graph.add_external_pass_with_depth(
            pass::PREPASS,
            pass::PREPASS,
            vec![],
            vec![ao.normal],
            ao.depth,
        );
        if let Some((_, instances, args)) = cull {
            graph.read_buffers(prepass, &[instances, args]);
        }
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
    scene_reads.extend(local_shadow_atlas);
    // A blended mesh shades like the scene pass minus ambient occlusion, which reads opaque depth and
    // so means nothing behind a surface the opaque pass never drew: its two passes read the same tail
    // in the same order. A frame with none declares no reads there.
    let blended_reads: Vec<ResourceId> = if chain.blended {
        shadow_map
            .into_iter()
            .chain(gi_dynamic)
            .chain(local_shadow_atlas)
            .collect()
    } else {
        Vec::new()
    };
    // The lit decals shade like the scene pass, so they read what it reads, after their own inputs.
    let decals_lit_tail = chain.lit_decals.then(|| scene_reads.clone());
    // The rain's overhead depth map runs ahead of the scene pass, which reads it last to wet the
    // surfaces the rain reaches; `transparents` and `transparent_depth` read it last as well. The
    // lit decals bind none, so it joins the scene's reads after their tail was copied.
    if let Some(map) = rain_occlusion {
        graph.add_external_pass_with_depth(
            pass::RAIN_OCCLUSION,
            pass::RAIN_OCCLUSION,
            vec![],
            vec![],
            map,
        );
        scene_reads.push(map);
    }
    let scene_pass = graph.add_external_pass_with_depth(
        pass::SCENE,
        pass::SCENE,
        scene_reads,
        {
            let mut writes = vec![scene, scene_normal, scene_specular];
            writes.extend(scene_medium);
            writes
        },
        scene_depth,
    );
    if let Some((_, instances, args)) = cull {
        graph.read_buffers(scene_pass, &[instances, args]);
    }
    // Decal receiver ids are read from `scene_depth` alone. Both decal passes read them, so they sit
    // right after the opaque pass.
    if let Some(receivers) = decal_receivers {
        let recv = graph.add_external_pass(
            pass::DECAL_RECEIVERS,
            pass::DECAL_RECEIVERS,
            vec![scene_depth],
            vec![receivers],
        );
        // A chained mesh is redrawn at the level the cull selected for the camera.
        if let Some((_, instances, args)) = cull {
            graph.read_buffers(recv, &[instances, args]);
        }
    }
    // The lit decals paint into `scene` before the relight and the reflections, so those see the paint.
    if let Some(tail) = decals_lit_tail {
        let mut lit_reads = vec![scene_depth, scene_normal];
        lit_reads.extend(decal_receivers);
        lit_reads.extend(tail);
        let mut lit_writes = vec![scene];
        lit_writes.extend(lit_normal);
        lit_writes.push(scene_specular);
        graph.add_external_pass(pass::DECALS_LIT, pass::DECALS_LIT, lit_reads, lit_writes);
    }
    // Past the lit decals, the surface normal is the painted one.
    let surface_normal = lit_normal.unwrap_or(scene_normal);
    // Reads in the shader's binding order; additive into `scene`, after the opaque pass. The relight
    // table is the last read when the scene has probes. Like the injection, the relight reads nothing
    // the graph owns. With SSR on the pass also writes the fill into `refl_fill`, which `ssr_blend`
    // subtracts.
    let mut reflections_reads = vec![surface_normal, scene_specular, scene_depth];
    if let Some(probe_live) = probe_live {
        graph.add_external_compute_pass(
            pass::PROBE_RELIGHT,
            pass::PROBE_RELIGHT,
            vec![],
            probe_live,
        );
        reflections_reads.push(probe_live);
    }
    let refl_fill = ssr
        .is_some()
        .then(|| graph.transient("refl_fill", TextureDesc::hdr_color(render_w, render_h)));
    let mut reflections_writes = vec![scene];
    reflections_writes.extend(refl_fill);
    graph.add_external_pass(
        pass::REFLECTIONS,
        pass::REFLECTIONS,
        reflections_reads,
        reflections_writes,
    );
    // The pyramids sit between `reflections` and `transparents`: a read follows the latest writer
    // declared before it, so the colour seed sees the lit scene with the probe and sky fill, the Hi-Z
    // seed runs before `transparents` attach the depth, and the blend, which reads the trace and adds
    // into `scene`, runs before `transparents` too.
    if let (Some(ssr), Some(refl_fill)) = (&ssr, refl_fill) {
        declare_ssr_passes(&mut graph, ssr, scene, surface_normal, scene_depth);
        graph.add_external_pass(
            pass::SSR_BLEND,
            pass::SSR_BLEND,
            vec![
                refl_fill,
                surface_normal,
                scene_specular,
                scene_depth,
                ssr.trace,
            ],
            vec![scene],
        );
    }
    // The occlusion tiles, then the projector that reads them: after the last writer of `scene`
    // that lights it, before the transparents so a sprite or particle is not darkened by a shadow
    // it floats over.
    if let Some(atlas) = drop_shadow_atlas {
        graph.add_external_pass_with_depth(
            pass::DROP_SHADOW_OCCLUSION,
            pass::DROP_SHADOW_OCCLUSION,
            vec![],
            vec![],
            atlas,
        );
    }
    // The recorder maps the inputs by the chain's flags: the unpainted `scene_normal` when the lit
    // decals made a painted copy (its alpha sign flags a skinned surface, which the copy drops), the
    // atlas, then the receiver ids.
    if drop_shadow_atlas.is_some() || chain.decals {
        let mut projector_reads = vec![scene_depth, surface_normal];
        if lit_normal.is_some() {
            projector_reads.push(scene_normal);
        }
        projector_reads.extend(drop_shadow_atlas);
        projector_reads.extend(decal_receivers);
        graph.add_external_pass(
            pass::PROJECTOR,
            pass::PROJECTOR,
            projector_reads,
            vec![scene],
        );
    }
    // The two passes that draw the rain read the map last, so `record_blended`'s fixed input indices
    // do not move.
    let mut blended_reads = blended_reads;
    if let Some(map) = rain_occlusion {
        blended_reads.push(map);
    }
    graph.add_external_pass_with_depth(
        pass::TRANSPARENTS,
        pass::TRANSPARENTS,
        blended_reads.clone(),
        vec![scene],
        scene_depth,
    );
    if let Some(target) = transparent_layer {
        graph.add_external_pass_with_depth(
            pass::TRANSPARENT_DEPTH,
            pass::TRANSPARENT_DEPTH,
            blended_reads,
            vec![target],
            scene_depth,
        );
    }
    graph.add_external_pass(
        pass::BLOOM_THRESHOLD,
        pass::BLOOM_THRESHOLD,
        vec![scene],
        vec![bloom_bright],
    );
    // Reads what bloom reads: the finished HDR scene, plus the depth that tells sky from surface.
    if let Some(exposure) = exposure {
        graph.add_external_compute_pass(
            pass::EXPOSURE_METER,
            pass::EXPOSURE_METER,
            vec![scene, scene_depth],
            exposure,
        );
    }
    // The chain ends in one half-res resource the composite pass mixes in: level 0's accumulation.
    let first_level = bloom_targets.down[0];
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
    // The streak chain reads the pyramid's first level and mirrors the pyramid's read order.
    let lens_term = lens_targets.map(|LensTargets { down, up }| {
        for level in 1..=PYRAMID_LEVELS {
            let source = if level == 1 {
                first_level
            } else {
                down[level - 2]
            };
            let label = pass::LENS_DOWN[level - 1];
            graph.add_external_pass(label, label, vec![source], vec![down[level - 1]]);
        }
        for level in (0..PYRAMID_LEVELS).rev() {
            let coarser = if level == PYRAMID_LEVELS - 1 {
                down[PYRAMID_LEVELS - 1]
            } else {
                up[level + 1]
            };
            let same_level = if level == 0 {
                first_level
            } else {
                down[level - 1]
            };
            let label = pass::LENS_UP[level];
            graph.add_external_pass(label, label, vec![coarser, same_level], vec![up[level]]);
        }
        up[0]
    });
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
            // The transparent layer is the optional slot, declared last.
            [scene_depth, shadow_map]
                .into_iter()
                .chain(transparent_layer)
                .collect(),
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
            // The transparent layer is the optional slot, declared last.
            [scene, scene_depth]
                .into_iter()
                .chain(transparent_layer)
                .collect(),
            vec![dof.coc],
        );
        graph.add_external_pass(pass::DOF, pass::DOF, vec![dof.coc], vec![dof.gathered]);
        dof.gathered
    });
    // The local-light march reads the scene pass's depth and, when there are any, the transparent
    // layer and the local-shadow atlas (the optional slots, in binding order); its blur feeds
    // `composite`.
    let medium_scatter_term = medium_scatter.map(|targets| {
        graph.add_external_pass(
            pass::MEDIUM_SCATTER,
            pass::MEDIUM_SCATTER,
            [scene_depth]
                .into_iter()
                .chain(transparent_layer)
                .chain(local_shadow_atlas)
                .chain(gi_dynamic.filter(|_| chain.medium_gi))
                .collect(),
            vec![targets.raw],
        );
        graph.add_external_pass(
            pass::MEDIUM_SCATTER_BLUR,
            pass::MEDIUM_SCATTER_BLUR,
            vec![targets.raw],
            vec![targets.blurred],
        );
        targets.blurred
    });

    // Draws directly: it reads no cull buffer, because the mask holds only the silhouetted meshes.
    if let Some((mask, depth)) = silhouette {
        graph.add_external_pass_with_depth(
            pass::SILHOUETTE_MASK,
            pass::SILHOUETTE_MASK,
            vec![],
            vec![mask],
            depth,
        );
    }

    // The tail is a chain of at most three passes (six with SMAA), each writing the output directly
    // when it is the last one. Every edge below is derived by the scheduler from the next pass's
    // read.
    let composited = ldr.unwrap_or(output);
    // With SMAA the composite (or a debug view's stand-in) writes `aa_input`, and the blend writes
    // what the composite would have.
    let composite_target = smaa.map_or(composited, |smaa| smaa.input);
    // `scene_depth` is read on **every** frame, not only a fogged one: making it conditional would
    // fork `composite` into two bind group layouts and two pipelines for one shader, which the
    // sunless frame's zeroed uniform plus 1×1 fallback depth already refuses to do. The cost is that
    // the depth buffer now lives to the end of the frame instead of dying at the scene pass.
    //
    // The shaft term, the depth-of-field gather and the transparent layer are the inputs that
    // genuinely cannot be unconditional — none of their producers exists on most frames — so all are
    // declared **last**, after every required read and in their binding order. That ordering is the graph half of the
    // optional-slot rule `fullscreen_effect` states: the recorder walks the optional slots in their
    // fixed binding order and consumes a real input for each present one, so the graph must declare
    // the present ones in that same order, behind the required reads.
    let mut composite_reads = vec![scene, bloom, scene_depth];
    composite_reads.extend(shafts_term);
    composite_reads.extend(dof_term);
    composite_reads.extend(transparent_layer);
    composite_reads.extend(medium_scatter_term);
    composite_reads.extend(scene_medium);
    composite_reads.extend(
        silhouette
            .into_iter()
            .flat_map(|(mask, depth)| [mask, depth]),
    );
    composite_reads.extend(lens_term);
    composite_reads.extend(exposure);
    // A debug view stands in for the whole composite: the tail after it is unchanged. Bloom, shafts
    // and dof still schedule with unread outputs — acceptable for a debug frame.
    match (chain.ssr_view, &ssr) {
        _ if chain.exposure_meter_view => graph.add_external_pass(
            pass::EXPOSURE_METER_DEBUG,
            pass::EXPOSURE_METER_DEBUG,
            vec![scene, exposure.expect("the view implies a scheduled meter")],
            vec![composite_target],
        ),
        _ if chain.irradiance_view => graph.add_external_pass(
            pass::IRRADIANCE_DEBUG,
            pass::IRRADIANCE_DEBUG,
            vec![scene],
            vec![composite_target],
        ),
        _ if chain.local_shadow_view => graph.add_external_pass(
            pass::LOCAL_SHADOW_DEBUG,
            pass::LOCAL_SHADOW_DEBUG,
            vec![local_shadow_atlas.expect("the view implies an atlas")],
            vec![composite_target],
        ),
        _ if chain.probe_view => graph.add_external_pass(
            pass::PROBE_DEBUG,
            pass::PROBE_DEBUG,
            vec![surface_normal, scene_depth],
            vec![composite_target],
        ),
        (Some(_), Some(ssr)) => graph.add_external_pass(
            pass::SSR_DEBUG,
            pass::SSR_DEBUG,
            vec![ssr.trace],
            vec![composite_target],
        ),
        _ => graph.add_external_pass(
            pass::COMPOSITE,
            pass::COMPOSITE,
            composite_reads,
            vec![composite_target],
        ),
    };
    if let Some(smaa) = smaa {
        graph.add_external_pass(
            pass::SMAA_EDGES,
            pass::SMAA_EDGES,
            vec![smaa.input],
            vec![smaa.edges],
        );
        graph.add_external_pass(
            pass::SMAA_WEIGHTS,
            pass::SMAA_WEIGHTS,
            vec![smaa.edges],
            vec![smaa.weights],
        );
        graph.add_external_pass(
            pass::SMAA_BLEND,
            pass::SMAA_BLEND,
            vec![smaa.input, smaa.weights],
            vec![composited],
        );
    }
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
        // Effects: the overlay writes a whole-frame layer and each effect pass filters the previous
        // one. The layers are declared only here so a frame without effects keeps its resource list.
        let layers: Vec<ResourceId> = (0..usize::from(chain.screen_effects))
            .map(|k| graph.transient(pass::SCREEN_FX_LAYER[k], TextureDesc::color(width, height)))
            .collect();
        graph.add_external_pass(
            pass::OVERLAY,
            pass::OVERLAY,
            reads,
            vec![layers.first().copied().unwrap_or(output)],
        );
        for (k, layer) in layers.iter().enumerate() {
            graph.add_external_pass(
                pass::SCREEN_FX[k],
                pass::SCREEN_FX[k],
                vec![*layer],
                vec![layers.get(k + 1).copied().unwrap_or(output)],
            );
        }
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

/// The pyramids' seed and reduce passes, then the trace. They sit between `reflections` and
/// `transparents`: a read follows the latest writer declared before it, so the colour seed sees the
/// lit scene with the probe and sky fill, and the Hi-Z seed runs before `transparents` attach the
/// depth. The trace reads each pyramid whole, which orders it after every reduce.
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

/// The transparent layer as a minimal graph whose target is an **output** — it survives to readback,
/// where a transient could be aliased away and has no `COPY_SRC`. The `scene` pass has no reads, as
/// the recorder's has on a sunless, AO-free frame. It is rain-free: a rain frame's two passes read the
/// `rain_occlusion` map, which this graph does not schedule.
#[cfg(test)]
pub(crate) fn build_transparent_depth_graph(size: FrameSize) -> (RenderGraph, ResourceId) {
    let (render_w, render_h) = size.render;
    let mut graph = RenderGraph::new();
    let scene = graph.transient("scene", TextureDesc::hdr_color(render_w, render_h));
    let scene_depth = graph.transient(
        "scene_depth",
        TextureDesc::sampled_depth(render_w, render_h),
    );
    let scene_normal = graph.transient("scene_normal", TextureDesc::hdr_color(render_w, render_h));
    let scene_specular = graph.transient("scene_specular", TextureDesc::color(render_w, render_h));
    let mut desc = TextureDesc::layer_color(render_w, render_h);
    desc.usages.push(ludion_render_graph::TextureUsage::CopySrc);
    let layer = graph.output(pass::TRANSPARENT_DEPTH, desc);
    graph.add_external_pass_with_depth(
        pass::SCENE,
        pass::SCENE,
        vec![],
        vec![scene, scene_normal, scene_specular],
        scene_depth,
    );
    graph.add_external_pass_with_depth(
        pass::TRANSPARENTS,
        pass::TRANSPARENTS,
        vec![],
        vec![scene],
        scene_depth,
    );
    graph.add_external_pass_with_depth(
        pass::TRANSPARENT_DEPTH,
        pass::TRANSPARENT_DEPTH,
        vec![],
        vec![layer],
        scene_depth,
    );
    (graph, layer)
}

/// One reflection-probe cube face: the opaque scene lit as an ordinary frame (shadow, GI inject,
/// scene, reflections, transparents), then `probe_face` writes the linear HDR result with sky
/// visibility in alpha to the output. `chain` is the derived face scene's, which schedules no AO,
/// SSR, shafts, DoF, UI or debug lines. A second output, `probe_face_emissive`, carries the part of the
/// face that does not scale with the live ambient (the scene pass's fourth target), same layout.
pub(crate) fn build_probe_capture_graph(
    face: u32,
    chain: PostChain,
) -> (RenderGraph, ResourceId, ResourceId) {
    debug_assert!(
        !chain.ssao
            && chain.ssr.is_none()
            && chain.view().is_none()
            && !chain.shafts
            && !chain.medium_scatter
            && !chain.dof
            && !chain.ui
            && !chain.debug_lines
            && chain.screen_effects == 0
            && chain.drop_shadow_atlas.is_none()
            && !chain.decals
            && !chain.lit_decals
            && !chain.decal_receivers
            && !chain.rain
            && chain.local_shadow_atlas.is_none()
            && chain.sun_cascades.is_none(),
        "a probe face schedules none of the screen-space chains: {chain:?}"
    );
    debug_assert!(!chain.probe_relight, "a face scene carries no probes");
    let mut graph = RenderGraph::new();
    let scene = graph.transient("scene", TextureDesc::hdr_color(face, face));
    let scene_depth = graph.transient("scene_depth", TextureDesc::sampled_depth(face, face));
    let scene_normal = graph.transient("scene_normal", TextureDesc::hdr_color(face, face));
    let scene_specular = graph.transient("scene_specular", TextureDesc::color(face, face));
    let scene_emissive = graph.transient("scene_emissive", TextureDesc::hdr_color(face, face));
    let shadow_map = shadow_map_desc(&chain).map(|desc| graph.transient("shadow_map", desc));
    let gi_dynamic = chain.gi_inject.map(|[cx, cy, cz]| {
        graph.transient(pass::GI_INJECT, TextureDesc::hdr_storage_volume(cx, cy, cz))
    });
    let mut desc = TextureDesc::hdr_color(face, face);
    desc.usages.push(ludion_render_graph::TextureUsage::CopySrc);
    let output = graph.output(pass::PROBE_FACE, desc.clone());
    let emissive = graph.output("probe_face_emissive", desc);

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
        vec![scene, scene_normal, scene_specular, scene_emissive],
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
        vec![scene, scene_depth, scene_emissive],
        vec![output, emissive],
    );
    (graph, output, emissive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::math::glam;

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
        let (graph, output, _) = build_probe_capture_graph(64, bare);
        assert_eq!(
            pass_names(&graph),
            ["scene", "reflections", "transparents", "probe_face"]
        );
        let lit = PostChain {
            shadow_map: Some(256),
            gi_inject: Some([2, 2, 2]),
            ..bare
        };
        let (graph, output_lit, _) = build_probe_capture_graph(64, lit);
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

    fn cull_mesh(viewmodel: bool) -> crate::scene::MeshInstance {
        use ludion_core::math::glam::{Mat3, Mat4};
        crate::scene::MeshInstance {
            porosity: crate::DEFAULT_POROSITY,
            medium: 1.0,
            rim: [0.0; 4],
            shape: crate::scene::MeshShape::Cube,
            model: Mat4::IDENTITY,
            normal_matrix: Mat3::IDENTITY,
            color: [1.0; 4],
            material: [1.0, 0.0, 1.0],
            use_vertex_material: false,
            fade: 1.0,
            fade_height_min: -f32::MAX,
            flash: 0.0,
            flash_color: [1.0; 3],
            wrap: 0.0,
            scatter_color: [1.0; 3],
            bone_offset: 0,
            cast_shadows: true,
            viewmodel,
        }
    }

    /// A world mesh gives the chain a cull shape, a viewmodel-only or empty scene none, and the graph
    /// declares the pass with its three buffers exactly when the shape is there: `prepass` and `scene`
    /// read the compacted instances and the args, and the probe capture graph never has the pass.
    #[test]
    fn the_cull_pass_and_its_buffers_follow_the_chain() {
        let mut scene = RenderScene::empty((64, 64));
        assert_eq!(PostChain::for_scene(&scene).cull, None);
        scene.meshes.push(cull_mesh(true));
        assert_eq!(
            PostChain::for_scene(&scene).cull,
            None,
            "a gun is never culled"
        );
        scene.meshes.push(cull_mesh(false));
        let chain = PostChain::for_scene(&scene);
        assert_eq!(chain.cull, Some(CullShape::for_counts(1, 6, 1, 1)));
        let masked = chain.masked(&Quality::LOW, &QualityParams::LOW);
        assert_eq!(masked.cull, chain.cull, "content: masking keeps it");

        let with_ao = PostChain {
            ssao: true,
            ..chain
        };
        for chain in [chain, with_ao] {
            let (graph, _) = build_frame_graph(FrameSize::native(64, 64), chain);
            let names = pass_names(&graph);
            assert_eq!(names[0], "cull", "{names:?}");
            let buffers: Vec<&str> = graph.buffers().iter().map(|b| b.name.as_str()).collect();
            assert_eq!(buffers, ["cull_scratch", "cull_instances", "cull_args"]);
            let readers: Vec<&str> = graph
                .passes()
                .iter()
                .filter(|p| p.buffer_reads.len() == 2)
                .map(|p| p.name.as_str())
                .collect();
            let want: &[&str] = if chain.ssao {
                &["prepass", "scene"]
            } else {
                &["scene"]
            };
            assert_eq!(readers, want);
        }

        let (bare, _) = build_frame_graph(
            FrameSize::native(64, 64),
            PostChain {
                cull: None,
                ..chain
            },
        );
        assert!(!pass_names(&bare).contains(&"cull") && bare.buffers().is_empty());
        let (capture, _, _) = build_probe_capture_graph(64, PostChain::default());
        assert!(!pass_names(&capture).contains(&"cull") && capture.buffers().is_empty());
    }

    fn cull_shadow_scene() -> RenderScene {
        use crate::scene::{RenderCamera, SunData, aspect_ratio};
        use ludion_core::math::glam::{Mat4, Vec3};
        let mut scene = RenderScene::empty((64, 64));
        scene.camera = RenderCamera {
            view: Mat4::look_at_rh(Vec3::ZERO, -Vec3::Z, Vec3::Y),
            projection: Mat4::perspective_rh(1.0, aspect_ratio((64, 64)), 0.1, 100.0),
            position: Vec3::ZERO,
        };
        scene.meshes.push(cull_mesh(false));
        scene.sun = Some(SunData {
            direction: -Vec3::Y,
            color: Vec3::ONE,
            light_space: Mat4::IDENTITY,
            bias: 0.0,
            angular_radius: 0.0,
            cascades: None,
        });
        scene
    }

    fn with_point_light(scene: &mut RenderScene) {
        use crate::scene::{LocalShadowData, PointLightData};
        use ludion_core::math::glam::Vec3;
        scene.lights.push(PointLightData {
            position: Vec3::new(0.0, 0.0, -5.0),
            color: Vec3::ONE,
            intensity: 1.0,
            range: 3.0,
            dynamic: false,
            shadow: Some(LocalShadowData {
                bias: 0.05,
                near: 0.1,
                distance: None,
                fade: 0.0,
            }),
            scatter: 0.0,
        });
    }

    /// The cull has one view for the camera plus one per shadow view: one for a legacy sun map, one
    /// per cascade, six for a point light; masking every shadow off leaves the camera alone.
    #[test]
    fn the_cull_counts_the_camera_and_every_shadow_view() {
        let mut scene = cull_shadow_scene();
        let views = |scene: &RenderScene| PostChain::for_scene(scene).cull.unwrap().views;
        assert_eq!(views(&scene), 2, "camera + the legacy sun map");
        scene.sun.as_mut().unwrap().cascades = Some(crate::scene::SunCascadeConfig {
            count: 3,
            shadow_distance: 20.0,
            splits: None,
            shadow_near: 10.0,
            shadow_far: 10.0,
        });
        let chain = PostChain::for_scene(&scene);
        assert_eq!(chain.sun_cascades, Some(3));
        assert_eq!(chain.cull.unwrap().views, 4, "camera + 3 cascades");
        scene.sun.as_mut().unwrap().cascades = None;
        with_point_light(&mut scene);
        let chain = PostChain::for_scene(&scene);
        assert!(chain.local_shadow_atlas.is_some());
        assert_eq!(chain.cull.unwrap().views, 1 + 1 + 6);

        let off = Quality {
            shadows: false,
            ..Quality::FULL
        };
        let masked = chain.masked(&off, &QualityParams::FULL);
        assert!(masked.shadow_map.is_none() && masked.local_shadow_atlas.is_none());
        assert_eq!(masked.cull.unwrap().views, 1);
        let on = chain.masked(&Quality::FULL, &QualityParams::FULL);
        assert_eq!(on.cull, chain.cull);
    }

    /// `shadow` and `local_shadow` read the cull's buffers, so the cull schedules ahead of both.
    #[test]
    fn the_shadow_passes_read_the_cull_and_schedule_after_it() {
        let mut scene = cull_shadow_scene();
        with_point_light(&mut scene);
        let chain = PostChain::for_scene(&scene);
        let (graph, _) = build_frame_graph(FrameSize::native(64, 64), chain);
        let names = pass_names(&graph);
        let at = |n: &str| names.iter().position(|p| *p == n).unwrap();
        assert!(
            at("cull") < at("shadow") && at("cull") < at("local_shadow"),
            "{names:?}"
        );
        for name in ["shadow", "local_shadow"] {
            let pass = graph.passes().iter().find(|p| p.name == name).unwrap();
            assert_eq!(pass.buffer_reads.len(), 2, "{name} reads instances + args");
        }
        let (without, _) = build_frame_graph(
            FrameSize::native(64, 64),
            PostChain {
                cull: None,
                ..chain
            },
        );
        for p in without.passes() {
            assert!(p.buffer_reads.is_empty(), "{}", p.name);
        }
    }

    /// The compacted-instance buffer stays within one storage binding: the view count clamps, and
    /// reaches zero (no cull) when not even the camera view fits.
    #[test]
    fn the_cull_view_count_clamps_to_one_storage_binding() {
        assert_eq!(
            CULL_INSTANCE_BINDING_MAX,
            ludion_render_graph::engine_limits().max_storage_buffer_binding_size,
            "the WebGPU floor engine_limits keeps"
        );
        // 16 384 instances are 3.5 MiB a view: 128 MiB holds 36 of them, and 60 are requested.
        let shape = CullShape::for_counts(16_384, 6, 60, 1);
        assert_eq!(shape.views, 36);
        assert!(shape.instance_bytes() <= CULL_INSTANCE_BINDING_MAX);
        assert_eq!(
            CullShape::for_counts(524_288, 6, 7, 1).views,
            1,
            "the last size that fits"
        );
        assert_eq!(
            CullShape::for_counts(524_289, 6, 7, 1).views,
            0,
            "not even the camera view fits: no cull"
        );
        assert_eq!(CullShape::for_counts(256, 6, 7, 1).views, 7);
    }

    /// The scene pass writes the capture's fourth, emissive target, and the face pass copies it out
    /// beside the lit face.
    #[test]
    fn the_probe_capture_graph_carries_the_emissive_target() {
        let (graph, output, emissive) = build_probe_capture_graph(64, PostChain::default());
        let colors_of = |name: &str| {
            graph
                .passes()
                .iter()
                .find(|p| p.name == name)
                .expect("the pass exists")
                .color_targets()
                .to_vec()
        };
        assert_eq!(colors_of("scene").len(), 4);
        assert_eq!(colors_of("probe_face"), [output, emissive]);
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
                inner_color: [4.0, 0.4, 0.2, 0.6],
                style: crate::scene::GroundMarkerStyle::default(),
                flicker_seed: 0,
                clip: None,
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
                // Rides with `shafts`: the same switch masks it, with or without a shadow map.
                medium_scatter: bits & 12 != 0,
                local_scatter: bits & 8 != 0,
                medium_gi: bits & 4 != 0,
                dof: bits & 16 != 0,
                // Content like `ui`: no quality switch masks it, so it varies and survives.
                anamorphic: bits & 8 != 0,
                // Content like `ui`: no quality switch masks it.
                exposure_meter: bits & 1 != 0,
                gi_inject: (bits & 32 != 0).then_some([4, 4, 4]),
                cull: None,
                // Content like `ui`, so masking leaves it alone (`ui` already varies over the walk).
                debug_lines: bits & 1 != 0,
                // Content too: masking keeps the count.
                screen_effects: if bits & 1 != 0 { 2 } else { 0 },
                // Varies with `ssao` so both gates are walked; `FULL` keeps the same scale.
                ssr: (bits & 4 != 0).then_some(QualityParams::FULL.ssr_scale),
                ssr_view: None,
                // Content like `ui`: masking keeps it.
                probe_view: bits & 1 != 0,
                local_shadow_view: false,
                sun_cascade_view: false,
                irradiance_view: false,
                exposure_meter_view: false,
                probe_relight: bits & 1 != 0,
                // Content too, like the probes: no quality switch removes a drop shadow.
                drop_shadow_atlas: (bits & 1 != 0).then_some(128),
                local_shadow_atlas: None,
                sun_cascades: None,
                // Content like the drop shadows: masking keeps both.
                decals: bits & 1 != 0,
                lit_decals: bits & 1 != 0,
                decal_receivers: bits & 1 != 0,
                // Content too: no quality switch removes a transparent or the medium.
                transparent_cover: bits & 1 != 0,
                fog: bits & 1 != 0,
                mesh_medium: false,
                transparent_medium: false,
                // Content like the decals: no quality switch removes a silhouette.
                silhouette: bits & 1 != 0,
                // Content too: no quality switch removes the rain.
                rain: bits & 1 != 0,
                // Content too: no quality switch removes a blended mesh.
                blended: bits & 1 != 0,
                // A client setting: no quality preset masks it, so it varies with `ui` and survives.
                aa: if bits & 1 != 0 {
                    AntiAliasing::Smaa
                } else {
                    AntiAliasing::Off
                },
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
                    medium_scatter: false,
                    local_scatter: false,
                    medium_gi: false,
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

    /// What a pass reads and writes, by resource name.
    fn io<'g>(graph: &'g RenderGraph, pass: &str) -> (Vec<&'g str>, Vec<&'g str>) {
        let name_of = |id: &ResourceId| graph.resources()[id.index()].name.as_str();
        let p = graph.passes().iter().find(|p| p.name == pass).unwrap();
        (
            p.reads().iter().map(name_of).collect(),
            p.writes().iter().map(name_of).collect(),
        )
    }

    /// No screen effect leaves the graph exactly as it was, over every UI/debug-line/AA/resample
    /// combination: no layer is declared and the overlay still writes the output.
    #[test]
    fn no_screen_effects_leave_the_graph_untouched() {
        for bits in 0..16u8 {
            let base = PostChain {
                ui: bits & 1 != 0,
                debug_lines: bits & 2 != 0,
                aa: if bits & 4 != 0 {
                    AntiAliasing::Smaa
                } else {
                    AntiAliasing::Off
                },
                ..PostChain::default()
            };
            let size = if bits & 8 != 0 {
                FrameSize::scaled(1920, 1080, RenderScale::new(2.0).unwrap())
            } else {
                FrameSize::native(1920, 1080)
            };
            let (a, _) = build_frame_graph(size, base);
            let (b, _) = build_frame_graph(
                size,
                PostChain {
                    screen_effects: 0,
                    ..base
                },
            );
            assert_eq!(pass_names(&a), pass_names(&b));
            assert_eq!(resource_names(&a), resource_names(&b));
            assert!(
                !resource_names(&a)
                    .iter()
                    .any(|n| n.starts_with("screen_fx")),
                "{bits}: a no-effect frame declares no layer",
            );
        }
    }

    /// One effect: the overlay writes the layer, `screen_fx_0` reads it and writes `final`.
    #[test]
    fn one_screen_effect_filters_the_overlay_layer() {
        let chain = PostChain {
            ui: true,
            screen_effects: 1,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let names = pass_names(&graph);
        assert_eq!(names[names.len() - 2..], ["overlay", "screen_fx_0"]);
        assert_eq!(io(&graph, "overlay").1, ["screen_fx_layer_0"]);
        assert_eq!(
            io(&graph, "screen_fx_0"),
            (vec!["screen_fx_layer_0"], vec!["final"]),
        );
        assert!(schedule(&graph).is_ok());
    }

    /// Four effects chain through four layers, the last pass writing `final`.
    #[test]
    fn four_screen_effects_chain_through_four_layers() {
        let chain = PostChain {
            ui: true,
            screen_effects: 4,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let names = pass_names(&graph);
        assert_eq!(
            names[names.len() - 5..],
            [
                "overlay",
                "screen_fx_0",
                "screen_fx_1",
                "screen_fx_2",
                "screen_fx_3"
            ],
        );
        assert_eq!(io(&graph, "overlay").1, ["screen_fx_layer_0"]);
        for k in 0..3 {
            let (reads, writes) = io(&graph, pass::SCREEN_FX[k]);
            assert_eq!(reads, [pass::SCREEN_FX_LAYER[k]]);
            assert_eq!(writes, [pass::SCREEN_FX_LAYER[k + 1]]);
        }
        assert_eq!(
            io(&graph, "screen_fx_3"),
            (vec!["screen_fx_layer_3"], vec!["final"]),
        );
        assert!(schedule(&graph).is_ok());
    }

    /// Effects with no UI and no debug lines still build the overlay (and with it `ldr`).
    #[test]
    fn a_screen_effect_alone_builds_the_overlay_and_ldr() {
        let chain = PostChain {
            screen_effects: 1,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        assert!(pass_names(&graph).contains(&"overlay"));
        assert!(resource_names(&graph).contains(&"ldr"));
        assert_eq!(
            io(&graph, "overlay"),
            (vec!["ldr"], vec!["screen_fx_layer_0"])
        );
    }

    /// Under a resample the overlay reads `ldr_resolved`, so the layers sit at the output extent.
    #[test]
    fn a_screen_effect_under_a_resample_reads_ldr_resolved() {
        let chain = PostChain {
            screen_effects: 1,
            ..PostChain::default()
        };
        let size = FrameSize::scaled(1920, 1080, RenderScale::new(2.0).unwrap());
        let (graph, _) = build_frame_graph(size, chain);
        assert_eq!(
            io(&graph, "overlay"),
            (vec!["ldr_resolved"], vec!["screen_fx_layer_0"]),
        );
        assert!(schedule(&graph).is_ok());
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

    /// A sun authoring `cascades` derives its count; each quality caps it, and no map drops it.
    #[test]
    fn a_cascaded_sun_derives_and_masks_its_count() {
        use crate::scene::{SunCascadeConfig, SunData};
        use ludion_core::math::glam::{Mat4, Vec3};
        let mut scene = RenderScene::empty((1920, 1080));
        scene.sun = Some(SunData {
            direction: Vec3::NEG_Y,
            color: Vec3::ONE,
            light_space: Mat4::IDENTITY,
            bias: 0.0,
            angular_radius: 0.0,
            cascades: Some(SunCascadeConfig {
                count: 4,
                shadow_distance: 100.0,
                splits: None,
                shadow_near: -60.0,
                shadow_far: 60.0,
            }),
        });
        let chain = PostChain::for_scene(&scene);
        assert_eq!(chain.sun_cascades, Some(4));
        let masked =
            |quality: &Quality, params: &QualityParams| chain.masked(quality, params).sun_cascades;
        assert_eq!(masked(&Quality::FULL, &QualityParams::FULL), Some(4));
        assert_eq!(masked(&Quality::HIGH, &QualityParams::HIGH), Some(3));
        assert_eq!(masked(&Quality::MEDIUM, &QualityParams::MEDIUM), Some(2));
        let shadowless = Quality {
            shadows: false,
            ..Quality::FULL
        };
        assert_eq!(masked(&shadowless, &QualityParams::FULL), None);
    }

    fn cascaded_sun_scene() -> RenderScene {
        let mut scene = sun_scene();
        scene.sun.as_mut().unwrap().cascades = Some(crate::scene::SunCascadeConfig {
            count: 4,
            shadow_distance: 100.0,
            splits: None,
            shadow_near: -60.0,
            shadow_far: 60.0,
        });
        scene
    }

    /// The cascade view tints the `scene` pass: it needs fitted cascades, adds no pass, dies with
    /// the shadows and is named by `view()`.
    #[test]
    fn the_sun_cascade_view_needs_cascades_and_adds_no_pass() {
        let size = FrameSize::native(1920, 1080);
        let plain = cascaded_sun_scene();
        let mut viewed = cascaded_sun_scene();
        viewed.debug_view = Some(crate::DebugView::SunCascades);

        let plain_chain = PostChain::for_scene(&plain);
        assert!(!plain_chain.sun_cascade_view);
        let chain = PostChain::for_scene(&viewed);
        assert!(chain.sun_cascade_view);
        assert_eq!(chain.view(), Some(crate::DebugView::SunCascades));
        let names = |chain: PostChain| {
            let (graph, _) = build_frame_graph(size, chain);
            pass_names(&graph)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(chain), names(plain_chain));

        assert!(
            chain
                .masked(&Quality::FULL, &QualityParams::FULL)
                .sun_cascade_view
        );
        assert!(
            !chain
                .masked(&Quality::LOW, &QualityParams::LOW)
                .sun_cascade_view
        );

        let mut cascadeless = sun_scene();
        cascadeless.debug_view = Some(crate::DebugView::SunCascades);
        let none = PostChain::for_scene(&cascadeless);
        assert!(!none.sun_cascade_view);
        assert_eq!(none.view(), None);
    }

    /// The irradiance view needs no subject: it replaces `composite`, schedules no SMAA and survives
    /// every quality tier; a scene without it has neither the flag nor the pass.
    #[test]
    fn the_irradiance_view_replaces_composite_and_schedules_no_smaa() {
        let size = FrameSize::native(1920, 1080);
        let plain = PostChain::for_scene(&sun_scene());
        assert!(!plain.irradiance_view);
        let mut viewed = sun_scene();
        viewed.debug_view = Some(crate::DebugView::Irradiance);
        let mut chain = PostChain::for_scene(&viewed);
        assert!(chain.irradiance_view);
        assert_eq!(chain.view(), Some(crate::DebugView::Irradiance));
        assert!(
            chain
                .masked(&Quality::LOW, &QualityParams::LOW)
                .irradiance_view
        );
        chain.aa = AntiAliasing::Smaa;
        assert!(!chain.smaa_scheduled());

        let names = |chain: PostChain| {
            let (graph, _) = build_frame_graph(size, chain);
            pass_names(&graph)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };
        let viewed_names = names(chain);
        assert!(viewed_names.contains(&pass::IRRADIANCE_DEBUG.to_string()));
        assert!(!viewed_names.contains(&pass::COMPOSITE.to_string()));
        assert!(!viewed_names.contains(&pass::SMAA_EDGES.to_string()));
        let plain_names = names(plain);
        assert!(!plain_names.contains(&pass::IRRADIANCE_DEBUG.to_string()));
        assert!(plain_names.contains(&pass::COMPOSITE.to_string()));
    }

    /// The cascade atlas is `sun_atlas_grid` tiles of the chain's edge; an unset sun keeps the
    /// square map's descriptor exactly.
    #[test]
    fn the_cascade_atlas_is_declared_as_a_grid_of_tiles() {
        let extent = |chain: PostChain| {
            let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
            graph
                .resources()
                .iter()
                .find(|r| r.name == "shadow_map")
                .expect("the chain declares a shadow map")
                .clone()
        };
        for (count, expected) in [
            (1, (256, 256)),
            (2, (512, 256)),
            (3, (512, 512)),
            (4, (512, 512)),
        ] {
            let map = extent(PostChain {
                shadow_map: Some(256),
                sun_cascades: Some(count),
                ..PostChain::default()
            });
            assert_eq!((map.desc.extent.width, map.desc.extent.height), expected);
        }
        let unset = extent(PostChain {
            shadow_map: Some(256),
            ..PostChain::default()
        });
        assert_eq!(unset.desc, TextureDesc::sampled_depth(256, 256));
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

    /// The streak chain is absent unless asked for: no `lens_*` pass or resource, and the default
    /// chain's graph is the one it was. On, it adds ten passes in chain order whose extents halve in x
    /// only off `bloom_down_1`, and `lens_up_0` is the composite's last read.
    #[test]
    fn the_anamorphic_chain_is_absent_off_and_x_only_on() {
        let size = FrameSize::native(1920, 1080);
        let (plain, _) = build_frame_graph(size, PostChain::default());
        let is_lens = |name: &str| name.starts_with("lens_");
        assert!(plain.passes().iter().all(|p| !is_lens(&p.name)));
        assert!(plain.resources().iter().all(|r| !is_lens(&r.name)));

        let (graph, _) = build_frame_graph(
            size,
            PostChain {
                anamorphic: true,
                ..PostChain::default()
            },
        );
        let names = pass_names(&graph);
        let lens: Vec<&str> = names.iter().copied().filter(|n| is_lens(n)).collect();
        let expected: Vec<&str> = pass::LENS_DOWN
            .iter()
            .chain(pass::LENS_UP.iter().rev())
            .copied()
            .collect();
        assert_eq!(lens, expected);
        let at = |name: &str| names.iter().position(|n| *n == name).unwrap();
        assert!(at("bloom_down_1") < at("lens_down_1"));
        assert!(at("lens_up_0") < at("composite"));

        let extent = |name: &str| {
            let r = graph.resources().iter().find(|r| r.name == name).unwrap();
            (r.desc.extent.width, r.desc.extent.height)
        };
        assert_eq!(extent("bloom_down_1"), (480, 270));
        assert_eq!(extent("lens_up_0"), (480, 270));
        for (k, width) in [(1, 240), (2, 120), (3, 60), (4, 30), (5, 15)] {
            assert_eq!(extent(pass::LENS_DOWN[k - 1]), (width, 270));
            if k < 5 {
                assert_eq!(extent(pass::LENS_UP[k]), (width, 270));
            }
        }

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
        assert_eq!(reads("lens_down_1"), ["bloom_down_1"]);
        assert_eq!(reads("lens_down_3"), ["lens_down_2"]);
        assert_eq!(reads("lens_up_4"), ["lens_down_5", "lens_down_4"]);
        assert_eq!(reads("lens_up_2"), ["lens_up_3", "lens_down_2"]);
        assert_eq!(reads("lens_up_0"), ["lens_up_1", "bloom_down_1"]);
        assert_eq!(reads("composite").last().copied(), Some("lens_up_0"));
    }

    /// The streak chain is derived from the scene's own gate (`anamorphic > 0`) and no quality switch
    /// masks it.
    #[test]
    fn for_scene_gates_the_anamorphic_chain_on_the_strength() {
        let with = |anamorphic: f32| {
            let mut scene = sun_scene();
            scene.post = Some(crate::scene::PostData {
                anamorphic,
                ..crate::scene::PostData::default()
            });
            PostChain::for_scene(&scene)
        };
        assert!(!PostChain::for_scene(&sun_scene()).anamorphic);
        assert!(!with(0.0).anamorphic);
        let on = with(0.01);
        assert!(on.anamorphic);
        assert!(on.masked(&Quality::LOW, &QualityParams::LOW).anamorphic);
    }

    /// A metered scene adds exactly the `exposure_meter` pass and the `exposure` resource, which
    /// `composite` reads last; a manual scene's graph is the pre-meter shape.
    #[test]
    fn the_exposure_meter_is_absent_manual_and_composite_reads_its_table_last() {
        let size = FrameSize::native(1920, 1080);
        let (plain, _) = build_frame_graph(size, PostChain::default());
        assert!(plain.passes().iter().all(|p| p.name != "exposure_meter"));
        assert!(plain.resources().iter().all(|r| r.name != "exposure"));

        let metered_chain = PostChain {
            exposure_meter: true,
            ..PostChain::default()
        };
        let (metered, _) = build_frame_graph(size, metered_chain);
        let plain_passes = pass_names(&plain);
        let mut expected = pass_names(&metered);
        expected.retain(|n| *n != "exposure_meter");
        assert_eq!(expected, plain_passes, "the meter is the only added pass");
        let mut resources = resource_names(&metered);
        resources.retain(|n| *n != "exposure");
        assert_eq!(resources, resource_names(&plain));

        let names = pass_names(&metered);
        let at = |name: &str| names.iter().position(|n| *n == name).unwrap();
        assert!(at("bloom_threshold") < at("exposure_meter"));
        assert!(at("exposure_meter") < at("composite"));
        assert_eq!(
            reads_of(&metered, "exposure_meter"),
            ["scene", "scene_depth"]
        );
        assert_eq!(
            reads_of(&metered, "composite").last().copied(),
            Some("exposure")
        );
        let table = metered
            .resources()
            .iter()
            .find(|r| r.name == "exposure")
            .unwrap();
        assert_eq!((table.desc.extent.width, table.desc.extent.height), (65, 1));

        // With the streaks on, the table is still the last read (after `lens_up_0`).
        let (both, _) = build_frame_graph(
            size,
            PostChain {
                anamorphic: true,
                ..metered_chain
            },
        );
        let composite = reads_of(&both, "composite");
        assert_eq!(composite[composite.len() - 2..], ["lens_up_0", "exposure"]);
    }

    /// A debug view replaces `composite`, so the meter is not scheduled: no unread pass, and the
    /// `irradiance` view stays unmetered.
    #[test]
    fn a_debug_view_drops_the_exposure_meter() {
        let chain = PostChain {
            exposure_meter: true,
            irradiance_view: true,
            ..PostChain::default()
        };
        assert!(!chain.exposure_meter_scheduled());
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        assert!(graph.passes().iter().all(|p| p.name != "exposure_meter"));
        assert!(graph.resources().iter().all(|r| r.name != "exposure"));
    }

    /// The `exposure_meter` view replaces `composite`, keeps the meter it draws, schedules no SMAA,
    /// and exists only for a metered scene: a manual scene's view never turns the effect on.
    #[test]
    fn the_exposure_meter_view_replaces_composite_and_keeps_the_meter() {
        let size = FrameSize::native(1920, 1080);
        let mut scene = sun_scene();
        scene.post = Some(crate::scene::PostData {
            meter: Some(crate::scene::MeterData {
                weighting: crate::scene::MeterWeighting::Center,
                clip: [0.0, 0.0],
                range: [-2.0, 2.0],
            }),
            ..crate::scene::PostData::default()
        });
        scene.debug_view = Some(crate::DebugView::ExposureMeter);
        let mut chain = PostChain::for_scene(&scene);
        assert!(chain.exposure_meter_view);
        assert_eq!(chain.view(), Some(crate::DebugView::ExposureMeter));
        assert!(chain.exposure_meter_scheduled());
        assert!(
            chain
                .masked(&Quality::LOW, &QualityParams::LOW)
                .exposure_meter_view
        );
        chain.aa = AntiAliasing::Smaa;
        assert!(!chain.smaa_scheduled());

        let (graph, _) = build_frame_graph(size, chain);
        let names = pass_names(&graph);
        assert!(names.contains(&pass::EXPOSURE_METER));
        assert!(names.contains(&pass::EXPOSURE_METER_DEBUG));
        assert!(!names.contains(&pass::COMPOSITE));
        let at = |n: &str| names.iter().position(|p| *p == n).unwrap();
        assert!(at("exposure_meter") < at("exposure_meter_debug"));
        assert_eq!(
            reads_of(&graph, "exposure_meter_debug"),
            ["scene", "exposure"]
        );

        // A manual scene asking for the view draws the normal frame.
        scene.post = Some(crate::scene::PostData::default());
        let manual = PostChain::for_scene(&scene);
        assert!(!manual.exposure_meter_view);
        assert_eq!(manual.view(), None);
        let (graph, _) = build_frame_graph(size, manual);
        assert!(pass_names(&graph).contains(&pass::COMPOSITE));
        assert!(!pass_names(&graph).contains(&pass::EXPOSURE_METER_DEBUG));
    }

    /// The meter derives from the scene's own mode, and no quality switch masks it.
    #[test]
    fn for_scene_gates_the_exposure_meter_on_the_mode() {
        let with = |meter: Option<crate::scene::MeterData>| {
            let mut scene = sun_scene();
            scene.post = Some(crate::scene::PostData {
                meter,
                ..crate::scene::PostData::default()
            });
            PostChain::for_scene(&scene)
        };
        assert!(!PostChain::for_scene(&sun_scene()).exposure_meter);
        assert!(!with(None).exposure_meter);
        let on = with(Some(crate::scene::MeterData {
            weighting: crate::scene::MeterWeighting::Center,
            clip: [0.02, 0.02],
            range: [-2.0, 2.0],
        }));
        assert!(on.exposure_meter);
        assert!(on.masked(&Quality::LOW, &QualityParams::LOW).exposure_meter);
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

    /// A drop shadow adds a depth-only occlusion pass and the projector between `reflections` and
    /// `transparents`; the atlas is the last resource before `final`, so no existing list moves.
    #[test]
    fn a_drop_shadow_adds_the_occlusion_atlas_and_the_projector() {
        let chain = PostChain {
            drop_shadow_atlas: Some(128),
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let resources = resource_names(&graph);
        assert_eq!(
            resources[resources.len() - 2..],
            ["drop_shadow_occlusion", "final"],
        );
        assert_eq!(resources.len(), 17);
        assert_eq!(
            pass_names(&graph)[..5],
            [
                "scene",
                "reflections",
                "drop_shadow_occlusion",
                "projector",
                "transparents"
            ],
        );
        let find = |name: &str| graph.passes().iter().find(|p| p.name == name).unwrap();
        let name_of = |id: &ResourceId| graph.resources()[id.index()].name.as_str();
        let projector = find("projector");
        assert_eq!(
            projector.reads().iter().map(name_of).collect::<Vec<_>>(),
            ["scene_depth", "scene_normal", "drop_shadow_occlusion"],
        );
        assert_eq!(
            projector
                .color_targets()
                .iter()
                .map(name_of)
                .collect::<Vec<_>>(),
            ["scene"],
        );
        let occlusion = find("drop_shadow_occlusion");
        assert!(occlusion.color_targets().is_empty(), "depth-only");
        assert_eq!(
            name_of(&occlusion.depth_target().unwrap()),
            "drop_shadow_occlusion"
        );
        let atlas = graph
            .resources()
            .iter()
            .find(|r| r.name == "drop_shadow_occlusion")
            .unwrap();
        assert_eq!(
            (atlas.desc.extent.width, atlas.desc.extent.height),
            (128, 128)
        );
        assert!(schedule(&graph).is_ok());
    }

    /// With SSR on, the projector still lands after the blend and before the transparents.
    #[test]
    fn the_projector_follows_the_ssr_blend() {
        let chain = PostChain {
            drop_shadow_atlas: Some(128),
            ..ssr_chain()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let order = schedule(&graph).unwrap();
        let names = order.order_names();
        let pos = |n: &str| names.iter().position(|p| *p == n).unwrap();
        assert!(pos("ssr_blend") < pos("drop_shadow_occlusion"));
        assert!(pos("drop_shadow_occlusion") < pos("projector"));
        assert!(pos("projector") < pos("transparents"));
    }

    /// `for_scene` derives the atlas from the drop shadows; a scene without any keeps the frozen
    /// fifteen-pass graph.
    #[test]
    fn for_scene_sizes_the_atlas_from_the_drop_shadows() {
        let mut scene = RenderScene::empty((1920, 1080));
        let bare = PostChain::for_scene(&scene);
        assert_eq!(bare.drop_shadow_atlas, None);
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), bare);
        assert_eq!(pass_names(&graph).len(), 15);
        scene.drop_shadows.push(crate::scene::DropShadowInstance {
            shape: crate::scene::GroundMarkerShape::Disc { radius: 1.0 },
            anchor: ludion_core::math::glam::Vec3::ZERO,
            yaw: 0.0,
            opacity: 0.5,
            softness: 0.0,
            max_distance: 5.0,
            fade_distance: 1.0,
            exclude: Vec::new(),
        });
        assert_eq!(PostChain::for_scene(&scene).drop_shadow_atlas, Some(128));
    }

    /// No quality switch reaches the atlas: a drop shadow is content, not an effect.
    #[test]
    fn masking_keeps_the_drop_shadow_atlas() {
        let chain = PostChain {
            drop_shadow_atlas: Some(256),
            ..PostChain::default()
        };
        let low = chain.masked(
            &Quality::preset("low").unwrap(),
            &QualityParams::preset("low").unwrap(),
        );
        assert_eq!(low.drop_shadow_atlas, Some(256));
        let no_shadows = Quality {
            shadows: false,
            ..Quality::FULL
        };
        assert_eq!(
            chain
                .masked(&no_shadows, &QualityParams::FULL)
                .drop_shadow_atlas,
            Some(256)
        );
    }

    /// A decal over nothing in particular: `receivers` is its list.
    fn decal_instance(receivers: crate::scene::DecalReceivers) -> crate::scene::DecalInstance {
        use ludion_core::math::glam::Mat4;
        crate::scene::DecalInstance {
            box_to_world: Mat4::IDENTITY,
            world_to_box: Mat4::IDENTITY,
            blend: crate::scene::DecalBlend::Multiply,
            color: [1.0; 4],
            opacity: 1.0,
            roughness: None,
            metallic: 0.0,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            angle_fade: [60.0, 80.0],
            layer: 0,
            maps: crate::scene::DecalMaps::default(),
            receivers,
        }
    }

    /// A scene with no decal and no drop shadow keeps the frozen pass list: neither flag, no resource.
    #[test]
    fn a_decal_free_scene_keeps_the_frozen_graph() {
        let chain = PostChain::for_scene(&RenderScene::empty((1920, 1080)));
        assert!(!chain.decals && !chain.lit_decals && !chain.decal_receivers);
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let names = pass_names(&graph);
        assert_eq!(names.len(), 15);
        assert!(!names.contains(&"projector") && !names.contains(&"decal_receivers"));
        assert!(!resource_names(&graph).contains(&"decal_receivers"));
    }

    /// Decals alone schedule the projector once, before `transparents`, with no occlusion pass and
    /// no receiver pass, reading only the depth and the normal.
    #[test]
    fn decals_alone_schedule_one_projector_and_nothing_else() {
        let chain = PostChain {
            decals: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        assert_eq!(
            pass_names(&graph)[..4],
            ["scene", "reflections", "projector", "transparents"],
        );
        assert_eq!(pass_names(&graph).len(), 16);
        let name_of = |id: &ResourceId| graph.resources()[id.index()].name.as_str();
        let projector = graph
            .passes()
            .iter()
            .find(|p| p.name == "projector")
            .unwrap();
        assert_eq!(
            projector.reads().iter().map(name_of).collect::<Vec<_>>(),
            ["scene_depth", "scene_normal"],
        );
        assert!(!resource_names(&graph).contains(&"decal_receivers"));
        assert!(schedule(&graph).is_ok());
    }

    /// A receiver list adds the id pass right after `scene`, the id target is the last resource
    /// before `final`, and the projector reads it last.
    #[test]
    fn receiver_lists_add_the_id_pass_after_the_scene() {
        let chain = PostChain {
            decals: true,
            decal_receivers: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let resources = resource_names(&graph);
        assert_eq!(
            resources[resources.len() - 2..],
            ["decal_receivers", "final"]
        );
        assert_eq!(
            pass_names(&graph)[..5],
            [
                "scene",
                "decal_receivers",
                "reflections",
                "projector",
                "transparents"
            ],
        );
        let name_of = |id: &ResourceId| graph.resources()[id.index()].name.as_str();
        let find = |name: &str| graph.passes().iter().find(|p| p.name == name).unwrap();
        assert_eq!(
            find("decal_receivers")
                .reads()
                .iter()
                .map(name_of)
                .collect::<Vec<_>>(),
            ["scene_depth"],
        );
        assert_eq!(
            find("projector")
                .reads()
                .iter()
                .map(name_of)
                .collect::<Vec<_>>(),
            ["scene_depth", "scene_normal", "decal_receivers"],
        );
        let ids = graph
            .resources()
            .iter()
            .find(|r| r.name == "decal_receivers")
            .unwrap();
        assert_eq!(
            ids.desc.format,
            ludion_render_graph::TextureFormat::R32Float
        );
        assert_eq!(
            (ids.desc.extent.width, ids.desc.extent.height),
            (1920, 1080)
        );
        assert!(schedule(&graph).is_ok());
    }

    /// With a drop shadow too there is still one projector, after the occlusion pass, reading the
    /// atlas before the receiver ids.
    #[test]
    fn decals_and_drop_shadows_share_one_projector_after_the_occlusion_pass() {
        let chain = PostChain {
            drop_shadow_atlas: Some(128),
            decals: true,
            decal_receivers: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let names = pass_names(&graph);
        assert_eq!(names.iter().filter(|n| **n == "projector").count(), 1);
        let order = schedule(&graph).unwrap();
        let ordered = order.order_names();
        let pos = |n: &str| ordered.iter().position(|p| *p == n).unwrap();
        assert!(pos("drop_shadow_occlusion") < pos("projector"));
        assert!(pos("decal_receivers") < pos("projector"));
        assert!(pos("projector") < pos("transparents"));
        let name_of = |id: &ResourceId| graph.resources()[id.index()].name.as_str();
        let projector = graph
            .passes()
            .iter()
            .find(|p| p.name == "projector")
            .unwrap();
        assert_eq!(
            projector.reads().iter().map(name_of).collect::<Vec<_>>(),
            [
                "scene_depth",
                "scene_normal",
                "drop_shadow_occlusion",
                "decal_receivers"
            ],
        );
    }

    /// Decals are content: `masked` at low keeps both flags.
    #[test]
    fn masking_keeps_the_decal_flags() {
        let chain = PostChain {
            decals: true,
            decal_receivers: true,
            ..PostChain::default()
        };
        let low = chain.masked(
            &Quality::preset("low").unwrap(),
            &QualityParams::preset("low").unwrap(),
        );
        assert!(low.decals && low.decal_receivers);
    }

    /// A lit-only scene: the lit pass sits after `scene` and before `reflections`, and no projector
    /// is scheduled for it.
    #[test]
    fn a_lit_only_scene_schedules_decals_lit_between_scene_and_reflections() {
        let chain = PostChain {
            lit_decals: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let names = pass_names(&graph);
        assert_eq!(names[..3], ["scene", "decals_lit", "reflections"]);
        assert!(!names.contains(&"projector") && !names.contains(&"decal_receivers"));
        assert_eq!(names.len(), 16);
        let name_of = |id: &ResourceId| graph.resources()[id.index()].name.as_str();
        let lit = graph
            .passes()
            .iter()
            .find(|p| p.name == "decals_lit")
            .unwrap();
        assert_eq!(
            lit.reads().iter().map(name_of).collect::<Vec<_>>(),
            ["scene_depth", "scene_normal"],
        );
        assert_eq!(
            lit.writes().iter().map(name_of).collect::<Vec<_>>(),
            ["scene", "lit_normal", "scene_specular"]
        );
        assert!(schedule(&graph).is_ok());
    }

    /// With lit decals every reader past `decals_lit` takes the painted normal (the projector also
    /// reads the unpainted one, for the skinned flag); the scene pass keeps the receiver's. A
    /// multiply-only scene has no `lit_normal`.
    #[test]
    fn lit_decals_hand_the_painted_normal_to_the_later_readers() {
        let chain = PostChain {
            lit_decals: true,
            decals: true,
            ssr: Some(QualityParams::FULL.ssr_scale),
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let name_of = |id: &ResourceId| graph.resources()[id.index()].name.as_str();
        let reads = |pass: &str| -> Vec<&str> {
            let pass = graph.passes().iter().find(|p| p.name == pass).unwrap();
            pass.reads().iter().map(name_of).collect()
        };
        for pass in ["reflections", "ssr_trace", "ssr_blend", "projector"] {
            let r = reads(pass);
            assert!(r.contains(&"lit_normal"), "{pass}: {r:?}");
            assert!(
                !r.contains(&"scene_normal") || pass == "projector",
                "{pass}: {r:?}"
            );
        }
        // The projector also reads the unpainted normal, whose alpha sign flags a skinned surface.
        assert_eq!(
            reads("projector"),
            ["scene_depth", "lit_normal", "scene_normal"]
        );
        assert!(!reads("scene").contains(&"lit_normal"));

        let multiply_only = PostChain {
            decals: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), multiply_only);
        assert!(graph.resources().iter().all(|r| r.name != "lit_normal"));
    }

    /// Lit and multiply decals together schedule both passes, the lit one first, and the receiver
    /// ids before both.
    #[test]
    fn lit_and_multiply_decals_schedule_both_passes_lit_first() {
        let chain = PostChain {
            decals: true,
            lit_decals: true,
            decal_receivers: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let order = schedule(&graph).unwrap();
        let ordered = order.order_names();
        let pos = |n: &str| ordered.iter().position(|p| *p == n).unwrap();
        assert!(pos("scene") < pos("decal_receivers"));
        assert!(pos("decal_receivers") < pos("decals_lit"));
        assert!(pos("decals_lit") < pos("reflections"));
        assert!(pos("reflections") < pos("projector"));
        let name_of = |id: &ResourceId| graph.resources()[id.index()].name.as_str();
        let lit = graph
            .passes()
            .iter()
            .find(|p| p.name == "decals_lit")
            .unwrap();
        assert_eq!(
            lit.reads().iter().map(name_of).collect::<Vec<_>>(),
            ["scene_depth", "scene_normal", "decal_receivers"],
        );
    }

    /// The lit pass reads the scene pass's tail, in the scene pass's order: the shadow map, then the
    /// blurred AO.
    #[test]
    fn decals_lit_reads_the_shadow_map_and_the_ao_after_its_own_inputs() {
        let chain = PostChain {
            lit_decals: true,
            shadow_map: Some(1024),
            ssao: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let name_of = |id: &ResourceId| graph.resources()[id.index()].name.as_str();
        let reads = |pass: &str| -> Vec<&str> {
            let pass = graph.passes().iter().find(|p| p.name == pass).unwrap();
            pass.reads().iter().map(name_of).collect()
        };
        let scene_reads = reads("scene");
        assert_eq!(scene_reads.len(), 2);
        let lit = reads("decals_lit");
        assert_eq!(lit[..2], ["scene_depth", "scene_normal"]);
        assert_eq!(lit[2..], scene_reads[..]);
        assert!(schedule(&graph).is_ok());
    }

    /// The lit flag is content: `masked` at low keeps it, and the sun's shadow read goes (the lit
    /// decal then shades without a shadow).
    #[test]
    fn masking_keeps_the_lit_flag_and_drops_the_shadow_read_at_low() {
        let chain = PostChain {
            lit_decals: true,
            shadow_map: Some(1024),
            ..PostChain::default()
        };
        let low = chain.masked(
            &Quality::preset("low").unwrap(),
            &QualityParams::preset("low").unwrap(),
        );
        assert!(low.lit_decals && low.shadow_map.is_none());
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), low);
        let lit = graph
            .passes()
            .iter()
            .find(|p| p.name == "decals_lit")
            .unwrap();
        assert_eq!(lit.reads().len(), 2);
    }

    /// `for_scene` derives `decals` from a multiply or add decal and `lit_decals` from an alpha or
    /// coat one, separately.
    #[test]
    fn for_scene_derives_the_two_decal_flags_separately() {
        use crate::scene::{DecalBlend, DecalReceivers};
        let mut scene = RenderScene::empty((1920, 1080));
        let mut lit = decal_instance(DecalReceivers::Exclude(Vec::new()));
        lit.blend = DecalBlend::Coat;
        scene.decals.push(lit);
        let chain = PostChain::for_scene(&scene);
        assert!(chain.lit_decals && !chain.decals);
        scene
            .decals
            .push(decal_instance(DecalReceivers::Exclude(Vec::new())));
        let chain = PostChain::for_scene(&scene);
        assert!(chain.lit_decals && chain.decals);
    }

    /// `for_scene` derives `decals` from any decal and `decal_receivers` from a non-empty list.
    #[test]
    fn for_scene_derives_the_decal_flags() {
        use crate::scene::DecalReceivers;
        let mut scene = RenderScene::empty((1920, 1080));
        scene
            .decals
            .push(decal_instance(DecalReceivers::Exclude(Vec::new())));
        let chain = PostChain::for_scene(&scene);
        assert!(chain.decals && !chain.decal_receivers);
        scene
            .decals
            .push(decal_instance(DecalReceivers::Only(vec![3])));
        let chain = PostChain::for_scene(&scene);
        assert!(chain.decals && chain.decal_receivers);
    }

    /// A scene whose camera at the origin looks down -Z, with `spot` as its only spot light.
    fn scene_with_spot(
        position: ludion_core::math::glam::Vec3,
        shadow: Option<crate::scene::LocalShadowData>,
    ) -> RenderScene {
        use crate::scene::{RenderCamera, SpotLightData, aspect_ratio};
        use ludion_core::math::glam::{Mat4, Vec3};
        let extent = (1920, 1080);
        let mut scene = RenderScene::empty(extent);
        scene.camera = RenderCamera {
            view: Mat4::look_at_rh(Vec3::ZERO, -Vec3::Z, Vec3::Y),
            projection: Mat4::perspective_rh(
                60.0_f32.to_radians(),
                aspect_ratio(extent),
                0.1,
                1000.0,
            ),
            position: Vec3::ZERO,
        };
        scene.spot_lights.push(SpotLightData {
            position,
            direction: -Vec3::Z,
            color: Vec3::ONE,
            intensity: 1.0,
            range: 3.0,
            cos_inner: 15.0_f32.to_radians().cos(),
            cos_outer: 30.0_f32.to_radians().cos(),
            shadow,
            scatter: 0.0,
        });
        scene
    }

    /// The atlas is derived only for a casting light the camera can see, and is the quality's
    /// edge once masked; the shadows toggle removes it.
    #[test]
    fn for_scene_derives_the_local_shadow_atlas_from_granted_lights() {
        use crate::scene::LocalShadowData;
        use ludion_core::math::glam::Vec3;
        let casting = Some(LocalShadowData {
            bias: 0.05,
            near: 0.1,
            distance: None,
            fade: 0.0,
        });
        let seen = Vec3::new(0.0, 0.0, -5.0);
        let granted = PostChain::for_scene(&scene_with_spot(seen, casting));
        assert_eq!(granted.local_shadow_atlas, Some(4096));
        assert_eq!(
            PostChain::for_scene(&scene_with_spot(Vec3::new(0.0, 0.0, 500.0), casting))
                .local_shadow_atlas,
            None,
            "a casting light outside the frustum is not granted",
        );
        assert_eq!(
            PostChain::for_scene(&scene_with_spot(seen, None)).local_shadow_atlas,
            None,
        );
        let low = granted.masked(&Quality::LOW, &QualityParams::LOW);
        assert_eq!(low.local_shadow_atlas, None);
        let medium = granted.masked(&Quality::MEDIUM, &QualityParams::MEDIUM);
        assert_eq!(medium.local_shadow_atlas, Some(2048));
    }

    /// `local_shadow` precedes `scene`, whose last input is the atlas.
    #[test]
    fn an_invalid_local_shadow_edge_masks_the_atlas_away() {
        let chain = PostChain {
            local_shadow_atlas: Some(QualityParams::FULL.local_shadow_atlas_size),
            local_shadow_view: true,
            ..PostChain::default()
        };
        for edge in [1000, 0] {
            let params = QualityParams {
                local_shadow_atlas_size: edge,
                ..QualityParams::FULL
            };
            let masked = chain.masked(&Quality::FULL, &params);
            assert_eq!(masked.local_shadow_atlas, None, "edge {edge}");
            assert!(!masked.local_shadow_view, "edge {edge}");
        }
    }

    #[test]
    fn the_local_shadow_pass_feeds_the_scene_pass_last() {
        let chain = PostChain {
            local_shadow_atlas: Some(256),
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        let names = pass_names(&graph);
        let pos = |n: &str| names.iter().position(|p| *p == n).unwrap();
        assert!(pos("local_shadow") < pos("scene"));
        let scene_pass = &graph.passes()[pos("scene")];
        let atlas = graph.passes()[pos("local_shadow")].depth;
        assert_eq!(scene_pass.reads.last().copied(), atlas);
        assert_eq!(scene_pass.reads.len(), 1, "no other scene input moved");
    }
    #[test]
    fn ssr_pyramids_schedule_between_reflections_and_the_blend() {
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), ssr_chain());
        let order = schedule(&graph).unwrap();
        let names = order.order_names();
        let pos = |n: &str| names.iter().position(|p| *p == n).unwrap();
        assert!(pos("scene") < pos("reflections"));
        // The colour seed sees the lit scene with the fill; the Hi-Z seed reads the depth only.
        assert!(pos("reflections") < pos("ssr_color_seed"));
        assert!(pos("hiz_seed") < pos("hiz_reduce_1"));
        assert!(pos("hiz_reduce_9") < pos("ssr_trace"));
        assert!(pos("ssr_color_reduce_9") < pos("ssr_trace"));
        assert!(pos("ssr_trace") < pos("ssr_blend"));
        assert!(pos("ssr_blend") < pos("transparents"));
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
    fn the_ssr_trace_reads_both_pyramids_and_the_blend_runs_before_transparents() {
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
        // `reflections` reads its three surface targets and never the trace; the blend reads the fill,
        // the surface targets and the trace.
        assert_eq!(
            reads("reflections"),
            ["scene_normal", "scene_specular", "scene_depth"]
        );
        assert_eq!(
            reads("ssr_blend"),
            [
                "refl_fill",
                "scene_normal",
                "scene_specular",
                "scene_depth",
                "ssr_trace"
            ]
        );
        let writes = |name: &str| {
            graph
                .passes()
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .color_targets()
                .iter()
                .map(|id| graph.resources()[id.index()].name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(writes("reflections"), ["scene", "refl_fill"]);
        assert_eq!(writes("ssr_blend"), ["scene"]);
        let order = schedule(&graph).unwrap();
        let names = order.order_names();
        let pos = |n: &str| names.iter().position(|p| *p == n).unwrap();
        assert!(pos("hiz_reduce_9") < pos("ssr_trace"));
        assert!(pos("ssr_color_reduce_9") < pos("ssr_trace"));
        assert!(pos("reflections") < pos("ssr_color_seed"));
        assert!(pos("ssr_trace") < pos("ssr_blend"));
        assert!(pos("ssr_blend") < pos("transparents"));
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

    /// A scene with probes relights them in a compute pass that `reflections` reads last; a
    /// probe-free chain declares neither the pass nor the table.
    #[test]
    fn a_probe_chain_relights_before_reflections_and_a_probe_free_one_does_not() {
        let size = FrameSize::native(1920, 1080);
        let chain = PostChain {
            probe_relight: true,
            ssr: Some(0.5),
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(size, chain);
        let relight = graph
            .passes()
            .iter()
            .find(|p| p.name == "probe_relight")
            .unwrap();
        assert!(relight.reads().is_empty());
        let table = relight.storage_target().unwrap();
        assert_eq!(graph.resources()[table.index()].name, "probe_live");
        let (reads, _) = graph_pass(&graph, "reflections").unwrap();
        assert_eq!(reads.last().map(String::as_str), Some("probe_live"));
        let order = pass_names(&graph);
        let at = |name: &str| order.iter().position(|p| *p == name).unwrap();
        assert!(at("probe_relight") < at("reflections"));
        assert!(schedule(&graph).is_ok());

        let (plain, _) = build_frame_graph(size, PostChain::default());
        assert!(graph_pass(&plain, "probe_relight").is_none());
        assert!(
            plain
                .resources()
                .iter()
                .all(|resource| resource.name != "probe_live")
        );
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

    fn shadowed_scene() -> RenderScene {
        let extent = (1920, 1080);
        let mut scene = RenderScene::empty(extent);
        scene.camera = crate::RenderCamera {
            view: ludion_core::math::glam::Mat4::look_at_rh(
                Default::default(),
                -ludion_core::math::glam::Vec3::Z,
                ludion_core::math::glam::Vec3::Y,
            ),
            projection: ludion_core::math::glam::Mat4::perspective_rh(
                1.0,
                crate::scene::aspect_ratio(extent),
                0.1,
                100.0,
            ),
            position: Default::default(),
        };
        scene.lights = vec![crate::PointLightData {
            position: ludion_core::math::glam::Vec3::new(0.0, 0.0, -5.0),
            color: ludion_core::math::glam::Vec3::ONE,
            intensity: 1.0,
            range: 3.0,
            dynamic: false,
            shadow: Some(crate::LocalShadowData {
                bias: 0.02,
                near: 0.1,
                distance: None,
                fade: 0.0,
            }),
            scatter: 0.0,
        }];
        scene
    }

    /// The atlas view replaces `composite` and reads only the atlas.
    #[test]
    fn a_local_shadow_view_replaces_composite_and_reads_the_atlas() {
        let size = FrameSize::native(1920, 1080);
        let chain = PostChain {
            local_shadow_atlas: Some(1024),
            local_shadow_view: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(size, chain);
        assert!(graph_pass(&graph, "composite").is_none());
        let (reads, _) = graph_pass(&graph, "local_shadow_debug").unwrap();
        assert_eq!(reads, ["local_shadow_atlas"]);
        assert!(schedule(&graph).is_ok());
    }

    /// The view needs a granted light, survives only while shadows do, and `view()` names it.
    #[test]
    fn the_local_shadow_view_needs_a_grant_and_a_surviving_atlas() {
        let mut scene = shadowed_scene();
        scene.debug_view = Some(crate::DebugView::LocalShadowAtlas);
        let chain = PostChain::for_scene(&scene);
        assert!(chain.local_shadow_view);
        assert_eq!(chain.view(), Some(crate::DebugView::LocalShadowAtlas));
        assert!(
            chain
                .masked(&Quality::FULL, &QualityParams::FULL)
                .local_shadow_view
        );
        assert!(
            !chain
                .masked(&Quality::LOW, &QualityParams::LOW)
                .local_shadow_view
        );

        scene.lights.clear();
        let none = PostChain::for_scene(&scene);
        assert!(!none.local_shadow_view);
        assert_eq!(none.view(), None);
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

    // --- SMAA ------------------------------------------------------------------------------

    fn smaa() -> PostChain {
        PostChain {
            aa: AntiAliasing::Smaa,
            ..PostChain::default()
        }
    }

    fn reads_of<'g>(graph: &'g RenderGraph, pass: &str) -> Vec<&'g str> {
        let pass = graph.passes().iter().find(|p| p.name == pass).unwrap();
        pass.reads()
            .iter()
            .map(|r| graph.resources()[r.index()].name.as_str())
            .collect()
    }

    fn writes_of<'g>(graph: &'g RenderGraph, pass: &str) -> Vec<&'g str> {
        let pass = graph.passes().iter().find(|p| p.name == pass).unwrap();
        pass.color_targets()
            .iter()
            .map(|r| graph.resources()[r.index()].name.as_str())
            .collect()
    }

    /// The AA-on graph is the AA-off graph of the same frame plus the three passes right after
    /// `composite` and the three transients after every other resource, so the AA-off lists the
    /// other tests pin stay prefixes. Returns the AA-on graph.
    fn assert_smaa_extends(size: FrameSize, base: PostChain) -> RenderGraph {
        let (off, _) = build_frame_graph(size, base);
        let (on, _) = build_frame_graph(
            size,
            PostChain {
                aa: AntiAliasing::Smaa,
                ..base
            },
        );
        let mut passes = pass_names(&off);
        let at = passes.iter().position(|p| *p == "composite").unwrap() + 1;
        passes.splice(at..at, ["smaa_edges", "smaa_weights", "smaa_blend"]);
        assert_eq!(pass_names(&on), passes);
        assert_eq!(schedule(&on).unwrap().order_names(), passes);
        let mut resources = resource_names(&off);
        resources.extend(["aa_input", "smaa_edges", "smaa_weights"]);
        assert_eq!(resource_names(&on), resources);

        assert_eq!(reads_of(&on, "smaa_edges"), ["aa_input"]);
        assert_eq!(reads_of(&on, "smaa_weights"), ["smaa_edges"]);
        assert_eq!(reads_of(&on, "smaa_blend"), ["aa_input", "smaa_weights"]);
        assert_eq!(writes_of(&on, "composite"), ["aa_input"]);
        assert_eq!(
            writes_of(&on, "smaa_blend"),
            writes_of(&off, "composite"),
            "the blend writes what composite would have",
        );
        let (render_w, render_h) = (size.render.0, size.render.1);
        for name in ["aa_input", "smaa_edges", "smaa_weights"] {
            let r = on.resources().iter().find(|r| r.name == name).unwrap();
            assert_eq!(
                (r.desc.extent.width, r.desc.extent.height),
                (render_w, render_h)
            );
            assert_eq!(r.desc.format, TextureDesc::hdr_color(1, 1).format, "{name}");
        }
        on
    }

    fn format_of(graph: &RenderGraph, name: &str) -> String {
        let r = graph.resources().iter().find(|r| r.name == name).unwrap();
        r.desc.format.as_str().to_owned()
    }

    /// The live pass-timing ring holds `MAX_PASSES`: the fullest graph at the largest extent (every
    /// non-debug-view feature on) must fit, or the profiler would name its passes and time none.
    #[test]
    fn the_fullest_graph_fits_the_timing_ring() {
        let fullest = PostChain {
            ui: true,
            shadow_map: Some(4096),
            sun_cascades: Some(4),
            ssao: true,
            shafts: true,
            dof: true,
            gi_inject: Some([16, 16, 16]),
            debug_lines: true,
            ssr: Some(1.0),
            aa: AntiAliasing::Smaa,
            probe_relight: true,
            drop_shadow_atlas: Some(1024),
            local_shadow_atlas: Some(2048),
            decals: true,
            lit_decals: true,
            decal_receivers: true,
            transparent_cover: true,
            fog: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(8192, 8192), fullest);
        let passes = graph.passes().len();
        assert!(passes > 32, "the test went hollow: {passes} passes");
        assert!(
            passes <= ludion_render_graph::PassTimingRing::<u64>::MAX_PASSES,
            "the fullest graph schedules {passes} passes; raise PassTimingRing::MAX_PASSES",
        );
    }

    /// Under a resample the `ldr` seam is half-float, so the output dither the resample applies has
    /// bits below 8 to land on; at native it stays 8-bit, and `ldr_resolved` always does.
    #[test]
    fn ldr_is_half_float_exactly_when_the_resample_owns_the_output() {
        let with_ui = |aa| PostChain {
            ui: true,
            aa,
            ..PostChain::default()
        };
        for aa in [AntiAliasing::Off, AntiAliasing::Smaa] {
            let native = FrameSize::native(960, 540);
            let (graph, _) = build_frame_graph(native, with_ui(aa));
            assert_eq!(format_of(&graph, "ldr"), "rgba8unorm_srgb", "native {aa:?}");
            for size in [ssaa(960, 540), sub_native(960, 540)] {
                for chain in [
                    with_ui(aa),
                    PostChain {
                        ui: false,
                        ..with_ui(aa)
                    },
                ] {
                    assert!(chain.resample_owns_output(size));
                    let (graph, _) = build_frame_graph(size, chain);
                    assert_eq!(format_of(&graph, "ldr"), "rgba16float", "{size:?} {aa:?}");
                    if chain.ui {
                        assert_eq!(format_of(&graph, "ldr_resolved"), "rgba8unorm_srgb");
                    }
                }
            }
            assert!(!with_ui(aa).resample_owns_output(native));
        }
        assert!(!PostChain::default().resample_owns_output(FrameSize::native(960, 540)));
    }

    /// A debug view has no half-float twin for its pass, so under a view the seam stays 8-bit and the
    /// resample adds nothing.
    #[test]
    fn ldr_stays_8_bit_under_a_debug_view() {
        let views = [
            PostChain {
                probe_view: true,
                ..PostChain::default()
            },
            PostChain {
                local_shadow_view: true,
                local_shadow_atlas: Some(256),
                ..PostChain::default()
            },
            PostChain {
                sun_cascade_view: true,
                ..PostChain::default()
            },
            ssr_view_chain(),
        ];
        for chain in views {
            assert!(chain.view().is_some());
            assert!(!chain.resample_owns_output(ssaa(960, 540)));
            let (graph, _) = build_frame_graph(ssaa(960, 540), chain);
            assert_eq!(format_of(&graph, "ldr"), "rgba8unorm_srgb");
        }
    }

    #[test]
    fn smaa_native_graph_blends_into_the_output() {
        let graph = assert_smaa_extends(FrameSize::native(960, 540), PostChain::default());
        assert_eq!(writes_of(&graph, "smaa_blend"), ["final"]);
    }

    #[test]
    fn smaa_with_ui_blends_into_ldr_before_the_overlay() {
        let ui = PostChain {
            ui: true,
            ..PostChain::default()
        };
        let graph = assert_smaa_extends(FrameSize::native(960, 540), ui);
        assert_eq!(writes_of(&graph, "smaa_blend"), ["ldr"]);
        assert_eq!(reads_of(&graph, "overlay"), ["ldr"]);
    }

    /// Both resample directions: the AA runs at the render extent, before the resample.
    #[test]
    fn smaa_runs_at_the_render_extent_before_the_resample() {
        let graph = assert_smaa_extends(ssaa(960, 540), PostChain::default());
        assert_eq!(writes_of(&graph, "smaa_blend"), ["ldr"]);
        assert_eq!(reads_of(&graph, "resolve"), ["ldr"]);
        let graph = assert_smaa_extends(sub_native(960, 540), PostChain::default());
        assert_eq!(writes_of(&graph, "smaa_blend"), ["ldr"]);
        assert_eq!(reads_of(&graph, "upscale"), ["ldr"]);
    }

    #[test]
    fn smaa_with_supersampling_and_ui_ends_resolve_then_overlay() {
        let ui = PostChain {
            ui: true,
            ..PostChain::default()
        };
        let graph = assert_smaa_extends(ssaa(960, 540), ui);
        let names = pass_names(&graph);
        assert_eq!(
            names[names.len() - 6..],
            [
                "composite",
                "smaa_edges",
                "smaa_weights",
                "smaa_blend",
                "resolve",
                "overlay"
            ],
        );
        assert_eq!(writes_of(&graph, "smaa_blend"), ["ldr"]);
        assert_eq!(writes_of(&graph, "resolve"), ["ldr_resolved"]);
    }

    /// A debug view stands in for the finished picture, so no AA pass is scheduled behind it.
    #[test]
    fn a_debug_view_schedules_no_smaa() {
        let views = [
            PostChain {
                ssr: Some(0.5),
                ssr_view: Some(crate::DebugView::SsrConfidence),
                ..PostChain::default()
            },
            PostChain {
                probe_view: true,
                ..PostChain::default()
            },
            PostChain {
                local_shadow_atlas: Some(512),
                local_shadow_view: true,
                ..PostChain::default()
            },
            PostChain {
                shadow_map: Some(1024),
                sun_cascades: Some(2),
                sun_cascade_view: true,
                ..PostChain::default()
            },
        ];
        for view in views {
            assert!(
                !PostChain {
                    aa: AntiAliasing::Smaa,
                    ..view
                }
                .smaa_scheduled()
            );
            let size = FrameSize::native(64, 64);
            let (off, _) = build_frame_graph(size, view);
            let (on, _) = build_frame_graph(
                size,
                PostChain {
                    aa: AntiAliasing::Smaa,
                    ..view
                },
            );
            assert_eq!(pass_names(&on), pass_names(&off), "{view:?}");
            assert_eq!(resource_names(&on), resource_names(&off), "{view:?}");
        }
        assert!(smaa().smaa_scheduled());
        assert!(!PostChain::default().smaa_scheduled());
    }

    /// No quality preset masks the AA: a machine's `low` keeps SMAA at its reduced extent.
    #[test]
    fn masking_never_changes_the_anti_aliasing() {
        for (quality, params) in [
            (Quality::FULL, QualityParams::FULL),
            (Quality::LOW, QualityParams::LOW),
        ] {
            assert_eq!(smaa().masked(&quality, &params).aa, AntiAliasing::Smaa);
        }
    }

    #[test]
    fn anti_aliasing_lists_and_names_every_mode() {
        assert_eq!(AntiAliasing::default(), AntiAliasing::Off);
        assert_eq!(AntiAliasing::ALL.map(AntiAliasing::name), ["off", "smaa"]);
        assert_eq!(
            PostChain::for_scene(&RenderScene::empty((64, 64))).aa,
            AntiAliasing::Off
        );
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
            angular_radius: 0.0,
            cascades: None,
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

    /// `medium_scatter` needs a medium **and** a selected light, and nothing else: no sun, no shaft
    /// strength. A light that is off, off-frustum or outside the medium's reach is not selected.
    #[test]
    fn for_scene_gates_medium_scatter_on_a_medium_and_a_selected_light() {
        use ludion_core::math::glam::{Mat4, Vec3};
        let light = |position: Vec3, scatter: f32| crate::scene::PointLightData {
            position,
            color: Vec3::ONE,
            intensity: 1.0,
            range: 4.0,
            dynamic: false,
            shadow: None,
            scatter,
        };
        let extent = (1920, 1080);
        let mut scene = RenderScene::empty(extent);
        scene.camera = crate::scene::RenderCamera {
            view: Mat4::look_at_rh(Vec3::ZERO, -Vec3::Z, Vec3::Y),
            projection: Mat4::perspective_rh(
                60.0_f32.to_radians(),
                crate::scene::aspect_ratio(extent),
                0.1,
                1000.0,
            ),
            position: Vec3::ZERO,
        };
        let in_view = Vec3::new(0.0, 0.0, -6.0);
        scene.lights.push(light(in_view, 1.0));
        assert!(!PostChain::for_scene(&scene).medium_scatter, "no post");
        let post = |fog_density| crate::scene::PostData {
            fog_density,
            ..crate::scene::PostData::default()
        };
        scene.post = Some(post(0.0));
        assert!(!PostChain::for_scene(&scene).medium_scatter, "no medium");
        scene.post = Some(post(0.02));
        assert!(PostChain::for_scene(&scene).medium_scatter);
        assert!(scene.sun.is_none(), "sunless still builds the term");
        scene.lights[0].scatter = 0.0;
        assert!(!PostChain::for_scene(&scene).medium_scatter, "scatter 0");
        scene.lights[0] = light(Vec3::new(0.0, 0.0, 60.0), 1.0);
        assert!(
            !PostChain::for_scene(&scene).medium_scatter,
            "behind the camera"
        );
    }

    /// `medium_gi` needs a medium **and** a `GiVolume` that authors a nonzero strength, and nothing
    /// else: no light, no sun. It schedules the pass on its own; zero strengths, no medium and the Low
    /// preset's shafts switch each leave it out.
    #[test]
    fn for_scene_gates_medium_gi_on_a_medium_and_a_nonzero_volume() {
        let mut scene = RenderScene::empty((1920, 1080));
        let gi = |sky_visibility, bounce| crate::scene::GiData {
            probe_spacing: 1.0,
            normal_bias: 0.0,
            sky_visibility,
            bounce,
            count: [2, 2, 2],
            origin: ludion_core::math::glam::Vec3::ZERO,
            span: ludion_core::math::glam::Vec3::ONE,
        };
        let fogged = Some(crate::scene::PostData {
            fog_density: 0.02,
            ..crate::scene::PostData::default()
        });
        scene.gi = Some(gi(1.0, 0.0));
        assert!(!PostChain::for_scene(&scene).medium_gi, "no post");
        scene.post = Some(crate::scene::PostData::default());
        assert!(!PostChain::for_scene(&scene).medium_gi, "no medium");
        scene.post = fogged;
        let chain = PostChain::for_scene(&scene);
        assert!(chain.medium_gi && chain.medium_scatter && !chain.local_scatter);
        scene.gi = Some(gi(0.0, 1.0));
        assert!(PostChain::for_scene(&scene).medium_gi, "bounce alone");
        scene.gi = Some(gi(0.0, 0.0));
        let off = PostChain::for_scene(&scene);
        assert!(!off.medium_gi && !off.medium_scatter, "both strengths zero");
        scene.gi = None;
        assert!(!PostChain::for_scene(&scene).medium_gi, "no volume");
        scene.gi = Some(gi(1.0, 1.0));
        let low = PostChain::for_scene(&scene).masked(&Quality::LOW, &QualityParams::LOW);
        assert!(!low.medium_gi && !low.medium_scatter, "LOW");
    }

    /// The GI pass reads the injected volume (after the atlas slot) exactly when the chain injects and
    /// the pass carries the GI term: an ordering edge after `gi_inject`.
    #[test]
    fn medium_scatter_reads_the_injected_volume_exactly_when_it_carries_gi() {
        let reads = |chain: PostChain| {
            let (graph, _) = build_frame_graph(FrameSize::native(64, 64), chain);
            graph
                .passes()
                .iter()
                .find(|p| p.name == pass::MEDIUM_SCATTER)
                .map(|p| p.reads().len())
        };
        let base = PostChain {
            medium_scatter: true,
            gi_inject: Some([2, 2, 2]),
            ..PostChain::default()
        };
        let with_gi = PostChain {
            medium_gi: true,
            ..base
        };
        assert_eq!(reads(with_gi), reads(base).map(|n| n + 1));
        let no_inject = PostChain {
            gi_inject: None,
            ..with_gi
        };
        assert_eq!(reads(no_inject), reads(base));
    }

    /// The Low preset's shafts switch drops the pair, and a machine with shadows off keeps it: the
    /// unshadowed march reads no shadow map.
    #[test]
    fn masking_low_drops_medium_scatter_and_shadows_off_keeps_it() {
        let chain = PostChain {
            medium_scatter: true,
            ..PostChain::default()
        };
        assert!(
            chain
                .masked(&Quality::FULL, &QualityParams::FULL)
                .medium_scatter
        );
        assert!(
            !chain
                .masked(&Quality::LOW, &QualityParams::LOW)
                .medium_scatter
        );
        let no_shadows = Quality {
            shadows: false,
            ..Quality::FULL
        };
        assert!(
            chain
                .masked(&no_shadows, &QualityParams::FULL)
                .medium_scatter
        );
    }

    /// With no scattering light the graph declares neither target nor pass, and `composite` reads
    /// exactly the three inputs it always did.
    #[test]
    fn medium_scatter_off_declares_no_scatter_passes_or_resources() {
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), PostChain::default());
        assert!(!resource_names(&graph).contains(&pass::MEDIUM_SCATTER));
        assert!(!resource_names(&graph).contains(&pass::MEDIUM_SCATTER_BLUR));
        assert!(!pass_names(&graph).contains(&pass::MEDIUM_SCATTER));
        assert!(!pass_names(&graph).contains(&pass::MEDIUM_SCATTER_BLUR));

        let on = PostChain {
            medium_scatter: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), on);
        let composite = graph
            .passes()
            .iter()
            .find(|p| p.name == pass::COMPOSITE)
            .unwrap();
        assert_eq!(
            composite.reads().len(),
            4,
            "the scatter term is the 4th read"
        );
        let scatter = graph
            .passes()
            .iter()
            .find(|p| p.name == pass::MEDIUM_SCATTER)
            .unwrap();
        assert_eq!(
            scatter.reads().len(),
            1,
            "scene_depth only, without a layer"
        );
    }

    /// The march reads the local-shadow atlas exactly when the chain has one (the optional slot, after
    /// `scene_depth`), and never declares it otherwise.
    #[test]
    fn medium_scatter_reads_the_local_shadow_atlas_exactly_when_present() {
        let reads = |chain: PostChain| {
            let (graph, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
            let scatter = graph
                .passes()
                .iter()
                .find(|p| p.name == pass::MEDIUM_SCATTER)
                .map(|p| p.reads().len());
            (
                scatter,
                resource_names(&graph).contains(&"local_shadow_atlas"),
            )
        };
        let chain = |atlas: Option<u32>| PostChain {
            medium_scatter: true,
            local_shadow_atlas: atlas,
            ..PostChain::default()
        };
        assert_eq!(reads(chain(None)), (Some(1), false));
        assert_eq!(reads(chain(Some(2048))), (Some(2), true));
        let atlas_only = PostChain {
            local_shadow_atlas: Some(2048),
            ..PostChain::default()
        };
        assert_eq!(reads(atlas_only), (None, true));
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

    // --- The transparent layer ----------------------------------------------------------------

    fn layer_chain() -> PostChain {
        PostChain {
            transparent_cover: true,
            fog: true,
            ..PostChain::default()
        }
    }

    fn a_sprite() -> crate::scene::SpriteInstance {
        crate::scene::SpriteInstance {
            center: glam::Vec3::ZERO,
            right: glam::Vec3::X,
            up: glam::Vec3::Y,
            color: [1.0; 4],
            texture: None,
            uv: crate::scene::SpriteInstance::FULL_UV,
            lit: false,
            layer: 0,
            additive: false,
            flash: 0.0,
            flash_color: [1.0; 3],
            uv_repeat: crate::scene::SpriteInstance::NO_TILING,
            uv_offset: [0.0; 2],
            medium: 1.0,
        }
    }

    fn a_particle() -> crate::scene::ParticleInstance {
        crate::scene::ParticleInstance {
            center: glam::Vec3::ZERO,
            right: glam::Vec3::X,
            up: glam::Vec3::Y,
            size: 1.0,
            color: [1.0; 4],
            softness: 0.0,
            additive: false,
            lit: false,
            viewmodel: false,
            medium: 1.0,
        }
    }

    fn a_text_run() -> crate::scene::TextRun {
        crate::scene::TextRun {
            position: glam::Vec3::ZERO,
            size: 1.0,
            color: [1.0; 4],
            text: "A".to_string(),
            quads: Vec::new(),
            extent: [1.0, 1.0],
            font: None,
        }
    }

    fn foggy_scene() -> RenderScene {
        let mut scene = RenderScene::empty((64, 64));
        scene.post = Some(crate::scene::PostData {
            fog_density: 0.02,
            ..crate::scene::PostData::default()
        });
        scene
    }

    /// The chain asks for the layer only for a transparent of any of the three kinds **and** a reader
    /// (fog or depth of field), and a sprite, a particle or a text run alone each count.
    #[test]
    fn for_scene_reads_the_transparent_layer_gate() {
        let bare = foggy_scene();
        assert!(PostChain::for_scene(&bare).fog);
        assert!(!PostChain::for_scene(&bare).transparent_cover);
        assert!(!PostChain::for_scene(&bare).transparent_depth_scheduled());

        let mut sprite = foggy_scene();
        sprite.sprites = vec![a_sprite()];
        let mut particle = foggy_scene();
        particle.particles = vec![a_particle()];
        let mut text = foggy_scene();
        text.texts = vec![a_text_run()];
        for scene in [&sprite, &particle, &text] {
            let chain = PostChain::for_scene(scene);
            assert!(chain.transparent_cover && chain.fog);
            assert!(chain.transparent_depth_scheduled());
        }

        let mut dry = RenderScene::empty((64, 64));
        dry.sprites = vec![a_sprite()];
        assert!(!PostChain::for_scene(&dry).fog);
        assert!(!PostChain::for_scene(&dry).transparent_depth_scheduled());
        dry.post = Some(crate::scene::PostData::default());
        assert!(
            !PostChain::for_scene(&dry).fog,
            "a PostProcess with no density is no medium"
        );
        assert!(!PostChain::for_scene(&dry).transparent_depth_scheduled());

        let mut blurred = RenderScene::empty((64, 64));
        blurred.sprites = vec![a_sprite()];
        blurred.post = Some(crate::scene::PostData {
            dof: 0.02,
            ..crate::scene::PostData::default()
        });
        let chain = PostChain::for_scene(&blurred);
        assert!(chain.dof && !chain.fog);
        assert!(
            chain.transparent_depth_scheduled(),
            "a transparent under depth of field alone schedules the layer"
        );
        blurred.sprites.clear();
        assert!(!PostChain::for_scene(&blurred).transparent_depth_scheduled());
    }

    /// A machine masking depth of field drops the layer when nothing else reads it, and keeps it
    /// when fog does.
    #[test]
    fn masking_depth_of_field_drops_the_layer_unless_fog_needs_it() {
        let no_dof = Quality {
            dof: false,
            ..Quality::default()
        };
        let blur_only = PostChain {
            transparent_cover: true,
            dof: true,
            ..PostChain::default()
        };
        assert!(blur_only.transparent_depth_scheduled());
        assert!(
            !blur_only
                .masked(&no_dof, &QualityParams::FULL)
                .transparent_depth_scheduled()
        );
        let both = PostChain {
            fog: true,
            ..blur_only
        };
        assert!(
            both.masked(&no_dof, &QualityParams::FULL)
                .transparent_depth_scheduled()
        );
    }

    /// With depth of field the prefilter reads the layer last, behind the scene and its depth, and
    /// the pass runs after `transparents` and before `dof_coc`.
    #[test]
    fn the_dof_prefilter_reads_the_layer_last() {
        let chain = PostChain {
            transparent_cover: true,
            dof: true,
            ..PostChain::default()
        };
        let (graph, _) = build_frame_graph(FrameSize::native(64, 64), chain);
        let names = pass_names(&graph);
        assert!(names.contains(&pass::TRANSPARENT_DEPTH));
        let reads = graph
            .passes()
            .iter()
            .find(|p| p.name == pass::DOF_COC)
            .expect("dof_coc")
            .reads
            .len();
        assert_eq!(reads, 3, "scene, scene_depth, then the layer");
        let sched = schedule(&graph).expect("schedules");
        let order = sched.order_names();
        let at = |n: &str| order.iter().position(|&p| p == n).unwrap();
        assert!(at(pass::TRANSPARENTS) < at(pass::TRANSPARENT_DEPTH));
        assert!(at(pass::TRANSPARENT_DEPTH) < at(pass::DOF_COC));
    }

    /// With shafts the march reads the layer last, behind the scene depth and the shadow map, and
    /// the layer's pass runs before it. Without a transparent the reads are the two they always were.
    #[test]
    fn the_shaft_march_reads_the_layer_last() {
        let shafts_chain = |cover| PostChain {
            shadow_map: Some(2048),
            shafts: true,
            fog: true,
            transparent_cover: cover,
            ..PostChain::default()
        };
        let reads = |graph: &RenderGraph| {
            graph
                .passes()
                .iter()
                .find(|p| p.name == pass::SHAFTS)
                .expect("shafts")
                .reads
                .len()
        };

        let (with, _) = build_frame_graph(FrameSize::native(64, 64), shafts_chain(true));
        assert!(pass_names(&with).contains(&pass::TRANSPARENT_DEPTH));
        assert_eq!(reads(&with), 3, "scene_depth, shadow_map, then the layer");
        let shafts = with
            .passes()
            .iter()
            .find(|p| p.name == pass::SHAFTS)
            .expect("shafts");
        let last = shafts.reads.last().expect("reads");
        assert_eq!(with.resources()[last.index()].name, pass::TRANSPARENT_DEPTH);
        let scheduled = schedule(&with).expect("schedules");
        let order = scheduled.order_names();
        let at = |n: &str| order.iter().position(|&p| p == n).unwrap();
        assert!(at(pass::TRANSPARENT_DEPTH) < at(pass::SHAFTS));

        let (without, _) = build_frame_graph(FrameSize::native(64, 64), shafts_chain(false));
        assert!(!pass_names(&without).contains(&pass::TRANSPARENT_DEPTH));
        assert_eq!(reads(&without), 2, "scene_depth and shadow_map only");
    }

    /// A machine's mask never removes content: both flags survive `masked`.
    #[test]
    fn masking_keeps_the_transparent_layer_flags() {
        let masked = layer_chain().masked(&Quality::default(), &QualityParams::FULL);
        assert!(masked.transparent_cover && masked.fog);
    }

    /// A debug view stands in for the finished picture, so it drops the layer.
    #[test]
    fn a_debug_view_drops_the_transparent_layer() {
        let viewing = PostChain {
            probe_view: true,
            ..layer_chain()
        };
        assert!(!viewing.transparent_depth_scheduled());
        let (graph, _) = build_frame_graph(FrameSize::native(64, 64), viewing);
        assert!(!pass_names(&graph).contains(&pass::TRANSPARENT_DEPTH));
        assert!(!resource_names(&graph).contains(&pass::TRANSPARENT_DEPTH));
    }

    /// Fog without a transparent, or a transparent without fog, schedules neither the pass nor its
    /// target: the graph is exactly the layer-free one.
    #[test]
    fn the_transparent_layer_needs_both_halves() {
        let baseline = build_frame_graph(FrameSize::native(64, 64), PostChain::default()).0;
        for chain in [
            PostChain {
                fog: true,
                ..PostChain::default()
            },
            PostChain {
                transparent_cover: true,
                ..PostChain::default()
            },
        ] {
            let (graph, _) = build_frame_graph(FrameSize::native(64, 64), chain);
            assert_eq!(pass_names(&graph), pass_names(&baseline));
            assert_eq!(resource_names(&graph), resource_names(&baseline));
        }
    }

    /// With both halves the pass and the `Rg16Float` target exist at the render extent, the pass
    /// draws after `transparents` over the opaque depth and before `composite`, and `composite`
    /// reads exactly what it did without the layer, plus the layer last.
    #[test]
    fn the_transparent_layer_pass_follows_the_transparents() {
        let size = FrameSize::scaled(64, 64, crate::RenderScale::new(2.0).unwrap());
        let (without, _) = build_frame_graph(size, PostChain::default());
        let (graph, _) = build_frame_graph(size, layer_chain());
        let layer = graph
            .resources()
            .iter()
            .find(|r| r.name == pass::TRANSPARENT_DEPTH)
            .expect("the target is declared");
        assert_eq!(layer.desc.format.as_str(), "rg16float");
        assert_eq!(
            (layer.desc.extent.width, layer.desc.extent.height),
            size.render
        );

        let find = |name: &str| graph.passes().iter().find(|p| p.name == name).unwrap();
        let pass = find(pass::TRANSPARENT_DEPTH);
        assert_eq!(pass.color_targets().len(), 1);
        assert_eq!(pass.reads().len(), 0);
        assert_eq!(
            graph.resources()[pass.depth_target().unwrap().index()].name,
            "scene_depth"
        );

        let scheduled = schedule(&graph).unwrap();
        let order = scheduled.order_names();
        let at = |name: &str| order.iter().position(|&n| n == name).unwrap();
        assert!(at(pass::TRANSPARENTS) < at(pass::TRANSPARENT_DEPTH));
        assert!(at(pass::TRANSPARENT_DEPTH) < at(pass::COMPOSITE));

        let composite_reads = |graph: &RenderGraph| {
            graph
                .passes()
                .iter()
                .find(|p| p.name == pass::COMPOSITE)
                .unwrap()
                .reads()
                .iter()
                .map(|r| graph.resources()[r.index()].name.clone())
                .collect::<Vec<_>>()
        };
        let mut expected = composite_reads(&without);
        expected.push(pass::TRANSPARENT_DEPTH.to_string());
        assert_eq!(composite_reads(&graph), expected);
        let (plain, _) = build_frame_graph(size, PostChain::default());
        assert_eq!(composite_reads(&plain), composite_reads(&without));
        assert!(!composite_reads(&plain).contains(&pass::TRANSPARENT_DEPTH.to_string()));
    }

    /// The dump names the pass and its format, which is how an agent sees the layer is scheduled.
    #[test]
    fn dump_frame_graph_json_names_the_transparent_layer() {
        let json = dump_frame_graph_json(FrameSize::native(64, 64), layer_chain()).unwrap();
        assert!(json.contains("\"transparent_depth\""));
        assert!(json.contains("rg16float"));
        let plain = dump_frame_graph_json(FrameSize::native(64, 64), PostChain::default()).unwrap();
        assert!(!plain.contains("transparent_depth") && !plain.contains("rg16float"));
    }

    /// `mesh_medium` needs the medium **and** a mesh scaling it below 1; either alone derives `false`.
    #[test]
    fn for_scene_reads_the_mesh_medium_gate() {
        let fog = crate::scene::PostData {
            fog_density: 0.05,
            ..crate::scene::PostData::default()
        };
        let mut scene = RenderScene::empty((64, 64));
        scene.meshes = vec![cull_mesh(false), cull_mesh(false)];
        scene.meshes[1].medium = 0.0;
        assert!(!PostChain::for_scene(&scene).mesh_medium, "no post");
        scene.post = Some(crate::scene::PostData::default());
        assert!(!PostChain::for_scene(&scene).mesh_medium, "no fog");
        scene.post = Some(fog);
        assert!(PostChain::for_scene(&scene).mesh_medium);
        scene.meshes[1].medium = 1.0;
        assert!(!PostChain::for_scene(&scene).mesh_medium, "every mesh at 1");
    }

    /// Off, the graph has no `scene_medium`, the scene pass writes three targets and `composite` reads
    /// what it did. On, the mask is the scene pass's fourth write and `composite`'s last read; a debug
    /// view that replaces the composite does not schedule it.
    #[test]
    fn the_scene_medium_mask_is_the_fourth_scene_write_and_the_last_composite_read() {
        let size = FrameSize::native(64, 64);
        let names = |graph: &RenderGraph, pass_name: &str, writes: bool| {
            let pass = graph.passes().iter().find(|p| p.name == pass_name).unwrap();
            let ids: Vec<_> = if writes {
                pass.color_targets().to_vec()
            } else {
                pass.reads().to_vec()
            };
            ids.iter()
                .map(|r| graph.resources()[r.index()].name.clone())
                .collect::<Vec<_>>()
        };
        let (off, _) = build_frame_graph(size, PostChain::default());
        assert!(!off.resources().iter().any(|r| r.name == "scene_medium"));
        assert_eq!(names(&off, pass::SCENE, true).len(), 3);

        let on_chain = PostChain {
            fog: true,
            mesh_medium: true,
            ..PostChain::default()
        };
        let (on, _) = build_frame_graph(size, on_chain);
        let mask = on
            .resources()
            .iter()
            .find(|r| r.name == "scene_medium")
            .expect("the mask is declared");
        assert_eq!(mask.desc.format.as_str(), "r8unorm");
        let writes = names(&on, pass::SCENE, true);
        assert_eq!(writes.len(), 4);
        assert_eq!(writes[3], "scene_medium");
        let mut expected = names(&off, pass::COMPOSITE, false);
        expected.push("scene_medium".to_string());
        assert_eq!(names(&on, pass::COMPOSITE, false), expected);

        let viewed = PostChain {
            probe_view: true,
            ..on_chain
        };
        assert!(!viewed.mesh_medium_scheduled());
        let (graph, _) = build_frame_graph(size, viewed);
        assert!(!graph.resources().iter().any(|r| r.name == "scene_medium"));
    }

    /// `transparent_medium` needs the medium **and** a sprite or world-space particle scaling it below
    /// 1; a viewmodel particle never reaches the layer. The layer is `Rgba16Float` exactly when it is
    /// scheduled, and a debug view drops it.
    #[test]
    fn the_transparent_layer_gains_a_blue_channel_only_for_a_scaled_transparent() {
        let fog = crate::scene::PostData {
            fog_density: 0.05,
            ..crate::scene::PostData::default()
        };
        let mut scene = RenderScene::empty((64, 64));
        scene.sprites = vec![a_sprite()];
        scene.particles = vec![a_particle()];
        assert!(!PostChain::for_scene(&scene).transparent_medium, "no post");
        scene.post = Some(crate::scene::PostData::default());
        scene.sprites[0].medium = 0.0;
        assert!(!PostChain::for_scene(&scene).transparent_medium, "no fog");
        scene.post = Some(fog);
        let chain = PostChain::for_scene(&scene);
        assert!(chain.transparent_medium && chain.transparent_medium_scheduled());
        scene.sprites[0].medium = 1.0;
        assert!(!PostChain::for_scene(&scene).transparent_medium, "all at 1");
        scene.particles[0].medium = 0.5;
        scene.particles[0].viewmodel = true;
        assert!(
            !PostChain::for_scene(&scene).transparent_medium,
            "viewmodel only"
        );
        scene.particles[0].viewmodel = false;
        let chain = PostChain::for_scene(&scene);
        assert!(chain.transparent_medium);

        let layer_format = |chain: PostChain| {
            let (graph, _) = build_frame_graph(FrameSize::native(64, 64), chain);
            graph
                .resources()
                .iter()
                .find(|r| r.name == pass::TRANSPARENT_DEPTH)
                .map(|r| r.desc.format.as_str().to_string())
        };
        assert_eq!(layer_format(chain).as_deref(), Some("rgba16float"));
        let plain = PostChain {
            transparent_medium: false,
            ..chain
        };
        assert_eq!(layer_format(plain).as_deref(), Some("rg16float"));
        let viewed = PostChain {
            probe_view: true,
            ..chain
        };
        assert!(!viewed.transparent_medium_scheduled());
        assert_eq!(layer_format(viewed), None);
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

    /// A rain-free frame declares neither the occlusion pass nor its map; a rain frame adds both, runs
    /// the pass ahead of `scene`, and the map is the last read of `scene`, `transparents` and
    /// `transparent_depth`.
    #[test]
    fn rain_graph_is_conditional_and_ends_both_transparent_reads_with_the_map() {
        let reads = |graph: &RenderGraph, name: &str| {
            let pass = graph.passes().iter().find(|p| p.name == name).unwrap();
            pass.reads()
                .iter()
                .map(|id| graph.resources()[id.index()].name.to_string())
                .collect::<Vec<_>>()
        };
        let size = FrameSize::native(1920, 1080);
        let off_chain = PostChain {
            fog: true,
            transparent_cover: true,
            ..PostChain::default()
        };
        let (off, _) = build_frame_graph(size, off_chain);
        assert!(!pass_names(&off).contains(&pass::RAIN_OCCLUSION));
        assert!(!resource_names(&off).contains(&pass::RAIN_OCCLUSION));

        let (on, _) = build_frame_graph(
            size,
            PostChain {
                rain: true,
                ..off_chain
            },
        );
        let names = pass_names(&on);
        let at = |name: &str| names.iter().position(|n| *n == name).unwrap();
        assert!(at(pass::RAIN_OCCLUSION) < at(pass::SCENE));
        assert!(at(pass::RAIN_OCCLUSION) < at(pass::TRANSPARENTS));
        assert!(at(pass::RAIN_OCCLUSION) < at(pass::TRANSPARENT_DEPTH));
        assert_eq!(resource_names(&on).len(), resource_names(&off).len() + 1);
        assert_eq!(names.len(), pass_names(&off).len() + 1);
        for name in [pass::SCENE, pass::TRANSPARENTS, pass::TRANSPARENT_DEPTH] {
            assert_eq!(
                reads(&on, name).last().map(String::as_str),
                Some(pass::RAIN_OCCLUSION),
                "{name}"
            );
            assert_eq!(
                reads(&on, name).len(),
                reads(&off, name).len() + 1,
                "{name}"
            );
        }
    }

    /// A frame with no silhouette declares neither the mask pass nor its two transients, and
    /// `composite` reads exactly what it did before; one with a silhouette adds the pass and ends
    /// `composite`'s reads with the mask then its depth.
    #[test]
    fn silhouette_graph_is_conditional_and_ends_composite_reads_with_the_mask() {
        let composite_reads = |graph: &RenderGraph| {
            graph
                .passes()
                .iter()
                .find(|p| p.name == pass::COMPOSITE)
                .unwrap()
                .reads()
                .len()
        };
        let (off, _) = build_frame_graph(FrameSize::native(1920, 1080), PostChain::default());
        assert!(!pass_names(&off).contains(&pass::SILHOUETTE_MASK));
        assert!(!resource_names(&off).contains(&pass::SILHOUETTE_MASK));
        assert!(!resource_names(&off).contains(&"silhouette_depth"));

        let chain = PostChain {
            silhouette: true,
            ..PostChain::default()
        };
        let (on, _) = build_frame_graph(FrameSize::native(1920, 1080), chain);
        assert!(pass_names(&on).contains(&pass::SILHOUETTE_MASK));
        let resources = resource_names(&on);
        let mask = resources
            .iter()
            .position(|n| *n == pass::SILHOUETTE_MASK)
            .expect("the mask transient is declared");
        assert_eq!(resources[mask + 1], "silhouette_depth");
        assert_eq!(resources.len(), resource_names(&off).len() + 2);
        assert_eq!(pass_names(&on).len(), pass_names(&off).len() + 1);
        assert_eq!(composite_reads(&on), composite_reads(&off) + 2);
        let at = |name: &str| pass_names(&on).iter().position(|n| *n == name).unwrap();
        assert!(at(pass::SILHOUETTE_MASK) < at(pass::COMPOSITE));
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
