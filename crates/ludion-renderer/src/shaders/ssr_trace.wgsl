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
// minimum never exceeds the true one, so a skip stays safe. The march is a fixed 128 iterations with no
// jitter and no history. After a descent the march only climbs again once the ray has left the coarse
// cell that forced it, so a ray grazing a silhouette does not ping-pong between levels.
//
// **Level-0 resolve.** A level-0 cell holds the minimum depth of its 2x2 render footprint, so a
// candidate there is resolved against the real render pixels the ray crosses: the first pixel the ray
// is behind decides. Within the thickness it is a hit, beyond it the ray is occluded and the march
// ends as a miss. Normal, colour and thickness all come from that pixel.
//
// `VsOut` + `vs_main` come from `snippets/fullscreen_vs.wgsl`.

const SSR_MAX_ITERS: u32 = 128u;
// The most render pixels a level-0 resolve walks: a cell spans at most 8 per axis, so a line crosses 17.
const SSR_RESOLVE_MAX: u32 = 32u;
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
    // x = proj[1][1], y = the pyramids' level count, z = the footprint view flag, w = the frame's
    // viewmodel depth slice (0 without a viewmodel).
    proj_levels: vec4<f32>,
    // x = max distance, y = thickness, zw = the roughness fade (full below z, zero above w).
    look: vec4<f32>,
    // xyz = unit world-space camera forward, w = 1 under an orthographic projection (zero otherwise).
    view_forward: vec4<f32>,
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

struct Resolve {
    // 0 clear, 1 hit, 2 occluded.
    state: u32,
    t: f32,
    pixel: vec2<i32>,
    uv: vec2<f32>,
    excess: f32,
}

// Walks the render pixels the ray crosses over `[t0, t1]` and reports the first one it is behind.
fn trace_resolve(o: vec3<f32>, d: vec3<f32>, t0: f32, t1: f32) -> Resolve {
    let render = textureDimensions(trace_depth);
    let r = vec2<f32>(render) / vec2<f32>(textureDimensions(trace_hiz, 0));
    let ro = vec3<f32>(o.xy * r, o.z);
    let rd = vec3<f32>(d.xy * r, d.z);
    let eps_r = SSR_STEP_EPSILON / max(max(abs(rd.x), abs(rd.y)), 1e-6);
    var out: Resolve;
    out.state = 0u;
    var t = t0;
    for (var i = 0u; i < SSR_RESOLVE_MAX; i = i + 1u) {
        if (t > t1) {
            break;
        }
        let px = clamp(vec2<i32>(floor((ro + rd * t).xy)), vec2<i32>(0), vec2<i32>(render) - vec2<i32>(1));
        let tb = min(t1, trace_exit(ro, rd, px, vec2<f32>(1.0)));
        let pz = textureLoad(trace_depth, px, 0);
        let za = o.z + d.z * t;
        let zb = o.z + d.z * tb;
        if (pz < 1.0 && pz >= trace_params.proj_levels.w && max(za, zb) >= pz) {
            // The least-excess behind point: the crossing, else the end nearer the camera.
            var ts = select(tb, t, d.z >= 0.0);
            if (min(za, zb) < pz) {
                ts = (pz - o.z) / d.z;
            }
            let uv = (ro.xy + rd.xy * ts) / vec2<f32>(render);
            out.t = ts;
            out.pixel = px;
            out.uv = uv;
            out.excess = length(trace_unproject(uv, o.z + d.z * ts) - trace_unproject(uv, pz)) / trace_params.look.y;
            out.state = select(2u, 1u, out.excess <= 1.0);
            return out;
        }
        t = tb + eps_r;
    }
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let render = textureDimensions(trace_depth);
    let trace = textureDimensions(trace_hiz, 0);
    let trace_f = vec2<f32>(trace);
    let p = ssr_source_pixel(vec2<i32>(in.pos.xy), render, trace);
    let depth = textureLoad(trace_depth, p, 0);
    if (depth >= 1.0 || depth < trace_params.proj_levels.w) {
        return vec4<f32>(0.0);
    }
    let nr = textureLoad(trace_normal, p, 0);
    // The sign of the roughness flags a skinned surface (`mesh.wgsl`).
    let roughness = abs(nr.w);
    let fade = trace_params.look.zw;
    if (roughness >= fade.y) {
        return vec4<f32>(0.0);
    }
    let max_distance = trace_params.look.x;
    let origin_uv = (vec2<f32>(p) + vec2<f32>(0.5)) / vec2<f32>(render);
    let world = trace_unproject(origin_uv, depth);
    let n = normalize(nr.xyz);
    let v = normalize(view_to_eye(trace_params.cam_pos.xyz - world, trace_params.view_forward));
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
    var hit_t = 0.0;
    var hit_pixel = vec2<i32>(0);
    var hit_uv = vec2<f32>(0.0);
    var thickness_excess = 0.0;
    var hold = 0.0;
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
        let t_exit = trace_exit(o, d, cell, s) + eps_t;
        var t_from = t;
        var candidate = solid && q.z >= zmin;
        if (!candidate && solid && d.z > 0.0) {
            let t_plane = (zmin - o.z) / d.z;
            if (t_plane < t_exit) {
                if (t_plane > 1.0) {
                    break;
                }
                t = t_plane;
                t_from = t_plane;
                candidate = true;
            }
        }
        if (candidate) {
            if (level == 0) {
                let res = trace_resolve(o, d, t_from, min(t_exit, 1.0));
                if (res.state == 2u) {
                    break;
                }
                if (res.state == 1u) {
                    hit = true;
                    hit_t = res.t;
                    hit_pixel = res.pixel;
                    hit_uv = res.uv;
                    thickness_excess = res.excess;
                    break;
                }
                t = t_exit;
                continue;
            }
            hold = t_exit;
            level = level - 1;
            continue;
        }
        t = t_exit;
        if (t >= hold) {
            level = min(level + 1, max_level);
        }
    }
    if (!hit) {
        return vec4<f32>(0.0);
    }

    let hit_world = trace_unproject(hit_uv, o.z + d.z * hit_t);
    let hit_distance = length(hit_world - world);
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
    // The footprint debug view reads the pyramid level in place of the radiance.
    if (trace_params.proj_levels.z > 0.5) {
        return vec4<f32>(level_f, 0.0, 0.0, confidence);
    }
    return vec4<f32>(hdr_safe(rgb), confidence);
}
