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

use crate::animation::{
    AnimationDocument, AnimationEvent, emit_events, parse_events, validate_event_list,
};
use crate::envelope;
use crate::fields::parse_number;
use crate::rig_blend::{
    BlendSpace, check_blend_points, emit_blends, parse_blends, validate_blends,
};
use crate::rig_mask::{RigMask, emit_masks, parse_masks, validate_mask_names};
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, NodeValue, pointer_token};

/// The character document format version this crate reads and writes.
pub const CHARACTER_FORMAT_VERSION: u64 = 1;

const CHARACTER_KEYS: [&str; 7] = ["format", "kind", "name", "mesh", "clips", "masks", "blends"];
const CLIP_KEYS: [&str; 3] = ["clip", "speed", "events"];

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
    pub masks: Vec<RigMask>,
    /// Blend spaces, sorted by name (the canonical order `emit` writes).
    pub blends: Vec<BlendSpace>,
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
                doc.masks =
                    parse_masks(entry, code::CHARACTER_DOCUMENT_STRUCTURE, &mut diagnostics);
            }
            "blends" => {
                doc.blends =
                    parse_blends(entry, code::CHARACTER_DOCUMENT_STRUCTURE, &mut diagnostics);
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
    validate_mask_names(
        &doc.masks,
        code::CHARACTER_DATA_CONSISTENCY,
        &mut diagnostics,
    );
    let names: Vec<&str> = doc.clips.iter().map(|clip| clip.name.as_str()).collect();
    validate_blends(
        &doc.blends,
        &names,
        code::CHARACTER_DATA_CONSISTENCY,
        &mut diagnostics,
    );
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
    let names: Vec<&str> = doc.clips.iter().map(|clip| clip.name.as_str()).collect();
    check_blend_points(
        &doc.blends,
        &names,
        clips,
        |slot| {
            if clips[slot].events.is_empty() {
                &doc.clips[slot].events
            } else {
                &clips[slot].events
            }
        },
        code::CHARACTER_DATA_CONSISTENCY,
        &mut diagnostics,
    );
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
    emit_masks(&mut out, &doc.masks);
    emit_blends(&mut out, &doc.blends);
    out
}
