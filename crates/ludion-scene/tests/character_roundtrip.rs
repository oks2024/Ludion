//! Canonical-form and diagnostic properties for character documents (`kind: character`,
//! `Plan/animation-clips.md` decisions 10–12), the character analog of `assembly_roundtrip.rs`:
//! canonical byte-idempotence, one spelling per meaning, and every `L1199`/`L1200`/`L1198` rule
//! keyed on its JSON Pointer, the cross-file ones included.

use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{
    AnimationDocument, check_animation, check_character, check_character_clips, emit_character,
    parse_character, validate_character,
};

const CANONICAL: &str = "\
format: 1
kind: character
name: goblin
mesh: goblin.mesh.yaml
clips:
  idle:
    clip: clips/idle.anim.yaml
  run:
    clip: clips/run.anim.yaml
    speed: 1.5
    events:
      - t: 0.2
        name: footstep
      - t: 0.5
        name: footstep
";

fn has(diagnostics: &[Diagnostic], code: &str, pointer: &str) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code.as_str() == code && d.path.as_deref() == Some(pointer))
}

fn structure_errors(source: &str) -> Vec<Diagnostic> {
    parse_character(source).expect_err("the source is structurally invalid")
}

fn data_errors(source: &str) -> Vec<Diagnostic> {
    let doc = parse_character(source).expect("the source is well-shaped");
    validate_character(&doc)
}

fn clip(events: &str, duration: f64, target: &str) -> AnimationDocument {
    let source = format!(
        "format: 1\nkind: animation\nduration: {duration}\nloop: true\n{events}tracks:\n  {target}:\n    rotation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n      - t: 0.1\n        v:\n          - 10.0\n          - 0.0\n          - 0.0\n"
    );
    check_animation(&source).expect("the clip is valid")
}

#[test]
fn canonical_text_is_a_fixed_point() {
    let doc = check_character(CANONICAL).expect("valid");
    assert_eq!(emit_character(&doc), CANONICAL);
    let again = check_character(&emit_character(&doc)).expect("re-parses");
    assert_eq!(again, doc);
}

#[test]
fn speed_one_is_dropped_and_clips_sort() {
    let source = "\
format: 1
kind: character
mesh: m.mesh.yaml
clips:
  walk:
    clip: w.anim.yaml
    speed: 1.0
  jump:
    clip: j.anim.yaml
";
    let doc = check_character(source).expect("valid");
    assert_eq!(
        emit_character(&doc),
        "\
format: 1
kind: character
mesh: m.mesh.yaml
clips:
  jump:
    clip: j.anim.yaml
  walk:
    clip: w.anim.yaml
"
    );
}

#[test]
fn empty_clips_canonicalize_to_absence() {
    let doc = check_character("format: 1\nkind: character\nmesh: m.mesh.yaml\nclips: {}\n")
        .expect("valid");
    assert_eq!(
        emit_character(&doc),
        "format: 1\nkind: character\nmesh: m.mesh.yaml\n"
    );
}

#[test]
fn structure_errors_carry_pointers() {
    let missing_mesh = structure_errors("format: 1\nkind: character\n");
    assert!(has(&missing_mesh, "L1199", ""));

    let unknown = structure_errors("format: 1\nkind: character\nmesh: m\nmash: x\n");
    let diagnostic = unknown
        .iter()
        .find(|d| d.path.as_deref() == Some("/mash"))
        .expect("unknown key");
    assert_eq!(diagnostic.code.as_str(), "L1199");
    assert!(diagnostic.help.as_deref().unwrap_or("").contains("mesh"));

    let no_clip =
        structure_errors("format: 1\nkind: character\nmesh: m\nclips:\n  run:\n    speed: 2.0\n");
    assert!(has(&no_clip, "L1199", "/clips/run"));

    let bad_key = structure_errors(
        "format: 1\nkind: character\nmesh: m\nclips:\n  run:\n    clip: c\n    spead: 2.0\n",
    );
    assert!(has(&bad_key, "L1199", "/clips/run/spead"));

    let bad_speed = structure_errors(
        "format: 1\nkind: character\nmesh: m\nclips:\n  run:\n    clip: c\n    speed: fast\n",
    );
    assert!(has(&bad_speed, "L1199", "/clips/run/speed"));
}

#[test]
fn empty_events_list_is_l1198() {
    let errors = structure_errors(
        "format: 1\nkind: character\nmesh: m\nclips:\n  run:\n    clip: c\n    events: []\n",
    );
    assert!(has(&errors, "L1198", "/clips/run/events"));
}

#[test]
fn blank_paths_and_bad_speed_are_l1200() {
    let errors = data_errors(
        "format: 1\nkind: character\nmesh: \"\"\nclips:\n  run:\n    clip: \"\"\n    speed: 0.0\n  walk:\n    clip: w\n    speed: -1.0\n",
    );
    assert!(has(&errors, "L1200", "/mesh"));
    assert!(has(&errors, "L1200", "/clips/run/clip"));
    assert!(has(&errors, "L1200", "/clips/run/speed"));
    assert!(has(&errors, "L1200", "/clips/walk/speed"));
}

#[test]
fn absolute_drive_or_device_paths_are_l1200_and_a_parent_is_fine() {
    for bad in ["C:/x", "/x", "sub/nul.yaml"] {
        let mesh = format!(
            "format: 1
kind: character
mesh: {bad}
clips:
  run:
    clip: c
"
        );
        assert!(has(&data_errors(&mesh), "L1200", "/mesh"), "{bad}");
        let clip = format!(
            "format: 1
kind: character
mesh: m
clips:
  run:
    clip: {bad}
"
        );
        assert!(
            has(&data_errors(&clip), "L1200", "/clips/run/clip"),
            "{bad}"
        );
    }
    let ok = "format: 1
kind: character
mesh: ../m.mesh.yaml
clips:
  run:
    clip: ../c.anim.yaml
";
    assert!(data_errors(ok).is_empty());
}

#[test]
fn overlay_event_data_is_l1198() {
    let errors = data_errors(
        "format: 1\nkind: character\nmesh: m\nclips:\n  run:\n    clip: c\n    events:\n      - t: 0.5\n        name: a\n      - t: 0.5\n        name: \"\"\n",
    );
    assert!(has(&errors, "L1198", "/clips/run/events/1/t"));
    assert!(has(&errors, "L1198", "/clips/run/events/1/name"));

    let negative = data_errors(
        "format: 1\nkind: character\nmesh: m\nclips:\n  run:\n    clip: c\n    events:\n      - t: -0.1\n        name: a\n",
    );
    assert!(has(&negative, "L1198", "/clips/run/events/0/t"));
}

#[test]
fn overlay_event_named_end_is_l1198() {
    let errors = data_errors(
        "format: 1
kind: character
mesh: m
clips:
  run:
    clip: c
    events:
      - t: 0.5
        name: end
",
    );
    assert!(has(&errors, "L1198", "/clips/run/events/0/name"));
}

#[test]
fn clips_check_flags_overlay_against_own_events() {
    let doc = check_character(CANONICAL).expect("valid");
    let clips = [
        clip("", 1.0, "hip"),
        clip("events:\n  - t: 0.1\n    name: x\n", 1.0, "hip"),
    ];
    let errors = check_character_clips(&doc, &clips, &["hip"]);
    assert_eq!(errors.len(), 1);
    assert!(has(&errors, "L1200", "/clips/run/events"));
}

#[test]
fn clips_check_bounds_overlay_events_by_duration() {
    let doc = check_character(CANONICAL).expect("valid");
    let clips = [clip("", 1.0, "hip"), clip("", 0.4, "hip")];
    let errors = check_character_clips(&doc, &clips, &["hip"]);
    assert_eq!(errors.len(), 1);
    assert!(has(&errors, "L1198", "/clips/run/events/1/t"));
}

#[test]
fn clips_check_refuses_overlay_event_at_a_looping_clips_duration() {
    let doc = check_character(CANONICAL).expect("valid");
    // `clip` builds a looping clip, so the overlay's `t: 0.5` is the seam.
    let clips = [clip("", 1.0, "hip"), clip("", 0.5, "hip")];
    let errors = check_character_clips(&doc, &clips, &["hip"]);
    assert_eq!(errors.len(), 1);
    assert!(has(&errors, "L1198", "/clips/run/events/1/t"));
}

#[test]
fn clips_check_flags_unknown_joint_with_nearest() {
    let doc = check_character(CANONICAL).expect("valid");
    let clips = [clip("", 1.0, "hipp"), clip("", 1.0, "hip")];
    let errors = check_character_clips(&doc, &clips, &["hip", "knee"]);
    assert_eq!(errors.len(), 1);
    assert!(has(&errors, "L1200", "/clips/idle/clip"));
    let help = errors[0].help.as_deref().unwrap_or("");
    assert!(help.contains("`hip`"), "{help}");
}

#[test]
fn clean_character_has_no_cross_file_findings() {
    let doc = check_character(CANONICAL).expect("valid");
    let clips = [clip("", 1.0, "hip"), clip("", 1.0, "hip")];
    assert!(check_character_clips(&doc, &clips, &["hip"]).is_empty());
}
