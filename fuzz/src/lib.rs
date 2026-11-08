//! Shared fuzz harness bodies — the property checks listed in [`TARGETS`], called by BOTH the coverage-guided
//! libFuzzer targets (`fuzz_targets/*.rs`, nightly + sanitizer) and the deterministic mutation
//! driver (`src/bin/mutation_driver.rs`, which runs on any toolchain when libFuzzer's sanitizer
//! runtime is unavailable — see the crate README / Plan/phase-9-hardening.md decision 3).
//!
//! Each function takes arbitrary bytes and encodes one invariant. A violation is a panic (an
//! `assert!`, an `unwrap`, an overflow, or a slice OOB inside the crate under test); the caller
//! decides how to react (libFuzzer aborts + saves the input; the driver `catch_unwind`s and
//! records it). The functions themselves never deliberately panic on well-formed rejection — a
//! `Diagnostic` is the designed outcome for bad input.

use std::collections::BTreeMap;
use std::sync::{Once, OnceLock};

use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_ecs::Registry;
use ludion_engine::Engine;
use ludion_import::ExternalFiles;
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::frame::FrameHeader;
use ludion_protocol::replay::ReplayReader;
use ludion_protocol::{Command, Request};
use ludion_scene::{KINDS, KindCheck};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

/// A harness: the name of its `fuzz_targets/` wrapper and corpus directory, and its entry point.
pub type Harness = (&'static str, fn(&[u8]));

/// Every harness.
pub const TARGETS: &[Harness] = &[
    ("scene_parse", scene_parse),
    ("protocol_envelope", protocol_envelope),
    ("frame_header", frame_header),
    ("replay_reader", replay_reader),
    ("canonical_idempotence", canonical_idempotence),
    ("kind_roundtrip", kind_roundtrip),
    ("png_decode", png_decode),
    ("wav_decode", wav_decode),
    ("restore_snapshot", restore_snapshot),
    ("gltf_import", gltf_import),
    ("font_bake", font_bake),
];

/// Target 1 — the strict-subset YAML scene parser never panics on arbitrary input.
pub fn scene_parse(data: &[u8]) {
    let source = String::from_utf8_lossy(data);
    let _ = ludion_scene::parse(&source);
}

/// Target 2 — the wire request envelope + `Command` JSON decoder never panics on arbitrary input.
pub fn protocol_envelope(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let _ = Request::from_json(&text);
}

/// Target 3 — the 36-byte LFR1 frame-header decoder never panics on arbitrary bytes.
pub fn frame_header(data: &[u8]) {
    let _ = FrameHeader::decode(data);
}

/// Target 4 — the LRP1 replay reader never panics on corrupt input; drives header parse, full
/// record decode (every zstd body), and seeks across the frame range.
pub fn replay_reader(data: &[u8]) {
    if let Ok(reader) = ReplayReader::new(data) {
        let _ = reader.records();
        let _ = reader.seek(FrameIndex::new(0));
        let _ = reader.seek(FrameIndex::new(u64::MAX));
    }
}

/// A nested struct with one `f32`, behind a `$ref`.
#[derive(Serialize, Deserialize, JsonSchema)]
struct FzInner {
    v: f32,
}

/// Every `f32` schema position, an `f64` beside them, a string and a nested struct.
#[derive(Serialize, Deserialize, JsonSchema)]
struct FzFloats {
    single: f32,
    maybe: Option<f32>,
    triple: [f32; 3],
    by_name: BTreeMap<String, f32>,
    wide: f64,
    label: String,
    inner: FzInner,
}

/// Externally tagged: a unit variant and a struct variant carrying an `f32`.
#[derive(Serialize, Deserialize, JsonSchema)]
enum FzAction {
    Idle,
    Move { speed: f32, dir: String },
}

/// Internally tagged: one `f32` variant, one `f64` variant.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FzTagged {
    Gain { amount: f32 },
    Wide { amount: f64 },
}

/// A plain-string component.
#[derive(Serialize, Deserialize, JsonSchema)]
struct FzNote(String);

/// The registry the idempotence target emits with: real schemas, so the f32 rule is exercised.
fn fuzz_registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = Registry::new();
        registry.register::<FzFloats>("FzFloats").unwrap();
        registry.register::<FzAction>("FzAction").unwrap();
        registry.register::<FzTagged>("FzTagged").unwrap();
        registry.register::<FzNote>("FzNote").unwrap();
        registry
    })
}

/// Whether `original` read back as `back` loses nothing: every non-float leaf is `==`, and two
/// floats agree by f64 bits, or `original` is f32-exact and both narrow to the same f32 (the
/// writer's f32 rule respells such a value as its shortest f32 decimal).
fn same_value(original: &Value, back: &Value) -> bool {
    match (original, back) {
        (Value::Number(a), Value::Number(b)) if a.is_f64() && b.is_f64() => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            #[allow(clippy::cast_possible_truncation, reason = "the narrowing is the test")]
            let (a32, b32) = (a as f32, b as f32);
            a.to_bits() == b.to_bits()
                || (f64::from(a32).to_bits() == a.to_bits() && a32.to_bits() == b32.to_bits())
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_value(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, a)| b.get(k).is_some_and(|b| same_value(a, b)))
        }
        _ => original == back,
    }
}

/// Like [`same_value`] but every float must match by f64 bits.
fn same_bits(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) if a.is_f64() && b.is_f64() => {
            a.as_f64().unwrap().to_bits() == b.as_f64().unwrap().to_bits()
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_bits(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, a)| b.get(k).is_some_and(|b| same_bits(a, b)))
        }
        _ => a == b,
    }
}

/// The value at its registered type's precision (`None` when it is not that type, or the name is
/// not one of this harness's components).
fn at_type(name: &str, value: &Value) -> Option<Value> {
    fn through<T: Serialize + DeserializeOwned>(value: &Value) -> Option<Value> {
        serde_json::to_value(serde_json::from_value::<T>(value.clone()).ok()?).ok()
    }
    match name {
        "FzFloats" => through::<FzFloats>(value),
        "FzAction" => through::<FzAction>(value),
        "FzTagged" => through::<FzTagged>(value),
        "FzNote" => through::<FzNote>(value),
        _ => None,
    }
}

/// `back` is `original` up to the f32 rule: names and structure equal, floats per [`same_value`],
/// and, where both values deserialize as the registered type, equal by f64 bits once passed through
/// it — the oracle that catches the f32 rule applied to an `f64` field.
fn assert_value_preserved(original: &ludion_scene::Scene, back: &ludion_scene::Scene) {
    assert_eq!(original.name, back.name, "scene name changed");
    assert_eq!(original.generated_from, back.generated_from);
    assert_eq!(original.seed, back.seed);
    assert_eq!(original.physics, back.physics);
    assert_eq!(
        original.entities.len(),
        back.entities.len(),
        "entity count changed"
    );
    fn by_name(entity: &ludion_scene::SceneEntity) -> Vec<&(String, Value)> {
        let mut components: Vec<_> = entity.components.iter().collect();
        components.sort_by(|x, y| x.0.cmp(&y.0));
        components
    }
    for (a, b) in original.entities.iter().zip(&back.entities) {
        assert_eq!(a.name, b.name, "entity name changed");
        assert_eq!(a.presentation, b.presentation);
        assert_eq!(a.instance.is_some(), b.instance.is_some());
        assert_eq!(
            a.components.len(),
            b.components.len(),
            "component count changed"
        );
        for ((name, va), (other, vb)) in by_name(a).into_iter().zip(by_name(b)) {
            assert_eq!(name, other, "component set changed");
            assert!(
                same_value(va, vb),
                "component `{name}` changed value: {va} became {vb}"
            );
            if let (Some(ta), Some(tb)) = (at_type(name, va), at_type(name, vb)) {
                assert!(
                    same_bits(&ta, &tb),
                    "component `{name}` changed value at its type: {ta} became {tb}"
                );
            }
        }
    }
}

/// Target 5 — canonical-writer idempotence and value preservation: `emit(parse(emit(s))) ==
/// emit(s)`, canonical output always re-parses, and re-parsing it loses no value (an `f32` field is
/// respelled shorter but reads back as the same `f32`). A violation panics (asserts), which is the
/// finding.
pub fn canonical_idempotence(data: &[u8]) {
    let source = String::from_utf8_lossy(data);
    let Ok(scene) = ludion_scene::parse(&source) else {
        return;
    };
    let registry = fuzz_registry();
    let once = ludion_scene::emit(&scene, registry);
    let reparsed =
        ludion_scene::parse(&once).expect("canonical emitter produced text that does not re-parse");
    let twice = ludion_scene::emit(&reparsed, registry);
    assert_eq!(once, twice, "canonical emit is not idempotent");
    assert_value_preserved(&scene, &reparsed);
}

/// `KINDS` rows a `Here` checker validates but that have no canonical emitter.
const CHECK_ONLY: &[&str] = &["voxel-ops"];

/// Checks `source` as `kind`; a document that passes must emit canonical text that re-checks and
/// re-emits byte-identical.
fn canonical<D>(
    kind: &str,
    source: &str,
    check: fn(&str) -> Result<D, Vec<Diagnostic>>,
    emit: fn(&D) -> String,
) {
    let Ok(doc) = check(source) else { return };
    let once = emit(&doc);
    let again = check(&once).unwrap_or_else(|errors| {
        panic!("canonical `{kind}` output fails its own check: {errors:?}\n{once}")
    });
    assert_eq!(
        once,
        emit(&again),
        "canonical `{kind}` emit is not idempotent"
    );
}

/// Runs the round trip for `kind`; `false` when the kind has no arm, so an empty `source` doubles
/// as the coverage probe.
fn round_trip(kind: &str, source: &str) -> bool {
    use ludion_scene as s;
    match kind {
        "mesh" => canonical(kind, source, s::check_mesh, s::emit_mesh),
        "mesh-edit" => canonical(kind, source, s::check_mesh_edit, s::emit_mesh_edit),
        "voxel" => canonical(kind, source, s::check_voxel, s::emit_voxel),
        "assembly" => canonical(kind, source, s::check_assembly, s::emit_assembly),
        "part" => canonical(kind, source, s::check_part, s::emit_part),
        "character" => canonical(kind, source, s::check_character, s::emit_character),
        "prefab" => canonical(
            kind,
            source,
            |source| s::check_prefab(source, &Registry::new()),
            |doc| s::emit_prefab(doc, &Registry::new()),
        ),
        "path" => canonical(kind, source, s::check_path, s::emit_path),
        "animation" => canonical(kind, source, s::check_animation, s::emit_animation),
        "sprite" => canonical(kind, source, s::check_sprite, s::emit_sprite),
        "palette" => canonical(kind, source, s::check_palette, s::emit_palette),
        "atlas" => canonical(kind, source, s::check_atlas, s::emit_atlas),
        "atlas-source" => canonical(kind, source, s::check_atlas_source, s::emit_atlas_source),
        "font" => canonical(kind, source, s::check_font, s::emit_font),
        "font-source" => canonical(kind, source, s::check_font_source, s::emit_font_source),
        "fonts" => canonical(kind, source, s::check_fonts, s::emit_fonts),
        "sculpt" => canonical(kind, source, s::check_sculpt, s::emit_sculpt),
        "generation" => canonical(kind, source, s::check_generation, s::emit_generation),
        "origin" => canonical(
            kind,
            source,
            s::check_origin_document,
            s::emit_origin_document,
        ),
        _ => return false,
    }
    true
}

static KINDS_COVERED: Once = Once::new();

/// Target 6 — every `KINDS` document checker never panics, and each canonical emitter's output
/// re-checks and re-emits byte-identical. A `Here` row with neither an emitter arm nor a
/// `CHECK_ONLY` entry panics on the first input, so a kind added to the table cannot go unfuzzed.
pub fn kind_roundtrip(data: &[u8]) {
    KINDS_COVERED.call_once(|| {
        for entry in KINDS {
            if matches!(entry.check, KindCheck::Here(_))
                && !round_trip(entry.name, "")
                && !CHECK_ONLY.contains(&entry.name)
            {
                panic!(
                    "KINDS row `{}` has no arm in fuzz `round_trip` and is not in CHECK_ONLY",
                    entry.name
                );
            }
        }
    });
    let source = String::from_utf8_lossy(data);
    let Some(kind) = ludion_scene::peek_kind(&source) else {
        return;
    };
    let Some(entry) = ludion_scene::kind_entry(&kind) else {
        return;
    };
    if let KindCheck::Here(check) = entry.check {
        let _ = check(&source);
        round_trip(entry.name, &source);
    }
}

/// Target 7 — the PNG decoder never panics on arbitrary bytes, and a decoded image is exactly
/// `width × height` RGBA8 pixels.
pub fn png_decode(data: &[u8]) {
    if let Ok((width, height, pixels)) = ludion_assets::decode_png(data, "fuzz") {
        assert_eq!(pixels.len(), width as usize * height as usize * 4);
    }
}

/// Target 8 — the WAV decoder never panics on arbitrary bytes.
pub fn wav_decode(data: &[u8]) {
    let _ = ludion_assets::decode_wav(data, "fuzz");
}

/// Target 9 — `engine.restore` never panics on a hostile snapshot, and a restored state can be
/// captured again. The engine roots at a directory that does not exist, so no path in the snapshot
/// resolves into the checkout.
pub fn restore_snapshot(data: &[u8]) {
    let Ok(snapshot) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    let mut engine = Engine::with_project_root(std::env::temp_dir().join("ludion-fuzz-no-project"));
    if engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .is_ok()
    {
        // One tick on the restored frame, so a frame the restore accepted must also survive a step.
        let _ = engine.execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: true,
        }));
        let _ = engine.execute(Command::EngineSnapshot);
    }
}

/// Target 10 — the glTF/GLB importer never panics on arbitrary bytes; external files are refused,
/// so the fuzzer never reads the filesystem.
pub fn gltf_import(data: &[u8]) {
    let _ = ludion_import::import(
        data,
        ExternalFiles::Refused,
        &ludion_import::ImportOptions::lossy(false),
    );
}

/// Target 11 — the font bake never panics on arbitrary face bytes, as an SDF face and as a bitmap
/// one; an `Err` is the designed outcome for a bad face.
pub fn font_bake(data: &[u8]) {
    use ludion_scene::{CharsetRange, FontCharset, FontMode, FontSourceDocument};

    let source = |mode, em_px, spread_px| FontSourceDocument {
        name: None,
        file: String::new(),
        mode,
        em_px,
        spread_px,
        line_height: None,
        charset: FontCharset {
            ranges: vec![CharsetRange::Ascii],
            chars: String::new(),
        },
    };
    let _ = ludion_font::bake::bake(&source(FontMode::Sdf, 16, Some(2)), data, "f.yaml", "f");
    let _ = ludion_font::bake::bake(&source(FontMode::Bitmap, 8, None), data, "f.yaml", "f");
}
