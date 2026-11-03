// Particle pass: instanced camera-facing analytic discs, premultiplied, into the HDR target. Drawn
// inside the scene pass between the sprites and the world-space text. Unlit by default: the authored
// colour is the look. A lit instance's colour is shaded once per centre in the vertex stage.
//
// There is no texture and no atlas: a particle is one radial falloff evaluated on the quad, shaped
// by a per-instance `softness` that runs from a hard anti-aliased edge to a gaussian. That is what
// makes an ember and a smoke puff one branch-free shader, and what lets an emitter animate a
// particle's size and colour as a per-frame float write.
//
// Alpha and additive particles share this one pipeline, one buffer and one sort: the blend is
// `(One, OneMinusSrcAlpha)` on both channels, so the fragment premultiplies its colour and an
// additive instance simply reports zero output alpha — the destination is then never attenuated and
// the weighted colour is added on top.
//
// The `Camera` uniform (@group 0), the clustered light set (@group 1) and `shade_two_sided` are
// shared snippets composed ahead of this file (`snippets/camera.wgsl`, `lights.wgsl`,
// `billboard_light.wgsl`). A lit centre takes the lit sprite's light rule — one cluster lookup, the
// cell a sprite fragment at that point would read; the vertex stage has no derivatives, and the loop
// needs none. An HDR value above 1 rides the bloom chain exactly like an emissive mesh.

struct VsIn {
    @location(0) center: vec3<f32>,
    // Unit camera-basis axes, already resolved on the CPU (the sprite pass's billboarding).
    @location(1) right: vec3<f32>,
    @location(2) up: vec3<f32>,
    @location(3) color: vec4<f32>,
    // x = disc diameter in world units, y = softness in [0, 1], z = the additive flag (0 or 1),
    // w = the lit flag (0 or 1).
    @location(4) params: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // The fragment's position on the disc, normalized so the rim sits at length 1.
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) params: vec4<f32>,
};

// The six corners of a quad (two triangles) in [-0.5, 0.5], the sprite pass's convention.
const CORNERS = array<vec2<f32>, 6>(
    vec2<f32>(-0.5, -0.5), vec2<f32>(0.5, -0.5), vec2<f32>(0.5, 0.5),
    vec2<f32>(-0.5, -0.5), vec2<f32>(0.5, 0.5), vec2<f32>(-0.5, 0.5),
);

// The quad is drawn slightly larger than the disc so the `fwidth` fringe has room to fall off
// instead of being clipped at the rim itself (the ground-marker convention). A ratio rather than an
// absolute margin: the extra band is then the same fraction of any particle, and its fragments are
// fully transparent.
const QUAD_PAD: f32 = 1.05;

// The gaussian's exponent at the rim. Steep enough that the truncation the renormalization below
// performs is a small correction rather than a visible step, shallow enough to stay a soft puff.
const SOFT_FALLOFF: f32 = 4.0;

@vertex
fn vs_main(in: VsIn, @builtin(vertex_index) vi: u32) -> VsOut {
    let corner = CORNERS[vi] * QUAD_PAD;
    let world = in.center + corner.x * in.params.x * in.right + corner.y * in.params.x * in.up;

    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(world, 1.0);
    // The corner spans half the diameter, so doubling puts the rim at length 1 whatever the size.
    out.local = corner * 2.0;
    var color = in.color;
    if (in.params.w > 0.5) {
        // One lookup per particle centre, the cell a sprite fragment there would read.
        let c = camera.view_proj * vec4<f32>(in.center, 1.0);
        let ndc = c.xy / max(abs(c.w), 1e-4);
        let frag = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) / lights.grid.xy;
        color = vec4<f32>(in.color.rgb * shade_two_sided(in.center, cross(in.right, in.up), frag), in.color.a);
    }
    out.color = color;
    out.params = in.params;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d = length(in.local);

    // Hard end: analytic anti-aliasing over one screen-space texel of the radial distance, centred
    // on the rim, so the edge is one pixel wide at any particle size and any distance.
    let aa = max(fwidth(d), 1e-6);
    let hard = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, d);

    // Soft end: a gaussian shifted and rescaled to reach exactly zero at the rim, so the quad's own
    // edge is never visible however soft the particle is.
    let rim = exp(-SOFT_FALLOFF);
    let soft = clamp((exp(-SOFT_FALLOFF * d * d) - rim) / (1.0 - rim), 0.0, 1.0);

    let coverage = mix(hard, soft, clamp(in.params.y, 0.0, 1.0));
    let weight = clamp(in.color.a, 0.0, 1.0) * coverage;
    // Premultiplied output. An additive instance withholds the alpha, which under
    // `(One, OneMinusSrcAlpha)` is what turns the same `over` blend into a plain add.
    let alpha = select(weight, 0.0, in.params.z > 0.5);
    return vec4<f32>(in.color.rgb * weight, alpha);
}

// The transparent layer's twin of the vertex stage (`snippets/transparent_layer.wgsl`): the same
// quad, plus the world position the distance is measured at. Only a lit additive is shaded: its
// claim comes from the colour it adds.
struct DepthOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) params: vec4<f32>,
    @location(3) world: vec3<f32>,
};

@vertex
fn vs_depth(in: VsIn, @builtin(vertex_index) vi: u32) -> DepthOut {
    let corner = CORNERS[vi] * QUAD_PAD;
    let world = in.center + corner.x * in.params.x * in.right + corner.y * in.params.x * in.up;

    var out: DepthOut;
    out.clip = camera.view_proj * vec4<f32>(world, 1.0);
    out.local = corner * 2.0;
    var color = in.color;
    if (in.params.w > 0.5 && in.params.z > 0.5) {
        // A lit additive claims from the colour it adds, shaded exactly as `vs_main` does.
        let c = camera.view_proj * vec4<f32>(in.center, 1.0);
        let ndc = c.xy / max(abs(c.w), 1e-4);
        let frag = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) / lights.grid.xy;
        color = vec4<f32>(in.color.rgb * shade_two_sided(in.center, cross(in.right, in.up), frag), in.color.a);
    }
    out.color = color;
    out.params = in.params;
    out.world = world;
    return out;
}

// The twin of `fs_main`: the same coverage, reduced to the weight the colour pass blends with. An
// additive instance claims its brightest channel, clamped to 1, as the sprite pass does.
@fragment
fn fs_depth(in: DepthOut) -> @location(0) vec4<f32> {
    let d = length(in.local);
    let aa = max(fwidth(d), 1e-6);
    let hard = 1.0 - smoothstep(1.0 - aa, 1.0 + aa, d);
    let rim = exp(-SOFT_FALLOFF);
    let soft = clamp((exp(-SOFT_FALLOFF * d * d) - rim) / (1.0 - rim), 0.0, 1.0);
    let coverage = mix(hard, soft, clamp(in.params.y, 0.0, 1.0));
    var weight = clamp(in.color.a, 0.0, 1.0) * coverage;
    if (in.params.z > 0.5) {
        let rgb = in.color.rgb * weight;
        weight = weight * min(1.0, max(rgb.r, max(rgb.g, rgb.b)));
    }
    return transparent_layer(in.world, weight);
}
