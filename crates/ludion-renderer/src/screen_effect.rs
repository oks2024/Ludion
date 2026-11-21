//! The extracted form of a `ScreenEffect`: one applied effect with its output-pixel rectangle, its
//! UI split point and its resolved stages. Plain data; the component lives in `ludion-components`.

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
