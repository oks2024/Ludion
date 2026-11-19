//! The `irradiance` debug view's pass: it stands in for `composite` and shows the scene pass's output
//! as it is, already `E / IRRADIANCE_VIEW_SCALE` ([`crate::DebugView::Irradiance`]).

use super::{FrameRecorder, LayoutId, RecordedPass, SRC_TEX_GROUP, ShaderId};

impl FrameRecorder<'_> {
    /// A fullscreen blit of the `scene` target: the sRGB target does the encoding.
    pub(super) fn record_irradiance_debug(&self, pass: &mut RecordedPass<'_>) {
        let blit_bg = self.r.texture_bind_group(
            pass.device,
            LayoutId::Src,
            pass.inputs[0],
            &self.r.nearest_sampler,
        );
        let mut rp = pass.begin();
        rp.set_pipeline(self.r.pipeline(ShaderId::Blit));
        rp.set_bind_group(SRC_TEX_GROUP, &blit_bg, &[]);
        rp.draw(0..3, 0..1);
    }
}
