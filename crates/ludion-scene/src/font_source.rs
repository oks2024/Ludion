//! Font source documents: `*.font-source.yaml` — the authored input of a font bake
//! (`Plan/font-assets.md`, decisions 1–4).
//!
//! A `kind: font-source` document names a `.ttf`/`.otf` face and says how to bake it: SDF or
//! bitmap, at what pixel size, over which characters. It reads no file; the bake in `ludion-font`
//! and the `ludion generate-atlas` verb resolve `file` and produce a `kind: font` document plus its
//! PNG. An optional `line_height` (font units) overrides the face's line height by moving the
//! ascender. The crate's usual split applies: [`parse`] checks structure/shape, [`validate`] checks
//! semantics, [`emit`] writes canonical text, all under `L1251`.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use serde_json::Value;

use crate::envelope;
use crate::font::FontMode;
use crate::scalar::scalar_string;
use crate::yaml::{self, MapEntry, Node, NodeValue, pointer_token};

/// The font source document format version this crate reads and writes.
pub const FONT_SOURCE_FORMAT_VERSION: u64 = 1;

const KEYS: [&str; 9] = [
    "format",
    "kind",
    "name",
    "file",
    "mode",
    "em_px",
    "spread_px",
    "line_height",
    "charset",
];
const CHARSET_KEYS: [&str; 2] = ["ranges", "chars"];

/// The largest `em_px` a bake accepts.
const MAX_EM_PX: u32 = 256;
/// The largest `line_height` a source accepts, in font units.
const MAX_LINE_HEIGHT: u32 = 65_535;

/// A named block of characters a charset can pull in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CharsetRange {
    /// `ascii`: U+0020–U+007E.
    Ascii,
    /// `latin-1`: U+00A0–U+00FF.
    Latin1,
    /// `general-punctuation`: U+2010–U+2027 and U+2030–U+205E.
    GeneralPunctuation,
}

impl CharsetRange {
    /// Every range, in keyword order.
    pub const ALL: [CharsetRange; 3] = [
        CharsetRange::Ascii,
        CharsetRange::GeneralPunctuation,
        CharsetRange::Latin1,
    ];

    /// The range's document keyword.
    pub fn keyword(self) -> &'static str {
        match self {
            CharsetRange::Ascii => "ascii",
            CharsetRange::Latin1 => "latin-1",
            CharsetRange::GeneralPunctuation => "general-punctuation",
        }
    }

    fn from_keyword(keyword: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.keyword() == keyword)
    }

    /// The inclusive code-point intervals this range covers.
    pub fn intervals(self) -> &'static [(u32, u32)] {
        match self {
            CharsetRange::Ascii => &[(0x20, 0x7E)],
            CharsetRange::Latin1 => &[(0xA0, 0xFF)],
            CharsetRange::GeneralPunctuation => &[(0x2010, 0x2027), (0x2030, 0x205E)],
        }
    }
}

/// The characters a bake covers: named ranges plus loose characters.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FontCharset {
    /// Named ranges, in authored order (emit sorts them by keyword).
    pub ranges: Vec<CharsetRange>,
    /// Extra characters, as authored.
    pub chars: String,
}

/// A parsed `kind: font-source` document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FontSourceDocument {
    /// The output stem; absent means the source file's own stem.
    pub name: Option<String>,
    /// The face file, relative to this document.
    pub file: String,
    /// How the bake encodes coverage.
    pub mode: FontMode,
    /// The pixel size the glyphs are baked at.
    pub em_px: u32,
    /// The SDF falloff in texels; `Some` iff the mode is [`FontMode::Sdf`].
    pub spread_px: Option<u32>,
    /// The line height in font units; replaces the face's `ascender − descender + line_gap` by
    /// moving the ascender (decision 18).
    pub line_height: Option<u32>,
    /// The characters to bake.
    pub charset: FontCharset,
}

fn diag(pointer: &str, message: String, help: &str) -> Diagnostic {
    Diagnostic::new(code::FONT_SOURCE_DOCUMENT_STRUCTURE, message)
        .with_path(pointer)
        .with_help(help)
}

fn expect_u32(
    node: &Node,
    pointer: &str,
    what: &str,
    help: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<u32> {
    match &node.value {
        NodeValue::Scalar(Value::Number(n)) if n.as_u64().is_some() => {
            let v = n.as_u64().expect("checked by the guard");
            match u32::try_from(v) {
                Ok(v) => Some(v),
                Err(_) => {
                    diagnostics.push(diag(
                        pointer,
                        format!("{what} is {v}, outside the 32-bit range"),
                        "use a smaller value",
                    ));
                    None
                }
            }
        }
        other => {
            diagnostics.push(diag(
                pointer,
                format!(
                    "{what} must be a non-negative integer, found {} (line {})",
                    other.type_phrase(),
                    node.line
                ),
                help,
            ));
            None
        }
    }
}

fn string_of(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    envelope::expect_string(
        node,
        pointer,
        what,
        code::FONT_SOURCE_DOCUMENT_STRUCTURE,
        diagnostics,
    )
}

/// Parses strict-subset YAML into a [`FontSourceDocument`], checking structure/shape only (`L1251`).
pub(crate) fn parse(source: &str) -> Result<FontSourceDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![diag(
            "",
            "the font source document is empty; it needs at least `format: 1`, \
             `kind: font-source`, `file`, `mode`, `em_px` and `charset`"
                .to_string(),
            "see `ludion docs` for a minimal font source",
        )]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![diag(
            "",
            format!(
                "the font source root must be a mapping, found {} (line {})",
                root.value.type_phrase(),
                root.line
            ),
            "start the file with the top-level keys `format:` and `kind: font-source`",
        )]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = FontSourceDocument::default();
    let mut seen = [false; KEYS.len()];
    for entry in &entries {
        if let Some(i) = KEYS.iter().position(|k| *k == entry.key) {
            seen[i] = true;
        }
        match entry.key.as_str() {
            "format" => envelope::check_envelope(
                entry,
                code::FONT_SOURCE_DOCUMENT_STRUCTURE,
                code::FONT_SOURCE_DOCUMENT_STRUCTURE,
                FONT_SOURCE_FORMAT_VERSION,
                "the font source document",
                "font source",
                "font source",
                &mut diagnostics,
            ),
            "kind" => envelope::check_kind(
                entry,
                code::FONT_SOURCE_DOCUMENT_STRUCTURE,
                "font-source",
                "a font source document",
                &mut diagnostics,
            ),
            "name" => doc.name = string_of(&entry.node, "/name", "`name`", &mut diagnostics),
            "file" => {
                doc.file =
                    string_of(&entry.node, "/file", "`file`", &mut diagnostics).unwrap_or_default();
            }
            "mode" => parse_mode(entry, &mut doc, &mut diagnostics),
            "em_px" => {
                doc.em_px = expect_u32(
                    &entry.node,
                    "/em_px",
                    "`em_px`",
                    "write a whole number of pixels",
                    &mut diagnostics,
                )
                .unwrap_or_default();
            }
            "spread_px" => {
                doc.spread_px = expect_u32(
                    &entry.node,
                    "/spread_px",
                    "`spread_px`",
                    "write a whole number of pixels",
                    &mut diagnostics,
                );
            }
            "line_height" => {
                doc.line_height = expect_u32(
                    &entry.node,
                    "/line_height",
                    "`line_height`",
                    "write a whole number of font units",
                    &mut diagnostics,
                );
            }
            "charset" => parse_charset(entry, &mut doc, &mut diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a font source document",
                KEYS,
                code::FONT_SOURCE_DOCUMENT_STRUCTURE,
            )),
        }
    }

    const REQUIRED: [(&str, &str); 6] = [
        ("format", "add `format: 1` as the first line"),
        ("kind", "add `kind: font-source`"),
        ("file", "add `file:` naming the .ttf or .otf face"),
        ("mode", "add `mode: sdf` or `mode: bitmap`"),
        ("em_px", "add `em_px:`, the baked pixel size, e.g. `56`"),
        (
            "charset",
            "add a `charset` mapping with `ranges` and/or `chars`",
        ),
    ];
    for (key, help) in REQUIRED {
        let i = KEYS
            .iter()
            .position(|k| *k == key)
            .expect("a font source key");
        if !seen[i] {
            diagnostics.push(envelope::missing_key(
                key,
                help,
                code::FONT_SOURCE_DOCUMENT_STRUCTURE,
                "font source document",
            ));
        }
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

fn parse_mode(entry: &MapEntry, doc: &mut FontSourceDocument, diagnostics: &mut Vec<Diagnostic>) {
    let Some(text) = string_of(&entry.node, "/mode", "`mode`", diagnostics) else {
        return;
    };
    match text.as_str() {
        "sdf" => doc.mode = FontMode::Sdf,
        "bitmap" => doc.mode = FontMode::Bitmap,
        other => diagnostics.push(diag(
            "/mode",
            format!("unknown font mode `{other}` (line {})", entry.node.line),
            "`mode` is `sdf` (a vector face) or `bitmap` (a pixel face)",
        )),
    }
}

fn parse_charset(
    entry: &MapEntry,
    doc: &mut FontSourceDocument,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(diag(
            "/charset",
            format!(
                "`charset` must be a mapping, found {} (line {})",
                entry.node.value.type_phrase(),
                entry.node.line
            ),
            "write `ranges:` and/or `chars:` as indented keys",
        ));
        return;
    };
    if entries.is_empty() {
        diagnostics.push(diag(
            "/charset",
            format!("`charset` is empty (line {})", entry.node.line),
            "add `ranges:` (e.g. `- ascii`) and/or `chars:`",
        ));
    }
    for e in entries {
        let pointer = format!("/charset/{}", pointer_token(&e.key));
        match e.key.as_str() {
            "ranges" => parse_ranges(e, doc, diagnostics),
            "chars" => {
                doc.charset.chars =
                    string_of(&e.node, &pointer, "`chars`", diagnostics).unwrap_or_default();
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                e,
                "/charset",
                "the charset mapping",
                CHARSET_KEYS,
                code::FONT_SOURCE_DOCUMENT_STRUCTURE,
            )),
        }
    }
}

fn parse_ranges(entry: &MapEntry, doc: &mut FontSourceDocument, diagnostics: &mut Vec<Diagnostic>) {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(diag(
            "/charset/ranges",
            format!(
                "`ranges` must be a sequence, found {} (line {})",
                entry.node.value.type_phrase(),
                entry.node.line
            ),
            "write one `- <range>` line per range",
        ));
        return;
    };
    let keywords = CharsetRange::ALL.map(CharsetRange::keyword);
    for (i, item) in items.iter().enumerate() {
        let pointer = format!("/charset/ranges/{i}");
        let Some(text) = string_of(item, &pointer, "a range", diagnostics) else {
            continue;
        };
        match CharsetRange::from_keyword(&text) {
            Some(range) => doc.charset.ranges.push(range),
            None => {
                let help = match ludion_core::suggest::nearest(&text, keywords) {
                    Some(candidate) => format!("nearest match is `{candidate}`"),
                    None => format!("valid ranges are: {}", keywords.join(", ")),
                };
                diagnostics.push(diag(
                    &pointer,
                    format!("unknown charset range `{text}` (line {})", item.line),
                    &help,
                ));
            }
        }
    }
}

/// Checks the semantic invariants of a parsed document (`L1251`): a `name` that is a plain file
/// stem, a non-empty `file`, `em_px` in
/// `1..=256`, `spread_px` present and within `1..=em_px` exactly for an SDF face, `line_height`
/// (when set) in `1..=65535`, and a charset that resolves to at least one character.
pub(crate) fn validate(doc: &FontSourceDocument) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if let Some(name) = &doc.name
        && let Some(refusal) = ludion_core::paths::stem_refusal(name)
    {
        out.push(diag(
            "/name",
            format!("`name` {}", refusal.describe()),
            "write a plain file stem such as `inter`; it names the baked `<name>.font.yaml` and `<name>.png`",
        ));
    }
    if doc.file.is_empty() {
        out.push(diag(
            "/file",
            "`file` is empty".to_string(),
            "name the .ttf or .otf face, relative to this document",
        ));
    }
    if doc.em_px == 0 || doc.em_px > MAX_EM_PX {
        out.push(diag(
            "/em_px",
            format!("`em_px` is {}; it must be in 1..={MAX_EM_PX}", doc.em_px),
            "pick the pixel size the face is baked at, e.g. 56 for an SDF or 8 for a pixel face",
        ));
    }
    match (doc.mode, doc.spread_px) {
        (FontMode::Sdf, None) => out.push(diag(
            "/spread_px",
            "an `sdf` face needs `spread_px`".to_string(),
            "add `spread_px:`, the distance falloff in texels, e.g. `6`",
        )),
        (FontMode::Sdf, Some(spread)) if spread == 0 || spread > doc.em_px => out.push(diag(
            "/spread_px",
            format!(
                "`spread_px` is {spread}; it must be in 1..=`em_px` ({})",
                doc.em_px
            ),
            "use a smaller falloff",
        )),
        (FontMode::Bitmap, Some(_)) => out.push(diag(
            "/spread_px",
            "a `bitmap` face has no `spread_px`".to_string(),
            "remove `spread_px`, or switch to `mode: sdf`",
        )),
        _ => {}
    }
    if let Some(n) = doc.line_height
        && (n == 0 || n > MAX_LINE_HEIGHT)
    {
        out.push(diag(
            "/line_height",
            format!("`line_height` is {n}; it must be in 1..={MAX_LINE_HEIGHT} font units"),
            "set the line height in the face's font units, e.g. 1250 for a 1000-unit em with 10-pixel lines",
        ));
    }
    if doc.charset.ranges.is_empty() && doc.charset.chars.is_empty() {
        out.push(diag(
            "/charset",
            "the charset resolves to no characters".to_string(),
            "add `ranges:` (e.g. `- ascii`) and/or a non-empty `chars:`",
        ));
    }
    out
}

/// Writes `doc` in canonical form: the keys in the order of [`KEYS`], ranges sorted by keyword and
/// deduplicated, `chars` as authored. Byte-idempotent, and a fixed point on canonical input.
pub(crate) fn emit(doc: &FontSourceDocument) -> String {
    let mut out = String::from("format: 1\nkind: font-source\n");
    if let Some(name) = &doc.name {
        out.push_str(&format!("name: {}\n", scalar_string(name)));
    }
    out.push_str(&format!("file: {}\n", scalar_string(&doc.file)));
    out.push_str(&format!("mode: {}\n", doc.mode.keyword()));
    out.push_str(&format!("em_px: {}\n", doc.em_px));
    if let Some(spread) = doc.spread_px {
        out.push_str(&format!("spread_px: {spread}\n"));
    }
    if let Some(line_height) = doc.line_height {
        out.push_str(&format!("line_height: {line_height}\n"));
    }
    out.push_str("charset:\n");
    let mut keywords: Vec<&str> = doc.charset.ranges.iter().map(|r| r.keyword()).collect();
    keywords.sort_unstable();
    keywords.dedup();
    if !keywords.is_empty() {
        out.push_str("  ranges:\n");
        for keyword in keywords {
            out.push_str(&format!("    - {keyword}\n"));
        }
    }
    if !doc.charset.chars.is_empty() {
        out.push_str(&format!("  chars: {}\n", scalar_string(&doc.charset.chars)));
    }
    out
}

#[cfg(test)]
#[path = "font_source_tests.rs"]
mod tests;
