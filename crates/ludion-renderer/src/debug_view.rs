//! Full-screen debug views (offline `ludion render --debug-view`).
//!
//! A view replaces the finished picture with data one effect produced, so "why is this reflection
//! missing" is answerable from one PNG. It is host render state, like debug-draw categories: it never
//! enters a scene file or a `WorldHash`, and it never turns its effect on — with SSR off for the frame
//! there is no trace to show, a scene without probes has no index to draw, and a frame with no granted
//! local-light shadow has no atlas, and a sun without fitted cascades has none to tint by; the normal
//! frame is drawn in each case.

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
    /// The frame's local-light shadow atlas, letterboxed: each granted tile's depth linearised to gray
    /// (near bright, empty black), ungranted cells flat dark blue, a red line on every cell border.
    LocalShadowAtlas,
    /// The lit frame, each mesh fragment tinted by the cascade that shades it
    /// ([`SUN_CASCADE_VIEW_PALETTE`]); the seam band blends two tints, and past the last cascade the
    /// fragment is untinted, which is where the sun's shadows end.
    SunCascades,
}

impl DebugView {
    /// Every view, in the order the CLI lists them.
    pub const ALL: [DebugView; 5] = [
        DebugView::SsrConfidence,
        DebugView::SsrFootprint,
        DebugView::ProbeIndex,
        DebugView::LocalShadowAtlas,
        DebugView::SunCascades,
    ];

    /// The spelling used by the CLI and in summaries.
    pub fn name(self) -> &'static str {
        match self {
            DebugView::SsrConfidence => "ssr_confidence",
            DebugView::SsrFootprint => "ssr_footprint",
            DebugView::ProbeIndex => "probe_index",
            DebugView::LocalShadowAtlas => "local_shadow_atlas",
            DebugView::SunCascades => "sun_cascades",
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

/// The tint of sun cascade `k`, nearest first: soft multiplicative colours, so the lit picture stays
/// readable under them. The `sun_cascades` view's source; `mesh.wgsl`'s `SUN_CASCADE_TINT` mirrors it.
pub const SUN_CASCADE_VIEW_PALETTE: [[f32; 3]; 4] = [
    [1.0, 0.45, 0.45],
    [0.45, 1.0, 0.45],
    [0.5, 0.6, 1.0],
    [1.0, 1.0, 0.4],
];
