//! Assembly documents: `*.assembly.yaml` — the model of record for a multi-part voxel model
//! (`Plan/voxel-animation.md` decisions 3, 4 and 9).
//!
//! An assembly owns **the one palette** every part's glyphs resolve against, places lean
//! [`crate::part`] files into one assembly-global grid, declares the part hierarchy (pivots,
//! mirroring), and binds animation clips by short name. The rest pose *is* the assembled grid:
//! `size` is that grid's extent, and each part entry's `place` is where its own cell `(0, 0, 0)`
//! lands inside it.
//!
//! ```yaml
//! format: 1
//! kind: assembly
//! name: player
//! size:
//!   - 12
//!   - 24
//!   - 8
//! palette:
//!   S:
//!     color:
//!       - 0.3
//!       - 0.22
//!       - 0.18
//!       - 1.0
//! parts:
//!   - name: torso
//!     file: parts/torso.part.yaml
//!     place:
//!       - 4
//!       - 10
//!       - 2
//!   - name: upper-arm-r
//!     file: parts/arm.part.yaml
//!     parent: torso
//!     place:
//!       - 9
//!       - 14
//!       - 3
//!     pivot:
//!       - 1.0
//!       - 5.0
//!       - 1.0
//!     mirror: x
//! clips:
//!   walk: anims/humanoid-walk.anim.yaml
//! ```
//!
//! **Block-style collections, not flow-style.** `Plan/voxel-animation.md`'s reference sketch writes
//! `place: [4, 10, 2]` and `S: { color: [...] }`, but the crate's strict-YAML subset rejects
//! non-empty flow collections (`L1005`, see [`crate::yaml`]'s module docs) — so the canonical, and
//! only accepted, spelling is one entry per line, as above.
//!
//! **This module stays IO-free.** Part and clip paths are stored verbatim, doc-relative, exactly as
//! [`crate::map::MapDoc::kit`] stores its kit path: resolving and reading them is the caller's job.
//! The checks that need a part's *content* are therefore separate, pure functions the caller feeds
//! already-resolved documents to — [`check_parts`] (the decision-4 unknown-glyph lint and the
//! does-it-fit check) and [`part_voxel`] (the per-part voxel document the engine meshes).
//!
//! This module owns assembly *structure* (shape/type checking, `L1137`) and *data consistency*
//! (palette values, part-name uniqueness, exactly one root, no cycles, in-grid placement; `L1138`)
//! — the `L1113`/`L1114` split, extended to the assembly family. The palette half of both passes is
//! literally the voxel format's (decision 4: an assembly's palette *is* a voxel document's), shared
//! from [`crate::voxel`] with this family's own diagnostic codes rather than copied.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::paths::sibling_refusal;
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::part::PartDocument;
use crate::scalar::{format_float, scalar_string};
use crate::voxel::{PaletteEntry, VoxelDocument, single_glyph};
use crate::yaml::{self, MapEntry, Node, NodeValue, pointer_token};

/// The assembly document format version this crate reads and writes.
pub const ASSEMBLY_FORMAT_VERSION: u64 = 1;

/// The reserved glyph meaning "empty cell" — the [`crate::voxel`] convention, shared so an
/// assembly's palette reads identically to a voxel model's.
const EMPTY_GLYPH: char = '.';

const ASSEMBLY_KEYS: [&str; 8] = [
    "format", "kind", "name", "size", "palette", "parts", "clips", "joints",
];
const PART_ENTRY_KEYS: [&str; 6] = ["name", "file", "parent", "place", "pivot", "mirror"];
const ASSEMBLY_JOINT_KEYS: [&str; 4] = ["kind", "axis", "limits", "break_impulse"];

/// The joint kind names an assembly's `joints:` entry may spell `kind:` as — a flat `kind: hinge`
/// string, unlike the live `ludion_spatial` `Joint` component's externally tagged `kind: {hinge:
/// {...}}`, because this format has no serde and the strict-YAML subset reads one flat mapping per
/// entry.
const JOINT_KIND_NAMES: [&str; 4] = ["hinge", "ball", "slider", "fixed"];

/// The only legal `mirror` value. One axis, because a mirrored limb file is a left/right pair and
/// nothing else has ever been asked for; a second axis would need its own winding proof.
const MIRROR_AXIS: &str = "x";

/// Axis names for the three-component fields, indexed by component — so a diagnostic says "on x"
/// rather than "at index 0".
const AXES: [&str; 3] = ["x", "y", "z"];

/// One placed part of an assembly.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AssemblyPartEntry {
    /// The part's name inside this assembly — unique, and the name animation clips address it by.
    pub name: String,
    /// The part document's path, relative to the assembly file. Stored verbatim; the caller
    /// resolves and parses it (this crate is IO-free).
    pub file: String,
    /// The parent part's name, or `None` for the one root part.
    pub parent: Option<String>,
    /// Where this part's own cell `(0, 0, 0)` sits in the assembly-global grid.
    pub place: [u32; 3],
    /// The rotation pivot in **part-local, unmirrored** voxel coordinates; floats, so a half
    /// reaches the center of an even-width limb. `[0.0, 0.0, 0.0]` (the part-local origin) is the
    /// default, and the canonical writer omits the key for it — one spelling per meaning.
    ///
    /// A mirrored part's mesh is mirrored but this pivot is not: use
    /// [`Self::pivot_in_mesh_frame`] to get the pivot in the frame [`crate::assembly_part_voxel`]
    /// produces.
    pub pivot: [f64; 3],
    /// Whether the part's geometry is mirrored about x — one limb file serving both sides.
    pub mirror_x: bool,
}

impl AssemblyPartEntry {
    /// This part's pivot in the frame [`crate::assembly_part_voxel`] produces: mirrored about the
    /// part's x extent when [`Self::mirror_x`] is set, the stored [`Self::pivot`] otherwise.
    ///
    /// [`crate::assembly_part_voxel`] mirrors the *grid* (it reverses each row string), so a
    /// mirrored part's cell
    /// `x` becomes `part_size_x - 1 - x` and a continuous coordinate `px` becomes
    /// `part_size_x - px`. Consumers call this instead of branching on [`Self::mirror_x`]
    /// themselves, which is the whole reason it exists: the stored pivot and the emitted mesh live
    /// in different frames, and every consumer that forgets it produces a limb that rotates about
    /// the wrong end.
    pub fn pivot_in_mesh_frame(&self, part_size_x: u32) -> [f64; 3] {
        let [px, py, pz] = self.pivot;
        if self.mirror_x {
            [f64::from(part_size_x) - px, py, pz]
        } else {
            [px, py, pz]
        }
    }
}

/// One joint binding: a non-root part joined to its `parent` (`Plan/physics.md` W3's ragdoll
/// authoring, decision 4). Keyed by the child part's name — the entry's `name` field — with the
/// joint kind, its axis/limits (where the kind has them), and an optional break impulse.
///
/// A part with no entry in [`AssemblyDocument::joints`] is implicitly [`AssemblyJointKind::Fixed`]
/// to its parent when a ragdoll switch inserts joints (a lantern rides its hand) — that default lives
/// with the switch, not here, since a plain assembly with no `joints:` at all never runs a switch.
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyJoint {
    /// The child part's name — the key under `joints:`.
    pub part: String,
    /// The constraint kind and its axis/limits.
    pub kind: AssemblyJointKind,
    /// The linear constraint impulse magnitude, in one step, above which the joint breaks (default
    /// unset: the joint never breaks).
    pub break_impulse: Option<f64>,
}

/// An [`AssemblyJoint`]'s constraint kind, its axis (where it has one) and optional limits — the
/// assembly-format twin of `ludion_spatial`'s `JointKind`, in `f64` like every other assembly-format
/// number ([`AssemblyPartEntry::pivot`]'s precedent); the ragdoll switch converts to `f32` when it
/// builds the live `Joint` component. `ludion-scene` does not depend on `ludion-spatial` and must not
/// start to, so the two enums are kept independently rather than shared.
#[derive(Clone, Debug, PartialEq)]
pub enum AssemblyJointKind {
    /// One rotational degree of freedom about `axis`, in the parent's local frame.
    Hinge {
        /// The hinge axis (must be non-zero and finite; default up, `[0.0, 1.0, 0.0]`, omitted by
        /// the canonical writer).
        axis: [f64; 3],
        /// The rotation limit about `axis`, in degrees, as `[min, max]` (default unset: free
        /// rotation).
        limits: Option<[f64; 2]>,
    },
    /// A ball-and-socket joint: free rotation about the anchor, optionally limited per axis.
    Ball {
        /// Per-axis rotation limits, in degrees, as `[[min, max]; 3]` on the parent's local x/y/z
        /// (default unset: free rotation).
        limits: Option<[[f64; 2]; 3]>,
    },
    /// One translational degree of freedom along `axis`, in the parent's local frame.
    Slider {
        /// The slide axis (must be non-zero and finite; default up, `[0.0, 1.0, 0.0]`, omitted by
        /// the canonical writer).
        axis: [f64; 3],
        /// The translation limit along `axis`, in world units, as `[min, max]` (default unset: free
        /// translation).
        limits: Option<[f64; 2]>,
    },
    /// No relative motion at all: the child is welded to its parent at the switch pose.
    Fixed,
}

/// One clip binding: the short name game code selects by, and the clip document's path.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AssemblyClip {
    /// The short name (`walk`, `attack`) — model-agnostic, so a clip is reusable across assemblies
    /// sharing a part-naming convention.
    pub name: String,
    /// The clip document's path, relative to the assembly file. Stored verbatim, like
    /// [`AssemblyPartEntry::file`].
    pub file: String,
}

/// A parsed, structurally valid assembly document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AssemblyDocument {
    /// Optional human-readable model name.
    pub name: Option<String>,
    /// The assembled bounding grid in cells, rest pose: `[x, y, z]`.
    pub size: [u32; 3],
    /// The one palette every part's glyphs resolve against, in sorted-glyph order (the canonical
    /// order `emit` writes) — exactly a [`crate::voxel::VoxelDocument`]'s palette.
    pub palette: Vec<PaletteEntry>,
    /// The placed parts, in **authored** order (never sorted): the hierarchy reads top-down as
    /// written, the way a scene's entities do.
    pub parts: Vec<AssemblyPartEntry>,
    /// Clip bindings, sorted by short name (the canonical order `emit` writes). Empty means the
    /// assembly binds no clips — an authored `clips: {}` and an absent `clips` are the same thing,
    /// and both canonicalize to the key's absence.
    pub clips: Vec<AssemblyClip>,
    /// Joint bindings, sorted by child part name (the canonical order `emit` writes). Empty means
    /// the ragdoll switch fixes every non-root part to its parent — an authored `joints: {}` and an
    /// absent `joints` are the same thing, and both canonicalize to the key's absence.
    pub joints: Vec<AssemblyJoint>,
}

// --- Parsing (`L1137`) --------------------------------------------------------------------------

/// Parses strict-subset YAML into an [`AssemblyDocument`], checking structure/shape only (`L1137`).
pub(crate) fn parse(source: &str) -> Result<AssemblyDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                "the assembly document is empty; an assembly needs at least `format: 1`, \
                 `kind: assembly`, `size`, `palette`, and `parts`",
            )
            .with_path("")
            .with_help(
                "a minimal assembly is:\n  format: 1\n  kind: assembly\n  size:\n    - 1\n    \
                 - 1\n    - 1\n  palette:\n    A:\n      color:\n        - 1.0\n        - 1.0\n   \
                 \x20    - 1.0\n        - 1.0\n  parts:\n    - name: body\n      file: \
                 parts/body.part.yaml\n      place:\n        - 0\n        - 0\n        - 0",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "the assembly document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: assembly`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = AssemblyDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut size_seen = false;
    let mut palette_seen = false;
    let mut parts_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    ASSEMBLY_FORMAT_VERSION,
                    "the assembly document",
                    "assembly",
                    "assembly",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    "assembly",
                    "an assembly document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the assembly name",
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "size" => {
                size_seen = true;
                if let Some(size) = crate::fields::parse_size3(
                    entry,
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                ) {
                    doc.size = size;
                }
            }
            "palette" => {
                palette_seen = true;
                doc.palette = crate::voxel::parse_palette(
                    entry,
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "parts" => {
                parts_seen = true;
                doc.parts = parse_parts(entry, &mut diagnostics);
            }
            "clips" => {
                doc.clips = envelope::parse_clips(
                    entry,
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    "anims/humanoid-walk.anim.yaml",
                    &mut diagnostics,
                );
            }
            "joints" => {
                doc.joints = parse_joints(entry, &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "an assembly document",
                ASSEMBLY_KEYS,
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key("format", "add `format: 1` as the first line"));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: assembly`"));
    }
    if !size_seen {
        diagnostics.push(missing_key(
            "size",
            "add a `size` of three positive integers: the assembled grid's width (x), height (y), \
             and depth (z)",
        ));
    }
    if !palette_seen {
        diagnostics.push(missing_key(
            "palette",
            "add a `palette`: the one glyph-to-color mapping every part's glyphs resolve against",
        ));
    }
    if !parts_seen {
        diagnostics.push(missing_key(
            "parts",
            "add `parts`: one entry per part file, each with `name`, `file`, and `place`",
        ));
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Parses a part entry's `place`: a sequence of exactly three non-negative integers.
fn parse_place(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[u32; 3]> {
    let items = triple(
        node,
        pointer,
        "place",
        "three non-negative integers",
        "      place:\n        - 4\n        - 10\n        - 2",
        diagnostics,
    )?;
    let mut out = [0u32; 3];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match integer_component(item, 0) {
            Some(value) => out[index] = value,
            None => {
                diagnostics.push(
                    Diagnostic::new(
                        code::ASSEMBLY_DOCUMENT_STRUCTURE,
                        format!(
                            "`place[{index}]` must be a non-negative integer, found {} (line {})",
                            item.value.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}"))
                    .with_help(
                        "`place` is the grid offset of the part's own cell (0, 0, 0), so each \
                         component is a whole number of cells from the assembly's origin",
                    ),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// Parses a part entry's `pivot`: a sequence of exactly three numbers. Finiteness is an `L1138`
/// data-consistency concern, checked later by [`validate`].
fn parse_pivot(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[f64; 3]> {
    let items = triple(
        node,
        pointer,
        "pivot",
        "three numbers",
        "      pivot:\n        - 1.0\n        - 5.0\n        - 1.0",
        diagnostics,
    )?;
    let mut out = [0.0f64; 3];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::Number(n)) => out[index] = crate::fields::number(n),
            other => {
                diagnostics.push(
                    Diagnostic::new(
                        code::ASSEMBLY_DOCUMENT_STRUCTURE,
                        format!(
                            "`pivot[{index}]` must be a number, found {} (line {})",
                            other.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}"))
                    .with_help(
                        "a pivot is part-local voxel coordinates; halves are legal (they reach the \
                         center of an even-width limb)",
                    ),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// The three items of a `size`/`place`/`pivot` node, or `None` (with a diagnostic) when the node is
/// not a sequence of exactly three. `shape` names what the field holds ("three numbers"); `example`
/// is the indented block form the help prints.
fn triple<'a>(
    node: &'a Node,
    pointer: &str,
    field: &str,
    shape: &str,
    example: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a [Node]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "`{field}` must be a sequence of {shape}, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!(
                "write `{field}` one component per line:\n{example}"
            )),
        );
        return None;
    };
    if items.len() != 3 {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "`{field}` must have exactly 3 components (x, y, z), found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!("supply exactly {shape}")),
        );
        return None;
    }
    Some(items)
}

/// The two items of a `limits` node (a `[min, max]` pair), or `None` (with a diagnostic) when the
/// node is not a sequence of exactly two. [`triple`]'s twin for two-element fields.
fn pair<'a>(
    node: &'a Node,
    pointer: &str,
    field: &str,
    shape: &str,
    example: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a [Node]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "`{field}` must be a sequence of {shape}, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!(
                "write `{field}` one component per line:\n{example}"
            )),
        );
        return None;
    };
    if items.len() != 2 {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "`{field}` must have exactly 2 components (min, max), found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help(format!("supply exactly {shape}")),
        );
        return None;
    }
    Some(items)
}

/// One integer component of a `size`/`place`, at least `minimum` and within `u32`.
fn integer_component(item: &Node, minimum: u64) -> Option<u32> {
    let NodeValue::Scalar(Value::Number(n)) = &item.value else {
        return None;
    };
    n.as_u64()
        .filter(|v| *v >= minimum && *v <= u64::from(u32::MAX))
        .map(|v| v as u32)
}

/// Parses `parts`: a sequence of part entries, kept in authored order.
fn parse_parts(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<AssemblyPartEntry> {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "`parts` must be a sequence of part entries, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/parts")
            .with_help(
                "write one entry per part file:\n  parts:\n    - name: torso\n      file: \
                 parts/torso.part.yaml\n      place:\n        - 0\n        - 0\n        - 0",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let pointer = format!("/parts/{index}");
        if let Some(part) = parse_part_entry(item, &pointer, diagnostics) {
            out.push(part);
        }
    }
    out
}

/// Parses one part entry: a mapping with required `name`/`file`/`place` and optional
/// `parent`/`pivot`/`mirror`.
fn parse_part_entry(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<AssemblyPartEntry> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "a part entry must be a mapping with `name`, `file`, and `place`, found {} \
                     (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(
                "write the entry as keys under one `-`:\n    - name: torso\n      file: \
                 parts/torso.part.yaml\n      place:\n        - 0\n        - 0\n        - 0",
            ),
        );
        return None;
    };
    let mut name = None;
    let mut file = None;
    let mut parent = None;
    let mut place = None;
    let mut pivot = [0.0f64; 3];
    let mut mirror_x = false;
    for sub in entries {
        match sub.key.as_str() {
            "name" => {
                name = envelope::expect_string(
                    &sub.node,
                    &format!("{pointer}/name"),
                    "a part's `name`",
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "file" => {
                file = envelope::expect_string(
                    &sub.node,
                    &format!("{pointer}/file"),
                    "a part's `file`",
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "parent" => {
                parent = envelope::expect_string(
                    &sub.node,
                    &format!("{pointer}/parent"),
                    "a part's `parent`",
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "place" => {
                place = parse_place(&sub.node, &format!("{pointer}/place"), diagnostics);
            }
            "pivot" => {
                if let Some(value) =
                    parse_pivot(&sub.node, &format!("{pointer}/pivot"), diagnostics)
                {
                    pivot = value;
                }
            }
            "mirror" => {
                mirror_x = parse_mirror(&sub.node, &format!("{pointer}/mirror"), diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                sub,
                pointer,
                "a part entry",
                PART_ENTRY_KEYS,
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if name.is_none() {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                "a part entry is missing its required `name`",
            )
            .with_path(pointer.to_string())
            .with_help("add `name`: the part's name inside this assembly, unique and clip-facing"),
        );
    }
    if file.is_none() {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                "a part entry is missing its required `file`",
            )
            .with_path(pointer.to_string())
            .with_help("add `file`: the part document's path, relative to this assembly file"),
        );
    }
    if place.is_none() {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                "a part entry is missing its required `place`",
            )
            .with_path(pointer.to_string())
            .with_help(
                "add `place`: three integers, where the part's own cell (0, 0, 0) sits in the \
                 assembled grid",
            ),
        );
    }
    Some(AssemblyPartEntry {
        name: name?,
        file: file?,
        parent,
        place: place?,
        pivot,
        mirror_x,
    })
}

/// Parses a part entry's `mirror`: the string `x`, the only axis this format mirrors about.
fn parse_mirror(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> bool {
    match &node.value {
        NodeValue::Scalar(Value::String(s)) if s == MIRROR_AXIS => true,
        NodeValue::Scalar(Value::String(s)) => {
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    format!(
                        "`mirror` value `{s}` is not a mirror axis (line {})",
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(format!(
                    "`{MIRROR_AXIS}` is the only axis this format mirrors about (one limb file \
                     serving both sides); remove the key to leave the part unmirrored"
                )),
            );
            false
        }
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    format!(
                        "`mirror` must be a string, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(format!("write `mirror: {MIRROR_AXIS}`, or remove the key")),
            );
            false
        }
    }
}

/// Parses `joints`: a mapping of child part name to joint spec, returned sorted by part name (the
/// canonical order [`emit`] writes). Mirrors [`envelope::parse_clips`]'s mapping walk.
fn parse_joints(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<AssemblyJoint> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "`joints` must be a mapping of child part name to joint spec, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/joints")
            .with_help(
                "write one entry per jointed child part:\n  joints:\n    lower-arm-l:\n      kind: \
                 hinge",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for sub in entries {
        let pointer = format!("/joints/{}", pointer_token(&sub.key));
        if let Some(joint) = parse_joint_entry(sub, &pointer, diagnostics) {
            out.push(joint);
        }
    }
    out.sort_by(|a, b| a.part.cmp(&b.part));
    out
}

/// Parses one joint entry: a mapping with a required `kind` and, depending on it, `axis`/`limits`,
/// plus an optional `break_impulse`. Structure only (`L1137`): finiteness, zero-length axes, inverted
/// limits, and a non-positive `break_impulse` are `L1184`/`ASSEMBLY_JOINT_INVALID` data-consistency
/// concerns, checked later by [`validate`].
fn parse_joint_entry(
    sub: &MapEntry,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<AssemblyJoint> {
    let NodeValue::Mapping(entries) = &sub.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "a joint entry must be a mapping with `kind`, found {} (line {})",
                    sub.node.value.type_phrase(),
                    sub.node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(
                "write the entry under the child part's name:\n    lower-arm-l:\n      kind: hinge",
            ),
        );
        return None;
    };

    let mut kind_seen = false;
    let mut kind_name = None;
    let mut break_impulse = None;
    for field in entries {
        let field_pointer = format!("{pointer}/{}", field.key);
        match field.key.as_str() {
            "kind" => {
                kind_seen = true;
                kind_name = parse_joint_kind_name(&field.node, &field_pointer, diagnostics);
            }
            "axis" | "limits" => {} // shape depends on `kind`; dispatched below by direct lookup
            "break_impulse" => {
                break_impulse = parse_break_impulse(&field.node, &field_pointer, diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                field,
                pointer,
                "a joint entry",
                ASSEMBLY_JOINT_KEYS,
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if !kind_seen {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                "a joint entry is missing its required `kind`",
            )
            .with_path(pointer.to_string())
            .with_help("add `kind: hinge`, `kind: ball`, `kind: slider`, or `kind: fixed`"),
        );
    }
    let kind_name = kind_name?;

    let axis_entry = entries.iter().find(|e| e.key == "axis");
    let limits_entry = entries.iter().find(|e| e.key == "limits");
    let axis_allowed = matches!(kind_name, "hinge" | "slider");
    let limits_allowed = kind_name != "fixed";
    let mut ok = true;
    if let Some(axis_entry) = axis_entry
        && !axis_allowed
    {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "`axis` does not apply to a `{kind_name}` joint (line {})",
                    axis_entry.node.line
                ),
            )
            .with_path(format!("{pointer}/axis"))
            .with_help("`axis` is only meaningful for `hinge` and `slider`; remove the key"),
        );
        ok = false;
    }
    if let Some(limits_entry) = limits_entry
        && !limits_allowed
    {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "`limits` does not apply to a `fixed` joint (line {})",
                    limits_entry.node.line
                ),
            )
            .with_path(format!("{pointer}/limits"))
            .with_help("a `fixed` joint has no degrees of freedom to limit; remove the key"),
        );
        ok = false;
    }
    if !ok {
        return None;
    }

    let kind = match kind_name {
        "hinge" | "slider" => {
            let axis = match axis_entry {
                Some(entry) => {
                    parse_joint_axis(&entry.node, &format!("{pointer}/axis"), diagnostics)?
                }
                None => [0.0, 1.0, 0.0],
            };
            let limits = match limits_entry {
                Some(entry) => Some(parse_limit_pair(
                    &entry.node,
                    &format!("{pointer}/limits"),
                    diagnostics,
                )?),
                None => None,
            };
            if kind_name == "hinge" {
                AssemblyJointKind::Hinge { axis, limits }
            } else {
                AssemblyJointKind::Slider { axis, limits }
            }
        }
        "ball" => {
            let limits = match limits_entry {
                Some(entry) => Some(parse_ball_limits(
                    &entry.node,
                    &format!("{pointer}/limits"),
                    diagnostics,
                )?),
                None => None,
            };
            AssemblyJointKind::Ball { limits }
        }
        "fixed" => AssemblyJointKind::Fixed,
        other => unreachable!("`{other}` is not one of {JOINT_KIND_NAMES:?}"),
    };

    Some(AssemblyJoint {
        part: sub.key.clone(),
        kind,
        break_impulse,
    })
}

/// Parses a joint entry's `kind`: one of [`JOINT_KIND_NAMES`].
fn parse_joint_kind_name(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'static str> {
    match &node.value {
        NodeValue::Scalar(Value::String(s)) => {
            match JOINT_KIND_NAMES.iter().find(|&&name| name == s) {
                Some(&name) => Some(name),
                None => {
                    let mut diagnostic = Diagnostic::new(
                        code::ASSEMBLY_DOCUMENT_STRUCTURE,
                        format!(
                            "`kind` value `{s}` is not a joint kind (line {})",
                            node.line
                        ),
                    )
                    .with_path(pointer.to_string());
                    diagnostic = match suggest::nearest(s, JOINT_KIND_NAMES) {
                        Some(candidate) => {
                            diagnostic.with_help(format!("nearest match is `{candidate}`"))
                        }
                        None => diagnostic
                            .with_help(format!("valid kinds are: {}", JOINT_KIND_NAMES.join(", "))),
                    };
                    diagnostics.push(diagnostic);
                    None
                }
            }
        }
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    format!(
                        "`kind` must be a string, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(format!(
                    "set `kind` to one of: {}",
                    JOINT_KIND_NAMES.join(", ")
                )),
            );
            None
        }
    }
}

/// Parses a joint entry's `axis`: a sequence of exactly three numbers (hinge/slider only).
/// Finiteness and non-zero-ness are `L1184` data-consistency concerns, checked later by [`validate`].
fn parse_joint_axis(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; 3]> {
    let items = triple(
        node,
        pointer,
        "axis",
        "three numbers",
        "      axis:\n        - 1.0\n        - 0.0\n        - 0.0",
        diagnostics,
    )?;
    let mut out = [0.0f64; 3];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::Number(n)) => out[index] = crate::fields::number(n),
            other => {
                diagnostics.push(
                    Diagnostic::new(
                        code::ASSEMBLY_DOCUMENT_STRUCTURE,
                        format!(
                            "`axis[{index}]` must be a number, found {} (line {})",
                            other.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}"))
                    .with_help("axis components are numbers, in the parent part's local frame"),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// Parses a `limits` node as one `[min, max]` pair. Finiteness and ordering are `L1184`
/// data-consistency concerns, checked later by [`validate`].
fn parse_limit_pair(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; 2]> {
    let items = pair(
        node,
        pointer,
        "limits",
        "two numbers",
        "      limits:\n        - 0.0\n        - 120.0",
        diagnostics,
    )?;
    let mut out = [0.0f64; 2];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::Number(n)) => out[index] = crate::fields::number(n),
            other => {
                diagnostics.push(
                    Diagnostic::new(
                        code::ASSEMBLY_DOCUMENT_STRUCTURE,
                        format!(
                            "`limits[{index}]` must be a number, found {} (line {})",
                            other.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}"))
                    .with_help("`limits` is `[min, max]`"),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// Parses a ball joint's `limits`: a sequence of exactly three `[min, max]` pairs, one per axis.
fn parse_ball_limits(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[[f64; 2]; 3]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "`limits` must be a sequence of three [min, max] pairs, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(
                "write one pair per axis:\n      limits:\n        -\n          - -10.0\n          \
                 - 10.0\n        -\n          - -10.0\n          - 10.0\n        -\n          - \
                 -10.0\n          - 10.0",
            ),
        );
        return None;
    };
    if items.len() != 3 {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "`limits` must have exactly 3 components (x, y, z), found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help("supply exactly three [min, max] pairs, one per axis"),
        );
        return None;
    }
    let mut out = [[0.0f64; 2]; 3];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match parse_limit_pair(item, &format!("{pointer}/{index}"), diagnostics) {
            Some(value) => out[index] = value,
            None => ok = false,
        }
    }
    ok.then_some(out)
}

/// Parses a joint entry's `break_impulse`: a number. Finiteness and positivity are `L1184`
/// data-consistency concerns, checked later by [`validate`].
fn parse_break_impulse(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<f64> {
    match &node.value {
        NodeValue::Scalar(Value::Number(n)) => Some(crate::fields::number(n)),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    format!(
                        "`break_impulse` must be a number, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(
                    "`break_impulse` is the linear impulse magnitude, in one step, above which the \
                     joint breaks",
                ),
            );
            None
        }
    }
}

fn missing_key(key: &str, help: &str) -> Diagnostic {
    envelope::missing_key(
        key,
        help,
        code::ASSEMBLY_DOCUMENT_STRUCTURE,
        "assembly document",
    )
}

// --- Validation (`L1138`) -----------------------------------------------------------------------

/// Validates data-consistency invariants of an already-parsed [`AssemblyDocument`] (`L1138`): the
/// palette's keys/colors/material scalars (the voxel format's rules, under this family's code),
/// non-empty `parts` with unique non-empty names and files, `parent` links that resolve to a real
/// part without self-parenting or cycles, exactly one root, finite pivots, and `place` components
/// inside the assembly's own `size`.
///
/// Whether each part *fits* the grid needs the part file's own `size`, so it lives in
/// [`check_parts`] instead — this crate never reads a file.
///
/// Independent of how the document was built — a hand-authored one via [`parse`], or one assembled
/// programmatically — so this is the one place those invariants are enforced. A part `file` or clip
/// path may climb with `..` but is refused when absolute, drive-prefixed or a Windows device.
pub(crate) fn validate(doc: &AssemblyDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    crate::voxel::validate_palette(
        &doc.palette,
        code::ASSEMBLY_DATA_CONSISTENCY,
        "palette",
        &mut diagnostics,
    );

    let cells: u128 = doc.size.iter().map(|&d| u128::from(d)).product();
    if cells > u128::from(crate::VOXEL_MAX_CELLS) {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DATA_CONSISTENCY,
                format!(
                    "the assembly's `size` {}x{}x{} holds {cells} cells, past the {}-cell ceiling",
                    doc.size[0],
                    doc.size[1],
                    doc.size[2],
                    crate::VOXEL_MAX_CELLS
                ),
            )
            .with_path("/size")
            .with_help("shrink `size`; an assembly may hold at most 256x256x256 cells"),
        );
    }

    if doc.parts.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DATA_CONSISTENCY,
                "an assembly must place at least one part",
            )
            .with_path("/parts")
            .with_help("add at least one entry under `parts`, or delete this assembly"),
        );
    }

    for (index, part) in doc.parts.iter().enumerate() {
        let pointer = format!("/parts/{index}");
        if part.name.is_empty() {
            diagnostics.push(
                Diagnostic::new(code::ASSEMBLY_DATA_CONSISTENCY, "a part's `name` is empty")
                    .with_path(format!("{pointer}/name"))
                    .with_help("give the part a name; clips address parts by name"),
            );
        }
        if doc.parts[..index]
            .iter()
            .any(|other| other.name == part.name)
        {
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_DATA_CONSISTENCY,
                    format!("the part name `{}` is used more than once", part.name),
                )
                .with_path(format!("{pointer}/name"))
                .with_help(
                    "each part needs its own name — a clip track or a `parent` naming a duplicated \
                     name cannot say which part it means",
                ),
            );
        }
        if part.file.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_DATA_CONSISTENCY,
                    format!("part `{}` has an empty `file`", part.name),
                )
                .with_path(format!("{pointer}/file"))
                .with_help("point `file` at the part document, relative to this assembly file"),
            );
        } else if let Some(refusal) = sibling_refusal(&part.file) {
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_DATA_CONSISTENCY,
                    format!("`file` (`{}`) {}", part.file, refusal.describe()),
                )
                .with_path(format!("{pointer}/file"))
                .with_help("point `file` at the part document, relative to this assembly file"),
            );
        }
        if let Some(parent) = &part.parent {
            if *parent == part.name {
                diagnostics.push(
                    Diagnostic::new(
                        code::ASSEMBLY_DATA_CONSISTENCY,
                        format!("part `{}` names itself as its own `parent`", part.name),
                    )
                    .with_path(format!("{pointer}/parent"))
                    .with_help(
                        "point `parent` at the part this one hangs off, or remove the key to make \
                         this the root",
                    ),
                );
            } else if index_of(doc, parent).is_none() {
                let mut diagnostic = Diagnostic::new(
                    code::ASSEMBLY_DATA_CONSISTENCY,
                    format!(
                        "part `{}` names the parent `{parent}`, which is not a part of this \
                         assembly",
                        part.name
                    ),
                )
                .with_path(format!("{pointer}/parent"));
                let names: Vec<&str> = doc.parts.iter().map(|p| p.name.as_str()).collect();
                diagnostic = match suggest::nearest(parent, names) {
                    Some(candidate) => {
                        diagnostic.with_help(format!("nearest match is `{candidate}`"))
                    }
                    None => diagnostic
                        .with_help("`parent` must name another part's `name` in this same file"),
                };
                diagnostics.push(diagnostic);
            }
        }
        for (axis, &component) in part.pivot.iter().enumerate() {
            if !component.is_finite() {
                diagnostics.push(
                    Diagnostic::new(
                        code::ASSEMBLY_DATA_CONSISTENCY,
                        format!(
                            "part `{}` has a non-finite `pivot` component `{component}` on {}",
                            part.name, AXES[axis]
                        ),
                    )
                    .with_path(format!("{pointer}/pivot/{axis}"))
                    .with_help("pivot components must be finite part-local voxel coordinates"),
                );
            }
        }
        for (axis, &component) in part.place.iter().enumerate() {
            if component >= doc.size[axis] {
                diagnostics.push(
                    Diagnostic::new(
                        code::ASSEMBLY_DATA_CONSISTENCY,
                        format!(
                            "part `{}` is placed at {} = {component}, outside the assembly's \
                             `size.{}` = {}",
                            part.name, AXES[axis], AXES[axis], doc.size[axis]
                        ),
                    )
                    .with_path(format!("{pointer}/place/{axis}"))
                    .with_help(
                        "`place` is the grid cell the part's own (0, 0, 0) lands on, so every \
                         component must be less than the assembly's `size` on that axis",
                    ),
                );
            }
        }
    }

    validate_hierarchy(doc, &mut diagnostics);

    for clip in &doc.clips {
        let pointer = format!("/clips/{}", pointer_token(&clip.name));
        if clip.name.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_DATA_CONSISTENCY,
                    "a clip binding has an empty short name",
                )
                .with_path(pointer.clone())
                .with_help(
                    "name the clip the way game code selects it — `walk`, `attack` — or remove \
                     the binding",
                ),
            );
        }
        if clip.file.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_DATA_CONSISTENCY,
                    format!("the clip binding `{}` has an empty file path", clip.name),
                )
                .with_path(pointer)
                .with_help(
                    "point the binding at the clip document, relative to this assembly file",
                ),
            );
        } else if let Some(refusal) = sibling_refusal(&clip.file) {
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_DATA_CONSISTENCY,
                    format!("`{}` (`{}`) {}", clip.name, clip.file, refusal.describe()),
                )
                .with_path(pointer)
                .with_help(
                    "point the binding at the clip document, relative to this assembly file",
                ),
            );
        }
    }

    validate_joints(doc, &mut diagnostics);
    diagnostics
}

/// The `joints` half of [`validate`] (`L1184`/`ASSEMBLY_JOINT_INVALID`): each entry names a real,
/// non-root part, and its axis/limits/break_impulse hang together — the `Joint` component's own
/// `L1183` checks, applied to the authored list instead of a live component.
fn validate_joints(doc: &AssemblyDocument, diagnostics: &mut Vec<Diagnostic>) {
    for joint in &doc.joints {
        let pointer = format!("/joints/{}", pointer_token(&joint.part));
        match index_of(doc, &joint.part) {
            None => {
                let mut diagnostic = Diagnostic::new(
                    code::ASSEMBLY_JOINT_INVALID,
                    format!(
                        "a joint entry names the part `{}`, which is not a part of this assembly",
                        joint.part
                    ),
                )
                .with_path(pointer.clone());
                let names: Vec<&str> = doc.parts.iter().map(|p| p.name.as_str()).collect();
                diagnostic = match suggest::nearest(&joint.part, names) {
                    Some(candidate) => {
                        diagnostic.with_help(format!("nearest match is `{candidate}`"))
                    }
                    None => {
                        diagnostic.with_help("a joint entry's key must name another part's `name`")
                    }
                };
                diagnostics.push(diagnostic);
            }
            Some(index) if doc.parts[index].parent.is_none() => {
                diagnostics.push(
                    Diagnostic::new(
                        code::ASSEMBLY_JOINT_INVALID,
                        format!(
                            "a joint entry names `{}`, the assembly's root part — a joint joins a \
                             part to its `parent`, which the root does not have",
                            joint.part
                        ),
                    )
                    .with_path(pointer.clone())
                    .with_help(
                        "remove this entry, or point it at a non-root part; the root is never \
                         jointed",
                    ),
                );
            }
            Some(_) => {}
        }

        match &joint.kind {
            AssemblyJointKind::Hinge { axis, limits }
            | AssemblyJointKind::Slider { axis, limits } => {
                validate_joint_axis(axis, &format!("{pointer}/axis"), diagnostics);
                if let Some(limits) = limits {
                    validate_joint_limit_pair(*limits, &format!("{pointer}/limits"), diagnostics);
                }
            }
            AssemblyJointKind::Ball { limits } => {
                if let Some(limits) = limits {
                    for (index, pair) in limits.iter().enumerate() {
                        validate_joint_limit_pair(
                            *pair,
                            &format!("{pointer}/limits/{index}"),
                            diagnostics,
                        );
                    }
                }
            }
            AssemblyJointKind::Fixed => {}
        }
        if let Some(break_impulse) = joint.break_impulse {
            validate_joint_break_impulse(
                break_impulse,
                &format!("{pointer}/break_impulse"),
                diagnostics,
            );
        }
    }
}

/// Below this f32 length, `joint_kind_of`'s f64→f32 cast (`ludion-engine`'s `ragdoll.rs`) and
/// `ludion-physics`'s subsequent `normalize()` are too close to dividing by zero to trust: an axis
/// like `[1e-50, 0.0, 0.0]` passes an exactly-zero check on the authored `f64`s, then rounds to
/// exactly `0.0` once narrowed to `f32` and poisons the solver with `NaN`
/// (the `Joint` component's own `L1183` `MIN_AXIS_LENGTH` precedent, `ludion-spatial`'s `joint.rs`).
const MIN_AXIS_LENGTH: f32 = 1e-6;

/// Pushes an `L1184` when `axis` is non-finite or too short to normalize once narrowed to `f32`
/// (see [`MIN_AXIS_LENGTH`]).
fn validate_joint_axis(axis: &[f64; 3], pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    if !axis.iter().all(|c| ludion_core::number::fits_f32(*c)) {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_JOINT_INVALID,
                format!("joint `axis` must be finite (as f32), found {axis:?}"),
            )
            .with_path(pointer.to_string())
            .with_help("give every `axis` component a finite number within ±3.4e38"),
        );
        return;
    }
    let [x, y, z] = axis.map(|c| c as f32);
    let length = (x * x + y * y + z * z).sqrt();
    if length < MIN_AXIS_LENGTH {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_JOINT_INVALID,
                format!(
                    "joint `axis` must not be the zero vector (or round to it once narrowed to \
                     f32), found {axis:?}"
                ),
            )
            .with_path(pointer.to_string())
            .with_help("give the axis a non-zero direction"),
        );
    }
}

/// Pushes an `L1184` when a `[min, max]` limit pair is non-finite or inverted.
fn validate_joint_limit_pair(limits: [f64; 2], pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    let [min, max] = limits;
    if !min.is_finite() || !max.is_finite() {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_JOINT_INVALID,
                format!("joint limit [{min}, {max}] must be finite"),
            )
            .with_path(pointer.to_string())
            .with_help("give both `limits` components finite numbers"),
        );
    } else if min > max {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_JOINT_INVALID,
                format!("joint limit minimum {min} exceeds its maximum {max}"),
            )
            .with_path(pointer.to_string())
            .with_help("order `limits` as [min, max]"),
        );
    }
}

/// Pushes an `L1184` when `break_impulse` is not strictly positive and finite.
fn validate_joint_break_impulse(value: f64, pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    if !value.is_finite() || value <= 0.0 {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_JOINT_INVALID,
                format!(
                    "joint `break_impulse` {value} is not valid; it must be a strictly positive, \
                     finite number"
                ),
            )
            .with_path(pointer.to_string())
            .with_help(
                "give `break_impulse` a value greater than 0, or remove it for a joint that never \
                 breaks",
            ),
        );
    }
}

/// The root/cycle half of [`validate`]: exactly one part omits `parent`, and no chain of `parent`
/// links loops. Self-parenting is deliberately not re-reported here — it has its own message.
fn validate_hierarchy(doc: &AssemblyDocument, diagnostics: &mut Vec<Diagnostic>) {
    if doc.parts.is_empty() {
        // "No root" would only restate the empty-`parts` diagnostic in weaker words.
        return;
    }
    let roots: Vec<&str> = doc
        .parts
        .iter()
        .filter(|part| part.parent.is_none())
        .map(|part| part.name.as_str())
        .collect();
    match roots.len() {
        1 => {}
        0 => diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DATA_CONSISTENCY,
                "no part is the root: every part declares a `parent`",
            )
            .with_path("/parts")
            .with_help(
                "remove `parent` from the one part the others hang off (the torso, for a \
                 humanoid) — an assembly has exactly one root",
            ),
        ),
        _ => diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DATA_CONSISTENCY,
                format!(
                    "{} parts declare no `parent`, so the assembly has {} roots: {}",
                    roots.len(),
                    roots.len(),
                    roots
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with_path("/parts")
            .with_help(
                "give every part but one a `parent` — an assembly is a single tree, so exactly one \
                 part is the root",
            ),
        ),
    }

    let parents = parent_indices(doc);
    for index in 0..doc.parts.len() {
        let Some(cycle) = cycle_from(index, &parents) else {
            continue;
        };
        // A one-element cycle is self-parenting, which `validate` already reported in its own
        // words; and reporting only from the cycle's lowest member names each cycle exactly once.
        if cycle.len() < 2 || cycle.iter().min() != Some(&index) {
            continue;
        }
        let mut chain: Vec<String> = cycle
            .iter()
            .map(|&i| format!("`{}`", doc.parts[i].name))
            .collect();
        chain.push(format!("`{}`", doc.parts[index].name));
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DATA_CONSISTENCY,
                format!("the `parent` chain loops: {}", chain.join(" -> ")),
            )
            .with_path(format!("/parts/{index}/parent"))
            .with_help(
                "an assembly is a tree: break the loop by re-pointing one `parent`, or by removing \
                 it to make that part the root",
            ),
        );
    }
}

/// The cycle reachable by following `parents` from `start`, in walk order beginning at the first
/// repeated node, or `None` when the chain ends at a root. Terminates because each step either
/// closes the loop or visits a part not yet on the path.
fn cycle_from(start: usize, parents: &[Option<usize>]) -> Option<Vec<usize>> {
    let mut path: Vec<usize> = Vec::new();
    let mut current = start;
    loop {
        if let Some(position) = path.iter().position(|&node| node == current) {
            return Some(path[position..].to_vec());
        }
        path.push(current);
        current = parents[current]?;
    }
}

/// The index of the first part named `name`, or `None`.
fn index_of(doc: &AssemblyDocument, name: &str) -> Option<usize> {
    doc.parts.iter().position(|part| part.name == name)
}

/// Each part's parent index, `None` for the root.
///
/// On a [`validate`]-clean document every `Some` is a real index and exactly one entry is `None`.
/// On an invalid one a `parent` naming no part also reads as `None` — that case is already an
/// `L1138` of its own, so this function never invents an index to paper over it.
pub(crate) fn parent_indices(doc: &AssemblyDocument) -> Vec<Option<usize>> {
    doc.parts
        .iter()
        .map(|part| part.parent.as_deref().and_then(|name| index_of(doc, name)))
        .collect()
}

// --- Cross-file composition (`L1138`) -----------------------------------------------------------

/// Checks an assembly against its resolved part documents (`L1138`): every glyph a part draws with
/// is defined by the assembly's palette (decision 4 — parts have no palette of their own), and
/// every part fits the assembled grid at its `place`.
///
/// `parts` is aligned by index with [`AssemblyDocument::parts`]: the caller resolved each entry's
/// `file` and checked it with [`crate::check_part`], so each [`PartDocument`] is assumed
/// self-consistent. A length mismatch is a caller bug, reported as one diagnostic with no further
/// checking rather than as a wall of misattributed ones.
///
/// Every diagnostic points into the **assembly** document (`/parts/<i>/...`) and names the part and
/// its file, because that is where the placement and the palette are authored.
pub(crate) fn check_parts(assembly: &AssemblyDocument, parts: &[PartDocument]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if parts.len() != assembly.parts.len() {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DATA_CONSISTENCY,
                format!(
                    "{} resolved part documents were supplied for an assembly declaring {} parts",
                    parts.len(),
                    assembly.parts.len()
                ),
            )
            .with_path("/parts")
            .with_help(
                "supply one resolved part document per `parts` entry, in the same order as the \
                 assembly declares them",
            ),
        );
        return diagnostics;
    }

    let palette: Vec<char> = assembly
        .palette
        .iter()
        .filter_map(|entry| single_glyph(&entry.glyph))
        .collect();

    for (index, (entry, part)) in assembly.parts.iter().zip(parts).enumerate() {
        let pointer = format!("/parts/{index}");
        let mut reported: Vec<char> = Vec::new();
        for layer in &part.slices {
            for row in layer {
                for glyph in row.chars() {
                    if glyph != EMPTY_GLYPH
                        && !palette.contains(&glyph)
                        && !reported.contains(&glyph)
                    {
                        reported.push(glyph);
                        diagnostics.push(
                            Diagnostic::new(
                                code::ASSEMBLY_DATA_CONSISTENCY,
                                format!(
                                    "part `{}` ({}) draws with the glyph `{glyph}`, which this \
                                     assembly's palette does not define",
                                    entry.name, entry.file
                                ),
                            )
                            .with_path(format!("{pointer}/file"))
                            .with_help(format!(
                                "add `{glyph}` to the assembly's `palette`, or redraw those cells \
                                 with a glyph it already defines — a part has no palette of its own"
                            )),
                        );
                    }
                }
            }
        }

        for (axis, name) in AXES.iter().enumerate() {
            let reach = u64::from(entry.place[axis]) + u64::from(part.size[axis]);
            let limit = u64::from(assembly.size[axis]);
            if reach > limit {
                diagnostics.push(
                    Diagnostic::new(
                        code::ASSEMBLY_DATA_CONSISTENCY,
                        format!(
                            "part `{}` ({}) does not fit the assembled grid on {name}: place {} + \
                             part size {} = {reach}, past `size.{name}` = {limit}",
                            entry.name, entry.file, entry.place[axis], part.size[axis],
                        ),
                    )
                    .with_path(format!("{pointer}/place/{axis}"))
                    .with_help(
                        "move the part with `place`, trim the part file's own `size`, or grow the \
                         assembly's `size` — the rest pose is the assembled grid, so every part \
                         lies inside it",
                    ),
                );
            }
        }
    }
    diagnostics
}

/// The voxel document the engine meshes for one part: the assembly's palette over that part's own
/// grid, named by the part entry.
///
/// When the entry sets `mirror: x` the slices are mirrored about x here — each row string reversed,
/// a pure grid transform — so the mesher sees ordinary cells and produces correct winding and
/// normals with no negative scale anywhere downstream. The stored
/// [`AssemblyPartEntry::pivot`] is deliberately *not* transformed (it stays in unmirrored
/// part-local coordinates, as authored); a consumer that needs the pivot in this document's frame
/// calls [`AssemblyPartEntry::pivot_in_mesh_frame`].
///
/// # Panics
/// If `index` is not a valid index into [`AssemblyDocument::parts`].
pub(crate) fn part_voxel(
    assembly: &AssemblyDocument,
    index: usize,
    part: &PartDocument,
) -> VoxelDocument {
    let entry = &assembly.parts[index];
    let slices = if entry.mirror_x {
        part.slices
            .iter()
            .map(|layer| {
                layer
                    .iter()
                    .map(|row| row.chars().rev().collect::<String>())
                    .collect()
            })
            .collect()
    } else {
        part.slices.clone()
    };
    VoxelDocument {
        name: Some(entry.name.clone()),
        size: part.size,
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette: assembly.palette.clone(),
        slices,
    }
}

// --- Grid-level composition (`Plan/voxel-animation.md` decisions 10-11) -------------------------

/// Every part of an assembly composed into one assembly-global grid at rest pose, plus per-cell part
/// attribution — the substrate the assembled-grid analysis family (`--exposure`, `--mirror-x`,
/// connectivity) runs over.
///
/// [`Self::voxel`] is a synthesized [`VoxelDocument`] (the assembly's own `size`/`name`/`palette`,
/// slices painted from every part) so every single-voxel analysis method
/// ([`VoxelDocument::exposure`], [`VoxelDocument::mirror_x_regions`], [`VoxelDocument::connectivity`],
/// [`VoxelDocument::render_slice`], [`VoxelDocument::render_column`]) runs over an assembly exactly as
/// it runs over an ordinary voxel model — no analysis logic is duplicated for this family.
///
/// Built by `assembled_grid`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AssembledGrid {
    /// The composed grid.
    pub voxel: VoxelDocument,
    /// Every filled cell's owning parts: indices into [`AssemblyDocument::parts`], ascending
    /// (authored order), one entry per cell in `(y, z, x)` scan order — the same indexing
    /// [`VoxelDocument::for_each_cell`] visits in. Empty for an unfilled cell, one entry for an
    /// ordinary cell, more than one where parts overlap (decision 10: legal, and counted as filled
    /// exactly once in [`Self::voxel`] regardless of how many parts contributed it).
    owners: Vec<Vec<usize>>,
}

impl AssembledGrid {
    /// The owning parts of the cell at `(x, y, z)` — indices into [`AssemblyDocument::parts`],
    /// ascending — or `&[]` for an unfilled or out-of-range cell.
    pub fn owners_at(&self, x: u32, y: u32, z: u32) -> &[usize] {
        let [sx, sy, sz] = self.voxel.size;
        if x >= sx || y >= sy || z >= sz {
            return &[];
        }
        &self.owners[owner_index(sx, sz, x, y, z)]
    }

    /// Every filled cell more than one part contributed, in `(y, z, x)` scan order: the cell and its
    /// owning parts (always 2 or more). Empty when no two parts of this assembly overlap.
    pub fn overlaps(&self) -> Vec<([u32; 3], &[usize])> {
        let [sx, sy, sz] = self.voxel.size;
        let mut out = Vec::new();
        for y in 0..sy {
            for z in 0..sz {
                for x in 0..sx {
                    let owners = &self.owners[owner_index(sx, sz, x, y, z)];
                    if owners.len() > 1 {
                        out.push(([x, y, z], owners.as_slice()));
                    }
                }
            }
        }
        out
    }
}

/// The `(y, z, x)` scan-order index of cell `(x, y, z)` in a grid sized `sx` by (implicitly) `sy` by
/// `sz` — the same formula [`VoxelDocument::for_each_cell`]'s scan order and this crate's other
/// whole-grid sweeps use, so an [`AssembledGrid::owners`] entry lines up with every other per-cell
/// listing this crate produces.
fn owner_index(sx: u32, sz: u32, x: u32, y: u32, z: u32) -> usize {
    (y as usize * sz as usize + z as usize) * sx as usize + x as usize
}

/// Composes every part of `assembly` into one assembly-global grid at rest pose (decision 9: the rest
/// pose *is* the assembled grid), painting each part's cells — already mirrored, via [`part_voxel`] —
/// at its `place` offset.
///
/// `parts` is aligned by index with [`AssemblyDocument::parts`], exactly [`check_parts`]'s
/// convention (the caller resolved each entry's `file` and checked it with [`crate::check_part`]).
/// Assumed [`check_parts`]-clean: a cell a part would place outside the assembly's own `size` — which
/// only an uncaught fit violation produces — is silently dropped rather than panicking, so a caller
/// that composes before checking gets a degraded grid, never a crash.
///
/// Where two parts fill the same global cell (a joint plug, decision 10), the **later part in
/// authored order** wins the composed glyph — deterministic, and immaterial to occupancy: decision 10
/// counts an overlapped cell as filled exactly once regardless of which glyph is shown. Both parts
/// are recorded in [`AssembledGrid::owners`] either way, so a reader always has the full attribution
/// even though only one glyph is visible in [`AssembledGrid::voxel`].
///
/// # Panics
/// If `parts` is not the same length as [`AssemblyDocument::parts`] — the [`mesh_assembly`]-style
/// convention this crate's cross-file functions share (see [`check_parts`]).
pub(crate) fn assembled_grid(assembly: &AssemblyDocument, parts: &[PartDocument]) -> AssembledGrid {
    assert_eq!(
        assembly.parts.len(),
        parts.len(),
        "one resolved part document per `parts` entry, in the assembly's own order"
    );
    let [sx, sy, sz] = assembly.size;
    let cell_count = sx as usize * sy as usize * sz as usize;
    let mut glyphs = vec![EMPTY_GLYPH; cell_count];
    let mut owners: Vec<Vec<usize>> = vec![Vec::new(); cell_count];

    for (part_index, (entry, part)) in assembly.parts.iter().zip(parts).enumerate() {
        let voxel = part_voxel(assembly, part_index, part);
        let [px, py, pz] = entry.place;
        voxel.for_each_cell(|x, y, z, palette_index| {
            let Some(palette_index) = palette_index else {
                return;
            };
            let (gx, gy, gz) = (px + x, py + y, pz + z);
            if gx >= sx || gy >= sy || gz >= sz {
                return;
            }
            let Some(glyph) = voxel
                .palette
                .get(palette_index as usize)
                .and_then(|entry| single_glyph(&entry.glyph))
            else {
                return;
            };
            let cell = owner_index(sx, sz, gx, gy, gz);
            glyphs[cell] = glyph;
            owners[cell].push(part_index);
        });
    }

    let mut slices: Vec<Vec<String>> = Vec::with_capacity(sy as usize);
    for y in 0..sy {
        let mut layer = Vec::with_capacity(sz as usize);
        for z in 0..sz {
            let row: String = (0..sx)
                .map(|x| glyphs[owner_index(sx, sz, x, y, z)])
                .collect();
            layer.push(row);
        }
        slices.push(layer);
    }

    AssembledGrid {
        voxel: VoxelDocument {
            name: assembly.name.clone(),
            size: assembly.size,
            scale: None,
            generated_from: None,
            budget_exempt: None,
            palette: assembly.palette.clone(),
            slices,
        },
        owners,
    }
}

// --- The overlap / joint-plug lint (`L1143`, decision 10) ---------------------------------------

/// The smallest plug radius the lint ever uses, in cells — a one-cell part still gets a full cell of
/// slack around its pivot, so a plug on a thin limb is never warned about for sitting half a cell
/// off-center.
const MIN_PLUG_RADIUS: f64 = 1.0;

/// One 6-connected cluster of cells that the same two parts both fill.
struct OverlapCluster {
    /// The two overlapping parts, ascending authored index.
    pair: [usize; 2],
    /// How many cells the cluster spans.
    cells: u64,
    /// Inclusive lower corner of the cluster's bounding box, assembly-global.
    min: [u32; 3],
    /// Inclusive upper corner of the cluster's bounding box, assembly-global.
    max: [u32; 3],
    /// The cluster's center of mass in assembly-global *continuous* coordinates: cell `(x, y, z)`
    /// contributes its center `(x + 0.5, y + 0.5, z + 0.5)`, which is the corner-origin frame
    /// `place` and `pivot` are authored in — so a distance to a pivot compares like with like.
    center: [f64; 3],
}

/// The radius, in cells, inside which an overlap between a part and its parent reads as a deliberate
/// joint plug rather than a mistake: **half the child part's largest grid extent**, floored at
/// [`MIN_PLUG_RADIUS`].
///
/// Deliberately crude, and deliberately generous. A plug sits inside the socket the limb turns in,
/// so a correct pivot leaves the cluster a cell or two away; the mistake this lint exists to catch —
/// a pivot at the *wrong end* of the limb — leaves it a whole limb length away, comfortably past
/// half of it. Erring generous is the point: this is a heuristic warning (the `L1028`/`L1029`
/// class), and an occasional missed plug costs nothing, while a false alarm on legal content teaches
/// an author to ignore the lint.
fn plug_radius(part: &PartDocument) -> f64 {
    let extent = part.size.iter().copied().max().unwrap_or(1);
    (f64::from(extent) * 0.5).max(MIN_PLUG_RADIUS)
}

/// A part's pivot in assembly-global coordinates: its `place` offset plus the pivot in the frame
/// [`part_voxel`] emits (so a `mirror: x` part's pivot is mirrored first, via
/// [`AssemblyPartEntry::pivot_in_mesh_frame`] — the composed grid holds mirrored cells, and the two
/// have to be measured in one frame).
fn pivot_in_assembly(
    assembly: &AssemblyDocument,
    parts: &[PartDocument],
    index: usize,
) -> [f64; 3] {
    let entry = &assembly.parts[index];
    let pivot = entry.pivot_in_mesh_frame(parts[index].size[0]);
    [
        f64::from(entry.place[0]) + pivot[0],
        f64::from(entry.place[1]) + pivot[1],
        f64::from(entry.place[2]) + pivot[2],
    ]
}

/// Euclidean distance between two points, through [`ludion_core::math`] — the determinism rule, so
/// this warning's text cannot differ between two machines.
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    ludion_core::math::f64::sqrt(dx * dx + dy * dy + dz * dz)
}

/// The 6-connected clusters of `cells` (each a distinct cell of a grid sized `size`), as cell lists.
///
/// Marks the cells in a grid of their own and runs [`VoxelDocument::connectivity_components`] over
/// it — the trick [`VoxelDocument::mirror_x_regions`] plays with its own marker grid, so this lint's
/// notion of "one region" is literally the flood fill `L1129` and `--mirror-x` report, never a second
/// implementation of it.
fn connected_clusters(size: [u32; 3], cells: &[[u32; 3]]) -> Vec<Vec<[u32; 3]>> {
    const MARKER: char = '#';
    let [sx, sy, sz] = size;
    let mut glyphs = vec![EMPTY_GLYPH; sx as usize * sy as usize * sz as usize];
    for &[x, y, z] in cells {
        glyphs[owner_index(sx, sz, x, y, z)] = MARKER;
    }
    let slices = (0..sy)
        .map(|y| {
            (0..sz)
                .map(|z| {
                    (0..sx)
                        .map(|x| glyphs[owner_index(sx, sz, x, y, z)])
                        .collect::<String>()
                })
                .collect()
        })
        .collect();
    let marked = VoxelDocument {
        name: None,
        size,
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette: vec![PaletteEntry {
            glyph: MARKER.to_string(),
            color: [1.0, 1.0, 1.0, 1.0],
            emissive: None,
            metallic: None,
            roughness: None,
        }],
        slices,
    };
    marked
        .connectivity_components()
        .into_iter()
        .map(|component| component.cells)
        .collect()
}

/// Every overlap cluster of a composed grid, grouped **per owning pair**: a cell shared by three
/// parts belongs to each of its three pairs, so a pile-up reads as its pairwise joints rather than
/// one shapeless blob — the pair is what a verdict is about, since one of the two holds the pivot.
///
/// Ordered by pair (ascending authored indices), then by the cluster's lower corner — integers all
/// the way down, so the order never rests on a float comparison.
fn overlap_clusters(grid: &AssembledGrid) -> Vec<OverlapCluster> {
    let mut pairs: Vec<([usize; 2], Vec<[u32; 3]>)> = Vec::new();
    for (cell, owners) in grid.overlaps() {
        for (position, &a) in owners.iter().enumerate() {
            for &b in &owners[position + 1..] {
                // An assembly holds a handful of parts, so a linear search over the pairs seen so
                // far beats any index and keeps the result a plain `Vec`.
                match pairs.iter_mut().find(|(pair, _)| *pair == [a, b]) {
                    Some((_, cells)) => cells.push(cell),
                    None => pairs.push(([a, b], vec![cell])),
                }
            }
        }
    }
    pairs.sort_by_key(|(pair, _)| *pair);

    let mut clusters = Vec::new();
    for (pair, cells) in &pairs {
        for cluster in connected_clusters(grid.voxel.size, cells) {
            let mut min = [u32::MAX; 3];
            let mut max = [0u32; 3];
            let mut sum = [0.0f64; 3];
            for cell in &cluster {
                for axis in 0..3 {
                    min[axis] = min[axis].min(cell[axis]);
                    max[axis] = max[axis].max(cell[axis]);
                    sum[axis] += f64::from(cell[axis]) + 0.5;
                }
            }
            let count = cluster.len() as f64;
            clusters.push(OverlapCluster {
                pair: *pair,
                cells: cluster.len() as u64,
                min,
                max,
                center: [sum[0] / count, sum[1] / count, sum[2] / count],
            });
        }
    }
    clusters.sort_by_key(|cluster| (cluster.pair, cluster.min));
    clusters
}

/// The overlap / joint-plug lint (`L1143`, `Plan/voxel-animation.md` decision 10): classifies every
/// cluster of cells two parts both fill in the composed grid, and warns about the ones the hierarchy
/// and the declared pivots cannot explain.
///
/// Overlap itself is legal and expected — the joint-plug technique interpenetrates a limb with its
/// socket so rotation never opens a gap — so a cluster is **silent** when it is a part overlapping
/// its own parent within `plug_radius` of that child's declared pivot. Every other cluster warns:
/// a parent/child overlap far from the pivot (usually a wrong `pivot`, the valuable catch) names the
/// distance, and an overlap between parts with no parent/child relation names the pair. A warning
/// points at `/parts/<i>` of the child, or of the later-authored part when there is no child.
///
/// A heuristic warning, never an error (the `L1028`/`L1029` class): `ludion check-scene` still exits
/// success, because a deliberate interpenetration this lint cannot explain is legal content.
///
/// `parts` is aligned by index with [`AssemblyDocument::parts`] — `check_parts`'s convention — and
/// `grid` is `assembled_grid`'s output for that same pair of arguments.
///
/// # Panics
/// If `parts` is not the same length as [`AssemblyDocument::parts`].
pub fn overlap_lint(
    assembly: &AssemblyDocument,
    parts: &[PartDocument],
    grid: &AssembledGrid,
) -> Vec<Diagnostic> {
    assert_eq!(
        assembly.parts.len(),
        parts.len(),
        "one resolved part document per `parts` entry, in the assembly's own order"
    );
    let parents = parent_indices(assembly);
    let mut diagnostics = Vec::new();
    for cluster in overlap_clusters(grid) {
        let [a, b] = cluster.pair;
        let [minx, miny, minz] = cluster.min;
        let [maxx, maxy, maxz] = cluster.max;
        let span = format!(
            "a cluster of {} cell{} spanning ({minx},{miny},{minz})-({maxx},{maxy},{maxz})",
            cluster.cells,
            if cluster.cells == 1 { "" } else { "s" },
        );
        let child = if parents[b] == Some(a) {
            Some(b)
        } else if parents[a] == Some(b) {
            Some(a)
        } else {
            None
        };
        let Some(child) = child else {
            let later = a.max(b);
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_UNEXPECTED_OVERLAP,
                    format!(
                        "parts `{}` and `{}` overlap in {span}, but neither is the other's parent",
                        assembly.parts[a].name, assembly.parts[b].name,
                    ),
                )
                .with_path(format!("/parts/{later}"))
                .with_help(
                    "only a joint plug — a part interpenetrating its own parent around the pivot it \
                     turns about — is expected to overlap; move one part with `place` or trim its \
                     geometry so they no longer share cells, or, if this really is one joint, \
                     declare the hierarchy with `parent:` so the pivot check applies",
                ),
            );
            continue;
        };
        let parent = if child == a { b } else { a };
        let pivot = pivot_in_assembly(assembly, parts, child);
        let separation = distance(cluster.center, pivot);
        let radius = plug_radius(&parts[child]);
        if separation <= radius {
            // The joint plug itself: legal, expected, silent.
            continue;
        }
        let [px, py, pz] = pivot;
        let child_name = &assembly.parts[child].name;
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_UNEXPECTED_OVERLAP,
                format!(
                    "part `{child_name}` overlaps its parent `{}` in {span}, centered \
                     {separation:.2} cells from `{child_name}`'s pivot at ({},{},{}) — past this \
                     joint's plug radius {radius:.2}",
                    assembly.parts[parent].name,
                    format_float(px),
                    format_float(py),
                    format_float(pz),
                ),
            )
            .with_path(format!("/parts/{child}"))
            .with_help(format!(
                "a joint plug overlaps *around* the joint it turns about, so an overlap this far \
                 from the declared pivot is usually a wrong `pivot` on `{child_name}` — classically \
                 authored at the wrong end of the limb; move the pivot onto the socket the overlap \
                 sits in, or move the parts apart with `place` if they were never meant to \
                 interpenetrate"
            )),
        );
    }
    diagnostics
}

// --- Canonical writer ---------------------------------------------------------------------------

/// Writes `doc` in canonical form: `format`, `kind`, `name` (optional), `size`, `palette`, `parts`
/// (in authored order, each `name`, `file`, `parent?`, `place`, `pivot?`, `mirror?`), `clips`
/// (sorted by short name, omitted when empty) — the assembly analog of [`crate::voxel::emit`].
///
/// A pivot equal to the part-local origin and an unset mirror are the *absence* of their keys, so
/// each meaning has exactly one spelling (the `ease: linear` rule in [`crate::animation`]).
pub(crate) fn emit(doc: &AssemblyDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: assembly\n");
    if let Some(name) = &doc.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }

    out.push_str("size:\n");
    for dim in doc.size {
        out.push_str("  - ");
        out.push_str(&dim.to_string());
        out.push('\n');
    }

    crate::voxel::emit_palette(&mut out, &doc.palette);

    if doc.parts.is_empty() {
        out.push_str("parts: []\n");
    } else {
        out.push_str("parts:\n");
        for part in &doc.parts {
            out.push_str("  - name: ");
            out.push_str(&scalar_string(&part.name));
            out.push_str("\n    file: ");
            out.push_str(&scalar_string(&part.file));
            out.push('\n');
            if let Some(parent) = &part.parent {
                out.push_str("    parent: ");
                out.push_str(&scalar_string(parent));
                out.push('\n');
            }
            out.push_str("    place:\n");
            for component in part.place {
                out.push_str("      - ");
                out.push_str(&component.to_string());
                out.push('\n');
            }
            if part.pivot != [0.0, 0.0, 0.0] {
                out.push_str("    pivot:\n");
                for component in part.pivot {
                    out.push_str("      - ");
                    out.push_str(&format_float(component));
                    out.push('\n');
                }
            }
            if part.mirror_x {
                out.push_str("    mirror: ");
                out.push_str(MIRROR_AXIS);
                out.push('\n');
            }
        }
    }

    if !doc.clips.is_empty() {
        out.push_str("clips:\n");
        let mut clips = doc.clips.clone();
        clips.sort_by(|a, b| a.name.cmp(&b.name));
        for clip in &clips {
            out.push_str("  ");
            out.push_str(&scalar_string(&clip.name));
            out.push_str(": ");
            out.push_str(&scalar_string(&clip.file));
            out.push('\n');
        }
    }

    if !doc.joints.is_empty() {
        out.push_str("joints:\n");
        let mut joints = doc.joints.clone();
        joints.sort_by(|a, b| a.part.cmp(&b.part));
        for joint in &joints {
            out.push_str("  ");
            out.push_str(&scalar_string(&joint.part));
            out.push_str(":\n");
            match &joint.kind {
                AssemblyJointKind::Hinge { axis, limits } => {
                    out.push_str("    kind: hinge\n");
                    emit_joint_axis(&mut out, axis);
                    if let Some(pair) = limits {
                        emit_joint_limit_pair(&mut out, *pair);
                    }
                }
                AssemblyJointKind::Slider { axis, limits } => {
                    out.push_str("    kind: slider\n");
                    emit_joint_axis(&mut out, axis);
                    if let Some(pair) = limits {
                        emit_joint_limit_pair(&mut out, *pair);
                    }
                }
                AssemblyJointKind::Ball { limits } => {
                    out.push_str("    kind: ball\n");
                    if let Some(limits) = limits {
                        out.push_str("    limits:\n");
                        for pair in limits {
                            out.push_str("      -\n");
                            for component in pair {
                                out.push_str("        - ");
                                out.push_str(&format_float(*component));
                                out.push('\n');
                            }
                        }
                    }
                }
                AssemblyJointKind::Fixed => out.push_str("    kind: fixed\n"),
            }
            if let Some(break_impulse) = joint.break_impulse {
                out.push_str("    break_impulse: ");
                out.push_str(&format_float(break_impulse));
                out.push('\n');
            }
        }
    }

    out
}

/// Appends a joint's `axis:` block, omitted when it equals the default up vector (one spelling per
/// meaning, the [`emit`] convention throughout this format).
fn emit_joint_axis(out: &mut String, axis: &[f64; 3]) {
    if *axis == [0.0, 1.0, 0.0] {
        return;
    }
    out.push_str("    axis:\n");
    for component in axis {
        out.push_str("      - ");
        out.push_str(&format_float(*component));
        out.push('\n');
    }
}

/// Appends a hinge/slider joint's `limits: [min, max]` block.
fn emit_joint_limit_pair(out: &mut String, pair: [f64; 2]) {
    out.push_str("    limits:\n");
    for component in pair {
        out.push_str("      - ");
        out.push_str(&format_float(component));
        out.push('\n');
    }
}
