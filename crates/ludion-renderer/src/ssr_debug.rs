//! Full-screen debug views of the screen-space reflection trace (offline `ludion render
//! --debug-view`).
//!
//! A view replaces the finished picture with data the trace produced, so "why is this reflection
//! missing" is answerable from one PNG. It is host render state, like debug-draw categories: it never
//! enters a scene file or a `WorldHash`, and it never turns SSR on — with SSR off for the frame there
//! is nothing to show and the normal frame is drawn.

/// One of the full-screen views of the reflection trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugView {
    /// The trace's confidence as gray: 0 where there is no hit, 1 for a certain one.
    SsrConfidence,
    /// The `ssr_color` pyramid level the trace read, as gray over the level count; magenta where the
    /// trace found no hit, so a miss is distinct from level 0.
    SsrFootprint,
}

impl DebugView {
    /// Every view, in the order the CLI lists them.
    pub const ALL: [DebugView; 2] = [DebugView::SsrConfidence, DebugView::SsrFootprint];

    /// The spelling used by the CLI and in summaries.
    pub fn name(self) -> &'static str {
        match self {
            DebugView::SsrConfidence => "ssr_confidence",
            DebugView::SsrFootprint => "ssr_footprint",
        }
    }
}
