// A full-screen view of the SSR trace (`ludion render --debug-view`), drawn in place of the finished
// picture. It reads the trace texel nearest the render pixel — honest per-trace-texel data, no
// upsample — and writes a gray the sRGB target stores as `round(value * 255)`, so
// `ludion image-stats` reads the data directly.
//
// mode.x = 0: confidence (the trace's alpha). mode.x = 1: footprint — the pyramid level the trace
// read, over `mode.y - 1`; magenta where the trace has no confident hit (a miss, a
// back-face or past the fade band), so it is not level 0.
//
// `VsOut` + `vs_main` come from `snippets/fullscreen_vs.wgsl`; the transfer functions from
// `snippets/srgb.wgsl`.

struct SsrDebug {
    // x = view (0 confidence, 1 footprint), y = the colour pyramid's level count.
    mode: vec4<f32>,
}

@group(0) @binding(0) var dbg_trace: texture_2d<f32>;
// Declared but never sampled: exact `textureLoad`s only.
@group(0) @binding(1) var dbg_samp: sampler;
@group(0) @binding(2) var<uniform> dbg_params: SsrDebug;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let trace = vec2<f32>(textureDimensions(dbg_trace));
    // `in.uv` is the pixel centre over the render extent, so this is the trace texel under the pixel.
    let texel = min(vec2<i32>(floor(in.uv * trace)), vec2<i32>(trace) - vec2<i32>(1));
    let t = textureLoad(dbg_trace, texel, 0);
    var gray = t.a;
    if (dbg_params.mode.x > 0.5) {
        if (t.a <= 0.0) {
            return vec4<f32>(1.0, 0.0, 1.0, 1.0);
        }
        gray = t.r / max(dbg_params.mode.y - 1.0, 1.0);
    }
    let linear = srgb_to_linear(vec3<f32>(clamp(gray, 0.0, 1.0)));
    return vec4<f32>(linear, 1.0);
}
