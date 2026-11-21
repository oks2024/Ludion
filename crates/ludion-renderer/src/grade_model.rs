//! The CPU twin of `composite.wgsl`'s display-referred colour grade (`grade()`: white balance,
//! lift/gamma/gain, contrast, saturation) and of `snippets/srgb.wgsl`'s transfer pair. `ludion lut
//! bake` evaluates the scene's analytic grade through this twin to write a `.cube`;
//! `grade_model_matches_the_wgsl` pins the statements to the shader (`Plan/post-film-and-screen-
//! effects.md` decision 14). Edit this file and the WGSL together.
//!
//! Nothing here is sim-side: it runs in a CLI verb, never in a tick, so `f32::powf` is fine and the
//! determinism denylist does not apply.

use crate::PostData;

/// Rec.709 luma weights, `composite.wgsl`'s `LUMA_709`.
const LUMA_709: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// The floor `gamma` is clamped to before the reciprocal, `composite.wgsl`'s `MIN_GAMMA`.
const MIN_GAMMA: f32 = 0.0009765625;

fn luma(c: [f32; 3]) -> f32 {
    c[0] * LUMA_709[0] + c[1] * LUMA_709[1] + c[2] * LUMA_709[2]
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

/// White balance as a diagonal channel scale normalized to keep a grey's Rec.709 luma.
fn white_balance(color: [f32; 3], temperature: f32, tint: f32) -> [f32; 3] {
    let amber = [1.0 + temperature, 1.0, 1.0 - temperature];
    let green = [1.0 - tint, 1.0 + tint, 1.0 - tint];
    let gain = [
        amber[0] * green[0],
        amber[1] * green[1],
        amber[2] * green[2],
    ];
    let norm = luma(gain);
    [
        color[0] * gain[0] / norm,
        color[1] * gain[1] / norm,
        color[2] * gain[2] / norm,
    ]
}

/// The ASC CDL shape `pow(max(color * gain + lift, 0), 1 / gamma)`, skipping the power at gamma 1.
fn lift_gamma_gain(color: [f32; 3], lift: [f32; 3], gamma: [f32; 3], gain: [f32; 3]) -> [f32; 3] {
    let leveled: [f32; 3] = std::array::from_fn(|i| (color[i] * gain[i] + lift[i]).max(0.0));
    if gamma == [1.0; 3] {
        return leveled;
    }
    std::array::from_fn(|i| leveled[i].powf(1.0 / gamma[i].max(MIN_GAMMA)))
}

/// The display-referred grade of `color` (linear, post-tonemap) under `post`'s grade fields, in the
/// shader's pinned order with its skips: an identity grade returns its input untouched.
///
/// # Example
/// ```
/// use ludion_renderer::{PostData, grade_model::grade};
/// let grey = [0.25, 0.25, 0.25];
/// assert_eq!(grade(grey, &PostData::default()), grey);
/// ```
#[must_use]
pub fn grade(color: [f32; 3], post: &PostData) -> [f32; 3] {
    let mut color = color;
    if post.temperature != 0.0 || post.tint != 0.0 {
        color = white_balance(color, post.temperature, post.tint);
    }
    if post.lift != [0.0; 3] || post.gamma != [1.0; 3] || post.gain != [1.0; 3] {
        color = lift_gamma_gain(color, post.lift, post.gamma, post.gain);
    }
    if post.contrast != 1.0 {
        color = color.map(|c| (c - 0.5) * post.contrast + 0.5);
    }
    if post.saturation != 1.0 {
        let l = luma(color);
        color = color.map(|c| mix(l, c, post.saturation));
    }
    color
}

/// Whether `post`'s grade fields skip every step, so [`grade`] returns its input untouched.
#[must_use]
pub fn is_identity(post: &PostData) -> bool {
    post.temperature == 0.0
        && post.tint == 0.0
        && post.lift == [0.0; 3]
        && post.gamma == [1.0; 3]
        && post.gain == [1.0; 3]
        && post.contrast == 1.0
        && post.saturation == 1.0
}

/// Linear to sRGB-encoded, `srgb.wgsl`'s `linear_to_srgb`: the linear segment below `0.0031308`
/// (and for negatives), the power segment above.
#[must_use]
pub fn linear_to_srgb(c: f32) -> f32 {
    if c < 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB-encoded to linear, `srgb.wgsl`'s `srgb_to_linear`.
#[must_use]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c < 0.04045 {
        c / 12.92
    } else {
        ((c.max(0.0) + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_returns_the_input_bit_exactly() {
        for c in [
            [0.0, 0.0, 0.0],
            [0.1, 0.5, 0.9],
            [1.0, 1.0, 1.0],
            [1.7, -0.2, 0.3],
        ] {
            assert_eq!(grade(c, &PostData::default()), c);
        }
    }

    #[test]
    fn zero_saturation_gives_equal_channels() {
        let post = PostData {
            saturation: 0.0,
            ..PostData::default()
        };
        let out = grade([0.8, 0.3, 0.1], &post);
        assert_eq!(out[0], out[1]);
        assert_eq!(out[1], out[2]);
    }

    #[test]
    fn temperature_keeps_a_greys_luma() {
        let post = PostData {
            temperature: 0.4,
            tint: -0.2,
            ..PostData::default()
        };
        let out = grade([0.5, 0.5, 0.5], &post);
        assert!((luma(out) - 0.5).abs() < 1e-5, "{out:?}");
        assert!(out[0] > out[2], "warm: red above blue, {out:?}");
    }

    #[test]
    fn gamma_above_one_brightens_midtones() {
        let post = PostData {
            gamma: [2.0; 3],
            ..PostData::default()
        };
        let out = grade([0.25; 3], &post);
        assert!((out[0] - 0.5).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn srgb_pair_round_trips() {
        for v in [0.0, 0.002, 0.04, 0.2, 0.5, 1.0] {
            assert!((srgb_to_linear(linear_to_srgb(v)) - v).abs() < 1e-5);
        }
    }

    #[test]
    fn grade_model_matches_the_wgsl() {
        let composite = include_str!("shaders/composite.wgsl");
        let srgb = include_str!("shaders/snippets/srgb.wgsl");
        let want: [(&str, &str, &[&str]); 2] = [
            (
                "composite.wgsl",
                composite,
                &[
                    "const LUMA_709: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);",
                    "const MIN_GAMMA: f32 = 0.0009765625;",
                    "let amber = vec3<f32>(1.0 + temperature, 1.0, 1.0 - temperature);",
                    "let green = vec3<f32>(1.0 - tint, 1.0 + tint, 1.0 - tint);",
                    "return color * gain / dot(gain, LUMA_709);",
                    "let leveled = max(color * gain + lift, vec3<f32>(0.0));",
                    "if (all(gamma == vec3<f32>(1.0))) {",
                    "return pow(leveled, vec3<f32>(1.0) / max(gamma, vec3<f32>(MIN_GAMMA)));",
                    "return (color - vec3<f32>(0.5)) * contrast + vec3<f32>(0.5);",
                    "return mix(vec3<f32>(dot(color, LUMA_709)), color, saturation);",
                    "if (composite.temperature != 0.0 || composite.tint != 0.0) {",
                    "if (any(composite.lift != vec3<f32>(0.0))\n        || any(composite.gamma != vec3<f32>(1.0))\n        || any(composite.gain != vec3<f32>(1.0))) {",
                    "if (composite.contrast != 1.0) {",
                    "if (composite.saturation != 1.0) {",
                ],
            ),
            (
                "srgb.wgsl",
                srgb,
                &[
                    "let low = c * 12.92;",
                    "let high = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;",
                    "return select(high, low, c < vec3<f32>(0.0031308));",
                    "let low = c / 12.92;",
                    "let high = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));",
                    "return select(high, low, c < vec3<f32>(0.04045));",
                ],
            ),
        ];
        for (file, source, statements) in want {
            for statement in statements {
                assert!(
                    source.contains(statement),
                    "{file} no longer contains `{statement}`: edit `grade_model.rs` and the WGSL \
                     together (Plan/post-film-and-screen-effects.md decision 14)"
                );
            }
        }
    }
}
