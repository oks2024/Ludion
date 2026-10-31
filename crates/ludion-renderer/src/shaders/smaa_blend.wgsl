// SMAA 1x, pass 3: neighbourhood blending, then the film grain and output dither. A port of
// `SMAANeighborhoodBlendingPS` from SMAA (<https://github.com/iryoku/smaa>), MIT-licensed; the notice
// is in `THIRD-PARTY-NOTICES.md` and `licenses/SMAA-MIT.txt`.
//
// Copyright (C) 2013 Jorge Jimenez, Jose I. Echevarria, Belen Masia, Fernando Navarro, Diego
// Gutierrez.
//
// The blend runs in linear light on the half-float composite, with the linear sampler's bilinear
// fetch doing the two-tap mix, and writes the sRGB 8-bit output the composite would have. That makes
// this pass the last stage of the chain, so it owns what the composite pins as its last two steps:
// the grain and the dither. With SMAA on, `composite` runs with both zeroed (they would be smoothed
// and re-quantized through the half-float round trip otherwise) and this pass applies them.
//
// `hash32`, `grain_offset` and `encoded_output` below are copies of `composite.wgsl`'s, operation for
// operation, under this pass's own uniform names: that file stays frozen and its text is not a
// snippet. Keep the two in step. The grain cell is sized from the source's height, the render height,
// exactly as the composite sizes it from `scene_tex`, and `in.pos.xy` is the same render-extent
// position, so the pattern is the one the composite would have drawn.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`; `BAYER4` from
// `bayer.wgsl`; `linear_to_srgb` / `srgb_to_linear` from `srgb.wgsl`.

struct SmaaBlendParams {
    // (1 / width, 1 / height, width, height) of the render extent.
    rt_metrics: vec4<f32>,
    // The film grain amplitude (0 off) and its cell as a fraction of the frame height.
    grain: f32,
    grain_size: f32,
    // Nonzero enables the ordered output dither.
    dither: u32,
    pad: u32,
};

@group(0) @binding(0) var smaa_blend_src: texture_2d<f32>;
@group(0) @binding(1) var smaa_blend_weights: texture_2d<f32>;
@group(0) @binding(2) var smaa_blend_samp: sampler;
@group(0) @binding(3) var<uniform> smaa_blend_params: SmaaBlendParams;

// The grain cell's floor, in render pixels (see `composite.wgsl`).
const MIN_GRAIN_CELL: f32 = 1.0;

fn hash32(seed: u32) -> u32 {
    var x = seed;
    x = x ^ (x >> 17u);
    x = x * 0xed5ad4bbu;
    x = x ^ (x >> 11u);
    x = x * 0xac4c1b51u;
    x = x ^ (x >> 15u);
    x = x * 0x31848babu;
    x = x ^ (x >> 14u);
    return x;
}

fn grain_offset(frag: vec2<f32>) -> f32 {
    let cell_px = max(
        smaa_blend_params.grain_size * f32(textureDimensions(smaa_blend_src).y),
        MIN_GRAIN_CELL,
    );
    let cell = vec2<u32>(floor(frag / cell_px));
    let noise = f32(hash32(cell.x ^ hash32(cell.y)) >> 9u) + 0.5;
    return (noise * (1.0 / 4194304.0) - 1.0) * smaa_blend_params.grain;
}

fn encoded_output(color: vec3<f32>, frag: vec2<f32>) -> vec3<f32> {
    var offset = 0.0;
    if (smaa_blend_params.grain != 0.0) {
        offset = offset + grain_offset(frag);
    }
    if (smaa_blend_params.dither != 0u) {
        let bx = u32(frag.x) % 4u;
        let by = u32(frag.y) % 4u;
        offset = offset + (BAYER4[by * 4u + bx] + 1.0 / 32.0 - 0.5) / 255.0;
    }
    return srgb_to_linear(linear_to_srgb(color) + offset);
}

fn color_at(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(smaa_blend_src, smaa_blend_samp, uv, 0.0).rgb;
}

fn weights_at(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(smaa_blend_weights, smaa_blend_samp, uv, 0.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let rt = smaa_blend_params.rt_metrics;
    let uv = in.pos.xy * rt.xy;
    let offset = rt.xyxy * vec4<f32>(1.0, 0.0, 0.0, 1.0) + uv.xyxy;

    // The blending weights for this pixel: right, top, bottom, left.
    var a: vec4<f32>;
    a.x = weights_at(offset.xy).a;
    a.y = weights_at(offset.zw).g;
    let own = weights_at(uv);
    a.w = own.x;
    a.z = own.z;

    var color: vec3<f32>;
    if (dot(a, vec4<f32>(1.0, 1.0, 1.0, 1.0)) < 1e-5) {
        color = color_at(uv);
    } else {
        let h = max(a.x, a.z) > max(a.y, a.w); // max(horizontal) > max(vertical)

        var blending_offset = vec4<f32>(0.0, a.y, 0.0, a.w);
        var blending_weight = a.yw;
        if (h) {
            blending_offset = vec4<f32>(a.x, 0.0, a.z, 0.0);
            blending_weight = a.xz;
        }
        blending_weight = blending_weight / dot(blending_weight, vec2<f32>(1.0, 1.0));

        // The bilinear fetch mixes the current pixel with the chosen neighbour.
        let blending_coord = blending_offset * vec4<f32>(rt.xy, -rt.xy) + uv.xyxy;
        color = blending_weight.x * color_at(blending_coord.xy)
            + blending_weight.y * color_at(blending_coord.zw);
    }

    if (smaa_blend_params.grain != 0.0 || smaa_blend_params.dither != 0u) {
        color = encoded_output(color, in.pos.xy);
    }
    return vec4<f32>(color, 1.0);
}
