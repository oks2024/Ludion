// The GPU bone palette: model-space bone matrices for every skinned instance in the frame, prepended
// with an identity entry at upload time so the buffer is never empty and the scene's own entries start
// at offset 1 — `bone_offset == 0` (an unskinned instance) never reads entry 0 or any other, since the
// callers' `if (bone_offset != 0u)` guard skips the blend entirely. Composed into `mesh`, `prepass` and
// `shadow` (Plan/skeletal-animation.md) — the one group all three already bind every frame (`camera`
// for mesh/prepass, `shadow_view` for shadow), so a skinned draw costs no new bind group
// (`@group(0) @binding(1)`, alongside whichever uniform that pass's own group-0 snippet declares).
@group(0) @binding(1) var<storage, read> bone_palette: array<mat4x4<f32>>;

// The linear blend skin matrix for a vertex: `bone_offset` is the instance's base index into the
// uploaded palette (`0` = unskinned, never called — see the callers' `if (bone_offset != 0u)` guard),
// `bones` the four palette indices relative to it, `weights` their blend weights (summing to 1).
fn skin_matrix(offset: u32, bones: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    return bone_palette[offset + bones.x] * weights.x
        + bone_palette[offset + bones.y] * weights.y
        + bone_palette[offset + bones.z] * weights.z
        + bone_palette[offset + bones.w] * weights.w;
}

// A skinned vertex normal: the normalized cofactor of the blended skin matrix's upper 3x3, which
// stays perpendicular to the surface under a non-uniform joint scale where `skin * normal` does not.
// cofactor = det · inverse-transpose; the select undoes a mirroring blend and, unlike sign(),
// keeps a flattened (singular) skin's cofactor.
fn skin_normal(skin: mat4x4<f32>, normal: vec3<f32>) -> vec3<f32> {
    let c0 = skin[0].xyz;
    let c1 = skin[1].xyz;
    let c2 = skin[2].xyz;
    let n = mat3x3<f32>(cross(c1, c2), cross(c2, c0), cross(c0, c1)) * normal
        * select(1.0, -1.0, dot(c0, cross(c1, c2)) < 0.0);
    let len2 = dot(n, n);
    return select(vec3<f32>(0.0), n * inverseSqrt(len2), len2 > 0.0);
}
