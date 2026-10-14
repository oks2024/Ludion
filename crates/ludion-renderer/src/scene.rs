//! [`RenderScene`] — the plain-data handoff from the world to the renderer.
//!
//! This is the *only* thing the renderer consumes. It holds no ECS references and no asset paths:
//! transforms are baked into model matrices, textures are decoded RGBA8 images referenced by index,
//! and the camera is resolved to view/projection matrices. `ludion-engine`'s `extract()` produces
//! it; the renderer uploads it and draws it. Keeping it plain data is what lets the renderer depend
//! on neither `ludion-ecs` nor `ludion-scene` (boundary rule, `engine-architecture.md` §2).
//!
//! Every field derives `PartialEq`, so two extractions of the same world can be asserted equal — the
//! extract-determinism test.

use std::sync::Arc;

use ludion_core::math::glam::{Mat3, Mat4, Quat, Vec3};
use ludion_font::GlyphQuad;

/// The engine's historical flat-ambient term (linear RGB) — the small constant light added to every
/// lit surface when a scene declares no [`EnvironmentData`]. One source of truth for both the
/// extract default (`ludion-engine`'s former private `AMBIENT` constant) and [`RenderScene::empty`],
/// unifying the two defaults that once disagreed (`[0.05,0.05,0.06]` vs `[0.03,0.03,0.04]`). Every
/// committed golden renders through `extract`, which already used this value, so unifying `empty`
/// onto it changes no golden — only the renderer's own no-camera default framing moves onto the
/// same constant. An `Environment`'s authored `ambient` supersedes this when present.
pub const DEFAULT_AMBIENT: [f32; 3] = [0.05, 0.05, 0.06];

/// A resolved camera: the matrices and position the renderer needs, with no projection enum left to
/// interpret.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RenderCamera {
    /// World→view matrix (built from the camera entity's `Transform`).
    pub view: Mat4,
    /// View→clip matrix (built from the `Camera` component and the viewport aspect ratio).
    pub projection: Mat4,
    /// The camera's world-space position (for light attenuation and any view-dependent term).
    pub position: Vec3,
}

/// What geometry a [`MeshInstance`] draws: one of the three procedural low-poly primitives, or an
/// asset mesh referenced by index into [`RenderScene::mesh_assets`]. Faceted shading gives the HD-2D
/// look for both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MeshShape {
    /// A unit cube centered at the origin.
    Cube,
    /// A unit plane in the XZ plane (normal +Y), centered at the origin.
    Plane,
    /// A unit-radius icosphere-ish UV sphere centered at the origin.
    Sphere,
    /// An asset mesh: an index into [`RenderScene::mesh_assets`]. Drawn **indexed**, unlike the
    /// non-indexed primitives, through the same lit pipeline.
    Asset(usize),
}

/// A single low-poly mesh to draw: its shape, world transform, flat color, and material.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MeshInstance {
    /// Which procedural primitive to draw.
    pub shape: MeshShape,
    /// Local→world transform.
    pub model: Mat4,
    /// The normal matrix: transforms object-space normals to world space. For a uniform scale this
    /// is the [`model`](MeshInstance::model)'s upper-3×3 verbatim; for a non-uniform scale it is the
    /// inverse-transpose (built cofactor-style so it stays finite under a zero-scaled axis). Resolved
    /// at extract time — the shader normalizes the interpolated result, so only its direction matters.
    pub normal_matrix: Mat3,
    /// Flat RGBA color (linear).
    pub color: [f32; 4],
    /// Per-instance material `(roughness, metallic, emissive)` from the `Mesh` component. Selected
    /// only when [`use_vertex_material`](MeshInstance::use_vertex_material) is false — a mesh that
    /// authors its own per-vertex material ignores it.
    pub material: [f32; 3],
    /// Whether the referenced geometry carries its own per-vertex material (a voxel palette or
    /// mesh-doc lists). True selects the vertex material; false selects the per-instance `material`.
    pub use_vertex_material: bool,
    /// Per-instance screen-door dither fade amount in `[0, 1]` (`1.0` = fully visible), from the
    /// entity's `Fade` component (default `1.0` when absent). Consumed by the mesh shader's
    /// Bayer-dither discard; the default can never discard, so it is render-identical by construction.
    pub fade: f32,
    /// World-space height (Y) threshold for [`fade`](MeshInstance::fade): only fragments above it fade
    /// (default `-f32::MAX` — every fragment eligible).
    pub fade_height_min: f32,
    /// Hit-flash amount in `[0, 1]` (default `0.0`): the shaded colour lerps toward
    /// [`flash_color`](MeshInstance::flash_color) by this much. The default can never change a pixel,
    /// so a flash-free frame is render-identical by construction.
    pub flash: f32,
    /// The colour a hit-flash lerps toward (linear RGB, HDR-capable; default white).
    pub flash_color: [f32; 3],
    /// Index into the renderer's uploaded bone palette (`[identity] ++ `[`RenderScene::bone_palette`]):
    /// `0` selects the identity entry, meaning this instance is unskinned, so scene entries start at
    /// `1`. Rides the vertex shader as `fade.z`; an unskinned instance's shader branch never reads the
    /// palette, so `0` is render-identical to today's unskinned draw.
    pub bone_offset: u32,
}

/// A single sprite/billboard: a textured, tinted quad in world space.
///
/// The quad's corners are `center ± right/2 ± up/2` — `right`/`up` are the quad's **full**
/// width/height vectors, so the drawn quad is exactly `|right|` × `|up|` world units. For a
/// billboard, their directions are the camera's axes (so it faces the camera); for a non-billboard
/// sprite they come from the entity's `Transform`. Billboarding is therefore fully resolved at
/// extract time — the shader is identical for both.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SpriteInstance {
    /// World-space center of the quad.
    pub center: Vec3,
    /// Full-width vector along the quad's local X (world space).
    pub right: Vec3,
    /// Full-height vector along the quad's local Y (world space).
    pub up: Vec3,
    /// RGBA tint multiplied into the sampled texel (linear, straight alpha — the texel it multiplies
    /// is premultiplied).
    pub color: [f32; 4],
    /// Index into [`RenderScene::textures`], or `None` for an untextured (white) sprite.
    pub texture: Option<usize>,
    /// The sub-rectangle of the texture this quad samples, `[u0, v0, u1, v1]` in the top-left-origin
    /// convention of the source PNG. `[0, 0, 1, 1]` is the whole image; one region of an atlas is a
    /// sub-rect, which is what lets many sprites of one atlas batch into a single draw.
    pub uv: [f32; 4],
    /// Whether the quad takes light: `true` is the clustered-forward two-sided shading, `false` is
    /// texel × tint with no light lookup. A per-instance flag, not a second pipeline, so it never
    /// splits a batch.
    pub lit: bool,
    /// The sort key: sprites draw in `(layer, view depth back-to-front, scene order)` order, `layer`
    /// the primary key. Batches split on a `(layer run, texture)` change.
    pub layer: i16,
    /// Additive compositing: the shader withholds the output alpha, so under `One, OneMinusSrcAlpha`
    /// the `over` blend becomes a plain add. A per-instance flag, never a batch split.
    pub additive: bool,
    /// Hit-flash amount in `[0, 1]` (default `0.0`): the final colour lerps toward
    /// [`flash_color`](SpriteInstance::flash_color) by this much. `0.0` is render-identical.
    pub flash: f32,
    /// The colour a hit-flash lerps toward (linear RGB, HDR-capable; default white).
    pub flash_color: [f32; 3],
    /// How many times [`uv`](SpriteInstance::uv) tiles across the quad along its right/up axes
    /// (default `[1, 1]`). Anything but the default, or a non-zero
    /// [`uv_offset`](SpriteInstance::uv_offset), switches the shader to its wrapping path.
    pub uv_repeat: [f32; 2],
    /// The wrapped scroll offset in frame widths (default `[0, 0]`), already reduced to `[0, 1)` —
    /// `ludion-engine`'s extract evaluates the sprite's scroll rate against sim time, so the
    /// renderer never reads a clock.
    pub uv_offset: [f32; 2],
}

impl SpriteInstance {
    /// The whole texture, in the top-left-origin `[u0, v0, u1, v1]` convention: what a quad sampling
    /// a loose PNG rather than one region of an atlas draws.
    pub const FULL_UV: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

    /// One copy of the frame across the quad — the [`uv_repeat`](SpriteInstance::uv_repeat) that,
    /// paired with a zero [`uv_offset`](SpriteInstance::uv_offset), keeps the shader on its
    /// clamping path.
    pub const NO_TILING: [f32; 2] = [1.0, 1.0];
}

/// One camera-facing particle disc. `ludion-engine`'s extract emits these in
/// `(ascending EntityId, particle index)` order; the renderer sorts a copy back-to-front.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ParticleInstance {
    /// World-space centre of the disc.
    pub center: Vec3,
    /// Unit camera-basis right vector (world space), the same basis the sprite path billboards with.
    pub right: Vec3,
    /// Unit camera-basis up vector (world space).
    pub up: Vec3,
    /// Disc diameter in world units.
    pub size: f32,
    /// Linear RGBA, HDR-capable, straight alpha; the shader premultiplies.
    pub color: [f32; 4],
    /// 0 = a hard anti-aliased edge, 1 = a gaussian falloff.
    pub softness: f32,
    /// Additive: the shader forces the output alpha to zero under the premultiplied blend.
    pub additive: bool,
}

/// The world-space Y offset every ground marker is lifted by, so a marker lying on a floor never
/// z-fights the geometry it marks.
///
/// Fixed and non-authorable on purpose: a coplanar quad has exactly one right answer, and letting
/// each scene pick an offset would replicate the same decision per game. `ludion-engine`'s extract
/// bakes it into the marker's placement, so [`GroundMarkerInstance::model`] arrives already lifted
/// and the renderer applies no offset of its own.
pub const GROUND_MARKER_LIFT: f32 = 0.01;

/// The footprint a [`GroundMarkerInstance`] draws, in **final world units** — the authored maximum
/// already multiplied by the entity's uniform scale, so the renderer does no sizing arithmetic.
///
/// Both variants are evaluated by one analytic signed-distance function in `ground_marker.wgsl`
/// (a rounded box), so a variant is a different `(half-extent, corner radius)` pair rather than a
/// different shader branch.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GroundMarkerShape {
    /// A filled circle of `radius` world units, centred on the instance's origin — an area-of-effect
    /// or landing-point telegraph.
    Disc {
        /// The circle's radius in world units (`> 0`).
        radius: f32,
    },
    /// A filled capsule along the instance's local **+X** axis — a charge/line telegraph. The drawn
    /// footprint is exactly `length` × `width` world units: the capsule's straight section is
    /// `length - width` long and its round caps have radius `width / 2` (a `width` at or above
    /// `length` degenerates to a disc of radius `length / 2`).
    Segment {
        /// Total length along local +X, in world units (`> 0`).
        length: f32,
        /// Total width across local +Z, in world units (`> 0`).
        width: f32,
    },
}

impl GroundMarkerShape {
    /// The shape as the rounded-box SDF's two parameters: the box's half-extent in the marker's
    /// local XZ plane and its corner radius. One packing for both variants — this is what lets the
    /// shader carry a single branch-free distance function.
    ///
    /// # Example
    /// ```
    /// use ludion_renderer::GroundMarkerShape;
    ///
    /// // A disc is a zero-extent box with the radius in the corner.
    /// assert_eq!(
    ///     GroundMarkerShape::Disc { radius: 3.0 }.rounded_box(),
    ///     ([0.0, 0.0], 3.0),
    /// );
    /// // A segment's straight section shortens by the cap radius, so the drawn box is 8 x 2.
    /// assert_eq!(
    ///     GroundMarkerShape::Segment { length: 8.0, width: 2.0 }.rounded_box(),
    ///     ([3.0, 0.0], 1.0),
    /// );
    /// ```
    pub fn rounded_box(self) -> ([f32; 2], f32) {
        match self {
            GroundMarkerShape::Disc { radius } => ([0.0, 0.0], radius),
            GroundMarkerShape::Segment { length, width } => {
                let radius = (width * 0.5).min(length * 0.5);
                ([(length * 0.5 - radius).max(0.0), 0.0], radius)
            }
        }
    }
}

/// A flat, ground-anchored telegraph marker: an analytic disc or capsule drawn in the marker's own
/// XZ plane, alpha-blended over the scene and depth-tested against it.
///
/// `ludion-engine`'s extract resolves a `GroundMarker` component plus its `Transform` into this,
/// which is what the renderer never has to interpret:
/// - [`shape`](GroundMarkerInstance::shape) carries **final world-space** dimensions — the authored
///   maximum already multiplied by the entity's uniform `Transform.scale.x`, so a telegraph filling
///   in as its warmup counts down is a per-frame scale write with no shape re-authoring.
/// - [`model`](GroundMarkerInstance::model) is a **rigid** local→world transform: rotation and
///   translation only, with [`GROUND_MARKER_LIFT`] already added to its translation's Y. The scale
///   is deliberately *not* in it — it lives in the dimensions above, because the signed-distance
///   evaluation has to happen in a metric space.
/// - The marker's local plane is XZ with local **+X** the segment's length axis, matching the
///   `Plane` primitive's orientation (normal +Y).
/// - [`color`](GroundMarkerInstance::color) is linear RGBA and HDR-capable: a value above `1.0`
///   rides the bloom chain exactly like an emissive mesh, which is how urgency reads at a glance.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GroundMarkerInstance {
    /// The marker's footprint, in final world units.
    pub shape: GroundMarkerShape,
    /// Rigid local→world transform (rotation + lifted translation, no scale).
    pub model: Mat4,
    /// Linear RGBA, HDR-capable. Alpha multiplies the analytic coverage, so `a` is the marker's
    /// overall opacity.
    pub color: [f32; 4],
}

/// One world-space debug line: plain data for the renderer's debug pass.
///
/// `ludion-engine` fills [`RenderScene::debug_lines`] from the categories the host enabled; the
/// default is none, and an empty list changes no frame. [`color`](DebugSegment::color) is linear RGBA.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DebugSegment {
    /// One end, world space.
    pub a: [f32; 3],
    /// The other end, world space.
    pub b: [f32; 3],
    /// Linear RGBA.
    pub color: [f32; 4],
}

/// A directional "sun" light with one orthographic shadow map — plain data for the renderer.
///
/// The mesh pass's sun term and the depth-only shadow pass consume it; a `None`
/// [`RenderScene::sun`] uploads a zeroed uniform, which is zero pixel change by construction.
/// `ludion-engine`'s `extract()` resolves the `DirectionalLight` component's explicit direction,
/// premultiplied color, and shadow tunables into these baked values — the renderer never sees the
/// raw component (boundary rule).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SunData {
    /// Normalized direction the light travels (world space).
    pub direction: Vec3,
    /// Linear RGB color, premultiplied by the light's intensity (the [`PointLightData`] convention,
    /// folded to one value since a directional light has no attenuation to re-scale).
    pub color: Vec3,
    /// World→light-clip matrix (the orthographic view-projection the shadow map is rendered and
    /// sampled with), in the wgpu 0..1 depth convention — the same `orthographic_rh` convention the
    /// camera projection uses.
    pub light_space: Mat4,
    /// Depth bias applied when sampling the shadow map, to fight acne: light-NDC depth at a 2048²
    /// map. `extract` converts the authored world-unit bias.
    pub bias: f32,
}

/// The scene's sky and lighting environment — plain data for the renderer, resolved from the
/// lowest-`EntityId` `Environment` component (`None` for a scene with no `Environment`).
///
/// The sky triangle and the prefiltered IBL read it; a `None` [`RenderScene::environment`] keeps the
/// engine's historical ambient/clear pipeline byte-for-byte. `ludion-engine`'s `extract()` copies the
/// component's authored fields into these baked values; the renderer never sees the raw component
/// (boundary rule). Imaging — what the post stack does to the rendered image — is [`PostData`]'s.
///
/// Colors are linear RGB, HDR-capable (may exceed 1.0). The sun/moon disc's *direction* is not here —
/// the renderer reads it from [`RenderScene::sun`] at draw time (no double-count with the analytic
/// sun; the disc is background-only, excluded from the IBL prefilter).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EnvironmentData {
    /// Sky gradient color at the zenith (straight up), linear RGB (HDR).
    pub sky_zenith: [f32; 3],
    /// Sky gradient color at the horizon, linear RGB (HDR).
    pub sky_horizon: [f32; 3],
    /// Sky gradient color at the ground hemisphere (straight down), linear RGB (HDR).
    pub sky_ground: [f32; 3],
    /// Angular diameter of the sun/moon disc, in degrees (`0.0` = no disc).
    pub sun_disc_size: f32,
    /// Brightness of the sun/moon disc, linear (HDR). Drawn only when `sun_disc_size > 0`.
    pub sun_disc_intensity: f32,
    /// Flat ambient term, linear RGB. `extract()` copies it into [`RenderScene::ambient`], which is
    /// what the shader actually reads (an environment-free scene falls back to the historical
    /// constant).
    pub ambient: [f32; 3],
    /// Overall image-based-lighting intensity multiplier.
    pub ibl_intensity: f32,
}

/// The scene's post-process (imaging) settings — plain data for the renderer, resolved from the
/// lowest-`EntityId` `PostProcess` component (`None` for a scene with no `PostProcess`).
///
/// A `None` [`RenderScene::post`] makes the renderer fall back to [`PostData::default`], so an
/// unauthored scene renders through the engine's shipped imaging. `ludion-engine`'s `extract()`
/// copies the component's authored fields into these baked values; the renderer never sees the raw
/// component (boundary rule).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PostData {
    /// Bloom threshold (luminance above which a pixel blooms).
    pub bloom_threshold: f32,
    /// Bloom strength (how strongly bloom is mixed back in).
    pub bloom_strength: f32,
    /// Linear exposure multiplier applied to scene + bloom immediately before the tonemap operator.
    pub exposure: f32,
    /// Which tonemap operator the composite pass runs.
    pub tonemap: TonemapOperator,
    /// Whether the composite pass adds ordered output dither.
    pub dither: bool,
    /// Grade: contrast about a `0.5` pivot (`1.0` = no change).
    pub contrast: f32,
    /// Grade: chroma mix toward Rec.709 luma (`1.0` = no change, `0.0` = greyscale).
    pub saturation: f32,
    /// Grade: white balance along the blue↔amber axis, `[-1, 1]` (`0.0` = no change).
    pub temperature: f32,
    /// Grade: white balance along the magenta↔green axis, `[-1, 1]` (`0.0` = no change).
    pub tint: f32,
    /// Grade: the ASC CDL offset, per channel (`[0, 0, 0]` = no change).
    pub lift: [f32; 3],
    /// Grade: the ASC CDL power, applied as its reciprocal, per channel (`[1, 1, 1]` = no change).
    pub gamma: [f32; 3],
    /// Grade: the ASC CDL slope, per channel (`[1, 1, 1]` = no change).
    pub gain: [f32; 3],
    /// Fog: extinction per world unit (`0.0` = no fog, and the gate for the whole fog step).
    pub fog_density: f32,
    /// Fog: what the fog blends toward, linear RGB (HDR-capable).
    pub fog_color: [f32; 3],
    /// The medium: the distance before which there is no medium at all (`0.0` = from the near
    /// plane). Both in-scatter terms honour it, so it removes the beams along with the haze.
    pub fog_start: f32,
    /// The medium: the world height at which the density is exactly `fog_density`.
    pub fog_height: f32,
    /// The medium: exponential density falloff per world unit of height (`0.0` = height-independent,
    /// and a skip of the height half). Shapes **both** in-scatter terms — the beams thicken toward
    /// the ground exactly as the haze does.
    pub fog_height_falloff: f32,
    /// The medium: its single-scattering albedo α — the fraction of `fog_density` that scatters
    /// rather than absorbs. Scales **both** in-scatter terms, the ambient haze and the sun's beams
    /// (`1.0` = purely scattering, and byte-identical to the analytic fog that predates it).
    pub fog_albedo: f32,
    /// The medium: how much of its ambient in-scatter is the [`EnvironmentData`] sky's own radiance
    /// along the view ray rather than the flat `fog_color` (`0.0` = the authored slab, and a skip;
    /// `1.0` = the sky itself). Needs an `Environment` to mean anything — the renderer forces it to
    /// zero without one, so a scene authoring it with no `Environment` is byte-identical to one that
    /// leaves it unauthored.
    pub fog_sky: f32,
    /// The medium: its Henyey–Greenstein phase asymmetry `g`, exclusive `(-1, 1)` (`0.0` = isotropic,
    /// so the sun term reads as uniform fill; higher concentrates it into forward beams).
    pub fog_anisotropy: f32,
    /// Vignette: corner darkening strength (`0.0` = none, and the gate).
    pub vignette: f32,
    /// Vignette: the normalized screen distance at which the darkening begins.
    pub vignette_radius: f32,
    /// Ambient occlusion: how much of the estimate to apply (`0.0` = off, and the gate for the whole
    /// prepass/ssao/blur chain).
    pub ssao: f32,
    /// Ambient occlusion: the hemisphere's **world-space** radius.
    pub ssao_radius: f32,
    /// Ambient occlusion: the world-space depth bias a sample must clear before it counts as
    /// occluded.
    pub ssao_bias: f32,
    /// Ambient occlusion: how much of the finished factor also attenuates *direct* light (`0.0` = the
    /// ambient + IBL term only, which is the historical rule and byte-identical to no field at all).
    pub ssao_direct: f32,
    /// Light shafts: how much of the medium's in-scattered sun light to add (`0.0` = off, and half the
    /// gate for the march and its blur — [`RenderScene::sun`] is the other half). The march's
    /// transmittance, reach, brightness and lobe all come from the medium's own
    /// `fog_density`/`fog_albedo`/`fog_anisotropy`, so this is a pure intensity.
    pub shafts: f32,
    /// Depth of field: the far field's circle of confusion, as a fraction of the frame's **height**
    /// (`0.0` = everything in focus, and the gate for the prefilter and the gather).
    pub dof: f32,
    /// Depth of field: the focal plane's distance from the camera, in world units.
    pub focus_distance: f32,
    /// Film emulation: the per-channel radial displacement of the **scene fetch** at an edge
    /// midpoint, in normalized screen units (`0.0` = off, and the gate).
    pub chromatic_aberration: f32,
    /// Film emulation: how much warm halation to add on top of the bloom mix (`0.0` = off, and the
    /// gate). Reuses the bloom chain's blurred bright field, so it inherits its radius and threshold.
    pub halation: f32,
    /// Film emulation: grain amplitude, as a signed offset in the sRGB-**encoded** `[0, 1]` range
    /// (`0.0` = off, and the gate).
    pub grain: f32,
    /// Film emulation: the grain cell's side, as a fraction of the frame's **height**.
    pub grain_size: f32,
}

impl Default for PostData {
    /// **The engine's shipped imaging**, in one place: what the composite and bloom passes do for a
    /// scene that declares no `PostProcess`, and equally what a fully-default `PostProcess`
    /// deserializes to. The two must agree field for field — that agreement is what makes "an empty
    /// `PostProcess: {}` is byte-identical to no component at all" true rather than merely intended,
    /// and `ludion-engine`'s extract tests pin it.
    ///
    /// Three of these are the arc's shipped imaging chain rather than neutral values, and each is
    /// derived rather than picked: `bloom_strength` is `0.6 / 5.85`, the strength at which the
    /// pyramid adds the light the engine's former single-blur chain added at its own `0.6` (the
    /// 5.85× is measured, not the nominal six); `exposure` is what puts `0.18` linear mid-grey at
    /// the output value ACES gave it, so swapping the operator does not read as a darkening; and the
    /// dither is on because banding is a defect rather than a look. Everything else is neutral, and
    /// every effect gate is off — the engine ships machinery, not a look.
    fn default() -> PostData {
        PostData {
            bloom_threshold: 1.0,
            bloom_strength: 0.1,
            exposure: 1.3,
            tonemap: TonemapOperator::Agx,
            dither: true,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
            lift: [0.0; 3],
            gamma: [1.0; 3],
            gain: [1.0; 3],
            fog_density: 0.0,
            fog_color: [0.5, 0.5, 0.5],
            fog_start: 0.0,
            fog_height: 0.0,
            fog_height_falloff: 0.0,
            fog_albedo: 1.0,
            fog_sky: 0.0,
            fog_anisotropy: 0.7,
            vignette: 0.0,
            vignette_radius: 0.5,
            ssao: 0.0,
            ssao_radius: 0.5,
            ssao_bias: 0.02,
            ssao_direct: 0.0,
            shafts: 0.0,
            dof: 0.0,
            focus_distance: 10.0,
            chromatic_aberration: 0.0,
            halation: 0.0,
            grain: 0.0,
            grain_size: 0.002,
        }
    }
}

/// Which tonemap operator the composite pass applies — the renderer's plain-data mirror of the
/// scene's authored `PostProcess.tonemap`.
///
/// Every variant is the same one pass with a different branch inside it: an operator is a pure
/// per-pixel function of already-sampled colour, which is precisely the class
/// `Plan/post-process-pipeline.md` decision 2 folds into the composite rather than giving its own
/// graph pass.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TonemapOperator {
    /// AgX: a log-encoded sigmoid in a Rec.2020 working space that desaturates gracefully toward
    /// white as values clip, the way film does — the engine's shipped operator, and so the
    /// [`Default`].
    #[default]
    Agx,
    /// The Narkowicz ACES filmic fit.
    Aces,
    /// The Khronos PBR Neutral operator: hue-preserving, leaving everything below its compression
    /// knee untouched and desaturating only the highlights.
    Neutral,
    /// No operator at all: the linear value passes through and the 8-bit output target's own store
    /// clamps it. Useful for judging what the scene pass actually produced.
    None,
}

/// A dynamic point light: position, color, intensity, and a hard range cutoff.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PointLightData {
    /// World-space position.
    pub position: Vec3,
    /// Linear RGB color.
    pub color: Vec3,
    /// Scalar brightness multiplier.
    pub intensity: f32,
    /// Distance beyond which the light contributes nothing (smooth falloff to zero at `range`).
    pub range: f32,
    /// Whether the scene's `GiDynamic` marker tagged this light's entity — renderer plain data,
    /// **not** wire, **not hashed** (the authoring-side `GiDynamic` component is world state and
    /// hashes like any other; this flag is `ludion-engine`'s per-frame read of it, carried onto the
    /// extracted light so a later GI pass can split the light set without a second world walk).
    /// Does not affect clustered forward in any way: a marked light still reaches every consumer of
    /// `RenderScene.lights` in the same content and order as an unmarked one.
    pub dynamic: bool,
}

/// How a text run samples a baked font's atlas: `texture` indexes [`RenderScene::textures`], and
/// `sdf` selects the linear-sampled, thresholded path over the nearest-sampled bitmap one. A run
/// without one draws the renderer's embedded 8x8 atlas.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FontDraw {
    /// Index into [`RenderScene::textures`] of the face's atlas image.
    pub texture: usize,
    /// Whether the atlas is a signed-distance field (`mode: sdf`).
    pub sdf: bool,
}

/// A run of text placed in world space.
///
/// `ludion-engine`'s extract lays the text out (`ludion_font::layout`) and hands the renderer the
/// positioned [`GlyphQuad`]s in world XY; the renderer draws them verbatim at [`position`](TextRun::position)`.z`.
#[derive(Clone, PartialEq, Debug)]
pub struct TextRun {
    /// World-space top-left of the first glyph cell.
    pub position: Vec3,
    /// World size of one glyph cell.
    pub size: f32,
    /// RGBA text color (linear); alpha multiplies the glyph coverage.
    pub color: [f32; 4],
    /// The text as authored; introspection only — the renderer draws `quads`.
    pub text: String,
    /// The laid-out glyph quads, world XY (`pos[1]` the top edge, above `pos[3]`).
    pub quads: Vec<GlyphQuad>,
    /// The laid-out block's `[width, height]` in world units.
    pub extent: [f32; 2],
    /// The face's atlas, or `None` for the embedded 8x8 face.
    pub font: Option<FontDraw>,
}

/// One resolved screen-space UI element: a colored/textured rect, or a run of screen-space
/// text. `ludion-engine`'s `extract()` resolves anchor + offset + size/measured-text into this plain
/// pixel-space data — the renderer never sees an anchor or a canvas (boundary rule,
/// `engine-architecture.md` §2) — and emits [`RenderScene::ui`] pre-sorted by `(order, EntityId)`,
/// one interleaved vec so rects and text layer correctly across kinds.
#[derive(Clone, PartialEq, Debug)]
pub enum UiInstance {
    /// A colored/textured screen-space rectangle.
    Rect {
        /// Pixel-space `[x, y, w, h]`, top-left origin, +Y down.
        rect_px: [f32; 4],
        /// Index into [`RenderScene::textures`], or `None` for a solid color (the [`SpriteInstance`]
        /// convention: no texture is a plain tinted quad, not a separate rect/sprite pair).
        texture: Option<usize>,
        /// Linear RGBA tint multiplied into the sampled texel (or the solid fill, if untextured).
        /// Straight alpha, over a premultiplied texel — the [`SpriteInstance::color`] convention,
        /// which the two share because they resolve through the one texture cache.
        color: [f32; 4],
        /// The sub-rectangle of the texture this rect samples, `[u0, v0, u1, v1]` in the
        /// top-left-origin convention of the source PNG. `[0, 0, 1, 1]` (the whole texture) is a
        /// loose `texture:` PNG or an untextured fill; a packed atlas region is a sub-rect, the
        /// [`SpriteInstance::uv`] convention — so a run of rects sharing one atlas still batches.
        uv: [f32; 4],
    },
    /// A run of screen-space text, laid out in pixel space by extract (the same layout as
    /// [`TextRun`]).
    Text {
        /// Linear RGBA text color; alpha multiplies the glyph coverage.
        color: [f32; 4],
        /// The run's source text, as authored; introspection only — the renderer draws `quads`.
        text: String,
        /// Pixel-space glyph quads, top-left origin, +Y down.
        quads: Vec<GlyphQuad>,
        /// The face's atlas, or `None` for the embedded 8x8 face.
        font: Option<FontDraw>,
    },
}

/// An asset mesh's CPU-side vertex data — the mesh analog of [`TextureImage`]: plain data (flat
/// float/index lists), no wgpu type, referenced by index from [`MeshShape::Asset`].
///
/// [`positions`](MeshData::positions) and [`normals`](MeshData::normals) are flat, 3 floats per
/// vertex and the same length; normals are always present (computed faceted at load when the source
/// document omits them). [`colors`](MeshData::colors) is flat RGBA, 4 floats per vertex, and is
/// **always populated** — mesh-asset decode fills opaque white so every vertex rides the one mesh
/// pipeline; voxel meshing fills per-quad palette colors instead. [`uvs`](MeshData::uvs) (2 floats
/// per vertex) and [`tangents`](MeshData::tangents) (4) are always populated for the same reason,
/// defaulting to `(0, 0)` and `(1, 0, 0, 1)` when the source carries neither. [`indices`](MeshData::indices)
/// is a flat triangle list. [`hash`](MeshData::hash) is the content hash of the source `.mesh.yaml`
/// bytes — the renderer's mesh-cache key, exactly as [`TextureImage::hash`] keys the texture cache.
/// [`skin_joints`](MeshData::skin_joints) is extract input only, like `authored_material`: empty for
/// an unskinned mesh, one [`SkinJoint`] per joint otherwise.
#[derive(Clone, PartialEq, Debug)]
pub struct MeshData {
    /// Flat object-space positions, 3 floats per vertex.
    pub positions: Vec<f32>,
    /// Flat object-space faceted normals, 3 floats per vertex (same length as `positions`).
    pub normals: Vec<f32>,
    /// Flat per-vertex RGBA color (linear), 4 floats per vertex. Always populated; opaque white
    /// (`[1, 1, 1, 1]`) for asset/primitive meshes, so `vertex_color × instance_color` is exact.
    pub colors: Vec<f32>,
    /// Flat per-vertex material `(roughness, metallic, emissive)`, 3 floats per vertex. Always
    /// populated: the default material `(1, 0, 0)` fills unauthored vertices/channels, so every
    /// vertex rides the one mesh pipeline. Consumed only when [`authored_material`](MeshData::authored_material)
    /// is set (the extract-time selection flag).
    pub materials: Vec<f32>,
    /// Flat per-vertex texture coordinates, 2 floats per vertex. Always populated: a source with no
    /// UVs fills `(0, 0)`, so every vertex rides the one mesh pipeline whatever the source knows.
    pub uvs: Vec<f32>,
    /// Flat per-vertex tangent frame `(x, y, z, handedness)`, 4 floats per vertex — the tangent
    /// pointing along +U, its `w` the bitangent's sign. Always populated: a source with no tangent
    /// frame fills `(1, 0, 0, 1)`, so every vertex rides the one mesh pipeline.
    pub tangents: Vec<f32>,
    /// Flat per-vertex skin weights, 4 floats per vertex, summing to 1. Always populated: a source
    /// with no skin fills `(1, 0, 0, 0)`, paired with [`bone_indices`](MeshData::bone_indices)'
    /// default of the root bone (index 0) so the unskinned arithmetic is exact regardless.
    pub weights: Vec<f32>,
    /// Flat per-vertex bone indices, 4 per vertex, indexing the instance's slice of the renderer's
    /// bone palette. Always populated: a source with no skin fills `(0, 0, 0, 0)`.
    pub bone_indices: Vec<u32>,
    /// The skin's joints, parent-first, empty for an unskinned mesh. Extract input only — the
    /// renderer never reads it — consumed by extract to build each instance's slice of
    /// [`RenderScene::bone_palette`]: posed from a part entity's world transform when one follows a
    /// joint, else the joint's rest pose relative to its parent.
    pub skin_joints: Vec<SkinJoint>,
    /// The texture maps this mesh's material binds, or `None` for a mesh whose material is scalar
    /// only. Part of the mesh rather than the instance: the maps are the document's, and the
    /// renderer's per-draw material bind group lives as long as the cached mesh upload keyed on
    /// [`hash`](MeshData::hash).
    pub textures: Option<MeshTextures>,
    /// Whether the source document authored any material (a mesh-doc material list, or a voxel
    /// palette entry field). Selects per-vertex material over the `Mesh` component's fields at
    /// extract — false for primitives and plain asset meshes.
    pub authored_material: bool,
    /// Triangle list: flat vertex indices, 3 per triangle.
    pub indices: Vec<u32>,
    /// Content hash of the source mesh document folded with the hashes of every texture file it names
    /// (`fold_source_hashes`, set by the engine's mesh loader). It keys the renderer's mesh cache and
    /// per-mesh material bind group, so an edit that changes only a texture must still change it.
    pub hash: u64,
}

/// One joint of a [`MeshData::skin_joints`] skin, in the mesh document's own units (model space,
/// unscaled by any instance's `Transform`). Extract input only — the renderer never reads it.
#[derive(Clone, PartialEq, Debug)]
pub struct SkinJoint {
    /// The joint's name, unique within the skin (what a part's `SkinBone.bone` names).
    pub name: String,
    /// The parent's index in this skin's joint list; `None` for a root.
    pub parent: Option<u32>,
    /// The joint's local TRS at bind, relative to its parent (a root's is its own local transform).
    pub rest_local: Mat4,
    /// The bind translation `rest_local` is built from; what a clip channel replaces.
    pub rest_translation: Vec3,
    /// The bind rotation `rest_local` is built from; what a clip channel replaces.
    pub rest_rotation: Quat,
    /// The bind scale `rest_local` is built from; what a clip channel replaces.
    pub rest_scale: Vec3,
    /// The inverse bind matrix.
    pub inverse_bind: Mat4,
    /// The joint's rest-pose bone-palette entry: `rest_model × inverse_bind`, composed in `f64` then
    /// narrowed (identity up to the export's precision). What a part-less skin pushes verbatim.
    pub rest: Mat4,
}

/// The texture maps a mesh asset binds per draw — glTF's metallic-roughness set. `None` for every
/// source but an imported textured mesh document.
///
/// Each map is independently optional, and the mesh pass branches on which ones are present rather
/// than multiplying by a neutral stand-in: a draw with no map takes the untextured arithmetic exactly.
#[derive(Clone, PartialEq, Debug)]
pub struct MeshTextures {
    /// Multiplies the albedo (sRGB-encoded source).
    pub base_color: Option<MeshTexture>,
    /// glTF's packed metallic-roughness: `g` scales roughness, `b` scales metallic (linear source).
    pub metallic_roughness: Option<MeshTexture>,
    /// Tangent-space normal perturbation, `(0.5, 0.5, 1)` for an unperturbed texel (linear source).
    pub normal: Option<MeshTexture>,
    /// Ambient occlusion in `r`, attenuating the ambient/IBL term alone (linear source).
    pub occlusion: Option<MeshTexture>,
    /// Multiplies the emissive term (sRGB-encoded source).
    pub emissive: Option<MeshTexture>,
    /// How hard the [`normal`](MeshTextures::normal) map's tangent-space xy is scaled (glTF
    /// `normalTexture.scale`, default `1.0`).
    pub normal_scale: f32,
    /// How far the [`occlusion`](MeshTextures::occlusion) map darkens toward its sampled value
    /// (glTF `occlusionTexture.strength`, default `1.0`; `0` leaves the term untouched).
    pub occlusion_strength: f32,
}

/// One texture map of a [`MeshTextures`] set: the decoded image and the wrap mode its glTF sampler
/// authored.
///
/// The image is the same [`TextureImage`] the sprite path carries, `Arc`-shared so one PNG referenced
/// by several materials decodes once — but **never premultiplied** (the sprite path's premultiply is a
/// blending convention, and a metallic-roughness or normal map has no alpha to weight by), and
/// [`TextureImage::filter_linear`] is ignored: a mesh map is always sampled trilinear.
#[derive(Clone, PartialEq, Debug)]
pub struct MeshTexture {
    /// The decoded RGBA8 image.
    pub image: Arc<TextureImage>,
    /// The address mode both axes are sampled with.
    pub wrap: TextureWrap,
}

/// How a [`MeshTexture`]'s sampler addresses coordinates outside `[0, 1]` — glTF's wrap modes, the
/// same on both axes (a material with unequal `wrapS`/`wrapT` is refused at import).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TextureWrap {
    /// Tile the image (glTF's default).
    #[default]
    Repeat,
    /// Hold the edge texel.
    Clamp,
    /// Tile, mirroring every other copy.
    Mirror,
}

/// The ceiling on any single axis of a global-illumination 3D texture, in texels — both the
/// [`WorldVoxelGrid`]'s cell extent and the probe volume's [`GiData::count`], since each becomes one
/// `TextureDimension::D3` texture.
///
/// 2048 is WebGPU's **guaranteed minimum** for `max_texture_dimension_3d` — the floor every
/// conformant adapter must meet — deliberately in place of the limit the local adapter reports. A
/// scene that validates against the spec floor validates on every conformant GPU, which is the
/// contract an engine shipping per-OS golden sets on two pinned GPUs needs; a scene validated
/// against one machine's larger limit would be a scene that only renders on that machine.
///
/// It also bounds the bake's compute dispatch: 2048 probes on an axis at the shader's
/// `@workgroup_size(4, 4, 4)` is 512 workgroups, far under the 65535 that
/// `max_compute_workgroups_per_dimension` guarantees.
///
/// **Enforced in `ludion-engine`'s sizing**, before anything is allocated: `L2103` for a grid axis,
/// `L2105` for a probe-lattice axis. The texture-creation sites in this crate only assert it, so a
/// breach is a scene diagnostic rather than a wgpu validation panic inside frame preparation.
pub const MAX_GI_TEXTURE_AXIS: u32 = 2048;

/// A world-space voxelization of the scene's placed geometry: occupancy plus the albedo and
/// emissive of whatever filled each cell — the mesh analog of [`MeshData`] for volumes, plain data
/// with no wgpu type, produced by `ludion-engine`'s voxelizer and carried on
/// [`RenderScene::voxel`].
///
/// The grid is an **input to indirect lighting, not an image**. No pass samples it and no shader
/// binds it; its product is consumed by the global-illumination bake. A scene that authors no
/// `GiVolume` carries `None` here and renders through the historical pipeline untouched.
///
/// **Layout.** Cell `(x, y, z)` covers the world box
/// `[origin + (x,y,z)·cell_size, origin + (x+1,y+1,z+1)·cell_size]`, half-open at the upper face so
/// every world point belongs to exactly one cell. The three per-cell vectors are parallel, all of
/// length `extent.x · extent.y · extent.z`, flat-indexed `x + ex·(y + ey·z)` — the same flat order
/// the voxel-model grids use. [`albedo`](WorldVoxelGrid::albedo) and
/// [`emissive`](WorldVoxelGrid::emissive) are meaningless where
/// [`occupancy`](WorldVoxelGrid::occupancy) is false (they hold the zeroed allocation).
///
/// Occupancy is a `Vec<bool>` rather than a bitset deliberately: at one byte per cell it is a
/// twentieth of the albedo vector's sixteen, so packing it would shave ~4% off the grid while
/// costing every reader a shift and a mask.
///
/// Every input that affects the grid is *in* the grid — the extract-time model transforms, colors
/// and palettes have already been resolved into these buffers — so the struct's own bytes are a
/// complete cache key for anything derived from it, with no per-instance state to re-gather. That is
/// literal rather than aspirational: [`hash`](WorldVoxelGrid::hash) *is* those bytes' content hash,
/// computed once by the producer, so a consumer keys on a `u64` compare and never re-walks the grid.
#[derive(Clone, PartialEq, Debug)]
pub struct WorldVoxelGrid {
    /// World-space minimum corner of cell `(0, 0, 0)`, snapped outward to the `cell_size` lattice.
    pub origin: Vec3,
    /// World units per cell — the authored `GiVolume.cell_size`.
    pub cell_size: f32,
    /// Extent in cells, `[ex, ey, ez]`, each at least 1.
    pub extent: [u32; 3],
    /// Whether each cell contains geometry, flat-indexed.
    pub occupancy: Vec<bool>,
    /// Each cell's linear RGBA albedo, flat-indexed.
    pub albedo: Vec<[f32; 4]>,
    /// Each cell's emissive strength, flat-indexed.
    pub emissive: Vec<f32>,
    /// [`content_hash`](WorldVoxelGrid::content_hash) of every other field, stamped by the producer
    /// once the buffers are final (see [`sealed`](WorldVoxelGrid::sealed)) — the cache key every
    /// consumer of the grid keys on, so hashing megabytes is a build-time cost and never a per-frame
    /// one.
    ///
    /// A pure function of the other fields, so it participates in `PartialEq` like any other field:
    /// it cannot make two equal grids compare unequal, and it makes a producer that forgot to stamp
    /// it loud instead of silent.
    pub hash: u64,
}

impl WorldVoxelGrid {
    /// The flat index of cell `(x, y, z)` — `x + ex·(y + ey·z)`. The caller is responsible for the
    /// cell being in range; an out-of-range cell yields an index past the vectors' end.
    pub fn index(&self, cell: [u32; 3]) -> usize {
        let [ex, ey, _] = self.extent;
        let [x, y, z] = cell;
        x as usize + ex as usize * (y as usize + ey as usize * z as usize)
    }

    /// The number of cells the grid holds — the length of each of the three parallel vectors.
    pub fn cell_count(&self) -> usize {
        let [ex, ey, ez] = self.extent;
        ex as usize * ey as usize * ez as usize
    }

    /// Content hash of everything the grid carries: origin, cell size, extent, then the three
    /// parallel per-cell vectors in their own flat order. Raw little-endian bytes through
    /// [`ludion_assets::content_hash`] (the XXH3-64 the mesh and texture caches use) rather than
    /// `serde`/JSON — floats round-trip exactly and there is no format-version concern for an
    /// in-memory-only key.
    ///
    /// Costs a walk of the whole grid, so call it once, at the point the buffers stop changing:
    /// [`hash`](WorldVoxelGrid::hash) is where the answer lives. Exposed so a caller can re-derive it
    /// and prove the stored value has not drifted from the bytes.
    pub fn content_hash(&self) -> u64 {
        let mut bytes = Vec::with_capacity(16 + self.cell_count() * (16 + 4 + 1));
        bytes.extend_from_slice(&self.origin.x.to_le_bytes());
        bytes.extend_from_slice(&self.origin.y.to_le_bytes());
        bytes.extend_from_slice(&self.origin.z.to_le_bytes());
        bytes.extend_from_slice(&self.cell_size.to_le_bytes());
        for e in self.extent {
            bytes.extend_from_slice(&e.to_le_bytes());
        }
        for c in &self.albedo {
            for v in c {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        for v in &self.emissive {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend(self.occupancy.iter().map(|&b| u8::from(b)));
        ludion_assets::content_hash(&bytes)
    }

    /// Stamps [`hash`](WorldVoxelGrid::hash) with the grid's own
    /// [`content_hash`](WorldVoxelGrid::content_hash) — the producer's last step, once no buffer will
    /// change again.
    pub fn sealed(mut self) -> WorldVoxelGrid {
        self.hash = self.content_hash();
        self
    }
}

/// The scene's global-illumination parameters — plain data for the renderer, resolved from the
/// lowest-`EntityId` `GiVolume` component alongside the [`WorldVoxelGrid`] it sizes.
///
/// Carried separately from the grid because the two have different lifetimes of meaning: the grid is
/// the bake's *input* geometry (content-hash-cached), while these are the authored knobs the bake and
/// the shader read. `None` on [`RenderScene::gi`] for a scene that authors no `GiVolume` — or authors
/// one over a scene with no geometry to voxelize, which leaves nothing to place probes around.
///
/// The lattice is sized by `ludion-engine` (the probe budget is a scene diagnostic, not a renderer
/// panic), so [`count`](GiData::count) and [`span`](GiData::span) arrive resolved and the renderer
/// does no sizing arithmetic of its own.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GiData {
    /// World units between adjacent probes — the authored `GiVolume.probe_spacing`.
    pub probe_spacing: f32,
    /// World-space offset along the surface normal before sampling the volume, the authored
    /// `GiVolume.normal_bias` — leak control that lives at sample time, not at bake time.
    pub normal_bias: f32,
    /// How strongly the baked visibility attenuates the ambient term, the authored
    /// `GiVolume.sky_visibility` in `[0, 1]`.
    pub sky_visibility: f32,
    /// How strongly baked one-bounce irradiance is added to the ambient term, the authored
    /// `GiVolume.bounce` (`>= 0`, default `0.0`).
    pub bounce: f32,
    /// Probes per axis, at least 2 everywhere so trilinear interpolation is always defined.
    pub count: [u32; 3],
    /// World-space position of probe `(0, 0, 0)` — the voxel grid's own origin.
    pub origin: Vec3,
    /// World-space extent the lattice covers, `(count - 1) · probe_spacing` per axis. At least the
    /// grid's own box on every axis, since the count rounds up.
    pub span: Vec3,
}

/// A decoded texture the scene references, in packed top-to-bottom RGBA8.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TextureImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Packed RGBA8 pixels.
    pub rgba: Vec<u8>,
    /// Content hash of the source bytes (with [`premultiplied_on_load`](Self::premultiplied_on_load),
    /// the renderer's texture-cache key).
    pub hash: u64,
    /// Whether the sprite pass samples this texture with a linear (bilinear) rather than nearest
    /// (point) filter. Set from the atlas document's `filter:` when an atlas resolves; a loose
    /// `texture:` PNG is `false` (nearest — the pixel-art default). Read per batch in
    /// `build_sprite_draws` to pick the sampler.
    pub filter_linear: bool,
    /// Whether the engine premultiplied these bytes when it decoded them (a loose PNG). `false` for
    /// bytes uploaded as stored: an atlas colour map (baked premultiplied) and a mesh map (never
    /// premultiplied). One PNG read both ways has one `hash` and two byte streams, so the cache key
    /// carries this too.
    pub premultiplied_on_load: bool,
}

/// Everything the renderer needs to draw one frame — plain data, no ECS, no paths.
#[derive(Clone, PartialEq, Debug)]
pub struct RenderScene {
    /// The resolved camera.
    pub camera: RenderCamera,
    /// Small constant ambient light added to every lit surface (linear RGB).
    pub ambient: [f32; 3],
    /// Unique decoded textures, referenced by index from [`SpriteInstance::texture`]. Their colour
    /// is **premultiplied** by alpha (the engine converts a loose PNG at load; an atlas is baked
    /// that way), which is the convention the sprite pass's `One, OneMinusSrcAlpha` blend needs.
    /// `Arc`-shared with the engine's `AssetCache`, like [`mesh_assets`](Self::mesh_assets).
    pub textures: Vec<Arc<TextureImage>>,
    /// Unique asset meshes, referenced by index from [`MeshShape::Asset`] (first-encounter order,
    /// deduplicated by content hash — a deterministic function of the world). `Arc`-shared with the
    /// engine's `AssetCache` so a cache hit clones a refcount, not the mesh's megabyte-scale buffers
    /// (`Plan/voxel-artist-scale.md`).
    pub mesh_assets: Vec<Arc<MeshData>>,
    /// Low-poly meshes.
    pub meshes: Vec<MeshInstance>,
    /// Sprites and billboards.
    pub sprites: Vec<SpriteInstance>,
    /// Ground-anchored telegraph markers, in ascending-`EntityId` extract order. Drawn inside the
    /// scene pass **after** the opaque meshes and **before** the sprites, so a marker is occluded by
    /// the geometry standing on it and never paints over a character. Empty for a scene that authors
    /// none, which leaves the pass's recorded command stream exactly the pre-marker one.
    pub ground_markers: Vec<GroundMarkerInstance>,
    /// World-space debug lines from the categories the host enabled; empty by default.
    pub debug_lines: Vec<DebugSegment>,
    /// Camera-facing particle discs, in `(ascending EntityId, particle index)` extract order. Drawn
    /// inside the scene pass **after** the sprites and **before** the world-space text, so a particle
    /// blends over the character it was emitted from and never paints over a label. The renderer
    /// sorts a copy back-to-front by view depth; this list itself is never reordered. Empty for a
    /// scene that authors none, which leaves the pass's recorded command stream exactly the
    /// pre-particle one.
    pub particles: Vec<ParticleInstance>,
    /// Dynamic point lights — **all** of them, uncapped: the renderer uploads the whole list to a
    /// storage buffer and clusters it (`Plan/clustered-forward.md`). The order is load-bearing:
    /// `ludion-engine`'s ascending-`EntityId` extract order is the per-fragment float summation
    /// order, so this list is packed verbatim and never sorted.
    pub lights: Vec<PointLightData>,
    /// The single directional "sun" (lowest-`EntityId` `DirectionalLight`), or `None` for a sunless
    /// scene (a zeroed `Sun` uniform, byte-identical to the pre-sun pipeline).
    pub sun: Option<SunData>,
    /// The scene's sky/lighting environment (lowest-`EntityId` `Environment`), or `None` for a scene
    /// with no `Environment` — in which case the engine's historical ambient/clear pipeline is kept
    /// byte-for-byte.
    pub environment: Option<EnvironmentData>,
    /// The scene's post-process settings (lowest-`EntityId` `PostProcess`), or `None` for a scene
    /// with no `PostProcess` — in which case the renderer's own shipped post defaults apply and the
    /// image is byte-identical to the pre-`PostProcess` pipeline.
    pub post: Option<PostData>,
    /// World-space text runs.
    pub texts: Vec<TextRun>,
    /// Resolved screen-space UI elements (rects and text), pre-sorted by `(order, EntityId)` — the
    /// pinned draw order (screen space has no view depth to sort by instead). Empty for a UI-free
    /// scene, which keeps the overlay pass unbuilt (`Plan/phase-14-ui.md` decision 5).
    pub ui: Vec<UiInstance>,
    /// The scene's world-space voxelization (a `GiVolume` singleton opts in), or `None` for a scene
    /// that authors none — in which case nothing is voxelized and the pipeline is the one that
    /// predates the component. Included in the `PartialEq` above, so the extract-determinism test
    /// covers the grid the same way it covers every other field (`Arc<T>: PartialEq` compares the
    /// grids, not the pointers).
    ///
    /// `Arc`-shared with the engine's voxelization cache: an extract whose instances are unchanged
    /// hands on the same grid rather than rasterizing the world again, so a live `GiVolume` scene
    /// pays the rasterization once, not once per frame.
    pub voxel: Option<Arc<WorldVoxelGrid>>,
    /// The authored global-illumination parameters and the probe lattice they size over
    /// [`voxel`](RenderScene::voxel), or `None` when there is no grid to place probes around. Never
    /// `Some` while `voxel` is `None`: the lattice is a function of the grid's own box.
    pub gi: Option<GiData>,
    /// Model-space bone matrices for every skinned instance, packed back-to-back in extract order.
    /// The renderer prepends an identity matrix at upload time, so [`MeshInstance::bone_offset`]
    /// indexes the uploaded buffer directly (`0` = identity, this list's entries start at `1`). Empty
    /// for a scene with no skinned mesh.
    pub bone_palette: Vec<Mat4>,
}

impl RenderScene {
    /// An empty scene with the documented default camera framing for `viewport` (width, height).
    ///
    /// Used when a scene declares no camera: the view looks at the origin from `(0, 2.5, 7)` down
    /// the −Z axis, with a 60° vertical FOV. Deterministic and independent of scene content.
    pub fn empty(viewport: (u32, u32)) -> RenderScene {
        RenderScene {
            camera: RenderCamera::default_framing(viewport),
            ambient: DEFAULT_AMBIENT,
            textures: Vec::new(),
            mesh_assets: Vec::new(),
            meshes: Vec::new(),
            sprites: Vec::new(),
            ground_markers: Vec::new(),
            debug_lines: Vec::new(),
            particles: Vec::new(),
            lights: Vec::new(),
            sun: None,
            environment: None,
            post: None,
            texts: Vec::new(),
            ui: Vec::new(),
            voxel: None,
            gi: None,
            bone_palette: Vec::new(),
        }
    }
}

impl RenderCamera {
    /// The documented default framing for a scene with no camera (see [`RenderScene::empty`]).
    pub fn default_framing(viewport: (u32, u32)) -> RenderCamera {
        let position = Vec3::new(0.0, 2.5, 7.0);
        let view = Mat4::look_at_rh(position, Vec3::ZERO, Vec3::Y);
        let aspect = aspect_ratio(viewport);
        let projection = Mat4::perspective_rh(60.0_f32.to_radians(), aspect, 0.1, 1000.0);
        RenderCamera {
            view,
            projection,
            position,
        }
    }
}

/// The aspect ratio (width / height) of a viewport, guarding against a zero height.
pub fn aspect_ratio(viewport: (u32, u32)) -> f32 {
    let (w, h) = viewport;
    if h == 0 { 1.0 } else { w as f32 / h as f32 }
}
