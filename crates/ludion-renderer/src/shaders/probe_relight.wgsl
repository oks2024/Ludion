// The probe relight kernel: the normal-free diffuse irradiance E at a world point, composed the way
// `mesh.wgsl` composes its ambient term with `diffuse = 1` and no occlusion. One thread per point.
// The same kernel runs at capture (baked E, stored with the cube) and every frame (live E), so a
// static scene's ratio is x / x = 1 exactly.
//
//   E = base * mix(1, clamp(vis.x, 0, 1), sky_visibility)  [+ bounce * (bounce_r.x, bounce_g.x, bounce_b.x)]
//   base = k * mean(irradiance cube at +-X +-Y +-Z) + ambient   (with an Environment; else ambient)
//
// The visibility factor applies only with a `GiVolume` (flag on), the bounce term only at
// `bounce > 0`; both read the DC term (`.x`) of the SH-L1 volumes. The dynamic injection is not
// included: it is a graph transient, and it carries moving lights rather than the clock.

const MAX_RELIGHT_POINTS: u32 = 32u;

struct RelightParams {
    // xyz = world position, per point.
    points: array<vec4<f32>, 32>,
    // x = the point count, y = the Environment flag (1 = on), z = `ibl_intensity`, w = the GI flag.
    counts: vec4<f32>,
    // xyz = the volume's world-space origin.
    gi_origin: vec4<f32>,
    // xyz = the reciprocal of the volume's world-space span.
    gi_scale: vec4<f32>,
    // x = `GiVolume.sky_visibility`, y = `GiVolume.bounce`.
    strengths: vec4<f32>,
    // xyz = the flat ambient term.
    ambient: vec4<f32>,
}

@group(0) @binding(0) var<uniform> relight: RelightParams;
@group(0) @binding(1) var relight_irradiance: texture_cube<f32>;
@group(0) @binding(2) var relight_samp: sampler;
@group(0) @binding(3) var relight_vis: texture_3d<f32>;
@group(0) @binding(4) var relight_bounce_r: texture_3d<f32>;
@group(0) @binding(5) var relight_bounce_g: texture_3d<f32>;
@group(0) @binding(6) var relight_bounce_b: texture_3d<f32>;
// One vec4 per point: E in `xyz`.
@group(0) @binding(7) var<storage, read_write> relight_out: array<vec4<f32>>;

@compute @workgroup_size(32)
fn relight_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u32(relight.counts.x)) {
        return;
    }
    let p = relight.points[id.x].xyz;
    var e = relight.ambient.rgb;
    if (relight.counts.y > 0.5) {
        var sum = textureSampleLevel(relight_irradiance, relight_samp, vec3<f32>(1.0, 0.0, 0.0), 0.0).rgb;
        sum = sum + textureSampleLevel(relight_irradiance, relight_samp, vec3<f32>(-1.0, 0.0, 0.0), 0.0).rgb;
        sum = sum + textureSampleLevel(relight_irradiance, relight_samp, vec3<f32>(0.0, 1.0, 0.0), 0.0).rgb;
        sum = sum + textureSampleLevel(relight_irradiance, relight_samp, vec3<f32>(0.0, -1.0, 0.0), 0.0).rgb;
        sum = sum + textureSampleLevel(relight_irradiance, relight_samp, vec3<f32>(0.0, 0.0, 1.0), 0.0).rgb;
        sum = sum + textureSampleLevel(relight_irradiance, relight_samp, vec3<f32>(0.0, 0.0, -1.0), 0.0).rgb;
        e = sum / 6.0 * relight.counts.z + e;
    }
    if (relight.counts.w > 0.5) {
        let uv = (p - relight.gi_origin.xyz) * relight.gi_scale.xyz;
        let vis = textureSampleLevel(relight_vis, relight_samp, uv, 0.0).x;
        e = e * mix(1.0, clamp(vis, 0.0, 1.0), relight.strengths.x);
        if (relight.strengths.y > 0.0) {
            let bounce = vec3<f32>(
                textureSampleLevel(relight_bounce_r, relight_samp, uv, 0.0).x,
                textureSampleLevel(relight_bounce_g, relight_samp, uv, 0.0).x,
                textureSampleLevel(relight_bounce_b, relight_samp, uv, 0.0).x,
            );
            e = e + relight.strengths.y * bounce;
        }
    }
    relight_out[id.x] = vec4<f32>(e, 0.0);
}
