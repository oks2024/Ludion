// The mesh shading, groups 2–3, shared by every shader that lights a surface like a mesh: the sun and
// local-light shadows (@group 2), the IBL / AO / GI environment (@group 3), the Cook-Torrance lobes and
// `shade`. Composed after the camera, lights, dfg, gi_probe and view_ray snippets; needs `math.wgsl` ahead of it.

// True only in the scene pass's viewmodel twins, which draw the gun: its fragments read the viewmodel
// light cell and take their shadows at the camera eye. A pipeline constant rather than a uniform, so a
// world fragment carries none of those branches.
override VIEWMODEL: bool = false;

// True only in the blended-mesh twins (`Mesh.blend`): the Bayer fade dither does not discard and
// `shade_fragment` returns the opacity in alpha, the fade multiplied in. A pipeline constant, so an
// opaque fragment carries none of it.
override BLENDED: bool = false;

// True only in the scene pass's `sun_cascades` debug-view twins: the lit colour is tinted by the cascade
// that shades it. A pipeline constant, so a frame without the view carries no branch for it.
override SUN_CASCADE_VIEW: bool = false;

// True only in the scene pass's `irradiance` debug-view twins: `shade` sums the diffuse irradiance
// (albedo 1, no specular) and `fs_main` stores it as `E / IRRADIANCE_VIEW_SCALE`.
override IRRADIANCE_VIEW: bool = false;
// Mirrors `debug_view.rs::IRRADIANCE_VIEW_SCALE`.
const IRRADIANCE_VIEW_SCALE: f32 = 4.0;

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
    // The rain-occlusion map's world-to-clip view (straight down, orthographic); read by `mesh.wgsl` only.
    wet_view: mat4x4<f32>,
    // x = `Rain.wetness` (0 = no rain: the map is never read), y = the map flag (1 = `wet_map` holds
    // this frame's `rain_occlusion`), z = the map's half extent in metres (the edge fade), w reserved
    // and written zero.
    wet: vec4<f32>,
};
@group(3) @binding(0) var irradiance_tex: texture_cube<f32>;
// Binding 1 (the specular cube) is the `reflections` pass's; binding 2, the split-sum BRDF LUT, is read by
// both passes (here for the direct lobes' multi-scatter compensation and the diffuse's `1 - E_ss`).
@group(3) @binding(2) var env_brdf_lut: texture_2d<f32>;
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

// GGX/Trowbridge-Reitz normal distribution with alpha = roughness^2.
fn distribution_ggx(ndh: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = ndh * ndh * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn fresnel_schlick(vdh: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(clamp(1.0 - vdh, 0.0, 1.0), 5.0);
}

// Golden-angle rotation of a Vogel spiral, applied as a complex multiply so no tap pays a cos/sin.
const GOLDEN_COS: f32 = -0.73736888;
const GOLDEN_SIN: f32 = 0.67549029;
// A penumbra narrower than this many texels falls back to the caller's PCF: its 3x3 already reaches +-1 texel,
// so a disc this narrow only re-estimates it.
const PCSS_PCF_FALLBACK_TEXELS: f32 = 1.5;
// The receiver plane's depth slope is clamped where the surface turns edge-on to the light (about 84 degrees):
// past it the slope diverges and a tiny normal error would swing a tap's compare by the whole depth range.
const PCSS_MIN_NORMAL_Z: f32 = 0.1;

// The receiver plane's depth change per atlas-UV unit, as `(d/du, d/dv)`, for a surface of normal `n` under the
// orthographic light matrix `m`. The upper 3x3 of `m` has orthogonal rows of lengths `s_x, s_y, s_z`, so the
// normal's metric light-space components are `dot(row_i, n) / s_i` and the plane's NDC depth slope is
// `-(m_x s_z)/(m_z s_x)`. `rect_size` is the tile's UV size (the whole map for the anchored sun), which turns
// tile-local UV into atlas UV.
fn pcss_receiver_grad(m: mat4x4<f32>, n: vec3<f32>, rect_size: vec2<f32>) -> vec2<f32> {
    let row_x = vec3<f32>(m[0][0], m[1][0], m[2][0]);
    let row_y = vec3<f32>(m[0][1], m[1][1], m[2][1]);
    let row_z = vec3<f32>(m[0][2], m[1][2], m[2][2]);
    let s_x = length(row_x);
    let s_y = length(row_y);
    let s_z = length(row_z);
    let m_x = dot(row_x, n) / s_x;
    let m_y = dot(row_y, n) / s_y;
    let m_z = dot(row_z, n) / s_z;
    let m_z_safe = select(-1.0, 1.0, m_z >= 0.0) * max(abs(m_z), PCSS_MIN_NORMAL_Z);
    let dz_dx = -(m_x * s_z) / (m_z_safe * s_x);
    let dz_dy = -(m_y * s_z) / (m_z_safe * s_y);
    return vec2<f32>(2.0 * dz_dx, -2.0 * dz_dy) / rect_size;
}

// One `textureGatherCompare` of the 2x2 footprint at `c`, summed: 4 = all lit. `c` is clamped so the footprint
// (texels `floor(c*dims - 0.5)` and the next) stays inside the tile that `lo`/`hi` bound. The compare depth is
// the receiver plane evaluated at the unclamped `c`.
fn pcss_gather_lit(c: vec2<f32>, uv: vec2<f32>, compare: f32, grad: vec2<f32>, texel: vec2<f32>, lo: vec2<f32>, hi: vec2<f32>) -> f32 {
    let g = textureGatherCompare(shadow_tex, shadow_samp, clamp(c, lo + 0.5 * texel, hi - 0.5 * texel), compare + dot(grad, c - uv));
    return g.x + g.y + g.z + g.w;
}

// PCSS for a sun with an angular radius, in four stages, each an approximation of the full search + filter:
// 1. classify: 8 gathers (4 at `(+-1, +-1)` texels, which cover the PCF's 4x4 footprint, and 4 at `ring` texels)
//    call the point lit (32/32) or umbra (0/32) outright, so a blocker thinner than the gather spacing reads lit
//    and the penumbra's ends snap to 1/0;
// 2. blocker search: a Vogel disc of `textureLoad`s, its tap count scaled to the search area;
// 3. a penumbra under `PCSS_PCF_FALLBACK_TEXELS` returns -1.0 and the caller runs its PCF (a seam where the radius
//    crosses it);
// 4. a Vogel comparison filter of radius `(receiver - blocker depth) * scale` texels, tap count scaled to the disc.
// `scale` is texels per unit of light-NDC depth (the cascade's `params.w` or `penumbra_anchored`); `lo`/`hi` are the
// tile's UV clamp and `tile_lo_px`/`tile_hi_px` its integer texel clamp, so no tap crosses into a neighbouring
// cascade. Every gather and tap compares against the receiver plane `compare + dot(grad, offset)` (`grad` from
// `pcss_receiver_grad`, clamped near edge-on), so a grazing receiver does not shadow itself across the filter;
// `search` keeps the centre `compare`. The per-texel rotation is interleaved gradient noise: deterministic, no
// history.
fn sun_pcss(uv: vec2<f32>, compare: f32, grad: vec2<f32>, scale: f32, lo: vec2<f32>, hi: vec2<f32>, tile_lo_px: vec2<i32>, tile_hi_px: vec2<i32>) -> f32 {
    let dims = vec2<f32>(textureDimensions(shadow_tex));
    let texel = 1.0 / dims;
    let px = uv * dims;
    let rot = 6.2831853 * fract(52.9829189 * fract(dot(floor(px), vec2<f32>(0.06711056, 0.00583715))));
    let rot_dir = vec2<f32>(cos(rot), sin(rot));
    let search = min(max(compare, 0.0) * scale, MAX_PENUMBRA_TEXELS);
    let ring = max(0.7 * search, 1.0);
    let ring_x = rot_dir * ring * texel;
    let ring_y = vec2<f32>(-rot_dir.y, rot_dir.x) * ring * texel;
    let lit = pcss_gather_lit(uv + texel, uv, compare, grad, texel, lo, hi)
        + pcss_gather_lit(uv - texel, uv, compare, grad, texel, lo, hi)
        + pcss_gather_lit(uv + vec2<f32>(texel.x, -texel.y), uv, compare, grad, texel, lo, hi)
        + pcss_gather_lit(uv + vec2<f32>(-texel.x, texel.y), uv, compare, grad, texel, lo, hi)
        + pcss_gather_lit(uv + ring_x, uv, compare, grad, texel, lo, hi)
        + pcss_gather_lit(uv - ring_x, uv, compare, grad, texel, lo, hi)
        + pcss_gather_lit(uv + ring_y, uv, compare, grad, texel, lo, hi)
        + pcss_gather_lit(uv - ring_y, uv, compare, grad, texel, lo, hi);
    if (lit >= 32.0) {
        return 1.0;
    }
    if (lit <= 0.0) {
        return 0.0;
    }
    let n_b = max(1u, min(sun.pcss_blocker_taps, max(4u, u32(ceil(search * search)))));
    var gap_sum = 0.0;
    var blockers = 0.0;
    var dir = rot_dir;
    for (var k = 0u; k < n_b; k = k + 1u) {
        let r = sqrt((f32(k) + 0.5) / f32(n_b));
        let off = dir * r * search;
        let coord = clamp(vec2<i32>(floor(px + off)), tile_lo_px, tile_hi_px);
        dir = vec2<f32>(dir.x * GOLDEN_COS - dir.y * GOLDEN_SIN, dir.x * GOLDEN_SIN + dir.y * GOLDEN_COS);
        let d = textureLoad(shadow_tex, coord, 0);
        let tap_compare = compare + dot(grad, off * texel);
        if (d < tap_compare) {
            gap_sum = gap_sum + (tap_compare - d);
            blockers = blockers + 1.0;
        }
    }
    if (blockers == 0.0) {
        return 1.0;
    }
    if (blockers == f32(n_b)) {
        return 0.0;
    }
    let radius_tx = clamp((gap_sum / blockers) * scale, 1.0, MAX_PENUMBRA_TEXELS);
    if (radius_tx < PCSS_PCF_FALLBACK_TEXELS) {
        return -1.0;
    }
    let n_f = max(1u, min(sun.pcss_filter_taps, max(8u, u32(ceil(PI * radius_tx * radius_tx)))));
    var sum = 0.0;
    dir = rot_dir;
    for (var k = 0u; k < n_f; k = k + 1u) {
        let r = sqrt((f32(k) + 0.5) / f32(n_f));
        let off = dir * r * radius_tx * texel;
        let tap = clamp(uv + off, lo, hi);
        dir = vec2<f32>(dir.x * GOLDEN_COS - dir.y * GOLDEN_SIN, dir.x * GOLDEN_SIN + dir.y * GOLDEN_COS);
        sum = sum + textureSampleCompareLevel(shadow_tex, shadow_samp, tap, compare + dot(grad, off));
    }
    return sum / f32(n_f);
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
fn sun_shadow_anchored(world_pos: vec3<f32>, n: vec3<f32>) -> f32 {
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
    if (sun.penumbra_anchored > 0.0) {
        let dims = vec2<f32>(textureDimensions(shadow_tex));
        let grad = pcss_receiver_grad(sun.light_space, n, vec2<f32>(1.0));
        let p = sun_pcss(uv, compare, grad, sun.penumbra_anchored, 0.5 * texel, vec2<f32>(1.0) - 0.5 * texel, vec2<i32>(0), vec2<i32>(dims) - vec2<i32>(1));
        if (p >= 0.0) {
            return p;
        }
    }
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

// Whether cascade `i`'s box holds the point: ndc xy inside [-1, 1], z inside [0, 1], in front of the
// light. A fragment the box does not hold is lit by that cascade, which is not the same as shadowed
// by nothing — the seam band must not blend toward it.
fn sun_cascade_covers(i: u32, world_pos: vec3<f32>) -> bool {
    let clip = sun.cascade_space[i] * vec4<f32>(world_pos, 1.0);
    if (clip.w <= 0.0) {
        return false;
    }
    let ndc = clip.xyz / clip.w;
    return !(abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0 || ndc.z < 0.0 || ndc.z > 1.0);
}

// One cascade's PCF factor, 1 = lit. The cascade's tile of the atlas stands where the anchored map
// does: a point the box does not cover is lit. Every tap is clamped half a texel inside the tile, so
// the kernel never reads the neighbouring cascade. The bias is already in this cascade's light-NDC,
// scaled so it holds the same number of texels in every cascade.
fn sun_cascade_factor(i: u32, world_pos: vec3<f32>, n: vec3<f32>) -> f32 {
    if (!sun_cascade_covers(i, world_pos)) {
        return 1.0;
    }
    let clip = sun.cascade_space[i] * vec4<f32>(world_pos, 1.0);
    let ndc = clip.xyz / clip.w;
    let rect = sun.cascade_rect[i];
    let local = vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5);
    let uv = rect.xy + local * rect.zw;
    let texel = 1.0 / vec2<f32>(textureDimensions(shadow_tex));
    let compare = ndc.z - sun.cascade_params[i].z;
    let reach = i32(sun.pcf) / 2;
    let lo = rect.xy + 0.5 * texel;
    let hi = rect.xy + rect.zw - 0.5 * texel;
    if (sun.cascade_params[i].w > 0.0) {
        let dims = vec2<f32>(textureDimensions(shadow_tex));
        let tile_lo_px = vec2<i32>(floor(rect.xy * dims + vec2<f32>(0.5)));
        let tile_hi_px = vec2<i32>(floor((rect.xy + rect.zw) * dims + vec2<f32>(0.5))) - vec2<i32>(1);
        let grad = pcss_receiver_grad(sun.cascade_space[i], n, rect.zw);
        let p = sun_pcss(uv, compare, grad, sun.cascade_params[i].w, lo, hi, tile_lo_px, tile_hi_px);
        if (p >= 0.0) {
            return p;
        }
    }
    var sum = 0.0;
    for (var dy = -reach; dy <= reach; dy = dy + 1) {
        for (var dx = -reach; dx <= reach; dx = dx + 1) {
            let tap = clamp(uv + vec2<f32>(f32(dx), f32(dy)) * texel, lo, hi);
            // `Level` variant for the anchored lookup's reason: the cascade pick is non-uniform.
            sum = sum + textureSampleCompareLevel(shadow_tex, shadow_samp, tap, compare);
        }
    }
    return sum / f32(sun.pcf * sun.pcf);
}

// The cascaded lookup. A fragment takes the first cascade whose far depth exceeds its view depth —
// the nearest, finest map that covers it; past the last far the sun is simply lit. The last tenth of
// a cascade's depth range blends spatially into the next cascade (or, for the last, into lit), so
// the change of texel size at a seam is a gradient rather than a visible line. The blend targets the
// next cascade only where its box covers the fragment, else this cascade's own factor. The blend reads both
// maps from this fragment alone, with no history, so a still frame stays deterministic.
fn sun_shadow_cascaded(world_pos: vec3<f32>, n: vec3<f32>) -> f32 {
    let depth = dot(sun.view_depth.xyz, world_pos) + sun.view_depth.w;
    var i = 0u;
    for (; i < sun.cascade_count; i = i + 1u) {
        if (sun.cascade_params[i].y > depth) {
            break;
        }
    }
    if (i >= sun.cascade_count) {
        return 1.0;
    }
    let near = sun.cascade_params[i].x;
    let far = sun.cascade_params[i].y;
    let band = 0.1 * (far - near);
    let here = sun_cascade_factor(i, world_pos, n);
    let band_start = far - band;
    if (depth <= band_start) {
        return here;
    }
    var beyond = 1.0;
    if (i + 1u < sun.cascade_count) {
        beyond = here;
        if (sun_cascade_covers(i + 1u, world_pos)) {
            beyond = sun_cascade_factor(i + 1u, world_pos, n);
        }
    }
    return mix(here, beyond, (depth - band_start) / band);
}

// Mirrors `debug_view.rs::SUN_CASCADE_VIEW_PALETTE`.
const SUN_CASCADE_TINT = array<vec3<f32>, 4>(
    vec3<f32>(1.0, 0.45, 0.45),
    vec3<f32>(0.45, 1.0, 0.45),
    vec3<f32>(0.5, 0.6, 1.0),
    vec3<f32>(1.0, 1.0, 0.4),
);

// The `sun_cascades` view's multiplier: the tint of the cascade that shades the fragment. It picks and
// blends exactly as `sun_shadow_cascaded` does (toward the next cascade only where its box covers the
// fragment), with white past the last cascade.
fn sun_cascade_tint(world_pos: vec3<f32>) -> vec3<f32> {
    var tints = SUN_CASCADE_TINT;
    let depth = dot(sun.view_depth.xyz, world_pos) + sun.view_depth.w;
    var i = 0u;
    for (; i < sun.cascade_count; i = i + 1u) {
        if (sun.cascade_params[i].y > depth) {
            break;
        }
    }
    if (i >= sun.cascade_count) {
        return vec3<f32>(1.0);
    }
    let near = sun.cascade_params[i].x;
    let far = sun.cascade_params[i].y;
    let band = 0.1 * (far - near);
    let band_start = far - band;
    if (depth <= band_start) {
        return tints[i];
    }
    var beyond = vec3<f32>(1.0);
    if (i + 1u < sun.cascade_count) {
        beyond = tints[i];
        if (sun_cascade_covers(i + 1u, world_pos)) {
            beyond = tints[i + 1u];
        }
    }
    return mix(tints[i], beyond, (depth - band_start) / band);
}

// The sun's shadow factor: the anchored lookup for a sun without fitted cascades, the cascaded one
// otherwise. The anchored body is untouched, so a count of 0 shades byte for byte as before.
fn sun_shadow(world_pos: vec3<f32>, n: vec3<f32>) -> f32 {
    if (sun.cascade_count == 0u) {
        return sun_shadow_anchored(world_pos, n);
    }
    return sun_shadow_cascaded(world_pos, n);
}

// The shadow factor of the light at list index `index` for a fragment, 1.0 = lit. `l` is the unit
// vector toward the light and `dist` the distance to it. An unshadowed light returns before any
// other read, so multiplying by the result is bit-preserving for every light that has no view.
// A view with `params.w` below 1 (a light inside its `shadow_distance` band) blends the PCF result
// toward lit by that strength; at 1 the result is untouched.
//
// The cube face is picked by the major axis of the light-to-fragment vector, ties resolved in the
// order X, Y, Z — the table in `local_shadow.rs`. The bias is applied in world space BEFORE
// projecting (perspective depth is non-linear, so an NDC bias would vary with distance): a step
// toward the light plus a normal offset of one texel. A cube fragment whose pushed position
// leaves its face is clamped to the face's edge, so the seam between faces stays lit; a spot
// fragment outside its frustum is lit. Every tap is clamped inside the tile so PCF never reads the
// neighbouring tile.
fn local_shadow(index: u32, world_pos: vec3<f32>, n: vec3<f32>, l: vec3<f32>, dist: f32) -> f32 {
    let first = light_shadow[index];
    if (first == UNSHADOWED) {
        return 1.0;
    }
    var view_index = first;
    let cube = local_views[first].params.z > 1.5;
    if (cube) {
        let a = abs(-l);
        if (a.x >= a.y && a.x >= a.z) {
            view_index = first + select(1u, 0u, -l.x > 0.0);
        } else if (a.y >= a.z) {
            view_index = first + select(3u, 2u, -l.y > 0.0);
        } else {
            view_index = first + select(5u, 4u, -l.z > 0.0);
        }
    }
    let view = local_views[view_index];
    let texel_world = dist * view.params.y;
    let p = world_pos + l * view.params.x + n * (NORMAL_OFFSET_TEXELS * texel_world);
    let clip = view.light_space * vec4<f32>(p, 1.0);
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    if (ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    if (!cube && (abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0)) {
        return 1.0;
    }
    let local = clamp(vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5), vec2<f32>(0.0), vec2<f32>(1.0));
    let uv = view.rect.xy + local * view.rect.z;
    let texel = view.rect.w;
    let reach = i32(sun.pcf) / 2;
    let margin = 0.5 * texel;
    let lo = view.rect.xy + vec2<f32>(margin);
    let hi = view.rect.xy + vec2<f32>(view.rect.z - margin);
    var sum = 0.0;
    for (var dy = -reach; dy <= reach; dy = dy + 1) {
        for (var dx = -reach; dx <= reach; dx = dx + 1) {
            let tap = clamp(uv + vec2<f32>(f32(dx), f32(dy)) * texel, lo, hi);
            // `Level` variant for the same reason as the sun's: this runs in the light loop's
            // non-uniform control flow.
            sum = sum + textureSampleCompareLevel(local_shadow_tex, shadow_samp, tap, ndc.z);
        }
    }
    let shadow = sum / f32(sun.pcf * sun.pcf);
    // `select`, not a bare `mix`: strength 1 must return the PCF result bit for bit.
    return select(mix(1.0, shadow, view.params.w), shadow, view.params.w >= 1.0);
}

// The viewmodel's local shadow: one sample at the camera eye toward the light, so a wall the gun
// pokes into cannot darken it (`Plan/viewmodel.md` decision 7). Zero normal: no gun surface offset.
fn viewmodel_local_shadow(index: u32, light: Light) -> f32 {
    let eye = camera.cam_pos.xyz;
    let to_light = light.position.xyz - eye;
    let dist = length(to_light);
    let l = select(vec3<f32>(0.0), normalize(to_light), dist > 0.0);
    return local_shadow(index, eye, vec3<f32>(0.0), l, dist);
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
    // `(roughness, metallic, emissive)`.
    surface: vec3<f32>,
    ao: f32,
    // This fragment's framebuffer position (`@builtin(position)`.xy) — the tile half of the cluster
    // lookup. The shell half is the radial camera distance, computed below from `camera.cam_pos`.
    frag_pos: vec2<f32>,
    // The material's baked occlusion factor, already mixed by its strength; 1.0 when it has none.
    occlusion_map: f32,
) -> Shaded {
    // GGX is singular at roughness 0; metallic saturates.
    let roughness = clamp(surface.x, 0.045, 1.0);
    let metallic = clamp(surface.y, 0.0, 1.0);
    let alpha = roughness * roughness;
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    var diffuse = (1.0 - metallic) * albedo;
    var ambient_albedo = albedo;
    if (IRRADIANCE_VIEW) {
        diffuse = vec3<f32>(1.0);
        ambient_albedo = vec3<f32>(1.0);
    }

    let n = normalize(n_in);
    // `to_camera` serves twice: normalized it is the view vector, and its own dot product is the
    // squared camera distance the cluster shell lookup wants — no square root anywhere. Distance
    // always comes from `to_camera`; `v` goes through `view_to_eye`, which swaps in the constant
    // forward of an orthographic camera. `normalize` is left as `normalize` rather than a hand-rolled
    // divide, because the two are not guaranteed to emit the same instruction and `v` feeds every
    // specular lobe in the frame.
    let to_camera = camera.cam_pos.xyz - world_pos;
    let v = normalize(view_to_eye(to_camera, camera.view_forward));
    let ndv = max(dot(n, v), 1e-4);
    // One fetch, shared by every specular lobe below.
    let dfg = textureSampleLevel(env_brdf_lut, env_samp, vec2<f32>(ndv, roughness), 0.0).rg;
    let energy = dfg_energy(dfg, f0);

    // Ambient / environment term. Flag off ⇒ the exact historical flat ambient (`ambient * albedo`).
    // Flag on ⇒ the irradiance cube feeds the diffuse lobe, scaled by `ibl_intensity`; the sky's specular is added later by the `reflections` pass.
    var acc: vec3<f32>;
    if (env.params.x > 0.5) {
        let irradiance = textureSampleLevel(irradiance_tex, env_samp, n, 0.0).rgb;
        let diffuse_ibl = irradiance * diffuse;
        acc = diffuse_ibl * env.params.y;
        // `Environment.ambient` is a flat floor under the sky's IBL, not scaled by `ibl_intensity`.
        // `diffuse`, not `albedo`, for the reason the IBL term uses it.
        acc = acc + lights.ambient.rgb * diffuse;
        // The specular lobe took E_ss of the energy; the diffuse keeps the rest (Filament's 1 - E).
        if (!IRRADIANCE_VIEW) {
            acc = acc * (vec3<f32>(1.0) - (f0 * dfg.x + vec3<f32>(dfg.y)));
        }
    } else {
        acc = lights.ambient.rgb * ambient_albedo;
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
    acc = acc * occlusion_map;
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
    let is_viewmodel = VIEWMODEL;
    var slice = cluster_slice(frag_pos, dot(to_camera, to_camera));
    if (is_viewmodel) {
        slice = viewmodel_slice();
    }
    for (var i: u32 = 0u; i < slice.y; i = i + 1u) {
        let light_index = cluster_lights[slice.x + i];
        let light = light_list[light_index];
        let to_light = light.position.xyz - world_pos;
        let dist = length(to_light);
        let range = max(light.position.w, 1e-3);
        let atten = clamp(1.0 - dist / range, 0.0, 1.0);
        // `normalize` keeps the blessed bytes; the select guards a light exactly at the point.
        let l = select(vec3<f32>(0.0), normalize(to_light), dist > 0.0);
        let h = normalize(l + v);
        let ndl = max(dot(n, l), 0.0);
        let ndh = max(dot(n, h), 0.0);
        let vdh = max(dot(v, h), 0.0);
        // Premultiplied convention: radiance IS the diffuse response at ndl = 1, ranged atten^2.
        var local_vis: f32;
        if (is_viewmodel) {
            local_vis = viewmodel_local_shadow(light_index, light);
        } else {
            local_vis = local_shadow(light_index, world_pos, n, l, dist);
        }
        let radiance = light.color.rgb * spot_factor(light, l) * local_vis * ndl * atten * atten * direct_ao;
        let d = distribution_ggx(ndh, alpha);
        let vis = visibility_smith(ndl, ndv, alpha);
        let f = fresnel_schlick(vdh, f0);
        // Specular scaled by PI to stay proportioned with the 1/PI-free diffuse lobe.
        var specular = PI * d * vis * f * energy;
        if (IRRADIANCE_VIEW) {
            specular = vec3<f32>(0.0);
        }
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
        var sun_point = world_pos;
        if (is_viewmodel) {
            sun_point = camera.cam_pos.xyz;
        }
        let shadow = sun_shadow(sun_point, n) * direct_ao;
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
            var specular_rep = PI * distribution_ggx(ndh_rep, alpha) * visibility_smith(ndl_rep, ndv, alpha)
                * fresnel_schlick(vdh_rep, f0) * norm * energy;
            if (IRRADIANCE_VIEW) {
                specular_rep = vec3<f32>(0.0);
            }
            acc = acc + (diffuse * ndl_sun + specular_rep * ndl_rep) * sun.color.rgb * shadow;
        } else {
            let d_sun = distribution_ggx(ndh_sun, alpha);
            let vis_sun = visibility_smith(ndl_sun, ndv, alpha);
            let f_sun = fresnel_schlick(vdh_sun, f0);
            var specular_sun = PI * d_sun * vis_sun * f_sun * energy;
            if (IRRADIANCE_VIEW) {
                specular_sun = vec3<f32>(0.0);
            }
            acc = acc + (diffuse + specular_sun) * sun.color.rgb * ndl_sun * shadow;
        }
    }
    var specular_occlusion = occlusion_map;
    if (env.ao.x > 0.5) {
        specular_occlusion = specular_occlusion * ao;
    }
    return Shaded(acc, n, roughness, f0, specular_occlusion);
}
