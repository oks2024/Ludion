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
// The source is linear: half-float under `resample_owns_output`, `Rgba8UnormSrgb` (decoded to linear by
// `textureLoad`) under a debug view; each texel is clamped to [0, 1] before mixing. The target
// re-encodes on store: the blend happens in linear light, which is the same reasoning the resolve
// states. As the last output pass this applies the grain and dither at the output extent.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of
// this body (Plan/shader-bindgen.md decision 16).

struct UpscaleParams {
    // The source (render-extent) texture size in texels.
    source_extent: vec2<f32>,
    // Source texels per target texel, per axis — always <= 1 (a render extent at or above the output
    // extent resolves instead).
    ratio: vec2<f32>,
    // The film grain amplitude (0 off), its cell as a fraction of the output height, the ordered
    // dither flag, and the output height in texels: the resample is the last output pass, so it
    // applies the grain and dither at the output extent (`output_encode.wgsl`).
    grain: f32,
    grain_size: f32,
    dither: u32,
    output_height: f32,
    // Nonzero folds `grain_tick` (the game tick) into the grain hash.
    grain_animated: u32,
    grain_tick: u32,
    pad0: u32,
    pad1: u32,
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
    let c00 = clamp(textureLoad(upscale_src, vec2<i32>(lo.x, lo.y), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let c10 = clamp(textureLoad(upscale_src, vec2<i32>(hi.x, lo.y), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let c01 = clamp(textureLoad(upscale_src, vec2<i32>(lo.x, hi.y), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let c11 = clamp(textureLoad(upscale_src, vec2<i32>(hi.x, hi.y), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));

    let top = mix(c00, c10, frac.x);
    let bottom = mix(c01, c11, frac.x);
    var color = mix(top, bottom, frac.y);
    if (upscale_params.grain != 0.0 || upscale_params.dither != 0u) {
        color = encoded_output(
            color,
            in.pos.xy,
            upscale_params.grain,
            upscale_params.grain_size,
            upscale_params.dither,
            upscale_params.output_height,
            upscale_params.grain_animated,
            upscale_params.grain_tick,
        );
    }
    return vec4<f32>(color, 1.0);
}
