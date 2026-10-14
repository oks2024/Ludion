//! [`assert_saves_respell_nothing`] — the corpus gate behind `Plan/audits/2026-10-03-scene-text.md`
//! S1: a committed hand-authored scene or prefab already spells every value the way the engine
//! writes it.
//!
//! `scene.save` (and `prefab.apply`) re-emit a document through the canonical writer, which prints
//! floats as their shortest decimal and quotes strings its own way. A source spelled otherwise
//! (`1` in a float field, a widened 17-digit f32) comes back respelled, so the first save of a
//! hand edit buries it in churn. The gate compares the *spelling* of every value scalar, per JSON
//! Pointer, between the source and what the writer would produce; comments and layout are not
//! compared. `LUDION_BLESS=1` rewrites same-value respellings in place, touching only the scalar's
//! own characters.
//!
//! Spellings are read with `saphyr_parser` directly, not through `ludion-scene`'s reader, so the
//! oracle shares no code with the writer it checks.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

use ludion_core::diagnostics::pointer_token;
use ludion_ecs::Registry;
use ludion_engine::EngineBuilder;
use ludion_protocol::Command;
use ludion_protocol::command::SceneLoadParams;
use saphyr_parser::{Event, Parser, ScalarStyle};
use serde_json::Value;

/// Directories the walk never enters: derived trees, build output and goldens.
const SKIPPED_DIRS: &[&str] = &[".ludion", "target", "goldens", "node_modules"];

/// One value scalar of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Spelled {
    pub pointer: String,
    /// The scalar's own source characters, quotes included.
    pub raw: String,
    pub decoded: String,
    pub plain: bool,
    /// Char indices (saphyr counts chars, not bytes) of `raw` in the source.
    pub span: Range<usize>,
}

enum Frame {
    Map { key: Option<String> },
    Seq { index: usize },
}

impl Frame {
    fn segment(&self) -> String {
        match self {
            Frame::Map { key } => key.clone().unwrap_or_default(),
            Frame::Seq { index } => index.to_string(),
        }
    }

    fn advance(&mut self) {
        match self {
            Frame::Map { key } => *key = None,
            Frame::Seq { index } => *index += 1,
        }
    }
}

/// Every non-empty value scalar of `text`, in document order. Keys are not recorded.
pub(crate) fn value_scalars(text: &str) -> Result<Vec<Spelled>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut stack: Vec<Frame> = Vec::new();
    let mut out = Vec::new();
    for step in Parser::new_from_str(text) {
        let (event, span) = step.map_err(|err| {
            format!(
                "{} (line {}, column {})",
                err.info(),
                err.marker().line(),
                err.marker().col() + 1
            )
        })?;
        match event {
            Event::Scalar(decoded, style, ..) => {
                if let Some(Frame::Map { key: key @ None }) = stack.last_mut() {
                    *key = Some(decoded.to_string());
                    continue;
                }
                let range = span.start.index()..span.end.index();
                if !range.is_empty() {
                    let mut pointer = String::new();
                    for frame in &stack {
                        pointer.push('/');
                        pointer.push_str(&pointer_token(&frame.segment()));
                    }
                    out.push(Spelled {
                        pointer,
                        raw: chars[range.clone()].iter().collect(),
                        decoded: decoded.to_string(),
                        plain: style == ScalarStyle::Plain,
                        span: range,
                    });
                }
                if let Some(top) = stack.last_mut() {
                    top.advance();
                }
            }
            Event::SequenceStart(..) => stack.push(Frame::Seq { index: 0 }),
            Event::MappingStart(..) => stack.push(Frame::Map { key: None }),
            Event::SequenceEnd | Event::MappingEnd => {
                stack.pop();
                if let Some(top) = stack.last_mut() {
                    top.advance();
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// The save respells the scalar to the same value; blessable.
    SameValue = 0,
    ValueChanged = 1,
    Dropped = 2,
}

#[derive(Debug)]
struct Finding {
    pointer: String,
    class: Class,
    source: String,
    saved: String,
    span: Range<usize>,
}

/// A plain scalar YAML resolves to null, a bool or a number — a quoted twin is a different value.
fn resolves_to_non_string(spelled: &Spelled) -> bool {
    spelled.plain
        && (matches!(
            spelled.decoded.as_str(),
            "~" | "null" | "Null" | "NULL" | "true" | "True" | "TRUE" | "false" | "False" | "FALSE"
        ) || spelled.decoded.parse::<f64>().is_ok())
}

/// The numeric type of the field a scalar sits in, read from its component's schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumKind {
    /// `format: float` — the writer prints the shortest f32 decimal.
    F32,
    /// A `number` field without the f32 format, read and written as f64.
    F64,
    Integer,
}

fn is_integer_spelling(text: &str) -> bool {
    text.parse::<i128>().is_ok()
}

/// Whether two plain numbers are one value in a field of `kind` (`None`: the position is not a
/// registered component's field, or its type is ambiguous, so only a same-kind respelling passes).
fn same_number(a: &str, b: &str, kind: Option<NumKind>) -> bool {
    if let (Ok(x), Ok(y)) = (a.parse::<i128>(), b.parse::<i128>()) {
        return x == y;
    }
    let (Ok(x), Ok(y)) = (a.parse::<f64>(), b.parse::<f64>()) else {
        return false;
    };
    match kind {
        Some(NumKind::F32) => (x as f32).to_bits() == (y as f32).to_bits(),
        Some(NumKind::F64) => x == y,
        Some(NumKind::Integer) | None => x == y && is_integer_spelling(a) == is_integer_spelling(b),
    }
}

fn same_value(source: &Spelled, saved: &Spelled, kind: Option<NumKind>) -> bool {
    if source.plain && saved.plain {
        return same_number(&source.decoded, &saved.decoded, kind);
    }
    source.decoded == saved.decoded
        && !resolves_to_non_string(source)
        && !resolves_to_non_string(saved)
}

/// Bound on `$ref`/alternative chains, so a reference cycle cannot hang the walk.
const SCHEMA_DEPTH: usize = 16;

/// Every subschema `node` can stand for: itself, its `$ref` target and each `anyOf`/`oneOf`/`allOf`
/// alternative, recursively.
fn alternatives<'a>(root: &'a Value, node: &'a Value, depth: usize, out: &mut Vec<&'a Value>) {
    if depth > SCHEMA_DEPTH {
        return;
    }
    if let Some(target) = node
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix('#'))
        .and_then(|p| root.pointer(p))
    {
        alternatives(root, target, depth + 1, out);
    }
    out.push(node);
    for key in ["anyOf", "oneOf", "allOf"] {
        for part in node
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            alternatives(root, part, depth + 1, out);
        }
    }
}

/// The numeric kind of the value at `path` (unescaped segments) in a component's `schema`; `None`
/// when the path leaves the schema or the candidate subschemas disagree.
fn number_kind(schema: &Value, path: &[String]) -> Option<NumKind> {
    let mut nodes = Vec::new();
    alternatives(schema, schema, 0, &mut nodes);
    for segment in path {
        let mut next = Vec::new();
        for node in nodes {
            let child = node
                .get("properties")
                .and_then(|p| p.get(segment))
                .or_else(|| {
                    let index = segment.parse::<usize>().ok()?;
                    node.get("prefixItems")
                        .and_then(|p| p.get(index))
                        .or_else(|| node.get("items"))
                })
                .or_else(|| node.get("additionalProperties"));
            if let Some(child) = child {
                alternatives(schema, child, 0, &mut next);
            }
        }
        nodes = next;
    }
    let mut kinds = nodes.iter().filter_map(|node| {
        let types: Vec<&str> = match node.get("type")? {
            Value::String(t) => vec![t.as_str()],
            Value::Array(list) => list.iter().filter_map(Value::as_str).collect(),
            _ => return None,
        };
        if types.contains(&"integer") {
            Some(NumKind::Integer)
        } else if types.contains(&"number") {
            Some(
                if node.get("format").and_then(Value::as_str) == Some("float") {
                    NumKind::F32
                } else {
                    NumKind::F64
                },
            )
        } else {
            None
        }
    });
    let first = kinds.next()?;
    kinds.all(|k| k == first).then_some(first)
}

/// The numeric kind at a document JSON Pointer: the part after `.../components/<Name>` resolved
/// through that component's schema.
fn pointer_kind(registry: &Registry, pointer: &str) -> Option<NumKind> {
    let segments: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect();
    let at = segments.iter().position(|s| s == "components")? + 1;
    let info = registry.get(segments.get(at)?)?;
    number_kind(info.schema(), &segments[at + 1..])
}

/// Compares `source` with `saved` by pointer over the source's scalars. A pointer only in `saved`
/// (a default or layout the writer adds) is ignored.
fn compare(
    source: &str,
    saved: &str,
    kind_at: &dyn Fn(&str) -> Option<NumKind>,
) -> Result<Vec<Finding>, String> {
    let saved_by_pointer: BTreeMap<String, Spelled> = value_scalars(saved)
        .map_err(|e| format!("the saved text does not parse: {e}"))?
        .into_iter()
        .map(|s| (s.pointer.clone(), s))
        .collect();
    let mut findings = Vec::new();
    for spelled in value_scalars(source)? {
        let (class, saved_raw) = match saved_by_pointer.get(&spelled.pointer) {
            None => (Class::Dropped, String::new()),
            Some(other) if other.raw == spelled.raw => continue,
            Some(other) if same_value(&spelled, other, kind_at(&spelled.pointer)) => {
                (Class::SameValue, other.raw.clone())
            }
            Some(other) => (Class::ValueChanged, other.raw.clone()),
        };
        findings.push(Finding {
            pointer: spelled.pointer,
            class,
            source: spelled.raw,
            saved: saved_raw,
            span: spelled.span,
        });
    }
    Ok(findings)
}

/// Replaces every `SameValue` span with its saved spelling, back to front.
fn blessed_text(source: &str, findings: &[Finding]) -> String {
    let mut chars: Vec<char> = source.chars().collect();
    let mut same: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.class == Class::SameValue)
        .collect();
    same.sort_by_key(|f| std::cmp::Reverse(f.span.start));
    for finding in same {
        chars.splice(finding.span.clone(), finding.saved.chars());
    }
    chars.into_iter().collect()
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if !SKIPPED_DIRS.contains(&name) {
                collect_yaml(&path, out);
            }
        } else if name.ends_with(".yaml") {
            out.push(path);
        }
    }
}

fn is_generated(text: &str) -> bool {
    text.lines().any(|line| line.starts_with("generated_from:"))
}

/// Asserts that loading and saving every hand-authored scene and prefab under `project/<root>`
/// respells no value (S1 of `Plan/audits/2026-10-03-scene-text.md`).
///
/// Scenes go through `scene.load` and `Engine::scene_text`, prefabs through `check_prefab` and
/// `emit_prefab`. Files with a top-level `generated_from:` and any other `kind:` are skipped; each
/// has its own gate. With `LUDION_BLESS=1`, same-value respellings are rewritten in place (only
/// the scalar's characters change); a changed or dropped value is never blessed.
///
/// ```no_run
/// let project = std::path::Path::new("games/breakout");
/// ludion_testkit::assert_saves_respell_nothing(project, &["scenes"], ludion::prelude::Engine::builder());
/// ```
///
/// # Panics
/// If no file was checked, or any file fails to load or would be saved with a different spelling.
/// The message names each file, JSON Pointer, source spelling and saved spelling.
pub fn assert_saves_respell_nothing(project: &Path, roots: &[&str], builder: EngineBuilder) {
    let mut files = Vec::new();
    for root in roots {
        collect_yaml(&project.join(root), &mut files);
    }
    let mut engine = builder
        .project_root(project)
        .build()
        .expect("the project's engine builds");
    let bless = ludion_core::bless::bless_requested();
    let mut checked = 0usize;
    let mut report = String::new();
    let mut tally = [0usize; 4];
    for path in files {
        let rel = path
            .strip_prefix(project)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let source = std::fs::read_to_string(&path).expect("a scene file reads");
        if is_generated(&source) {
            continue;
        }
        let kind = match ludion_scene::peek_kind_checked(&source) {
            Ok(kind) => kind,
            Err(err) => {
                checked += 1;
                tally[3] += 1;
                let _ = writeln!(report, "{rel}: does not read as strict YAML: {err}");
                continue;
            }
        };
        let saved = match kind.as_deref() {
            None => engine
                .execute(Command::SceneLoad(SceneLoadParams { path: rel.clone() }))
                .map_err(|err| err.to_string())
                .and_then(|_| engine.scene_text().map_err(|err| err.to_string())),
            Some("prefab") => ludion_scene::check_prefab(&source, engine.registry())
                .map(|doc| ludion_scene::emit_prefab(&doc, engine.registry()))
                .map_err(|errs| {
                    errs.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n")
                }),
            Some(_) => continue,
        };
        checked += 1;
        let saved = match saved {
            Ok(saved) => saved,
            Err(err) => {
                tally[3] += 1;
                let _ = writeln!(report, "{rel}: does not load: {err}");
                continue;
            }
        };
        let findings = match compare(&source, &saved, &|p| pointer_kind(engine.registry(), p)) {
            Ok(findings) => findings,
            Err(err) => {
                tally[3] += 1;
                let _ = writeln!(report, "{rel}: {err}");
                continue;
            }
        };
        if bless && findings.iter().any(|f| f.class == Class::SameValue) {
            ludion_core::atomic::write_atomic(&path, blessed_text(&source, &findings))
                .expect("a blessed file writes");
        }
        for finding in &findings {
            tally[finding.class as usize] += 1;
            let line = match finding.class {
                Class::SameValue if bless => continue,
                Class::SameValue => {
                    format!("source `{}` saves as `{}`", finding.source, finding.saved)
                }
                Class::ValueChanged => format!(
                    "the save changes this value: source `{}` saves as `{}`",
                    finding.source, finding.saved
                ),
                Class::Dropped => format!("the save drops this value: source `{}`", finding.source),
            };
            let _ = writeln!(report, "{rel}: {}: {line}", finding.pointer);
        }
    }
    assert!(
        checked > 0,
        "no hand-authored scene or prefab found under {roots:?} in `{}` — the gate checked nothing",
        project.display()
    );
    assert!(
        report.is_empty(),
        "committed scenes and prefabs must be spelled the way the engine writes them, so a save \
         never respells a value (audit scene-text S1) — {} respelled, {} changed, {} dropped, {} \
         unloadable, in {checked} files:\n{report}\nfix: respell the source to the saved form; \
         `LUDION_BLESS=1` on this test rewrites same-value spellings in place, comments kept. A \
         changed or dropped value is a load/save defect and is never blessed.",
        tally[0],
        tally[1],
        tally[2],
        tally[3],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raws(text: &str) -> Vec<(String, String)> {
        value_scalars(text)
            .unwrap()
            .into_iter()
            .map(|s| (s.pointer, s.raw))
            .collect()
    }

    fn pair(pointer: &str, raw: &str) -> (String, String) {
        (pointer.to_string(), raw.to_string())
    }

    #[test]
    fn block_and_flow_sequences_and_nested_maps() {
        let text = "a:\n  b: [1, 2.50]\n  c:\n    - x\n    - y: 3\nd: 4\n";
        assert_eq!(
            raws(text),
            vec![
                pair("/a/b/0", "1"),
                pair("/a/b/1", "2.50"),
                pair("/a/c/0", "x"),
                pair("/a/c/1/y", "3"),
                pair("/d", "4"),
            ]
        );
    }

    #[test]
    fn raw_is_exactly_the_token() {
        let text = "p: plain  # note\ns: 'single'\nd: \"double \\n\"\nf: {k: 1.5}\n";
        let found = raws(text);
        assert_eq!(
            found,
            vec![
                pair("/p", "plain"),
                pair("/s", "'single'"),
                pair("/d", "\"double \\n\""),
                pair("/f/k", "1.5"),
            ]
        );
    }

    #[test]
    fn keys_are_escaped_and_not_recorded() {
        let text = "\"a/b\": 1\n\"c~d\": 2\n";
        assert_eq!(raws(text), vec![pair("/a~1b", "1"), pair("/c~0d", "2")]);
    }

    #[test]
    fn spans_slice_back_to_raw_in_multibyte_text() {
        let text = "name: \"é\"\nv: 0.5\n";
        let found = value_scalars(text).unwrap();
        assert_eq!(found[1].raw, "0.5");
        assert_eq!(found[1].span, 13..16);
    }

    fn classes_as(source: &str, saved: &str, kind: Option<NumKind>) -> Vec<Class> {
        compare(source, saved, &|_| kind)
            .unwrap()
            .iter()
            .map(|f| f.class)
            .collect()
    }

    fn classes(source: &str, saved: &str) -> Vec<Class> {
        classes_as(source, saved, Some(NumKind::F64))
    }

    #[test]
    fn identical_spelling_is_no_finding() {
        assert!(classes("a: 0.3\n", "a: 0.3\n").is_empty());
    }

    #[test]
    fn integer_to_float_and_widened_f32_are_same_value() {
        assert_eq!(classes("a: 1\n", "a: 1.0\n"), vec![Class::SameValue]);
        assert_eq!(
            classes_as("a: 0.30000001192092896\n", "a: 0.3\n", Some(NumKind::F32)),
            vec![Class::SameValue]
        );
        assert_eq!(classes("a: 1.0e-7\n", "a: 1e-7\n"), vec![Class::SameValue]);
    }

    #[test]
    fn a_different_number_is_a_changed_value() {
        assert_eq!(classes("a: 0.3\n", "a: 0.4\n"), vec![Class::ValueChanged]);
    }

    #[test]
    fn the_f32_rule_applies_only_in_an_f32_field() {
        let widened = ("a: 0.30000001192092896\n", "a: 0.3\n");
        assert_eq!(
            classes_as(widened.0, widened.1, Some(NumKind::F64)),
            vec![Class::ValueChanged]
        );
        assert_eq!(
            classes_as(widened.0, widened.1, None),
            vec![Class::ValueChanged]
        );
    }

    #[test]
    fn an_integer_field_keeps_its_value_and_its_spelling() {
        for kind in [Some(NumKind::Integer), None] {
            assert_eq!(
                classes_as("a: 16777217\n", "a: 16777216\n", kind),
                vec![Class::ValueChanged]
            );
            assert_eq!(
                classes_as("a: 1\n", "a: 1.0\n", kind),
                vec![Class::ValueChanged]
            );
        }
    }

    #[test]
    fn number_kind_follows_refs_options_arrays_and_alternatives() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "wide": { "type": "number" },
                "narrow": { "type": ["number", "null"], "format": "float" },
                "count": { "$ref": "#/$defs/Count" },
                "pos": { "type": "array", "items": { "type": "number", "format": "float" } },
                "tagged": { "oneOf": [
                    { "properties": { "n": { "type": "integer" } } },
                    { "properties": { "n": { "type": "integer" } } }
                ] },
                "mixed": { "oneOf": [
                    { "properties": { "n": { "type": "integer" } } },
                    { "properties": { "n": { "type": "number" } } }
                ] }
            },
            "$defs": { "Count": { "type": "integer" } }
        });
        let at = |path: &[&str]| {
            let path: Vec<String> = path.iter().map(ToString::to_string).collect();
            number_kind(&schema, &path)
        };
        assert_eq!(at(&["wide"]), Some(NumKind::F64));
        assert_eq!(at(&["narrow"]), Some(NumKind::F32));
        assert_eq!(at(&["count"]), Some(NumKind::Integer));
        assert_eq!(at(&["pos", "2"]), Some(NumKind::F32));
        assert_eq!(at(&["tagged", "n"]), Some(NumKind::Integer));
        assert_eq!(at(&["mixed", "n"]), None);
        assert_eq!(at(&["missing"]), None);
    }

    #[test]
    fn a_registered_u64_field_is_an_integer_kind() {
        let engine = EngineBuilder::default().build().unwrap();
        let kind = |pointer| pointer_kind(engine.registry(), pointer);
        assert_eq!(
            kind("/entities/0/components/ParticleEmitter/seed"),
            Some(NumKind::Integer)
        );
        assert_eq!(
            kind("/entities/0/components/Transform/translation/1"),
            Some(NumKind::F32)
        );
        assert_eq!(kind("/seed"), None);
    }

    #[test]
    fn quoting_a_string_is_same_value_but_not_a_number() {
        assert_eq!(classes("a: 'x'\n", "a: \"x\"\n"), vec![Class::SameValue]);
        assert_eq!(
            classes("a: 1.0\n", "a: \"1.0\"\n"),
            vec![Class::ValueChanged]
        );
        assert_eq!(
            classes("a: 'true'\n", "a: true\n"),
            vec![Class::ValueChanged]
        );
    }

    #[test]
    fn a_missing_pointer_is_dropped_and_a_saved_only_one_is_ignored() {
        assert_eq!(classes("a: 1\nb: 2\n", "a: 1\n"), vec![Class::Dropped]);
        assert!(classes("a: 1\n", "a: 1\nb: 2\n").is_empty());
    }

    #[test]
    fn bless_rewrites_only_the_scalar_and_keeps_comments() {
        let source = "# head\na: 1 # one\nb: [2, 0.30000001192092896]\n";
        let saved = "a: 1.0\nb: [2, 0.3]\n";
        let findings = compare(source, saved, &|_| Some(NumKind::F32)).unwrap();
        assert_eq!(
            blessed_text(source, &findings),
            "# head\na: 1.0 # one\nb: [2, 0.3]\n"
        );
    }
}
