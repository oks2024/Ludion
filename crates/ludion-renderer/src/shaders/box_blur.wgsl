// A 4×4 box filter over one scalar field, at the source's own extent — the engine's single
// Bayer-period blur, shared by every effect whose estimator is dithered with `BAYER4`.
//
// The size is not a quality dial: it is the **period** of that tile. Every 4×4 block of a
// `BAYER4`-dithered target carries each of the sixteen offsets exactly once, so averaging four
// consecutive texels per axis averages one full period and the pattern cancels *exactly*, leaving
// the estimate and no stipple. A Gaussian, or any other radius, would leave residue. That argument
// is why both consumers dither with Bayer rather than with an aperiodic hash, so both want this
// filter and not merely a filter — one shader, two differently-named graph passes (a pass name is
// what a budget and `--dump-timings` attribute a cost to, so those stay distinct).
//
// Its consumers:
//
//   - `ssao_blur`   — cancels the SSAO estimator's per-pixel hemisphere rotation.
//   - `shafts_blur` — cancels the light-shaft march's per-pixel step offset.
//
// Both write a scalar into every channel, so the filter reads `.r` and broadcasts the mean.
//
// It is a plain box rather than a depth-aware bilateral filter: sixteen unweighted taps at half
// resolution, over quantities that are then multiplied into the ambient term or added as a soft
// glow, so the bleed across a silhouette is a fraction of a fraction. A bilateral variant would need
// a depth target as a second input and a falloff to tune, for a difference neither effect's role
// pays for.
//
// The taps go through the shared filtering sampler at exact integer texel offsets, which lands each
// one on a texel centre and makes the bilinear fetch exact.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl` (dec. 16).

struct BoxBlurParams {
    // One texel of this pass's target, in UV. Source and target share an extent, so it is also one
    // source texel.
    texel: vec4<f32>,
};

@group(0) @binding(0) var box_src: texture_2d<f32>;
@group(0) @binding(1) var box_samp: sampler;
@group(0) @binding(2) var<uniform> box_params: BoxBlurParams;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var sum = 0.0;
    for (var y = -2; y <= 1; y = y + 1) {
        for (var x = -2; x <= 1; x = x + 1) {
            let offset = vec2<f32>(f32(x), f32(y)) * box_params.texel.xy;
            sum = sum + textureSampleLevel(box_src, box_samp, in.uv + offset, 0.0).r;
        }
    }
    return vec4<f32>(sum / 16.0);
}
