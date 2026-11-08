// A 4×4 box filter over all four channels of the local-light in-scatter term — `box_blur.wgsl`'s
// kernel, which averages one full `BAYER4` period so the march's step offset cancels exactly. That
// shader broadcasts a scalar from `.r`; this term is an RGB radiance (and the alpha the march writes
// is a constant 1, which a mean preserves exactly), so it keeps its own copy rather than widening the
// scalar blur, whose bytes are blessed.
//
// `VsOut` + `vs_main` come from `shaders/snippets/fullscreen_vs.wgsl`.

struct ScatterBlurParams {
    // One texel of this pass's target, in UV (source and target share an extent).
    texel: vec4<f32>,
};

@group(0) @binding(0) var scatter_blur_src: texture_2d<f32>;
@group(0) @binding(1) var scatter_blur_samp: sampler;
@group(0) @binding(2) var<uniform> scatter_blur_params: ScatterBlurParams;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var sum = vec4<f32>(0.0);
    for (var y = -2; y <= 1; y = y + 1) {
        for (var x = -2; x <= 1; x = x + 1) {
            let offset = vec2<f32>(f32(x), f32(y)) * scatter_blur_params.texel.xy;
            sum = sum + textureSampleLevel(scatter_blur_src, scatter_blur_samp, in.uv + offset, 0.0);
        }
    }
    return sum / 16.0;
}
