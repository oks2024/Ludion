// GI bake — one compute entry point that traces the voxel occupancy grid from every irradiance probe
// and projects two quantities into SH-L1: how much sky the probe sees, and how much radiance the
// surfaces around it send back.
//
// SKY VISIBILITY. A ray that leaves the grid without meeting an occupied cell contributes 1; the
// first occupied cell it meets contributes 0 and ends it. No light source enters that term: its
// output is a *multiplier* for the ambient term, which is what keeps a zero-strength scene
// bit-preserving.
//
// BOUNCE. The same walk, the same rays. Where a ray hits, the cell's outgoing radiance
// `albedo * (emissive + direct)` is projected into three more SH-L1 coefficient sets, one per colour
// channel. An escaped ray contributes exactly zero — the sky arrives through the prefiltered IBL
// cube modulated by the visibility term above, and adding it here would double-count it. The bounce
// half is gated by `bake_params.flags.x`: at zero the radiance work never runs, nothing is written, and
// this shader's cost and its visibility output are the visibility-only bake's exactly.
//
// THE LIGHT BIN. `direct_at` loops the shaded cell's slice of `gi_bin.rs`'s world-space light bin,
// never the whole light buffer. The bin is conservative, so this is byte-identical to the flat loop
// it replaces rather than merely close — the argument is written out at `direct_at` itself.
//
// THE HIT CACHE. The bounce half also writes down, per probe, which cell each of HIT_RAYS rays hit
// and through which face — every `cache_stride()`'th ray, so the recorded directions span the sphere
// rather than crowding one pole. Static geometry means that answer never changes
// between bakes, so the per-frame dynamic-light injection replays these instead of marching again —
// and inherits this march's leak control exactly, because it IS this march's answers.
//
// Deterministic per machine, not across machines: the ray set is a fixed function of the ray index
// (no RNG, no jitter), the light sum runs in the pinned extract order, each probe is one invocation
// with no atomics and no cross-invocation reduction, but the accumulation is GPU float arithmetic.
// It sits where the IBL prefilter sits.
//
// This is a compute pipeline outside the frame graph with an auto-derived bind group layout;
// shader-bindgen reflects its uniform PODs, binding indices and workgroup size into `shader_gen.rs`.
// Its resources carry a `bake_` prefix because bindgen unifies resources by name across all shaders.

// x/y/z = the voxel grid's world-space origin, w = its cell size in world units.
// The occupancy texture's own dimensions give the extent, so it is not carried here.
struct GiBakeParams {
    grid: vec4<f32>,
    // xyz = probe (0,0,0)'s world position, w = world units between probes.
    probe: vec4<f32>,
    // xyz = probes per axis, w = how many entries `bake_light_list` holds. The light loop's bound is the
    // bin slice, not this — it describes the buffer, not the walk.
    counts: vec4<u32>,
    // x = 1 when the three bounce volumes and the hit cache are real and must be written, 0 when the
    // fallbacks are bound and the bounce half is skipped entirely.
    // y = rays traced per probe (`QualityParams::gi_rays`), a whole multiple of HIT_RAYS. It took a
    // pad lane rather than growing this struct, so every prior field's offset is unchanged.
    // z = the first probe z this dispatch covers (the bake is submitted in z-slabs), w = pad.
    flags: vec4<u32>,
    // xyz = the direction sunlight travels (normalized), w = 1.0 when the scene has a sun.
    sun_dir: vec4<f32>,
    // rgb = the sun's premultiplied colour (`SunData::color`), w = pad.
    sun_color: vec4<f32>,
    bin: GiBinParams,
};

// `snippets/lights.wgsl`'s `Light`, declared here because `lights.wgsl` also declares its group 1
// bindings, so it cannot be composed into this shader: xyz = world position, w = range;
// rgb = color * intensity (premultiplied), w = 0; axis = (0, 0, 0, -2), the point-light encoding
// (spots never reach GI). Bindgen unifies it with that snippet's `Light`, so the two layouts cannot
// drift.
struct Light {
    position: vec4<f32>,
    color: vec4<f32>,
    axis: vec4<f32>,
};

@group(0) @binding(0) var<uniform> bake_params: GiBakeParams;
// `r` = emissive, `g` = occupancy. Only ever `textureLoad`ed — see `gi_bake.rs` on why pairing
// either voxel texture with a sampler would break the auto-derived layout.
@group(0) @binding(1) var occupancy: texture_3d<f32>;
@group(0) @binding(2) var probes_out: texture_storage_3d<rgba16float, write>;
// The grid's per-cell RGBA albedo, `textureLoad`ed at a hit. Same non-filterable rule as `occupancy`.
@group(0) @binding(3) var bake_albedo: texture_3d<f32>;
// The scene's **static** (unmarked) lights, in `ludion-engine`'s ascending-`EntityId` extract order
// — which is also this shader's float summation order, exactly as it is the lit fragment's. A light
// an entity marked `GiDynamic` carries never reaches this buffer: it is injected per frame instead,
// and its motion is not an input to this bake.
@group(0) @binding(4) var<storage, read> bake_light_list: array<Light>;
// The bounce volumes, one SH-L1 coefficient set per colour channel. 1×1×1 fallbacks when
// `bake_params.flags.x` is 0, in which case nothing below writes them.
@group(0) @binding(5) var bounce_r_out: texture_storage_3d<rgba16float, write>;
@group(0) @binding(6) var bounce_g_out: texture_storage_3d<rgba16float, write>;
@group(0) @binding(7) var bounce_b_out: texture_storage_3d<rgba16float, write>;
// `gi_bin.rs`'s world-space light bin over `bake_light_list`: one `(offset, count)` per bin cell, then
// the concatenated per-cell index lists. Each cell's list ascends by construction, so the sum below
// runs in the same relative order a flat loop would.
@group(0) @binding(8) var<storage, read> bake_bin_table: array<vec2<u32>>;
@group(0) @binding(9) var<storage, read> bake_bin_indices: array<u32>;
// The hit cache: one packed word per (probe, slot) for the HIT_RAYS strided rays, so the per-frame
// injection can replay this march's answers instead of marching again. Written only when
// `bake_params.flags.x` is 1 (it rides the bounce discriminant); a 4-byte dummy is bound otherwise.
@group(0) @binding(10) var<storage, read_write> bake_hit_cache: array<u32>;

// Half the shell radius the probe-in-solid push-out searches: the 3^3 neighbourhood, then the 5^3.
const PUSH_OUT_RADIUS: i32 = 2;

// How many of the traced rays are written to the hit cache — the direction budget the per-frame
// injection replays, and the one ray-set number that is *not* a quality axis: it sizes the cache
// buffer. `gi_bake.rs`'s `HIT_RAYS` must equal this, and
// `the_shader_and_the_cpu_agree_on_the_hit_cache_layout` is what says so.
const HIT_RAYS: u32 = 16u;

// Which rays the cache holds: every `stride`'th one, ray `i` landing in slot `i / stride`, where
// `stride = bake_params.flags.y / HIT_RAYS` — derived from the ray count rather than fixed, which is why
// a selectable `gi_rays` must be a whole multiple of HIT_RAYS.
//
// **A strided subset, never a contiguous prefix**, and the reason is the ray set's own construction.
// Only the *azimuth* is golden-angle distributed; the elevation is a monotone sweep from `y = +1` to
// `y = -1`, so the first sixteen of sixty-four are a 59°-half-angle cone about world +Y — a probe
// would see its ceiling and never its floor, and a light over open ground would bounce off nothing
// at all. Striding spends the same sixteen-direction budget across the whole sphere, at no cost: the
// slot's ray index is `k * stride`, which the reader recomputes from `k` alone.
fn cache_stride() -> u32 {
    return bake_params.flags.y / HIT_RAYS;
}
// The face code's bit position in a packed hit word: the flat cell index occupies bits 0..24 (the
// L2103 32M-cell budget is under 2^25), the face code bits 25..27.
const HIT_FACE_SHIFT: u32 = 25u;
// Face code 6: the cell was entered by no DDA step (the ray started inside it, or the boundary clamp
// put it there), so its normal is not an axis and the reader recomputes it as
// `-ray_direction(k * stride)` from the slot index alone.
const HIT_FACE_BOUNDARY: u32 = 6u;
// Face code 7: the ray escaped the grid. The whole word is then `7 << HIT_FACE_SHIFT` — escape folds
// into the face code rather than taking a sentinel word, because the reader is already switching on
// the code and a sentinel would be a second thing to test.
const HIT_FACE_ESCAPED: u32 = 7u;

// What one march brought back. `cell`, `normal` and `face` are meaningful only when `hit` is true;
// `normal` is the face the ray crossed to enter `cell`, pointing back along the ray, and `face` is
// that same face as a code (0 = -X, 1 = +X, 2 = -Y, 3 = +Y, 4 = -Z, 5 = +Z, HIT_FACE_BOUNDARY when
// no step was taken).
struct Hit {
    hit: bool,
    cell: vec3<i32>,
    normal: vec3<f32>,
    face: u32,
};

fn is_occupied(cell: vec3<i32>) -> bool {
    // `g` is the occupancy channel; `r` is emissive, which sky visibility does not read.
    return textureLoad(occupancy, cell, 0).g > 0.5;
}

// The march along `dir` from `start` (grid-cell units, so a cell is one unit wide): an
// Amanatides-Woo DDA that walks cell by cell to the grid's boundary. Reports the first occupied cell
// it meets, or `hit: false` when the ray leaves without meeting one.
//
// Visibility is `select(1.0, 0.0, hit)` — the same value the visibility-only bake returned, from the
// same walk. Every property of that walk is preserved verbatim, and `gi_bake.rs`'s exact-value tests
// (`33/64`, `1.0`, `[0.0; 4]`) are the guard that says so.
//
// A `start` outside the grid is not a special case: the slab test below advances the ray to where it
// enters the box, and a ray that never enters has escaped by definition.
fn trace_hit(start: vec3<f32>, dir: vec3<f32>, extent: vec3<i32>) -> Hit {
    var out: Hit;
    out.hit = false;
    out.cell = vec3<i32>(0);
    out.normal = vec3<f32>(0.0);
    out.face = HIT_FACE_ESCAPED;

    let d = nonzero(dir);
    let hi = vec3<f32>(extent);

    let t0 = (vec3<f32>(0.0) - start) / d;
    let t1 = (hi - start) / d;
    let enter = max(max(min(t0.x, t1.x), min(t0.y, t1.y)), min(t0.z, t1.z));
    let exit = min(min(max(t0.x, t1.x), max(t0.y, t1.y)), max(t0.z, t1.z));
    // `<=`, not `<`: the tangent case is an escape, not a hit. A ray that leaves the box at the
    // instant it starts (a probe on a max face pointing outward — the lattice puts probes exactly
    // there whenever `probe_spacing` divides the box span) traverses no cell at all, and neither
    // does one that grazes an edge. Under `<` both fall through to the boundary clamp and read a
    // cell they never enter, which is a dark rim around the volume.
    if (exit <= max(enter, 0.0)) {
        return out;
    }

    let entry_point = start + d * max(enter, 0.0);
    var cell = clamp(vec3<i32>(floor(entry_point)), vec3<i32>(0), extent - vec3<i32>(1));

    let step = select(vec3<i32>(-1), vec3<i32>(1), d > vec3<f32>(0.0));
    let next_face = vec3<f32>(cell) + select(vec3<f32>(0.0), vec3<f32>(1.0), d > vec3<f32>(0.0));
    var t_max = (next_face - entry_point) / d;
    let t_delta = abs(1.0 / d);

    // The face the ray crossed to enter the cell it is standing in, pointing back along the ray. The
    // DDA already branches on which axis it stepped, so the exact voxel face is free — no gradient,
    // no reconstruction. The *first* cell was entered by no step (the ray started inside it, or the
    // boundary clamp put it there), so it takes the negated ray direction instead; `d` is `dir` with
    // its zeros nudged, which changes nothing at unit length.
    var normal = -normalize(d);
    // The face code beside it, on the same three branches: `axis * 2 + (the normal points +)`. It
    // exists for the hit cache alone — the bake itself shades from `normal`.
    var face = HIT_FACE_BOUNDARY;

    // One step per cell crossed, and a ray crosses at most the grid's L1 diameter of them.
    let cap = u32(extent.x + extent.y + extent.z);
    for (var i = 0u; i < cap; i = i + 1u) {
        if (is_occupied(cell)) {
            out.hit = true;
            out.cell = cell;
            out.normal = normal;
            out.face = face;
            return out;
        }
        if (t_max.x <= t_max.y && t_max.x <= t_max.z) {
            cell.x = cell.x + step.x;
            t_max.x = t_max.x + t_delta.x;
            normal = vec3<f32>(-f32(step.x), 0.0, 0.0);
            face = select(0u, 1u, step.x < 0);
            if (cell.x < 0 || cell.x >= extent.x) { return out; }
        } else if (t_max.y <= t_max.z) {
            cell.y = cell.y + step.y;
            t_max.y = t_max.y + t_delta.y;
            normal = vec3<f32>(0.0, -f32(step.y), 0.0);
            face = select(2u, 3u, step.y < 0);
            if (cell.y < 0 || cell.y >= extent.y) { return out; }
        } else {
            cell.z = cell.z + step.z;
            t_max.z = t_max.z + t_delta.z;
            normal = vec3<f32>(0.0, 0.0, -f32(step.z));
            face = select(4u, 5u, step.z < 0);
            if (cell.z < 0 || cell.z >= extent.z) { return out; }
        }
    }
    return out;
}

// One `(probe, k)` entry of the hit cache: the flat cell index in bits 0..24, the face code in
// 25..27. An escaped ray packs to `HIT_FACE_ESCAPED << HIT_FACE_SHIFT` with a zero cell.
fn pack_hit(h: Hit, extent: vec3<i32>) -> u32 {
    if (!h.hit) {
        return HIT_FACE_ESCAPED << HIT_FACE_SHIFT;
    }
    let c = vec3<u32>(h.cell);
    let e = vec3<u32>(extent);
    let flat = c.x + e.x * (c.y + e.y * c.z);
    return (h.face << HIT_FACE_SHIFT) | flat;
}

// Whether the sun reaches `cell`'s `n`-facing surface: one more DDA, from one cell off that face
// toward the sun. Escaped ⇒ 1.0, blocked ⇒ 0.0.
//
// The sun is the one source that must be occluded here. It has no range, so without this every cell
// in a sealed interior would bounce sunlight and the room would flood. Note the deliberate
// divergence from the render pass: this is the *grid's* shadow, not the shadow map's. The shadow map
// is camera-fitted and unavailable to a cached load-time bake, and for an indirect term the grid's
// answer is the right one.
fn sun_shadow(cell: vec3<i32>, n: vec3<f32>, extent: vec3<i32>) -> f32 {
    // One whole cell off the face rather than a small `t` epsilon: the face normal is an exact unit
    // axis, so this lands squarely in the neighbouring cell centre and cannot re-enter the cell it
    // came from through a rounding error.
    let start = vec3<f32>(cell) + vec3<f32>(0.5) + n;
    let h = trace_hit(start, -bake_params.sun_dir.xyz, extent);
    return select(1.0, 0.0, h.hit);
}

// Direct light arriving at `cell`'s `n`-facing surface, in `mesh.wgsl`'s own premultiplied
// convention: `light.color` IS the diffuse response at `ndl = 1`, ranged by `atten^2`. The bounce is
// then consistent with the light the player sees.
//
// No shadow rays for point lights: the grid's own occupancy provides the occlusion coarsely, which
// is the right amount for an indirect term, and the cosine test does most of the work for free — a
// torch on the far side of a wall faces the wall's *other* normal, so `dot(n, l) <= 0` and it
// contributes exactly zero. What survives is light leaking around a corner, which is accepted.
//
// The loop walks the *bin slice* of the cell being shaded, not every light in the buffer. That is
// byte-identical to the flat loop it replaces, not merely equivalent: a light the bin excludes is,
// by its conservative filing rule, out of range of every point in that cell — including this cell
// centre — so its `atten` is exactly 0.0 and its term exactly `+0.0`, and dropping an exact `+0.0`
// from an accumulator that is itself a sum of non-negatives leaves every partial sum bit-identical.
// The survivors keep their relative order, because each bin list ascends by construction.
fn direct_at(cell: vec3<i32>, n: vec3<f32>, extent: vec3<i32>) -> vec3<f32> {
    let world = cell_center(cell, bake_params.grid);
    var acc = vec3<f32>(0.0);

    let slice = bake_bin_table[bin_cell_of(world, bake_params.bin)];
    for (var i = 0u; i < slice.y; i = i + 1u) {
        let light = bake_light_list[bake_bin_indices[slice.x + i]];
        let to_light = light.position.xyz - world;
        let dist = length(to_light);
        let range = max(light.position.w, 1e-3);
        let atten = clamp(1.0 - dist / range, 0.0, 1.0);
        let l = to_light / max(dist, 1e-6);
        let ndl = max(dot(n, l), 0.0);
        acc = acc + light.color.rgb * ndl * atten * atten;
    }

    if (bake_params.sun_dir.w > 0.5) {
        let l_sun = normalize(-bake_params.sun_dir.xyz);
        let ndl_sun = max(dot(n, l_sun), 0.0);
        // A sunless scene pays nothing, and neither does a back-facing surface — the shadow march
        // runs only where the sun could actually land.
        if (ndl_sun > 0.0) {
            acc = acc + bake_params.sun_color.rgb * ndl_sun * sun_shadow(cell, n, extent);
        }
    }
    return acc;
}

// The outgoing radiance of the cell a ray hit: `albedo * (emissive + direct)`.
//
// `albedo *` covers the emissive term too, exactly as `fs_main` forms it (`lit + albedo *
// emissive`), and it is what makes the bounce carry the emitter's colour rather than a white glow.
fn radiance(h: Hit, extent: vec3<i32>) -> vec3<f32> {
    let a = textureLoad(bake_albedo, h.cell, 0).rgb;
    // `r` is the emissive channel of the same texel the march already read for occupancy.
    let e = textureLoad(occupancy, h.cell, 0).r;
    return a * (vec3<f32>(e) + direct_at(h.cell, h.normal, extent));
}

// The nearest free cell to `cell`, searched shell by shell (the 3^3 neighbourhood, then the 5^3) in
// ascending z, y, x within each shell — a fixed order, so which of several equidistant free cells
// wins is a property of the code rather than of the hardware. `cell` itself is known occupied.
// Returns `false` when every cell in reach is solid.
fn push_out(cell: vec3<i32>, extent: vec3<i32>, free: ptr<function, vec3<i32>>) -> bool {
    for (var r = 1; r <= PUSH_OUT_RADIUS; r = r + 1) {
        for (var dz = -r; dz <= r; dz = dz + 1) {
            for (var dy = -r; dy <= r; dy = dy + 1) {
                for (var dx = -r; dx <= r; dx = dx + 1) {
                    // Only this shell: the inner ones were searched by a previous `r`.
                    if (max(max(abs(dx), abs(dy)), abs(dz)) != r) { continue; }
                    let c = cell + vec3<i32>(dx, dy, dz);
                    if (any(c < vec3<i32>(0)) || any(c >= extent)) { continue; }
                    if (!is_occupied(c)) {
                        *free = c;
                        return true;
                    }
                }
            }
        }
    }
    return false;
}

// Writes all four volumes at once, so the buried-probe escape and the ordinary path cannot disagree
// about which of them got a value. Every probe is written; none is ever left undefined.
fn store_all(gid: vec3<i32>, vis: vec4<f32>, r: vec4<f32>, g: vec4<f32>, b: vec4<f32>) {
    textureStore(probes_out, gid, vis);
    if (bake_params.flags.x == 1u) {
        textureStore(bounce_r_out, gid, r);
        textureStore(bounce_g_out, gid, g);
        textureStore(bounce_b_out, gid, b);
    }
}

// The hit cache's base offset for one probe: HIT_RAYS consecutive words at
// `probe_flat * HIT_RAYS`, with `probe_flat` in the probe volumes' own flat order
// (`x + count.x * (y + count.y * z)`). Slot `k` holds ray `k * cache_stride()`.
fn hit_base(gid: vec3<u32>) -> u32 {
    return (gid.x + bake_params.counts.x * (gid.y + bake_params.counts.y * gid.z)) * HIT_RAYS;
}

// Every one of a probe's cache words as an escape — the buried-probe path's counterpart to
// `store_all`'s zeros. A buried probe traces nothing, and a cache entry nobody wrote is a cache
// entry the injection would read as garbage.
fn store_escaped_hits(gid: vec3<u32>) {
    if (bake_params.flags.x != 1u) {
        return;
    }
    let base = hit_base(gid);
    for (var k = 0u; k < HIT_RAYS; k = k + 1u) {
        bake_hit_cache[base + k] = HIT_FACE_ESCAPED << HIT_FACE_SHIFT;
    }
}

@compute @workgroup_size(4, 4, 4)
fn bake(@builtin(global_invocation_id) dispatch_id: vec3<u32>) {
    let gid = dispatch_id + vec3<u32>(0u, 0u, bake_params.flags.z);
    if (any(gid >= bake_params.counts.xyz)) {
        return;
    }

    let extent = vec3<i32>(textureDimensions(occupancy));
    let world = bake_params.probe.xyz + vec3<f32>(gid) * bake_params.probe.w;
    // Grid-cell units: one unit per cell, so the DDA needs no cell size of its own.
    var start = (world - bake_params.grid.xyz) / bake_params.grid.w;

    let cell = vec3<i32>(floor(start));
    if (all(cell >= vec3<i32>(0)) && all(cell < extent) && is_occupied(cell)) {
        var free = vec3<i32>(0);
        if (!push_out(cell, extent, &free)) {
            // A probe buried past the push-out's reach contributes darkness. That is the correct
            // failure direction for both terms: a buried probe reporting sky is precisely the leak
            // the push-out exists to prevent, and one reporting radiance is the same leak, brighter.
            store_all(
                vec3<i32>(gid),
                vec4<f32>(0.0),
                vec4<f32>(0.0),
                vec4<f32>(0.0),
                vec4<f32>(0.0),
            );
            store_escaped_hits(gid);
            return;
        }
        start = vec3<f32>(free) + 0.5;
    }

    let bounce = bake_params.flags.x == 1u;
    let stride = cache_stride();
    let base = hit_base(gid);
    var sh0 = 0.0;
    var sh1 = vec3<f32>(0.0);
    var b0 = vec3<f32>(0.0);
    var b1r = vec3<f32>(0.0);
    var b1g = vec3<f32>(0.0);
    var b1b = vec3<f32>(0.0);
    for (var i = 0u; i < bake_params.flags.y; i = i + 1u) {
        let dir = ray_direction(i, bake_params.flags.y);
        let h = trace_hit(start, dir, extent);
        let v = select(1.0, 0.0, h.hit);
        sh0 = sh0 + v;
        sh1 = sh1 + v * dir;
        // The strided subset this march writes down for the injection to replay. It rides the bounce
        // discriminant because the injection's whole subject is the bounce term.
        if (bounce && (i % stride) == 0u) {
            bake_hit_cache[base + i / stride] = pack_hit(h, extent);
        }
        if (bounce && h.hit) {
            let l = radiance(h, extent);
            b0 = b0 + l;
            b1r = b1r + l.r * dir;
            b1g = b1g + l.g * dir;
            b1b = b1b + l.b * dir;
        }
    }

    let n = f32(bake_params.flags.y);
    // The 3.0 is the L1 band's normalization, so the consumer reconstructs with a plain dot product.
    // `sh0/N` is the cosine-weighted mean incoming radiance the IBL irradiance cube also stores (no
    // 1/PI), so `bounce * diffuse` is unit-for-unit `irradiance * diffuse`.
    store_all(
        vec3<i32>(gid),
        vec4<f32>(sh0 / n, 3.0 * sh1 / n),
        vec4<f32>(b0.r / n, 3.0 * b1r / n),
        vec4<f32>(b0.g / n, 3.0 * b1g / n),
        vec4<f32>(b0.b / n, 3.0 * b1b / n),
    );
}
