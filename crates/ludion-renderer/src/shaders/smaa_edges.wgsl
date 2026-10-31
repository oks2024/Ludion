// SMAA 1x, pass 1: luma edge detection (HIGH preset). A port of `SMAALumaEdgeDetectionPS` from SMAA
// (<https://github.com/iryoku/smaa>), MIT-licensed; the notice is in `THIRD-PARTY-NOTICES.md` and
// `licenses/SMAA-MIT.txt`.
//
// Copyright (C) 2013 Jorge Jimenez, Jose I. Echevarria, Belen Masia, Fernando Navarro, Diego
// Gutierrez.
//
// Luma detection wants gamma-encoded colour; `smaa_edges_src` is the linear half-float composite, so
// every tap is encoded before the dot. Taps are exact `textureLoad`s (the reference's point samples)
// with the coordinate clamped to the edge, the reference's sampler addressing. There is no stencil
// here: a non-edge pixel is discarded onto the pass's black clear, which is what the stencil skip
// leaves for the next pass to read.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`; `linear_to_srgb`
// from `srgb.wgsl`.

struct SmaaEdgesParams {
    // (1 / width, 1 / height, width, height) of the render extent.
    rt_metrics: vec4<f32>,
};

@group(0) @binding(0) var smaa_edges_src: texture_2d<f32>;
// Declared but never sampled: every fullscreen effect binds textures 0..N, a sampler at N and its
// uniform at N+1 (see `FrameRecorder::fullscreen_effect`).
@group(0) @binding(1) var smaa_edges_samp: sampler;
@group(0) @binding(2) var<uniform> smaa_edges_params: SmaaEdgesParams;

const SMAA_THRESHOLD: f32 = 0.1;
const SMAA_LOCAL_CONTRAST_ADAPTATION_FACTOR: f32 = 2.0;
const SMAA_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn smaa_luma(p: vec2<i32>) -> f32 {
    let last = vec2<i32>(textureDimensions(smaa_edges_src)) - vec2<i32>(1, 1);
    let c = textureLoad(smaa_edges_src, clamp(p, vec2<i32>(0, 0), last), 0).rgb;
    return dot(linear_to_srgb(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0))), SMAA_LUMA);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let threshold = vec2<f32>(SMAA_THRESHOLD, SMAA_THRESHOLD);

    let l = smaa_luma(p);
    let l_left = smaa_luma(p + vec2<i32>(-1, 0));
    let l_top = smaa_luma(p + vec2<i32>(0, -1));

    var delta: vec4<f32>;
    delta.x = abs(l - l_left);
    delta.y = abs(l - l_top);
    var edges = step(threshold, delta.xy);
    if (dot(edges, vec2<f32>(1.0, 1.0)) == 0.0) {
        discard;
    }

    let l_right = smaa_luma(p + vec2<i32>(1, 0));
    let l_bottom = smaa_luma(p + vec2<i32>(0, 1));
    delta.z = abs(l - l_right);
    delta.w = abs(l - l_bottom);
    var max_delta = max(delta.xy, delta.zw);

    let l_leftleft = smaa_luma(p + vec2<i32>(-2, 0));
    let l_toptop = smaa_luma(p + vec2<i32>(0, -2));
    delta.z = abs(l_left - l_leftleft);
    delta.w = abs(l_top - l_toptop);
    max_delta = max(max_delta, delta.zw);
    let final_delta = max(max_delta.x, max_delta.y);

    // Local contrast adaptation: an edge must be at least half the strongest nearby one.
    edges = edges * step(vec2<f32>(final_delta), SMAA_LOCAL_CONTRAST_ADAPTATION_FACTOR * delta.xy);
    return vec4<f32>(edges, 0.0, 0.0);
}
