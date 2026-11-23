//! The one table of engine `kind:` strings, and the sniffers that read a document's `kind:` key.

use ludion_core::diagnostics::Diagnostic;
use serde_json::Value;

use crate::yaml::{self, NodeValue};

/// How `ludion check-scene` (and any other caller holding only a file's text) validates a kind.
pub enum KindCheck {
    /// Checked from its own text by the `ludion-scene` function held here.
    Here(fn(&str) -> Result<(), Vec<Diagnostic>>),
    /// Checked by another verb or host; the text names it for a user's help line.
    Elsewhere(&'static str),
}

/// One row of [`KINDS`].
pub struct KindEntry {
    /// The `kind:` string a document declares.
    pub name: &'static str,
    /// What the document is, with its article — "a mesh asset" — for messages.
    pub noun: &'static str,
    /// Who validates it.
    pub check: KindCheck,
    /// `true` when a game registers the kind as a data-doc (`pattern`, through
    /// `Engine::builder().patterns()`), so the content-root walk validates it instead of skipping it.
    pub doc_store: bool,
    /// `true` when a model-sheet render (`ludion render`, a golden scene) draws the document.
    pub model_sheet: bool,
}

/// Every engine `kind:` string, one row each. A new kind is one row here (plus its runner, where
/// it wants a rich summary): the engine's content-root walk skips the rows with
/// `doc_store: false` (their owners load them), `ludion check-scene` dispatches or points at the
/// checking verb, the testkit renders the `model_sheet` rows, and the facade's `--check-docs`
/// counts everything else as a game data-doc.
pub const KINDS: &[KindEntry] = &[
    KindEntry {
        name: "mesh",
        noun: "a mesh asset",
        check: KindCheck::Here(|s| crate::check_mesh(s).map(drop)),
        doc_store: false,
        model_sheet: true,
    },
    KindEntry {
        name: "mesh-edit",
        noun: "a mesh edit op list",
        check: KindCheck::Here(|s| crate::check_mesh_edit(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "texture",
        noun: "a procedural texture recipe",
        check: KindCheck::Elsewhere(
            "`ludion texture check <file>` validates it and drift-checks its map set",
        ),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "voxel",
        noun: "a voxel model",
        check: KindCheck::Here(|s| crate::check_voxel(s).map(drop)),
        doc_store: false,
        model_sheet: true,
    },
    KindEntry {
        name: "voxel-ops",
        noun: "a voxel op list",
        check: KindCheck::Here(|s| crate::parse_voxel_ops(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "profile",
        noun: "a player profile save",
        check: KindCheck::Elsewhere(
            "the game that wrote it validates it at boot (`ludion docs player-profiles`)",
        ),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "settings",
        noun: "a player settings save",
        check: KindCheck::Elsewhere(
            "the engine loads it leniently at boot (`ludion docs settings`)",
        ),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "input-bindings",
        noun: "a player input bindings save",
        check: KindCheck::Elsewhere("the engine loads it leniently at boot (`ludion docs input`)"),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "map",
        noun: "a map source",
        check: KindCheck::Elsewhere(
            "`ludion generate-map <file>` compiles and validates it; `ludion map-check` probes the result",
        ),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "map-kit",
        noun: "a map kit",
        check: KindCheck::Elsewhere(
            "`ludion generate-map` validates it through a map that names it",
        ),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "assembly",
        noun: "a voxel or skinned assembly",
        check: KindCheck::Here(|s| crate::check_assembly(s).map(drop)),
        doc_store: false,
        model_sheet: true,
    },
    KindEntry {
        name: "part",
        noun: "an assembly part",
        check: KindCheck::Here(|s| crate::check_part(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "character",
        noun: "a retired kind (merged into assembly)",
        check: KindCheck::Here(|s| crate::check_assembly(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    // The registry half runs in `ludion check-scene`'s prefab arm.
    KindEntry {
        name: "prefab",
        noun: "a prefab",
        check: KindCheck::Here(|s| crate::parse_prefab(s).map(drop)),
        doc_store: false,
        model_sheet: true,
    },
    KindEntry {
        name: "path",
        noun: "a path",
        check: KindCheck::Here(|s| crate::check_path(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "patrol",
        noun: "a patrol",
        check: KindCheck::Here(|s| crate::check_patrol(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "animation",
        noun: "an animation clip",
        check: KindCheck::Here(|s| crate::check_animation(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "sprite",
        noun: "a sprite",
        check: KindCheck::Here(|s| crate::check_sprite(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "palette",
        noun: "a sprite palette",
        check: KindCheck::Here(|s| crate::check_palette(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "atlas",
        noun: "a sprite atlas",
        check: KindCheck::Here(|s| crate::check_atlas(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "atlas-source",
        noun: "an atlas source",
        check: KindCheck::Here(|s| crate::check_atlas_source(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "font",
        noun: "a font",
        check: KindCheck::Here(|s| crate::check_font(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "font-source",
        noun: "a font source",
        check: KindCheck::Here(|s| crate::check_font_source(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "fonts",
        noun: "a font table",
        check: KindCheck::Here(|s| crate::check_fonts(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "sculpt",
        noun: "a sculpted model",
        check: KindCheck::Here(|s| crate::check_sculpt(s).map(drop)),
        doc_store: false,
        model_sheet: true,
    },
    KindEntry {
        name: "generation",
        noun: "an image-generation recipe",
        check: KindCheck::Here(|s| crate::check_generation(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "origin",
        noun: "an asset provenance sidecar",
        check: KindCheck::Here(|s| crate::check_origin_document(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "touch-layout",
        noun: "a touch-controls layout",
        check: KindCheck::Elsewhere("the native host's embedding core reads it (`ludion export`)"),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "pattern",
        noun: "a projectile pattern",
        check: KindCheck::Elsewhere(
            "the `--check-docs` of a game that registers `Engine::builder().patterns()`",
        ),
        doc_store: true,
        model_sheet: false,
    },
    KindEntry {
        name: "paired",
        noun: "a paired action",
        check: KindCheck::Here(|s| crate::check_paired(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
];

/// The [`KINDS`] row for a `kind:` string, or `None` when it names no engine kind.
///
/// # Example
/// ```
/// let mesh = ludion_scene::kind_entry("mesh").expect("mesh is an engine kind");
/// assert!(mesh.model_sheet && !mesh.doc_store);
/// assert!(ludion_scene::kind_entry("tilemap").is_none());
/// ```
pub fn kind_entry(name: &str) -> Option<&'static KindEntry> {
    KINDS.iter().find(|entry| entry.name == name)
}

/// Sniffs a document's top-level `kind:` key without fully parsing it, distinguishing "not
/// recognizably a mesh document" from "the strict-YAML subset rejects this file outright".
///
/// `Ok(None)` when the document has no `kind` key, is empty, or is not a mapping — every one of
/// those cases parses fine and means "not this kind"; the caller falls through to the scene
/// parser, whose own diagnostics explain the real problem when there is one. `Err` when the
/// strict-YAML reader itself rejects the source (a syntax error, an anchor, a non-empty flow
/// collection, ...) — that document was never a candidate for anything, and silently treating it
/// as "no kind" would drop it from every content-root walk with no diagnostic at all.
pub(crate) fn peek_kind_checked(source: &str) -> Result<Option<String>, Diagnostic> {
    let Some(root) = yaml::parse_document(source)? else {
        return Ok(None);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Ok(None);
    };
    Ok(entries
        .into_iter()
        .find(|entry| entry.key == "kind")
        .and_then(|entry| match entry.node.value {
            NodeValue::Scalar(Value::String(s)) => Some(s),
            _ => None,
        }))
}

/// What a textual scan says about a document's top-level `kind` — see `sniff_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindSniff<'a> {
    /// No line could be a top-level `kind` key: the full peek would answer `Ok(None)` or `Err`.
    Absent,
    /// The one top-level `kind` key, decoded: the full peek answers `Ok(Some(k))` or `Err`.
    Kind(&'a str),
    /// The scan cannot decide: full-parse.
    Undecided,
}

/// A textual pre-check for [`peek_kind_checked`]: decides a document's top-level `kind` without the
/// strict-YAML parse whenever the text is plain enough to read, and says [`KindSniff::Undecided`]
/// otherwise. See the public wrapper for the contract.
///
/// The root block mapping's keys all sit at the indentation of the first content line (blank,
/// comment, `%` directive and bare `---`/`...` marker lines are not content), so only lines at that
/// indentation or less are candidates. `\n`, `\r\n` and bare `\r` all break lines; a leading BOM is
/// ignored; a tab never counts as indentation. A key this scan does not decode (`?`, `&`/`!`/`*`, an
/// escaped or unclosed quoted key, a marker with content) is `Undecided`. Otherwise a file with no
/// `kind` key is `Absent`; once one is found, the scan runs to the end and a second `kind` key, a
/// tab-led line or a quoted value that does not close on its line (a multi-line scalar could hide a
/// `kind:`-shaped line) makes it `Undecided`. A `kind:` inside
/// a comment or nested under another key is not a candidate.
pub(crate) fn sniff_kind(source: &str) -> KindSniff<'_> {
    let mut root_indent = None;
    let mut found = None;
    let mut awaiting_continuation_check = false;
    // Only a found `kind` makes these matter: a kind-less file is `Absent` however odd its lines.
    let (mut saw_tab_line, mut saw_open_quote) = (false, false);
    for line in source.split(['\n', '\r']) {
        let line = line.trim_start_matches('\u{feff}');
        let indent = line.len() - line.trim_start_matches(' ').len();
        let rest = line[indent..].trim_start_matches([' ', '\t']);
        if rest.is_empty() {
            continue;
        }
        if awaiting_continuation_check {
            // A scalar folds its continuation lines: the kind value is the whole scalar only when
            // the next line cannot continue it.
            if indent > root_indent.unwrap_or(0) || line[indent..].starts_with('\t') {
                return KindSniff::Undecided;
            }
            awaiting_continuation_check = false;
        }
        if rest.starts_with('#') {
            continue;
        }
        saw_tab_line |= line[indent..].starts_with('\t');
        if indent == 0 {
            if rest.starts_with('%') {
                continue;
            }
            if let Some(after) = line
                .strip_prefix("---")
                .or_else(|| line.strip_prefix("..."))
                && (after.is_empty() || after.starts_with([' ', '\t']))
            {
                let after = after.trim_start_matches([' ', '\t']);
                if after.is_empty() || after.starts_with('#') {
                    continue;
                }
                return KindSniff::Undecided;
            }
        }
        if indent > *root_indent.get_or_insert(indent) {
            continue;
        }
        match classify_root_line(rest) {
            RootLine::NotKind => {}
            RootLine::OpenQuote => saw_open_quote = true,
            RootLine::Undecided => return KindSniff::Undecided,
            RootLine::Kind(value) => {
                if found.is_some() {
                    return KindSniff::Undecided;
                }
                let Some(kind) = decode_kind_value(value) else {
                    return KindSniff::Undecided;
                };
                found = Some(kind);
                awaiting_continuation_check = true;
            }
        }
    }
    match found {
        None => KindSniff::Absent,
        Some(_) if saw_tab_line || saw_open_quote => KindSniff::Undecided,
        Some(kind) => KindSniff::Kind(kind),
    }
}

/// What one root-level line (indentation stripped) is, as far as [`sniff_kind`] can tell.
enum RootLine<'a> {
    /// A key other than `kind`, or no key at all.
    NotKind,
    /// A `kind` key; the text after its `:`.
    Kind(&'a str),
    /// Starts in a way this scan does not decode.
    Undecided,
    /// A value whose quote does not close on the line: it may fold onto the following lines.
    OpenQuote,
}

fn classify_root_line(rest: &str) -> RootLine<'_> {
    if rest.starts_with(['?', '&', '!', '*']) {
        return RootLine::Undecided;
    }
    let (key, after_key) = if let Some(body) = rest.strip_prefix('"') {
        match body.find(['"', '\\']) {
            Some(end) if body[end..].starts_with('"') => (&body[..end], &body[end + 1..]),
            _ => return RootLine::Undecided,
        }
    } else if let Some(body) = rest.strip_prefix('\'') {
        let Some(end) = single_quote_end(body) else {
            return RootLine::Undecided;
        };
        (&body[..end], &body[end + 1..])
    } else {
        return classify_plain_key(rest);
    };
    match after_key.trim_start_matches([' ', '\t']).strip_prefix(':') {
        Some(value) if key == "kind" => RootLine::Kind(value),
        Some(value) => value_guard(value),
        None => RootLine::NotKind,
    }
}

fn classify_plain_key(rest: &str) -> RootLine<'_> {
    if let Some(after) = rest.strip_prefix("kind")
        && let Some(value) = after.trim_start_matches([' ', '\t']).strip_prefix(':')
    {
        return RootLine::Kind(value);
    }
    match value_colon(rest) {
        Some(colon) => value_guard(&rest[colon + 1..]),
        None => RootLine::NotKind,
    }
}

/// Byte offset of the `'` that closes a single-quoted body (`''` is an escape), if it does.
fn single_quote_end(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            if bytes.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Byte offset of a plain key's `:` — one followed by a space, a tab or the end of the line.
fn value_colon(rest: &str) -> Option<usize> {
    rest.match_indices(':')
        .map(|(i, _)| i)
        .find(|&i| matches!(rest.as_bytes().get(i + 1), None | Some(b' ' | b'\t')))
}

/// A non-`kind` key's value: a quote that does not close on the line may fold onto the next lines.
fn value_guard(value: &str) -> RootLine<'_> {
    let value = value.trim_start_matches([' ', '\t']);
    let closes = if let Some(body) = value.strip_prefix('"') {
        let mut chars = body.chars();
        loop {
            match chars.next() {
                Some('\\') => {
                    chars.next();
                }
                Some('"') => break true,
                Some(_) => {}
                None => break false,
            }
        }
    } else if let Some(body) = value.strip_prefix('\'') {
        single_quote_end(body).is_some()
    } else {
        true
    };
    if closes {
        RootLine::NotKind
    } else {
        RootLine::OpenQuote
    }
}

/// The `kind` scalar's string value when it is a one-line quoted scalar or a plain scalar that
/// resolves to a string; `None` for anything this scan will not decode.
fn decode_kind_value(value: &str) -> Option<&str> {
    if !(value.is_empty() || value.starts_with([' ', '\t'])) {
        return None; // `kind:mesh` is not a key
    }
    let value = value.trim_matches([' ', '\t']);
    let only_comment_after = |after: &str| {
        after.is_empty()
            || (after.starts_with([' ', '\t'])
                && after.trim_start_matches([' ', '\t']).starts_with('#'))
    };
    if let Some(body) = value.strip_prefix('"') {
        let end = body.find('"')?;
        return (!body[..end].contains('\\') && only_comment_after(&body[end + 1..]))
            .then_some(&body[..end]);
    }
    if let Some(body) = value.strip_prefix('\'') {
        let end = body.find('\'')?;
        return (!body[end + 1..].starts_with('\'') && only_comment_after(&body[end + 1..]))
            .then_some(&body[..end]);
    }
    if value.is_empty()
        || value.starts_with([
            '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '%', '@', '`',
        ])
    {
        return None;
    }
    let comment = value
        .match_indices('#')
        .map(|(i, _)| i)
        .find(|&i| value[..i].ends_with([' ', '\t']));
    let text = comment
        .map_or(value, |i| &value[..i])
        .trim_end_matches([' ', '\t']);
    if text.contains(": ") || text.contains(":\t") || text.ends_with(':') {
        return None;
    }
    matches!(
        crate::scalar::resolve_plain(text),
        crate::scalar::Resolved::Str
    )
    .then_some(text)
}

/// Thin wrapper over [`peek_kind_checked`] for callers that only ever fell through to a scene
/// parse on failure anyway (`ludion check-scene`'s dispatch): a parse error collapses to `None`
/// alongside every other "not this kind" case.
pub(crate) fn peek_kind(source: &str) -> Option<String> {
    peek_kind_checked(source).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE_TRI: &str = "\
format: 1
kind: mesh
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
indices:
  - 0
  - 1
  - 2
";

    #[test]
    fn peek_kind_finds_mesh() {
        assert_eq!(peek_kind(CUBE_TRI).as_deref(), Some("mesh"));
        assert_eq!(peek_kind("format: 1\nentities: []\n"), None);
        assert_eq!(peek_kind(""), None);
        assert_eq!(peek_kind("a: \"unclosed\n"), None); // a syntax error, not a panic
    }

    #[test]
    fn peek_kind_checked_distinguishes_no_kind_from_a_parse_failure() {
        assert_eq!(
            peek_kind_checked(CUBE_TRI).unwrap().as_deref(),
            Some("mesh")
        );
        assert_eq!(
            peek_kind_checked("format: 1\nentities: []\n").unwrap(),
            None,
            "no `kind` key: a scene, not a parse failure"
        );
        assert_eq!(peek_kind_checked("").unwrap(), None, "an empty document");
        peek_kind_checked("a: \"unclosed\n")
            .expect_err("a syntax error is reported, not silently treated as `None`");
        peek_kind_checked("a: { b: 1 }\n")
            .expect_err("a non-empty flow mapping is outside the strict subset");
    }

    /// Asserts the sniff's answer and its agreement contract with the full reader on every case:
    /// `Absent` ⇒ no `Ok(Some(_))`; `Kind(k)` ⇒ `Ok(Some(k))` or `Err`.
    fn check_sniff(source: &str, expected: KindSniff<'_>) {
        assert_eq!(sniff_kind(source), expected, "{source:?}");
        let full = peek_kind_checked(source);
        match expected {
            KindSniff::Absent => {
                assert!(!matches!(full, Ok(Some(_))), "false Absent on {source:?}")
            }
            KindSniff::Kind(kind) => assert!(
                full.as_ref().map_or(true, |k| k.as_deref() == Some(kind)),
                "the full peek disagrees with Kind({kind:?}) on {source:?}: {full:?}"
            ),
            KindSniff::Undecided => {}
        }
    }

    #[test]
    fn sniff_kind_decodes_every_top_level_spelling() {
        let mesh = KindSniff::Kind("mesh");
        for source in [
            "format: 1\nkind: mesh\n",
            "kind: mesh\n",
            "kind : mesh\n",
            "kind\t: mesh\n",
            "\"kind\": mesh\n",
            "\"kind\" : mesh\n",
            "'kind': mesh\n",
            "\u{feff}kind: mesh\n",
            "format: 1\r\nkind: mesh\r\n",
            "format: 1\rkind: mesh\r",
            "---\nkind: mesh\n",
            "%YAML 1.2\n---\nkind: mesh\n...\n",
            "# header\n\n  format: 1\n  kind: mesh\n",
            "kind: mesh # c\n",
            "kind: \"mesh\"\n",
            "kind: 'mesh'\n",
            "kind: \"mesh\" # c\n",
            "  format: 1\n  kind: mesh\n",
            "kind: mesh\n\n# note\nformat: 1\n",
            CUBE_TRI,
        ] {
            check_sniff(source, mesh);
        }
        check_sniff("\"kind\": 'prefab'\n", KindSniff::Kind("prefab"));
        let big = format!(
            "kind: mesh\nformat: 1\npositions:\n{}",
            "  - 0.5\n".repeat(1000)
        );
        check_sniff(&big, mesh);
    }

    #[test]
    fn sniff_kind_is_undecided_where_it_does_not_decode() {
        for source in [
            "kind:\n",
            "kind:mesh\n",
            "kind: \"me\\u0073h\"\n",
            "kind: 'it''s'\n",
            "kind: me\n  sh\n",
            "kind: \"me\n  sh\"\n",
            "kind: 1\n",
            "kind: true\n",
            "kind: ~\n",
            "kind: &a mesh\n",
            "kind: !t mesh\n",
            "kind: [mesh]\n",
            "kind: mesh: x\n",
            "kind: mesh\nkind: prefab\n",
            "kind: mesh\n\tmore\n",
            "kind: \"mesh\"x\n",
            "a: \"x\nkind: mesh\"\n",
            "a: 'x\nkind: mesh'\n",
            "? kind\n: mesh\n",
            "?\n  kind\n: mesh\n",
            "&a kind: mesh\n",
            "\"k\\u0069nd\": mesh\n",
            "--- kind: mesh\n",
            "\"unclosed\n",
            "a: 1\n\tkind: mesh\n",
        ] {
            check_sniff(source, KindSniff::Undecided);
        }
    }

    #[test]
    fn sniff_kind_rules_out_what_cannot_be_a_top_level_kind() {
        for source in [
            "",
            "\n\n# only comments\n",
            "format: 1\nentities: []\n",
            "format: 1\nlight:\n  kind: point\n",
            "# kind: mesh\nformat: 1\n",
            "format: 1 # kind: mesh\n",
            "note: |\n  kind: mesh\n",
            "format: 1\n\tnote: x\n",
            "a: \"x\n  y\"\n",
            "a: 'x\n  y'\n",
            "- a: \"x\n",
            "kinds: 2\nkind_of: 3\n\"kinds\": 4\n'kind''': 5\n",
            "kind of: mesh\n",
            "- kind: mesh\n",
            "---\n...\n",
        ] {
            check_sniff(source, KindSniff::Absent);
        }
    }

    /// Every kind a `ludion-scene` parser declares through `envelope::check_kind(entry, CODE,
    /// "<kind>", …)` — the first string literal after the call opens.
    fn scene_kinds() -> Vec<(String, String)> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut kinds = Vec::new();
        for entry in std::fs::read_dir(dir).expect("ludion-scene/src is readable") {
            let path = entry.expect("directory entry").path();
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            // This file's docs quote the call.
            if path.extension().is_none_or(|ext| ext != "rs") || file == "kinds.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("source file is readable");
            let mut rest = source.as_str();
            while let Some(at) = rest.find("envelope::check_kind(") {
                rest = &rest[at + "envelope::check_kind(".len()..];
                let Some(open) = rest.find('"') else { break };
                let literal = &rest[open + 1..];
                let Some(close) = literal.find('"') else {
                    break;
                };
                kinds.push((file.clone(), literal[..close].to_string()));
            }
        }
        kinds
    }

    #[test]
    fn every_scene_kind_has_a_row() {
        let kinds = scene_kinds();
        assert!(
            kinds.len() >= 15,
            "the scan found only {} kinds — did `envelope::check_kind` change shape?",
            kinds.len()
        );
        for (file, kind) in kinds {
            assert!(
                kind_entry(&kind).is_some(),
                "ludion-scene/src/{file} parses `kind: {kind}` but it has no row in \
                 `ludion_scene::KINDS` — add one, or a content root holding it fails its game's \
                 boot with L1119"
            );
        }
        assert!(kind_entry(crate::profile::PROFILE_KIND).is_some());
        assert!(kind_entry("settings").is_some());
        assert!(kind_entry("input-bindings").is_some());
    }

    #[test]
    fn kind_names_are_unique() {
        for (at, entry) in KINDS.iter().enumerate() {
            assert!(
                KINDS[..at].iter().all(|earlier| earlier.name != entry.name),
                "`{}` has two rows in KINDS",
                entry.name
            );
        }
    }

    /// The committed generation recipes and origin sidecars have no other gate: the engine walk
    /// skips them and no loader reads them, so a schema drift would surface only when `gen` runs.
    #[test]
    fn committed_recipes_and_origins_pass_their_checker() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("directory is readable") {
                let path = entry.expect("directory entry").path();
                if path.is_dir() {
                    let name = path.file_name().unwrap().to_string_lossy();
                    if !matches!(name.as_ref(), ".ludion" | "target" | "node_modules") {
                        walk(&path, out);
                    }
                } else if path.extension().is_some_and(|ext| ext == "yaml") {
                    out.push(path);
                }
            }
        }
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let games = crates.join("../games");
        let mut files = Vec::new();
        walk(&games, &mut files);
        // The Lore-only game is absent from a public clone; its recipes survive as frozen fixtures.
        walk(
            &crates.join("ludion-cli/tests/fixtures/idol-generation"),
            &mut files,
        );
        files.sort();
        files.dedup();
        let (mut checked, mut failures) = (0, Vec::new());
        for path in files {
            let source = std::fs::read_to_string(&path).expect("yaml file is readable");
            if sniff_kind(&source) == KindSniff::Absent {
                continue;
            }
            let Some(kind) = peek_kind(&source) else {
                continue;
            };
            if !matches!(kind.as_str(), "generation" | "origin") {
                continue;
            }
            let Some(KindEntry {
                check: KindCheck::Here(check),
                ..
            }) = kind_entry(&kind)
            else {
                panic!("`{kind}` must be checked from its own text");
            };
            checked += 1;
            if let Err(diagnostics) = check(&source) {
                failures.push(format!("{}: {diagnostics:?}", path.display()));
            }
        }
        assert!(
            failures.is_empty(),
            "committed documents fail their checker: {failures:#?}"
        );
        let floor = if games.join("vigil").is_dir() { 5 } else { 2 };
        assert!(
            checked >= floor,
            "found only {checked} committed generation/origin documents — did they move?"
        );
    }
}
