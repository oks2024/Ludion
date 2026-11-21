// Screen-space UI polygon pass: a tessellated filled polygon, premultiplied, unlit.
//
// Vertices are generated on the CPU in final pixel space (top-left origin, +Y down): the interior
// triangles over the inset outline plus a one-pixel feathered fringe whose outer vertices carry zero
// coverage. The colour is premultiplied and already carries that coverage, so the rasterizer's
// interpolation draws the anti-aliased edge. The fragment multiplies in the analytic clip (the same
// packing `ui_rect.wgsl` reads). Rotation is applied on the CPU.

struct Ui {
    // xy = viewport pixel extent; zw = padding to a 16-byte uniform.
    viewport: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ui: Ui;

struct VsIn {
    @location(0) position: vec2<f32>,
    // Premultiplied RGBA, coverage included.
    @location(1) color: vec4<f32>,
    // The analytic clip: two boxes, then (radius_a, radius_b, on_a, on_b), then the boxes' rotation.
    @location(2) clip_a: vec4<f32>,
    @location(3) clip_b: vec4<f32>,
    @location(4) clip_r: vec4<f32>,
    @location(5) clip_t: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(flat) cl_a: vec4<f32>,
    @location(2) @interpolate(flat) cl_b: vec4<f32>,
    @location(3) @interpolate(flat) cl_r: vec4<f32>,
    @location(4) @interpolate(flat) cl_t: vec4<f32>,
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

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let ndc = vec2<f32>(
        in.position.x / ui.viewport.x * 2.0 - 1.0,
        1.0 - in.position.y / ui.viewport.y * 2.0,
    );
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = in.color;
    out.cl_a = in.clip_a;
    out.cl_b = in.clip_b;
    out.cl_r = in.clip_r;
    out.cl_t = in.clip_t;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let cov = clip_cov(in.clip.xy, in.cl_a, in.cl_b, in.cl_r, in.cl_t);
    return in.color * cov;
}
