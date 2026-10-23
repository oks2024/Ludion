// Reflection-probe shading, binding-free: a probe's weight at a point and its parallax-corrected
// cube lookup, plus the relight of its captured radiance. The `reflections` pass composes it; the
// functions take their inputs as arguments, so any pass could call them. Twins of `probe.rs`'s `probe_weight`,
// `probe_lookup`, `ratio` and `relit`, in the same operation order.
//
// Every member is a `vec4` so the struct has a 16-byte stride in a uniform array.
struct ShadeProbe {
    // xyz = world position (the capture point and volume centre), w = the cube array slot.
    center: vec4<f32>,
    // The world-space columns of the box rotation (xyz); w unused.
    axis_x: vec4<f32>,
    axis_y: vec4<f32>,
    axis_z: vec4<f32>,
    // A box: xyz = half-extents. A sphere: x = radius.
    extent: vec4<f32>,
    // x = 0 for a box, 1 for a sphere; y = the outward blend distance.
    params: vec4<f32>,
    // xyz = the irradiance baked with the capture (the relight's denominator); w unused.
    light: vec4<f32>,
};

const MAX_SHADE_PROBES: u32 = 32u;

// The world offset `v` in the probe's local frame (the rotation's transpose).
fn probe_to_local(probe: ShadeProbe, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(probe.axis_x.xyz, v), dot(probe.axis_y.xyz, v), dot(probe.axis_z.xyz, v));
}

// 1 inside and on the volume, ramping to 0 over `blend` world units outside it.
fn probe_weight(probe: ShadeProbe, p: vec3<f32>) -> f32 {
    var d: f32;
    if (probe.params.x > 0.5) {
        d = max(length(p - probe.center.xyz) - probe.extent.x, 0.0);
    } else {
        let q = probe_to_local(probe, p - probe.center.xyz);
        d = length(max(abs(q) - probe.extent.xyz, vec3<f32>(0.0)));
    }
    if (d <= 0.0) {
        return 1.0;
    }
    if (probe.params.y <= 0.0) {
        return 0.0;
    }
    return clamp(1.0 - d / probe.params.y, 0.0, 1.0);
}

// The direction from the capture point to sample for reflection ray `r` from `p`: the ray's hit on
// the volume, relative to the capture point. A point in the blend shell outside the volume is
// clamped onto it first, so the lookup is continuous across the surface.
fn probe_lookup(probe: ShadeProbe, p: vec3<f32>, r: vec3<f32>) -> vec3<f32> {
    if (probe.params.x > 0.5) {
        var q = p - probe.center.xyz;
        let len = length(q);
        if (len > probe.extent.x) {
            q = q * (probe.extent.x / len);
        }
        let b = dot(q, r);
        let c = dot(q, q) - probe.extent.x * probe.extent.x;
        let t = -b + sqrt(max(b * b - c, 0.0));
        return q + t * r;
    }
    let q = clamp(
        probe_to_local(probe, p - probe.center.xyz), -probe.extent.xyz, probe.extent.xyz);
    let d = probe_to_local(probe, r);
    let guarded = select(max(d, vec3<f32>(1e-6)), min(d, vec3<f32>(-1e-6)), d < vec3<f32>(0.0));
    let inv = vec3<f32>(1.0) / guarded;
    let a = (probe.extent.xyz - q) * inv;
    let b = (-probe.extent.xyz - q) * inv;
    let far = max(a, b);
    let t = min(far.x, min(far.y, far.z));
    let local = q + t * d;
    return probe.axis_x.xyz * local.x + probe.axis_y.xyz * local.y + probe.axis_z.xyz * local.z;
}

// The live-to-baked irradiance ratio, per channel; 1 where the baked value is too small to divide by.
fn probe_ratio(baked: vec3<f32>, live: vec3<f32>) -> vec3<f32> {
    return select(vec3<f32>(1.0), live / baked, baked > vec3<f32>(1e-6));
}

// A prefiltered probe sample relit: its geometry radiance (rgb) follows the irradiance `ratio`, its
// sky fraction (alpha) is the live sky `sky` (the sky cube along the same ray) scaled by `k`.
fn probe_relit(sample: vec4<f32>, ratio: vec3<f32>, sky: vec3<f32>, k: f32) -> vec3<f32> {
    return sample.rgb * ratio + sample.a * sky * k;
}
