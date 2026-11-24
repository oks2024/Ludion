//! The views sheet: the frame and each rendered view in one row, for judging them side by side.

use ludion_renderer::RenderedImage;

/// Pixels between two cells.
pub const GUTTER: u32 = 8;
/// The sheet's fill, visible in the gutters and below a short cell.
const FILL: [u8; 4] = [20, 20, 28, 255];

/// One row, top-aligned, at native sizes: `frame`, then each of `views` in order, `GUTTER` pixels apart
/// on a dark fill. Width is the cells' widths plus the gutters, height the tallest cell. Unlabelled.
///
/// # Example
/// ```
/// use ludion_engine::views_sheet;
/// use ludion_renderer::RenderedImage;
///
/// let cell = |w, h| RenderedImage { width: w, height: h, rgba: vec![255; (w * h * 4) as usize] };
/// let sheet = views_sheet(&cell(8, 4), &[cell(2, 6)]);
/// assert_eq!((sheet.width, sheet.height), (8 + 8 + 2, 6));
/// ```
#[must_use]
pub fn views_sheet(frame: &RenderedImage, views: &[RenderedImage]) -> RenderedImage {
    let cells: Vec<&RenderedImage> = std::iter::once(frame).chain(views).collect();
    let gutters = GUTTER * (cells.len() as u32 - 1);
    let width = cells.iter().map(|cell| cell.width).sum::<u32>() + gutters;
    let height = cells.iter().map(|cell| cell.height).max().unwrap_or(0);
    let mut rgba = FILL.repeat((width * height) as usize);
    let mut left = 0;
    for cell in cells {
        for row in 0..cell.height {
            let from = (row * cell.width * 4) as usize;
            let to = ((row * width + left) * 4) as usize;
            let len = (cell.width * 4) as usize;
            rgba[to..to + len].copy_from_slice(&cell.rgba[from..from + len]);
        }
        left += cell.width + GUTTER;
    }
    RenderedImage {
        width,
        height,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, texel: [u8; 4]) -> RenderedImage {
        RenderedImage {
            width,
            height,
            rgba: texel.repeat((width * height) as usize),
        }
    }

    fn at(image: &RenderedImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * image.width + x) * 4) as usize;
        image.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn the_row_is_the_cells_plus_gutters_by_the_tallest() {
        let sheet = views_sheet(
            &solid(10, 4, [1, 1, 1, 255]),
            &[solid(3, 6, [2, 2, 2, 255]), solid(5, 2, [3, 3, 3, 255])],
        );
        assert_eq!((sheet.width, sheet.height), (10 + 3 + 5 + 2 * GUTTER, 6));
        assert_eq!(sheet.rgba.len(), (sheet.width * sheet.height * 4) as usize);
    }

    #[test]
    fn cells_sit_at_their_offsets_top_aligned_on_the_fill() {
        let sheet = views_sheet(
            &solid(10, 4, [1, 1, 1, 255]),
            &[solid(3, 6, [2, 2, 2, 255])],
        );
        assert_eq!(at(&sheet, 0, 0), [1, 1, 1, 255]);
        assert_eq!(at(&sheet, 9, 3), [1, 1, 1, 255]);
        // Below the shorter frame and in the gutter: the fill.
        assert_eq!(at(&sheet, 0, 4), FILL);
        assert_eq!(at(&sheet, 10, 0), FILL);
        assert_eq!(at(&sheet, 10 + GUTTER - 1, 5), FILL);
        // The view starts after the gutter and runs the full height.
        assert_eq!(at(&sheet, 10 + GUTTER, 0), [2, 2, 2, 255]);
        assert_eq!(at(&sheet, 10 + GUTTER + 2, 5), [2, 2, 2, 255]);
    }

    #[test]
    fn a_frame_alone_is_the_frame() {
        let frame = solid(4, 3, [9, 8, 7, 255]);
        assert_eq!(views_sheet(&frame, &[]), frame);
    }
}
