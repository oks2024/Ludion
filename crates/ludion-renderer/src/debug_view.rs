//! Full-screen debug views (offline `ludion render --debug-view`).
//!
//! A view replaces the finished picture with data one effect produced, so "why is this reflection
//! missing" is answerable from one PNG. It is host render state, like debug-draw categories: it never
//! enters a scene file or a `WorldHash`, and it never turns its effect on — with SSR off for the frame
//! there is no trace to show, and a scene without probes has no index to draw; the normal frame is
//! drawn in both cases.

/// One of the full-screen debug views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugView {
    /// The trace's confidence as gray: 0 where there is no hit, 1 for a certain one.
    SsrConfidence,
    /// The `ssr_color` pyramid level the trace read, as gray over the level count; magenta where the
    /// trace has no confident hit (a miss, a back-face or past the fade band), so it is distinct from level 0.
    SsrFootprint,
    /// The winning reflection probe per pixel, coloured by rank from [`PROBE_VIEW_PALETTE`] and scaled
    /// by its contribution; gray where no probe weights the surface, black only where there is no surface.
    ProbeIndex,
}

impl DebugView {
    /// Every view, in the order the CLI lists them.
    pub const ALL: [DebugView; 3] = [
        DebugView::SsrConfidence,
        DebugView::SsrFootprint,
        DebugView::ProbeIndex,
    ];

    /// The spelling used by the CLI and in summaries.
    pub fn name(self) -> &'static str {
        match self {
            DebugView::SsrConfidence => "ssr_confidence",
            DebugView::SsrFootprint => "ssr_footprint",
            DebugView::ProbeIndex => "probe_index",
        }
    }

    /// Whether this view draws the SSR trace (and so exists only with SSR on).
    pub fn is_ssr(self) -> bool {
        matches!(self, DebugView::SsrConfidence | DebugView::SsrFootprint)
    }
}

/// The colour of probe rank `k` is entry `k % 8`, in sRGB: eight saturated hues, no gray and no black,
/// so a probe never reads as "no probe" or "no surface". The one source for the `probe_index` view
/// and for the probe volume lines.
pub const PROBE_VIEW_PALETTE: [[f32; 3]; 8] = [
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 1.0],
    [1.0, 0.0, 1.0],
    [1.0, 0.5, 0.0],
    [0.5, 0.0, 1.0],
];
