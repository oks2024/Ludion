//! Patrol documents: `kind: patrol` — an ordered list of stops a guard walks, each with an optional
//! wait, facing, look-around sequence and tag (`Plan/nav.md` decision 13).
//!
//! ```yaml
//! format: 1
//! kind: patrol
//! name: yard
//! mode: loop
//! points:
//!   - at:
//!       - 0.0
//!       - 0.0
//!       - 0.0
//!     wait: 1.5
//!     look:
//!       - yaw_deg: -45.0
//!         hold: 0.5
//!       - yaw_deg: 45.0
//!         hold: 0.5
//!     tag: door
//! ```
//!
//! `mode` is `loop` (the default), `ping_pong` or `once`; the canonical writer always spells it. A
//! stop lasts `max(wait, Σ look.hold)`. `face_deg` is the world yaw to face while stopped; without it
//! the guard keeps the heading of the leg it just walked. A one-point `loop` is a post.
//!
//! **This module stays IO-free** and carries no walking logic: the engine's `patrol_update` reads the
//! validated [`PatrolDocument`]. Structure (`L1522`) and data consistency (`L1523`) split as in the
//! path family.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::fits_f32;
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::fields::parse_number;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, Node, NodeValue};

/// The patrol document format version this crate reads and writes.
pub const PATROL_FORMAT_VERSION: u64 = 1;

const PATROL_KEYS: [&str; 6] = ["format", "kind", "name", "generated_from", "mode", "points"];
const POINT_KEYS: [&str; 5] = ["at", "wait", "face_deg", "look", "tag"];
const LOOK_KEYS: [&str; 2] = ["yaw_deg", "hold"];
const MODES: [&str; 3] = ["loop", "ping_pong", "once"];

/// How a patrol runs through its points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PatrolMode {
    /// After the last point, back to the first.
    #[default]
    Loop,
    /// Out to the last point, then back, forever.
    PingPong,
    /// Through the points once, then done.
    Once,
}

impl PatrolMode {
    fn keyword(self) -> &'static str {
        match self {
            Self::Loop => "loop",
            Self::PingPong => "ping_pong",
            Self::Once => "once",
        }
    }
}

/// One look-around entry: face `yaw_deg` (relative to the stop's base facing) for `hold` seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatrolLook {
    /// The yaw offset from the stop's base facing, in degrees.
    pub yaw_deg: f64,
    /// How long the look lasts, in gameplay seconds; positive.
    pub hold: f64,
}

/// One stop on a patrol.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PatrolPoint {
    /// The world position to walk to.
    pub at: [f64; 3],
    /// The minimum time spent here, in gameplay seconds.
    pub wait: f64,
    /// The world yaw, in degrees, to face while stopped; `None` keeps the heading of the leg walked.
    pub face_deg: Option<f64>,
    /// The look-around sequence played from the start of the stop.
    pub look: Vec<PatrolLook>,
    /// A free label the game reads (`patrol_motion`), such as `door`.
    pub tag: Option<String>,
}

/// A parsed, structurally valid patrol document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PatrolDocument {
    /// Optional human-readable patrol name.
    pub name: Option<String>,
    /// The content-root-relative source that emitted this patrol, as provenance only.
    pub generated_from: Option<String>,
    /// How the patrol runs through its points.
    pub mode: PatrolMode,
    /// The stops, in order.
    pub points: Vec<PatrolPoint>,
}

// --- Parsing (`L1522`) --------------------------------------------------------------------------

/// Parses strict-subset YAML into a [`PatrolDocument`], checking structure/shape only (`L1522`).
pub(crate) fn parse(source: &str) -> Result<PatrolDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::PATROL_DOCUMENT_STRUCTURE,
                "the patrol document is empty; a patrol needs at least `format: 1`, `kind: patrol` \
                 and `points`",
            )
            .with_path("")
            .with_help(
                "a minimal patrol is:\n  format: 1\n  kind: patrol\n  points:\n    - at:\n        - 0.0\n        - 0.0\n        - 0.0",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::PATROL_DOCUMENT_STRUCTURE,
                format!(
                    "the patrol document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: patrol`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = PatrolDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut points_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::PATROL_DOCUMENT_STRUCTURE,
                    code::PATROL_DOCUMENT_STRUCTURE,
                    PATROL_FORMAT_VERSION,
                    "the patrol document",
                    "patrol",
                    "patrol",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::PATROL_DOCUMENT_STRUCTURE,
                    "patrol",
                    "a patrol document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the patrol name",
                    code::PATROL_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "generated_from" => {
                doc.generated_from = envelope::expect_string(
                    &entry.node,
                    "/generated_from",
                    "`generated_from`",
                    code::PATROL_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "mode" => {
                if let Some(mode) = parse_mode(&entry.node, "/mode", &mut diagnostics) {
                    doc.mode = mode;
                }
            }
            "points" => {
                points_seen = true;
                if let Some(points) = parse_points(&entry.node, "/points", &mut diagnostics) {
                    doc.points = points;
                }
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a patrol document",
                PATROL_KEYS,
                code::PATROL_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key("format", "add `format: 1` as the first line"));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: patrol`"));
    }
    if !points_seen {
        diagnostics.push(missing_key(
            "points",
            "add `points`: a list of stops, each with an `at` of three numbers",
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
        code::PATROL_DOCUMENT_STRUCTURE,
        "patrol document",
    )
}

fn parse_mode(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<PatrolMode> {
    let listed = format!("`mode` is one of: {}", MODES.join(", "));
    match &node.value {
        NodeValue::Scalar(Value::String(s)) => {
            match MODES.iter().position(|choice| *choice == s.as_str()) {
                Some(0) => return Some(PatrolMode::Loop),
                Some(1) => return Some(PatrolMode::PingPong),
                Some(_) => return Some(PatrolMode::Once),
                None => {}
            }
            let help = match suggest::nearest(s, MODES.iter().copied()) {
                Some(candidate) => format!("nearest match is `{candidate}`"),
                None => listed,
            };
            diagnostics.push(
                Diagnostic::new(
                    code::PATROL_DOCUMENT_STRUCTURE,
                    format!("unknown mode `{s}` (line {})", node.line),
                )
                .with_path(pointer.to_string())
                .with_help(help),
            );
            None
        }
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::PATROL_DOCUMENT_STRUCTURE,
                    format!(
                        "`mode` must be a string, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(listed),
            );
            None
        }
    }
}

fn parse_points(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<PatrolPoint>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::PATROL_DOCUMENT_STRUCTURE,
                format!(
                    "`points` must be a sequence, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write `points` as a list, one `- at:` entry per stop"),
        );
        return None;
    };
    let mut points = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match parse_point(item, &format!("{pointer}/{index}"), diagnostics) {
            Some(point) => points.push(point),
            None => ok = false,
        }
    }
    ok.then_some(points)
}

fn parse_point(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<PatrolPoint> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::PATROL_DOCUMENT_STRUCTURE,
                format!(
                    "a patrol point must be a mapping, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write each stop as `- at:` followed by three numbers"),
        );
        return None;
    };
    let mut point = PatrolPoint::default();
    let mut at_seen = false;
    let mut ok = true;
    for entry in entries {
        let child = format!("{pointer}/{}", entry.key);
        match entry.key.as_str() {
            "at" => {
                at_seen = true;
                match parse_vec3(&entry.node, &child, diagnostics) {
                    Some(at) => point.at = at,
                    None => ok = false,
                }
            }
            "wait" => match parse_number(
                &entry.node,
                &child,
                "`wait`",
                code::PATROL_DOCUMENT_STRUCTURE,
                diagnostics,
            ) {
                Some(wait) => point.wait = wait,
                None => ok = false,
            },
            "face_deg" => match parse_number(
                &entry.node,
                &child,
                "`face_deg`",
                code::PATROL_DOCUMENT_STRUCTURE,
                diagnostics,
            ) {
                Some(face) => point.face_deg = Some(face),
                None => ok = false,
            },
            "look" => match parse_looks(&entry.node, &child, diagnostics) {
                Some(look) => point.look = look,
                None => ok = false,
            },
            "tag" => {
                match envelope::expect_string(
                    &entry.node,
                    &child,
                    "`tag`",
                    code::PATROL_DOCUMENT_STRUCTURE,
                    diagnostics,
                ) {
                    Some(tag) => point.tag = Some(tag),
                    None => ok = false,
                }
            }
            unknown => {
                ok = false;
                diagnostics.push(envelope::unknown_key(
                    unknown,
                    entry,
                    pointer,
                    "a patrol point",
                    POINT_KEYS,
                    code::PATROL_DOCUMENT_STRUCTURE,
                ));
            }
        }
    }
    if !at_seen {
        ok = false;
        diagnostics.push(
            Diagnostic::new(
                code::PATROL_DOCUMENT_STRUCTURE,
                format!(
                    "a patrol point is missing the required key `at` (line {})",
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("add `at:` with three numbers, one per line"),
        );
    }
    ok.then_some(point)
}

fn parse_vec3(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[f64; 3]> {
    let help = "write `at` as three numbers, one per line";
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::PATROL_DOCUMENT_STRUCTURE,
                format!(
                    "`at` must be a sequence of three numbers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(help),
        );
        return None;
    };
    if items.len() != 3 {
        diagnostics.push(
            Diagnostic::new(
                code::PATROL_DOCUMENT_STRUCTURE,
                format!(
                    "`at` must have exactly 3 components, found {} (line {})",
                    items.len(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(help),
        );
        return None;
    }
    let mut out = [0.0; 3];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match parse_number(
            item,
            &format!("{pointer}/{index}"),
            "`at`",
            code::PATROL_DOCUMENT_STRUCTURE,
            diagnostics,
        ) {
            Some(value) => out[index] = value,
            None => ok = false,
        }
    }
    ok.then_some(out)
}

fn parse_looks(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<PatrolLook>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::PATROL_DOCUMENT_STRUCTURE,
                format!(
                    "`look` must be a sequence, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write `look` as a list of `- yaw_deg:` / `hold:` entries"),
        );
        return None;
    };
    let mut looks = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match parse_look(item, &format!("{pointer}/{index}"), diagnostics) {
            Some(look) => looks.push(look),
            None => ok = false,
        }
    }
    ok.then_some(looks)
}

fn parse_look(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<PatrolLook> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::PATROL_DOCUMENT_STRUCTURE,
                format!(
                    "a look entry must be a mapping, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write each look as `- yaw_deg: 45.0` with `hold: 0.5` beneath it"),
        );
        return None;
    };
    let (mut yaw_deg, mut hold) = (None, None);
    let mut ok = true;
    for entry in entries {
        let child = format!("{pointer}/{}", entry.key);
        let slot = match entry.key.as_str() {
            "yaw_deg" => &mut yaw_deg,
            "hold" => &mut hold,
            unknown => {
                ok = false;
                diagnostics.push(envelope::unknown_key(
                    unknown,
                    entry,
                    pointer,
                    "a look entry",
                    LOOK_KEYS,
                    code::PATROL_DOCUMENT_STRUCTURE,
                ));
                continue;
            }
        };
        match parse_number(
            &entry.node,
            &child,
            &format!("`{}`", entry.key),
            code::PATROL_DOCUMENT_STRUCTURE,
            diagnostics,
        ) {
            Some(value) => *slot = Some(value),
            None => ok = false,
        }
    }
    for (key, value) in [("yaw_deg", yaw_deg), ("hold", hold)] {
        if value.is_none() && ok {
            ok = false;
            diagnostics.push(missing_look_key(key, node, pointer));
        }
    }
    match (ok, yaw_deg, hold) {
        (true, Some(yaw_deg), Some(hold)) => Some(PatrolLook { yaw_deg, hold }),
        _ => None,
    }
}

fn missing_look_key(key: &str, node: &Node, pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::PATROL_DOCUMENT_STRUCTURE,
        format!(
            "a look entry is missing the required key `{key}` (line {})",
            node.line
        ),
    )
    .with_path(pointer.to_string())
    .with_help("a look needs both `yaw_deg` and `hold`")
}

// --- Validation (`L1523`) -----------------------------------------------------------------------

/// Validates data-consistency invariants of an already-parsed [`PatrolDocument`] (`L1523`): at least
/// one point, every number finite and inside the f32 range, `wait` and `hold` non-negative, a `look`
/// `hold` above zero, and a non-empty `tag` when present.
pub(crate) fn validate(doc: &PatrolDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if doc.points.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::PATROL_DATA_CONSISTENCY,
                "a patrol needs at least 1 point, found 0",
            )
            .with_path("/points")
            .with_help("add a stop to `points`; a single point is a post"),
        );
    }
    for (index, point) in doc.points.iter().enumerate() {
        let base = format!("/points/{index}");
        for (component, value) in point.at.iter().enumerate() {
            number_problem(
                *value,
                &format!("{base}/at/{component}"),
                "a point component",
                &mut diagnostics,
            );
        }
        if number_problem(
            point.wait,
            &format!("{base}/wait"),
            "`wait`",
            &mut diagnostics,
        ) && point.wait < 0.0
        {
            diagnostics.push(negative(&format!("{base}/wait"), "`wait`", point.wait));
        }
        if let Some(face) = point.face_deg {
            number_problem(
                face,
                &format!("{base}/face_deg"),
                "`face_deg`",
                &mut diagnostics,
            );
        }
        if point.tag.as_deref() == Some("") {
            diagnostics.push(
                Diagnostic::new(
                    code::PATROL_DATA_CONSISTENCY,
                    format!("point {index} has an empty `tag`"),
                )
                .with_path(format!("{base}/tag"))
                .with_help("name the tag, or delete the key"),
            );
        }
        for (look_index, look) in point.look.iter().enumerate() {
            let look_base = format!("{base}/look/{look_index}");
            number_problem(
                look.yaw_deg,
                &format!("{look_base}/yaw_deg"),
                "`yaw_deg`",
                &mut diagnostics,
            );
            if number_problem(
                look.hold,
                &format!("{look_base}/hold"),
                "`hold`",
                &mut diagnostics,
            ) && look.hold <= 0.0
            {
                diagnostics.push(
                    Diagnostic::new(
                        code::PATROL_DATA_CONSISTENCY,
                        format!(
                            "`hold` is {}, but a look has to last longer than 0 seconds",
                            look.hold
                        ),
                    )
                    .with_path(format!("{look_base}/hold"))
                    .with_help("give the look a positive `hold`, or delete the entry"),
                );
            }
        }
    }
    diagnostics
}

/// Reports a non-finite or out-of-f32-range `value`; returns whether it was fine.
fn number_problem(
    value: f64,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    if value.is_finite() && fits_f32(value) {
        return true;
    }
    diagnostics.push(
        Diagnostic::new(
            code::PATROL_DATA_CONSISTENCY,
            format!("{what} is {value}, which is not a finite f32-range number"),
        )
        .with_path(pointer.to_string())
        .with_help("keep every number finite and below 3.4e38 in magnitude"),
    );
    false
}

fn negative(pointer: &str, what: &str, value: f64) -> Diagnostic {
    Diagnostic::new(
        code::PATROL_DATA_CONSISTENCY,
        format!("{what} is {value}, but a duration cannot be negative"),
    )
    .with_path(pointer.to_string())
    .with_help("use 0 or a positive number of seconds")
}

// --- Canonical writer ---------------------------------------------------------------------------

/// Writes `doc` in canonical form: `format`, `kind`, `name` and `generated_from` (both optional),
/// `mode` (always spelled), `points`; a point's `wait` (at 0), `face_deg`, `look` (empty) and `tag`
/// are omitted when absent.
pub(crate) fn emit(doc: &PatrolDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: patrol\n");
    if let Some(name) = &doc.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }
    if let Some(source) = &doc.generated_from {
        out.push_str("generated_from: ");
        out.push_str(&scalar_string(source));
        out.push('\n');
    }
    out.push_str("mode: ");
    out.push_str(doc.mode.keyword());
    out.push('\n');
    out.push_str("points:\n");
    for point in &doc.points {
        out.push_str("  - at:\n");
        for value in point.at {
            out.push_str(&format!("      - {}\n", format_float(value)));
        }
        if point.wait != 0.0 {
            out.push_str(&format!("    wait: {}\n", format_float(point.wait)));
        }
        if let Some(face) = point.face_deg {
            out.push_str(&format!("    face_deg: {}\n", format_float(face)));
        }
        if !point.look.is_empty() {
            out.push_str("    look:\n");
            for look in &point.look {
                out.push_str(&format!(
                    "      - yaw_deg: {}\n        hold: {}\n",
                    format_float(look.yaw_deg),
                    format_float(look.hold)
                ));
            }
        }
        if let Some(tag) = &point.tag {
            out.push_str("    tag: ");
            out.push_str(&scalar_string(tag));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str =
        "format: 1\nkind: patrol\npoints:\n  - at:\n      - 1.0\n      - 0.0\n      - 2.0\n";

    const FULL: &str = "\
format: 1
kind: patrol
name: yard
generated_from: gen/yard.py
mode: ping_pong
points:
  - at:
      - 0.0
      - 0.0
      - 0.0
    wait: 1.5
    face_deg: 90.0
    look:
      - yaw_deg: -45.0
        hold: 0.5
      - yaw_deg: 45.0
        hold: 0.25
    tag: door
  - at:
      - 4.0
      - 0.0
      - -3.5
";

    fn codes_and_paths(source: &str) -> Vec<(String, String)> {
        let errors = match parse(source) {
            Err(errors) => errors,
            Ok(doc) => validate(&doc),
        };
        errors
            .iter()
            .map(|d| (d.code.to_string(), d.path.clone().unwrap_or_default()))
            .collect()
    }

    #[test]
    fn a_minimal_document_parses_with_defaults() {
        let doc = parse(MINIMAL).expect("parses");
        assert_eq!(doc.mode, PatrolMode::Loop);
        assert_eq!(doc.points.len(), 1);
        assert_eq!(doc.points[0].at, [1.0, 0.0, 2.0]);
        assert_eq!(doc.points[0].wait, 0.0);
        assert!(validate(&doc).is_empty());
    }

    #[test]
    fn emit_parse_round_trips_and_is_idempotent() {
        let doc = parse(FULL).expect("parses");
        assert_eq!(doc.points[0].look.len(), 2);
        assert_eq!(doc.points[0].tag.as_deref(), Some("door"));
        assert!(validate(&doc).is_empty());
        let emitted = emit(&doc);
        assert_eq!(emitted, FULL, "FULL is already canonical");
        assert_eq!(parse(&emitted).expect("re-parses"), doc);
        assert_eq!(emit(&parse(&emitted).unwrap()), emitted);
    }

    #[test]
    fn emit_spells_the_mode_and_omits_defaults() {
        let doc = parse(MINIMAL).expect("parses");
        assert_eq!(
            emit(&doc),
            "format: 1\nkind: patrol\nmode: loop\npoints:\n  - at:\n      - 1.0\n      - 0.0\n      - 2.0\n"
        );
    }

    #[test]
    fn a_one_point_post_with_looks_is_valid() {
        let source = "format: 1\nkind: patrol\nmode: loop\npoints:\n  - at:\n      - 0.0\n      - 0.0\n      - 0.0\n    look:\n      - yaw_deg: 30.0\n        hold: 1.0\n";
        let doc = parse(source).expect("parses");
        assert!(validate(&doc).is_empty());
        assert_eq!(emit(&doc), source);
    }

    #[test]
    fn structure_errors_carry_pointers() {
        let at = |body: &str| {
            format!(
                "format: 1\nkind: patrol\npoints:\n  - at:\n      - 0.0\n      - 0.0\n      - 0.0\n{body}"
            )
        };
        let cases = [
            (at("    wiat: 1.0\n"), "/points/0/wiat"),
            (at("    look:\n      - yaw_deg: 1.0\n        hold: 1.0\n        extra: 1\n"), "/points/0/look/0/extra"),
            (at("    look:\n      - yaw_deg: 1.0\n"), "/points/0/look/0"),
            (at("    wait: soon\n"), "/points/0/wait"),
            (at("    tag: 3\n"), "/points/0/tag"),
            (
                "format: 1\nkind: patrol\nmode: bounce\npoints:\n  - at:\n      - 0.0\n      - 0.0\n      - 0.0\n".to_string(),
                "/mode",
            ),
            (
                "format: 1\nkind: patrol\npoints:\n  - wait: 1.0\n".to_string(),
                "/points/0",
            ),
            (
                "format: 1\nkind: patrol\npoints:\n  - at:\n      - 0.0\n      - 0.0\n".to_string(),
                "/points/0/at",
            ),
            ("format: 1\nkind: patrol\nroute: x\npoints: []\n".to_string(), "/route"),
        ];
        for (source, pointer) in cases {
            let found = codes_and_paths(&source);
            assert!(
                found.contains(&("L1522".to_string(), pointer.to_string())),
                "{pointer}: {found:?}"
            );
        }
        assert!(
            codes_and_paths("format: 1\nkind: path\npoints: []\n")
                .iter()
                .all(|(code, _)| code == "L1522")
        );
    }

    #[test]
    fn data_errors_carry_pointers() {
        let doc = |body: &str| {
            format!(
                "format: 1\nkind: patrol\npoints:\n  - at:\n      - 0.0\n      - 0.0\n      - 0.0\n{body}"
            )
        };
        let cases = [
            (doc("    wait: -1.0\n"), "/points/0/wait"),
            (doc("    tag: \"\"\n"), "/points/0/tag"),
            (doc("    look:\n      - yaw_deg: 1.0\n        hold: 0.0\n"), "/points/0/look/0/hold"),
            (doc("    look:\n      - yaw_deg: 1.0\n        hold: -2.0\n"), "/points/0/look/0/hold"),
            (doc("    face_deg: 1.0e300\n"), "/points/0/face_deg"),
            (
                "format: 1\nkind: patrol\npoints: []\n".to_string(),
                "/points",
            ),
            (
                "format: 1\nkind: patrol\npoints:\n  - at:\n      - 0.0\n      - 1.0e300\n      - 0.0\n".to_string(),
                "/points/0/at/1",
            ),
        ];
        for (source, pointer) in cases {
            let found = codes_and_paths(&source);
            assert!(
                found.contains(&("L1523".to_string(), pointer.to_string())),
                "{pointer}: {found:?}"
            );
        }
    }
}
