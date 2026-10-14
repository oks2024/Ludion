//! Deterministic pseudo-random numbers.
//!
//! [`DeterministicRng`] is a hand-rolled PCG32 generator (the **PCG-XSH-RR 64/32** variant from
//! O'Neill's reference implementation). The engine owns exactly one RNG, seeded from the
//! scene/replay header and carried as an ECS resource; `rand::thread_rng` and friends are banned on
//! the sim path because their per-process entropy would destroy reproducibility.
//!
//! We implement PCG ourselves rather than depend on the `rand` crate so the algorithm and its
//! constants are pinned, auditable, and stable across `rand` releases. The full state is two
//! `u64`s, so snapshotting is just `serde` on the struct.
//!
//! The `next_u32` output is validated against O'Neill's published reference sequence in the tests
//! (seed 42, stream 54).
//!
//! # Derived streams
//!
//! Drawing from the one master resource makes every draw depend on every earlier draw's *order*.
//! [`DeterministicRng::derived`] and [`DeterministicRng::derived_salted`] instead build a
//! throwaway generator whose stream is a pure function of `(world_seed, frame, entity | salt)`, so
//! the value a caller gets is independent of what else drew this tick. [`WorldSeed`] is the world
//! resource carrying the first argument.

use serde::{Deserialize, Serialize};
use std::ops::Range;

use crate::ids::EntityId;
use crate::time::FrameIndex;

/// PCG multiplier (the 64-bit LCG constant from the reference implementation).
const PCG_MULTIPLIER: u64 = 6_364_136_223_846_793_005;

/// Default stream selector for the single-argument [`DeterministicRng::new`].
///
/// Chosen so the resulting increment `(DEFAULT_STREAM << 1) | 1` equals PCG's well-known default
/// increment `1442695040888963407`. Different streams give distinct, non-correlated sequences from
/// the same seed.
const DEFAULT_STREAM: u64 = 0x0a02_bdbf_7bb3_c0a7;

/// Widest stream selector [`DeterministicRng::new_with_stream`] can actually distinguish.
///
/// The constructor computes `increment = (stream << 1) | 1`, so bit 63 of the stream is shifted
/// out and lost. Streams derived from a 64-bit hash must be masked to 63 bits, or half of all
/// derived pairs silently share an increment.
const STREAM_MASK: u64 = u64::MAX >> 1;

/// Odd multipliers mixing each input word before the splitmix64 finalizer. Distinct constants per
/// word are what make the words positionally significant (so entity index and generation cannot
/// trade places).
const WORD0_ODD: u64 = 0x9e37_79b9_7f4a_7c15;
const WORD1_ODD: u64 = 0xc2b2_ae3d_27d4_eb4f;
const WORD2_ODD: u64 = 0xd6e8_feb8_6659_fd93;

/// splitmix64 finalizer constants.
const SPLITMIX_A: u64 = 0xbf58_476d_1ce4_e5b9;
const SPLITMIX_B: u64 = 0x94d0_49bb_1331_11eb;

/// Occupies the generation slot for salted (non-entity) streams. A generation is a `u32` and this
/// constant is not, so a salt that happens to equal an entity index cannot land on that entity's
/// stream.
const SALT_DOMAIN: u64 = 0x517c_c1b7_2722_0a95;

/// Folds three input words into a 63-bit stream selector with a splitmix64 finalizer.
///
/// Each word is mixed by its own odd multiplier (`WORD1_ODD`'s product additionally rotated) before
/// the words are combined, so the fold is order- and position-sensitive; the finalizer then
/// avalanches the result. Masked to [`STREAM_MASK`] because the top bit is not a usable stream bit.
const fn fold_stream(word0: u64, word1: u64, word2: u64) -> u64 {
    let mut z = word0.wrapping_mul(WORD0_ODD)
        ^ word1.wrapping_mul(WORD1_ODD).rotate_left(32)
        ^ word2.wrapping_mul(WORD2_ODD);
    z = (z ^ (z >> 30)).wrapping_mul(SPLITMIX_A);
    z = (z ^ (z >> 27)).wrapping_mul(SPLITMIX_B);
    z ^= z >> 31;
    z & STREAM_MASK
}

/// The seed the current world was instantiated with, carried as an ECS resource.
///
/// The seed itself is already inside the master [`DeterministicRng`]'s state, but unrecoverably so.
/// Systems need it as a *value* to build derived streams, which is what this resource is for. The
/// driver inserts it next to the RNG on every world it builds (`SimDriver::new`/`restore`), so it
/// is present wherever the RNG is.
///
/// # Example
///
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_core::rng::{DeterministicRng, WorldSeed};
/// use ludion_core::time::FrameIndex;
///
/// let seed = WorldSeed(7);
/// let mut rng = DeterministicRng::derived(seed.0, FrameIndex::new(12), EntityId::new(3, 0));
/// let roll = rng.gen_range_u32(1..7);
/// assert!((1..7).contains(&roll));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct WorldSeed(pub u64);

/// A seeded, reproducible PCG32 random number generator.
///
/// Deterministic across platforms and toolchain-pinned builds: the same `(seed, stream)` always
/// yields the same sequence. The entire state is serializable, so it round-trips through snapshots
/// and replay keyframes.
///
/// # Example
///
/// ```
/// use ludion_core::rng::DeterministicRng;
///
/// // Two generators seeded identically agree forever.
/// let mut a = DeterministicRng::new(42);
/// let mut b = DeterministicRng::new(42);
/// assert_eq!(a.next_u32(), b.next_u32());
///
/// // Bounded draws land in range.
/// let mut rng = DeterministicRng::new(7);
/// let d6 = rng.gen_range_u32(1..7);
/// assert!((1..7).contains(&d6));
///
/// // `next_f32` is in [0, 1).
/// let f = rng.next_f32();
/// assert!((0.0..1.0).contains(&f));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DeterministicRng {
    state: u64,
    increment: u64,
}

impl DeterministicRng {
    /// Creates a generator from a seed, using the default stream.
    pub fn new(seed: u64) -> Self {
        Self::new_with_stream(seed, DEFAULT_STREAM)
    }

    /// Creates a generator from a seed and an explicit stream selector.
    ///
    /// Two generators with the same seed but different streams produce distinct sequences. This is
    /// the standard PCG seeding procedure: set the increment from the stream, step once, add the
    /// seed, step once.
    ///
    /// ```
    /// use ludion_core::rng::DeterministicRng;
    /// // The canonical PCG reference vector: seed 42, stream 54.
    /// let mut rng = DeterministicRng::new_with_stream(42, 54);
    /// assert_eq!(rng.next_u32(), 0xa15c_02b7);
    /// ```
    pub fn new_with_stream(seed: u64, stream: u64) -> Self {
        let mut rng = DeterministicRng {
            state: 0,
            increment: (stream << 1) | 1,
        };
        rng.step();
        rng.state = rng.state.wrapping_add(seed);
        rng.step();
        rng
    }

    /// Creates a throwaway generator for one entity's draws on one frame.
    ///
    /// The stream is a pure function of `(frame, entity index, entity generation)` — never of draw
    /// order — so the value an entity gets does not depend on which other entities or systems drew
    /// first. Both halves of the [`EntityId`] participate because storage slots recycle: an index
    /// alone would hand a fresh entity its predecessor's stream.
    ///
    /// The `world_seed` is the run's seed (the [`WorldSeed`] resource), so two runs of the same
    /// scene agree and two runs of different seeds do not.
    ///
    /// The folded stream is masked to 63 bits: [`new_with_stream`](DeterministicRng::new_with_stream)
    /// discards the top bit when it forms `increment = (stream << 1) | 1`, so an unmasked 64-bit
    /// hash would collide on bit 63 — half of all derived pairs sharing an increment.
    ///
    /// ```
    /// use ludion_core::ids::EntityId;
    /// use ludion_core::rng::DeterministicRng;
    /// use ludion_core::time::FrameIndex;
    ///
    /// let frame = FrameIndex::new(120);
    /// let a = EntityId::new(4, 0);
    /// let b = EntityId::new(5, 0);
    ///
    /// // Same inputs, same sequence — order of construction is irrelevant.
    /// let mut once = DeterministicRng::derived(9, frame, a);
    /// let mut again = DeterministicRng::derived(9, frame, a);
    /// assert_eq!(once.next_u64(), again.next_u64());
    ///
    /// // A different entity draws from a different stream.
    /// let mut other = DeterministicRng::derived(9, frame, b);
    /// assert_ne!(
    ///     DeterministicRng::derived(9, frame, a).next_u64(),
    ///     other.next_u64(),
    /// );
    /// ```
    pub fn derived(world_seed: u64, frame: FrameIndex, entity: EntityId) -> Self {
        let stream = fold_stream(
            frame.as_u64(),
            u64::from(entity.index()),
            u64::from(entity.generation()),
        );
        Self::new_with_stream(world_seed, stream)
    }

    /// Creates a throwaway generator for a draw that belongs to no single entity.
    ///
    /// The `salt` names the draw site — a chunk index, a wave number, a spawn slot. Same contract as
    /// [`derived`](DeterministicRng::derived): the stream is a pure function of `(frame, salt)`, and
    /// it is masked to 63 bits for the same reason. A salt that coincides with an entity index does
    /// not land on that entity's stream: the entity form's third fold word is a `u32` generation,
    /// this one's is a constant larger than `u32::MAX`.
    ///
    /// ```
    /// use ludion_core::rng::DeterministicRng;
    /// use ludion_core::time::FrameIndex;
    ///
    /// let frame = FrameIndex::new(30);
    /// // Two chunks of the same frame draw independently.
    /// let mut chunk0 = DeterministicRng::derived_salted(1, frame, 0);
    /// let mut chunk1 = DeterministicRng::derived_salted(1, frame, 1);
    /// assert_ne!(chunk0.next_u64(), chunk1.next_u64());
    /// ```
    pub fn derived_salted(world_seed: u64, frame: FrameIndex, salt: u64) -> Self {
        let stream = fold_stream(frame.as_u64(), salt, SALT_DOMAIN);
        Self::new_with_stream(world_seed, stream)
    }

    /// Advances the internal LCG state by one step and returns the *old* state.
    #[inline]
    fn step(&mut self) -> u64 {
        let old = self.state;
        self.state = old
            .wrapping_mul(PCG_MULTIPLIER)
            .wrapping_add(self.increment);
        old
    }

    /// Draws the next `u32`.
    ///
    /// This is the PCG-XSH-RR output function applied to the pre-step state: xorshift-high, then a
    /// rotate-right by the top 5 bits.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old = self.step();
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Draws the next `u64`, high 32 bits first.
    ///
    /// PCG32 is a 32-bit generator; a `u64` is two consecutive `u32` draws, high word first. The
    /// ordering is fixed so the value is reproducible.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let hi = u64::from(self.next_u32());
        let lo = u64::from(self.next_u32());
        (hi << 32) | lo
    }

    /// Draws an `f32` uniformly in `[0, 1)` using 24 bits of precision.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        // Take the top 24 bits and scale by 2^-24; the result is exactly representable and < 1.0.
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Draws a `u32` uniformly in the half-open range `start..end`.
    ///
    /// Uses rejection sampling to avoid modulo bias.
    ///
    /// # Panics
    /// Panics if the range is empty (`start >= end`).
    pub fn gen_range_u32(&mut self, range: Range<u32>) -> u32 {
        let Range { start, end } = range;
        assert!(start < end, "gen_range_u32 requires a non-empty range");
        let width = end - start;
        // Number of low outputs to reject so the accepted region is an exact multiple of `width`.
        // `width.wrapping_neg() % width` == `2^32 mod width`.
        let reject_below = width.wrapping_neg() % width;
        loop {
            let v = self.next_u32();
            if v >= reject_below {
                return start + v % width;
            }
        }
    }

    /// Draws a `usize` uniformly in the half-open range `start..end`.
    ///
    /// Uses rejection sampling over a 64-bit draw to avoid modulo bias.
    ///
    /// # Panics
    /// Panics if the range is empty (`start >= end`).
    pub fn gen_range_usize(&mut self, range: Range<usize>) -> usize {
        let Range { start, end } = range;
        assert!(start < end, "gen_range_usize requires a non-empty range");
        let width = (end - start) as u64;
        let reject_below = width.wrapping_neg() % width;
        loop {
            let v = self.next_u64();
            if v >= reject_below {
                return start + (v % width) as usize;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O'Neill's published PCG32 reference output for the demo seed 42 / stream 54. Matching this
    /// vector proves our core algorithm and constants are the standard PCG-XSH-RR 64/32.
    const PCG_REF_42_54: [u32; 6] = [
        0xa15c_02b7,
        0x7b47_f409,
        0xba1d_3330,
        0x83d2_f293,
        0xbfa4_784b,
        0xcbed_606e,
    ];

    #[test]
    fn matches_pcg_reference_vector() {
        let mut rng = DeterministicRng::new_with_stream(42, 54);
        for expected in PCG_REF_42_54 {
            assert_eq!(rng.next_u32(), expected);
        }
    }

    /// Golden sequence for the default-stream constructor, seed 42. Captured from this
    /// implementation after the reference vector above validated the algorithm.
    const GOLDEN_SEED_42: [u32; 8] = [
        0xc2f5_7bd6,
        0x6b07_c4a9,
        0x72b7_b29b,
        0x4421_5383,
        0xf5af_5ead,
        0x68be_b632,
        0xcbc7_312c,
        0xd5ef_c7d7,
    ];

    /// Golden sequence for the default-stream constructor, seed 7.
    const GOLDEN_SEED_7: [u32; 8] = [
        0x4be7_88d7,
        0xfa6a_c9dc,
        0x68ef_91ea,
        0xe32a_3f28,
        0x2580_c85c,
        0x8c46_201b,
        0x486f_8604,
        0x7311_74a4,
    ];

    #[test]
    fn golden_sequence_seed_42() {
        let mut rng = DeterministicRng::new(42);
        for expected in GOLDEN_SEED_42 {
            assert_eq!(rng.next_u32(), expected);
        }
    }

    #[test]
    fn golden_sequence_seed_7() {
        let mut rng = DeterministicRng::new(7);
        for expected in GOLDEN_SEED_7 {
            assert_eq!(rng.next_u32(), expected);
        }
    }

    #[test]
    fn same_seed_same_sequence() {
        let mut a = DeterministicRng::new(123);
        let mut b = DeterministicRng::new(123);
        for _ in 0..64 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_streams_differ() {
        let mut a = DeterministicRng::new_with_stream(1, 1);
        let mut b = DeterministicRng::new_with_stream(1, 2);
        // Overwhelmingly likely to differ within a few draws.
        let differ = (0..8).any(|_| a.next_u32() != b.next_u32());
        assert!(differ);
    }

    #[test]
    fn next_f32_in_unit_interval() {
        let mut rng = DeterministicRng::new(999);
        for _ in 0..10_000 {
            let f = rng.next_f32();
            assert!((0.0..1.0).contains(&f), "f32 {f} out of [0,1)");
        }
    }

    #[test]
    fn gen_range_stays_in_bounds() {
        let mut rng = DeterministicRng::new(2024);
        for _ in 0..10_000 {
            let v = rng.gen_range_u32(10..20);
            assert!((10..20).contains(&v));
            let u = rng.gen_range_usize(0..3);
            assert!(u < 3);
        }
    }

    /// Golden sequence for `derived(world_seed = 42, frame = 7, entity = e13v2)`. Captured from this
    /// implementation; it pins the fold's constants and word order, so a change to the derivation is
    /// a visible, deliberate edit rather than a silent re-shuffle of every derived draw.
    const GOLDEN_DERIVED_42_7_E13V2: [u32; 8] = [
        0x0399_77ec,
        0x337e_3464,
        0xb899_77eb,
        0xbc51_413a,
        0x8ba2_b0bc,
        0x9c7f_82ff,
        0x229b_0cf3,
        0x411a_2d35,
    ];

    /// Golden sequence for `derived_salted(world_seed = 42, frame = 7, salt = 13)`.
    const GOLDEN_DERIVED_SALTED_42_7_13: [u32; 8] = [
        0xebb5_e54c,
        0xecad_a478,
        0xbb6b_6ebc,
        0x1856_4fdc,
        0x8603_a40e,
        0x85b0_815c,
        0x5c73_40e2,
        0x682c_e946,
    ];

    #[test]
    fn golden_sequence_derived() {
        let mut rng = DeterministicRng::derived(42, FrameIndex::new(7), EntityId::new(13, 2));
        for expected in GOLDEN_DERIVED_42_7_E13V2 {
            assert_eq!(rng.next_u32(), expected);
        }
    }

    #[test]
    fn golden_sequence_derived_salted() {
        let mut rng = DeterministicRng::derived_salted(42, FrameIndex::new(7), 13);
        for expected in GOLDEN_DERIVED_SALTED_42_7_13 {
            assert_eq!(rng.next_u32(), expected);
        }
    }

    #[test]
    fn derived_is_a_pure_function_of_its_inputs() {
        let frame = FrameIndex::new(4096);
        let entity = EntityId::new(77, 1);
        let mut a = DeterministicRng::derived(11, frame, entity);
        let mut b = DeterministicRng::derived(11, frame, entity);
        for _ in 0..64 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        assert_eq!(
            DeterministicRng::derived_salted(11, frame, 5),
            DeterministicRng::derived_salted(11, frame, 5),
        );
    }

    /// Every input word must move the stream on its own: seed, frame, entity index, entity
    /// generation, and salt. A fold that dropped one would silently share a stream between draws the
    /// caller believes are independent.
    #[test]
    fn distinct_inputs_give_distinct_streams() {
        let base = DeterministicRng::derived(1, FrameIndex::new(2), EntityId::new(3, 4));
        let variants = [
            DeterministicRng::derived(2, FrameIndex::new(2), EntityId::new(3, 4)),
            DeterministicRng::derived(1, FrameIndex::new(3), EntityId::new(3, 4)),
            DeterministicRng::derived(1, FrameIndex::new(2), EntityId::new(4, 4)),
            DeterministicRng::derived(1, FrameIndex::new(2), EntityId::new(3, 5)),
            DeterministicRng::derived_salted(1, FrameIndex::new(2), 3),
        ];
        for variant in variants {
            assert_ne!(base, variant);
        }
    }

    /// Index and generation occupy different fold words, so a recycled slot never inherits the
    /// stream of the entity that held it — and swapping the two numbers gives a different stream.
    #[test]
    fn entity_index_and_generation_are_not_interchangeable() {
        let frame = FrameIndex::new(9);
        assert_ne!(
            DeterministicRng::derived(0, frame, EntityId::new(6, 2)),
            DeterministicRng::derived(0, frame, EntityId::new(2, 6)),
        );
        // Same slot, next generation: a fresh entity, a fresh stream.
        assert_ne!(
            DeterministicRng::derived(0, frame, EntityId::new(6, 0)),
            DeterministicRng::derived(0, frame, EntityId::new(6, 1)),
        );
    }

    /// The stream selector never sets bit 63, which `new_with_stream` would shift away — an unmasked
    /// fold would make bit-63-differing pairs share an increment.
    #[test]
    fn folded_streams_fit_the_usable_stream_width() {
        for frame in 0..512u64 {
            for index in 0..8u32 {
                let stream = fold_stream(frame, u64::from(index), 0);
                assert_eq!(stream & !STREAM_MASK, 0, "stream {stream:#x} sets bit 63");
            }
        }
    }

    #[test]
    fn world_seed_round_trips() {
        let seed = WorldSeed(0xdead_beef_0000_0001);
        let json = serde_json::to_string(&seed).unwrap();
        assert_eq!(serde_json::from_str::<WorldSeed>(&json).unwrap(), seed);
    }

    #[test]
    fn serde_round_trips_state() {
        let mut rng = DeterministicRng::new(5);
        for _ in 0..17 {
            rng.next_u32();
        }
        let json = serde_json::to_string(&rng).unwrap();
        let mut restored: DeterministicRng = serde_json::from_str(&json).unwrap();
        assert_eq!(rng.next_u64(), restored.next_u64());
    }
}
