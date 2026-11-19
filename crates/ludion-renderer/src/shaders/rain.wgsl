// Rain pass: a world-anchored lattice of velocity-stretched streaks, evaluated entirely in the vertex
// stage from one `RainParams` record — no vertex or instance buffer. Drawn in the transparents pass
// right after the blended meshes; the transparent layer redraws it through `vs_depth`/`fs_depth`.
//
// World space is tiled in cubes of `cell` metres. Drop `k` of tile `(i, j, l)` hashes (seed, tile, k)
// with PCG rounds into a start phase, a speed and a gust factor, so a drop's position is a function of
// the tile and the time alone — never of the camera. Each tile column wraps inside itself, so the
// lattice repeats without a seam. The CPU twin `drop_position` (`rain.rs`) is the same arithmetic and
// its hash is bit-identical.
//
// The streak is a quad along the drop's world velocity (camera motion is never added), at least one
// output pixel wide with its alpha scaled by the width it was widened from. Blending is premultiplied.
// A lit rain shades each streak centre once through the lit-sprite rule (`shade_two_sided`), unshadowed.
//
// A fragment under a roof is hidden: it reads the `rain_occlusion` depth map (every shadow caster seen
// straight down) and fades out below the caster's top at its column (`rain_open_sky`).
//
// The `Camera` uniform (@group 0), the clustered light set (@group 1) and the map's `occluder_top` are
// shared snippets composed ahead of this file.

struct RainParams {
    // xyz = the base tile of the drawn block, w unused.
    base: vec4<i32>,
    // x = tiles per side (xz), y = tiles in height, z = drops per tile, w = seed.
    counts: vec4<u32>,
    // x = cell (m), y = time (s), z = radius (m), w = lit flag (0 or 1).
    shape: vec4<f32>,
    // x = height band low, y = high (relative to the camera), z = fall speed min, w = max.
    band: vec4<f32>,
    // xyz = wind (m/s), w = gust.
    wind: vec4<f32>,
    // x = drop width (m), y = streak length per unit speed, z = world size of one output pixel at
    // distance 1 (or at any distance under an orthographic camera), w = medium scale.
    look: vec4<f32>,
    // The authored colour (rgb) and base opacity (a).
    color: vec4<f32>,
    // The overhead occlusion map's world-to-clip view: straight down and orthographic, so its depth is
    // affine in height (column 1's z and the translation's z invert it).
    occ_view: mat4x4<f32>,
};

@group(2) @binding(0) var<uniform> rain: RainParams;
// The `rain_occlusion` map: the nearest shadow caster's depth over each texel, 1.0 where there is none.
@group(2) @binding(1) var rain_occlusion: texture_depth_2d;

// One round of PCG: a cheap integer hash with good avalanche.
fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

// The top 24 bits as a float in [0, 1).
fn unit_hash(h: u32) -> f32 {
    return f32(h >> 8u) * (1.0 / 16777216.0);
}

struct RainVert {
    clip: vec4<f32>,
    local: vec2<f32>,
    color: vec4<f32>,
    world: vec3<f32>,
};

fn rain_vertex(vi: u32, ii: u32) -> RainVert {
    let per_tile = rain.counts.z;
    let tile_index = ii / per_tile;
    let k = ii % per_tile;
    let nxz = rain.counts.x;
    let tile = rain.base.xyz + vec3<i32>(
        i32(tile_index % nxz),
        i32(tile_index / (nxz * nxz)),
        i32((tile_index / nxz) % nxz),
    );

    var h = pcg(rain.counts.w);
    h = pcg(h ^ bitcast<u32>(tile.x));
    h = pcg(h ^ bitcast<u32>(tile.y));
    h = pcg(h ^ bitcast<u32>(tile.z));
    h = pcg(h ^ k);
    let hx = unit_hash(pcg(h + 1u));
    let hz = unit_hash(pcg(h + 2u));
    let phase = unit_hash(pcg(h + 3u));
    let speed_t = unit_hash(pcg(h + 4u));
    let gust_t = unit_hash(pcg(h + 5u));

    let cell = rain.shape.x;
    let time = rain.shape.y;
    let speed = rain.band.z * (1.0 - speed_t) + rain.band.w * speed_t;
    let gust = rain.wind.w;
    let g = 1.0 - gust + 2.0 * gust * gust_t;
    let fall = fract(phase + time * speed / cell);
    let pos = vec3<f32>(
        (f32(tile.x) + fract(hx + time * rain.wind.x * g / cell)) * cell,
        (f32(tile.y) + 1.0 - fall) * cell,
        (f32(tile.z) + fract(hz + time * rain.wind.z * g / cell)) * cell,
    );
    let vel = vec3<f32>(rain.wind.x * g, -speed, rain.wind.z * g);

    // Fade over the last 15 % of the radius and of the height band.
    let cam = camera.cam_pos.xyz;
    let radius = rain.shape.z;
    let dxz = length(pos.xz - cam.xz);
    let rel_y = pos.y - cam.y;
    let h0 = rain.band.x;
    let h1 = rain.band.y;
    let span = h1 - h0;
    let fade = (1.0 - smoothstep(0.85 * radius, radius, dxz))
        * smoothstep(h0, h0 + 0.15 * span, rel_y)
        * (1.0 - smoothstep(h1 - 0.15 * span, h1, rel_y));

    let ortho = camera.view_forward.w > 0.5;
    let to_pos = pos - cam;
    let view_dir = select(to_pos, camera.view_forward.xyz, ortho);
    let dist = length(to_pos);
    let axis = normalize(vel);
    var side = cross(axis, view_dir);
    let side_len = length(side);
    side = select(vec3<f32>(1.0, 0.0, 0.0), side / max(side_len, 1e-6), side_len > 1e-4);

    // Widen to one output pixel, keeping the covered area: alpha scales by the authored width.
    let pixel = rain.look.z * select(dist, 1.0, ortho);
    let width = max(rain.look.x, pixel);
    let half_len = 0.5 * length(vel) * rain.look.y;
    let corner = CORNERS[vi] * 2.0;
    let world = pos + axis * (corner.y * half_len) + side * (corner.x * 0.5 * width);

    var out: RainVert;
    out.clip = select(
        vec4<f32>(2.0, 2.0, 2.0, 1.0),
        camera.view_proj * vec4<f32>(world, 1.0),
        fade > 0.0,
    );
    out.local = corner;
    var rgb = rain.color.rgb;
    if (rain.shape.w > 0.5) {
        let c = camera.view_proj * vec4<f32>(pos, 1.0);
        let ndc = c.xy / max(abs(c.w), 1e-4);
        let frag = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) / lights.grid.xy;
        rgb = rgb * shade_two_sided(pos, cam - pos, frag);
    }
    out.color = vec4<f32>(rgb, rain.color.a * fade * (rain.look.x / width));
    out.world = world;
    return out;
}

// Soft across the width, brighter at the head (the leading end along the fall) than the tail.
fn rain_coverage(local: vec2<f32>) -> f32 {
    let across = 1.0 - local.x * local.x;
    let along = (1.0 - local.y * local.y) * mix(0.3, 1.0, local.y * 0.5 + 0.5);
    return clamp(across * along, 0.0, 1.0);
}

// 1 above the nearest shadow caster over `world`'s column, fading to 0 over 5 cm below its top, so
// the streaks stop under a roof with a crisp edge. A column outside the map, or with no caster, is open.
fn rain_open_sky(world: vec3<f32>) -> f32 {
    let column = occluder_top(rain.occ_view, rain_occlusion, world);
    if (column.y < 0.5 || column.x <= NO_OCCLUDER) {
        return 1.0;
    }
    return smoothstep(column.x - 0.05, column.x, world.y);
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let v = rain_vertex(vi, ii);
    var out: VsOut;
    out.clip = v.clip;
    out.local = v.local;
    out.color = v.color;
    out.world = v.world;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let weight = clamp(in.color.a, 0.0, 1.0) * rain_coverage(in.local) * rain_open_sky(in.world);
    return vec4<f32>(in.color.rgb * weight, weight);
}

// The transparent layer's twin of the vertex stage: the same quad, plus the world position the
// distance is measured at.
struct DepthOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_depth(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> DepthOut {
    let v = rain_vertex(vi, ii);
    var out: DepthOut;
    out.clip = v.clip;
    out.local = v.local;
    out.color = v.color;
    out.world = v.world;
    return out;
}

@fragment
fn fs_depth(in: DepthOut) -> @location(0) vec4<f32> {
    let weight = clamp(in.color.a, 0.0, 1.0) * rain_coverage(in.local) * rain_open_sky(in.world);
    return transparent_layer(in.world, weight, rain.look.w);
}
