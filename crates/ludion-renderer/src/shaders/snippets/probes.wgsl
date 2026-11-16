// Reflection-probe shading, binding-free: a probe's weight at a point and its parallax-corrected
// cube lookup, plus the relight of its captured radiance. The `reflections` pass composes it; the
// functions take their inputs as arguments, so any pass could call them. Twins of `probe.rs`'s `probe_weight`,
// `probe_lookup`, `axis_rows`, `axis_ratio`, `ratio` and `relit`, in the same operation order.
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
    // x = 0 for a box, 1 for a sphere; y = the outward blend distance; z = 1 when the probe has an
    // emissive cube to sample, else 0.
    params: vec4<f32>,
    // xyz = the irradiance baked with the capture per cube axis (+X -X +Y -Y +Z -Z), the relight's
    // denominator.
    baked: array<vec4<f32>, 6>,
};

const MAX_SHADE_PROBES: u32 = 32u;

// The world offset `v` in the probe's local frame (the rotation's transpose).
fn probe_to_local(probe: ShadeProbe, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(probe.axis_x.xyz, v), dot(probe.axis_y.xyz, v), dot(probe.axis_z.xyz, v));
}

// A point this close to the volume counts as on it: a receiver rebuilt from depth lands on a face
// within float error that grows with its distance from the camera. Twins in `probe.rs`.
const PROBE_FACE_TOLERANCE_REL: f32 = 1e-4;
const PROBE_FACE_TOLERANCE_MIN: f32 = 1e-3;

// 1 inside, on and within the face tolerance of the volume (`view_dist` = the point's distance from
// the camera), ramping to 0 over `blend` world units past that.
fn probe_weight(probe: ShadeProbe, p: vec3<f32>, view_dist: f32) -> f32 {
    var d: f32;
    if (probe.params.x > 0.5) {
        d = max(length(p - probe.center.xyz) - probe.extent.x, 0.0);
    } else {
        let q = probe_to_local(probe, p - probe.center.xyz);
        d = length(max(abs(q) - probe.extent.xyz, vec3<f32>(0.0)));
    }
    d = d - max(PROBE_FACE_TOLERANCE_REL * view_dist, PROBE_FACE_TOLERANCE_MIN);
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

// The ambient-cube rows (+X -X +Y -Y +Z -Z = 0..5) a unit direction `n` weights; the weights are `n * n`.
fn probe_axis_rows(n: vec3<f32>) -> vec3<u32> {
    return vec3<u32>(select(1u, 0u, n.x >= 0.0), select(3u, 2u, n.y >= 0.0), select(5u, 4u, n.z >= 0.0));
}

// The ceiling on the relight ratio: a near-dark capture lit up live would otherwise blow the relit radiance to infinity.
const MAX_RELIGHT_RATIO: f32 = 16.0;

// The live-to-baked irradiance ratio, per channel, at most `MAX_RELIGHT_RATIO`; 1 where the baked value is too small to divide by.
fn probe_ratio(baked: vec3<f32>, live: vec3<f32>) -> vec3<f32> {
    return min(select(vec3<f32>(1.0), live / baked, baked > vec3<f32>(1e-6)), vec3<f32>(MAX_RELIGHT_RATIO));
}

// The ratio along a facing with ambient-cube weights `w` (`n * n`) over the baked rows `b*` and the
// live rows `l*`. Both sums spell the same operations, so equal bits give exactly 1.
fn probe_axis_ratio(
    bx: vec3<f32>, by: vec3<f32>, bz: vec3<f32>,
    lx: vec3<f32>, ly: vec3<f32>, lz: vec3<f32>,
    w: vec3<f32>,
) -> vec3<f32> {
    return probe_ratio(w.x * bx + w.y * by + w.z * bz, w.x * lx + w.y * ly + w.z * lz);
}

// A prefiltered probe sample relit: its geometry radiance (rgb) is the capture `T` with the emissive
// part `em` taken out of the irradiance `ratio` (`T·ratio + em·(1 - ratio)`, so only the lit part
// follows the ambient); its sky fraction (alpha) is the live sky `sky` (the sky cube along the same
// ray) scaled by `k`. At ratio 1 the em term is exactly 0. The sum is clamped because `T` and `em`
// are rounded to fp16 independently.
fn probe_relit(sample: vec4<f32>, em: vec3<f32>, ratio: vec3<f32>, sky: vec3<f32>, k: f32) -> vec3<f32> {
    return max(sample.rgb * ratio + em * (vec3<f32>(1.0) - ratio), vec3<f32>(0.0)) + sample.a * sky * k;
}
