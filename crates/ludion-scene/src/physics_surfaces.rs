//! The `physics.surfaces` list and `physics.default_surface` (`Plan/surface-tags.md`): the named
//! surfaces a `SurfaceTag` may name. This module holds the parse and the value checks the parser
//! and the snapshot restore share ([`check_surfaces`]).

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;

use crate::envelope;
use crate::yaml::{MapEntry, NodeValue};

/// The most surfaces a scene may declare: a `SurfaceId` is one byte of table index.
pub const MAX_SURFACES: usize = 256;

/// Parses `physics.surfaces`: a sequence of strings. Structural errors only — the value checks run
/// in [`check_surfaces`] over the parsed list.
pub(crate) fn parse_surfaces(
    entry: &MapEntry,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<String>> {
    let NodeValue::Sequence(items) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::SCENE_TYPE_MISMATCH,
                format!(
                    "`physics.surfaces` must be a sequence of names, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/physics/surfaces")
            .with_help(
                "write the surfaces as a list:\n  surfaces:\n    - concrete\n    - metal_grate",
            ),
        );
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match envelope::expect_string(
            item,
            &format!("/physics/surfaces/{index}"),
            &format!("`physics.surfaces[{index}]`"),
            code::SCENE_TYPE_MISMATCH,
            diagnostics,
        ) {
            Some(name) => out.push(name),
            None => ok = false,
        }
    }
    ok.then_some(out)
}

/// Parses `physics.default_surface`: a string.
pub(crate) fn parse_default_surface(
    entry: &MapEntry,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    envelope::expect_string(
        &entry.node,
        "/physics/default_surface",
        "`physics.default_surface`",
        code::SCENE_TYPE_MISMATCH,
        diagnostics,
    )
}

/// `L1691` for an empty or repeated name or more than [`MAX_SURFACES`]; `L1690` for a
/// `default_surface` that names no declared surface. Shared by the parser and
/// [`crate::validate_physics`]; `base` is the pointer prefix (`/physics`, or `/scene_physics` from
/// a restore).
pub(crate) fn check_surfaces(
    surfaces: &[String],
    default_surface: Option<&str>,
    base: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if surfaces.len() > MAX_SURFACES {
        diagnostics.push(
            Diagnostic::new(
                code::SURFACES_INVALID,
                format!(
                    "`physics.surfaces` declares {} surfaces, more than the {MAX_SURFACES} a \
                     surface id can index",
                    surfaces.len()
                ),
            )
            .with_path(format!("{base}/surfaces"))
            .with_help(format!("declare at most {MAX_SURFACES} surfaces")),
        );
    }
    for (i, name) in surfaces.iter().enumerate() {
        let pointer = format!("{base}/surfaces/{i}");
        if name.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::SURFACES_INVALID,
                    format!("`physics.surfaces[{i}]` is empty"),
                )
                .with_path(pointer),
            );
        } else if let Some(first) = surfaces[..i].iter().position(|earlier| earlier == name) {
            diagnostics.push(
                Diagnostic::new(
                    code::SURFACES_INVALID,
                    format!(
                        "`physics.surfaces[{i}]` repeats `{name}`, first declared at [{first}]"
                    ),
                )
                .with_path(pointer)
                .with_help("delete the repeated entry"),
            );
        }
    }
    if let Some(default) = default_surface
        && !surfaces.iter().any(|name| name == default)
    {
        let mut diagnostic = Diagnostic::new(
            code::SURFACE_UNKNOWN,
            format!("`physics.default_surface` names `{default}`, which `physics.surfaces` does not declare"),
        )
        .with_path(format!("{base}/default_surface"));
        diagnostic = match suggest::nearest(default, surfaces.iter().map(String::as_str)) {
            Some(near) => diagnostic.with_help(format!("did you mean `{near}`?")),
            None => diagnostic.with_help("add the name to `physics.surfaces`"),
        };
        diagnostics.push(diagnostic);
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_clean_list_passes() {
        assert!(check_surfaces(&names(&["a", "b"]), Some("b"), "/physics").is_empty());
        assert!(check_surfaces(&[], None, "/physics").is_empty());
    }

    #[test]
    fn empty_and_duplicate_are_l1691() {
        let found = check_surfaces(&names(&["a", "", "a"]), None, "/physics");
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|d| d.code == code::SURFACES_INVALID));
        assert_eq!(found[0].path.as_deref(), Some("/physics/surfaces/1"));
        assert_eq!(found[1].path.as_deref(), Some("/physics/surfaces/2"));
    }

    #[test]
    fn too_many_is_l1691() {
        let many: Vec<String> = (0..=MAX_SURFACES).map(|i| format!("s{i}")).collect();
        let found = check_surfaces(&many, None, "/physics");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::SURFACES_INVALID);
    }

    #[test]
    fn an_undeclared_default_is_l1690_with_a_suggestion() {
        let found = check_surfaces(&names(&["concrete"]), Some("concrte"), "/physics");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::SURFACE_UNKNOWN);
        assert_eq!(found[0].path.as_deref(), Some("/physics/default_surface"));
        assert!(format!("{:?}", found[0].help).contains("concrete"));
    }
}
