//! The seam a host uses to let the presenter re-resolve a tick's camera.

use crate::blend::CameraBlend;

/// The frame's resolved camera, re-resolvable at present time. The engine implements it; the
/// renderer never sees a rig.
pub trait CameraLatch: Send + Sync {
    /// The camera with `extra_look_deg` (`[yaw, pitch]` degrees) beyond the tick's; `[0, 0]` is the
    /// tick's camera bit for bit. `None` when it cannot re-resolve.
    fn resolve(&self, extra_look_deg: [f32; 2]) -> Option<CameraBlend>;
    /// Look degrees for raw mouse counts, through the rig's one sensitivity.
    fn look_from_counts(&self, dx: f32, dy: f32) -> [f32; 2];
}
