// Screen-space UI shape pass: one instanced quad per analytic shape, evaluated as a signed distance in
// pixels, fill and stroke composited in the fragment, premultiplied, unlit.
//
// Each instance carries the shape's bounds quad `[x, y, w, h]` in pixels (top-left origin, +Y down), its
// origin and kind, the kind's parameters, a fill and a stroke colour, the analytic clip (the same
// packing `ui_rect.wgsl` reads) and the instance rotation. The vertex stage rotates only the output
// position about the pivot; the fragment's local position stays the unrotated one, so the SDF never
// inverse-rotates. Kinds: 0 sector (disc, annulus, slice), 1 segment (capsule or butt-ended box),
// 2 rounded box. Coverage: fill `clamp(0.5 - d, 0, 1)`, stroke `clamp(0.5 - (|d| - w/2), 0, 1)`, the
// stroke over the fill. The Rust twin is `ludion-engine`'s `ui/shape.rs`.

struct Ui {
    // xy = viewport pixel extent; zw = padding to a 16-byte uniform.
    viewport: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ui: Ui;

struct VsIn {
    @location(0) rect: vec4<f32>,
    // (origin_x, origin_y, kind, stroke_width).
    @location(1) s0: vec4<f32>,
    // sector (r_out, r_in, full, 0); segment (ax, ay, bx, by); box (hx, hy, radius, 0).
    @location(2) s1: vec4<f32>,
    // sector (sin m, cos m, sin h, cos h); segment (half_width, round, 0, 0).
    @location(3) s2: vec4<f32>,
    @location(4) fill: vec4<f32>,
    @location(5) stroke_color: vec4<f32>,
    @location(6) clip_a: vec4<f32>,
    @location(7) clip_b: vec4<f32>,
    @location(8) clip_r: vec4<f32>,
    @location(9) clip_t: vec4<f32>,
    // The instance's rotation: (cos, sin, pivot_x, pivot_y), (1, 0, 0, 0) when unrotated.
    @location(10) rot: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) lp: vec2<f32>,
    @location(1) @interpolate(flat) s0: vec4<f32>,
    @location(2) @interpolate(flat) s1: vec4<f32>,
    @location(3) @interpolate(flat) s2: vec4<f32>,
    @location(4) @interpolate(flat) fill: vec4<f32>,
    @location(5) @interpolate(flat) stroke_color: vec4<f32>,
    @location(6) @interpolate(flat) cl_a: vec4<f32>,
    @location(7) @interpolate(flat) cl_b: vec4<f32>,
    @location(8) @interpolate(flat) cl_r: vec4<f32>,
    @location(9) @interpolate(flat) cl_t: vec4<f32>,
};

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
    var out: VsOut;
    out.lp = px - in.s0.xy;
    let d = px - in.rot.zw;
    let rotated = in.rot.zw + vec2<f32>(in.rot.x * d.x - in.rot.y * d.y, in.rot.y * d.x + in.rot.x * d.y);
    px = select(px, rotated, in.rot.x != 1.0 || in.rot.y != 0.0);
    let ndc = vec2<f32>(
        px.x / ui.viewport.x * 2.0 - 1.0,
        1.0 - px.y / ui.viewport.y * 2.0,
    );
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.s0 = in.s0;
    out.s1 = in.s1;
    out.s2 = in.s2;
    out.fill = in.fill;
    out.stroke_color = in.stroke_color;
    out.cl_a = in.clip_a;
    out.cl_b = in.clip_b;
    out.cl_r = in.clip_r;
    out.cl_t = in.clip_t;
    return out;
}

// A slice of a disc or annulus. `s = (sin m, cos m, sin h, cos h)` for the mid angle `m` and half sweep
// `h`; the point folds about the mid ray, so one edge ray and one cap segment are enough.
fn sector_sdf(p: vec2<f32>, r_out: f32, r_in: f32, full: bool, s: vec4<f32>) -> f32 {
    let q = vec2<f32>(abs(dot(p, vec2<f32>(s.y, s.x))), dot(p, vec2<f32>(s.x, -s.y)));
    let len = length(q);
    let d_r = select(max(len - r_out, r_in - len), len - r_out, r_in <= 0.0);
    let e = vec2<f32>(s.z, s.w);
    let t = clamp(dot(q, e), r_in, r_out);
    let d_c = length(q - e * t);
    let side = select(-1.0, 1.0, s.w * q.x - s.z * q.y > 0.0);
    return select(max(d_r, side * d_c), d_r, full);
}

fn segment_sdf(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, hw: f32, round: bool) -> f32 {
    let ba = b - a;
    if round {
        let pa = p - a;
        let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-12), 0.0, 1.0);
        return length(pa - ba * h) - hw;
    }
    let len = max(length(ba), 1e-12);
    let u = ba / len;
    let pm = p - (a + b) * 0.5;
    let local = vec2<f32>(dot(pm, u), dot(pm, vec2<f32>(-u.y, u.x)));
    return rounded_box_sdf(local, vec4<f32>(-len * 0.5, -hw, len, hw * 2.0), 0.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let kind = in.s0.z;
    var d: f32;
    if kind < 0.5 {
        d = sector_sdf(in.lp, in.s1.x, in.s1.y, in.s1.z > 0.5, in.s2);
    } else if kind < 1.5 {
        d = segment_sdf(in.lp, in.s1.xy, in.s1.zw, in.s2.x, in.s2.y > 0.5);
    } else {
        d = rounded_box_sdf(in.lp, vec4<f32>(-in.s1.x, -in.s1.y, in.s1.x * 2.0, in.s1.y * 2.0), in.s1.z);
    }
    let w = in.s0.w;
    let cf = clamp(0.5 - d, 0.0, 1.0);
    let cs = select(0.0, clamp(0.5 - (abs(d) - w * 0.5), 0.0, 1.0), w > 0.0);
    let a_s = in.stroke_color.a * cs;
    let a_f = in.fill.a * cf;
    let rgb = in.stroke_color.rgb * a_s + in.fill.rgb * a_f * (1.0 - a_s);
    let a = a_s + a_f * (1.0 - a_s);
    let cov = clip_cov(in.clip.xy, in.cl_a, in.cl_b, in.cl_r, in.cl_t);
    return vec4<f32>(rgb * cov, a * cov);
}
