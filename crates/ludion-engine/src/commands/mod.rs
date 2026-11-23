//! Command handlers — the *logic* the [`dispatch`](crate::dispatch) parity gate routes to.
//!
//! One module per method family (scene, entity, sim, render, stream, asset, engine). Each handler is
//! a plain function taking `&mut Engine` and the method's params, returning a
//! [`ResponseBody`](ludion_protocol::ResponseBody) or a [`Diagnostic`]. `dispatch.rs` contains no
//! logic; it only destructures params and calls these (architecture §2).

pub(crate) mod ai;
pub(crate) mod asset;
pub(crate) mod checkpoint;
pub(crate) mod engine_cmds;
pub(crate) mod entity;
pub(crate) mod perf;
pub(crate) mod prefab;
pub(crate) mod render;
pub(crate) mod scene;
pub(crate) mod sequence;
pub(crate) mod sim;
pub(crate) mod stream;

use ludion_core::diagnostics::Diagnostic;

/// Collapses a `Vec<Diagnostic>` (as the scene/extract pipelines produce) into a single wire
/// diagnostic: the first becomes the primary, the rest become its `related` sub-diagnostics, so the
/// structured response carries every problem without inventing a new shape.
///
/// The vector is never empty at a call site (a handler only collapses on the error path), but an
/// empty vector degrades to `L0029` (an engine bug) rather than panicking.
pub(crate) fn collapse(mut diagnostics: Vec<Diagnostic>) -> Diagnostic {
    if diagnostics.is_empty() {
        return Diagnostic::new(
            ludion_core::codes::code::OPERATION_FAILED_SILENTLY,
            "the operation failed but reported no diagnostic",
        )
        .with_help("this is an engine bug, report it with the command that triggered it");
    }
    let mut primary = diagnostics.remove(0);
    primary.related.extend(diagnostics);
    primary
}

#[cfg(test)]
mod tests {
    use super::collapse;
    use ludion_core::codes::code;
    use ludion_core::diagnostics::Diagnostic;

    #[test]
    fn collapsing_nothing_is_an_engine_bug_not_a_protocol_error() {
        assert_eq!(collapse(Vec::new()).code, code::OPERATION_FAILED_SILENTLY);
    }

    #[test]
    fn collapse_keeps_the_first_as_primary_with_the_rest_related() {
        let a = Diagnostic::new(code::SCENE_IO, "a");
        let b = Diagnostic::new(code::OUTPUT_IO, "b");
        let collapsed = collapse(vec![a, b]);
        assert_eq!(collapsed.code, code::SCENE_IO);
        assert_eq!(collapsed.related.len(), 1);
        assert_eq!(collapsed.related[0].code, code::OUTPUT_IO);
    }
}
