// A full-screen view of the winning reflection probe per pixel (`ludion render --debug-view
// probe_index`), drawn in place of the finished picture. The mask walk is `reflections.wgsl`'s: the
// largest `weight × (1 − coverage)` wins, ties to the earlier rank. The pixel is the probe's rank
// colour scaled by that contribution; gray where no probe weights the surface, black where there is
// none. The linearised palette makes the stored sRGB byte `palette × contribution × 255`, so
// `ludion image-stats` reads it directly.
//
// `VsOut` + `vs_main` come from `snippets/fullscreen_vs.wgsl`; `ShadeProbe` and `probe_weight` from
// `snippets/probes.wgsl`, `cluster_cell` from `snippets/cluster_grid.wgsl`, the transfer from
// `snippets/srgb.wgsl`.

// Declared in the reflections pass's order (normal, then depth); the surface test reads the depth.
@group(0) @binding(0) var pdbg_normal: texture_2d<f32>;
@group(0) @binding(1) var pdbg_depth: texture_depth_2d;

struct ProbeDebug {
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    // The view's rank colours (sRGB), indexed by rank modulo 8.
    palette: array<vec4<f32>, 8>,
}
@group(0) @binding(2) var<uniform> pdbg: ProbeDebug;

struct ProbeList {
    // x = the probe count (0 skips the probe path), y = the cube's max LOD, z = `ibl_intensity`.
    header: vec4<f32>,
    // The frame's `LightHeader.grid`, the cluster cell lookup's input.
    grid: vec4<f32>,
    // The ranked probes; bit `k` of a cell's mask is `probes[k]`.
    probes: array<ShadeProbe, 32>,
}
@group(0) @binding(10) var<uniform> probe_list: ProbeList;
// One bit-mask per cluster cell, in the lights' cell order.
@group(0) @binding(11) var<storage, read> probe_mask: array<u32>;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let depth = textureLoad(pdbg_depth, p, 0);
    if (depth >= 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let size = vec2<f32>(textureDimensions(pdbg_depth));
    let ndc = vec2<f32>(2.0 * (in.pos.x / size.x) - 1.0, 1.0 - 2.0 * (in.pos.y / size.y));
    let clip = pdbg.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    let world = clip.xyz / clip.w;

    var best = 0.0;
    var winner = 0u;
    if (u32(probe_list.header.x) > 0u) {
        let cell = cluster_cell(in.pos.xy, dot(world - pdbg.cam_pos.xyz, world - pdbg.cam_pos.xyz), probe_list.grid);
        var mask = probe_mask[cell];
        var coverage = 0.0;
        var taken = 0u;
        loop {
            if (mask == 0u || taken >= 4u || coverage >= 1.0) {
                break;
            }
            let k = firstTrailingBit(mask);
            mask = mask & (mask - 1u);
            let w = probe_weight(probe_list.probes[k], world);
            if (w <= 0.0) {
                continue;
            }
            let contribution = w * (1.0 - coverage);
            if (contribution > best) {
                best = contribution;
                winner = k;
            }
            coverage = coverage + contribution;
            taken = taken + 1u;
        }
    }
    if (best <= 0.0) {
        return vec4<f32>(srgb_to_linear(vec3<f32>(0.25)), 1.0);
    }
    return vec4<f32>(srgb_to_linear(pdbg.palette[winner % 8u].rgb * best), 1.0);
}
