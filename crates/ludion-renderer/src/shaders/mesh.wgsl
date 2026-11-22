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
// `snippets/bayer.wgsl`), as is the mesh shading (groups 2–3 and `shade`, `snippets/mesh_shading.wgsl`) and the
// per-draw material maps (@group 4, `snippets/mesh_material.wgsl`) —
// see Plan/shader-bindgen.md decision 16. This body declares only what is unique to the mesh pass.

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // Per-vertex color (opaque white for primitives/asset meshes; palette color for voxels).
    @location(2) vertex_color: vec4<f32>,
    // Per-vertex material (roughness, metallic, emissive) — authored per palette entry / mesh-doc
    // vertex, default (1, 0, 0) otherwise.
    @location(3) vertex_material: vec4<f32>,
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
    // Per-instance rim (color·strength rgb, power); all-zero rgb is no rim.
    @location(20) rim: vec4<f32>,
    // Per-instance `(medium scale, cull scatter slot, porosity)`: `.x` scales the medium's optical depth
    // to this mesh (1.0 = today's frame), `.y` is written by the cull pass and never read here, `.z` is
    // the wet-surface porosity.
    @location(21) medium: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    // `(roughness, metallic, emissive, porosity)`, the selected vertex or instance material.
    @location(3) material: vec4<f32>,
    // Per-instance occlusion fade `(fade, height_min, bone_offset)`, flat (a per-instance constant — no
    // interpolation) so the fragment stage sees the exact authored values for the Bayer discard; the
    // bone offset is nonzero for a skinned instance (`fs_main` flags the surface with it).
    @location(4) @interpolate(flat) fade: vec3<f32>,
    // Per-instance hit-flash `(flash_color.rgb, amount)`, flat for the same reason.
    @location(5) @interpolate(flat) flash: vec4<f32>,
    // The vertex UV, interpolated — what every group-4 map is sampled at.
    @location(6) uv: vec2<f32>,
    // The world-space tangent frame: `xyz` the interpolated tangent, `w` the bitangent's handedness
    // (a per-vertex constant sign, carried through untouched). Only the normal map reads it.
    @location(7) tangent: vec4<f32>,
    // Per-instance wrap diffuse `(scatter.rgb, wrap)` and world half-extents, flat like `flash`.
    @location(8) @interpolate(flat) wrap: vec4<f32>,
    @location(9) @interpolate(flat) wrap_extent: vec3<f32>,
    // Per-instance rim `(color·strength rgb, power)`, flat like `flash`.
    @location(10) @interpolate(flat) rim: vec4<f32>,
    // Per-instance medium scale, flat like `rim`; only `fs_medium` reads it.
    @location(11) @interpolate(flat) medium: f32,
};

struct WrapLanes {
    wrap: vec4<f32>,
    extent: vec3<f32>,
}

// The only reader of the wrap data. The normal matrix is used as `(normal_mat * vec4(n, 0.0)).xyz`, so its
// `w` row and its last column reach no result; the CPU packs `(scatter.rgb, wrap)` and the half-extents
// there (`instance_normal_lanes` in render.rs). A wrap of 0 means the instance does not wrap.
fn wrap_lanes(in: VsIn) -> WrapLanes {
    return WrapLanes(vec4<f32>(in.n3.xyz, in.n0.w), vec3<f32>(in.n1.w, in.n2.w, in.n3.w));
}

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
    out.material = vec4<f32>(
        mix(in.instance_material.xyz, in.vertex_material.xyz, in.instance_material.w),
        mix(in.medium.z, in.vertex_material.w, in.instance_material.w),
    );
    // Carry the fade lane (fade, height_min) to the fragment stage for the screen-door discard.
    out.fade = in.fade.xyz;
    // Carry the hit-flash lane to the fragment stage.
    out.flash = in.flash;
    out.rim = in.rim;
    out.medium = in.medium.x;
    let wrap = wrap_lanes(in);
    out.wrap = wrap.wrap;
    out.wrap_extent = wrap.extent;
    out.uv = in.uv;
    // The tangent rides the **model** matrix, not the normal matrix: a tangent is a direction *along*
    // the surface, so it transforms like a position difference, where a normal transforms by the
    // inverse-transpose. The handedness lane is a sign, not a direction: it passes through times the
    // model's own sign, because a reflection negates the bitangent `cross(n, t)` builds.
    out.tangent = vec4<f32>((model * vec4<f32>(tangent, 0.0)).xyz, in.tangent.w * in.fade.w);
    return out;
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

// A skinned surface stores its roughness negated: the decal passes read the sign as "skinned" (an
// `all` decal skips it) and every other reader of `scene_normal.a` takes `abs`. Roughness is clamped
// to [0.045, 1], never zero, so the flag is exact. Probe captures (`fs_capture`) stay unsigned.
fn flag_skinned(normal_roughness: vec4<f32>, bone_offset: f32) -> vec4<f32> {
    return vec4<f32>(normal_roughness.xyz, select(normal_roughness.w, -normal_roughness.w, bone_offset != 0.0));
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front_facing: bool) -> FsOut {
    let c = shade_fragment(in, front_facing, true);
    return FsOut(c.color, flag_skinned(c.normal_roughness, in.fade.z), c.specular);
}

// The medium twin's extra target: the mesh's medium scale per pixel, read by the composite.
struct MediumOut {
    @location(0) color: vec4<f32>,
    @location(1) normal_roughness: vec4<f32>,
    @location(2) specular: vec4<f32>,
    @location(3) medium: vec4<f32>,
}

@fragment
fn fs_medium(in: VsOut, @builtin(front_facing) front_facing: bool) -> MediumOut {
    let c = shade_fragment(in, front_facing, true);
    return MediumOut(c.color, flag_skinned(c.normal_roughness, in.fade.z), c.specular, vec4<f32>(in.medium, 0.0, 0.0, 0.0));
}

// True in the blended twins of a `Mesh.lit: false` mesh: albedo, emissive, rim and flash, with no direct
// light, ambient, wrap or indirect specular.
override BLEND_UNLIT: bool = false;

// The indirect specular a blended mesh gives itself: the twin of `reflections.wgsl` `reflection_fill`, which
// reads surface targets a blended mesh never wrote. The same operations in the same order over this
// fragment's own values (`nr` the normal and roughness, `spec` the F0 and specular occlusion), minus the depth
// fetch and the sky early-out: a change to one is a change to the other.
fn blend_fill(world: vec3<f32>, pos: vec4<f32>, nr: vec4<f32>, spec: vec4<f32>) -> vec3<f32> {
    let env_on = env.params.x > 0.5;
    let probe_count = u32(blend_probe_list.header.x);
    if (!env_on && probe_count == 0u) {
        return vec3<f32>(0.0);
    }
    let n = normalize(nr.xyz);
    let roughness = abs(nr.w);
    let v = normalize(view_to_eye(camera.cam_pos.xyz - world, camera.view_forward));
    let ndv = max(dot(n, v), 1e-4);
    let r = reflect(-v, n);
    let rd = normalize(mix(r, n, dominant_lerp(roughness * roughness, ndv)));
    var sky_fill = vec3<f32>(0.0);
    if (env_on) {
        sky_fill = sky_specular(blend_sky_cube, env_brdf_lut, env_samp, rd, ndv, roughness, spec.rgb, env.params.z)
            * env.params.y * spec.a;
        if (env.gi_origin.w > 0.5) {
            sky_fill = sky_fill * mix(
                1.0,
                sky_visibility_at(gi_tex, env_samp, env.gi_origin.xyz, env.gi_scale, world, n),
                env.params.w,
            );
        }
    }
    var fill = sky_fill;
    if (probe_count > 0u) {
        let cell = cluster_cell(pos.xy, dot(world - camera.cam_pos.xyz, world - camera.cam_pos.xyz), blend_probe_list.grid);
        var mask = blend_probe_mask[cell];
        var coverage = 0.0;
        var radiance = vec3<f32>(0.0);
        var taken = 0u;
        let lod = specular_cube_lod(roughness, blend_probe_list.header.y);
        let sky_lod = specular_cube_lod(roughness, env.params.z);
        let sky_k = select(0.0, env.params.y, env_on);
        let view_dist = length(world - camera.cam_pos.xyz);
        loop {
            if (mask == 0u || taken >= 4u || coverage >= 1.0) {
                break;
            }
            let k = firstTrailingBit(mask);
            mask = mask & (mask - 1u);
            let probe = blend_probe_list.probes[k];
            let w = probe_weight(probe, world, view_dist);
            if (w <= 0.0) {
                continue;
            }
            let contribution = w * (1.0 - coverage);
            let dir = probe_lookup(probe, world, rd);
            let sampled = textureSampleLevel(blend_probe_cubes, env_samp, dir, i32(probe.center.w), lod);
            var em = vec3<f32>(0.0);
            if (probe.params.z > 0.5) {
                em = textureSampleLevel(blend_probe_emissive, env_samp, dir, i32(probe.center.w), lod).rgb;
            }
            let facing = select(vec3<f32>(0.0, 1.0, 0.0), -normalize(dir), dot(dir, dir) > 1e-12);
            let rows = probe_axis_rows(facing);
            let ratio = probe_axis_ratio(
                probe.baked[rows.x].xyz, probe.baked[rows.y].xyz, probe.baked[rows.z].xyz,
                textureLoad(blend_probe_live, vec2<i32>(i32(k), i32(rows.x)), 0).xyz,
                textureLoad(blend_probe_live, vec2<i32>(i32(k), i32(rows.y)), 0).xyz,
                textureLoad(blend_probe_live, vec2<i32>(i32(k), i32(rows.z)), 0).xyz,
                facing * facing,
            );
            let sky = textureSampleLevel(blend_sky_cube, env_samp, rd, sky_lod).rgb;
            radiance = radiance + contribution * probe_relit(sampled, em, ratio, sky, sky_k);
            coverage = coverage + contribution;
            taken = taken + 1u;
        }
        if (coverage > 0.0) {
            let ab = textureSampleLevel(env_brdf_lut, env_samp, vec2<f32>(ndv, roughness), 0.0).rg;
            fill = radiance * dfg_specular(ab, spec.rgb) * spec.a + sky_fill * (1.0 - coverage);
        }
    }
    return hdr_safe(fill);
}

// A blended mesh's colour: the scene pass's shading plus, when lit, the indirect specular the `reflections`
// pass gives an opaque mesh (`blend_fill`; never a screen-space trace). `specular` already carries the
// hit-flash complement, so the sum matches the opaque path's after-the-fact add.
fn blend_shade(in: VsOut, front_facing: bool) -> vec4<f32> {
    let c = shade_fragment(in, front_facing, true);
    var rgb = c.color.rgb;
    if (!BLEND_UNLIT) {
        rgb = hdr_safe(rgb + blend_fill(in.world_pos, in.clip, c.normal_roughness, c.specular));
    }
    return vec4<f32>(rgb, c.color.a);
}

// The blended twins' colour: premultiplied by the opacity, so the alpha pipeline's
// `(One, OneMinusSrcAlpha)` blend covers what is behind and the additive pipeline's `(One, One)` adds it.
@fragment
fn fs_blend(in: VsOut, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    let c = blend_shade(in, front_facing);
    return vec4<f32>(c.rgb * c.a, c.a);
}

// True only in the additive blend layer twin: the weight is the brightest channel the colour pass adds,
// as the particle layer's is.
override BLEND_ADDITIVE: bool = false;

// The blended meshes' transparent layer: the same shading, reduced to the weight the colour pass
// blended with (`particle.wgsl` `fs_depth`).
@fragment
fn fs_blend_layer(in: VsOut, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    let c = blend_shade(in, front_facing);
    var weight = clamp(c.a, 0.0, 1.0);
    if (BLEND_ADDITIVE) {
        let rgb = c.rgb * weight;
        weight = weight * min(1.0, max(rgb.r, max(rgb.g, rgb.b)));
    }
    return transparent_layer(in.world_pos, weight, in.medium);
}

@fragment
fn fs_capture(in: VsOut, @builtin(front_facing) front_facing: bool) -> CaptureOut {
    return shade_fragment(in, front_facing, false);
}

// The diffuse a wrapped term adds past plain Lambert, per channel; never negative, zero at n·l = 1.
// The *total* diffuse `T = x * mix(scatter, 1, g)` is what stays smooth across the terminator
// (`x` the wrapped cosine, `g = 1 - w * (1 - x)`): tinting only the excess over Lambert would leave
// Lambert's kink visible under a coloured scatter. White scatter gives `T = x`, plain wrap. For any
// scatter >= 0, `x * g - nl = w * (x - 1)^2 >= 0`, so `T` never falls below Lambert.
fn wrap_excess(nl: f32, w: f32, scatter: vec3<f32>) -> vec3<f32> {
    let x = max((nl + w) / (1.0 + w), 0.0);
    let g = 1.0 - w * (1.0 - x);
    let total = x * mix(scatter, vec3<f32>(1.0), g);
    return max(total - vec3<f32>(max(nl, 0.0)), vec3<f32>(0.0));
}

// How far toward the light to test this fragment's shadow: casters draw their lit side, so a fragment past
// the terminator is shadowed by its own object unless the test clears the object's thickness along the ray.
// Along `l`, the shadow-map column and cube face stay the same.
fn wrap_lift(nl: f32, l: vec3<f32>, extent: vec3<f32>) -> f32 {
    return 2.0 * dot(abs(l), extent) * (1.0 - max(nl, 0.0));
}

// The wrapped diffuse past plain Lambert, direct lights only: the same light loop and sun as `shade`,
// each term `wrap_excess` (the scatter colour is inside it), shadowed at a point lifted toward the light.
fn wrap_band(
    world_pos: vec3<f32>,
    n: vec3<f32>,
    diffuse: vec3<f32>,
    ao: f32,
    frag_pos: vec2<f32>,
    wrap: vec4<f32>,
    extent: vec3<f32>,
) -> vec3<f32> {
    var direct_ao = 1.0;
    if (env.ao.x > 0.5 && env.ao.w > 0.0) {
        direct_ao = mix(1.0, ao, env.ao.w);
    }
    var sum = vec3<f32>(0.0);
    let to_camera = camera.cam_pos.xyz - world_pos;
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
        if (dist <= 0.0) {
            continue;
        }
        let range = max(light.position.w, 1e-3);
        let atten = clamp(1.0 - dist / range, 0.0, 1.0);
        let l = normalize(to_light);
        let nl = dot(n, l);
        let excess = wrap_excess(nl, wrap.a, wrap.rgb);
        if (any(excess > vec3<f32>(0.0))) {
            let lift = min(wrap_lift(nl, l, extent), dist);
            var local_vis: f32;
            if (is_viewmodel) {
                local_vis = viewmodel_local_shadow(light_index, light);
            } else {
                local_vis = local_shadow(light_index, world_pos + l * lift, n, l, dist - lift);
            }
            sum += light.color.rgb * spot_factor(light, l)
                * local_vis
                * excess * atten * atten;
        }
    }
    // A sunless frame has a zero direction, whose normalize is NaN: test the colour first.
    if (any(sun.color.rgb != vec3<f32>(0.0))) {
        let l_sun = normalize(-sun.direction.xyz);
        let nl = dot(n, l_sun);
        let excess = wrap_excess(nl, wrap.a, wrap.rgb);
        if (any(excess > vec3<f32>(0.0))) {
            var sun_point = world_pos + l_sun * wrap_lift(nl, l_sun, extent);
            if (is_viewmodel) {
                sun_point = camera.cam_pos.xyz;
            }
            sum += sun.color.rgb * sun_shadow(sun_point, n) * excess;
        }
    }
    return sum * diffuse * direct_ao;
}

// The overhead rain-occlusion map, read with `textureLoad` by `surface_wetness`. It is declared here, not
// in `mesh_shading.wgsl`, because the lit decal pass shares that snippet and already fills the fragment
// stage's sampled-texture limit: only the mesh pipelines bind it. A frame without rain (and the blended
// draws) binds a 1×1 fallback and carries `wet = 0`, so it is never read.
@group(3) @binding(11) var wet_map: texture_depth_2d;

// The reflection resources a blended mesh reads for its own indirect specular (`blend_fill`): a blended
// mesh has no opaque surface targets for the `reflections` pass to fill, so `fs_blend`/`fs_blend_layer`
// compute the fill inline. They mirror the pass's bindings (`reflections.wgsl`); every other mesh draw
// binds fallbacks and never reads them.
@group(3) @binding(12) var blend_sky_cube: texture_cube<f32>;
@group(3) @binding(13) var blend_probe_cubes: texture_cube_array<f32>;
@group(3) @binding(14) var blend_probe_emissive: texture_cube_array<f32>;
@group(3) @binding(15) var blend_probe_live: texture_2d<f32>;
struct ProbeList {
    // x = the probe count (0 skips the probe path), y = the cube's max LOD.
    header: vec4<f32>,
    // The frame's `LightHeader.grid`, the cluster cell lookup's input.
    grid: vec4<f32>,
    // The ranked probes; bit `k` of a cell's mask is `probes[k]`.
    probes: array<ShadeProbe, 32>,
}
@group(3) @binding(16) var<uniform> blend_probe_list: ProbeList;
@group(3) @binding(17) var<storage, read> blend_probe_mask: array<u32>;

// How wet the rain has left a surface point, in [0, 1]: `wetness × exposure × facing`. Exposure is the
// overhead occlusion map's — a point at its column's top is 1, one under a roof 0 — and blends over the
// map's outer 10 % to the sky visibility (the open sky without a `GiVolume`). Facing is 1 looking up,
// 0.5 on a wall, 0 looking down. 0 without rain; `n` is the geometric normal.
fn surface_wetness(world_pos: vec3<f32>, n: vec3<f32>) -> f32 {
    if (env.wet.x <= 0.0) {
        return 0.0;
    }
    var beyond = 1.0;
    if (env.gi_origin.w > 0.5) {
        beyond = mix(1.0, gi_sky_visibility(world_pos, n), env.params.w);
    }
    var exposure = beyond;
    if (env.wet.y > 0.5) {
        let column = occluder_top(env.wet_view, wet_map, world_pos);
        if (column.y > 0.5) {
            var under = 1.0;
            if (column.x > NO_OCCLUDER) {
                under = smoothstep(column.x - 0.2, column.x, world_pos.y);
            }
            let clip = env.wet_view * vec4<f32>(world_pos, 1.0);
            let edge = max(abs(clip.x), abs(clip.y)) * env.wet.z;
            exposure = mix(beyond, under, 1.0 - smoothstep(0.9 * env.wet.z, env.wet.z, edge));
        }
    }
    let facing = clamp(0.5 + 0.5 * n.y, 0.0, 1.0);
    return env.wet.x * exposure * facing;
}

// The `rain_occlusion` debug view's tints (`env.wet.w` = 1): blue where rain reaches a surface, orange where it
// is covered. Mirrors `debug_view.rs::RAIN_VIEW_EXPOSED` / `RAIN_VIEW_COVERED`.
const RAIN_VIEW_EXPOSED = vec3<f32>(0.15, 0.55, 1.0);
const RAIN_VIEW_COVERED = vec3<f32>(1.0, 0.45, 0.1);

// The view's tint for a fragment: untinted outside the map; inside, its column's exposure (the `under` term of
// `surface_wetness`, 1 where no occluder is above) picks between the two.
fn rain_view_tint(world_pos: vec3<f32>) -> vec3<f32> {
    let column = occluder_top(env.wet_view, wet_map, world_pos);
    if (column.y < 0.5) {
        return vec3<f32>(1.0);
    }
    var under = 1.0;
    if (column.x > NO_OCCLUDER) {
        under = smoothstep(column.x - 0.2, column.x, world_pos.y);
    }
    return mix(RAIN_VIEW_COVERED, RAIN_VIEW_EXPOSED, under);
}

// The wet look (`Plan/rain-and-wet.md` decision 14): the albedo multiplier at full wetness and porosity,
// and the roughness of a water film. Porosity is authored per vertex or per instance (`in.material.w`).
const WET_ALBEDO_DARKEN: f32 = 0.6;
const WET_ROUGHNESS: f32 = 0.08;

fn shade_fragment(in: VsOut, front_facing: bool, with_rim: bool) -> CaptureOut {
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
    if (!BLENDED) {
        if (eff_fade <= BAYER4[by * 4u + bx]) {
            discard;
        }
    }

    // The AO factor for this fragment, behind the same uniform flag `shade` branches on: an AO-off
    // frame does no texture read at all, which is the part of decision 9's skip rule that is not free.
    var ao = 1.0;
    if (env.ao.x > 0.5) {
        ao = textureSampleLevel(ao_tex, env_samp, in.clip.xy * env.ao.yz, 0.0).r;
        // The prepass leaves the gun out, so the AO texture holds the world behind it.
        if (VIEWMODEL) {
            ao = 1.0;
        }
    }

    // The instance × vertex colour — the albedo of an untextured draw, and the emissive term's base
    // whether or not a base-colour map is bound (glTF's emissive is independent of base colour).
    let base = in.color.rgb;
    var albedo = base;
    // Cutout coverage: the instance × vertex alpha, times the base-colour map's where one is bound.
    var coverage = in.color.a;
    if ((material.flags & MATERIAL_BASE_COLOR) != 0u) {
        let texel = textureSample(base_color_tex, material_samp, in.uv);
        albedo = albedo * texel.rgb;
        coverage = coverage * texel.a;
    }
    if (material_masked(coverage)) {
        discard;
    }

    // glTF packs roughness in `g` and metallic in `b`; both are factors on what the vertex/instance
    // material authored, so a map over a `(1, 0, …)` default reads as the map's own values.
    var surface = in.material.xyz;
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
    // A double-sided back face is the same surface seen from behind: the whole shading frame flips.
    if (material_back_face(front_facing)) {
        normal = -normal;
    }

    var occlusion_map = 1.0;
    if ((material.flags & MATERIAL_OCCLUSION) != 0u) {
        occlusion_map = mix(1.0, textureSample(occlusion_tex, material_samp, in.uv).r, material.occlusion_strength);
    }

    // Rain wets what it reaches: the albedo darkens by the porosity and the roughness drops toward a
    // film of water, except on metal. The viewmodel and the blended draws stay dry.
    if (!VIEWMODEL && !BLENDED) {
        var geometric = normalize(in.world_normal);
        if (material_back_face(front_facing)) {
            geometric = -geometric;
        }
        let wet = surface_wetness(in.world_pos, geometric);
        albedo = albedo * mix(1.0, WET_ALBEDO_DARKEN, wet * in.material.w);
        surface.x = mix(surface.x, WET_ROUGHNESS, wet * (1.0 - surface.y));
        // Standing water ripples where the ground is wet, so covered ground stays still.
        if (env.ripple.z > 0.0 && wet > 0.0) {
            normal = ripple_normal(normal, geometric, in.world_pos, env.ripple.x, bitcast<u32>(env.ripple.y), env.ripple.z * wet);
        }
    }

    let shaded = shade(in.world_pos, normal, albedo, surface, ao, in.clip.xy, occlusion_map);
    if (IRRADIANCE_VIEW) {
        // The irradiance the light summed, as the view stores it: no wrap, emissive, rim or flash, and
        // no specular for the reflections pass to add.
        var view_out: CaptureOut;
        view_out.color = vec4<f32>(hdr_safe(shaded.lit / IRRADIANCE_VIEW_SCALE), in.color.a);
        view_out.emissive = vec4<f32>(0.0);
        view_out.normal_roughness = vec4<f32>(shaded.n, shaded.roughness);
        view_out.specular = vec4<f32>(0.0);
        return view_out;
    }
    var lit = shaded.lit;
    if (BLEND_UNLIT) {
        lit = albedo;
    }
    if (in.wrap.a > 0.0 && !BLEND_UNLIT) {
        let diffuse = (1.0 - clamp(surface.y, 0.0, 1.0)) * albedo;
        lit = lit + wrap_band(in.world_pos, shaded.n, diffuse, ao, in.clip.xy, in.wrap, in.wrap_extent);
    }
    if (SUN_CASCADE_VIEW && sun.cascade_count > 0u) {
        lit = lit * sun_cascade_tint(in.world_pos);
    }
    if (!VIEWMODEL && !BLENDED && env.wet.w > 0.5) {
        lit = lit * rain_view_tint(in.world_pos);
    }
    // Emissive rides the HDR/bloom pipeline: colour * intensity, no new pass, no new uniform — times
    // the emissive map where one is bound.
    var emissive = base * max(in.material.z, 0.0);
    if ((material.flags & MATERIAL_EMISSIVE) != 0u) {
        emissive = emissive * textureSample(emissive_tex, material_samp, in.uv).rgb;
    }
    lit = lit + emissive;
    // Rim: an unlit Fresnel edge, added like emissive so the composite's medium treats them alike.
    // A probe capture leaves it out: its eye is not the player's.
    if (with_rim && any(in.rim.rgb > vec3<f32>(0.0))) {
        let v = normalize(view_to_eye(camera.cam_pos.xyz - in.world_pos, camera.view_forward));
        let edge = 1.0 - clamp(dot(shaded.n, v), 0.0, 1.0);
        lit = lit + in.rim.rgb * pow(edge, in.rim.a);
    }
    // Hit-flash: lerp the shaded colour toward flash_color by the amount. amount 0 mixes nothing —
    // render-identical by construction.
    lit = mix(lit, in.flash.rgb, clamp(in.flash.a, 0.0, 1.0));
    // Clamp below half-float overflow: a mirror highlight can shade past 65504, which the scene
    // target would store as infinity (or saturate), and the bloom chain would smear it.
    var out: CaptureOut;
    out.color = vec4<f32>(hdr_safe(lit), in.color.a);
    if (BLENDED) {
        out.color.a = coverage * eff_fade;
    }
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
