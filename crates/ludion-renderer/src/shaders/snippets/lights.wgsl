// Shared clustered-forward light bindings: the per-frame header uniform plus the three storage
// buffers every lit shader (mesh, sprite, particle) reads, all at @group(1). Composed ahead of every
// body so the structs, the grid shape and the bindings live in exactly one place
// (Plan/shader-bindgen.md decision 16, Plan/clustered-forward.md decision 5).
//
// THE SHAPE. `light_list` carries **every** light in the scene — there is no cap and no selection.
// The screen is divided into a fixed CLUSTER_TILES_X x CLUSTER_TILES_Y grid of tiles x
// CLUSTER_SLICES radial-distance shells; `cluster_table[cell]` is that cell's (offset, count) into
// `cluster_lights`, whose entries index `light_list`. A fragment maps itself to one cell and
// iterates only that cell's slice, so per-pixel cost tracks the lights actually near the pixel.
//
// THE IDENTITY ARGUMENT (load-bearing — Plan/clustered-forward.md "The design"). Two properties
// together make the clustered loop *arithmetically identical* to a flat loop over every light:
//   - Within a cell the indices ascend, and the global array is `ludion-engine`'s ascending-EntityId
//     extract order, so a fragment sums its lights in exactly the order a flat loop would. Float
//     addition order is preserved. Never re-associate the sum; never sort a cell's list by anything
//     but the light index.
//   - Every consumer's attenuation is `clamp(1 - dist/range, 0, 1)` squared — a hard zero at range —
//     so a light the CPU build admits *conservatively* contributes exactly +0.0 and cannot move a
//     pixel. A light the build *misses* is a visible hole; conservatism is mandatory, tightness is
//     negotiable.
//
// THE LOOP BOUND IS NON-UNIFORM, which forbids implicit derivatives inside a light loop: no
// `textureSample`, no `dpdx`/`dpdy`. Every consumer's loop is ALU-only today and must stay that way
// (an effect that wants a per-light texture read moves the read out of the loop, or pays
// `textureSampleLevel`).

struct Light {
    // xyz = world position, w = range.
    position: vec4<f32>,
    // rgb = color * intensity (premultiplied), w = a spot's cos(inner half-angle), 0 for a point.
    color: vec4<f32>,
    // A spot: xyz = unit cone axis, w = cos(outer half-angle). A point light: (0, 0, 0, -2) — any
    // `w < -1.5` marks a point, and shaders skip the cone factor for it entirely.
    axis: vec4<f32>,
};

// Everything that is one value for the whole frame; the unbounded data rides the storage buffers.
struct LightHeader {
    // rgb = the scene's flat ambient term, w unused.
    ambient: vec4<f32>,
    // xy = the reciprocal of the render extent in pixels (what turns a fragment's framebuffer
    // position into tile space — the same convention `EnvParams.ao.yz` uses for the AO texture).
    // zw = the radial shell ladder's (scale, bias), an exponential ladder over a fixed distance
    // range: shell = log2(dist^2) * scale + bias. It is fed the **squared** camera distance with the
    // halving of `log2` folded into `scale` CPU-side, which spares every lit fragment a square root;
    // both terms are computed CPU-side.
    grid: vec4<f32>,
    // The sun, for the sprite and particle shaders (the mesh shader reads its own `sun` uniform,
    // which also carries the shadow map). xyz = normalized travel direction, w unused.
    sun_direction: vec4<f32>,
    // rgb = the sun's premultiplied colour, zero for a sunless frame, w unused.
    sun_color: vec4<f32>,
};

@group(1) @binding(0) var<uniform> lights: LightHeader;
// Every light in the scene, in extract order. Runtime-sized: content, not a compile-time constant,
// decides how many there are.
@group(1) @binding(1) var<storage, read> light_list: array<Light>;
// One entry per cell, in (shell, tile_y, tile_x) row-major order: x = offset into `cluster_lights`,
// y = how many entries belong to this cell.
@group(1) @binding(2) var<storage, read> cluster_table: array<vec2<u32>>;
// The concatenated per-cell index lists; each entry indexes `light_list`.
@group(1) @binding(3) var<storage, read> cluster_lights: array<u32>;

// This fragment's (offset, count) slice of `cluster_lights`: the cell from its framebuffer position
// and **squared** radial distance to the camera (`cluster_cell`, composed ahead of this snippet).
// Everything arrives as a parameter rather than through the camera snippet's globals, so either
// consumer can compose it alone; taking the squared distance also lets both consumers hand over a
// `dot` of a vector they already had instead of a square root.
fn cluster_slice(frag_pos: vec2<f32>, view_dist_sq: f32) -> vec2<u32> {
    return cluster_table[cluster_cell(frag_pos, view_dist_sq, lights.grid)];
}

// A spot's cone factor for the surface-to-light unit vector `l`: 1 inside the inner cone, a Hermite
// ramp between, and an exact 0.0 at or outside the outer cone (which is what keeps a conservatively
// over-filed spot a `+0.0`). Written out rather than `smoothstep` so a hard edge
// (`cos_inner == cos_outer`) never divides by zero, and a NaN `l` yields 0.
fn spot_cone(light: Light, l: vec3<f32>) -> f32 {
    let cos_angle = dot(-l, light.axis.xyz);
    let cos_outer = light.axis.w;
    let cos_inner = light.color.w;
    if (!(cos_angle > cos_outer)) {
        return 0.0;
    }
    if (cos_angle >= cos_inner) {
        return 1.0;
    }
    let t = (cos_angle - cos_outer) / (cos_inner - cos_outer);
    return t * t * (3.0 - 2.0 * t);
}

// The factor both lit loops multiply into a light's colour **first**: an exact 1.0 for a point light
// (`axis.w < -1.5`), so `color * 1.0` is bit-identical and every later product, rounding and
// contraction is the point-only shader's; a spot's cone factor otherwise.
fn spot_factor(light: Light, l: vec3<f32>) -> f32 {
    if (light.axis.w < -1.5) {
        return 1.0;
    }
    return spot_cone(light, l);
}
