// The clustered-forward cell index, binding-free: the grid's shape and the lookup from a fragment's
// framebuffer position and squared camera distance to its cell. `lights.wgsl` indexes its light table
// with it and the `reflections` pass indexes its probe masks; `grid` is the frame's
// `LightHeader.grid` (xy = reciprocal render extent, zw = the shell ladder's scale and bias).
//
// The grid's shape: fixed *counts*, never a fixed pixel size, so the grid is identical at every
// `--scale` — a supersampled frame clusters exactly like its native twin. WGSL is the source
// of truth: `ludion-shader-bindgen` reflects these into `shader_gen`, and the renderer's CPU-side
// build sizes its tables from them.
const CLUSTER_TILES_X: u32 = 16u;
const CLUSTER_TILES_Y: u32 = 9u;
const CLUSTER_SLICES: u32 = 16u;
// The viewmodel's own cell, one past the grid: the light table appends it when a frame draws a
// viewmodel, and gun fragments read it instead of a tile (their screen position is the lens's).
const VIEWMODEL_CELL: u32 = CLUSTER_TILES_X * CLUSTER_TILES_Y * CLUSTER_SLICES;

// Both coordinates clamp on the float side before the u32 conversion, so an edge fragment or a
// distance off either end of the ladder lands in a real cell instead of reading out of bounds.
fn cluster_cell(frag_pos: vec2<f32>, view_dist_sq: f32, grid: vec4<f32>) -> u32 {
    let tile_uv = clamp(frag_pos * grid.xy, vec2<f32>(0.0), vec2<f32>(1.0));
    let tx = min(u32(tile_uv.x * f32(CLUSTER_TILES_X)), CLUSTER_TILES_X - 1u);
    let ty = min(u32(tile_uv.y * f32(CLUSTER_TILES_Y)), CLUSTER_TILES_Y - 1u);
    let ladder = log2(max(view_dist_sq, 1e-12)) * grid.z + grid.w;
    let shell = u32(clamp(ladder, 0.0, f32(CLUSTER_SLICES - 1u)));
    return (shell * CLUSTER_TILES_Y + ty) * CLUSTER_TILES_X + tx;
}
