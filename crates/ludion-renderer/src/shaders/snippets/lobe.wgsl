// Twin of `lobe.rs::cube_lod`: the specular cube lod for a roughness, from the GGX lobe's footprint.
// `lobe.rs`'s `needs_gpu_the_lobe_snippet_matches_its_rust_twin` fails on any edit made to one side.
fn specular_cube_lod(roughness: f32, max_lod: f32) -> f32 {
    let r = clamp(roughness, 0.0, 1.0);
    let xc = max(r * r, 1e-6);
    return clamp(max_lod + 0.5 + log2(xc / sqrt(1.0 + xc * xc)), 0.0, max_lod);
}

// Twin of `lobe.rs::MAX_HALF_ANGLE`: keeps `tan` finite in `ssr_pyramid_level`.
const MAX_HALF_ANGLE: f32 = 1.55;

// Twin of `lobe.rs::cone_half_angle`: the half-angle of the cone holding half the GGX lobe.
fn ssr_cone_half_angle(roughness: f32) -> f32 {
    let r = clamp(roughness, 0.0, 1.0);
    return 2.0 * atan(r * r);
}

// Twin of `lobe.rs::pyramid_level`: the colour-pyramid level whose texels span the cone's diameter at
// the hit. `ppu` is the focal length in pixels over the hit's view depth.
fn ssr_pyramid_level(half_angle: f32, hit_distance: f32, ppu: f32) -> f32 {
    let diameter = 2.0 * hit_distance * tan(min(half_angle, MAX_HALF_ANGLE)) * ppu;
    return log2(max(diameter, 1.0));
}
