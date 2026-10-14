//! The facade names what a game needs without a second dependency: `WorldHash` (the type
//! `replay_hashes` returns) and the Euler/quaternion pair.

use ludion::math::{euler_xyz_degrees_to_quat, quat_to_euler_xyz_degrees};
use ludion::prelude::WorldHash;

#[test]
fn world_hash_is_nameable_through_the_prelude() {
    let hashes: Vec<WorldHash> = Vec::new();
    assert!(hashes.is_empty());
}

#[test]
fn euler_helpers_round_trip_through_the_facade() {
    let quat = euler_xyz_degrees_to_quat([30.0, 20.0, 10.0]);
    let back = quat_to_euler_xyz_degrees(quat);
    for (a, b) in back.iter().zip([30.0, 20.0, 10.0]) {
        assert!((a - b).abs() < 1e-3, "{back:?}");
    }
}
