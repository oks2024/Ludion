//! The CPU twin of the renderer's light model: the clamps, falloff, cone and sky-irradiance
//! formulas that `lights.wgsl`, `mesh_shading.wgsl`, `sky_gradient.wgsl` and `ibl.wgsl` evaluate on
//! the GPU. Extract takes its clamps from here and the sim-side `light_exposure` query takes every
//! formula, so each exists once on the CPU; `light_model_matches_the_wgsl` pins the statements to
//! the shaders (`Plan/light-exposure.md` decisions 1–2). Edit this file and the WGSL together.
//!
//! The sim calls this module, so it obeys the sim determinism rules though this crate's clippy
//! denylist does not enforce them: every transcendental goes through `ludion_core::math`, never
//! `f32::cos`/`sin`/`powf`/`exp`, a glam trig helper or `to_radians` followed by std trig. Only
//! `sqrt` and the IEEE basics are used; there is no hash container and no clock.

use ludion_core::math::glam::{Vec2, Vec3};

/// A point or spot light's colour, intensity and range after the clamps extract has always applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalLight {
    /// Linear colour, clamped non-negative.
    pub color: Vec3,
    /// Intensity, clamped non-negative.
    pub intensity: f32,
    /// Range in metres, finite and positive.
    pub range: f32,
}

/// Clamps a local light's authored values: `None` for a range that is not finite and positive (no
/// light to cast); colour and intensity clamp to non-negative, and `f32::max` drops a NaN.
#[must_use]
pub fn local_light(color: [f32; 3], intensity: f32, range: f32) -> Option<LocalLight> {
    if !(range.is_finite() && range > 0.0) {
        return None;
    }
    Some(LocalLight {
        color: Vec3::from(color).max(Vec3::ZERO),
        intensity: intensity.max(0.0),
        range,
    })
}

/// A spot's `(cos_inner, cos_outer)` from its authored half-angles in degrees: both clamp into
/// `[0, 89.9]` with the inner never past the outer, so the cosines stay positive and ordered.
#[must_use]
pub fn spot_cosines(inner_deg: f32, outer_deg: f32) -> (f32, f32) {
    let outer = outer_deg.clamp(0.0, 89.9);
    let inner = inner_deg.clamp(0.0, 89.9).min(outer);
    (
        ludion_core::math::cos(inner.to_radians()),
        ludion_core::math::cos(outer.to_radians()),
    )
}

/// The range falloff before squaring: `mesh_shading.wgsl`'s `clamp(1.0 - dist / range, 0.0, 1.0)`
/// with `range = max(range, 1e-3)`. An exact `+0.0` at and beyond the range.
#[must_use]
pub fn atten(dist: f32, range: f32) -> f32 {
    (1.0 - dist / range.max(1e-3)).clamp(0.0, 1.0)
}

/// A spot's cone factor, a line-for-line port of `lights.wgsl` `spot_cone`: 1 inside the inner
/// cone, a Hermite ramp between, an exact `0.0` at or outside the outer cone. `cos_angle` is
/// `dot(-l, axis)`; a NaN gives 0 and a hard edge (`cos_inner == cos_outer`) never divides.
#[must_use]
#[allow(clippy::neg_cmp_op_on_partial_ord)] // `!(a > b)` is the shader's NaN-to-zero guard
pub fn spot_cone(cos_angle: f32, cos_inner: f32, cos_outer: f32) -> f32 {
    if !(cos_angle > cos_outer) {
        return 0.0;
    }
    if cos_angle >= cos_inner {
        return 1.0;
    }
    let t = (cos_angle - cos_outer) / (cos_inner - cos_outer);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a * (1.0 - t) + b * t
}

/// The authored sky radiance in a normalized direction (`snippets/sky_gradient.wgsl`): the horizon
/// at the horizontal, a sqrt easing to zenith above and ground below.
#[must_use]
pub fn sky_gradient(dir: Vec3, zenith: Vec3, horizon: Vec3, ground: Vec3) -> Vec3 {
    let t = dir.y;
    if t >= 0.0 {
        return mix(
            horizon,
            zenith,
            ludion_core::math::powf(t.clamp(0.0, 1.0), 0.5),
        );
    }
    mix(
        horizon,
        ground,
        ludion_core::math::powf((-t).clamp(0.0, 1.0), 0.5),
    )
}

/// Samples per irradiance estimate, `IRR_SAMPLES` in `ibl.wgsl`.
const IRR_SAMPLES: u32 = 64;

fn radical_inverse_vdc(bits_in: u32) -> f32 {
    let mut bits = bits_in;
    bits = bits.rotate_right(16);
    bits = ((bits & 0x5555_5555) << 1) | ((bits & 0xAAAA_AAAA) >> 1);
    bits = ((bits & 0x3333_3333) << 2) | ((bits & 0xCCCC_CCCC) >> 2);
    bits = ((bits & 0x0F0F_0F0F) << 4) | ((bits & 0xF0F0_F0F0) >> 4);
    bits = ((bits & 0x00FF_00FF) << 8) | ((bits & 0xFF00_FF00) >> 8);
    bits as f32 * 2.328_306_4e-10
}

fn hammersley(i: u32, n: u32) -> Vec2 {
    Vec2::new(i as f32 / n as f32, radical_inverse_vdc(i))
}

/// `ibl.wgsl` `tangent_basis`: `(t, b, n)` for a unit `n`. The shader's `up` is parallel to `n` at
/// exactly `+-X` (its cube texels never land there); a query can, so that case takes `Y`.
fn tangent_basis(n: Vec3) -> (Vec3, Vec3, Vec3) {
    let up = if n.z.abs() < 0.999 && n.x.abs() < 0.999 {
        Vec3::X
    } else {
        Vec3::Y
    };
    let t = up.cross(n).normalize();
    let b = n.cross(t);
    (t, b, n)
}

/// The cosine-weighted mean sky radiance about a unit normal: the 64-sample Hammersley estimate
/// `ibl.wgsl` `irradiance` bakes into the diffuse cube, stored as the mean (no `1/PI`).
#[must_use]
pub fn sky_irradiance(n: Vec3, zenith: Vec3, horizon: Vec3, ground: Vec3) -> Vec3 {
    let (t, b, n) = tangent_basis(n);
    let mut acc = Vec3::ZERO;
    for i in 0..IRR_SAMPLES {
        let xi = hammersley(i, IRR_SAMPLES);
        let phi = 2.0 * std::f32::consts::PI * xi.x;
        let cos_t = (1.0 - xi.y).sqrt();
        let sin_t = xi.y.sqrt();
        let (sin_phi, cos_phi) = ludion_core::math::sin_cos(phi);
        let local = Vec3::new(cos_phi * sin_t, sin_phi * sin_t, cos_t);
        let dir = t * local.x + b * local.y + n * local.z;
        acc += sky_gradient(dir, zenith, horizon, ground);
    }
    acc / IRR_SAMPLES as f32
}

/// The sky's mean over the whole sphere, the closed form of [`sky_gradient`]: each hemisphere
/// averages `sqrt(t)` to 2/3, so the result is `(zenith + horizon + ground) / 3`.
#[must_use]
pub fn sky_sphere_mean(zenith: Vec3, horizon: Vec3, ground: Vec3) -> Vec3 {
    (zenith + horizon + ground) / 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atten_is_exact_at_the_edges() {
        assert_eq!(atten(5.0, 5.0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(atten(9.0, 5.0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(atten(0.0, 5.0), 1.0);
        assert_eq!(atten(2.5, 5.0), 0.5);
    }

    #[test]
    fn hard_edged_cone_is_all_or_nothing() {
        let (inner, outer) = spot_cosines(30.0, 30.0);
        assert_eq!(inner, outer);
        assert_eq!(spot_cone(1.0, inner, outer), 1.0);
        assert_eq!(
            spot_cone(outer - 1e-4, inner, outer).to_bits(),
            0.0_f32.to_bits()
        );
        assert_eq!(spot_cone(outer, inner, outer).to_bits(), 0.0_f32.to_bits());
    }

    #[test]
    fn nan_cone_is_zero_and_the_ramp_midpoint_is_half() {
        assert_eq!(spot_cone(f32::NAN, 0.9, 0.8).to_bits(), 0.0_f32.to_bits());
        assert!((spot_cone(0.85, 0.9, 0.8) - 0.5).abs() < 1e-5);
        assert_eq!(spot_cone(0.95, 0.9, 0.8), 1.0);
    }

    #[test]
    fn local_light_clamps_like_extract() {
        assert!(local_light([1.0; 3], 1.0, 0.0).is_none());
        assert!(local_light([1.0; 3], 1.0, f32::NAN).is_none());
        let l = local_light([-1.0, 0.5, f32::NAN], -2.0, 3.0).unwrap();
        assert_eq!(l.color, Vec3::new(0.0, 0.5, 0.0));
        assert_eq!(l.intensity, 0.0);
        assert_eq!(l.range, 3.0);
    }

    #[test]
    fn irradiance_up_approaches_the_closed_form() {
        let (z, h, g) = (
            Vec3::new(0.2, 0.4, 1.0),
            Vec3::new(0.8, 0.7, 0.5),
            Vec3::splat(0.1),
        );
        let e = sky_irradiance(Vec3::Y, z, h, g);
        let want = 0.2 * h + 0.8 * z;
        assert!(
            (e - want).abs().max_element() < 0.02 * want.max_element(),
            "{e:?} vs {want:?}"
        );
    }

    #[test]
    fn uniform_sky_irradiance_is_that_colour() {
        let c = Vec3::new(0.3, 0.5, 0.7);
        for n in [
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::X,
            Vec3::new(1.0, 2.0, 3.0).normalize(),
        ] {
            let e = sky_irradiance(n, c, c, c);
            assert!((e - c).abs().max_element() < 1e-5, "{e:?}");
        }
        assert!((sky_sphere_mean(c, c, c) - c).abs().max_element() < 1e-6);
    }

    /// The twin mirrors these statements; the numeric agreement is the rendered gate's.
    #[test]
    fn light_model_matches_the_wgsl() {
        let lights = include_str!("shaders/snippets/lights.wgsl");
        let mesh = include_str!("shaders/snippets/mesh_shading.wgsl");
        let sky = include_str!("shaders/snippets/sky_gradient.wgsl");
        let ibl = include_str!("shaders/ibl.wgsl");
        let want: [(&str, &str, &[&str]); 4] = [
            (
                "lights.wgsl",
                lights,
                &[
                    "if (!(cos_angle > cos_outer)) {\n        return 0.0;\n    }",
                    "if (cos_angle >= cos_inner) {\n        return 1.0;\n    }",
                    "let t = (cos_angle - cos_outer) / (cos_inner - cos_outer);",
                    "return t * t * (3.0 - 2.0 * t);",
                    "let cos_angle = dot(-l, light.axis.xyz);",
                ],
            ),
            (
                "mesh_shading.wgsl",
                mesh,
                &[
                    "let range = max(light.position.w, 1e-3);",
                    "let atten = clamp(1.0 - dist / range, 0.0, 1.0);",
                    "* ndl * atten * atten",
                ],
            ),
            (
                "sky_gradient.wgsl",
                sky,
                &[
                    "return mix(horizon, zenith, pow(clamp(t, 0.0, 1.0), 0.5));",
                    "return mix(horizon, ground, pow(clamp(-t, 0.0, 1.0), 0.5));",
                ],
            ),
            (
                "ibl.wgsl",
                ibl,
                &[
                    "const IRR_SAMPLES: u32 = 64u;",
                    "let phi = 2.0 * PI * xi.x;\n        let cos_t = sqrt(1.0 - xi.y);\n        let sin_t = sqrt(xi.y);",
                    "let local = vec3<f32>(cos(phi) * sin_t, sin(phi) * sin_t, cos_t);",
                ],
            ),
        ];
        for (file, source, statements) in want {
            for statement in statements {
                assert!(
                    source.contains(statement),
                    "{file} no longer contains `{statement}`: edit `light_model.rs` and the WGSL \
                     together (Plan/light-exposure.md decision 2)"
                );
            }
        }
    }
}
