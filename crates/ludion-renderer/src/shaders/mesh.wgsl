// Low-poly mesh pass: instanced, depth-tested, physically-based, into the HDR target.
//
// Cook-Torrance GGX/Smith/Schlick per point or spot light + ambient. Light units keep the Lambert-era
// convention: a light's premultiplied color is the diffuse response at ndl = 1 (no 1/PI on the
// diffuse lobe), and the specular lobe is scaled by PI so the two lobes stay physically
// proportioned — pre-BRDF scenes keep their brightness and gain highlights. Colors can exceed 1.0
// under bright lights or emissive surfaces, which is what feeds the bloom pass.

// The `Camera` uniform (@group 0), the clustered-forward light set (@group 1: the `LightHeader`
// uniform, `light_list`, `cluster_table`, `cluster_lights`) and the shared `BAYER4` dither tile are
// snippets composed ahead of this file (`shaders/snippets/camera.wgsl`, `snippets/lights.wgsl`,
// `snippets/bayer.wgsl`) — see Plan/shader-bindgen.md decision 16. This body declares only what is
// unique to the mesh pass.

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
    // is the gate. The matrices give this struct 16-byte alignment, so its size is 112 either way.
    disc_sin: f32,
    disc_cos: f32,
    disc_spread: f32,
};

@group(2) @binding(0) var<uniform> sun: Sun;
@group(2) @binding(1) var shadow_tex: texture_depth_2d;
@group(2) @binding(2) var shadow_samp: sampler_comparison;

// Image-based lighting environment — a mesh-only @group(3), sourced from the scene's `Environment`
// (Environment/IBL). The textures are runtime-prefiltered from the analytic sky gradient (never an
// image asset); this pass binds the diffuse irradiance cube, and the `reflections` pass binds the
// roughness-mipped specular cube and the split-sum BRDF LUT. `env.params.x` is the on/off flag: 0 (no `Environment`, 1×1 fallbacks bound)
// selects the exact historical flat-ambient math below — byte-identical to before; 1 selects IBL.
//
// Screen-space ambient occlusion rides this group too, because what it modulates is exactly what this
// group carries — the ambient + IBL term and nothing else (Plan/post-process-pipeline.md decision 8).
// `env.ao.x` is its flag, and an AO-off frame binds a 1×1 fallback and takes the branch, so the
// texture is never sampled rather than sampled and multiplied by one.
//
// The baked sky-visibility probe volume rides this group for the same reason SSAO does: what it
// modulates is exactly what this group carries. `gi_origin.w` is its flag; a frame with no
// `GiVolume` binds a 1×1×1 fallback volume and leaves the flag at 0, so the branch is not taken and
// the volume is never sampled.
struct EnvParams {
    // x = ibl flag (1 = on), y = ibl_intensity, z = the specular max LOD (mip count - 1; this pass
    // does not read it, the `reflections` uniform carries it), w = gi_strength (the authored `GiVolume.sky_visibility`).
    params: vec4<f32>,
    // x = ambient-occlusion flag (1 = on), yz = the reciprocal of the render extent in pixels (what
    // turns this fragment's framebuffer position into the AO texture's UV), w = `ssao_direct`.
    ao: vec4<f32>,
    // xyz = the probe volume's world-space origin, w = the sky-visibility flag (1 = on).
    gi_origin: vec4<f32>,
    // xyz = the reciprocal of the volume's world-space span (what turns a world position into a
    // [0, 1] sample coordinate), w = `normal_bias`, the world-space offset along the surface normal
    // applied before sampling.
    gi_scale: vec4<f32>,
    // x = the authored `GiVolume.bounce` — how hard baked one-bounce irradiance is ADDED to the
    // ambient term (0 = the bounce volumes were never baked and the fallbacks are bound). y = the
    // dynamic-injection flag (1 = the `gi_inject` pass ran this frame and `gi_dyn_tex` holds its
    // output; 0 = the 1×1×1 fallback is bound). zw are reserved and written zero.
    gi_bounce: vec4<f32>,
};
@group(3) @binding(0) var irradiance_tex: texture_cube<f32>;
// Bindings 1 and 2 (the specular cube and the BRDF LUT) are the `reflections` pass's.
@group(3) @binding(3) var env_samp: sampler;
@group(3) @binding(4) var<uniform> env: EnvParams;
// The blurred SSAO factor, at a fraction of the output extent — read with the shared linear sampler,
// so the upscale to the render extent is bilinear.
@group(3) @binding(5) var ao_tex: texture_2d<f32>;
// The SH-L1 sky-visibility probes: `.x` the DC term, `.yzw` the L1 vector. Sampled with `env_samp`
// rather than a sampler of its own — that sampler is `ClampToEdge` on all three axes with linear
// min/mag, which is exactly what a single-level 3D fetch wants: a point outside the volume reads the
// boundary probe instead of wrapping.
@group(3) @binding(6) var gi_tex: texture_3d<f32>;
// The SH-L1 one-bounce irradiance probes, one volume per colour channel (`.x` the DC term, `.yzw`
// the L1 vector), on the same lattice and sampled with the same `env_samp` as `gi_tex` — three
// textures rather than one packed volume because hardware trilinear between probes is not optional
// and a packed axis would blend across coefficient sets. A frame that baked no bounce (no
// `GiVolume`, or one authored at `bounce: 0.0`) binds 1×1×1 fallbacks and carries a zero strength,
// so `gi_bounce` is never called.
@group(3) @binding(7) var gi_bounce_r: texture_3d<f32>;
@group(3) @binding(8) var gi_bounce_g: texture_3d<f32>;
@group(3) @binding(9) var gi_bounce_b: texture_3d<f32>;
// The one-bounce irradiance from lights the simulation moves, injected fresh every frame by the
// `gi_inject` compute pass: RGB DC only, no L1 bands, on the same lattice and sampled with the same
// `env_samp` as its baked siblings. A frame that schedules no injection — no marked light, a scene
// at `bounce: 0.0`, or a machine with the effect off — binds a 1×1×1 fallback and carries a zero
// flag in `gi_bounce.y`, so `gi_dynamic` is never called.
@group(3) @binding(10) var gi_dyn_tex: texture_3d<f32>;

// The per-draw material maps — glTF's metallic-roughness set, a mesh-only @group(4) set once per draw
// (the sprite pass's `SPRITE_TEX_GROUP` is the precedent). One pipeline, no re-batching: which maps a
// draw actually carries lives in `material.flags`, and each term below reads its own bit. A draw with
// a bit clear takes the historical arithmetic *unchanged* — no sample, no multiply by a neutral
// texel — which is what keeps an untextured frame byte-identical on every driver
// (`Plan/textured-pbr.md` G4). The 1×1 defaults bound in the clear slots exist only so the bind
// group is complete; nothing reads them.
struct MeshMaterial {
    // Which maps are bound, as the `MATERIAL_*` bits below.
    flags: u32,
    // glTF `normalTexture.scale` — how hard the sampled tangent-space xy is scaled.
    normal_scale: f32,
    // glTF `occlusionTexture.strength` — how far the ambient term darkens toward the sampled value.
    occlusion_strength: f32,
    // The struct's 16-byte alignment rounds its size up, and `shader_gen`'s generated POD carries no
    // implicit padding — a hole has to have a name, exactly as `Sun`'s three tail lanes do.
    pad0: f32,
};

const MATERIAL_BASE_COLOR: u32 = 1u;
const MATERIAL_METALLIC_ROUGHNESS: u32 = 2u;
const MATERIAL_NORMAL: u32 = 4u;
const MATERIAL_OCCLUSION: u32 = 8u;
const MATERIAL_EMISSIVE: u32 = 16u;

@group(4) @binding(0) var base_color_tex: texture_2d<f32>;
@group(4) @binding(1) var metallic_roughness_tex: texture_2d<f32>;
@group(4) @binding(2) var normal_tex: texture_2d<f32>;
@group(4) @binding(3) var occlusion_tex: texture_2d<f32>;
@group(4) @binding(4) var emissive_tex: texture_2d<f32>;
// Trilinear, anisotropic, with the wrap mode the material authored — one sampler for all five maps,
// picked per draw from the renderer's small shared set.
@group(4) @binding(5) var material_samp: sampler;
@group(4) @binding(6) var<uniform> material: MeshMaterial;

const PI: f32 = 3.14159265358979;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // Per-vertex color (opaque white for primitives/asset meshes; palette color for voxels).
    @location(2) vertex_color: vec4<f32>,
    // Per-vertex material (roughness, metallic, emissive) — authored per palette entry / mesh-doc
    // vertex, default (1, 0, 0) otherwise.
    @location(3) vertex_material: vec3<f32>,
    // Per-vertex texture coordinates: they address the five material maps.
    @location(4) uv: vec2<f32>,
    // Per-vertex tangent frame (xyz along +U, w the bitangent's handedness): the normal map's TBN
    // frame.
    @location(5) tangent: vec4<f32>,
    // Per-instance model matrix rows + flat instance color.
    @location(6) m0: vec4<f32>,
    @location(7) m1: vec4<f32>,
    @location(8) m2: vec4<f32>,
    @location(9) m3: vec4<f32>,
    @location(10) color: vec4<f32>,
    // Per-instance material (roughness, metallic, emissive, use_vertex_material). The w lane is the
    // branch-free selection flag: 1.0 = the geometry's own vertex material wins, 0.0 = these scalars.
    @location(11) instance_material: vec4<f32>,
    // Per-instance normal matrix rows (inverse-transpose of the model's upper-3x3, embedded in a
    // mat4 CPU-side). Transforms object-space normals to world space correctly under non-uniform scale.
    @location(12) n0: vec4<f32>,
    @location(13) n1: vec4<f32>,
    @location(14) n2: vec4<f32>,
    @location(15) n3: vec4<f32>,
    // Per-instance (fade, height_min, bone_offset, handedness): the first two lanes `fs_main`'s
    // Bayer-dither discard reads (default (1.0, -f32::MAX) never discards, so it leaves output
    // bit-identical); the third `vs_main` reads as the bone-palette offset (0 = unskinned, never read);
    // the fourth is the model's handedness sign (-1 for a reflection, which negates cross(n, t)).
    @location(16) fade: vec4<f32>,
    // Per-instance hit-flash (flash_color.rgb, amount): the shaded colour lerps toward flash_color.
    // The default (1, 1, 1, 0) mixes nothing, so it leaves output bit-identical.
    @location(17) flash: vec4<f32>,
    // Per-vertex skin weights for the four bones in `bones`, summing to 1. The default (1, 0, 0, 0)
    // pairs with `bones`' default to select the palette's identity entry, but is never read unskinned
    // (bone_offset 0 skips the blend entirely).
    @location(18) weights: vec4<f32>,
    // Per-vertex bone indices into the instance's slice of the bone palette. The default (0, 0, 0, 0).
    @location(19) bones: vec4<u32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) material: vec3<f32>,
    // Per-instance occlusion fade `(fade, height_min)`, flat (a per-instance constant — no
    // interpolation) so the fragment stage sees the exact authored values for the Bayer discard.
    @location(4) @interpolate(flat) fade: vec2<f32>,
    // Per-instance hit-flash `(flash_color.rgb, amount)`, flat for the same reason.
    @location(5) @interpolate(flat) flash: vec4<f32>,
    // The vertex UV, interpolated — what every group-4 map is sampled at.
    @location(6) uv: vec2<f32>,
    // The world-space tangent frame: `xyz` the interpolated tangent, `w` the bitangent's handedness
    // (a per-vertex constant sign, carried through untouched). Only the normal map reads it.
    @location(7) tangent: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    var position = in.position;
    var normal = in.normal;
    var tangent = in.tangent.xyz;
    // Skinning runs in object space, ahead of the model matrix: `bone_offset == 0` (unskinned) takes
    // exactly today's arithmetic, unchanged, so a skin matrix blended to identity is never in the
    // path and the existing golden set stays bit-exact.
    let bone_offset = u32(in.fade.z);
    if (bone_offset != 0u) {
        let skin = skin_matrix(bone_offset, in.bones, in.weights);
        position = (skin * vec4<f32>(position, 1.0)).xyz;
        normal = skin_normal(skin, normal);
        tangent = (skin * vec4<f32>(tangent, 0.0)).xyz;
    }
    let world = model * vec4<f32>(position, 1.0);
    var out: VsOut;
    out.clip = camera.view_proj * world;
    out.world_pos = world.xyz;
    // Transform the normal by the per-instance normal matrix (inverse-transpose of the model's
    // upper-3x3), correct under non-uniform scale. Same `mat4 * vec4(n, 0)` form the old upper-3x3
    // path used: for a uniform scale the normal matrix IS that upper-3x3, so this stays bit-identical.
    let normal_mat = mat4x4<f32>(in.n0, in.n1, in.n2, in.n3);
    out.world_normal = (normal_mat * vec4<f32>(normal, 0.0)).xyz;
    // Per-vertex color modulates the per-instance color; white vertices leave the instance color exact.
    out.color = in.vertex_color * in.color;
    // Selection is per-instance (flag in w), so mixing here then interpolating is exact.
    out.material = mix(in.instance_material.xyz, in.vertex_material, in.instance_material.w);
    // Carry the fade lane (fade, height_min) to the fragment stage for the screen-door discard.
    out.fade = in.fade.xy;
    // Carry the hit-flash lane to the fragment stage.
    out.flash = in.flash;
    out.uv = in.uv;
    // The tangent rides the **model** matrix, not the normal matrix: a tangent is a direction *along*
    // the surface, so it transforms like a position difference, where a normal transforms by the
    // inverse-transpose. The handedness lane is a sign, not a direction: it passes through times the
    // model's own sign, because a reflection negates the bitangent `cross(n, t)` builds.
    out.tangent = vec4<f32>((model * vec4<f32>(tangent, 0.0)).xyz, in.tangent.w * in.fade.w);
    return out;
}

// GGX/Trowbridge-Reitz normal distribution with alpha = roughness^2.
fn distribution_ggx(ndh: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = ndh * ndh * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

// Smith height-correlated visibility (the 1 / (4 ndl ndv) denominator is folded in).
fn visibility_smith(ndl: f32, ndv: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let v = ndl * sqrt(ndv * ndv * (1.0 - a2) + a2);
    let l = ndv * sqrt(ndl * ndl * (1.0 - a2) + a2);
    return 0.5 / max(v + l, 1e-5);
}

fn fresnel_schlick(vdh: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(clamp(1.0 - vdh, 0.0, 1.0), 5.0);
}

// PCF sun-shadow factor in [0, 1]: 1 = fully lit, 0 = fully shadowed. Hardware depth comparison
// (`sampler_comparison`) does the per-tap test; a centred `sun.pcf`-square of texel offsets averages
// the result. A point outside the light frustum (clip xy/z out of range) or behind the light plane
// is fully lit — standard practice. A zeroed light-space matrix (no shadow map this frame) yields
// `clip.w == 0`, taking the fully-lit early-out, so the map is never sampled when none was drawn.
//
// The texel size comes from `textureDimensions`, so a quality that shrinks the map needs no telling:
// the kernel keeps covering the same neighbourhood in texels, and the bias — authored in world units
// at a 2048² map, converted to light-NDC depth by extract — grows with the texel, since a grazing
// surface's depth error per tap does too (a 1024² map at the unscaled bias stripes every sunlit
// slope with acne). The shadow pass adds a hardware slope-scaled bias on top of this constant.
fn sun_shadow(world_pos: vec3<f32>) -> f32 {
    let clip = sun.light_space * vec4<f32>(world_pos, 1.0);
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    // Clip xy [-1, 1] → UV [0, 1] with the y flip; depth is already wgpu 0..1.
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    let texel = 1.0 / vec2<f32>(textureDimensions(shadow_tex));
    let compare = ndc.z - sun.direction.w * 2048.0 * texel.x;
    // Half the kernel's edge, which is why the edge must be odd: 3 taps reach ±1, 1 tap reaches 0.
    let reach = i32(sun.pcf) / 2;
    var sum = 0.0;
    for (var dy = -reach; dy <= reach; dy = dy + 1) {
        for (var dx = -reach; dx <= reach; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * texel;
            // `Level` variant: samples mip 0 with no derivatives, so it is valid in this loop's
            // non-uniform control flow (the plain `textureSampleCompare` would not be).
            sum = sum + textureSampleCompareLevel(shadow_tex, shadow_samp, uv + offset, compare);
        }
    }
    return sum / f32(sun.pcf * sun.pcf);
}

// The sky-visibility read, through the shared probe snippet (`gi_probe.wgsl`): the ONE function that
// reads the visibility volume is `sky_visibility_at`, and the mesh pass binds its inputs here.
fn gi_sky_visibility(world_pos: vec3<f32>, n: vec3<f32>) -> f32 {
    return sky_visibility_at(gi_tex, env_samp, env.gi_origin.xyz, env.gi_scale, world_pos, n);
}

// One bounce of light arriving at this surface point, as RGB irradiance in the same units as
// `irradiance_tex` — the ONE function that reads the bounce volumes, exactly as `gi_sky_visibility`
// is the one that reads the visibility volume (`Plan/global-illumination.md` decision 4). The sample
// point, the `normal_bias` push and the reconstruction are `gi_sky_visibility`'s verbatim; only the
// floor differs. `max`, not `clamp(…, 0, 1)`: this is HDR radiance with no upper bound, and an SH-L1
// field can ring negative under a strongly directional bounce.
fn gi_bounce(world_pos: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let p = world_pos + n * env.gi_scale.w;
    let uv = (p - env.gi_origin.xyz) * env.gi_scale.xyz;
    let r = textureSampleLevel(gi_bounce_r, env_samp, uv, 0.0);
    let g = textureSampleLevel(gi_bounce_g, env_samp, uv, 0.0);
    let b = textureSampleLevel(gi_bounce_b, env_samp, uv, 0.0);
    let e = vec3<f32>(sh_l1(r, n), sh_l1(g, n), sh_l1(b, n));
    return max(e, vec3<f32>(0.0));
}

// One bounce of light arriving from the scene's *dynamic* lights, RGB, with no directionality — the
// ONE function that reads the injected volume, beside `gi_sky_visibility` and `gi_bounce`. The
// sample point takes the same `normal_bias` push for the same reason: the volume lives on the same
// lattice, and the wall-interpolation leak it guards against does not care which term is crossing
// the wall. There are no SH bands to reconstruct — the pass stores the DC term alone — so this is a
// plain fetch under the same `max(…, 0)` floor its baked sibling uses.
fn gi_dynamic(world_pos: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let p = world_pos + n * env.gi_scale.w;
    let uv = (p - env.gi_origin.xyz) * env.gi_scale.xyz;
    return max(textureSampleLevel(gi_dyn_tex, env_samp, uv, 0.0).rgb, vec3<f32>(0.0));
}

// What `shade` hands back: the lit colour, and the surface description the reflections pass reads
// from the opaque pass's extra targets. `specular_occlusion` is the AO factor (when AO is on) times
// the baked occlusion map — the occlusion that applies to indirect specular too.
struct Shaded {
    lit: vec3<f32>,
    n: vec3<f32>,
    roughness: f32,
    f0: vec3<f32>,
    specular_occlusion: f32,
}

fn shade(
    world_pos: vec3<f32>,
    n_in: vec3<f32>,
    albedo: vec3<f32>,
    // `(roughness, metallic, emissive)`. Named apart from the `material` uniform above, which this
    // function reads for the occlusion map.
    surface: vec3<f32>,
    ao: f32,
    // This fragment's framebuffer position (`@builtin(position)`.xy) — the tile half of the cluster
    // lookup. The shell half is the radial camera distance, computed below from `camera.cam_pos`.
    frag_pos: vec2<f32>,
    // The interpolated vertex UV, for the one map this function samples.
    uv: vec2<f32>,
) -> Shaded {
    // GGX is singular at roughness 0; metallic saturates.
    let roughness = clamp(surface.x, 0.045, 1.0);
    let metallic = clamp(surface.y, 0.0, 1.0);
    let alpha = roughness * roughness;
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let diffuse = (1.0 - metallic) * albedo;

    let n = normalize(n_in);
    // `to_camera` serves twice: normalized it is the view vector, and its own dot product is the
    // squared camera distance the cluster shell lookup wants — no square root anywhere. `normalize`
    // is left as `normalize` rather than a hand-rolled divide, because the two are not guaranteed to
    // emit the same instruction and `v` feeds every specular lobe in the frame.
    let to_camera = camera.cam_pos.xyz - world_pos;
    let v = normalize(to_camera);
    let ndv = max(dot(n, v), 1e-4);

    // Ambient / environment term. Flag off ⇒ the exact historical flat ambient (`ambient * albedo`),
    // so an env-less scene is byte-identical. Flag on ⇒ the irradiance cube feeds the diffuse lobe,
    // scaled by `ibl_intensity`; the sky's specular is added later by the `reflections` pass.
    var acc: vec3<f32>;
    if (env.params.x > 0.5) {
        let irradiance = textureSampleLevel(irradiance_tex, env_samp, n, 0.0).rgb;
        let diffuse_ibl = irradiance * diffuse;
        acc = diffuse_ibl * env.params.y;
        // `Environment.ambient` is a flat floor under the sky's IBL, not scaled by `ibl_intensity`.
        // `diffuse`, not `albedo`, for the reason the IBL term uses it.
        acc = acc + lights.ambient.rgb * diffuse;
    } else {
        acc = lights.ambient.rgb * albedo;
    }
    // Occlusion applies **here and nowhere else** for the diffuse term — both kinds: the screen-space
    // AO factor and the material's baked occlusion map. Indirect specular is added by the
    // `reflections` pass, which scales it by `scene_specular.a`: the same two factors, written by
    // `fs_main`. By the time this function returns, direct light, the sun and
    // emissive have been summed into the same number, and multiplying that by occlusion is the classic
    // "AO looks like dirt" failure — it would dim torch highlights and glowing props, which is the
    // worst case for a night city lit by point lights and emissive accents.
    if (env.ao.x > 0.5) {
        acc = acc * ao;
    }
    // The baked map is a second, independent factor at the same site: an AO-off frame still honours a
    // material that authored one, and `occlusion_strength` says how far toward the sampled value the
    // term darkens (`0` leaves it exactly alone).
    // The same factor also attenuates the GI bounce below: baked occlusion is a statement about
    // indirect light reaching the surface, whatever its source.
    var occlusion_map = 1.0;
    if ((material.flags & MATERIAL_OCCLUSION) != 0u) {
        let occlusion = textureSample(occlusion_tex, material_samp, uv).r;
        occlusion_map = mix(1.0, occlusion, material.occlusion_strength);
        acc = acc * occlusion_map;
    }
    // Baked sky visibility attenuates the same term, after AO and independently of it: AO is the
    // contact-scale darkening screen space can see, this is the metre-scale question of whether the
    // sky reaches here at all. `acc` already holds whichever of the two branches above ran, so one
    // multiply covers both — an interior with no `Environment` still has a flat ambient it should
    // not receive. Direct light and emissive are deliberately outside it: this is an indirect-only
    // term, and emissive is added in `fs_main`.
    //
    // At `gi_strength = 0` the factor is exactly 1.0 and the multiply is bit-preserving in
    // IEEE-754, which is what makes an authored volume at that strength byte-identical to no volume
    // at all.
    //
    // Baked one-bounce irradiance is ADDED to the same term, nested inside this branch so a non-GI
    // frame cannot reach the condition at all. It is added *after* the AO multiply and *after* the
    // sky-visibility multiply and attenuated by neither: sky visibility is definitionally the sky's
    // occlusion and a bounce is not sky, and AO is left off it both because any reordering of the
    // expression above would move every AO golden and because darkening indirect bounce in a
    // torch-lit interior removes the only light there is. The material's baked occlusion map is the
    // exception: it is indirect by definition, so `occlusion_map` rides both bounce terms.
    // `× diffuse` (not `albedo`) because `gi_bounce` returns incident irradiance and the outgoing
    // diffuse radiance is
    // `E(n) · (1 - metallic) · albedo` — the same factor the IBL branch's `irradiance * diffuse`
    // uses; `albedo` would light metals diffusely. Diffuse only: indirect specular is not in an
    // SH-L1 volume's power and is the `reflections` pass's job. Direct light and emissive are
    // outside, exactly as they are for sky visibility.
    // The dynamic half of that same bounce is added inside the same branch, one level deeper still,
    // so a frame that scheduled no injection cannot reach the condition at all. It rides the authored
    // `bounce` strength rather than a knob of its own — it IS the bounce term, from lights that
    // happen to move — and it is attenuated by neither AO nor sky visibility (but by the occlusion map),
    // for the reasons above.
    if (env.gi_origin.w > 0.5) {
        acc = acc * mix(1.0, gi_sky_visibility(world_pos, n), env.params.w);
        if (env.gi_bounce.x > 0.0) {
            acc = acc + gi_bounce(world_pos, n) * diffuse * env.gi_bounce.x * occlusion_map;
            if (env.gi_bounce.y > 0.5) {
                acc = acc + gi_dynamic(world_pos, n) * diffuse * env.gi_bounce.x * occlusion_map;
            }
        }
    }

    // `ssao_direct` (`env.ao.w`) opens that rule by a bounded fraction, for the case decision 8 does
    // not cover: a scene whose ambient term is a small part of its light — a night city lit by point
    // lights — where AO applied to ambient alone has almost nothing to remove. At 0 the rule above is
    // exact and this factor is exactly 1.0; at 1 the direct lobes are attenuated by the same finished
    // factor the ambient term got.
    //
    // Applying it as a factor rather than under decision 9's uniform skip is deliberate and is the
    // only shape that keeps the neutral frame byte-identical: multiplication by an exact 1.0 is
    // bit-preserving in IEEE-754, whereas accumulating the direct lobes into a second register so the
    // multiply could be skipped would change the summation order of every lit mesh pixel and move
    // every mesh golden. Emissive is untouched for free — it is added in `fs_main`, outside `shade`.
    var direct_ao = 1.0;
    if (env.ao.x > 0.5 && env.ao.w > 0.0) {
        direct_ao = mix(1.0, ao, env.ao.w);
    }

    // The clustered light loop: this fragment's own cell's slice of `cluster_lights`, in stored
    // order. Indices ascend within a cell and the array is extract order, so the sum below is the
    // same sum, in the same order, a flat loop over every light would produce — the snippet's
    // identity argument. ALU only: the non-uniform bound forbids an implicit-derivative sample here.
    let slice = cluster_slice(frag_pos, dot(to_camera, to_camera));
    for (var i: u32 = 0u; i < slice.y; i = i + 1u) {
        let light = light_list[cluster_lights[slice.x + i]];
        let to_light = light.position.xyz - world_pos;
        let dist = length(to_light);
        let range = max(light.position.w, 1e-3);
        let atten = clamp(1.0 - dist / range, 0.0, 1.0);
        let l = normalize(to_light);
        let h = normalize(l + v);
        let ndl = max(dot(n, l), 0.0);
        let ndh = max(dot(n, h), 0.0);
        let vdh = max(dot(v, h), 0.0);
        // Premultiplied convention: radiance IS the diffuse response at ndl = 1, ranged atten^2.
        let radiance = light.color.rgb * spot_factor(light, l) * ndl * atten * atten * direct_ao;
        let d = distribution_ggx(ndh, alpha);
        let vis = visibility_smith(ndl, ndv, alpha);
        let f = fresnel_schlick(vdh, f0);
        // Specular scaled by PI to stay proportioned with the 1/PI-free diffuse lobe.
        let specular = PI * d * vis * f;
        acc = acc + (diffuse + specular) * radiance;
    }

    // The sun: the same Cook-Torrance lobes with no attenuation, × the PCF shadow factor. `l` is the
    // vector toward the light (−travel direction). `sun.color` is zero for a sunless frame, so this
    // adds exactly nothing then; `ndl_sun > 0.0` also skips a back-facing (or NaN-direction) surface.
    let l_sun = normalize(-sun.direction.xyz);
    let ndl_sun = max(dot(n, l_sun), 0.0);
    if (ndl_sun > 0.0) {
        let h_sun = normalize(l_sun + v);
        let ndh_sun = max(dot(n, h_sun), 0.0);
        let vdh_sun = max(dot(v, h_sun), 0.0);
        let shadow = sun_shadow(world_pos) * direct_ao;
        if (sun.disc_sin > 0.0) {
            // Representative point (Karis 2013): the direction inside the sun's disc closest to the
            // mirror ray. The disc is absent from the IBL, the probes and SSR's input, so this is
            // its only specular source.
            let r = reflect(-v, n);
            let c = dot(r, l_sun);
            var l_rep = r;
            if (c < sun.disc_cos) {
                let t = r - c * l_sun;
                let t_len = length(t);
                l_rep = l_sun;
                if (t_len > 1e-6) {
                    l_rep = sun.disc_cos * l_sun + sun.disc_sin * (t / t_len);
                }
            }
            let h_rep = normalize(l_rep + v);
            let ndl_rep = max(dot(n, l_rep), 0.0);
            let ndh_rep = max(dot(n, h_rep), 0.0);
            let vdh_rep = max(dot(v, h_rep), 0.0);
            // Widening alpha by the disc keeps the lobe energy-correct: scale by (alpha / alpha')^2.
            let alpha_wide = min(alpha + sun.disc_spread, 1.0);
            let norm = (alpha / alpha_wide) * (alpha / alpha_wide);
            let specular_rep = PI * distribution_ggx(ndh_rep, alpha) * visibility_smith(ndl_rep, ndv, alpha)
                * fresnel_schlick(vdh_rep, f0) * norm;
            acc = acc + (diffuse * ndl_sun + specular_rep * ndl_rep) * sun.color.rgb * shadow;
        } else {
            let d_sun = distribution_ggx(ndh_sun, alpha);
            let vis_sun = visibility_smith(ndl_sun, ndv, alpha);
            let f_sun = fresnel_schlick(vdh_sun, f0);
            let specular_sun = PI * d_sun * vis_sun * f_sun;
            acc = acc + (diffuse + specular_sun) * sun.color.rgb * ndl_sun * shadow;
        }
    }
    var specular_occlusion = occlusion_map;
    if (env.ao.x > 0.5) {
        specular_occlusion = specular_occlusion * ao;
    }
    return Shaded(acc, n, roughness, f0, specular_occlusion);
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @location(1) normal_roughness: vec4<f32>,
    @location(2) specular: vec4<f32>,
}

// The probe capture's extra target: the part of `color` that does not scale with the live ambient —
// emissive and hit-flash — so the relight (decision 17) can leave it out of the live-to-baked ratio.
struct CaptureOut {
    @location(0) color: vec4<f32>,
    @location(1) normal_roughness: vec4<f32>,
    @location(2) specular: vec4<f32>,
    @location(3) emissive: vec4<f32>,
}

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let c = shade_fragment(in);
    return FsOut(c.color, c.normal_roughness, c.specular);
}

@fragment
fn fs_capture(in: VsOut) -> CaptureOut {
    return shade_fragment(in);
}

fn shade_fragment(in: VsOut) -> CaptureOut {
    // Screen-door occlusion fade (opaque pipeline — no blend, no depth-write change; discard is the
    // whole mechanism). `height_min` (fade.y) gates by world height: only fragments strictly ABOVE it
    // fade, so the top-fade-stories case leaves the lower body fully opaque; at or below it the
    // effective fade is 1.0. The default lane `(1.0, -f32::MAX)` makes `world_pos.y > -f32::MAX` true
    // but `eff_fade == 1.0`, which `1.0 <= threshold` never discards — byte-identical to no fade.
    var eff_fade = 1.0;
    if (in.world_pos.y > in.fade.y) {
        eff_fade = in.fade.x;
    }
    let bx = u32(in.clip.x) % 4u;
    let by = u32(in.clip.y) % 4u;
    if (eff_fade <= BAYER4[by * 4u + bx]) {
        discard;
    }

    // The AO factor for this fragment, behind the same uniform flag `shade` branches on: an AO-off
    // frame does no texture read at all, which is the part of decision 9's skip rule that is not free.
    var ao = 1.0;
    if (env.ao.x > 0.5) {
        ao = textureSampleLevel(ao_tex, env_samp, in.clip.xy * env.ao.yz, 0.0).r;
    }

    // The instance × vertex colour — the albedo of an untextured draw, and the emissive term's base
    // whether or not a base-colour map is bound (glTF's emissive is independent of base colour).
    let base = in.color.rgb;
    var albedo = base;
    if ((material.flags & MATERIAL_BASE_COLOR) != 0u) {
        albedo = albedo * textureSample(base_color_tex, material_samp, in.uv).rgb;
    }

    // glTF packs roughness in `g` and metallic in `b`; both are factors on what the vertex/instance
    // material authored, so a map over a `(1, 0, …)` default reads as the map's own values.
    var surface = in.material;
    if ((material.flags & MATERIAL_METALLIC_ROUGHNESS) != 0u) {
        let mr = textureSample(metallic_roughness_tex, material_samp, in.uv);
        surface = vec3<f32>(surface.x * mr.g, surface.y * mr.b, surface.z);
    }

    // The normal map replaces the geometric normal outright rather than nudging it: the TBN frame is
    // built from the interpolated normal and tangent (Gram-Schmidt, so an interpolated tangent that
    // drifted off the tangent plane is re-orthogonalized), the bitangent from the handedness lane.
    var normal = in.world_normal;
    if ((material.flags & MATERIAL_NORMAL) != 0u) {
        let n_geom = normalize(in.world_normal);
        let t_raw = in.tangent.xyz;
        let t = normalize(t_raw - n_geom * dot(n_geom, t_raw));
        let b = cross(n_geom, t) * in.tangent.w;
        let sampled = textureSample(normal_tex, material_samp, in.uv).xyz * 2.0 - 1.0;
        let scaled = sampled * vec3<f32>(material.normal_scale, material.normal_scale, 1.0);
        normal = normalize(mat3x3<f32>(t, b, n_geom) * scaled);
    }

    let shaded = shade(in.world_pos, normal, albedo, surface, ao, in.clip.xy, in.uv);
    var lit = shaded.lit;
    // Emissive rides the HDR/bloom pipeline: colour * intensity, no new pass, no new uniform — times
    // the emissive map where one is bound.
    var emissive = base * max(in.material.z, 0.0);
    if ((material.flags & MATERIAL_EMISSIVE) != 0u) {
        emissive = emissive * textureSample(emissive_tex, material_samp, in.uv).rgb;
    }
    lit = lit + emissive;
    // Hit-flash: lerp the shaded colour toward flash_color by the amount. amount 0 mixes nothing —
    // render-identical by construction.
    lit = mix(lit, in.flash.rgb, clamp(in.flash.a, 0.0, 1.0));
    // Clamp below half-float overflow: a mirror highlight can shade past 65504, which the scene
    // target would store as infinity (or saturate), and the bloom chain would smear it.
    var out: CaptureOut;
    out.color = vec4<f32>(hdr_safe(lit), in.color.a);
    out.emissive = vec4<f32>(
        hdr_safe(mix(emissive, in.flash.rgb, clamp(in.flash.a, 0.0, 1.0))),
        0.0,
    );
    out.normal_roughness = vec4<f32>(shaded.n, shaded.roughness);
    // The flash factor makes `mix(lit + s, flash, a) == mix(lit, flash, a) + (1 - a) * s` exact once
    // the reflections pass adds `s` after the fact.
    out.specular = vec4<f32>(
        shaded.f0,
        shaded.specular_occlusion * (1.0 - clamp(in.flash.a, 0.0, 1.0)),
    );
    return out;
}
