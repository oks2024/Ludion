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
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // The fragment's position in the marker's local XZ plane — the SDF's domain.
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) shape: vec4<f32>,
    @location(3) @interpolate(flat) wedge: vec4<f32>,
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
    return out;
}

// Wedge look constants. The authored colour is the look, so none of these is a field; add one if the
// slice needs to tune it.
// Outline, inner arc and fill front: crisp enough to read as a vector line at gameplay distance.
const EDGE_PX: f32 = 1.5;
// Grid lines are thinner and fainter than the outline so the cone reads as a wireframe, not a mesh.
const GRID_PX: f32 = 1.0;
const GRID_ALPHA: f32 = 0.45;
// Mostly transparent interior, so the ground and anything standing in the cone stay readable.
const INTERIOR_ALPHA: f32 = 0.06;
// The filled part reads clearly stronger than the interior without hiding the ground.
const FILL_ALPHA: f32 = 0.30;

// Coverage of a line `px` pixels wide at world distance `dist`, given `wpp` world units per pixel.
fn wedge_line(dist: f32, px: f32, wpp: f32) -> f32 {
    return 1.0 - smoothstep(px - 0.5, px + 0.5, dist / wpp);
}

// A wireframe circular sector: apex at the origin, facing local +X, in the marker's local XZ plane.
fn wedge_color(in: VsOut, wpp: f32) -> vec4<f32> {
    let p = in.local;
    let ha = in.wedge.x;
    let inner = in.wedge.y;
    let fill = in.wedge.z;
    let s = in.wedge.w;
    let outer = in.shape.z;

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
    let outline = wedge_line(abs(d), EDGE_PX, wpp);
    var lines = 0.0;
    if (inner > 0.0) {
        lines = max(lines, wedge_line(abs(r - inner), EDGE_PX, wpp));
    }
    if (fill > 0.0 && fill < 1.0) {
        lines = max(lines, wedge_line(abs(r - fill * outer), EDGE_PX, wpp));
    }
    // Concentric grid arcs every `s` metres.
    let arc = abs(fract(r / s + 0.5) - 0.5) * s;
    var grid = wedge_line(arc, GRID_PX, wpp);
    // Radial spokes, spaced so cells are about square at the rim; skipped near the apex, where they
    // converge into moire.
    let n = max(1.0, round(2.0 * ha * outer / s));
    let spoke_step = 2.0 * ha / n;
    let spoke = abs(fract((ang + ha) / spoke_step + 0.5) - 0.5) * spoke_step * r;
    if (r > s) {
        grid = max(grid, wedge_line(spoke, GRID_PX, wpp));
    }
    // Interior, stronger where the fill has reached.
    let filled = 1.0 - smoothstep(-wpp, wpp, r - fill * outer);
    let interior = mix(INTERIOR_ALPHA, FILL_ALPHA, filled * select(0.0, 1.0, fill > 0.0));

    let body = max(max(lines, grid * GRID_ALPHA), interior);
    let coverage = max(outline, inside * body);
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Derivatives sit outside the branch so they stay in uniform control flow.
    let wpp = max(length(fwidth(in.local)), 1e-6); // world units per pixel
    if (in.shape.w > 0.5) {
        return wedge_color(in, wpp);
    }
    let d = sd_rounded_box(in.local, in.shape.xy, in.shape.z);
    // Analytic anti-aliasing: one screen-space texel of the distance field, centred on the outline,
    // so the edge is one pixel wide at any distance and any marker size.
    let aa = max(fwidth(d), 1e-6);
    let coverage = 1.0 - smoothstep(-aa, aa, d);
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
