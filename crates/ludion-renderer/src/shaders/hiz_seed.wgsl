// Hi-Z seed (mip 0): the per-cell **minimum** depth of `scene_depth`, at the trace extent.
//
// Depth is standard-Z (far = 1.0), and a trace may skip a cell only while the ray is in front of the
// closest surface in it, so each cell keeps the minimum over its footprint (`snippets/footprint.wgsl`:
// every source texel the cell touches, at any render-to-trace ratio). `R32Float`, because fp16 depth
// has too little precision near 1.
//
// `VsOut` + `vs_main` come from `shaders/snippets/fullscreen_vs.wgsl`.

struct HizParams {
    // The target (trace-extent) mip size in texels.
    target_extent: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var hiz_depth: texture_depth_2d;
// Declared but never sampled: fullscreen effects bind textures 0..N, a sampler at N and the uniform
// at N+1; this pass fetches exact texels.
@group(0) @binding(1) var hiz_samp: sampler;
@group(0) @binding(2) var<uniform> hiz_params: HizParams;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let src_dims = textureDimensions(hiz_depth);
    let r = footprint_ratio(src_dims, hiz_params.target_extent);
    let lo = footprint_lo(floor(in.pos.xy), r);
    let first = footprint_first(lo);
    let last = footprint_last(lo, r);
    let max_texel = vec2<i32>(src_dims) - vec2<i32>(1, 1);

    var nearest = 1.0;
    for (var y = first.y; y <= last.y; y = y + 1) {
        for (var x = first.x; x <= last.x; x = x + 1) {
            let texel = clamp(vec2<i32>(x, y), vec2<i32>(0, 0), max_texel);
            nearest = min(nearest, textureLoad(hiz_depth, texel, 0));
        }
    }
    return vec4<f32>(nearest, 0.0, 0.0, 1.0);
}
