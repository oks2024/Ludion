//! `mirror:` and `base:` + `patches:` frames bake exactly like the hand-authored full grid.

use ludion_scene::check_sprite;
use ludion_sprite::{FrameImage, bake, bake_steps};

const HEADER: &str = "format: 1
kind: sprite
size:
  - 5
  - 3
palette:
  A:
    color: \"#ff0000\"
  B:
    color: \"#0000ff\"
frames:
";

const SHADE: &str = "effects:
  - op: shade
    light:
      - -1.0
      - -1.0
    strength: 0.5
";

fn frames(source: &str) -> Vec<(String, FrameImage)> {
    let doc = check_sprite(source).expect("the fixture sprite is valid");
    bake(&doc)
        .expect("the fixture sprite bakes")
        .into_iter()
        .map(|frame| (frame.name, frame.image))
        .collect()
}

fn image_of<'a>(frames: &'a [(String, FrameImage)], name: &str) -> &'a FrameImage {
    &frames.iter().find(|(n, _)| n == name).expect("frame").1
}

const MIRRORED: &str = "  half:
    mirror: xy
    rows:
      - \"AB.\"
      - \".AB\"
  full:
    rows:
      - \"AB.BA\"
      - \".ABA.\"
      - \"AB.BA\"
";

#[test]
fn a_mirror_xy_frame_bakes_equal_to_the_hand_authored_full_frame() {
    let source = format!("{HEADER}{MIRRORED}");
    let baked = frames(&source);
    assert_eq!(image_of(&baked, "half"), image_of(&baked, "full"));
}

#[test]
fn effects_run_on_the_full_frame() {
    let source = format!("{HEADER}{MIRRORED}{SHADE}");
    let baked = frames(&source);
    let (half, full) = (image_of(&baked, "half"), image_of(&baked, "full"));
    assert_eq!(half, full);
    let width = half.width as usize;
    let mirrored_back: Vec<[f32; 4]> = half
        .texels
        .chunks(width)
        .flat_map(|row| row.iter().rev().copied())
        .collect();
    assert_ne!(
        half.texels, mirrored_back,
        "a directional shade is not left-right symmetric"
    );
}

#[test]
fn a_patched_frame_bakes_equal_to_its_hand_authored_result() {
    let source = format!(
        "{HEADER}  base:
    mirror: x
    rows:
      - \"AB.\"
      - \"BA.\"
      - \"AB.\"
  hit:
    base: base
    patches:
      - at:
          - 1
          - 1
        rows:
          - \" .B\"
      - at:
          - 0
          - 2
        rows:
          - \"B\"
  expected:
    rows:
      - \"AB.BA\"
      - \"BA.BB\"
      - \"BB.BA\"
"
    );
    let baked = frames(&source);
    assert_eq!(image_of(&baked, "hit"), image_of(&baked, "expected"));
}

#[test]
fn bake_steps_starts_from_the_expanded_paint() {
    let source = format!(
        "{HEADER}  half:
    mirror: xy
    rows:
      - \"AB.\"
      - \".AB\"
  full:
    rows:
      - \"AB.BA\"
      - \".ABA.\"
      - \"AB.BA\"
{SHADE}"
    );
    let doc = check_sprite(&source).expect("valid");
    let stepped = bake_steps(&doc).expect("bakes");
    let find = |name: &str| stepped.iter().find(|f| f.name == name).expect("frame");
    assert_eq!(find("half").paint, find("full").paint);
    assert_eq!(find("half").steps.len(), 1);
}
