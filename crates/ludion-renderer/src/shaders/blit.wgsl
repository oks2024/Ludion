// Fullscreen passthrough blit: sample the input texture and write it unchanged.
//
// The overlay pass uses this to copy the tonemapped LDR image (an `Rgba8UnormSrgb` transient) into
// the present target before drawing UI on top. Sampling an sRGB texture decodes to linear and the
// sRGB target re-encodes on store, so the blit is an identity (modulo the final 8-bit rounding).
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of
// this body (Plan/shader-bindgen.md decision 16).

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_sampler, in.uv);
}
