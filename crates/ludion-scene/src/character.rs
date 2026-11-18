//! Character documents: `kind: character` — binds a skinned mesh to named animation clips
//! (`Plan/animation-clips.md` decisions 10, 11 and 12).
//!
//! A character is hand-authored (the clips it names are derived, the binding is not). It names the
//! skinned mesh document, then binds each clip by short name with an optional playback `speed` and,
//! for an imported clip that carries no events of its own, an `events` overlay.
//!
//! ```yaml
//! format: 1
//! kind: character
//! name: goblin
//! mesh: goblin.mesh.yaml
//! clips:
//!   run:
//!     clip: clips/run.anim.yaml
//!     speed: 1.5
//!     events:
//!       - t: 0.2
//!         name: footstep
//!   idle:
//!     clip: clips/idle.anim.yaml
//! ```
//!
//! **This module stays IO-free.** Paths are stored verbatim, doc-relative. The rules that need a
//! resolved clip or the mesh's joint names live in [`check_clips`], which the caller feeds
//! already-parsed documents to, the way [`crate::assembly::check_parts`] takes resolved parts.
//!
//! Structure (`L1199`) and data consistency (`L1200`) split as in the assembly family; overlay event
//! data errors reuse `L1198` because they are the same fact a clip's own events raise.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::paths::sibling_refusal;
use ludion_core::suggest;
use serde_json::Value;

use crate::animation::{
    AnimationDocument, AnimationEvent, emit_events, parse_events, validate_event_list,
};
use crate::blend_space::{MAX_BLEND_POINTS, triangulate};
use crate::envelope;
use crate::fields::parse_number;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, NodeValue, pointer_token};

/// The character document format version this crate reads and writes.
pub const CHARACTER_FORMAT_VERSION: u64 = 1;

const CHARACTER_KEYS: [&str; 7] = ["format", "kind", "name", "mesh", "clips", "masks", "blends"];
const CLIP_KEYS: [&str; 3] = ["clip", "speed", "events"];
const MASK_KEYS: [&str; 3] = ["joints", "descendants", "weights"];
const BLEND_KEYS: [&str; 2] = ["points", "sync"];
const BLEND_POINT_KEYS: [&str; 2] = ["clip", "at"];

/// One clip binding: the short name game code selects by, the clip document's path, and the
/// per-character playback overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterClip {
    /// The short name (`run`, `idle`) game code selects the clip by.
    pub name: String,
    /// The `kind: animation` document's path, relative to the character file.
    pub clip: String,
    /// The playback speed multiplier; finite and positive on a `validate`-clean document.
    pub speed: f64,
    /// Events for a clip that has none of its own, time-strictly-ascending as authored. Empty means
    /// absent.
    pub events: Vec<AnimationEvent>,
}

/// A named bone mask: which joints an `AnimationLayers` layer touches and how strongly
/// (`Plan/animation-layers.md` decision 1).
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterMask {
    /// The short name a layer's `mask` selects by.
    pub name: String,
    /// The listed joints' names, in authored order.
    pub joints: Vec<String>,
    /// Whether an unlisted joint inherits its nearest listed ancestor's weight.
    pub descendants: bool,
    /// One weight in `[0, 1]` per listed joint; empty means every listed joint weighs 1.0.
    pub weights: Vec<f64>,
}

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
pub struct CharacterBlend {
    /// The short name `Animation.clip` selects by.
    pub name: String,
    /// The points, in authored order (2 to 16).
    pub points: Vec<BlendPoint>,
    /// The event every point clip aligns its phase on, when the clips are phase-synced.
    pub sync: Option<String>,
}

/// A parsed, structurally valid character document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CharacterDocument {
    /// Optional human-readable character name.
    pub name: Option<String>,
    /// The skinned mesh document's path, relative to the character file.
    pub mesh: String,
    /// Clip bindings, sorted by short name (the canonical order `emit` writes).
    pub clips: Vec<CharacterClip>,
    /// Bone masks, sorted by name (the canonical order `emit` writes).
    pub masks: Vec<CharacterMask>,
    /// Blend spaces, sorted by name (the canonical order `emit` writes).
    pub blends: Vec<CharacterBlend>,
}

// --- Parsing (`L1199`) --------------------------------------------------------------------------

/// Parses strict-subset YAML into a [`CharacterDocument`], checking structure/shape only (`L1199`).
pub(crate) fn parse(source: &str) -> Result<CharacterDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                "the character document is empty; a character needs at least `format: 1`, \
                 `kind: character` and `mesh`",
            )
            .with_path("")
            .with_help(
                "a minimal character is:\n  format: 1\n  kind: character\n  mesh: goblin.mesh.yaml",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                format!(
                    "the character document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: character`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = CharacterDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut mesh_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    CHARACTER_FORMAT_VERSION,
                    "the character document",
                    "character",
                    "character",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    "character",
                    "a character document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the character name",
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "mesh" => {
                mesh_seen = true;
                if let Some(mesh) = envelope::expect_string(
                    &entry.node,
                    "/mesh",
                    "`mesh`",
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                ) {
                    doc.mesh = mesh;
                }
            }
            "clips" => {
                doc.clips = parse_clips(entry, &mut diagnostics);
            }
            "masks" => {
                doc.masks = parse_masks(entry, &mut diagnostics);
            }
            "blends" => {
                doc.blends = parse_blends(entry, &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a character document",
                CHARACTER_KEYS,
                code::CHARACTER_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key("format", "add `format: 1` as the first line"));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: character`"));
    }
    if !mesh_seen {
        diagnostics.push(missing_key(
            "mesh",
            "add `mesh`: the skinned mesh document's path, relative to this file",
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
        code::CHARACTER_DOCUMENT_STRUCTURE,
        "character document",
    )
}

/// Parses `clips`: a mapping of short name to clip entry, returned sorted by short name (the
/// canonical order [`emit`] writes). A duplicate short name is already rejected as `L1007` by the
/// shared YAML layer.
fn parse_clips(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<CharacterClip> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                format!(
                    "`clips` must be a mapping of clip name to clip entry, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/clips")
            .with_help(
                "write one entry per clip:\n  clips:\n    run:\n      clip: clips/run.anim.yaml",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for sub in entries {
        let pointer = format!("/clips/{}", pointer_token(&sub.key));
        if let Some(clip) = parse_clip_entry(sub, &pointer, diagnostics) {
            out.push(clip);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parses one clip entry: a mapping with a required `clip` and optional `speed`/`events`.
fn parse_clip_entry(
    sub: &MapEntry,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<CharacterClip> {
    let NodeValue::Mapping(fields) = &sub.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                format!(
                    "a clip entry must be a mapping with `clip`, found {} (line {})",
                    sub.node.value.type_phrase(),
                    sub.node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write the entry under the clip's short name:\n    run:\n      clip: clips/run.anim.yaml"),
        );
        return None;
    };
    let mut clip = None;
    let mut speed = 1.0;
    let mut events = Vec::new();
    for field in fields {
        let field_pointer = format!("{pointer}/{}", field.key);
        match field.key.as_str() {
            "clip" => {
                clip = envelope::expect_string(
                    &field.node,
                    &field_pointer,
                    "a clip's `clip`",
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "speed" => {
                if let Some(value) = parse_number(
                    &field.node,
                    &field_pointer,
                    "`speed`",
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    diagnostics,
                ) {
                    speed = value;
                }
            }
            "events" => {
                events = parse_events(
                    &field.node,
                    &field_pointer,
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
                if events.is_empty() && matches!(field.node.value, NodeValue::Sequence(_)) {
                    diagnostics.push(
                        Diagnostic::new(
                            code::ANIMATION_EVENT_INVALID,
                            "`events` is declared but has zero entries",
                        )
                        .with_path(field_pointer)
                        .with_help(
                            "add at least one `{t, name}` event, or remove the `events` key",
                        ),
                    );
                }
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                field,
                pointer,
                "a clip entry",
                CLIP_KEYS,
                code::CHARACTER_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if clip.is_none() {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                "a clip entry is missing its required `clip`",
            )
            .with_path(pointer.to_string())
            .with_help("add `clip`: the animation document's path, relative to this file"),
        );
    }
    Some(CharacterClip {
        name: sub.key.clone(),
        clip: clip?,
        speed,
        events,
    })
}

/// Parses `masks`: a mapping of mask name to mask entry, returned sorted by name.
fn parse_masks(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<CharacterMask> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                format!(
                    "`masks` must be a mapping of mask name to mask entry, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/masks")
            .with_help(
                "write one entry per mask:\n  masks:\n    upper_body:\n      joints:\n        - torso\n      descendants: true",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for sub in entries {
        let pointer = format!("/masks/{}", pointer_token(&sub.key));
        if let Some(mask) = parse_mask_entry(sub, &pointer, diagnostics) {
            out.push(mask);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parses one mask entry: a mapping with a required non-empty `joints` and optional
/// `descendants`/`weights`.
fn parse_mask_entry(
    sub: &MapEntry,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<CharacterMask> {
    let NodeValue::Mapping(fields) = &sub.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                format!(
                    "a mask entry must be a mapping with `joints`, found {} (line {})",
                    sub.node.value.type_phrase(),
                    sub.node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(
                "write the entry under the mask's name:\n    upper_body:\n      joints:\n        - torso",
            ),
        );
        return None;
    };
    let mut joints: Option<Vec<String>> = None;
    let mut descendants = false;
    let mut weights: Option<Vec<f64>> = None;
    let mut failed = false;
    for field in fields {
        let field_pointer = format!("{pointer}/{}", field.key);
        match field.key.as_str() {
            "joints" => {
                let before = diagnostics.len();
                joints = parse_joint_names(&field.node, &field_pointer, diagnostics);
                failed |= diagnostics.len() != before;
            }
            "descendants" => match &field.node.value {
                NodeValue::Scalar(Value::Bool(value)) => descendants = *value,
                other => {
                    failed = true;
                    diagnostics.push(
                        Diagnostic::new(
                            code::CHARACTER_DOCUMENT_STRUCTURE,
                            format!(
                                "`descendants` must be true or false, found {} (line {})",
                                other.type_phrase(),
                                field.node.line
                            ),
                        )
                        .with_path(field_pointer)
                        .with_help("write `descendants: true` or remove the key"),
                    );
                }
            },
            "weights" => {
                let before = diagnostics.len();
                weights = parse_mask_weights(&field.node, &field_pointer, diagnostics);
                failed |= diagnostics.len() != before;
            }
            unknown => {
                failed = true;
                diagnostics.push(envelope::unknown_key(
                    unknown,
                    field,
                    pointer,
                    "a mask entry",
                    MASK_KEYS,
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                ));
            }
        }
    }
    let Some(joints) = joints else {
        if !failed {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    "a mask entry is missing its required `joints`",
                )
                .with_path(pointer.to_string())
                .with_help("add `joints`: a block list of joint names, one `- name` per line"),
            );
        }
        return None;
    };
    if let Some(weights) = &weights
        && weights.len() != joints.len()
    {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                format!(
                    "`weights` has {} entries but `joints` has {}",
                    weights.len(),
                    joints.len()
                ),
            )
            .with_path(format!("{pointer}/weights"))
            .with_help("give one weight per listed joint, in the same order"),
        );
        return None;
    }
    Some(CharacterMask {
        name: sub.key.clone(),
        joints,
        descendants,
        weights: weights.unwrap_or_default(),
    })
}

/// Parses `blends`: a mapping of blend name to blend entry, returned sorted by name.
fn parse_blends(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<CharacterBlend> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
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
        if let Some(blend) = parse_blend_entry(sub, &pointer, diagnostics) {
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
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<CharacterBlend> {
    let NodeValue::Mapping(fields) = &sub.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
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
                points = parse_blend_points(&field.node, &field_pointer, diagnostics);
                failed |= diagnostics.len() != before;
            }
            "sync" => {
                let before = diagnostics.len();
                sync = envelope::expect_string(
                    &field.node,
                    &field_pointer,
                    "a blend's `sync`",
                    code::CHARACTER_DOCUMENT_STRUCTURE,
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
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                ));
            }
        }
    }
    let Some(points) = points else {
        if !failed {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    "a blend entry is missing its required `points`",
                )
                .with_path(pointer.to_string())
                .with_help("add `points`: a block list of `clip` and `at` entries"),
            );
        }
        return None;
    };
    Some(CharacterBlend {
        name: sub.key.clone(),
        points,
        sync,
    })
}

/// Parses a blend's point list: 2 to 16 entries, all the same dimension.
fn parse_blend_points(
    node: &yaml::Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<BlendPoint>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
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
                code::CHARACTER_DOCUMENT_STRUCTURE,
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
        let Some(point) = parse_blend_point(item, &item_pointer, diagnostics) else {
            continue;
        };
        if let Some(first) = out.first()
            && std::mem::discriminant(&first.at) != std::mem::discriminant(&point.at)
        {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DOCUMENT_STRUCTURE,
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
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<BlendPoint> {
    let NodeValue::Mapping(fields) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
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
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "at" => at = parse_blend_at(&field.node, &field_pointer, diagnostics),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                field,
                pointer,
                "a blend point",
                BLEND_POINT_KEYS,
                code::CHARACTER_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if diagnostics.len() == before && (clip.is_none() || at.is_none()) {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                "a blend point needs both `clip` and `at`",
            )
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
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<BlendAt> {
    let finite = |value: f64, pointer: &str, diagnostics: &mut Vec<Diagnostic>| {
        if value.is_finite() {
            return Some(value);
        }
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
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
                    code::CHARACTER_DOCUMENT_STRUCTURE,
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
                    code::CHARACTER_DOCUMENT_STRUCTURE,
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
            let value = parse_number(
                node,
                pointer,
                "a blend position",
                code::CHARACTER_DOCUMENT_STRUCTURE,
                diagnostics,
            )?;
            finite(value, pointer, diagnostics).map(BlendAt::One)
        }
    }
}

/// Parses a non-empty sequence of unique joint-name strings.
fn parse_joint_names(
    node: &yaml::Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<String>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                format!(
                    "`joints` must be a sequence of joint names, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write one `- joint` line per joint under `joints:`"),
        );
        return None;
    };
    if items.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                "`joints` is declared but has zero entries",
            )
            .with_path(pointer.to_string())
            .with_help("list at least one joint, or remove the mask"),
        );
        return None;
    }
    let before = diagnostics.len();
    let mut names: Vec<String> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_pointer = format!("{pointer}/{index}");
        let Some(name) = envelope::expect_string(
            item,
            &item_pointer,
            "a joint name",
            code::CHARACTER_DOCUMENT_STRUCTURE,
            diagnostics,
        ) else {
            continue;
        };
        if names.contains(&name) {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    format!("the joint `{name}` is listed twice in one mask"),
                )
                .with_path(item_pointer)
                .with_help("list each joint once"),
            );
        }
        names.push(name);
    }
    (diagnostics.len() == before).then_some(names)
}

/// Parses a sequence of finite numbers in `[0, 1]`.
fn parse_mask_weights(
    node: &yaml::Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<f64>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DOCUMENT_STRUCTURE,
                format!(
                    "`weights` must be a sequence of numbers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write one `- 0.5` line per joint under `weights:`"),
        );
        return None;
    };
    let before = diagnostics.len();
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_pointer = format!("{pointer}/{index}");
        let Some(weight) = parse_number(
            item,
            &item_pointer,
            "a weight",
            code::CHARACTER_DOCUMENT_STRUCTURE,
            diagnostics,
        ) else {
            continue;
        };
        if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DOCUMENT_STRUCTURE,
                    format!("a mask weight must be finite and within [0, 1], found {weight}"),
                )
                .with_path(item_pointer)
                .with_help("0 leaves the joint on the base pose, 1 takes the layer fully"),
            );
        }
        out.push(weight);
    }
    (diagnostics.len() == before).then_some(out)
}

/// Resolves a mask to one weight per skin joint, in skin order. `joints` is `(name, parent index)`
/// per joint, parent-first. A listed joint takes its weight (1.0 without `weights`); with
/// `descendants`, an unlisted joint inherits its nearest listed ancestor's weight; every other joint
/// is 0.
#[must_use]
pub(crate) fn resolve_mask(mask: &CharacterMask, joints: &[(&str, Option<u32>)]) -> Vec<f32> {
    let mut resolved: Vec<Option<f32>> = Vec::with_capacity(joints.len());
    for (name, parent) in joints {
        let listed = mask
            .joints
            .iter()
            .position(|joint| joint == name)
            .map(|i| mask.weights.get(i).map_or(1.0, |w| *w as f32));
        let inherited = if mask.descendants {
            parent.and_then(|p| resolved.get(p as usize).copied().flatten())
        } else {
            None
        };
        resolved.push(listed.or(inherited));
    }
    resolved.into_iter().map(|w| w.unwrap_or(0.0)).collect()
}

// --- Validation (`L1200`, `L1198`) --------------------------------------------------------------

/// Validates data-consistency invariants of an already-parsed [`CharacterDocument`] (`L1200`): a
/// non-blank `mesh`, non-blank clip names and paths (none absolute, drive-prefixed or a device;
/// `..` is fine), and a finite positive `speed`; overlay events have a finite non-negative `t`,
/// strictly increasing, and non-blank names (`L1198`). The duration bound needs the clip and lives
/// in [`check_clips`].
pub(crate) fn validate(doc: &CharacterDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if doc.mesh.trim().is_empty() {
        diagnostics.push(
            Diagnostic::new(code::CHARACTER_DATA_CONSISTENCY, "`mesh` is empty")
                .with_path("/mesh")
                .with_help("point `mesh` at the skinned mesh document, relative to this file"),
        );
    } else if let Some(refusal) = sibling_refusal(&doc.mesh) {
        diagnostics.push(
            Diagnostic::new(
                code::CHARACTER_DATA_CONSISTENCY,
                format!("`mesh` (`{}`) {}", doc.mesh, refusal.describe()),
            )
            .with_path("/mesh")
            .with_help("point `mesh` at the skinned mesh document, relative to this file"),
        );
    }
    for mask in &doc.masks {
        if mask.name.trim().is_empty() {
            diagnostics.push(
                Diagnostic::new(code::CHARACTER_DATA_CONSISTENCY, "a mask has an empty name")
                    .with_path(format!("/masks/{}", pointer_token(&mask.name)))
                    .with_help("name the mask the way a layer selects it — `upper_body`"),
            );
        }
    }
    validate_blends(doc, &mut diagnostics);
    for clip in &doc.clips {
        let pointer = format!("/clips/{}", pointer_token(&clip.name));
        if clip.name.trim().is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DATA_CONSISTENCY,
                    "a clip binding has an empty short name",
                )
                .with_path(pointer.clone())
                .with_help("name the clip the way game code selects it — `run`, `idle`"),
            );
        }
        if clip.clip.trim().is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DATA_CONSISTENCY,
                    format!("the clip binding `{}` has an empty `clip` path", clip.name),
                )
                .with_path(format!("{pointer}/clip"))
                .with_help("point `clip` at the animation document, relative to this file"),
            );
        } else if let Some(refusal) = sibling_refusal(&clip.clip) {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DATA_CONSISTENCY,
                    format!("`clip` (`{}`) {}", clip.clip, refusal.describe()),
                )
                .with_path(format!("{pointer}/clip"))
                .with_help("point `clip` at the animation document, relative to this file"),
            );
        }
        if !clip.speed.is_finite() || clip.speed <= 0.0 {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DATA_CONSISTENCY,
                    format!(
                        "the clip binding `{}` has `speed` {}, which must be finite and greater \
                         than zero",
                        clip.name, clip.speed
                    ),
                )
                .with_path(format!("{pointer}/speed"))
                .with_help("`speed` multiplies playback rate; 1.0 is authored speed"),
            );
        }
        validate_event_list(
            &clip.events,
            &format!("{pointer}/events"),
            None,
            false,
            &mut diagnostics,
        );
    }
    diagnostics
}

/// The `L1200` rules for blends: a non-blank name that no clip binding shares, no two points at one
/// position, and a 2D blend whose points span an area.
fn validate_blends(doc: &CharacterDocument, diagnostics: &mut Vec<Diagnostic>) {
    for blend in &doc.blends {
        let pointer = format!("/blends/{}", pointer_token(&blend.name));
        if blend.name.trim().is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DATA_CONSISTENCY,
                    "a blend has an empty name",
                )
                .with_path(pointer.clone())
                .with_help("name the blend the way `Animation.clip` selects it, e.g. `locomotion`"),
            );
        }
        if doc.clips.iter().any(|clip| clip.name == blend.name) {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DATA_CONSISTENCY,
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
                        code::CHARACTER_DATA_CONSISTENCY,
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
                    code::CHARACTER_DATA_CONSISTENCY,
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

/// Checks a character against its resolved clip documents and its mesh's joint names. Pure and
/// IO-free.
///
/// `clips` is aligned by index with [`CharacterDocument::clips`]; `joints` is the character mesh's
/// joint names. Raises `L1200` when the overlay and the clip both carry events, `L1198` when an
/// overlay event's `t` exceeds the clip's `duration` (or sits at it on a looping clip), and `L1200` for every clip track that names a
/// joint the mesh lacks — each pointing into the *character* document.
///
/// # Panics
/// If `clips` is not the same length as [`CharacterDocument::clips`].
pub(crate) fn check_clips(
    doc: &CharacterDocument,
    clips: &[AnimationDocument],
    joints: &[&str],
) -> Vec<Diagnostic> {
    assert_eq!(
        doc.clips.len(),
        clips.len(),
        "`clips` must be aligned by index with the character's clip bindings"
    );
    let mut diagnostics = Vec::new();
    for mask in &doc.masks {
        for (index, joint) in mask.joints.iter().enumerate() {
            if joints.contains(&joint.as_str()) {
                continue;
            }
            let help = match suggest::nearest(joint, joints.iter().copied()) {
                Some(candidate) => format!("nearest joint is `{candidate}`"),
                None => "a mask's joints must be joints of the character's mesh".to_string(),
            };
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DATA_CONSISTENCY,
                    format!(
                        "the mask `{}` lists `{joint}`, which is not a joint of the character's \
                         mesh",
                        mask.name
                    ),
                )
                .with_path(format!(
                    "/masks/{}/joints/{index}",
                    pointer_token(&mask.name)
                ))
                .with_help(help),
            );
        }
    }
    check_blend_points(doc, clips, &mut diagnostics);
    for (binding, clip) in doc.clips.iter().zip(clips) {
        let pointer = format!("/clips/{}", pointer_token(&binding.name));
        if !binding.events.is_empty() && !clip.events.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DATA_CONSISTENCY,
                    format!(
                        "the clip binding `{}` carries `events`, but its clip document has its \
                         own",
                        binding.name
                    ),
                )
                .with_path(format!("{pointer}/events"))
                .with_help(
                    "a clip's events live in one place: remove the overlay `events`, or the ones \
                     in the clip document",
                ),
            );
        }
        for (index, event) in binding.events.iter().enumerate() {
            if event.t.is_finite() && event.t > clip.duration {
                diagnostics.push(
                    Diagnostic::new(
                        code::ANIMATION_EVENT_INVALID,
                        format!(
                            "event `t` = {} is outside the clip's [0, {}] duration",
                            format_float(event.t),
                            format_float(clip.duration)
                        ),
                    )
                    .with_path(format!("{pointer}/events/{index}/t"))
                    .with_help("keep every event's `t` within [0, duration]"),
                );
            } else if clip.looped && event.t == clip.duration {
                diagnostics.push(
                    Diagnostic::new(
                        code::ANIMATION_EVENT_INVALID,
                        format!(
                            "event `t` = {} is at the end of a looping clip, which is the same \
                             instant as its start",
                            format_float(event.t)
                        ),
                    )
                    .with_path(format!("{pointer}/events/{index}/t"))
                    .with_help("a loop's duration is its 0: move the event to `t: 0`"),
                );
            }
        }
        for track in &clip.tracks {
            if joints.contains(&track.target.as_str()) {
                continue;
            }
            let help = match suggest::nearest(&track.target, joints.iter().copied()) {
                Some(candidate) => format!("nearest joint is `{candidate}`"),
                None => "a track's target must be a joint of the character's mesh".to_string(),
            };
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_DATA_CONSISTENCY,
                    format!(
                        "the clip binding `{}` animates `{}`, which is not a joint of the \
                         character's mesh",
                        binding.name, track.target
                    ),
                )
                .with_path(format!("{pointer}/clip"))
                .with_help(help),
            );
        }
    }
    diagnostics
}

/// A blend's point clips must be bound, looping, and (with `sync`) carry the sync event.
fn check_blend_points(
    doc: &CharacterDocument,
    clips: &[AnimationDocument],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for blend in &doc.blends {
        let pointer = format!("/blends/{}", pointer_token(&blend.name));
        for (index, point) in blend.points.iter().enumerate() {
            let point_pointer = format!("{pointer}/points/{index}/clip");
            let Some(slot) = doc.clips.iter().position(|clip| clip.name == point.clip) else {
                let help = match suggest::nearest(
                    &point.clip,
                    doc.clips.iter().map(|clip| clip.name.as_str()),
                ) {
                    Some(candidate) => format!("nearest bound clip is `{candidate}`"),
                    None => "a blend point names a clip bound under `clips`".to_string(),
                };
                diagnostics.push(
                    Diagnostic::new(
                        code::CHARACTER_DATA_CONSISTENCY,
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
            let (binding, clip) = (&doc.clips[slot], &clips[slot]);
            if !clip.looped {
                diagnostics.push(
                    Diagnostic::new(
                        code::CHARACTER_DATA_CONSISTENCY,
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
                let events = if clip.events.is_empty() {
                    &binding.events
                } else {
                    &clip.events
                };
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
                            code::CHARACTER_DATA_CONSISTENCY,
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

// --- Canonical writer ---------------------------------------------------------------------------

/// Writes `doc` in canonical form: `format`, `kind`, `name` (optional), `mesh`, `clips` (sorted by
/// short name, omitted when empty; each `clip`, `speed` only when not 1.0, `events` when present).
pub(crate) fn emit(doc: &CharacterDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: character\n");
    if let Some(name) = &doc.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }
    out.push_str("mesh: ");
    out.push_str(&scalar_string(&doc.mesh));
    out.push('\n');
    if !doc.clips.is_empty() {
        out.push_str("clips:\n");
        let mut clips: Vec<&CharacterClip> = doc.clips.iter().collect();
        clips.sort_by(|a, b| a.name.cmp(&b.name));
        for clip in clips {
            out.push_str("  ");
            out.push_str(&scalar_string(&clip.name));
            out.push_str(":\n    clip: ");
            out.push_str(&scalar_string(&clip.clip));
            out.push('\n');
            if clip.speed != 1.0 {
                out.push_str("    speed: ");
                out.push_str(&format_float(clip.speed));
                out.push('\n');
            }
            emit_events(&mut out, &clip.events, "    ");
        }
    }
    if !doc.masks.is_empty() {
        out.push_str("masks:\n");
        let mut masks: Vec<&CharacterMask> = doc.masks.iter().collect();
        masks.sort_by(|a, b| a.name.cmp(&b.name));
        for mask in masks {
            out.push_str("  ");
            out.push_str(&scalar_string(&mask.name));
            out.push_str(":\n    joints:\n");
            for joint in &mask.joints {
                out.push_str("      - ");
                out.push_str(&scalar_string(joint));
                out.push('\n');
            }
            if mask.descendants {
                out.push_str("    descendants: true\n");
            }
            if !mask.weights.is_empty() {
                out.push_str("    weights:\n");
                for weight in &mask.weights {
                    out.push_str("      - ");
                    out.push_str(&format_float(*weight));
                    out.push('\n');
                }
            }
        }
    }
    if !doc.blends.is_empty() {
        out.push_str("blends:\n");
        let mut blends: Vec<&CharacterBlend> = doc.blends.iter().collect();
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
    out
}
