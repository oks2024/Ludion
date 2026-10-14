//! Generates the cross-language frame-codec fixtures consumed by the editor's decoder tests (via
//! `editor/app/src/protocol/__fixtures__/`).
//!
//! # Why this exists
//!
//! Frames cross the wire encoded by the `qoi` or `lz4_flex` crate
//! (`crates/ludion-server/src/delivery.rs::route_frame`), but the viewer decodes them with
//! hand-rolled TS decoders (`editor/app/src/protocol/qoi.ts`, `lz4.ts`) — no npm codec dependency.
//! Nothing else in the workspace pins that the two sides agree byte-for-byte. This example is that
//! pin: it builds a small, fully deterministic synthetic RGBA image (no GPU render, no randomness —
//! every pixel is a closed-form function of its index, so re-running this binary reproduces the exact
//! same bytes), encodes it with the same crates the delivery path uses, and writes the inputs and
//! outputs of those encodes to disk so a TS test can decode `fixture.qoi` / `fixture.lz4` and assert
//! each matches `fixture.rgba` exactly.
//!
//! `fixture.lz4` is a raw LZ4 **block** with no prepended size — exactly what the server sends, where
//! the LFR1 header is the one source of truth for the dimensions (`fixture.json` plays that role
//! here), so the decoder's output buffer is always `width * height * 4`.
//!
//! # Regenerating
//!
//! ```text
//! cargo run -p ludion-server --example gen_frame_fixtures
//! ```
//!
//! This overwrites the four files below; commit the result. Only re-run it if the fixture's pixel
//! pattern changes here or the `qoi` / `lz4_flex` crate versions change in `Cargo.toml` — the whole
//! point is a stable, checked-in pin.
//!
//! # Fixture layout
//!
//! The image is 64x64 RGBA, laid out as eight 8-row bands (512 pixels each) in scan order, each
//! aimed at a different QOI op class:
//!
//! | rows  | op class targeted                                                        |
//! |-------|---------------------------------------------------------------------------|
//! | 0-7   | `QOI_OP_RUN` — one long run of a single solid color                       |
//! | 8-15  | `QOI_OP_DIFF` — small per-channel deltas (each in -2..=1), alpha held      |
//! | 16-23 | `QOI_OP_LUMA` — larger green-biased deltas (dg in -32..=31)               |
//! | 24-31 | `QOI_OP_INDEX` — a 5-color palette replayed out of order (index hits)     |
//! | 32-39 | `QOI_OP_RGB` — large deltas that overrun diff/luma range, alpha constant  |
//! | 40-47 | `QOI_OP_RGBA` — large deltas with alpha changing on every pixel           |
//! | 48-55 | `QOI_OP_RUN` boundary stress — run lengths sweep 1..=63, straddling the   |
//! |       | encoder's 62-pixel per-chunk bias limit                                   |
//! | 56-63 | interleaved mix of every band above, to stress op-to-op transitions       |
//!
//! The `qoi` crate's encoder greedily picks the smallest legal op per pixel (per the QOI spec), so
//! these bands are *targets* for the generator, not a per-pixel guarantee — the property the
//! fixture actually pins is that decoding `fixture.qoi` reproduces `fixture.rgba` exactly, with
//! every op class exercised somewhere in the stream.

use std::fs;
use std::path::PathBuf;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;
const BAND_PIXELS: u32 = 512; // 8 rows * 64 columns

fn main() {
    let rgba = build_fixture_image();
    assert_eq!(
        rgba.len(),
        (WIDTH * HEIGHT * 4) as usize,
        "fixture image is the wrong size"
    );

    let encoded = qoi::encode_to_vec(&rgba, WIDTH, HEIGHT)
        .expect("encoding the synthetic fixture should never fail");
    // Raw block, matching `route_frame`: no `compress_prepend_size`, because the frame header (here,
    // `fixture.json`) is the one place the dimensions live.
    let compressed = lz4_flex::block::compress(&rgba);

    let dir = fixtures_dir();
    fs::create_dir_all(&dir).expect("create __fixtures__ dir");

    fs::write(dir.join("fixture.qoi"), &encoded).expect("write fixture.qoi");
    fs::write(dir.join("fixture.lz4"), &compressed).expect("write fixture.lz4");
    fs::write(dir.join("fixture.rgba"), &rgba).expect("write fixture.rgba");
    fs::write(
        dir.join("fixture.json"),
        format!("{{\n  \"width\": {WIDTH},\n  \"height\": {HEIGHT}\n}}\n"),
    )
    .expect("write fixture.json");

    println!(
        "wrote {}x{} fixture: {} bytes raw rgba, {} bytes qoi, {} bytes lz4 -> {}",
        WIDTH,
        HEIGHT,
        rgba.len(),
        encoded.len(),
        compressed.len(),
        dir.display(),
    );
}

/// Where the fixture files live, relative to this crate (`crates/ludion-server`).
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../editor/app/src/protocol/__fixtures__")
}

/// Wraps a signed delta into a `u8` channel value (matches QOI's `& 0xff` wraparound semantics).
fn wrap8(v: i32) -> u8 {
    v.rem_euclid(256) as u8
}

/// Builds the 64x64 synthetic RGBA image described in the module doc comment, band by band, in
/// scan order (row-major, matching QOI's own pixel order).
fn build_fixture_image() -> Vec<u8> {
    let mut pixels: Vec<[u8; 4]> = Vec::with_capacity((WIDTH * HEIGHT) as usize);

    // Palette shared by the index band and the interleaved band; `PERM` replays it out of
    // sequential order so consecutive picks are (almost always) distinct colors, forcing
    // QOI_OP_INDEX rather than QOI_OP_RUN.
    const PALETTE: [[u8; 4]; 5] = [
        [18, 40, 90, 255],
        [200, 10, 10, 255],
        [10, 200, 10, 255],
        [10, 10, 200, 255],
        [250, 250, 10, 255],
    ];
    const PERM: [usize; 5] = [0, 2, 4, 1, 3];

    // Band 0 (rows 0-7): QOI_OP_RUN -- one long run of a solid color.
    let run_color = PALETTE[0];
    for _ in 0..BAND_PIXELS {
        pixels.push(run_color);
    }
    // `prev` only needs to track the running pixel for bands that compute a delta from it (1, 2,
    // and the matching arms of 7); bands 3-5 derive every pixel from its own index instead, so
    // they don't read or update it.
    let mut prev: [u8; 4] = run_color;

    // Band 1 (rows 8-15): QOI_OP_DIFF -- small per-channel deltas, alpha held constant.
    for i in 0..BAND_PIXELS {
        let dr = (i % 4) as i32 - 2;
        let dg = ((i / 4) % 4) as i32 - 2;
        let db = ((i / 2) % 4) as i32 - 2;
        let px = [
            wrap8(prev[0] as i32 + dr),
            wrap8(prev[1] as i32 + dg),
            wrap8(prev[2] as i32 + db),
            prev[3],
        ];
        pixels.push(px);
        prev = px;
    }

    // Band 2 (rows 16-23): QOI_OP_LUMA -- larger green-biased deltas, alpha held constant.
    for i in 0..BAND_PIXELS {
        let dg = (i % 64) as i32 - 32; // -32..=31
        let dr_minus_dg = ((i / 64) % 16) as i32 - 8; // -8..=7
        let db_minus_dg = ((i / 16) % 16) as i32 - 8; // -8..=7
        let dr = dg + dr_minus_dg;
        let db = dg + db_minus_dg;
        let px = [
            wrap8(prev[0] as i32 + dr),
            wrap8(prev[1] as i32 + dg),
            wrap8(prev[2] as i32 + db),
            prev[3],
        ];
        pixels.push(px);
        prev = px;
    }

    // Band 3 (rows 24-31): QOI_OP_INDEX -- the palette replayed out of order.
    for i in 0..BAND_PIXELS {
        let px = PALETTE[PERM[(i as usize) % PERM.len()]];
        pixels.push(px);
    }

    // Band 4 (rows 32-39): QOI_OP_RGB -- large deltas overrunning diff/luma range, alpha constant.
    for i in 0..BAND_PIXELS {
        let n = i as i32;
        let px = [
            wrap8(n * 97 + 11),
            wrap8(n * 131 + 53),
            wrap8(n * 181 + 197),
            255,
        ];
        pixels.push(px);
    }

    // Band 5 (rows 40-47): QOI_OP_RGBA -- large deltas, alpha changing on every pixel (multiplier
    // 53 is coprime with 256, so consecutive alpha values are never equal).
    for i in 0..BAND_PIXELS {
        let n = i as i32;
        let px = [
            wrap8(n * 71 + 3),
            wrap8(n * 113 + 17),
            wrap8(n * 163 + 41),
            wrap8(n * 53 + 7),
        ];
        pixels.push(px);
    }

    // Band 6 (rows 48-55): QOI_OP_RUN boundary stress -- run lengths sweep 1..=63, straddling the
    // encoder's 62-pixel per-chunk bias limit (a run of 63 identical pixels must split into two
    // RUN chunks).
    {
        let mut remaining = BAND_PIXELS;
        let mut run_len: u32 = 1;
        let mut step: i32 = 0;
        while remaining > 0 {
            let len = run_len.min(remaining);
            let base = 60 + (step % 150);
            let color = [wrap8(base), wrap8(base + 40), wrap8(base + 80), 255];
            for _ in 0..len {
                pixels.push(color);
            }
            remaining -= len;
            run_len = if run_len >= 63 { 1 } else { run_len + 1 };
            step += 23;
        }
        prev = *pixels.last().expect("band 6 pushed at least one pixel");
    }

    // Band 7 (rows 56-63): interleaved mix of every band above, to stress transitions between op
    // classes at their boundaries.
    for i in 0..BAND_PIXELS {
        let n = i as i32;
        let px = match i % 6 {
            0 => prev, // repeats the previous pixel -> a QOI_OP_RUN contributor
            1 => {
                // small diff
                [
                    wrap8(prev[0] as i32 + 1),
                    wrap8(prev[1] as i32 - 1),
                    prev[2],
                    prev[3],
                ]
            }
            2 => {
                // luma diff
                let dg = 20;
                [
                    wrap8(prev[0] as i32 + dg - 4),
                    wrap8(prev[1] as i32 + dg),
                    wrap8(prev[2] as i32 + dg + 3),
                    prev[3],
                ]
            }
            3 => PALETTE[PERM[(i as usize) % PERM.len()]], // index hit
            4 => [
                wrap8(n * 97 + 5),
                wrap8(n * 61 + 9),
                wrap8(n * 151 + 3),
                255,
            ], // raw RGB
            _ => [
                wrap8(n * 83 + 2),
                wrap8(n * 137 + 6),
                wrap8(n * 199 + 10),
                wrap8(n * 45 + 1),
            ], // raw RGBA, alpha varies
        };
        pixels.push(px);
        prev = px;
    }

    assert_eq!(
        pixels.len(),
        (WIDTH * HEIGHT) as usize,
        "band pixel counts must sum to WIDTH*HEIGHT"
    );

    let mut rgba = Vec::with_capacity(pixels.len() * 4);
    for px in pixels {
        rgba.extend_from_slice(&px);
    }
    rgba
}
