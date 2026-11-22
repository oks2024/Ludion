// The sun and local-light shadow bindings, @group(2): split out of `mesh_shading.wgsl` so a shader that only
// takes shadow taps (the rain, particles) composes them without the mesh lobes. Compose it immediately
// before `mesh_shading.wgsl` where both are listed.

// The directional "sun" — a mesh-only @group(2) (Plan/shadow-mapping.md decision 3), so the shared
// camera/lights layouts and the sprite/text shaders are untouched. `sun.color` is premultiplied and
// is zero for a sunless frame (a zeroed uniform), which makes the whole sun term vanish with no
// shader permutation. The shadow map is a comparison-sampled depth texture; PCF taps below.
struct Sun {
    // World→light-clip (orthographic, wgpu 0..1 depth). Zero for a frame that draws no shadow map —
    // a sunless scene, or a machine whose quality masks shadows off — which sends `sun_shadow` down
    // its fully-lit early-out. The colour beside it is untouched either way, so masking shadows
    // leaves the sun lighting the scene.
    light_space: mat4x4<f32>,
    // xyz = normalized direction the light travels; w = shadow depth bias in light-NDC depth (extract converts the authored world units).
    direction: vec4<f32>,
    // rgb = color * intensity (premultiplied), w unused. Zero for a sunless frame.
    color: vec4<f32>,
    // The PCF kernel's edge length in taps (`QualityParams.pcf`): 3 averages a 3×3, 1 takes the
    // centre tap alone. Odd, because the kernel is centred on the shaded texel. A uniform rather
    // than a `const` because it is a **quality** setting — the same call `ssao.wgsl`'s `samples`
    // makes, on the client's side of the split between what the machine pays for and what the scene
    // authors.
    pcf: u32,
    // The disc's angular radius θ, for the specular representative point: sin θ, cos θ and
    // tan θ / 2 (the roughness widening). All zero for a point light or a sunless frame; `disc_sin`
    // is the gate. The cascade lanes below start 16-aligned, so the fields above are 112 bytes.
    disc_sin: f32,
    disc_cos: f32,
    disc_spread: f32,
    // Per cascade, nearest first; lanes count.. are zero. A count of 0 keeps the anchored box above.
    cascade_space: array<mat4x4<f32>, 4>,
    // xy = the tile's UV origin in the atlas, zw = its UV size (a 2×1 atlas has non-square UV tiles).
    cascade_rect: array<vec4<f32>, 4>,
    // x = near view depth (m), y = far view depth (m), z = depth bias in this cascade's light-NDC, w = texels
    // of this cascade's tile per unit of light-NDC depth that the penumbra radius grows by; 0 = hard PCF.
    cascade_params: array<vec4<f32>, 4>,
    // View depth of p = dot(view_depth.xyz, p) + view_depth.w: the negated third row of the camera's view matrix.
    view_depth: vec4<f32>,
    cascade_count: u32,
    // Disc-tap counts of the PCSS blocker search and comparison filter (`QualityParams.pcss_blocker_taps` /
    // `pcss_filter_taps`), read only where the sun has an angular radius.
    pcss_blocker_taps: u32,
    pcss_filter_taps: u32,
    // The anchored map's penumbra growth, as `cascade_params[i].w` is a cascade's: texels of the whole map
    // per unit of light-NDC depth; 0 = hard PCF.
    penumbra_anchored: f32,
};

@group(2) @binding(0) var<uniform> sun: Sun;
@group(2) @binding(1) var shadow_tex: texture_depth_2d;
@group(2) @binding(2) var shadow_samp: sampler_comparison;

// Local-light shadows: the shadowed spot and point lights' views in one depth atlas.
// They ride this mesh-only group beside the sun, not the shared lights group, so `Light` stays 48
// bytes and the sprite/text shaders never see them (sprites do not receive). `light_shadow` holds
// one entry per light-list index: the index of that light's first view in `local_views`, or
// `0xFFFFFFFF` for a light with no shadow, which `local_shadow` answers with an exact 1.0 before
// reading anything else. A frame with no shadowed light binds the 1×1 fallback depth texture and a
// `light_shadow` of one `0xFFFFFFFF` entry per light (at least one), so nothing here is ever sampled.
struct LocalView {
    // World→light-clip, as the sun's: perspective, wgpu 0..1 depth.
    light_space: mat4x4<f32>,
    // xy = the tile's UV origin in the atlas, z = its UV edge, w = one atlas texel in UV.
    rect: vec4<f32>,
    // x = depth bias in world units, already scaled to the atlas size; y = world size of one tile
    // texel per unit of distance from the light; z = how many views this light has (1 or 6).
    // w = distance-fade strength in (0, 1]: the shadow term scales toward lit as it falls.
    params: vec4<f32>,
};

@group(2) @binding(3) var local_shadow_tex: texture_depth_2d;
@group(2) @binding(4) var<storage, read> local_views: array<LocalView>;
@group(2) @binding(5) var<storage, read> light_shadow: array<u32>;

const UNSHADOWED: u32 = 0xFFFFFFFFu;
// How far along the surface normal the receiver is pushed before projecting, in tile texels. It
// takes the grazing-angle acne that the constant bias along the light vector cannot.
const NORMAL_OFFSET_TEXELS: f32 = 1.0;
// The penumbra radius cap per map or cascade, in that map's texels: a large angular radius saturates here
// instead of growing the kernel, and the blocker-search radius is capped by it too.
const MAX_PENUMBRA_TEXELS: f32 = 16.0;
