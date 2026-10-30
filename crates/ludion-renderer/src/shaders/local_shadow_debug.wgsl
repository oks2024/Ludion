// A full-screen view of the local-light shadow atlas (`ludion render --debug-view
// local_shadow_atlas`), drawn in place of the finished picture. The atlas is letterboxed into the
// largest centred square; outside it the pixel is black. Each texel is loaded unfiltered: a granted
// cell shows its depth linearised to gray (near bright, far and empty black), an ungranted cell is
// flat dark blue, and a red line marks every cell border.
//
// `VsOut` + `vs_main` come from `snippets/fullscreen_vs.wgsl`; the transfer from `snippets/srgb.wgsl`.

@group(0) @binding(0) var lsdbg_atlas: texture_depth_2d;

struct LocalShadowDebug {
    // x, y = the render extent, z = the atlas edge, w = the cell edge, in texels.
    extent: vec4<f32>,
    // Per atlas cell (row-major, 8 × 8): x = the view's near plane, y = its far plane, z = 1 when a
    // granted view covers the cell.
    cells: array<vec4<f32>, 64>,
}
@group(0) @binding(1) var<uniform> lsdbg: LocalShadowDebug;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let side = min(lsdbg.extent.x, lsdbg.extent.y);
    let origin = vec2<f32>((lsdbg.extent.x - side) * 0.5, (lsdbg.extent.y - side) * 0.5);
    let local = (in.pos.xy - origin) / side;
    if (any(local < vec2<f32>(0.0)) || any(local >= vec2<f32>(1.0))) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let texel = vec2<u32>(local * lsdbg.extent.z);
    let cell_edge = u32(lsdbg.extent.w);
    if (texel.x % cell_edge == 0u || texel.y % cell_edge == 0u) {
        return vec4<f32>(srgb_to_linear(vec3<f32>(1.0, 0.0, 0.0)), 1.0);
    }
    let cell = lsdbg.cells[(texel.y / cell_edge) * 8u + texel.x / cell_edge];
    if (cell.z < 0.5) {
        return vec4<f32>(srgb_to_linear(vec3<f32>(0.05, 0.05, 0.2)), 1.0);
    }
    let d = textureLoad(lsdbg_atlas, vec2<i32>(texel), 0);
    let n = cell.x;
    let f = cell.y;
    let z = n * f / (f - d * (f - n));
    let gray = clamp(1.0 - (z - n) / (f - n), 0.0, 1.0);
    return vec4<f32>(srgb_to_linear(vec3<f32>(gray)), 1.0);
}
