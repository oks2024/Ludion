//! Phase 13 (E2 input) Wave 2 — "the encoding proof", scheduled *before* Wave 4 records real
//! captures (`Plan/phase-13-input.md`, decision 3). Wave 1 encodes every analog `InputEvent` field
//! (`MouseMove.position`, `MouseWheel.delta`, `GamepadAxis.value`) as an IEEE-754 bit pattern
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
fn analog_events(value: f32) -> [InputEvent; 3] {
    [
        InputEvent::MouseMove {
            position: [value, -value],
        },
        InputEvent::MouseWheel {
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
        InputEvent::MouseWheel { delta } => vec![delta[0].to_bits(), delta[1].to_bits()],
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
