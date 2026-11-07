//! The pool digest the engine folds into its world hash.

use ludion_spatial::spatial3d::{Handle, HandleKind};
use xxhash_rust::xxh3::Xxh3;

use crate::pool::{PIERCE_MEMORY, ProjectilePool, Response};

const DOMAIN: &[u8] = b"ludion.projectiles.v2";
/// Canonical NaN: x86 and ARM disagree on the default NaN bits.
const NAN_BITS: u32 = 0x7fc0_0000;

/// A fixed stack buffer one slot is encoded into.
struct Buf {
    bytes: [u8; 192],
    len: usize,
}

impl Buf {
    fn u8(&mut self, v: u8) {
        self.bytes[self.len] = v;
        self.len += 1;
    }
    fn u16(&mut self, v: u16) {
        self.bytes[self.len..self.len + 2].copy_from_slice(&v.to_le_bytes());
        self.len += 2;
    }
    fn u32(&mut self, v: u32) {
        self.bytes[self.len..self.len + 4].copy_from_slice(&v.to_le_bytes());
        self.len += 4;
    }
    /// Raw bits (`-0.0` stays distinct from `0.0`), NaN canonicalised.
    fn f32(&mut self, v: f32) {
        self.u32(if v.is_nan() { NAN_BITS } else { v.to_bits() });
    }
    fn f32x3(&mut self, v: [f32; 3]) {
        v.iter().for_each(|&c| self.f32(c));
    }
    fn handle(&mut self, h: Handle) {
        self.u8(match h.kind {
            HandleKind::Entity => 0,
            HandleKind::Slot => 1,
        });
        self.u32(h.index);
        self.u32(h.generation);
    }
}

impl ProjectilePool {
    /// XXH3-64 over the domain tag, the live count, the id-allocation state (high-water mark, the
    /// free list in the order it will be reused, and the generation of every dead slot below the
    /// high-water mark) and, for each live slot in ascending order, its index, generation and every
    /// field as little-endian bits (NaN canonicalised, `-0.0` distinct). Covering the allocation
    /// state means a divergence shows at once, not at the next spawn.
    ///
    /// # Example
    /// ```
    /// use ludion_projectiles::{ProjectileDesc, ProjectilePool};
    ///
    /// let mut a = ProjectilePool::with_capacity(4);
    /// let b = ProjectilePool::with_capacity(4);
    /// assert_eq!(a.digest(), b.digest());
    /// a.spawn(ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0])).unwrap();
    /// assert_ne!(a.digest(), b.digest());
    /// ```
    pub fn digest(&self) -> u64 {
        let mut hasher = Xxh3::new();
        hasher.update(DOMAIN);
        hasher.update(&(self.len() as u32).to_le_bytes());
        let c = &self.cols;
        hasher.update(&(self.high_water as u32).to_le_bytes());
        hasher.update(&(self.free.len() as u32).to_le_bytes());
        for slot in self.free.iter().rev() {
            hasher.update(&slot.to_le_bytes());
        }
        for i in (0..self.high_water).filter(|&i| !c.alive[i]) {
            hasher.update(&c.generation[i].to_le_bytes());
        }
        for i in (0..self.high_water).filter(|&i| c.alive[i]) {
            let mut b = Buf {
                bytes: [0; 192],
                len: 0,
            };
            b.u32(i as u32);
            b.u32(c.generation[i]);
            b.f32x3(c.pos[i]);
            b.f32x3(c.vel[i]);
            b.f32(c.gravity_scale[i]);
            b.f32(c.drag[i]);
            b.f32(c.radius[i]);
            match c.owner[i] {
                None => b.u8(0),
                Some(h) => {
                    b.u8(1);
                    b.handle(h);
                }
            }
            b.f32(c.life[i]);
            b.u32(c.tag[i]);
            match c.response[i] {
                Response::Despawn => b.u8(0),
                Response::Reflect {
                    restitution,
                    max_bounces,
                } => {
                    b.u8(1);
                    b.f32(restitution);
                    b.u16(max_bounces);
                }
                Response::Pierce { max } => {
                    b.u8(2);
                    b.u16(max);
                }
                Response::Stop => b.u8(3),
            }
            b.u16(c.bounces[i]);
            b.u16(c.pierces[i]);
            for h in &c.pierced[i][..(c.pierces[i] as usize).min(PIERCE_MEMORY)] {
                b.handle(*h);
            }
            b.u8(c.parked[i] as u8);
            hasher.update(&b.bytes[..b.len]);
        }
        hasher.digest()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProjectileDesc;

    fn pool_with(velocity: [f32; 3]) -> ProjectilePool {
        let mut pool = ProjectilePool::with_capacity(4);
        pool.spawn(ProjectileDesc::new([1.0, 2.0, 3.0], velocity))
            .unwrap();
        pool
    }

    #[test]
    fn an_empty_pool_has_a_stable_digest() {
        let a = ProjectilePool::with_capacity(0).digest();
        assert_eq!(a, ProjectilePool::with_capacity(64).digest());
        let mut hasher = Xxh3::new();
        hasher.update(b"ludion.projectiles.v2");
        // Live count, high water and free count, all zero.
        hasher.update(&[0u8; 12]);
        assert_eq!(a, hasher.digest());
    }

    #[test]
    fn one_changed_field_changes_the_digest() {
        let base = pool_with([1.0, 0.0, 0.0]).digest();
        assert_eq!(base, pool_with([1.0, 0.0, 0.0]).digest());
        assert_ne!(base, pool_with([1.0, 0.0, 0.5]).digest());
        let mut tagged = pool_with([1.0, 0.0, 0.0]);
        let mut desc = ProjectileDesc::new([1.0, 2.0, 3.0], [1.0, 0.0, 0.0]);
        desc.tag = 1;
        let first = tagged.ids().next().unwrap();
        tagged.despawn(first);
        tagged.spawn(desc).unwrap();
        assert_ne!(base, tagged.digest());
    }

    #[test]
    fn identity_and_content_both_count() {
        // Same two contents; slot order differs, so the digest differs.
        let a = ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0]);
        let b = ProjectileDesc::new([0.0; 3], [2.0, 0.0, 0.0]);
        let mut ab = ProjectilePool::with_capacity(2);
        ab.spawn(a).unwrap();
        ab.spawn(b).unwrap();
        let mut ba = ProjectilePool::with_capacity(2);
        ba.spawn(b).unwrap();
        ba.spawn(a).unwrap();
        assert_ne!(ab.digest(), ba.digest());
        // A freed slot is not hashed, but the generation bump it leaves behind is.
        let mut churned = ProjectilePool::with_capacity(2);
        let id = churned.spawn(a).unwrap();
        churned.despawn(id);
        churned.spawn(a).unwrap();
        let mut fresh = ProjectilePool::with_capacity(2);
        fresh.spawn(a).unwrap();
        assert_ne!(churned.digest(), fresh.digest());
    }

    #[test]
    fn the_free_list_order_is_hashed() {
        let desc = ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0]);
        let drained = |order: [usize; 2]| {
            let mut pool = ProjectilePool::with_capacity(4);
            let ids: Vec<_> = (0..3).map(|_| pool.spawn(desc).unwrap()).collect();
            pool.despawn(ids[order[0]]);
            pool.despawn(ids[order[1]]);
            pool.digest()
        };
        assert_ne!(drained([0, 2]), drained([2, 0]));
    }

    #[test]
    fn nan_hashes_alike_and_negative_zero_is_distinct() {
        let mut p = pool_with([1.0, 0.0, 0.0]);
        let mut q = pool_with([1.0, 0.0, 0.0]);
        let id = p.ids().next().unwrap();
        p.cols.vel[id.index as usize][1] = f32::from_bits(0x7fc0_0000);
        q.cols.vel[id.index as usize][1] = f32::from_bits(0xffc0_0001);
        assert_eq!(p.digest(), q.digest());
        q.cols.vel[id.index as usize][1] = -0.0;
        let zero = pool_with([1.0, 0.0, 0.0]).digest();
        assert_ne!(q.digest(), zero);
    }
}
