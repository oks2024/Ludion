//! The lying posture's geometry, in one place: its facing, rotation, centre and segment. Upright
//! motors never go through here; each caller branches on [`InForce::lying`] so an upright motor keeps
//! its exact arithmetic.

use core::f32::consts::FRAC_1_SQRT_2;

use ludion_core::math::sin_cos;
use ludion_spatial::{Body, Collider, Shape};

use super::carry::yaw_delta;
use super::component::CharacterMotor;

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// A lying capsule's authored dimensions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Lying {
    pub length: f32,
    pub radius: f32,
    pub offset: f32,
}

impl Lying {
    /// The axis segment's half-length, excluding the two caps.
    pub fn half_segment(&self) -> f32 {
        self.length / 2.0 - self.radius
    }
}

/// The motor in force with its lying shape alongside, when the posture has one. For a lying posture
/// `motor.radius` is the lying radius and `motor.height` is `2 * radius`.
#[derive(Clone, Debug)]
pub(crate) struct InForce {
    pub motor: CharacterMotor,
    pub lying: Option<Lying>,
}

impl InForce {
    /// The kinematic `Collider` the write pass builds the body from.
    pub fn solver_collider(&self) -> Collider {
        match &self.lying {
            None => self.motor.solver_collider(self.motor.height),
            Some(lying) => Collider {
                shape: Shape::Capsule {
                    half_height: lying.half_segment(),
                    radius: lying.radius,
                },
                body: Body::new_kinematic(),
                layer: self.motor.layer.clone(),
                sensor: false,
            },
        }
    }
}

/// The yaw about +Y of a pose rotation: its twist, pitch and roll dropped. In `(-pi, pi]`.
pub(crate) fn yaw_of(rotation: [f32; 4]) -> f32 {
    yaw_delta(IDENTITY, rotation)
}

/// The facing for `yaw`: local `-Z` turned about +Y.
pub(crate) fn forward(yaw: f32) -> [f32; 3] {
    let (sin, cos) = sin_cos(yaw);
    [-sin, 0.0, -cos]
}

/// The rotation of the lying capsule: yaw about +Y after a quarter turn about X, which lays parry's
/// local Y axis along local Z.
pub(crate) fn lying_rotation(yaw: f32) -> [f32; 4] {
    let (sin, cos) = sin_cos(yaw / 2.0);
    [
        cos * FRAC_1_SQRT_2,
        sin * FRAC_1_SQRT_2,
        -sin * FRAC_1_SQRT_2,
        cos * FRAC_1_SQRT_2,
    ]
}

/// The lying capsule's centre: `offset` ahead of the feet, a radius above them.
pub(crate) fn lying_centre(lying: &Lying, feet: [f32; 3], yaw: f32) -> [f32; 3] {
    let ahead = forward(yaw);
    [
        feet[0] + lying.offset * ahead[0],
        feet[1] + lying.radius,
        feet[2] + lying.offset * ahead[2],
    ]
}

/// The lying capsule's axis ends and radius at `feet` facing `yaw`.
pub(crate) fn lying_segment(lying: &Lying, feet: [f32; 3], yaw: f32) -> ([f32; 3], [f32; 3], f32) {
    let centre = lying_centre(lying, feet, yaw);
    let ahead = forward(yaw);
    let half = lying.half_segment();
    (
        [
            centre[0] - ahead[0] * half,
            centre[1],
            centre[2] - ahead[2] * half,
        ],
        [
            centre[0] + ahead[0] * half,
            centre[1],
            centre[2] + ahead[2] * half,
        ],
        lying.radius,
    )
}

/// The segment of the motor's capsule for the debug draw: the axis end points and the radius, at
/// `feet` with the pose `rotation` (only a lying posture reads its yaw). An upright capsule's ends
/// are a radius above the feet and a radius below the crown.
///
/// ```no_run
/// # use ludion_physics::{motor_capsule_segment, CharacterMotor, CharacterMotorState};
/// # fn demo(motor: &CharacterMotor, state: Option<&CharacterMotorState>) {
/// let (a, b, radius) = motor_capsule_segment(motor, state, [0.0; 3], [0.0, 0.0, 0.0, 1.0]);
/// # }
/// ```
pub fn motor_capsule_segment(
    motor: &CharacterMotor,
    state: Option<&super::CharacterMotorState>,
    feet: [f32; 3],
    rotation: [f32; 4],
) -> ([f32; 3], [f32; 3], f32) {
    let force = super::component::in_force(motor, state);
    match &force.lying {
        Some(lying) => lying_segment(lying, feet, yaw_of(rotation)),
        None => {
            let radius = force.motor.radius;
            let up = |y: f32| [feet[0], feet[1] + y, feet[2]];
            (up(radius), up(force.motor.height - radius), radius)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LYING: Lying = Lying {
        length: 1.7,
        radius: 0.17,
        offset: 0.35,
    };

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
        super::super::carry::rotate(q, v)
    }

    #[test]
    fn yaw_zero_lies_along_z_with_the_centre_ahead_on_minus_z() {
        let feet = [1.0, 2.0, 3.0];
        let centre = lying_centre(&LYING, feet, 0.0);
        assert!(near(centre, [1.0, 2.17, 3.0 - 0.35]));
        assert!(near(
            rotate(lying_rotation(0.0), [0.0, 1.0, 0.0]),
            [0.0, 0.0, 1.0]
        ));
        let (a, b, radius) = lying_segment(&LYING, feet, 0.0);
        let half = LYING.half_segment();
        assert!(near(a, [1.0, 2.17, centre[2] + half]));
        assert!(near(b, [1.0, 2.17, centre[2] - half]));
        assert_eq!(radius, 0.17);
    }

    #[test]
    fn yaw_ninety_lies_along_x() {
        let yaw = core::f32::consts::FRAC_PI_2;
        let axis = rotate(lying_rotation(yaw), [0.0, 1.0, 0.0]);
        assert!(
            near(axis, [-1.0, 0.0, 0.0]) || near(axis, [1.0, 0.0, 0.0]),
            "{axis:?}"
        );
        let centre = lying_centre(&LYING, [0.0; 3], yaw);
        assert!(near(centre, [-0.35, 0.17, 0.0]));
        let (a, b, _) = lying_segment(&LYING, [0.0; 3], yaw);
        assert!(a[2].abs() < 1e-5 && b[2].abs() < 1e-5);
        assert!((b[0] - a[0]).abs() > 1.0);
    }

    #[test]
    fn the_yaw_of_a_pose_drops_pitch_and_roll() {
        let yaw = 0.7;
        let (s, c) = sin_cos(yaw / 2.0);
        assert!((yaw_of([0.0, s, 0.0, c]) - yaw).abs() < 1e-6);
        // a pitch about X on top of the yaw leaves the twist alone.
        let (px, pw) = sin_cos(0.3);
        let pitched = [pw * 0.0 + px * c, px * 0.0 + pw * s, -px * s, pw * c];
        assert!((yaw_of(pitched) - yaw).abs() < 1e-5);
    }
}
