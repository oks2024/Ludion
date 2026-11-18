// Ground-marker pass: instanced flat quads carrying an analytic telegraph shape, alpha-blended and
// unlit, into the HDR target. Drawn inside the scene pass between the opaque meshes and the sprites.
//
// There is no texture and no atlas: each instance carries its footprint as the two parameters of one
// rounded-box signed-distance function, evaluated in the marker's local XZ plane. That is what makes
// a disc and a capsule one branch-free shader, and what makes a telegraph that fills in as its
// warmup counts down a per-frame float write rather than a texture swap.
//
//   disc(r)               -> half_extent (0, 0),           corner radius r
//   segment(length, width) -> half_extent ((L - w)/2, 0),   corner radius w/2
//   wedge(outer, ...)      -> half_extent (0, 0),           corner radius outer (the bounding disc),
//                             shape.w = 1; `wedge` = (half_angle, inner_radius, fill, grid_spacing)
//   ring(radius, ...)      -> half_extent (0, 0),           corner radius radius (the bounding disc),
//                             shape.w = 2; `extra` = (thickness, fill, dashes, 0)
//
// The look is per instance: `style_a` = (outline_px, grid_px or 0 for no grid, grid_alpha,
// interior_alpha), `style_b` = (inner_alpha, fill_alpha, flicker, seed), `inner_color` tints a
// wedge's inner zone, and a wedge's `extra.x` is its inner half-angle (negative: single zone).
//
// The `Camera` uniform (@group 0) is a shared snippet composed ahead of this file
// (`shaders/snippets/camera.wgsl`), then `sd_rounded_box` (`shaders/snippets/rounded_box.wgsl`).
// Markers are deliberately unlit — the authored colour is the look, and an HDR value above 1 rides
// the bloom chain exactly like an emissive mesh.

struct VsIn {
    // Columns of the instance's rigid local->world transform (rotation + lifted translation, no
    // scale), in the mesh pass's packing.
    @location(0) model_0: vec4<f32>,
    @location(1) model_1: vec4<f32>,
    @location(2) model_2: vec4<f32>,
    @location(3) model_3: vec4<f32>,
    @location(4) color: vec4<f32>,
    // xy = the rounded box's half-extent in the local XZ plane, z = its corner radius, w = 1 for a
    // wedge (z is then its outer radius), 0 otherwise.
    @location(5) shape: vec4<f32>,
    // A wedge's (half_angle in radians, inner_radius, fill fraction, grid_spacing); zero otherwise.
    @location(6) wedge: vec4<f32>,
    @location(7) inner_color: vec4<f32>,
    @location(8) style_a: vec4<f32>,
    // The seed is a float-encoded integer below 2^24, read back with `u32()`.
    @location(9) style_b: vec4<f32>,
    // A wedge's inner half-angle (-1 for none); a ring's (thickness, fill, dashes, 0).
    @location(10) extra: vec4<f32>,
    // x = the float offset of this marker's line-of-sight profile in `profiles`, -1 for none.
    @location(11) clip: vec4<f32>,
};

// The clipped markers' visibility profiles, packed back to back: a header (arc_start, arc_span,
// partial_alpha, bins), then `bins` radii, then `bins` partial radii. A scene with no clipped
// marker binds a dummy buffer.
@group(1) @binding(0) var<storage, read> profiles: array<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // The fragment's position in the marker's local XZ plane — the SDF's domain.
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) shape: vec4<f32>,
    @location(3) @interpolate(flat) wedge: vec4<f32>,
    @location(4) @interpolate(flat) inner_color: vec4<f32>,
    @location(5) @interpolate(flat) style_a: vec4<f32>,
    @location(6) @interpolate(flat) style_b: vec4<f32>,
    @location(7) @interpolate(flat) extra: vec4<f32>,
    @location(8) @interpolate(flat) profile: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn, @builtin(vertex_index) vi: u32) -> VsOut {
    let model = mat4x4<f32>(in.model_0, in.model_1, in.model_2, in.model_3);
    let extent = (in.shape.xy + in.shape.z) * QUAD_PAD;
    let local = CORNERS[vi] * 2.0 * extent;
    // The marker's local plane is XZ (normal +Y), matching the `Plane` primitive.
    let world = model * vec4<f32>(local.x, 0.0, local.y, 1.0);

    var out: VsOut;
    out.clip = camera.view_proj * world;
    out.local = local;
    out.color = in.color;
    out.shape = in.shape;
    out.wedge = in.wedge;
    out.inner_color = in.inner_color;
    out.style_a = in.style_a;
    out.style_b = in.style_b;
    out.extra = in.extra;
    out.profile = in.clip;
    return out;
}

// The wedge look is `GroundMarkerStyle`, per instance: outline/inner-arc/fill-front width
// (`style_a.x`), grid line width (`.y`, 0 = no grid) and opacity (`.z`), interior opacity (`.w`), the
// inner zone's interior opacity (`style_b.x`) and the filled part's opacity (`.y`). Defaults are the
// original constants: 1.5, 1.0, 0.45, 0.06, 0.12, 0.30.

// Coverage of a line `px` pixels wide at world distance `dist`, given `wpp` world units per pixel.
fn wedge_line(dist: f32, px: f32, wpp: f32) -> f32 {
    return 1.0 - smoothstep(px - 0.5, px + 0.5, dist / wpp);
}

// A wireframe circular sector: apex at the origin, facing local +X, in the marker's local XZ plane.
// With `extra.x >= 0` the wedge has a second, narrower inner zone (half-angle `extra.x`, range
// `inner`) outlined and tinted `inner_color`; with `extra.x < 0` this is the original single-zone path.
fn wedge_color(in: VsOut, wpp: f32) -> vec4<f32> {
    let p = in.local;
    let ha = in.wedge.x;
    let inner = in.wedge.y;
    let fill = in.wedge.z;
    let s = in.wedge.w;
    let outer = in.shape.z;
    let edge_px = in.style_a.x;
    let grid_px = in.style_a.y;
    let grid_alpha = in.style_a.z;
    let interior_alpha = in.style_a.w;
    let inner_alpha = in.style_b.x;
    let fill_alpha = in.style_b.y;
    let two_zone = in.extra.x >= 0.0;

    let r = length(p);
    let ang = atan2(p.y, p.x); // 0 on the facing axis
    // Sector SDF (iq's sdPie, axis rotated onto +X): q = (|p.y|, p.x), c = (sin ha, cos ha).
    let q = vec2<f32>(abs(p.y), p.x);
    let c = vec2<f32>(sin(ha), cos(ha));
    let l = r - outer;
    let m = length(q - c * clamp(dot(q, c), 0.0, outer));
    let d = max(l, m * sign(c.y * q.x - c.x * q.y));
    let inside = 1.0 - smoothstep(-wpp, wpp, d);

    // Outline: the two radial edges and the outer arc, at full alpha.
    let outline = wedge_line(abs(d), edge_px, wpp);
    var lines = 0.0;
    // A single zone draws the inner range as a full-width arc; two zones outline the inner sector.
    if (inner > 0.0 && !two_zone) {
        lines = max(lines, wedge_line(abs(r - inner), edge_px, wpp));
    }
    if (fill > 0.0 && fill < 1.0) {
        lines = max(lines, wedge_line(abs(r - fill * outer), edge_px, wpp));
    }
    // Concentric grid arcs every `s` metres.
    let arc = abs(fract(r / s + 0.5) - 0.5) * s;
    var grid = wedge_line(arc, grid_px, wpp);
    // Radial spokes, spaced so cells are about square at the rim; skipped near the apex, where they
    // converge into moire.
    let n = max(1.0, round(2.0 * ha * outer / s));
    let spoke_step = 2.0 * ha / n;
    let spoke = abs(fract((ang + ha) / spoke_step + 0.5) - 0.5) * spoke_step * r;
    if (r > s) {
        grid = max(grid, wedge_line(spoke, grid_px, wpp));
    }
    // Interior, stronger where the fill has reached.
    let filled = 1.0 - smoothstep(-wpp, wpp, r - fill * outer);
    let fill_gate = filled * select(0.0, 1.0, fill > 0.0);
    let interior = mix(interior_alpha, fill_alpha, fill_gate);

    let grid_cov = select(0.0, grid * grid_alpha, grid_px > 0.0);
    let body = max(max(lines, grid_cov), interior);
    let coverage = max(outline, inside * body);
    if (!two_zone) {
        return vec4<f32>(in.color.rgb, in.color.a * coverage);
    }

    // The inner sector, through the same sdPie.
    let ihs = in.extra.x;
    let ci = vec2<f32>(sin(ihs), cos(ihs));
    let li = r - inner;
    let mi = length(q - ci * clamp(dot(q, ci), 0.0, inner));
    let d_in = max(li, mi * sign(ci.y * q.x - ci.x * q.y));
    let inside_in = 1.0 - smoothstep(-wpp, wpp, d_in);
    let inner_outline = wedge_line(abs(d_in), edge_px, wpp) * inside;
    let inner_body = max(inner_alpha, filled * select(0.0, fill_alpha, fill > 0.0));
    let inner_cov = max(inner_outline, inside_in * inner_body);
    let w = max(inside_in, inner_outline);
    let base = vec4<f32>(in.color.rgb, in.color.a * coverage);
    let tint = vec4<f32>(in.inner_color.rgb, in.inner_color.a * max(coverage, inner_cov));
    return mix(base, tint, w);
}

// An annulus of `thickness` ending at `radius`, optionally broken into `dashes`, with an optional
// interior fill from the centre.
fn ring_color(in: VsOut, wpp: f32) -> vec4<f32> {
    let p = in.local;
    let radius = in.shape.z;
    let thickness = in.extra.x;
    let fill = in.extra.y;
    let dashes = in.extra.z;
    let r = length(p);
    let half_band = thickness * 0.5;
    let d = abs(r - (radius - half_band)) - half_band;
    var band = 1.0 - smoothstep(-wpp, wpp, d);
    if (dashes > 0.0) {
        // Keep the first half of every dash period; the distance to a dash edge is in arc length.
        let t = fract(dashes * (atan2(p.y, p.x) + 3.14159265) / 6.2831853);
        let from_centre = abs(fract(t - 0.25 + 0.5) - 0.5);
        let edge = (from_centre - 0.25) * (6.2831853 / dashes) * r;
        band = band * (1.0 - smoothstep(-wpp, wpp, edge));
    }
    var coverage = band;
    if (fill > 0.0) {
        let filled = 1.0 - smoothstep(-wpp, wpp, r - fill * radius);
        coverage = max(coverage, filled * in.style_b.y);
    }
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}

// The coverage left by the marker's line-of-sight profile at this fragment: out to the first
// blocker's range along its bearing (interpolated between the two nearest bins, wrapping for a full
// circle), dimmed to `partial_alpha` beyond the first partial blocker.
fn clip_coverage(in: VsOut, wpp: f32) -> f32 {
    let base = u32(in.profile.x);
    let arc_start = profiles[base];
    let arc_span = profiles[base + 1u];
    let partial_alpha = profiles[base + 2u];
    let bins = u32(profiles[base + 3u]);
    let nb = f32(bins);
    let r = length(in.local);
    var t = (atan2(in.local.y, in.local.x) - arc_start) / arc_span * nb - 0.5;
    var i0: u32;
    var i1: u32;
    if (arc_span > 6.28) {
        t = t - nb * floor(t / nb);
        i0 = min(u32(floor(t)), bins - 1u);
        i1 = (i0 + 1u) % bins;
    } else {
        t = clamp(t, 0.0, nb - 1.0);
        i0 = u32(floor(t));
        i1 = min(i0 + 1u, bins - 1u);
    }
    let f = t - floor(t);
    let radius = mix(profiles[base + 4u + i0], profiles[base + 4u + i1], f);
    let partial = mix(profiles[base + 4u + bins + i0], profiles[base + 4u + bins + i1], f);
    let visible = 1.0 - smoothstep(-wpp, wpp, r - radius);
    return visible * mix(1.0, partial_alpha, smoothstep(-wpp, wpp, r - partial));
}

// One round of PCG: a cheap integer hash with good avalanche, so adjacent pixels decorrelate.
fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Derivatives sit outside the branch so they stay in uniform control flow.
    let wpp = max(length(fwidth(in.local)), 1e-6); // world units per pixel
    var color: vec4<f32>;
    if (in.shape.w > 1.5) {
        color = ring_color(in, wpp);
    } else if (in.shape.w > 0.5) {
        color = wedge_color(in, wpp);
    } else {
        let d = sd_rounded_box(in.local, in.shape.xy, in.shape.z);
        // Analytic anti-aliasing: one screen-space texel of the distance field, centred on the outline,
        // so the edge is one pixel wide at any distance and any marker size.
        let aa = max(fwidth(d), 1e-6);
        let coverage = 1.0 - smoothstep(-aa, aa, d);
        color = vec4<f32>(in.color.rgb, in.color.a * coverage);
    }
    if (in.profile.x >= 0.0) {
        color.a = color.a * clip_coverage(in, wpp);
    }
    // Flicker: drop a `flicker` fraction of pixels, re-rolled every sim tick through the seed.
    if (in.style_b.z > 0.0) {
        let h = pcg(u32(in.clip.x) ^ pcg(u32(in.clip.y) ^ pcg(u32(in.style_b.w))));
        if (f32(h >> 8u) / 16777216.0 < in.style_b.z) {
            color.a = 0.0;
        }
    }
    return color;
}
