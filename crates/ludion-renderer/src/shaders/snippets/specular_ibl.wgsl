// The sky's specular IBL for one surface point: the prefiltered cube at the lobe's lod, times the
// split-sum scale and bias. Binding-free — the textures and sampler arrive as parameters — so any
// pipeline composes it (`Plan/reflections.md` decision 6). Its callers are the `reflections` pass;
// any future transparent mesh shades its sky reflection through this function too.
//
// Composed after `lobe.wgsl`, whose `specular_cube_lod` picks the cube's mip.
fn sky_specular(
    cube: texture_cube<f32>,
    lut: texture_2d<f32>,
    samp: sampler,
    r: vec3<f32>,
    ndv: f32,
    roughness: f32,
    f0: vec3<f32>,
    max_lod: f32,
) -> vec3<f32> {
    let lod = specular_cube_lod(roughness, max_lod);
    let prefiltered = textureSampleLevel(cube, samp, r, lod).rgb;
    let ab = textureSampleLevel(lut, samp, vec2<f32>(ndv, roughness), 0.0).rg;
    return prefiltered * dfg_specular(ab, f0);
}
