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
// The source is `Rgba8UnormSrgb`, so `textureLoad` returns **linear** values and the target re-encodes
// on store: the averaging happens in linear light, which is the whole point of resolving after
// tonemap rather than before.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of
// this body (Plan/shader-bindgen.md decision 16).

struct ResolveParams {
    // The source (render-extent) texture size in texels.
    source_extent: vec2<f32>,
    // Source texels per target texel, per axis — always >= 1 (a render scale below native is
    // rejected on the CPU side).
    ratio: vec2<f32>,
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
            acc = acc + textureLoad(resolve_src, texel, 0).rgb * w;
            total = total + w;
        }
    }
    return vec4<f32>(acc / total, 1.0);
}
