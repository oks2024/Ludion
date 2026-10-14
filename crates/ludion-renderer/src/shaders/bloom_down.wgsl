// Bloom pyramid downsample: the 13-tap progressive filter that halves the previous level.
//
// Thirteen taps resolve into five overlapping 2x2 boxes — the centre box plus one per corner —
// combined 0.5 / 0.125x4. That footprint is why a pyramid built from it has no level-boundary
// blockiness: consecutive levels overlap instead of tiling. Every offset is in *target* texel units
// (`down_params.texel`), never source texels, so the filter covers the same fraction of the frame at
// any render scale.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of
// this body (Plan/shader-bindgen.md decision 16).

struct DownsampleParams {
    // xy = one target texel in UV (1 / target extent).
    // z = 1.0 to Karis-average the five boxes, 0.0 to weight them plainly. w unused.
    texel: vec4<f32>,
};

@group(0) @binding(0) var down_src: texture_2d<f32>;
@group(0) @binding(1) var down_samp: sampler;
@group(0) @binding(2) var<uniform> down_params: DownsampleParams;

// Inverse-luma weight: a box holding one blindingly bright pixel counts for less, so a lone hot
// pixel cannot survive as a firefly through the whole chain. Applied on the first downsample only —
// past that the energy is already spread over many texels and the weighting would only dim the glow.
fn karis_weight(c: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = down_params.texel.xy;
    let uv = in.uv;

    // The 3x3 outer ring, one target texel apart.
    let a = textureSample(down_src, down_samp, uv + vec2<f32>(-1.0, -1.0) * t).rgb;
    let b = textureSample(down_src, down_samp, uv + vec2<f32>(0.0, -1.0) * t).rgb;
    let c = textureSample(down_src, down_samp, uv + vec2<f32>(1.0, -1.0) * t).rgb;
    let d = textureSample(down_src, down_samp, uv + vec2<f32>(-1.0, 0.0) * t).rgb;
    let e = textureSample(down_src, down_samp, uv).rgb;
    let f = textureSample(down_src, down_samp, uv + vec2<f32>(1.0, 0.0) * t).rgb;
    let g = textureSample(down_src, down_samp, uv + vec2<f32>(-1.0, 1.0) * t).rgb;
    let h = textureSample(down_src, down_samp, uv + vec2<f32>(0.0, 1.0) * t).rgb;
    let i = textureSample(down_src, down_samp, uv + vec2<f32>(1.0, 1.0) * t).rgb;
    // The four inner taps, half a target texel apart.
    let j = textureSample(down_src, down_samp, uv + vec2<f32>(-0.5, -0.5) * t).rgb;
    let k = textureSample(down_src, down_samp, uv + vec2<f32>(0.5, -0.5) * t).rgb;
    let l = textureSample(down_src, down_samp, uv + vec2<f32>(-0.5, 0.5) * t).rgb;
    let m = textureSample(down_src, down_samp, uv + vec2<f32>(0.5, 0.5) * t).rgb;

    // Each box is sanitised before it is weighted: a lone inf/NaN texel would otherwise turn the
    // Karis weight into 0 * inf and poison the whole pyramid.
    var boxes = array<vec3<f32>, 5>(
        hdr_safe((j + k + l + m) * 0.25),
        hdr_safe((a + b + d + e) * 0.25),
        hdr_safe((b + c + e + f) * 0.25),
        hdr_safe((d + e + g + h) * 0.25),
        hdr_safe((e + f + h + i) * 0.25),
    );
    var weights = array<f32, 5>(0.5, 0.125, 0.125, 0.125, 0.125);

    // Weights sum to 1, so the plain branch is exactly the standard filter; the Karis branch
    // renormalises because inverse-luma weighting is not energy-preserving on its own.
    let karis = down_params.texel.z > 0.5;
    var acc = vec3<f32>(0.0);
    var total = 0.0;
    for (var n = 0; n < 5; n = n + 1) {
        var w = weights[n];
        if karis {
            w = w * karis_weight(boxes[n]);
        }
        acc = acc + boxes[n] * w;
        total = total + w;
    }
    return vec4<f32>(acc / total, 1.0);
}
