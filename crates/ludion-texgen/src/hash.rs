//! Integer hashing and seed mixing: the only source of randomness, a pure function of its inputs.

/// SplitMix64's finalizer; a bijection with good avalanche.
pub fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Combines two seeds into one; order matters.
pub fn mix(a: u64, b: u64) -> u64 {
    splitmix64(a.wrapping_add(splitmix64(b)))
}

/// Folds a 64-bit seed to the 32 bits the lattice hash takes.
pub fn fold32(seed: u64) -> u32 {
    (seed ^ (seed >> 32)) as u32
}

/// Chris Wellons' `lowbias32` integer hash.
pub fn lowbias32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^ (x >> 16)
}

/// Hashes a lattice coordinate under a seed.
pub fn hash2(x: u32, y: u32, seed: u32) -> u32 {
    lowbias32(x ^ lowbias32(y ^ lowbias32(seed)))
}

/// Maps a hash to `[0, 1)` using its top 24 bits, exact in `f32`.
pub fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / 16_777_216.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_pinned() {
        assert_eq!(splitmix64(0), 0xE220_A839_7B1D_CDAF);
        assert_eq!(lowbias32(1), 1_753_845_952);
        assert_ne!(mix(1, 2), mix(2, 1));
    }

    #[test]
    fn unit_stays_below_one() {
        assert_eq!(unit(u32::MAX), 16_777_215.0 / 16_777_216.0);
        assert_eq!(unit(0), 0.0);
    }
}
