// The composite pass: the defocused, chromatically-aberrated scene plus blurred bloom and its
// halation, then fog, the light shafts, exposure, a selectable tonemap operator, the colour grade, a
// vignette and optional film grain and output dither, into the LDR target.
//
// Plan/post-process-pipeline.md decision 2 decides what belongs here: an effect needing its own
// sampling footprint, resolution or ping-pong gets its own graph pass (bloom, DoF, SSAO); an effect
// that is a **pure per-pixel function of already-sampled colour** folds in here. So the steps below
// are branches in one fragment shader, not one full-resolution round trip each.
//
// **The pinned order of the whole chain**, written out once here because half of it is a physical
// claim and the other half a convention, and neither is guessable from the code:
//
//   0. chromatic aberration         — at the fetch: a radial, per-channel displacement of the scene
//   1. depth of field               — the lens's defocus, blended over the sharp scene target
//   2. + bloom * strength + halation — the mix
//   3. the medium's ambient in-scatter — scene-referred: extinction + sky-coloured haze, linear HDR
//   4. the medium's sun in-scatter  — scene-referred: the marched beams, added to what survived
//   5. exposure                     — the camera's own gain
//   6. tonemap                      — HDR to display
//   7. grade                        — display-referred (see the grading section below)
//   8. vignette                     — display-referred: a lens darkening of the finished image
//   9. grain                        — the emulsion: light passes the lens, *then* it is recorded
//  10. dither                       — strictly last; it hides the *output* quantization
//
// Steps 9 and 10 belong to the last output pass (`output_encode.wgsl`): this one, unless SMAA or a
// resample follows, in which case `smaa_blend` or `resolve`/`upscale` applies them and this pass runs
// with both zeroed.
//
// Steps 3 and 8 are the pair worth justifying. Fog is atmospheric scattering: it happens to the light
// on its way to the lens, so it belongs in linear scene-referred colour, before the exposure and the
// operator — a camera photographs fogged light, it does not fog a photograph. A vignette is the
// opposite: it is the lens and aperture darkening the *image*, so it lands after the operator and the
// grade, and only the grain and the dither may follow it.
//
// Steps 0, 2 and 9 are the film-emulation roster (decision 13), and their positions follow the same
// reading. Chromatic aberration is a property of the *lens*, so it happens before anything the lens
// delivered has been touched — it is not applied to the image, it is how the image was formed, which
// is why it sits at the fetch rather than as a per-pixel step later. Halation is light scattering
// back off the film base into the emulsion, so it is scene-referred and joins the bloom mix it
// borrows its blurred field from. Grain is the emulsion recording that light: it lands after the
// vignette, because light passes the lens and *then* hits the film, and before the dither, which is
// not a camera at all but the last thing between us and an 8-bit integer.
//
// **Steps 3 and 4 are two terms of one medium, not two effects.** The medium is single-scattering with
// one extinction σ_t = `fog_density` and one albedo α = `fog_albedo` (σ_s = α·σ_t):
//
//     L = T(d)·surface + ∫₀ᵈ σ_s·T(t)·[ L_ambient + p(θ)·V(t)·L_sun ] dt
//
// Step 3 is the first bracketed term, and with a constant ambient radiance it collapses to the closed
// form `T(d)·surface + α·(1 - T(d))·L_ambient` — one exponential per pixel, no march, which is why the
// haze is analytic and cheap. Step 4 is the second: `shafts.wgsl` marches the sun's shadow map for
// `V(t)` and folds in the same σ_s, so it arrives as an add on top of whatever the extinction left.
// The order follows from the terms — the extinction acts on the surface, the in-scatter is what the
// medium puts back — and the shared σ_t is what makes thicker fog dim its own beams.
//
// **`L_ambient` is `mix(fog_color, sky_gradient(view_dir), fog_sky)`.** A real haze in-scatters the
// sky it sits under, so `fog_sky: 1` makes the medium blue toward the zenith and warm in the horizon
// band, and makes it *meet* the sky at the horizon instead of stopping at a seam. Evaluating the
// gradient here rather than sampling a prefiltered cube is available only because this engine's sky is
// procedural and will never be an image asset: it is three authored colours and a direction, so
// `shaders/snippets/sky_gradient.wgsl` gives the exact radiance for less arithmetic than an SH
// approximation of it, with no new binding and no truncation error.
//
// The direction is the **view ray**, which makes this the aerial-perspective approximation and not the
// isotropic integral: for a homogeneous medium under a fixed sky the true isotropic in-scatter is
// direction-independent, and what an eye actually reads as haze is the sky *behind* the surface
// showing through it. `fog_albedo` scales the term as it scales every in-scatter;
// `Environment.ibl_intensity` deliberately does not, because that knob grades how much the sky lights
// *surfaces* and this is the sky seen directly through a medium.
//
// They also share the medium's *shape*: `fog_start`, `fog_height` and `fog_height_falloff` reach both
// terms through `shaders/snippets/medium.wgsl`, so the `T(t)` the march integrates is the very
// function `fog_factor` evaluates at the surface. A height medium therefore thickens the beams toward
// the ground exactly as it thickens the haze, and neither term exists inside `fog_start`.
//
// Bloom is computed from the scene target before either term exists, so a shaft does not itself
// bloom — it is already a low-frequency glow, and adding a second full-resolution round trip to blur
// it again is exactly what decision 2's folding rule refuses.
//
// **Step 1 is placed by the gather's input, not by the optical path.** A lens defocuses light that has
// already been fogged and already carries the medium's in-scatter, which would argue for putting the
// blend at step 5 — but the gather reads the `scene` target, which is what the scene pass wrote:
// pre-fog, pre-shafts. Blending after the fog would therefore mix a fogged sharp image against an
// unfogged blurred one, and thick fog would vanish exactly where the defocus is strongest. Blending
// first applies one fog to one image, using each pixel's own (sharp) depth. The shafts sit behind the
// blend for a stronger reason than order of operations: in-scattered light is spread along the whole
// ray, so it has no single depth and therefore no circle of confusion, and it is already a
// deliberately low-frequency field — defocusing it would cost a gather to change nothing.
//
// Bloom is likewise computed from the *sharp* scene and added at step 2, because the bright pass reads
// `scene` and moving the defocus in front of it would insert a full-resolution HDR round trip between
// the scene pass and the bright pass on every DoF frame — the cost decision 2's folding rule exists to
// refuse. The recorded consequence: a defocused highlight blooms with its focused shape.
//
// The output target is `Rgba8UnormSrgb`, so the hardware applies the sRGB transfer function on store;
// every operator returns *linear* colour, and the `none` operator returns it unmapped, letting the
// target's own store clamp.
//
// **Neutral is skipped, never computed** (plan decision 9, normative): each step sits behind a
// uniform `if`, so a scene that authors none of them — no fog, an identity grade, no vignette, no
// aberration, no halation, no grain — pays for none of them. `x * 1.0` is exact in IEEE, but
// `pow(x, 1.0)` is not guaranteed across backends, and what this arc rests on is byte-identity, not
// near-identity. Two shipped defaults are deliberately *not* the skip: `exposure` is 1.3 and the
// dither is on, because both are part of the imaging chain rather than an authored effect.
//
// `VsOut` + `vs_main` come from `shaders/snippets/fullscreen_vs.wgsl`, `BAYER4` from
// `shaders/snippets/bayer.wgsl`, and the medium's geometry and the sky's gradient from
// `snippets/medium.wgsl` and `snippets/sky_gradient.wgsl` — all composed ahead of this body
// (Plan/shader-bindgen.md dec. 16).

@group(0) @binding(0) var scene_tex: texture_2d<f32>;
@group(0) @binding(1) var bloom_tex: texture_2d<f32>;
// The scene pass's depth buffer, bound on **every** frame rather than behind a pipeline permutation:
// a conditional read would fork this one shader into two bind group layouts and two pipelines, which
// is what the sunless frame's zeroed uniform plus 1×1 fallback depth already refuses to do for the
// mesh pass. Fog skips by a uniform branch like every other neutral step; the descriptor is free.
@group(0) @binding(2) var scene_depth_tex: texture_depth_2d;
// The blurred light-shaft term, at a fraction of the output extent — an **optional** input, and the
// first one any fullscreen effect has. When the scene asks for no shafts (or carries no sun) the
// graph declares neither the passes nor the target, and the recorder binds a 1×1 stand-in here
// instead; `composite.shafts` is then zero and the branch below never reads it. The binding index is
// fixed whether or not the feature is on, which is the whole point — see `fullscreen_effect`'s doc
// comment in `render.rs` for the rule every later optional input follows.
@group(0) @binding(3) var shafts_tex: texture_2d<f32>;
// The defocused scene and its blend weight (rgb, a), at a fraction of the output extent — the second
// **optional** input, and the one the fixed-slot rule exists for: it sits behind `shafts_tex` and
// neither slot can move the other, present or absent. When the scene asks for no depth of field the
// graph declares neither the passes nor the target, the recorder binds the same 1x1 stand-in here,
// and `composite.dof` is zero so the branch below never reads it.
@group(0) @binding(4) var dof_tex: texture_2d<f32>;
// The transparent layer: weighted view distance in R, coverage in G, at the render extent — the third
// **optional** input, behind `dof_tex`. It exists when the scene draws a sprite, particle or world
// text under fog or depth of field; otherwise the recorder binds the same 1x1 stand-in and
// `composite.transparent_depth` is zero, so the fog block below never reads it. With depth of field
// alone the layer exists and the flag is one, but the fog block is still unreachable.
@group(0) @binding(5) var transparent_depth_tex: texture_2d<f32>;
// The local-light in-scatter term (`medium_scatter_blur`): premultiplied radiance at a fraction of
// the output extent — the fourth **optional** input, behind `transparent_depth_tex`. It exists when a
// scattering light is selected in a medium or the haze reads a `GiVolume` (`medium_gi`); its alpha
// carries the GI ambient scale V̄. Otherwise the recorder binds the same 1x1 stand-in and
// `composite.medium_scatter` is zero, so the add below never reads it.
@group(0) @binding(6) var medium_scatter_tex: texture_2d<f32>;
@group(0) @binding(7) var samp: sampler;

// The selector values `Composite.tonemap` carries, one per `PostProcess.tonemap` variant. The
// shader-bindgen hoists integer consts into `shader_gen`, so the recorder maps its `TonemapOperator`
// onto *these* numbers rather than onto a parallel copy that could drift from the switch below.
const TONEMAP_AGX: u32 = 0u;
const TONEMAP_ACES: u32 = 1u;
const TONEMAP_NEUTRAL: u32 = 2u;
const TONEMAP_NONE: u32 = 3u;

// The composite's per-frame knobs, from the scene's `PostProcess` (or the renderer's shipped defaults
// when it declares none).
//
// 288 bytes. A `vec3<f32>` starts on a 16-byte boundary and leaves the fourth lane of its block free,
// so each triple carries one loose scalar in that lane rather than a hole, and `tint`'s own block
// carries the three fog/vignette scalars that would otherwise need one. The two aberration/halation
// scalars took the block `dof` sits in, which had two free lanes; the grain pair opens a block whose
// remaining two lanes are `fog_albedo` and `fog_sky`. The three sky colours are the one place the rule
// cannot be kept — they are three triples and there is no fourth scalar left to ride with them, so
// their spare lanes are spelled pads, except the zenith's, which carries `transparent_depth`. The
// prefix is 208 bytes, the matrix 64 and the `view_forward` lane 16. `inv_view_proj` precedes the
// lane because a `mat4x4<f32>` must start on a 16-byte boundary; the tail scalars ahead of it are
// what buys that alignment (`shader_gen`'s generated POD has no implicit padding, so a hole must be
// spelled).
struct Composite {
    // How strongly the blurred bloom is mixed back in (`bloom_strength`, default 0.1 — small
    // because the pyramid composites the sum of all six of its levels).
    strength: f32,
    // A linear exposure multiplier (`exposure`, default 1.3 — what puts 0.18 mid-grey through AgX
    // where ACES put it), applied to scene + bloom immediately before the operator. Applying it here
    // and nowhere else leaves `bloom_threshold`'s meaning — scene luminance — unchanged.
    exposure: f32,
    // Which operator runs: one of the `TONEMAP_*` selectors above.
    tonemap: u32,
    // Nonzero enables the output dither (`dither`, default on).
    dither: u32,
    // The CDL offset (`lift`, default 0), added after `gain`.
    lift: vec3<f32>,
    // Contrast about a 0.5 pivot (`contrast`, default 1.0).
    contrast: f32,
    // The CDL power, applied as its **reciprocal** (`gamma`, default 1) — higher brightens midtones.
    gamma: vec3<f32>,
    // Chroma mix toward Rec.709 luma (`saturation`, default 1.0; 0 is greyscale).
    saturation: f32,
    // The CDL slope (`gain`, default 1), multiplied before `lift`.
    gain: vec3<f32>,
    // White balance along the blue↔amber axis (`temperature`, default 0, range [-1, 1]).
    temperature: f32,
    // White balance along the magenta↔green axis (`tint`, default 0, range [-1, 1]).
    tint: f32,
    // Distance before which there is no medium at all (`fog_start`, default 0). The march reads it
    // too, so a beam does not exist inside it either.
    fog_start: f32,
    // The world height the medium's density is `fog_density` at (`fog_height`, default 0).
    fog_height: f32,
    // Exponential density falloff per world unit of height (`fog_height_falloff`, default 0 —
    // height-independent medium, and a skip). It shapes the beams as well as the haze; the three
    // fields above travel to the march in its own uniform.
    fog_height_falloff: f32,
    // What the fog blends toward, linear HDR (`fog_color`).
    fog_color: vec3<f32>,
    // Extinction per world unit (`fog_density`, default 0). Nonzero enables the whole fog step.
    fog_density: f32,
    // The camera's world position — one end of the ray the fog integrates along.
    cam_pos: vec3<f32>,
    // Corner darkening strength (`vignette`, default 0). Nonzero enables the vignette step.
    vignette: f32,
    // Normalized screen distance at which the darkening starts (`vignette_radius`, range [0, 1]).
    vignette_radius: f32,
    // Nonzero enables the depth-of-field composite. It mirrors the graph's own gate exactly: the
    // prefilter and the gather exist when the scene authors a positive `dof`, and only then is
    // `dof_tex` real.
    dof: u32,
    // The per-channel radial displacement of the **scene fetch** at an edge midpoint, in normalized
    // UV (`chromatic_aberration`, default 0). Nonzero enables the aberration.
    chromatic_aberration: f32,
    // How much warm halation to add on top of the bloom mix (`halation`, default 0). Nonzero enables
    // the step. It reuses `bloom_tex`, so it inherits `bloom_threshold` and the bloom chain's radius.
    halation: f32,
    // What the light shafts are tinted with: the sun's premultiplied colour. The march writes a
    // scalar in-scattered fraction (one channel is all a shadow test produces, and it is what lets the
    // 4×4 blur stay one shared shader), so the colour is applied here — it is constant across the
    // frame, which makes tinting after the blur exactly tinting before it.
    shafts_color: vec3<f32>,
    // Nonzero enables the light-shaft add. It mirrors the graph's own gate exactly: the passes exist
    // when the scene asks for shafts **and** carries a sun, and only then is `shafts_tex` real.
    shafts: u32,
    // Film grain amplitude, as a signed offset in the sRGB-**encoded** [0, 1] range (`grain`,
    // default 0 — the gate for the whole grain step). 0.02 is about ±5 output steps. Zeroed when a
    // later pass owns the grain.
    grain: f32,
    // The grain cell's side as a fraction of the frame's **height** (`grain_size`, default 0.002 —
    // about 2 pixels at 1080p), floored at one pixel of the target the grain is applied at by
    // `MIN_GRAIN_CELL`.
    grain_size: f32,
    // The medium's single-scattering albedo α (`fog_albedo`, default 1.0), the coefficient of the
    // ambient in-scatter. It lives in the grain block's spare lane rather than beside `fog_density`,
    // whose own block is full: every fog/vignette scalar already rides in a triple's fourth lane, and
    // giving the albedo a new block would open sixteen bytes to place one scalar. `fog_anisotropy` is
    // absent on purpose — the ambient term is isotropic, so the phase only reaches the march.
    fog_albedo: f32,
    // How much of the ambient in-scatter is the sky's own radiance rather than the authored
    // `fog_color` (`fog_sky`, default 0 — the flat slab, and a skip). The recorder forces it to zero on
    // a scene with no `Environment`, so the three colours below are never read unauthored.
    fog_sky: f32,
    // The `Environment`'s authored sky gradient, linear HDR — zenith, horizon, ground. Zeroed when the
    // scene carries no `Environment`.
    sky_zenith: vec3<f32>,
    // Nonzero when `transparent_depth_tex` is real. It mirrors the graph's own gate exactly, as `dof`
    // does for `dof_tex`.
    transparent_depth: u32,
    sky_horizon: vec3<f32>,
    // The frame's viewmodel depth slice `s` (0 without a viewmodel): a pixel whose depth is below it is
    // the gun, which the defocus mix leaves alone.
    viewmodel_slice: f32,
    sky_ground: vec3<f32>,
    // Nonzero when `medium_scatter_tex` is real; mirrors the graph's gate as `shafts` does.
    medium_scatter: u32,
    // Inverse view-projection, for reconstructing a fragment's world position from its depth.
    // Inverted on the CPU, where the matrix already lives and the cost is once per frame.
    inv_view_proj: mat4x4<f32>,
    // The `view_ray.wgsl` lane: the camera's one view direction, with `w` set for an orthographic
    // camera. Fog measures distance along it there rather than along the per-pixel ray.
    view_forward: vec4<f32>,
    // `.x` is nonzero when the local-light pass also carries the GI-scaled ambient in its alpha
    // (`medium_scatter.wgsl`); the other lanes are pad.
    medium_gi: vec4<u32>,
};
@group(0) @binding(8) var<uniform> composite: Composite;

// Rec.709 luma weights: the grey axis the white balance normalizes against and the saturation mix
// collapses toward. One definition, so "luminance-preserving" means the same thing in both steps.
const LUMA_709: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// --- Fog ------------------------------------------------------------------------------------------
//
// Step 3 of the chain, and the first post effect to consume the scene depth buffer.
//
// The fetch is a `textureLoad`, not a `textureSample`, for the same reason the `resolve` pass loads:
// `scene_depth` is `Depth32Float`, which is not a filterable format, and this pass binds the shared
// *filtering* sampler — pairing the two would be a validation error. A load needs no sampler at all,
// is exact, and is 1:1 here because `composite` and `scene_depth` are both sized from the render
// extent.

// The world-space position of the fragment at `uv` whose stored device depth is `depth`.
//
// wgpu's clip space is x,y in [-1, 1] with +y up and z in [0, 1] running near→far — the convention
// glam's `perspective_rh`/`orthographic_rh` build and the convention the scene pass's 1.0 depth clear
// asserts. `uv.y` runs top-down (the fullscreen triangle flips it), which is the one sign to get
// right.
// Kept out of world_from_depth.wgsl: this function moves 3090 pixels through compilation alone (Plan/transparent-depth.md, A fragility).
fn world_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let world = composite.inv_view_proj * vec4<f32>(ndc, 1.0);
    return world.xyz / world.w;
}

// How much of `fog_color` a surface at `world` is buried under, in `[0, 1)`: the extinguished
// fraction `1 - T(d)` over the segment of the camera ray past `fog_start`.
//
// The optical depth itself comes from `shaders/snippets/medium.wgsl`, which is where the medium's
// height model and the `fog_start` offset are written down — the march in `shafts.wgsl` integrates
// the *same* function, so the two terms cannot disagree about where the medium is or how thick it is
// at a given height.
//
// An orthographic camera has no eye the rays fan out from: every ray runs along the one view
// direction, so the distance is the depth along it and the ray's height is the surface's own less
// the climb along that direction. A perspective camera falls through to the expressions it always
// had, untouched by the branch above them.
fn fog_factor(world: vec3<f32>) -> f32 {
    let to_surface = world - composite.cam_pos;
    if (composite.view_forward.w > 0.5) {
        let forward = composite.view_forward.xyz;
        let depth_along = dot(to_surface, forward);
        let optical_depth = medium_optical_depth(
            composite.fog_density,
            composite.fog_start,
            composite.fog_height,
            composite.fog_height_falloff,
            world.y - forward.y * depth_along,
            forward.y,
            depth_along,
        );
        return 1.0 - exp(-optical_depth);
    }
    let distance = length(to_surface);
    let optical_depth = medium_optical_depth(
        composite.fog_density,
        composite.fog_start,
        composite.fog_height,
        composite.fog_height_falloff,
        composite.cam_pos.y,
        to_surface.y / max(distance, 1e-6),
        distance,
    );
    return 1.0 - exp(-optical_depth);
}

// The point at distance `t` along this pixel's view ray: the transparent layer's mean distance,
// turned back into a position `fog_factor` can measure. Under an orthographic camera `t` is a depth
// along the view direction, so the point is the far-plane point slid back along it.
fn layer_world(uv: vec2<f32>, t: f32) -> vec3<f32> {
    let far = world_from_depth(uv, 1.0);
    let to_far = far - composite.cam_pos;
    if (composite.view_forward.w > 0.5) {
        let forward = composite.view_forward.xyz;
        return far - forward * (dot(to_far, forward) - t);
    }
    return composite.cam_pos + normalize(to_far) * t;
}

// Narkowicz's ACES filmic curve fit — the engine's shipped operator, kept verbatim so `aces` is the
// frozen path. It hue-shifts bright saturated colours toward yellow and then white as they clip.
fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

// AgX's default-contrast sigmoid: Missing Deadlines' 6th-order polynomial approximation of the
// reference curve (https://iolite-engine.com/blog_posts/minimal_agx_implementation), over an input
// already normalized to [0, 1] in log space.
fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x
        - 0.00232;
}

// The log2 window AgX encodes into [0, 1]: middle grey 0.18 at LOG2_MIN = -10 and LOG2_MAX = +6.5
// stops, i.e. log2(2^-10 x 0.18) and log2(2^6.5 x 0.18).
const AGX_MIN_EV: f32 = -12.47393;
const AGX_MAX_EV: f32 = 4.026069;

// AgX, in the Filament / three.js `AgXToneMapping` formulation (the standard "minimal AgX": a Rec.2020
// working space, the inset matrix, a log2 encoding of the reference range, the default-contrast
// sigmoid approximation, the outset matrix, then linearize back to Rec.709). Unlike ACES it
// desaturates gracefully toward white the way film does, which is what a night city lit by saturated
// torch light and emissive props needs (plan decision 12).
//
// The matrices are transcribed from three.js's `ToneMappingShader`, whose `mat3(a, b, c)` arguments
// are **columns** — the convention WGSL's `mat3x3<f32>` uses — so they carry over unchanged.
fn agx(color_in: vec3<f32>) -> vec3<f32> {
    let srgb_to_rec2020 = mat3x3<f32>(
        vec3<f32>(0.6274, 0.0691, 0.0164),
        vec3<f32>(0.3293, 0.9195, 0.0880),
        vec3<f32>(0.0433, 0.0113, 0.8956),
    );
    let rec2020_to_srgb = mat3x3<f32>(
        vec3<f32>(1.6605, -0.1246, -0.0182),
        vec3<f32>(-0.5876, 1.1329, -0.1006),
        vec3<f32>(-0.0728, -0.0083, 1.1187),
    );
    let inset = mat3x3<f32>(
        vec3<f32>(0.856627153315983, 0.137318972929847, 0.11189821299995),
        vec3<f32>(0.0951212405381588, 0.761241990602591, 0.0767994186031903),
        vec3<f32>(0.0482516061458583, 0.101439036467562, 0.811302368396859),
    );
    let outset = mat3x3<f32>(
        vec3<f32>(1.1271005818144368, -0.1413297634984383, -0.14132976349843826),
        vec3<f32>(-0.11060664309660323, 1.157823702216272, -0.11060664309660294),
        vec3<f32>(-0.016493938717834573, -0.016493938717834257, 1.2519364065950405),
    );

    var v = srgb_to_rec2020 * color_in;
    v = inset * max(v, vec3<f32>(0.0));
    // log2 of a non-positive value is undefined, so the floor is the encoding's own black point.
    v = log2(max(v, vec3<f32>(1e-10)));
    v = (v - AGX_MIN_EV) / (AGX_MAX_EV - AGX_MIN_EV);
    v = agx_contrast(clamp(v, vec3<f32>(0.0), vec3<f32>(1.0)));
    v = outset * v;
    // The sigmoid's output is display-encoded; 2.2 linearizes it before the gamut conversion back.
    v = pow(max(v, vec3<f32>(0.0)), vec3<f32>(2.2));
    return clamp(rec2020_to_srgb * v, vec3<f32>(0.0), vec3<f32>(1.0));
}

// The Khronos PBR Neutral tone mapper, transcribed from the reference implementation in
// KhronosGroup/ToneMapping. It leaves everything below the compression knee *exactly* as authored and
// desaturates only the highlights, so material colour is preserved wherever a viewer can still judge
// it — which is why it is the glTF viewers' default. Input and output are linear Rec.709.
fn neutral(color_in: vec3<f32>) -> vec3<f32> {
    let start_compression = 0.8 - 0.04;
    let desaturation = 0.15;

    var color = max(color_in, vec3<f32>(0.0));
    let x = min(color.r, min(color.g, color.b));
    var offset = 0.04;
    if (x < 0.08) {
        offset = x - 6.25 * x * x;
    }
    color = color - offset;

    let peak = max(color.r, max(color.g, color.b));
    if (peak < start_compression) {
        return color;
    }
    let d = 1.0 - start_compression;
    let new_peak = 1.0 - d * d / (peak + d - start_compression);
    color = color * (new_peak / peak);
    let g = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(color, vec3<f32>(new_peak), g);
}

fn apply_tonemap(mode: u32, hdr: vec3<f32>) -> vec3<f32> {
    switch mode {
        case TONEMAP_ACES: {
            return aces(hdr);
        }
        case TONEMAP_NEUTRAL: {
            return neutral(hdr);
        }
        // `none` is a genuine skip, not a clamp written here: the value passes through and the
        // `Rgba8UnormSrgb` target's own store clamps it.
        case TONEMAP_NONE: {
            return hdr;
        }
        // `agx`, the shipped operator, is the `default` arm so the path almost every frame takes is
        // the switch's fallthrough rather than the end of a comparison chain.
        default: {
            return agx(hdr);
        }
    }
}

// --- Colour grading -----------------------------------------------------------------------------
//
// The grade is **display-referred**: it runs after the tonemap operator and before the dither. That
// is the AgX workflow (the operator ships muted and expects a look on top — plan decision 12), and it
// is also the tractable place: the three real operators land in [0, 1], so a pivot, a power and a
// chroma mix behave predictably instead of chasing an unbounded HDR range. `none` is the exception
// by design — it is a diagnostic setting that passes HDR straight through, so a grade on top of it
// works on unbounded values and the output target's store does the clamping.
//
// Grading has **no canonical order**, so the order is part of this component's contract and is
// pinned here and in `PostProcess`'s rustdoc:
//
//   1. white balance — `temperature` (blue↔amber), then `tint` (green↔magenta)
//   2. lift / gamma / gain — the ASC CDL shape
//   3. contrast — about a 0.5 pivot
//   4. saturation — a mix toward Rec.709 luma
//
// It is the order a colourist works in: neutralize the light the scene was shot under, set the
// levels, then shape the curve and the chroma over them.

// White balance as a **diagonal channel scale** — a von Kries adaptation performed directly in the
// display RGB space, normalized so a neutral grey keeps its Rec.709 luma exactly:
//
//     gain  = (1 + temperature, 1, 1 - temperature) * (1 - tint, 1 + tint, 1 - tint)
//     gain /= dot(gain, LUMA_709)
//
// Deliberately not a chromatic adaptation transform: a CAT wants a white point taken off the
// Planckian locus and a matrix into LMS (what Unity's `ColorAdjustments` does) to earn a difference
// no eye would find in a display-referred look knob. The normalizer is what keeps the cheap version
// honest — without it, `temperature` would double as a brightness control.
//
// Both factors are exactly `(1, 1, 1)` at 0, so the normalizer is exactly 1 and the whole step is
// exactly neutral. Because the factors are diagonal they commute, so "temperature then tint" is the
// single product below. Over the validated `[-1, 1]` box every component stays non-negative and the
// normalizer never falls below ~0.29 (its minimum, at `temperature = tint = -1`), so it needs no
// divide-by-zero guard.
fn white_balance(color: vec3<f32>, temperature: f32, tint: f32) -> vec3<f32> {
    let amber = vec3<f32>(1.0 + temperature, 1.0, 1.0 - temperature);
    let green = vec3<f32>(1.0 - tint, 1.0 + tint, 1.0 - tint);
    let gain = amber * green;
    return color * gain / dot(gain, LUMA_709);
}

// The floor `gamma` is clamped to before the reciprocal. Zero and negative elements never reach the
// GPU — the component's semantic validator rejects them (`L1130`) — so this is defence in depth
// against a *representable* gamma small enough that its reciprocal overflows: `1e-40` is a legal,
// validating value whose reciprocal is not finite. The floor is far below any usable grade.
const MIN_GAMMA: f32 = 0.0009765625;

// Lift / gamma / gain, the ASC CDL slope-offset-power shape:
//
//     out = pow(max(color * gain + lift, 0), 1 / gamma)
//
// Note the **reciprocal**: a *higher* `gamma` brightens the midtones. That is the convention every
// grading UI uses and the inverse of a bare `pow(x, gamma)`, so it is worth saying twice.
fn lift_gamma_gain(
    color: vec3<f32>,
    lift: vec3<f32>,
    gamma: vec3<f32>,
    gain: vec3<f32>,
) -> vec3<f32> {
    let leveled = max(color * gain + lift, vec3<f32>(0.0));
    // The reciprocal power is the one step inside this block whose identity is not exact in IEEE, so
    // it carries its own skip — decision 9's rule applied one level down, not just at the block.
    if (all(gamma == vec3<f32>(1.0))) {
        return leveled;
    }
    return pow(leveled, vec3<f32>(1.0) / max(gamma, vec3<f32>(MIN_GAMMA)));
}

// Contrast about a pivot of 0.5 — mid-grey in the display-referred space this runs in. Values leave
// [0, 1] at the ends and the output target's store clamps them, which is what a contrast push is
// supposed to do.
fn apply_contrast(color: vec3<f32>, contrast: f32) -> vec3<f32> {
    return (color - vec3<f32>(0.5)) * contrast + vec3<f32>(0.5);
}

// Saturation as a mix between the pixel's Rec.709 luma and its own chroma: 0 is greyscale, 1 is
// unchanged, above 1 extrapolates past the original chroma.
fn apply_saturation(color: vec3<f32>, saturation: f32) -> vec3<f32> {
    return mix(vec3<f32>(dot(color, LUMA_709)), color, saturation);
}

// The grade, in the pinned order above. Each of the four steps is skipped by its own uniform
// comparison, so any subset can be neutral and an all-identity grade returns its input untouched —
// the byte-identity claim, and the reason no step is written as an identity constant.
fn grade(color_in: vec3<f32>) -> vec3<f32> {
    var color = color_in;
    if (composite.temperature != 0.0 || composite.tint != 0.0) {
        color = white_balance(color, composite.temperature, composite.tint);
    }
    if (any(composite.lift != vec3<f32>(0.0))
        || any(composite.gamma != vec3<f32>(1.0))
        || any(composite.gain != vec3<f32>(1.0))) {
        color = lift_gamma_gain(color, composite.lift, composite.gamma, composite.gain);
    }
    if (composite.contrast != 1.0) {
        color = apply_contrast(color, composite.contrast);
    }
    if (composite.saturation != 1.0) {
        color = apply_saturation(color, composite.saturation);
    }
    return color;
}

// --- Vignette ---------------------------------------------------------------------------------
//
// Step 8, and display-referred: a vignette is the lens and the aperture darkening the *finished*
// image, so it runs after the operator and the grade, with only the grain and the dither behind it.
//
// Its geometry is a function of the normalized screen position and nothing else — no pixels, no
// aspect correction — which is the arc's standing screen-relative rule, and here it makes the
// darkening identical at any resolution and any render scale. The deliberate consequence is that the
// shape is the ellipse inscribed in the frame rather than a circle: the vignette follows the format
// instead of being cropped by it.
//
//     d   = length((uv - 0.5) * 2)          // 0 at the centre, 1 at an edge midpoint, √2 at a corner
//     out = colour * (1 - vignette * smoothstep(vignette_radius, √2, d))
//
// One shape knob, and it is the inner edge. The outer edge needs none because the frame supplies it —
// the corner, at √2 in these units — so the band is fully determined by where the darkening starts,
// and a `vignette_smoothness` would be a second spelling of the same number. `vignette_radius` is
// validated to `[0, 1]`, which keeps the band non-degenerate (the corner is strictly further out)
// while still reaching from dead centre to the edge midpoints.
const CORNER_DISTANCE: f32 = 1.4142135;

fn apply_vignette(color: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    let d = length((uv - vec2<f32>(0.5)) * 2.0);
    let falloff = smoothstep(composite.vignette_radius, CORNER_DISTANCE, d);
    return color * (1.0 - composite.vignette * falloff);
}

// --- Film emulation -------------------------------------------------------------------------------
//
// Steps 0, 2 and 9 — the three effects that model the camera rather than the scene (plan decision
// 13). All three are pure per-pixel functions of colour this pass has already sampled, so all three
// fold in here by decision 2 and none of them adds a graph pass.

// Transverse (lateral) chromatic aberration: a lens focuses short and long wavelengths at slightly
// different **magnifications**, so the three channels are imaged at slightly different scales. That
// is a radial displacement — zero on the optical axis, growing toward the corner — and never a
// constant offset, which would be longitudinal CA and reads as a focus error rather than as a lens.
//
//     p     = (uv - 0.5) * 2         // 0 at the centre, 1 at an edge midpoint, √2 at a corner
//     red   = scene(uv - chromatic_aberration * p)
//     green = scene(uv)
//     blue  = scene(uv + chromatic_aberration * p)
//
// So the authored number is **the displacement at an edge midpoint, in normalized UV**: at 0.002,
// red is fetched from 0.2% of the frame nearer the centre there and blue from 0.2% further out,
// growing to 0.28% at the corner and falling to exactly nothing at the centre. Normalized UV and
// nothing else is the arc's screen-relative rule, so the fringe is identical at any
// resolution and any render scale. The deliberate consequence is the vignette's: because UV is
// normalized per *axis*, the displacement is a fixed fraction of the width horizontally and of the
// height vertically, so on a wide frame it is more pixels sideways than up — the aberration follows
// the format, exactly as the vignette's inscribed ellipse does.
//
// Red in and blue out is the ordinary direction for crown glass: shorter wavelengths refract more, so
// the blue image is the larger one. Green is left on the sensor's own grid, which is also what makes
// a neutral value exactly the fetch it replaces.
//
// **Only the scene fetch is displaced.** Bloom, the shaft term and the defocus layer are all
// low-frequency by construction — a deliberately blurred field offset by a few pixels is the same
// field — and the fog term reads the pixel's *own* depth, so displacing that would fog a surface
// which is at a different pixel. Skipping them saves four texture fetches per pixel and costs nothing
// an eye can find. It is the call the depth-of-field pass makes when it places the depth-of-field blend by the gather's
// *input* rather than by the optical path: what an effect reads decides where it applies.
//
// The shared sampler clamps to edge, so a displaced fetch past the border repeats the border texel
// rather than wrapping the frame in from the far side.
fn chromatic_scene(uv: vec2<f32>) -> vec3<f32> {
    let shift = (uv - vec2<f32>(0.5)) * 2.0 * composite.chromatic_aberration;
    let red = hdr_safe(textureSample(scene_tex, samp, uv - shift).rgb).r;
    let green = hdr_safe(textureSample(scene_tex, samp, uv).rgb).g;
    let blue = hdr_safe(textureSample(scene_tex, samp, uv + shift).rgb).b;
    return vec3<f32>(red, green, blue);
}

// Halation's colour, and it is a **constant rather than a field**.
//
// Halation is light that passes through the emulsion, reflects off the film base behind it and
// scatters back into the layers around a bright source. Its warm red-orange is a property of the
// *stock* — of how far back the base sits and which layer the returning light still reaches — not of
// the shot, so a constant is the faithful model, not the cheap one.
//
// It is also the Henyey–Greenstein `g` argument, which is the decisive half: what an authorable
// `halation_color` would actually control is achromatic glow versus chromatic glow, and achromatic
// glow is exactly what `bloom_strength` already spells. A colour field would be a second way to write
// bloom, and the first way is not going anywhere.
const HALATION_TINT: vec3<f32> = vec3<f32>(1.0, 0.35, 0.12);

// The tint **normalized by Rec.709 luma**, which is `white_balance`'s trick applied to the same
// hazard. The raw tint's luma is 0.47, so an unnormalized `halation` would be a hue knob and a dimmer
// at once: `halation: 0.6` would add less than half the light `bloom_strength: 0.6` does, and an
// author trading one for the other would watch the image darken for a reason they did not author.
// Normalized, `halation` is a strength on exactly bloom's scale and only the hue differs.
fn halation_gain() -> vec3<f32> {
    return HALATION_TINT / dot(HALATION_TINT, LUMA_709);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Step 0: the aberration is in the fetch itself, not applied to a colour afterwards — three
    // radially-displaced channel taps, or the one plain fetch when it is off.
    var scene: vec3<f32>;
    if (composite.chromatic_aberration != 0.0) {
        scene = chromatic_scene(in.uv);
    } else {
        // Every reader of `scene` clamps: the opaque pass and `reflections` each store up to the
        // ceiling, and their sum may not fit half-float.
        scene = hdr_safe(textureSample(scene_tex, samp, in.uv).rgb);
    }
    let bloom = textureSample(bloom_tex, samp, in.uv).rgb;
    // Depth of field: the gather's defocus layer, composited **over** the sharp scene.
    //
    // The layer arrives premultiplied by the coverage it claims — a pixel's own defocus, or the share
    // of the aperture a nearer blurred surface threw across it — so this is the over operator, spelled
    // as the `mix` its premultiplied form is equal to. That is what makes the bilinear upscale below
    // correct across a silhouette instead of a source of halos (see `dof.wgsl`); it is also why the
    // circle of confusion is computed once, in the depth-of-field chain, and never again here.
    //
    // Sampled, not loaded, because the layer lives at a fraction of the output extent while this pass
    // runs at the render extent — and a deliberately blurred field is exactly what wants a bilinear
    // upscale.
    if (composite.dof != 0u) {
        let defocused = textureSample(dof_tex, samp, in.uv);
        // A blurred near-world foreground claims gun pixels through its coverage; the gun stays sharp.
        var is_gun = false;
        if (composite.viewmodel_slice > 0.0) {
            is_gun = textureLoad(scene_depth_tex, vec2<i32>(in.pos.xy), 0) < composite.viewmodel_slice;
        }
        if (!is_gun) {
            scene = mix(scene, defocused.rgb / max(defocused.a, 1e-4), defocused.a);
        }
    }
    var hdr = scene + bloom * composite.strength;
    // Halation reuses the blurred bright field `bloom_tex` already holds rather than blurring again,
    // which is what makes it near-free and is also the recorded consequence: it inherits
    // `bloom_threshold` and the bloom chain's radius, and cannot be tuned apart from them. Trigger
    // for its own chain: a look verdict at the arc's closing sitting.
    if (composite.halation != 0.0) {
        hdr = hdr + bloom * halation_gain() * composite.halation;
    }
    if (composite.fog_density > 0.0) {
        // The transparent layer: sprites, particles and world text, which write no depth, hold their
        // own mean distance and coverage here. Under one the pixel is fogged at that distance, not at
        // whatever the opaque pass left behind it.
        var coverage = 0.0;
        var layer_t = 0.0;
        if (composite.transparent_depth != 0u) {
            let layer = textureLoad(transparent_depth_tex, vec2<i32>(in.pos.xy), 0);
            coverage = layer.g;
            layer_t = layer.r / max(layer.g, 1e-4);
        }
        // The medium's ambient radiance: the authored slab, the sky along this pixel's view ray, or a
        // blend. The ray is reconstructed at the **far plane** because a ray's direction does not
        // depend on how far along it a surface sits — `shafts.wgsl` reconstructs its own the same way.
        //
        // This sits outside the depth branch below, and that placement is measured rather than
        // stylistic: the radiance is the same for a pixel whatever its depth, and computing it *inside*
        // the branch — between the depth load and the blend that consumes it — perturbs how the
        // backend schedules the rest of the fragment enough to move a pixel of `post-full` by one
        // output step, with the branch never even taken. Out here the neutral frame's arithmetic is
        // untouched and the byte-identity claim holds. The layer load above sits ahead of it for the same
        // measured reason.
        var ambient = composite.fog_color;
        if (composite.fog_sky > 0.0) {
            let view_dir = normalize(view_from_eye(
                world_from_depth(in.uv, 1.0) - composite.cam_pos,
                composite.view_forward,
            ));
            ambient = mix(
                ambient,
                sky_gradient(
                    view_dir,
                    composite.sky_zenith,
                    composite.sky_horizon,
                    composite.sky_ground,
                ),
                composite.fog_sky,
            );
        }
        // The GI-weighted mean of the ambient along this ray (`medium_scatter.wgsl`'s alpha), 1 where the
        // volume lets the sky through. Beside the ambient it scales, outside the depth branch below.
        if (composite.medium_gi.x != 0u) {
            ambient = ambient * textureSample(medium_scatter_tex, samp, in.uv).a;
        }
        let depth = textureLoad(scene_depth_tex, vec2<i32>(in.pos.xy), 0);
        // The medium does not apply where nothing was drawn. A pixel still at the far plane is the
        // background — the clear colour, or an `Environment` sky, which the scene pass fills at
        // exactly far depth without writing it — and the sky is at infinite distance, so the
        // extinction would replace it outright, flattening an authored gradient into a slab of fog
        // colour. The corollary for an author is what `fog_sky` answers: at 0 the haze is one slab and
        // the horizon reads continuous only if `fog_color` is picked near `Environment.sky_horizon`,
        // while at 1 the haze *is* the sky along that ray and the two meet by construction.
        if (coverage > 0.0) {
            // One fog is applied to the already blended image, so a layer of weight `a` over a
            // background takes the mixed extinction `mix(F_background, F_layer, a)`. The in-scatter
            // term is exact; the transmitted colour is off by `a(1-a)(S-B)(F_s-F_b)`, zero for an
            // opaque or absent layer and at most a quarter of `|S-B||F_s-F_b|` on a half-covered
            // texel. The background's own share is zero over the sky, which the medium never touches.
            var back = 0.0;
            if (depth < 1.0) {
                back = fog_factor(world_from_depth(in.uv, depth));
            }
            let extinguished = mix(back, fog_factor(layer_world(in.uv, layer_t)), coverage);
            if (composite.fog_albedo == 1.0) {
                hdr = mix(hdr, ambient, extinguished);
            } else {
                hdr = hdr * (1.0 - extinguished)
                    + ambient * (composite.fog_albedo * extinguished);
            }
        } else if (depth < 1.0) {
            // The ambient in-scatter, in its closed form: `T·surface + α·(1 - T)·L_ambient`, with
            // `1 - T` the extinguished fraction `fog_factor` returns. The extinction is σ_t's alone —
            // a surface behind an absorbing medium is buried whether or not that medium glows — and
            // only the in-scattered radiance carries α.
            //
            // A purely scattering medium keeps the `mix` intrinsic rather than the algebraically equal
            // sum below it. `mix` lowers to each backend's own blend (HLSL `lerp` is `x + s*(y - x)`,
            // which is not the same bits as `x*(1 - s) + y*s`), so the shipped fog's bytes live in the
            // intrinsic, not in the algebra — decision 9's skip-don't-compute rule applied one level
            // down, exactly as `lift_gamma_gain` guards its own reciprocal power.
            let extinguished = fog_factor(world_from_depth(in.uv, depth));
            if (composite.fog_albedo == 1.0) {
                hdr = mix(hdr, ambient, extinguished);
            } else {
                hdr = hdr * (1.0 - extinguished)
                    + ambient * (composite.fog_albedo * extinguished);
            }
        }
    }
    // The medium's directional term: in-scattered sun light added on top of what survived the
    // extinction, tinted by the sun's own colour. The march already folded in σ_s and the phase, so
    // there is nothing left to scale here. The term is sampled (not loaded) because it lives at a
    // fraction of the output extent while this pass runs at the render extent — a bilinear upscale of
    // an already-blurred, deliberately low-frequency field.
    if (composite.shafts != 0u) {
        let shafts = textureSample(shafts_tex, samp, in.uv).r;
        hdr = hdr + composite.shafts_color * shafts;
    }
    // The local lights' directional term: already coloured and scaled by the march, so a plain add.
    if (composite.medium_scatter != 0u) {
        hdr = hdr + textureSample(medium_scatter_tex, samp, in.uv).rgb;
    }
    if (composite.exposure != 1.0) {
        hdr = hdr * composite.exposure;
    }
    var color = grade(apply_tonemap(composite.tonemap, hdr));
    if (composite.vignette != 0.0) {
        color = apply_vignette(color, in.uv);
    }
    // Grain then dither, in one encode/decode trip (zeroed when a later pass owns them). Dither stays strictly last within it: it exists
    // to hide the *output* quantization, so anything after it would re-quantize what it just spread.
    if (composite.grain != 0.0 || composite.dither != 0u) {
        color = encoded_output(
            color,
            in.pos.xy,
            composite.grain,
            composite.grain_size,
            composite.dither,
            f32(textureDimensions(scene_tex).y),
        );
    }
    return vec4<f32>(color, 1.0);
}
