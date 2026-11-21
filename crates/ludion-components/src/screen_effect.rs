//! The [`ScreenEffect`] component: a display-look filter (CRT, analog video, scan bar) over the frame
//! a rectangle of it, or a `RenderView`'s texture, drawn in UI order. Render-only; each stage is optional and an unset stage
//! takes the preset's value.

use crate::Anchor;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where a [`ScreenEffect`] draws.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ScreenEffectTarget {
    /// The whole output frame.
    #[default]
    Fill,
    /// A rectangle laid out like a `UiRect`, in the UI's canvas units.
    Region {
        /// Which canvas point the rectangle hangs from.
        anchor: Anchor,
        /// Canvas-unit shift from the anchor `[x, y]`.
        offset: [f32; 2],
        /// Canvas-unit size `[width, height]`; both must be positive.
        size: [f32; 2],
    },
    /// A RenderView's texture, named by RenderView.name.
    View(String),
}

/// A named starting point for the stages.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenEffectPreset {
    /// Every stage off.
    #[default]
    None,
    /// A green phosphor CRT: scanlines, an aperture mask, curvature, glow and light noise.
    Crt,
    /// Analog video: chroma bleed, tracking, jitter, noise and an amber tint.
    AnalogVideo,
    /// Fine scanlines, a slow sweep bar and a slight posterize.
    Scan,
}

/// The shadow mask pattern of a CRT.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenMaskKind {
    /// No mask.
    #[default]
    None,
    /// Vertical RGB stripes.
    Aperture,
    /// Staggered RGB slots.
    Slot,
    /// Triangular RGB dots.
    Shadow,
}

/// Every stage resolved to a concrete value: a preset with the author's overrides applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenEffectStages {
    /// Scanline darkening, in `[0, 1]`.
    pub scanlines: f32,
    /// Scanlines across the effect's height, in `[1, 4320]`.
    pub scanline_count: u32,
    /// The shadow mask pattern.
    pub mask: ScreenMaskKind,
    /// The shadow mask's strength, in `[0, 1]`.
    pub mask_strength: f32,
    /// Barrel curvature, in `[0, 0.3]`.
    pub curvature: f32,
    /// Linear mono tint; `[0, 0, 0]` keeps the colour.
    pub phosphor: [f32; 3],
    /// Bloom of the bright parts, in `[0, 1]`.
    pub glow: f32,
    /// Horizontal chroma smear, in `[0, 1]`.
    pub chroma_bleed: f32,
    /// Animated noise, in `[0, 1]`.
    pub noise: f32,
    /// Tracking error bands, in `[0, 1]`.
    pub tracking: f32,
    /// Vertical roll in regions per second, in `[-4, 4]`.
    pub roll: f32,
    /// Horizontal line jitter, in `[0, 1]`.
    pub jitter: f32,
    /// Brightness flicker, in `[0, 1]`.
    pub flicker: f32,
    /// Alternate the scanline phase each frame.
    pub interlace: bool,
    /// Colour levels per channel; 0 or 1 is off.
    pub posterize: u32,
    /// The sweep bar's strength, in `[0, 1]`.
    pub sweep: f32,
    /// Seconds per sweep, in `[0.1, 600]`.
    pub sweep_period: f32,
}

impl ScreenEffectStages {
    /// True when no stage changes a pixel: the effect would draw the region back unchanged.
    pub fn is_neutral(&self) -> bool {
        self.scanlines <= 0.0
            && (self.mask == ScreenMaskKind::None || self.mask_strength <= 0.0)
            && self.curvature == 0.0
            && self.phosphor == [0.0; 3]
            && self.glow <= 0.0
            && self.chroma_bleed <= 0.0
            && self.noise <= 0.0
            && self.tracking == 0.0
            && self.roll == 0.0
            && self.jitter == 0.0
            && self.flicker <= 0.0
            && !self.interlace
            && self.posterize < 2
            && self.sweep <= 0.0
    }
}

impl ScreenEffectPreset {
    /// The preset's stages: every stage off for `None`, the named look otherwise.
    pub fn stages(self) -> ScreenEffectStages {
        let base = ScreenEffectStages {
            scanlines: 0.0,
            scanline_count: 240,
            mask: ScreenMaskKind::None,
            mask_strength: 0.0,
            curvature: 0.0,
            phosphor: [0.0; 3],
            glow: 0.0,
            chroma_bleed: 0.0,
            noise: 0.0,
            tracking: 0.0,
            roll: 0.0,
            jitter: 0.0,
            flicker: 0.0,
            interlace: false,
            posterize: 0,
            sweep: 0.0,
            sweep_period: 2.0,
        };
        match self {
            Self::None => base,
            Self::Crt => ScreenEffectStages {
                scanlines: 0.35,
                mask: ScreenMaskKind::Aperture,
                mask_strength: 0.2,
                curvature: 0.06,
                phosphor: [0.25, 1.0, 0.45],
                glow: 0.3,
                noise: 0.04,
                flicker: 0.02,
                ..base
            },
            Self::AnalogVideo => ScreenEffectStages {
                scanlines: 0.1,
                chroma_bleed: 0.4,
                tracking: 0.15,
                jitter: 0.2,
                noise: 0.08,
                flicker: 0.03,
                phosphor: [1.0, 0.72, 0.4],
                interlace: true,
                ..base
            },
            Self::Scan => ScreenEffectStages {
                scanlines: 0.2,
                scanline_count: 540,
                sweep: 0.25,
                sweep_period: 4.0,
                posterize: 24,
                noise: 0.02,
                ..base
            },
        }
    }
}

/// A screen-look filter over the frame (`target: fill`) or a rectangle of it (`target: region`).
/// Drawn in UI order: elements with a lower `order` draw under it, the rest over it. Each stage is
/// optional; an unset stage takes the `preset`'s value, so the scene text carries only what was
/// overridden. An effect with every stage off is not applied. At most four are applied per frame
/// (`L2392`). Animated stages follow game time and `seed`.
///
/// ```
/// use ludion_components::{ScreenEffect, ScreenEffectPreset};
///
/// let fx: ScreenEffect = serde_json::from_value(serde_json::json!({
///     "preset": "crt",
///     "scanlines": 0.5,
/// }))
/// .unwrap();
/// assert_eq!(fx.preset, ScreenEffectPreset::Crt);
/// assert_eq!(fx.stages().scanlines, 0.5);
/// assert_eq!(fx.stages().curvature, 0.06);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct ScreenEffect {
    /// Where the effect draws (default the whole frame).
    #[serde(default)]
    pub target: ScreenEffectTarget,
    /// UI order: elements with a lower `order` draw under the effect (default 0).
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub order: i32,
    /// The starting look (default none).
    #[serde(default, skip_serializing_if = "is_no_preset")]
    pub preset: ScreenEffectPreset,
    /// Scanline darkening, in `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub scanlines: Option<f32>,
    /// Scanlines across the effect's height, in `[1, 4320]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 4320))]
    pub scanline_count: Option<u32>,
    /// The shadow mask pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<ScreenMaskKind>,
    /// The shadow mask's strength, in `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub mask_strength: Option<f32>,
    /// Barrel curvature, in `[0, 0.3]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 0.3))]
    pub curvature: Option<f32>,
    /// Linear mono tint `[r, g, b]`, each at least 0; `[0, 0, 0]` keeps the colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phosphor: Option<[f32; 3]>,
    /// Bloom of the bright parts, in `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub glow: Option<f32>,
    /// Horizontal chroma smear, in `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub chroma_bleed: Option<f32>,
    /// Animated noise, in `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub noise: Option<f32>,
    /// Tracking error bands, in `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub tracking: Option<f32>,
    /// Vertical roll in regions per second, in `[-4, 4]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = -4.0, max = 4.0))]
    pub roll: Option<f32>,
    /// Horizontal line jitter, in `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub jitter: Option<f32>,
    /// Brightness flicker, in `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub flicker: Option<f32>,
    /// Alternate the scanline phase each frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interlace: Option<bool>,
    /// Colour levels per channel, in `[0, 256]`; 0 or 1 is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0, max = 256))]
    pub posterize: Option<u32>,
    /// The sweep bar's strength, in `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub sweep: Option<f32>,
    /// Seconds per sweep, in `[0.1, 600]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0.1, max = 600.0))]
    pub sweep_period: Option<f32>,
    /// Varies the noise and jitter hash (default 0).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub seed: u32,
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}
fn is_no_preset(v: &ScreenEffectPreset) -> bool {
    *v == ScreenEffectPreset::None
}

impl ScreenEffect {
    /// The preset's stages with every `Some` override applied.
    pub fn stages(&self) -> ScreenEffectStages {
        let mut s = self.preset.stages();
        macro_rules! over {
            ($($f:ident),*) => { $( if let Some(v) = self.$f { s.$f = v; } )* };
        }
        over!(
            scanlines,
            scanline_count,
            mask,
            mask_strength,
            curvature,
            phosphor,
            glow,
            chroma_bleed,
            noise,
            tracking,
            roll,
            jitter,
            flicker,
            interlace,
            posterize,
            sweep,
            sweep_period
        );
        s
    }
}

/// Every unusable part of `target` as `(JSON Pointer, what is wrong)`; empty when valid. Shared by
/// the component validator and the engine's extract.
pub fn screen_effect_target_problems(target: &ScreenEffectTarget) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let ScreenEffectTarget::Region { offset, size, .. } = target {
        for (i, v) in offset.iter().enumerate() {
            if !v.is_finite() {
                out.push((
                    format!("/target/region/offset/{i}"),
                    format!("offset element {i} is {v}; it must be finite"),
                ));
            }
        }
        for (i, v) in size.iter().enumerate() {
            if !(v.is_finite() && *v > 0.0) {
                out.push((
                    format!("/target/region/size/{i}"),
                    format!("size element {i} is {v}; it must be a positive number"),
                ));
            }
        }
    }
    if let ScreenEffectTarget::View(name) = target
        && name.is_empty()
    {
        out.push((
            "/target/view".into(),
            "name is empty; it must name a RenderView".into(),
        ));
    }
    out
}

impl SemanticValidate for ScreenEffect {
    /// `L1761` for an unusable region, `L1130` for a negative `phosphor` element; a wrong shape or an
    /// out-of-range scalar is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let Ok(fx) = serde_json::from_value::<ScreenEffect>(value.clone()) else {
            return Vec::new();
        };
        let mut out: Vec<Diagnostic> = screen_effect_target_problems(&fx.target)
            .into_iter()
            .map(|(path, what)| {
                Diagnostic::new(
                    code::SCREEN_EFFECT_INVALID_TARGET,
                    format!("`ScreenEffect` target {what}"),
                )
                .with_path(path)
                .with_help("see `ludion explain L1761` and `ludion docs screen-effects`")
            })
            .collect();
        if let Some(phosphor) = fx.phosphor {
            for (i, v) in phosphor.iter().enumerate() {
                if *v < 0.0 {
                    out.push(
                        Diagnostic::new(
                            code::POST_PROCESS_INVALID_TRIPLE,
                            format!(
                                "`ScreenEffect` phosphor element {i} is {v}; a tint is never negative"
                            ),
                        )
                        .with_path(format!("/phosphor/{i}"))
                        .with_help("see `ludion explain L1130`"),
                    );
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paths(value: Value) -> Vec<(String, String)> {
        ScreenEffect::validate_semantics(&value)
            .into_iter()
            .map(|d| (d.code.to_string(), d.path.unwrap()))
            .collect()
    }

    #[test]
    fn empty_object_is_a_neutral_fill() {
        let fx: ScreenEffect = serde_json::from_value(json!({})).unwrap();
        assert_eq!(fx, ScreenEffect::default());
        assert_eq!(fx.target, ScreenEffectTarget::Fill);
        assert_eq!(fx.preset, ScreenEffectPreset::None);
        assert!(fx.stages().is_neutral());
        assert!(paths(json!({})).is_empty());
    }

    #[test]
    fn each_preset_resolves_to_its_table() {
        let none = ScreenEffectPreset::None.stages();
        assert_eq!((none.scanline_count, none.sweep_period), (240, 2.0));
        let crt = ScreenEffectPreset::Crt.stages();
        assert_eq!(
            (crt.scanlines, crt.mask, crt.mask_strength, crt.curvature),
            (0.35, ScreenMaskKind::Aperture, 0.2, 0.06)
        );
        assert_eq!(
            (crt.phosphor, crt.glow, crt.noise, crt.flicker),
            ([0.25, 1.0, 0.45], 0.3, 0.04, 0.02)
        );
        let analog = ScreenEffectPreset::AnalogVideo.stages();
        assert_eq!(
            (analog.scanlines, analog.chroma_bleed, analog.tracking),
            (0.1, 0.4, 0.15)
        );
        assert_eq!(
            (
                analog.jitter,
                analog.noise,
                analog.flicker,
                analog.interlace
            ),
            (0.2, 0.08, 0.03, true)
        );
        assert_eq!(analog.phosphor, [1.0, 0.72, 0.4]);
        let scan = ScreenEffectPreset::Scan.stages();
        assert_eq!(
            (scan.scanlines, scan.scanline_count, scan.sweep),
            (0.2, 540, 0.25)
        );
        assert_eq!(
            (scan.sweep_period, scan.posterize, scan.noise),
            (4.0, 24, 0.02)
        );
        for p in [
            ScreenEffectPreset::Crt,
            ScreenEffectPreset::AnalogVideo,
            ScreenEffectPreset::Scan,
        ] {
            assert!(!p.stages().is_neutral());
        }
    }

    #[test]
    fn an_override_replaces_exactly_one_stage() {
        let fx = ScreenEffect {
            preset: ScreenEffectPreset::Crt,
            glow: Some(0.9),
            ..ScreenEffect::default()
        };
        let want = ScreenEffectStages {
            glow: 0.9,
            ..ScreenEffectPreset::Crt.stages()
        };
        assert_eq!(fx.stages(), want);
    }

    #[test]
    fn an_override_on_preset_none_can_enable_a_stage() {
        let fx = ScreenEffect {
            noise: Some(0.1),
            ..ScreenEffect::default()
        };
        assert!(!fx.stages().is_neutral());
        let masked = ScreenEffect {
            mask: Some(ScreenMaskKind::Slot),
            ..ScreenEffect::default()
        };
        assert!(masked.stages().is_neutral(), "a mask at strength 0 is off");
    }

    #[test]
    fn serialization_skips_every_unset_key() {
        let v = serde_json::to_value(ScreenEffect::default()).unwrap();
        assert_eq!(v, json!({ "target": "fill" }));
    }

    #[test]
    fn the_requests_example_round_trips() {
        let v = json!({
            "target": { "region": { "anchor": "top_right", "offset": [-32.0, 32.0], "size": [320.0, 320.0] } },
            "order": 3,
            "preset": "crt",
            "scanlines": 0.5,
            "mask": "slot",
            "phosphor": [0.0, 0.0, 0.0],
            "interlace": true,
            "seed": 9,
        });
        let fx: ScreenEffect = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(
            fx.target,
            ScreenEffectTarget::Region {
                anchor: Anchor::TopRight,
                offset: [-32.0, 32.0],
                size: [320.0, 320.0]
            }
        );
        assert_eq!(serde_json::to_value(&fx).unwrap(), v);
        assert!(paths(v).is_empty());
    }

    #[test]
    fn region_problems_name_their_pointers() {
        let region = |offset: [f64; 2], size: [f64; 2]| json!({ "target": { "region": { "anchor": "center", "offset": offset, "size": size } } });
        assert_eq!(
            paths(region([0.0, 0.0], [0.0, 100.0])),
            [("L1761".into(), "/target/region/size/0".into())]
        );
        assert_eq!(
            paths(region([0.0, 0.0], [10.0, -1.0])),
            [("L1761".into(), "/target/region/size/1".into())]
        );
        assert!(paths(region([4.0, -4.0], [10.0, 10.0])).is_empty());
        let bad = ScreenEffectTarget::Region {
            anchor: Anchor::Center,
            offset: [f32::NAN, 0.0],
            size: [f32::INFINITY, 5.0],
        };
        let p: Vec<String> = screen_effect_target_problems(&bad)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(p, ["/target/region/offset/0", "/target/region/size/0"]);
    }

    #[test]
    fn a_view_target_round_trips_and_an_empty_name_is_l1761() {
        let v = json!({ "target": { "view": "radar" }, "preset": "crt" });
        let fx: ScreenEffect = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(fx.target, ScreenEffectTarget::View("radar".into()));
        assert_eq!(serde_json::to_value(&fx).unwrap(), v);
        assert!(paths(v).is_empty());
        assert_eq!(
            paths(json!({ "target": { "view": "" } })),
            [("L1761".into(), "/target/view".into())]
        );
    }

    #[test]
    fn a_negative_phosphor_element_is_l1130() {
        assert_eq!(
            paths(json!({ "phosphor": [0.5, -0.1, 0.0] })),
            [("L1130".into(), "/phosphor/1".into())]
        );
        assert!(paths(json!({ "phosphor": [0.0, 0.0, 0.0] })).is_empty());
    }
}
