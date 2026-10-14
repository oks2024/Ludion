//! Independent-oracle regression for the golden gate: perturbations computed from every committed
//! golden's own pixels must fail the comparison, and a golden must pass against itself exactly.
//!
//! The perturbations are the two audit findings (render-graph G1: a colour-only change; G2: a small
//! element vanishing) plus the identity case. Pure CPU, no adapter: it decodes the committed PNGs
//! of both pinned platforms (including the macOS set, which cannot be rendered here) and runs them
//! through the public [`ludion_testkit::compare_png_bytes`] gate.

use std::path::{Path, PathBuf};

use ludion_testkit::compare_png_bytes;
use ludion_testkit::golden::DEFAULT_SSIM_THRESHOLD;

const GOLDEN_ROOTS: [&str; 7] = [
    "examples/gallery",
    "examples/import",
    "examples/import-textured",
    "games/breakout",
    "games/hello-paddle",
    "games/sokoban",
    "games/vigil", // Lore-only: absent from a public clone
];

struct Rgba {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

fn goldens() -> Vec<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut found = Vec::new();
    for root in GOLDEN_ROOTS {
        if !repo.join(root).is_dir() {
            continue;
        }
        for platform in ["windows", "macos"] {
            let dir = repo.join(root).join("goldens").join(platform);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "png") {
                    found.push(path);
                }
            }
        }
    }
    found.sort();
    assert!(
        found.len() > 100,
        "found only {} goldens — the glob is broken or the goldens moved",
        found.len()
    );
    found
}

fn decode(bytes: &[u8]) -> Rgba {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().expect("golden is a PNG");
    let mut buffer = vec![0; reader.output_buffer_size().expect("sized")];
    let info = reader.next_frame(&mut buffer).expect("golden decodes");
    buffer.truncate(info.buffer_size());
    let pixels = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        other => panic!("unsupported golden colour type {other:?}"),
    };
    Rgba {
        width: info.width,
        height: info.height,
        pixels,
    }
}

fn encode(image: &Rgba) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("header")
        .write_image_data(&image.pixels)
        .expect("pixels");
    out
}

fn luma(px: &[u8]) -> f64 {
    0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2])
}

fn grayscale(image: &Rgba) -> Rgba {
    let mut pixels = image.pixels.clone();
    for px in pixels.chunks_exact_mut(4) {
        let g = luma(px).round() as u8;
        px[..3].copy_from_slice(&[g, g, g]);
    }
    Rgba { pixels, ..*image }
}

fn swap_red_blue(image: &Rgba) -> Rgba {
    let mut pixels = image.pixels.clone();
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Rgba { pixels, ..*image }
}

/// Flattens the 16×16 block (stride-aligned scan, step 8) with the highest luma variance to its
/// mean colour.
fn flatten_busiest_block(image: &Rgba) -> Rgba {
    const BLOCK: u32 = 16;
    let (w, h) = (image.width, image.height);
    let mut best: Option<(f64, u32, u32)> = None;
    if w >= BLOCK && h >= BLOCK {
        for y in (0..=h - BLOCK).step_by(8) {
            for x in (0..=w - BLOCK).step_by(8) {
                let (mut sum, mut sum_sq) = (0.0, 0.0);
                for by in y..y + BLOCK {
                    for bx in x..x + BLOCK {
                        let i = ((by * w + bx) * 4) as usize;
                        let l = luma(&image.pixels[i..i + 3]);
                        sum += l;
                        sum_sq += l * l;
                    }
                }
                let n = f64::from(BLOCK * BLOCK);
                let variance = sum_sq / n - (sum / n) * (sum / n);
                if best.is_none_or(|(v, _, _)| variance > v) {
                    best = Some((variance, x, y));
                }
            }
        }
    }
    let mut pixels = image.pixels.clone();
    if let Some((_, x, y)) = best {
        let mut mean = [0u64; 3];
        for by in y..y + BLOCK {
            for bx in x..x + BLOCK {
                let i = ((by * w + bx) * 4) as usize;
                for (sum, &channel) in mean.iter_mut().zip(&image.pixels[i..i + 3]) {
                    *sum += u64::from(channel);
                }
            }
        }
        let n = u64::from(BLOCK * BLOCK);
        for by in y..y + BLOCK {
            for bx in x..x + BLOCK {
                let i = ((by * w + bx) * 4) as usize;
                for (out, sum) in pixels[i..i + 3].iter_mut().zip(mean) {
                    *out = ((sum + n / 2) / n) as u8;
                }
            }
        }
    }
    Rgba { pixels, ..*image }
}

/// Runs `perturb` over every golden and fails once, listing every golden whose perturbation the
/// gate let through (or that the perturbation left unchanged).
fn assert_every_golden_rejects(name: &str, perturb: fn(&Rgba) -> Rgba) {
    let mut survivors = Vec::new();
    for path in goldens() {
        let bytes = std::fs::read(&path).expect("golden readable");
        let original = decode(&bytes);
        let changed = perturb(&original);
        let label = path
            .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .unwrap_or(&path)
            .display()
            .to_string();
        if changed.pixels == original.pixels {
            survivors.push(format!("{label}: the perturbation changed nothing"));
            continue;
        }
        let report = compare_png_bytes(&bytes, &encode(&changed), DEFAULT_SSIM_THRESHOLD)
            .expect("both sides decode");
        if report.pass {
            survivors.push(format!(
                "{label}: ssim {:.4}, worst tile {:.4}, worst colour {:.2}",
                report.ssim.unwrap_or(f64::NAN),
                report.worst_tile.as_ref().map_or(f64::NAN, |t| t.ssim),
                report
                    .worst_color_tile
                    .as_ref()
                    .map_or(f64::NAN, |t| t.color_error),
            ));
        }
    }
    assert!(
        survivors.is_empty(),
        "{name} slipped through the golden gate on {} golden(s):\n{}",
        survivors.len(),
        survivors.join("\n")
    );
}

#[test]
fn every_golden_passes_against_itself_exactly() {
    for path in goldens() {
        let bytes = std::fs::read(&path).expect("golden readable");
        let report = compare_png_bytes(&bytes, &bytes, DEFAULT_SSIM_THRESHOLD).expect("decodes");
        assert!(report.pass, "{}: {}", path.display(), report.summary);
        assert_eq!(report.ssim, Some(1.0), "{}", path.display());
        assert_eq!(report.mae, Some(0.0), "{}", path.display());
    }
}

#[test]
fn grayscale_of_every_golden_fails_the_gate() {
    assert_every_golden_rejects("grayscale", grayscale);
}

#[test]
fn red_blue_swap_of_every_golden_fails_the_gate() {
    assert_every_golden_rejects("a red/blue swap", swap_red_blue);
}

#[test]
fn a_flattened_16x16_block_of_every_golden_fails_the_gate() {
    assert_every_golden_rejects("a flattened 16x16 block", flatten_busiest_block);
}
