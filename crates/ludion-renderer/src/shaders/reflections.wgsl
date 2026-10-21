// The reflections pass: a fullscreen additive draw over the opaque scene that adds the indirect
// specular of the sky. It reads the opaque pass's surface description (normal + roughness, F0 +
// specular occlusion, depth) and shades the sky cube through the shared `sky_specular` snippet;
// `mesh.wgsl` adds no indirect specular itself (`Plan/reflections.md` decision 5).
//
// The first three inputs are bound in the order the frame graph declares the pass's reads, so the
// bind group follows the graph.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`.

// World normal in `xyz`, the clamped roughness in `w`; zero where no opaque surface was drawn.
@group(0) @binding(0) var refl_normal: texture_2d<f32>;
// F0 in `rgb`, specular occlusion (SSAO × occlusion map × the hit-flash complement) in `a`.
@group(0) @binding(1) var refl_specular: texture_2d<f32>;
// The opaque pass's depth, at the render extent (loaded, never filtered).
@group(0) @binding(2) var refl_depth: texture_depth_2d;
// The `Environment`'s roughness-mipped specular cube and split-sum BRDF LUT.
@group(0) @binding(3) var refl_sky_cube: texture_cube<f32>;
@group(0) @binding(4) var refl_brdf_lut: texture_2d<f32>;
// The sky-visibility probe volume (a 1×1×1 fallback when the scene has no `GiVolume`).
@group(0) @binding(5) var refl_gi: texture_3d<f32>;
@group(0) @binding(6) var refl_samp: sampler;

struct Reflections {
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    // x = ibl flag (1 = on), y = ibl_intensity, z = specular max LOD, w = gi sky-visibility strength.
    env: vec4<f32>,
    // xyz = the probe volume's world-space origin, w = the volume flag (1 = on).
    gi_origin: vec4<f32>,
    // xyz = the reciprocal of the volume's world-space span, w = `normal_bias`.
    gi_scale: vec4<f32>,
}
@group(0) @binding(7) var<uniform> reflections: Reflections;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    // No `Environment`: nothing to add. First, so such a frame never reaches a sample and 0 + dst is
    // exact.
    if (reflections.env.x <= 0.5) {
        return vec4<f32>(0.0);
    }
    // Far depth is the sky, which `transparents` draws later; a sky texel would otherwise pick up
    // `prefiltered × ab.y` even at zero F0.
    let depth = textureLoad(refl_depth, p, 0);
    if (depth >= 1.0) {
        return vec4<f32>(0.0);
    }
    let size = vec2<f32>(textureDimensions(refl_depth));
    let ndc = vec2<f32>(2.0 * (in.pos.x / size.x) - 1.0, 1.0 - 2.0 * (in.pos.y / size.y));
    let clip = reflections.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    let world = clip.xyz / clip.w;

    let nr = textureLoad(refl_normal, p, 0);
    let n = normalize(nr.xyz);
    let roughness = nr.w;
    let spec = textureLoad(refl_specular, p, 0);
    let v = normalize(reflections.cam_pos.xyz - world);
    let ndv = max(dot(n, v), 1e-4);
    let r = reflect(-v, n);
    var s = sky_specular(refl_sky_cube, refl_brdf_lut, refl_samp, r, ndv, roughness, spec.rgb, reflections.env.z)
        * reflections.env.y * spec.a;
    // Sky visibility darkens the sky fill only (decision 7), and here the sky fill is the whole term.
    if (reflections.gi_origin.w > 0.5) {
        s = s * mix(
            1.0,
            sky_visibility_at(refl_gi, refl_samp, reflections.gi_origin.xyz, reflections.gi_scale, world, n),
            reflections.env.w,
        );
    }
    return vec4<f32>(hdr_safe(s), 0.0);
}
