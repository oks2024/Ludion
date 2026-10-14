//! The one shelf-placement loop every atlas packer shares.
//!
//! Sort order, gutter policy and the width rule stay with the caller: they differ by design between
//! packers, and changing any of them moves committed atlas bytes.

/// Padded boxes placed on shelves, left to right, in the order given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shelves {
    /// Each box's top-left, in input order.
    pub origins: Vec<[u32; 2]>,
    /// The bottom of the last shelf, unaligned.
    pub height: u32,
}

/// Places `padded` boxes — each a full extent, gutters included — on shelves `width` wide.
///
/// A box that would cross `width` opens a new shelf, unless it is the shelf's first; a shelf is as
/// tall as its tallest box. `width` must be at least the widest box.
#[must_use]
pub fn place(padded: &[[u32; 2]], width: u32) -> Shelves {
    let mut origins = Vec::with_capacity(padded.len());
    let (mut cursor, mut shelf_y, mut shelf_height) = (0u32, 0u32, 0u32);
    for &[box_width, box_height] in padded {
        if cursor > 0 && cursor + box_width > width {
            shelf_y += shelf_height;
            cursor = 0;
            shelf_height = 0;
        }
        shelf_height = shelf_height.max(box_height);
        origins.push([cursor, shelf_y]);
        cursor += box_width;
    }
    Shelves {
        origins,
        height: shelf_y + shelf_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_fit_inside_the_width_without_overlap() {
        let boxes: Vec<[u32; 2]> = (0..40).map(|i| [7 + i % 7, 11 + i % 5]).collect();
        let width = 40;
        let shelves = place(&boxes, width);
        let rects: Vec<[u32; 4]> = shelves
            .origins
            .iter()
            .zip(&boxes)
            .map(|(o, s)| [o[0], o[1], s[0], s[1]])
            .collect();
        for (i, a) in rects.iter().enumerate() {
            assert!(a[0] + a[2] <= width && a[1] + a[3] <= shelves.height);
            for b in &rects[i + 1..] {
                let apart = a[0] + a[2] <= b[0]
                    || b[0] + b[2] <= a[0]
                    || a[1] + a[3] <= b[1]
                    || b[1] + b[3] <= a[1];
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn a_box_exactly_the_width_starts_its_own_shelf() {
        let shelves = place(&[[4, 3], [10, 5], [2, 2]], 10);
        assert_eq!(shelves.origins, vec![[0, 0], [0, 3], [0, 8]]);
        assert_eq!(shelves.height, 10);
    }

    #[test]
    fn no_boxes_have_no_height() {
        let shelves = place(&[], 8);
        assert!(shelves.origins.is_empty());
        assert_eq!(shelves.height, 0);
    }
}
