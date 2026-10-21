// The SH-L1 probe reconstruction and the sky-visibility read. Binding-free — the volume and sampler
// arrive as parameters — so the mesh pass and the `reflections` pass share the one function that
// reads the visibility volume (`Plan/global-illumination.md` decision 4).

// The cosine-convolved SH-L1 evaluation (A0 = 1, A1 = 2/3) both probe volumes reconstruct through —
// correct because the term each one carries is a cosine-weighted mean. The bake writes a `3.0`-scaled
// L1 vector, so the band's own normalization is already folded in and this is a plain `dot`.
fn sh_l1(sh: vec4<f32>, n: vec3<f32>) -> f32 {
    return sh.x + (2.0 / 3.0) * dot(sh.yzw, n);
}

// How much of the sky this surface point can reach, in [0, 1] — the ONE function that reads the
// probe volume. An ambient-cube experiment replaces this body and the bake's write, and nothing
// else in the pass has to know.
//
// The sample point is pushed `scale.w` (`normal_bias`) along the normal first, which is the leak
// control: a probe lattice metres apart interpolates across a wall, and offsetting toward the lit
// side is what keeps a surface from reading the darkness sealed behind it. The reconstruction is
// `sh_l1`, clamped to [0, 1] because a visibility fraction has both a floor and a ceiling.
fn sky_visibility_at(
    vol: texture_3d<f32>,
    samp: sampler,
    origin: vec3<f32>,
    scale: vec4<f32>,
    world_pos: vec3<f32>,
    n: vec3<f32>,
) -> f32 {
    let p = world_pos + n * scale.w;
    let uv = (p - origin) * scale.xyz;
    let sh = textureSampleLevel(vol, samp, uv, 0.0);
    return clamp(sh_l1(sh, n), 0.0, 1.0);
}
