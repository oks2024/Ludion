//! The one proptest configuration for this crate's suites: a fixed seed and no failure
//! persistence, so a red run reproduces on every machine. A case a property finds becomes a named
//! regression test beside it.

use proptest::prelude::ProptestConfig;
use proptest::test_runner::RngSeed;

pub fn deterministic_config() -> ProptestConfig {
    ProptestConfig {
        rng_seed: RngSeed::Fixed(7919),
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}
