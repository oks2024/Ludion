// The reflections pass: a fullscreen draw over the opaque scene that adds the indirect specular
// fill. It reads the opaque pass's surface description (normal + roughness, F0 + specular occlusion,
// depth) and resolves each texel through a chain, probes then sky: the ranked reflection probes whose
// volume holds the texel (its cluster cell's mask, at most four) take `weight × (1 - coverage)` each
// and the sky fills the remaining coverage. The sky is shaded through the shared `sky_specular`
// snippet; `mesh.wgsl` adds no indirect specular itself (`Plan/reflections.md` decisions 2, 3, 5).
// A probe's emissive part rides a second cube array the relight's ambient ratio does not scale.
// Occlusion follows decision 7: the sky fill is darkened by specular occlusion and sky visibility, the
// probe term by specular occlusion only.
//
// `fs_main` adds the fill onto the scene. `fs_split` (with screen-space reflections on) also writes it
// to a second target, `refl_fill`, which `ssr_blend.wgsl` subtracts when it mixes the trace in.
//
// The inputs are bound in the order the frame graph declares the pass's reads, so the bind group
// follows the graph.
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
// The slotted array of prefiltered probe cubes (a 1×1 fallback when no probe was captured).
@group(0) @binding(9) var refl_probe_cubes: texture_cube_array<f32>;

struct ProbeList {
    // x = the probe count (0 skips the probe path), y = the cube's max LOD.
    header: vec4<f32>,
    // The frame's `LightHeader.grid`, the cluster cell lookup's input.
    grid: vec4<f32>,
    // The ranked probes; bit `k` of a cell's mask is `probes[k]`.
    probes: array<ShadeProbe, 32>,
}
@group(0) @binding(10) var<uniform> probe_list: ProbeList;
// One bit-mask per cluster cell, in the lights' cell order.
@group(0) @binding(11) var<storage, read> probe_mask: array<u32>;
// The live irradiance at each ranked probe's capture point, in `xyz` (`probe_relight.wgsl`).
@group(0) @binding(12) var probe_live: texture_2d<f32>;
// The emissive part of each flagged probe's capture, prefiltered into the same slots (the cube array
// above as a fallback when no probe sees emissive; a probe sets `light.w` to sample it).
@group(0) @binding(13) var refl_probe_emissive: texture_cube_array<f32>;

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

// The fill at the fragment `pos`: probes over sky, zero where nothing is drawn.
fn reflection_fill(pos: vec4<f32>) -> vec3<f32> {
    let p = vec2<i32>(pos.xy);
    let env_on = reflections.env.x > 0.5;
    // No `Environment` and no probe: nothing to add. First, so such a frame never reaches a sample and
    // 0 + dst is exact.
    let probe_count = u32(probe_list.header.x);
    if (!env_on && probe_count == 0u) {
        return vec3<f32>(0.0);
    }
    // Far depth is the sky, which `transparents` draws later; a sky texel would otherwise pick up
    // `prefiltered × ab.y` even at zero F0.
    let depth = textureLoad(refl_depth, p, 0);
    if (depth >= 1.0) {
        return vec3<f32>(0.0);
    }
    let size = vec2<f32>(textureDimensions(refl_depth));
    let ndc = vec2<f32>(2.0 * (pos.x / size.x) - 1.0, 1.0 - 2.0 * (pos.y / size.y));
    let clip = reflections.inv_view_proj * vec4<f32>(ndc, depth, 1.0);
    let world = clip.xyz / clip.w;

    let nr = textureLoad(refl_normal, p, 0);
    let n = normalize(nr.xyz);
    let roughness = nr.w;
    let spec = textureLoad(refl_specular, p, 0);
    let v = normalize(reflections.cam_pos.xyz - world);
    let ndv = max(dot(n, v), 1e-4);
    let r = reflect(-v, n);
    // The sky fill: zero with no `Environment`.
    var sky_fill = vec3<f32>(0.0);
    if (env_on) {
        sky_fill = sky_specular(refl_sky_cube, refl_brdf_lut, refl_samp, r, ndv, roughness, spec.rgb, reflections.env.z)
            * reflections.env.y * spec.a;
        // Sky visibility darkens the sky fill only (decision 7).
        if (reflections.gi_origin.w > 0.5) {
            sky_fill = sky_fill * mix(
                1.0,
                sky_visibility_at(refl_gi, refl_samp, reflections.gi_origin.xyz, reflections.gi_scale, world, n),
                reflections.env.w,
            );
        }
    }
    var fill = sky_fill;
    if (probe_count > 0u) {
        let cell = cluster_cell(pos.xy, dot(world - reflections.cam_pos.xyz, world - reflections.cam_pos.xyz), probe_list.grid);
        var mask = probe_mask[cell];
        var coverage = 0.0;
        var radiance = vec3<f32>(0.0);
        var taken = 0u;
        let lod = specular_cube_lod(roughness, probe_list.header.y);
        // The captured sky fraction is the live sky fill's own lookup; with no `Environment` it is 0.
        let sky_lod = specular_cube_lod(roughness, reflections.env.z);
        let sky_k = select(0.0, reflections.env.y, env_on);
        // Lowest set bit first is rank order. Only a positive weight takes one of the four slots, so
        // a spuriously binned probe is skipped.
        loop {
            if (mask == 0u || taken >= 4u || coverage >= 1.0) {
                break;
            }
            let k = firstTrailingBit(mask);
            mask = mask & (mask - 1u);
            let probe = probe_list.probes[k];
            let w = probe_weight(probe, world);
            if (w <= 0.0) {
                continue;
            }
            let contribution = w * (1.0 - coverage);
            let dir = probe_lookup(probe, world, r);
            let sampled = textureSampleLevel(refl_probe_cubes, refl_samp, dir, i32(probe.center.w), lod);
            var em = vec3<f32>(0.0);
            if (probe.light.w > 0.5) {
                em = textureSampleLevel(refl_probe_emissive, refl_samp, dir, i32(probe.center.w), lod).rgb;
            }
            let ratio = probe_ratio(probe.light.xyz, textureLoad(probe_live, vec2<i32>(i32(k), 0), 0).xyz);
            let sky = textureSampleLevel(refl_sky_cube, refl_samp, r, sky_lod).rgb;
            radiance = radiance + contribution * probe_relit(sampled, em, ratio, sky, sky_k);
            coverage = coverage + contribution;
            taken = taken + 1u;
        }
        if (coverage > 0.0) {
            let ab = textureSampleLevel(refl_brdf_lut, refl_samp, vec2<f32>(ndv, roughness), 0.0).rg;
            fill = radiance * dfg_specular(ab, spec.rgb) * spec.a + sky_fill * (1.0 - coverage);
        }
    }
    return hdr_safe(fill);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(reflection_fill(in.pos), 0.0);
}

struct SplitOut {
    @location(0) scene: vec4<f32>,
    @location(1) fill: vec4<f32>,
}

@fragment
fn fs_split(in: VsOut) -> SplitOut {
    let fill = vec4<f32>(reflection_fill(in.pos), 0.0);
    return SplitOut(fill, fill);
}
