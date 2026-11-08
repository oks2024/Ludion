//! The byte codec the engine's snapshot carries a pool and its last events in.

use std::fmt;

use ludion_spatial::spatial3d::{Handle, HandleKind};

use crate::pool::{Cols, PIERCE_MEMORY, ProjectileId, ProjectilePool, Response, has_ext};
use crate::step::{Expired, HitEvent, HitOutcome, StepEvents};

const POOL_MAGIC: &[u8; 4] = b"LPRJ";
const EVENTS_MAGIC: &[u8; 4] = b"LPEV";
const VERSION: u8 = 1;

/// Largest pool capacity, for [`ProjectilePool::decode`] and the engine's config: 4× the 5,000-shot
/// target, rounded up to a power of two. A step's per-slot scratch is about 0.7 KB, so the cap bounds
/// the worst case near 30 MB and a corrupt length cannot request an unbounded allocation.
pub const MAX_DECODE_CAPACITY: u32 = 1 << 15;

/// Why a [`ProjectilePool`] or [`StepEvents`] blob did not decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeError(&'static str);

impl DecodeError {
    /// What was wrong with the bytes.
    pub fn reason(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid projectile blob: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

type Result<T> = std::result::Result<T, DecodeError>;

fn put_f32x3(out: &mut Vec<u8>, v: [f32; 3]) {
    v.iter()
        .for_each(|c| out.extend_from_slice(&c.to_bits().to_le_bytes()));
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_handle(out: &mut Vec<u8>, h: Handle) {
    out.push(match h.kind {
        HandleKind::Entity => 0,
        HandleKind::Slot => 1,
    });
    put_u32(out, h.index);
    put_u32(out, h.generation);
}

fn put_opt_handle(out: &mut Vec<u8>, h: Option<Handle>) {
    match h {
        None => out.push(0),
        Some(h) => {
            out.push(1);
            put_handle(out, h);
        }
    }
}

fn put_response(out: &mut Vec<u8>, r: Response) {
    match r {
        Response::Despawn => out.push(0),
        Response::Reflect {
            restitution,
            max_bounces,
        } => {
            out.push(1);
            put_u32(out, restitution.to_bits());
            out.extend_from_slice(&max_bounces.to_le_bytes());
        }
        Response::Pierce { max } => {
            out.push(2);
            out.extend_from_slice(&max.to_le_bytes());
        }
        Response::Stop => out.push(3),
        Response::Settle {
            restitution,
            friction,
            rest_speed,
            floor_cos,
        } => {
            out.push(4);
            for v in [restitution, friction, rest_speed, floor_cos] {
                put_u32(out, v.to_bits());
            }
        }
    }
}

/// A bounds-checked cursor over the input; every read is `Err` instead of a panic.
struct Reader<'a, 'b>(&'b mut &'a [u8]);

impl Reader<'_, '_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        if self.0.len() < n {
            return Err(DecodeError("truncated"));
        }
        let (head, tail) = self.0.split_at(n);
        *self.0 = tail;
        Ok(head)
    }
    fn remaining(&self) -> usize {
        self.0.len()
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn f32x3(&mut self) -> Result<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    fn flag(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError("bad flag byte")),
        }
    }
    fn handle(&mut self) -> Result<Handle> {
        let kind = match self.u8()? {
            0 => HandleKind::Entity,
            1 => HandleKind::Slot,
            _ => return Err(DecodeError("bad handle kind")),
        };
        Ok(Handle {
            kind,
            index: self.u32()?,
            generation: self.u32()?,
        })
    }
    fn opt_handle(&mut self) -> Result<Option<Handle>> {
        Ok(if self.flag()? {
            Some(self.handle()?)
        } else {
            None
        })
    }
    fn response(&mut self) -> Result<Response> {
        Ok(match self.u8()? {
            0 => Response::Despawn,
            1 => Response::Reflect {
                restitution: self.f32()?,
                max_bounces: self.u16()?,
            },
            2 => Response::Pierce { max: self.u16()? },
            3 => Response::Stop,
            4 => Response::Settle {
                restitution: self.f32()?,
                friction: self.f32()?,
                rest_speed: self.f32()?,
                floor_cos: self.f32()?,
            },
            _ => return Err(DecodeError("bad response tag")),
        })
    }
    fn header(&mut self, magic: &[u8; 4]) -> Result<()> {
        if self.take(4)? != magic {
            return Err(DecodeError("bad magic"));
        }
        if self.u8()? != VERSION {
            return Err(DecodeError("unsupported version"));
        }
        Ok(())
    }
}

/// Smallest encoded slot (`owner` is a 1-byte tag when absent and `Response` is 1 byte for
/// the payload-free variants): `decode` rejects a `high_water` the payload cannot hold before
/// allocating.
const SLOT_BYTES: usize = 12 + 12 + 4 * 3 + 1 + 4 + 4 + 1 + 2 + 2 + PIERCE_MEMORY * 9 + 1 + 4 + 1;

impl ProjectilePool {
    /// Appends the whole pool: every column of every slot below the high-water mark, the free
    /// list (order matters: it decides the next ids) and the live count. Floats are raw bits, so
    /// NaN and `-0.0` survive. A slot whose spin, orientation or per-shot targets are not at their
    /// defaults sets bit 1 of its parked byte and appends them; every other slot writes the
    /// original bytes. [`ProjectilePool::decode`] is the inverse.
    ///
    /// # Example
    /// ```
    /// use ludion_projectiles::{ProjectileDesc, ProjectilePool};
    ///
    /// let mut pool = ProjectilePool::with_capacity(4);
    /// pool.spawn(ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0])).unwrap();
    /// let mut bytes = Vec::new();
    /// pool.encode(&mut bytes);
    /// let back = ProjectilePool::decode(&mut bytes.as_slice()).unwrap();
    /// assert_eq!(back.digest(), pool.digest());
    /// ```
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(POOL_MAGIC);
        out.push(VERSION);
        put_u32(out, self.capacity() as u32);
        put_u32(out, self.high_water as u32);
        put_u32(out, self.live as u32);
        put_u32(out, self.free.len() as u32);
        self.free.iter().for_each(|&s| put_u32(out, s));
        let c = &self.cols;
        for i in 0..self.high_water {
            put_f32x3(out, c.pos[i]);
            put_f32x3(out, c.vel[i]);
            for v in [c.gravity_scale[i], c.drag[i], c.radius[i]] {
                put_u32(out, v.to_bits());
            }
            put_opt_handle(out, c.owner[i]);
            put_u32(out, c.life[i].to_bits());
            put_u32(out, c.tag[i]);
            put_response(out, c.response[i]);
            out.extend_from_slice(&c.bounces[i].to_le_bytes());
            out.extend_from_slice(&c.pierces[i].to_le_bytes());
            c.pierced[i].iter().for_each(|&h| put_handle(out, h));
            let ext = has_ext(c, i);
            out.push(c.parked[i] as u8 | (ext as u8) << 1);
            if ext {
                put_f32x3(out, c.spin[i]);
                c.orient[i].iter().for_each(|v| put_u32(out, v.to_bits()));
                match c.targets[i] {
                    None => out.push(0),
                    Some(m) => {
                        out.push(1);
                        put_u32(out, m);
                    }
                }
            }
            put_u32(out, c.generation[i]);
            out.push(c.alive[i] as u8);
        }
    }

    /// Reads a pool written by [`ProjectilePool::encode`], advancing `bytes` past it. Checks the
    /// magic, version, every length and tag, and the pool's invariants (the free list holds
    /// exactly the dead slots below the high-water mark, once each; `live` matches). Never panics.
    pub fn decode(bytes: &mut &[u8]) -> Result<Self> {
        let mut r = Reader(bytes);
        r.header(POOL_MAGIC)?;
        let capacity = r.u32()?;
        let high_water = r.u32()?;
        let live = r.u32()?;
        let free_len = r.u32()?;
        if capacity > MAX_DECODE_CAPACITY {
            return Err(DecodeError("capacity too large"));
        }
        if high_water > capacity {
            return Err(DecodeError("high water above capacity"));
        }
        if free_len > high_water || live > high_water {
            return Err(DecodeError("count above high water"));
        }
        if r.remaining() < free_len as usize * 4 {
            return Err(DecodeError("truncated"));
        }
        let free: Vec<u32> = (0..free_len).map(|_| r.u32()).collect::<Result<_>>()?;
        if (high_water as usize).saturating_mul(SLOT_BYTES) > r.remaining() {
            return Err(DecodeError("truncated"));
        }
        let mut pool = ProjectilePool::with_capacity(capacity as usize);
        let c: &mut Cols = &mut pool.cols;
        for i in 0..high_water as usize {
            c.pos[i] = r.f32x3()?;
            c.vel[i] = r.f32x3()?;
            c.gravity_scale[i] = r.f32()?;
            c.drag[i] = r.f32()?;
            c.radius[i] = r.f32()?;
            c.owner[i] = r.opt_handle()?;
            c.life[i] = r.f32()?;
            c.tag[i] = r.u32()?;
            c.response[i] = r.response()?;
            c.bounces[i] = r.u16()?;
            c.pierces[i] = r.u16()?;
            for k in 0..PIERCE_MEMORY {
                c.pierced[i][k] = r.handle()?;
            }
            let state = r.u8()?;
            if state > 3 {
                return Err(DecodeError("bad parked byte"));
            }
            c.parked[i] = state & 1 == 1;
            if state & 2 != 0 {
                c.spin[i] = r.f32x3()?;
                c.orient[i] = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
                c.targets[i] = if r.flag()? { Some(r.u32()?) } else { None };
            }
            c.generation[i] = r.u32()?;
            c.alive[i] = r.flag()?;
        }
        let alive = c.alive[..high_water as usize]
            .iter()
            .filter(|&&a| a)
            .count();
        if alive != live as usize {
            return Err(DecodeError("live count does not match alive slots"));
        }
        if free.len() + alive != high_water as usize {
            return Err(DecodeError("free list does not cover the dead slots"));
        }
        let mut seen = vec![false; high_water as usize];
        for &s in &free {
            let s = s as usize;
            if s >= seen.len() || c.alive[s] || seen[s] {
                return Err(DecodeError("bad free list entry"));
            }
            seen[s] = true;
        }
        pool.free.extend_from_slice(&free);
        pool.high_water = high_water as usize;
        pool.live = live as usize;
        Ok(pool)
    }
}

fn outcome_tag(o: HitOutcome) -> u8 {
    match o {
        HitOutcome::Despawned => 0,
        HitOutcome::Reflected => 1,
        HitOutcome::Pierced => 2,
        HitOutcome::Stopped => 3,
    }
}

/// Smallest encoded hit: id 8 + handle 9 + point 12 + normal 12 + t 4 + velocity 12 + tag 4 + outcome 1.
const HIT_BYTES: usize = 62;
/// Encoded expiry: id 8 + position 12 + tag 4.
const EXPIRED_BYTES: usize = 24;

impl StepEvents {
    /// Appends the hits and expiries (floats as raw bits). [`StepEvents::decode`] is the inverse.
    ///
    /// # Example
    /// ```
    /// use ludion_projectiles::StepEvents;
    ///
    /// let mut bytes = Vec::new();
    /// StepEvents::default().encode(&mut bytes);
    /// let back = StepEvents::decode(&mut bytes.as_slice()).unwrap();
    /// assert!(back.hits.is_empty() && back.expired.is_empty());
    /// ```
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(EVENTS_MAGIC);
        out.push(VERSION);
        put_u32(out, self.hits.len() as u32);
        for h in &self.hits {
            put_u32(out, h.id.index);
            put_u32(out, h.id.generation);
            put_handle(out, h.handle);
            put_f32x3(out, h.point);
            put_f32x3(out, h.normal);
            put_u32(out, h.t.to_bits());
            put_f32x3(out, h.velocity);
            put_u32(out, h.tag);
            out.push(outcome_tag(h.outcome));
        }
        put_u32(out, self.expired.len() as u32);
        for e in &self.expired {
            put_u32(out, e.id.index);
            put_u32(out, e.id.generation);
            put_f32x3(out, e.position);
            put_u32(out, e.tag);
        }
    }

    /// Reads events written by [`StepEvents::encode`], advancing `bytes` past them. Never panics.
    pub fn decode(bytes: &mut &[u8]) -> Result<Self> {
        let mut r = Reader(bytes);
        r.header(EVENTS_MAGIC)?;
        let n = r.u32()? as usize;
        if r.remaining() < n.saturating_mul(HIT_BYTES) {
            return Err(DecodeError("truncated"));
        }
        let mut hits = Vec::with_capacity(n);
        for _ in 0..n {
            let id = ProjectileId {
                index: r.u32()?,
                generation: r.u32()?,
            };
            let handle = r.handle()?;
            let point = r.f32x3()?;
            let normal = r.f32x3()?;
            let t = r.f32()?;
            let velocity = r.f32x3()?;
            let tag = r.u32()?;
            let outcome = match r.u8()? {
                0 => HitOutcome::Despawned,
                1 => HitOutcome::Reflected,
                2 => HitOutcome::Pierced,
                3 => HitOutcome::Stopped,
                _ => return Err(DecodeError("bad outcome tag")),
            };
            hits.push(HitEvent {
                id,
                handle,
                point,
                normal,
                t,
                velocity,
                tag,
                outcome,
            });
        }
        let n = r.u32()? as usize;
        if r.remaining() < n.saturating_mul(EXPIRED_BYTES) {
            return Err(DecodeError("truncated"));
        }
        let mut expired = Vec::with_capacity(n);
        for _ in 0..n {
            expired.push(Expired {
                id: ProjectileId {
                    index: r.u32()?,
                    generation: r.u32()?,
                },
                position: r.f32x3()?,
                tag: r.u32()?,
            });
        }
        Ok(Self { hits, expired })
    }
}
