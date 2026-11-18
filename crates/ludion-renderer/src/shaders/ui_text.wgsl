// Screen-space UI text pass: font glyph quads (bitmap or SDF atlas) generated in pixel space, alpha-blended, unlit.
//
// Vertices are generated on the CPU, one quad per glyph in pixel space with atlas UVs; the quads come
// from `ludion_font::layout` via extract, the same layout `ludion_engine::ui::measure_ui_text`
// measures (+Y down, newlines advance +Y). The vertex stage converts pixel coordinates to NDC (top-left origin, +Y down
// → NDC +Y up). The atlas group and the fragment stage (coverage, bitmap alpha or the thresholded SDF,
// modulated by the text color) come from `shaders/snippets/glyph.wgsl`, shared with `text.wgsl`.

struct Ui {
    // xy = viewport pixel extent; zw = padding to a 16-byte uniform.
    viewport: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ui: Ui;

struct VsIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) sdf: f32,
    // The analytic clip (see `clip_cov`): two boxes, then (radius_a, radius_b, on_a, on_b).
    @location(4) clip_a: vec4<f32>,
    @location(5) clip_b: vec4<f32>,
    @location(6) clip_r: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) sdf: f32,
    @location(3) @interpolate(flat) cl_a: vec4<f32>,
    @location(4) @interpolate(flat) cl_b: vec4<f32>,
    @location(5) @interpolate(flat) cl_r: vec4<f32>,
};

// Analytic UI clip: `a`/`b` are the two clip boxes `[x, y, w, h]` in pixels, `r` is
// `(radius_a, radius_b, on_a, on_b)`. An absent box is a flag, never a sentinel box, and gives exactly
// 1.0, so an unclipped fragment multiplies its alpha by 1.0 untouched.
fn rounded_box_sdf(p: vec2<f32>, rect: vec4<f32>, radius: f32) -> f32 {
    let half = rect.zw * 0.5;
    let q = abs(p - (rect.xy + half)) - half + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn clip_cov(p: vec2<f32>, a: vec4<f32>, b: vec4<f32>, r: vec4<f32>) -> f32 {
    let cov_a = clamp(0.5 - rounded_box_sdf(p, a, r.x), 0.0, 1.0);
    let cov_b = clamp(0.5 - rounded_box_sdf(p, b, r.y), 0.0, 1.0);
    return min(select(1.0, cov_a, r.z > 0.5), select(1.0, cov_b, r.w > 0.5));
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let ndc = vec2<f32>(
        in.position.x / ui.viewport.x * 2.0 - 1.0,
        1.0 - in.position.y / ui.viewport.y * 2.0,
    );
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.sdf = in.sdf;
    out.cl_a = in.clip_a;
    out.cl_b = in.clip_b;
    out.cl_r = in.clip_r;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = glyph_coverage(in.uv, in.sdf);
    return vec4<f32>(in.color.rgb, in.color.a * coverage * clip_cov(in.clip.xy, in.cl_a, in.cl_b, in.cl_r));
}
