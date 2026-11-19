//! An assembly's `ik:` key: named two-part chains and look-ats over rigid parts
//! (`Plan/animation-layers.md` decisions 36–37). A chain bends two parts toward a target by a
//! closed-form solve; a look-at turns a part mask toward a direction. Targets arrive at run time
//! through the `IkTargets` component, so this document holds only the rig.
//!
//! Structure and shape are `L1137`; the cross checks against parts, masks and joints are `L1138`
//! ([`validate_ik`]).

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use serde_json::Value;

use crate::assembly::{AssemblyDocument, AssemblyJointKind};
use crate::envelope;
use crate::fields::parse_number;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, NodeValue, pointer_token};

const IK_ENTRY_KEYS: [&str; 7] = [
    "parts",
    "effector",
    "hinge",
    "ground",
    "aim",
    "forward",
    "limits_deg",
];
const GROUND_KEYS: [&str; 2] = ["max_adjust", "root_drop"];
const HINGE_KEYS: [&str; 2] = ["axis", "bend"];
const LIMIT_KEYS: [&str; 2] = ["yaw", "pitch"];

/// The look-at forward when `forward` is omitted: the engine's `-Z`.
pub const DEFAULT_LOOK_FORWARD: [f64; 3] = [0.0, 0.0, -1.0];
/// The look-at limits `[yaw, pitch]` in degrees when `limits_deg` omits a side.
pub const DEFAULT_LOOK_LIMITS_DEG: [f64; 2] = [180.0, 180.0];

/// An authored hinge for a two-part chain: the flex axis in the first part's frame and the bend side.
#[derive(Clone, Debug, PartialEq)]
pub struct IkHinge {
    /// The flex axis, in the first (upper) part's local frame; finite and non-zero.
    pub axis: [f64; 3],
    /// `1` or `-1`: the sign of the flex angle the target's distance asks for.
    pub bend: i8,
}

/// What an `ik:` entry solves.
#[derive(Clone, Debug, PartialEq)]
pub enum IkKind {
    /// A two-part chain: `parts[1]` is a child of `parts[0]`, and its `effector` is driven to the
    /// target.
    Chain {
        /// The upper and lower part names.
        parts: [String; 2],
        /// The effector in the lower part's part-local, unmirrored voxel coordinates.
        effector: [f64; 3],
        /// The authored hinge; absent resolves from the lower part's `joints:` entry or the
        /// humanoid naming convention.
        hinge: Option<IkHinge>,
        /// Whether the effector is placed on the static ground under it (needs the document's
        /// top-level `ground:` block).
        ground: bool,
    },
    /// A look-at: the `aim` mask's parts turn together so `forward` faces the target.
    LookAt {
        /// The `masks:` entry naming the aim parts and their shares.
        aim: String,
        /// The facing direction in the assembly frame; finite and non-zero.
        forward: [f64; 3],
        /// The yaw and pitch limits in degrees, each in `(0, 180]`.
        limits_deg: [f64; 2],
    },
}

/// The top-level `ground:` block: how far a grounded chain may move its effector and whether the
/// whole assembly may drop so the lower sole reaches.
#[derive(Clone, Debug, PartialEq)]
pub struct IkGround {
    /// The largest move of a foot up or down, in the assembly's voxels; finite and positive.
    pub max_adjust: f64,
    /// Whether the assembly root lowers to the lowest foot's ground.
    pub root_drop: bool,
}

/// One named `ik:` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct IkEntry {
    /// The name `IkTargets` targets by.
    pub name: String,
    /// The chain or look-at.
    pub kind: IkKind,
}

/// Parses `ik`: a mapping of name to entry, returned sorted by name.
pub(crate) fn parse_ik(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<IkEntry> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!(
                    "`ik` must be a mapping of name to chain or look-at, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/ik")
            .with_help(
                "write one entry per chain or look-at:\n  ik:\n    look:\n      aim: head_look",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for sub in entries {
        let pointer = format!("/ik/{}", pointer_token(&sub.key));
        if let Some(kind) = parse_entry(sub, &pointer, diagnostics) {
            out.push(IkEntry {
                name: sub.key.clone(),
                kind,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parses the top-level `ground:` block.
pub(crate) fn parse_ground(
    entry: &MapEntry,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<IkGround> {
    let NodeValue::Mapping(fields) = &entry.node.value else {
        diagnostics.push(structure(
            format!(
                "`ground` must be a mapping with `max_adjust`, found {} (line {})",
                entry.node.value.type_phrase(),
                entry.node.line
            ),
            "/ground",
            "write:\n  ground:\n    max_adjust: 3.0",
        ));
        return None;
    };
    let before = diagnostics.len();
    let mut max_adjust = None;
    let mut root_drop = false;
    for field in fields {
        let pointer = format!("/ground/{}", field.key);
        match field.key.as_str() {
            "max_adjust" => {
                max_adjust = parse_number(
                    &field.node,
                    &pointer,
                    "max_adjust",
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    diagnostics,
                )
                .and_then(|value| {
                    if value.is_finite() && value > 0.0 {
                        Some(value)
                    } else {
                        diagnostics.push(structure(
                            format!(
                                "`max_adjust` must be finite and greater than 0, found {value}"
                            ),
                            &pointer,
                            "give the largest foot move in voxels, e.g. `3.0`",
                        ));
                        None
                    }
                });
            }
            "root_drop" => match &field.node.value {
                NodeValue::Scalar(Value::Bool(value)) => root_drop = *value,
                other => diagnostics.push(structure(
                    format!(
                        "`root_drop` must be a boolean, found {} (line {})",
                        other.type_phrase(),
                        field.node.line
                    ),
                    &pointer,
                    "write `root_drop: true` or remove the key",
                )),
            },
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                field,
                "/ground",
                "`ground`",
                GROUND_KEYS,
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if diagnostics.len() != before {
        return None;
    }
    let Some(max_adjust) = max_adjust else {
        diagnostics.push(structure(
            "`ground` is missing its required `max_adjust`".to_string(),
            "/ground",
            "add `max_adjust: 3.0`, the largest foot move in voxels",
        ));
        return None;
    };
    Some(IkGround {
        max_adjust,
        root_drop,
    })
}

fn structure(message: String, pointer: &str, help: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::ASSEMBLY_DOCUMENT_STRUCTURE, message)
        .with_path(pointer.to_string())
        .with_help(help.into())
}

fn parse_entry(sub: &MapEntry, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<IkKind> {
    let NodeValue::Mapping(fields) = &sub.node.value else {
        diagnostics.push(structure(
            format!(
                "an `ik` entry must be a mapping with `parts` or `aim`, found {} (line {})",
                sub.node.value.type_phrase(),
                sub.node.line
            ),
            pointer,
            "a chain has `parts` and `effector`; a look-at has `aim`",
        ));
        return None;
    };
    let before = diagnostics.len();
    let mut parts = None;
    let mut effector = None;
    let mut hinge = None;
    let mut ground = None;
    let mut aim = None;
    let mut forward = None;
    let mut limits = None;
    for field in fields {
        let field_pointer = format!("{pointer}/{}", field.key);
        match field.key.as_str() {
            "parts" => parts = parse_parts(&field.node, &field_pointer, diagnostics),
            "effector" => {
                effector = parse_vec3(&field.node, &field_pointer, "effector", false, diagnostics);
            }
            "hinge" => hinge = parse_hinge(&field.node, &field_pointer, diagnostics),
            "aim" => {
                aim = envelope::expect_string(
                    &field.node,
                    &field_pointer,
                    "an aim mask name",
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "forward" => {
                forward = parse_vec3(&field.node, &field_pointer, "forward", true, diagnostics);
            }
            "limits_deg" => limits = parse_limits(&field.node, &field_pointer, diagnostics),
            "ground" => match &field.node.value {
                NodeValue::Scalar(Value::Bool(value)) => ground = Some(*value),
                other => diagnostics.push(structure(
                    format!(
                        "`ground` must be a boolean, found {} (line {})",
                        other.type_phrase(),
                        field.node.line
                    ),
                    &field_pointer,
                    "write `ground: true` on a chain, or remove the key",
                )),
            },
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                field,
                pointer,
                "an `ik` entry",
                IK_ENTRY_KEYS,
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if diagnostics.len() != before {
        return None;
    }
    let is_chain = fields.iter().any(|f| f.key == "parts");
    let is_look = fields.iter().any(|f| f.key == "aim");
    match (is_chain, is_look) {
        (true, false) => {
            for (key, present) in [
                ("forward", forward.is_some()),
                ("limits_deg", limits.is_some()),
            ] {
                if present {
                    diagnostics.push(structure(
                        format!("a chain entry cannot have `{key}`, which belongs to a look-at"),
                        &format!("{pointer}/{key}"),
                        "remove the key, or split the look-at into its own entry",
                    ));
                }
            }
            let Some(effector) = effector else {
                diagnostics.push(structure(
                    "a chain entry is missing its required `effector`".to_string(),
                    pointer,
                    "add `effector`: three numbers, part-local voxels on the second part",
                ));
                return None;
            };
            if diagnostics.len() != before {
                return None;
            }
            Some(IkKind::Chain {
                parts: parts?,
                effector,
                hinge,
                ground: ground.unwrap_or(false),
            })
        }
        (false, true) => {
            for (key, present) in [
                ("effector", effector.is_some()),
                ("hinge", hinge.is_some()),
                ("ground", ground.is_some()),
            ] {
                if present {
                    diagnostics.push(structure(
                        format!("a look-at entry cannot have `{key}`, which belongs to a chain"),
                        &format!("{pointer}/{key}"),
                        "remove the key, or split the chain into its own entry",
                    ));
                }
            }
            if diagnostics.len() != before {
                return None;
            }
            Some(IkKind::LookAt {
                aim: aim?,
                forward: forward.unwrap_or(DEFAULT_LOOK_FORWARD),
                limits_deg: limits.unwrap_or(DEFAULT_LOOK_LIMITS_DEG),
            })
        }
        _ => {
            diagnostics.push(structure(
                if is_chain {
                    "an `ik` entry has both `parts` and `aim`".to_string()
                } else {
                    "an `ik` entry has neither `parts` nor `aim`".to_string()
                },
                pointer,
                "a chain has `parts` and `effector`; a look-at has `aim`; an entry is exactly one",
            ));
            None
        }
    }
}

fn parse_parts(
    node: &yaml::Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[String; 2]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(structure(
            format!(
                "`parts` must be a sequence of two part names, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ),
            pointer,
            "write the upper then the lower part:\n      parts:\n        - upper-arm-r\n        - lower-arm-r",
        ));
        return None;
    };
    if items.len() != 2 {
        diagnostics.push(structure(
            format!("`parts` must name exactly two parts, found {}", items.len()),
            pointer,
            if items.len() > 2 {
                "a third part is not supported; chain the upper and lower part only"
            } else {
                "name the upper part then the lower part"
            },
        ));
        return None;
    }
    let mut names = Vec::with_capacity(2);
    for (index, item) in items.iter().enumerate() {
        names.push(envelope::expect_string(
            item,
            &format!("{pointer}/{index}"),
            "a part name",
            code::ASSEMBLY_DOCUMENT_STRUCTURE,
            diagnostics,
        )?);
    }
    let lower = names.pop()?;
    let upper = names.pop()?;
    Some([upper, lower])
}

/// Parses three numbers. `non_zero` also requires the vector be non-zero (a direction); both
/// require finite components.
fn parse_vec3(
    node: &yaml::Node,
    pointer: &str,
    field: &str,
    non_zero: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; 3]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(structure(
            format!(
                "`{field}` must be a sequence of three numbers, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ),
            pointer,
            format!("write `{field}` one component per line:\n        - 0.0\n        - 0.0\n        - -1.0"),
        ));
        return None;
    };
    if items.len() != 3 {
        diagnostics.push(structure(
            format!(
                "`{field}` must have exactly 3 components (x, y, z), found {}",
                items.len()
            ),
            pointer,
            "supply exactly three numbers",
        ));
        return None;
    }
    let mut out = [0.0f64; 3];
    let before = diagnostics.len();
    for (index, item) in items.iter().enumerate() {
        if let Some(value) = parse_number(
            item,
            &format!("{pointer}/{index}"),
            field,
            code::ASSEMBLY_DOCUMENT_STRUCTURE,
            diagnostics,
        ) {
            out[index] = value;
        }
    }
    if diagnostics.len() != before {
        return None;
    }
    if out.iter().any(|c| !c.is_finite()) {
        diagnostics.push(structure(
            format!("`{field}` components must be finite"),
            pointer,
            "use plain numbers",
        ));
        return None;
    }
    if non_zero && out == [0.0, 0.0, 0.0] {
        diagnostics.push(structure(
            format!("`{field}` must not be the zero vector"),
            pointer,
            "give a direction, e.g. `[0.0, 0.0, -1.0]`",
        ));
        return None;
    }
    Some(out)
}

fn parse_hinge(
    node: &yaml::Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<IkHinge> {
    let NodeValue::Mapping(fields) = &node.value else {
        diagnostics.push(structure(
            format!(
                "`hinge` must be a mapping with `axis` and `bend`, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ),
            pointer,
            "write:\n      hinge:\n        axis:\n          - 1.0\n          - 0.0\n          - 0.0\n        bend: 1",
        ));
        return None;
    };
    let before = diagnostics.len();
    let mut axis = None;
    let mut bend = None;
    for field in fields {
        let field_pointer = format!("{pointer}/{}", field.key);
        match field.key.as_str() {
            "axis" => axis = parse_vec3(&field.node, &field_pointer, "axis", true, diagnostics),
            "bend" => {
                bend = parse_number(
                    &field.node,
                    &field_pointer,
                    "bend",
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                    diagnostics,
                )
                .and_then(|value| {
                    if value == 1.0 || value == -1.0 {
                        Some(value as i8)
                    } else {
                        diagnostics.push(structure(
                            format!("`bend` must be 1 or -1, found {value}"),
                            &field_pointer,
                            "1 flexes about the axis in its positive sense, -1 the other way",
                        ));
                        None
                    }
                });
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                field,
                pointer,
                "a `hinge`",
                HINGE_KEYS,
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if diagnostics.len() != before {
        return None;
    }
    match (axis, bend) {
        (Some(axis), Some(bend)) => Some(IkHinge { axis, bend }),
        _ => {
            diagnostics.push(structure(
                "a `hinge` needs both `axis` and `bend`".to_string(),
                pointer,
                "add the missing key, or remove `hinge` to take the axis from `joints:` or the naming convention",
            ));
            None
        }
    }
}

fn parse_limits(
    node: &yaml::Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; 2]> {
    let NodeValue::Mapping(fields) = &node.value else {
        diagnostics.push(structure(
            format!(
                "`limits_deg` must be a mapping with `yaw` and `pitch`, found {} (line {})",
                node.value.type_phrase(),
                node.line
            ),
            pointer,
            "write:\n      limits_deg:\n        yaw: 70.0\n        pitch: 40.0",
        ));
        return None;
    };
    let before = diagnostics.len();
    let mut out = DEFAULT_LOOK_LIMITS_DEG;
    for field in fields {
        let field_pointer = format!("{pointer}/{}", field.key);
        let slot = match field.key.as_str() {
            "yaw" => 0,
            "pitch" => 1,
            unknown => {
                diagnostics.push(envelope::unknown_key(
                    unknown,
                    field,
                    pointer,
                    "`limits_deg`",
                    LIMIT_KEYS,
                    code::ASSEMBLY_DOCUMENT_STRUCTURE,
                ));
                continue;
            }
        };
        let Some(value) = parse_number(
            &field.node,
            &field_pointer,
            &field.key,
            code::ASSEMBLY_DOCUMENT_STRUCTURE,
            diagnostics,
        ) else {
            continue;
        };
        if !value.is_finite() || value <= 0.0 || value > 180.0 {
            diagnostics.push(structure(
                format!("a look-at limit must be in (0, 180] degrees, found {value}"),
                &field_pointer,
                "use 180 for no limit on that axis",
            ));
            continue;
        }
        out[slot] = value;
    }
    (diagnostics.len() == before).then_some(out)
}

/// The `ik` cross checks (`L1138`): parts exist and chain, an aim names a mask, and a chain
/// without a `hinge:` has a hinge to take.
pub(crate) fn validate_ik(doc: &AssemblyDocument, diagnostics: &mut Vec<Diagnostic>) {
    let part_names: Vec<&str> = doc.parts.iter().map(|p| p.name.as_str()).collect();
    for entry in &doc.ik {
        let pointer = format!("/ik/{}", pointer_token(&entry.name));
        if entry.name.trim().is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::ASSEMBLY_DATA_CONSISTENCY,
                    "an `ik` entry has an empty name",
                )
                .with_path(pointer.clone())
                .with_help("name the entry the way `IkTargets` targets it — `hand_r`, `look`"),
            );
        }
        match &entry.kind {
            IkKind::Chain {
                parts,
                effector: _,
                hinge,
                ground,
            } => {
                if *ground && doc.ground.is_none() {
                    diagnostics.push(
                        Diagnostic::new(
                            code::ASSEMBLY_DATA_CONSISTENCY,
                            format!(
                                "the chain `{}` is `ground: true` but the assembly has no `ground:` block",
                                entry.name
                            ),
                        )
                        .with_path(format!("{pointer}/ground"))
                        .with_help("add a top-level block:\nground:\n  max_adjust: 3.0"),
                    );
                }
                let mut known = true;
                for (index, part) in parts.iter().enumerate() {
                    if !part_names.contains(&part.as_str()) {
                        known = false;
                        diagnostics.push(unknown_name(
                            &entry.name,
                            "part",
                            part,
                            &part_names,
                            format!("{pointer}/parts/{index}"),
                        ));
                    }
                }
                if !known {
                    continue;
                }
                let lower = doc.parts.iter().find(|p| p.name == parts[1]);
                if lower.and_then(|p| p.parent.as_deref()) != Some(parts[0].as_str()) {
                    diagnostics.push(
                        Diagnostic::new(
                            code::ASSEMBLY_DATA_CONSISTENCY,
                            format!(
                                "the chain `{}`'s second part `{}` is not a child of `{}`",
                                entry.name, parts[1], parts[0]
                            ),
                        )
                        .with_path(format!("{pointer}/parts"))
                        .with_help("list the upper part first, then its child"),
                    );
                    continue;
                }
                if hinge.is_none() {
                    check_hinge_source(doc, &entry.name, &parts[1], &pointer, diagnostics);
                }
            }
            IkKind::LookAt { aim, .. } => {
                if !doc.masks.iter().any(|m| &m.name == aim) {
                    diagnostics.push(unknown_name(
                        &entry.name,
                        "mask",
                        aim,
                        &doc.masks
                            .iter()
                            .map(|m| m.name.as_str())
                            .collect::<Vec<_>>(),
                        format!("{pointer}/aim"),
                    ));
                }
            }
        }
    }
    let grounded = doc
        .ik
        .iter()
        .any(|e| matches!(e.kind, IkKind::Chain { ground: true, .. }));
    if doc.ground.is_some() && !grounded {
        diagnostics.push(
            Diagnostic::new(
                code::ASSEMBLY_DATA_CONSISTENCY,
                "the assembly has a `ground:` block but no `ik:` chain is `ground: true`",
            )
            .with_path("/ground")
            .with_help("add `ground: true` to a foot chain, or remove the `ground:` block"),
        );
    }
}

fn unknown_name(
    entry: &str,
    noun: &str,
    name: &str,
    candidates: &[&str],
    pointer: String,
) -> Diagnostic {
    let help = match suggest::nearest(name, candidates.iter().copied()) {
        Some(candidate) => format!("nearest {noun} is `{candidate}`"),
        None => format!("name a {noun} of this assembly"),
    };
    Diagnostic::new(
        code::ASSEMBLY_DATA_CONSISTENCY,
        format!("the `ik` entry `{entry}` names `{name}`, which is not a {noun} of this assembly"),
    )
    .with_path(pointer)
    .with_help(help)
}

/// A chain with no `hinge:` takes the lower part's `joints:` hinge, else the naming convention.
fn check_hinge_source(
    doc: &AssemblyDocument,
    entry: &str,
    lower: &str,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let message = match doc.joints.iter().find(|j| j.part == lower) {
        Some(joint) if matches!(joint.kind, AssemblyJointKind::Hinge { .. }) => return,
        Some(_) => format!(
            "the chain `{entry}` has no `hinge:` and its second part `{lower}` has a joint that is not a hinge"
        ),
        None if lower.starts_with("lower-leg-") || lower.starts_with("lower-arm-") => return,
        None => format!(
            "the chain `{entry}` has no `hinge:` and its second part `{lower}` is not named `lower-leg-*` or `lower-arm-*`"
        ),
    };
    diagnostics.push(
        Diagnostic::new(code::ASSEMBLY_DATA_CONSISTENCY, message)
            .with_path(pointer.to_string())
            .with_help(
                "add a `hinge:` with `axis` and `bend`, or give the second part a hinge in `joints:`",
            ),
    );
}

/// Appends the canonical `ik:` block (sorted by name); writes nothing for an empty list.
pub(crate) fn emit_ik(out: &mut String, entries: &[IkEntry]) {
    if entries.is_empty() {
        return;
    }
    out.push_str("ik:\n");
    let mut sorted: Vec<&IkEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in sorted {
        out.push_str("  ");
        out.push_str(&scalar_string(&entry.name));
        out.push_str(":\n");
        match &entry.kind {
            IkKind::Chain {
                parts,
                effector,
                hinge,
                ground,
            } => {
                out.push_str("    parts:\n");
                for part in parts {
                    out.push_str("      - ");
                    out.push_str(&scalar_string(part));
                    out.push('\n');
                }
                emit_vec3(out, "    effector:\n", "      - ", effector);
                if let Some(hinge) = hinge {
                    out.push_str("    hinge:\n");
                    emit_vec3(out, "      axis:\n", "        - ", &hinge.axis);
                    out.push_str(&format!("      bend: {}\n", hinge.bend));
                }
                if *ground {
                    out.push_str("    ground: true\n");
                }
            }
            IkKind::LookAt {
                aim,
                forward,
                limits_deg,
            } => {
                out.push_str("    aim: ");
                out.push_str(&scalar_string(aim));
                out.push('\n');
                if *forward != DEFAULT_LOOK_FORWARD {
                    emit_vec3(out, "    forward:\n", "      - ", forward);
                }
                if *limits_deg != DEFAULT_LOOK_LIMITS_DEG {
                    out.push_str("    limits_deg:\n");
                    for (key, value, default) in [
                        ("yaw", limits_deg[0], DEFAULT_LOOK_LIMITS_DEG[0]),
                        ("pitch", limits_deg[1], DEFAULT_LOOK_LIMITS_DEG[1]),
                    ] {
                        if value != default {
                            out.push_str("      ");
                            out.push_str(key);
                            out.push_str(": ");
                            out.push_str(&format_float(value));
                            out.push('\n');
                        }
                    }
                }
            }
        }
    }
}

/// Appends the canonical top-level `ground:` block; writes nothing when absent.
pub(crate) fn emit_ground(out: &mut String, ground: Option<&IkGround>) {
    let Some(ground) = ground else {
        return;
    };
    out.push_str("ground:\n  max_adjust: ");
    out.push_str(&format_float(ground.max_adjust));
    out.push('\n');
    if ground.root_drop {
        out.push_str("  root_drop: true\n");
    }
}

fn emit_vec3(out: &mut String, header: &str, bullet: &str, value: &[f64; 3]) {
    out.push_str(header);
    for component in value {
        out.push_str(bullet);
        out.push_str(&format_float(*component));
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use crate::assembly::{emit, parse, validate};

    const HEAD: &str = "format: 1\nkind: assembly\nsize:\n  - 4\n  - 4\n  - 4\npalette:\n  A:\n    color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nparts:\n";
    const PARTS: &str = "  - name: upper-arm-r\n    file: u.part.yaml\n    place:\n      - 0\n      - 0\n      - 0\n  - name: lower-arm-r\n    file: l.part.yaml\n    parent: upper-arm-r\n    place:\n      - 0\n      - 0\n      - 0\n  - name: head\n    file: h.part.yaml\n    parent: upper-arm-r\n    place:\n      - 0\n      - 0\n      - 0\n";
    const MASKS: &str = "masks:\n  head_look:\n    joints:\n      - head\n";

    fn doc(ik: &str) -> String {
        format!("{HEAD}{PARTS}{MASKS}{ik}")
    }

    fn errors(source: &str) -> Vec<(String, String)> {
        let parsed = parse(source).expect("parses");
        validate(&parsed)
            .into_iter()
            .map(|d| (d.code.to_string(), d.path.unwrap_or_default()))
            .collect()
    }

    const CHAIN_AND_LOOK: &str = "ik:\n  hand_r:\n    parts:\n      - upper-arm-r\n      - lower-arm-r\n    effector:\n      - 1.0\n      - 0.0\n      - 1.0\n    hinge:\n      axis:\n        - 1.0\n        - 0.0\n        - 0.0\n      bend: -1\n  look:\n    aim: head_look\n    forward:\n      - 0.0\n      - 0.0\n      - 1.0\n    limits_deg:\n      yaw: 70.0\n      pitch: 40.0\n";

    #[test]
    fn a_chain_and_a_look_at_round_trip() {
        let source = doc(CHAIN_AND_LOOK);
        let parsed = parse(&source).expect("parses");
        assert_eq!(parsed.ik.len(), 2);
        assert_eq!(emit(&parsed), source);
        assert!(errors(&source).is_empty());
    }

    #[test]
    fn defaults_are_omitted() {
        let source = doc("ik:\n  look:\n    aim: head_look\n");
        let parsed = parse(&source).expect("parses");
        assert_eq!(emit(&parsed), source);
    }

    #[test]
    fn a_chain_with_no_hinge_takes_the_convention() {
        let source = doc(
            "ik:\n  hand_r:\n    parts:\n      - upper-arm-r\n      - lower-arm-r\n    effector:\n      - 1.0\n      - 0.0\n      - 1.0\n",
        );
        assert!(errors(&source).is_empty());
    }

    fn structure_error(ik: &str, pointer: &str) {
        let found = errors_parse(&doc(ik));
        assert!(
            found.contains(&("L1137".to_string(), pointer.to_string())),
            "{ik}\n{found:?}"
        );
    }

    fn errors_parse(source: &str) -> Vec<(String, String)> {
        match parse(source) {
            Ok(_) => Vec::new(),
            Err(list) => list
                .into_iter()
                .map(|d| (d.code.to_string(), d.path.unwrap_or_default()))
                .collect(),
        }
    }

    #[test]
    fn structure_cases_are_l1137() {
        structure_error(
            "ik:\n  x:\n    aim: m\n    parts:\n      - a\n      - b\n",
            "/ik/x",
        );
        structure_error(
            "ik:\n  x:\n    forward:\n      - 0.0\n      - 0.0\n      - 1.0\n",
            "/ik/x",
        );
        structure_error(
            "ik:\n  x:\n    parts:\n      - a\n      - b\n      - c\n    effector:\n      - 0.0\n      - 0.0\n      - 0.0\n",
            "/ik/x/parts",
        );
        structure_error(
            "ik:\n  x:\n    parts:\n      - a\n      - b\n    ground: 3\n    effector:\n      - 0.0\n      - 0.0\n      - 0.0\n",
            "/ik/x/ground",
        );
        structure_error("ik:\n  x:\n    aim: m\n    ground: true\n", "/ik/x/ground");
        structure_error(
            "ik:\n  x:\n    aim: m\n    limits_deg:\n      yaw: 0.0\n",
            "/ik/x/limits_deg/yaw",
        );
        structure_error(
            "ik:\n  x:\n    aim: m\n    limits_deg:\n      yaw: 181.0\n",
            "/ik/x/limits_deg/yaw",
        );
        structure_error(
            "ik:\n  x:\n    aim: m\n    forward:\n      - 0.0\n      - 0.0\n      - 0.0\n",
            "/ik/x/forward",
        );
        structure_error(
            "ik:\n  x:\n    parts:\n      - a\n      - b\n    effector:\n      - 0.0\n      - 0.0\n      - 0.0\n    hinge:\n      axis:\n        - 0.0\n        - 0.0\n        - 0.0\n      bend: 1\n",
            "/ik/x/hinge/axis",
        );
        structure_error(
            "ik:\n  x:\n    parts:\n      - a\n      - b\n    effector:\n      - 0.0\n      - 0.0\n      - 0.0\n    hinge:\n      axis:\n        - 1.0\n        - 0.0\n        - 0.0\n      bend: 2\n",
            "/ik/x/hinge/bend",
        );
    }

    const GROUND_CHAIN: &str = "ik:\n  foot_r:\n    parts:\n      - upper-arm-r\n      - lower-arm-r\n    effector:\n      - 1.0\n      - 0.0\n      - 1.0\n    ground: true\n";

    #[test]
    fn a_ground_chain_and_block_round_trip() {
        let source = doc(&format!(
            "{GROUND_CHAIN}ground:\n  max_adjust: 3.0\n  root_drop: true\n"
        ));
        let parsed = parse(&source).expect("parses");
        assert_eq!(parsed.ground.as_ref().map(|g| g.root_drop), Some(true));
        assert_eq!(emit(&parsed), source);
        assert!(errors(&source).is_empty());
        let no_drop = doc(&format!("{GROUND_CHAIN}ground:\n  max_adjust: 2.5\n"));
        assert_eq!(emit(&parse(&no_drop).expect("parses")), no_drop);
    }

    #[test]
    fn ground_block_shape_errors_are_l1137() {
        structure_error("ground: 3\n", "/ground");
        structure_error("ground:\n  root_drop: true\n", "/ground");
        structure_error("ground:\n  max_adjust: 0.0\n", "/ground/max_adjust");
        structure_error("ground:\n  max_adjust: -1.0\n", "/ground/max_adjust");
        structure_error(
            "ground:\n  max_adjust: 3.0\n  root_drop: 1\n",
            "/ground/root_drop",
        );
        structure_error("ground:\n  max_adjust: 3.0\n  speed: 1\n", "/ground/speed");
    }

    #[test]
    fn ground_cross_checks_are_l1138() {
        consistency_error(GROUND_CHAIN, "/ik/foot_r/ground");
        consistency_error("ground:\n  max_adjust: 3.0\n", "/ground");
    }

    fn consistency_error(ik: &str, pointer: &str) {
        let found = errors(&doc(ik));
        assert!(
            found.contains(&("L1138".to_string(), pointer.to_string())),
            "{ik}\n{found:?}"
        );
    }

    #[test]
    fn cross_checks_are_l1138() {
        let chain = |a: &str, b: &str| {
            format!(
                "ik:\n  x:\n    parts:\n      - {a}\n      - {b}\n    effector:\n      - 0.0\n      - 0.0\n      - 0.0\n"
            )
        };
        consistency_error(&chain("upper-arm-r", "lower-arm-l"), "/ik/x/parts/1");
        consistency_error(&chain("lower-arm-r", "upper-arm-r"), "/ik/x/parts");
        // `head` is a child of the upper arm but is neither a lower limb nor jointed.
        consistency_error(&chain("upper-arm-r", "head"), "/ik/x");
        consistency_error("ik:\n  x:\n    aim: nope\n", "/ik/x/aim");
    }

    #[test]
    fn a_non_hinge_joint_needs_an_authored_hinge() {
        let source = format!(
            "{}joints:\n  lower-arm-r:\n    kind: ball\n",
            doc(
                "ik:\n  x:\n    parts:\n      - upper-arm-r\n      - lower-arm-r\n    effector:\n      - 0.0\n      - 0.0\n      - 0.0\n"
            )
        );
        assert!(errors(&source).contains(&("L1138".to_string(), "/ik/x".to_string())));
    }
}
