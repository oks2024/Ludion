//! The pool: SoA columns, generational ids, a LIFO free list.

use std::fmt;

use ludion_spatial::spatial3d::Handle;

use crate::step::Work;

/// How many recently pierced handles a projectile remembers.
pub const PIERCE_MEMORY: usize = 8;

/// A generational projectile id: a slot and the generation it was spawned in. A stale id (its slot
/// was freed, and maybe reused) is rejected by every accessor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectileId {
    /// Pool slot.
    pub index: u32,
    /// Slot generation at spawn time.
    pub generation: u32,
}

impl ProjectileId {
    /// The id as an index/physics-free [`Handle`] (a pool slot), for putting a projectile in a
    /// `spatial3d` index.
    pub fn handle(self) -> Handle {
        Handle::slot(self.index, self.generation)
    }
}

/// What a projectile does when it hits something. See the [crate docs](crate#hit-responses).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Response {
    /// Freed at the contact.
    Despawn,
    /// Bounces: `v − (1 + restitution)(v·n)n`; the bounce after `max_bounces` despawns.
    Reflect {
        /// Normal-velocity scale in `[0, 1]`.
        restitution: f32,
        /// Bounces allowed before the next hit despawns.
        max_bounces: u16,
    },
    /// Passes through up to `max` index targets; a world hit or the `max + 1`th target despawns.
    Pierce {
        /// Targets pierced before the next hit despawns.
        max: u16,
    },
    /// Parks at the contact.
    Stop,
}

/// Everything [`ProjectilePool::spawn`] needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectileDesc {
    /// World-space start.
    pub position: [f32; 3],
    /// Velocity in m/s.
    pub velocity: [f32; 3],
    /// Scale on [`StepConfig::gravity`](crate::StepConfig::gravity).
    pub gravity_scale: f32,
    /// Linear drag, `>= 0`: `v *= 1 / (1 + drag·dt)` each tick.
    pub drag: f32,
    /// Sweep radius; `0` sweeps a ray.
    pub radius: f32,
    /// Never hit by this projectile (a shooter). Only an entity handle also skips physics.
    pub owner: Option<Handle>,
    /// Seconds to live, `> 0`.
    pub lifetime: f32,
    /// Opaque game value, echoed in events.
    pub tag: u32,
    /// What a hit does.
    pub response: Response,
}

impl ProjectileDesc {
    /// A ray with no gravity or drag, ten seconds to live, that despawns on its first hit.
    pub fn new(position: [f32; 3], velocity: [f32; 3]) -> Self {
        Self {
            position,
            velocity,
            gravity_scale: 0.0,
            drag: 0.0,
            radius: 0.0,
            owner: None,
            lifetime: 10.0,
            tag: 0,
            response: Response::Despawn,
        }
    }
}

/// Why [`ProjectilePool::spawn`] refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// Every slot is live.
    PoolFull,
    /// A field of the [`ProjectileDesc`] is out of range; names the field.
    InvalidDesc(&'static str),
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpawnError::PoolFull => write!(f, "the projectile pool is full"),
            SpawnError::InvalidDesc(field) => {
                write!(
                    f,
                    "invalid projectile `{field}`: out of range or not finite"
                )
            }
        }
    }
}

impl std::error::Error for SpawnError {}

/// The SoA columns. `pub(crate)` so the step can share-borrow them while it writes its scratch.
pub(crate) struct Cols {
    pub(crate) pos: Vec<[f32; 3]>,
    pub(crate) vel: Vec<[f32; 3]>,
    pub(crate) gravity_scale: Vec<f32>,
    pub(crate) drag: Vec<f32>,
    pub(crate) radius: Vec<f32>,
    pub(crate) owner: Vec<Option<Handle>>,
    pub(crate) life: Vec<f32>,
    pub(crate) tag: Vec<u32>,
    pub(crate) response: Vec<Response>,
    pub(crate) bounces: Vec<u16>,
    pub(crate) pierces: Vec<u16>,
    pub(crate) pierced: Vec<[Handle; PIERCE_MEMORY]>,
    pub(crate) parked: Vec<bool>,
    pub(crate) generation: Vec<u32>,
    pub(crate) alive: Vec<bool>,
}

/// A fixed-capacity pool of projectiles. Every column and the step's scratch are allocated up
/// front; nothing grows afterwards.
///
/// # Example
/// ```
/// use ludion_projectiles::{ProjectileDesc, ProjectilePool};
///
/// let mut pool = ProjectilePool::with_capacity(2);
/// let id = pool.spawn(ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0])).unwrap();
/// assert!(pool.despawn(id));
/// assert!(!pool.despawn(id)); // stale
/// let again = pool.spawn(ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0])).unwrap();
/// assert_eq!(again.index, id.index);
/// assert_ne!(again.generation, id.generation);
/// ```
pub struct ProjectilePool {
    pub(crate) cols: Cols,
    pub(crate) scratch: Vec<Work>,
    /// LIFO of freed slots.
    pub(crate) free: Vec<u32>,
    /// Slots ever handed out; the step only visits `0..high_water`.
    pub(crate) high_water: usize,
    pub(crate) live: usize,
}

const NO_HANDLE: Handle = Handle {
    kind: ludion_spatial::spatial3d::HandleKind::Slot,
    index: 0,
    generation: 0,
};

impl ProjectilePool {
    /// A pool of `capacity` slots, every buffer preallocated.
    pub fn with_capacity(capacity: usize) -> Self {
        let cols = Cols {
            pos: vec![[0.0; 3]; capacity],
            vel: vec![[0.0; 3]; capacity],
            gravity_scale: vec![0.0; capacity],
            drag: vec![0.0; capacity],
            radius: vec![0.0; capacity],
            owner: vec![None; capacity],
            life: vec![0.0; capacity],
            tag: vec![0; capacity],
            response: vec![Response::Despawn; capacity],
            bounces: vec![0; capacity],
            pierces: vec![0; capacity],
            pierced: vec![[NO_HANDLE; PIERCE_MEMORY]; capacity],
            parked: vec![false; capacity],
            generation: vec![0; capacity],
            alive: vec![false; capacity],
        };
        Self {
            cols,
            scratch: vec![Work::empty(); capacity],
            free: Vec::with_capacity(capacity),
            high_water: 0,
            live: 0,
        }
    }

    /// Slots ever handed out; a pool cannot shrink below this.
    pub fn high_water(&self) -> usize {
        self.high_water
    }

    /// Changes the slot count to `capacity`, keeping every live shot, id and the free list.
    /// Returns `false` and changes nothing when `capacity` is below [`high_water`](Self::high_water).
    ///
    /// # Example
    /// ```
    /// use ludion_projectiles::{ProjectileDesc, ProjectilePool};
    ///
    /// let mut pool = ProjectilePool::with_capacity(4);
    /// let id = pool.spawn(ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0])).unwrap();
    /// assert!(pool.resize(16));
    /// assert_eq!(pool.capacity(), 16);
    /// assert!(pool.position(id).is_some());
    /// assert!(!pool.resize(0)); // one slot has been handed out
    /// ```
    pub fn resize(&mut self, capacity: usize) -> bool {
        if capacity < self.high_water {
            return false;
        }
        let c = &mut self.cols;
        c.pos.resize(capacity, [0.0; 3]);
        c.vel.resize(capacity, [0.0; 3]);
        c.gravity_scale.resize(capacity, 0.0);
        c.drag.resize(capacity, 0.0);
        c.radius.resize(capacity, 0.0);
        c.owner.resize(capacity, None);
        c.life.resize(capacity, 0.0);
        c.tag.resize(capacity, 0);
        c.response.resize(capacity, Response::Despawn);
        c.bounces.resize(capacity, 0);
        c.pierces.resize(capacity, 0);
        c.pierced.resize(capacity, [NO_HANDLE; PIERCE_MEMORY]);
        c.parked.resize(capacity, false);
        c.generation.resize(capacity, 0);
        c.alive.resize(capacity, false);
        self.scratch.resize(capacity, Work::empty());
        self.free.reserve(capacity.saturating_sub(self.free.len()));
        true
    }

    /// Slot count.
    pub fn capacity(&self) -> usize {
        self.cols.alive.len()
    }

    /// Live projectiles.
    pub fn len(&self) -> usize {
        self.live
    }

    /// Whether no projectile is live.
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Adds a projectile in the most recently freed slot, else the next never-used one. The id
    /// sequence is a pure function of the spawn/despawn history.
    ///
    /// # Example
    /// ```
    /// use ludion_projectiles::{ProjectileDesc, ProjectilePool, SpawnError};
    ///
    /// let mut pool = ProjectilePool::with_capacity(1);
    /// let mut desc = ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0]);
    /// desc.lifetime = 0.0;
    /// assert_eq!(pool.spawn(desc), Err(SpawnError::InvalidDesc("lifetime")));
    /// desc.lifetime = 2.0;
    /// let id = pool.spawn(desc).unwrap();
    /// assert!(pool.contains(id));
    /// assert_eq!(pool.spawn(desc), Err(SpawnError::PoolFull));
    /// ```
    pub fn spawn(&mut self, desc: ProjectileDesc) -> Result<ProjectileId, SpawnError> {
        validate(&desc)?;
        let slot = match self.free.pop() {
            Some(slot) => slot as usize,
            None if self.high_water < self.capacity() => {
                self.high_water += 1;
                self.high_water - 1
            }
            None => return Err(SpawnError::PoolFull),
        };
        let c = &mut self.cols;
        c.pos[slot] = desc.position;
        c.vel[slot] = desc.velocity;
        c.gravity_scale[slot] = desc.gravity_scale;
        c.drag[slot] = desc.drag;
        c.radius[slot] = desc.radius;
        c.owner[slot] = desc.owner;
        c.life[slot] = desc.lifetime;
        c.tag[slot] = desc.tag;
        c.response[slot] = desc.response;
        c.bounces[slot] = 0;
        c.pierces[slot] = 0;
        c.pierced[slot] = [NO_HANDLE; PIERCE_MEMORY];
        c.parked[slot] = false;
        c.alive[slot] = true;
        self.live += 1;
        Ok(ProjectileId {
            index: slot as u32,
            generation: c.generation[slot],
        })
    }

    /// Frees `id`'s slot; `false` for a stale or unknown id.
    pub fn despawn(&mut self, id: ProjectileId) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.free_slot(id.index as usize);
        true
    }

    pub(crate) fn free_slot(&mut self, slot: usize) {
        self.cols.alive[slot] = false;
        self.cols.generation[slot] = self.cols.generation[slot].wrapping_add(1);
        self.free.push(slot as u32);
        self.live -= 1;
    }

    /// Whether `id` names a live projectile.
    pub fn contains(&self, id: ProjectileId) -> bool {
        self.slot(id).is_some()
    }

    fn slot(&self, id: ProjectileId) -> Option<usize> {
        let i = id.index as usize;
        (i < self.high_water && self.cols.alive[i] && self.cols.generation[i] == id.generation)
            .then_some(i)
    }

    /// Live ids, ascending by slot.
    pub fn ids(&self) -> impl Iterator<Item = ProjectileId> + '_ {
        (0..self.high_water)
            .filter(|&i| self.cols.alive[i])
            .map(|i| ProjectileId {
                index: i as u32,
                generation: self.cols.generation[i],
            })
    }

    /// Current position.
    pub fn position(&self, id: ProjectileId) -> Option<[f32; 3]> {
        self.slot(id).map(|i| self.cols.pos[i])
    }

    /// Current velocity.
    pub fn velocity(&self, id: ProjectileId) -> Option<[f32; 3]> {
        self.slot(id).map(|i| self.cols.vel[i])
    }

    /// Replaces the velocity (the homing hook); `false` for a stale id or a non-finite velocity.
    /// A parked projectile stays parked.
    pub fn set_velocity(&mut self, id: ProjectileId, velocity: [f32; 3]) -> bool {
        match self.slot(id) {
            Some(i) if velocity.iter().all(|c| c.is_finite()) => {
                self.cols.vel[i] = velocity;
                true
            }
            _ => false,
        }
    }

    /// The opaque tag.
    pub fn tag(&self, id: ProjectileId) -> Option<u32> {
        self.slot(id).map(|i| self.cols.tag[i])
    }

    /// The owner.
    pub fn owner(&self, id: ProjectileId) -> Option<Option<Handle>> {
        self.slot(id).map(|i| self.cols.owner[i])
    }

    /// The hit response.
    pub fn response(&self, id: ProjectileId) -> Option<Response> {
        self.slot(id).map(|i| self.cols.response[i])
    }

    /// Whether the projectile stopped (a [`Response::Stop`] hit).
    pub fn is_parked(&self, id: ProjectileId) -> Option<bool> {
        self.slot(id).map(|i| self.cols.parked[i])
    }

    /// Bounces so far.
    pub fn bounces(&self, id: ProjectileId) -> Option<u16> {
        self.slot(id).map(|i| self.cols.bounces[i])
    }

    /// Targets pierced so far.
    pub fn pierces(&self, id: ProjectileId) -> Option<u16> {
        self.slot(id).map(|i| self.cols.pierces[i])
    }
}

fn validate(d: &ProjectileDesc) -> Result<(), SpawnError> {
    let finite3 = |v: &[f32; 3]| v.iter().all(|c| c.is_finite());
    let bad = |field| Err(SpawnError::InvalidDesc(field));
    if !finite3(&d.position) {
        return bad("position");
    }
    if !finite3(&d.velocity) {
        return bad("velocity");
    }
    if !d.gravity_scale.is_finite() {
        return bad("gravity_scale");
    }
    if !(d.drag.is_finite() && d.drag >= 0.0) {
        return bad("drag");
    }
    if !(d.radius.is_finite() && d.radius >= 0.0) {
        return bad("radius");
    }
    if !(d.lifetime.is_finite() && d.lifetime > 0.0) {
        return bad("lifetime");
    }
    if let Response::Reflect { restitution, .. } = d.response
        && !(0.0..=1.0).contains(&restitution)
    {
        return bad("response.restitution");
    }
    Ok(())
}
