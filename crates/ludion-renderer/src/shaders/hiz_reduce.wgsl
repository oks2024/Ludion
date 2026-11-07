// Hi-Z reduce (mip k >= 1): the per-cell **minimum** of the parent level over the cell's footprint
// (`snippets/footprint.wgsl`). The parent is the single-mip view of mip k-1, so its size is
// `textureDimensions`; the cell conservatively covers an odd parent's last row or column.
//
// `hiz_src` is `R32Float`, which is unfilterable (`UNFILTERABLE_TEXTURES`): exact `textureLoad`s only.
//
// `VsOut` + `vs_main` come from `shaders/snippets/fullscreen_vs.wgsl`.

struct HizParams {
    // The target (this mip) size in texels.
    target_extent: vec2<f32>,
    // Unused here; the seed reads its viewmodel slice from the lane (one struct, one layout).
    viewmodel_slice: f32,
    pad: f32,
};

@group(0) @binding(0) var hiz_src: texture_2d<f32>;
@group(0) @binding(1) var hiz_samp: sampler;
@group(0) @binding(2) var<uniform> hiz_params: HizParams;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let src_dims = textureDimensions(hiz_src);
    let r = footprint_ratio(src_dims, hiz_params.target_extent);
    let lo = footprint_lo(floor(in.pos.xy), r);
    let first = footprint_first(lo);
    let last = footprint_last(lo, r);
    let max_texel = vec2<i32>(src_dims) - vec2<i32>(1, 1);

    var nearest = 1.0;
    for (var y = first.y; y <= last.y; y = y + 1) {
        for (var x = first.x; x <= last.x; x = x + 1) {
            let texel = clamp(vec2<i32>(x, y), vec2<i32>(0, 0), max_texel);
            nearest = min(nearest, textureLoad(hiz_src, texel, 0).r);
        }
    }
    return vec4<f32>(nearest, 0.0, 0.0, 1.0);
}
