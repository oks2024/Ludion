// The reflections pass: a fullscreen additive draw over the opaque scene that adds the indirect
// specular. It reads the opaque pass's surface description (normal + roughness, F0 + specular
// occlusion, depth) and resolves each texel through a chain, SSR then probes then sky: the ranked
// reflection probes whose volume holds the texel (its cluster cell's mask, at most four) take
// `weight × (1 − coverage)` each and the sky fills the remaining coverage; with screen-space
// reflections on, the half-res `ssr_trace` is upsampled with depth and normal weights and mixed over
// that fill by its confidence, so a texel the trace missed is exactly the fill. The sky is shaded
// through the shared `sky_specular` snippet; `mesh.wgsl` adds no indirect specular itself
// (`Plan/reflections.md` decisions 2, 3, 5). Occlusion follows decision 7: the sky fill is darkened
// by specular occlusion and sky visibility, the probe term by specular occlusion only, the SSR term
// by neither (the trace found the occluder).
//
// The first three inputs are bound in the order the frame graph declares the pass's reads (the trace
// is the fourth, present only with SSR on), so the bind group follows the graph.
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
// The SSR trace at its own (smaller) extent: radiance in `rgb`, confidence in `a`. A 1×1 fallback
// when SSR is off, never read then.
@group(0) @binding(8) var refl_ssr: texture_2d<f32>;
// The slotted array of prefiltered probe cubes (a 1×1 fallback when no probe was captured).
@group(0) @binding(9) var refl_probe_cubes: texture_cube_array<f32>;

struct ProbeList {
    // x = the probe count (0 skips the probe path), y = the cube's max LOD, z = `ibl_intensity`.
    header: vec4<f32>,
    // The frame's `LightHeader.grid`, the cluster cell lookup's input.
    grid: vec4<f32>,
    // The ranked probes; bit `k` of a cell's mask is `probes[k]`.
    probes: array<ShadeProbe, 32>,
}
@group(0) @binding(10) var<uniform> probe_list: ProbeList;
// One bit-mask per cluster cell, in the lights' cell order.
@group(0) @binding(11) var<storage, read> probe_mask: array<u32>;

struct Reflections {
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    // x = ibl flag (1 = on), y = ibl_intensity, z = specular max LOD, w = gi sky-visibility strength.
    env: vec4<f32>,
    // xyz = the probe volume's world-space origin, w = the volume flag (1 = on).
    gi_origin: vec4<f32>,
    // xyz = the reciprocal of the volume's world-space span, w = `normal_bias`.
    gi_scale: vec4<f32>,
    // x = SSR flag (1 = on), y = the look's intensity (scales the confidence).
    ssr: vec4<f32>,
}
@group(0) @binding(7) var<uniform> reflections: Reflections;

// The camera distance of the texel at render pixel `q` with depth `d`.
fn refl_distance(q: vec2<i32>, d: f32, size: vec2<f32>) -> f32 {
    let ndc = vec2<f32>(2.0 * ((f32(q.x) + 0.5) / size.x) - 1.0, 1.0 - 2.0 * ((f32(q.y) + 0.5) / size.y));
    let clip = reflections.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    return length(clip.xyz / clip.w - reflections.cam_pos.xyz);
}

// The trace upsampled to render pixel `p`: bilinear over the four nearest trace texels, each
// weighted by how well the pixel it traced from (`ssr_source_pixel`, shared with the trace) agrees
// with `p` in depth and normal. Zero where no tap agrees, which is the sky fallback.
fn ssr_upsample(p: vec2<i32>, n: vec3<f32>, dist: f32) -> vec4<f32> {
    let render = textureDimensions(refl_depth);
    let trace = textureDimensions(refl_ssr);
    let render_f = vec2<f32>(render);
    let c = (vec2<f32>(p) + vec2<f32>(0.5)) * vec2<f32>(trace) / render_f - vec2<f32>(0.5);
    let base = floor(c);
    let f = c - base;
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        let o = vec2<f32>(f32(i & 1), f32(i >> 1));
        let t = vec2<i32>(clamp(base + o, vec2<f32>(0.0), vec2<f32>(trace) - vec2<f32>(1.0)));
        let bilinear = select(1.0 - f.x, f.x, o.x > 0.5) * select(1.0 - f.y, f.y, o.y > 0.5);
        let g = ssr_source_pixel(t, render, trace);
        let gd = textureLoad(refl_depth, g, 0);
        if (gd >= 1.0) {
            continue;
        }
        let depth_gap = abs(refl_distance(g, gd, render_f) - dist) / (0.1 * dist);
        let wd = saturate(1.0 - depth_gap);
        let wn = pow(saturate(dot(n, normalize(textureLoad(refl_normal, g, 0).xyz))), 8.0);
        let w = bilinear * wd * wd * wn;
        let tap = textureLoad(refl_ssr, t, 0);
        // Premultiplied by confidence: a miss tap carries no radiance and must not darken a hit's.
        sum = sum + vec4<f32>(tap.rgb * tap.a, tap.a) * w;
        total = total + w;
    }
    if (total < 1e-4) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(sum.rgb / max(sum.a, 1e-6), sum.a / total);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let env_on = reflections.env.x > 0.5;
    let ssr_on = reflections.ssr.x > 0.5;
    // No `Environment` and no SSR: nothing to add. First, so such a frame never reaches a sample and
    // 0 + dst is exact.
    let probe_count = u32(probe_list.header.x);
    if (!env_on && !ssr_on && probe_count == 0u) {
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
        let cell = cluster_cell(in.pos.xy, dot(world - reflections.cam_pos.xyz, world - reflections.cam_pos.xyz), probe_list.grid);
        var mask = probe_mask[cell];
        var coverage = 0.0;
        var radiance = vec3<f32>(0.0);
        var taken = 0u;
        let lod = specular_cube_lod(roughness, probe_list.header.y);
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
            radiance = radiance + contribution * probe_sky_scaled(sampled, probe_list.header.z);
            coverage = coverage + contribution;
            taken = taken + 1u;
        }
        if (coverage > 0.0) {
            let ab = textureSampleLevel(refl_brdf_lut, refl_samp, vec2<f32>(ndv, roughness), 0.0).rg;
            fill = radiance * (spec.rgb * ab.x + vec3<f32>(ab.y)) * spec.a + sky_fill * (1.0 - coverage);
        }
    }
    var s = fill;
    if (ssr_on) {
        let hit = ssr_upsample(p, n, length(world - reflections.cam_pos.xyz));
        // The intensity scales the confidence, so intensity 0 is exactly the sky.
        let confidence = hit.a * reflections.ssr.y;
        if (confidence > 0.0) {
            // The sky's split-sum scale and bias, so the two paths match in BRDF.
            let ab = textureSampleLevel(refl_brdf_lut, refl_samp, vec2<f32>(ndv, roughness), 0.0).rg;
            s = mix(fill, hit.rgb * (spec.rgb * ab.x + vec3<f32>(ab.y)), confidence);
        }
    }
    return vec4<f32>(hdr_safe(s), 0.0);
}
