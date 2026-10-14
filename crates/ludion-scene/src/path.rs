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
//! joins the last point back to the first.
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

const PATH_KEYS: [&str; 6] = [
    "format",
    "kind",
    "name",
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
/// points (3 when closed), every component finite and inside the f32 range, and no two neighbouring
/// points at the same position (the last and the first included when closed).
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

// --- Canonical writer ---------------------------------------------------------------------------

/// Writes `doc` in canonical form: `format`, `kind`, `name` (optional), `interpolation`, `closed`,
/// `points` — the last three always spelled.
pub(crate) fn emit(doc: &PathDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: path\n");
    if let Some(name) = &doc.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
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

/// One arc-length table sample: the curve position at `local_t` of `segment`, `s` metres along.
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
                    s += distance(previous.position, position);
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
            s += distance(previous.position, position);
        }
        table.push(TableEntry {
            s,
            segment: last,
            local_t: 1.0,
            position,
        });
        sampler.length = s;
        sampler.table = table;
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
        let (j, frac) = self.locate(s);
        let (a, b) = (&self.table[j], &self.table[j + 1]);
        let u = a.u() * (1.0 - frac) + b.u() * frac;
        let last_segment = self.segment_count() - 1;
        let segment = (u.floor().max(0.0) as usize).min(last_segment);
        self.evaluate(segment, u - segment as f64)
    }

    /// The unit direction of travel at `s`: the bracketing table chord, so it is piecewise constant
    /// across a segment's sixteenth-parts.
    pub fn tangent_at(&self, s: f64) -> [f64; 3] {
        let (j, _) = self.locate(s);
        let (a, b) = (self.table[j].position, self.table[j + 1].position);
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        [d[0] / len, d[1] / len, d[2] / len]
    }

    fn segment_count(&self) -> usize {
        if self.closed {
            self.points.len()
        } else {
            self.points.len() - 1
        }
    }

    /// The table index `j` whose chord `[j, j + 1]` brackets `s`, and how far along it `s` sits.
    fn locate(&self, s: f64) -> (usize, f64) {
        let s = if self.closed {
            s.rem_euclid(self.length)
        } else {
            s.clamp(0.0, self.length)
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(interpolation: PathInterpolation, closed: bool, points: &[[f64; 3]]) -> PathDocument {
        PathDocument {
            name: None,
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
    fn two_builds_are_bitwise_identical() {
        let source = doc(PathInterpolation::CatmullRom, true, &ZIGZAG);
        let (a, b) = (PathSampler::new(&source), PathSampler::new(&source));
        assert_eq!(a.length().to_bits(), b.length().to_bits());
        for (x, y) in a.table.iter().zip(&b.table) {
            assert_eq!(x.s.to_bits(), y.s.to_bits());
            for c in 0..3 {
                assert_eq!(x.position[c].to_bits(), y.position[c].to_bits());
            }
        }
        for k in 0..50 {
            let s = k as f64 * 0.7;
            assert_eq!(
                a.point_at(s).map(f64::to_bits),
                b.point_at(s).map(f64::to_bits)
            );
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
}
