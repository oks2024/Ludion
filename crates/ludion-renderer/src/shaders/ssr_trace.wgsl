// The SSR trace: one mirror ray per trace texel, marched through the Hi-Z pyramid, written as
// `vec4(radiance, confidence)`; a miss writes `vec4(0)`.
//
// **Trace space** is (trace-pixel xy, NDC z). NDC z is affine in screen space under perspective and
// orthographic alike, so the ray is a straight line `o + d·t`, `t ∈ [0, 1]`, and only `view_proj`,
// `inv_view_proj` and `proj[1][1]` are needed — no projection-type assumption.
//
// **Min-Z skip rule.** Depth is standard-Z (far = 1), so `hiz` holds each cell's *closest* depth. A
// ray point in front of that (`q.z < zmin`) cannot be behind any surface in the cell, so the cell is
// skipped at once; otherwise the march refines a level, and a level-0 hit is a hit. An all-sky cell
// (`zmin >= 1`) is never solid.
//
// **Cell geometry.** The pyramid's footprints are conservative, so level-k cell `c` covers at least
// `[c, c+1)·s_k` in mip-0 texels, `s_k = dims(0) / dims(k)` per axis (not `2^k` on odd sizes); a stored
// minimum never exceeds the true one, so a skip stays safe. The march is a fixed 64 iterations with no
// jitter and no history.
//
// `VsOut` + `vs_main` come from `snippets/fullscreen_vs.wgsl`.

const SSR_MAX_ITERS: u32 = 64u;
// Nudge past a cell boundary, in trace pixels, so a step cannot land back in the cell it left.
const SSR_STEP_EPSILON: f32 = 0.01;

// The Hi-Z pyramid (R32Float, loaded and never filtered).
@group(0) @binding(0) var trace_hiz: texture_2d<f32>;
// The colour pyramid, sampled at the cone's level.
@group(0) @binding(1) var trace_color: texture_2d<f32>;
// World normal in `xyz`, roughness in `w`, at the render extent.
@group(0) @binding(2) var trace_normal: texture_2d<f32>;
@group(0) @binding(3) var trace_depth: texture_depth_2d;
@group(0) @binding(4) var trace_samp: sampler;

struct SsrTrace {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    // x = proj[1][1], y = the pyramids' level count.
    proj_levels: vec4<f32>,
    // x = max distance, y = thickness, zw = the roughness fade (full below z, zero above w).
    look: vec4<f32>,
}
@group(0) @binding(5) var<uniform> trace_params: SsrTrace;

fn trace_unproject(uv: vec2<f32>, z: f32) -> vec3<f32> {
    let ndc = vec2<f32>(2.0 * uv.x - 1.0, 1.0 - 2.0 * uv.y);
    let clip = trace_params.inv_view_proj * vec4<f32>(ndc, z, 1.0);
    return clip.xyz / clip.w;
}

fn trace_pixels(clip: vec4<f32>, extent: vec2<f32>) -> vec3<f32> {
    let ndc = clip.xyz / clip.w;
    return vec3<f32>((ndc.x * 0.5 + 0.5) * extent.x, (0.5 - ndc.y * 0.5) * extent.y, ndc.z);
}

// The smallest `t` at which the ray leaves the box `[cell, cell + 1) * s`.
fn trace_exit(o: vec3<f32>, d: vec3<f32>, cell: vec2<i32>, s: vec2<f32>) -> f32 {
    let lo = vec2<f32>(cell) * s;
    let hi = lo + s;
    let tx = select(1e30, (select(lo.x, hi.x, d.x > 0.0) - o.x) / d.x, d.x != 0.0);
    let ty = select(1e30, (select(lo.y, hi.y, d.y > 0.0) - o.y) / d.y, d.y != 0.0);
    return min(tx, ty);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let render = textureDimensions(trace_depth);
    let trace = textureDimensions(trace_hiz, 0);
    let trace_f = vec2<f32>(trace);
    let p = ssr_source_pixel(vec2<i32>(in.pos.xy), render, trace);
    let depth = textureLoad(trace_depth, p, 0);
    if (depth >= 1.0) {
        return vec4<f32>(0.0);
    }
    let nr = textureLoad(trace_normal, p, 0);
    let roughness = nr.w;
    let fade = trace_params.look.zw;
    if (roughness >= fade.y) {
        return vec4<f32>(0.0);
    }
    let max_distance = trace_params.look.x;
    let origin_uv = (vec2<f32>(p) + vec2<f32>(0.5)) / vec2<f32>(render);
    let world = trace_unproject(origin_uv, depth);
    let n = normalize(nr.xyz);
    let v = normalize(trace_params.cam_pos.xyz - world);
    let r = reflect(-v, n);

    // The ray's two ends in trace space; the far end is pulled back to the near plane if it crosses it.
    let c0 = trace_params.view_proj * vec4<f32>(world, 1.0);
    var c1 = trace_params.view_proj * vec4<f32>(world + r * max_distance, 1.0);
    if (c1.z < 0.0) {
        c1 = mix(c0, c1, c0.z / (c0.z - c1.z));
    }
    if (c1.w <= 0.0) {
        return vec4<f32>(0.0);
    }
    let o = trace_pixels(c0, trace_f);
    let d = trace_pixels(c1, trace_f) - o;
    let eps_t = SSR_STEP_EPSILON / max(max(abs(d.x), abs(d.y)), 1e-6);

    var level = 0;
    let max_level = i32(trace_params.proj_levels.y) - 1;
    var t = trace_exit(o, d, vec2<i32>(floor(o.xy)), vec2<f32>(1.0)) + eps_t;
    var hit = false;
    var hit_q = vec3<f32>(0.0);
    var hit_zmin = 0.0;
    for (var i = 0u; i < SSR_MAX_ITERS; i = i + 1u) {
        if (t > 1.0) {
            break;
        }
        let q = o + d * t;
        if (any(q.xy < vec2<f32>(0.0)) || any(q.xy >= trace_f)) {
            break;
        }
        let dims = vec2<f32>(textureDimensions(trace_hiz, level));
        let s = trace_f / dims;
        let cell = clamp(vec2<i32>(floor(q.xy / s)), vec2<i32>(0), vec2<i32>(dims) - vec2<i32>(1));
        let zmin = textureLoad(trace_hiz, cell, level).r;
        let solid = zmin < 1.0;
        if (solid && q.z >= zmin) {
            if (level == 0) {
                hit = true;
                hit_q = q;
                hit_zmin = zmin;
                break;
            }
            level = level - 1;
            continue;
        }
        let t_exit = trace_exit(o, d, cell, s) + eps_t;
        let t_plane = select(1e30, (zmin - o.z) / d.z, solid && d.z > 0.0);
        if (t_plane < t_exit) {
            t = t_plane;
            if (t > 1.0) {
                break;
            }
            if (level == 0) {
                hit = true;
                hit_q = o + d * t;
                hit_zmin = zmin;
                break;
            }
            level = level - 1;
        } else {
            t = t_exit;
            level = min(level + 1, max_level);
        }
    }
    if (!hit) {
        return vec4<f32>(0.0);
    }

    let hit_uv = hit_q.xy / trace_f;
    let hit_world = trace_unproject(hit_uv, hit_q.z);
    let surf_world = trace_unproject(hit_uv, hit_zmin);
    let hit_distance = length(hit_world - world);
    let thickness_excess = length(hit_world - surf_world) / trace_params.look.y;
    let hit_pixel = clamp(vec2<i32>(floor(hit_uv * vec2<f32>(render))), vec2<i32>(0), vec2<i32>(render) - vec2<i32>(1));
    let hit_normal = textureLoad(trace_normal, hit_pixel, 0).xyz;
    let facing = dot(hit_normal / max(length(hit_normal), 1e-4), r);

    let hit_clip = trace_params.view_proj * vec4<f32>(hit_world, 1.0);
    let ppu = 0.5 * trace_f.y * trace_params.proj_levels.x / max(hit_clip.w, 1e-4);
    let level_f = clamp(ssr_pyramid_level(ssr_cone_half_angle(roughness), hit_distance, ppu), 0.0, trace_params.proj_levels.y - 1.0);
    // The shared sampler's mip filter is nearest, so blend the two levels by hand.
    let lo = floor(level_f);
    let rgb = mix(
        textureSampleLevel(trace_color, trace_samp, hit_uv, lo).rgb,
        textureSampleLevel(trace_color, trace_samp, hit_uv, min(lo + 1.0, trace_params.proj_levels.y - 1.0)).rgb,
        level_f - lo,
    );
    let confidence = ssr_confidence(hit_uv, hit_distance, thickness_excess, facing, roughness, fade, max_distance);
    return vec4<f32>(hdr_safe(rgb), confidence);
}
