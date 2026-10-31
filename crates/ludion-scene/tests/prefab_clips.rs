//! A prefab's `clips:` map: parse, canonical emit, validation (`L1226`, `L1311`) and rebase.

mod common;

use ludion_core::codes::code;
use ludion_ecs::Registry;
use ludion_scene::{check_prefab, emit_prefab, parse_prefab};

fn registry() -> Registry {
    let mut registry = common::registry();
    registry.register::<ludion_ecs::Parent>("Parent").unwrap();
    registry
}

const WITH_CLIPS: &str = "\
format: 1
kind: prefab
name: hinge
clips:
  open: clips/open.anim.yaml
  snap: clips/snap.anim.yaml
entities:
  - name: base
    components: {}
";

const WITHOUT_CLIPS: &str = "\
format: 1
kind: prefab
name: hinge
entities:
  - name: base
    components: {}
";

#[test]
fn clips_round_trip_byte_identically() {
    let doc = check_prefab(WITH_CLIPS, &registry()).unwrap();
    assert_eq!(doc.clips.len(), 2);
    assert_eq!(doc.clips[0].name, "open");
    assert_eq!(emit_prefab(&doc, &registry()), WITH_CLIPS);
}

#[test]
fn clips_emit_sorted_by_short_name() {
    let mut doc = check_prefab(WITH_CLIPS, &registry()).unwrap();
    doc.clips.reverse();
    assert_eq!(emit_prefab(&doc, &registry()), WITH_CLIPS);
}

#[test]
fn a_prefab_without_clips_emits_unchanged() {
    let doc = check_prefab(WITHOUT_CLIPS, &registry()).unwrap();
    assert!(doc.clips.is_empty());
    assert_eq!(emit_prefab(&doc, &registry()), WITHOUT_CLIPS);
}

#[test]
fn an_empty_clips_map_canonicalizes_away() {
    let source = WITHOUT_CLIPS.replace("entities:", "clips: {}\nentities:");
    let doc = check_prefab(&source, &registry()).unwrap();
    assert_eq!(emit_prefab(&doc, &registry()), WITHOUT_CLIPS);
}

#[test]
fn a_non_mapping_clips_is_l1226() {
    let source = WITHOUT_CLIPS.replace("entities:", "clips: open\nentities:");
    let errors = parse_prefab(&source).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|d| d.code == code::PREFAB_DOCUMENT_STRUCTURE
                && d.path.as_deref() == Some("/clips")),
        "{errors:?}"
    );
}

#[test]
fn a_rooted_clip_file_is_l1311() {
    let source = WITH_CLIPS.replace("clips/open.anim.yaml", "/abs/open.anim.yaml");
    let errors = check_prefab(&source, &registry()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|d| d.code == code::PREFAB_ASSET_PATH && d.path.as_deref() == Some("/clips/open")),
        "{errors:?}"
    );
}

#[test]
fn an_empty_clip_file_is_l1226() {
    let source = WITH_CLIPS.replace("clips/open.anim.yaml", "''");
    let errors = check_prefab(&source, &registry()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|d| d.code == code::PREFAB_DOCUMENT_STRUCTURE),
        "{errors:?}"
    );
}

#[test]
fn rebase_rewrites_clip_files_to_content_root_keys() {
    let mut doc = check_prefab(WITH_CLIPS, &registry()).unwrap();
    doc.rebase("props/hinge/model.prefab.yaml", &registry());
    assert_eq!(doc.clips[0].file, "props/hinge/clips/open.anim.yaml");
    assert_eq!(doc.clips[1].file, "props/hinge/clips/snap.anim.yaml");
}
