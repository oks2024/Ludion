// Bloom bright-pass: keep only the HDR energy above 1.0, into the half-res bloom target.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of
// this body (Plan/shader-bindgen.md decision 16).

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

// The bloom bright-pass luminance threshold, from `PostProcess.bloom_threshold` (default 1.0 — the
// former hard-coded literal, so a scene with no `PostProcess`, or a default one, is byte-identical).
// Padded to a 16-byte uniform.
struct BloomThreshold {
    level: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
};
@group(0) @binding(2) var<uniform> bloom: BloomThreshold;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = hdr_safe(textureSample(src_tex, src_sampler, in.uv).rgb);
    // Only the portion of each channel above the threshold blooms; dimmer pixels contribute nothing.
    let bright = max(c - vec3<f32>(bloom.level), vec3<f32>(0.0));
    return vec4<f32>(bright, 1.0);
}
