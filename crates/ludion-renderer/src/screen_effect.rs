//! The extracted form of a `ScreenEffect`: one applied effect with its output-pixel rectangle, its
//! UI split point and its resolved stages. Plain data; the component lives in `ludion-components`.

use crate::scene::UiInstance;
use crate::shader_gen::ScreenEffectParams;

/// The most screen effects applied to one frame.
pub const MAX_SCREEN_EFFECTS: usize = 4;

/// The shadow mask pattern of a screen effect.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ScreenMask {
    /// No mask.
    #[default]
    None,
    /// Vertical RGB stripes.
    Aperture,
    /// Staggered RGB slots.
    Slot,
    /// Triangular RGB dots.
    Shadow,
}

/// One applied screen effect, stages resolved. A neutral effect is never extracted.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ScreenEffectData {
    /// `[x, y, w, h]` in output pixels, top-left origin, clamped to the viewport.
    pub rect_px: [f32; 4],
    /// How many `RenderScene.ui` entries draw under the effect.
    pub ui_start: u32,
    /// Game seconds, wrapped at `RAIN_TIME_WRAP_S`.
    pub time_s: f32,
    /// The game tick.
    pub frame: u32,
    /// Varies the noise and jitter hash.
    pub seed: u32,
    /// Scanline darkening, in `[0, 1]`.
    pub scanlines: f32,
    /// Shadow mask strength, in `[0, 1]`.
    pub mask_strength: f32,
    /// Barrel curvature, in `[0, 0.3]`.
    pub curvature: f32,
    /// Bloom of the bright parts, in `[0, 1]`.
    pub glow: f32,
    /// Horizontal chroma smear, in `[0, 1]`.
    pub chroma_bleed: f32,
    /// Animated noise, in `[0, 1]`.
    pub noise: f32,
    /// Tracking error bands, in `[0, 1]`.
    pub tracking: f32,
    /// Vertical roll in regions per second.
    pub roll: f32,
    /// Horizontal line jitter, in `[0, 1]`.
    pub jitter: f32,
    /// Brightness flicker, in `[0, 1]`.
    pub flicker: f32,
    /// The sweep bar's strength, in `[0, 1]`.
    pub sweep: f32,
    /// Seconds per sweep.
    pub sweep_period: f32,
    /// Scanlines across the rectangle's height.
    pub scanline_count: u32,
    /// Colour levels per channel; below 2 is off.
    pub posterize: u32,
    /// The shadow mask pattern.
    pub mask: ScreenMask,
    /// Linear mono tint; `[0, 0, 0]` keeps the colour.
    pub phosphor: [f32; 3],
    /// Alternate the scanline phase each frame.
    pub interlace: bool,
}

/// Splits `ui` at each effect's `ui_start`: segment 0 is what draws under effect 0, segment `k + 1`
/// what draws between effects `k` and `k + 1` (the last runs to the end). Starts are clamped to the
/// list and made monotone, so an out-of-range or reordered start yields an empty segment rather than
/// a panic. With no effect it is the whole list as the one segment.
pub(crate) fn ui_segments<'a>(
    ui: &'a [UiInstance],
    effects: &[ScreenEffectData],
) -> Vec<&'a [UiInstance]> {
    let mut segments = Vec::with_capacity(effects.len() + 1);
    let mut from = 0;
    for effect in effects {
        let start = (effect.ui_start as usize).min(ui.len()).max(from);
        segments.push(&ui[from..start]);
        from = start;
    }
    segments.push(&ui[from..]);
    segments
}

/// The shader's uniform for one effect over a `viewport` of output pixels.
pub(crate) fn effect_params(data: &ScreenEffectData, viewport: [f32; 2]) -> ScreenEffectParams {
    let [r, g, b] = data.phosphor;
    ScreenEffectParams {
        rect: data.rect_px,
        viewport,
        time_s: data.time_s,
        frame: data.frame,
        seed: data.seed,
        scanline_count: data.scanline_count,
        posterize: data.posterize,
        mask: match data.mask {
            ScreenMask::None => 0,
            ScreenMask::Aperture => 1,
            ScreenMask::Slot => 2,
            ScreenMask::Shadow => 3,
        },
        phosphor: [r, g, b, 0.0],
        scanlines: data.scanlines,
        mask_strength: data.mask_strength,
        curvature: data.curvature,
        glow: data.glow,
        chroma_bleed: data.chroma_bleed,
        noise: data.noise,
        tracking: data.tracking,
        roll: data.roll,
        jitter: data.jitter,
        flicker: data.flicker,
        sweep: data.sweep,
        sweep_period: data.sweep_period,
        interlace: u32::from(data.interlace),
        pad0: 0,
        pad1: 0,
        pad2: 0,
    }
}
