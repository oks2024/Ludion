//! Accessor pre-checks: the gltf reader asserts on a stride shorter than its element, overflows on
//! an empty or oversized accessor, and has no answer for a component type it does not expect. So
//! every accessor the importer reads is checked here first, and a bad one is an `L1201` naming it.

use gltf::accessor::{DataType, Dimensions};
use ludion_core::diagnostics::Diagnostic;

use crate::diagnostics::malformed;
use crate::sources::Buffers;

/// Checks that `accessor` has one of the `allowed` layouts and that every element it names lies
/// inside its buffer view, and the view inside its buffer. `role` names the attribute (`POSITION`,
/// `indices`); `label` names the primitive.
///
/// # Errors
/// `L1201` naming `label`, `role` and the accessor index.
pub(crate) fn check_accessor(
    accessor: &gltf::Accessor,
    allowed: &[(Dimensions, DataType)],
    role: &str,
    label: &str,
    buffers: &Buffers,
) -> Result<(), Diagnostic> {
    let bad = |detail: String| {
        malformed(format!(
            "{label}: {role} accessor {} {detail}",
            accessor.index()
        ))
    };
    let layout = (accessor.dimensions(), accessor.data_type());
    if !allowed.contains(&layout) {
        return Err(bad(format!(
            "is {:?} of {:?}, but must be one of {allowed:?}",
            layout.0, layout.1
        )));
    }
    let count = accessor.count();
    if count == 0 {
        return Err(bad("has a count of 0".into()));
    }
    let Some(view) = accessor.view() else {
        return Err(bad("has no buffer view".into()));
    };
    let element = accessor.data_type().size() * accessor.dimensions().multiplicity();
    let stride = view.stride().unwrap_or(element);
    if stride < element {
        return Err(bad(format!(
            "has a stride of {stride} bytes, shorter than its {element}-byte element"
        )));
    }
    let end = (count - 1)
        .checked_mul(stride)
        .and_then(|span| span.checked_add(accessor.offset()))
        .and_then(|start| start.checked_add(element));
    if end.is_none_or(|end| end > view.length()) {
        return Err(bad(format!(
            "reads {count} elements from offset {}, past the {}-byte end of its buffer view",
            accessor.offset(),
            view.length()
        )));
    }
    let buffer_len = buffers.get(view.buffer().index()).map_or(0, <[u8]>::len);
    if view
        .offset()
        .checked_add(view.length())
        .is_none_or(|view_end| view_end > buffer_len)
    {
        return Err(bad(format!(
            "sits in a buffer view that runs past the end of buffer {}",
            view.buffer().index()
        )));
    }
    Ok(())
}
