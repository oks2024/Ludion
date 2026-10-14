//! The strict-subset YAML layer: saphyr-parser events in, a plain document tree out.
//!
//! This is the only module in the engine that knows YAML syntax exists. It drives
//! [`saphyr_parser::Parser`] and rebuilds the single document as a [`Node`] tree, rejecting every
//! YAML feature outside the strict subset (`engine-architecture.md` §7) with an L-coded
//! [`Diagnostic`] that carries a JSON Pointer and the line/column of the offense:
//!
//! - anchors and aliases (`L1002`),
//! - tags (`L1003`),
//! - multiple documents (`L1004`),
//! - non-empty flow collections — `[]` and `{}` stay legal (`L1005`),
//! - literal/folded block scalars (`L1006`),
//! - duplicate mapping keys (`L1007`),
//! - non-string mapping keys (`L1008`),
//! - mapping keys too long to re-emit as a simple key (`L1030`),
//! - collections nested deeper than 128 levels (`L1034`),
//! - any directive but `%YAML 1.2` (`L1037`).
//!
//! Plain scalars are resolved here per the YAML 1.2 core schema ([`crate::scalar`]); quoted
//! scalars are always strings. Scalars JSON cannot represent (`.inf`, 65-bit integers) are
//! rejected with `L1009` so nothing unrepresentable ever reaches a [`serde_json::Value`].

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use saphyr_parser::{Event, Marker, Parser, ScalarStyle, Span};
use serde_json::Value;

use crate::scalar::{Resolved, resolve_plain, scalar_string};

/// The longest mapping key the parser accepts, measured on its **canonical spelling**.
///
/// 1024 is saphyr-parser's simple-key scan limit (characters from the key's start to its `:`).
/// The check is on the canonical form, not the source form, because the writer may expand a key —
/// quoting adds two characters and each control character escapes to six (`\u00XX`) — and a key
/// that emits longer than the scanner accepts would produce a document that no longer re-parses.
const MAX_CANONICAL_KEY_CHARS: usize = 1024;

/// The deepest collection nesting the parser accepts.
///
/// Every reader of the parsed tree (`Node`'s drop, `node_to_value`, the checker, the writer) is
/// recursive, so an unbounded depth is a stack overflow on hostile input. 128 is serde_json's own
/// recursion limit and about 12× the deepest committed document; the builder's stack is a flat
/// `Vec`, so refusing here keeps every tree it hands out within the cap.
const MAX_NESTING_DEPTH: usize = 128;

/// One node of the parsed document, with its source position (1-indexed line/column).
#[derive(Debug)]
pub(crate) struct Node {
    pub value: NodeValue,
    pub line: usize,
    pub col: usize,
}

/// The three shapes a strict-subset node can take.
#[derive(Debug)]
pub(crate) enum NodeValue {
    /// A resolved scalar: `Null`, `Bool`, `Number`, or `String`.
    Scalar(Value),
    Sequence(Vec<Node>),
    /// Entries in document order; keys are guaranteed unique strings.
    Mapping(Vec<MapEntry>),
}

/// One `key: value` entry of a mapping.
#[derive(Debug)]
pub(crate) struct MapEntry {
    pub key: String,
    pub key_line: usize,
    pub node: Node,
}

impl NodeValue {
    /// A short human phrase for type-mismatch messages ("expected a mapping, found ...").
    pub fn type_phrase(&self) -> &'static str {
        match self {
            NodeValue::Scalar(value) => ludion_core::diagnostics::value_phrase(value),
            NodeValue::Sequence(_) => "a sequence",
            NodeValue::Mapping(_) => "a mapping",
        }
    }
}

pub(crate) use ludion_core::diagnostics::pointer_token;

/// The 1-based column of `marker` (saphyr counts columns from 0, lines from 1).
fn column(marker: Marker) -> usize {
    marker.col() + 1
}

/// Refuses every directive except `%YAML 1.2`.
///
/// saphyr drops the `%YAML` version and turns an unknown directive into an empty token without an
/// error, so the source text is scanned instead. Directives can only precede the first `---`, and
/// a plain scalar cannot start with `%`, so stopping at the first line that is not blank, a
/// comment or a directive never reaches content.
fn check_directives(source: &str) -> Result<(), Diagnostic> {
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !trimmed.starts_with('%') {
            break;
        }
        let body = trimmed.split(" #").next().unwrap_or(trimmed);
        let words: Vec<&str> = body.split_whitespace().collect();
        if words != ["%YAML", "1.2"] {
            return Err(Diagnostic::new(
                code::SCENE_YAML_DIRECTIVE,
                format!(
                    "the directive `{trimmed}` is not allowed in scene files (line {}, column 1)",
                    index + 1
                ),
            )
            .with_help(
                "delete the line; scene and doc files are always read as YAML 1.2 (core \
                 schema), and `%YAML 1.2` is the only directive tolerated",
            ));
        }
    }
    Ok(())
}

/// Parses `source` as exactly one strict-subset YAML document.
///
/// Returns `Ok(None)` for an empty document (empty file, comments only). Fails fast on the first
/// syntax or strictness violation — after one, the rest of the event stream is not trustworthy.
pub(crate) fn parse_document(source: &str) -> Result<Option<Node>, Diagnostic> {
    // saphyr reads a BOM as content, which turns `%YAML 1.2` + `---` into a second document.
    Builder::new(source.strip_prefix('\u{feff}').unwrap_or(source)).run()
}

/// An in-progress collection on the builder stack.
enum Frame {
    Sequence {
        items: Vec<Node>,
        line: usize,
        col: usize,
        flow: bool,
    },
    Mapping {
        entries: Vec<MapEntry>,
        pending_key: Option<(String, usize)>,
        /// Every key of `entries` → its line; keeps the duplicate check logarithmic.
        seen: BTreeMap<String, usize>,
        line: usize,
        col: usize,
        flow: bool,
    },
}

struct Builder<'a> {
    source: &'a str,
    /// The source as chars, because saphyr's `Marker::index` counts chars, not bytes.
    chars: Vec<char>,
    stack: Vec<Frame>,
    root: Option<Node>,
    document_seen: bool,
}

impl<'a> Builder<'a> {
    fn new(source: &'a str) -> Self {
        Builder {
            source,
            chars: source.chars().collect(),
            stack: Vec::new(),
            root: None,
            document_seen: false,
        }
    }

    fn run(mut self) -> Result<Option<Node>, Diagnostic> {
        check_directives(self.source)?;
        let mut parser = Parser::new_from_str(self.source);
        for step in &mut parser {
            let (event, span) = step.map_err(|err| {
                Diagnostic::new(
                    code::SCENE_YAML_SYNTAX,
                    format!(
                        "{} (line {}, column {})",
                        err.info(),
                        err.marker().line(),
                        column(*err.marker())
                    ),
                )
                .with_help(
                    "scene files are plain block-style YAML: check indentation (spaces, not \
                     tabs), a space after every `:`, and balanced quotes",
                )
            })?;
            match event {
                Event::Nothing | Event::StreamStart | Event::DocumentEnd => {}
                Event::StreamEnd => break,
                Event::DocumentStart(_) => {
                    if self.document_seen {
                        return Err(Diagnostic::new(
                            code::SCENE_YAML_MULTIPLE_DOCUMENTS,
                            format!(
                                "a second YAML document starts at line {}; a scene file is \
                                 exactly one document",
                                span.start.line()
                            ),
                        )
                        .with_help(
                            "remove the extra `---` marker, or split the content into one \
                             scene per file",
                        ));
                    }
                    self.document_seen = true;
                }
                Event::Alias(_) => {
                    return Err(self.forbidden(
                        code::SCENE_YAML_ANCHOR,
                        span,
                        "a YAML alias (`*name`) is not allowed in scene files",
                        "replace the alias with the actual value, repeated where needed",
                    ));
                }
                Event::Scalar(text, style, anchor_id, tag) => {
                    self.check_common(anchor_id, tag.as_deref(), span)?;
                    if matches!(style, ScalarStyle::Literal | ScalarStyle::Folded) {
                        return Err(self.forbidden(
                            code::SCENE_YAML_BLOCK_SCALAR,
                            span,
                            "block scalars (`|`, `>`) are not allowed in scene files",
                            "write the value as a quoted scalar; use `\\n` escapes inside \
                             double quotes for line breaks",
                        ));
                    }
                    let value = self.resolve_scalar(&text, style, span)?;
                    let node = Node {
                        value: NodeValue::Scalar(value),
                        line: span.start.line(),
                        col: column(span.start),
                    };
                    self.push_node(node, span)?;
                }
                Event::SequenceStart(anchor_id, tag) => {
                    self.check_common(anchor_id, tag.as_deref(), span)?;
                    self.check_key_position(span)?;
                    self.check_depth(span)?;
                    let flow = self.starts_with(span, '[');
                    self.stack.push(Frame::Sequence {
                        items: Vec::new(),
                        line: span.start.line(),
                        col: column(span.start),
                        flow,
                    });
                }
                Event::SequenceEnd => {
                    let Some(Frame::Sequence {
                        items, line, col, ..
                    }) = self.stack.pop()
                    else {
                        unreachable!("saphyr guarantees balanced sequence events");
                    };
                    let node = Node {
                        value: NodeValue::Sequence(items),
                        line,
                        col,
                    };
                    self.push_node(node, span)?;
                }
                Event::MappingStart(anchor_id, tag) => {
                    self.check_common(anchor_id, tag.as_deref(), span)?;
                    self.check_key_position(span)?;
                    self.check_depth(span)?;
                    let flow = self.starts_with(span, '{');
                    self.stack.push(Frame::Mapping {
                        entries: Vec::new(),
                        pending_key: None,
                        seen: BTreeMap::new(),
                        line: span.start.line(),
                        col: column(span.start),
                        flow,
                    });
                }
                Event::MappingEnd => {
                    let Some(Frame::Mapping {
                        entries, line, col, ..
                    }) = self.stack.pop()
                    else {
                        unreachable!("saphyr guarantees balanced mapping events");
                    };
                    let node = Node {
                        value: NodeValue::Mapping(entries),
                        line,
                        col,
                    };
                    self.push_node(node, span)?;
                }
            }
        }
        Ok(self.root)
    }

    /// Rejects anchors and tags, which can decorate any node kind.
    fn check_common(
        &self,
        anchor_id: usize,
        tag: Option<&saphyr_parser::Tag>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        if anchor_id != 0 {
            return Err(self.forbidden(
                code::SCENE_YAML_ANCHOR,
                span,
                "a YAML anchor (`&name`) is not allowed in scene files",
                "remove the anchor and repeat the value wherever an alias would have used it",
            ));
        }
        if let Some(tag) = tag {
            return Err(self.forbidden(
                code::SCENE_YAML_TAG,
                span,
                &format!("the YAML tag `{tag}` is not allowed in scene files"),
                "remove the tag; to force a scalar to be a string, quote it instead",
            ));
        }
        Ok(())
    }

    /// A collection is starting; if we are sitting at a mapping's key position, that is a
    /// complex key — forbidden.
    fn check_key_position(&self, span: Span) -> Result<(), Diagnostic> {
        if let Some(Frame::Mapping {
            pending_key: None, ..
        }) = self.stack.last()
        {
            return Err(self.forbidden(
                code::SCENE_YAML_COMPLEX_KEY,
                span,
                "a sequence or mapping cannot be used as a mapping key in scene files",
                "use a plain string key",
            ));
        }
        Ok(())
    }

    /// A collection is starting; refuse it if it would nest deeper than [`MAX_NESTING_DEPTH`].
    fn check_depth(&self, span: Span) -> Result<(), Diagnostic> {
        if self.stack.len() >= MAX_NESTING_DEPTH {
            return Err(self.forbidden(
                code::SCENE_YAML_TOO_DEEP,
                span,
                &format!("collections are nested more than {MAX_NESTING_DEPTH} levels deep"),
                "flatten the structure; a document this deep is usually corrupt or generated by mistake",
            ));
        }
        Ok(())
    }

    /// Whether the collection that starts at `span` opens with the flow indicator `c`.
    ///
    /// Sound because a block collection's start marker points at its first token (`-`, a key, or
    /// a quote), and no plain scalar or key may begin with `[` or `{`.
    fn starts_with(&self, span: Span, c: char) -> bool {
        self.chars.get(span.start.index()) == Some(&c)
    }

    fn resolve_scalar(
        &self,
        text: &str,
        style: ScalarStyle,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        if matches!(style, ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted) {
            return Ok(Value::String(text.to_string()));
        }
        match resolve_plain(text) {
            Resolved::Value(value) => Ok(value),
            Resolved::Str => Ok(Value::String(text.to_string())),
            Resolved::Unrepresentable(reason) => Err(Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "{reason} (line {}, column {})",
                    span.start.line(),
                    column(span.start)
                ),
            )
            .with_path(self.pointer())
            .with_help("if the text was meant as a string, quote it")),
        }
    }

    /// Attaches a completed node to the enclosing frame (or makes it the document root).
    fn push_node(&mut self, node: Node, span: Span) -> Result<(), Diagnostic> {
        // Any push into a flow collection makes it non-empty, which the strict subset forbids.
        // (A push into a flow mapping is always at the key position: the key push errors before
        // a value push can ever happen.)
        let flow_violation = match self.stack.last() {
            Some(Frame::Sequence { flow: true, .. }) => Some(("sequence", "[1, 2]")),
            Some(Frame::Mapping { flow: true, .. }) => Some(("mapping", "{x: 1}")),
            _ => None,
        };
        if let Some((kind, example)) = flow_violation {
            return Err(self.non_empty_flow(span, kind, example));
        }
        let base_pointer = self.pointer();
        match self.stack.last_mut() {
            None => {
                self.root = Some(node);
                Ok(())
            }
            Some(Frame::Sequence { items, .. }) => {
                items.push(node);
                Ok(())
            }
            Some(Frame::Mapping {
                entries,
                pending_key,
                seen,
                ..
            }) => match pending_key.take() {
                None => {
                    // `node` is this entry's key.
                    let NodeValue::Scalar(Value::String(key)) = node.value else {
                        return Err(Diagnostic::new(
                            code::SCENE_YAML_COMPLEX_KEY,
                            format!(
                                "mapping keys must be strings, found {} (line {}, column {})",
                                node.value.type_phrase(),
                                node.line,
                                node.col
                            ),
                        )
                        .with_path(base_pointer)
                        .with_help("quote the key to make it a string"));
                    };
                    if let Some(first_line) = seen.get(&key) {
                        return Err(Diagnostic::new(
                            code::SCENE_YAML_DUPLICATE_KEY,
                            format!(
                                "the key `{key}` appears twice in the same mapping (first at \
                                 line {}, again at line {})",
                                first_line, node.line
                            ),
                        )
                        .with_path(format!("{base_pointer}/{}", pointer_token(&key)))
                        .with_help(
                            "keep exactly one of the entries; if both were intended, one of \
                             them is misnamed",
                        ));
                    }
                    let canonical_len = scalar_string(&key).chars().count();
                    if canonical_len > MAX_CANONICAL_KEY_CHARS {
                        let prefix: String = key.chars().take(24).collect();
                        return Err(Diagnostic::new(
                            code::SCENE_YAML_KEY_TOO_LONG,
                            format!(
                                "the key starting {} is {canonical_len} characters once written in \
                                 canonical form, over the {MAX_CANONICAL_KEY_CHARS}-character \
                                 limit for a mapping key (line {})",
                                scalar_string(&prefix),
                                node.line
                            ),
                        )
                        .with_path(format!("{base_pointer}/{}", pointer_token(&key)))
                        .with_help(
                            "shorten the key; keys name things (fields, components) and are not \
                             a place to carry payload text",
                        ));
                    }
                    seen.insert(key.clone(), node.line);
                    *pending_key = Some((key, node.line));
                    Ok(())
                }
                Some((key, key_line)) => {
                    entries.push(MapEntry {
                        key,
                        key_line,
                        node,
                    });
                    Ok(())
                }
            },
        }
    }

    fn non_empty_flow(&self, span: Span, kind: &str, example: &str) -> Diagnostic {
        Diagnostic::new(
            code::SCENE_YAML_FLOW_COLLECTION,
            format!(
                "a non-empty flow {kind} (like `{example}`) is not allowed in scene files \
                 (line {}, column {})",
                span.start.line(),
                column(span.start)
            ),
        )
        .with_path(self.pointer())
        .with_help(
            "write the collection in block style, one element per line; only the empty `[]` \
             and `{}` may stay in flow style",
        )
    }

    /// The JSON Pointer of the position the builder is currently filling.
    ///
    /// Walks the open frames: a sequence contributes its next index, a mapping contributes its
    /// pending key (or nothing, when the cursor sits at a key position).
    fn pointer(&self) -> String {
        let mut pointer = String::new();
        for frame in &self.stack {
            match frame {
                Frame::Sequence { items, .. } => {
                    pointer.push('/');
                    pointer.push_str(&items.len().to_string());
                }
                Frame::Mapping {
                    pending_key: Some((key, _)),
                    ..
                } => {
                    pointer.push('/');
                    pointer.push_str(&pointer_token(key));
                }
                Frame::Mapping {
                    pending_key: None, ..
                } => {}
            }
        }
        pointer
    }

    fn forbidden(&self, code: DiagnosticCode, span: Span, message: &str, help: &str) -> Diagnostic {
        let mut diagnostic = Diagnostic::new(
            code,
            format!(
                "{message} (line {}, column {})",
                span.start.line(),
                column(span.start)
            ),
        )
        .with_help(help);
        let pointer = self.pointer();
        if !pointer.is_empty() {
            diagnostic = diagnostic.with_path(pointer);
        }
        diagnostic
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_err(source: &str) -> Diagnostic {
        parse_document(source).expect_err("expected a diagnostic")
    }

    fn parse_ok(source: &str) -> Node {
        parse_document(source)
            .expect("expected a clean parse")
            .expect("expected a non-empty document")
    }

    #[test]
    fn parses_a_small_tree() {
        let root = parse_ok("a: 1\nb:\n  - x\n  - 2.5\nc: {}\n");
        let NodeValue::Mapping(entries) = root.value else {
            panic!("root should be a mapping")
        };
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].key, "a");
        assert!(matches!(&entries[0].node.value, NodeValue::Scalar(v) if v == &Value::from(1)));
        let NodeValue::Sequence(items) = &entries[1].node.value else {
            panic!("b should be a sequence")
        };
        assert_eq!(items.len(), 2);
        assert!(matches!(&entries[2].node.value, NodeValue::Mapping(m) if m.is_empty()));
    }

    #[test]
    fn empty_documents_are_none() {
        assert!(parse_document("").unwrap().is_none());
        assert!(parse_document("# just a comment\n").unwrap().is_none());
    }

    #[test]
    fn syntax_error_is_l1001() {
        let d = parse_err("a: \"unclosed\n");
        assert_eq!(d.code, code::SCENE_YAML_SYNTAX);
        assert!(d.message.contains("line"), "message was: {}", d.message);
    }

    #[test]
    fn anchor_is_l1002() {
        // Any alias needs an anchor first, and the anchor itself is rejected — so aliases can
        // never be reached with a well-formed anchor in place.
        let d = parse_err("a: &anchor 1\nb: *anchor\n");
        assert_eq!(d.code, code::SCENE_YAML_ANCHOR);
        let d = parse_err("list: &a\n  - 1\n");
        assert_eq!(d.code, code::SCENE_YAML_ANCHOR);
        // An alias with no anchor is a saphyr-level error (unknown anchor) → syntax diagnostic.
        let d = parse_err("a: 1\nb: *ghost\n");
        assert_eq!(d.code, code::SCENE_YAML_SYNTAX);
    }

    #[test]
    fn tag_is_l1003() {
        let d = parse_err("a: !!str 42\n");
        assert_eq!(d.code, code::SCENE_YAML_TAG);
        assert_eq!(d.path.as_deref(), Some("/a"));
    }

    #[test]
    fn multiple_documents_are_l1004() {
        let d = parse_err("a: 1\n---\nb: 2\n");
        assert_eq!(d.code, code::SCENE_YAML_MULTIPLE_DOCUMENTS);
    }

    #[test]
    fn non_empty_flow_is_l1005_but_empty_flow_is_fine() {
        let d = parse_err("a: [1, 2]\n");
        assert_eq!(d.code, code::SCENE_YAML_FLOW_COLLECTION);
        assert_eq!(d.path.as_deref(), Some("/a/0"));
        let d = parse_err("a: {x: 1}\n");
        assert_eq!(d.code, code::SCENE_YAML_FLOW_COLLECTION);
        parse_ok("a: []\nb: {}\n");
    }

    #[test]
    fn block_scalar_is_l1006() {
        let d = parse_err("a: |\n  text\n");
        assert_eq!(d.code, code::SCENE_YAML_BLOCK_SCALAR);
        let d = parse_err("a: >\n  text\n");
        assert_eq!(d.code, code::SCENE_YAML_BLOCK_SCALAR);
    }

    #[test]
    fn nesting_past_the_cap_is_l1034() {
        let source = "- ".repeat(10_000) + "x";
        let d = parse_err(&source);
        assert_eq!(d.code, code::SCENE_YAML_TOO_DEEP);
        assert!(d.message.contains("line 1"), "message was: {}", d.message);
    }

    #[test]
    fn nesting_at_the_cap_parses() {
        let source = "- ".repeat(MAX_NESTING_DEPTH) + "x";
        let root = parse_ok(&source);
        let value = crate::parse::node_to_value(&root);
        assert!(value.is_array());
        drop(root);
        let d = parse_err(&("- ".repeat(MAX_NESTING_DEPTH + 1) + "x"));
        assert_eq!(d.code, code::SCENE_YAML_TOO_DEEP);
    }

    #[test]
    fn mapping_nesting_past_the_cap_is_l1034() {
        let source: String = (0..200)
            .map(|level| format!("{}k:\n", "  ".repeat(level)))
            .collect();
        let d = parse_err(&source);
        assert_eq!(d.code, code::SCENE_YAML_TOO_DEEP);
    }

    #[test]
    fn duplicate_key_is_l1007() {
        let d = parse_err("a: 1\na: 2\n");
        assert_eq!(d.code, code::SCENE_YAML_DUPLICATE_KEY);
        assert_eq!(d.path.as_deref(), Some("/a"));
        assert!(d.message.contains("line 1"), "message was: {}", d.message);
    }

    #[test]
    fn complex_or_non_string_key_is_l1008() {
        let d = parse_err("? [1, 2]\n: x\n");
        assert_eq!(d.code, code::SCENE_YAML_COMPLEX_KEY);
        let d = parse_err("1: x\n");
        assert_eq!(d.code, code::SCENE_YAML_COMPLEX_KEY);
        assert!(
            d.message.contains("an integer"),
            "message was: {}",
            d.message
        );
    }

    #[test]
    fn unrepresentable_scalars_are_l1009_with_pointer() {
        let d = parse_err("a:\n  b: .inf\n");
        assert_eq!(d.code, code::SCENE_TYPE_MISMATCH);
        assert_eq!(d.path.as_deref(), Some("/a/b"));
        let d = parse_err("a: 99999999999999999999\n");
        assert_eq!(d.code, code::SCENE_TYPE_MISMATCH);
    }

    #[test]
    fn quoted_scalars_stay_strings() {
        let root = parse_ok("a: \"42\"\nb: 'true'\nc: no\n");
        let NodeValue::Mapping(entries) = root.value else {
            panic!()
        };
        for entry in &entries {
            assert!(
                matches!(&entry.node.value, NodeValue::Scalar(Value::String(_))),
                "{} should be a string",
                entry.key
            );
        }
    }

    #[test]
    fn pointer_walks_nesting() {
        let d = parse_err("top:\n  - inner:\n      deep: [1]\n");
        assert_eq!(d.path.as_deref(), Some("/top/0/inner/deep/0"));
    }

    /// 1-based column of the first `needle` on source line `line` (1-based); ASCII sources only.
    fn column_of(source: &str, line: usize, needle: &str) -> usize {
        source.lines().nth(line - 1).unwrap().find(needle).unwrap() + 1
    }

    #[test]
    fn columns_are_one_based() {
        let source = "entities: [1]\n";
        let d = parse_err(source);
        assert_eq!(d.code, code::SCENE_YAML_FLOW_COLLECTION);
        let want = format!("column {})", column_of(source, 1, "1]"));
        assert!(d.message.contains(&want), "message was: {}", d.message);
        assert!(
            d.message.contains("column 12)"),
            "message was: {}",
            d.message
        );

        let source = "a:\n  - name: &x a\n";
        let d = parse_err(source);
        assert_eq!(d.code, code::SCENE_YAML_ANCHOR);
        // saphyr marks the anchored scalar itself, not its `&x` decoration.
        let want = format!(
            "(line 2, column {})",
            source.lines().nth(1).unwrap().rfind('a').unwrap() + 1
        );
        assert!(d.message.contains(&want), "message was: {}", d.message);

        let d = parse_err("1: x\n");
        assert_eq!(d.code, code::SCENE_YAML_COMPLEX_KEY);
        assert!(
            d.message.contains("column 1)"),
            "message was: {}",
            d.message
        );
    }

    #[test]
    fn duplicate_key_in_a_large_mapping_names_both_lines() {
        let mut source = String::new();
        for i in 0..20_000 {
            source.push_str(&format!("k{i}: 1\n"));
        }
        source.push_str("k0: 2\n");
        let d = parse_err(&source);
        assert_eq!(d.code, code::SCENE_YAML_DUPLICATE_KEY);
        assert!(
            d.message.contains("first at line 1, again at line 20001"),
            "message was: {}",
            d.message
        );
    }

    #[test]
    fn foreign_directives_are_l1037() {
        for source in [
            "%YAML 1.1\n---\nname: yes\n",
            "%TAG ! tag:x.org,2000:\n---\na: 1\n",
            "%FOO bar\n---\na: 1\n",
            "\u{feff}%YAML 1.1\n---\na: 1\n",
        ] {
            let d = parse_err(source);
            assert_eq!(d.code, code::SCENE_YAML_DIRECTIVE, "source: {source:?}");
            assert!(
                d.message.contains("line 1, column 1"),
                "message was: {}",
                d.message
            );
        }
    }

    #[test]
    fn a_leading_bom_is_ignored() {
        for source in ["\u{feff}%YAML 1.2\n---\na: 1\n", "\u{feff}a: 1\n"] {
            let NodeValue::Mapping(entries) = parse_ok(source).value else {
                panic!("root should be a mapping, source: {source:?}")
            };
            assert_eq!(entries.len(), 1, "source: {source:?}");
            assert_eq!(entries[0].key, "a", "source: {source:?}");
            assert!(
                matches!(&entries[0].node.value, NodeValue::Scalar(v) if v == &Value::from(1)),
                "source: {source:?}"
            );
        }
    }

    #[test]
    fn yaml_1_2_directive_and_quoted_percent_parse() {
        parse_ok("%YAML 1.2\n---\na: 1\n");
        parse_ok("# c\n\n%YAML 1.2 # ok\n---\na: 1\n");
        parse_ok("a: \"%YAML 1.1\"\n");
    }
}
