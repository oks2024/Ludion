// Screen-space UI rect pass: instanced pixel-space quads, textured × tint, premultiplied, unlit.
//
// Each instance is a pixel-space rect `[x, y, w, h]` (top-left origin, +Y down) plus a linear RGBA
// tint. The vertex stage expands 6 vertices from the vertex index and converts pixel coordinates to
// NDC using the viewport extent (top-left pixel origin, +Y down → NDC +Y up). Target is the LDR
// sRGB output — colors are linear, the hardware encodes on store.
//
// Every sampled texture is PREMULTIPLIED — a `UiRect` texture rides the same convention as a sprite
// because both resolve through the one texture cache — and the blend is `One, OneMinusSrcAlpha`, so
// the tint weights the texel by its own alpha rather than the blend doing it. An untextured rect
// binds the 1×1 white texture, on which premultiplication is the identity, so the tint is still the
// solid fill (the sprite convention).

struct Ui {
    // xy = viewport pixel extent; zw = padding to a 16-byte uniform.
    viewport: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ui: Ui;
@group(1) @binding(0) var rect_tex: texture_2d<f32>;
@group(1) @binding(1) var rect_sampler: sampler;

struct VsIn {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    // The sampled sub-rectangle (u0, v0, u1, v1), top-left origin: the whole texture (a loose PNG or
    // an untextured fill) is (0, 0, 1, 1); one packed atlas region is a sub-rect.
    @location(2) uv: vec4<f32>,
    // The analytic clip (see `clip_cov`): two boxes, then (radius_a, radius_b, on_a, on_b).
    @location(3) clip_a: vec4<f32>,
    @location(4) clip_b: vec4<f32>,
    @location(5) clip_r: vec4<f32>,
    // The clip boxes' rotation: (cos_a, sin_a, cos_b, sin_b), (1, 0, 1, 0) when unrotated.
    @location(6) clip_t: vec4<f32>,
    // The instance's rotation: (cos, sin, pivot_x, pivot_y), (1, 0, 0, 0) when unrotated.
    @location(7) rot: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) cl_a: vec4<f32>,
    @location(3) @interpolate(flat) cl_b: vec4<f32>,
    @location(4) @interpolate(flat) cl_r: vec4<f32>,
    @location(5) @interpolate(flat) cl_t: vec4<f32>,
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

// The six corners of a quad (two triangles) as [0,1] fractions of the rect.
const CORNERS = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
);

@vertex
fn vs_main(in: VsIn, @builtin(vertex_index) vi: u32) -> VsOut {
    let corner = CORNERS[vi];
    var px = in.rect.xy + corner * in.rect.zw;
    // Rotate about the pivot only when the instance is rotated, so an unrotated vertex is untouched.
    let d = px - in.rot.zw;
    let rotated = in.rot.zw + vec2<f32>(in.rot.x * d.x - in.rot.y * d.y, in.rot.y * d.x + in.rot.x * d.y);
    px = select(px, rotated, in.rot.x != 1.0 || in.rot.y != 0.0);
    let ndc = vec2<f32>(
        px.x / ui.viewport.x * 2.0 - 1.0,
        1.0 - px.y / ui.viewport.y * 2.0,
    );
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    // `corner` is already the [0,1] top-left-origin unit UV, so the sub-rect map is the identity for
    // the default (0, 0, 1, 1) — a whole-texture rect samples exactly as before.
    out.uv = in.uv.xy + corner * (in.uv.zw - in.uv.xy);
    out.color = in.color;
    out.cl_a = in.clip_a;
    out.cl_b = in.clip_b;
    out.cl_r = in.clip_r;
    out.cl_t = in.clip_t;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(rect_tex, rect_sampler, in.uv);
    // The clip coverage multiplies the tint's alpha; an unclipped instance multiplies by exactly 1.0.
    let a = in.color.a * clip_cov(in.clip.xy, in.cl_a, in.cl_b, in.cl_r, in.cl_t);
    // The texel is premultiplied, so the tint's own alpha scales its colour as well as its coverage.
    return vec4<f32>(texel.rgb * in.color.rgb * a, texel.a * a);
}
