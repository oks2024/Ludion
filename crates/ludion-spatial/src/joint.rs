//! The [`Joint`] builtin component and its [`JointKind`] enum.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A physical constraint between two rigid bodies, mapped by `ludion-physics` onto a rapier impulse
/// joint. Carried by any entity — including `a` or `b` itself — the [`Joint`]'s **carrier**; one
/// `Joint` per carrier, so a body that needs more than one joint takes a dedicated carrier entity per
/// joint (`Plan/physics.md`).
///
/// `a` and `b` name their bodies by [`EntityId`], the `Parent` precedent: live ids at runtime and on
/// the wire, document positions (generation `0`) in a scene. `anchor` and each kind's `axis` are
/// authored in `a`'s local frame; `b`'s frame is derived when the joint is built so the bodies'
/// current relative pose becomes the joint's zero — a mid-flight grab forms where the bodies already
/// are.
///
/// ```yaml
/// components:
///   Joint:
///     a: { index: 0, generation: 0 }
///     b: { index: 1, generation: 0 }
///     kind:
///       hinge:
///         axis: [0.0, 1.0, 0.0]
///         limits: [-45.0, 45.0]
///     anchor: [0.0, 0.5, 0.0]
///     break_impulse: 500.0
///     collide: false
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Joint {
    /// The first body, whose local frame `anchor` and the kind's `axis` are authored in.
    #[schemars(with = "ludion_ecs::EntityRefSelfSchema")]
    pub a: EntityId,
    /// The second body.
    #[schemars(with = "ludion_ecs::EntityRefSelfSchema")]
    pub b: EntityId,
    /// The constraint kind and its axis/limits.
    pub kind: JointKind,
    /// The joint's anchor point, in `a`'s local frame (default the origin).
    #[serde(default)]
    pub anchor: [f32; 3],
    /// The linear constraint impulse magnitude, in one step, above which the joint breaks (default
    /// unset: the joint never breaks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub break_impulse: Option<f32>,
    /// Whether `a` and `b` still collide with each other (rapier's `contacts_enabled`; default
    /// `false` — a joint's two bodies do not collide).
    #[serde(default)]
    pub collide: bool,
    /// The rotation of `b` relative to `a` that counts as the joint's zero, `q_a⁻¹·q_b` as
    /// `[x, y, z, w]` (default unset: captured from the two bodies' relative pose at build, so the
    /// zero is wherever the bodies happened to be). Set, the joint's limits, [`JointMotor`]
    /// `target` and [`JointPoses`] are measured from it, and it survives a body rebuild, a ragdoll
    /// retire and re-admission, a snapshot restore and a scene save and reload. The anchors keep
    /// coinciding at build; only the rotation comes from the zero. A body that starts outside the
    /// limits measured from the zero is pulled back by the solver within the first step.
    ///
    /// [`JointMotor`]: crate::JointMotor
    /// [`JointPoses`]: crate::JointPoses
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zero: Option<[f32; 4]>,
}

/// A joint's constraint kind, its axis (where it has one) and optional limits.
///
/// Serialized with serde's external tagging (a one-key map whose key is the snake_case variant
/// name), mirroring [`Shape`](crate::Shape). Angles are degrees, the `Transform` convention;
/// `ludion-physics` converts to radians when it builds the rapier joint.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum JointKind {
    /// One rotational degree of freedom about `axis`, in `a`'s local frame.
    Hinge {
        /// The hinge axis, in `a`'s local frame (must be non-zero and finite; default up,
        /// `[0.0, 1.0, 0.0]`).
        #[serde(default = "up")]
        axis: [f32; 3],
        /// The rotation limit about `axis`, in degrees, as `[min, max]` (default unset: free
        /// rotation).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limits: Option<[f32; 2]>,
    },
    /// A ball-and-socket joint: free rotation about `a`'s anchor, optionally limited per axis.
    Ball {
        /// Per-axis rotation limits, in degrees, as `[[min, max]; 3]` on `a`'s local x/y/z (default
        /// unset: free rotation).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limits: Option<[[f32; 2]; 3]>,
    },
    /// One translational degree of freedom along `axis`, in `a`'s local frame.
    Slider {
        /// The slide axis, in `a`'s local frame (must be non-zero and finite; default up,
        /// `[0.0, 1.0, 0.0]`).
        #[serde(default = "up")]
        axis: [f32; 3],
        /// The translation limit along `axis`, in world units, as `[min, max]` (default unset: free
        /// translation).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limits: Option<[f32; 2]>,
    },
    /// No relative motion at all: `b` is welded to `a` at the joint's captured frame.
    Fixed {},
}

impl SemanticValidate for Joint {
    /// Rejects `a == b` (`/a`), a non-finite `anchor` (`/anchor`), a non-finite or zero-length axis
    /// (`/kind/hinge/axis`, `/kind/slider/axis`), an inverted or non-finite limit
    /// (`/kind/hinge/limits`, `/kind/slider/limits`, `/kind/ball/limits`), and a `break_impulse` that
    /// is not strictly positive and finite (`/break_impulse`) — all `L1183` — and a `zero` with a
    /// non-finite element or a length under `1e-6` (`/zero`, `L1750`). JSON Schema reaches none of
    /// these: `a`/`b` equality and a limit's ordering are cross-field, and `range(min)` does not
    /// reach array elements. Defensive: runs on a schema-clean value.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();

        if let (Some(a), Some(b)) = (value.get("a"), value.get("b"))
            && a == b
        {
            out.push(joint_invalid(
                "a joint's `a` and `b` name the same entity; a joint needs two distinct bodies"
                    .to_string(),
                "/a",
            ));
        }

        if let Some(anchor) = value.get("anchor").and_then(Value::as_array)
            && as_finite_vec3(anchor).is_none()
        {
            out.push(joint_invalid(
                format!(
                    "joint `anchor` must be finite, found {}",
                    Value::Array(anchor.clone())
                ),
                "/anchor",
            ));
        }

        if let Some(kind) = value.get("kind") {
            if let Some(hinge) = kind.get("hinge") {
                validate_axis(hinge, "/kind/hinge/axis", &mut out);
                validate_limit_pair(hinge, "/kind/hinge/limits", &mut out);
            } else if let Some(slider) = kind.get("slider") {
                validate_axis(slider, "/kind/slider/axis", &mut out);
                validate_limit_pair(slider, "/kind/slider/limits", &mut out);
            } else if let Some(ball) = kind.get("ball") {
                validate_ball_limits(ball, &mut out);
            }
            // `fixed` has no axis or limits to check.
        }

        if let Some(break_impulse) = value.get("break_impulse").and_then(Value::as_f64) {
            check_break_impulse(break_impulse, &mut out);
        }

        if let Some(zero) = value.get("zero").and_then(Value::as_array) {
            out.extend(check_zero(zero));
        }

        out
    }
}

/// `L1750` when a joint `zero` has a non-finite element or a length under `1e-6`.
fn check_zero(zero: &[Value]) -> Option<Diagnostic> {
    let mut length_sq = 0.0_f64;
    for element in zero {
        let Some(element) = element.as_f64() else {
            continue;
        };
        if !element.is_finite() {
            return Some(zero_invalid(format!(
                "joint `zero` holds {element}; every element must be finite"
            )));
        }
        length_sq += element * element;
    }
    (length_sq < 1e-12).then(|| {
        zero_invalid("joint `zero` has length under 1e-6; it must be a quaternion".to_string())
    })
}

fn zero_invalid(message: String) -> Diagnostic {
    Diagnostic::new(code::JOINT_ZERO_INVALID, message)
        .with_path("/zero".to_string())
        .with_help("give `zero` four finite numbers `[x, y, z, w]` of a non-zero quaternion")
}

/// Pushes an `L1183` when `break_impulse` is not greater than zero as the `f32` the joint stores
/// (zero, negative, non-finite, past `f32`'s range, or so small it narrows to `0.0`), so the solver
/// never sees a joint that breaks on any load (the `Collider` `require_positive_finite` precedent).
fn check_break_impulse(value: f64, out: &mut Vec<Diagnostic>) {
    if !ludion_core::number::positive_f32(value) {
        out.push(joint_invalid(
            format!(
                "joint `break_impulse` {value} is not valid; it must be greater than 0 as an \
                 f32 (at least ~1.4e-45, at most ~3.4e38)"
            ),
            "/break_impulse",
        ));
    }
}

/// Below this f32 length, `ludion-physics`'s `to_vector(axis).normalize()` (`joints.rs`) is too
/// close to dividing by zero to trust: an axis like `[1e-50, 0.0, 0.0]` passes an exactly-zero
/// check on the authored `f64`s, then rounds to exactly `0.0` once narrowed to `f32` and poisons
/// the solver with `NaN`. Comfortably
/// above `f32::EPSILON` and comfortably below any axis a scene author would type on purpose.
const MIN_AXIS_LENGTH: f32 = 1e-6;

/// Pushes an `L1183` when `field`'s `axis` is missing, non-finite, or too short to normalize once
/// narrowed to `f32` (see [`MIN_AXIS_LENGTH`]).
fn validate_axis(field: &Value, pointer: &str, out: &mut Vec<Diagnostic>) {
    let Some(axis) = field.get("axis").and_then(Value::as_array) else {
        return;
    };
    let Some(components) = as_finite_vec3(axis) else {
        out.push(joint_invalid(
            format!(
                "`axis` must be finite, found {}",
                Value::Array(axis.clone())
            ),
            pointer,
        ));
        return;
    };
    let [x, y, z] = components.map(|c| c as f32);
    if (x * x + y * y + z * z).sqrt() < MIN_AXIS_LENGTH {
        out.push(joint_invalid(
            format!(
                "`axis` must not be the zero vector (or round to it once narrowed to f32), found {}",
                Value::Array(axis.clone())
            ),
            pointer,
        ));
    }
}

/// Pushes an `L1183` when `field`'s `limits` (a `[min, max]` pair) is non-finite or inverted.
fn validate_limit_pair(field: &Value, pointer: &str, out: &mut Vec<Diagnostic>) {
    let Some(limits) = field.get("limits").and_then(Value::as_array) else {
        return;
    };
    let [Some(min), Some(max)] = [limits.first(), limits.get(1)].map(|v| v.and_then(Value::as_f64))
    else {
        return;
    };
    check_limit(min, max, pointer, out);
}

/// Pushes an `L1183` for each of a ball joint's three per-axis `limits` pairs that is non-finite or
/// inverted.
fn validate_ball_limits(ball: &Value, out: &mut Vec<Diagnostic>) {
    let Some(limits) = ball.get("limits").and_then(Value::as_array) else {
        return;
    };
    for pair in limits {
        let Some(pair) = pair.as_array() else {
            continue;
        };
        let [Some(min), Some(max)] = [pair.first(), pair.get(1)].map(|v| v.and_then(Value::as_f64))
        else {
            continue;
        };
        check_limit(min, max, "/kind/ball/limits", out);
    }
}

/// Pushes an `L1183` when a `[min, max]` limit pair is non-finite or `min > max`.
fn check_limit(min: f64, max: f64, pointer: &str, out: &mut Vec<Diagnostic>) {
    if !min.is_finite() || !max.is_finite() {
        out.push(joint_invalid(
            format!("joint limit [{min}, {max}] must be finite"),
            pointer,
        ));
    } else if min > max {
        out.push(joint_invalid(
            format!("joint limit minimum {min} exceeds its maximum {max}"),
            pointer,
        ));
    }
}

/// A 3-element JSON array as finite `f64`s, or `None` if it is the wrong length or any element is
/// missing, not a number, or non-finite.
fn as_finite_vec3(elements: &[Value]) -> Option<[f64; 3]> {
    if elements.len() != 3 {
        return None;
    }
    let mut out = [0.0; 3];
    for (i, element) in elements.iter().enumerate() {
        let value = element.as_f64()?;
        if !value.is_finite() {
            return None;
        }
        out[i] = value;
    }
    Some(out)
}

/// `L1183` — an invalid `Joint`.
fn joint_invalid(message: String, pointer: &str) -> Diagnostic {
    Diagnostic::new(code::JOINT_INVALID, message)
        .with_path(pointer.to_string())
        .with_help(
            "name two distinct bodies, give every axis and anchor finite coordinates, keep each \
             axis non-zero, order each limit as [min, max] with both finite, and give \
             `break_impulse` a value greater than 0 if set",
        )
}

fn up() -> [f32; 3] {
    [0.0, 1.0, 0.0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(index: u32) -> EntityId {
        EntityId::new(index, 0)
    }

    #[test]
    fn hinge_round_trips_externally_tagged() {
        let joint = Joint {
            a: id(0),
            b: id(1),
            kind: JointKind::Hinge {
                axis: [0.0, 1.0, 0.0],
                limits: Some([-45.0, 45.0]),
            },
            anchor: [0.0, 0.5, 0.0],
            break_impulse: Some(500.0),
            collide: false,
            zero: None,
        };
        let value = serde_json::to_value(&joint).unwrap();
        assert_eq!(
            value,
            json!({
                "a": { "index": 0, "generation": 0 },
                "b": { "index": 1, "generation": 0 },
                "kind": { "hinge": { "axis": [0.0, 1.0, 0.0], "limits": [-45.0, 45.0] } },
                "anchor": [0.0, 0.5, 0.0],
                "break_impulse": 500.0,
                "collide": false,
            })
        );
        assert_eq!(serde_json::from_value::<Joint>(value).unwrap(), joint);
    }

    #[test]
    fn fixed_round_trips_as_an_empty_object() {
        let joint = Joint {
            a: id(0),
            b: id(1),
            kind: JointKind::Fixed {},
            anchor: [0.0, 0.0, 0.0],
            break_impulse: None,
            collide: false,
            zero: None,
        };
        let value = serde_json::to_value(&joint).unwrap();
        assert_eq!(value["kind"], json!({ "fixed": {} }));
        assert!(value.get("zero").is_none(), "an absent zero writes no key");
        assert_eq!(serde_json::from_value::<Joint>(value).unwrap(), joint);
    }

    #[test]
    fn a_zero_round_trips() {
        let joint = Joint {
            a: id(0),
            b: id(1),
            kind: JointKind::Fixed {},
            anchor: [0.0, 0.0, 0.0],
            break_impulse: None,
            collide: false,
            zero: Some([0.0, 0.382_683_43, 0.0, 0.923_879_5]),
        };
        let value = serde_json::to_value(&joint).unwrap();
        assert_eq!(serde_json::from_value::<Joint>(value).unwrap(), joint);
    }

    #[test]
    fn an_invalid_zero_is_l1750_at_zero() {
        let cases = [
            (json!([0.0, 0.0, 0.0, 1.0]), 0),
            (json!([0.0, 1.0, 0.0, 1.0]), 0),
            (json!([0.0, 0.0, 0.0, 0.0]), 1),
            (json!([0.0, 0.0, 0.0, 1e-7]), 1),
        ];
        for (zero, expected) in cases {
            let value = json!({
                "a": { "index": 0, "generation": 0 },
                "b": { "index": 1, "generation": 0 },
                "kind": { "fixed": {} },
                "zero": zero,
            });
            let out = Joint::validate_semantics(&value);
            assert_eq!(out.len(), expected, "{value}");
            for d in out {
                assert_eq!(d.code.as_str(), "L1750");
                assert_eq!(d.path.as_deref(), Some("/zero"));
            }
        }
    }

    #[test]
    fn defaults_fill_in_when_only_a_b_kind_are_given() {
        let joint: Joint = serde_json::from_value(json!({
            "a": { "index": 0, "generation": 0 },
            "b": { "index": 1, "generation": 0 },
            "kind": { "slider": {} },
        }))
        .unwrap();
        assert_eq!(joint.anchor, [0.0, 0.0, 0.0]);
        assert_eq!(joint.break_impulse, None);
        assert!(!joint.collide);
        assert_eq!(
            joint.kind,
            JointKind::Slider {
                axis: [0.0, 1.0, 0.0],
                limits: None,
            }
        );
    }

    #[test]
    fn a_equal_to_b_is_l1183_at_a() {
        let bad = json!({
            "a": { "index": 0, "generation": 0 },
            "b": { "index": 0, "generation": 0 },
            "kind": { "fixed": {} },
        });
        let out = Joint::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1183");
        assert_eq!(out[0].path.as_deref(), Some("/a"));
    }

    #[test]
    fn a_non_finite_anchor_is_l1183_at_anchor() {
        let bad = json!({
            "a": { "index": 0, "generation": 0 },
            "b": { "index": 1, "generation": 0 },
            "kind": { "fixed": {} },
            "anchor": [0.0, f64::NAN, 0.0],
        });
        let out = Joint::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1183");
        assert_eq!(out[0].path.as_deref(), Some("/anchor"));
    }

    #[test]
    fn a_zero_length_hinge_axis_is_l1183() {
        let bad = json!({
            "a": { "index": 0, "generation": 0 },
            "b": { "index": 1, "generation": 0 },
            "kind": { "hinge": { "axis": [0.0, 0.0, 0.0] } },
        });
        let out = Joint::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1183");
        assert_eq!(out[0].path.as_deref(), Some("/kind/hinge/axis"));
        assert!(out[0].message.contains("zero vector"), "{}", out[0].message);
    }

    /// An axis this short survives an exactly-zero check on the authored `f64`s but rounds to
    /// exactly `0.0` once narrowed to `f32` — the cast `ludion-engine`'s `ragdoll.rs`
    /// (`joint_kind_of`) performs — and would poison the solver with `NaN` if it slipped through.
    #[test]
    fn a_near_zero_hinge_axis_is_l1183() {
        let bad = json!({
            "a": { "index": 0, "generation": 0 },
            "b": { "index": 1, "generation": 0 },
            "kind": { "hinge": { "axis": [1e-50, 0.0, 0.0] } },
        });
        let out = Joint::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1183");
        assert_eq!(out[0].path.as_deref(), Some("/kind/hinge/axis"));
        assert!(out[0].message.contains("zero vector"), "{}", out[0].message);
    }

    #[test]
    fn a_non_finite_slider_axis_is_l1183() {
        let bad = json!({
            "a": { "index": 0, "generation": 0 },
            "b": { "index": 1, "generation": 0 },
            "kind": { "slider": { "axis": [f64::INFINITY, 0.0, 0.0] } },
        });
        let out = Joint::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1183");
        assert_eq!(out[0].path.as_deref(), Some("/kind/slider/axis"));
    }

    #[test]
    fn an_inverted_hinge_limit_is_l1183() {
        let bad = json!({
            "a": { "index": 0, "generation": 0 },
            "b": { "index": 1, "generation": 0 },
            "kind": { "hinge": { "limits": [45.0, -45.0] } },
        });
        let out = Joint::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1183");
        assert_eq!(out[0].path.as_deref(), Some("/kind/hinge/limits"));
        assert!(out[0].message.contains("exceeds"), "{}", out[0].message);
    }

    /// A non-finite limit cannot arrive through JSON (no JSON value can carry `NaN`/`inf` —
    /// `ludion-scene`'s `scalar.rs` rejects it upstream), so this unit-tests the guard directly, the
    /// `Collider` `require_positive_finite` precedent.
    #[test]
    fn a_non_finite_limit_is_l1183() {
        let mut out = Vec::new();
        check_limit(0.0, f64::NAN, "/kind/ball/limits", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1183");
        assert_eq!(out[0].path.as_deref(), Some("/kind/ball/limits"));
        assert!(out[0].message.contains("finite"), "{}", out[0].message);
    }

    #[test]
    fn an_inverted_ball_limit_is_l1183_at_kind_ball_limits() {
        let bad = json!({
            "a": { "index": 0, "generation": 0 },
            "b": { "index": 1, "generation": 0 },
            "kind": { "ball": { "limits": [[-10.0, 10.0], [10.0, -10.0], [-5.0, 5.0]] } },
        });
        let out = Joint::validate_semantics(&bad);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code.as_str(), "L1183");
        assert_eq!(out[0].path.as_deref(), Some("/kind/ball/limits"));
    }

    #[test]
    fn a_non_positive_break_impulse_is_l1183_through_the_full_document() {
        // 1e-46 is positive as an f64 but narrows to 0.0, a joint that breaks on any load.
        for bad_value in [0.0, -1.0, 1e-46] {
            let bad = json!({
                "a": { "index": 0, "generation": 0 },
                "b": { "index": 1, "generation": 0 },
                "kind": { "fixed": {} },
                "break_impulse": bad_value,
            });
            let out = Joint::validate_semantics(&bad);
            assert_eq!(out.len(), 1, "expected one diagnostic for {bad_value}");
            assert_eq!(out[0].code.as_str(), "L1183");
            assert_eq!(out[0].path.as_deref(), Some("/break_impulse"));
        }
    }

    /// A non-finite `break_impulse` cannot arrive through JSON either; unit-tests the guard directly.
    #[test]
    fn a_non_finite_break_impulse_is_l1183() {
        for bad_value in [f64::NAN, f64::INFINITY] {
            let mut out = Vec::new();
            check_break_impulse(bad_value, &mut out);
            assert_eq!(out.len(), 1, "expected one diagnostic for {bad_value}");
            assert_eq!(out[0].code.as_str(), "L1183");
            assert_eq!(out[0].path.as_deref(), Some("/break_impulse"));
        }
    }

    #[test]
    fn a_valid_joint_of_each_kind_is_accepted() {
        let cases = [
            json!({
                "a": { "index": 0, "generation": 0 }, "b": { "index": 1, "generation": 0 },
                "kind": { "hinge": { "axis": [0.0, 1.0, 0.0], "limits": [-45.0, 45.0] } },
                "break_impulse": 10.0,
            }),
            json!({
                "a": { "index": 0, "generation": 0 }, "b": { "index": 1, "generation": 0 },
                "kind": { "ball": { "limits": [[-10.0, 10.0], [-10.0, 10.0], [-10.0, 10.0]] } },
            }),
            json!({
                "a": { "index": 0, "generation": 0 }, "b": { "index": 1, "generation": 0 },
                "kind": { "slider": { "axis": [1.0, 0.0, 0.0] } },
            }),
            json!({
                "a": { "index": 0, "generation": 0 }, "b": { "index": 1, "generation": 0 },
                "kind": { "fixed": {} },
            }),
        ];
        for case in cases {
            let out = Joint::validate_semantics(&case);
            assert!(out.is_empty(), "{case:?} -> {out:?}");
        }
    }
}
