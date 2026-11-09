// The lit decal pass: `alpha` and `coat` decals shaded like a mesh, drawn into `scene` right after the
// opaque pass and before the reflections. The decal is the projector's box volume (back faces only,
// the pixel's world position rebuilt from `scene_depth`, the same receiver test and the same
// `ProjectorDecal` instance); what differs is that the surface under the box is re-lit by the mesh
// lighting (`shade`) with the decal's albedo, roughness, metallic and normal.
//
// The pass writes three targets, so the reflections, SSR and the projector see the paint as a surface:
// location 0 is `scene`, location 1 the `lit_normal` copy of the receiver's normal (rgb) and roughness
// (a), location 2 `scene_specular`'s F0 (rgb; its occlusion alpha stays the receiver's). A pass cannot
// sample what it blends into, so `fs_lit_copy` first fills `lit_normal` from `scene_normal` with a
// fullscreen triangle, and every later reader takes `lit_normal`.
//
// Each decal is two draws, a base and a finish, run back to back for that one decal so overlaps blend
// exactly. All over `lit_sample`:
//   - `fs_lit_alpha_base`: `lerp(lit, shaded, c)` as a premultiplied blend (`One, OneMinusSrcAlpha`),
//     the painted normal and F0 blended by `c`, and the roughness scaled by `1 - c`;
//   - `fs_lit_alpha_finish`: adds `c * roughness` onto what the base left, completing the lerp;
//   - `fs_lit_coat_base`: the tint, `lit * lerp(1, rgb, c)`, a multiply (`Zero, Src`), with the same
//     surface blend at the dielectric F0;
//   - `fs_lit_coat_finish`: the dielectric specular lobe of the sun and the clustered lights at the
//     decal's roughness and normal, added (`One, One`), and the roughness completion. A zero albedo
//     leaves exactly those lobes: every diffuse, ambient and GI term of `shade` carries the albedo as
//     a factor.
//
// `c`, the coverage, is `fs_decal`'s: the map's alpha, the tint's alpha, the opacity and the angle
// fade, zero outside the box, behind no surface or on a mesh the decal does not paint.
//
// The map taps and derivatives run for every pixel in uniform control flow; only then does a zero
// coverage leave early.
//
// Normal map frame: the receiver normal `n_r` is the surface normal, the tangent is the box's world
// +X projected onto the receiver's plane, and the bitangent is chosen toward the box's −Z, so map +X
// (red) is box +X and map +Y (green) is image up (v grows along box +Z), the glTF +Y-up convention.

struct LitParams {
    inv_view_proj: mat4x4<f32>,
}

// The opaque pass's depth, at the render extent (loaded, never filtered).
@group(4) @binding(0) var lit_depth: texture_depth_2d;
// World normal in `xyz`, roughness in `w`; zero where no opaque surface was drawn.
@group(4) @binding(1) var lit_surface: texture_2d<f32>;
// Mesh index + 1 of the listed mesh visible at each pixel, 0 where none (or no decal lists any).
@group(4) @binding(2) var lit_receivers: texture_2d<f32>;
@group(4) @binding(3) var<uniform> lit_params: LitParams;
@group(4) @binding(4) var<storage, read> lit_decals: array<ProjectorDecal>;
@group(4) @binding(5) var<storage, read> lit_receiver_ids: array<u32>;
@group(4) @binding(6) var lit_base_map: texture_2d<f32>;
@group(4) @binding(7) var lit_base_samp: sampler;
@group(4) @binding(8) var lit_normal_map: texture_2d<f32>;
@group(4) @binding(9) var lit_normal_samp: sampler;
@group(4) @binding(10) var lit_mr_map: texture_2d<f32>;
@group(4) @binding(11) var lit_mr_samp: sampler;

// `ProjectorDecal.ids.z` bit 0: a normal map is bound.
const LIT_NORMAL_MAP: u32 = 1u;

struct LitVsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) decal: u32,
}

// The decal's box as the 36-vertex cube in box space `[-0.5, 0.5]^3`. A mirrored box reverses the
// winding the transform gives it, so its `x` is negated to keep the front-face cull on the back faces.
@vertex
fn vs_lit(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> LitVsOut {
    let d = lit_decals[ii];
    var corner = cube_corner(vi);
    corner.x = corner.x * d.look.w;
    var out: LitVsOut;
    out.clip = camera.view_proj * (d.box_to_world * vec4<f32>(corner * 0.5, 1.0));
    out.decal = ii;
    return out;
}

// Whether the listed mesh visible at `pixel` lets decal `d` paint: `projector.wgsl`'s test over this
// pass's globals.
fn lit_receives(d: ProjectorDecal, pixel: vec2<i32>) -> bool {
    let value = textureLoad(lit_receivers, pixel, 0).x;
    let id = u32(abs(value) + 0.5);
    let wanted = id - 1u;
    var lo = 0u;
    var hi = select(0u, d.receivers.z, id != 0u);
    while (lo < hi) {
        let mid = (lo + hi) / 2u;
        if (lit_receiver_ids[d.receivers.y + mid] < wanted) {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    let listed = id != 0u && lo < d.receivers.z && lit_receiver_ids[d.receivers.y + lo] == wanted;
    let in_list = select(!listed, listed, d.receivers.x == RECEIVERS_ONLY);
    return select(in_list, value >= 0.0, d.receivers.x == RECEIVERS_UNSKINNED);
}

// What the decal paints at one pixel.
struct LitSample {
    // The coverage.
    c: f32,
    world: vec3<f32>,
    // The shaded normal: the receiver's, perturbed by the normal map.
    n: vec3<f32>,
    // The tint times the base map.
    rgb: vec3<f32>,
    roughness: f32,
    metallic: f32,
}

fn lit_sample(in: LitVsOut) -> LitSample {
    let frag = in.clip;
    let d = lit_decals[in.decal];
    let pixel = vec2<i32>(frag.xy);
    let depth = textureLoad(lit_depth, pixel, 0);
    let uv_screen = frag.xy / vec2<f32>(textureDimensions(lit_depth));
    let ndc = vec3<f32>(uv_screen.x * 2.0 - 1.0, 1.0 - uv_screen.y * 2.0, depth);
    let world4 = lit_params.inv_view_proj * vec4<f32>(ndc, 1.0);
    let world = world4.xyz / world4.w;
    let surface = textureLoad(lit_surface, pixel, 0);
    let n_r = normalize(surface.xyz);
    let local = (d.world_to_box * vec4<f32>(world, 1.0)).xyz;
    let uv = mix(d.uv_rect.xy, d.uv_rect.zw, vec2<f32>(local.x + 0.5, local.z + 0.5));

    let longest = GRADIENT_CLAMP * max(length(d.uv_rect.zw - d.uv_rect.xy), 1e-6);
    let gx = clamp_gradient(dpdx(uv), longest);
    let gy = clamp_gradient(dpdy(uv), longest);
    let base = textureSampleGrad(lit_base_map, lit_base_samp, uv, gx, gy);
    let mr = textureSampleGrad(lit_mr_map, lit_mr_samp, uv, gx, gy);
    let normal_tap = textureSampleGrad(lit_normal_map, lit_normal_samp, uv, gx, gy).xyz * 2.0 - 1.0;

    let inside = all(abs(local) <= vec3<f32>(0.5));
    let kept = depth < 1.0 && inside && lit_receives(d, pixel);
    let coverage = base.a * d.color.a * d.look.x * angle_weight(d, n_r);

    var n = n_r;
    if ((d.ids.z & LIT_NORMAL_MAP) != 0u) {
        let bx = d.box_to_world[0].xyz;
        let bz = d.box_to_world[2].xyz;
        // The box's +X flattened onto the receiver's plane; a receiver facing along it falls back to
        // the box's Z so the frame never degenerates.
        var t_raw = bx - n_r * dot(n_r, bx);
        t_raw = select(t_raw, bz - n_r * dot(n_r, bz), dot(t_raw, t_raw) < 1e-8);
        let t = normalize(t_raw);
        var b = normalize(cross(n_r, t));
        b = select(-b, b, dot(b, -bz) >= 0.0);
        let scaled = vec3<f32>(normal_tap.xy * d.surface.z, normal_tap.z);
        n = normalize(mat3x3<f32>(t, b, n_r) * scaled);
    }

    var out: LitSample;
    out.c = select(0.0, clamp(coverage, 0.0, 1.0), kept);
    out.world = world;
    out.n = n;
    out.rgb = base.rgb * d.color.rgb;
    out.roughness = select(d.surface.x, surface.w, d.surface.x < 0.0) * mr.g;
    out.metallic = d.surface.y * mr.b;
    return out;
}

// The AO factor at this fragment, as `mesh.wgsl` samples it.
fn lit_ao(frag: vec4<f32>) -> f32 {
    var ao = 1.0;
    if (env.ao.x > 0.5) {
        ao = textureSampleLevel(ao_tex, env_samp, frag.xy * env.ao.yz, 0.0).r;
    }
    return ao;
}

// What a lit draw writes: scene colour, painted normal + roughness, and F0. Each pipeline's blend
// state and write mask choose which of the three it touches.
struct LitOut {
    @location(0) color: vec4<f32>,
    @location(1) surface: vec4<f32>,
    @location(2) f0: vec4<f32>,
}

fn lit_out(color: vec4<f32>, surface: vec4<f32>, f0: vec4<f32>) -> LitOut {
    var out: LitOut;
    out.color = color;
    out.surface = surface;
    out.f0 = f0;
    return out;
}

// The roughness `shade` stores for a mesh.
fn lit_roughness(r: f32) -> f32 {
    return clamp(r, 0.045, 1.0);
}

// The pass's first draw: `lit_normal` starts as the receiver's normal and roughness. The other two
// targets are masked off.
@fragment
fn fs_lit_copy(in: VsOut) -> LitOut {
    let surface = textureLoad(lit_surface, vec2<i32>(in.pos.xy), 0);
    return lit_out(vec4<f32>(0.0), surface, vec4<f32>(0.0));
}

@fragment
fn fs_lit_alpha_base(in: LitVsOut) -> LitOut {
    let s = lit_sample(in);
    if (s.c <= 0.0) {
        return lit_out(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    }
    let shaded = shade(s.world, s.n, s.rgb, vec3<f32>(s.roughness, s.metallic, 0.0), lit_ao(in.clip), in.clip.xy, 1.0);
    return lit_out(
        vec4<f32>(s.c * hdr_safe(shaded.lit), s.c),
        vec4<f32>(s.c * shaded.n, s.c),
        vec4<f32>(s.c * shaded.f0, s.c),
    );
}

@fragment
fn fs_lit_alpha_finish(in: LitVsOut) -> LitOut {
    let s = lit_sample(in);
    return lit_out(vec4<f32>(0.0), vec4<f32>(0.0, 0.0, 0.0, s.c * lit_roughness(s.roughness)), vec4<f32>(0.0));
}

@fragment
fn fs_lit_coat_base(in: LitVsOut) -> LitOut {
    let s = lit_sample(in);
    // A zero normal (sky, or no opaque surface) normalizes to NaN, which `0 * NaN` would carry into
    // the blend: zero coverage writes exact zeros instead.
    let covered = s.c > 0.0;
    return lit_out(
        vec4<f32>(mix(vec3<f32>(1.0), s.rgb, s.c), 1.0),
        select(vec4<f32>(0.0), vec4<f32>(s.c * normalize(s.n), s.c), covered),
        select(vec4<f32>(0.0), vec4<f32>(vec3<f32>(s.c * 0.04), s.c), covered),
    );
}

@fragment
fn fs_lit_coat_finish(in: LitVsOut) -> LitOut {
    let s = lit_sample(in);
    if (s.c <= 0.0) {
        return lit_out(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    }
    let spec = shade(s.world, s.n, vec3<f32>(0.0), vec3<f32>(s.roughness, 0.0, 0.0), lit_ao(in.clip), in.clip.xy, 1.0);
    return lit_out(
        vec4<f32>(s.c * hdr_safe(spec.lit), 0.0),
        vec4<f32>(0.0, 0.0, 0.0, s.c * lit_roughness(s.roughness)),
        vec4<f32>(0.0),
    );
}
