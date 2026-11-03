// The `Environment`'s authored sky radiance as a closed-form function of direction. Shared by the
// pass that *draws* the sky (`sky.wgsl`), the pass that *in-scatters* it — `composite.wgsl`'s
// ambient haze term (Plan/volumetric-medium.md) — so the haze and the background it meets at the
// horizon cannot disagree about what colour the sky is, and the compute bake that integrates it into
// the diffuse and specular IBL cubes (`ibl.wgsl`).
//
// This is closed form only because the engine's sky is procedural by decision and will never be an
// image asset: three authored colours and a direction, evaluated exactly, for less arithmetic than a
// low-order spherical-harmonic approximation of the same thing would cost — and with no truncation
// error and no cube to bind.
//
// A snippet cannot read a per-shader uniform struct, so the three colours are parameters; each caller
// passes its own uniform's values.

// The authored gradient at a normalized direction: horizon at the horizontal, easing to zenith
// above and ground below. A sqrt easing keeps the horizon band tight, the sky-dome look.
fn sky_gradient(
    dir: vec3<f32>,
    zenith: vec3<f32>,
    horizon: vec3<f32>,
    ground: vec3<f32>,
) -> vec3<f32> {
    let t = dir.y;
    if (t >= 0.0) {
        return mix(horizon, zenith, pow(clamp(t, 0.0, 1.0), 0.5));
    }
    return mix(horizon, ground, pow(clamp(-t, 0.0, 1.0), 0.5));
}
