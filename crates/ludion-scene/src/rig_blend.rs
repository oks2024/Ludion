//! Named blend spaces, shared by the document families that bind clips to a rig: a skinned
//! character's `blends:` and an assembly's (`Plan/animation-layers.md` decisions 13-19 and 31).
//!
//! The structure and consistency codes differ per family, so each function takes them as
//! parameters; the message text is the same for both. The point check takes the *effective events*
//! of each bound clip as an input: a character passes its overlay-else-own rule, an assembly a
//! clip's own `events`.

use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::suggest;

use crate::animation::{AnimationDocument, AnimationEvent};
use crate::blend_space::{MAX_BLEND_POINTS, triangulate};
use crate::envelope;
use crate::fields::parse_number;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, NodeValue, pointer_token};

const BLEND_KEYS: [&str; 2] = ["points", "sync"];
const BLEND_POINT_KEYS: [&str; 2] = ["clip", "at"];

/// Where a blend point sits: a number for a 1D blend, `[x, z]` for a 2D one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlendAt {
    /// A 1D position, driven by `Animation.params[0]`.
    One(f64),
    /// A 2D position, driven by `Animation.params[0..2]`.
    Two([f64; 2]),
}

/// One blend point: a bound clip at a position.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendPoint {
    /// The short name of a clip binding.
    pub clip: String,
    /// The point's position; every point of a blend has the same dimension.
    pub at: BlendAt,
}

/// A named blend space (`Plan/animation-layers.md` decisions 13-19): clips at positions, weighted
/// by `Animation.params` and advanced on one shared phase.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendSpace {
    /// The short name `Animation.clip` selects by.
    pub name: String,
    /// The points, in authored order (2 to 16).
    pub points: Vec<BlendPoint>,
    /// The event every point clip aligns its phase on, when the clips are phase-synced.
    pub sync: Option<String>,
}

// --- Parsing --------------------------------------------------------------------------------

/// Parses `blends`: a mapping of blend name to blend entry, returned sorted by name.
pub(crate) fn parse_blends(
    entry: &MapEntry,
    structure: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<BlendSpace> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure,
                format!(
                    "`blends` must be a mapping of blend name to blend entry, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/blends")
            .with_help(
                "write one entry per blend:\n  blends:\n    locomotion:\n      sync: foot_l\n      points:\n        - clip: idle\n          at: 0\n        - clip: run\n          at: 4",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for sub in entries {
        let pointer = format!("/blends/{}", pointer_token(&sub.key));
        if let Some(blend) = parse_blend_entry(sub, &pointer, structure, diagnostics) {
            out.push(blend);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parses one blend entry: a mapping with a required `points` (2 to 16, one dimension) and an
/// optional `sync`.
fn parse_blend_entry(
    sub: &MapEntry,
    pointer: &str,
    structure: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<BlendSpace> {
    let NodeValue::Mapping(fields) = &sub.node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure,
                format!(
                    "a blend entry must be a mapping with `points`, found {} (line {})",
                    sub.node.value.type_phrase(),
                    sub.node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write the entry under the blend's name:\n    locomotion:\n      points:\n        - clip: idle\n          at: 0"),
        );
        return None;
    };
    let mut points: Option<Vec<BlendPoint>> = None;
    let mut sync = None;
    let mut failed = false;
    for field in fields {
        let field_pointer = format!("{pointer}/{}", field.key);
        match field.key.as_str() {
            "points" => {
                let before = diagnostics.len();
                points = parse_blend_points(&field.node, &field_pointer, structure, diagnostics);
                failed |= diagnostics.len() != before;
            }
            "sync" => {
                let before = diagnostics.len();
                sync = envelope::expect_string(
                    &field.node,
                    &field_pointer,
                    "a blend's `sync`",
                    structure,
                    diagnostics,
                );
                failed |= diagnostics.len() != before;
            }
            unknown => {
                failed = true;
                diagnostics.push(envelope::unknown_key(
                    unknown,
                    field,
                    pointer,
                    "a blend entry",
                    BLEND_KEYS,
                    structure,
                ));
            }
        }
    }
    let Some(points) = points else {
        if !failed {
            diagnostics.push(
                Diagnostic::new(structure, "a blend entry is missing its required `points`")
                    .with_path(pointer.to_string())
                    .with_help("add `points`: a block list of `clip` and `at` entries"),
            );
        }
        return None;
    };
    Some(BlendSpace {
        name: sub.key.clone(),
        points,
        sync,
    })
}

/// Parses a blend's point list: 2 to 16 entries, all the same dimension.
fn parse_blend_points(
    node: &yaml::Node,
    pointer: &str,
    structure: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<BlendPoint>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure,
                format!(
                    "`points` must be a sequence of `{{clip, at}}` entries, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write one `- clip: name` entry with an `at:` per point"),
        );
        return None;
    };
    if items.len() < 2 || items.len() > MAX_BLEND_POINTS {
        diagnostics.push(
            Diagnostic::new(
                structure,
                format!(
                    "a blend needs between 2 and {MAX_BLEND_POINTS} points, found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help("a blend of one clip is just that clip; split a larger space in two"),
        );
        return None;
    }
    let before = diagnostics.len();
    let mut out: Vec<BlendPoint> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_pointer = format!("{pointer}/{index}");
        let Some(point) = parse_blend_point(item, &item_pointer, structure, diagnostics) else {
            continue;
        };
        if let Some(first) = out.first()
            && std::mem::discriminant(&first.at) != std::mem::discriminant(&point.at)
        {
            diagnostics.push(
                Diagnostic::new(
                    structure,
                    "a blend mixes 1D (`at: 1`) and 2D (`at: [x, z]`) points",
                )
                .with_path(format!("{item_pointer}/at"))
                .with_help("give every point of one blend the same dimension"),
            );
            continue;
        }
        out.push(point);
    }
    (diagnostics.len() == before).then_some(out)
}

/// Parses one blend point: a mapping with `clip` and `at`.
fn parse_blend_point(
    node: &yaml::Node,
    pointer: &str,
    structure: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<BlendPoint> {
    let NodeValue::Mapping(fields) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure,
                format!(
                    "a blend point must be a mapping with `clip` and `at`, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write `- clip: run` and, indented under it, `at: 4`"),
        );
        return None;
    };
    let before = diagnostics.len();
    let mut clip = None;
    let mut at = None;
    for field in fields {
        let field_pointer = format!("{pointer}/{}", field.key);
        match field.key.as_str() {
            "clip" => {
                clip = envelope::expect_string(
                    &field.node,
                    &field_pointer,
                    "a blend point's `clip`",
                    structure,
                    diagnostics,
                );
            }
            "at" => at = parse_blend_at(&field.node, &field_pointer, structure, diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                field,
                pointer,
                "a blend point",
                BLEND_POINT_KEYS,
                structure,
            )),
        }
    }
    if diagnostics.len() == before && (clip.is_none() || at.is_none()) {
        diagnostics.push(
            Diagnostic::new(structure, "a blend point needs both `clip` and `at`")
                .with_path(pointer.to_string())
                .with_help("write `- clip: run` and, indented under it, `at: 4`"),
        );
    }
    Some(BlendPoint {
        clip: clip?,
        at: at?,
    })
}

/// Parses `at`: a finite number (1D) or a block list of two finite numbers (2D).
fn parse_blend_at(
    node: &yaml::Node,
    pointer: &str,
    structure: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<BlendAt> {
    let finite = |value: f64, pointer: &str, diagnostics: &mut Vec<Diagnostic>| {
        if value.is_finite() {
            return Some(value);
        }
        diagnostics.push(
            Diagnostic::new(
                structure,
                format!("a blend position must be finite, found {value}"),
            )
            .with_path(pointer.to_string())
            .with_help("give `at` plain finite numbers"),
        );
        None
    };
    match &node.value {
        NodeValue::Sequence(items) if items.len() == 2 => {
            let mut xz = [0.0; 2];
            let before = diagnostics.len();
            for (index, item) in items.iter().enumerate() {
                let item_pointer = format!("{pointer}/{index}");
                if let Some(value) = parse_number(
                    item,
                    &item_pointer,
                    "a blend position",
                    structure,
                    diagnostics,
                ) && let Some(value) = finite(value, &item_pointer, diagnostics)
                {
                    xz[index] = value;
                }
            }
            (diagnostics.len() == before).then_some(BlendAt::Two(xz))
        }
        NodeValue::Sequence(items) => {
            diagnostics.push(
                Diagnostic::new(
                    structure,
                    format!(
                        "a 2D `at` takes exactly two numbers `[x, z]`, found {} (line {})",
                        items.len(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help("write a block list with the x then the z position"),
            );
            None
        }
        _ => {
            let value = parse_number(node, pointer, "a blend position", structure, diagnostics)?;
            finite(value, pointer, diagnostics).map(BlendAt::One)
        }
    }
}

// --- Validation -----------------------------------------------------------------------------

/// The consistency rules for blends: a non-blank name that no clip binding (`bindings`, the bound
/// short names) shares, no two points at one position, and a 2D blend whose points span an area.
pub(crate) fn validate_blends(
    blends: &[BlendSpace],
    bindings: &[&str],
    consistency: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for blend in blends {
        let pointer = format!("/blends/{}", pointer_token(&blend.name));
        if blend.name.trim().is_empty() {
            diagnostics.push(
                Diagnostic::new(consistency, "a blend has an empty name")
                    .with_path(pointer.clone())
                    .with_help(
                        "name the blend the way `Animation.clip` selects it, e.g. `locomotion`",
                    ),
            );
        }
        if bindings.contains(&blend.name.as_str()) {
            diagnostics.push(
                Diagnostic::new(
                    consistency,
                    format!(
                        "the blend `{}` has the name of a clip binding; `Animation.clip` resolves \
                         both",
                        blend.name
                    ),
                )
                .with_path(pointer.clone())
                .with_help("rename the blend or the clip binding"),
            );
        }
        for (index, point) in blend.points.iter().enumerate() {
            if blend.points[..index].iter().any(|p| p.at == point.at) {
                diagnostics.push(
                    Diagnostic::new(
                        consistency,
                        format!(
                            "the blend `{}` has two points at the same position",
                            blend.name
                        ),
                    )
                    .with_path(format!("{pointer}/points/{index}/at"))
                    .with_help("give each point its own position"),
                );
            }
        }
        let flat: Vec<[f64; 2]> = blend
            .points
            .iter()
            .filter_map(|point| match point.at {
                BlendAt::Two(at) => Some(at),
                BlendAt::One(_) => None,
            })
            .collect();
        if !flat.is_empty() && triangulate(&flat).is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    consistency,
                    format!("the points of the 2D blend `{}` are collinear", blend.name),
                )
                .with_path(format!("{pointer}/points"))
                .with_help(
                    "a 2D blend needs points that span an area; use a 1D `at: <number>` for a line",
                ),
            );
        }
    }
}

/// A blend's point clips must be bound, looping, and (with `sync`) carry the sync event.
///
/// `bindings` are the bound short names, index-aligned with `clips`; `events(i)` is the effective
/// event list of binding `i`.
pub(crate) fn check_blend_points<'a>(
    blends: &[BlendSpace],
    bindings: &[&str],
    clips: &[AnimationDocument],
    events: impl Fn(usize) -> &'a [AnimationEvent],
    consistency: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for blend in blends {
        let pointer = format!("/blends/{}", pointer_token(&blend.name));
        for (index, point) in blend.points.iter().enumerate() {
            let point_pointer = format!("{pointer}/points/{index}/clip");
            let Some(slot) = bindings.iter().position(|name| *name == point.clip) else {
                let help = match suggest::nearest(&point.clip, bindings.iter().copied()) {
                    Some(candidate) => format!("nearest bound clip is `{candidate}`"),
                    None => "a blend point names a clip bound under `clips`".to_string(),
                };
                diagnostics.push(
                    Diagnostic::new(
                        consistency,
                        format!(
                            "the blend `{}` names the clip `{}`, which is not bound under `clips`",
                            blend.name, point.clip
                        ),
                    )
                    .with_path(point_pointer)
                    .with_help(help),
                );
                continue;
            };
            let clip = &clips[slot];
            if !clip.looped {
                diagnostics.push(
                    Diagnostic::new(
                        consistency,
                        format!(
                            "the blend `{}` uses `{}`, which does not loop; a blend advances every \
                             clip on one repeating phase",
                            blend.name, point.clip
                        ),
                    )
                    .with_path(point_pointer.clone())
                    .with_help(
                        "set `loop: true` on the clip document, or leave the clip out of the blend",
                    ),
                );
            }
            if let Some(sync) = &blend.sync {
                let events = events(slot);
                if !events.iter().any(|event| &event.name == sync) {
                    let help = match suggest::nearest(
                        sync,
                        events.iter().map(|event| event.name.as_str()),
                    ) {
                        Some(candidate) => format!("nearest event is `{candidate}`"),
                        None => format!(
                            "add a `{sync}` event to the clip, or to the binding's `events` overlay"
                        ),
                    };
                    diagnostics.push(
                        Diagnostic::new(
                            consistency,
                            format!(
                                "the blend `{}` syncs on `{sync}`, which the clip `{}` has no event \
                                 for",
                                blend.name, point.clip
                            ),
                        )
                        .with_path(format!("{pointer}/sync"))
                        .with_help(help),
                    );
                }
            }
        }
    }
}

// --- Canonical writer -----------------------------------------------------------------------

/// Writes the `blends:` block (sorted by name), or nothing when `blends` is empty.
pub(crate) fn emit_blends(out: &mut String, blends: &[BlendSpace]) {
    if !blends.is_empty() {
        out.push_str("blends:\n");
        let mut blends: Vec<&BlendSpace> = blends.iter().collect();
        blends.sort_by(|a, b| a.name.cmp(&b.name));
        for blend in blends {
            out.push_str("  ");
            out.push_str(&scalar_string(&blend.name));
            out.push_str(":\n");
            if let Some(sync) = &blend.sync {
                out.push_str("    sync: ");
                out.push_str(&scalar_string(sync));
                out.push('\n');
            }
            out.push_str("    points:\n");
            for point in &blend.points {
                out.push_str("      - clip: ");
                out.push_str(&scalar_string(&point.clip));
                match point.at {
                    BlendAt::One(at) => {
                        out.push_str("\n        at: ");
                        out.push_str(&format_float(at));
                        out.push('\n');
                    }
                    BlendAt::Two([x, z]) => {
                        out.push_str("\n        at:\n          - ");
                        out.push_str(&format_float(x));
                        out.push_str("\n          - ");
                        out.push_str(&format_float(z));
                        out.push('\n');
                    }
                }
            }
        }
    }
}
