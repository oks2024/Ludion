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
// and re-quantized through the half-float round trip otherwise) and this pass applies them, unless a
// resample follows: the last output pass owns them, so this pass then runs with both zeroed and
// writes a half-float `ldr`.
//
// `encoded_output` comes from `output_encode.wgsl`. The grain cell is sized from the source's height,
// the render height, exactly as the composite sizes it from `scene_tex`.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`; `BAYER4` from
// `bayer.wgsl`; `linear_to_srgb` / `srgb_to_linear` from `srgb.wgsl`; the grain and dither from
// `output_encode.wgsl`.

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

// The Off path's 8-bit store clamps; the Rgba16Float intermediate does not. Every texel is clamped before
// it is mixed, so the blend mixes what the Off path would have stored.
fn color_at(uv: vec2<f32>) -> vec3<f32> {
    return clamp(
        textureSampleLevel(smaa_blend_src, smaa_blend_samp, uv, 0.0).rgb,
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
}

// What the linear sampler's fetch at `uv + shift * texel` returns, as an explicit mix of two clamped
// texels: `shift` is nonzero along one axis only, and `uv` is a texel centre.
fn color_shifted(uv: vec2<f32>, shift: vec2<f32>, texel: vec2<f32>) -> vec3<f32> {
    return mix(color_at(uv), color_at(uv + sign(shift) * texel), abs(shift.x + shift.y));
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

        // The two taps the reference reads with one bilinear fetch each: the current pixel mixed with the
        // chosen neighbour, one on each side.
        color = blending_weight.x * color_shifted(uv, blending_offset.xy, rt.xy)
            + blending_weight.y * color_shifted(uv, -blending_offset.zw, rt.xy);
    }

    if (smaa_blend_params.grain != 0.0 || smaa_blend_params.dither != 0u) {
        color = encoded_output(
            color,
            in.pos.xy,
            smaa_blend_params.grain,
            smaa_blend_params.grain_size,
            smaa_blend_params.dither,
            f32(textureDimensions(smaa_blend_src).y),
        );
    }
    return vec4<f32>(color, 1.0);
}
