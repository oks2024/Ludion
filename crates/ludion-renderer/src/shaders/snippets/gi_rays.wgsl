// The GI bake's ray set and its geometry helpers, shared by `gi_bake.wgsl` (which traces the rays) and
// `gi_inject.wgsl` (which replays the hits it cached). Binding-free: each caller passes the uniform
// fields it reads. The injection reproduces the bake's boundary normal and bin lookup exactly, so the
// two halves of one bounce term cannot disagree about either. Needs `math.wgsl` ahead of it.

// Where the world-space light bin sits and how big its cells are — `gi_bin.rs`'s `GiBinParams`.
// The dims travel in the uniform rather than as constants so `gi_bin.rs`'s `BIN_DIMS` is the only
// place the grid's shape is written down.
struct GiBinParams {
    // xyz = the bin box's world minimum corner (the voxel grid's origin), w = pad.
    origin: vec4<f32>,
    // xyz = 1 / bin cell size, w = pad. The reciprocal, so neither side divides.
    inv_cell: vec4<f32>,
    // xyz = bin cells per axis, w = pad.
    dims: vec4<u32>,
};

// The reciprocal golden ratio, the spherical Fibonacci sequence's azimuth increment in turns.
const GOLDEN_RATIO_INVERSE: f32 = 0.6180339887498949;

// Ray `i` of the `rays`-ray spherical Fibonacci set: evenly spaced in cos(elevation), advanced by the
// golden angle in azimuth. `fract` on the turn count keeps the azimuth exact in f32 however long the
// sequence runs. `i` is a ray index, not a cache slot: a caller replaying the cache multiplies by the
// stride first.
//
// The elevation divides by the ray count, so the set is *re-derived* at every count rather than
// truncated — ray `i` of a 32-ray set is nowhere near ray `i` of the 64-ray one. Each count is a
// complete, well-formed sphere on its own terms, which is what makes the count a quality axis; it is
// also why the injection must pass the count the bake actually traced, never its own: any other
// denominator reconstructs a direction the march never took.
fn ray_direction(i: u32, rays: u32) -> vec3<f32> {
    let y = 1.0 - 2.0 * (f32(i) + 0.5) / f32(rays);
    let r = sqrt(max(0.0, 1.0 - y * y));
    let phi = TAU * fract(f32(i) * GOLDEN_RATIO_INVERSE);
    return vec3<f32>(r * cos(phi), y, r * sin(phi));
}

// `d` with every component pushed off exact zero, keeping its sign. The DDA divides by each
// component, and an axis-aligned ray would otherwise produce an infinity that turns into a NaN the
// moment the origin also sits on that axis's slab. The injection's boundary normal is
// `-normalize(nonzero(dir))`, the one the march produced.
fn nonzero(d: vec3<f32>) -> vec3<f32> {
    let m = max(abs(d), vec3<f32>(1e-8));
    return select(m, -m, d < vec3<f32>(0.0));
}

// The world-space centre of `cell`; `grid` is xyz = the voxel grid's origin, w = its cell size.
fn cell_center(cell: vec3<i32>, grid: vec4<f32>) -> vec3<f32> {
    return grid.xyz + (vec3<f32>(cell) + vec3<f32>(0.5)) * grid.w;
}

// The bin cell a world position falls in — `gi_bin.rs`'s `axis_index`/`cell_index` mirrored
// operation for operation: subtract the origin, multiply by the stored reciprocal, floor, clamp in
// float, and only then truncate. A divergence here is a hole no test of either side alone can see;
// the binned-vs-degenerate bake oracle is what catches it.
fn bin_cell_of(p: vec3<f32>, bin: GiBinParams) -> u32 {
    let dims = vec3<f32>(bin.dims.xyz);
    let f = floor((p - bin.origin.xyz) * bin.inv_cell.xyz);
    let c = vec3<u32>(clamp(f, vec3<f32>(0.0), dims - vec3<f32>(1.0)));
    return (c.z * bin.dims.y + c.y) * bin.dims.x + c.x;
}
