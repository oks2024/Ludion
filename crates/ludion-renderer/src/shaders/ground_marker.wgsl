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
    // xy = the rounded box's half-extent in the local XZ plane, z = its corner radius, w = padding.
    @location(5) shape: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // The fragment's position in the marker's local XZ plane — the SDF's domain.
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) shape: vec4<f32>,
};

// The six corners of a quad (two triangles) in [-0.5, 0.5], the sprite pass's convention.
const CORNERS = array<vec2<f32>, 6>(
    vec2<f32>(-0.5, -0.5), vec2<f32>(0.5, -0.5), vec2<f32>(0.5, 0.5),
    vec2<f32>(-0.5, -0.5), vec2<f32>(0.5, 0.5), vec2<f32>(-0.5, 0.5),
);

// The quad is drawn slightly larger than the marker so the `fwidth` fringe has room to fall off
// instead of being clipped at the footprint's own edge. A ratio rather than an absolute margin: the
// extra band is then the same fraction of any marker, and the extra fragments are fully transparent.
const QUAD_PAD: f32 = 1.05;

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
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d = sd_rounded_box(in.local, in.shape.xy, in.shape.z);
    // Analytic anti-aliasing: one screen-space texel of the distance field, centred on the outline,
    // so the edge is one pixel wide at any distance and any marker size.
    let aa = max(fwidth(d), 1e-6);
    let coverage = 1.0 - smoothstep(-aa, aa, d);
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
