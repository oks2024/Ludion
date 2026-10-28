//! Seeded trauma noise.

use ludion_core::math::sin;

const TAU: f32 = core::f32::consts::TAU;

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn phase(hash: u64) -> f32 {
    (hash >> 40) as f32 / (1u64 << 24) as f32 * TAU
}

/// Smooth noise in `[-1, 1]` for `channel` at gameplay `time`: two sines with seeded phases. The
/// cycle fraction is taken in `f64` so a long session keeps its precision.
pub(crate) fn shake_noise(seed: u64, channel: u32, time: f64, frequency: f32) -> f32 {
    let h0 = splitmix64(seed ^ splitmix64(u64::from(channel)));
    let h1 = splitmix64(h0);
    let cycles = f64::from(frequency) * time;
    let a = (cycles.rem_euclid(1.0) as f32) * TAU + phase(h0);
    let b = ((cycles * 2.17).rem_euclid(1.0) as f32) * TAU + phase(h1);
    (sin(a) + 0.5 * sin(b)) / 1.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_bounded_deterministic_and_seeded() {
        let mut differs = false;
        for i in 0..2000 {
            let t = f64::from(i) / 60.0;
            for channel in 0..6 {
                let n = shake_noise(1, channel, t, 18.0);
                assert!((-1.0..=1.0).contains(&n), "{n}");
                assert_eq!(n.to_bits(), shake_noise(1, channel, t, 18.0).to_bits());
                differs |= n != shake_noise(2, channel, t, 18.0);
            }
        }
        assert!(differs);
    }

    #[test]
    fn channels_are_decorrelated() {
        assert_ne!(shake_noise(1, 0, 0.3, 18.0), shake_noise(1, 1, 0.3, 18.0));
    }
}
