//! Sprite document → pixels: paint the glyph grids, then apply the `effects:` list in order.
//!
//! The bake is a pure function of the document. Every op is integer or `f32` arithmetic with a
//! fixed traversal order, every transcendental goes through [`ludion_core::math`], and the one
//! random stream is an explicit integer hash of `(seed, frame, x, y, channel)` — so a texel's value
//! never depends on the order texels are visited, and a document bakes to the same bytes on
//! Windows x86-64 and macOS arm64 forever.

use ludion_core::diagnostics::Diagnostic;
use ludion_core::math;
use ludion_scene::{
    GradientBlend, GradientDirection, OutlineSide, SpriteDocument, SpriteEffect, SpriteEffectOp,
    SpriteFrame,
};

use crate::image::FrameImage;

/// One frame of a baked sprite document: the pixels plus the pivot the atlas will carry.
#[derive(Clone, Debug, PartialEq)]
pub struct BakedFrame {
    /// The frame's name, as authored. The atlas mints its region name from it.
    pub name: String,
    /// The painted, effect-applied pixels in the bake's working format.
    pub image: FrameImage,
    /// The frame's own `pivot`, or the document's default (the frame's centre), in texels from the
    /// frame's top-left. Resolved here so the packer never has to reach back to the document.
    pub pivot: [f64; 2],
}

/// The 4×4 ordered-dispersed (Bayer) matrix the `dither` op thresholds against, indexed `[y][x]`.
const BAYER_4X4: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Bakes every frame of `doc`: paints its glyph grid from the palette, then applies each entry of
/// `doc.effects` in list order to the frames its `frames:` filter names.
///
/// # Errors
/// Every diagnostic [`ludion_scene::validate_sprite`] reports. The bake refuses to run on an
/// invalid document rather than guessing at one: an out-of-range colour or an effect naming a glyph
/// the palette does not define has no defined image, and silently picking one would put the guess
/// in a committed PNG.
pub fn bake(doc: &SpriteDocument) -> Result<Vec<BakedFrame>, Vec<Diagnostic>> {
    let diagnostics = ludion_scene::validate_sprite(doc);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let palette = palette_lookup(doc);
    let mut baked = Vec::with_capacity(doc.frames.len());
    for (frame_index, frame) in doc.frames.iter().enumerate() {
        let glyphs = glyph_grid(doc, frame);
        let mut image = paint(doc, &glyphs, &palette);
        for effect in &doc.effects {
            if !applies_to(effect, &frame.name) {
                continue;
            }
            apply(
                effect,
                &glyphs,
                &palette,
                doc.seed.unwrap_or(0),
                frame_index,
                &mut image,
            );
            clamp(&mut image);
        }
        baked.push(BakedFrame {
            name: frame.name.clone(),
            image,
            pivot: frame.pivot.unwrap_or_else(|| doc.default_pivot()),
        });
    }
    Ok(baked)
}

/// Palette colours indexed by their glyph's ASCII code, so a texel's colour is one array read.
///
/// Only `color` is read: [`ludion_scene::SpritePaletteEntry::emissive`] is the input to an emissive
/// map, never to the colour map this function feeds.
fn palette_lookup(doc: &SpriteDocument) -> [Option<[f32; 4]>; 128] {
    let mut lookup = [None; 128];
    for entry in &doc.palette {
        let Some(glyph) = entry.glyph.chars().next() else {
            continue;
        };
        let code = glyph as usize;
        if code < lookup.len() {
            lookup[code] = Some(entry.color.map(|component| component as f32));
        }
    }
    lookup
}

/// The frame's authored glyph per texel, `None` for a transparent texel.
///
/// This grid is the *identity* an effect's `glyphs:` filter tests against, and it never changes: a
/// texel an earlier effect repainted keeps the glyph it was authored as, and a texel an effect
/// created out of transparency (outline, shadow, blur) belongs to no glyph at all. Without that
/// rule, two effects naming the same glyph would silently mean different sets.
fn glyph_grid(doc: &SpriteDocument, frame: &SpriteFrame) -> Vec<Option<char>> {
    let [width, height] = doc.size;
    let mut grid = vec![None; (width as usize) * (height as usize)];
    for y in 0..height as usize {
        let Some(row) = frame.rows.get(y) else {
            continue;
        };
        for (x, glyph) in row.chars().take(width as usize).enumerate() {
            if glyph != ludion_scene::SPRITE_EMPTY_GLYPH {
                grid[y * (width as usize) + x] = Some(glyph);
            }
        }
    }
    grid
}

/// Paints a glyph grid: each glyph takes its palette entry's colour, `.` stays transparent black.
fn paint(
    doc: &SpriteDocument,
    glyphs: &[Option<char>],
    palette: &[Option<[f32; 4]>; 128],
) -> FrameImage {
    let [width, height] = doc.size;
    let mut image = FrameImage::new(width, height);
    for (index, glyph) in glyphs.iter().enumerate() {
        if let Some(color) = glyph.and_then(|glyph| palette.get(glyph as usize).copied().flatten())
        {
            image.texels[index] = color;
        }
    }
    image
}

/// Whether `effect` touches the frame named `name`. A missing `frames:` filter means every frame.
fn applies_to(effect: &SpriteEffect, name: &str) -> bool {
    match &effect.frames {
        None => true,
        Some(frames) => frames.iter().any(|frame| frame == name),
    }
}

/// The texels whose *authored* glyph is one of `names`.
fn glyph_mask(names: &[String], glyphs: &[Option<char>]) -> Vec<bool> {
    let wanted: Vec<char> = names
        .iter()
        .filter_map(|name| name.chars().next())
        .collect();
    glyphs
        .iter()
        .map(|glyph| glyph.is_some_and(|glyph| wanted.contains(&glyph)))
        .collect()
}

/// The texels that are opaque right now — the default target of an effect with no `glyphs:` filter.
///
/// "Right now" is deliberate: an effect with no filter works on what the previous effects left, so
/// an outline follows a shadow's silhouette rather than the original glyph grid's.
fn opaque_mask(image: &FrameImage) -> Vec<bool> {
    image.texels.iter().map(|texel| texel[3] > 0.0).collect()
}

/// The target set of `effect`, by that op's own default rule.
fn target_mask(effect: &SpriteEffect, glyphs: &[Option<char>], image: &FrameImage) -> Vec<bool> {
    match &effect.glyphs {
        Some(names) => glyph_mask(names, glyphs),
        // `blur` with no filter blurs the whole frame *including* transparent texels — that spread
        // into transparency is what makes a glow. Every other op defaults to what is already drawn.
        None if matches!(effect.op, SpriteEffectOp::Blur { .. }) => vec![true; image.texels.len()],
        None => opaque_mask(image),
    }
}

fn apply(
    effect: &SpriteEffect,
    glyphs: &[Option<char>],
    palette: &[Option<[f32; 4]>; 128],
    seed: u64,
    frame_index: usize,
    image: &mut FrameImage,
) {
    let mask = target_mask(effect, glyphs, image);
    match effect.op {
        SpriteEffectOp::Gradient {
            from,
            to,
            direction,
            blend,
        } => gradient(image, &mask, from, to, direction, blend),
        SpriteEffectOp::Outline { color, side } => outline(image, &mask, color, side),
        SpriteEffectOp::Shade { light, strength } => shade(image, &mask, light, strength),
        SpriteEffectOp::Shadow { offset, color } => shadow(image, &mask, offset, color),
        SpriteEffectOp::Dither { density } => {
            dither(image, glyphs, palette, effect.glyphs.as_deref(), density);
        }
        SpriteEffectOp::Noise { amount } => noise(image, &mask, seed, frame_index, amount),
        SpriteEffectOp::Blur { radius } => blur(image, &mask, radius),
        SpriteEffectOp::Glow {
            color,
            radius_px,
            strength,
        } => glow(image, &mask, color, radius_px, strength),
    }
}

/// Clamps every channel of every texel to `0..=1`, run after each op so no op ever reads a value
/// outside the range it was written to expect.
fn clamp(image: &mut FrameImage) {
    for texel in &mut image.texels {
        for channel in texel.iter_mut() {
            *channel = channel.clamp(0.0, 1.0);
        }
    }
}

/// The `[left, top, right, bottom]` inclusive texel bounds of `mask`, or `None` when it is empty.
fn bounding_box(image: &FrameImage, mask: &[bool]) -> Option<[u32; 4]> {
    let width = image.width;
    let mut bounds: Option<[u32; 4]> = None;
    for (index, _) in mask.iter().enumerate().filter(|(_, on)| **on) {
        let x = (index as u32) % width;
        let y = (index as u32) / width;
        bounds = Some(match bounds {
            None => [x, y, x, y],
            Some([left, top, right, bottom]) => {
                [left.min(x), top.min(y), right.max(x), bottom.max(y)]
            }
        });
    }
    bounds
}

/// `gradient`: ramps between two linear RGBA colours across the target set's bounding box.
///
/// `t` runs over the *target set's* box, not the frame's, so a gradient over one glyph fills that
/// glyph's own extent whatever else the frame holds. Vertical is `(y - top) / (bottom - top)` and
/// horizontal the same on `x`, both `0` when the box is one texel across that axis; radial is the
/// distance from the box centre over half the box diagonal, clamped to `0..=1`. `replace` takes the
/// ramp's RGB and scales the texel's alpha by the ramp's; `multiply` scales all four.
fn gradient(
    image: &mut FrameImage,
    mask: &[bool],
    from: [f64; 4],
    to: [f64; 4],
    direction: GradientDirection,
    blend: GradientBlend,
) {
    let Some([left, top, right, bottom]) = bounding_box(image, mask) else {
        return;
    };
    let from = from.map(|component| component as f32);
    let to = to.map(|component| component as f32);
    let span_x = (right - left) as f32;
    let span_y = (bottom - top) as f32;
    let centre_x = (left as f32 + right as f32) / 2.0;
    let centre_y = (top as f32 + bottom as f32) / 2.0;
    let half_diagonal = 0.5 * math::sqrt(span_x * span_x + span_y * span_y);

    let width = image.width;
    for index in mask
        .iter()
        .enumerate()
        .filter(|(_, on)| **on)
        .map(|(i, _)| i)
    {
        let x = ((index as u32) % width) as f32;
        let y = ((index as u32) / width) as f32;
        let t = match direction {
            GradientDirection::Vertical if span_y > 0.0 => (y - top as f32) / span_y,
            GradientDirection::Horizontal if span_x > 0.0 => (x - left as f32) / span_x,
            GradientDirection::Radial if half_diagonal > 0.0 => {
                let dx = x - centre_x;
                let dy = y - centre_y;
                (math::sqrt(dx * dx + dy * dy) / half_diagonal).clamp(0.0, 1.0)
            }
            _ => 0.0,
        };
        let ramp = [
            from[0] + (to[0] - from[0]) * t,
            from[1] + (to[1] - from[1]) * t,
            from[2] + (to[2] - from[2]) * t,
            from[3] + (to[3] - from[3]) * t,
        ];
        let texel = &mut image.texels[index];
        match blend {
            GradientBlend::Replace => {
                texel[0] = ramp[0];
                texel[1] = ramp[1];
                texel[2] = ramp[2];
                texel[3] *= ramp[3];
            }
            GradientBlend::Multiply => {
                for channel in 0..4 {
                    texel[channel] *= ramp[channel];
                }
            }
        }
    }
}

/// `outline`: a one-texel border along the target set's silhouette.
///
/// `outside` paints every texel *not* in the set that has a 4-neighbour in it; the frame edge clips,
/// so an author leaves a one-texel margin. `inside` paints every texel *in* the set that has a
/// 4-neighbour outside it, counting past the frame edge as outside. The border set is decided from
/// the silhouette before anything is written, so an outline never grows into its own output.
fn outline(image: &mut FrameImage, mask: &[bool], color: [f64; 4], side: OutlineSide) {
    let color = color.map(|component| component as f32);
    let width = image.width as i32;
    let height = image.height as i32;
    let inside_set =
        |x: i32, y: i32| -> bool { image.index(x, y).is_some_and(|neighbour| mask[neighbour]) };

    let mut border = vec![false; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            let neighbours = [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)];
            border[index] = match side {
                OutlineSide::Outside => {
                    !mask[index] && neighbours.iter().any(|&(nx, ny)| inside_set(nx, ny))
                }
                OutlineSide::Inside => {
                    mask[index] && neighbours.iter().any(|&(nx, ny)| !inside_set(nx, ny))
                }
            };
        }
    }
    for (index, _) in border.iter().enumerate().filter(|(_, on)| **on) {
        image.texels[index] = color;
    }
}

/// `shade`: a cheap silhouette-derived bevel — lighten the edge facing the light, darken the one
/// away from it.
///
/// With `d = (signum(light.x), signum(light.y))` in texel space (`+y` down): if the neighbour at
/// `p + d` is transparent the texel is a lit edge and its RGB moves toward white by `strength`;
/// otherwise, if the neighbour at `p - d` is transparent it is a shadowed edge and its RGB scales by
/// `1 - strength`; otherwise it is interior and unchanged. Neighbours are read from the silhouette
/// as it stood before this op, so the bevel is one texel deep rather than a flood.
fn shade(image: &mut FrameImage, mask: &[bool], light: [f64; 2], strength: f64) {
    let before = image.clone();
    let dx = signum(light[0]);
    let dy = signum(light[1]);
    let strength = strength as f32;
    let width = image.width as i32;

    for index in mask
        .iter()
        .enumerate()
        .filter(|(_, on)| **on)
        .map(|(i, _)| i)
    {
        let x = (index as i32) % width;
        let y = (index as i32) / width;
        let texel = &mut image.texels[index];
        if before.get(x + dx, y + dy)[3] == 0.0 {
            for channel in &mut texel[0..3] {
                *channel += (1.0 - *channel) * strength;
            }
        } else if before.get(x - dx, y - dy)[3] == 0.0 {
            for channel in &mut texel[0..3] {
                *channel *= 1.0 - strength;
            }
        }
    }
}

/// `-1`, `0` or `1`. Not `f64::signum`, which reports `1.0` for `+0.0` — a light with one zero
/// component would then bevel along an axis it does not point down.
fn signum(value: f64) -> i32 {
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}

/// `shadow`: an offset copy of the silhouette, painted behind it.
///
/// Every texel of the target set casts to `p + offset`, and only lands there if that texel is
/// transparent — a shadow never overwrites the sprite it belongs to. The offset is a rigid
/// translation, so no two source texels compete for one destination.
fn shadow(image: &mut FrameImage, mask: &[bool], offset: [i32; 2], color: [f64; 4]) {
    let before = image.clone();
    let color = color.map(|component| component as f32);
    let width = image.width as i32;

    for index in mask
        .iter()
        .enumerate()
        .filter(|(_, on)| **on)
        .map(|(i, _)| i)
    {
        let x = (index as i32) % width + offset[0];
        let y = (index as i32) / width + offset[1];
        if let Some(destination) = image.index(x, y)
            && before.texels[destination][3] == 0.0
        {
            image.texels[destination] = color;
        }
    }
}

/// `dither`: an ordered two-tone checker between the effect's exactly-two glyphs.
///
/// A texel authored as the first glyph takes the second glyph's colour when the 4×4 Bayer threshold
/// at `(x % 4, y % 4)` falls under `density`; texels of the second glyph are left alone. Ordered
/// rather than random so the pattern is stable under any edit that does not move the texel.
fn dither(
    image: &mut FrameImage,
    glyphs: &[Option<char>],
    palette: &[Option<[f32; 4]>; 128],
    names: Option<&[String]>,
    density: f64,
) {
    // `validate_sprite` guarantees exactly two glyphs, both naming palette entries.
    let Some([first, second]) = names.and_then(|names| <&[String; 2]>::try_from(names).ok()) else {
        return;
    };
    let (Some(from), Some(to)) = (first.chars().next(), second.chars().next()) else {
        return;
    };
    let Some(color) = palette.get(to as usize).copied().flatten() else {
        return;
    };
    let density = density as f32;
    let width = image.width as usize;

    for (index, glyph) in glyphs.iter().enumerate() {
        if *glyph != Some(from) {
            continue;
        }
        let threshold = BAYER_4X4[(index / width) % 4][(index % width) % 4];
        if (threshold as f32 + 0.5) / 16.0 < density {
            image.texels[index] = color;
        }
    }
}

/// `noise`: seeded per-texel RGB jitter in `[-amount, amount]`.
///
/// The value at a texel is a pure function of `(seed, frame index, x, y, channel)` through
/// [`texel_random`], so it does not depend on the order texels are visited, on how many effects ran
/// before, or on anything the platform decides. Alpha is never jittered — a noisy silhouette is a
/// different sprite, not a shaded one.
fn noise(image: &mut FrameImage, mask: &[bool], seed: u64, frame_index: usize, amount: f64) {
    let amount = amount as f32;
    let width = image.width as u64;

    for index in mask
        .iter()
        .enumerate()
        .filter(|(_, on)| **on)
        .map(|(i, _)| i)
    {
        let x = (index as u64) % width;
        let y = (index as u64) / width;
        for channel in 0..3usize {
            let unit = texel_random(seed, frame_index as u64, x, y, channel as u64);
            image.texels[index][channel] =
                (image.texels[index][channel] + (unit * 2.0 - 1.0) * amount).clamp(0.0, 1.0);
        }
    }
}

/// A `[0, 1)` value drawn from the document seed and a texel address.
///
/// Written as an explicit integer hash rather than a sequential RNG precisely so the stream has no
/// position: there is nothing to advance, and nothing to get out of step. A document with no `seed`
/// draws from `0` — unseeded still means one fixed image, never an arbitrary one.
fn texel_random(seed: u64, frame_index: u64, x: u64, y: u64, channel: u64) -> f32 {
    let mut state = splitmix64(seed);
    for value in [frame_index, x, y, channel] {
        state = splitmix64(state ^ splitmix64(value));
    }
    // The top 24 bits divide exactly by 2^24, so every draw is representable in `f32`.
    ((state >> 40) as f32) / 16_777_216.0
}

/// SplitMix64 — a fixed, portable integer mixer. Not `DeterministicRng`, because noise needs a
/// *stateless* draw addressed by texel, not a sequence.
fn splitmix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `blur`: a box blur of radius `r` over the whole frame, written back only to the target set.
///
/// The average is taken in **premultiplied** space with out-of-bounds texels counted as transparent
/// — averaging straight colours would pull the colour of fully transparent texels into the result
/// and halo the silhouette. The window sum is the direct `(2r+1)²` sum in a fixed row-major order,
/// not a separable two-pass, so the result has one definition rather than one per pass split.
fn blur(image: &mut FrameImage, mask: &[bool], radius: u32) {
    let radius = radius as i32;
    let width = image.width as i32;
    let height = image.height as i32;
    let count = ((2 * radius + 1) * (2 * radius + 1)) as f32;

    let premultiplied: Vec<[f32; 4]> = image
        .texels
        .iter()
        .map(|texel| {
            let alpha = texel[3];
            [texel[0] * alpha, texel[1] * alpha, texel[2] * alpha, alpha]
        })
        .collect();

    let mut blurred = vec![[0.0f32; 4]; image.texels.len()];
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0.0f32; 4];
            for oy in -radius..=radius {
                for ox in -radius..=radius {
                    if let Some(index) = image.index(x + ox, y + oy) {
                        for channel in 0..4 {
                            sum[channel] += premultiplied[index][channel];
                        }
                    }
                }
            }
            let alpha = sum[3] / count;
            let unpremultiplied = if alpha > 0.0 {
                [
                    (sum[0] / count) / alpha,
                    (sum[1] / count) / alpha,
                    (sum[2] / count) / alpha,
                    alpha,
                ]
            } else {
                [0.0; 4]
            };
            blurred[(y * width + x) as usize] = unpremultiplied;
        }
    }

    for (index, texel) in blurred.into_iter().enumerate() {
        if mask[index] {
            image.texels[index] = texel;
        }
    }
}

/// `glow`: a soft halo grown *outside* the silhouette — the fractional-radius glow primitive.
///
/// `mask` is the *source* silhouette (the whole frame's opaque texels, or one glyph set under a
/// `glyphs:` filter). Every texel that was transparent before this op accumulates a contribution
/// from each opaque source texel within `radius_px`, weighted by a parabolic `1 - d²/r²` falloff on
/// the **squared** texel distance — squared so the weight is an exact binary fraction of integer
/// offsets, with no `sqrt` to disagree across platforms. The accumulated coverage times `strength`
/// is the halo's alpha; the colour is `color` when set, else the contribution-weighted average of
/// the contributing sources' own colours, so a red core glows red. Opaque texels are never written,
/// so the core — and its emissive palette values — stay crisp behind the halo.
fn glow(
    image: &mut FrameImage,
    mask: &[bool],
    color: Option<[f64; 4]>,
    radius_px: f64,
    strength: f64,
) {
    let before = image.clone();
    let radius = radius_px as f32;
    let radius_sq = radius * radius;
    let strength = strength as f32;
    let bound = radius_px.ceil() as i32;
    let tint = color.map(|components| components.map(|component| component as f32));
    let width = image.width as i32;

    for index in 0..image.texels.len() {
        // Only a transparent texel takes a halo — an opaque core texel is left exactly as it stood,
        // so the core stays crisp rather than being averaged away as `blur` would.
        if before.texels[index][3] > 0.0 {
            continue;
        }
        let dx = (index as i32) % width;
        let dy = (index as i32) / width;
        let mut weight = 0.0f32;
        let mut rgb = [0.0f32; 3];
        for oy in -bound..=bound {
            for ox in -bound..=bound {
                let distance_sq = (ox * ox + oy * oy) as f32;
                if distance_sq >= radius_sq {
                    continue;
                }
                let Some(source) = image.index(dx + ox, dy + oy) else {
                    continue;
                };
                let source_alpha = before.texels[source][3];
                if !mask[source] || source_alpha <= 0.0 {
                    continue;
                }
                let contribution = (1.0 - distance_sq / radius_sq) * source_alpha;
                weight += contribution;
                for (accum, &channel) in rgb.iter_mut().zip(&before.texels[source][0..3]) {
                    *accum += contribution * channel;
                }
            }
        }
        if weight > 0.0 {
            let halo = match tint {
                Some(tint) => [tint[0], tint[1], tint[2]],
                None => [rgb[0] / weight, rgb[1] / weight, rgb[2] / weight],
            };
            image.texels[index] = [halo[0], halo[1], halo[2], weight * strength];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_is_pinned() {
        // A change here re-bakes every committed atlas that uses `noise`, so the constant is the
        // gate: an accidental edit to the mixer fails this test rather than a golden.
        assert_eq!(splitmix64(0), 16_294_208_416_658_607_535);
        assert_eq!(splitmix64(1), 10_451_216_379_200_822_465);
    }

    #[test]
    fn signum_treats_zero_as_zero() {
        assert_eq!(signum(0.0), 0);
        assert_eq!(signum(-0.0), 0);
        assert_eq!(signum(0.5), 1);
        assert_eq!(signum(-0.5), -1);
    }
}
