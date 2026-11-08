//! The document-level `rim:` block shared by voxel, sculpt and mesh documents: one rim per
//! document, the same shape and ranges as `ludion_components::Rim`. This crate holds it as `f64`
//! like every other float here; the engine narrows to `f32`.

use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::suggest;
use serde_json::Value;

use crate::fields::parse_number;
use crate::scalar::format_float;
use crate::yaml::{Node, NodeValue, pointer_token};

const RIM_KEYS: [&str; 3] = ["color", "strength", "power"];
const DEFAULT_COLOR: [f64; 3] = [1.0, 1.0, 1.0];
const DEFAULT_STRENGTH: f64 = 1.0;
const DEFAULT_POWER: f64 = 3.0;

/// A document's rim: a view-angle highlight colour, intensity and falloff exponent.
///
/// One per document — it wins over the entity's `Mesh.rim` and any inherited one. Ranges (checked
/// by [`validate_rim`]): colour channels and `strength` finite `>= 0`, `power` finite `> 0`.
#[derive(Clone, Debug, PartialEq)]
pub struct DocRim {
    /// Linear RGB rim colour (default white), HDR-capable.
    pub color: [f64; 3],
    /// Intensity multiplier (default `1.0`).
    pub strength: f64,
    /// Falloff exponent (default `3.0`); higher hugs the silhouette.
    pub power: f64,
}

impl Default for DocRim {
    fn default() -> Self {
        Self {
            color: DEFAULT_COLOR,
            strength: DEFAULT_STRENGTH,
            power: DEFAULT_POWER,
        }
    }
}

/// Parses a `rim:` mapping; each key is optional. Shape errors carry `code`; ranges are
/// [`validate_rim`]'s job.
pub(crate) fn parse_rim(
    node: &Node,
    pointer: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<DocRim> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "`rim` must be a mapping, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write `rim:` with any of `color`, `strength`, `power` indented below it"),
        );
        return None;
    };
    let mut rim = DocRim::default();
    let mut ok = true;
    for entry in entries {
        let path = format!("{pointer}/{}", pointer_token(&entry.key));
        match entry.key.as_str() {
            "color" => match parse_color(&entry.node, &path, code, diagnostics) {
                Some(color) => rim.color = color,
                None => ok = false,
            },
            "strength" => match parse_number(&entry.node, &path, "strength", code, diagnostics) {
                Some(v) => rim.strength = v,
                None => ok = false,
            },
            "power" => match parse_number(&entry.node, &path, "power", code, diagnostics) {
                Some(v) => rim.power = v,
                None => ok = false,
            },
            unknown => {
                ok = false;
                let diagnostic = Diagnostic::new(
                    code,
                    format!(
                        "`rim` does not have a key named `{unknown}` (line {})",
                        entry.key_line
                    ),
                )
                .with_path(path);
                diagnostics.push(match suggest::nearest(unknown, RIM_KEYS) {
                    Some(candidate) => {
                        diagnostic.with_help(format!("nearest match is `{candidate}`"))
                    }
                    None => {
                        diagnostic.with_help(format!("valid keys are: {}", RIM_KEYS.join(", ")))
                    }
                });
            }
        }
    }
    ok.then_some(rim)
}

fn parse_color(
    node: &Node,
    pointer: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; 3]> {
    let help = "write `color` as three linear RGB numbers:\n    color:\n      - 1.0\n      - 0.5\n      - 0.2";
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "`rim.color` must be a sequence of three numbers, found {} (line {})",
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
                code,
                format!(
                    "`rim.color` must have exactly 3 entries (r, g, b), found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help(help),
        );
        return None;
    }
    let mut color = [0.0; 3];
    let mut ok = true;
    for (i, item) in items.iter().enumerate() {
        if !matches!(item.value, NodeValue::Scalar(Value::Number(_))) {
            ok = false;
        }
        match parse_number(
            item,
            &format!("{pointer}/{i}"),
            "rim.color",
            code,
            diagnostics,
        ) {
            Some(v) => color[i] = v,
            None => ok = false,
        }
    }
    ok.then_some(color)
}

/// Range-checks a parsed rim; every diagnostic carries `code` and a pointer under `pointer`.
pub(crate) fn validate_rim(rim: &DocRim, pointer: &str, code: DiagnosticCode) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for (i, channel) in rim.color.iter().enumerate() {
        if !((*channel as f32).is_finite() && *channel >= 0.0) {
            out.push(
                Diagnostic::new(
                    code,
                    format!("`rim.color[{i}]` is {channel}; a rim colour channel must be finite and `>= 0`"),
                )
                .with_path(format!("{pointer}/color/{i}"))
                .with_help("colour channels are linear and may exceed 1.0 for an HDR rim; use `0` for none"),
            );
        }
    }
    if !((rim.strength as f32).is_finite() && rim.strength >= 0.0) {
        out.push(
            Diagnostic::new(
                code,
                format!(
                    "`rim.strength` is {}; it must be finite after narrowing to f32 and `>= 0`",
                    rim.strength
                ),
            )
            .with_path(format!("{pointer}/strength"))
            .with_help("strength scales the rim colour; `0` renders as no rim, 1.0 is the default"),
        );
    }
    if !ludion_core::number::positive_f32(rim.power) {
        out.push(
            Diagnostic::new(
                code,
                format!(
                    "`rim.power` is {}; it must be finite and `> 0` after narrowing to f32",
                    rim.power
                ),
            )
            .with_path(format!("{pointer}/power"))
            .with_help("power is the falloff exponent (default 3.0); higher hugs the silhouette"),
        );
    }
    out
}

/// Writes the canonical `rim:` block: `color`, `strength`, `power`, each omitted at its default.
/// A rim that is all defaults is written as the flow `rim: {}` so it still round-trips as `Some`.
pub(crate) fn emit_rim(out: &mut String, rim: &DocRim) {
    let color = rim.color != DEFAULT_COLOR;
    let strength = rim.strength != DEFAULT_STRENGTH;
    let power = rim.power != DEFAULT_POWER;
    if !(color || strength || power) {
        out.push_str("rim: {}\n");
        return;
    }
    out.push_str("rim:\n");
    if color {
        out.push_str("  color:\n");
        for channel in rim.color {
            out.push_str("    - ");
            out.push_str(&format_float(channel));
            out.push('\n');
        }
    }
    if strength {
        out.push_str("  strength: ");
        out.push_str(&format_float(rim.strength));
        out.push('\n');
    }
    if power {
        out.push_str("  power: ");
        out.push_str(&format_float(rim.power));
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml;
    use ludion_core::codes::code;

    fn parse_block(text: &str) -> (Option<DocRim>, Vec<Diagnostic>) {
        let root = yaml::parse_document(text)
            .expect("yaml")
            .expect("non-empty");
        let NodeValue::Mapping(entries) = &root.value else {
            panic!("mapping")
        };
        let mut diags = Vec::new();
        let rim = parse_rim(
            &entries[0].node,
            "/rim",
            code::VOXEL_DOCUMENT_STRUCTURE,
            &mut diags,
        );
        (rim, diags)
    }

    #[test]
    fn empty_mapping_is_all_defaults() {
        let (rim, diags) = parse_block("rim: {}\n");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(rim, Some(DocRim::default()));
    }

    #[test]
    fn round_trips_and_omits_defaults() {
        let rim = DocRim {
            color: [1.0, 0.5, 0.25],
            strength: 2.0,
            power: 3.0,
        };
        let mut out = String::new();
        emit_rim(&mut out, &rim);
        assert_eq!(
            out,
            "rim:\n  color:\n    - 1.0\n    - 0.5\n    - 0.25\n  strength: 2.0\n"
        );
        let (back, diags) = parse_block(&out);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(back, Some(rim));
        let mut empty = String::new();
        emit_rim(&mut empty, &DocRim::default());
        assert_eq!(empty, "rim: {}\n");
    }

    #[test]
    fn unknown_key_suggests() {
        let (rim, diags) = parse_block("rim:\n  strenth: 1.0\n");
        assert!(rim.is_none());
        assert_eq!(diags[0].path.as_deref(), Some("/rim/strenth"));
        assert!(diags[0].help.as_deref().unwrap().contains("strength"));
    }

    #[test]
    fn each_range_error_has_its_pointer() {
        let code = code::VOXEL_DATA_CONSISTENCY;
        let bad = DocRim {
            color: [1.0, -1.0, 1.0],
            strength: -0.5,
            power: 0.0,
        };
        let paths: Vec<_> = validate_rim(&bad, "/rim", code)
            .iter()
            .map(|d| d.path.clone().unwrap())
            .collect();
        assert_eq!(paths, ["/rim/color/1", "/rim/strength", "/rim/power"]);
        let inf = DocRim {
            strength: f64::INFINITY,
            ..DocRim::default()
        };
        assert_eq!(validate_rim(&inf, "/rim", code).len(), 1);
        assert!(validate_rim(&DocRim::default(), "/rim", code).is_empty());
    }

    #[test]
    fn wrong_shapes_are_reported() {
        let (_, d) = parse_block("rim: 3\n");
        assert_eq!(d[0].path.as_deref(), Some("/rim"));
        let (_, d) = parse_block("rim:\n  color:\n    - 1.0\n");
        assert_eq!(d[0].path.as_deref(), Some("/rim/color"));
    }
}
