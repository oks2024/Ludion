// The SSR blend: a fullscreen additive draw that mixes the traced reflection into the fill the
// `reflections` pass already added. Where the trace is confident the scene ends at
// `fill + confidence × (hit × (F0 × ab.x + ab.y) - fill)`, a mix of the fill and the hit by confidence,
// so it adds `confidence × (hit_radiance - fill)` and is exactly zero where the confidence is (a
// negative add is intended). The trace is the half-resolution `ssr_trace`, upsampled with depth and
// normal weights; its colour pyramid was seeded after the fill, so a hit shows the probe and sky look.
// The look's intensity scales the confidence, so intensity 0 adds nothing.
//
// The inputs are bound in the order the frame graph declares the pass's reads.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`.

// The fill `reflections` wrote, at the render extent.
@group(0) @binding(0) var blend_fill: texture_2d<f32>;
// World normal in `xyz`, the clamped roughness in `w`; zero where no opaque surface was drawn.
@group(0) @binding(1) var blend_normal: texture_2d<f32>;
// F0 in `rgb`, specular occlusion in `a`.
@group(0) @binding(2) var blend_specular: texture_2d<f32>;
// The opaque pass's depth, at the render extent (loaded, never filtered).
@group(0) @binding(3) var blend_depth: texture_depth_2d;
// The SSR trace at its own (smaller) extent: radiance in `rgb`, confidence in `a`.
@group(0) @binding(4) var blend_trace: texture_2d<f32>;
// The split-sum BRDF LUT.
@group(0) @binding(5) var blend_brdf_lut: texture_2d<f32>;
@group(0) @binding(6) var blend_samp: sampler;

struct SsrBlendParams {
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    // x = the look's intensity (scales the confidence).
    look: vec4<f32>,
}
@group(0) @binding(7) var<uniform> blend_params: SsrBlendParams;

// The camera distance of the texel at render pixel `q` with depth `d`.
fn blend_distance(q: vec2<i32>, d: f32, size: vec2<f32>) -> f32 {
    let ndc = vec2<f32>(2.0 * ((f32(q.x) + 0.5) / size.x) - 1.0, 1.0 - 2.0 * ((f32(q.y) + 0.5) / size.y));
    let clip = blend_params.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    return length(clip.xyz / clip.w - blend_params.cam_pos.xyz);
}

// The trace upsampled to render pixel `p`: bilinear over the four nearest trace texels, each
// weighted by how well the pixel it traced from (`ssr_source_pixel`, shared with the trace) agrees
// with `p` in depth and normal. Zero where no tap agrees, which is the sky fallback.
fn ssr_upsample(p: vec2<i32>, n: vec3<f32>, dist: f32) -> vec4<f32> {
    let render = textureDimensions(blend_depth);
    let trace = textureDimensions(blend_trace);
    let render_f = vec2<f32>(render);
    let c = (vec2<f32>(p) + vec2<f32>(0.5)) * vec2<f32>(trace) / render_f - vec2<f32>(0.5);
    let base = floor(c);
    let f = c - base;
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        let o = vec2<f32>(f32(i & 1), f32(i >> 1));
        let t = vec2<i32>(clamp(base + o, vec2<f32>(0.0), vec2<f32>(trace) - vec2<f32>(1.0)));
        let bilinear = select(1.0 - f.x, f.x, o.x > 0.5) * select(1.0 - f.y, f.y, o.y > 0.5);
        let g = ssr_source_pixel(t, render, trace);
        let gd = textureLoad(blend_depth, g, 0);
        if (gd >= 1.0) {
            continue;
        }
        let depth_gap = abs(blend_distance(g, gd, render_f) - dist) / (0.1 * dist);
        let wd = saturate(1.0 - depth_gap);
        let wn = pow(saturate(dot(n, normalize(textureLoad(blend_normal, g, 0).xyz))), 8.0);
        let w = bilinear * wd * wd * wn;
        let tap = textureLoad(blend_trace, t, 0);
        // Premultiplied by confidence: a miss tap carries no radiance and must not darken a hit's.
        sum = sum + vec4<f32>(tap.rgb * tap.a, tap.a) * w;
        total = total + w;
    }
    if (total < 1e-4) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(sum.rgb / max(sum.a, 1e-6), sum.a / total);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    // Far depth is the sky, which `transparents` draws later.
    let depth = textureLoad(blend_depth, p, 0);
    if (depth >= 1.0) {
        return vec4<f32>(0.0);
    }
    let size = vec2<f32>(textureDimensions(blend_depth));
    let ndc = vec2<f32>(2.0 * (in.pos.x / size.x) - 1.0, 1.0 - 2.0 * (in.pos.y / size.y));
    let clip = blend_params.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    let world = clip.xyz / clip.w;

    let nr = textureLoad(blend_normal, p, 0);
    let n = normalize(nr.xyz);
    let hit = ssr_upsample(p, n, length(world - blend_params.cam_pos.xyz));
    let confidence = hit.a * blend_params.look.x;
    if (confidence <= 0.0) {
        return vec4<f32>(0.0);
    }
    let spec = textureLoad(blend_specular, p, 0);
    let v = normalize(blend_params.cam_pos.xyz - world);
    let ndv = max(dot(n, v), 1e-4);
    // The sky's split-sum scale and bias, so the two paths match in BRDF.
    let ab = textureSampleLevel(blend_brdf_lut, blend_samp, vec2<f32>(ndv, nr.w), 0.0).rg;
    let fill = textureLoad(blend_fill, p, 0).rgb;
    let traced = hdr_safe(hit.rgb * (spec.rgb * ab.x + vec3<f32>(ab.y)));
    return vec4<f32>(confidence * (traced - fill), 0.0);
}
