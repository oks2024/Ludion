//! The stepped bake: per-effect images that end exactly where `bake()` ends.

use ludion_scene::check_sprite;
use ludion_sprite::{bake, bake_steps};

const FILTERED: &str = r#"format: 1
kind: sprite
size:
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
frames:
  lit:
    rows:
      - "A"
  dark:
    rows:
      - "A"
effects:
  - op: gradient
    frames:
      - dark
    from:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
    to:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
"#;
const NO_EFFECTS: &str = r#"format: 1
kind: sprite
size:
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 1.0
      - 1.0
      - 1.0
frames:
  lit:
    rows:
      - "A"
  dark:
    rows:
      - "A"
"#;

#[test]
fn the_last_step_of_every_frame_equals_the_bake() {
    let doc = check_sprite(FILTERED).expect("the fixture is valid");
    let baked = bake(&doc).expect("bakes");
    let stepped = bake_steps(&doc).expect("bakes");
    assert_eq!(baked.len(), stepped.len());
    for (frame, steps) in baked.iter().zip(&stepped) {
        assert_eq!(frame.name, steps.name);
        assert_eq!(frame.pivot, steps.pivot);
        let last = steps.steps.last().map_or(&steps.paint, |step| &step.image);
        assert_eq!(&frame.image, last);
    }
}

#[test]
fn a_filtered_effect_is_absent_from_the_other_frames_steps() {
    let doc = check_sprite(FILTERED).expect("the fixture is valid");
    let stepped = bake_steps(&doc).expect("bakes");
    assert!(stepped[0].steps.is_empty(), "lit is not in the filter");
    assert_eq!(stepped[1].steps.len(), 1);
    assert_eq!(stepped[1].steps[0].effect, 0);
}

#[test]
fn a_document_without_effects_has_no_steps() {
    let doc = check_sprite(NO_EFFECTS).expect("the fixture is valid");
    for frame in bake_steps(&doc).expect("bakes") {
        assert!(frame.steps.is_empty());
    }
}
