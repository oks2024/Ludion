//! Range rules for [`CameraRig`] (`L1304`): the document validator and the runtime check.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::SemanticValidate;
use serde_json::Value;

use crate::rig::{CameraRig, RigMode};

const NOT_NEGATIVE: &str = "must be a finite number, not negative";
const POSITIVE: &str = "must be a finite number above 0";
const FINITE: &str = "must be a finite number";

/// Every field of `rig` that breaks a range rule, as `(JSON Pointer, rule)` in declaration order.
fn violations(rig: &CameraRig) -> Vec<(&'static str, &'static str)> {
    let mut found = Vec::new();
    let mut non_negative = |pointer, value: f32| {
        if !value.is_finite() || value < 0.0 {
            found.push((pointer, NOT_NEGATIVE));
        }
    };
    let config = &rig.rig;
    non_negative(
        "/rig/springs/position_half_life",
        config.springs.position_half_life,
    );
    non_negative(
        "/rig/springs/rotation_half_life",
        config.springs.rotation_half_life,
    );
    non_negative("/rig/look_ahead/distance", config.look_ahead.distance);
    non_negative("/rig/look_ahead/half_life", config.look_ahead.half_life);
    non_negative(
        "/rig/vertical_anchor/up_slack",
        config.vertical_anchor.up_slack,
    );
    non_negative(
        "/rig/vertical_anchor/down_slack",
        config.vertical_anchor.down_slack,
    );
    non_negative(
        "/rig/vertical_anchor/half_life",
        config.vertical_anchor.half_life,
    );
    non_negative("/rig/blend_in/seconds", config.blend_in.seconds);
    non_negative("/shake/decay_per_second", rig.shake.decay_per_second);
    for (axis, value) in rig.shake.max_offset.into_iter().enumerate() {
        non_negative(
            [
                "/shake/max_offset/0",
                "/shake/max_offset/1",
                "/shake/max_offset/2",
            ][axis],
            value,
        );
    }
    for (axis, value) in rig.shake.max_angle_deg.into_iter().enumerate() {
        non_negative(
            [
                "/shake/max_angle_deg/0",
                "/shake/max_angle_deg/1",
                "/shake/max_angle_deg/2",
            ][axis],
            value,
        );
    }
    let mut positive = |pointer, value: f32| {
        if !value.is_finite() || value <= 0.0 {
            found.push((pointer, POSITIVE));
        }
    };
    let RigMode::Track(track) = &config.mode;
    positive("/rig/mode/track/search_window", track.search_window);
    positive("/rig/look_ahead/full_speed", config.look_ahead.full_speed);
    positive("/shake/frequency", rig.shake.frequency);
    positive("/shake/exponent", rig.shake.exponent);
    let mut finite = |pointer, value: f32| {
        if !value.is_finite() {
            found.push((pointer, FINITE));
        }
    };
    for (axis, value) in track.offset.into_iter().enumerate() {
        finite(
            [
                "/rig/mode/track/offset/0",
                "/rig/mode/track/offset/1",
                "/rig/mode/track/offset/2",
            ][axis],
            value,
        );
    }
    for (axis, value) in config.focus_offset.into_iter().enumerate() {
        finite(
            [
                "/rig/focus_offset/0",
                "/rig/focus_offset/1",
                "/rig/focus_offset/2",
            ][axis],
            value,
        );
    }
    if let Some(fov) = config.fov_y_deg
        && !(fov.is_finite() && fov > 0.0 && fov < 180.0)
    {
        found.push((
            "/rig/fov_y_deg",
            "must be between 0 and 180 degrees, exclusive",
        ));
    }
    if !(0.0..=1.0).contains(&rig.trauma) {
        found.push(("/trauma", "must be between 0 and 1"));
    }
    found
}

/// The first field of `rig` that breaks a range rule, as `(JSON Pointer, rule)`. The runtime check
/// for a rig built in Rust (`L1304`); the document validator reports every field.
pub fn invalid_field(rig: &CameraRig) -> Option<(&'static str, &'static str)> {
    violations(rig).into_iter().next()
}

impl SemanticValidate for CameraRig {
    /// Rejects every out-of-range field (`L1304`) at its pointer under `/rig/…` or `/shake/…`:
    /// half-lives, `look_ahead.distance`, slacks, `blend_in.seconds`, `decay_per_second` and the shake
    /// maxima negative or not finite; `search_window`, `full_speed`, `frequency`, `exponent` not
    /// positive; offsets not finite; `fov_y_deg` outside `(0, 180)`; `trauma` outside `[0, 1]`. A value
    /// that does not deserialize is left to the schema.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let Ok(rig) = serde_json::from_value::<CameraRig>(value.clone()) else {
            return Vec::new();
        };
        violations(&rig)
            .into_iter()
            .map(|(pointer, rule)| {
                Diagnostic::new(
                    code::CAMERA_RIG_INVALID,
                    format!("`CameraRig` `{pointer}` {rule}"),
                )
                .with_path(pointer.to_string())
                .with_help(
                    "keep half-lives, slacks and shake limits non-negative, `search_window`, \
                     `full_speed`, `frequency` and `exponent` above 0, `fov_y_deg` inside (0, 180) \
                     and `trauma` in [0, 1]",
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pointers(value: Value) -> Vec<(String, Option<String>)> {
        CameraRig::validate_semantics(&value)
            .into_iter()
            .map(|d| (d.code.as_str().to_string(), d.path))
            .collect()
    }

    #[test]
    fn defaults_validate_clean() {
        assert!(CameraRig::validate_semantics(&json!({})).is_empty());
        assert!(invalid_field(&CameraRig::default()).is_none());
    }

    #[test]
    fn each_bad_field_names_its_pointer() {
        for (value, pointer) in [
            (
                json!({"rig": {"springs": {"position_half_life": -0.1}}}),
                "/rig/springs/position_half_life",
            ),
            (
                json!({"rig": {"springs": {"rotation_half_life": -1.0}}}),
                "/rig/springs/rotation_half_life",
            ),
            (
                json!({"rig": {"look_ahead": {"distance": -1.0}}}),
                "/rig/look_ahead/distance",
            ),
            (
                json!({"rig": {"look_ahead": {"half_life": -1.0}}}),
                "/rig/look_ahead/half_life",
            ),
            (
                json!({"rig": {"look_ahead": {"full_speed": 0.0}}}),
                "/rig/look_ahead/full_speed",
            ),
            (
                json!({"rig": {"vertical_anchor": {"up_slack": -1.0}}}),
                "/rig/vertical_anchor/up_slack",
            ),
            (
                json!({"rig": {"vertical_anchor": {"down_slack": -1.0}}}),
                "/rig/vertical_anchor/down_slack",
            ),
            (
                json!({"rig": {"vertical_anchor": {"half_life": -1.0}}}),
                "/rig/vertical_anchor/half_life",
            ),
            (
                json!({"rig": {"blend_in": {"seconds": -1.0}}}),
                "/rig/blend_in/seconds",
            ),
            (
                json!({"rig": {"mode": {"track": {"search_window": 0.0}}}}),
                "/rig/mode/track/search_window",
            ),
            (json!({"rig": {"fov_y_deg": 0.0}}), "/rig/fov_y_deg"),
            (json!({"rig": {"fov_y_deg": 180.0}}), "/rig/fov_y_deg"),
            (
                json!({"shake": {"decay_per_second": -1.0}}),
                "/shake/decay_per_second",
            ),
            (
                json!({"shake": {"max_offset": [0.0, -1.0, 0.0]}}),
                "/shake/max_offset/1",
            ),
            (
                json!({"shake": {"max_angle_deg": [0.0, 0.0, -1.0]}}),
                "/shake/max_angle_deg/2",
            ),
            (json!({"shake": {"frequency": 0.0}}), "/shake/frequency"),
            (json!({"shake": {"exponent": -2.0}}), "/shake/exponent"),
            (json!({"trauma": 1.5}), "/trauma"),
            (json!({"trauma": -0.1}), "/trauma"),
        ] {
            assert_eq!(
                pointers(value.clone()),
                vec![("L1304".to_string(), Some(pointer.to_string()))],
                "{value}"
            );
        }
    }

    #[test]
    fn a_non_finite_field_is_refused_in_rust() {
        let mut rig = CameraRig::default();
        rig.rig.springs.position_half_life = f32::NAN;
        assert_eq!(
            invalid_field(&rig).map(|(pointer, _)| pointer),
            Some("/rig/springs/position_half_life")
        );
        let mut rig = CameraRig::default();
        rig.rig.focus_offset = [0.0, f32::INFINITY, 0.0];
        assert_eq!(
            invalid_field(&rig).map(|(pointer, _)| pointer),
            Some("/rig/focus_offset/1")
        );
        let mut rig = CameraRig::default();
        rig.rig.mode = RigMode::Track(crate::TrackRig {
            offset: [f32::NAN, 0.0, 0.0],
            ..Default::default()
        });
        assert_eq!(
            invalid_field(&rig).map(|(pointer, _)| pointer),
            Some("/rig/mode/track/offset/0")
        );
    }

    #[test]
    fn a_wrongly_shaped_document_is_left_to_the_schema() {
        assert!(CameraRig::validate_semantics(&json!({"trauma": "loud"})).is_empty());
    }
}
