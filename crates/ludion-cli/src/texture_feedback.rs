//! The feedback sheets `ludion texture build --preview` writes beside (never into) the committed
//! map set: a per-map sheet, a step contact sheet and 2x2 tiling views.
//!
//! Pure layout over already-evaluated images plus one write function; labels use the embedded face
//! the generation contact sheets use.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ludion_core::atomic::write_atomic;
use ludion_texgen::encode::to_rgba8;
use ludion_texgen::{Image, Maps};

const BACKGROUND: [u8; 4] = [40, 40, 44, 255];
const INK: [u8; 4] = [255, 255, 255, 255];
const PAD: u32 = 4;
const LABEL_BAND: u32 = 12;
const MAPS_CELL: u32 = 256;
const STEP_CELL: u32 = 128;
const TILING_SIDE: u32 = 512;
/// Glyphs are 8 px at scale 1 and advance 9 px, so a 128 px cell holds 14.
const STEP_LABEL_CHARS: usize = 14;

/// The four sheets a `--preview` build writes.
pub struct Sheets {
    pub maps: PathBuf,
    pub steps: PathBuf,
    pub tiling_base_color: PathBuf,
    pub tiling_height: PathBuf,
}

struct Cell {
    label: String,
    size: u32,
    rgba: Vec<u8>,
}

impl Cell {
    fn new(label: &str, image: &Image, srgb: bool) -> Cell {
        Cell {
            label: label.to_string(),
            size: image.size,
            rgba: to_rgba8(image, srgb),
        }
    }
}

fn embedded_atlas() -> &'static [u8] {
    static ATLAS: OnceLock<Vec<u8>> = OnceLock::new();
    ATLAS.get_or_init(|| {
        ludion_assets::decode_png(ludion_font::EMBEDDED_PNG, "default.png")
            .expect("the embedded face's atlas is a valid PNG")
            .2
    })
}

fn blit(canvas: &mut [u8], canvas_w: u32, at: [u32; 2], size: u32, rgba: &[u8]) {
    let row = size as usize * 4;
    for y in 0..size as usize {
        let to = ((at[1] as usize + y) * canvas_w as usize + at[0] as usize) * 4;
        canvas[to..to + row].copy_from_slice(&rgba[y * row..(y + 1) * row]);
    }
}

fn filled(width: u32, height: u32) -> Vec<u8> {
    BACKGROUND.repeat(width as usize * height as usize)
}

/// Lays `cells` out row-major in `cols` columns with a label band under each.
fn sheet(cells: &[Cell], cols: u32) -> (u32, u32, Vec<u8>) {
    let pitch = cells.iter().map(|c| c.size).max().unwrap_or(1);
    let rows = (cells.len() as u32).div_ceil(cols);
    let (width, height) = (
        PAD + cols * (pitch + PAD),
        PAD + rows * (pitch + LABEL_BAND + PAD),
    );
    let mut canvas = filled(width, height);
    for (i, cell) in cells.iter().enumerate() {
        let (column, row) = (i as u32 % cols, i as u32 / cols);
        let x = PAD + column * (pitch + PAD);
        let y = PAD + row * (pitch + LABEL_BAND + PAD);
        blit(&mut canvas, width, [x, y], cell.size, &cell.rgba);
        ludion_font::stamp_label(
            embedded_atlas(),
            &mut canvas,
            [width, height],
            [x, y + pitch + 2],
            &cell.label,
            1,
            INK,
        );
    }
    (width, height, canvas)
}

/// `image` downsampled to at most `side` and repeated 2x2.
fn tiled(image: &Image, side: u32, srgb: bool) -> (u32, Vec<u8>) {
    let small = image.downsample(side);
    let (s, rgba) = (small.size, to_rgba8(&small, srgb));
    let mut canvas = filled(s * 2, s * 2);
    for (x, y) in [(0, 0), (s, 0), (0, s), (s, s)] {
        blit(&mut canvas, s * 2, [x, y], s, &rgba);
    }
    (s * 2, canvas)
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let bytes = ludion_assets::encode_png(width, height, rgba)
        .map_err(|e| format!("could not encode `{}`: {e}", path.display()))?;
    write_atomic(path, &bytes).map_err(|e| format!("could not write `{}`: {e}", path.display()))
}

/// Writes `maps.png`, `steps.png`, `tiling_base_color.png` and `tiling_height.png` into `dir`.
///
/// # Errors
/// The first create or write failure, as one line.
pub fn write_sheets(dir: &Path, maps: &Maps, steps: &[(String, Image)]) -> Result<Sheets, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("could not create `{}`: {e}", dir.display()))?;
    let cell = |label: &str, image: &Image, srgb: bool| {
        Cell::new(label, &image.downsample(MAPS_CELL), srgb)
    };
    let mut cells = vec![
        cell("base_color", &maps.base_color, true),
        cell("roughness", &maps.roughness, false),
        cell("metallic", &maps.metallic, false),
        cell("normal", &maps.normal, false),
        cell("occlusion", &maps.occlusion, false),
    ];
    if let Some(emissive) = &maps.emissive {
        cells.push(cell("emissive", emissive, true));
    }
    cells.push(cell("height", &maps.height, false));
    let (w, h, rgba) = sheet(&cells, cells.len() as u32);
    let maps_path = dir.join("maps.png");
    write_png(&maps_path, w, h, &rgba)?;

    let cells: Vec<Cell> = steps
        .iter()
        .map(|(name, image)| {
            let label: String = name.chars().take(STEP_LABEL_CHARS).collect();
            Cell::new(&label, &image.downsample(STEP_CELL), image.channels == 3)
        })
        .collect();
    let cols = (cells.len() as f64).sqrt().ceil().max(1.0) as u32;
    let (w, h, rgba) = sheet(&cells, cols);
    let steps_path = dir.join("steps.png");
    write_png(&steps_path, w, h, &rgba)?;

    let (side, rgba) = tiled(&maps.base_color, TILING_SIDE, true);
    let tiling_base_color = dir.join("tiling_base_color.png");
    write_png(&tiling_base_color, side, side, &rgba)?;
    let (side, rgba) = tiled(&maps.height, TILING_SIDE, false);
    let tiling_height = dir.join("tiling_height.png");
    write_png(&tiling_height, side, side, &rgba)?;

    Ok(Sheets {
        maps: maps_path,
        steps: steps_path,
        tiling_base_color,
        tiling_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiling_repeats_the_map_in_both_axes() {
        let image = Image::generate(4, 1, |x, y, t| {
            t[0] = if x == 0 && y == 0 { 1.0 } else { 0.0 }
        });
        let (side, rgba) = tiled(&image, 512, false);
        assert_eq!(side, 8);
        let lit = |x: usize, y: usize| rgba[(y * 8 + x) * 4] == 255;
        assert!(lit(0, 0) && lit(4, 0) && lit(0, 4) && lit(4, 4));
        assert!(!lit(1, 0));
    }

    #[test]
    fn a_sheet_has_a_label_band_under_each_row() {
        let image = Image::filled(8, 1, 0.5);
        let cells = vec![Cell::new("a", &image, false), Cell::new("b", &image, false)];
        let (w, h, canvas) = sheet(&cells, 2);
        assert_eq!((w, h), (PAD + 2 * (8 + PAD), PAD + 8 + LABEL_BAND + PAD));
        assert_eq!(canvas.len(), (w * h * 4) as usize);
    }
}
