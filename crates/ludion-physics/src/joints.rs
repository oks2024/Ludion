//! [`JointBroken`]: a joint's break event. The joint table itself ([`JointEntry`]) and the mapping
//! from an authored [`Joint`] onto a rapier [`GenericJoint`] — [`build_generic_joint`] — also live
//! here; [`PhysicsWorld`](crate::PhysicsWorld) owns the table and drives reconciliation from
//! `systems.rs::write`, the `BodyEntry`/`collect_contacts` precedent.

use ludion_core::ids::EntityId;
use ludion_core::math::{sin, sqrt};
use ludion_spatial::{Joint, JointKind, JointMotor, expected_targets};
use rapier3d::prelude::{
    GenericJoint, ImpulseJointHandle, JointAxesMask, JointAxis, MotorModel, Pose, Rotation, Vector,
};
use serde::{Deserialize, Serialize};

use crate::world::{source_as_json, to_vector};

/// One carrier's joint break, on the step `PhysicsWorld::step` that
/// detected it.
///
/// Read through
/// [`PhysicsWorld::joint_events`](crate::PhysicsWorld::joint_events): one step's list, sorted
/// ascending by [`joint`](JointBroken::joint) (the carrier) — the
/// [`ContactEvent`](crate::ContactEvent) contract, though a joint break, unlike a contact
/// transition, never repeats: once a carrier's [`Joint`] is gone, [`PhysicsWorld`](crate::PhysicsWorld)
/// has nothing left to break. The carrier's `Joint` component is removed by the mirror system in the
/// same tick — a joint exists iff its component does (`Plan/physics.md` design decision 4).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct JointBroken {
    /// The entity that carried the broken [`Joint`].
    pub joint: EntityId,
    /// The joint's first body.
    pub a: EntityId,
    /// The joint's second body.
    pub b: EntityId,
    /// The linear constraint impulse magnitude that broke it.
    pub impulse: f32,
}

/// One row of the joint handle table: creation order, the only iteration order — the `BodyEntry`
/// precedent. `handle` is `None` for an inert joint: one of its bodies is missing, or it just broke
/// this step and awaits the mirror's removal of its component.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct JointEntry {
    pub(crate) carrier: EntityId,
    pub(crate) handle: Option<ImpulseJointHandle>,
    /// Set the step `PhysicsWorld::step` detects the break, so
    /// `reconcile_joints` never tries to reattach this entry before the mirror removes the
    /// carrier's `Joint` component (`Plan/physics.md` design decision 4).
    pub(crate) broken: bool,
    /// The `Joint` this entry's rapier joint (when attached) was built from — compared bit-for-bit
    /// against the carrier's current one every tick to detect a live edit, the `BodyEntry::source`
    /// precedent. Encoded through [`source_as_json`] for the same reason: `Joint`'s `limits` and
    /// `break_impulse` fields are `skip_serializing_if`, which desyncs bincode's positional codec.
    #[serde(with = "source_as_json")]
    pub(crate) source: Joint,
}

/// The rapier joint for `joint`'s kind, with `anchor` and the kind's `axis` read in `a`'s local
/// frame and its second frame derived against `b`'s current pose `pb` so the bodies' current
/// relative pose — `pa` — becomes the joint's zero (`Plan/physics.md` design decision 3), unless
/// `joint.zero` is set, whose rotation is used instead. Angles are authored in degrees, the
/// `Transform` convention, and converted to radians here.
pub(crate) fn build_generic_joint(pa: Pose, pb: Pose, joint: &Joint) -> GenericJoint {
    let mut generic = GenericJoint::new(locked_axes(&joint.kind));
    generic.set_local_anchor1(to_vector(joint.anchor));
    match &joint.kind {
        JointKind::Hinge { axis, limits } => {
            generic.set_local_axis1(to_vector(*axis).normalize());
            if let Some([min, max]) = limits {
                generic.set_limits(JointAxis::AngX, [min.to_radians(), max.to_radians()]);
            }
        }
        JointKind::Slider { axis, limits } => {
            generic.set_local_axis1(to_vector(*axis).normalize());
            if let Some([min, max]) = limits {
                generic.set_limits(JointAxis::LinX, [*min, *max]);
            }
        }
        JointKind::Ball { limits } => {
            if let Some([x, y, z]) = limits {
                for (axis, [min, max]) in [
                    (JointAxis::AngX, x),
                    (JointAxis::AngY, y),
                    (JointAxis::AngZ, z),
                ] {
                    generic.set_limits(axis, [min.to_radians(), max.to_radians()]);
                }
            }
        }
        JointKind::Fixed {} => {}
    }
    generic.contacts_enabled = joint.collide;
    let current = pb.inverse() * pa * generic.local_frame1;
    generic.local_frame2 = match joint.zero.and_then(zero_rotation) {
        // The anchors still coincide at the bodies' current poses; only the rotation comes from the
        // stored zero: at `pb.rot = pa.rot · r0` the two frames are aligned.
        Some(r0) => Pose::from_parts(
            current.translation,
            r0.inverse() * generic.local_frame1.rotation,
        ),
        None => current,
    };
    generic
}

/// The stored zero as a unit rotation, or `None` when it is non-finite or shorter than 1e-6 (treated as
/// absent, the checks `L1750` makes on the validated paths). Normalized by hand with the pure-Rust
/// `sqrt`: glam's `normalize` is not on the determinism allowlist.
fn zero_rotation([x, y, z, w]: [f32; 4]) -> Option<Rotation> {
    let length = sqrt(x * x + y * y + z * z + w * w);
    if !length.is_finite() || length < 1e-6 {
        return None;
    }
    Some(Rotation::from_xyzw(
        x / length,
        y / length,
        z / length,
        w / length,
    ))
}

/// Rapier's per-axis motor rows, indexed by `JointAxis as usize`: `[target_pos, stiffness, damping,
/// max_force]` for a driven axis, `None` for a free one.
pub(crate) type MotorRows = [Option<[f32; 4]>; 6];

/// Every `JointAxis`, in the order `JointAxis as usize` indexes them.
const AXES: [JointAxis; 6] = [
    JointAxis::LinX,
    JointAxis::LinY,
    JointAxis::LinZ,
    JointAxis::AngX,
    JointAxis::AngY,
    JointAxis::AngZ,
];

/// The motor rows `motor` asks of a joint of `kind`: `[None; 6]` for no motor or a passive one
/// (`stiffness` and `damping` both zero), otherwise the rows for the axes `kind` frees, with angles
/// converted to radians as `limits` are; `stiffness` and `damping` pass through in the force-based
/// model's units (see [`apply_motor`]). `Err` names why a motor cannot drive this joint: a `fixed`
/// kind, a `target` of the wrong arity, or a value that is non-finite or negative — the checks
/// `L1188` makes on the validated paths, repeated here for values that bypassed them.
pub(crate) fn desired_motor(
    kind: &JointKind,
    motor: Option<&JointMotor>,
) -> Result<MotorRows, String> {
    let mut rows: MotorRows = [None; 6];
    let Some(motor) = motor else {
        return Ok(rows);
    };
    if motor.stiffness == 0.0 && motor.damping == 0.0 {
        return Ok(rows);
    }
    for (field, value) in [("stiffness", motor.stiffness), ("damping", motor.damping)] {
        if !value.is_finite() || value < 0.0 {
            return Err(format!(
                "motor `{field}` {value} must be a finite number of 0 or more"
            ));
        }
    }
    if let Some(max_force) = motor.max_force
        && (!max_force.is_finite() || max_force < 0.0)
    {
        return Err(format!(
            "motor `max_force` {max_force} must be a finite number of 0 or more"
        ));
    }
    if let Some(index) = motor.target.iter().position(|t| !t.is_finite()) {
        return Err(format!(
            "motor `target` element {index} is {}; it must be finite",
            motor.target[index]
        ));
    }
    let Some(arity) = expected_targets(kind) else {
        return Err("a fixed joint has no free axis to drive".to_string());
    };
    if motor.target.len() != arity {
        return Err(format!(
            "motor `target` has {} elements but this joint kind takes {arity}",
            motor.target.len()
        ));
    }
    let max_force = motor.max_force.unwrap_or(f32::MAX);
    let mut row = |axis: JointAxis, target: f32| {
        rows[axis as usize] = Some([target, motor.stiffness, motor.damping, max_force]);
    };
    match kind {
        JointKind::Hinge { .. } => row(JointAxis::AngX, motor.target[0].to_radians()),
        JointKind::Slider { .. } => row(JointAxis::LinX, motor.target[0]),
        JointKind::Ball { .. } => {
            for (axis, target) in [JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ]
                .into_iter()
                .zip(&motor.target)
            {
                row(axis, target.to_radians());
            }
        }
        JointKind::Fixed {} => unreachable!("a fixed joint has no motor arity"),
    }
    Ok(rows)
}

/// The motor rows `data` currently holds, or `None` if any driven axis is not on the force-based
/// model [`apply_motor`] sets — a joint restored from an older blob is then rewritten. Reads the
/// drive parameters only: a motor's `impulse` is warm-start state the solver owns, and comparing it
/// would rewrite the motor every tick and keep waking its bodies.
pub(crate) fn current_motor(data: &GenericJoint) -> Option<MotorRows> {
    let mut rows: MotorRows = [None; 6];
    for axis in AXES {
        if let Some(m) = data.motor(axis) {
            if m.model != MotorModel::ForceBased {
                return None;
            }
            rows[axis as usize] = Some([m.target_pos, m.stiffness, m.damping, m.max_force]);
        }
    }
    Some(rows)
}

/// Writes `rows` onto `data`: every axis's motor is cleared, then each driven axis is set as a
/// position drive (`target_vel` zero) on rapier's force-based model, so `stiffness` is N·m/rad
/// (N/m on a slider), `damping` N·m·s/rad (N·s/m) and `max_force` a torque in N·m (a force in N).
/// Leaves each motor's `impulse` alone so a retarget keeps its warm start.
pub(crate) fn apply_motor(data: &mut GenericJoint, rows: &MotorRows) {
    data.motor_axes = JointAxesMask::empty();
    for (axis, row) in AXES.into_iter().zip(rows) {
        if let Some([target, stiffness, damping, max_force]) = *row {
            data.set_motor(axis, target, 0.0, stiffness, damping);
            data.set_motor_max_force(axis, max_force);
            data.set_motor_model(axis, MotorModel::ForceBased);
        }
    }
}

/// Below this a driven axis counts as on target: ≈0.11° of angle in the half-angle-sine domain
/// [`motor_off_target`] measures in, or 1 mm of slide. A drive that settles off target (a limit, a
/// `max_force` cap, gravity sag) therefore keeps its bodies awake; that is the price of a steady
/// motor never stalling asleep short of its target.
const MOTOR_REST_EPSILON: f32 = 1e-3;

/// Whether any axis `rows` drives is farther than [`MOTOR_REST_EPSILON`] from its target, measured
/// as rapier's motor row measures it from the two bodies' poses `pa`/`pb`, but in the
/// half-angle-sine domain (`ludion_core::math` has no `asin`): the relative rotation's vector part
/// on an angular axis against `sin(target / 2)`, the slide along frame 1's x on `LinX`.
pub(crate) fn motor_off_target(pa: Pose, pb: Pose, data: &GenericJoint, rows: &MotorRows) -> bool {
    let f1 = pa * data.local_frame1;
    let f2 = pb * data.local_frame2;
    let mut q = f1.rotation.inverse() * f2.rotation;
    if q.w < 0.0 {
        q = -q;
    }
    let half_sines = [q.x, q.y, q.z];
    if let Some([target, ..]) = rows[JointAxis::LinX as usize] {
        let slide = (f2.translation - f1.translation).dot(f1.rotation * Vector::X);
        if (slide - target).abs() > MOTOR_REST_EPSILON {
            return true;
        }
    }
    [JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ]
        .into_iter()
        .enumerate()
        .any(|(k, axis)| {
            rows[axis as usize].is_some_and(|[target, ..]| {
                (half_sines[k] - sin(target * 0.5)).abs() > MOTOR_REST_EPSILON
            })
        })
}

/// The degrees of freedom `kind` locks — one rapier `JointAxesMask` constant per kind.
fn locked_axes(kind: &JointKind) -> JointAxesMask {
    match kind {
        JointKind::Hinge { .. } => JointAxesMask::LOCKED_REVOLUTE_AXES,
        JointKind::Ball { .. } => JointAxesMask::LOCKED_SPHERICAL_AXES,
        JointKind::Slider { .. } => JointAxesMask::LOCKED_PRISMATIC_AXES,
        JointKind::Fixed {} => JointAxesMask::LOCKED_FIXED_AXES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joint() -> Joint {
        Joint {
            a: EntityId::new(0, 0),
            b: EntityId::new(1, 0),
            kind: JointKind::Hinge {
                axis: [0.0, 0.0, 1.0],
                limits: Some([-30.0, 30.0]),
            },
            anchor: [0.0; 3],
            break_impulse: None,
            collide: false,
            zero: None,
        }
    }

    /// A zero-less joint encodes to exactly the bytes of the bare `Joint` with no `zero` key, so a
    /// world that never uses a zero serializes as it did before the field existed.
    #[test]
    fn a_zero_less_joints_blob_bytes_are_the_bare_joint() {
        let joint = joint();
        let bytes = serde_json::to_vec(&joint).unwrap();
        assert!(!String::from_utf8(bytes).unwrap().contains("zero"));
    }

    #[test]
    fn a_joint_round_trips_with_and_without_a_zero() {
        for zero in [None, Some([0.0, 0.5, 0.0, 0.866])] {
            let source = Joint { zero, ..joint() };
            let bytes = serde_json::to_vec(&source).unwrap();
            assert_eq!(serde_json::from_slice::<Joint>(&bytes).unwrap(), source);
        }
    }
}
