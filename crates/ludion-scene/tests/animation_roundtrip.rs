//! Round-trip and canonical-form properties for animation clip documents (`anims/*.anim.yaml`,
//! `Plan/voxel-animation.md` decision 8) — the clip analog of `part_roundtrip.rs`:
//!
//! - **value round trip**: clip → emit → check loses nothing;
//! - **emit idempotence**: `emit_animation(parse_animation(emit_animation(d)))` is *byte-equal* to
//!   `emit_animation(d)`;
//! - the emitted text is LF-only and validates cleanly;
//! - a hand-authored fixture pins the exact canonical text (the plan's `humanoid-walk` sketch,
//!   translated from its ratified flow-style spelling into the block form the strict-YAML subset
//!   actually accepts — see `crates/ludion-scene/src/animation.rs`'s module docs);
//! - every `L1141`/`L1142` misshape is reported with a JSON Pointer.

use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{
    AnimationDocument, AnimationEvent, AnimationTrack, Ease, Keyframe, check_animation,
    emit_animation, parse_animation, validate_animation,
};
use proptest::prelude::*;
#[path = "support/deterministic.rs"]
mod deterministic;
use deterministic::deterministic_config;

// --- Strategies -------------------------------------------------------------------------------

/// The part-name pool, deliberately including a hyphenated name (`scalar_string` still emits it
/// plain — hyphens are identifier-safe).
const PART_POOL: [&str; 5] = ["torso", "upper-arm-l", "upper-arm-r", "head", "leg"];

/// Keyframe `t`s are drawn from this many even subdivisions of `duration` (`0, duration/8, ...,
/// duration`) rather than arbitrary floats: distinct grid indices multiplied by a fixed positive
/// `duration` stay strictly ordered and inside `[0, duration]` by construction (IEEE-754
/// multiplication by a value in `[0, 1]` never rounds outside that interval), which is what lets
/// [`build_channel`] guarantee strictly increasing, in-range times without a monotonicity retry
/// loop.
const TIME_GRID: u32 = 8;

/// One raw keyframe candidate: a `TIME_GRID` index (not yet a time), a value, and an easing.
type RawKeyframe = (u32, [f64; 3], Ease);

fn ease_strategy() -> impl Strategy<Value = Ease> {
    prop_oneof![Just(Ease::Linear), Just(Ease::Smooth), Just(Ease::Step)]
}

fn v_strategy() -> impl Strategy<Value = [f64; 3]> {
    (-100.0f64..100.0, -100.0f64..100.0, -100.0f64..100.0).prop_map(|(x, y, z)| [x, y, z])
}

fn keyframe_raw() -> impl Strategy<Value = RawKeyframe> {
    (0..=TIME_GRID, v_strategy(), ease_strategy())
}

/// Raw material for one channel: three candidates, deduped by grid index and sorted ascending by
/// [`build_channel`] — always at least one survives, so a channel built from this is never empty.
fn raw_channel() -> impl Strategy<Value = Vec<RawKeyframe>> {
    proptest::collection::vec(keyframe_raw(), 3)
}

/// Turns [`raw_channel`] output into actual keyframes against a concrete `duration`: dedupe by
/// grid index (first occurrence wins), sort ascending, map index `i` to `t = duration * i / 8`.
fn build_channel(duration: f64, raw: Vec<RawKeyframe>) -> Vec<Keyframe> {
    let mut seen: Vec<u32> = Vec::new();
    let mut kept: Vec<RawKeyframe> = Vec::new();
    for (index, v, ease) in raw {
        if seen.contains(&index) {
            continue;
        }
        seen.push(index);
        kept.push((index, v, ease));
    }
    kept.sort_by_key(|(index, _, _)| *index);
    kept.into_iter()
        .map(|(index, v, ease)| Keyframe {
            t: duration * (f64::from(index) / f64::from(TIME_GRID)),
            v,
            ease,
        })
        .collect()
}

/// One [`PART_POOL`] slot's raw material: which channel(s) to animate (`0` rotation-only, `1`
/// translation-only, `2` both) plus enough raw keyframes for each, generated unconditionally so
/// the strategy stays fixed-arity (unused raw data for a channel this slot doesn't animate is
/// simply discarded in [`animation_doc`]).
/// The scale channel is optional per slot: `None` leaves it absent, `Some` supplies raw keyframes
/// whose values are mapped into `0.1..4.0` so they satisfy the positive-scale rule.
type Slot = (
    u8,
    Vec<RawKeyframe>,
    Vec<RawKeyframe>,
    Option<Vec<RawKeyframe>>,
);

fn slot() -> impl Strategy<Value = Slot> {
    (
        0u8..3,
        raw_channel(),
        raw_channel(),
        proptest::option::of(raw_channel()),
    )
}

/// Event names come from a small pool; event times are distinct [`TIME_GRID`] indices.
const EVENT_POOL: [&str; 3] = ["spin_active", "footstep", "hit"];

fn events_strategy() -> impl Strategy<Value = Vec<u32>> {
    proptest::collection::btree_set(0..=TIME_GRID, 0..4).prop_map(|set| set.into_iter().collect())
}

/// A structurally consistent [`AnimationDocument`]: a non-empty, distinct subset of [`PART_POOL`]
/// (bitmask over 5 slots — the `voxel_roundtrip.rs` glyph-mask technique), each with at least one
/// non-empty channel of strictly increasing, in-range keyframes — exactly the properties
/// [`validate_animation`] enforces. `tracks` comes out pre-sorted by part name, matching what
/// [`parse_animation`] itself produces, so the round-trip equality checks compare like with like.
fn animation_doc() -> impl Strategy<Value = AnimationDocument> {
    (
        1u32..32,
        0.1f64..10.0,
        any::<bool>(),
        proptest::option::of("[a-zA-Z0-9 ]{0,6}"),
        proptest::collection::vec(slot(), PART_POOL.len()),
        events_strategy(),
    )
        .prop_map(|(mask, duration, looped, name, slots, event_grid)| {
            let mut tracks: Vec<AnimationTrack> = PART_POOL
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(i, part)| {
                    let (which, rotation_raw, translation_raw, scale_raw) = slots[i].clone();
                    let rotation = if which == 1 {
                        Vec::new()
                    } else {
                        let mut channel = build_channel(duration, rotation_raw);
                        // Every axis within ±25 degrees keeps each key within 75 degrees of the
                        // identity, so any two keys' quaternion dot stays positive: neither
                        // `L1142` rule (per axis, combined) can fire.
                        for keyframe in &mut channel {
                            keyframe.v = keyframe.v.map(|c| c * 0.25);
                        }
                        channel
                    };
                    let translation = if which == 0 {
                        Vec::new()
                    } else {
                        build_channel(duration, translation_raw)
                    };
                    let scale = scale_raw
                        .map(|raw| {
                            let mut channel = build_channel(duration, raw);
                            for keyframe in &mut channel {
                                keyframe.v = keyframe.v.map(|c| 0.1 + (c + 100.0) / 200.0 * 3.9);
                            }
                            channel
                        })
                        .unwrap_or_default();
                    AnimationTrack {
                        target: part.to_string(),
                        rotation,
                        translation,
                        scale,
                    }
                })
                .collect();
            tracks.sort_by(|a, b| a.target.cmp(&b.target));
            let events = event_grid
                .into_iter()
                // A loop's end is its start, so no event sits on the last grid point.
                .filter(|&index| !looped || index < TIME_GRID)
                .enumerate()
                .map(|(i, index)| AnimationEvent {
                    t: duration * (f64::from(index) / f64::from(TIME_GRID)),
                    name: EVENT_POOL[i % EVENT_POOL.len()].to_string(),
                })
                .collect();
            AnimationDocument {
                name,
                generated_from: None,
                duration,
                looped,
                events,
                tracks,
            }
        })
}

// --- Properties -------------------------------------------------------------------------------

proptest! {
    #![proptest_config(deterministic_config())]
    #[test]
    fn emit_then_check_loses_nothing(doc in animation_doc()) {
        let text = emit_animation(&doc);
        let checked = check_animation(&text).expect("canonical output must check clean");
        prop_assert_eq!(checked, doc);
    }

    #[test]
    fn emit_is_idempotent_byte_for_byte(doc in animation_doc()) {
        let first = emit_animation(&doc);
        let reparsed = parse_animation(&first).expect("canonical output must parse cleanly");
        let second = emit_animation(&reparsed);
        prop_assert_eq!(&second, &first);
        let third = emit_animation(&parse_animation(&second).unwrap());
        prop_assert_eq!(&third, &first);
    }

    #[test]
    fn emitted_text_is_lf_only_and_validates(doc in animation_doc()) {
        let text = emit_animation(&doc);
        prop_assert!(!text.contains('\r'), "canonical output must use LF only");
        prop_assert!(text.ends_with('\n'), "canonical output ends with a newline");
        let parsed = parse_animation(&text).expect("canonical output must parse cleanly");
        let diagnostics = validate_animation(&parsed);
        prop_assert!(
            diagnostics.is_empty(),
            "canonical output of a consistent clip must validate: {:?}",
            diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }
}

// --- Deterministic golden shape ---------------------------------------------------------------

/// Pins the exact canonical text of `Plan/voxel-animation.md`'s ratified `humanoid-walk` sketch,
/// translated from its flow-style keyframes (`{ t: 0.0, v: [25, 0, 0] }`) into the block form —
/// the strict-YAML subset rejects non-empty flow collections (`L1005`), so the plan's exact
/// spelling never parses; this is what an agent regenerating that file from the plan's clip must
/// actually write. Field order is `format, kind, name, duration, loop, tracks`; within the track,
/// keyframes stay in authored (ascending-`t`) order; the middle keyframe's `ease: smooth` is the
/// only explicit `ease` — `linear` never appears in canonical text.
#[test]
fn canonical_form_golden() {
    let source = "\
format: 1
kind: animation
name: humanoid-walk
duration: 0.6
loop: true
tracks:
  upper-leg-l:
    rotation:
      - t: 0.0
        v:
          - 25.0
          - 0.0
          - 0.0
      - t: 0.3
        v:
          - -25.0
          - 0.0
          - 0.0
        ease: smooth
      - t: 0.6
        v:
          - 25.0
          - 0.0
          - 0.0
";
    let clip = parse_animation(source).expect("clip should parse");
    assert!(validate_animation(&clip).is_empty());
    assert_eq!(clip.name.as_deref(), Some("humanoid-walk"));
    assert_eq!(clip.duration, 0.6);
    assert!(clip.looped);
    assert_eq!(clip.tracks.len(), 1);
    assert_eq!(clip.tracks[0].target, "upper-leg-l");
    assert_eq!(clip.tracks[0].rotation.len(), 3);
    assert!(clip.tracks[0].translation.is_empty());
    assert_eq!(clip.tracks[0].rotation[1].ease, Ease::Smooth);
    let canonical = emit_animation(&clip);
    assert_eq!(canonical, source);
    // Emission is a fixed point.
    assert_eq!(
        emit_animation(&parse_animation(&canonical).unwrap()),
        canonical
    );
}

/// Tracks emit sorted by target whatever their stored order, so a producer that builds them in
/// skeleton order writes the same bytes a parse-and-emit round trip does.
#[test]
fn tracks_emit_sorted_by_target_whatever_the_stored_order() {
    let source = "\
format: 1
kind: animation
duration: 1.0
loop: false
tracks:
  head:
    translation:
      - t: 0.0
        v:
          - 0.0
          - 1.0
          - 0.0
  pelvis:
    translation:
      - t: 0.0
        v:
          - 0.0
          - 0.5
          - 0.0
";
    let sorted = parse_animation(source).expect("clip should parse");
    let mut reversed = sorted.clone();
    reversed.tracks.reverse();
    assert_eq!(reversed.tracks[0].target, "pelvis");
    let canonical = emit_animation(&reversed);
    assert_eq!(canonical, source);
    assert_eq!(emit_animation(&sorted), canonical);
    assert_eq!(
        emit_animation(&parse_animation(&canonical).unwrap()),
        canonical
    );
}

/// An explicit `ease: linear` parses to [`Ease::Linear`] just like an absent `ease`, and canonical
/// emission collapses both to the key's absence — one spelling per meaning.
#[test]
fn explicit_ease_linear_canonicalizes_to_absence() {
    let source = "\
format: 1
kind: animation
duration: 1.0
loop: false
tracks:
  torso:
    translation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
        ease: linear
";
    let clip = parse_animation(source).expect("clip should parse");
    assert_eq!(clip.tracks[0].translation[0].ease, Ease::Linear);
    let canonical = emit_animation(&clip);
    assert!(!canonical.contains("ease"));
    assert_eq!(
        canonical,
        "format: 1\nkind: animation\nduration: 1.0\nloop: false\ntracks:\n  torso:\n    translation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n"
    );
}

/// A clip with no `name` and no tracks emits the minimal flow-collection shape.
#[test]
fn minimal_clip_uses_flow_empty_tracks() {
    let doc = AnimationDocument {
        duration: 1.0,
        tracks: Vec::new(),
        ..Default::default()
    };
    assert_eq!(
        emit_animation(&doc),
        "format: 1\nkind: animation\nduration: 1.0\nloop: false\ntracks: {}\n"
    );
}

// --- L1141 structure -----------------------------------------------------------------------------

fn errors_for(source: &str) -> Vec<Diagnostic> {
    check_animation(source).expect_err("expected the clip to be rejected")
}

fn has_code_at(diagnostics: &[Diagnostic], code: &str, pointer: &str) -> bool {
    diagnostics
        .iter()
        .any(|d| d.code.as_str() == code && d.path.as_deref() == Some(pointer))
}

const MINIMAL_TRACK: &str = "\
format: 1
kind: animation
duration: 1.0
loop: false
tracks:
  torso:
    translation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
";

#[test]
fn empty_document_is_l1141() {
    assert!(errors_for("").iter().any(|d| d.code.as_str() == "L1141"));
}

#[test]
fn wrong_kind_is_l1141() {
    let source = MINIMAL_TRACK.replace("kind: animation", "kind: part");
    assert!(has_code_at(&errors_for(&source), "L1141", "/kind"));
}

#[test]
fn unknown_top_level_key_is_l1141() {
    let source = MINIMAL_TRACK.replace("duration: 1.0\n", "duration: 1.0\nspeed: 2.0\n");
    assert!(has_code_at(&errors_for(&source), "L1141", "/speed"));
}

#[test]
fn missing_duration_is_l1141() {
    let source = "format: 1\nkind: animation\nloop: false\ntracks:\n  torso:\n    translation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n";
    assert!(
        errors_for(source)
            .iter()
            .any(|d| d.code.as_str() == "L1141" && d.path.as_deref() == Some(""))
    );
}

#[test]
fn missing_loop_is_l1141() {
    let source = "format: 1\nkind: animation\nduration: 1.0\ntracks:\n  torso:\n    translation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n";
    assert!(
        errors_for(source)
            .iter()
            .any(|d| d.code.as_str() == "L1141" && d.path.as_deref() == Some(""))
    );
}

#[test]
fn missing_tracks_is_l1141() {
    let source = "format: 1\nkind: animation\nduration: 1.0\nloop: false\n";
    assert!(
        errors_for(source)
            .iter()
            .any(|d| d.code.as_str() == "L1141" && d.path.as_deref() == Some(""))
    );
}

#[test]
fn unknown_track_key_is_l1141() {
    let source = MINIMAL_TRACK.replace("  torso:\n", "  torso:\n    skew:\n      - t: 0.0\n");
    assert!(has_code_at(
        &errors_for(&source),
        "L1141",
        "/tracks/torso/skew"
    ));
}

#[test]
fn unknown_keyframe_key_is_l1141() {
    let source = MINIMAL_TRACK.replace("        v:\n", "        speed: 1.0\n        v:\n");
    assert!(has_code_at(
        &errors_for(&source),
        "L1141",
        "/tracks/torso/translation/0/speed"
    ));
}

#[test]
fn v_arity_two_is_l1141() {
    let source = MINIMAL_TRACK.replace(
        "        v:\n          - 0.0\n          - 0.0\n          - 0.0\n",
        "        v:\n          - 0.0\n          - 0.0\n",
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1141",
        "/tracks/torso/translation/0/v"
    ));
}

#[test]
fn bad_ease_value_is_l1141_with_suggestion() {
    let source = MINIMAL_TRACK.replace(
        "        v:\n          - 0.0\n          - 0.0\n          - 0.0\n",
        "        v:\n          - 0.0\n          - 0.0\n          - 0.0\n        ease: smoot\n",
    );
    let diagnostics = errors_for(&source);
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/tracks/torso/translation/0/ease"))
        .expect("an /ease diagnostic");
    assert_eq!(d.code.as_str(), "L1141");
    assert_eq!(d.help.as_deref(), Some("nearest match is `smooth`"));
}

// --- L1142 data consistency (each with a JSON Pointer assertion) -------------------------------

#[test]
fn non_monotonic_t_is_l1142() {
    let source = MINIMAL_TRACK.replace(
        "      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n",
        "      - t: 0.5\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n      - t: 0.2\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n",
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1142",
        "/tracks/torso/translation/1/t"
    ));
}

#[test]
fn t_past_duration_is_l1142() {
    let source = MINIMAL_TRACK.replace("t: 0.0", "t: 2.0");
    assert!(has_code_at(
        &errors_for(&source),
        "L1142",
        "/tracks/torso/translation/0/t"
    ));
}

#[test]
fn empty_channel_is_l1142() {
    let source = MINIMAL_TRACK.replace(
        "    translation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n",
        "    translation: []\n",
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1142",
        "/tracks/torso/translation"
    ));
}

#[test]
fn trackless_track_is_l1142() {
    let source = "format: 1\nkind: animation\nduration: 1.0\nloop: false\ntracks:\n  torso: {}\n";
    assert!(has_code_at(&errors_for(source), "L1142", "/tracks/torso"));
}

#[test]
fn empty_tracks_is_l1142() {
    let source = "format: 1\nkind: animation\nduration: 1.0\nloop: false\ntracks: {}\n";
    assert!(has_code_at(&errors_for(source), "L1142", "/tracks"));
}

#[test]
fn non_finite_or_non_positive_duration_is_l1142() {
    let doc = AnimationDocument {
        duration: 0.0,
        tracks: vec![AnimationTrack {
            target: "torso".to_string(),
            rotation: Vec::new(),
            translation: vec![Keyframe {
                t: 0.0,
                v: [0.0, 0.0, 0.0],
                ease: Ease::Linear,
            }],
            scale: Vec::new(),
        }],
        ..Default::default()
    };
    assert!(has_code_at(&validate_animation(&doc), "L1142", "/duration"));
}

#[test]
fn duration_ceiling_is_inclusive_and_l1142_beyond() {
    let clip = |duration: f64| AnimationDocument {
        duration,
        tracks: vec![AnimationTrack {
            target: "torso".to_string(),
            rotation: Vec::new(),
            translation: vec![Keyframe {
                t: 0.0,
                v: [0.0, 0.0, 0.0],
                ease: Ease::Linear,
            }],
            scale: Vec::new(),
        }],
        ..Default::default()
    };
    assert!(validate_animation(&clip(3600.0)).is_empty());
    assert!(has_code_at(
        &validate_animation(&clip(3600.5)),
        "L1142",
        "/duration"
    ));
}

// --- Committed clips ---------------------------------------------------------------------------

/// Every committed (or frozen vigil) clip re-emits byte-identically once its whole-line comments
/// (which the clip writer does not carry) are removed.
#[test]
fn existing_clips_reemit_byte_identically() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files = [
        "crates/ludion-scene/tests/fixtures/anims/humanoid-walk.anim.yaml",
        "crates/ludion-scene/tests/fixtures/anims/player-attack.anim.yaml",
        "examples/gallery/voxels/anims/figure-wave.anim.yaml",
        "crates/ludion-cli/tests/fixtures/good_animation.anim.yaml",
        "crates/ludion-cli/tests/fixtures/assembly/anims/walk.anim.yaml",
    ];
    for file in files {
        let source = std::fs::read_to_string(root.join(file)).expect("committed clip is readable");
        let stripped: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .map(|line| format!("{line}\n"))
            .collect();
        let clip = check_animation(&stripped).unwrap_or_else(|e| panic!("{file}: {e:?}"));
        assert_eq!(emit_animation(&clip), stripped, "{file} is not canonical");
    }
}

// --- scale, events, generated_from ---------------------------------------------------------------

const EXTENDED: &str = "\
format: 1
kind: animation
name: spin
duration: 1.0
loop: false
events:
  - t: 0.05
    name: spin_active
  - t: 0.5
    name: hit
tracks:
  blade:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
    translation:
      - t: 0.0
        v:
          - 0.0
          - 1.0
          - 0.0
    scale:
      - t: 0.0
        v:
          - 1.0
          - 1.0
          - 1.0
      - t: 1.0
        v:
          - 2.0
          - 0.5
          - 2.0
        ease: smooth
";

/// Pins the canonical key order with every new key present: `format, kind, name, generated_from,
/// duration, loop, events, tracks`; `rotation, translation, scale` within a track.
#[test]
fn scale_events_and_generated_from_golden() {
    let clip = check_animation(EXTENDED).expect("clip should check clean");
    assert_eq!(clip.events.len(), 2);
    assert_eq!(clip.tracks[0].scale.len(), 2);
    assert_eq!(emit_animation(&clip), EXTENDED);

    let derived = "\
format: 1
kind: animation
name: walk
generated_from: art/walk.glb
duration: 1.0
loop: true
tracks:
  hip:
    scale:
      - t: 0.0
        v:
          - 1.0
          - 1.0
          - 1.0
";
    let clip = check_animation(derived).expect("derived clip should check clean");
    assert_eq!(clip.generated_from.as_deref(), Some("art/walk.glb"));
    assert_eq!(emit_animation(&clip), derived);
}

#[test]
fn non_positive_scale_is_l1142() {
    let source = EXTENDED.replace(
        "          - 2.0\n          - 0.5",
        "          - 2.0\n          - 0.0",
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1142",
        "/tracks/blade/scale/1/v/1"
    ));
}

/// A one-track clip whose `torso` rotates from `from` to `to`, the destination key easing `ease`.
fn turn_clip(from: [f64; 3], to: [f64; 3], ease: Ease) -> AnimationDocument {
    let key = |t: f64, v: [f64; 3], ease: Ease| Keyframe { t, v, ease };
    AnimationDocument {
        name: None,
        generated_from: None,
        duration: 1.0,
        looped: false,
        events: Vec::new(),
        tracks: vec![AnimationTrack {
            target: "torso".to_string(),
            rotation: vec![key(0.0, from, Ease::Linear), key(1.0, to, ease)],
            translation: Vec::new(),
            scale: Vec::new(),
        }],
    }
}

#[test]
fn a_rotation_segment_turning_a_half_circle_is_l1142_with_a_split_hint() {
    let diagnostics = validate_animation(&turn_clip([0.0; 3], [0.0, 360.0, 0.0], Ease::Linear));
    assert!(has_code_at(
        &diagnostics,
        "L1142",
        "/tracks/torso/rotation/1/v/1"
    ));
    let help = diagnostics[0].help.as_deref().unwrap_or_default();
    assert!(help.contains("split the segment"), "{help}");

    assert!(!validate_animation(&turn_clip([0.0; 3], [180.0, 0.0, 0.0], Ease::Smooth)).is_empty());
    assert!(validate_animation(&turn_clip([0.0; 3], [179.9, 0.0, 0.0], Ease::Linear)).is_empty());
}

#[test]
fn a_rotation_turn_too_large_to_count_segments_for_is_l1142_not_a_panic() {
    // Found by the `kind_roundtrip` fuzzer: the segment count saturates `u32`.
    let diagnostics = validate_animation(&turn_clip([0.0; 3], [0.0, 1e300, 0.0], Ease::Linear));
    assert!(has_code_at(
        &diagnostics,
        "L1142",
        "/tracks/torso/rotation/1/v/1"
    ));
}

#[test]
fn a_step_rotation_segment_may_jump_any_angle() {
    assert!(validate_animation(&turn_clip([0.0; 3], [0.0, 360.0, 0.0], Ease::Step)).is_empty());
}

#[test]
fn a_rotation_segment_is_judged_per_axis() {
    assert!(validate_animation(&turn_clip([0.0; 3], [120.0, 0.0, 120.0], Ease::Linear)).is_empty());
}

/// The angle, in degrees, that playback actually turns between a clip's two rotation keys.
fn played_turn_degrees(clip: &AnimationDocument) -> f64 {
    let keys = &clip.tracks[0].rotation;
    let (a, b) = (
        ludion_scene::sample_rotation(keys, 0.0).expect("first key"),
        ludion_scene::sample_rotation(keys, 1.0).expect("last key"),
    );
    let dot: f64 = (0..4).map(|k| a[k] * b[k]).sum();
    2.0 * ludion_core::math::f64::acos(dot.abs().min(1.0)).to_degrees()
}

#[test]
fn axes_that_combine_past_half_a_circle_are_l1142_with_a_split_hint() {
    let clip = turn_clip([0.0; 3], [179.0; 3], Ease::Linear);
    // The independent oracle: playback turns under 2 degrees, nothing like the written 300+.
    assert!(played_turn_degrees(&clip) < 2.0);
    let diagnostics = validate_animation(&clip);
    assert!(has_code_at(
        &diagnostics,
        "L1142",
        "/tracks/torso/rotation/1/v"
    ));
    let help = diagnostics[0].help.as_deref().unwrap_or_default();
    assert!(help.contains("2 equal segments"), "{help}");

    assert!(!validate_animation(&turn_clip([0.0; 3], [105.0; 3], Ease::Linear)).is_empty());

    // Split at the Euler midpoint, the segment pair validates clean.
    let mut split = turn_clip([0.0; 3], [179.0; 3], Ease::Linear);
    split.tracks[0].rotation.insert(
        1,
        Keyframe {
            t: 0.5,
            v: [89.5; 3],
            ease: Ease::Linear,
        },
    );
    assert!(validate_animation(&split).is_empty());
}

#[test]
fn segments_that_turn_as_written_stay_clean() {
    for to in [[90.0, 90.0, 0.0], [60.0; 3]] {
        assert!(validate_animation(&turn_clip([0.0; 3], to, Ease::Linear)).is_empty());
    }
    for k in [1.0, 45.0, 90.0, 135.0, 179.0] {
        for axis in 0..3 {
            let mut to = [0.0; 3];
            to[axis] = k;
            assert!(
                validate_animation(&turn_clip([0.0; 3], to, Ease::Linear)).is_empty(),
                "axis {axis} by {k}"
            );
        }
    }
}

#[test]
fn empty_scale_channel_is_l1142() {
    let source = MINIMAL_TRACK.replace("    translation:\n", "    scale: []\n    translation:\n");
    assert!(has_code_at(
        &errors_for(&source),
        "L1142",
        "/tracks/torso/scale"
    ));
}

#[test]
fn empty_events_is_l1198() {
    let source = MINIMAL_TRACK.replace("tracks:\n", "events: []\ntracks:\n");
    assert!(has_code_at(&errors_for(&source), "L1198", "/events"));
}

#[test]
fn event_time_out_of_range_is_l1198() {
    let source = EXTENDED.replace("t: 0.5\n    name: hit", "t: 1.5\n    name: hit");
    assert!(has_code_at(&errors_for(&source), "L1198", "/events/1/t"));
}

#[test]
fn looping_clip_event_at_duration_is_l1198() {
    let source = EXTENDED.replace("loop: false", "loop: true").replace(
        "t: 0.5
    name: hit",
        "t: 1.0
    name: hit",
    );
    assert!(has_code_at(&errors_for(&source), "L1198", "/events/1/t"));
}

#[test]
fn one_shot_clip_event_at_duration_is_accepted() {
    let source = EXTENDED.replace(
        "t: 0.5
    name: hit",
        "t: 1.0
    name: hit",
    );
    assert!(check_animation(&source).is_ok());
}

#[test]
fn non_increasing_event_times_are_l1198() {
    let source = EXTENDED.replace("t: 0.5\n    name: hit", "t: 0.05\n    name: hit");
    assert!(has_code_at(&errors_for(&source), "L1198", "/events/1/t"));
}

#[test]
fn blank_event_name_is_l1198() {
    let source = EXTENDED.replace("name: hit", "name: \" \"");
    assert!(has_code_at(&errors_for(&source), "L1198", "/events/1/name"));
}

#[test]
fn reserved_end_event_name_is_l1198() {
    let source = EXTENDED.replace("name: hit", "name: end");
    let diagnostics = errors_for(&source);
    assert!(has_code_at(&diagnostics, "L1198", "/events/1/name"));
    let d = diagnostics
        .iter()
        .find(|d| d.code.as_str() == "L1198")
        .unwrap();
    assert!(d.help.as_deref().unwrap().contains("rename"));
}

#[test]
fn events_on_a_derived_clip_are_l1198() {
    let source = EXTENDED.replace("name: spin\n", "name: spin\ngenerated_from: art/spin.glb\n");
    let diagnostics = errors_for(&source);
    assert!(has_code_at(&diagnostics, "L1198", "/events"));
    let d = diagnostics
        .iter()
        .find(|d| d.code.as_str() == "L1198")
        .unwrap();
    assert!(d.help.as_deref().unwrap().contains("clip overlay"));
}

#[test]
fn unknown_event_key_is_l1141_with_suggestion() {
    let source = EXTENDED.replace("    name: hit", "    nme: hit");
    let diagnostics = errors_for(&source);
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/events/1/nme"))
        .expect("an unknown-key diagnostic");
    assert_eq!(d.code.as_str(), "L1141");
    assert!(d.help.as_deref().unwrap().contains("name"), "{d:?}");
}

// --- The reserved `$root` track (root motion) -----------------------------------------------

const ROOT_CLIP: &str = "\
format: 1
kind: animation
duration: 1.0
loop: false
tracks:
  $root:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 0.0
          - 90.0
          - 0.0
    translation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 0.0
          - 0.0
          - -8.0
";

#[test]
fn a_valid_root_track_passes_and_round_trips_byte_identically() {
    let doc = check_animation(ROOT_CLIP).expect("a valid $root clip");
    let emitted = emit_animation(&doc);
    assert_eq!(emit_animation(&parse_animation(&emitted).unwrap()), emitted);
    assert_eq!(
        emitted,
        ROOT_CLIP.replace("  $root:", "  \"$root\":"),
        "the emitter quotes the `$root` key"
    );
}

#[test]
fn a_root_scale_channel_is_l1142() {
    let source = ROOT_CLIP.replace(
        "    translation:",
        "    scale:\n      - t: 0.0\n        v:\n          - 1.0\n          - 1.0\n          - 1.0\n    translation:",
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1142",
        "/tracks/$root/scale"
    ));
}

#[test]
fn a_root_translation_y_is_l1142() {
    let source = ROOT_CLIP.replacen(
        "          - 0.0\n          - 0.0\n          - -8.0",
        "          - 0.0\n          - 2.0\n          - -8.0",
        1,
    );
    assert!(has_code_at(
        &errors_for(&source),
        "L1142",
        "/tracks/$root/translation/1/v/1"
    ));
}

#[test]
fn a_root_rotation_off_the_vertical_axis_is_l1142() {
    let source = ROOT_CLIP.replace(
        "          - 0.0\n          - 90.0\n          - 0.0",
        "          - 10.0\n          - 90.0\n          - 5.0",
    );
    let errors = errors_for(&source);
    assert!(has_code_at(
        &errors,
        "L1142",
        "/tracks/$root/rotation/1/v/0"
    ));
    assert!(has_code_at(
        &errors,
        "L1142",
        "/tracks/$root/rotation/1/v/2"
    ));
}
