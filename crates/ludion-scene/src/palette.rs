//! Palette documents: `*.palette.yaml` — a standalone glyph palette a `kind: sprite` document can
//! reference instead of inlining (`Plan/sprite-engine.md`, "Data and bake").
//!
//! A palette document is `format: 1`, `kind: palette`, an optional `name`, and a `palette:` map of
//! single-glyph keys to `{ color: [r, g, b, a], emissive? }` — exactly the shape a sprite document's
//! own inline `palette:` uses ([`crate::SpritePaletteEntry`]), so one shared palette can back many
//! sprites without copying sixteen colours into each. A sprite references it with
//! `palette: { from: <path>, overrides? }`, and [`resolve`] merges the two into the concrete inline
//! palette the bake consumes — so a sprite authored inline and the same sprite authored via an include
//! with equal colours bake to byte-identical images.
//!
//! The split is the crate's usual one: [`parse`] checks structure/shape, [`validate`] checks
//! semantics, and [`emit`] writes canonical text — all under `L1147`, the code that owns the shared
//! palette both as a document and as a sprite's reference to one. The palette-entry parse and
//! validation themselves are [`crate::sprite`]'s, shared verbatim so the two palette families cannot
//! disagree about what a glyph or a colour is.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::envelope;
use crate::scalar::scalar_string;
use crate::sprite::{
    SpriteDocument, SpritePaletteEntry, emit_palette, parse_palette_entries_map,
    validate_palette_entries,
};
use crate::yaml::{self, MapEntry, NodeValue};

/// The palette document format version this crate reads and writes.
pub const PALETTE_FORMAT_VERSION: u64 = 1;

const PALETTE_KEYS: [&str; 4] = ["format", "kind", "name", "palette"];

/// A parsed `kind: palette` document: a name and the palette entries, sorted by glyph.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PaletteDocument {
    /// Optional human-readable palette name.
    pub name: Option<String>,
    /// The palette, sorted by glyph — the same entry shape a sprite document's inline palette uses.
    pub palette: Vec<SpritePaletteEntry>,
}

// --- Parse --------------------------------------------------------------------------------------

/// Parses strict-subset YAML into a [`PaletteDocument`], checking structure/shape only (`L1147`).
pub(crate) fn parse(source: &str) -> Result<PaletteDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::SPRITE_PALETTE_STRUCTURE,
                "the palette document is empty; a palette needs at least `format: 1`, \
                 `kind: palette`, and `palette`",
            )
            .with_path("")
            .with_help(
                "a minimal palette is:\n  format: 1\n  kind: palette\n  palette:\n    A:\n      \
                 color:\n        - 1.0\n        - 1.0\n        - 1.0\n        - 1.0",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::SPRITE_PALETTE_STRUCTURE,
                format!(
                    "the palette document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: palette`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = PaletteDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut palette_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::SPRITE_PALETTE_STRUCTURE,
                    code::SPRITE_PALETTE_STRUCTURE,
                    PALETTE_FORMAT_VERSION,
                    "the palette document",
                    "palette",
                    "palette",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::SPRITE_PALETTE_STRUCTURE,
                    "palette",
                    "a palette document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the palette name",
                    code::SPRITE_PALETTE_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "palette" => {
                palette_seen = true;
                doc.palette = parse_palette_map(entry, &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a palette document",
                PALETTE_KEYS,
                code::SPRITE_PALETTE_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key("format", "add `format: 1` as the first line"));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: palette`"));
    }
    if !palette_seen {
        diagnostics.push(missing_key(
            "palette",
            "add a `palette` mapping each glyph to a `color`",
        ));
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

fn missing_key(key: &str, help: &str) -> Diagnostic {
    envelope::missing_key(
        key,
        help,
        code::SPRITE_PALETTE_STRUCTURE,
        "palette document",
    )
}

/// Parses the `palette:` mapping into entries, reusing the sprite crate's palette-entry parser under
/// this family's own `L1147`.
fn parse_palette_map(
    entry: &MapEntry,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<SpritePaletteEntry> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_PALETTE_STRUCTURE,
                format!(
                    "`palette` must be a mapping of glyphs to colors, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/palette")
            .with_help(
                "write `palette` as glyph keys, each mapping to a `color`:\n  palette:\n    \"#\":\n\
                 \x20     color:\n        - 1.0\n        - 0.0\n        - 0.0\n        - 1.0",
            ),
        );
        return Vec::new();
    };
    parse_palette_entries_map(
        entries,
        "/palette",
        code::SPRITE_PALETTE_STRUCTURE,
        diagnostics,
    )
}

// --- Validate -----------------------------------------------------------------------------------

/// Validates semantic invariants of a [`PaletteDocument`] (`L1147`): a non-empty palette whose keys
/// are single printable-ASCII glyphs with colors in `0..=1` and finite non-negative emissives — the
/// sprite crate's own palette rules, so an inline palette and a shared one are held to one standard.
pub(crate) fn validate(doc: &PaletteDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if doc.palette.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::SPRITE_PALETTE_STRUCTURE,
                "the palette is empty; a palette document must name at least one color",
            )
            .with_path("/palette")
            .with_help(
                "add one glyph with a `color`; `.` is always transparent and never a palette entry",
            ),
        );
    }
    validate_palette_entries(
        &doc.palette,
        "/palette",
        code::SPRITE_PALETTE_STRUCTURE,
        &mut diagnostics,
    );
    diagnostics
}

// --- Resolve ------------------------------------------------------------------------------------

/// Merges `overrides` into `palette`: a glyph the palette defines is replaced, one it does not is
/// added, and the result is sorted by glyph. The one merge behind a sprite's `palette: { from,
/// overrides }` include and an atlas source's per-input `palette`.
pub(crate) fn apply_overrides(
    palette: &mut Vec<SpritePaletteEntry>,
    overrides: &[SpritePaletteEntry],
) {
    for over in overrides {
        match palette.iter_mut().find(|entry| entry.glyph == over.glyph) {
            Some(entry) => *entry = over.clone(),
            None => palette.push(over.clone()),
        }
    }
    palette.sort_by(|a, b| a.glyph.cmp(&b.glyph));
}

/// Resolves a sprite document's shared-palette include against the palette document `palette_source`:
/// the referenced palette with the sprite's local `overrides` applied on top (a glyph the referenced
/// palette defines is replaced, one it does not is added), producing an ordinary inline sprite
/// document the bake consumes unchanged.
///
/// A sprite with no include is returned as-is — callers may call this unconditionally. The result is
/// **not** re-validated here: the caller runs [`crate::validate_sprite`] (or the bake) on it, which is
/// where a glyph a frame uses but the merged palette still lacks surfaces as the ordinary `L1153`.
///
/// # Errors
/// `L1147` if `palette_source` is not a well-formed `kind: palette` document.
pub(crate) fn resolve(
    doc: &SpriteDocument,
    palette_source: &str,
) -> Result<SpriteDocument, Vec<Diagnostic>> {
    let Some(include) = &doc.palette_include else {
        return Ok(doc.clone());
    };
    let palette_doc = parse(palette_source)?;
    let diagnostics = validate(&palette_doc);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let mut merged = palette_doc.palette;
    apply_overrides(&mut merged, &include.overrides);

    let mut resolved = doc.clone();
    resolved.palette = merged;
    resolved.palette_include = None;
    Ok(resolved)
}

// --- Emit ---------------------------------------------------------------------------------------

/// Writes `doc` in canonical form: `format`, `kind`, `name` (optional), `palette` (sorted by glyph),
/// ryu shortest-round-trip floats, LF endings. Byte-idempotent on any document and a fixed point on
/// canonical input.
pub(crate) fn emit(doc: &PaletteDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\nkind: palette\n");
    if let Some(name) = &doc.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }
    emit_palette(&mut out, &doc.palette);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "\
format: 1
kind: palette
name: world
palette:
  \"#\":
    color:
      - 0.35
      - 0.4
      - 0.5
      - 1.0
  o:
    color:
      - 0.3
      - 0.85
      - 1.0
      - 1.0
    emissive: 2.5
";

    fn check(source: &str) -> Result<PaletteDocument, Vec<Diagnostic>> {
        let doc = parse(source)?;
        let diagnostics = validate(&doc);
        if diagnostics.is_empty() {
            Ok(doc)
        } else {
            Err(diagnostics)
        }
    }

    fn errors(source: &str) -> Vec<Diagnostic> {
        match check(source) {
            Ok(_) => panic!("expected the palette to be rejected"),
            Err(diagnostics) => diagnostics,
        }
    }

    #[test]
    fn the_good_palette_round_trips_byte_identically() {
        let doc = check(GOOD).expect("the palette is valid");
        assert_eq!(doc.name.as_deref(), Some("world"));
        assert_eq!(doc.palette.len(), 2);
        assert_eq!(emit(&doc), GOOD);
    }

    #[test]
    fn a_missing_palette_is_reported() {
        let diagnostics = errors("format: 1\nkind: palette\n");
        assert!(diagnostics.iter().any(|d| d.code.as_str() == "L1147"));
        assert!(diagnostics.iter().any(|d| d.message.contains("palette")));
    }

    #[test]
    fn the_wrong_kind_is_reported() {
        let diagnostics = errors(&GOOD.replace("kind: palette", "kind: sprite"));
        let diagnostic = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/kind"))
            .expect("a /kind diagnostic");
        assert_eq!(diagnostic.code.as_str(), "L1147");
    }

    #[test]
    fn an_out_of_range_color_is_reported() {
        let diagnostics = errors(&GOOD.replace("      - 0.35", "      - 1.5"));
        assert_eq!(
            diagnostics
                .iter()
                .find(|d| d.path.as_deref() == Some("/palette/#/color/0"))
                .expect("a colour diagnostic")
                .code
                .as_str(),
            "L1147"
        );
    }

    #[test]
    fn an_unknown_top_level_key_suggests_the_nearest() {
        let diagnostics = errors(&GOOD.replace("palette:", "palettes:\n  A: {}\npalette:"));
        let diagnostic = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/palettes"))
            .expect("a /palettes diagnostic");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("nearest match is `palette`")
        );
    }
}
