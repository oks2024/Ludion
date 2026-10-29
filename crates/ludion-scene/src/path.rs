//! Path documents: `kind: path` — an ordered list of 3D points read as a spline, and the pure
//! sampler that turns it into position and direction by arc length (`Plan/character-controller.md`
//! decision 9).
//!
//! A path is hand-authored: a lift's route, a patrol, a camera dolly. The same file serves any
//! consumer that wants "where is the point `s` metres along this curve".
//!
//! ```yaml
//! format: 1
//! kind: path
//! name: lift
//! interpolation: catmull_rom
//! closed: false
//! points:
//!   - - 0.0
//!     - 0.0
//!     - 0.0
//!   - - 4.0
//!     - 0.5
//!     - -6.0
//! ```
//!
//! `interpolation` is `catmull_rom` (centripetal, so an uneven point spacing never loops or cusps) or
//! `linear`; both default when absent, and the canonical writer always spells them. `closed: true`
//! joins the last point back to the first. The optional `generated_from` names the
//! content-root-relative script that emitted the file (provenance only, no drift gate).
//!
//! **This module stays IO-free.** [`PathSampler`] is a pure function of a validated
//! [`PathDocument`]: only `+ − × ÷` and `sqrt`, so two builds from one document are bit-identical.
//!
//! Structure (`L1213`) and data consistency (`L1214`) split as in the character family.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::fields::parse_number;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, Node, NodeValue};
use ludion_core::number::fits_f32;

/// The path document format version this crate reads and writes.
pub const PATH_FORMAT_VERSION: u64 = 1;

const PATH_KEYS: [&str; 7] = [
    "format",
    "kind",
    "name",
    "generated_from",
    "interpolation",
    "closed",
    "points",
];
const INTERPOLATIONS: [&str; 2] = ["catmull_rom", "linear"];

/// How a path runs between its points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PathInterpolation {
    /// Centripetal Catmull-Rom: passes through every point, smooth, never loops on uneven spacing.
    #[default]
    CatmullRom,
    /// Straight segments between the points.
    Linear,
}

impl PathInterpolation {
    fn keyword(self) -> &'static str {
        match self {
            Self::CatmullRom => "catmull_rom",
            Self::Linear => "linear",
        }
    }
}

/// A parsed, structurally valid path document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PathDocument {
    /// Optional human-readable path name.
    pub name: Option<String>,
    /// The content-root-relative source that emitted this path, as provenance only: the engine
    /// runs no drift gate on it.
    pub generated_from: Option<String>,
    /// How the path runs between its points.
    pub interpolation: PathInterpolation,
    /// Whether the last point joins back to the first.
    pub closed: bool,
    /// The control points, in order.
    pub points: Vec<[f64; 3]>,
}

// --- Parsing (`L1213`) --------------------------------------------------------------------------

/// Parses strict-subset YAML into a [`PathDocument`], checking structure/shape only (`L1213`).
pub(crate) fn parse(source: &str) -> Result<PathDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::PATH_DOCUMENT_STRUCTURE,
                "the path document is empty; a path needs at least `format: 1`, `kind: path` and \
                 `points`",
            )
            .with_path("")
            .with_help(
                "a minimal path is:\n  format: 1\n  kind: path\n  points:\n    - - 0.0\n      - 0.0\n      - 0.0\n    - - 1.0\n      - 0.0\n      - 0.0",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::PATH_DOCUMENT_STRUCTURE,
                format!(
                    "the path document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: path`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = PathDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut points_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::PATH_DOCUMENT_STRUCTURE,
                    code::PATH_DOCUMENT_STRUCTURE,
                    PATH_FORMAT_VERSION,
                    "the path document",
                    "path",
                    "path",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::PATH_DOCUMENT_STRUCTURE,
                    "path",
                    "a path document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the path name",
                    code::PATH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "generated_from" => {
                doc.generated_from = envelope::expect_string(
                    &entry.node,
                    "/generated_from",
                    "`generated_from`",
                    code::PATH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "interpolation" => {
                if let Some(interpolation) =
                    parse_interpolation(&entry.node, "/interpolation", &mut diagnostics)
                {
                    doc.interpolation = interpolation;
                }
            }
            "closed" => {
                if let Some(closed) =
                    parse_bool(&entry.node, "/closed", "`closed`", &mut diagnostics)
                {
                    doc.closed = closed;
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
                "a path document",
                PATH_KEYS,
                code::PATH_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key("format", "add `format: 1` as the first line"));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: path`"));
    }
    if !points_seen {
        diagnostics.push(missing_key(
            "points",
            "add `points`: a list of three-number points, one number per line",
        ));
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

fn missing_key(key: &str, help: &str) -> Diagnostic {
    envelope::missing_key(key, help, code::PATH_DOCUMENT_STRUCTURE, "path document")
}

/// Parses `points:` — a sequence of three-number points. The minimum count is a data rule
/// (`L1214`), not a structure one.
fn parse_points(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<[f64; 3]>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::PATH_DOCUMENT_STRUCTURE,
                format!(
                    "`points` must be a sequence, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write `points` as a list, one `- - x` entry per point"),
        );
        return None;
    };
    let mut points = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match parse_vec3(item, &format!("{pointer}/{index}"), "a point", diagnostics) {
            Some(point) => points.push(point),
            None => ok = false,
        }
    }
    ok.then_some(points)
}

fn parse_vec3(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; 3]> {
    let help = format!("write `{what}` as three numbers, one per line");
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::PATH_DOCUMENT_STRUCTURE,
                format!(
                    "`{what}` must be a sequence of three numbers, found {} (line {})",
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
                code::PATH_DOCUMENT_STRUCTURE,
                format!(
                    "`{what}` must have exactly 3 components, found {} (line {})",
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
            what,
            code::PATH_DOCUMENT_STRUCTURE,
            diagnostics,
        ) {
            Some(value) => out[index] = value,
            None => ok = false,
        }
    }
    ok.then_some(out)
}

fn parse_interpolation(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<PathInterpolation> {
    let listed = format!("`interpolation` is one of: {}", INTERPOLATIONS.join(", "));
    match &node.value {
        NodeValue::Scalar(Value::String(s)) => {
            match INTERPOLATIONS
                .iter()
                .position(|choice| *choice == s.as_str())
            {
                Some(0) => return Some(PathInterpolation::CatmullRom),
                Some(_) => return Some(PathInterpolation::Linear),
                None => {}
            }
            let help = match suggest::nearest(s, INTERPOLATIONS.iter().copied()) {
                Some(candidate) => format!("nearest match is `{candidate}`"),
                None => listed,
            };
            diagnostics.push(
                Diagnostic::new(
                    code::PATH_DOCUMENT_STRUCTURE,
                    format!("unknown interpolation `{s}` (line {})", node.line),
                )
                .with_path(pointer.to_string())
                .with_help(help),
            );
            None
        }
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::PATH_DOCUMENT_STRUCTURE,
                    format!(
                        "`interpolation` must be a string, found {} (line {})",
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

fn parse_bool(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<bool> {
    match &node.value {
        NodeValue::Scalar(Value::Bool(value)) => Some(*value),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::PATH_DOCUMENT_STRUCTURE,
                    format!(
                        "`{what}` must be a boolean, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(format!("write `{what}: true` or `{what}: false`")),
            );
            None
        }
    }
}

// --- Validation (`L1214`) -----------------------------------------------------------------------

/// Validates data-consistency invariants of an already-parsed [`PathDocument`] (`L1214`): at least 2
/// points (3 when closed), every component finite and inside the f32 range, no two neighbouring
/// points at the same position (the last and the first included when closed), and a `generated_from`
/// that is a non-blank, relative path without `..`.
pub(crate) fn validate(doc: &PathDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let n = doc.points.len();
    let minimum = if doc.closed { 3 } else { 2 };
    if n < minimum {
        diagnostics.push(
            Diagnostic::new(
                code::PATH_DATA_CONSISTENCY,
                format!(
                    "{} path needs at least {minimum} points, found {n}",
                    if doc.closed { "a closed" } else { "a" }
                ),
            )
            .with_path("/points")
            .with_help("add points to `points`, or set `closed: false` for a two-point path"),
        );
    }
    if let Some(source) = &doc.generated_from
        && let Some(problem) = generated_from_problem(source)
    {
        diagnostics.push(
            Diagnostic::new(
                code::PATH_DATA_CONSISTENCY,
                format!("`generated_from` {problem}"),
            )
            .with_path("/generated_from")
            .with_help(
                "name the source as a content-root-relative path such as \
                 `generation/blender/level/build.py`",
            ),
        );
    }
    let mut all_finite = true;
    for (index, point) in doc.points.iter().enumerate() {
        if let Some(value) = point.iter().find(|v| !v.is_finite() || !fits_f32(**v)) {
            all_finite = false;
            diagnostics.push(
                Diagnostic::new(
                    code::PATH_DATA_CONSISTENCY,
                    format!(
                        "point {index} has the component {value}, which is not a finite f32-range \
                         number"
                    ),
                )
                .with_path(format!("/points/{index}"))
                .with_help("keep every component a finite number below 3.4e38 in magnitude"),
            );
        }
    }
    if all_finite && n >= 2 {
        let neighbours = if doc.closed { n } else { n - 1 };
        for i in 0..neighbours {
            let next = (i + 1) % n;
            if distance(doc.points[i], doc.points[next]) == 0.0 {
                diagnostics.push(
                    Diagnostic::new(
                        code::PATH_DATA_CONSISTENCY,
                        format!(
                            "point {next} sits exactly on point {i}, which leaves a segment with \
                             no direction"
                        ),
                    )
                    .with_path(format!("/points/{next}"))
                    .with_help("move one of the two points apart, or delete it"),
                );
            }
        }
    }
    diagnostics
}

/// Why `source` is not a content-root-relative path, if it is not.
fn generated_from_problem(source: &str) -> Option<&'static str> {
    if source.trim().is_empty() {
        return Some("is blank");
    }
    if source.starts_with(['/', '\\']) || source.as_bytes().get(1) == Some(&b':') {
        return Some("is an absolute path");
    }
    if source.split(['/', '\\']).any(|part| part == "..") {
        return Some("climbs out of the content root with `..`");
    }
    None
}

// --- Canonical writer ---------------------------------------------------------------------------

/// Writes `doc` in canonical form: `format`, `kind`, `name` and `generated_from` (both optional),
/// `interpolation`, `closed`, `points` — the last three always spelled.
pub(crate) fn emit(doc: &PathDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: path\n");
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
    out.push_str("interpolation: ");
    out.push_str(doc.interpolation.keyword());
    out.push('\n');
    out.push_str(if doc.closed {
        "closed: true\n"
    } else {
        "closed: false\n"
    });
    out.push_str("points:\n");
    for point in &doc.points {
        for (component, value) in point.iter().enumerate() {
            let lead = if component == 0 { "- - " } else { "  - " };
            out.push_str(&format!("  {lead}{}\n", format_float(*value)));
        }
    }
    out
}

// --- Sampler ------------------------------------------------------------------------------------

/// Uniform local-t samples per segment in the arc-length table.
const SUBDIV: usize = 16;

/// Abscissae of the 8-point Gauss-Legendre rule on `[-1, 1]`, paired by index with [`GL_WEIGHTS`].
const GL_NODES: [f64; 8] = [
    -0.960_289_856_497_536_2,
    -0.796_666_477_413_626_7,
    -0.525_532_409_916_329,
    -0.183_434_642_495_649_8,
    0.183_434_642_495_649_8,
    0.525_532_409_916_329,
    0.796_666_477_413_626_7,
    0.960_289_856_497_536_2,
];

/// Weights of the 8-point Gauss-Legendre rule, paired by index with [`GL_NODES`].
const GL_WEIGHTS: [f64; 8] = [
    0.101_228_536_290_376_3,
    0.222_381_034_453_374_5,
    0.313_706_645_877_887_3,
    0.362_683_783_378_362,
    0.362_683_783_378_362,
    0.313_706_645_877_887_3,
    0.222_381_034_453_374_5,
    0.101_228_536_290_376_3,
];

/// Safeguarded Newton steps inverting arc length to a chord's curve parameter.
const NEWTON_STEPS: usize = 8;

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn combine(a: [f64; 3], b: [f64; 3], wa: f64, wb: f64) -> [f64; 3] {
    [
        a[0] * wa + b[0] * wb,
        a[1] * wa + b[1] * wb,
        a[2] * wa + b[2] * wb,
    ]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalized(v: [f64; 3]) -> Option<[f64; 3]> {
    let len = dot(v, v).sqrt();
    (len > 0.0 && len.is_finite()).then(|| scale(v, 1.0 / len))
}

/// Tangents whose horizontal magnitude `sqrt(x² + z²)` is below this count as vertical.
const VERTICAL_EPS: f64 = 1e-6;

/// Golden-section probes per nearest-point refinement.
const REFINE_STEPS: usize = 48;

/// `1 / φ`, the golden-section interior ratio.
const GOLDEN: f64 = 0.618_033_988_749_894_9;

/// `normalize(tangent × +Y)`: the horizontal axis to the right of the direction of travel, or `None`
/// when the tangent is vertical.
fn horizontal_right(tangent: [f64; 3]) -> Option<[f64; 3]> {
    let horizontal = (tangent[0] * tangent[0] + tangent[2] * tangent[2]).sqrt();
    (horizontal >= VERTICAL_EPS).then(|| [-tangent[2] / horizontal, 0.0, tangent[0] / horizontal])
}

/// How [`PathSampler::nearest_s`] measures distance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathMetric {
    /// Straight-line distance in 3D.
    Full,
    /// Distance in the XZ plane; the curve point's height only breaks ties between equally near
    /// points. Use it where two legs cross over each other at different heights.
    Horizontal,
}

/// A no-roll frame on a path: see [`PathSampler::frame_at`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathFrame {
    /// The point on the path.
    pub position: [f64; 3],
    /// The unit direction of travel.
    pub tangent: [f64; 3],
    /// The unit axis to the right of travel, horizontal wherever the tangent is not vertical.
    pub right: [f64; 3],
    /// The unit axis `right × tangent`; `y > 0` wherever the tangent is not vertical.
    pub up: [f64; 3],
}

impl PathFrame {
    /// The unit quaternion `[x, y, z, w]` (`w ≥ 0`) whose basis columns are `(right, up, −tangent)`:
    /// local −Z faces travel (as a camera's does), +Y is `up`, +X is `right`.
    pub fn rotation(&self) -> [f64; 4] {
        let (r, u) = (self.right, self.up);
        let f = scale(self.tangent, -1.0);
        // Matrix entries m[row][col]; the columns are r, u, f.
        let (m00, m01, m02) = (r[0], u[0], f[0]);
        let (m10, m11, m12) = (r[1], u[1], f[1]);
        let (m20, m21, m22) = (r[2], u[2], f[2]);
        let trace = m00 + m11 + m22;
        let [x, y, z, w] = if trace >= m00 && trace >= m11 && trace >= m22 {
            let s = (1.0 + trace).sqrt() * 2.0;
            [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s]
        } else if m00 >= m11 && m00 >= m22 {
            let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
            [0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
        } else if m11 >= m22 {
            let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
            [(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s]
        } else {
            let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
            [(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s]
        };
        if w < 0.0 {
            [-x, -y, -z, -w]
        } else {
            [x, y, z, w]
        }
    }
}

/// The comparison key of a candidate point at arc length `s`: smaller is nearer, compared
/// lexicographically so ties fall to the smaller `s`.
fn distance_key(p: [f64; 3], q: [f64; 3], s: f64, metric: PathMetric) -> [f64; 3] {
    let d = sub(q, p);
    match metric {
        PathMetric::Full => [dot(d, d), 0.0, s],
        PathMetric::Horizontal => [d[0] * d[0] + d[2] * d[2], d[1] * d[1], s],
    }
}

/// Where along `a → b` (`0..=1`) the point nearest `p` sits; a segment with no extent in the metric
/// gives its start.
fn chord_param(p: [f64; 3], a: [f64; 3], b: [f64; 3], metric: PathMetric) -> f64 {
    let (ab, ap) = (sub(b, a), sub(p, a));
    let (numerator, denominator) = match metric {
        PathMetric::Full => (dot(ap, ab), dot(ab, ab)),
        PathMetric::Horizontal => (ap[0] * ab[0] + ap[2] * ab[2], ab[0] * ab[0] + ab[2] * ab[2]),
    };
    if denominator > 0.0 {
        (numerator / denominator).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// One arc-length table sample: the curve position at `local_t` of `segment`, `s` metres along
/// (the integrated arc length on Catmull-Rom, the exact segment sum on linear).
#[derive(Clone, Copy, Debug, PartialEq)]
struct TableEntry {
    s: f64,
    segment: usize,
    local_t: f64,
    position: [f64; 3],
}

impl TableEntry {
    /// The entry's place along the whole curve: segment index plus local parameter.
    fn u(&self) -> f64 {
        self.segment as f64 + self.local_t
    }
}

/// Arc-length parameterised access to a path: position and direction `s` metres along it.
///
/// Built from a document that passed [`crate::validate_path`]; an invalid one (fewer than two points,
/// a coincident neighbour, a non-finite component) gives unspecified, possibly non-finite results.
#[derive(Clone, Debug)]
pub struct PathSampler {
    points: Vec<[f64; 3]>,
    closed: bool,
    interpolation: PathInterpolation,
    table: Vec<TableEntry>,
    length: f64,
    carried_right: Vec<[f64; 3]>,
}

impl PathSampler {
    /// Builds the sampler and its arc-length table. `doc` must be validate-clean.
    pub fn new(doc: &PathDocument) -> Self {
        debug_assert!(doc.points.len() >= 2, "a path needs at least two points");
        let mut sampler = Self {
            points: doc.points.clone(),
            closed: doc.closed,
            interpolation: doc.interpolation,
            table: Vec::new(),
            length: 0.0,
            carried_right: Vec::new(),
        };
        // A linear segment is exactly straight: its start and end are the whole table.
        let subdiv = match sampler.interpolation {
            PathInterpolation::CatmullRom => SUBDIV,
            PathInterpolation::Linear => 1,
        };
        let segments = sampler.segment_count();
        let mut table = Vec::with_capacity(segments * subdiv + 1);
        let mut s = 0.0;
        for segment in 0..segments {
            for k in 0..subdiv {
                let local_t = k as f64 / subdiv as f64;
                let position = sampler.evaluate(segment, local_t);
                if let Some(previous) = table.last() {
                    let previous: &TableEntry = previous;
                    s += sampler.step_length(previous, segment, local_t, position);
                }
                table.push(TableEntry {
                    s,
                    segment,
                    local_t,
                    position,
                });
            }
        }
        let last = segments - 1;
        let position = sampler.evaluate(last, 1.0);
        if let Some(previous) = table.last() {
            s += sampler.step_length(previous, last, 1.0, position);
        }
        table.push(TableEntry {
            s,
            segment: last,
            local_t: 1.0,
            position,
        });
        sampler.length = s;
        sampler.table = table;
        sampler.carried_right = sampler.carry_rights();
        sampler
    }

    /// The total arc length.
    pub fn length(&self) -> f64 {
        self.length
    }

    /// Whether the path wraps from its end back to its start.
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// The position `s` metres along the path. An open path clamps `s` to `[0, length]`; a closed one
    /// wraps it.
    pub fn point_at(&self, s: f64) -> [f64; 3] {
        let (segment, local_t) = self.curve_param(s);
        self.evaluate(segment, local_t)
    }

    /// The unit direction of travel at `s`: the curve's own derivative, continuous along a
    /// Catmull-Rom path. A linear path's tangent is its segment's direction; at an interior corner
    /// it is the outgoing segment's, and at an open end the last segment's.
    pub fn tangent_at(&self, s: f64) -> [f64; 3] {
        let (segment, local_t) = self.curve_param(s);
        self.curve_tangent(segment, local_t).unwrap_or_else(|| {
            let (j, _) = self.locate(s);
            self.chord_tangent(j)
        })
    }

    /// The no-roll frame at `s`: position, tangent, and an orthonormal `right`/`up` pair whose `up`
    /// leans toward +Y (the reference up is fixed at +Y; a loop-de-loop is out of scope). Where the
    /// tangent is vertical (horizontal magnitude below 1e-6), `right` carries over from the nearest
    /// non-vertical stretch of the path, so it never flips inside a vertical run.
    pub fn frame_at(&self, s: f64) -> PathFrame {
        let tangent = self.tangent_at(s);
        let right = match horizontal_right(tangent) {
            Some(right) => right,
            None => {
                let (j, frac) = self.locate(s);
                let nearer = if frac > 0.5 { j + 1 } else { j };
                let carried = self.carried_right[nearer];
                let along = dot(carried, tangent);
                normalized(sub(carried, scale(tangent, along))).unwrap_or(carried)
            }
        };
        PathFrame {
            position: self.point_at(s),
            tangent,
            right,
            up: cross(right, tangent),
        }
    }

    /// The `s` of the point on the path nearest `p`, searched within `window_metres` of arc length
    /// either side of `hint`. The window is what keeps a switchback from handing back the wrong
    /// leg: pass the previous result as `hint` and a window shorter than the hairpin. An open path
    /// clamps the window to `[0, length]`; a closed one wraps it, and a window of at least half the
    /// length is the global search. Ties go to the smaller `s`. The result is in `[0, length]`
    /// (`[0, length)` when closed).
    pub fn nearest_s(&self, p: [f64; 3], hint: f64, window_metres: f64, metric: PathMetric) -> f64 {
        let window = window_metres.max(0.0);
        let (lo, hi) = if self.closed {
            let hint = hint.rem_euclid(self.length);
            if 2.0 * window >= self.length {
                (0.0, self.length)
            } else {
                (hint - window, hint + window)
            }
        } else {
            let hint = hint.clamp(0.0, self.length);
            ((hint - window).max(0.0), (hint + window).min(self.length))
        };
        self.nearest_in(p, lo, hi, metric)
    }

    /// The `s` of the point on the whole path nearest `p`; ties go to the smaller `s`. Prefer
    /// [`PathSampler::nearest_s`] with a window for anything that moves continuously along the path.
    pub fn nearest_s_global(&self, p: [f64; 3], metric: PathMetric) -> f64 {
        self.nearest_in(p, 0.0, self.length, metric)
    }

    /// The `(segment, local_t)` curve parameter `s` metres along the path. A linear path's table
    /// chords are the segments, so it interpolates; a Catmull-Rom chord is inverted on the arc-length
    /// integral so `s` stays exact inside it.
    fn curve_param(&self, s: f64) -> (usize, f64) {
        let (j, frac) = self.locate(s);
        let (a, b) = (&self.table[j], &self.table[j + 1]);
        if self.interpolation == PathInterpolation::Linear {
            let u = a.u() * (1.0 - frac) + b.u() * frac;
            let last_segment = self.segment_count() - 1;
            let segment = (u.floor().max(0.0) as usize).min(last_segment);
            return (segment, u - segment as f64);
        }
        let segment = a.segment;
        let t_lo = a.local_t;
        let t_hi = if b.segment == a.segment {
            b.local_t
        } else {
            1.0
        };
        if frac <= 0.0 {
            return (segment, t_lo);
        }
        if frac >= 1.0 {
            return (segment, t_hi);
        }
        let target = self.wrapped(s) - a.s;
        let (mut lo, mut hi) = (t_lo, t_hi);
        let mut t = t_lo + (t_hi - t_lo) * frac;
        for _ in 0..NEWTON_STEPS {
            let f = self.arc(segment, t_lo, t) - target;
            if f > 0.0 {
                hi = t;
            } else {
                lo = t;
            }
            let step = t - f / self.speed(segment, t);
            t = if step >= lo && step <= hi {
                step
            } else {
                0.5 * (lo + hi)
            };
        }
        (segment, t)
    }

    /// The arc length from table entry `previous` to the point at `local_t` of `segment`.
    fn step_length(
        &self,
        previous: &TableEntry,
        segment: usize,
        local_t: f64,
        position: [f64; 3],
    ) -> f64 {
        match self.interpolation {
            PathInterpolation::Linear => distance(previous.position, position),
            PathInterpolation::CatmullRom => {
                let to = if segment == previous.segment {
                    local_t
                } else {
                    1.0
                };
                self.arc(previous.segment, previous.local_t, to)
            }
        }
    }

    /// The curve speed `|dC/dlocal_t|` at `local_t` of `segment`, in metres per unit of `local_t`.
    fn speed(&self, segment: usize, local_t: f64) -> f64 {
        let i = segment as isize;
        let (p0, p1, p2, p3) = (
            self.control(i - 1),
            self.control(i),
            self.control(i + 1),
            self.control(i + 2),
        );
        let knot_span = distance(p1, p2).sqrt();
        let d = catmull_rom_derivative(p0, p1, p2, p3, local_t);
        dot(d, d).sqrt() * knot_span
    }

    /// The arc length of `segment` between `from` and `to` (`local_t`): four panels of 8-point
    /// Gauss-Legendre.
    fn arc(&self, segment: usize, from: f64, to: f64) -> f64 {
        let panel = (to - from) / 4.0;
        let mut total = 0.0;
        for p in 0..4 {
            let a = from + panel * p as f64;
            let mid = a + 0.5 * panel;
            let half = 0.5 * panel;
            let mut sum = 0.0;
            for (node, weight) in GL_NODES.iter().zip(GL_WEIGHTS) {
                sum += weight * self.speed(segment, mid + half * node);
            }
            total += sum * half;
        }
        total
    }

    /// The unit curve derivative at `local_t` of `segment`; `None` if it vanishes.
    fn curve_tangent(&self, segment: usize, local_t: f64) -> Option<[f64; 3]> {
        let i = segment as isize;
        let (p1, p2) = (self.control(i), self.control(i + 1));
        let derivative = match self.interpolation {
            PathInterpolation::Linear => sub(p2, p1),
            PathInterpolation::CatmullRom => {
                let (p0, p3) = (self.control(i - 1), self.control(i + 2));
                catmull_rom_derivative(p0, p1, p2, p3, local_t.clamp(0.0, 1.0))
            }
        };
        normalized(derivative)
    }

    /// The unit direction of the table chord `[j, j + 1]`.
    fn chord_tangent(&self, j: usize) -> [f64; 3] {
        let d = sub(self.table[j + 1].position, self.table[j].position);
        let len = dot(d, d).sqrt();
        [d[0] / len, d[1] / len, d[2] / len]
    }

    /// Per table entry, the `right` axis a vertical tangent there borrows (see [`Self::frame_at`]).
    fn carry_rights(&self) -> Vec<[f64; 3]> {
        let n = self.table.len();
        let own: Vec<Option<[f64; 3]>> = self
            .table
            .iter()
            .map(|entry| {
                self.curve_tangent(entry.segment, entry.local_t)
                    .and_then(horizontal_right)
            })
            .collect();
        // Nearest regular entry at or before / at or after each index.
        let mut before = vec![None; n];
        let mut last = None;
        for (i, slot) in before.iter_mut().enumerate() {
            if own[i].is_some() {
                last = Some(i);
            }
            *slot = last;
        }
        let mut after = vec![None; n];
        let mut next = None;
        for i in (0..n).rev() {
            if own[i].is_some() {
                next = Some(i);
            }
            after[i] = next;
        }
        (0..n)
            .map(|i| {
                let source = match (before[i], after[i]) {
                    (Some(b), Some(a)) => {
                        let (db, da) = (
                            self.table[i].s - self.table[b].s,
                            self.table[a].s - self.table[i].s,
                        );
                        Some(if da < db { a } else { b })
                    }
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                };
                source
                    .and_then(|index| own[index])
                    .unwrap_or([1.0, 0.0, 0.0])
            })
            .collect()
    }

    /// The best `s` in the unwrapped arc-length interval `[lo, hi]` (a closed path wraps, so `lo < 0`
    /// or `hi > length` is fine): an exact scan of the table chords, then a fixed-count golden-section
    /// refinement around the winning chord.
    fn nearest_in(&self, p: [f64; 3], lo: f64, hi: f64, metric: PathMetric) -> f64 {
        let n = self.table.len();
        let (k_min, k_max) = if self.closed {
            (
                (lo / self.length).floor() as i64,
                (hi / self.length).floor() as i64,
            )
        } else {
            (0, 0)
        };
        // (key, unwrapped s, chord index, wrap count)
        let mut best: Option<([f64; 3], f64, usize, i64)> = None;
        for k in k_min..=k_max {
            let offset = k as f64 * self.length;
            let (lo_k, hi_k) = (lo - offset, hi - offset);
            let first = self
                .table
                .partition_point(|entry| entry.s <= lo_k)
                .saturating_sub(1)
                .min(n - 2);
            for i in first..n - 1 {
                let (a, b) = (&self.table[i], &self.table[i + 1]);
                if a.s > hi_k {
                    break;
                }
                if b.s < lo_k {
                    continue;
                }
                let (sa, sb) = (a.s.max(lo_k), b.s.min(hi_k));
                let span = b.s - a.s;
                let at = |s: f64| {
                    let frac = (s - a.s) / span;
                    combine(a.position, b.position, 1.0 - frac, frac)
                };
                let (pa, pb) = (at(sa), at(sb));
                let t = chord_param(p, pa, pb, metric);
                let q = combine(pa, pb, 1.0 - t, t);
                let s = sa + (sb - sa) * t + offset;
                let key = distance_key(p, q, s, metric);
                if best.is_none_or(|(best_key, ..)| key < best_key) {
                    best = Some((key, s, i, k));
                }
            }
        }
        let Some((_, scan_s, chord, wraps)) = best else {
            return self.wrap_s(lo);
        };
        if self.interpolation == PathInterpolation::Linear {
            // A linear path's table chords are its segments: the scan is already exact.
            return self.wrap_s(scan_s);
        }

        let offset = wraps as f64 * self.length;
        let span_before = if chord > 0 {
            self.table[chord].s - self.table[chord - 1].s
        } else if self.closed {
            self.table[n - 1].s - self.table[n - 2].s
        } else {
            0.0
        };
        let span_after = if chord + 2 < n {
            self.table[chord + 2].s - self.table[chord + 1].s
        } else if self.closed {
            self.table[1].s - self.table[0].s
        } else {
            0.0
        };
        let bracket_lo = (self.table[chord].s + offset - span_before).max(lo);
        let bracket_hi = (self.table[chord + 1].s + offset + span_after).min(hi);

        let key_at = |s: f64| distance_key(p, self.point_at(s), s, metric);
        let (mut a, mut b) = (bracket_lo, bracket_hi);
        let mut x1 = b - GOLDEN * (b - a);
        let mut x2 = a + GOLDEN * (b - a);
        let (mut f1, mut f2) = (key_at(x1), key_at(x2));
        for _ in 0..REFINE_STEPS {
            if f1 <= f2 {
                b = x2;
                x2 = x1;
                f2 = f1;
                x1 = b - GOLDEN * (b - a);
                f1 = key_at(x1);
            } else {
                a = x1;
                x1 = x2;
                f1 = f2;
                x2 = a + GOLDEN * (b - a);
                f2 = key_at(x2);
            }
        }
        let mut winner = (key_at(scan_s), scan_s);
        for candidate in [bracket_lo, (a + b) * 0.5, bracket_hi] {
            let key = key_at(candidate);
            if key < winner.0 {
                winner = (key, candidate);
            }
        }
        self.wrap_s(winner.1)
    }

    /// Folds an unwrapped arc length into the path's range.
    fn wrap_s(&self, s: f64) -> f64 {
        if self.closed {
            let wrapped = s.rem_euclid(self.length);
            if wrapped >= self.length { 0.0 } else { wrapped }
        } else {
            s.clamp(0.0, self.length)
        }
    }

    fn segment_count(&self) -> usize {
        if self.closed {
            self.points.len()
        } else {
            self.points.len() - 1
        }
    }

    /// `s` wrapped into `[0, length)` when closed, clamped to `[0, length]` when open.
    fn wrapped(&self, s: f64) -> f64 {
        if self.closed {
            s.rem_euclid(self.length)
        } else {
            s.clamp(0.0, self.length)
        }
    }

    /// The table index `j` whose chord `[j, j + 1]` brackets `s`, and how far along it `s` sits.
    fn locate(&self, s: f64) -> (usize, f64) {
        let s = self.wrapped(s);
        let j = self
            .table
            .partition_point(|entry| entry.s <= s)
            .saturating_sub(1)
            .min(self.table.len() - 2);
        let span = self.table[j + 1].s - self.table[j].s;
        let frac = if span > 0.0 {
            (s - self.table[j].s) / span
        } else {
            0.0
        };
        (j, frac)
    }

    /// The control point at `index`, wrapping when closed and reflecting the end points outward
    /// (`2·P0 − P1`) when open.
    fn control(&self, index: isize) -> [f64; 3] {
        let n = self.points.len() as isize;
        if self.closed {
            return self.points[index.rem_euclid(n) as usize];
        }
        if index < 0 {
            let (p0, p1) = (self.points[0], self.points[1]);
            return combine(p0, p1, 2.0, -1.0);
        }
        if index >= n {
            let (last, before) = (self.points[n as usize - 1], self.points[n as usize - 2]);
            return combine(last, before, 2.0, -1.0);
        }
        self.points[index as usize]
    }

    /// The curve at `local_t` (`0..=1`) of `segment`; exact at both ends.
    fn evaluate(&self, segment: usize, local_t: f64) -> [f64; 3] {
        let i = segment as isize;
        let (p1, p2) = (self.control(i), self.control(i + 1));
        if local_t <= 0.0 {
            return p1;
        }
        if local_t >= 1.0 {
            return p2;
        }
        match self.interpolation {
            PathInterpolation::Linear => combine(p1, p2, 1.0 - local_t, local_t),
            PathInterpolation::CatmullRom => {
                let (p0, p3) = (self.control(i - 1), self.control(i + 2));
                catmull_rom(p0, p1, p2, p3, local_t)
            }
        }
    }
}

/// Centripetal (alpha 0.5) Catmull-Rom by the Barry–Goldman pyramid, evaluated between `p1` and `p2`.
fn catmull_rom(p0: [f64; 3], p1: [f64; 3], p2: [f64; 3], p3: [f64; 3], local_t: f64) -> [f64; 3] {
    let t0 = 0.0;
    let t1 = t0 + distance(p0, p1).sqrt();
    let t2 = t1 + distance(p1, p2).sqrt();
    let t3 = t2 + distance(p2, p3).sqrt();
    let t = t1 + (t2 - t1) * local_t;
    let a1 = combine(p0, p1, (t1 - t) / (t1 - t0), (t - t0) / (t1 - t0));
    let a2 = combine(p1, p2, (t2 - t) / (t2 - t1), (t - t1) / (t2 - t1));
    let a3 = combine(p2, p3, (t3 - t) / (t3 - t2), (t - t2) / (t3 - t2));
    let b1 = combine(a1, a2, (t2 - t) / (t2 - t0), (t - t0) / (t2 - t0));
    let b2 = combine(a2, a3, (t3 - t) / (t3 - t1), (t - t1) / (t3 - t1));
    combine(b1, b2, (t2 - t) / (t2 - t1), (t - t1) / (t2 - t1))
}

/// The derivative of [`catmull_rom`] with respect to its knot parameter (product rule through the
/// pyramid). Only the direction is meaningful: the constant `t2 − t1` factor to `local_t` is dropped.
fn catmull_rom_derivative(
    p0: [f64; 3],
    p1: [f64; 3],
    p2: [f64; 3],
    p3: [f64; 3],
    local_t: f64,
) -> [f64; 3] {
    let t0 = 0.0;
    let t1 = t0 + distance(p0, p1).sqrt();
    let t2 = t1 + distance(p1, p2).sqrt();
    let t3 = t2 + distance(p2, p3).sqrt();
    let t = t1 + (t2 - t1) * local_t;
    let a1 = combine(p0, p1, (t1 - t) / (t1 - t0), (t - t0) / (t1 - t0));
    let a2 = combine(p1, p2, (t2 - t) / (t2 - t1), (t - t1) / (t2 - t1));
    let a3 = combine(p2, p3, (t3 - t) / (t3 - t2), (t - t2) / (t3 - t2));
    let da1 = scale(sub(p1, p0), 1.0 / (t1 - t0));
    let da2 = scale(sub(p2, p1), 1.0 / (t2 - t1));
    let da3 = scale(sub(p3, p2), 1.0 / (t3 - t2));
    let (wa1, wa2) = ((t2 - t) / (t2 - t0), (t - t0) / (t2 - t0));
    let (wa2b, wa3) = ((t3 - t) / (t3 - t1), (t - t1) / (t3 - t1));
    let b1 = combine(a1, a2, wa1, wa2);
    let b2 = combine(a2, a3, wa2b, wa3);
    let db1 = add(
        combine(da1, da2, wa1, wa2),
        scale(sub(a2, a1), 1.0 / (t2 - t0)),
    );
    let db2 = add(
        combine(da2, da3, wa2b, wa3),
        scale(sub(a3, a2), 1.0 / (t3 - t1)),
    );
    let (wb1, wb2) = ((t2 - t) / (t2 - t1), (t - t1) / (t2 - t1));
    add(
        combine(db1, db2, wb1, wb2),
        scale(sub(b2, b1), 1.0 / (t2 - t1)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(interpolation: PathInterpolation, closed: bool, points: &[[f64; 3]]) -> PathDocument {
        PathDocument {
            name: None,
            generated_from: None,
            interpolation,
            closed,
            points: points.to_vec(),
        }
    }

    const ZIGZAG: [[f64; 3]; 5] = [
        [0.0, 0.0, 0.0],
        [4.0, 0.5, -6.0],
        [7.0, 1.0, 0.0],
        [3.0, 2.0, 5.0],
        [-2.0, 2.5, 3.0],
    ];

    fn close(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
        distance(a, b) <= tol
    }

    /// Every linear query at fixed probes, as `f64::to_bits`.
    fn linear_fingerprint(closed: bool) -> Vec<u64> {
        let sampler = PathSampler::new(&doc(PathInterpolation::Linear, closed, &ZIGZAG));
        let first = distance(ZIGZAG[0], ZIGZAG[1]);
        let length = sampler.length();
        let mut bits = vec![length.to_bits()];
        for s in [0.0, 1.7, first, 0.5 * length, 0.83 * length, length] {
            bits.extend(sampler.point_at(s).map(f64::to_bits));
            bits.extend(sampler.tangent_at(s).map(f64::to_bits));
            bits.extend(sampler.frame_at(s).rotation().map(f64::to_bits));
        }
        for probe in [[1.0, 0.0, -2.0], [5.0, 3.0, 4.0]] {
            bits.push(
                sampler
                    .nearest_s(probe, 3.0, 2.0, PathMetric::Full)
                    .to_bits(),
            );
            bits.push(
                sampler
                    .nearest_s_global(probe, PathMetric::Horizontal)
                    .to_bits(),
            );
        }
        bits
    }

    #[test]
    fn linear_queries_are_byte_identical() {
        let open = linear_fingerprint(false);
        let closed = linear_fingerprint(true);
        assert_eq!(open, LINEAR_OPEN_BITS);
        assert_eq!(closed, LINEAR_CLOSED_BITS);
    }
    const LINEAR_OPEN_BITS: [u64; 65] = [
        0x4039_d823_c4b6_2a44,
        0x0000_0000_0000_0000,
        0x0000_0000_0000_0000,
        0x0000_0000_0000_0000,
        0x3fe1_b538_4c0c_28ba,
        0x3fb1_b538_4c0c_28ba,
        0xbfea_8fd4_7212_3d17,
        0x3fa0_f54f_1a94_1ac9,
        0xbfd2_88fa_c8ea_8875,
        0x3f84_89c5_3928_b104,
        0x3fee_9bcd_32f5_c311,
        0x3fee_1a79_4e14_aba2,
        0x3fbe_1a79_4e14_aba2,
        0xbff6_93da_fa8f_80ba,
        0x3fe1_b538_4c0c_28ba,
        0x3fb1_b538_4c0c_28ba,
        0xbfea_8fd4_7212_3d17,
        0x3fa0_f54f_1a94_1ac9,
        0xbfd2_88fa_c8ea_8875,
        0x3f84_89c5_3928_b104,
        0x3fee_9bcd_32f5_c311,
        0x4010_0000_0000_0000,
        0x3fe0_0000_0000_0000,
        0xc018_0000_0000_0000,
        0x3fdc_8ae0_f76a_7d36,
        0x3fb3_0740_a4f1_a8cf,
        0x3fec_8ae0_f76a_7d36,
        0x3f81_7fd6_4e9e_5c58,
        0xbfef_1f57_0ef2_173f,
        0x3fa2_8839_41cc_460a,
        0x3fcd_6355_de8e_d288,
        0x401a_2836_85a8_74a8,
        0x3fed_8af3_5ce0_9b8a,
        0xbfed_7c97_a578_b588,
        0x3fdc_8ae0_f76a_7d36,
        0x3fb3_0740_a4f1_a8cf,
        0x3fec_8ae0_f76a_7d36,
        0x3f81_7fd6_4e9e_5c58,
        0xbfef_1f57_0ef2_173f,
        0x3fa2_8839_41cc_460a,
        0x3fcd_6355_de8e_d288,
        0x4000_7e9b_9494_077c,
        0x4000_c023_a457_98da,
        0x4012_7fb8_b750_ce4c,
        0xbfed_9581_b5b9_2e4e,
        0x3fb7_aace_2afa_8b72,
        0xbfd7_aace_2afa_8b72,
        0x3fa3_9e7a_7be5_9056,
        0x3fe1_ebc0_57e5_6fb4,
        0xbf9a_90c4_54f7_8eeb,
        0x3fea_7842_8c11_695c,
        0xc000_0000_0000_0000,
        0x4004_0000_0000_0000,
        0x4008_0000_0000_0000,
        0xbfed_9581_b5b9_2e4e,
        0x3fb7_aace_2afa_8b72,
        0xbfd7_aace_2afa_8b72,
        0x3fa3_9e7a_7be5_9056,
        0x3fe1_ebc0_57e5_6fb4,
        0xbf9a_90c4_54f7_8eeb,
        0x3fea_7842_8c11_695c,
        0x4001_b538_4c0c_28ba,
        0x4001_cb03_a507_4b63,
        0x3ff0_0000_0000_0000,
        0x4032_618f_c2cb_0a48,
    ];
    const LINEAR_CLOSED_BITS: [u64; 65] = [
        0x403e_3b55_cd2b_df3d,
        0x0000_0000_0000_0000,
        0x0000_0000_0000_0000,
        0x0000_0000_0000_0000,
        0x3fe1_b538_4c0c_28ba,
        0x3fb1_b538_4c0c_28ba,
        0xbfea_8fd4_7212_3d17,
        0x3fa0_f54f_1a94_1ac9,
        0xbfd2_88fa_c8ea_8875,
        0x3f84_89c5_3928_b104,
        0x3fee_9bcd_32f5_c311,
        0x3fee_1a79_4e14_aba2,
        0x3fbe_1a79_4e14_aba2,
        0xbff6_93da_fa8f_80ba,
        0x3fe1_b538_4c0c_28ba,
        0x3fb1_b538_4c0c_28ba,
        0xbfea_8fd4_7212_3d17,
        0x3fa0_f54f_1a94_1ac9,
        0xbfd2_88fa_c8ea_8875,
        0x3f84_89c5_3928_b104,
        0x3fee_9bcd_32f5_c311,
        0x4010_0000_0000_0000,
        0x3fe0_0000_0000_0000,
        0xc018_0000_0000_0000,
        0x3fdc_8ae0_f76a_7d36,
        0x3fb3_0740_a4f1_a8cf,
        0x3fec_8ae0_f76a_7d36,
        0x3f81_7fd6_4e9e_5c58,
        0xbfef_1f57_0ef2_173f,
        0x3fa2_8839_41cc_460a,
        0x3fcd_6355_de8e_d288,
        0x4019_226e_7c1a_00a8,
        0x3ff2_dd91_83e5_ff58,
        0x3fec_a7af_26fb_f970,
        0xbfe3_c036_50e0_0e03,
        0x3fc3_c036_50e0_0e03,
        0x3fe8_b043_e518_1184,
        0x3f9a_3ab3_3a74_5b54,
        0x3fee_1b05_f5bc_50dc,
        0xbfb2_b188_f409_7cf8,
        0x3fd5_1ef4_a104_6fa4,
        0xbff4_e0aa_088f_b2e0,
        0x4003_71a2_1a07_2f58,
        0x400a_3977_97e3_42a0,
        0xbfed_9581_b5b9_2e4e,
        0x3fb7_aace_2afa_8b72,
        0xbfd7_aace_2afa_8b72,
        0x3fa3_9e7a_7be5_9056,
        0x3fe1_ebc0_57e5_6fb4,
        0xbf9a_90c4_54f7_8eeb,
        0x3fea_7842_8c11_695c,
        0x0000_0000_0000_0000,
        0x0000_0000_0000_0000,
        0x0000_0000_0000_0000,
        0x3fe1_b538_4c0c_28ba,
        0x3fb1_b538_4c0c_28ba,
        0xbfea_8fd4_7212_3d17,
        0x3fa0_f54f_1a94_1ac9,
        0xbfd2_88fa_c8ea_8875,
        0x3f84_89c5_3928_b104,
        0x3fee_9bcd_32f5_c311,
        0x4001_b538_4c0c_28ba,
        0x4001_cb03_a507_4b63,
        0x3ff0_0000_0000_0000,
        0x4032_618f_c2cb_0a48,
    ];

    #[test]
    fn open_endpoints_are_the_first_and_last_points() {
        for interpolation in [PathInterpolation::CatmullRom, PathInterpolation::Linear] {
            let sampler = PathSampler::new(&doc(interpolation, false, &ZIGZAG));
            assert_eq!(sampler.point_at(0.0), ZIGZAG[0]);
            assert_eq!(sampler.point_at(sampler.length()), ZIGZAG[4]);
            assert_eq!(sampler.point_at(-3.0), ZIGZAG[0]);
            assert_eq!(sampler.point_at(sampler.length() + 3.0), ZIGZAG[4]);
        }
    }

    #[test]
    fn segment_boundaries_land_on_the_control_points() {
        let sampler = PathSampler::new(&doc(PathInterpolation::CatmullRom, false, &ZIGZAG));
        for (i, point) in ZIGZAG.iter().enumerate() {
            let entry = sampler
                .table
                .iter()
                .find(|e| e.segment == i.min(3) && e.local_t == if i == 4 { 1.0 } else { 0.0 })
                .expect("the boundary entry exists");
            assert_eq!(entry.position, *point);
            assert!(close(sampler.point_at(entry.s), *point, 1e-9));
        }
    }

    #[test]
    fn linear_length_is_the_exact_sum_of_segment_lengths() {
        let sampler = PathSampler::new(&doc(PathInterpolation::Linear, false, &ZIGZAG));
        let mut sum = 0.0;
        for pair in ZIGZAG.windows(2) {
            sum += distance(pair[0], pair[1]);
        }
        assert_eq!(sampler.length(), sum);
        let mid = distance(ZIGZAG[0], ZIGZAG[1]) * 0.5;
        let expect = combine(ZIGZAG[0], ZIGZAG[1], 0.5, 0.5);
        assert!(close(sampler.point_at(mid), expect, 1e-9));
    }

    #[test]
    fn closed_paths_wrap() {
        let square = [
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 0.0],
            [4.0, 0.0, 4.0],
            [0.0, 0.0, 4.0],
        ];
        for interpolation in [PathInterpolation::CatmullRom, PathInterpolation::Linear] {
            let sampler = PathSampler::new(&doc(interpolation, true, &square));
            let l = sampler.length();
            assert_eq!(sampler.point_at(0.0), square[0]);
            for x in [0.3, 2.5, 7.1, l - 0.2] {
                assert!(close(sampler.point_at(l + x), sampler.point_at(x), 1e-9));
                assert_eq!(sampler.point_at(-x), sampler.point_at(l - x));
            }
        }
    }

    #[test]
    fn the_table_is_strictly_increasing() {
        for closed in [false, true] {
            for interpolation in [PathInterpolation::CatmullRom, PathInterpolation::Linear] {
                let sampler = PathSampler::new(&doc(interpolation, closed, &ZIGZAG));
                assert!(sampler.table.windows(2).all(|w| w[0].s < w[1].s));
                assert_eq!(sampler.table.last().map(|e| e.s), Some(sampler.length()));
            }
        }
    }

    const UNEVEN: [[f64; 3]; 5] = [
        [0.0, 0.0, 0.0],
        [0.01, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.0, 0.0, 5.0],
        [10.01, 0.0, 5.0],
    ];

    const MILD: [[f64; 3]; 5] = [
        [0.0, 0.0, 0.0],
        [0.5, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.0, 0.0, 5.0],
        [10.5, 0.0, 5.0],
    ];

    /// Open and closed Catmull-Rom samplers over each point set, with the tolerance each can hold.
    /// UNEVEN squeezes a right-angle turn into 1 cm beside 5 m segments, which the quadrature only
    /// bounds loosely.
    fn catmull_rom_samplers(tight: f64, loose: f64) -> Vec<(PathSampler, f64)> {
        let mut samplers = Vec::new();
        for (points, tolerance) in [
            (&ZIGZAG[..], tight),
            (&MILD[..], tight),
            (&UNEVEN[..], loose),
        ] {
            for closed in [false, true] {
                let sampler = PathSampler::new(&doc(PathInterpolation::CatmullRom, closed, points));
                samplers.push((sampler, tolerance));
            }
        }
        samplers
    }

    #[test]
    fn gauss_legendre_is_exact_to_degree_15() {
        assert!((GL_WEIGHTS.iter().sum::<f64>() - 2.0).abs() < 1e-15);
        let integral: f64 = GL_NODES
            .iter()
            .zip(GL_WEIGHTS)
            .map(|(x, w)| {
                let (x2, x4) = (x * x, x * x * x * x);
                w * x4 * x4 * x4 * x2
            })
            .sum();
        assert!((integral - 2.0 / 15.0).abs() < 1e-15, "{integral}");
    }

    #[test]
    fn catmull_rom_speed_is_unit_inside_and_across_chords() {
        let h = 1e-5;
        for (sampler, tolerance) in catmull_rom_samplers(1e-6, 1e-4) {
            for pair in sampler.table.windows(2) {
                let (a, b) = (pair[0].s, pair[1].s);
                for s in [a + 1e-4, 0.5 * (a + b), b - 1e-4, b] {
                    if !sampler.closed && (s - h < 0.0 || s + h > sampler.length) {
                        continue;
                    }
                    let speed =
                        distance(sampler.point_at(s + h), sampler.point_at(s - h)) / (2.0 * h);
                    assert!((speed - 1.0).abs() < tolerance, "speed {speed} at s = {s}");
                }
            }
        }
    }

    #[test]
    fn speed_is_continuous_across_chord_boundaries() {
        let h = 1e-6;
        for (sampler, tolerance) in catmull_rom_samplers(1e-5, 1e-3) {
            for entry in &sampler.table[1..sampler.table.len() - 1] {
                let s = entry.s;
                let after = distance(sampler.point_at(s + h), sampler.point_at(s)) / h;
                let before = distance(sampler.point_at(s), sampler.point_at(s - h)) / h;
                assert!(
                    (after - before).abs() < tolerance,
                    "{before} vs {after} at s = {s}"
                );
            }
        }
    }

    #[test]
    fn catmull_rom_length_is_the_dense_arc_length() {
        for (sampler, tolerance) in catmull_rom_samplers(1e-7, 1e-6) {
            let dense_steps = 200_000;
            let mut dense = 0.0;
            for segment in 0..sampler.segment_count() {
                let mut previous = sampler.evaluate(segment, 0.0);
                for k in 1..=dense_steps {
                    let next = sampler.evaluate(segment, k as f64 / dense_steps as f64);
                    dense += distance(previous, next);
                    previous = next;
                }
            }
            let relative = (sampler.length() - dense).abs() / dense;
            assert!(
                relative < tolerance,
                "length {} vs dense {dense}",
                sampler.length()
            );
            let chord_sum: f64 = sampler
                .table
                .windows(2)
                .map(|pair| distance(pair[0].position, pair[1].position))
                .sum();
            assert!(
                sampler.length() > chord_sum,
                "{} <= {chord_sum}",
                sampler.length()
            );
        }
    }

    #[test]
    fn uneven_spacing_stays_finite_and_bounded() {
        let points = [
            [0.0, 0.0, 0.0],
            [0.01, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 0.0, 5.0],
            [10.01, 0.0, 5.0],
        ];
        let sampler = PathSampler::new(&doc(PathInterpolation::CatmullRom, false, &points));
        let steps = 2000;
        for k in 0..=steps {
            let p = sampler.point_at(sampler.length() * k as f64 / steps as f64);
            assert!(p.iter().all(|c| c.is_finite()), "{p:?}");
            assert!(
                (-1.0..=11.0).contains(&p[0])
                    && (-1.0..=1.0).contains(&p[1])
                    && (-1.0..=6.0).contains(&p[2]),
                "overshoot at {p:?}"
            );
        }
    }

    #[test]
    fn two_builds_and_queries_are_bitwise_identical() {
        let source = doc(PathInterpolation::CatmullRom, true, &ZIGZAG);
        let (a, b) = (PathSampler::new(&source), PathSampler::new(&source));
        assert_eq!(a.length().to_bits(), b.length().to_bits());
        for (x, y) in a.table.iter().zip(&b.table) {
            assert_eq!(x.s.to_bits(), y.s.to_bits());
            for c in 0..3 {
                assert_eq!(x.position[c].to_bits(), y.position[c].to_bits());
            }
        }
        let bits3 = |v: [f64; 3]| v.map(f64::to_bits);
        let probe = [1.5, 0.25, -2.0];
        for k in 0..50 {
            let s = k as f64 * 0.7;
            assert_eq!(bits3(a.point_at(s)), bits3(b.point_at(s)));
            assert_eq!(bits3(a.tangent_at(s)), bits3(b.tangent_at(s)));
            let (fa, fb) = (a.frame_at(s), b.frame_at(s));
            assert_eq!(bits3(fa.position), bits3(fb.position));
            assert_eq!(bits3(fa.right), bits3(fb.right));
            assert_eq!(bits3(fa.up), bits3(fb.up));
            assert_eq!(
                fa.rotation().map(f64::to_bits),
                fb.rotation().map(f64::to_bits)
            );
            for metric in [PathMetric::Full, PathMetric::Horizontal] {
                assert_eq!(
                    a.nearest_s(probe, s, 3.0, metric).to_bits(),
                    b.nearest_s(probe, s, 3.0, metric).to_bits()
                );
                assert_eq!(
                    a.nearest_s_global(probe, metric).to_bits(),
                    b.nearest_s_global(probe, metric).to_bits()
                );
            }
        }
    }

    #[test]
    fn tangents_are_unit_length() {
        for closed in [false, true] {
            for interpolation in [PathInterpolation::CatmullRom, PathInterpolation::Linear] {
                let sampler = PathSampler::new(&doc(interpolation, closed, &ZIGZAG));
                for k in 0..=100 {
                    let t = sampler.tangent_at(sampler.length() * k as f64 / 100.0);
                    let len = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
                    assert!((len - 1.0).abs() < 1e-12, "{t:?}");
                }
            }
        }
    }

    const BOTH: [PathInterpolation; 2] = [PathInterpolation::CatmullRom, PathInterpolation::Linear];
    const METRICS: [PathMetric; 2] = [PathMetric::Full, PathMetric::Horizontal];

    fn assert_orthonormal(frame: &PathFrame) {
        let (r, u, t) = (frame.right, frame.up, frame.tangent);
        for axis in [r, u, t] {
            assert!((dot(axis, axis).sqrt() - 1.0).abs() < 1e-9, "{frame:?}");
        }
        for (a, b) in [(r, u), (r, t), (u, t)] {
            assert!(dot(a, b).abs() < 1e-9, "{frame:?}");
        }
    }

    /// Circular distance between two arc lengths on a path of length `l`.
    fn arc_gap(a: f64, b: f64, l: f64, closed: bool) -> f64 {
        let d = (a - b).abs();
        if closed { d.min(l - d) } else { d }
    }

    #[test]
    fn tangent_is_continuous_across_table_samples() {
        for closed in [false, true] {
            let sampler = PathSampler::new(&doc(PathInterpolation::CatmullRom, closed, &ZIGZAG));
            for entry in &sampler.table {
                let (before, after) = (
                    sampler.tangent_at(entry.s - 1e-7),
                    sampler.tangent_at(entry.s + 1e-7),
                );
                assert!(dot(before, after) > 1.0 - 1e-6, "kink at s = {}", entry.s);
            }
            let steps = 4000;
            let mut previous = sampler.tangent_at(0.0);
            for k in 1..=steps {
                let t = sampler.tangent_at(sampler.length() * k as f64 / steps as f64);
                assert!(dot(previous, t) > 1.0 - 1e-3, "jump at step {k}");
                previous = t;
            }
        }
    }

    #[test]
    fn linear_tangent_is_the_outgoing_segment_direction() {
        let sampler = PathSampler::new(&doc(PathInterpolation::Linear, false, &ZIGZAG));
        for (segment, pair) in ZIGZAG.windows(2).enumerate() {
            let expected = normalized(sub(pair[1], pair[0])).expect("non-degenerate");
            let s = sampler
                .table
                .iter()
                .find(|e| e.segment == segment)
                .map(|e| e.s);
            let s = s.expect("each segment has an entry") + 0.01;
            assert!(close(sampler.tangent_at(s), expected, 1e-12));
        }
    }

    #[test]
    fn frame_is_orthonormal_and_roll_free() {
        for closed in [false, true] {
            for interpolation in BOTH {
                let sampler = PathSampler::new(&doc(interpolation, closed, &ZIGZAG));
                for k in 0..=200 {
                    let frame = sampler.frame_at(sampler.length() * k as f64 / 200.0);
                    assert_orthonormal(&frame);
                    assert_eq!(frame.right[1], 0.0, "{frame:?}");
                    assert!(frame.up[1] > 0.0, "{frame:?}");
                }
            }
        }
    }

    #[test]
    fn frame_survives_a_vertical_segment() {
        let lift = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 4.0, 0.0],
            [4.0, 4.0, 0.0],
        ];
        // A Catmull-Rom S-bend swings the tangent's horizontal part through zero, which is a real
        // turn of `right`: that lift is only checked finite and orthonormal.
        let straight_up = [[0.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 8.0, 0.0]];
        for (interpolation, points, roll_free) in [
            (PathInterpolation::Linear, &lift[..], true),
            (PathInterpolation::CatmullRom, &lift[..], false),
            (PathInterpolation::CatmullRom, &straight_up[..], true),
        ] {
            let sampler = PathSampler::new(&doc(interpolation, false, points));
            let steps = 1000;
            let mut previous: Option<PathFrame> = None;
            for k in 0..=steps {
                let frame = sampler.frame_at(sampler.length() * k as f64 / steps as f64);
                assert!(
                    [frame.right, frame.up, frame.tangent]
                        .iter()
                        .all(|v| v.iter().all(|c| c.is_finite()))
                );
                assert_orthonormal(&frame);
                if let Some(previous) = previous.filter(|_| roll_free) {
                    assert!(
                        dot(previous.right, frame.right) > 0.99,
                        "{interpolation:?} flip at step {k}: {previous:?} {frame:?}"
                    );
                }
                previous = Some(frame);
            }
        }
        let vertical = PathSampler::new(&doc(
            PathInterpolation::Linear,
            false,
            &[[0.0, 0.0, 0.0], [0.0, 3.0, 0.0]],
        ));
        let frame = vertical.frame_at(1.5);
        assert_eq!(frame.right, [1.0, 0.0, 0.0]);
        assert_orthonormal(&frame);
    }

    fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
        let u = [q[0], q[1], q[2]];
        let uv = cross(u, v);
        add(add(v, scale(uv, 2.0 * q[3])), scale(cross(u, uv), 2.0))
    }

    #[test]
    fn rotation_maps_minus_z_to_the_tangent() {
        let lift = [[0.0, 0.0, 0.0], [0.0, 3.0, 0.0]];
        let samplers = [
            PathSampler::new(&doc(PathInterpolation::CatmullRom, false, &ZIGZAG)),
            PathSampler::new(&doc(PathInterpolation::Linear, true, &ZIGZAG)),
            PathSampler::new(&doc(PathInterpolation::Linear, false, &lift)),
        ];
        for sampler in &samplers {
            for k in 0..=100 {
                let frame = sampler.frame_at(sampler.length() * k as f64 / 100.0);
                let q = frame.rotation();
                assert!(q[3] >= 0.0);
                assert!((q.iter().map(|c| c * c).sum::<f64>().sqrt() - 1.0).abs() < 1e-12);
                assert!(close(rotate(q, [0.0, 0.0, -1.0]), frame.tangent, 1e-12));
                assert!(close(rotate(q, [0.0, 1.0, 0.0]), frame.up, 1e-12));
                assert!(close(rotate(q, [1.0, 0.0, 0.0]), frame.right, 1e-12));
            }
        }
    }

    #[test]
    fn nearest_s_on_a_point_on_the_curve_returns_its_s() {
        for closed in [false, true] {
            for interpolation in BOTH {
                let sampler = PathSampler::new(&doc(interpolation, closed, &ZIGZAG));
                let l = sampler.length();
                for metric in METRICS {
                    for k in 0..=40 {
                        let s = l * k as f64 / 40.0;
                        let p = sampler.point_at(s);
                        let global = sampler.nearest_s_global(p, metric);
                        let windowed = sampler.nearest_s(p, s, 2.0, metric);
                        for found in [global, windowed] {
                            let gap = arc_gap(found, s, l, closed);
                            assert!(
                                gap < 1e-6,
                                "{interpolation:?} {metric:?} s={s} found={found}"
                            );
                            assert!(close(sampler.point_at(found), p, 1e-6));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn nearest_s_does_not_jump_branch_on_a_switchback() {
        // Two 10 m legs 0.5 m apart joined by a short turn.
        let hairpin = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 0.0, 0.5],
            [0.0, 0.0, 0.5],
        ];
        let sampler = PathSampler::new(&doc(PathInterpolation::Linear, false, &hairpin));
        // Nearer to the return leg (0.2 m) than to the outbound one (0.3 m).
        let mut hint = 1.0;
        for step in 0..=70 {
            let x = 1.0 + step as f64 * 0.1;
            hint = sampler.nearest_s([x, 0.0, 0.3], hint, 0.4, PathMetric::Full);
            assert!(
                (hint - x).abs() < 1e-9,
                "left the outbound leg at x = {x}: s = {hint}"
            );
        }
        let global = sampler.nearest_s_global([5.0, 0.0, 0.3], PathMetric::Full);
        assert!(
            global > 10.0,
            "the global search takes the nearer return leg: {global}"
        );
    }

    #[test]
    fn nearest_s_wraps_on_a_closed_loop() {
        let square = [
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 0.0],
            [4.0, 0.0, 4.0],
            [0.0, 0.0, 4.0],
        ];
        for interpolation in BOTH {
            let sampler = PathSampler::new(&doc(interpolation, true, &square));
            let l = sampler.length();
            let just_past_start = sampler.point_at(0.1);
            let s = sampler.nearest_s(just_past_start, l - 0.05, 0.5, PathMetric::Full);
            assert!(
                arc_gap(s, 0.1, l, true) < 1e-6 && s < 1.0,
                "{interpolation:?}: {s}"
            );
            let just_before_end = sampler.point_at(l - 0.1);
            let s = sampler.nearest_s(just_before_end, 0.05, 0.5, PathMetric::Full);
            assert!(
                arc_gap(s, l - 0.1, l, true) < 1e-6 && s > l - 1.0,
                "{interpolation:?}: {s}"
            );
        }
    }

    #[test]
    fn nearest_s_ties_take_the_smaller_s() {
        // A U: the query is 1 m from both 4 m legs, 2 m from the bottom.
        let u = [
            [-1.0, 0.0, 2.0],
            [-1.0, 0.0, -2.0],
            [1.0, 0.0, -2.0],
            [1.0, 0.0, 2.0],
        ];
        let sampler = PathSampler::new(&doc(PathInterpolation::Linear, false, &u));
        for metric in METRICS {
            assert_eq!(sampler.nearest_s_global([0.0, 0.0, 0.0], metric), 2.0);
        }
    }

    #[test]
    fn horizontal_metric_ignores_height_on_a_crossover() {
        // Two diagonal legs cross at the XZ origin, the second 3 m higher.
        let lane = [
            [-5.0, 0.0, -5.0],
            [5.0, 0.0, 5.0],
            [5.0, 3.0, -5.0],
            [-5.0, 3.0, 5.0],
        ];
        let sampler = PathSampler::new(&doc(PathInterpolation::Linear, false, &lane));
        let leg_a = distance(lane[0], lane[1]);
        let cross_a = leg_a * 0.5;
        let cross_b = leg_a + distance(lane[1], lane[2]) + distance(lane[2], lane[3]) * 0.5;
        let h = PathMetric::Horizontal;
        let p = [0.0, 1.0, 0.0];
        assert!((sampler.nearest_s(p, cross_a, 1.0, h) - cross_a).abs() < 1e-9);
        assert!((sampler.nearest_s(p, cross_b, 1.0, h) - cross_b).abs() < 1e-9);
        assert!((sampler.nearest_s_global([0.0, 0.0, 0.0], h) - cross_a).abs() < 1e-9);
        assert!((sampler.nearest_s_global([0.0, 3.0, 0.0], h) - cross_b).abs() < 1e-9);
    }
}
