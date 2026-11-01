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
//! `LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it -E 'test(/^reflection_parity::/)'`.
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
        ludion_engine::AntiAliasing::Off,
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

/// One band's probe-only image, SSR-only image and fully confident mask (the `ssr_confidence` view's R
/// byte is 255), with `extra` overrides applied to every render, and the view the engine showed.
fn room_renders(
    roughness: f32,
    extra: &[String],
) -> Option<(DecodedImage, DecodedImage, Vec<bool>, Option<DebugView>)> {
    let mut probe_only = vec![
        format!("{ROOM_ROUGHNESS}={roughness}"),
        format!("{ROOM_POST}/ssr=0"),
    ];
    let mut ssr_only = vec![
        format!("{ROOM_ROUGHNESS}={roughness}"),
        format!("{ROOM_POST}/ssr=1"),
        format!("{ROOM_POST}/ssr_fade/0=0.9"),
        format!("{ROOM_POST}/ssr_fade/1=1.0"),
    ];
    probe_only.extend_from_slice(extra);
    ssr_only.extend_from_slice(extra);
    let (probe, _) = render("reflection-parity-room.yaml", &probe_only, None)?;
    let (ssr, _) = render("reflection-parity-room.yaml", &ssr_only, None)?;
    let (confidence, shown) = render(
        "reflection-parity-room.yaml",
        &ssr_only,
        Some(DebugView::SsrConfidence),
    )?;
    let mask = confidence
        .rgba
        .chunks_exact(4)
        .map(|px| px[0] == 255)
        .collect();
    Some((probe, ssr, mask, shown))
}

/// One band's SSR-against-probe SSIM over the fully confident floor.
fn room_band(roughness: f32) -> Option<f64> {
    let (probe, ssr, mask, shown) = room_renders(roughness, &[])?;
    assert_eq!(
        shown,
        Some(DebugView::SsrConfidence),
        "band {roughness}: the ssr_confidence view was dropped, so there is no mask to measure over"
    );
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

/// The room's camera (the fixture's transform and projection) and probe centre.
const ROOM_CAMERA: [f64; 3] = [0.0, 1.6, 1.0];
const ROOM_PITCH_DEG: f64 = -20.0;
const ROOM_FOV_Y_DEG: f64 = 45.0;
const ROOM_PROBE_CENTRE: [f64; 3] = [0.0, 1.5, -3.0];
/// The back wall's inner face and the room's height.
const ROOM_BACK_Z: f64 = -8.0;
const ROOM_HEIGHT: f64 = 3.0;

/// For image row `y` (centre column): the floor point it shows and the back-wall point the mirrored
/// view of that floor point reflects, as `(floor z, d_p / d_c)`, or `None` when the row misses the floor
/// or the reflected ray ends on the ceiling or the floor.
fn room_row_geometry(y: usize) -> Option<(f64, f64)> {
    let pitch = ROOM_PITCH_DEG.to_radians();
    let ndc = 1.0 - 2.0 * (y as f64 + 0.5) / f64::from(HEIGHT);
    let a = ndc * ludion_core::math::f64::tan(ROOM_FOV_Y_DEG.to_radians() / 2.0);
    let (sin, cos) = (
        ludion_core::math::f64::sin(pitch),
        ludion_core::math::f64::cos(pitch),
    );
    let (dir_y, dir_z) = (a * cos + sin, a * sin - cos);
    if dir_y >= 0.0 {
        return None;
    }
    let floor_z = ROOM_CAMERA[2] + ROOM_CAMERA[1] / -dir_y * dir_z;
    // The mirrored camera (0, -1.6, 1) looks through the floor point (0, 0, floor_z).
    let s = (ROOM_BACK_Z - ROOM_CAMERA[2]) / (floor_z - ROOM_CAMERA[2]);
    let hit = [0.0, -ROOM_CAMERA[1] + s * ROOM_CAMERA[1], ROOM_BACK_Z];
    if !(0.0..=ROOM_HEIGHT).contains(&hit[1]) {
        return None;
    }
    let dist = |from: [f64; 3]| {
        let (dy, dz) = (hit[1] - from[1], hit[2] - from[2]);
        (dy * dy + dz * dz).sqrt()
    };
    Some((floor_z, dist([0.0, 0.0, floor_z]) / dist(ROOM_PROBE_CENTRE)))
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// Probe blur against the distance to the reflected wall: the room's mirror floor rendered probe-only
/// and SSR-only, and per row the ratio `S = TV_probe / TV_ssr` of the mean adjacent-pixel luma step over
/// the confident pixels (relative to the row's mean luma). A probe that blurs by distance to the capture
/// point rather than to the hit has `S` fall where the hit is near, `d_p / d_c` small.
///
/// CONFIRMED if, in both bands, the median `S` of rows with `d_p / d_c <= NEAR` is <= 0.7 and the median
/// of rows with `d_p / d_c >= FAR` exceeds it by >= 0.2; REFUTED if the near median is >= 0.85 in both;
/// otherwise INCONCLUSIVE. Dither is off for these renders so it does not feed `S`. A second ratio,
/// `S_M`, compares the swing `(p95 - p5) / mean` of the 5-px box-filtered confident profile, and gets
/// its own verdict by the same rule. Prints the per-row table and the medians.
#[test]
fn needs_gpu_probe_blur_tracks_the_hit_distance() {
    const NEAR: f64 = 0.4;
    const FAR: f64 = 0.8;
    let mut verdicts = Vec::new();
    for band in [0.2_f32, 0.35] {
        let no_dither = [format!("{ROOM_POST}/dither=false")];
        let Some((probe, ssr, mask, _)) = room_renders(band, &no_dither) else {
            eprintln!("[reflection_parity] SKIPPED: no GPU adapter available");
            return;
        };
        let linear = |byte: u8| -> f64 {
            let c = f64::from(byte) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ludion_core::math::f64::powf((c + 0.055) / 1.055, 2.4)
            }
        };
        let lumas = |image: &DecodedImage| -> Vec<f64> {
            image
                .rgba
                .chunks_exact(4)
                .map(|px| 0.2126 * linear(px[0]) + 0.7152 * linear(px[1]) + 0.0722 * linear(px[2]))
                .collect()
        };
        let (p, c) = (lumas(&probe), lumas(&ssr));
        let w = WIDTH as usize;
        let tv = |luma: &[f64], row: usize| -> f64 {
            let at = |x: usize| row * w + x;
            let (mut steps, mut pairs, mut sum, mut n) = (0.0, 0usize, 0.0, 0usize);
            for x in 0..w {
                if mask[at(x)] {
                    sum += luma[at(x)];
                    n += 1;
                    if x + 1 < w && mask[at(x + 1)] {
                        steps += (luma[at(x + 1)] - luma[at(x)]).abs();
                        pairs += 1;
                    }
                }
            }
            steps / pairs.max(1) as f64 / (sum / n.max(1) as f64).max(f64::EPSILON)
        };
        // The 5-px box-filtered confident profile's (p95 - p5) / mean: the swing of the signal, which
        // dither and pixel noise barely move.
        let swing = |luma: &[f64], row: usize| -> f64 {
            let profile: Vec<f64> = (0..w)
                .filter(|&x| mask[row * w + x])
                .map(|x| luma[row * w + x])
                .collect();
            let mut smooth: Vec<f64> = profile
                .windows(5)
                .map(|win| win.iter().sum::<f64>() / 5.0)
                .collect();
            let mean = smooth.iter().sum::<f64>() / smooth.len().max(1) as f64;
            smooth.sort_by(f64::total_cmp);
            let at = |q: f64| smooth[((smooth.len() - 1) as f64 * q).round() as usize];
            (at(0.95) - at(0.05)) / mean.max(f64::EPSILON)
        };
        println!("band {band}: row floor_z d_p/d_c TV_probe TV_ssr S M_probe M_ssr S_M");
        let (mut near, mut far) = (Vec::new(), Vec::new());
        let (mut near_m, mut far_m) = (Vec::new(), Vec::new());
        for row in 0..HEIGHT as usize {
            let confident = (0..w).filter(|&x| mask[row * w + x]).count();
            let Some((floor_z, ratio)) = room_row_geometry(row).filter(|_| confident >= 64) else {
                continue;
            };
            let (tv_p, tv_c) = (tv(&p, row), tv(&c, row));
            let (m_p, m_c) = (swing(&p, row), swing(&c, row));
            let s = tv_p / tv_c.max(f64::EPSILON);
            let s_m = m_p / m_c.max(f64::EPSILON);
            println!(
                "  {row} {floor_z:.3} {ratio:.3} {tv_p:.5} {tv_c:.5} {s:.3} {m_p:.4} {m_c:.4} {s_m:.3}"
            );
            if ratio <= NEAR {
                near.push(s);
                near_m.push(s_m);
            } else if ratio >= FAR {
                far.push(s);
                far_m.push(s_m);
            }
        }
        assert!(
            near.len() >= 20 && far.len() >= 20,
            "band {band}: {} near-wall and {} far rows measured; the camera no longer frames both",
            near.len(),
            far.len()
        );
        let (rows_near, rows_far) = (near.len(), far.len());
        let medians = [&mut near, &mut far, &mut near_m, &mut far_m].map(|v| median(v));
        println!(
            "band {band}: near (d_p/d_c <= {NEAR}) median S {:.3} S_M {:.3} over {rows_near} rows; far \
             (>= {FAR}) median S {:.3} S_M {:.3} over {rows_far} rows",
            medians[0], medians[2], medians[1], medians[3]
        );
        verdicts.push(medians);
    }
    let verdict = |near: usize, far: usize| {
        if verdicts
            .iter()
            .all(|m| m[near] <= 0.7 && m[far] - m[near] >= 0.2)
        {
            "CONFIRMED"
        } else if verdicts.iter().all(|m| m[near] >= 0.85) {
            "REFUTED"
        } else {
            "INCONCLUSIVE"
        }
    };
    println!("probe blur verdict (S): {}", verdict(0, 1));
    println!("probe blur verdict (S_M): {}", verdict(2, 3));
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
         #   LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it -E 'test(/^reflection_parity::/)'\n\
         # Floors are the measured value minus the margin, written by LUDION_BLESS=1; the commit records the machine.\n\n\
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
             parity floors\"). Calibrate with `LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it \
             -E 'test(/^reflection_parity::/)'`",
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

const BLEND_POST: &str = "/entities/0/components/PostProcess";
const BLEND_CUBE_AWAY: &str = "/entities/4/components/Transform/translation/1=1000";
/// Pixels the confidence masks are eroded by.
const MASK_ERODE: usize = 4;
/// The fixture's four pillars (entities 6..=9), moved away to find the pixels that reflect them.
const BLEND_PILLARS: std::ops::RangeInclusive<usize> = 6..=9;

/// The hand-off has no absolute pop budget, because SSR and a probe disagree on *where* near content is
/// even when they agree on how it looks: the probe captures from one point and a sphere volume
/// reprojects it (parallax and cube resolution), SSR reads the screen. The same disagreement already
/// exists for dielectric pillars, whose fill both paths carry, so the dielectric reflection is the
/// reference and the chrome props may be no worse than it: `pop_metal <= POP_K * pop_dielectric`, with
/// `POP_K = 1` because the fill the metal adds is the same function of the same inputs, so nothing
/// but the placement error is left. Likewise brightness: the metal region's mean luma under SSR over
/// its mean luma under the probe must sit as close to 1 as the dielectric's does, to within
/// `RATIO_FLOOR` (the 8-bit rounding of a region mean, not a fit). Measured on an unpinned M5 over the
/// cube's mask: pop 0.381, ratio 1.017 (the cube's direct pixels 0.281, 0.972), against the pillars'
/// reflection 0.516, 0.741; before SSR's colour pyramid carried the fill the cube traced black: pop 1.0.
const POP_K: f64 = 1.0;
const RATIO_FLOOR: f64 = 0.05;

/// The metal props of `reflection-blend-metal.yaml` hand off from the probe fill to SSR without a pop:
/// SSR's colour pyramid carries the fill, so where the trace is fully confident it shows the props
/// as the probe path does, and the blend between the two is monotonic in confidence.
#[test]
fn needs_gpu_ssr_over_probes_hands_off_without_a_pop() {
    let scene = "reflection-blend-metal.yaml";
    let fade = [
        format!("{BLEND_POST}/ssr_fade/0=0.9"),
        format!("{BLEND_POST}/ssr_fade/1=1.0"),
    ];
    let with = |ssr: &str| -> Vec<String> {
        let mut set = vec![format!("{BLEND_POST}/ssr={ssr}")];
        set.extend(fade.iter().cloned());
        set
    };
    let Some((probe, _)) = render(scene, &with("0"), None) else {
        eprintln!("[reflection_parity] SKIPPED: no GPU adapter available");
        return;
    };
    let (half, _) =
        render(scene, &with("0.5"), None).expect("the adapter rendered the probe frame");
    let (full, _) = render(scene, &with("1"), None).expect("the adapter rendered the probe frame");
    let (confidence, _) = render(scene, &with("1"), Some(DebugView::SsrConfidence))
        .expect("the adapter rendered the probe frame");
    let mut moved = with("0");
    moved.push(BLEND_CUBE_AWAY.to_string());
    let (cube_away, _) = render(scene, &moved, None).expect("the adapter rendered the probe frame");
    let mut no_pillars = with("0");
    no_pillars.extend(
        BLEND_PILLARS.map(|i| format!("/entities/{i}/components/Transform/translation/1=1000")),
    );
    let (pillars_away, _) =
        render(scene, &no_pillars, None).expect("the adapter rendered the probe frame");

    // Linear luma (0..255 scale): the fixture has no tonemap, so the blend is linear only before the
    // sRGB encoding.
    let linear = |byte: u8| -> f64 {
        let c = f64::from(byte) / 255.0;
        255.0
            * if c <= 0.04045 {
                c / 12.92
            } else {
                ludion_core::math::f64::powf((c + 0.055) / 1.055, 2.4)
            }
    };
    let pixels = |image: &DecodedImage| -> Vec<f64> {
        image
            .rgba
            .chunks_exact(4)
            .map(|px| 0.2126 * linear(px[0]) + 0.7152 * linear(px[1]) + 0.0722 * linear(px[2]))
            .collect()
    };
    let (p, h, c, away, no_pillar) = (
        pixels(&probe),
        pixels(&half),
        pixels(&full),
        pixels(&cube_away),
        pixels(&pillars_away),
    );
    // The view shows the half-resolution trace, while the blend upsamples it: a pixel is only taken as
    // confident (or zero) when every pixel within `MASK_ERODE` agrees, so the mask edge is not measured.
    let eroded = |wanted: u8| -> Vec<bool> {
        let (w, h) = (WIDTH as usize, HEIGHT as usize);
        let hit: Vec<bool> = confidence
            .rgba
            .chunks_exact(4)
            .map(|px| px[0] == wanted)
            .collect();
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                (y.saturating_sub(MASK_ERODE)..=(y + MASK_ERODE).min(h - 1)).all(|yy| {
                    (x.saturating_sub(MASK_ERODE)..=(x + MASK_ERODE).min(w - 1))
                        .all(|xx| hit[yy * w + xx])
                })
            })
            .collect()
    };
    let (confident, unconfident) = (eroded(255), eroded(0));
    let differs = |a: &[f64], b: &[f64]| -> Vec<bool> {
        a.iter().zip(b).map(|(a, b)| (a - b).abs() > 1.0).collect()
    };
    let props = differs(&p, &away);
    let pillar_reflections = differs(&p, &no_pillar);

    // Confidence 0 is the probe-only frame, byte for byte.
    for (name, image) in [("ssr 0.5", &half), ("ssr 1", &full)] {
        let drifted = (0..p.len())
            .filter(|&i| {
                unconfident[i] && probe.rgba[i * 4..i * 4 + 3] != image.rgba[i * 4..i * 4 + 3]
            })
            .count();
        assert_eq!(
            drifted, 0,
            "{name}: {drifted} zero-confidence pixels differ from the probe-only frame"
        );
    }

    // Confidence 1 is SSR-only and agrees with the probe-only look of the props: no pop beyond the
    // placement disagreement the dielectric reflection shows.
    let measure = |mask: &[bool]| -> (f64, f64, usize) {
        let (mut pop, mut n, mut sum_p, mut sum_c) = (0.0, 0usize, 0.0, 0.0);
        for i in (0..p.len()).filter(|&i| confident[i] && mask[i]) {
            pop += (c[i] - p[i]).abs() / p[i].max(1.0);
            (sum_p, sum_c, n) = (sum_p + p[i], sum_c + c[i], n + 1);
        }
        (pop / n.max(1) as f64, sum_c / sum_p.max(f64::EPSILON), n)
    };
    let (pop, ratio, n) = measure(&props);
    let (pop_ref, ratio_ref, n_ref) = measure(&pillar_reflections);
    println!(
        "ssr over probes: props pop {pop:.4} ratio {ratio:.4} ({n} px); pillar reflections pop \
         {pop_ref:.4} ratio {ratio_ref:.4} ({n_ref} px)"
    );
    assert!(
        n >= 500 && n_ref >= 500,
        "only {n} prop and {n_ref} pillar-reflection confident pixels: the fixture no longer frames a measurement"
    );
    assert!(
        pop <= POP_K * pop_ref,
        "the hand-off pops: mean relative luma step {pop:.4} on the chrome props exceeds {POP_K} x the \
         dielectric reflection's {pop_ref:.4}"
    );
    assert!(
        (ratio - 1.0).abs() <= (ratio_ref - 1.0).abs().max(RATIO_FLOOR),
        "the props are {ratio:.4}x as bright under SSR as under the probe; the dielectric reflection is {ratio_ref:.4}x"
    );

    // Halfway is halfway: monotonic, no step.
    for i in (0..p.len()).filter(|&i| confident[i]) {
        let (lo, hi) = (p[i].min(c[i]), p[i].max(c[i]));
        assert!(
            h[i] >= lo - 2.0 && h[i] <= hi + 2.0,
            "pixel {i}: ssr 0.5 luma {:.1} lies outside probe {:.1} .. ssr {:.1}",
            h[i],
            p[i],
            c[i]
        );
        let mid = (p[i] + c[i]) / 2.0;
        assert!(
            (h[i] - mid).abs() <= 0.25 * (c[i] - p[i]).abs() + 2.0,
            "pixel {i}: ssr 0.5 luma {:.1} is not halfway between probe {:.1} and ssr {:.1}",
            h[i],
            p[i],
            c[i]
        );
    }
}
