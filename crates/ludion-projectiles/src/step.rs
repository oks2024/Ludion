//! The swept step: integrate, age, sweep, resolve.

use std::cmp::Ordering;

use ludion_core::par;
use ludion_physics::{PhysicsWorld, Sweep, SweepFilter};
use ludion_spatial::spatial3d::{Filter, Handle, Index3d};

use crate::pool::{Cols, PIERCE_MEMORY, ProjectileId, ProjectilePool, Response};

/// Projectiles per parallel chunk; fixed so the split never depends on the thread count.
const CHUNK: usize = 256;

/// Hits resolved per projectile per tick at most. A projectile that reaches the cap keeps its last
/// post-contact state and drops the rest of the tick's travel.
pub const MAX_HITS_PER_TICK: usize = 8;

/// Distance a reflected projectile is pushed off the surface along its normal. The reflected
/// velocity points away from the surface, so the next cast does not touch it again.
pub const SKIN: f32 = 1e-3;

/// What one tick needs besides the pool and the two worlds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepConfig {
    /// Acceleration in m/s², scaled per projectile by its `gravity_scale`.
    pub gravity: [f32; 3],
    /// The physics filter, resolved once per tick with `PhysicsWorld::sweep_filter`.
    pub world: SweepFilter,
    /// Layer mask of the `spatial3d` target index.
    pub targets: u32,
}

impl StepConfig {
    /// No gravity, every index layer.
    pub fn new(world: SweepFilter) -> Self {
        Self {
            gravity: [0.0; 3],
            world,
            targets: u32::MAX,
        }
    }
}

/// What a hit did to the projectile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitOutcome {
    /// Freed.
    Despawned,
    /// Bounced.
    Reflected,
    /// Went through an index target.
    Pierced,
    /// Parked.
    Stopped,
}

/// One resolved hit.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct HitEvent {
    /// The projectile (its generation is the one it hit with, stale if it despawned).
    pub id: ProjectileId,
    /// What was hit: an entity for a physics collider, whatever handle the index item carries.
    pub handle: Handle,
    /// Contact point on the hit surface.
    pub point: [f32; 3],
    /// Unit normal pointing from the surface toward the projectile.
    pub normal: [f32; 3],
    /// Fraction of the tick's `dt` elapsed at the contact.
    pub t: f32,
    /// The velocity on arrival, before the response (what damage reads).
    pub velocity: [f32; 3],
    /// The projectile's opaque tag.
    pub tag: u32,
    /// What the response did.
    pub outcome: HitOutcome,
}

/// A projectile that ran out of lifetime (a fuse).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Expired {
    /// The projectile, stale by the time the event is read.
    pub id: ProjectileId,
    /// Where it was.
    pub position: [f32; 3],
    /// Its opaque tag.
    pub tag: u32,
}

/// The events of one [`step`], ordered by slot ascending and then hit order.
#[derive(Clone, Debug, Default)]
pub struct StepEvents {
    /// Resolved hits.
    pub hits: Vec<HitEvent>,
    /// Lifetime expiries.
    pub expired: Vec<Expired>,
}

impl StepEvents {
    /// Buffers that hold `hits` and `expired` events without growing.
    pub fn with_capacity(hits: usize, expired: usize) -> Self {
        Self {
            hits: Vec::with_capacity(hits),
            expired: Vec::with_capacity(expired),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Fate {
    Live,
    Despawned,
    Expired,
}

/// One slot's result of phase A.
#[derive(Clone, Copy)]
pub(crate) struct Work {
    active: bool,
    fate: Fate,
    pos: [f32; 3],
    vel: [f32; 3],
    life: f32,
    bounces: u16,
    pierces: u16,
    pierced: [Handle; PIERCE_MEMORY],
    parked: bool,
    hit_count: usize,
    hits: [HitEvent; MAX_HITS_PER_TICK],
}

impl Work {
    pub(crate) fn empty() -> Self {
        let zero_hit = HitEvent {
            id: ProjectileId {
                index: 0,
                generation: 0,
            },
            handle: Handle::slot(0, 0),
            point: [0.0; 3],
            normal: [0.0; 3],
            t: 0.0,
            velocity: [0.0; 3],
            tag: 0,
            outcome: HitOutcome::Despawned,
        };
        Self {
            active: false,
            fate: Fate::Live,
            pos: [0.0; 3],
            vel: [0.0; 3],
            life: 0.0,
            bounces: 0,
            pierces: 0,
            pierced: [Handle::slot(0, 0); PIERCE_MEMORY],
            parked: false,
            hit_count: 0,
            hits: [zero_hit; MAX_HITS_PER_TICK],
        }
    }
}

/// The nearer of the two world hits, as `(t, handle, point, normal, from_world)`.
type Contact = (f32, Handle, [f32; 3], [f32; 3], bool);

/// Advances every live projectile by `dt` seconds and clears-then-fills `events`.
///
/// Per projectile: gravity then drag integrate the velocity, the lifetime ages (expiry frees the
/// slot with no sweep; a parked projectile ages but never moves), then the tick's segment
/// `v·dt` is swept against `physics` (excluding the owner entity) and `index` (skipping the owner
/// and the handles it pierced) and the nearer hit, by `(t, handle)`, is resolved with the time left
/// in the tick; a full tie goes to the physics hit. At most [`MAX_HITS_PER_TICK`] hits resolve per
/// projectile; past that it keeps its last post-contact state and drops the rest of the tick.
/// [`SKIN`] is the reflect push-out; [`PIERCE_MEMORY`] the pierce ring.
///
/// Per-slot work fans out over `ludion_core::par`; results apply in ascending slot order, so the
/// pool, the events and the freed-slot order are identical at any thread count.
///
/// # Example
/// ```
/// use ludion_core::time::TickRate;
/// use ludion_physics::{PhysicsWorld, QueryFilter};
/// use ludion_projectiles::{
///     HitOutcome, ProjectileDesc, ProjectilePool, StepConfig, StepEvents, step,
/// };
/// use ludion_spatial::spatial3d::{Handle, Index3d, Item, Volume};
///
/// let physics = PhysicsWorld::new(TickRate::DEFAULT);
/// let mut index = Index3d::new();
/// index.push(Item {
///     handle: Handle::slot(7, 0),
///     volume: Volume::Sphere { center: [5.0, 0.0, 0.0], radius: 1.0 },
///     layers: 1,
/// });
/// index.build();
/// let mut pool = ProjectilePool::with_capacity(4);
/// let mut desc = ProjectileDesc::new([0.0; 3], [600.0, 0.0, 0.0]);
/// desc.tag = 3;
/// let id = pool.spawn(desc).unwrap();
/// let cfg = StepConfig::new(physics.sweep_filter(&QueryFilter::new()).unwrap());
/// let mut events = StepEvents::with_capacity(4, 4);
/// step(&mut pool, 1.0 / 60.0, &physics, &index, &cfg, &mut events);
/// assert_eq!(events.hits.len(), 1);
/// assert_eq!(events.hits[0].handle, Handle::slot(7, 0));
/// assert_eq!(events.hits[0].outcome, HitOutcome::Despawned);
/// assert_eq!(events.hits[0].tag, 3);
/// assert!(!pool.contains(id));
/// ```
pub fn step(
    pool: &mut ProjectilePool,
    dt: f32,
    physics: &PhysicsWorld,
    index: &Index3d,
    cfg: &StepConfig,
    events: &mut StepEvents,
) {
    events.hits.clear();
    events.expired.clear();
    let hw = pool.high_water;
    {
        let ProjectilePool { cols, scratch, .. } = &mut *pool;
        let cols: &Cols = cols;
        par::par_chunks(&mut scratch[..hw], CHUNK, |chunk, works| {
            for (offset, work) in works.iter_mut().enumerate() {
                *work = sweep_slot(cols, chunk * CHUNK + offset, dt, physics, index, cfg);
            }
        });
    }
    for slot in 0..hw {
        let w = pool.scratch[slot];
        if !w.active {
            continue;
        }
        events.hits.extend_from_slice(&w.hits[..w.hit_count]);
        match w.fate {
            Fate::Live => {
                let c = &mut pool.cols;
                c.pos[slot] = w.pos;
                c.vel[slot] = w.vel;
                c.life[slot] = w.life;
                c.bounces[slot] = w.bounces;
                c.pierces[slot] = w.pierces;
                c.pierced[slot] = w.pierced;
                c.parked[slot] = w.parked;
            }
            Fate::Despawned => pool.free_slot(slot),
            Fate::Expired => {
                events.expired.push(Expired {
                    id: ProjectileId {
                        index: slot as u32,
                        generation: pool.cols.generation[slot],
                    },
                    position: w.pos,
                    tag: pool.cols.tag[slot],
                });
                pool.free_slot(slot);
            }
        }
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Phase A for one slot: reads the columns, writes nothing shared.
fn sweep_slot(
    c: &Cols,
    slot: usize,
    dt: f32,
    physics: &PhysicsWorld,
    index: &Index3d,
    cfg: &StepConfig,
) -> Work {
    let mut w = Work::empty();
    if !c.alive[slot] {
        return w;
    }
    w.active = true;
    w.pos = c.pos[slot];
    w.vel = c.vel[slot];
    w.life = c.life[slot];
    w.bounces = c.bounces[slot];
    w.pierces = c.pierces[slot];
    w.pierced = c.pierced[slot];
    w.parked = c.parked[slot];
    let tag = c.tag[slot];
    let id = ProjectileId {
        index: slot as u32,
        generation: c.generation[slot],
    };

    w.life -= dt;
    if w.life <= 0.0 {
        w.fate = Fate::Expired;
        return w;
    }
    if w.parked {
        return w;
    }
    let gs = c.gravity_scale[slot];
    let damp = 1.0 / (1.0 + c.drag[slot] * dt);
    for k in 0..3 {
        w.vel[k] += cfg.gravity[k] * gs * dt;
        w.vel[k] *= damp;
    }

    let radius = c.radius[slot];
    let owner = c.owner[slot];
    let owner_entity = owner.and_then(|o| o.as_entity());
    let mut filter = Filter::new(cfg.targets);
    if let Some(o) = owner {
        filter = filter.owned_by(o);
    }
    let response = c.response[slot];
    let mut remaining = dt;
    let mut elapsed = 0.0f32;
    for _ in 0..MAX_HITS_PER_TICK {
        let delta = scale(w.vel, remaining);
        if delta == [0.0; 3] {
            break;
        }
        let mut sweep = Sweep::new(w.pos, delta, radius);
        if let Some(e) = owner_entity {
            sweep = sweep.excluding(e);
        }
        let skip_len = (w.pierces as usize).min(PIERCE_MEMORY);
        let world = physics
            .first_hit(&sweep, &cfg.world)
            .map(|h| (h.t, Handle::entity(h.entity), h.point, h.normal, true) as Contact);
        let target = index
            .cast_skipping(w.pos, delta, radius, &filter, &w.pierced[..skip_len])
            .map(|h| (h.t, h.handle, h.point, h.normal, false) as Contact);
        let Some((t, handle, point, normal, from_world)) = nearer(world, target) else {
            w.pos = add(w.pos, delta);
            break;
        };
        let centre = add(w.pos, scale(delta, t));
        let incoming = w.vel;
        let event_t = (elapsed + t * remaining) / dt;
        let mut outcome = HitOutcome::Despawned;
        let mut keep_going = false;
        match response {
            Response::Despawn => w.pos = centre,
            Response::Reflect {
                restitution,
                max_bounces,
            } => {
                if w.bounces >= max_bounces {
                    w.pos = centre;
                } else {
                    let k = (1.0 + restitution) * dot(w.vel, normal);
                    for (v, n) in w.vel.iter_mut().zip(normal) {
                        *v -= k * n;
                    }
                    w.pos = add(centre, scale(normal, SKIN));
                    w.bounces += 1;
                    outcome = HitOutcome::Reflected;
                    keep_going = true;
                }
            }
            Response::Pierce { max } => {
                w.pos = centre;
                if !from_world && w.pierces < max {
                    w.pierced[w.pierces as usize % PIERCE_MEMORY] = handle;
                    w.pierces += 1;
                    outcome = HitOutcome::Pierced;
                    keep_going = true;
                }
            }
            Response::Stop => {
                w.pos = centre;
                w.vel = [0.0; 3];
                w.parked = true;
                outcome = HitOutcome::Stopped;
            }
        }
        w.hits[w.hit_count] = HitEvent {
            id,
            handle,
            point,
            normal,
            t: event_t,
            velocity: incoming,
            tag,
            outcome,
        };
        w.hit_count += 1;
        if outcome == HitOutcome::Despawned {
            w.fate = Fate::Despawned;
        }
        if !keep_going {
            break;
        }
        elapsed += t * remaining;
        remaining *= 1.0 - t;
    }
    w
}

/// The minimum by `(t, handle)`; on a full tie the world hit wins.
fn nearer(world: Option<Contact>, target: Option<Contact>) -> Option<Contact> {
    match (world, target) {
        (Some(w), Some(t)) => {
            if t.0.total_cmp(&w.0).then(t.1.cmp(&w.1)) == Ordering::Less {
                Some(t)
            } else {
                Some(w)
            }
        }
        (w, t) => w.or(t),
    }
}
