//! Phase 13 (E2 input) Wave 2 — "the encoding proof", scheduled *before* Wave 4 records real
//! captures (`Plan/phase-13-input.md`, decision 3). Wave 1 encodes every analog `InputEvent` field
//! (`MouseMove.position`, `MouseWheel.delta`, `MouseMotion.delta`, `GamepadAxis.value`) as an IEEE-754 bit pattern
//! (`f32_bits`/`f32x2_bits` in `crates/ludion-sim/src/input.rs`) rather than a JSON number, because
//! `serde_json` deserializes `f32` through `f64` — a decimal -> `f64` -> `f32` double rounding that
//! is not guaranteed to reproduce the original bits. This file proves the encoding survives the
//! *real* replay path, not just the enum's own serde (already covered by `input.rs`'s unit tests):
//!
//! `InputEvent` -> `serde_json::Value` -> `ReplayWriter::write_input` -> `ReplayReader` ->
//! `InputEvent`
//!
//! `ludion-protocol` must never depend on `ludion-sim` (crate boundary rule: the protocol never
//! names a concrete event type), so this integration test lives here, in `ludion-sim`, with a
//! dev-only dependency on `ludion-protocol` running the other direction.

use ludion_core::time::FrameIndex;
use ludion_protocol::replay::{
    REPLAY_FORMAT_VERSION, ReplayHeader, ReplayReader, ReplayRecord, ReplayWriter, SceneRef,
};
use ludion_protocol::{PROTOCOL_VERSION, schema};
use ludion_sim::{GamepadAxis, InputEvent};
use proptest::prelude::*;

fn header() -> ReplayHeader {
    ReplayHeader {
        format_version: REPLAY_FORMAT_VERSION,
        engine_version: "0.0.0".into(),
        protocol_version: PROTOCOL_VERSION,
        schema_hash: schema::schema_hash(),
        scene: SceneRef::Path {
            path: "level.yaml".into(),
        },
        seed: 1,
        profile: None,
        render_extent: None,
        replay_content_hash: None,
        game: None,
    }
}

/// Round-trips one `InputEvent` through the real replay path: `to_wire` (the `serde_json::Value`
/// `ReplayWriter::write_input` accepts, exactly what `ludion-engine` hands it at record time) ->
/// a one-record in-memory `.lrp` byte buffer -> `ReplayReader::records` (decompress + decode) ->
/// the recovered `Value` -> `from_wire` (deserialize back to an `InputEvent`).
fn round_trip_through_replay(event: &InputEvent) -> InputEvent {
    let wire = serde_json::to_value(event).expect("InputEvent serializes to a wire Value");

    let mut buffer = Vec::new();
    let mut writer = ReplayWriter::new(&mut buffer, &header()).expect("writer opens");
    writer
        .write_input(FrameIndex::new(0), &wire)
        .expect("write_input records the event");
    writer.into_inner().expect("writer flushes");

    let reader = ReplayReader::new(&buffer).expect("the buffer parses as LRP1");
    let records = reader.records().expect("the one record decodes");
    let [ReplayRecord::Input { event: decoded, .. }] = records.as_slice() else {
        panic!("expected exactly one Input record, got {records:?}");
    };

    serde_json::from_value(decoded.clone())
        .expect("the replayed Value deserializes back to an InputEvent")
}

/// Every finite/boundary bit pattern regime the plan names: both signs, subnormals,
/// `f32::MIN_POSITIVE`, and values right at a power-of-two mantissa boundary — plus the
/// non-finite bit patterns (`NAN`/`INFINITY`), which the bit encoding carries losslessly too
/// (unlike a JSON number, which cannot represent them at all).
fn boundary_bits() -> Vec<u32> {
    [
        0.0f32,
        -0.0f32,
        1.0f32,
        -1.0f32,
        f32::MIN_POSITIVE,
        -f32::MIN_POSITIVE,
        f32::MAX,
        f32::MIN,
        f32::EPSILON,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        16_777_216.0f32, // 2^24: the largest integer an f32 mantissa represents exactly
        16_777_217.0f32, // rounds to the same value as above at f32 precision — bit 0x4b800000
    ]
    .into_iter()
    .map(f32::to_bits)
    .chain([
        1u32,        // smallest positive subnormal
        0x007f_ffff, // largest subnormal
        0x3f7f_ffff, // just under 1.0 (mantissa-boundary bit pattern)
        0x8000_0000, // -0.0, spelled as a bit pattern directly
    ])
    .collect()
}

/// Builds one `InputEvent` of each analog-bearing variant from a single `f32`, so every variant's
/// `*_bits` field gets the same boundary/random sample.
fn analog_events(value: f32) -> [InputEvent; 4] {
    [
        InputEvent::MouseMove {
            position: [value, -value],
        },
        InputEvent::MouseWheel {
            delta: [value, -value],
        },
        InputEvent::MouseMotion {
            delta: [value, -value],
        },
        InputEvent::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            value,
        },
    ]
}

/// Extracts every analog `f32` field a round-tripped event carries, as bits, in a fixed order
/// matching the event that produced it — so the two lists can be compared element-wise.
fn analog_bits(event: &InputEvent) -> Vec<u32> {
    match *event {
        InputEvent::MouseMove { position } => vec![position[0].to_bits(), position[1].to_bits()],
        InputEvent::MouseWheel { delta } | InputEvent::MouseMotion { delta } => {
            vec![delta[0].to_bits(), delta[1].to_bits()]
        }
        InputEvent::GamepadAxis { value, .. } => vec![value.to_bits()],
        ref other => panic!("analog_bits called on a non-analog event: {other:?}"),
    }
}

#[test]
fn boundary_f32_bit_patterns_survive_the_real_replay_path() {
    for bits in boundary_bits() {
        let value = f32::from_bits(bits);
        for event in analog_events(value) {
            let replayed = round_trip_through_replay(&event);
            assert_eq!(
                analog_bits(&replayed),
                analog_bits(&event),
                "bits 0x{bits:08x} ({value:?}) did not survive {event:?} through the replay path"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { rng_seed: proptest::test_runner::RngSeed::Fixed(7919), failure_persistence: None, ..ProptestConfig::default() })]
    /// "Plus a large random draw": an arbitrary `u32` covers the entire f32 bit-pattern space
    /// uniformly (subnormals, infinities, NaNs, and every ordinary value alike), so this is a
    /// wide, unbiased complement to the explicit boundary list above.
    #[test]
    fn arbitrary_f32_bit_patterns_survive_the_real_replay_path(bits in any::<u32>()) {
        let value = f32::from_bits(bits);
        for event in analog_events(value) {
            let replayed = round_trip_through_replay(&event);
            prop_assert_eq!(
                analog_bits(&replayed),
                analog_bits(&event),
                "bits 0x{:08x} did not survive {:?} through the replay path", bits, event
            );
        }
    }
}

/// The serialized form of a script using every key and mouse button that existed before the full
/// key set was appended, checked in byte for byte: appending enum variants must never move it.
const PRE_EXISTING_SCRIPT: &str = concat!(
    "[",
    r#"{"frame":0,"event":{"kind":"key_down","key":"A"}},"#,
    r#"{"frame":1,"event":{"kind":"key_down","key":"B"}},"#,
    r#"{"frame":2,"event":{"kind":"key_down","key":"C"}},"#,
    r#"{"frame":3,"event":{"kind":"key_down","key":"D"}},"#,
    r#"{"frame":4,"event":{"kind":"key_down","key":"E"}},"#,
    r#"{"frame":5,"event":{"kind":"key_down","key":"F"}},"#,
    r#"{"frame":6,"event":{"kind":"key_down","key":"G"}},"#,
    r#"{"frame":7,"event":{"kind":"key_down","key":"H"}},"#,
    r#"{"frame":8,"event":{"kind":"key_down","key":"I"}},"#,
    r#"{"frame":9,"event":{"kind":"key_down","key":"J"}},"#,
    r#"{"frame":10,"event":{"kind":"key_down","key":"K"}},"#,
    r#"{"frame":11,"event":{"kind":"key_down","key":"L"}},"#,
    r#"{"frame":12,"event":{"kind":"key_down","key":"M"}},"#,
    r#"{"frame":13,"event":{"kind":"key_down","key":"N"}},"#,
    r#"{"frame":14,"event":{"kind":"key_down","key":"O"}},"#,
    r#"{"frame":15,"event":{"kind":"key_down","key":"P"}},"#,
    r#"{"frame":16,"event":{"kind":"key_down","key":"Q"}},"#,
    r#"{"frame":17,"event":{"kind":"key_down","key":"R"}},"#,
    r#"{"frame":18,"event":{"kind":"key_down","key":"S"}},"#,
    r#"{"frame":19,"event":{"kind":"key_down","key":"T"}},"#,
    r#"{"frame":20,"event":{"kind":"key_down","key":"U"}},"#,
    r#"{"frame":21,"event":{"kind":"key_down","key":"V"}},"#,
    r#"{"frame":22,"event":{"kind":"key_down","key":"W"}},"#,
    r#"{"frame":23,"event":{"kind":"key_down","key":"X"}},"#,
    r#"{"frame":24,"event":{"kind":"key_down","key":"Y"}},"#,
    r#"{"frame":25,"event":{"kind":"key_down","key":"Z"}},"#,
    r#"{"frame":26,"event":{"kind":"key_down","key":"Digit0"}},"#,
    r#"{"frame":27,"event":{"kind":"key_down","key":"Digit1"}},"#,
    r#"{"frame":28,"event":{"kind":"key_down","key":"Digit2"}},"#,
    r#"{"frame":29,"event":{"kind":"key_down","key":"Digit3"}},"#,
    r#"{"frame":30,"event":{"kind":"key_down","key":"Digit4"}},"#,
    r#"{"frame":31,"event":{"kind":"key_down","key":"Digit5"}},"#,
    r#"{"frame":32,"event":{"kind":"key_down","key":"Digit6"}},"#,
    r#"{"frame":33,"event":{"kind":"key_down","key":"Digit7"}},"#,
    r#"{"frame":34,"event":{"kind":"key_down","key":"Digit8"}},"#,
    r#"{"frame":35,"event":{"kind":"key_down","key":"Digit9"}},"#,
    r#"{"frame":36,"event":{"kind":"key_down","key":"Up"}},"#,
    r#"{"frame":37,"event":{"kind":"key_down","key":"Down"}},"#,
    r#"{"frame":38,"event":{"kind":"key_down","key":"Left"}},"#,
    r#"{"frame":39,"event":{"kind":"key_down","key":"Right"}},"#,
    r#"{"frame":40,"event":{"kind":"key_down","key":"Space"}},"#,
    r#"{"frame":41,"event":{"kind":"key_down","key":"Enter"}},"#,
    r#"{"frame":42,"event":{"kind":"key_down","key":"Escape"}},"#,
    r#"{"frame":43,"event":{"kind":"key_down","key":"Shift"}},"#,
    r#"{"frame":44,"event":{"kind":"key_down","key":"Ctrl"}},"#,
    r#"{"frame":45,"event":{"kind":"key_down","key":"Alt"}},"#,
    r#"{"frame":50,"event":{"kind":"mouse_down","button":"left"}},"#,
    r#"{"frame":50,"event":{"kind":"mouse_down","button":"right"}},"#,
    r#"{"frame":50,"event":{"kind":"mouse_down","button":"middle"}}"#,
    "]"
);

#[test]
fn pre_existing_keys_and_buttons_serialize_and_decode_unchanged() {
    use ludion_sim::InputScript;

    let decoded: InputScript =
        serde_json::from_str(PRE_EXISTING_SCRIPT).expect("the pre-change script decodes");
    assert_eq!(decoded.len(), 49);
    assert_eq!(
        serde_json::to_string(&decoded).unwrap(),
        PRE_EXISTING_SCRIPT,
        "re-serializing must reproduce the pre-change bytes"
    );
}
