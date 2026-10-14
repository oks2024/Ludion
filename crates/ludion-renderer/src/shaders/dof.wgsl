// The depth-of-field gather: one disc of taps over the prefilter's colour+CoC target, producing the
// defocused image and the coverage the composite blends it in with
// (Plan/post-process-pipeline.md).
//
// # One pass, not a near pass and a far pass
//
// A near/far split might be expected, and the plan's MRT deferral names this gather as
// the next candidate for a pass writing two colour targets. It is not one. What a split exists to fix
// is that a foreground must spread *outward* over its neighbours while a background must not spread
// *inward* over a sharp subject — and the scatter-as-gather weight below already decides both from
// one number:
//
//     a tap at offset `o` reaches this pixel only if its own disc covers it,  |coc(q)| >= |o|
//
// A sharp foreground has `coc ~ 0`, so it scatters nowhere and cannot smear onto the background
// behind it; a blurred foreground has a large `|coc|` and reaches out over whatever is behind it,
// which is what an out-of-focus near object does. So the sign of the CoC never has to select a pass —
// it only has to select an occlusion rule, and that is one line. Two targets would buy nothing, so
// the MRT trigger stays where the SSAO pass left it: **the first pass that genuinely writes two colour
// targets.**
//
// # The occlusion rule
//
// The disc test alone would let a blurred *background* wash over a sharp subject in front of it, and
// the subject's own surface is what stops that in reality. Since the circle of confusion is monotone
// in depth, comparing two CoCs compares two depths, so the rule needs no second fetch:
//
//     a tap *behind* this pixel contributes only in proportion to `|coc(p)| / |coc(q)|`
//
// which is zero when this pixel is in focus (nothing shows through a sharp surface), one when the two
// are equally defocused (they mix freely), and in between when this pixel is partly defocused and the
// background is partly visible through its own blur. A tap *in front* is never capped — spreading
// over what is behind it is exactly what an out-of-focus foreground does.
//
// # The kernel, and why every fetch is a `textureLoad`
//
// A Vogel (golden-angle) spiral: `r = sqrt((i + 0.5)/N)`, `theta = i * GOLDEN_ANGLE`. `sqrt` makes the
// samples uniform in **area** rather than in radius, so the disc is evenly covered, and the golden
// angle is the disc analogue of the argument for Roberts' R2 over Hammersley — it is well
// distributed at *any* prefix length, so lowering `dof_params.taps` cannot quietly leave a lobe
// unsampled.
// The radius is `PostProcess.dof` in fractions of the frame's **height**, converted to this target's
// own texels, so the disc is a circle on screen at any resolution and any render scale, never a fixed
// count of source texels (the screen-relative rule).
//
// Every tap is an **exact texel**, not a bilinear sample, and that is load-bearing rather than a
// micro-optimisation. This texture's alpha is a circle of confusion, and a bilinear tap near a
// silhouette returns the *average* of a sharp surface's CoC and a defocused one's — a number no
// surface in the scene has. The disc test would then admit that tap, and with it a share of the sharp
// surface's colour, which is how a sharp subject bleeds into the defocused background around it. A
// filtered colour is worth nothing here anyway: the whole pass is an average of a disc of them.
// The pass still declares the shared sampler, because the binding convention gives every fullscreen
// effect one (`resolve.wgsl` and `shafts.wgsl` declare one they never call for the same reason).
//
// # Determinism, and why there is no blur pass behind this one
//
// Nothing here reads a clock, a frame index or a random number generator, and — unlike the SSAO
// estimator and the shaft march — nothing here is dithered either. Those two are stochastic integrals
// of a few samples and trade per-pixel noise for a wider effective kernel, which is why each needs
// `box_blur.wgsl` to cancel its `BAYER4` period. This is a *filter*: a disc of taps of a fixed
// spiral over a colour field, already smooth by construction. Rotating it per pixel would buy nothing
// a fixed spiral does not already have and would then have to be blurred back out — and a 4x4 box on
// top of a defocus is a floor under how small the blur can be, which is precisely the artefact a
// depth of field must not have near the focal plane.
//
// `VsOut` + `vs_main` come from `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of this body
// (Plan/shader-bindgen.md decision 16).

struct Dof {
    // The disc's radius: the far field's circle of confusion, a fraction of the frame's height
    // (`PostProcess.dof`).
    max_coc: f32,
    // One target texel as a fraction of the frame's height. It converts the radius into texels, and it
    // is the width of the soft edge of a tap's disc.
    texel: f32,
    // Taps in the disc, this pixel's own centre tap aside (`QualityParams.dof_taps`). A **quality**
    // setting, not a look one, on the arc's standing split between what the client pays for and what
    // the scene authors (`ssao.wgsl`'s `samples` and `shafts.wgsl`'s `steps` make the same call). At
    // full quality it is thirty-two, where a Vogel spiral stops showing its arms in a strong bokeh at
    // half the output extent.
    taps: u32,
    pad0: f32,
};

// The prefilter's output: rgb the area-averaged scene, a the signed circle of confusion.
@group(0) @binding(0) var dof_src: texture_2d<f32>;
// Declared but never sampled: every fullscreen effect binds textures 0..N, a sampler at N and its
// uniform at N+1 (see `FrameRecorder::fullscreen_effect`), and this pass fetches exact texels.
@group(0) @binding(1) var dof_samp: sampler;
@group(0) @binding(2) var<uniform> dof_params: Dof;

// The golden angle, `pi * (3 - sqrt(5))` — the rotation between successive spiral samples that leaves
// no direction preferred at any sample count.
const DOF_GOLDEN_ANGLE: f32 = 2.39996322972865332;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let last = vec2<i32>(textureDimensions(dof_src)) - vec2<i32>(1);
    let here = clamp(vec2<i32>(floor(in.pos.xy)), vec2<i32>(0), last);
    let center = textureLoad(dof_src, here, 0);
    let coc_center = center.a;
    let softness = max(dof_params.texel, 1e-6);

    // This pixel's own defocus, as a coverage: it scatters onto itself in proportion to how wide its
    // own circle of confusion is, reaching a full share once that is one target texel across. An
    // in-focus pixel therefore contributes almost nothing to this layer — which is exactly right,
    // because the composite has the full-resolution sharp pixel and will keep it.
    let own = clamp(abs(coc_center) / softness, 0.0, 1.0);
    var sum = center.rgb * own;
    var weight = own;
    var near = 0.0;

    for (var i = 0u; i < dof_params.taps; i = i + 1u) {
        let radius = sqrt((f32(i) + 0.5) / f32(dof_params.taps)) * dof_params.max_coc;
        let angle = f32(i) * DOF_GOLDEN_ANGLE;
        let step = vec2<f32>(cos(angle), sin(angle)) * (radius / softness);
        let tap = textureLoad(
            dof_src,
            clamp(here + vec2<i32>(round(step)), vec2<i32>(0), last),
            0,
        );
        let coc_tap = tap.a;

        // Scatter as gather: does this tap's own disc reach here? The edge is softened over one target
        // texel so a disc growing across the frame does not step. The second factor is the same
        // question asked of the tap alone — a surface in focus scatters nowhere, so it may not join
        // this layer even at zero offset.
        var w = clamp((abs(coc_tap) - radius) / softness + 1.0, 0.0, 1.0)
            * clamp(abs(coc_tap) / softness, 0.0, 1.0);
        if (coc_tap > coc_center) {
            w = w * clamp(abs(coc_center) / max(abs(coc_tap), 1e-6), 0.0, 1.0);
        }
        // A tap nearer than the focal plane is foreground; how much of the aperture such taps cover is
        // what lets a blurred foreground claim a pixel whose own surface is perfectly sharp.
        if (coc_tap < 0.0) {
            near = near + w;
        }
        sum = sum + tap.rgb * w;
        weight = weight + w;
    }

    // The output is a **defocus layer**, premultiplied by the coverage it claims, which the composite
    // puts over the full-resolution sharp scene. Premultiplied because the composite reads this target
    // bilinearly: filtering a straight (colour, coverage) pair mixes colours a neighbour never had at
    // a coverage it never claimed, and a sharp silhouette against a defocused background halos. Under
    // premultiplication the interpolation *is* the over operator.
    //
    // `coverage` is the fraction of the aperture a nearer, blurred surface covered — the term that
    // lets a foreground claim a pixel whose own surface is sharp, and the one part of the blend that
    // genuinely should filter across a silhouette, because an out-of-focus edge is soft.
    let coverage = clamp(near / f32(dof_params.taps), 0.0, 1.0);
    let alpha = clamp(max(own, coverage), 0.0, 1.0);
    return vec4<f32>(sum / max(weight, 1e-4) * alpha, alpha);
}
