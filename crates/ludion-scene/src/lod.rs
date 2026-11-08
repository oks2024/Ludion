//! The `lods:` chain shared by mesh and voxel documents: the threshold rules (count, finite,
//! above zero, strictly decreasing) live here once, plus the voxel entry parser and writer.
//! A mesh level names a document; a voxel level is generated at load, so it carries only its
//! `screen_size`.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::envelope;
use crate::scalar::format_float;
use crate::voxel::VoxelLod;
use crate::yaml::{Node, NodeValue};

/// The most coarser levels a document may chain after its own (four levels in all).
pub(crate) const MAX_LODS: usize = 3;

const VOXEL_LOD_KEYS: [&str; 1] = ["screen_size"];

/// Reports a `lods` list longer than [`MAX_LODS`] (`L1395`); `noun` names the document kind.
pub(crate) fn check_count(len: usize, noun: &str, diagnostics: &mut Vec<Diagnostic>) {
    if len > MAX_LODS {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_LOD_CHAIN,
                format!(
                    "`lods` lists {len} levels; {noun} chains at most {MAX_LODS} coarser levels \
                     after its own"
                ),
            )
            .with_path("/lods")
            .with_help(format!("keep the first {MAX_LODS} levels, or merge two")),
        );
    }
}

/// Checks level `index`'s `screen_size` against the previous accepted one (`L1395`): finite, above
/// zero and strictly below it. `previous` advances only on a valid level.
pub(crate) fn check_screen_size(
    index: usize,
    screen_size: f64,
    previous: &mut f64,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !screen_size.is_finite() || screen_size <= 0.0 || screen_size >= *previous {
        let rule = if index == 0 {
            "finite and above zero"
        } else {
            "finite, above zero and strictly below the previous level's"
        };
        diagnostics.push(
            Diagnostic::new(
                code::MESH_LOD_CHAIN,
                format!(
                    "level {} `screen_size` {screen_size} must be {rule}",
                    index + 1
                ),
            )
            .with_path(format!("/lods/{index}/screen_size"))
            .with_help("a coarser level takes over at a smaller screen size, e.g. 0.25 then 0.08"),
        );
    } else {
        *previous = screen_size;
    }
}

/// Validates a voxel document's `lods` (`L1395`).
pub(crate) fn validate_voxel_lods(lods: &[VoxelLod], diagnostics: &mut Vec<Diagnostic>) {
    check_count(lods.len(), "a voxel model", diagnostics);
    let mut previous = f64::INFINITY;
    for (index, lod) in lods.iter().enumerate() {
        check_screen_size(index, lod.screen_size, &mut previous, diagnostics);
    }
}

/// Parses a voxel `lods`: a sequence of `{screen_size}` mappings. `None` when any entry is
/// malformed (a diagnostic is already pushed for each).
pub(crate) fn parse_voxel_lods(
    node: &Node,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<VoxelLod>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_LOD_CHAIN,
                format!(
                    "`lods` must be a sequence, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path("/lods")
            .with_help("write `lods` as a list, e.g. `- screen_size: 0.25`"),
        );
        return None;
    };
    let before = diagnostics.len();
    let lods: Vec<VoxelLod> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| parse_voxel_lod(item, index, diagnostics))
        .collect();
    (diagnostics.len() == before).then_some(lods)
}

fn parse_voxel_lod(
    node: &Node,
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<VoxelLod> {
    let pointer = format!("/lods/{index}");
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_LOD_CHAIN,
                format!(
                    "a level of `lods` must be a mapping, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer)
            .with_help("write it as `screen_size: 0.25`"),
        );
        return None;
    };
    let before = diagnostics.len();
    let mut screen_size = None;
    for entry in entries {
        let field = format!("{pointer}/{}", entry.key);
        match entry.key.as_str() {
            "screen_size" => {
                screen_size = crate::fields::parse_number(
                    &entry.node,
                    &field,
                    "screen_size",
                    code::MESH_LOD_CHAIN,
                    diagnostics,
                );
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                &pointer,
                "a level of `lods`",
                VOXEL_LOD_KEYS,
                code::MESH_LOD_CHAIN,
            )),
        }
    }
    if !entries.iter().any(|entry| entry.key == "screen_size") {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_LOD_CHAIN,
                "a level of `lods` is missing its required `screen_size`",
            )
            .with_path(pointer)
            .with_help("add `screen_size:` to it"),
        );
    }
    if diagnostics.len() != before {
        return None;
    }
    Some(VoxelLod {
        screen_size: screen_size?,
    })
}

/// Writes a voxel `lods` block; nothing when the chain is empty.
pub(crate) fn emit_voxel_lods(out: &mut String, lods: &[VoxelLod]) {
    if lods.is_empty() {
        return;
    }
    out.push_str("lods:\n");
    for lod in lods {
        out.push_str("  - screen_size: ");
        out.push_str(&format_float(lod.screen_size));
        out.push('\n');
    }
}
