// The per-draw material maps — glTF's metallic-roughness set, a mesh-only @group(4) set once per draw
// (the sprite pass's `SPRITE_TEX_GROUP` is the precedent). One pipeline, no re-batching: which maps a
// draw actually carries lives in `material.flags`, and each term reads its own bit. A draw with a bit
// clear takes the historical arithmetic *unchanged* — no sample, no multiply by a neutral texel —
// which is what keeps an untextured frame byte-identical on every driver (`Plan/textured-pbr.md` G4).
// The 1×1 defaults bound in the clear slots exist only so the bind group is complete; nothing reads
// them. The mesh pass and the SSAO prepass compose this one snippet, so both run the same cutout
// test.
struct MeshMaterial {
    // Which maps are bound, as the `MATERIAL_*` bits below.
    flags: u32,
    // glTF `normalTexture.scale` — how hard the sampled tangent-space xy is scaled.
    normal_scale: f32,
    // glTF `occlusionTexture.strength` — how far the ambient term darkens toward the sampled value.
    occlusion_strength: f32,
    // The `MATERIAL_ALPHA_MASK` cutoff: a fragment whose coverage is below it is discarded. Read only
    // with the mask bit set.
    alpha_cutoff: f32,
};

const MATERIAL_BASE_COLOR: u32 = 1u;
const MATERIAL_METALLIC_ROUGHNESS: u32 = 2u;
const MATERIAL_NORMAL: u32 = 4u;
const MATERIAL_OCCLUSION: u32 = 8u;
const MATERIAL_EMISSIVE: u32 = 16u;
const MATERIAL_ALPHA_MASK: u32 = 32u;
const MATERIAL_DOUBLE_SIDED: u32 = 64u;

@group(4) @binding(0) var base_color_tex: texture_2d<f32>;
@group(4) @binding(1) var metallic_roughness_tex: texture_2d<f32>;
@group(4) @binding(2) var normal_tex: texture_2d<f32>;
@group(4) @binding(3) var occlusion_tex: texture_2d<f32>;
@group(4) @binding(4) var emissive_tex: texture_2d<f32>;
// Trilinear, anisotropic, with the wrap mode the material authored — one sampler for all five maps,
// picked per draw from the renderer's small shared set.
@group(4) @binding(5) var material_samp: sampler;
@group(4) @binding(6) var<uniform> material: MeshMaterial;

// True when the draw is a cutout and this fragment's coverage (vertex × instance alpha, times the
// base-colour map's alpha) falls under the cutoff.
fn material_masked(coverage: f32) -> bool {
    return (material.flags & MATERIAL_ALPHA_MASK) != 0u && coverage < material.alpha_cutoff;
}

// True on the back face of a double-sided draw, whose shading normal flips.
fn material_back_face(front_facing: bool) -> bool {
    return (material.flags & MATERIAL_DOUBLE_SIDED) != 0u && !front_facing;
}
