// Shared glyph-atlas coverage for the world-space text pass and the screen-space UI text pass: the
// atlas bindings (@group 1) and `glyph_coverage`. Composed ahead of each pass's own `VsIn`, `VsOut`
// and stages (Plan/shader-bindgen.md decision 16), so the coverage lives in one place instead of
// two copies.
@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

// Both coverages are computed, then selected: `textureSample` and `fwidth` must stay in uniform
// control flow, and the bitmap result stays the sampled alpha untouched. An SDF atlas stores signed
// distance in alpha with the edge at 0.5; `w` is about one screen pixel of it.
fn glyph_coverage(uv: vec2<f32>, sdf: f32) -> f32 {
    let a = textureSample(atlas_tex, atlas_sampler, uv).a;
    let w = max(fwidth(a), 1e-4) * 0.7;
    let sdf_cov = smoothstep(0.5 - w, 0.5 + w, a);
    return select(a, sdf_cov, sdf > 0.5);
}
