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
// `scatter.steps` samples there alone. A light is unshadowed here: the air in front of a wall glows
// whether or not the light's own geometry would shade it.
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
// `VsOut` + `vs_main` come from `shaders/snippets/fullscreen_vs.wgsl`, `BAYER4` from
// `shaders/snippets/bayer.wgsl`, the medium functions from `shaders/snippets/medium.wgsl`.

struct ScatterLight {
    // xyz = apex, w = range (the CPU floors it at 1e-3, as `light_sphere` does).
    position: vec4<f32>,
    // rgb = colour · intensity · scatter, w = cos_inner.
    radiance: vec4<f32>,
    // xyz = unit axis, w = cos_outer; a point light writes (0, 0, 0, -2).
    axis: vec4<f32>,
    // The bounding sphere: xyz centre, w radius.
    sphere: vec4<f32>,
    // x = first shadow view or 0xFFFFFFFF, y = view count; always unshadowed here.
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
    lights: array<ScatterLight, 64>,
};

// The scene pass's depth; every fetch is a `textureLoad` (unfilterable depth, shared filtering sampler).
@group(0) @binding(0) var scatter_depth: texture_depth_2d;
// The transparent layer (weighted distance in R, coverage in G): the **optional** input. Otherwise a
// 1x1 stand-in is bound and `scatter.transparent_depth` is zero, so the fetch never reads it.
@group(0) @binding(1) var scatter_layer: texture_2d<f32>;
@group(0) @binding(2) var scatter_samp: sampler;
@group(0) @binding(3) var<uniform> scatter: MediumScatter;

const SCATTER_OPTICAL_CUTOFF: f32 = 3.0;
const SCATTER_MIN_DENSITY: f32 = 1e-4;
const SCATTER_FULL_COVERAGE: f32 = 0.999;
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
            var weight = cone_factor(light, l) * atten * atten * henyey_greenstein(dot(ray, l))
                * transmittance * local;
            if (coverage > 0.0 && t > layer_t) {
                weight = weight * past;
            }
            acc = acc + light.radiance.rgb * weight;
        }
        sum = sum + acc * dt;
    }
    return vec4<f32>(sum * scatter.albedo * scatter.density, 1.0);
}
