// Sub-native magnification: a bilinear filter from the render extent up to the output extent.
//
// The destination pixel's centre `dst + 0.5` sits at source coordinate `(dst + 0.5) * r - 0.5` for
// ratio `r = source / target`; the four texels straddling it are weighted by that coordinate's
// fractional part and lerped, and the coordinate clamps at the edges so a border pixel repeats its
// nearest texel instead of wrapping.
//
// This is not the `resolve` pass's filter and cannot be: an area-weighted box over a *magnified*
// footprint sees one source texel per destination pixel almost everywhere, which is
// nearest-neighbour with extra arithmetic, and the frame arrives as blocks.
//
// The fetches are `textureLoad` for the reason the resolve's are — exact integer texels, no sampler
// state, no half-texel offset to get wrong. The half texel is instead written once, above and below,
// where it can be read.
//
// The source is `Rgba8UnormSrgb`, so `textureLoad` returns **linear** values and the target re-encodes
// on store: the blend happens in linear light, which is the same reasoning the resolve states.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of
// this body (Plan/shader-bindgen.md decision 16).

struct UpscaleParams {
    // The source (render-extent) texture size in texels.
    source_extent: vec2<f32>,
    // Source texels per target texel, per axis — always <= 1 (a render extent at or above the output
    // extent resolves instead).
    ratio: vec2<f32>,
};

@group(0) @binding(0) var upscale_src: texture_2d<f32>;
// Declared but never sampled: every fullscreen effect binds textures 0..N, a sampler at N and its
// uniform at N+1 (see `FrameRecorder::fullscreen_effect`), and this pass fetches exact texels.
@group(0) @binding(1) var upscale_samp: sampler;
@group(0) @binding(2) var<uniform> upscale_params: UpscaleParams;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let last = vec2<i32>(upscale_params.source_extent) - vec2<i32>(1, 1);

    // The destination pixel's centre in source-texel coordinates, shifted by the half texel that puts
    // integer coordinates back on the source grid's own centres.
    let src = (floor(in.pos.xy) + vec2<f32>(0.5, 0.5)) * upscale_params.ratio - vec2<f32>(0.5, 0.5);
    let base = floor(src);
    let frac = src - base;

    let lo = clamp(vec2<i32>(base), vec2<i32>(0, 0), last);
    let hi = clamp(vec2<i32>(base) + vec2<i32>(1, 1), vec2<i32>(0, 0), last);
    let c00 = textureLoad(upscale_src, vec2<i32>(lo.x, lo.y), 0).rgb;
    let c10 = textureLoad(upscale_src, vec2<i32>(hi.x, lo.y), 0).rgb;
    let c01 = textureLoad(upscale_src, vec2<i32>(lo.x, hi.y), 0).rgb;
    let c11 = textureLoad(upscale_src, vec2<i32>(hi.x, hi.y), 0).rgb;

    let top = mix(c00, c10, frac.x);
    let bottom = mix(c01, c11, frac.x);
    return vec4<f32>(mix(top, bottom, frac.y), 1.0);
}
