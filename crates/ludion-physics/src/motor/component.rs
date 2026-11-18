//! [`CharacterMotor`] and [`CharacterMotorState`].

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::SemanticValidate;
use ludion_spatial::{Body, Collider, Shape};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A character controller's capsule and tunables. The physics write pass gives every motor entity
/// one kinematic capsule body whose **origin is the character's feet**: the body centre sits
/// `height / 2` above the entity's translation, and the capsule never tilts or yaws with the model.
/// The motor owns the capsule — an entity with a `Collider` as well is refused (`L1295`) — and must
/// be a root entity (`L1297`).
///
/// ```yaml
/// components:
///   CharacterMotor:
///     radius: 0.28
///     height: 0.9
///     slope_limit_deg: 45.0
///     step_height: 0.25
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct CharacterMotor {
    /// The capsule radius, in metres (default `0.35`; finite, `> 0`).
    pub radius: f32,
    /// The capsule's full height, feet to crown, in metres (default `1.6`; at least `2 * radius`).
    pub height: f32,
    /// The steepest slope that counts as ground, in degrees (default `50`; in `(0, 90)`).
    pub slope_limit_deg: f32,
    /// The tallest ledge the motor climbs without a jump, in metres (default `0.3`; `> 0` and
    /// below `height`).
    pub step_height: f32,
    /// How far below the feet the motor snaps back to ground while walking off a lip, in metres
    /// (default `0.25`; `>= 0`).
    pub snap_distance: f32,
    /// The gap kept between the capsule and what it touches, in metres (default `0.01`; `> 0`).
    pub skin_width: f32,
    /// The collision layer, named in the scene's `physics.layers`; `None` is the implicit `default`
    /// layer, and the field is omitted from the saved text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// The most slide iterations one move resolves (default `4`; `1..=16`).
    pub max_slides: u32,
}

impl Default for CharacterMotor {
    fn default() -> Self {
        CharacterMotor {
            radius: 0.35,
            height: 1.6,
            slope_limit_deg: 50.0,
            step_height: 0.3,
            snap_distance: 0.25,
            skin_width: 0.01,
            layer: None,
            max_slides: 4,
        }
    }
}

impl CharacterMotor {
    /// The capsule's cylinder half-length for a full `height`, excluding the two caps.
    pub fn half_height_for(&self, height: f32) -> f32 {
        height / 2.0 - self.radius
    }

    /// The kinematic `Collider` the write pass builds the motor's body from, for the `height` in
    /// force (see [`height_in_force`]).
    pub(crate) fn solver_collider(&self, height: f32) -> Collider {
        Collider {
            shape: Shape::Capsule {
                half_height: self.half_height_for(height),
                radius: self.radius,
            },
            body: Body::new_kinematic(),
            layer: self.layer.clone(),
            sensor: false,
        }
    }
}

/// The platform pose a grounded motor last rode, for platform carry.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq)]
pub struct PlatformPose {
    /// The platform's world translation.
    pub translation: [f32; 3],
    /// The platform's world rotation, as an `[x, y, z, w]` quaternion.
    pub rotation: [f32; 4],
}

/// What the engine knows about a motor from its last move. Engine-written: a game never edits it
/// except through the motor functions. It rides the world hash and the snapshot like any
/// component, with `ground` remapped on load and restore.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct CharacterMotorState {
    /// Whether the motor stood on ground at the end of its last move.
    #[serde(skip_serializing_if = "is_false")]
    pub grounded: bool,
    /// The entity the motor stands on, when grounded.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    pub ground: Option<EntityId>,
    /// The ground's pose at the end of the last move, which platform carry measures against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<PlatformPose>,
    /// A capsule height overriding the motor's (a crouch, say); `None` is the motor's `height`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
    /// The tick platform carry last ran on, so it applies once per tick.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carry_tick: Option<u64>,
    /// Parked by `suspend_character`: no body, no moves; `resume_character` brings it back.
    #[serde(skip_serializing_if = "is_false")]
    pub suspended: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The capsule height in force: the state's override when it has one, else the motor's `height`.
pub fn height_in_force(motor: &CharacterMotor, state: Option<&CharacterMotorState>) -> f32 {
    state.and_then(|state| state.height).unwrap_or(motor.height)
}

/// The first field of `motor` out of range, as its JSON Pointer (relative to the component) and
/// the reason; `Ok` when the motor is usable.
pub(crate) fn check(motor: &CharacterMotor) -> Result<(), (&'static str, String)> {
    let positive = |value: f32| value.is_finite() && value > 0.0;
    let non_negative = |value: f32| value.is_finite() && value >= 0.0;
    if !positive(motor.radius) {
        return Err((
            "/radius",
            format!("`radius` is {}; it must be finite and > 0", motor.radius),
        ));
    }
    if !positive(motor.height) {
        return Err((
            "/height",
            format!("`height` is {}; it must be finite and > 0", motor.height),
        ));
    }
    if motor.height < 2.0 * motor.radius + 1e-3 {
        return Err((
            "/height",
            format!(
                "`height` {} is shorter than the two caps of a `radius` {} capsule (`2 * radius`)",
                motor.height, motor.radius
            ),
        ));
    }
    if !(motor.slope_limit_deg.is_finite()
        && motor.slope_limit_deg > 0.0
        && motor.slope_limit_deg < 90.0)
    {
        return Err((
            "/slope_limit_deg",
            format!(
                "`slope_limit_deg` is {}; it must be in (0, 90)",
                motor.slope_limit_deg
            ),
        ));
    }
    if !positive(motor.step_height) {
        return Err((
            "/step_height",
            format!(
                "`step_height` is {}; it must be finite and > 0",
                motor.step_height
            ),
        ));
    }
    if motor.step_height >= motor.height {
        return Err((
            "/step_height",
            format!(
                "`step_height` {} must be below `height` {}",
                motor.step_height, motor.height
            ),
        ));
    }
    if !non_negative(motor.snap_distance) {
        return Err((
            "/snap_distance",
            format!(
                "`snap_distance` is {}; it must be finite and >= 0",
                motor.snap_distance
            ),
        ));
    }
    if !positive(motor.skin_width) {
        return Err((
            "/skin_width",
            format!(
                "`skin_width` is {}; it must be finite and > 0",
                motor.skin_width
            ),
        ));
    }
    if !(1..=16).contains(&motor.max_slides) {
        return Err((
            "/max_slides",
            format!("`max_slides` is {}; it must be in 1..=16", motor.max_slides),
        ));
    }
    Ok(())
}

impl SemanticValidate for CharacterMotor {
    /// Rejects the first out-of-range field (`L1294`) at its pointer: lengths finite and positive,
    /// `snap_distance` finite and non-negative, `skin_width` finite and positive, `slope_limit_deg`
    /// in `(0, 90)`, `step_height < height`, `height >= 2 * radius`, `max_slides` in `1..=16`. JSON
    /// Schema reaches none of the cross-field rules. A value that does not deserialize is left to the schema.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let Ok(motor) = serde_json::from_value::<CharacterMotor>(value.clone()) else {
            return Vec::new();
        };
        match check(&motor) {
            Ok(()) => Vec::new(),
            Err((pointer, reason)) => vec![motor_invalid(&reason, pointer)],
        }
    }
}

/// An `L1294` diagnostic at `pointer`.
pub(crate) fn motor_invalid(reason: &str, pointer: &str) -> Diagnostic {
    Diagnostic::new(code::CHARACTER_MOTOR_INVALID, reason.to_string())
        .with_path(pointer.to_string())
        .with_help(
            "keep every length finite and positive, `slope_limit_deg` inside (0, 90), \
             `step_height` below `height`, `height` at least `2 * radius`, and `max_slides` in 1..=16",
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn codes_at(value: Value) -> Vec<(String, Option<String>)> {
        CharacterMotor::validate_semantics(&value)
            .into_iter()
            .map(|d| (d.code.as_str().to_string(), d.path))
            .collect()
    }

    #[test]
    fn defaults_validate_clean() {
        assert!(CharacterMotor::validate_semantics(&json!({})).is_empty());
        assert!(check(&CharacterMotor::default()).is_ok());
    }

    #[test]
    fn an_empty_document_is_the_default() {
        let motor: CharacterMotor = serde_json::from_value(json!({})).unwrap();
        assert_eq!(motor, CharacterMotor::default());
    }

    #[test]
    fn the_clockout_preset_is_clean() {
        let preset = json!({
            "radius": 0.28, "height": 0.9, "slope_limit_deg": 45.0, "step_height": 0.25,
            "snap_distance": 0.2, "skin_width": 0.01, "max_slides": 4
        });
        assert!(CharacterMotor::validate_semantics(&preset).is_empty());
    }

    #[test]
    fn each_bad_field_names_its_pointer() {
        for (field, value, pointer) in [
            ("radius", json!(0.0), "/radius"),
            ("height", json!(-1.0), "/height"),
            ("height", json!(0.7), "/height"),
            ("slope_limit_deg", json!(90.0), "/slope_limit_deg"),
            ("slope_limit_deg", json!(0.0), "/slope_limit_deg"),
            ("step_height", json!(0.0), "/step_height"),
            ("step_height", json!(1.6), "/step_height"),
            ("snap_distance", json!(-0.1), "/snap_distance"),
            ("skin_width", json!(-0.01), "/skin_width"),
            ("skin_width", json!(0.0), "/skin_width"),
            ("max_slides", json!(0), "/max_slides"),
            ("max_slides", json!(17), "/max_slides"),
        ] {
            let found = codes_at(json!({ field: value }));
            assert_eq!(
                found,
                vec![("L1294".to_string(), Some(pointer.to_string()))],
                "{field} = {value}"
            );
        }
    }

    #[test]
    fn a_non_finite_length_is_refused_in_rust() {
        let motor = CharacterMotor {
            radius: f32::NAN,
            ..CharacterMotor::default()
        };
        assert_eq!(check(&motor).unwrap_err().0, "/radius");
    }

    #[test]
    fn the_capsule_is_feet_to_crown() {
        let motor = CharacterMotor::default();
        let Shape::Capsule {
            half_height,
            radius,
        } = motor.solver_collider(1.6).shape
        else {
            panic!("a capsule");
        };
        assert_eq!(radius, 0.35);
        assert!((half_height + radius - 0.8).abs() < 1e-6);
    }

    #[test]
    fn the_state_override_wins() {
        let motor = CharacterMotor::default();
        let state = CharacterMotorState {
            height: Some(1.0),
            ..Default::default()
        };
        assert_eq!(height_in_force(&motor, Some(&state)), 1.0);
        assert_eq!(height_in_force(&motor, None), 1.6);
    }
}
