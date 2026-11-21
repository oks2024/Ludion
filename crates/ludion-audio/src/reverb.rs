//! The sink's one reverb: a stereo Freeverb on a mono send bus.
//!
//! Per channel, eight parallel lowpass-feedback combs feed four series allpasses. Every buffer is
//! allocated in [`Reverb::new`] at its largest (`size = 1`) length and nothing grows afterwards, so
//! the real-time callback owns and steps it. Parameter changes apply at block boundaries: feedback
//! and damping are recomputed per block, and each delay length steps toward its target by at most
//! one sample per block, so `size` never clicks.

use ludion_core::audio::ReverbParams;
use ludion_core::math::{exp2, log2};

use crate::Envelope;

/// Comb delay lengths in samples at 44.1 kHz.
const COMB_LENGTHS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
/// Allpass delay lengths in samples at 44.1 kHz.
const ALLPASS_LENGTHS: [usize; 4] = [556, 441, 341, 225];
/// The right channel's extra length on every line, in samples at 44.1 kHz.
const STEREO_SPREAD: usize = 23;
const ALLPASS_FEEDBACK: f32 = 0.5;
/// Freeverb's fixed input gain.
const INPUT_GAIN: f32 = 0.015;
/// How far `damping` reaches into the comb's one-pole lowpass.
const DAMP_SCALE: f32 = 0.4;
/// The longest pre-delay, ms (the top of `ReverbParams::pre_delay_ms`).
const PRE_DELAY_MAX_MS: f32 = 200.0;

/// A delay line's length for `base` samples at 44.1 kHz on a device at `rate`, at room `size`.
fn scaled(base: usize, rate: f32, size: f32) -> usize {
    ((base as f32 * rate / 44_100.0 * (0.5 + size)).round() as usize).max(1)
}

/// A delay line that can run shorter than its buffer.
struct Line {
    buf: Vec<f32>,
    /// The current length, never above `buf.len()`.
    len: usize,
    pos: usize,
    /// The line's length at 44.1 kHz.
    base: usize,
}

impl Line {
    fn new(base: usize, rate: f32, size: f32) -> Line {
        let capacity = scaled(base, rate, 1.0);
        Line {
            buf: vec![0.0; capacity],
            len: scaled(base, rate, size).min(capacity),
            pos: 0,
            base,
        }
    }

    /// One sample nearer the length `size` asks for.
    fn step_toward(&mut self, rate: f32, size: f32) {
        let target = scaled(self.base, rate, size).min(self.buf.len());
        if self.len < target {
            self.len += 1;
        } else if self.len > target {
            self.len -= 1;
            if self.pos >= self.len {
                self.pos = 0;
            }
        }
    }

    fn advance(&mut self) {
        self.pos += 1;
        if self.pos >= self.len {
            self.pos = 0;
        }
    }
}

struct Comb {
    line: Line,
    store: f32,
    feedback: f32,
}

impl Comb {
    fn process(&mut self, x: f32, damp: f32) -> f32 {
        let out = self.line.buf[self.line.pos];
        self.store = out * (1.0 - damp) + self.store * damp;
        self.line.buf[self.line.pos] = x + self.store * self.feedback;
        self.line.advance();
        out
    }
}

struct Allpass {
    line: Line,
}

impl Allpass {
    fn process(&mut self, x: f32) -> f32 {
        let delayed = self.line.buf[self.line.pos];
        self.line.buf[self.line.pos] = x + delayed * ALLPASS_FEEDBACK;
        self.line.advance();
        delayed - x
    }
}

struct Side {
    combs: [Comb; 8],
    allpasses: [Allpass; 4],
}

impl Side {
    fn new(spread: usize, rate: f32, size: f32) -> Side {
        Side {
            combs: COMB_LENGTHS.map(|base| Comb {
                line: Line::new(base + spread, rate, size),
                store: 0.0,
                feedback: 0.0,
            }),
            allpasses: ALLPASS_LENGTHS.map(|base| Allpass {
                line: Line::new(base + spread, rate, size),
            }),
        }
    }

    fn process(&mut self, x: f32, damp: f32) -> f32 {
        let mut sum = 0.0;
        for comb in &mut self.combs {
            sum += comb.process(x, damp);
        }
        for allpass in &mut self.allpasses {
            sum = allpass.process(sum);
        }
        sum
    }

    fn clear(&mut self) {
        for comb in &mut self.combs {
            comb.line.buf.fill(0.0);
            comb.store = 0.0;
        }
        for allpass in &mut self.allpasses {
            allpass.line.buf.fill(0.0);
        }
    }
}

/// The reverb: a mono send in, a stereo return added to the mix.
pub(crate) struct Reverb {
    rate: f32,
    params: ReverbParams,
    sides: [Side; 2],
    pre: Vec<f32>,
    pre_pos: usize,
    pre_offset: usize,
    /// The return level.
    wet: Envelope,
    damp: f32,
}

impl Reverb {
    /// A dry reverb for a device at `rate` Hz, with every buffer allocated.
    pub(crate) fn new(rate: u32) -> Reverb {
        let rate = rate.max(1) as f32;
        let params = ReverbParams::DRY;
        Reverb {
            rate,
            params,
            sides: [
                Side::new(0, rate, params.size),
                Side::new(STEREO_SPREAD, rate, params.size),
            ],
            pre: vec![0.0; (PRE_DELAY_MAX_MS * rate / 1000.0).ceil() as usize + 1],
            pre_pos: 0,
            pre_offset: 0,
            wet: Envelope::at(0.0),
            damp: 0.0,
        }
    }

    /// Heads for `params` (already clamped): everything but the return level applies at the next
    /// block, and the level slews over `frames` device frames.
    pub(crate) fn set(&mut self, params: ReverbParams, frames: usize) {
        self.params = params;
        self.pre_offset =
            ((params.pre_delay_ms * self.rate / 1000.0).round() as usize).min(self.pre.len() - 1);
        self.wet.steer(params.wet, frames);
    }

    /// Slews the return to silence over `frames` device frames, keeping the room.
    pub(crate) fn release(&mut self, frames: usize) {
        self.params.wet = 0.0;
        self.wet.slew_to(0.0, frames);
    }

    /// Whether the reverb has a tail to render: its return is nonzero or still slewing.
    pub(crate) fn is_active(&self) -> bool {
        self.params.wet != 0.0
            || self.wet.target != 0.0
            || self.wet.level != 0.0
            || self.wet.left > 0
    }

    /// Steps the lengths one sample toward the room and recomputes feedback and damping.
    fn step_block(&mut self) {
        let (rate, size) = (self.rate, self.params.size);
        let decay = self.params.decay_s.max(0.1);
        let log2_10 = log2(10.0);
        for side in &mut self.sides {
            for comb in &mut side.combs {
                comb.line.step_toward(rate, size);
                let exponent = -3.0 * comb.line.len as f32 / (decay * rate);
                comb.feedback = exp2(exponent * log2_10);
            }
            for allpass in &mut side.allpasses {
                allpass.line.step_toward(rate, size);
            }
        }
        self.damp = self.params.damping * DAMP_SCALE;
    }

    /// Runs one block: `send` is the mono input, and the stereo return is added into `out`
    /// (`channels` interleaved; on mono it is the mean of both sides, past two channels only the
    /// first two get it). Once the return has settled at silence the state is cleared in place.
    pub(crate) fn process(&mut self, send: &[f32], out: &mut [f32], channels: usize) {
        self.step_block();
        let channels = channels.max(1);
        let [left, right] = &mut self.sides;
        for (frame, &x) in out.chunks_mut(channels).zip(send) {
            self.pre[self.pre_pos] = x;
            let read = (self.pre_pos + self.pre.len() - self.pre_offset) % self.pre.len();
            let input = self.pre[read] * INPUT_GAIN;
            self.pre_pos = (self.pre_pos + 1) % self.pre.len();
            let wet = self.wet.next();
            let l = left.process(input, self.damp) * wet;
            let r = right.process(input, self.damp) * wet;
            match frame {
                [only] => *only += (l + r) * 0.5,
                [a, b, ..] => {
                    *a += l;
                    *b += r;
                }
                [] => {}
            }
        }
        if !self.is_active() {
            self.clear();
        }
    }

    fn clear(&mut self) {
        for side in &mut self.sides {
            side.clear();
        }
        self.pre.fill(0.0);
    }

    /// Every delay buffer's capacity, pre-delay first.
    #[cfg(test)]
    pub(crate) fn capacities(&self) -> Vec<usize> {
        let mut all = vec![self.pre.capacity()];
        for side in &self.sides {
            all.extend(side.combs.iter().map(|c| c.line.buf.capacity()));
            all.extend(side.allpasses.iter().map(|a| a.line.buf.capacity()));
        }
        all
    }

    /// Every current delay length, left side's combs first.
    #[cfg(test)]
    pub(crate) fn lengths(&self) -> Vec<usize> {
        let mut all = Vec::new();
        for side in &self.sides {
            all.extend(side.combs.iter().map(|c| c.line.len));
            all.extend(side.allpasses.iter().map(|a| a.line.len));
        }
        all
    }

    /// Whether any delay buffer holds a nonzero sample.
    #[cfg(test)]
    pub(crate) fn holds_signal(&self) -> bool {
        self.sides.iter().any(|side| {
            side.combs
                .iter()
                .any(|c| c.line.buf.iter().any(|&s| s != 0.0))
                || side
                    .allpasses
                    .iter()
                    .any(|a| a.line.buf.iter().any(|&s| s != 0.0))
        }) || self.pre.iter().any(|&s| s != 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALL: ReverbParams = ReverbParams {
        decay_s: 0.5,
        pre_delay_ms: 0.0,
        damping: 0.5,
        size: 0.5,
        wet: 1.0,
    };

    /// The reverb's mono return to a unit impulse, `seconds` long at `rate`, block by block.
    fn impulse_response(params: ReverbParams, rate: u32, seconds: f32) -> Vec<f32> {
        let mut reverb = Reverb::new(rate);
        reverb.set(params, 0);
        let total = (seconds * rate as f32) as usize;
        let mut out = vec![0.0; total];
        let mut input = vec![0.0; total];
        input[0] = 1.0;
        for (send, block) in input.chunks(1024).zip(out.chunks_mut(1024)) {
            reverb.process(send, block, 1);
        }
        out
    }

    fn energy(window: &[f32]) -> f32 {
        window.iter().map(|s| s * s).sum()
    }

    #[test]
    fn the_tail_falls_by_the_decay_time() {
        let rate = 48_000;
        let window = |s: f32, e: f32| (s * rate as f32) as usize..(e * rate as f32) as usize;
        let short = impulse_response(
            ReverbParams {
                decay_s: 0.5,
                ..HALL
            },
            rate,
            0.5,
        );
        let long = impulse_response(
            ReverbParams {
                decay_s: 2.0,
                ..HALL
            },
            rate,
            0.5,
        );
        let early = energy(&short[window(0.0, 0.05)]);
        let tail = energy(&short[window(0.3, 0.35)]);
        assert!(early > 0.0);
        assert!(tail < early / 100.0, "tail {tail} early {early}");
        assert!(energy(&long[window(0.3, 0.35)]) > tail);
    }

    #[test]
    fn damping_takes_high_frequencies_from_the_tail() {
        let rate = 48_000;
        let rough = |damping: f32| {
            let response = impulse_response(ReverbParams { damping, ..HALL }, rate, 0.6);
            let tail = &response[(0.1 * rate as f32) as usize..];
            let diff: Vec<f32> = tail.windows(2).map(|w| w[1] - w[0]).collect();
            energy(&diff) / energy(tail)
        };
        assert!(rough(1.0) < rough(0.0));
    }

    #[test]
    fn no_length_reaches_zero_at_a_test_rate() {
        for size in [0.0, 0.5, 1.0] {
            let mut reverb = Reverb::new(400);
            reverb.set(ReverbParams { size, ..HALL }, 0);
            for _ in 0..40 {
                reverb.step_block();
            }
            assert!(reverb.lengths().iter().all(|&len| len >= 1), "{size}");
        }
    }

    #[test]
    fn size_steps_one_sample_per_block() {
        let mut reverb = Reverb::new(48_000);
        reverb.set(ReverbParams { size: 0.0, ..HALL }, 0);
        for _ in 0..2000 {
            reverb.step_block();
        }
        let before = reverb.lengths();
        reverb.set(ReverbParams { size: 1.0, ..HALL }, 0);
        reverb.step_block();
        let after = reverb.lengths();
        for (a, b) in before.iter().zip(&after) {
            assert_eq!(*b, a + 1);
        }
    }

    #[test]
    fn a_settled_dry_reverb_goes_idle_and_clears() {
        let mut reverb = Reverb::new(8_000);
        reverb.set(HALL, 0);
        let mut out = vec![0.0; 256];
        reverb.process(&vec![1.0; 256], &mut out, 1);
        assert!(reverb.is_active() && reverb.holds_signal());
        reverb.release(64);
        assert!(reverb.is_active());
        reverb.process(&vec![0.0; 256], &mut out, 1);
        assert!(!reverb.is_active());
        assert!(!reverb.holds_signal());
    }

    #[test]
    fn a_stereo_return_differs_between_the_sides() {
        let mut reverb = Reverb::new(8_000);
        reverb.set(HALL, 0);
        let mut input = vec![0.0; 2048];
        input[0] = 1.0;
        let mut out = vec![0.0; 4096];
        reverb.process(&input, &mut out, 2);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let right: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        assert_ne!(left, right);
        assert!(energy(&left) > 0.0 && energy(&right) > 0.0);
    }
}
