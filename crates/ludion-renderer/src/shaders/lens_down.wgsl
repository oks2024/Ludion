// Anamorphic streak downsample: halves the previous level along x only, so the streak chain keeps
// the full vertical resolution of the bloom level it starts from and blurs nothing vertically.
//
// Four bilinear taps at {-1.5, -0.5, 0.5, 1.5} source texels, weighted {1, 3, 3, 1} / 8 — a
// binomial row, one source texel apart, sampled where the target texel's centre lands between
// source texels. Offsets are in *source* texel units (`lens_down_params.texel`), so the reach is a
// fixed fraction of the frame at any render scale.
//
// `lens_down_params.texel.z` is an extra threshold taken off the source per channel; only the first
// level of the chain sets it, which is what makes the streak's own threshold a separate knob from
// the bloom's.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of
// this body (Plan/shader-bindgen.md decision 16).

struct LensDownParams {
    // xy = one source texel in UV (1 / source extent).
    // z = extra threshold (0.0 = none). w unused.
    texel: vec4<f32>,
};

@group(0) @binding(0) var lens_down_src: texture_2d<f32>;
@group(0) @binding(1) var lens_down_samp: sampler;
@group(0) @binding(2) var<uniform> lens_down_params: LensDownParams;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let step = vec2<f32>(lens_down_params.texel.x, 0.0);
    let uv = in.uv;

    var acc = textureSample(lens_down_src, lens_down_samp, uv - step * 1.5).rgb;
    acc = acc + textureSample(lens_down_src, lens_down_samp, uv - step * 0.5).rgb * 3.0;
    acc = acc + textureSample(lens_down_src, lens_down_samp, uv + step * 0.5).rgb * 3.0;
    acc = acc + textureSample(lens_down_src, lens_down_samp, uv + step * 1.5).rgb;
    acc = acc * 0.125;

    let cut = lens_down_params.texel.z;
    if cut > 0.0 {
        acc = max(acc - vec3<f32>(cut), vec3<f32>(0.0));
    }
    return vec4<f32>(hdr_safe(acc), 1.0);
}
