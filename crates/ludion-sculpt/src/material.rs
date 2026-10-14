//! Per-part material weights and the palette channels they blend.
//!
//! The weights ride the same fold as the field: a `union` part with smooth-combine weight `t` sets
//! `w = w (1 - t) + t e_part`, a `subtract` or `intersect` leaves `w` as it is, so the surviving parts
//! keep their proportions. A vertex's color and `(roughness, metallic, emissive)` are the
//! weight-sum of its parts' palette entries, an unauthored channel taking the default material the
//! voxel mesher uses.

use ludion_scene::{SculptDocument, SculptOp};

use crate::grid::{SculptField, fold};

/// `roughness` of a palette entry that authors none.
pub(crate) const DEFAULT_ROUGHNESS: f32 = 1.0;
/// `metallic` of a palette entry that authors none.
pub(crate) const DEFAULT_METALLIC: f32 = 0.0;
/// `emissive` of a palette entry that authors none.
pub(crate) const DEFAULT_EMISSIVE: f32 = 0.0;

/// A part's resolved palette entry: linear RGBA and `(roughness, metallic, emissive)`.
#[derive(Clone, Copy)]
pub(crate) struct Channels {
    color: [f32; 4],
    material: [f32; 3],
}

/// Every part's channels in part order; `None` for a part with no material (a `subtract` or
/// `intersect`).
pub(crate) fn part_channels(doc: &SculptDocument) -> Vec<Option<Channels>> {
    doc.parts
        .iter()
        .map(|part| {
            let entry = doc.palette_entry(part.material.as_deref()?)?;
            Some(Channels {
                color: entry.color.map(|c| c as f32),
                material: [
                    entry.roughness.map_or(DEFAULT_ROUGHNESS, |r| r as f32),
                    entry.metallic.map_or(DEFAULT_METALLIC, |m| m as f32),
                    entry.emissive.map_or(DEFAULT_EMISSIVE, |e| e as f32),
                ],
            })
        })
        .collect()
}

/// Writes the material weight of every part at `p` into `weights` (one slot per part).
pub(crate) fn weights_at(field: &SculptField, p: [f32; 3], weights: &mut [f32]) {
    weights.fill(0.0);
    let mut value = f32::INFINITY;
    for (index, part) in field.parts.iter().enumerate() {
        let (folded, t) = fold(value, part, part.distance(p));
        value = folded;
        if part.op == SculptOp::Union {
            for w in weights.iter_mut() {
                *w *= 1.0 - t;
            }
            weights[index] += t;
        }
    }
}

/// The part with the largest weight, ties to the lower index; `None` when no weight is positive.
pub(crate) fn dominant_part(weights: &[f32]) -> Option<usize> {
    let mut dominant: Option<(usize, f32)> = None;
    for (index, &w) in weights.iter().enumerate() {
        if w > 0.0 && dominant.is_none_or(|(_, best)| w > best) {
            dominant = Some((index, w));
        }
    }
    dominant.map(|(index, _)| index)
}

/// Appends the weight-blended color (4 values) and material (3 values) of one vertex.
pub(crate) fn blend(
    weights: &[f32],
    channels: &[Option<Channels>],
    colors: &mut Vec<f32>,
    materials: &mut Vec<f32>,
) {
    let mut color = [0.0f32; 4];
    let mut material = [0.0f32; 3];
    for (&w, part) in weights.iter().zip(channels) {
        if let Some(part) = part {
            for (sum, c) in color.iter_mut().zip(part.color) {
                *sum += w * c;
            }
            for (sum, m) in material.iter_mut().zip(part.material) {
                *sum += w * m;
            }
        }
    }
    colors.extend_from_slice(&color);
    materials.extend_from_slice(&material);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dominant_part_breaks_ties_low_and_ignores_zero() {
        assert_eq!(dominant_part(&[0.25, 0.5, 0.5]), Some(1));
        assert_eq!(dominant_part(&[0.0, 0.0]), None);
        assert_eq!(dominant_part(&[]), None);
    }
}
