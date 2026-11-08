// The participating medium's **geometry**: where it begins along a ray and how its density varies
// with world height. Shared by the two terms of the medium so they cannot disagree about its shape —
// `composite.wgsl`'s analytic ambient in-scatter and `shafts.wgsl`'s marched directional one
// (Plan/volumetric-medium.md).
//
// A snippet cannot read a per-shader uniform struct, so both functions take the medium's parameters
// explicitly: `start` (`fog_start`), `height` (`fog_height`), `falloff` (`fog_height_falloff`), the
// camera's world height `cam_y`, and `up` — the ray direction's normalized y, which is what turns a
// distance along the ray into a height.
//
// The density model is Wenzel's (Crytek, "Real-time Atmospheric Effects in Games", GDC 2006): the
// medium exists only past `start`, and within it
//
//     σ_t(y) = density * exp(-falloff * (y - height))
//
// so `height` is where the density is exactly `density` and `falloff` is how fast it thins going up.
// At `falloff = 0` that is a constant `density` and the closed form below collapses to `density * seg`
// exactly — the height half is the same formula at its own limit, never a separate case, which is what
// lets `fog_height_falloff: 0` be a *skip* rather than a branch in either consumer.

// Below this the height integral's `(1 - exp(-x)) / x` is 1 to well within f32, and at exactly zero
// (a level ray) it would be 0/0.
const HEIGHT_EPSILON: f32 = 1e-4;
// The height exponent is clamped to this magnitude: a camera far below `height` under a steep falloff
// would otherwise overflow the exponential, and the medium has saturated long before e^64.
const MAX_HEIGHT_EXPONENT: f32 = 64.0;

// The optical depth `∫ σ_t ds` from the camera out to distance `d`, so the transmittance to that point
// is `exp(-optical_depth)`. Wenzel's closed form for the integral over the segment past `start`:
//
//     T = density * seg * exp(-falloff * (y0 - height)) * (1 - exp(-falloff * dy)) / (falloff * dy)
//
// where `seg` is that segment's length, `y0` its starting height and `dy` its rise. Integrating over
// the segment rather than scaling the whole ray down is what makes `start` and the height falloff
// compose exactly rather than approximately.
fn medium_optical_depth(
    density: f32,
    start: f32,
    height: f32,
    falloff: f32,
    cam_y: f32,
    up: f32,
    d: f32,
) -> f32 {
    let segment = max(d - start, 0.0);
    if (segment <= 0.0) {
        return 0.0;
    }
    var optical_depth = density * segment;
    if (falloff != 0.0) {
        let start_y = cam_y + up * min(start, d);
        let rise = clamp(falloff * up * segment, -MAX_HEIGHT_EXPONENT, MAX_HEIGHT_EXPONENT);
        var ramp = 1.0;
        if (abs(rise) >= HEIGHT_EPSILON) {
            ramp = (1.0 - exp(-rise)) / rise;
        }
        let base_exponent = clamp(
            -falloff * (start_y - height),
            -MAX_HEIGHT_EXPONENT,
            MAX_HEIGHT_EXPONENT,
        );
        optical_depth = optical_depth * exp(base_exponent) * ramp;
    }
    return optical_depth;
}

// The medium's **local** density at distance `t`, as a ratio of `density` — 0 before the medium
// begins, and exactly `1.0` in a height-free one. A marched in-scatter multiplies its σ_s by this, so
// the coefficient it integrates is the density at the sample rather than the medium's nominal one.
//
// The exact `1.0` is load-bearing: `x * 1.0` is bit-preserving in IEEE-754, so a caller can fold the
// ratio into an existing product without a branch and a height-free medium still renders the bytes it
// rendered before the height term existed.
//
// The boundary is `t < start`, not `t <= start`, so a `start` of 0 admits the sample at the lens
// itself and the neutral case stays exact.
fn medium_density_ratio(
    start: f32,
    height: f32,
    falloff: f32,
    cam_y: f32,
    up: f32,
    t: f32,
) -> f32 {
    if (t < start) {
        return 0.0;
    }
    if (falloff == 0.0) {
        return 1.0;
    }
    let exponent = clamp(
        -falloff * (cam_y + up * t - height),
        -MAX_HEIGHT_EXPONENT,
        MAX_HEIGHT_EXPONENT,
    );
    return exp(exponent);
}

const FOUR_PI: f32 = 12.5663706143592;

// The Henyey–Greenstein phase at asymmetry `g`.
fn henyey_greenstein(g: f32, cos_theta: f32) -> f32 {
    let g2 = g * g;
    let denom = 1.0 + g2 - 2.0 * g * cos_theta;
    // The floor is defence in depth against a *validating* `g` close enough to ±1 that the exact
    // backscatter ray takes the denominator below f32 resolution; the component rejects ±1 itself.
    return (1.0 - g2) / (FOUR_PI * pow(max(denom, 1e-4), 1.5));
}
