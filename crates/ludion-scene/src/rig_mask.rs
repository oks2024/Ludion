//! Named rig masks, shared by the document families that layer animation over a rig: a skinned
//! character's `masks:` name joints, an assembly's name parts (`Plan/animation-layers.md`
//! decisions 1 and 26).
//!
//! The structure and consistency codes differ per family, so each function takes them as parameters;
//! the message text is the same for both.

use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use serde_json::Value;

use crate::envelope;
use crate::fields::parse_number;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, NodeValue, pointer_token};

const MASK_KEYS: [&str; 3] = ["joints", "descendants", "weights"];

/// A named rig mask: which joints (or assembly parts) an `AnimationLayers` layer touches and how
/// strongly (`Plan/animation-layers.md` decision 1).
#[derive(Clone, Debug, PartialEq)]
pub struct RigMask {
    /// The short name a layer's `mask` selects by.
    pub name: String,
    /// The listed joints' (or parts') names, in authored order.
    pub joints: Vec<String>,
    /// Whether an unlisted joint inherits its nearest listed ancestor's weight.
    pub descendants: bool,
    /// One weight in `[0, 1]` per listed joint; empty means every listed joint weighs 1.0.
    pub weights: Vec<f64>,
}

/// Parses `masks`: a mapping of mask name to mask entry, returned sorted by name.
pub(crate) fn parse_masks(
    entry: &MapEntry,
    structure: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<RigMask> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure,
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
        if let Some(mask) = parse_mask_entry(sub, &pointer, structure, diagnostics) {
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
    structure: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<RigMask> {
    let NodeValue::Mapping(fields) = &sub.node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure,
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
                joints = parse_joint_names(&field.node, &field_pointer, structure, diagnostics);
                failed |= diagnostics.len() != before;
            }
            "descendants" => match &field.node.value {
                NodeValue::Scalar(Value::Bool(value)) => descendants = *value,
                other => {
                    failed = true;
                    diagnostics.push(
                        Diagnostic::new(
                            structure,
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
                weights = parse_mask_weights(&field.node, &field_pointer, structure, diagnostics);
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
                    structure,
                ));
            }
        }
    }
    let Some(joints) = joints else {
        if !failed {
            diagnostics.push(
                Diagnostic::new(structure, "a mask entry is missing its required `joints`")
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
                structure,
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
    Some(RigMask {
        name: sub.key.clone(),
        joints,
        descendants,
        weights: weights.unwrap_or_default(),
    })
}

/// Parses a non-empty sequence of unique joint-name strings.
fn parse_joint_names(
    node: &yaml::Node,
    pointer: &str,
    structure: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<String>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure,
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
            Diagnostic::new(structure, "`joints` is declared but has zero entries")
                .with_path(pointer.to_string())
                .with_help("list at least one joint, or remove the mask"),
        );
        return None;
    }
    let before = diagnostics.len();
    let mut names: Vec<String> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_pointer = format!("{pointer}/{index}");
        let Some(name) =
            envelope::expect_string(item, &item_pointer, "a joint name", structure, diagnostics)
        else {
            continue;
        };
        if names.contains(&name) {
            diagnostics.push(
                Diagnostic::new(
                    structure,
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
    structure: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<f64>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure,
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
        let Some(weight) = parse_number(item, &item_pointer, "a weight", structure, diagnostics)
        else {
            continue;
        };
        if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
            diagnostics.push(
                Diagnostic::new(
                    structure,
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

/// Reports a mask with an empty name (the consistency code).
pub(crate) fn validate_mask_names(
    masks: &[RigMask],
    consistency: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for mask in masks {
        if mask.name.trim().is_empty() {
            diagnostics.push(
                Diagnostic::new(consistency, "a mask has an empty name")
                    .with_path(format!("/masks/{}", pointer_token(&mask.name)))
                    .with_help("name the mask the way a layer selects it — `upper_body`"),
            );
        }
    }
}

/// Appends the canonical `masks:` block (sorted by name); writes nothing for an empty list.
pub(crate) fn emit_masks(out: &mut String, masks: &[RigMask]) {
    if masks.is_empty() {
        return;
    }
    out.push_str("masks:\n");
    let mut sorted: Vec<&RigMask> = masks.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for mask in sorted {
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

/// Resolves a mask to one weight per rig element, in the rig's order. `joints` is `(name, parent
/// index)` per element, in any order (an assembly's parts are not topological). A listed element
/// takes its weight (1.0 without `weights`); with `descendants`, an unlisted element inherits its
/// nearest listed ancestor's weight; every other element is 0. A parent cycle or an out-of-range
/// parent ends the walk with no inherited weight.
#[must_use]
pub(crate) fn resolve_mask(mask: &RigMask, joints: &[(&str, Option<u32>)]) -> Vec<f32> {
    let listed: Vec<Option<f32>> = joints
        .iter()
        .map(|(name, _)| {
            mask.joints
                .iter()
                .position(|joint| joint == name)
                .map(|i| mask.weights.get(i).map_or(1.0, |w| *w as f32))
        })
        .collect();
    (0..joints.len())
        .map(|start| {
            let mut at = Some(start);
            let mut guard = 0;
            while let Some(i) = at {
                if let Some(weight) = listed[i] {
                    return weight;
                }
                guard += 1;
                if !mask.descendants || guard > joints.len() {
                    break;
                }
                at = joints[i]
                    .1
                    .map(|p| p as usize)
                    .filter(|p| *p < joints.len());
            }
            0.0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(joints: &[&str], descendants: bool) -> RigMask {
        RigMask {
            name: "m".into(),
            joints: joints.iter().map(|s| (*s).to_string()).collect(),
            descendants,
            weights: Vec::new(),
        }
    }

    #[test]
    fn child_authored_before_parent_inherits() {
        // leg is authored before its parent hip; hip is listed.
        let rig = [("leg", Some(1)), ("hip", Some(2)), ("root", None)];
        let weights = resolve_mask(&mask(&["hip"], true), &rig);
        assert_eq!(weights, vec![1.0, 1.0, 0.0]);
        let flat = resolve_mask(&mask(&["hip"], false), &rig);
        assert_eq!(flat, vec![0.0, 1.0, 0.0]);
    }

    #[test]
    fn parent_first_order_is_unchanged() {
        let rig = [("root", None), ("hip", Some(0)), ("leg", Some(1))];
        assert_eq!(
            resolve_mask(&mask(&["hip"], true), &rig),
            vec![0.0, 1.0, 1.0]
        );
    }

    #[test]
    fn a_parent_cycle_terminates() {
        let rig = [("a", Some(1)), ("b", Some(0))];
        assert_eq!(resolve_mask(&mask(&["z"], true), &rig), vec![0.0, 0.0]);
    }
}
