// SSR colour pyramid: pre-convolved scene radiance, one blur level per mip, so a reflection cone
// reads the level whose texels span its diameter (`lobe::pyramid_level`).
//
// Seed (`reduce == 0`, mip 0): the area-weighted box over the footprint (`snippets/footprint.wgsl`),
// `hdr_safe` on each fetch — the sharp mirror level, from the opaque scene before reflections add
// into it.
//
// Reduce (`reduce == 1`, mip k >= 1): a separable 4x4 tent with weights [1,3,3,1]/8 over the parent,
// taps `b ..= b+3` per axis with `b = floor(c - 0.5) - 1`, `c = (i + 0.5) * r` (at r = 2 that is
// `2i-1 ..= 2i+2`), edge-clamped. One tent has variance 0.75 parent-texel^2; stacked it gives
// sigma_m ~ 0.5 * 2^m base pixels, so +-sigma spans 2^m pixels — the cone diameter `pyramid_level`
// maps to level m. A plain 2x2 box would match the footprint but look blocky under trilinear.
//
// `VsOut` + `vs_main` come from `shaders/snippets/fullscreen_vs.wgsl`.

struct SsrColorParams {
    // The target mip size in texels.
    target_extent: vec2<f32>,
    // 0 = seed (area box), 1 = reduce (tent).
    reduce: u32,
    pad: u32,
};

@group(0) @binding(0) var ssr_src: texture_2d<f32>;
// Declared but never sampled: exact `textureLoad`s only.
@group(0) @binding(1) var ssr_samp: sampler;
@group(0) @binding(2) var<uniform> ssr_color_params: SsrColorParams;

fn tent_weight(k: i32) -> f32 {
    return select(1.0, 3.0, k == 1 || k == 2) / 8.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let src_dims = textureDimensions(ssr_src);
    let max_texel = vec2<i32>(src_dims) - vec2<i32>(1, 1);
    let r = footprint_ratio(src_dims, ssr_color_params.target_extent);
    let dst = floor(in.pos.xy);
    var acc = vec3<f32>(0.0);

    if (ssr_color_params.reduce == 0u) {
        let lo = footprint_lo(dst, r);
        let first = footprint_first(lo);
        let last = footprint_last(lo, r);
        var total = 0.0;
        for (var y = first.y; y <= last.y; y = y + 1) {
            for (var x = first.x; x <= last.x; x = x + 1) {
                let w = footprint_weight(lo, r, vec2<i32>(x, y));
                let texel = clamp(vec2<i32>(x, y), vec2<i32>(0, 0), max_texel);
                acc = acc + hdr_safe(textureLoad(ssr_src, texel, 0).rgb) * w;
                total = total + w;
            }
        }
        return vec4<f32>(acc / total, 1.0);
    }

    let base = vec2<i32>(floor((dst + vec2<f32>(0.5)) * r - vec2<f32>(0.5))) - vec2<i32>(1, 1);
    for (var j = 0; j < 4; j = j + 1) {
        for (var i = 0; i < 4; i = i + 1) {
            let texel = clamp(base + vec2<i32>(i, j), vec2<i32>(0, 0), max_texel);
            acc = acc + textureLoad(ssr_src, texel, 0).rgb * (tent_weight(i) * tent_weight(j));
        }
    }
    return vec4<f32>(acc, 1.0);
}
