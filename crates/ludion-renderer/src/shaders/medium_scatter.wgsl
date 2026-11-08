// Local-light in-scatter: the medium's third term, the share of each scattering point or spot light
// the air throws back along the eye ray. The shafts' march (`shafts.wgsl`) is the model — same ray,
// same end, same Bayer-jittered steps, same medium functions — with the sun replaced by a capped
// list of local lights, each marched only across the segment of the ray inside its bounding sphere.
//
// # The estimator
//
// For each selected light `l`, with `p(t) = origin + ray·t`:
//
//     L_l = ∫ σ_s(t) · T(t) · phase(ray · l̂(t)) · scatter_l · E_l(p(t)) dt
//     E_l = color · intensity · atten² · cone,   atten = clamp(1 - d/range, 0, 1)
//
// `E_l` is the surface model's own falloff (`lights.wgsl`), so a light that lights a wall in a
// radius glows in the air over that radius. `T` and `σ_s` are `shafts.wgsl`'s, through the shared
// `medium.wgsl`, so `fog_start` and the height falloff shape the glow exactly as they shape the haze
// and the beams. The phase is Henyey–Greenstein at `fog_anisotropy`, measured against the direction
// from the sample *toward* the light, as the shafts measure against `-sun_dir`.
//
// The light's contribution is zero outside its bounding sphere (`cluster::light_sphere`: the range
// sphere, or a spot's cone sphere), so the march clips the ray to that sphere and spends its
// `scatter.steps` samples there alone. A light that holds a local-shadow grant this frame shadows its
// own in-scatter: each step tests one exact atlas texel (`scatter_visibility`), so the air behind a
// pillar stays dark. A light without a grant scatters unshadowed.
//
// # The march's end and the transparent layer
//
// As `shafts.wgsl::layered_march`: a ray stops at the recorded surface or the medium's extinction
// cutoff; a fully covered texel stops at the layer's mean distance, and past it a sample counts only
// for the share the layer leaves uncovered. Under an orthographic camera the ray runs along the view
// from the pixel's own origin on the camera plane. This pass has one path for every pixel.
//
// # Determinism
//
// No clock, frame index or RNG: the step offset is `BAYER4` at the fragment's pixel, which
// `box_blur`'s 4×4 period averages away exactly (here through `medium_scatter_blur`).
//
// # The GI term
//
// The haze's ambient in-scatter is `α(1 - T)·L_ambient`, a glow at full ambient wherever the eye ray
// goes. In a scene with a `GiVolume` the march also integrates the volume along the same ray:
//
//     V̄ = Σ w·mix(1, vis, s_vis) / Σ w          (alpha: the composite scales its ambient by it)
//     L_b = σ_s · bounce · Σ w·B · dt           (added to rgb, on top of the lights' term)
//
// `w = T·local` is the in-scatter weight of the ambient closed form (`σ_t·T·local`, up to the constant
// the mean cancels), `vis` the volume's baked DC sky visibility and `B` its bounce DC plus the injected
// dynamic DC. The medium is isotropic, so only the DC term (`n = 0` in `mesh_shading.wgsl`'s `sh_l1`)
// is read, with the mesh pass's clamps. Outside the volume `vis = 1` and `B = 0`: no leak past the
// lattice. With the flag off alpha is exactly 1.
//
// `VsOut` + `vs_main` come from `shaders/snippets/fullscreen_vs.wgsl`, `BAYER4` from
// `shaders/snippets/bayer.wgsl`, the medium functions from `shaders/snippets/medium.wgsl`.

// Field-for-field `mesh_shading.wgsl`'s `LocalView`: one shadow view of the local-shadow atlas.
struct LocalView {
    light_space: mat4x4<f32>,
    // xy = the tile's UV origin in the atlas, z = its UV edge, w = one atlas texel in UV.
    rect: vec4<f32>,
    // x = depth bias in world units, y = tile texel per unit distance, z = views of this light,
    // w = distance-fade strength.
    params: vec4<f32>,
};

struct ScatterLight {
    // xyz = apex, w = range (the CPU floors it at 1e-3, as `light_sphere` does).
    position: vec4<f32>,
    // rgb = colour · intensity · scatter, w = cos_inner.
    radiance: vec4<f32>,
    // xyz = unit axis, w = cos_outer; a point light writes (0, 0, 0, -2).
    axis: vec4<f32>,
    // The bounding sphere: xyz centre, w radius.
    sphere: vec4<f32>,
    // x = this light's first slot in `views` or SCATTER_UNSHADOWED, y = its view count (1 or 6).
    shadow: vec4<u32>,
};

struct MediumScatter {
    // Clip→world, for reconstructing the eye ray and the recorded surface's world position.
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec3<f32>,
    // σ_t (`PostProcess.fog_density`).
    density: f32,
    // α (`fog_albedo`) and the Henyey–Greenstein g (`fog_anisotropy`).
    albedo: f32,
    anisotropy: f32,
    // The medium's geometry, as `shafts.wgsl` carries it.
    fog_start: f32,
    fog_height: f32,
    fog_height_falloff: f32,
    // Steps per light segment (`QualityParams.shaft_steps / 4`, floored at 2): a quality setting.
    steps: u32,
    // How many `lights` entries are real.
    light_count: u32,
    // Nonzero when `scatter_layer` is real.
    transparent_depth: u32,
    // `.xyz` is the unit view direction and `.w` is 1 under an orthographic camera.
    view_forward: vec4<f32>,
    // The GiVolume's placement, as `EnvParams` carries it: xyz origin, and xyz 1/span (w unused here).
    gi_origin: vec4<f32>,
    gi_scale: vec4<f32>,
    // x = sky_visibility strength, y = bounce strength, z = 1 when `scatter_dyn` is real, w = 1 when
    // the GI term runs at all.
    gi: vec4<f32>,
    lights: array<ScatterLight, 64>,
    // The granted lights' views, compacted: each light's views are consecutive from `shadow.x`.
    views: array<LocalView, 64>,
};

// The scene pass's depth; every fetch is a `textureLoad` (unfilterable depth, shared filtering sampler).
@group(0) @binding(0) var scatter_depth: texture_depth_2d;
// The transparent layer (weighted distance in R, coverage in G): the **optional** input. Otherwise a
// 1x1 stand-in is bound and `scatter.transparent_depth` is zero, so the fetch never reads it.
@group(0) @binding(1) var scatter_layer: texture_2d<f32>;
// The local-shadow atlas: the **optional** input. A light's `shadow.x` is SCATTER_UNSHADOWED whenever
// the frame has no atlas, so the 1x1 stand-in is never read.
@group(0) @binding(2) var scatter_atlas: texture_depth_2d;
// The injected dynamic-bounce volume: the **optional** input (the 1x1x1 fallback with `scatter.gi.z`
// zero otherwise).
@group(0) @binding(3) var scatter_dyn: texture_3d<f32>;
@group(0) @binding(4) var scatter_samp: sampler;
@group(0) @binding(5) var<uniform> scatter: MediumScatter;
// The baked volumes (renderer-owned, same lattice as `scatter_dyn`): the SH-L1 sky visibility and
// the bounce in R, G, B, each read at its DC term.
@group(1) @binding(0) var scatter_gi: texture_3d<f32>;
@group(1) @binding(1) var scatter_gi_r: texture_3d<f32>;
@group(1) @binding(2) var scatter_gi_g: texture_3d<f32>;
@group(1) @binding(3) var scatter_gi_b: texture_3d<f32>;

const SCATTER_OPTICAL_CUTOFF: f32 = 3.0;
const SCATTER_MIN_DENSITY: f32 = 1e-4;
const SCATTER_FULL_COVERAGE: f32 = 0.999;
const SCATTER_UNSHADOWED: u32 = 0xFFFFFFFFu;
const FOUR_PI: f32 = 12.5663706143592;

fn world_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let world = scatter.inv_view_proj * vec4<f32>(ndc, 1.0);
    return world.xyz / world.w;
}

fn henyey_greenstein(cos_theta: f32) -> f32 {
    let g = scatter.anisotropy;
    let g2 = g * g;
    let denom = 1.0 + g2 - 2.0 * g * cos_theta;
    return (1.0 - g2) / (FOUR_PI * pow(max(denom, 1e-4), 1.5));
}

// A spot's cone factor for the unit vector `l` from the sample toward the light: 1 inside the inner
// cone, a Hermite ramp between, an exact 0 outside the outer one. A local copy of
// `lights.wgsl::spot_cone`, which binds group 1; a point light (`axis.w < -1.5`) is 1.
fn cone_factor(light: ScatterLight, l: vec3<f32>) -> f32 {
    let cos_outer = light.axis.w;
    if (cos_outer < -1.5) {
        return 1.0;
    }
    let cos_inner = light.radiance.w;
    let cos_angle = dot(-l, light.axis.xyz);
    if (!(cos_angle > cos_outer)) {
        return 0.0;
    }
    if (cos_angle >= cos_inner) {
        return 1.0;
    }
    let t = (cos_angle - cos_outer) / (cos_inner - cos_outer);
    return t * t * (3.0 - 2.0 * t);
}

// 1.0 lit. `l` is the unit vector from the sample toward the light. A free-space point: one exact
// texel, no PCF and no normal offset (as the shafts' `sun_visibility`). The cube face is picked by the
// major axis of -l, ties X then Y then Z, +axis first: `mesh_shading.wgsl::local_shadow`'s table.
fn scatter_visibility(light: ScatterLight, p: vec3<f32>, l: vec3<f32>) -> f32 {
    if (light.shadow.x == SCATTER_UNSHADOWED) {
        return 1.0;
    }
    let cube = light.shadow.y > 1u;
    var vi = light.shadow.x;
    if (cube) {
        let a = abs(-l);
        if (a.x >= a.y && a.x >= a.z) {
            vi = vi + select(1u, 0u, -l.x > 0.0);
        } else if (a.y >= a.z) {
            vi = vi + select(3u, 2u, -l.y > 0.0);
        } else {
            vi = vi + select(5u, 4u, -l.z > 0.0);
        }
    }
    let view = scatter.views[vi];
    let clip = view.light_space * vec4<f32>(p + l * view.params.x, 1.0);
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
    // The tile in texels; the texel is clamped inside it so a cube seam never reads the neighbour.
    let dims = vec2<f32>(textureDimensions(scatter_atlas));
    let origin = view.rect.xy * dims;
    let size = view.rect.z * dims;
    let local = clamp(vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5), vec2<f32>(0.0), vec2<f32>(1.0));
    let texel = clamp(
        vec2<i32>(origin + local * size),
        vec2<i32>(origin),
        vec2<i32>(origin + size) - vec2<i32>(1),
    );
    let lit = select(0.0, 1.0, ndc.z <= textureLoad(scatter_atlas, texel, 0));
    // `select`, not a bare `mix`: strength 1 returns the texel bit for bit.
    return select(mix(1.0, lit, view.params.w), lit, view.params.w >= 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Snap to the source texel's centre, as `shafts.wgsl`: this target is a fraction of the render
    // extent, and the depth and the ray must describe one pixel.
    let dims = vec2<f32>(textureDimensions(scatter_depth));
    let texel = floor(in.uv * dims);
    let source_uv = (texel + vec2<f32>(0.5)) / dims;
    let depth = textureLoad(scatter_depth, vec2<i32>(texel), 0);

    var coverage = 0.0;
    var layer_t = 0.0;
    if (scatter.transparent_depth != 0u) {
        let layer = textureLoad(scatter_layer, vec2<i32>(texel), 0);
        coverage = layer.g;
        layer_t = layer.r / max(layer.g, 1e-4);
    }

    let far = world_from_depth(source_uv, 1.0);
    let to_far = far - scatter.cam_pos;
    let ortho = scatter.view_forward.w > 0.5;
    var origin = scatter.cam_pos;
    var ray = normalize(to_far);
    if (ortho) {
        ray = scatter.view_forward.xyz;
        origin = far - ray * dot(to_far, ray);
    }
    var end = SCATTER_OPTICAL_CUTOFF / max(scatter.density, SCATTER_MIN_DENSITY);
    if (depth < 1.0) {
        let to_surface = world_from_depth(source_uv, depth) - origin;
        var surface = length(to_surface);
        if (ortho) {
            surface = dot(to_surface, ray);
        }
        end = min(end, surface);
    }
    if (coverage >= SCATTER_FULL_COVERAGE) {
        end = min(end, layer_t);
    }
    let past = 1.0 - coverage;
    let jitter = BAYER4[(u32(in.pos.y) % 4u) * 4u + (u32(in.pos.x) % 4u)];

    var sum = vec3<f32>(0.0);
    for (var i: u32 = 0u; i < scatter.light_count; i = i + 1u) {
        let light = scatter.lights[i];
        // The ray against the bounding sphere, clipped to the march: nothing outside it scatters.
        let oc = origin - light.sphere.xyz;
        let b = dot(oc, ray);
        let q = dot(oc, oc) - light.sphere.w * light.sphere.w;
        let disc = b * b - q;
        if (disc <= 0.0) {
            continue;
        }
        let root = sqrt(disc);
        let t0 = max(-b - root, 0.0);
        let t1 = min(-b + root, end);
        if (t1 <= t0) {
            continue;
        }
        let dt = (t1 - t0) / f32(scatter.steps);
        var acc = vec3<f32>(0.0);
        for (var k: u32 = 0u; k < scatter.steps; k = k + 1u) {
            let t = t0 + (f32(k) + jitter) * dt;
            let p = origin + ray * t;
            let to_l = light.position.xyz - p;
            let dist = length(to_l);
            let l = select(vec3<f32>(0.0), to_l / dist, dist > 0.0);
            let atten = clamp(1.0 - dist / light.position.w, 0.0, 1.0);
            let transmittance = exp(-medium_optical_depth(
                scatter.density,
                scatter.fog_start,
                scatter.fog_height,
                scatter.fog_height_falloff,
                origin.y,
                ray.y,
                t,
            ));
            let local = medium_density_ratio(
                scatter.fog_start,
                scatter.fog_height,
                scatter.fog_height_falloff,
                origin.y,
                ray.y,
                t,
            );
            var weight = cone_factor(light, l) * scatter_visibility(light, p, l) * atten * atten
                * henyey_greenstein(dot(ray, l)) * transmittance * local;
            if (coverage > 0.0 && t > layer_t) {
                weight = weight * past;
            }
            acc = acc + light.radiance.rgb * weight;
        }
        sum = sum + acc * dt;
    }
    var ambient_scale = 1.0;
    // The background takes no medium (the composite skips it); a covered transparent texel does.
    if (scatter.gi.w != 0.0 && (depth < 1.0 || coverage > 0.0)) {
        let dt = end / f32(scatter.steps);
        var w_sum = 0.0;
        var w_vis = 0.0;
        var w_bounce = vec3<f32>(0.0);
        for (var k: u32 = 0u; k < scatter.steps; k = k + 1u) {
            let t = (f32(k) + jitter) * dt;
            let p = origin + ray * t;
            var weight = exp(-medium_optical_depth(
                scatter.density,
                scatter.fog_start,
                scatter.fog_height,
                scatter.fog_height_falloff,
                origin.y,
                ray.y,
                t,
            )) * medium_density_ratio(
                scatter.fog_start,
                scatter.fog_height,
                scatter.fog_height_falloff,
                origin.y,
                ray.y,
                t,
            );
            if (coverage > 0.0 && t > layer_t) {
                weight = weight * past;
            }
            let uv = (p - scatter.gi_origin.xyz) * scatter.gi_scale.xyz;
            var vis = 1.0;
            var bounce = vec3<f32>(0.0);
            if (all(uv >= vec3<f32>(0.0)) && all(uv <= vec3<f32>(1.0))) {
                vis = clamp(textureSampleLevel(scatter_gi, scatter_samp, uv, 0.0).x, 0.0, 1.0);
                bounce = max(vec3<f32>(
                    textureSampleLevel(scatter_gi_r, scatter_samp, uv, 0.0).x,
                    textureSampleLevel(scatter_gi_g, scatter_samp, uv, 0.0).x,
                    textureSampleLevel(scatter_gi_b, scatter_samp, uv, 0.0).x,
                ), vec3<f32>(0.0));
                if (scatter.gi.z > 0.5) {
                    bounce = bounce
                        + max(textureSampleLevel(scatter_dyn, scatter_samp, uv, 0.0).rgb, vec3<f32>(0.0));
                }
            }
            w_sum = w_sum + weight;
            w_vis = w_vis + weight * mix(1.0, vis, scatter.gi.x);
            w_bounce = w_bounce + weight * bounce;
        }
        if (w_sum > 0.0) {
            ambient_scale = w_vis / w_sum;
        }
        sum = sum + w_bounce * (scatter.gi.y * dt);
    }
    return vec4<f32>(sum * scatter.albedo * scatter.density, ambient_scale);
}
