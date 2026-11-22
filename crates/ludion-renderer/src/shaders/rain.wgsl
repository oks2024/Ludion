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
// A lit rain shades each streak centre once through the lit-sprite rule (`shade_two_sided`); with
// `shadowed` or `backlight` it takes `shade_billboard_shadowed`: one shadow tap per light and a forward-
// scatter lobe.
//
// After the streaks the same draw carries the splash instances (`splash_vertex`): rings and droplets on
// the tops the occlusion map finds, through the same fragment stages.
//
// A fragment under a roof is hidden: it reads the `rain_occlusion` depth map (every shadow caster seen
// straight down) and fades out below the caster's top at its column (`rain_open_sky`).
//
// The `Camera` uniform (@group 0), the clustered light set (@group 1), the sun group (@group 2), the map's `occluder_top` and the
// `pcg`/`unit_hash` rounds (`rain_ripple.wgsl`) are shared snippets composed ahead of this file.

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
    // xy = the splash block's base cell (i, j), z = cells per side, w = the first splash instance (the
    // streak instance count).
    splash_grid: vec4<i32>,
    // x = the splash probability per slot, the rest unused.
    splash: vec4<f32>,
    // x = the shadowed flag (0 or 1), y = backlight, z = the medium's anisotropy `g`, w unused.
    shade: vec4<f32>,
};

@group(3) @binding(0) var<uniform> rain: RainParams;
// The `rain_occlusion` map: the nearest shadow caster's depth over each texel, 1.0 where there is none.
@group(3) @binding(1) var rain_occlusion: texture_depth_2d;

struct RainVert {
    clip: vec4<f32>,
    local: vec2<f32>,
    color: vec4<f32>,
    world: vec3<f32>,
    // 1 for a splash's ring quad, 0 for streaks and droplets.
    ring: f32,
};

// The light at a shade point: the sprite rule, or the shadowed and backlit one when either is authored.
fn rain_shade(p: vec3<f32>, frag: vec2<f32>) -> vec3<f32> {
    let cam = camera.cam_pos.xyz;
    if (rain.shade.x > 0.5 || rain.shade.y > 0.0) {
        return shade_billboard_shadowed(p, cam - p, frag, rain.shade.x > 0.5, rain.shade.y, rain.shade.z);
    }
    return shade_two_sided(p, cam - p, frag);
}

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
        rgb = rgb * rain_shade(pos, frag);
    }
    out.color = vec4<f32>(rgb, rain.color.a * fade * (rain.look.x / width));
    out.world = world;
    return out;
}

// Splashes (`Plan/rain-and-wet.md` decisions 25-27): a world-anchored lattice of 0.5 m cells, one hashed
// splash per 0.16 s slot per cell, at the height the `rain_occlusion` map gives for its column. Each
// splash is 5 instances after the streaks: part 0 a horizontal ring, parts 1-4 ballistic droplets built
// like streaks. The CPU twin `splash_at` (`rain.rs`) is the cell hash; keep the two in step.
const SPLASH_CELL: f32 = 0.5;
const SPLASH_SLOT: f32 = 0.16;
const SPLASH_REACH: f32 = 12.0;
const SPLASH_DROP_SPEED: f32 = 1.2;
const OFF_SCREEN = vec4<f32>(2.0, 2.0, 2.0, 1.0);

fn splash_vertex(vi: u32, si: u32) -> RainVert {
    let s = si / 5u;
    let part = si % 5u;
    let n = u32(rain.splash_grid.z);
    let cell = rain.splash_grid.xy + vec2<i32>(i32(s % n), i32(s / n));

    var h = pcg(rain.counts.w ^ 0x85ebca6bu);
    h = pcg(h ^ bitcast<u32>(cell.x));
    h = pcg(h ^ bitcast<u32>(cell.y));
    let time = rain.shape.y;
    let u = time / SPLASH_SLOT + unit_hash(pcg(h + 1u));
    let age = fract(u);
    let hs = pcg(h ^ bitcast<u32>(i32(floor(u))));
    let present = unit_hash(pcg(hs + 1u)) < rain.splash.x;
    let o = vec2<f32>(
        (f32(cell.x) + unit_hash(pcg(hs + 2u))) * SPLASH_CELL,
        (f32(cell.y) + unit_hash(pcg(hs + 3u))) * SPLASH_CELL,
    );

    // The top at the origin and one map texel along x and z: no splash off the map, over no caster, or
    // on a slope past 45 degrees.
    let radius = rain.shape.z;
    let texel = 2.0 * radius / f32(textureDimensions(rain_occlusion).x);
    let c0 = occluder_top(rain.occ_view, rain_occlusion, vec3<f32>(o.x, 0.0, o.y));
    let cx = occluder_top(rain.occ_view, rain_occlusion, vec3<f32>(o.x + texel, 0.0, o.y));
    let cz = occluder_top(rain.occ_view, rain_occlusion, vec3<f32>(o.x, 0.0, o.y + texel));
    let on_map = c0.y > 0.5 && cx.y > 0.5 && cz.y > 0.5
        && c0.x > NO_OCCLUDER && cx.x > NO_OCCLUDER && cz.x > NO_OCCLUDER;
    let flat_enough = abs(cx.x - c0.x) <= texel && abs(cz.x - c0.x) <= texel;
    let top = c0.x;

    let cam = camera.cam_pos.xyz;
    let reach = min(radius, SPLASH_REACH);
    let fade = 1.0 - smoothstep(0.85 * reach, reach, length(o - cam.xz));
    let shown = present && on_map && flat_enough && fade > 0.0;

    let base = vec3<f32>(o.x, top, o.y);
    var out: RainVert;
    out.ring = select(0.0, 1.0, part == 0u);
    var alpha: f32;
    var world: vec3<f32>;
    if (part == 0u) {
        let ring_half = mix(0.03, 0.12, age);
        let corner = CORNERS[vi] * 2.0;
        world = base + vec3<f32>(corner.x * ring_half, 0.01, corner.y * ring_half);
        out.local = corner;
        alpha = rain.color.a * 1.5 * (1.0 - age) * (1.0 - age) * fade;
    } else {
        let spin = unit_hash(pcg(hs + 4u));
        let theta = 6.2831855 * (spin + f32(part - 1u) / 4.0);
        let vh = 0.45 + 0.3 * unit_hash(pcg(hs + 4u + part));
        let tau = age * SPLASH_SLOT;
        let vel = vec3<f32>(cos(theta) * vh, SPLASH_DROP_SPEED - 9.81 * tau, sin(theta) * vh);
        let pos = base + vec3<f32>(vel.x * tau, SPLASH_DROP_SPEED * tau - 4.905 * tau * tau, vel.z * tau);
        let ortho = camera.view_forward.w > 0.5;
        let to_pos = pos - cam;
        let view_dir = select(to_pos, camera.view_forward.xyz, ortho);
        let axis = normalize(vel);
        var side = cross(axis, view_dir);
        let side_len = length(side);
        side = select(vec3<f32>(1.0, 0.0, 0.0), side / max(side_len, 1e-6), side_len > 1e-4);
        let pixel = rain.look.z * select(length(to_pos), 1.0, ortho);
        let width = max(rain.look.x, pixel);
        let half_len = 0.5 * length(vel) * rain.look.y;
        let corner = CORNERS[vi] * 2.0;
        world = pos + axis * (corner.y * half_len) + side * (corner.x * 0.5 * width);
        out.local = corner;
        alpha = rain.color.a * 2.0 * (1.0 - age) * fade * (rain.look.x / width);
    }
    out.clip = select(OFF_SCREEN, camera.view_proj * vec4<f32>(world, 1.0), shown);
    var rgb = rain.color.rgb;
    if (rain.shape.w > 0.5 && shown) {
        let c = camera.view_proj * vec4<f32>(base, 1.0);
        let ndc = c.xy / max(abs(c.w), 1e-4);
        let frag = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) / lights.grid.xy;
        rgb = rgb * rain_shade(base, frag);
    }
    out.color = vec4<f32>(rgb, alpha);
    out.world = world;
    return out;
}

// Streak instances come first; the splash block follows at `splash_grid.w`.
fn rain_dispatch(vi: u32, ii: u32) -> RainVert {
    let first = u32(rain.splash_grid.w);
    if (ii >= first) {
        return splash_vertex(vi, ii - first);
    }
    return rain_vertex(vi, ii);
}

// A splash ring: a thin circle at 0.75 of the quad's half-size.
fn splash_ring_coverage(local: vec2<f32>) -> f32 {
    return 1.0 - smoothstep(0.0, 0.25, abs(length(local) - 0.75));
}

fn rain_shape(local: vec2<f32>, ring: f32) -> f32 {
    return select(rain_coverage(local), splash_ring_coverage(local), ring > 0.5);
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
    @location(3) @interpolate(flat) ring: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let v = rain_dispatch(vi, ii);
    var out: VsOut;
    out.clip = v.clip;
    out.local = v.local;
    out.color = v.color;
    out.world = v.world;
    out.ring = v.ring;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let weight = clamp(in.color.a, 0.0, 1.0) * rain_shape(in.local, in.ring) * rain_open_sky(in.world);
    return vec4<f32>(in.color.rgb * weight, weight);
}

// The transparent layer's twin of the vertex stage: the same quad, plus the world position the
// distance is measured at.
struct DepthOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) world: vec3<f32>,
    @location(3) @interpolate(flat) ring: f32,
};

@vertex
fn vs_depth(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> DepthOut {
    let v = rain_dispatch(vi, ii);
    var out: DepthOut;
    out.clip = v.clip;
    out.local = v.local;
    out.color = v.color;
    out.world = v.world;
    out.ring = v.ring;
    return out;
}

@fragment
fn fs_depth(in: DepthOut) -> @location(0) vec4<f32> {
    let weight = clamp(in.color.a, 0.0, 1.0) * rain_shape(in.local, in.ring) * rain_open_sky(in.world);
    return transparent_layer(in.world, weight, rain.look.w);
}
