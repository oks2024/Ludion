//! The per-voice filters: one low-pass and one high-pass biquad a voice runs only while its
//! [`VoiceFilter`] asks for them.
//!
//! Everything here is `Copy` and allocation-free, so the real-time callback owns and steps it. A
//! bypassed side has no stage at all: a voice whose filter is open is exactly the unfiltered voice.

use ludion_core::audio::{HIGH_PASS_OPEN_HZ, LOW_PASS_OPEN_HZ, VoiceFilter};
use ludion_core::math::{exp2, log2, sin_cos};

use crate::MIX_BLOCK_FRAMES;

/// The most channels a stage filters; a channel past it passes unfiltered.
pub(crate) const MAX_FILTER_CHANNELS: usize = 8;

/// The highest cutoff a stage takes, as a fraction of the device rate.
const NYQUIST_MARGIN: f32 = 0.45;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Low,
    High,
}

impl Side {
    fn open_hz(self) -> f32 {
        match self {
            Side::Low => LOW_PASS_OPEN_HZ,
            Side::High => HIGH_PASS_OPEN_HZ,
        }
    }
}

/// One RBJ biquad (Direct Form II transposed, Q = 1/sqrt 2) with a cutoff swept in log2 space.
#[derive(Clone, Copy, Debug)]
struct Stage {
    side: Side,
    /// The current cutoff, log2 Hz.
    log_cutoff: f32,
    /// Where the sweep ends, log2 Hz.
    target: f32,
    /// The log2 distance covered per block.
    step: f32,
    /// Blocks of sweep still to run.
    left: u32,
    /// The sweep heads for the open frequency, after which the stage is dropped.
    closing: bool,
    /// `b0 b1 b2 a1 a2`, already divided by `a0`.
    coeffs: [f32; 5],
    /// The two delay slots of each filtered channel.
    z: [[f32; 2]; MAX_FILTER_CHANNELS],
}

impl Stage {
    fn settled(side: Side, hz: f32, rate: f32) -> Stage {
        let mut stage = Stage {
            side,
            log_cutoff: log2(clamp_hz(hz, rate)),
            target: 0.0,
            step: 0.0,
            left: 0,
            closing: false,
            coeffs: [0.0; 5],
            z: [[0.0; 2]; MAX_FILTER_CHANNELS],
        };
        stage.target = stage.log_cutoff;
        stage.update_coeffs(rate);
        stage
    }

    /// Plans a sweep from the current cutoff to `hz` over `blocks` blocks.
    fn sweep_to(&mut self, hz: f32, rate: f32, blocks: u32) {
        self.target = log2(clamp_hz(hz, rate));
        self.left = blocks.max(1);
        self.step = (self.target - self.log_cutoff) / self.left as f32;
    }

    fn update_coeffs(&mut self, rate: f32) {
        let hz = clamp_hz(exp2(self.log_cutoff), rate);
        let (sin, cos) = sin_cos(std::f32::consts::TAU * hz / rate);
        let alpha = sin / std::f32::consts::SQRT_2;
        let (b0, b1) = match self.side {
            Side::Low => ((1.0 - cos) * 0.5, 1.0 - cos),
            Side::High => ((1.0 + cos) * 0.5, -(1.0 + cos)),
        };
        let a0 = 1.0 + alpha;
        self.coeffs = [
            b0 / a0,
            b1 / a0,
            b0 / a0,
            -2.0 * cos / a0,
            (1.0 - alpha) / a0,
        ];
    }

    /// Advances the sweep one block.
    fn step_block(&mut self, rate: f32) {
        if self.left == 0 {
            return;
        }
        self.left -= 1;
        self.log_cutoff = if self.left == 0 {
            self.target
        } else {
            self.log_cutoff + self.step
        };
        self.update_coeffs(rate);
    }

    fn process(&mut self, channel: usize, x: f32) -> f32 {
        let Some(z) = self.z.get_mut(channel) else {
            return x;
        };
        let [b0, b1, b2, a1, a2] = self.coeffs;
        let y = b0 * x + z[0];
        z[0] = b1 * x - a1 * y + z[1];
        z[1] = b2 * x - a2 * y;
        y
    }
}

fn clamp_hz(hz: f32, rate: f32) -> f32 {
    let hz = if hz.is_finite() {
        hz
    } else {
        HIGH_PASS_OPEN_HZ
    };
    hz.clamp(
        HIGH_PASS_OPEN_HZ,
        (NYQUIST_MARGIN * rate).max(HIGH_PASS_OPEN_HZ),
    )
}

/// A voice's filter state: a stage per side that is on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VoiceDsp {
    /// The filter last asked for; the stages sweep toward it.
    filter: VoiceFilter,
    rate: f32,
    slew_blocks: u32,
    low: Option<Stage>,
    high: Option<Stage>,
}

impl VoiceDsp {
    /// A dsp settled at `filter` for a device at `rate` Hz, whose later changes sweep over
    /// `ramp_frames` device frames.
    pub(crate) fn new(filter: VoiceFilter, rate: u32, ramp_frames: usize) -> VoiceDsp {
        let rate = rate as f32;
        VoiceDsp {
            filter,
            rate,
            slew_blocks: ramp_frames.div_ceil(MIX_BLOCK_FRAMES).max(1) as u32,
            low: filter
                .low_pass_on()
                .then(|| Stage::settled(Side::Low, filter.low_pass_hz, rate)),
            high: filter
                .high_pass_on()
                .then(|| Stage::settled(Side::High, filter.high_pass_hz, rate)),
        }
    }

    /// The filter last asked for.
    pub(crate) fn filter(&self) -> VoiceFilter {
        self.filter
    }

    /// Whether any stage is live.
    pub(crate) fn is_active(&self) -> bool {
        self.low.is_some() || self.high.is_some()
    }

    /// Moves the dsp toward `filter`: a side turning on sweeps in from its open frequency, a moving
    /// side re-plans from where it is, and a side turning off sweeps open and then drops its stage.
    pub(crate) fn retarget(&mut self, filter: VoiceFilter) {
        self.filter = filter;
        let (rate, blocks) = (self.rate, self.slew_blocks);
        retarget_side(
            &mut self.low,
            Side::Low,
            filter.low_pass_on(),
            filter.low_pass_hz,
            rate,
            blocks,
        );
        retarget_side(
            &mut self.high,
            Side::High,
            filter.high_pass_on(),
            filter.high_pass_hz,
            rate,
            blocks,
        );
    }

    /// Steps every live sweep one block; call once per block before filtering.
    pub(crate) fn step_block(&mut self) {
        let rate = self.rate;
        for slot in [&mut self.low, &mut self.high] {
            let drop_stage = slot.as_mut().is_some_and(|stage| {
                stage.step_block(rate);
                stage.closing && stage.left == 0
            });
            if drop_stage {
                *slot = None;
            }
        }
    }

    /// `x` through every live stage, on `channel`.
    pub(crate) fn process(&mut self, channel: usize, x: f32) -> f32 {
        let mut y = x;
        if let Some(stage) = &mut self.low {
            y = stage.process(channel, y);
        }
        if let Some(stage) = &mut self.high {
            y = stage.process(channel, y);
        }
        y
    }

    /// The current cutoffs in Hz as `(low, high)`, `None` for a bypassed side.
    #[cfg(test)]
    pub(crate) fn cutoffs(&self) -> (Option<f32>, Option<f32>) {
        let hz = |stage: &Option<Stage>| stage.as_ref().map(|s| exp2(s.log_cutoff));
        (hz(&self.low), hz(&self.high))
    }
}

fn retarget_side(slot: &mut Option<Stage>, side: Side, on: bool, hz: f32, rate: f32, blocks: u32) {
    match (slot.as_mut(), on) {
        (None, false) => {}
        (None, true) => {
            let mut stage = Stage::settled(side, side.open_hz(), rate);
            stage.sweep_to(hz, rate, blocks);
            *slot = Some(stage);
        }
        (Some(stage), true) => {
            let target = log2(clamp_hz(hz, rate));
            if stage.closing || stage.target != target {
                stage.closing = false;
                stage.sweep_to(hz, rate, blocks);
            }
        }
        (Some(stage), false) => {
            if !stage.closing {
                stage.closing = true;
                stage.sweep_to(side.open_hz(), rate, blocks);
            }
        }
    }
}
