// Shared glyph-atlas fragment stage for the world-space text pass and the screen-space UI text pass:
// the atlas bindings (@group 1), the interpolated `VsOut` and the coverage `fs_main`. Composed ahead
// of each pass's own `VsIn` + `vs_main` (Plan/shader-bindgen.md decision 16), so the fragment stage
// lives in one place instead of two copies.
@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) sdf: f32,
};

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Both coverages are computed, then selected: `textureSample` and `fwidth` must stay in
    // uniform control flow, and the bitmap result stays the sampled alpha untouched. An SDF atlas
    // stores signed distance in alpha with the edge at 0.5; `w` is about one screen pixel of it.
    let a = textureSample(atlas_tex, atlas_sampler, in.uv).a;
    let w = max(fwidth(a), 1e-4) * 0.7;
    let sdf_cov = smoothstep(0.5 - w, 0.5 + w, a);
    let coverage = select(a, sdf_cov, in.sdf > 0.5);
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
