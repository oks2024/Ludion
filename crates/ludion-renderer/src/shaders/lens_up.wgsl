// Anamorphic streak upsample: a horizontal 3-tap tent over the coarser level, blended into this
// level's own downsample result.
//
// The tent taps sit one coarse texel either side ({1, 2, 1} / 4), which is wide enough that bilinear
// magnification leaves no level-boundary creases. `lens_up_params.texel.z` is the stretch: how much of
// the coarse level replaces the fine one at this step, so `0` keeps only the first level's short
// smear and `1` carries the coarsest level's energy the whole way up the chain.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of
// this body (Plan/shader-bindgen.md decision 16).

struct LensUpParams {
    // xy = one coarse-level texel in UV (1 / coarse extent).
    // z = stretch in [0, 1]. w unused.
    texel: vec4<f32>,
};

@group(0) @binding(0) var lens_up_coarse: texture_2d<f32>;
@group(0) @binding(1) var lens_up_fine: texture_2d<f32>;
@group(0) @binding(2) var lens_up_samp: sampler;
@group(0) @binding(3) var<uniform> lens_up_params: LensUpParams;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let step = vec2<f32>(lens_up_params.texel.x, 0.0);
    let uv = in.uv;

    var coarse = textureSample(lens_up_coarse, lens_up_samp, uv - step).rgb;
    coarse = coarse + textureSample(lens_up_coarse, lens_up_samp, uv).rgb * 2.0;
    coarse = coarse + textureSample(lens_up_coarse, lens_up_samp, uv + step).rgb;
    coarse = coarse * 0.25;

    let fine = textureSample(lens_up_fine, lens_up_samp, uv).rgb;
    return vec4<f32>(hdr_safe(mix(fine, coarse, lens_up_params.texel.z)), 1.0);
}
