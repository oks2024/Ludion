//! Font faces for Ludion: the glyph, advance and kerning model over a `kind: font` document.
//!
//! This crate is **pure**: it reads no file, touches no GPU and knows no ECS. The `kind: font`
//! text format lives in [`ludion_scene`]; [`FontFace`] wraps a validated document with the lookups
//! text layout needs, and ships the embedded 8×8 face ([`FontFace::embedded`], [`EMBEDDED_PNG`]).
//! The atlas PNG a document names is resolved and decoded by the host — the bytes here are shipped,
//! not decoded.
//!
//! # Determinism
//!
//! Measurement feeds sim-visible text, so every function is a pure function of its arguments:
//! integer font units, sorted `Vec`s with binary search in place of hash maps, no wall clock and no
//! floating-point transcendental.
//!
//! # Example
//!
//! ```
//! use ludion_font::{FontFace, YAxis, layout};
//!
//! let face = FontFace::embedded();
//! assert_eq!(face.advance('A'), 8);
//! assert_eq!(face.glyph('a').map(|g| g.ch), Some('A')); // lowercase aliases to uppercase
//! assert!(face.glyph('é').is_none()); // unmapped: drawn as notdef
//!
//! // Shape "HI\nYO" at size 16 (no wrapping): 2 glyphs wide, 2 lines tall.
//! let text = layout(face, "HI\nYO", 16.0, 0.0);
//! assert_eq!(text.extent(), [32.0, 32.0]);
//! let quads = text.quads([10.0, 20.0], 16.0, YAxis::Down);
//! assert_eq!(quads.len(), 4);
//! assert_eq!(quads[1].pos, [26.0, 20.0, 42.0, 36.0]); // "I": left, top, right, bottom
//! ```

#[cfg(feature = "bake")]
pub mod bake;
mod layout;
mod stamp;

use std::sync::OnceLock;

use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{FontDocument, FontGlyph};

pub use layout::{GlyphQuad, LaidGlyph, LayoutLine, TextLayout, YAxis, fold_line_breaks, layout};
pub use stamp::{LABEL_CELL, LABEL_PITCH, stamp_label};

/// The embedded 8×8 face's document text.
const EMBEDDED_DOC: &str = include_str!("../assets/default.font.yaml");

/// The embedded 8×8 face's atlas: RGBA8 PNG, white with alpha coverage.
pub const EMBEDDED_PNG: &[u8] = include_bytes!("../assets/default.png");

/// A validated font document with the lookups layout needs.
#[derive(Clone, Debug)]
pub struct FontFace {
    doc: FontDocument,
}

impl FontFace {
    /// Wraps `doc` after running [`ludion_scene::validate_font`].
    ///
    /// # Errors
    /// The document's `L1250` diagnostics.
    pub fn new(doc: FontDocument) -> Result<FontFace, Vec<Diagnostic>> {
        let diagnostics = ludion_scene::validate_font(&doc);
        if diagnostics.is_empty() {
            Ok(FontFace { doc })
        } else {
            Err(diagnostics)
        }
    }

    /// The embedded 8×8 face: a bitmap face with `A–Z`, `0–9` and punctuation, lowercase aliased to
    /// uppercase, unknown chars drawn blank.
    pub fn embedded() -> &'static FontFace {
        static FACE: OnceLock<FontFace> = OnceLock::new();
        FACE.get_or_init(|| {
            let doc = ludion_scene::check_font(EMBEDDED_DOC)
                .expect("assets/default.font.yaml in ludion-font is a valid kind: font document");
            FontFace::new(doc).expect("assets/default.font.yaml in ludion-font is a valid face")
        })
    }

    /// The underlying document.
    pub fn document(&self) -> &FontDocument {
        &self.doc
    }

    /// The line height in font units: `ascender − descender + line_gap`.
    pub fn line_units(&self) -> i32 {
        self.doc.ascender - self.doc.descender + self.doc.line_gap
    }

    /// The glyph for `ch`, resolving aliases; `None` means the face has none (draw notdef).
    pub fn glyph(&self, ch: char) -> Option<&FontGlyph> {
        let ch = match self.doc.aliases.binary_search_by(|(from, _)| from.cmp(&ch)) {
            Ok(i) => self.doc.aliases[i].1,
            Err(_) => ch,
        };
        self.doc
            .glyphs
            .binary_search_by(|g| g.ch.cmp(&ch))
            .ok()
            .map(|i| &self.doc.glyphs[i])
    }

    /// The advance of `ch` in font units; the notdef's when the face has no glyph for it.
    pub fn advance(&self, ch: char) -> i32 {
        self.glyph(ch)
            .map_or(self.doc.notdef.advance, |glyph| glyph.advance)
    }

    /// The kerning adjustment between `left` and `right` in font units, 0 when the pair has none.
    /// Looked up on the chars as given (before alias resolution).
    pub fn kerning(&self, left: char, right: char) -> i32 {
        self.doc
            .kerning
            .binary_search_by(|(l, r, _)| (*l, *r).cmp(&(left, right)))
            .map_or(0, |i| self.doc.kerning[i].2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &str = "\
format: 1
kind: font
image: small.png
mode: bitmap
units_per_em: 8
ascender: 6
descender: -2
line_gap: 1
atlas:
  width: 16
  height: 8
  em_px: 8
notdef:
  advance: 5
glyphs:
  - char: A
    advance: 7
    bounds:
      - 0
      - 0
      - 7
      - 8
    rect:
      - 0
      - 0
      - 8
      - 8
  - char: V
    advance: 6
aliases:
  a: A
kerning:
  A:
    V: -2
";

    fn small() -> FontFace {
        FontFace::new(ludion_scene::parse_font(SMALL).expect("parses")).expect("valid")
    }

    #[test]
    fn line_units_adds_gap_to_ascent_and_descent() {
        assert_eq!(small().line_units(), 9);
    }

    #[test]
    fn an_alias_resolves_to_its_target_glyph() {
        let face = small();
        assert_eq!(face.glyph('a').map(|g| g.ch), Some('A'));
        assert_eq!(face.advance('a'), 7);
    }

    #[test]
    fn an_unmapped_char_takes_the_notdef_advance() {
        let face = small();
        assert!(face.glyph('~').is_none());
        assert_eq!(face.advance('~'), 5);
    }

    #[test]
    fn kerning_hits_and_misses() {
        let face = small();
        assert_eq!(face.kerning('A', 'V'), -2);
        assert_eq!(face.kerning('V', 'A'), 0);
    }

    #[test]
    fn an_invalid_document_is_refused() {
        let mut doc = ludion_scene::parse_font(SMALL).expect("parses");
        doc.units_per_em = 0;
        assert!(FontFace::new(doc).is_err());
    }
}
