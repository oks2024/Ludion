// Supersampling resolve: an exact area-weighted box filter from the render extent down to the
// output extent.
//
// Destination pixel `i` on an axis owns the source interval `[i*r, (i+1)*r)` for ratio
// `r = source / target`; every texel it touches contributes in proportion to how much of it the
// interval covers, and the weights are normalised by their own sum. At an exactly-integer ratio that
// reduces to a plain average of an r x r block; at 1.5 it is the correct fractional average, which is
// why the fetches are `textureLoad` (exact integer texels, no sampler state, no half-texel offsets to
// get wrong) rather than `textureSample`.
//
// The source is linear: half-float under `resample_owns_output`, `Rgba8UnormSrgb` (decoded to linear by
// `textureLoad`) under a debug view. Each texel is clamped to [0, 1] before mixing, so the average is
// of what an 8-bit store would have held. The target re-encodes on store: the averaging happens in
// linear light, which is the whole point of resolving after tonemap rather than before. As the last
// output pass this applies the grain and dither at the output extent (`output_encode.wgsl`).
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of
// this body (Plan/shader-bindgen.md decision 16).

struct ResolveParams {
    // The source (render-extent) texture size in texels.
    source_extent: vec2<f32>,
    // Source texels per target texel, per axis — always >= 1 (a sub-native scale takes `upscale`).
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

@group(0) @binding(0) var resolve_src: texture_2d<f32>;
// Declared but never sampled: every fullscreen effect binds textures 0..N, a sampler at N and its
// uniform at N+1 (see `FrameRecorder::fullscreen_effect`), and this pass fetches exact texels.
@group(0) @binding(1) var resolve_samp: sampler;
@group(0) @binding(2) var<uniform> resolve_params: ResolveParams;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dst = floor(in.pos.xy);
    let r = resolve_params.ratio;
    let last = vec2<i32>(resolve_params.source_extent) - vec2<i32>(1, 1);

    // The destination pixel's footprint in source-texel coordinates, and the texels it overlaps.
    let lo = dst * r;
    let hi = lo + r;
    let first = vec2<i32>(floor(lo));
    let final_texel = vec2<i32>(ceil(hi)) - vec2<i32>(1, 1);

    var acc = vec3<f32>(0.0);
    var total = 0.0;
    for (var y = first.y; y <= final_texel.y; y = y + 1) {
        let wy = min(hi.y, f32(y + 1)) - max(lo.y, f32(y));
        for (var x = first.x; x <= final_texel.x; x = x + 1) {
            let wx = min(hi.x, f32(x + 1)) - max(lo.x, f32(x));
            let w = wx * wy;
            let texel = vec2<i32>(clamp(x, 0, last.x), clamp(y, 0, last.y));
            acc = acc + clamp(textureLoad(resolve_src, texel, 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * w;
            total = total + w;
        }
    }
    var color = acc / total;
    // The composite's film weight, from the nearest source texel (never filtered).
    let film = textureLoad(resolve_src, clamp(vec2<i32>(floor((dst + vec2<f32>(0.5)) * r)), vec2<i32>(0), last), 0).a;
    if (resolve_params.grain != 0.0 || resolve_params.dither != 0u) {
        color = encoded_output(
            color,
            in.pos.xy,
            resolve_params.grain * film,
            resolve_params.grain_size,
            resolve_params.dither,
            resolve_params.output_height,
            resolve_params.grain_animated,
            resolve_params.grain_tick,
        );
    }
    return vec4<f32>(color, 1.0);
}
