//! The facade names what a game needs without a second dependency: `WorldHash` (the type
//! `replay_hashes` returns), the Euler/quaternion pair, the seeded RNG, the deterministic collections
//! and the physics query types.

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

#[test]
fn rng_types_are_nameable_through_the_prelude() {
    use ludion::prelude::{DeterministicRng, WorldSeed};

    let mut rng = DeterministicRng::new(WorldSeed(7).0);
    let _ = rng.next_u64();
}

#[test]
fn deterministic_collections_are_nameable_through_the_facade() {
    use ludion::collections::{StableMap, StableSet};

    let mut map: StableMap<u32, u32> = StableMap::default();
    map.insert(1, 2);
    let mut set: StableSet<u32> = StableSet::default();
    set.insert(1);
    assert_eq!((map.len(), set.len()), (1, 1));
}

#[test]
fn physics_query_types_are_nameable_through_the_prelude() {
    use ludion::prelude::{QueryLayers, Sweep, SweepFilter, SweepHit};

    let _ = QueryLayers::default();
    let _ = Sweep::new([0.0; 3], [1.0, 0.0, 0.0], 0.25);
    let _: Option<SweepFilter> = None;
    let _: Option<SweepHit> = None;
}
