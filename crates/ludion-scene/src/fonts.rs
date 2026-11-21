//! Font tables: `fonts.yaml`, a `kind: fonts` document — the content root's map from logical face
//! names to `*.font.yaml` documents (`Plan/font-assets.md`, decisions 5 and 10).
//!
//! ```yaml
//! format: 1
//! kind: fonts
//! fonts:
//!   body: fonts/inter.font.yaml
//!   default: fonts/inter.font.yaml
//! ```
//!
//! `default` is the one reserved name: the face for text that names none. The document reads no
//! file; the engine's `FontLibrary` resolves the paths. [`parse`] checks structure/shape,
//! [`validate`] checks each name and path, [`emit`] writes canonical text (names sorted), all under
//! `L1253`.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::envelope;
use crate::scalar::scalar_string;
use crate::yaml::{self, NodeValue, pointer_token};

/// The font table format version this crate reads and writes.
pub const FONTS_FORMAT_VERSION: u64 = 1;

const KEYS: [&str; 4] = ["format", "kind", "fonts", "locales"];

/// A parsed `kind: fonts` document.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FontsDocument {
    /// `(name, path)` pairs, sorted by name; paths are relative to the content root.
    pub fonts: Vec<(String, String)>,
    /// Per-locale overrides: `(locale tag, [(name, path)])`, tags and names sorted.
    pub locales: Vec<(String, Vec<(String, String)>)>,
}

fn diag(pointer: &str, message: String, help: &str) -> Diagnostic {
    Diagnostic::new(code::FONTS_TABLE_STRUCTURE, message)
        .with_path(pointer)
        .with_help(help)
}

/// Parses strict-subset YAML into a [`FontsDocument`], checking structure/shape only (`L1253`).
pub(crate) fn parse(source: &str) -> Result<FontsDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![diag(
            "",
            "the font table is empty; it needs at least `format: 1`, `kind: fonts` and `fonts`"
                .to_string(),
            "a minimal table is:\n  format: 1\n  kind: fonts\n  fonts:\n    body: fonts/inter.font.yaml",
        )]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![diag(
            "",
            format!(
                "the font table root must be a mapping, found {} (line {})",
                root.value.type_phrase(),
                root.line
            ),
            "start the file with the top-level keys `format:` and `kind: fonts`",
        )]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = FontsDocument::default();
    let mut seen = [false; KEYS.len()];
    for entry in &entries {
        if let Some(i) = KEYS.iter().position(|k| *k == entry.key) {
            seen[i] = true;
        }
        match entry.key.as_str() {
            "format" => envelope::check_envelope(
                entry,
                code::FONTS_TABLE_STRUCTURE,
                code::FONTS_TABLE_STRUCTURE,
                FONTS_FORMAT_VERSION,
                "the font table",
                "font table",
                "font table",
                &mut diagnostics,
            ),
            "kind" => envelope::check_kind(
                entry,
                code::FONTS_TABLE_STRUCTURE,
                "fonts",
                "a font table",
                &mut diagnostics,
            ),
            "fonts" => match &entry.node.value {
                NodeValue::Mapping(faces) => {
                    for face in faces {
                        let pointer = format!("/fonts/{}", pointer_token(&face.key));
                        if let Some(path) = envelope::expect_string(
                            &face.node,
                            &pointer,
                            "a face path",
                            code::FONTS_TABLE_STRUCTURE,
                            &mut diagnostics,
                        ) {
                            doc.fonts.push((face.key.clone(), path));
                        }
                    }
                }
                other => diagnostics.push(diag(
                    "/fonts",
                    format!(
                        "`fonts` must be a mapping, found {} (line {})",
                        other.type_phrase(),
                        entry.node.line
                    ),
                    "write one `name: path/to/face.font.yaml` per indented line, or `fonts: {}`",
                )),
            },
            "locales" => match &entry.node.value {
                NodeValue::Mapping(tags) => {
                    for tag in tags {
                        let tag_pointer = format!("/locales/{}", pointer_token(&tag.key));
                        let NodeValue::Mapping(faces) = &tag.node.value else {
                            diagnostics.push(diag(
                                &tag_pointer,
                                format!(
                                    "the overrides of locale `{}` must be a mapping, found {} (line {})",
                                    tag.key,
                                    tag.node.value.type_phrase(),
                                    tag.node.line
                                ),
                                "write one `name: path/to/face.font.yaml` per indented line",
                            ));
                            continue;
                        };
                        let mut pairs = Vec::new();
                        for face in faces {
                            let pointer = format!("{tag_pointer}/{}", pointer_token(&face.key));
                            if let Some(path) = envelope::expect_string(
                                &face.node,
                                &pointer,
                                "a face path",
                                code::FONTS_TABLE_STRUCTURE,
                                &mut diagnostics,
                            ) {
                                pairs.push((face.key.clone(), path));
                            }
                        }
                        pairs.sort();
                        doc.locales.push((tag.key.clone(), pairs));
                    }
                }
                other => diagnostics.push(diag(
                    "/locales",
                    format!(
                        "`locales` must be a mapping, found {} (line {})",
                        other.type_phrase(),
                        entry.node.line
                    ),
                    "write `locales:` then one `tag:` per indented line, each holding `name: path` entries",
                )),
            },
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a font table",
                KEYS,
                code::FONTS_TABLE_STRUCTURE,
            )),
        }
    }

    const REQUIRED: [(&str, &str); 3] = [
        ("format", "add `format: 1` as the first line"),
        ("kind", "add `kind: fonts`"),
        (
            "fonts",
            "add `fonts:` mapping face names to `*.font.yaml` paths",
        ),
    ];
    for (key, help) in REQUIRED {
        let i = KEYS.iter().position(|k| *k == key).expect("a fonts key");
        if !seen[i] {
            diagnostics.push(envelope::missing_key(
                key,
                help,
                code::FONTS_TABLE_STRUCTURE,
                "font table",
            ));
        }
    }

    if diagnostics.is_empty() {
        doc.fonts.sort();
        doc.locales.sort();
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Checks every name and path of a parsed [`FontsDocument`] (`L1253`).
pub(crate) fn validate(doc: &FontsDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (name, path) in &doc.fonts {
        let pointer = format!("/fonts/{}", pointer_token(name));
        if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c == '/') {
            diagnostics.push(diag(
                &pointer,
                format!("the face name `{name}` must be non-empty and hold no whitespace or `/`"),
                "use a short logical name such as `body` or `title`",
            ));
        }
        check_path(&pointer, name, path, &mut diagnostics);
    }
    let defined: Vec<&str> = doc.fonts.iter().map(|(n, _)| n.as_str()).collect();
    for (tag, overrides) in &doc.locales {
        let tag_pointer = format!("/locales/{}", pointer_token(tag));
        if tag.is_empty() || tag.chars().any(|c| c.is_whitespace() || c == '/') {
            diagnostics.push(diag(
                &tag_pointer,
                format!("the locale tag `{tag}` must be non-empty and hold no whitespace or `/`"),
                "use a locale tag such as `fr` or `pt-BR`",
            ));
        }
        for (name, path) in overrides {
            let pointer = format!("{tag_pointer}/{}", pointer_token(name));
            if !defined.contains(&name.as_str()) {
                let help = match ludion_core::suggest::nearest(name, defined.iter().copied()) {
                    Some(near) => format!("did you mean `{near}`?"),
                    None => format!(
                        "an override replaces a face `fonts:` defines; defined: {}",
                        defined.join(", ")
                    ),
                };
                diagnostics.push(diag(
                    &pointer,
                    format!(
                        "locale `{tag}` overrides the face `{name}`, which `fonts:` does not define"
                    ),
                    &help,
                ));
            }
            check_path(&pointer, name, path, &mut diagnostics);
        }
    }
    diagnostics
}

fn check_path(pointer: &str, name: &str, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let canonical = !path.contains('\\')
        && path.split('/').all(|s| !s.is_empty() && s != ".")
        && path.ends_with(".font.yaml");
    if let Some(refusal) = ludion_core::paths::relative_refusal(path) {
        diagnostics.push(diag(
            pointer,
            format!("the path `{path}` of face `{name}` {}", refusal.describe()),
            "point at a baked face below the content root, e.g. `fonts/inter.font.yaml`",
        ));
    } else if !canonical {
        diagnostics.push(diag(
            pointer,
            format!(
                "the path `{path}` of face `{name}` is not in canonical form: it must use `/` \
                 only, hold no empty or `.` segment, and end in `.font.yaml`"
            ),
            "write it as `fonts/x.font.yaml`",
        ));
    }
}

/// Writes `doc` in canonical form: names sorted. Byte-idempotent.
pub(crate) fn emit(doc: &FontsDocument) -> String {
    let mut out = String::from("format: 1\nkind: fonts\n");
    if doc.fonts.is_empty() {
        out.push_str("fonts: {}\n");
    } else {
        out.push_str("fonts:\n");
        let mut faces: Vec<&(String, String)> = doc.fonts.iter().collect();
        faces.sort();
        for (name, path) in faces {
            out.push_str(&format!(
                "  {}: {}\n",
                scalar_string(name),
                scalar_string(path)
            ));
        }
    }
    let mut locales: Vec<&(String, Vec<(String, String)>)> = doc.locales.iter().collect();
    locales.sort();
    if !locales.is_empty() {
        out.push_str("locales:\n");
        for (tag, overrides) in locales {
            if overrides.is_empty() {
                out.push_str(&format!("  {}: {{}}\n", scalar_string(tag)));
                continue;
            }
            out.push_str(&format!("  {}:\n", scalar_string(tag)));
            let mut pairs: Vec<&(String, String)> = overrides.iter().collect();
            pairs.sort();
            for (name, path) in pairs {
                out.push_str(&format!(
                    "    {}: {}\n",
                    scalar_string(name),
                    scalar_string(path)
                ));
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "fonts_tests.rs"]
mod tests;
