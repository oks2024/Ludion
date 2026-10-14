//! The contact sheet: every candidate of a run in one grid, each cell labelled with the index and
//! seed [`crate::accept()`] takes (a label that does not fit its cell shrinks, then drops its seed,
//! then is left out).
//!
//! Reading eight candidates means seeing them side by side, and a run folder is eight files. The
//! composition is deliberately split in two: [`layout`] is a pure function of the candidates' pixel
//! sizes, so the geometry is testable without a decoder, a GPU or a golden image, and [`compose`]
//! is the IO half that decodes, draws and encodes around it.
//!
//! Cells are scaled to one common size preserving aspect — letterboxed and centred rather than
//! stretched, because a sheet is read for shape. The scaler is a box filter (an area average), the
//! honest choice for a downscale and the one a nearest-neighbour sample would alias; nothing here
//! needs a filter kernel or an image crate.
//!
//! The labels are the embedded 8×8 face, stamped by `ludion-font` — a pure crate, so this one stays
//! free of wgpu.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ludion_core::atomic::write_atomic;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::run::{self, RunReport};

/// The sheet's own filename inside a run folder.
pub const SHEET_FILE: &str = "sheet.png";

/// The largest side a cell may take. A grid of four 1024-pixel candidates would otherwise be a
/// 4000-pixel sheet, which no one reads at once and every viewer downscales anyway.
const CELL_MAX: u32 = 512;

/// The gutter between cells and around the grid, in pixels.
const PAD: u32 = 8;

/// The margin above and below a label strip's glyphs, in pixels.
const LABEL_PAD: u32 = 3;

/// The most columns [`compose`] picks on its own. Past four, cells are too small to judge.
const MAX_AUTO_COLS: u32 = 4;

/// The sheet's background: a neutral dark, so a candidate's own edges are the brightest thing in
/// its cell and letterbox bars read as empty rather than as image.
const BACKGROUND: [u8; 4] = [24, 24, 28, 255];

/// The label's ink.
const INK: [u8; 4] = [202, 202, 210, 255];

/// Where one candidate's pixels land on the sheet.
///
/// `x`, `y`, `w`, `h` are the **drawn image** — the letterboxed rectangle inside the cell, not the
/// cell's own box, which is [`Layout::cell`] at the grid position `index` implies. A cell whose
/// candidate is squarer than the common cell size is centred in it, and the difference is
/// background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The candidate's index in the run — what the label spells and what `gen accept` takes.
    pub index: u32,
    /// Left edge of the drawn image, in sheet pixels.
    pub x: u32,
    /// Top edge of the drawn image, in sheet pixels.
    pub y: u32,
    /// Drawn width.
    pub w: u32,
    /// Drawn height.
    pub h: u32,
}

/// The sheet's geometry: one common cell size, a grid, and a label strip under every cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The common cell size every candidate is letterboxed into.
    pub cell: (u32, u32),
    /// Columns actually used — never more than there are candidates.
    pub cols: u32,
    /// Rows the candidates fill, the last one possibly short.
    pub rows: u32,
    /// The label strip's height under each cell, glyphs plus margins.
    pub label_height: u32,
    /// The whole sheet's size.
    pub canvas: (u32, u32),
    /// One entry per candidate, in run order.
    pub cells: Vec<Cell>,
}

/// The grid for candidates of these pixel sizes at this column count.
///
/// Pure: the same sizes and column count always give the same geometry, on every platform. `cols`
/// is clamped to at least one and to the candidate count, so a sheet never carries an empty column.
/// An empty run has no sheet, and its layout is all zeros.
///
/// # Example
/// ```
/// let layout = ludion_gen::sheet::layout(&[(1024, 1024), (512, 768)], 2);
/// assert_eq!(layout.cell, (512, 512));
/// assert_eq!(layout.cells[1].w, 341, "a taller candidate is letterboxed, never stretched");
/// ```
pub fn layout(sizes: &[(u32, u32)], cols: u32) -> Layout {
    if sizes.is_empty() {
        return Layout {
            cell: (0, 0),
            cols: 0,
            rows: 0,
            label_height: 0,
            canvas: (0, 0),
            cells: Vec::new(),
        };
    }
    let count = sizes.len() as u32;
    let cols = cols.clamp(1, count);
    let rows = count.div_ceil(cols);

    // The common cell is the largest candidate, shrunk as a whole so the widest side fits the cap:
    // one scale for the grid keeps a landscape run landscape.
    let widest = sizes.iter().map(|size| size.0.max(1)).max().unwrap_or(1);
    let tallest = sizes.iter().map(|size| size.1.max(1)).max().unwrap_or(1);
    let over = widest.max(tallest);
    let cell = if over > CELL_MAX {
        (
            (widest as u64 * CELL_MAX as u64 / over as u64).max(1) as u32,
            (tallest as u64 * CELL_MAX as u64 / over as u64).max(1) as u32,
        )
    } else {
        (widest, tallest)
    };

    let label_height = ludion_font::LABEL_CELL * label_scale(cell.0) + 2 * LABEL_PAD;
    let stride_y = cell.1 + label_height + PAD;
    let canvas = (PAD + cols * (cell.0 + PAD), PAD + rows * stride_y);

    let cells = sizes
        .iter()
        .enumerate()
        .map(|(index, &(width, height))| {
            let (w, h) = fit(width.max(1), height.max(1), cell);
            let column = index as u32 % cols;
            let row = index as u32 / cols;
            Cell {
                index: index as u32,
                x: PAD + column * (cell.0 + PAD) + (cell.0 - w) / 2,
                y: PAD + row * stride_y + (cell.1 - h) / 2,
                w,
                h,
            }
        })
        .collect();

    Layout {
        cell,
        cols,
        rows,
        label_height,
        canvas,
        cells,
    }
}

/// Composes a run folder's candidates into `sheet.png` and returns the path written.
///
/// `cols` defaults to `ceil(sqrt(n))` capped at four; `out` defaults to [`SHEET_FILE`] inside the
/// run folder. Every candidate is decoded, letterboxed into the common cell by an area average, and
/// labelled `#<index> seed <n>` under its cell, or `#<index>` or nothing where that is too wide.
///
/// # Errors
/// [`code::GENERATION_ACCEPT_REFUSED`] when the folder holds no readable `run.json`;
/// [`code::GENERATION_OUTPUT_KIND`] for a mesh run or a run that downloaded nothing; `L1102` for a
/// candidate that is not a decodable PNG; [`code::OUTPUT_IO`] when the sheet cannot be
/// written.
pub fn compose(
    run_dir: &Path,
    cols: Option<u32>,
    out: Option<&Path>,
) -> Result<PathBuf, Vec<Diagnostic>> {
    let report = run::read_report(run_dir)?;
    refuse_unless_images(&report)?;

    let mut images = Vec::with_capacity(report.outputs.len());
    for output in &report.outputs {
        let path = report.dir.join(&output.file);
        let bytes = std::fs::read(&path).map_err(|error| {
            vec![
                Diagnostic::new(
                    code::GENERATION_ACCEPT_REFUSED,
                    format!("could not read candidate `{}`: {error}", path.display()),
                )
                .with_help(
                    "the run folder is scratch and may have been cleaned; run the recipe again",
                ),
            ]
        })?;
        images.push(
            ludion_assets::decode_png(&bytes, &path.display().to_string())
                .map_err(|diagnostic| vec![diagnostic])?,
        );
    }

    let sizes: Vec<(u32, u32)> = images.iter().map(|(w, h, _)| (*w, *h)).collect();
    let layout = layout(
        &sizes,
        cols.unwrap_or_else(|| default_cols(sizes.len() as u32)),
    );

    let (canvas_w, canvas_h) = layout.canvas;
    let mut pixels = vec![0u8; canvas_w as usize * canvas_h as usize * 4];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&BACKGROUND);
    }

    for (cell, ((src_w, src_h, rgba), output)) in layout
        .cells
        .iter()
        .zip(images.iter().zip(report.outputs.iter()))
    {
        blit(&mut pixels, canvas_w, canvas_h, cell, *src_w, *src_h, rgba);
        let label = fitted_label(output.index, output.seed, layout.cell.0);
        let row = cell.index / layout.cols;
        let column = cell.index % layout.cols;
        let baseline =
            PAD + row * (layout.cell.1 + layout.label_height + PAD) + layout.cell.1 + LABEL_PAD;
        let left = PAD + column * (layout.cell.0 + PAD);
        if let Some((label, scale)) = label {
            ludion_font::stamp_label(
                embedded_atlas(),
                &mut pixels,
                [canvas_w, canvas_h],
                [left, baseline],
                &label,
                scale,
                INK,
            );
        }
    }

    let bytes = ludion_assets::encode_png(canvas_w, canvas_h, &pixels).map_err(|error| {
        vec![Diagnostic::new(
            code::OUTPUT_IO,
            format!("could not encode the contact sheet: {error}"),
        )]
    })?;
    let path = out.map_or_else(|| report.dir.join(SHEET_FILE), Path::to_path_buf);
    write_atomic(&path, &bytes).map_err(|error| {
        vec![
            Diagnostic::new(
                code::OUTPUT_IO,
                format!("could not write `{}`: {error}", path.display()),
            )
            .with_help("check the folder exists and is writable"),
        ]
    })?;
    Ok(path)
}

/// A sheet shows images. A mesh run's candidates are geometry, and an empty run has nothing to
/// show.
fn refuse_unless_images(report: &RunReport) -> Result<(), Vec<Diagnostic>> {
    if report.kind == crate::workflow::OutputKind::Mesh.as_str() {
        return Err(vec![
            Diagnostic::new(
                code::GENERATION_OUTPUT_KIND,
                format!(
                    "run `{}` produced meshes, and the sheet shows images; a mesh run's candidates \
                     are opened on the model stage",
                    report.run_id
                ),
            )
            .with_help(format!(
                "`ludion render --model {}/0000.glb` opens one candidate",
                report.dir.display()
            )),
        ]);
    }
    if report.outputs.is_empty() {
        return Err(vec![
            Diagnostic::new(
                code::GENERATION_OUTPUT_KIND,
                format!("run `{}` downloaded no candidate", report.run_id),
            )
            .with_help(if report.status == run::STATUS_FAILED {
                "every job failed or was lost; `run.json`'s `failed` names each one".to_string()
            } else {
                format!(
                    "its status is `{}`; `ludion gen fetch {}` completes a run whose jobs have \
                     finished",
                    report.status,
                    run::display_path(&report.dir)
                )
            }),
        ]);
    }
    Ok(())
}

/// `ceil(sqrt(n))`, capped: a run of four is a square, a run of sixteen is four rows.
fn default_cols(count: u32) -> u32 {
    let mut cols = 1;
    while cols * cols < count {
        cols += 1;
    }
    cols.clamp(1, MAX_AUTO_COLS)
}

/// The largest `(w, h)` with `width`/`height`'s aspect that fits inside `cell`.
fn fit(width: u32, height: u32, cell: (u32, u32)) -> (u32, u32) {
    let by_width = (
        cell.0,
        (u64::from(height) * u64::from(cell.0) / u64::from(width)).max(1) as u32,
    );
    if by_width.1 <= cell.1 {
        return by_width;
    }
    (
        (u64::from(width) * u64::from(cell.1) / u64::from(height)).max(1) as u32,
        cell.1,
    )
}

/// Draws one candidate into its cell with a box filter: each destination pixel is the average of
/// the source pixels it covers, which degenerates to a plain copy at 1:1 and to nearest-neighbour
/// on an upscale.
fn blit(
    pixels: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    cell: &Cell,
    src_w: u32,
    src_h: u32,
    rgba: &[u8],
) {
    for dy in 0..cell.h {
        let y0 = (u64::from(dy) * u64::from(src_h) / u64::from(cell.h)) as u32;
        let y1 = ((u64::from(dy) + 1) * u64::from(src_h) / u64::from(cell.h)).max(u64::from(y0) + 1)
            as u32;
        for dx in 0..cell.w {
            let x0 = (u64::from(dx) * u64::from(src_w) / u64::from(cell.w)) as u32;
            let x1 = ((u64::from(dx) + 1) * u64::from(src_w) / u64::from(cell.w))
                .max(u64::from(x0) + 1) as u32;
            let mut sums = [0u32; 4];
            let mut taken = 0u32;
            for sy in y0..y1.min(src_h) {
                for sx in x0..x1.min(src_w) {
                    let offset = (sy as usize * src_w as usize + sx as usize) * 4;
                    for channel in 0..4 {
                        sums[channel] += u32::from(rgba[offset + channel]);
                    }
                    taken += 1;
                }
            }
            if taken == 0 {
                continue;
            }
            let (x, y) = (cell.x + dx, cell.y + dy);
            if x >= canvas_w || y >= canvas_h {
                continue;
            }
            let offset = (y as usize * canvas_w as usize + x as usize) * 4;
            for channel in 0..4 {
                pixels[offset + channel] = (sums[channel] / taken) as u8;
            }
        }
    }
}

// --- The labels ---------------------------------------------------------------------------------

/// The decoded embedded atlas, once per process.
fn embedded_atlas() -> &'static [u8] {
    static ATLAS: OnceLock<Vec<u8>> = OnceLock::new();
    ATLAS.get_or_init(|| {
        ludion_assets::decode_png(ludion_font::EMBEDDED_PNG, "default.png")
            .expect("the embedded face's atlas is a valid PNG")
            .2
    })
}

/// The glyph scale a cell of this width starts from: one on a thumbnail, two at the 512 px
/// cell cap, so a label is legible at every sheet size. One scale step per 192 px
/// keeps about 21 characters of `#<index> seed <n>` inside a cell.
fn label_scale(cell_width: u32) -> u32 {
    (cell_width / 192).clamp(1, 4)
}

/// The label a cell of `cell_width` carries and the scale to stamp it at: `#<index> seed <n>` (`?`
/// for an unknown seed), else `#<index>`, each tried from [`label_scale`] down to 1, and `None`
/// when even that is wider than the cell. The ink of an `n`-glyph label is
/// `((n - 1) * LABEL_PITCH + LABEL_CELL) * scale` wide.
fn fitted_label(index: u32, seed: Option<u64>, cell_width: u32) -> Option<(String, u32)> {
    let short = format!("#{index}");
    let full = format!("{short} seed {}", run::seed_text(seed));
    for label in [full, short] {
        let glyphs = label.chars().count() as u32;
        let ink = (glyphs - 1) * ludion_font::LABEL_PITCH + ludion_font::LABEL_CELL;
        for scale in (1..=label_scale(cell_width)).rev() {
            if ink * scale <= cell_width {
                return Some((label, scale));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_set_of_sizes_gives_a_committed_geometry() {
        let layout = layout(&[(1024, 1024), (512, 768), (800, 600)], 2);

        assert_eq!(layout.cell, (512, 512), "the cap shrinks the common cell");
        assert_eq!((layout.cols, layout.rows), (2, 2));
        assert_eq!(layout.label_height, 22, "8 glyph rows at scale 2, padded");
        assert_eq!(layout.canvas, (1048, 1092));
        assert_eq!(
            layout.cells,
            vec![
                Cell {
                    index: 0,
                    x: 8,
                    y: 8,
                    w: 512,
                    h: 512
                },
                Cell {
                    index: 1,
                    x: 613,
                    y: 8,
                    w: 341,
                    h: 512
                },
                Cell {
                    index: 2,
                    x: 8,
                    y: 614,
                    w: 512,
                    h: 384
                },
            ],
            "every candidate keeps its aspect and is centred in the common cell"
        );
    }

    #[test]
    fn columns_never_exceed_the_candidate_count_and_an_empty_run_has_no_sheet() {
        assert_eq!(layout(&[(64, 64)], 4).cols, 1);
        assert_eq!(layout(&[], 4).canvas, (0, 0));
        assert_eq!(layout(&[(64, 64)], 0).cols, 1);
    }

    #[test]
    fn the_automatic_column_count_is_a_capped_square() {
        assert_eq!(default_cols(1), 1);
        assert_eq!(default_cols(2), 2);
        assert_eq!(default_cols(4), 2);
        assert_eq!(default_cols(8), 3);
        assert_eq!(default_cols(16), 4);
        assert_eq!(
            default_cols(64),
            4,
            "past four columns a cell is unreadable"
        );
    }

    #[test]
    fn every_character_a_label_spells_has_a_glyph() {
        for character in "#0123456789 seed?".chars() {
            assert!(
                ludion_font::FontFace::embedded().glyph(character).is_some(),
                "`{character}` has no glyph"
            );
        }
    }

    #[test]
    fn a_label_never_inks_past_its_cell() {
        let mut buffer = vec![0u8; 64 * 9 * 4 * 8 * 4 * 4];
        let widths = (1..=600).chain([1024]);
        for cell_width in widths {
            for index in [0, 63] {
                for seed in [None, Some(7), Some(u64::MAX)] {
                    let Some((label, scale)) = fitted_label(index, seed, cell_width) else {
                        continue;
                    };
                    let canvas_w = label.chars().count() as u32 * 9 * scale;
                    let canvas_h = 8 * scale;
                    let used = canvas_w as usize * canvas_h as usize * 4;
                    buffer[..used].fill(0);
                    ludion_font::stamp_label(
                        embedded_atlas(),
                        &mut buffer[..used],
                        [canvas_w, canvas_h],
                        [0, 0],
                        &label,
                        scale,
                        INK,
                    );
                    let right = buffer[..used]
                        .chunks_exact(4)
                        .enumerate()
                        .filter(|(_, px)| px[3] != 0)
                        .map(|(i, _)| i as u32 % canvas_w)
                        .max()
                        .expect("a fitted label inks something");
                    assert!(
                        right < cell_width,
                        "`{label}` at scale {scale} inks to column {right} in a {cell_width} px cell"
                    );
                }
            }
        }
    }

    #[test]
    fn the_label_falls_back_from_full_to_index_to_nothing() {
        assert_eq!(
            fitted_label(0, Some(7), 512),
            Some(("#0 seed 7".to_string(), 2))
        );
        assert_eq!(fitted_label(0, Some(7), 16), None);
        assert_eq!(fitted_label(0, Some(7), 17), Some(("#0".to_string(), 1)));
        assert_eq!(
            fitted_label(3, None, 512),
            Some(("#3 seed ?".to_string(), 2))
        );
    }
}
