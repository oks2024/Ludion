//! The [`AnimationLayers`] component: override and additive clip layers over a skinned character's
//! base [`Animation`](crate::Animation) (`Plan/animation-layers.md`, Neon Shroud request 0007).

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::{FrameIndex, GameTime, SimTime};
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most layers one [`AnimationLayers`] may hold (`L1670`).
pub const MAX_ANIMATION_LAYERS: usize = 4;

/// How a layer's clip combines with the pose beneath it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LayerMode {
    /// Blends toward the layer's channels by the layer's weight; a joint the layer clip has no
    /// track for is left alone.
    #[default]
    Override,
    /// Adds the layer clip's change from its own first frame on top of the pose beneath it.
    Additive,
}

/// One layer of an [`AnimationLayers`]: a clip, the bone mask it touches, and a weight that ramps.
///
/// The weight is `weight` once `fade` seconds have passed since `changed`; before that it lerps from
/// `from_weight`. Use [`fade_layer`] to change it without a jump.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct AnimationLayer {
    /// The layer's name, unique within the component; [`fade_layer`] and [`play_layer`] select by it.
    pub name: String,
    /// The clip's short name, as bound in the character's `clips:` map (default `""` — nothing
    /// plays). An unknown name is a silent degrade, as for `Animation`.
    #[serde(default)]
    pub clip: String,
    /// The character's `masks:` entry this layer touches (default `""` — the whole skin). An
    /// unknown name is a silent degrade: the layer does nothing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mask: String,
    /// How the layer combines with the pose beneath it (default `override`).
    #[serde(default, skip_serializing_if = "is_override")]
    pub mode: LayerMode,
    /// The target weight in `[0, 1]` (default `0` — the layer is off; `L1671` outside the range).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub weight: f32,
    /// The weight the ramp starts from at `changed` (default `0`; `L1671` outside `[0, 1]`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub from_weight: f32,
    /// The frame the weight ramp began at (default frame `0`).
    #[serde(default = "frame_zero", skip_serializing_if = "is_frame_zero")]
    #[schemars(with = "u64")]
    pub changed: FrameIndex,
    /// The ramp's length in seconds; `0` (default) jumps straight to `weight` (`L1671` when negative
    /// or not finite).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade: f32,
    /// The frame the layer's clip started at; its clock is measured from it (default frame `0`).
    #[serde(default = "frame_zero", skip_serializing_if = "is_frame_zero")]
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// The clip's playback rate multiplier (default `1.0`; `L1671` when negative or not finite).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub speed: f32,
    /// Whether the layer fires animation events, tagged with its name in `ClipEvent.layer` (default
    /// `false`; events fire only while the layer's weight is above `0`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub events: bool,
    /// The two parameters a blend named by `clip` is evaluated at, in the blend's axis order (default
    /// `[0, 0]`; `L1671` when not finite). Unused by a plain clip.
    #[serde(default, skip_serializing_if = "is_zero_params")]
    pub params: [f32; 2],
    /// An additive layer's reference clip: its change is measured from that clip's first frame (default
    /// `""` — the layer's own first frame). An unknown name skips the layer silently.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reference: String,
}

impl Default for AnimationLayer {
    fn default() -> Self {
        AnimationLayer {
            name: String::new(),
            clip: String::new(),
            mask: String::new(),
            mode: LayerMode::Override,
            weight: 0.0,
            from_weight: 0.0,
            changed: FrameIndex::ZERO,
            fade: 0.0,
            started: FrameIndex::ZERO,
            speed: 1.0,
            events: false,
            params: [0.0; 2],
            reference: String::new(),
        }
    }
}

/// Override and additive clip layers over a skinned `kind: character`'s base `Animation`.
///
/// Layers are evaluated in list order over the base pose (at most four, `L1670`). An override layer
/// blends toward its clip by `weight` × the bone mask's weight for each joint; a joint the clip has no
/// track for stays as it was. An additive layer adds its clip's change from its first frame. Each
/// layer has its own clock (`started`, `speed`) and its weight ramps from `from_weight` over `fade`
/// seconds from `changed`; both use the base `Animation.hold` grid.
///
/// **Presentation only.** Extract composes the layers into the skin palette; no system reads the
/// component, and a scene without it poses exactly as before. It is ordinary hashed sim state, so
/// write it only when a choice changes. A `SkinBone`-driven skin refuses it (`L1672`); an assembly or
/// prefab part ignores it. A layer raises no events unless it sets `events: true`. A layer's `clip` may
/// name a blend of the character, evaluated at the layer's `params`.
///
/// ```yaml
/// components:
///   Animation:
///     clip: run
///   AnimationLayers:
///     layers:
///       - name: upper
///         clip: aim
///         mask: upper_body
///         weight: 1.0
/// ```
///
/// ```
/// use ludion_components::{AnimationLayers, AnimationLayer, fade_layer, layer_weight};
/// use ludion_core::time::{SimTime, TickRate};
///
/// let mut layers = AnimationLayers {
///     layers: vec![AnimationLayer { name: "upper".into(), clip: "aim".into(), ..Default::default() }],
/// };
/// let now = SimTime::from_ticks(60, TickRate::DEFAULT);
/// assert!(fade_layer(&mut layers, "upper", 1.0, 0.5, now));
/// // Half the fade later the layer is half on.
/// let later = SimTime::from_ticks(75, TickRate::DEFAULT);
/// assert_eq!(layer_weight(&layers.layers[0], later), 0.5);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct AnimationLayers {
    /// The layers, bottom first (default none; at most four).
    #[serde(default)]
    pub layers: Vec<AnimationLayer>,
}

/// The layer's effective weight at `now`: `weight` once the ramp is over (or has no `fade`), else a
/// lerp from `from_weight`.
#[must_use]
pub fn layer_weight(layer: &AnimationLayer, now: SimTime) -> f32 {
    layer_weight_at(layer, GameTime::from(now))
}

/// [`layer_weight`] at a fractional-tick gameplay instant: what `extract` evaluates.
#[must_use]
pub fn layer_weight_at(layer: &AnimationLayer, now: GameTime) -> f32 {
    if layer.fade <= 0.0 || !layer.fade.is_finite() {
        return layer.weight;
    }
    let elapsed = now.ticks_since(layer.changed).max(0.0) * now.rate().seconds_per_tick();
    let t = (elapsed / f64::from(layer.fade)).clamp(0.0, 1.0) as f32;
    layer.from_weight + (layer.weight - layer.from_weight) * t
}

/// Ramps the layer `name` to `weight` over `seconds`, starting from its current weight at `now` (use
/// the game time). Returns `false` when there is no such layer; a layer already at that target is
/// left untouched (so a per-frame call does not churn the world hash).
///
/// ```
/// use ludion_components::{AnimationLayers, AnimationLayer, fade_layer};
/// use ludion_core::time::{SimTime, TickRate};
///
/// let mut layers = AnimationLayers {
///     layers: vec![AnimationLayer { name: "aim".into(), ..Default::default() }],
/// };
/// let now = SimTime::from_ticks(10, TickRate::DEFAULT);
/// assert!(fade_layer(&mut layers, "aim", 1.0, 0.25, now));
/// assert!(!fade_layer(&mut layers, "missing", 1.0, 0.25, now));
/// assert_eq!(layers.layers[0].weight, 1.0);
/// ```
pub fn fade_layer(
    layers: &mut AnimationLayers,
    name: &str,
    weight: f32,
    seconds: f32,
    now: SimTime,
) -> bool {
    let Some(layer) = layers.layers.iter_mut().find(|layer| layer.name == name) else {
        return false;
    };
    #[allow(clippy::float_cmp)]
    if layer.weight == weight {
        return true;
    }
    layer.from_weight = layer_weight(layer, now);
    layer.weight = weight;
    layer.fade = seconds;
    layer.changed = FrameIndex(now.ticks());
    true
}

/// Switches the layer `name` to `clip`, restarting its clock at `now`. Returns `false` when there is
/// no such layer; the same clip is a no-op.
///
/// ```
/// use ludion_components::{AnimationLayers, AnimationLayer, play_layer};
/// use ludion_core::time::{SimTime, TickRate};
///
/// let mut layers = AnimationLayers {
///     layers: vec![AnimationLayer { name: "aim".into(), ..Default::default() }],
/// };
/// let now = SimTime::from_ticks(30, TickRate::DEFAULT);
/// assert!(play_layer(&mut layers, "aim", "rifle", now));
/// assert_eq!(layers.layers[0].started.as_u64(), 30);
/// ```
pub fn play_layer(layers: &mut AnimationLayers, name: &str, clip: &str, now: SimTime) -> bool {
    let Some(layer) = layers.layers.iter_mut().find(|layer| layer.name == name) else {
        return false;
    };
    if layer.clip != clip {
        layer.clip = clip.to_string();
        layer.started = FrameIndex(now.ticks());
    }
    true
}

/// Sets the layer `name`'s blend parameters, writing only on a change. Returns `false` when there is
/// no such layer.
///
/// ```
/// use ludion_components::{AnimationLayers, AnimationLayer, set_layer_params};
///
/// let mut layers = AnimationLayers {
///     layers: vec![AnimationLayer { name: "lean".into(), clip: "tilt".into(), ..Default::default() }],
/// };
/// assert!(set_layer_params(&mut layers, "lean", [0.5, 0.0]));
/// assert_eq!(layers.layers[0].params, [0.5, 0.0]);
/// assert!(!set_layer_params(&mut layers, "missing", [1.0, 0.0]));
/// ```
pub fn set_layer_params(layers: &mut AnimationLayers, name: &str, params: [f32; 2]) -> bool {
    let Some(layer) = layers.layers.iter_mut().find(|layer| layer.name == name) else {
        return false;
    };
    if layer.params != params {
        layer.params = params;
    }
    true
}

impl SemanticValidate for AnimationLayers {
    /// More than four layers (`L1670`, `/layers`); a duplicate or empty name, a weight outside
    /// `[0, 1]`, a negative or non-finite `fade` or `speed`, a non-finite `params` entry (`L1671`,
    /// `/layers/<i>/<field>`).
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let Some(layers) = value.get("layers").and_then(Value::as_array) else {
            return out;
        };
        if layers.len() > MAX_ANIMATION_LAYERS {
            out.push(
                Diagnostic::new(
                    code::ANIMATION_LAYERS_TOO_MANY,
                    format!(
                        "AnimationLayers holds {} layers; at most {MAX_ANIMATION_LAYERS} are allowed",
                        layers.len()
                    ),
                )
                .with_path("/layers")
                .with_help("drop a layer, or switch one layer's clip with play_layer"),
            );
        }
        let mut seen: Vec<&str> = Vec::new();
        for (index, layer) in layers.iter().enumerate() {
            let name = layer.get("name").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() {
                out.push(invalid(index, "name", "the layer has no name"));
            } else if seen.contains(&name) {
                out.push(invalid(
                    index,
                    "name",
                    &format!("the layer name `{name}` repeats an earlier layer"),
                ));
            }
            seen.push(name);
            for field in ["weight", "from_weight"] {
                if let Some(v) = layer.get(field).and_then(Value::as_f64)
                    && !(0.0..=1.0).contains(&v)
                {
                    out.push(invalid(
                        index,
                        field,
                        &format!("`{field}` {v} is outside [0, 1]"),
                    ));
                }
            }
            for field in ["fade", "speed"] {
                if let Some(v) = layer.get(field).and_then(Value::as_f64)
                    && (!v.is_finite() || v < 0.0)
                {
                    out.push(invalid(
                        index,
                        field,
                        &format!("`{field}` {v} must be finite and not negative"),
                    ));
                }
            }
            if let Some(params) = layer.get("params").and_then(Value::as_array) {
                for (axis, v) in params.iter().enumerate() {
                    if !v.as_f64().is_some_and(f64::is_finite) {
                        out.push(invalid(
                            index,
                            &format!("params/{axis}"),
                            &format!("`params` entry {axis} ({v}) must be a finite number"),
                        ));
                    }
                }
            }
        }
        out
    }
}

fn invalid(index: usize, field: &str, message: &str) -> Diagnostic {
    Diagnostic::new(
        code::ANIMATION_LAYER_INVALID,
        format!("AnimationLayers layer {index}: {message}"),
    )
    .with_path(format!("/layers/{index}/{field}"))
    .with_help(
        "give each layer a unique non-empty name, weights in [0, 1], and finite non-negative \
         fade and speed",
    )
}

fn is_override(mode: &LayerMode) -> bool {
    *mode == LayerMode::Override
}

#[allow(clippy::float_cmp)]
fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

#[allow(clippy::float_cmp)]
fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero_params(value: &[f32; 2]) -> bool {
    is_zero(&value[0]) && is_zero(&value[1])
}

#[allow(clippy::float_cmp)]
fn is_one(value: &f32) -> bool {
    *value == 1.0
}

fn one() -> f32 {
    1.0
}

fn frame_zero() -> FrameIndex {
    FrameIndex::ZERO
}

fn is_frame_zero(frame: &FrameIndex) -> bool {
    frame.as_u64() == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn layer(name: &str) -> AnimationLayer {
        AnimationLayer {
            name: name.into(),
            clip: "aim".into(),
            ..AnimationLayer::default()
        }
    }

    #[test]
    fn default_layer_serializes_to_name_and_clip() {
        let value = serde_json::to_value(layer("upper")).unwrap();
        assert_eq!(value, json!({ "name": "upper", "clip": "aim" }));
        let back: AnimationLayer = serde_json::from_value(value).unwrap();
        assert_eq!(back, layer("upper"));
        // `speed` defaults to 1 when absent, not 0.
        assert_eq!(back.speed, 1.0);
    }

    #[test]
    fn weight_ramps_and_lands_on_target() {
        let mut layers = AnimationLayers {
            layers: vec![layer("a")],
        };
        let now = SimTime::from_ticks(60, ludion_core::time::TickRate::DEFAULT);
        assert!(fade_layer(&mut layers, "a", 1.0, 1.0, now));
        let at = |ticks| {
            layer_weight(
                &layers.layers[0],
                SimTime::from_ticks(ticks, ludion_core::time::TickRate::DEFAULT),
            )
        };
        assert_eq!(at(60), 0.0);
        assert!((at(90) - 0.5).abs() < 1e-6);
        assert_eq!(at(120), 1.0);
        assert_eq!(at(10_000), 1.0);
        // Before `changed` the saturating subtraction reads the start of the ramp.
        assert_eq!(at(0), 0.0);
    }

    #[test]
    fn fade_to_the_current_target_does_not_write() {
        let mut layers = AnimationLayers {
            layers: vec![layer("a")],
        };
        let now = SimTime::from_ticks(5, ludion_core::time::TickRate::DEFAULT);
        let before = layers.clone();
        assert!(fade_layer(&mut layers, "a", 0.0, 1.0, now));
        assert_eq!(layers, before);
    }

    #[test]
    fn interrupted_fade_starts_from_the_current_weight() {
        let mut layers = AnimationLayers {
            layers: vec![layer("a")],
        };
        let rate = ludion_core::time::TickRate::DEFAULT;
        fade_layer(&mut layers, "a", 1.0, 1.0, SimTime::from_ticks(0, rate));
        fade_layer(&mut layers, "a", 0.0, 1.0, SimTime::from_ticks(30, rate));
        assert!((layers.layers[0].from_weight - 0.5).abs() < 1e-6);
    }

    #[test]
    fn play_layer_restamps_only_on_a_new_clip() {
        let mut layers = AnimationLayers {
            layers: vec![layer("a")],
        };
        let rate = ludion_core::time::TickRate::DEFAULT;
        assert!(play_layer(
            &mut layers,
            "a",
            "aim",
            SimTime::from_ticks(9, rate)
        ));
        assert_eq!(layers.layers[0].started.as_u64(), 0);
        assert!(play_layer(
            &mut layers,
            "a",
            "jump",
            SimTime::from_ticks(9, rate)
        ));
        assert_eq!(layers.layers[0].started.as_u64(), 9);
        assert!(!play_layer(
            &mut layers,
            "b",
            "jump",
            SimTime::from_ticks(9, rate)
        ));
    }

    #[test]
    fn new_fields_stay_out_of_the_text_at_their_defaults() {
        let mut full = layer("a");
        full.events = true;
        full.params = [0.5, 0.0];
        full.reference = "idle".into();
        let value = serde_json::to_value(&full).unwrap();
        assert_eq!(
            value,
            json!({ "name": "a", "clip": "aim", "events": true, "params": [0.5, 0.0], "reference": "idle" })
        );
        let back: AnimationLayer = serde_json::from_value(value).unwrap();
        assert_eq!(back, full);
    }

    #[test]
    fn set_layer_params_writes_once() {
        let mut layers = AnimationLayers {
            layers: vec![layer("a")],
        };
        assert!(set_layer_params(&mut layers, "a", [1.0, 2.0]));
        let before = layers.clone();
        assert!(set_layer_params(&mut layers, "a", [1.0, 2.0]));
        assert_eq!(layers, before);
        assert_eq!(layers.layers[0].params, [1.0, 2.0]);
        assert!(!set_layer_params(&mut layers, "b", [0.0; 2]));
    }

    #[test]
    fn a_non_finite_param_is_l1671() {
        // JSON has no NaN: a non-finite `f32` serializes to null, and a string is not a number.
        let found = codes(json!({ "layers": [
            { "name": "a", "params": [0.5, null] },
            { "name": "b", "params": ["x", 1.0] },
            { "name": "c", "params": [0.5, 1.0] },
        ] }));
        assert_eq!(
            found,
            [
                ("L1671".to_string(), "/layers/0/params/1".to_string()),
                ("L1671".to_string(), "/layers/1/params/0".to_string()),
            ]
        );
    }

    fn codes(value: Value) -> Vec<(String, String)> {
        AnimationLayers::validate_semantics(&value)
            .into_iter()
            .map(|d| (d.code.as_str().to_string(), d.path.unwrap_or_default()))
            .collect()
    }

    #[test]
    fn a_valid_component_has_no_findings() {
        assert!(codes(json!({ "layers": [{ "name": "a", "weight": 1.0 }] })).is_empty());
    }

    #[test]
    fn five_layers_is_l1670() {
        let layers: Vec<Value> = (0..5).map(|i| json!({ "name": format!("l{i}") })).collect();
        assert_eq!(
            codes(json!({ "layers": layers })),
            [("L1670".to_string(), "/layers".to_string())]
        );
    }

    #[test]
    fn bad_layer_fields_are_l1671_with_pointers() {
        let found = codes(json!({ "layers": [
            { "name": "a", "weight": 1.5 },
            { "name": "a", "fade": -1.0 },
            { "name": "", "speed": -2.0, "from_weight": 2.0 },
        ] }));
        for pointer in [
            "/layers/0/weight",
            "/layers/1/name",
            "/layers/1/fade",
            "/layers/2/name",
            "/layers/2/speed",
            "/layers/2/from_weight",
        ] {
            assert!(
                found.contains(&("L1671".to_string(), pointer.to_string())),
                "{pointer} in {found:?}"
            );
        }
    }
}
