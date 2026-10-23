//! The reflection parity gate (`Plan/reflections.md` decision 27): SSR and the reflection probes are
//! designed to agree, and this test fails when they stop agreeing.
//!
//! `fixtures/reflection-parity-room.yaml` is a closed box room with one box probe fitted exactly to its
//! interior. Per roughness band the floor (one mesh, its `Mesh/roughness` overridden) is rendered
//! probe-only (`PostProcess/ssr = 0`) and SSR-only (`ssr = 1`, `ssr_fade = [0.9, 1.0]`, which puts every
//! band below the fade so a fully confident pixel is the trace alone), and the two are SSIM-compared over
//! the pixels where the trace is fully confident (the `ssr_confidence` view's R byte is 255). The score
//! must stay at or above its committed per-OS floor, `budgets/reflection-parity-<os>.toml`. Band `0.55`
//! sits inside the default fade band, where the product hands off between the two paths.
//!
//! `fixtures/reflection-parity-seam.yaml` measures the sky-to-probe seam, the step a player sees at a
//! probe's boundary: the floor with the probe in place against the probe moved away (the plain sky
//! fill), over the floor the `probe_index` view marks as covered. The seam is **reported, never
//! checked** (the floors file records it beside the bands).
//!
//! Every image is reproducible with `ludion render --scene <fixture> --set …`; the fixtures' headers
//! say how. Recalibrate the floors with
//! `LUDION_BLESS=1 cargo nextest run -p ludion-engine --test reflection_parity`.
//!
//! Needs a real GPU adapter and skips gracefully (printing why) when there is none.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ludion_engine::{
    DebugCategories, DebugView, FrameSize, QualitySetting, RenderOutcome, builtin_registry,
    render_scene_with_registry,
};
use ludion_scene::SceneOverride;
use ludion_testkit::{DecodedImage, bless_requested, masked_ssim, platform_dir};
use serde::Deserialize;

/// The floor's roughness per band; `0.55` is inside the default SSR fade band.
const BANDS: [f32; 4] = [0.05, 0.2, 0.35, 0.55];
/// A band whose confident floor spans fewer 11×11 windows no longer measures anything.
const MIN_WINDOWS: usize = 500;
/// The flat distance a floor sits below its measurement (the `image-stats` rule: the value is a pure
/// function of one machine's deterministic render, so no noise multiple).
const MARGIN: f64 = 0.02;
const WIDTH: u32 = 960;
const HEIGHT: u32 = 540;

const ROOM_POST: &str = "/entities/0/components/PostProcess";
const ROOM_ROUGHNESS: &str = "/entities/1/components/Mesh/roughness";
const SEAM_ROUGHNESS: &str = "/entities/2/components/Mesh/roughness";
const SEAM_PROBE_HEIGHT: &str = "/entities/3/components/Transform/translation/1=1000";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn floors_path(os: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../budgets")
        .join(format!("reflection-parity-{os}.toml"))
}

/// Renders a fixture with `overrides` (and optionally a debug view), or `None` (after printing why)
/// when the machine has no GPU adapter.
fn render(
    name: &str,
    overrides: &[String],
    view: Option<DebugView>,
) -> Option<(DecodedImage, Option<DebugView>)> {
    let specs: Vec<SceneOverride> = overrides
        .iter()
        .map(|spec| SceneOverride::parse(spec).expect("a well-formed test override"))
        .collect();
    match render_scene_with_registry(
        &builtin_registry(),
        &fixture(name),
        FrameSize::native(WIDTH, HEIGHT),
        &QualitySetting::full(),
        false,
        &specs,
        0,
        &DebugCategories::None,
        view,
    ) {
        Ok(RenderOutcome {
            image, debug_view, ..
        }) => Some((
            DecodedImage {
                width: image.width,
                height: image.height,
                rgba: image.rgba,
            },
            debug_view,
        )),
        Err(errors) if ludion_core::require_gpu::is_no_adapter(&errors) => None,
        Err(errors) => panic!("[reflection_parity] rendering {name} failed: {errors:?}"),
    }
}

fn luma(px: &[u8]) -> f64 {
    0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2])
}

/// One band's SSR-against-probe SSIM over the fully confident floor.
fn room_band(roughness: f32) -> Option<f64> {
    let rough = format!("{ROOM_ROUGHNESS}={roughness}");
    let ssr_only = [
        rough.clone(),
        format!("{ROOM_POST}/ssr=1"),
        format!("{ROOM_POST}/ssr_fade/0=0.9"),
        format!("{ROOM_POST}/ssr_fade/1=1.0"),
    ];
    let (probe, _) = render(
        "reflection-parity-room.yaml",
        &[rough, format!("{ROOM_POST}/ssr=0")],
        None,
    )?;
    let (ssr, _) = render("reflection-parity-room.yaml", &ssr_only, None)?;
    let (confidence, shown) = render(
        "reflection-parity-room.yaml",
        &ssr_only,
        Some(DebugView::SsrConfidence),
    )?;
    assert_eq!(
        shown,
        Some(DebugView::SsrConfidence),
        "band {roughness}: the ssr_confidence view was dropped, so there is no mask to measure over"
    );
    let mask: Vec<bool> = confidence
        .rgba
        .chunks_exact(4)
        .map(|px| px[0] == 255)
        .collect();
    let measured = masked_ssim(&probe, &ssr, &mask);
    let measured = measured.unwrap_or_else(|| {
        panic!("band {roughness}: no fully confident floor window — the room or camera no longer frames one")
    });
    assert!(
        measured.windows >= MIN_WINDOWS,
        "band {roughness}: only {} confident 11x11 windows (< {MIN_WINDOWS}) — the scene or camera \
         no longer frames a confident floor; render the ssr_confidence view (see the fixture header)",
        measured.windows
    );
    Some(measured.ssim)
}

/// The probe-in-place floor against the probe moved away, over the floor the probe covers:
/// `(masked SSIM, mean relative luma difference)`.
fn seam_band(roughness: f32) -> Option<(f64, f64)> {
    let rough = format!("{SEAM_ROUGHNESS}={roughness}");
    let (with, _) = render(
        "reflection-parity-seam.yaml",
        std::slice::from_ref(&rough),
        None,
    )?;
    let (without, _) = render(
        "reflection-parity-seam.yaml",
        &[rough.clone(), SEAM_PROBE_HEIGHT.to_string()],
        None,
    )?;
    let (index, _) = render(
        "reflection-parity-seam.yaml",
        &[rough],
        Some(DebugView::ProbeIndex),
    )?;
    // Covered surfaces are a saturated palette hue; no surface is black and the no-probe gray is neutral.
    let mask: Vec<bool> = index
        .rgba
        .chunks_exact(4)
        .map(|px| px[..3].iter().max().unwrap() - px[..3].iter().min().unwrap() > 32)
        .collect();
    let ssim = masked_ssim(&with, &without, &mask)
        .unwrap_or_else(|| panic!("seam band {roughness}: the probe covers no floor window"));
    let (mut diff, mut base) = (0.0, 0.0);
    for ((a, b), covered) in with
        .rgba
        .chunks_exact(4)
        .zip(without.rgba.chunks_exact(4))
        .zip(&mask)
    {
        if *covered {
            diff += (luma(a) - luma(b)).abs();
            base += luma(b);
        }
    }
    Some((ssim.ssim, diff / base.max(f64::EPSILON)))
}

#[derive(Deserialize)]
struct Floors {
    bands: BTreeMap<String, BandFloor>,
}

#[derive(Deserialize)]
struct BandFloor {
    ssim_floor: f64,
}

/// The floors file's text: a fixed header, then each band's floor and seam measurement.
fn floors_text(os: &str, rows: &[(f32, f64, f64, f64)]) -> String {
    let mut out = format!(
        "# Reflection parity floors - `tests/reflection_parity.rs` in ludion-engine (`Plan/reflections.md`\n\
         # decision 27), the {os} set. Per floor-roughness band, the SSIM between the SSR-only and the\n\
         # probe-only render of `tests/fixtures/reflection-parity-room.yaml`, over the pixels where SSR is\n\
         # fully confident. The test fails when a band's SSIM drops below its `ssim_floor`.\n\
         #\n\
         # Rule (the `image-stats-*.toml` rule): the measurement is a pure function of one machine's\n\
         # deterministic render, so each floor sits a flat `margin` below it, never a noise multiple. A\n\
         # measurement above floor + 2 x margin means the floor can be tightened by hand; it never fails.\n\
         # `[seam]` is the sky-to-probe step on `reflection-parity-seam.yaml` (SSIM, and the mean luma\n\
         # difference relative to the sky fill): measured and recorded here, never checked.\n\
         #\n\
         # Recalibrate (after a deliberate renderer change, with the reason in the commit):\n\
         #   LUDION_BLESS=1 cargo nextest run -p ludion-engine --test reflection_parity\n\
         # Calibrated on an unpinned Apple M5, 2026-10-22 (`Plan/owed.md`: pinned confirmation owed).\n\n\
         margin = {MARGIN}\n"
    );
    for (band, ssim, seam_ssim, seam_rel) in rows {
        out.push_str(&format!(
            "\n[bands.\"{band}\"]\nssim_floor = {:.4}   # measured {ssim:.4}\n",
            (ssim - MARGIN).max(0.0)
        ));
        out.push_str(&format!(
            "[seam.\"{band}\"]\nssim = {seam_ssim:.4}\nmean_rel_luma = {seam_rel:.4}\n"
        ));
    }
    out
}

#[test]
fn needs_gpu_ssr_and_probes_agree_in_a_fitted_box_room() {
    let os = match platform_dir() {
        Ok(os) => os,
        Err(diag) => {
            eprintln!("[reflection_parity] SKIPPED: {}", diag.message);
            return;
        }
    };
    let mut rows = Vec::new();
    for band in BANDS {
        let Some(ssim) = room_band(band) else {
            eprintln!("[reflection_parity] SKIPPED: no GPU adapter available");
            return;
        };
        let (seam_ssim, seam_rel) = seam_band(band).expect("the adapter rendered the room");
        println!("band {band}: ssim {ssim:.4} · seam ssim {seam_ssim:.4} rel {seam_rel:.4}");
        rows.push((band, ssim, seam_ssim, seam_rel));
    }

    let path = floors_path(os);
    if bless_requested() {
        std::fs::write(&path, floors_text(os, &rows)).expect("write the floors file");
        println!("[reflection_parity] blessed {}", path.display());
        return;
    }
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "no reflection parity floors at {} — this calibration is owed (`Plan/owed.md`, \"Reflections \
             parity floors\"). Calibrate with `LUDION_BLESS=1 cargo nextest run -p ludion-engine --test \
             reflection_parity`",
            path.display()
        )
    });
    let floors: Floors =
        toml::from_str(&text).unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));
    let mut failures = Vec::new();
    for (band, ssim, ..) in &rows {
        let key = band.to_string();
        match floors.bands.get(&key) {
            None => failures.push(format!("band {key} has no entry in {}", path.display())),
            Some(floor) if *ssim < floor.ssim_floor => failures.push(format!(
                "band {key}: SSR-vs-probe SSIM {ssim:.4} is below its floor {:.4} ({}); render both \
                 halves and `--debug-view ssr_confidence` (fixture header) to see where they diverge",
                floor.ssim_floor,
                path.display()
            )),
            Some(floor) if *ssim > floor.ssim_floor + 2.0 * MARGIN => println!(
                "note: band {key} measures {ssim:.4}, well above its floor {:.4} — tighten by hand",
                floor.ssim_floor
            ),
            Some(_) => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
