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
        noun: "a voxel assembly",
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
        noun: "a skinned character",
        check: KindCheck::Here(|s| crate::check_character(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    // The registry half runs in `ludion check-scene`'s prefab arm.
    KindEntry {
        name: "prefab",
        noun: "a prefab",
        check: KindCheck::Here(|s| crate::parse_prefab(s).map(drop)),
        doc_store: false,
        model_sheet: false,
    },
    KindEntry {
        name: "path",
        noun: "a path",
        check: KindCheck::Here(|s| crate::check_path(s).map(drop)),
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

/// A textual pre-check for [`peek_kind_checked`]: `false` only when no line of `source` could be a
/// top-level `kind` key, so the full parse can be skipped. Never `false` for a document whose
/// `peek_kind_checked` is `Ok(Some(_))`; `true` for anything it cannot rule out.
///
/// The root block mapping's keys all sit at the indentation of the first content line (blank,
/// comment, `%` directive and bare `---`/`...` marker lines are not content), so only lines at that
/// indentation or less are candidates. A candidate is `true` when it spells the key as plain
/// `kind`, `"kind"` or `'kind'` followed by optional spaces/tabs and `:`, or when it starts in a
/// way this scan does not decode: `?` (an explicit key), `&`/`!`/`*` (node properties, an alias), a
/// double-quoted key with a `\` escape or no closing quote, or a `---`/`...` marker with content
/// after it. `\n`, `\r\n` and bare `\r` all break lines; a leading BOM is ignored; a tab never
/// counts as indentation. A `kind:` inside a comment or nested under another key is `false`.
pub(crate) fn may_declare_kind(source: &str) -> bool {
    let mut root_indent = None;
    for line in source.split(['\n', '\r']) {
        let line = line.trim_start_matches('\u{feff}');
        let indent = line.len() - line.trim_start_matches(' ').len();
        let rest = line[indent..].trim_start_matches([' ', '\t']);
        if rest.is_empty() || rest.starts_with('#') {
            continue;
        }
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
                return true;
            }
        }
        if indent > *root_indent.get_or_insert(indent) {
            continue;
        }
        if candidate_key_may_be_kind(rest) {
            return true;
        }
    }
    false
}

/// Whether a candidate line (indentation stripped) could open a `kind` key — see
/// [`may_declare_kind`].
fn candidate_key_may_be_kind(rest: &str) -> bool {
    let colon_follows = |after: &str| after.trim_start_matches([' ', '\t']).starts_with(':');
    if rest.starts_with(['?', '&', '!', '*']) {
        return true;
    }
    if let Some(body) = rest.strip_prefix('"') {
        return match body.find(['"', '\\']) {
            Some(end) if body[end..].starts_with('"') => {
                &body[..end] == "kind" && colon_follows(&body[end + 1..])
            }
            _ => true,
        };
    }
    if let Some(body) = rest.strip_prefix('\'') {
        return body
            .strip_prefix("kind'")
            .is_some_and(|after| !after.starts_with('\'') && colon_follows(after));
    }
    rest.strip_prefix("kind").is_some_and(colon_follows)
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

    /// Asserts the pre-check's answer and, whenever the full reader finds a `kind`, that the
    /// pre-check said `true` — the no-false-negative contract, checked on every case.
    fn check_may_declare(source: &str, expected: bool) {
        assert_eq!(may_declare_kind(source), expected, "{source:?}");
        if peek_kind_checked(source).ok().flatten().is_some() {
            assert!(may_declare_kind(source), "false negative on {source:?}");
        }
    }

    #[test]
    fn may_declare_kind_accepts_every_top_level_spelling() {
        check_may_declare("format: 1\nkind: mesh\n", true);
        check_may_declare("kind: mesh\n", true);
        check_may_declare("kind : mesh\n", true);
        check_may_declare("kind\t: mesh\n", true);
        check_may_declare("kind:\n", true);
        check_may_declare("\"kind\": mesh\n", true);
        check_may_declare("\"kind\" : mesh\n", true);
        check_may_declare("'kind': mesh\n", true);
        check_may_declare("\u{feff}kind: mesh\n", true);
        check_may_declare("format: 1\r\nkind: mesh\r\n", true);
        check_may_declare("format: 1\rkind: mesh\r", true);
        check_may_declare("---\nkind: mesh\n", true);
        check_may_declare("%YAML 1.2\n---\nkind: mesh\n...\n", true);
        check_may_declare("# header\n\n  format: 1\n  kind: mesh\n", true);
        check_may_declare(CUBE_TRI, true);
    }

    #[test]
    fn may_declare_kind_is_conservative_where_it_does_not_decode() {
        check_may_declare("\"k\\u0069nd\": mesh\n", true);
        check_may_declare("? kind\n: mesh\n", true);
        check_may_declare("?\n  kind\n: mesh\n", true);
        check_may_declare("&a kind: mesh\n", true);
        check_may_declare("--- kind: mesh\n", true);
        check_may_declare("\"unclosed\n", true);
    }

    #[test]
    fn may_declare_kind_rejects_what_cannot_be_a_top_level_kind() {
        check_may_declare("", false);
        check_may_declare("\n\n# only comments\n", false);
        check_may_declare("format: 1\nentities: []\n", false);
        check_may_declare("format: 1\nlight:\n  kind: point\n", false);
        check_may_declare("# kind: mesh\nformat: 1\n", false);
        check_may_declare("format: 1 # kind: mesh\n", false);
        check_may_declare("note: |\n  kind: mesh\n", false);
        check_may_declare("kinds: 2\nkind_of: 3\n\"kinds\": 4\n'kind''': 5\n", false);
        check_may_declare("kind of: mesh\n", false);
        check_may_declare("- kind: mesh\n", false);
        check_may_declare("---\n...\n", false);
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
            if !may_declare_kind(&source) {
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
