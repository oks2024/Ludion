//! Shared readers for the numeric fields of the crate's hand-parsed kind documents.
//!
//! The engine holds document numbers as `f32`, but the YAML tree yields `f64`. A value such as
//! `1e39` is a finite `f64` that becomes `inf` once narrowed, past every `is_finite()` check that
//! ran on the `f64`. [`engine_number`](ludion_core::number::engine_number) closes that gap at the read: such a number reads as `±inf`,
//! so the finiteness checks every kind already has reject it.
//!
//! Each caller supplies its own [`DiagnosticCode`], as in [`crate::envelope`].

use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::number::engine_number;
use serde_json::{Number, Value};

use crate::yaml::{MapEntry, Node, NodeValue};

/// A YAML/JSON number as the engine reads it: `f64`, with `f32` overflow read as `±inf`.
pub(crate) fn number(n: &Number) -> f64 {
    engine_number(n.as_f64().unwrap_or(f64::NAN))
}

/// Parses any single number. Finiteness and range are semantic concerns, checked by the validators.
pub(crate) fn parse_number(
    node: &Node,
    pointer: &str,
    what: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<f64> {
    match &node.value {
        NodeValue::Scalar(Value::Number(n)) => Some(number(n)),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code,
                    format!(
                        "`{what}` must be a number, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(format!(
                    "write `{what}` as a single number, e.g. `{what}: 0.5`"
                )),
            );
            None
        }
    }
}

/// Parses `size`: a sequence of exactly three positive integers that fit in `u32`.
pub(crate) fn parse_size3(
    entry: &MapEntry,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[u32; 3]> {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "`size` must be a sequence of three positive integers, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/size")
            .with_help("write `size` as three integers:\n  size:\n    - 8\n    - 8\n    - 8"),
        );
        return None;
    };
    if items.len() != 3 {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "`size` must have exactly 3 entries (x, y, z), found {}",
                    items.len()
                ),
            )
            .with_path("/size")
            .with_help("supply exactly three positive integers: width (x), height (y), depth (z)"),
        );
        return None;
    }
    let mut out = [0u32; 3];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let value = match &item.value {
            NodeValue::Scalar(Value::Number(n)) => {
                n.as_u64().filter(|v| *v >= 1 && *v <= u64::from(u32::MAX))
            }
            _ => None,
        };
        match value {
            Some(v) => out[index] = v as u32,
            None => {
                diagnostics.push(
                    Diagnostic::new(
                        code,
                        format!(
                            "`size[{index}]` must be a positive integer (at least 1), found {} \
                             (line {})",
                            item.value.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("/size/{index}"))
                    .with_help("each voxel dimension must be a positive whole number"),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}
