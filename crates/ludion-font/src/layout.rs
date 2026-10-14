//! Text layout: advance + sparse kerning + greedy word wrap, and positioned glyph quads.
//!
//! Every width is an integer in font units; a unit→pixel conversion is one `units as f32 * unit`
//! multiply with `unit = size / line_units`, so for the 8×8 face the result is bit-identical to the
//! square-cell model (`n` glyphs → `8n * (size / 8) == n * size`, exactly).

use std::borrow::Cow;

use crate::FontFace;

/// Which way `+Y` points when quads are placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YAxis {
    /// Screen space: lines advance toward `+Y`.
    Down,
    /// World space: lines advance toward `-Y`.
    Up,
}

/// One laid-out glyph: its char and its advance with the kerning toward the next glyph folded in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaidGlyph {
    /// The character as authored, except a tab, which lays out as a space (aliases are resolved at
    /// quad time).
    pub ch: char,
    /// Advance in font units, kerning to the next glyph of the line included.
    pub advance_units: i32,
}

/// One line of a [`TextLayout`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutLine {
    /// The line's glyphs in order.
    pub glyphs: Vec<LaidGlyph>,
    /// The sum of the glyph advances, in font units.
    pub width_units: i64,
}

/// One positioned glyph quad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphQuad {
    /// The character this quad draws.
    pub ch: char,
    /// `[left, top, right, bottom]` in the caller's axis (`top` is the edge sampled at `uv[1]`).
    pub pos: [f32; 4],
    /// `[u0, v0, u1, v1]`, `v0` the atlas row of the glyph's top.
    pub uv: [f32; 4],
}

/// Text shaped against a face: lines of glyphs, ready to measure or to turn into quads.
#[derive(Clone, Debug)]
pub struct TextLayout<'f> {
    face: &'f FontFace,
    lines: Vec<LayoutLine>,
    size: f32,
}

/// `text` on one line: every line break (`"\r\n"`, `'\r'`, `'\n'`) becomes one space. Borrowed when
/// there is none.
pub fn fold_line_breaks(text: &str) -> Cow<'_, str> {
    if !text.contains(['\r', '\n']) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", " ").replace(['\r', '\n'], " "))
}

/// `text` with every line break as `'\n'` and every tab as a space; borrowed when there is neither.
fn normalized(text: &str) -> Cow<'_, str> {
    if !text.contains(['\r', '\t']) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', " "),
    )
}

fn is_break_after(ch: char) -> bool {
    ch == '-' || ch == '\u{2014}'
}

/// Width of `chars` as one line: advances plus the kerning between neighbours.
fn width_units(face: &FontFace, chars: &[char]) -> i64 {
    let mut total = 0i64;
    for (i, &ch) in chars.iter().enumerate() {
        total += i64::from(face.advance(ch));
        if let Some(&next) = chars.get(i + 1) {
            total += i64::from(face.kerning(ch, next));
        }
    }
    total
}

/// The greedy wrap over one paragraph; each returned line is a char run.
fn wrap_paragraph(face: &FontFace, paragraph: &str, limit_units: i64, lines: &mut Vec<Vec<char>>) {
    let advance = |ch: char| i64::from(face.advance(ch));
    let kern = |a: char, b: char| i64::from(face.kerning(a, b));
    let mut current: Vec<char> = Vec::new();
    let mut current_width = 0i64;
    for word in paragraph.split(' ').filter(|word| !word.is_empty()) {
        let mut pieces: Vec<Vec<char>> = vec![Vec::new()];
        for ch in word.chars() {
            if let Some(piece) = pieces.last_mut() {
                piece.push(ch);
            }
            if is_break_after(ch) {
                pieces.push(Vec::new());
            }
        }
        for (index, piece) in pieces.iter().enumerate() {
            let Some(&first) = piece.first() else {
                continue;
            };
            if let Some(&last) = current.last() {
                let piece_width = width_units(face, piece);
                let joined = if index == 0 {
                    kern(last, ' ') + advance(' ') + kern(' ', first) + piece_width
                } else {
                    kern(last, first) + piece_width
                };
                if current_width + joined <= limit_units {
                    if index == 0 {
                        current.push(' ');
                    }
                    current.extend_from_slice(piece);
                    current_width += joined;
                    continue;
                }
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            // A line always takes at least one glyph; the last chunk keeps filling.
            let mut rest = piece.as_slice();
            while !rest.is_empty() {
                let mut take = 1;
                let mut taken_width = advance(rest[0]);
                while take < rest.len() {
                    let grown =
                        taken_width + kern(rest[take - 1], rest[take]) + advance(rest[take]);
                    if grown > limit_units {
                        break;
                    }
                    taken_width = grown;
                    take += 1;
                }
                if take == rest.len() {
                    current = rest.to_vec();
                    current_width = taken_width;
                } else {
                    lines.push(rest[..take].to_vec());
                }
                rest = &rest[take..];
            }
        }
    }
    lines.push(current);
}

fn laid_line(face: &FontFace, chars: &[char]) -> LayoutLine {
    let glyphs: Vec<LaidGlyph> = chars
        .iter()
        .enumerate()
        .map(|(i, &ch)| LaidGlyph {
            ch,
            advance_units: face.advance(ch)
                + chars.get(i + 1).map_or(0, |&next| face.kerning(ch, next)),
        })
        .collect();
    let width_units = glyphs.iter().map(|g| i64::from(g.advance_units)).sum();
    LayoutLine {
        glyphs,
        width_units,
    }
}

/// Shapes `text` against `face` at `size` (the line height, in the caller's units).
///
/// `wrap_width` is in the same units as `size`; at or below zero, or with a non-finite `size` or
/// `wrap_width`, wrapping is off and every char stays verbatim. Wrapping is greedy by advance
/// units: `'\n'`, `"\r\n"` and a lone `'\r'` are paragraph breaks, a `'\t'` is a space, words are
/// runs of non-`' '` chars (space runs collapse, leading and trailing spaces drop), a word too long
/// for a line hard-breaks, and inside a word a line may also break after `-` and `—` — never at
/// U+00A0. A line always takes at least one glyph. Empty `text`, or text that wraps to nothing, has
/// no lines.
pub fn layout<'f>(face: &'f FontFace, text: &str, size: f32, wrap_width: f32) -> TextLayout<'f> {
    let text = normalized(text);
    let mut chars_by_line: Vec<Vec<char>> = Vec::new();
    if !text.is_empty() {
        if !wrap_width.is_finite() || wrap_width <= 0.0 || !size.is_finite() || size <= 0.0 {
            chars_by_line.extend(text.split('\n').map(|line| line.chars().collect()));
        } else {
            let limit_units = ((wrap_width / size) * face.line_units() as f32).floor() as i64;
            for paragraph in text.split('\n') {
                wrap_paragraph(face, paragraph, limit_units, &mut chars_by_line);
            }
        }
    }
    // Text that wraps down to nothing (only spaces) is the empty block, as the empty string is.
    if chars_by_line.len() == 1 && chars_by_line[0].is_empty() {
        chars_by_line.clear();
    }
    TextLayout {
        face,
        lines: chars_by_line
            .iter()
            .map(|chars| laid_line(face, chars))
            .collect(),
        size,
    }
}

impl TextLayout<'_> {
    /// The laid-out lines.
    pub fn lines(&self) -> &[LayoutLine] {
        &self.lines
    }

    /// The number of lines.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The block's `[width, height]` at the layout size: the widest line, and `line_count × size`.
    pub fn extent(&self) -> [f32; 2] {
        let unit = self.size / self.face.line_units() as f32;
        let widest = self.lines.iter().map(|l| l.width_units).max().unwrap_or(0);
        [widest as f32 * unit, self.lines.len() as f32 * self.size]
    }

    /// Line `i` as a string.
    ///
    /// # Panics
    /// If `i >= line_count()`.
    pub fn line_text(&self, i: usize) -> String {
        self.lines[i].glyphs.iter().map(|g| g.ch).collect()
    }

    /// The quads of every inked glyph, with the pen starting at `origin` and lines `size` apart.
    ///
    /// `size` is the draw size, separate from the layout size so a caller can wrap at canvas units
    /// and draw at pixels. A char with no glyph draws the notdef (a quad only if it has an image);
    /// the pen always advances.
    pub fn quads(&self, origin: [f32; 2], size: f32, axis: YAxis) -> Vec<GlyphQuad> {
        let doc = self.face.document();
        let unit = size / self.face.line_units() as f32;
        let (atlas_w, atlas_h) = (doc.atlas.width as f32, doc.atlas.height as f32);
        let ascender = doc.ascender;
        let mut quads = Vec::new();
        let mut pen_y = origin[1];
        for (line_index, line) in self.lines.iter().enumerate() {
            if line_index > 0 {
                match axis {
                    YAxis::Down => pen_y += size,
                    YAxis::Up => pen_y -= size,
                }
            }
            let mut pen_x = origin[0];
            for laid in &line.glyphs {
                let (bounds, rect) = match self.face.glyph(laid.ch) {
                    Some(glyph) => (glyph.bounds, glyph.rect),
                    None => (doc.notdef.bounds, doc.notdef.rect),
                };
                if let (Some(b), Some(r)) = (bounds, rect) {
                    let top_off = (ascender - b[3]) as f32 * unit;
                    let bottom_off = (ascender - b[1]) as f32 * unit;
                    let (top, bottom) = match axis {
                        YAxis::Down => (pen_y + top_off, pen_y + bottom_off),
                        YAxis::Up => (pen_y - top_off, pen_y - bottom_off),
                    };
                    quads.push(GlyphQuad {
                        ch: laid.ch,
                        pos: [
                            pen_x + b[0] as f32 * unit,
                            top,
                            pen_x + b[2] as f32 * unit,
                            bottom,
                        ],
                        uv: [
                            r[0] as f32 / atlas_w,
                            r[1] as f32 / atlas_h,
                            (r[0] + r[2]) as f32 / atlas_w,
                            (r[1] + r[3]) as f32 / atlas_h,
                        ],
                    });
                }
                pen_x += laid.advance_units as f32 * unit;
            }
        }
        quads
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROPORTIONAL: &str = "\
format: 1
kind: font
image: p.png
mode: bitmap
units_per_em: 1000
ascender: 800
descender: -200
line_gap: 0
atlas:
  width: 100
  height: 50
  em_px: 50
notdef:
  advance: 300
glyphs:
  - char: \" \"
    advance: 250
  - char: \"-\"
    advance: 300
  - char: A
    advance: 700
    bounds:
      - 10
      - 0
      - 690
      - 800
    rect:
      - 0
      - 0
      - 34
      - 40
  - char: B
    advance: 600
    bounds:
      - 10
      - 0
      - 590
      - 800
    rect:
      - 34
      - 0
      - 30
      - 40
  - char: C
    advance: 500
  - char: V
    advance: 700
    bounds:
      - 0
      - 0
      - 700
      - 800
    rect:
      - 64
      - 0
      - 35
      - 40
  - char: \"\\u00A0\"
    advance: 250
  - char: \"\\u2014\"
    advance: 900
aliases:
  a: A
kerning:
  A:
    V: -100
";

    fn face() -> FontFace {
        FontFace::new(ludion_scene::parse_font(PROPORTIONAL).expect("parses")).expect("valid")
    }

    fn texts(l: &TextLayout) -> Vec<String> {
        (0..l.line_count()).map(|i| l.line_text(i)).collect()
    }

    /// Frozen copy of the quadratic wrap: the oracle the linear one must match bit for bit.
    fn reference_wrap(
        face: &FontFace,
        paragraph: &str,
        limit_units: i64,
        lines: &mut Vec<Vec<char>>,
    ) {
        let fits = |chars: &[char]| width_units(face, chars) <= limit_units;
        let mut current: Vec<char> = Vec::new();
        for word in paragraph.split(' ').filter(|word| !word.is_empty()) {
            let mut pieces: Vec<Vec<char>> = vec![Vec::new()];
            for ch in word.chars() {
                if let Some(piece) = pieces.last_mut() {
                    piece.push(ch);
                }
                if is_break_after(ch) {
                    pieces.push(Vec::new());
                }
            }
            for (index, piece) in pieces.iter().enumerate() {
                if piece.is_empty() {
                    continue;
                }
                if !current.is_empty() {
                    let mut candidate = current.clone();
                    if index == 0 {
                        candidate.push(' ');
                    }
                    candidate.extend_from_slice(piece);
                    if fits(&candidate) {
                        current = candidate;
                        continue;
                    }
                    lines.push(std::mem::take(&mut current));
                }
                let mut rest = piece.as_slice();
                while !rest.is_empty() {
                    let mut take = 1;
                    while take < rest.len() && fits(&rest[..=take]) {
                        take += 1;
                    }
                    if take == rest.len() {
                        current = rest.to_vec();
                    } else {
                        lines.push(rest[..take].to_vec());
                    }
                    rest = &rest[take..];
                }
            }
        }
        lines.push(current);
    }

    #[test]
    fn the_linear_wrap_matches_the_quadratic_one() {
        let f = face();
        let corpus = [
            "",
            "AV",
            "AVAV AV",
            "AV AV AV AV AV",
            "BB-BB-BB CC-CC",
            "BB\u{2014}BB\u{2014}B A",
            "BB\u{A0}BB AA\u{A0}VV",
            "AAAAAAAAAAAAAAAAAAAA",
            "A    V   AV  ",
            "  leading and trailing  ",
            "- - - - -",
            "AVAVAVAVAVAVAV BBBBBBBB-CCCCCCCC\u{2014}AAVV V A",
            "~~~ ~ AV~",
        ];
        for limit in [1, 250, 700, 1299, 1300, 1301, 1400, 2650, 5000] {
            for text in corpus {
                let mut expected = Vec::new();
                reference_wrap(&f, text, limit, &mut expected);
                let mut got = Vec::new();
                wrap_paragraph(&f, text, limit, &mut got);
                assert_eq!(got, expected, "limit {limit}, text {text:?}");
            }
        }
    }

    #[test]
    fn kerning_shifts_extent_and_the_next_quad() {
        let f = face();
        let l = layout(&f, "AV", 10.0, 0.0);
        // unit = 10 / 1000; width = 700 - 100 + 700.
        assert_eq!(l.lines()[0].width_units, 1300);
        assert_eq!(l.extent(), [1300.0 * 0.01, 10.0]);
        let q = l.quads([0.0, 0.0], 10.0, YAxis::Down);
        assert_eq!(q.len(), 2);
        assert_eq!(q[1].pos[0], 600.0 * 0.01); // V starts after A's kerned advance
    }

    #[test]
    fn wrap_counts_units_not_chars() {
        let f = face();
        // "AA" is 1400 units: too wide for 1300 by units; "BB" (1200) fits.
        assert_eq!(texts(&layout(&f, "AA", 10.0, 13.0)), ["A", "A"]);
        assert_eq!(texts(&layout(&f, "BB", 10.0, 13.0)), ["BB"]);
        // Seven chars fit by units (1950) where 2 columns of 10-unit cells would not.
        let narrow = layout(&f, "- - - -", 10.0, 20.0);
        assert_eq!(texts(&narrow), ["- - - -"]);
    }

    #[test]
    fn a_word_breaks_after_hyphen_and_em_dash() {
        let f = face();
        assert_eq!(texts(&layout(&f, "BB-BB", 10.0, 15.0)), ["BB-", "BB"]);
        assert_eq!(
            texts(&layout(&f, "BB\u{2014}BB", 10.0, 21.0)),
            ["BB\u{2014}", "BB"]
        );
        // The continuation joins without a space when it fits.
        assert_eq!(texts(&layout(&f, "C-C", 10.0, 30.0)), ["C-C"]);
    }

    #[test]
    fn a_no_break_space_is_never_a_break() {
        let f = face();
        // One word: it hard-breaks by units, never dropping or collapsing the NBSP.
        assert_eq!(
            texts(&layout(&f, "BB\u{A0}BB", 10.0, 15.0)),
            ["BB\u{A0}", "BB"]
        );
        assert_eq!(texts(&layout(&f, "BB\u{A0}BB", 10.0, 40.0)), ["BB\u{A0}BB"]);
    }

    #[test]
    fn hard_newlines_and_empty_paragraphs() {
        let f = face();
        assert_eq!(layout(&f, "A\n\nB", 10.0, 0.0).line_count(), 3);
        assert_eq!(layout(&f, "A\n\nB", 10.0, 100.0).line_count(), 3);
        assert_eq!(texts(&layout(&f, "A\n\nB", 10.0, 100.0)), ["A", "", "B"]);
    }

    #[test]
    fn crlf_and_a_lone_cr_are_line_breaks() {
        let f = face();
        for wrap in [0.0, 100.0] {
            assert_eq!(texts(&layout(&f, "A\r\nB", 10.0, wrap)), ["A", "B"]);
            assert_eq!(texts(&layout(&f, "A\rB", 10.0, wrap)), ["A", "B"]);
            assert_eq!(texts(&layout(&f, "A\r\r\nB", 10.0, wrap)), ["A", "", "B"]);
            assert_eq!(
                texts(&layout(&f, "A\r", 10.0, wrap)),
                texts(&layout(&f, "A\n", 10.0, wrap))
            );
        }
    }

    #[test]
    fn a_tab_breaks_and_advances_like_a_space() {
        let f = face();
        let l = layout(&f, "A\tB", 10.0, 0.0);
        assert_eq!(l.line_text(0), "A B");
        assert_eq!(l.lines()[0].width_units, 1550);
        assert_eq!(texts(&layout(&f, "BB\tBB", 10.0, 15.0)), ["BB", "BB"]);
    }

    #[test]
    fn folding_line_breaks_gives_one_line() {
        assert_eq!(fold_line_breaks("A\r\nB\rC\nD"), "A B C D");
        assert!(matches!(fold_line_breaks("AB"), Cow::Borrowed("AB")));
    }

    #[test]
    fn line_height_is_the_size() {
        let f = face();
        let l = layout(&f, "A\nB", 10.0, 0.0);
        assert_eq!(l.extent()[1], 20.0);
        let q = l.quads([0.0, 5.0], 10.0, YAxis::Down);
        assert_eq!(q[1].pos[1] - q[0].pos[1], 10.0);
    }

    #[test]
    fn empty_text_has_no_lines_and_no_quads() {
        let f = face();
        for wrap in [0.0, 50.0] {
            let l = layout(&f, "", 10.0, wrap);
            assert_eq!(l.line_count(), 0);
            assert_eq!(l.extent(), [0.0, 0.0]);
            assert!(l.quads([0.0, 0.0], 10.0, YAxis::Down).is_empty());
        }
    }

    #[test]
    fn an_unmapped_char_advances_by_the_notdef() {
        let f = face();
        let l = layout(&f, "~A", 10.0, 0.0);
        assert_eq!(l.lines()[0].glyphs[0].advance_units, 300);
        let q = l.quads([0.0, 0.0], 10.0, YAxis::Down);
        assert_eq!(q.len(), 1, "notdef has no image");
        assert_eq!(q[0].pos[0], 300.0 * 0.01 + 10.0 * 0.01); // after notdef, plus A's left bearing
    }

    #[test]
    fn an_alias_lays_out_as_its_target() {
        let f = face();
        let lower = layout(&f, "a", 10.0, 0.0).quads([1.0, 2.0], 10.0, YAxis::Down);
        let upper = layout(&f, "A", 10.0, 0.0).quads([1.0, 2.0], 10.0, YAxis::Down);
        assert_eq!(lower[0].pos, upper[0].pos);
        assert_eq!(lower[0].uv, upper[0].uv);
    }

    #[test]
    fn up_mirrors_down_vertically() {
        let f = face();
        let l = layout(&f, "A\nB", 10.0, 0.0);
        let down = l.quads([3.0, 20.0], 10.0, YAxis::Down);
        let up = l.quads([3.0, -20.0], 10.0, YAxis::Up);
        for (d, u) in down.iter().zip(&up) {
            assert_eq!(d.pos[0], u.pos[0]);
            assert_eq!(d.pos[2], u.pos[2]);
            assert_eq!(d.pos[1], -u.pos[1]);
            assert_eq!(d.pos[3], -u.pos[3]);
            assert_eq!(d.uv, u.uv);
        }
    }
}
