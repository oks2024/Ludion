//! Golden-image test: renders `scenes/main.yaml` and compares it against a committed golden PNG.
//!
//! Green out of the box: while `goldens/` holds no golden at all it prints how to bless one and
//! passes without asserting; once any golden is committed, a missing one fails (`L2201`). Blessing is
//! explicit and goes through *this test* — `LUDION_BLESS=1 LUDION_BLESS_ANY_ADAPTER=1 cargo test` —
//! never through `ludion test --bless` (the CLI validates against the built-in registry only, so it
//! cannot render a scene that names this game's components). The second variable declares this
//! machine's GPU is your pinned one (`L2206` otherwise). Goldens are per-OS
//! (`goldens/<os>/main.png`, blessed on each pinned platform).
//!
//! The recipe is [`ludion_testkit::GoldenTest::single`]; see its doc comment for the details every
//! game's golden test shares.

use std::path::PathBuf;

use ludion_testkit::GoldenTest;

fn scene() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join("main.yaml")
}

fn goldens_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("goldens")
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("testkit")
}

#[test]
fn main_scene_matches_its_golden() {
    // The game's registry, not just the built-ins: the scene names this game's components.
    GoldenTest::single(
        scene(),
        goldens_root(),
        "main",
        {{name_ident}}::registry,
        &out_dir(),
    );
}
