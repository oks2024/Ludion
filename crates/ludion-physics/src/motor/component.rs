//! [`CharacterMotor`] and [`CharacterMotorState`].

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::SemanticValidate;
use ludion_spatial::{Body, Collider, Shape};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use super::shape::{InForce, Lying};

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
    /// Named postures (`crouch`, `low`): each an upright shape and optional tunable overrides the
    /// motor takes on while `CharacterMotorState.posture` names it. Empty is the plain motor and
    /// is omitted from the saved text. Names are identifiers: non-empty, no `/` or `~`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub postures: BTreeMap<String, Posture>,
}

/// One named posture of a [`CharacterMotor`]: a shape, and tunables that replace the motor's while
/// it is in force.
///
/// ```yaml
/// postures:
///   crouch:
///     shape: { upright: { height: 1.1 } }
///     step_height: 0.2
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Posture {
    /// The capsule this posture gives the motor.
    pub shape: PostureShape,
    /// Replaces the motor's `step_height` while in force; `None` inherits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_height: Option<f32>,
    /// Replaces the motor's `slope_limit_deg` while in force; `None` inherits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slope_limit_deg: Option<f32>,
    /// Replaces the motor's `snap_distance` while in force; `None` inherits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snap_distance: Option<f32>,
}

/// A posture's capsule. Externally tagged (`upright: { .. }`) like `Collider.shape`, so further
/// shapes can join without a breaking change.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PostureShape {
    /// A capsule standing on the feet, like the motor's own.
    Upright {
        /// The capsule's full height, feet to crown, in metres.
        height: f32,
        /// The capsule radius, in metres; `None` is the motor's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f32>,
    },
    /// A horizontal capsule along the character's facing (local `-Z`), resting on the ground: prone
    /// and crawl. It yaws with the entity's `Transform` and never pitches or rolls. The capsule's
    /// centre is `offset` ahead of the feet along the facing and a radius above them.
    Lying {
        /// The capsule's full length, tip to tip, in metres (more than `2 * radius`).
        length: f32,
        /// The capsule radius, in metres (finite, `> 0`); it is also half the body's height.
        radius: f32,
        /// How far ahead of the feet the capsule's centre lies, in metres; negative is behind
        /// (default `0`; at most `length / 2 - radius` in magnitude, so the feet stay under the axis).
        #[serde(default, skip_serializing_if = "is_zero")]
        offset: f32,
    },
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
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
            postures: BTreeMap::new(),
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
    /// The posture in force, a key of the motor's `postures`; `None` is the motor's own shape.
    /// Set by `set_character_posture` and `resume_character`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub posture: Option<String>,
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
/// For a lying posture it is the body's height, `2 * radius`.
pub fn height_in_force(motor: &CharacterMotor, state: Option<&CharacterMotorState>) -> f32 {
    motor_in_force(motor, state).height
}

/// The motor as it stands: the posture in force applied (its shape and tunables), then the state's
/// height override on top. The result has no `postures` of its own. A posture the motor does not
/// declare falls back to the base motor.
///
/// A `lying` posture is a horizontal capsule this struct cannot express: `radius` is its radius and
/// `height` is `2 * radius`, the body's height; the state's height override does not apply to it.
///
/// ```no_run
/// # use ludion_physics::{motor_in_force, CharacterMotor, CharacterMotorState};
/// # fn demo(motor: &CharacterMotor, state: Option<&CharacterMotorState>) {
/// let capsule_height = motor_in_force(motor, state).height;
/// # }
/// ```
pub fn motor_in_force(
    motor: &CharacterMotor,
    state: Option<&CharacterMotorState>,
) -> CharacterMotor {
    in_force(motor, state).motor
}

/// The in-force motor with the posture's lying shape alongside, when it has one.
pub(crate) fn in_force(motor: &CharacterMotor, state: Option<&CharacterMotorState>) -> InForce {
    let mut effective = CharacterMotor {
        postures: BTreeMap::new(),
        ..motor.clone()
    };
    let mut lying = None;
    if let Some(posture) = state
        .and_then(|state| state.posture.as_deref())
        .and_then(|name| motor.postures.get(name))
    {
        lying = apply_posture(&mut effective, posture);
    }
    if lying.is_none()
        && let Some(height) = state.and_then(|state| state.height)
    {
        effective.height = height;
    }
    InForce {
        motor: effective,
        lying,
    }
}

/// `motor` with `posture`'s shape and tunables applied; the lying shape when the posture has one.
pub(crate) fn apply_posture(motor: &mut CharacterMotor, posture: &Posture) -> Option<Lying> {
    let lying = match &posture.shape {
        PostureShape::Upright { height, radius } => {
            motor.height = *height;
            if let Some(radius) = radius {
                motor.radius = *radius;
            }
            None
        }
        PostureShape::Lying {
            length,
            radius,
            offset,
        } => {
            motor.radius = *radius;
            motor.height = 2.0 * *radius;
            Some(Lying {
                length: *length,
                radius: *radius,
                offset: *offset,
            })
        }
    };
    if let Some(value) = posture.step_height {
        motor.step_height = value;
    }
    if let Some(value) = posture.slope_limit_deg {
        motor.slope_limit_deg = value;
    }
    if let Some(value) = posture.snap_distance {
        motor.snap_distance = value;
    }
    lying
}

/// The motor with the named posture applied and no height override; `None` when the motor declares
/// no such posture.
pub(crate) fn posture_motor(motor: &CharacterMotor, name: &str) -> Option<InForce> {
    let posture = motor.postures.get(name)?;
    let mut effective = CharacterMotor {
        postures: BTreeMap::new(),
        ..motor.clone()
    };
    let lying = apply_posture(&mut effective, posture);
    Some(InForce {
        motor: effective,
        lying,
    })
}

/// Whether `state` names a posture `motor` does not declare.
pub(crate) fn posture_unknown(motor: &CharacterMotor, state: Option<&CharacterMotorState>) -> bool {
    state
        .and_then(|state| state.posture.as_deref())
        .is_some_and(|name| !motor.postures.contains_key(name))
}

/// The first posture of `motor` that makes it unusable: the JSON Pointer (relative to the
/// component) and the reason. A field the posture inherits from the base is reported at the
/// posture itself, naming the inherited value.
pub(crate) fn check_postures(motor: &CharacterMotor) -> Result<(), (String, String)> {
    for (name, posture) in &motor.postures {
        let at = format!("/postures/{name}");
        if name.is_empty() || name.contains('/') || name.contains('~') {
            return Err((
                at,
                format!("posture name {name:?} must be non-empty and contain no `/` or `~`"),
            ));
        }
        let mut effective = CharacterMotor {
            postures: BTreeMap::new(),
            ..motor.clone()
        };
        let lying = apply_posture(&mut effective, posture);
        let (checked, radius_at, height_at) = match (&posture.shape, &lying) {
            (PostureShape::Upright { radius, .. }, _) => (
                check(&effective),
                radius
                    .is_some()
                    .then(|| format!("{at}/shape/upright/radius")),
                Some(format!("{at}/shape/upright/height")),
            ),
            (_, Some(lying)) => (
                check_lying(lying).and_then(|()| check_tunables(&effective)),
                Some(format!("{at}/shape/lying/radius")),
                None,
            ),
            (_, None) => continue,
        };
        if let Err((field, reason)) = checked {
            let own = match field {
                "/radius" => radius_at,
                "/height" => height_at,
                "/length" => Some(format!("{at}/shape/lying/length")),
                "/offset" => Some(format!("{at}/shape/lying/offset")),
                "/step_height" => posture.step_height.map(|_| format!("{at}/step_height")),
                "/slope_limit_deg" => posture
                    .slope_limit_deg
                    .map(|_| format!("{at}/slope_limit_deg")),
                "/snap_distance" => posture.snap_distance.map(|_| format!("{at}/snap_distance")),
                _ => None,
            };
            return Err(match own {
                Some(pointer) => (pointer, reason),
                None => (
                    at,
                    format!("{reason} (inherited from the motor into posture {name:?})"),
                ),
            });
        }
    }
    Ok(())
}

/// The first field of `motor` out of range, as its JSON Pointer (relative to the component) and
/// the reason; `Ok` when the motor is usable.
pub(crate) fn check(motor: &CharacterMotor) -> Result<(), (&'static str, String)> {
    let positive = |value: f32| value.is_finite() && value > 0.0;
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
    check_tunables(motor)
}

/// The fields of `motor` that do not depend on its capsule's shape: the slope, step, snap and skin
/// distances and `max_slides` (the step against `motor.height`).
fn check_tunables(motor: &CharacterMotor) -> Result<(), (&'static str, String)> {
    let positive = |value: f32| value.is_finite() && value > 0.0;
    let non_negative = |value: f32| value.is_finite() && value >= 0.0;
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

/// A lying shape's own fields: the first out of range, as its pointer under the shape and the reason.
fn check_lying(lying: &Lying) -> Result<(), (&'static str, String)> {
    let Lying {
        length,
        radius,
        offset,
    } = *lying;
    if !(radius.is_finite() && radius > 0.0) {
        return Err((
            "/radius",
            format!("`radius` is {radius}; it must be finite and > 0"),
        ));
    }
    if !(length.is_finite() && length > 2.0 * radius + 1e-3) {
        return Err((
            "/length",
            format!(
                "`length` {length} must be finite and longer than the two caps of a `radius` {radius} capsule (`2 * radius`)"
            ),
        ));
    }
    let reach = length / 2.0 - radius;
    if !(offset.is_finite() && offset.abs() <= reach) {
        return Err((
            "/offset",
            format!(
                "`offset` is {offset}; it must be finite and at most `length / 2 - radius` ({reach}) in magnitude, so the feet stay under the capsule's axis"
            ),
        ));
    }
    Ok(())
}

/// [`check`] for the motor in force: an upright one exactly as `check`, a lying one its own fields
/// and every tunable against its body height, `2 * radius`.
pub(crate) fn check_in_force(force: &InForce) -> Result<(), (&'static str, String)> {
    match &force.lying {
        None => check(&force.motor),
        Some(lying) => check_lying(lying).and_then(|()| check_tunables(&force.motor)),
    }
}

impl SemanticValidate for CharacterMotor {
    /// Rejects the first out-of-range field (`L1294`) at its pointer: lengths finite and positive,
    /// `snap_distance` finite and non-negative, `skin_width` finite and positive, `slope_limit_deg`
    /// in `(0, 90)`, `step_height < height`, `height >= 2 * radius`, `max_slides` in `1..=16`; then every posture's
    /// effective motor the same way, at the posture's own pointer. JSON
    /// Schema reaches none of the cross-field rules. A value that does not deserialize is left to the schema.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let Ok(motor) = serde_json::from_value::<CharacterMotor>(value.clone()) else {
            return Vec::new();
        };
        match check(&motor) {
            Ok(()) => match check_postures(&motor) {
                Ok(()) => Vec::new(),
                Err((pointer, reason)) => vec![motor_invalid(&reason, &pointer)],
            },
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

    fn with_posture(posture: Value) -> Value {
        json!({ "postures": { "crouch": posture } })
    }

    #[test]
    fn each_bad_posture_field_names_its_pointer() {
        for (posture, pointer) in [
            (
                json!({"shape": {"upright": {"height": 0.5}}}),
                "/postures/crouch/shape/upright/height",
            ),
            (
                json!({"shape": {"upright": {"height": 1.0, "radius": -1.0}}}),
                "/postures/crouch/shape/upright/radius",
            ),
            (
                json!({"shape": {"upright": {"height": 1.0}}, "step_height": 0.0}),
                "/postures/crouch/step_height",
            ),
            (
                json!({"shape": {"upright": {"height": 1.0}}, "slope_limit_deg": 95.0}),
                "/postures/crouch/slope_limit_deg",
            ),
            (
                json!({"shape": {"upright": {"height": 1.0}}, "snap_distance": -1.0}),
                "/postures/crouch/snap_distance",
            ),
        ] {
            assert_eq!(
                codes_at(with_posture(posture.clone())),
                vec![("L1294".to_string(), Some(pointer.to_string()))],
                "{posture}"
            );
        }
    }

    #[test]
    fn each_bad_lying_field_names_its_pointer() {
        let lying = |fields: Value| json!({"shape": {"lying": fields}});
        for (posture, pointer) in [
            (
                lying(json!({"length": 1.7, "radius": 0.0})),
                "/postures/crouch/shape/lying/radius",
            ),
            (
                lying(json!({"length": 0.3, "radius": 0.17})),
                "/postures/crouch/shape/lying/length",
            ),
            (
                lying(json!({"length": 1.7, "radius": 0.17, "offset": 0.7})),
                "/postures/crouch/shape/lying/offset",
            ),
            (
                lying(json!({"length": 1.7, "radius": 0.17, "offset": -0.7})),
                "/postures/crouch/shape/lying/offset",
            ),
            (
                json!({"shape": {"lying": {"length": 1.7, "radius": 0.17}}, "step_height": 0.4}),
                "/postures/crouch/step_height",
            ),
            (
                json!({"shape": {"lying": {"length": 1.7, "radius": 0.17}}, "slope_limit_deg": 90.0}),
                "/postures/crouch/slope_limit_deg",
            ),
            (
                json!({"shape": {"lying": {"length": 1.7, "radius": 0.17}}, "snap_distance": -1.0}),
                "/postures/crouch/snap_distance",
            ),
        ] {
            assert_eq!(
                codes_at(with_posture(posture.clone())),
                vec![("L1294".to_string(), Some(pointer.to_string()))],
                "{posture}"
            );
        }
        let fine = lying(json!({"length": 1.7, "radius": 0.17, "offset": 0.68}));
        assert!(CharacterMotor::validate_semantics(&with_posture(fine)).is_empty());
    }

    #[test]
    fn an_inherited_step_that_is_too_tall_for_a_lying_body_is_reported_at_the_posture() {
        // the inherited step_height 0.3 is not below the lying body's height, 0.2.
        let found = CharacterMotor::validate_semantics(&with_posture(
            json!({"shape": {"lying": {"length": 1.0, "radius": 0.1}}}),
        ));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path.as_deref(), Some("/postures/crouch"));
    }

    #[test]
    fn a_lying_motor_in_force_has_the_lying_radius_and_twice_it_for_height() {
        let mut motor = CharacterMotor::default();
        motor.postures.insert(
            "prone".into(),
            Posture {
                shape: PostureShape::Lying {
                    length: 1.7,
                    radius: 0.17,
                    offset: 0.0,
                },
                step_height: Some(0.06),
                slope_limit_deg: None,
                snap_distance: None,
            },
        );
        let state = CharacterMotorState {
            posture: Some("prone".into()),
            height: Some(1.0),
            ..Default::default()
        };
        let force = in_force(&motor, Some(&state));
        assert_eq!(
            (
                force.motor.radius,
                force.motor.height,
                force.motor.step_height
            ),
            (0.17, 2.0 * 0.17, 0.06)
        );
        assert!(force.lying.is_some());
        assert!(check_in_force(&force).is_ok());
    }

    #[test]
    fn an_inherited_field_is_reported_at_the_posture() {
        // the inherited step_height 0.3 is not below the posture height 0.25.
        let found = CharacterMotor::validate_semantics(&with_posture(
            json!({"shape": {"upright": {"height": 0.25, "radius": 0.1}}}),
        ));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path.as_deref(), Some("/postures/crouch"));
        assert!(
            found[0].message.contains("inherited"),
            "{}",
            found[0].message
        );
    }

    #[test]
    fn a_bad_posture_name_is_refused() {
        for name in ["", "a/b", "a~b"] {
            let found =
                codes_at(json!({"postures": {name: {"shape": {"upright": {"height": 1.0}}}}}));
            assert_eq!(found.len(), 1, "{name:?}");
            assert_eq!(found[0].0, "L1294");
        }
    }

    #[test]
    fn a_motor_in_force_applies_the_posture_then_the_height() {
        let mut motor = CharacterMotor::default();
        motor.postures.insert(
            "crouch".into(),
            Posture {
                shape: PostureShape::Upright {
                    height: 1.0,
                    radius: Some(0.3),
                },
                step_height: Some(0.2),
                slope_limit_deg: None,
                snap_distance: None,
            },
        );
        let state = CharacterMotorState {
            posture: Some("crouch".into()),
            ..Default::default()
        };
        let effective = motor_in_force(&motor, Some(&state));
        assert_eq!(
            (effective.height, effective.radius, effective.step_height),
            (1.0, 0.3, 0.2)
        );
        assert_eq!(effective.slope_limit_deg, motor.slope_limit_deg);
        let resized = CharacterMotorState {
            height: Some(0.8),
            ..state.clone()
        };
        assert_eq!(motor_in_force(&motor, Some(&resized)).height, 0.8);
        let unknown = CharacterMotorState {
            posture: Some("prone".into()),
            ..Default::default()
        };
        assert_eq!(motor_in_force(&motor, Some(&unknown)).height, motor.height);
        assert!(posture_unknown(&motor, Some(&unknown)));
    }

    #[test]
    fn a_plain_motor_serializes_without_postures() {
        let text = serde_json::to_string(&CharacterMotor::default()).unwrap();
        assert!(!text.contains("postures"));
        let state = serde_json::to_string(&CharacterMotorState::default()).unwrap();
        assert!(!state.contains("posture"));
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
