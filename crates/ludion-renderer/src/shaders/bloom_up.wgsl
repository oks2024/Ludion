// Bloom pyramid upsample: a 3x3 tent filter over the coarser accumulation, added to this level's own
// downsample result. Running it once per level is what turns the pyramid into one wide, smooth glow
// rather than six visibly separate blurs.
//
// The tent's taps sit two target texels out, i.e. one texel of the coarse level being magnified —
// wide enough that bilinear magnification leaves no level-boundary creases. Like the downsample, the
// offsets are in *target* texel units, so the footprint is a fixed fraction of the frame.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of
// this body (Plan/shader-bindgen.md decision 16).

struct UpsampleParams {
    // xy = one target texel in UV (1 / target extent); zw unused.
    texel: vec4<f32>,
};

@group(0) @binding(0) var up_coarse: texture_2d<f32>;
@group(0) @binding(1) var up_fine: texture_2d<f32>;
@group(0) @binding(2) var up_samp: sampler;
@group(0) @binding(3) var<uniform> up_params: UpsampleParams;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let s = up_params.texel.xy * 2.0;
    let uv = in.uv;

    var acc = textureSample(up_coarse, up_samp, uv + vec2<f32>(-1.0, -1.0) * s).rgb;
    acc = acc + textureSample(up_coarse, up_samp, uv + vec2<f32>(0.0, -1.0) * s).rgb * 2.0;
    acc = acc + textureSample(up_coarse, up_samp, uv + vec2<f32>(1.0, -1.0) * s).rgb;
    acc = acc + textureSample(up_coarse, up_samp, uv + vec2<f32>(-1.0, 0.0) * s).rgb * 2.0;
    acc = acc + textureSample(up_coarse, up_samp, uv).rgb * 4.0;
    acc = acc + textureSample(up_coarse, up_samp, uv + vec2<f32>(1.0, 0.0) * s).rgb * 2.0;
    acc = acc + textureSample(up_coarse, up_samp, uv + vec2<f32>(-1.0, 1.0) * s).rgb;
    acc = acc + textureSample(up_coarse, up_samp, uv + vec2<f32>(0.0, 1.0) * s).rgb * 2.0;
    acc = acc + textureSample(up_coarse, up_samp, uv + vec2<f32>(1.0, 1.0) * s).rgb;

    let fine = textureSample(up_fine, up_samp, uv).rgb;
    return vec4<f32>(hdr_safe(acc * (1.0 / 16.0) + fine), 1.0);
}
