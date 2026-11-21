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
    /// `(name, [fallback name, ..])` for entries written `{ path, fallback }`, sorted by name; the
    /// order within a list is kept.
    pub fallbacks: Vec<(String, Vec<String>)>,
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
                        if let NodeValue::Mapping(fields) = &face.node.value {
                            if let Some(path) = parse_face_mapping(
                                face,
                                fields,
                                &pointer,
                                &mut doc,
                                &mut diagnostics,
                            ) {
                                doc.fonts.push((face.key.clone(), path));
                            }
                        } else if let Some(path) = envelope::expect_string(
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
        doc.fallbacks.sort_by(|a, b| a.0.cmp(&b.0));
        doc.locales.sort();
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Reads the `{ path, fallback }` form of a `fonts:` entry; records the chain in `doc.fallbacks`.
fn parse_face_mapping(
    face: &yaml::MapEntry,
    fields: &[yaml::MapEntry],
    pointer: &str,
    doc: &mut FontsDocument,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    let mut path = None;
    let mut missing_path = true;
    for field in fields {
        let field_pointer = format!("{pointer}/{}", pointer_token(&field.key));
        match field.key.as_str() {
            "path" => {
                missing_path = false;
                path = envelope::expect_string(
                    &field.node,
                    &field_pointer,
                    "a face path",
                    code::FONTS_TABLE_STRUCTURE,
                    diagnostics,
                );
            }
            "fallback" => {
                let NodeValue::Sequence(items) = &field.node.value else {
                    diagnostics.push(diag(
                        &field_pointer,
                        format!(
                            "`fallback` of face `{}` must be a sequence of face names, found {} (line {})",
                            face.key,
                            field.node.value.type_phrase(),
                            field.node.line
                        ),
                        "write `fallback:` then one `- name` per indented line",
                    ));
                    continue;
                };
                let mut names = Vec::new();
                for (i, item) in items.iter().enumerate() {
                    if let Some(name) = envelope::expect_string(
                        item,
                        &format!("{field_pointer}/{i}"),
                        "a face name",
                        code::FONTS_TABLE_STRUCTURE,
                        diagnostics,
                    ) {
                        names.push(name);
                    }
                }
                doc.fallbacks.push((face.key.clone(), names));
            }
            other => diagnostics.push(diag(
                &field_pointer,
                format!(
                    "unknown key `{other}` in the entry of face `{}` (line {})",
                    face.key, field.key_line
                ),
                "an entry holds `path:` and optionally `fallback:` with one `- name` per line",
            )),
        }
    }
    if missing_path {
        diagnostics.push(diag(
            pointer,
            format!(
                "the entry of face `{}` has no `path` (line {})",
                face.key, face.node.line
            ),
            "add `path: fonts/x.font.yaml`, or write the entry as a plain path",
        ));
    }
    path
}

/// Checks `fallback` lists: defined names, no self reference, no cycle, a non-empty list, a flattened
/// chain of at most [`MAX_CHAIN`] faces.
fn validate_fallbacks(doc: &FontsDocument, diagnostics: &mut Vec<Diagnostic>) {
    let defined: Vec<&str> = doc.fonts.iter().map(|(n, _)| n.as_str()).collect();
    for (name, list) in &doc.fallbacks {
        let pointer = format!("/fonts/{}/fallback", pointer_token(name));
        if list.is_empty() {
            diagnostics.push(diag(
                &pointer,
                format!("the `fallback` list of face `{name}` is empty"),
                "name at least one face, or drop the key",
            ));
            continue;
        }
        let mut broken = false;
        for (i, target) in list.iter().enumerate() {
            let item_pointer = format!("{pointer}/{i}");
            if target == name {
                broken = true;
                diagnostics.push(diag(
                    &item_pointer,
                    format!("face `{name}` lists itself in its own `fallback`"),
                    "a fallback chain names other faces",
                ));
            } else if !defined.contains(&target.as_str()) {
                broken = true;
                let help = match ludion_core::suggest::nearest(target, defined.iter().copied()) {
                    Some(near) => format!("did you mean `{near}`?"),
                    None => format!("defined: {}", defined.join(", ")),
                };
                diagnostics.push(diag(
                    &item_pointer,
                    format!("`fallback` of face `{name}` names `{target}`, which `fonts:` does not define"),
                    &help,
                ));
            }
        }
        if broken {
            continue;
        }
        let mut chain = vec![name.as_str()];
        let mut cyclic = false;
        flatten(doc, name, &mut chain, &mut cyclic);
        if cyclic {
            diagnostics.push(diag(
                &pointer,
                format!("the `fallback` chain of face `{name}` loops back to a face already in it"),
                "break the cycle: a chain only reaches faces that do not reach back",
            ));
        } else if chain.len() > MAX_CHAIN {
            diagnostics.push(diag(
                &pointer,
                format!(
                    "the flattened `fallback` chain of face `{name}` holds {} faces; the limit is {MAX_CHAIN}",
                    chain.len()
                ),
                "shorten the chain or drop nested fallbacks",
            ));
        }
    }
}

/// The longest flattened fallback chain, the entry's own face included.
pub const MAX_CHAIN: usize = 8;

/// `name`'s own fallback list (empty when it has none).
fn list_of<'a>(doc: &'a FontsDocument, name: &str) -> &'a [String] {
    doc.fallbacks
        .iter()
        .find(|(n, _)| n == name)
        .map_or(&[][..], |(_, l)| l.as_slice())
}

/// Depth-first flatten of `name`'s fallbacks into `chain` (deduped by name); sets `cyclic` on a
/// back edge to a face on the current path.
fn flatten<'a>(doc: &'a FontsDocument, name: &'a str, chain: &mut Vec<&'a str>, cyclic: &mut bool) {
    fn walk<'a>(
        doc: &'a FontsDocument,
        name: &'a str,
        path: &mut Vec<&'a str>,
        chain: &mut Vec<&'a str>,
        cyclic: &mut bool,
    ) {
        path.push(name);
        for target in list_of(doc, name) {
            if path.contains(&target.as_str()) {
                *cyclic = true;
                continue;
            }
            if !chain.contains(&target.as_str()) {
                chain.push(target);
            }
            walk(doc, target, path, chain, cyclic);
        }
        path.pop();
    }
    walk(doc, name, &mut Vec::new(), chain, cyclic);
}

/// Checks every name and path of a parsed [`FontsDocument`] (`L1253`).
pub(crate) fn validate(doc: &FontsDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    validate_fallbacks(doc, &mut diagnostics);
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
            match doc.fallbacks.iter().find(|(n, _)| n == name) {
                Some((_, list)) => {
                    out.push_str(&format!(
                        "  {}:\n    path: {}\n    fallback:\n",
                        scalar_string(name),
                        scalar_string(path)
                    ));
                    for target in list {
                        out.push_str(&format!("      - {}\n", scalar_string(target)));
                    }
                }
                None => out.push_str(&format!(
                    "  {}: {}\n",
                    scalar_string(name),
                    scalar_string(path)
                )),
            }
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
