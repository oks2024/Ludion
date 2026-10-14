//! The encoders are pinned with `=`, and `COOKER_VERSION` is what invalidates cooked stores when
//! they move. This ties the three together.

use ludion_cook::{ENCODER_VERSIONS, ENCODER_VERSIONS_FOR};

const MOVED: &str =
    "encoder or settings moved: bump COOKER_VERSION and update ENCODER_VERSIONS(_FOR)";

fn locked_version(lock: &str, name: &str) -> String {
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line == format!("name = \"{name}\"") {
            let version = lines.next().expect("version line after name");
            return version
                .trim_start_matches("version = ")
                .trim_matches('"')
                .to_string();
        }
    }
    panic!("`{name}` is not in Cargo.lock");
}

#[test]
fn encoder_versions_match_the_lock() {
    let lock =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")).unwrap();
    for (name, version) in ENCODER_VERSIONS {
        assert_eq!(locked_version(&lock, name), version, "{name}: {MOVED}");
    }
}

#[test]
fn cooker_version_matches_the_pin() {
    assert_eq!(
        ludion_assets::cooked::COOKER_VERSION,
        ENCODER_VERSIONS_FOR,
        "{MOVED}"
    );
}
