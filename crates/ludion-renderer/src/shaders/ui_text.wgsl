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
    // The outline: colour, the glyph's original uv rect, and (on, sdf edge or bitmap radius, 0, 0).
    @location(7) ol_color: vec4<f32>,
    @location(8) ol_rect: vec4<f32>,
    @location(9) ol_param: vec4<f32>,
    // The clip boxes' rotation: (cos_a, sin_a, cos_b, sin_b), (1, 0, 1, 0) when unrotated.
    @location(10) clip_t: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) sdf: f32,
    @location(3) @interpolate(flat) cl_a: vec4<f32>,
    @location(4) @interpolate(flat) cl_b: vec4<f32>,
    @location(5) @interpolate(flat) cl_r: vec4<f32>,
    @location(6) @interpolate(flat) ol_color: vec4<f32>,
    @location(7) @interpolate(flat) ol_rect: vec4<f32>,
    @location(8) @interpolate(flat) ol_param: vec4<f32>,
    @location(9) @interpolate(flat) cl_t: vec4<f32>,
};

// Analytic UI clip: `a`/`b` are the two clip boxes `[x, y, w, h]` in pixels, `r` is
// `(radius_a, radius_b, on_a, on_b)`, `t` is `(cos_a, sin_a, cos_b, sin_b)`. An absent box is a flag,
// never a sentinel box, and gives exactly 1.0, so an unclipped fragment multiplies its alpha by 1.0
// untouched. A rotated box is evaluated in its own frame; an unrotated one skips the mapping.
fn rounded_box_sdf(p: vec2<f32>, rect: vec4<f32>, radius: f32) -> f32 {
    let half = rect.zw * 0.5;
    let q = abs(p - (rect.xy + half)) - half + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn box_point(p: vec2<f32>, rect: vec4<f32>, cs: vec2<f32>) -> vec2<f32> {
    let c = rect.xy + rect.zw * 0.5;
    let d = p - c;
    let q = c + vec2<f32>(cs.x * d.x + cs.y * d.y, -cs.y * d.x + cs.x * d.y);
    return select(p, q, cs.x != 1.0 || cs.y != 0.0);
}

fn clip_cov(p: vec2<f32>, a: vec4<f32>, b: vec4<f32>, r: vec4<f32>, t: vec4<f32>) -> f32 {
    let cov_a = clamp(0.5 - rounded_box_sdf(box_point(p, a, t.xy), a, r.x), 0.0, 1.0);
    let cov_b = clamp(0.5 - rounded_box_sdf(box_point(p, b, t.zw), b, r.y), 0.0, 1.0);
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
    out.ol_color = in.ol_color;
    out.ol_rect = in.ol_rect;
    out.ol_param = in.ol_param;
    out.cl_t = in.clip_t;
    return out;
}

// A bitmap texel's alpha, or 0 outside the glyph's own uv rect so a neighbouring atlas glyph never
// bleeds into the outline. `textureSampleLevel` keeps the tap loop legal in non-uniform flow.
fn masked_alpha(uv: vec2<f32>, rect: vec4<f32>) -> f32 {
    let inside = all(uv >= rect.xy) && all(uv <= rect.zw);
    return select(0.0, textureSampleLevel(atlas_tex, atlas_sampler, uv, 0.0).a, inside);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = glyph_coverage(in.uv, in.sdf);
    let clip = clip_cov(in.clip.xy, in.cl_a, in.cl_b, in.cl_r, in.cl_t);
    // Uniform-flow samples for the SDF path, before the outline branch.
    let raw = textureSample(atlas_tex, atlas_sampler, in.uv).a;
    let w = max(fwidth(raw), 1e-4) * 0.7;
    if in.ol_param.x < 0.5 {
        return vec4<f32>(in.color.rgb, in.color.a * coverage * clip);
    }
    var fill_cov: f32;
    var out_cov: f32;
    if in.sdf > 0.5 {
        fill_cov = smoothstep(0.5 - w, 0.5 + w, raw);
        out_cov = smoothstep(in.ol_param.y - w, in.ol_param.y + w, raw);
    } else {
        let texel = 1.0 / vec2<f32>(textureDimensions(atlas_tex, 0));
        let k = i32(in.ol_param.y);
        fill_cov = masked_alpha(in.uv, in.ol_rect);
        out_cov = fill_cov;
        for (var j = -k; j <= k; j = j + 1) {
            for (var i = -k; i <= k; i = i + 1) {
                out_cov = max(out_cov, masked_alpha(in.uv + vec2<f32>(f32(i), f32(j)) * texel, in.ol_rect));
            }
        }
    }
    let af = in.color.a * fill_cov;
    let ao = in.ol_color.a * out_cov;
    let a = af + ao * (1.0 - af);
    let rgb = (in.color.rgb * af + in.ol_color.rgb * ao * (1.0 - af)) / max(a, 1e-6);
    return vec4<f32>(rgb, a * clip);
}
