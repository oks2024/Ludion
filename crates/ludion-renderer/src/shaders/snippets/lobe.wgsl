// Twin of `lobe.rs::cube_lod`: the specular cube lod for a roughness, from the GGX lobe's footprint.
// `lobe.rs`'s `needs_gpu_the_lobe_snippet_matches_its_rust_twin` fails on any edit made to one side.
fn specular_cube_lod(roughness: f32, max_lod: f32) -> f32 {
    let r = clamp(roughness, 0.0, 1.0);
    let xc = max(r * r, 1e-6);
    return clamp(max_lod + 0.5 + log2(xc / sqrt(1.0 + xc * xc)), 0.0, max_lod);
}
