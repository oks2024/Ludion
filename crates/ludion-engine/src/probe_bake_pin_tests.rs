//! The bake-version pin: a change to what a probe capture produces cannot ship without a
//! `PROBE_BAKE_VERSION` bump, or a `Store` host would serve stale cubes under an unchanged key.
//!
//! `fixtures/probe-bake-pin.yaml` is captured on the pinned adapter and reduced to a numeric
//! signature (per mip and face: mean RGB and luminance standard deviation; the baked irradiance;
//! face size and mip count), compared within a tolerance against `fixtures/probe-bake-pin.json`,
//! which holds one `{version, signature}` entry per OS. A numeric signature rather than raw bytes, so
//! a driver update on a pinned box does not redden it. One term dominates each cube face of the
//! fixture (sun, point, spot, emissive, GI bounce and sky visibility), so a 2% change in any of them
//! clears the tolerance; a term sharing a face would hide under the others' statistics. It cannot see
//! a change only a feature the fixture lacks reaches (skinned or voxel-only geometry, a textured map),
//! the emissive layer's prefilter, the GI dynamic injection (a 2% change there stays inside the
//! tolerance), nor any other change inside the tolerance. Blessing is refused when the signature
//! moved but the version did not.
//!
//! Needs the pinned reference adapter and skips (printing why) anywhere else, or on an OS with no
//! entry yet.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::{Engine, resolve_startup_quality};
use ludion_renderer::PROBE_BAKE_VERSION;
use serde::{Deserialize, Serialize};

/// A value passes when it differs by at most `max(ABS_FLOOR, REL * |recorded|)`.
const REL: f64 = 0.005;
const ABS_FLOOR: f64 = 1e-3;

const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Signature {
    face_size: u32,
    /// Per mip, per face (+X −X +Y −Y +Z −Z): `[mean r, mean g, mean b, luminance std]`.
    mips: Vec<Vec<[f64; 4]>>,
    /// The irradiance stored with the capture, per cube axis.
    baked_irradiance: Vec<[f64; 3]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Entry {
    version: u32,
    signature: Signature,
}

fn pin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe-bake-pin.json")
}

fn read_pin() -> BTreeMap<String, Entry> {
    match std::fs::read_to_string(pin_path()) {
        Ok(text) => serde_json::from_str(&text).expect("probe-bake-pin.json parses"),
        Err(_) => BTreeMap::new(),
    }
}

/// Reduces one prefiltered mip (RGBA texels, six faces in layer order) to per-face statistics.
fn face_stats(level: &[f32]) -> Vec<[f64; 4]> {
    let per_face = level.len() / 6;
    level
        .chunks_exact(per_face)
        .map(|face| {
            let n = (per_face / 4) as f64;
            let mut sum = [0.0f64; 3];
            let (mut lsum, mut lsq) = (0.0f64, 0.0f64);
            for texel in face.chunks_exact(4) {
                let mut l = 0.0;
                for c in 0..3 {
                    let v = f64::from(texel[c]);
                    sum[c] += v;
                    l += LUMA[c] * v;
                }
                lsum += l;
                lsq += l * l;
            }
            let mean_l = lsum / n;
            let std = (lsq / n - mean_l * mean_l).max(0.0).sqrt();
            [sum[0] / n, sum[1] / n, sum[2] / n, std]
        })
        .collect()
}

/// Every difference between `recorded` and `current` beyond the tolerance, shapes first.
fn moved(recorded: &Signature, current: &Signature, rel: f64, abs_floor: f64) -> Vec<String> {
    let mut out = Vec::new();
    if recorded.face_size != current.face_size {
        out.push(format!(
            "face size {} -> {}",
            recorded.face_size, current.face_size
        ));
    }
    if recorded.mips.len() != current.mips.len() {
        out.push(format!(
            "mip count {} -> {}",
            recorded.mips.len(),
            current.mips.len()
        ));
    }
    if recorded.baked_irradiance.len() != current.baked_irradiance.len() {
        out.push("irradiance axis count changed".to_string());
    }
    if !out.is_empty() {
        return out;
    }
    let mut check = |what: String, was: f64, now: f64| {
        if (was - now).abs() > abs_floor.max(rel * was.abs()) {
            out.push(format!("{what}: {was} -> {now}"));
        }
    };
    for (m, (was, now)) in recorded.mips.iter().zip(&current.mips).enumerate() {
        for (f, (was, now)) in was.iter().zip(now).enumerate() {
            for (s, name) in ["r", "g", "b", "lum-std"].into_iter().enumerate() {
                check(format!("mip {m} face {f} {name}"), was[s], now[s]);
            }
        }
    }
    for (a, (was, now)) in recorded
        .baked_irradiance
        .iter()
        .zip(&current.baked_irradiance)
        .enumerate()
    {
        for c in 0..3 {
            check(format!("irradiance axis {a} ch {c}"), was[c], now[c]);
        }
    }
    out
}

/// Why a bless of `current` at `version` must be refused, or `None`: the signature moved against an
/// existing entry while `PROBE_BAKE_VERSION` equals the recorded one.
fn bless_refused(
    existing: Option<&Entry>,
    version: u32,
    current: &Signature,
    rel: f64,
    abs_floor: f64,
) -> Option<String> {
    let existing = existing?;
    if existing.version != version {
        return None;
    }
    let diff = moved(&existing.signature, current, rel, abs_floor);
    (!diff.is_empty()).then(|| {
        format!(
            "refusing to bless: the capture moved but PROBE_BAKE_VERSION is still {version}; bump \
             it first ({} values moved, first: {})",
            diff.len(),
            diff[0]
        )
    })
}

fn capture() -> Option<(Signature, String)> {
    let mut engine = Engine::builder()
        .patterns()
        .load_scene_file(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe-bake-pin.yaml"),
        )
        .expect("the fixture loads");
    engine.set_quality(resolve_startup_quality(Some("medium"), None).unwrap());
    let scene = engine.extract_render_scene(64, 64).expect("extract");
    let renderer = match engine.renderer() {
        Ok(renderer) => renderer,
        Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
            eprintln!("SKIPPED probe bake pin: no GPU adapter");
            return None;
        }
        Err(d) => panic!("renderer init failed: {d:?}"),
    };
    let adapter = renderer.adapter_name().to_string();
    let probes = renderer.read_probes(&scene, None).expect("read_probes");
    let irradiance = renderer
        .read_probe_irradiance(&scene)
        .expect("read_probe_irradiance");
    assert_eq!(probes.len(), 1, "the fixture holds one probe");
    let probe = &probes[0];
    let signature = Signature {
        face_size: probe.face_size,
        mips: probe.levels.iter().map(|l| face_stats(l)).collect(),
        baked_irradiance: irradiance[0]
            .baked
            .iter()
            .map(|e| e.map(f64::from))
            .collect(),
    };
    Some((signature, adapter))
}

#[test]
fn needs_gpu_capture_output_matches_the_pinned_bake_version() {
    let Ok(os) = ludion_testkit::platform_dir() else {
        eprintln!("SKIPPED probe bake pin: no pinned tier on this OS");
        return;
    };
    let Some((current, adapter)) = capture() else {
        return;
    };
    let reference = ludion_testkit::reference_adapter(os).expect("a pinned OS has an adapter");
    let bless = ludion_testkit::bless_requested();
    if !adapter.contains(reference) {
        assert!(
            !bless,
            "cannot bless the probe pin from {adapter}, not the pinned {reference}"
        );
        eprintln!("SKIPPED probe bake pin: {adapter} is not the pinned {reference}");
        return;
    }
    let mut pin = read_pin();
    if bless {
        if let Some(why) = bless_refused(pin.get(os), PROBE_BAKE_VERSION, &current, REL, ABS_FLOOR)
        {
            panic!("{why}");
        }
        pin.insert(
            os.to_string(),
            Entry {
                version: PROBE_BAKE_VERSION,
                signature: current,
            },
        );
        let mut text = serde_json::to_string_pretty(&pin).unwrap();
        text.push('\n');
        std::fs::write(pin_path(), text).expect("write the pin");
        return;
    }
    let Some(recorded) = pin.get(os) else {
        eprintln!("SKIPPED probe bake pin: no `{os}` entry yet");
        return;
    };
    assert_eq!(
        recorded.version, PROBE_BAKE_VERSION,
        "the pin records bake version {} but the code is {PROBE_BAKE_VERSION}: re-bless on this OS \
         (LUDION_BLESS=1 cargo test -p ludion-engine --lib probe_bake_pin)",
        recorded.version
    );
    let diff = moved(&recorded.signature, &current, REL, ABS_FLOOR);
    assert!(
        diff.is_empty(),
        "capture output moved ({} values, first: {}): bump PROBE_BAKE_VERSION, then LUDION_BLESS=1 \
         cargo test -p ludion-engine --lib probe_bake_pin",
        diff.len(),
        diff[0]
    );
}

fn sample() -> Signature {
    Signature {
        face_size: 4,
        mips: vec![vec![[0.5, 0.25, 0.1, 0.05]; 6]; 2],
        baked_irradiance: vec![[1.0, 1.0, 1.0]; 6],
    }
}

#[test]
fn moved_tolerates_noise_and_flags_a_real_change() {
    let base = sample();
    assert!(moved(&base, &base, REL, ABS_FLOOR).is_empty());
    let mut noise = base.clone();
    noise.mips[0][0][0] *= 1.001;
    noise.mips[1][2][3] += 5e-4;
    assert!(moved(&base, &noise, REL, ABS_FLOOR).is_empty());
    let mut changed = base.clone();
    changed.baked_irradiance[3][1] *= 1.02;
    assert_eq!(moved(&base, &changed, REL, ABS_FLOOR).len(), 1);
    let mut reshaped = base.clone();
    reshaped.mips.pop();
    assert_eq!(moved(&base, &reshaped, REL, ABS_FLOOR).len(), 1);
}

#[test]
fn a_bless_without_a_version_bump_is_refused() {
    let recorded = Entry {
        version: 6,
        signature: sample(),
    };
    let mut changed = sample();
    changed.mips[0][1][2] *= 1.5;
    assert!(bless_refused(Some(&recorded), 6, &changed, REL, ABS_FLOOR).is_some());
    assert!(bless_refused(Some(&recorded), 7, &changed, REL, ABS_FLOOR).is_none());
    assert!(bless_refused(None, 6, &changed, REL, ABS_FLOOR).is_none());
    assert!(bless_refused(Some(&recorded), 6, &sample(), REL, ABS_FLOOR).is_none());
}
