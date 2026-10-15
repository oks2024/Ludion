// GI dynamic-light injection — one compute entry point that gives every probe the one-bounce
// irradiance arriving from the scene's `GiDynamic`-marked lights, recomputed whole every frame with
// zero history (`Plan/gi-wave3-dynamic-injection.md` §5.2).
//
// IT NEVER MARCHES. `gi_bake.wgsl` already walked the occupancy grid from every probe and wrote down
// which cell each of HIT_RAYS strided rays hit and through which face; static geometry makes that answer
// stable between bakes, so this pass *replays* those HIT_RAYS words instead of tracing again. Two fetches
// per ray where a fresh march is tens of DDA steps — and the leak control comes with it for free,
// because the hit set IS the bake's own visibility: a probe sealed in a room cached no hit outside
// it, so a light outside the wall reaches it through nothing.
//
// WHAT IT INTEGRATES. Per replayed hit: the cell's albedo times the direct light the marked lights
// deposit on the face the ray crossed, in `mesh.wgsl`'s premultiplied convention — the arithmetic of
// `gi_bake.wgsl`'s `direct_at`, verbatim, so the dynamic half of the bounce composes with the baked
// half unit for unit. Three deliberate absences: an escaped ray contributes nothing (the sky is the
// visibility term, and air carries no radiance); the hit cell's *emissive* is not re-added (it is
// static content and already lives in the baked bounce, so adding it here would double-count every
// torch a bolt flies past); and static lights are absent from the bin by construction, so a light
// contributes to exactly one of the two terms.
//
// The output is one RGB DC value per probe — irradiance only, no SH-L1 bands: a third of the memory,
// bandwidth and bindings of the static trio, for a term whose measured population is single-digit,
// ground-up and low-contrast.
//
// NO PER-PROBE EARLY-OUT (the §15 micro-decision, decided here in the code). A probe's cached hits
// can land anywhere the rays reached, so there is no bin-space neighbourhood that conservatively
// bounds them short of reading the HIT_RAYS words — which is the per-hit skip itself. The skip is therefore
// already minimal: one word read and one compare retires an escaped ray, one more table read retires
// a hit whose cell holds no marked light, and the inner loop simply never runs.
//
// Deterministic per machine, not across machines, exactly where the bake sits: `k` ascends, each bin
// list ascends by construction, one invocation per probe, no atomics and no cross-invocation
// reduction — but the accumulation is GPU float arithmetic.
//
// This is a compute pipeline with an auto-derived bind group layout; shader-bindgen reflects its
// uniform PODs, binding indices and workgroup size into `shader_gen.rs`. Its resources carry an
// `inject_` prefix because bindgen unifies resources by name across all shaders.

// `gi_bin.rs`'s `GiBinParams`, the same struct `gi_bake.wgsl` embeds: the dims travel in the uniform
// rather than as constants here so `BIN_DIMS` is the only place the grid's shape is written down.
struct GiBinParams {
    // xyz = the bin box's world minimum corner (the voxel grid's origin), w = pad.
    origin: vec4<f32>,
    // xyz = 1 / bin cell size, w = pad. The reciprocal, so neither side divides.
    inv_cell: vec4<f32>,
    // xyz = bin cells per axis, w = pad.
    dims: vec4<u32>,
};

struct GiInjectParams {
    // xyz = the voxel grid's world-space origin, w = its cell size in world units. The grid's extent
    // comes from `textureDimensions(inject_albedo)`, exactly as the bake takes it from its occupancy input.
    grid: vec4<f32>,
    // xyz = probes per axis, w = how many entries `inject_light_list` holds. The light loop's bound is the
    // bin slice, not this — it describes the buffer, not the walk.
    counts: vec4<u32>,
    bin: GiBinParams,
    // x = the ray count the bake this frame replays actually traced (`QualityParams::gi_rays`),
    // yzw = pad. A whole new vec4 rather than a reclaimed pad lane, because this struct carried
    // none; it is appended, so every prior field's offset is unchanged.
    rays: vec4<u32>,
};

// `snippets/lights.wgsl`'s `Light`, restated here because this shader composes no snippets: xyz =
// world position, w = range; rgb = color * intensity (premultiplied), w = 0; axis = (0, 0, 0, -2),
// the point-light encoding (spots never reach GI). Bindgen unifies it with that snippet's `Light`,
// so the two layouts cannot drift.
struct Light {
    position: vec4<f32>,
    color: vec4<f32>,
    axis: vec4<f32>,
};

@group(0) @binding(0) var<uniform> inject_params: GiInjectParams;
// The grid's per-cell RGBA albedo, `textureLoad`ed at a replayed hit. Never paired with a sampler —
// that is what lets the non-filterable `Rgba32Float` upload satisfy the auto-derived layout with no
// device feature (the rule `gi_bake.rs` states for its own voxel inputs).
@group(0) @binding(1) var inject_albedo: texture_3d<f32>;
// `gi_bake.wgsl`'s hit cache: one packed word per (probe, slot) for the HIT_RAYS strided rays, in the probe
// volumes' own flat order. Bit layout at HIT_FACE_SHIFT below. Slot `k` holds ray
// `k * (inject_params.rays.x / HIT_RAYS)`.
@group(0) @binding(2) var<storage, read> inject_hit_cache: array<u32>;
// The scene's **dynamic** (marked) lights, in `ludion-engine`'s ascending-`EntityId` extract order —
// which is also this shader's float summation order. An unmarked light never reaches this buffer: it
// is baked instead.
@group(0) @binding(3) var<storage, read> inject_light_list: array<Light>;
// `gi_bin.rs`'s world-space bin over `inject_light_list`: one `(offset, count)` per bin cell, then the
// concatenated per-cell index lists. Each cell's list ascends by construction.
@group(0) @binding(4) var<storage, read> inject_bin_table: array<vec2<u32>>;
@group(0) @binding(5) var<storage, read> inject_bin_indices: array<u32>;
// The dynamic irradiance volume, on the static volumes' own probe lattice: rgb = the RGB DC term,
// a = written zero, reserved. Every probe is written every frame — there is no history and no texel
// this pass leaves undefined.
@group(0) @binding(6) var dyn_out: texture_storage_3d<rgba16float, write>;

// How many of the bake's rays the hit cache holds, and therefore the gather's budget. `gi_bake.wgsl`'s
// HIT_RAYS must equal this. It is a constant on both sides because it sizes the cache buffer, unlike
// the ray count above it, which is a quality axis.
const HIT_RAYS: u32 = 16u;
// The reciprocal golden ratio, the spherical Fibonacci sequence's azimuth increment in turns.
const GOLDEN_RATIO_INVERSE: f32 = 0.6180339887498949;
const TAU: f32 = 6.283185307179586;

// The packed hit word's layout, mirroring `gi_bake.wgsl`'s: flat cell index in bits 0..24, face code
// in 25..27. Codes 0..5 are the six axis faces (0 = -X, 1 = +X, 2 = -Y, 3 = +Y, 4 = -Z, 5 = +Z).
const HIT_FACE_SHIFT: u32 = 25u;
// Face code 6: the cell was entered by no DDA step, so its normal is not an axis and is recomputed
// from `k` alone as the negated ray direction.
const HIT_FACE_BOUNDARY: u32 = 6u;
// Face code 7: the ray escaped the grid, and contributes nothing.
const HIT_FACE_ESCAPED: u32 = 7u;

// Ray `i` of the spherical Fibonacci set — `gi_bake.wgsl`'s `ray_direction`, restated so the
// boundary-face normal below is the one that march used. `i` is a ray index, not a cache slot: the
// caller multiplies by the stride.
//
// The count is the *denominator*, never a loop bound here: this pass replays the bake's hits rather
// than tracing its own, and the ray set is re-derived at every count (the elevation divides by it),
// so dividing by anything but the count the bake actually traced reconstructs a direction the march
// never took. `inject_params.rays.x` is that count, carried from the bake's own resolved volumes.
fn ray_direction(i: u32) -> vec3<f32> {
    let y = 1.0 - 2.0 * (f32(i) + 0.5) / f32(inject_params.rays.x);
    let r = sqrt(max(0.0, 1.0 - y * y));
    let phi = TAU * fract(f32(i) * GOLDEN_RATIO_INVERSE);
    return vec3<f32>(r * cos(phi), y, r * sin(phi));
}

// `d` with every component pushed off exact zero, keeping its sign — the bake's own nudge, carried
// here for the same reason the ray set is: the boundary normal it produced was `-normalize(nonzero(dir))`,
// and reproducing it exactly costs three instructions.
fn nonzero(d: vec3<f32>) -> vec3<f32> {
    let m = max(abs(d), vec3<f32>(1e-8));
    return select(m, -m, d < vec3<f32>(0.0));
}

// The surface normal at a replayed hit: the axis face the ray crossed to enter the cell, or — for
// the boundary case the DDA reached by no step — the negated ray direction, recomputed from the slot
// index `k` through the same `k * stride` mapping the bake wrote it under, where the stride is
// `inject_params.rays.x / HIT_RAYS` exactly as `gi_bake.wgsl`'s `cache_stride` derives it. This is the one place
// the whole design needs `k` for anything but indexing, and the one place a slot→ray drift would
// show up as a silently wrong normal rather than a compile error.
fn hit_normal(face: u32, k: u32) -> vec3<f32> {
    if (face == HIT_FACE_BOUNDARY) {
        return -normalize(nonzero(ray_direction(k * (inject_params.rays.x / HIT_RAYS))));
    }
    let axis = face >> 1u;
    // The low bit is "the normal points along +axis": code 0 is -X, code 1 is +X, and so on.
    let sign = select(-1.0, 1.0, (face & 1u) == 1u);
    return vec3<f32>(
        select(0.0, sign, axis == 0u),
        select(0.0, sign, axis == 1u),
        select(0.0, sign, axis == 2u),
    );
}

// The cell a packed word names, un-flattened through the grid's own `x + e.x * (y + e.y * z)`.
fn unpack_cell(word: u32, extent: vec3<u32>) -> vec3<i32> {
    let flat = word & ((1u << HIT_FACE_SHIFT) - 1u);
    let plane = extent.x * extent.y;
    return vec3<i32>(
        i32(flat % extent.x),
        i32((flat / extent.x) % extent.y),
        i32(flat / plane),
    );
}

// The world-space centre of `cell` — the point both the bin lookup and the light arithmetic evaluate
// at, exactly as the bake's `direct_at` does.
fn cell_center(cell: vec3<i32>) -> vec3<f32> {
    return inject_params.grid.xyz + (vec3<f32>(cell) + vec3<f32>(0.5)) * inject_params.grid.w;
}

// The bin cell a world position falls in — `gi_bin.rs`'s `axis_index`/`cell_index` mirrored operation
// for operation, and `gi_bake.wgsl`'s `bin_cell_of` verbatim: subtract the origin, multiply by the
// stored reciprocal, floor, clamp in float, and only then truncate.
fn bin_cell_of(p: vec3<f32>) -> u32 {
    let dims = vec3<f32>(inject_params.bin.dims.xyz);
    let f = floor((p - inject_params.bin.origin.xyz) * inject_params.bin.inv_cell.xyz);
    let c = vec3<u32>(clamp(f, vec3<f32>(0.0), dims - vec3<f32>(1.0)));
    return (c.z * inject_params.bin.dims.y + c.y) * inject_params.bin.dims.x + c.x;
}

// Direct light the marked lights deposit on `cell`'s `n`-facing surface, times that surface's albedo.
//
// The per-light arithmetic is `gi_bake.wgsl`'s `direct_at`, verbatim — premultiplied colour, the
// `max(range, 1e-3)` floor, the hard zero at range — never re-derived, because the two halves of one
// bounce term disagreeing about what a light does at 0.9 of its range is a seam nobody would find.
// The loop walks the shaded cell's *bin slice*: a light the bin excludes is out of range of every
// point in that cell, so its `atten` is exactly 0.0 and its term exactly `+0.0`, and dropping an
// exact `+0.0` from an accumulator that is a sum of non-negatives leaves every partial sum
// bit-identical (the culled-vs-degenerate oracle is what says so).
//
// No shadow rays and no sun: the sun is always static, and a point light's occlusion at this scale
// is what the cached hit already answered.
fn outgoing_at(cell: vec3<i32>, n: vec3<f32>) -> vec3<f32> {
    let world = cell_center(cell);
    let a = textureLoad(inject_albedo, cell, 0).rgb;
    var acc = vec3<f32>(0.0);

    let slice = inject_bin_table[bin_cell_of(world)];
    for (var i = 0u; i < slice.y; i = i + 1u) {
        let light = inject_light_list[inject_bin_indices[slice.x + i]];
        let to_light = light.position.xyz - world;
        let dist = length(to_light);
        let range = max(light.position.w, 1e-3);
        let atten = clamp(1.0 - dist / range, 0.0, 1.0);
        let l = to_light / max(dist, 1e-6);
        let ndl = max(dot(n, l), 0.0);
        acc = acc + a * light.color.rgb * ndl * atten * atten;
    }
    return acc;
}

@compute @workgroup_size(4, 4, 4)
fn inject(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (any(gid >= inject_params.counts.xyz)) {
        return;
    }

    let extent = vec3<u32>(textureDimensions(inject_albedo));
    // The cache's flat order is the probe volumes' own: `x + count.x * (y + count.y * z)`.
    let base = (gid.x + inject_params.counts.x * (gid.y + inject_params.counts.y * gid.z)) * HIT_RAYS;

    var acc = vec3<f32>(0.0);
    for (var k = 0u; k < HIT_RAYS; k = k + 1u) {
        let word = inject_hit_cache[base + k];
        let face = word >> HIT_FACE_SHIFT;
        if (face == HIT_FACE_ESCAPED) {
            continue;
        }
        acc = acc + outgoing_at(unpack_cell(word, extent), hit_normal(face, k));
    }

    // `(1/HIT_RAYS) Σ L` — the static DC term's own estimator over HIT_RAYS of its directions, so `dyn × diffuse`
    // composes with `irradiance × diffuse` and with the baked `bounce × diffuse` unit for unit.
    textureStore(dyn_out, vec3<i32>(gid), vec4<f32>(acc / f32(HIT_RAYS), 0.0));
}
