//! Geometry occlusion of positional audio (`EngineBuilder::audio_occlusion`).
//!
//! Rays run from the listener to each positional source through [`PhysicsWorld::ray_cast`]; the
//! blockers they cross become a gain multiplier and a low-pass cutoff written into the messages the
//! engine already sends. Host-side bookkeeping over an out-of-sim stream: never hashed, snapshotted,
//! recorded or sent over the wire.

use ludion_core::audio::{HIGH_PASS_OPEN_HZ, LOW_PASS_OPEN_HZ, VoiceSpatial};
use ludion_core::collections::StableMap;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::World;
use ludion_physics::{PhysicsWorld, QueryFilter};

use crate::audio::{ListenerPose, VoiceId};
use ludion_core::codes::code;

/// How geometry muffles and quiets positional sounds: the game authors it once on the builder
/// ([`EngineBuilder::audio_occlusion`](crate::EngineBuilder::audio_occlusion)). Off unless set, so a
/// game that never calls it keeps the same message stream.
///
/// Every positional source (an `AudioEmitter` loop, `play_at` / `push_at`, `play_on` / `push_on`)
/// is probed with `rays` rays from the listener to the source (`3` adds two side rays `spread_m`
/// either side of it, `5` also two above and below). A ray's loss is `per_hit_db` per distinct
/// blocker, floored at `max_db`; the source's gain is the mean loss of its rays and its low-pass
/// cutoff slides from open toward `occluded_low_pass_hz` by the share of blocked rays. Global sounds
/// (`play`, `loop_start`, music) are never occluded; a blocker that shares a hierarchy root with the
/// source or the listener is ignored.
///
/// ```
/// use ludion_engine::AudioOcclusion;
///
/// let default = AudioOcclusion::default();
/// assert_eq!(default.rays, 3);
/// // Walls only, a stronger muffle for a heavier building.
/// let heavy = AudioOcclusion { per_hit_db: -9.0, max_db: -24.0, ..default };
/// assert!(heavy.per_hit_db > heavy.max_db);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct AudioOcclusion {
    /// Which colliders occlude; the default is static colliders only.
    pub filter: QueryFilter,
    /// Rays per source: `1`, `3` or `5`.
    pub rays: u8,
    /// The side-ray offset at the source, in metres.
    pub spread_m: f32,
    /// The loss, in dB, per distinct blocker on one ray (`0.0` or below).
    pub per_hit_db: f32,
    /// The floor of one ray's loss, in dB (`per_hit_db` or below).
    pub max_db: f32,
    /// The low-pass cutoff, in Hz, when every ray is blocked.
    pub occluded_low_pass_hz: f32,
    /// Sim ticks between two casts at one source (at least `1`).
    pub interval_ticks: u32,
    /// The ray budget per tick, shared round-robin by the due sources (at least `rays`).
    pub max_rays_per_tick: u32,
    /// Sim ticks a changed result takes to be fully applied; `0` jumps.
    pub smoothing_ticks: u32,
}

impl Default for AudioOcclusion {
    fn default() -> AudioOcclusion {
        AudioOcclusion {
            filter: QueryFilter::new().only_fixed(),
            rays: 3,
            spread_m: 0.6,
            per_hit_db: -6.0,
            max_db: -18.0,
            occluded_low_pass_hz: 1200.0,
            interval_ticks: 2,
            max_rays_per_tick: 96,
            smoothing_ticks: 8,
        }
    }
}

/// Refuses a config that cannot run (`L5101`), naming the bad field.
pub(crate) fn validate(config: &AudioOcclusion) -> Result<(), Diagnostic> {
    let bad_db = |db: f32| !db.is_finite() || db > 0.0;
    let problem = if !matches!(config.rays, 1 | 3 | 5) {
        format!("`rays` is {} but must be 1, 3 or 5", config.rays)
    } else if !config.spread_m.is_finite() || config.spread_m < 0.0 {
        format!(
            "`spread_m` is {} but must be finite and at least 0",
            config.spread_m
        )
    } else if bad_db(config.per_hit_db) {
        format!(
            "`per_hit_db` is {} but must be finite and at most 0",
            config.per_hit_db
        )
    } else if bad_db(config.max_db) {
        format!(
            "`max_db` is {} but must be finite and at most 0",
            config.max_db
        )
    } else if config.max_db > config.per_hit_db {
        format!(
            "`max_db` ({}) is above `per_hit_db` ({}), so no blocker could reach the floor",
            config.max_db, config.per_hit_db
        )
    } else if !(config.occluded_low_pass_hz > HIGH_PASS_OPEN_HZ
        && config.occluded_low_pass_hz < LOW_PASS_OPEN_HZ)
    {
        format!(
            "`occluded_low_pass_hz` is {} but must lie between {HIGH_PASS_OPEN_HZ} and \
             {LOW_PASS_OPEN_HZ}",
            config.occluded_low_pass_hz
        )
    } else if config.interval_ticks == 0 {
        "`interval_ticks` is 0 but must be at least 1".to_string()
    } else if config.max_rays_per_tick < u32::from(config.rays) {
        format!(
            "`max_rays_per_tick` ({}) is below `rays` ({}), so no source could be cast",
            config.max_rays_per_tick, config.rays
        )
    } else {
        return Ok(());
    };
    Err(Diagnostic::new(
        code::AUDIO_OCCLUSION_INVALID,
        format!("`audio_occlusion`: {problem}"),
    )
    .with_help(
        "start from `AudioOcclusion::default()` and change one field at a time: `rays` 1, 3 or 5, \
         `per_hit_db` and `max_db` finite and at most 0 with `max_db <= per_hit_db`, \
         `occluded_low_pass_hz` between 20 and 20000, `interval_ticks` at least 1, \
         `max_rays_per_tick` at least `rays`",
    ))
}

/// What the rays at one source measured: the mean loss in dB and the share of blocked rays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Measure {
    pub(crate) gain_db: f32,
    pub(crate) blocked: f32,
}

impl Measure {
    /// Nothing between the listener and the source.
    pub(crate) const CLEAR: Measure = Measure {
        gain_db: 0.0,
        blocked: 0.0,
    };

    /// `gain` and `spatial` with this result folded in: the gain multiplied and the low-pass cutoff
    /// the lower of the authored one and the occluded one.
    pub(crate) fn apply(
        self,
        config: &AudioOcclusion,
        gain: f32,
        mut spatial: VoiceSpatial,
    ) -> (f32, VoiceSpatial) {
        spatial.filter.low_pass_hz = spatial.filter.low_pass_hz.min(self.cutoff(config));
        (gain * self.gain(), spatial)
    }

    /// The gain multiplier.
    pub(crate) fn gain(self) -> f32 {
        if self.gain_db == 0.0 {
            1.0
        } else {
            crate::audio::db_to_gain(self.gain_db)
        }
    }

    /// The low-pass cutoff: exactly open (bypass) when no ray is blocked, else a log2 lerp from open
    /// to `config.occluded_low_pass_hz` by the blocked share.
    pub(crate) fn cutoff(self, config: &AudioOcclusion) -> f32 {
        if self.blocked <= 0.0 {
            return LOW_PASS_OPEN_HZ;
        }
        let open = ludion_core::math::log2(LOW_PASS_OPEN_HZ);
        let shut = ludion_core::math::log2(config.occluded_low_pass_hz);
        ludion_core::math::exp2(open + (shut - open) * self.blocked.min(1.0))
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn offset(base: [f32; 3], axis: [f32; 3], by: f32) -> [f32; 3] {
    [
        base[0] + axis[0] * by,
        base[1] + axis[1] * by,
        base[2] + axis[2] * by,
    ]
}

/// Casts `config.rays` rays from `listener` to `source` and measures what they cross. A hit whose
/// hierarchy root is one of `skip` (the source's and the listener's own root) is dropped. The
/// `world` is the one `physics` lives in, read for those roots.
pub(crate) fn measure(
    world: &World,
    physics: &PhysicsWorld,
    config: &AudioOcclusion,
    listener: [f32; 3],
    source: [f32; 3],
    skip: [Option<EntityId>; 2],
) -> Result<Measure, ludion_physics::PhysicsError> {
    let d = [
        source[0] - listener[0],
        source[1] - listener[1],
        source[2] - listener[2],
    ];
    let len = ludion_core::math::sqrt(dot(d, d));
    if !len.is_finite() || len < 1e-3 {
        return Ok(Measure::CLEAR);
    }
    let dir = [d[0] / len, d[1] / len, d[2] / len];
    let mut right = cross(d, [0.0, 1.0, 0.0]);
    let right_len = ludion_core::math::sqrt(dot(right, right));
    right = if right_len <= 1e-4 * len {
        [1.0, 0.0, 0.0]
    } else {
        [
            right[0] / right_len,
            right[1] / right_len,
            right[2] / right_len,
        ]
    };
    let up = cross(right, dir);
    let spread = config.spread_m;
    let ends = [
        source,
        offset(source, right, spread),
        offset(source, right, -spread),
        offset(source, up, spread),
        offset(source, up, -spread),
    ];
    let rays = usize::from(config.rays).min(ends.len());
    let mut total_db = 0.0f32;
    let mut blocked = 0usize;
    for end in &ends[..rays] {
        let v = [
            end[0] - listener[0],
            end[1] - listener[1],
            end[2] - listener[2],
        ];
        let distance = ludion_core::math::sqrt(dot(v, v));
        if !distance.is_finite() || distance < 1e-3 {
            continue;
        }
        let hits = physics.ray_cast(listener, v, distance, &config.filter)?;
        let mut seen: Vec<EntityId> = Vec::new();
        for hit in hits {
            let root = crate::hierarchy::hierarchy_root(world, hit.entity);
            if skip.contains(&Some(root)) || seen.contains(&hit.entity) {
                continue;
            }
            seen.push(hit.entity);
        }
        if !seen.is_empty() {
            blocked += 1;
            total_db += (seen.len() as f32 * config.per_hit_db).max(config.max_db);
        }
    }
    Ok(Measure {
        gain_db: total_db / rays as f32,
        blocked: blocked as f32 / rays as f32,
    })
}

/// The identity of an occluded source: an emitter's entity or an attached voice. Emitters sort
/// before voices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum OccluderKey {
    Entity(EntityId),
    Voice(VoiceId),
}

/// One source's last result and the slew toward it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Occluded {
    last_cast: FrameIndex,
    target_db: f32,
    target_blocked: f32,
    now_db: f32,
    now_blocked: f32,
    step_db: f32,
    step_blocked: f32,
    ticks_left: u32,
}

impl Occluded {
    /// A source's first result, applied at once.
    fn first(frame: FrameIndex, result: Measure) -> Occluded {
        Occluded {
            last_cast: frame,
            target_db: result.gain_db,
            target_blocked: result.blocked,
            now_db: result.gain_db,
            now_blocked: result.blocked,
            step_db: 0.0,
            step_blocked: 0.0,
            ticks_left: 0,
        }
    }

    /// A new result to slew toward over `smoothing_ticks` steps (`0` jumps).
    fn retarget(&mut self, frame: FrameIndex, result: Measure, smoothing_ticks: u32) {
        self.last_cast = frame;
        self.target_db = result.gain_db;
        self.target_blocked = result.blocked;
        if smoothing_ticks == 0 {
            self.now_db = result.gain_db;
            self.now_blocked = result.blocked;
            self.ticks_left = 0;
            return;
        }
        let ticks = smoothing_ticks as f32;
        self.step_db = (self.target_db - self.now_db) / ticks;
        self.step_blocked = (self.target_blocked - self.now_blocked) / ticks;
        self.ticks_left = smoothing_ticks;
    }

    /// One tick of slew; the last step lands exactly on the target.
    fn step(&mut self) {
        if self.ticks_left == 0 {
            return;
        }
        self.ticks_left -= 1;
        if self.ticks_left == 0 {
            self.now_db = self.target_db;
            self.now_blocked = self.target_blocked;
        } else {
            self.now_db += self.step_db;
            self.now_blocked += self.step_blocked;
        }
    }

    fn applied(&self) -> Measure {
        Measure {
            gain_db: self.now_db,
            blocked: self.now_blocked,
        }
    }
}

/// The engine's occlusion bookkeeping: each source's result, the round-robin cursor and the tick's
/// ray budget. Cleared by `install_audio_queue`, so a rebuild resets it.
#[derive(Default)]
pub(crate) struct OcclusionState {
    sources: StableMap<OccluderKey, Occluded>,
    cursor: Option<OccluderKey>,
    budget_left: u32,
    /// The most rays any one tick spent; read by tests through `Engine::audio_occlusion_peak_rays`.
    pub(crate) peak_rays: u32,
    /// Latched once the physics query refused the filter: sources then read clear.
    pub(crate) reported: bool,
    /// The message of that refusal until the engine reports it.
    pub(crate) rejected: Option<String>,
}

impl OcclusionState {
    /// Starts a tick with the full ray budget.
    pub(crate) fn begin_tick(&mut self, config: &AudioOcclusion) {
        self.budget_left = config.max_rays_per_tick;
    }

    /// The applied (smoothed) result of `key`, or clear when it has none.
    pub(crate) fn applied(&self, key: &OccluderKey) -> Measure {
        self.sources
            .get(key)
            .map_or(Measure::CLEAR, Occluded::applied)
    }

    /// Stores a fresh `result` for `key`: a first result is applied unsmoothed, a later one slews.
    pub(crate) fn record(
        &mut self,
        config: &AudioOcclusion,
        key: OccluderKey,
        frame: FrameIndex,
        result: Measure,
    ) {
        match self.sources.get_mut(&key) {
            Some(source) => source.retarget(frame, result, config.smoothing_ticks),
            None => {
                self.sources.insert(key, Occluded::first(frame, result));
            }
        }
    }

    /// Forgets every key not in `keys` (ascending).
    pub(crate) fn retain(&mut self, keys: &[OccluderKey]) {
        let gone: Vec<OccluderKey> = self
            .sources
            .keys()
            .filter(|key| keys.binary_search(key).is_err())
            .copied()
            .collect();
        for key in gone {
            self.sources.remove(&key);
        }
    }

    /// One tick of slew for every source.
    pub(crate) fn step_all(&mut self) {
        for source in self.sources.iter_mut().map(|(_, source)| source) {
            source.step();
        }
    }

    /// The keys among `candidates` (ascending) due for a cast at `frame`: no result yet, or the last
    /// one at least `interval_ticks` old.
    pub(crate) fn due(
        &self,
        config: &AudioOcclusion,
        frame: FrameIndex,
        candidates: &[OccluderKey],
    ) -> Vec<OccluderKey> {
        candidates
            .iter()
            .filter(|key| {
                self.sources.get(key).is_none_or(|source| {
                    frame.0.saturating_sub(source.last_cast.0) >= u64::from(config.interval_ticks)
                })
            })
            .copied()
            .collect()
    }

    /// Chooses which of `due` (ascending) to cast now: walking from just after the cursor and
    /// wrapping, while the budget covers `rays` more. Spends the budget and moves the cursor to the
    /// last key chosen.
    pub(crate) fn select(&mut self, due: &[OccluderKey], rays: u32) -> Vec<OccluderKey> {
        let start = match self.cursor {
            Some(cursor) => due.partition_point(|key| *key <= cursor),
            None => 0,
        };
        let mut chosen = Vec::new();
        for key in due[start..].iter().chain(&due[..start]) {
            if self.budget_left < rays {
                break;
            }
            self.budget_left -= rays;
            chosen.push(*key);
        }
        if let Some(last) = chosen.last() {
            self.cursor = Some(*last);
        }
        chosen
    }

    /// One cast now that spends the budget; a short budget reads as clear.
    pub(crate) fn cast(
        &mut self,
        world: &World,
        config: &AudioOcclusion,
        listener: &ListenerPose,
        source_entity: Option<EntityId>,
        at: [f32; 3],
    ) -> Measure {
        let rays = u32::from(config.rays);
        if self.budget_left < rays {
            return Measure::CLEAR;
        }
        self.budget_left -= rays;
        self.probe(world, config, listener, source_entity, at)
    }

    /// One cast whose budget [`select`](OcclusionState::select) already spent; no physics or a
    /// refused query read as clear. The first refusal is kept in `rejected` for the engine to report.
    pub(crate) fn probe(
        &mut self,
        world: &World,
        config: &AudioOcclusion,
        listener: &ListenerPose,
        source_entity: Option<EntityId>,
        at: [f32; 3],
    ) -> Measure {
        if self.reported {
            return Measure::CLEAR;
        }
        let Some(physics) = world.resource::<PhysicsWorld>() else {
            return Measure::CLEAR;
        };
        let root = |entity: Option<EntityId>| {
            entity.map(|entity| crate::hierarchy::hierarchy_root(world, entity))
        };
        let skip = [root(source_entity), root(listener.entity)];
        match measure(world, physics, config, listener.position, at, skip) {
            Ok(result) => result,
            Err(error) => {
                self.reported = true;
                self.rejected = Some(format!(
                    "the audio occlusion query was refused ({error}); positional sounds are not \
                     occluded: declare the layer in `physics.layers` or fix the \
                     `AudioOcclusion::filter`"
                ));
                Measure::CLEAR
            }
        }
    }

    /// Closes the tick's accounting: the peak ray spend of any tick so far.
    pub(crate) fn end_tick(&mut self, config: &AudioOcclusion) {
        self.peak_rays = self
            .peak_rays
            .max(config.max_rays_per_tick - self.budget_left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u32) -> OccluderKey {
        OccluderKey::Entity(EntityId::new(n, 0))
    }

    fn config() -> AudioOcclusion {
        AudioOcclusion::default()
    }

    #[test]
    fn mapping_per_hit_floor_and_cutoff() {
        let config = config();
        // One wall on every ray: the mean is the per-hit loss and the cutoff the occluded one.
        let walled = Measure {
            gain_db: config.per_hit_db,
            blocked: 1.0,
        };
        assert!((walled.cutoff(&config) - config.occluded_low_pass_hz).abs() < 1.0);
        assert!((walled.gain() - crate::audio::db_to_gain(-6.0)).abs() < 1e-6);
        // Clear is exactly bypass and unity.
        assert_eq!(Measure::CLEAR.cutoff(&config), LOW_PASS_OPEN_HZ);
        assert_eq!(Measure::CLEAR.gain(), 1.0);
        // A third of the rays blocked sits a third of the way along the log2 sweep.
        let partial = Measure {
            gain_db: -2.0,
            blocked: 1.0 / 3.0,
        };
        let cutoff = partial.cutoff(&config);
        assert!(cutoff < LOW_PASS_OPEN_HZ && cutoff > config.occluded_low_pass_hz);
    }

    #[test]
    fn validate_refuses_each_bad_field() {
        assert!(validate(&config()).is_ok());
        let bad = [
            AudioOcclusion {
                rays: 2,
                ..config()
            },
            AudioOcclusion {
                spread_m: -1.0,
                ..config()
            },
            AudioOcclusion {
                spread_m: f32::NAN,
                ..config()
            },
            AudioOcclusion {
                per_hit_db: 1.0,
                ..config()
            },
            AudioOcclusion {
                max_db: f32::INFINITY,
                ..config()
            },
            AudioOcclusion {
                max_db: -3.0,
                per_hit_db: -6.0,
                ..config()
            },
            AudioOcclusion {
                occluded_low_pass_hz: 20_000.0,
                ..config()
            },
            AudioOcclusion {
                occluded_low_pass_hz: 10.0,
                ..config()
            },
            AudioOcclusion {
                interval_ticks: 0,
                ..config()
            },
            AudioOcclusion {
                max_rays_per_tick: 2,
                ..config()
            },
        ];
        for config in bad {
            let error = validate(&config).unwrap_err();
            assert_eq!(error.code, code::AUDIO_OCCLUSION_INVALID, "{config:?}");
        }
    }

    #[test]
    fn smoothing_lands_in_exactly_smoothing_ticks() {
        let config = config();
        let mut state = OcclusionState::default();
        state.record(&config, key(1), FrameIndex(0), Measure::CLEAR);
        let target = Measure {
            gain_db: -12.0,
            blocked: 1.0,
        };
        state.record(&config, key(1), FrameIndex(2), target);
        for tick in 1..config.smoothing_ticks {
            state.step_all();
            assert_ne!(state.applied(&key(1)), target, "tick {tick}");
        }
        state.step_all();
        assert_eq!(state.applied(&key(1)), target);
        // A first result is not slewed, and a zero-tick config jumps.
        state.record(&config, key(2), FrameIndex(0), target);
        assert_eq!(state.applied(&key(2)), target);
        let jump = AudioOcclusion {
            smoothing_ticks: 0,
            ..config
        };
        state.record(&jump, key(2), FrameIndex(1), Measure::CLEAR);
        assert_eq!(state.applied(&key(2)), Measure::CLEAR);
    }

    #[test]
    fn round_robin_visits_every_key_in_order_and_wraps() {
        let config = config();
        let mut state = OcclusionState::default();
        let due: Vec<OccluderKey> = (0..10).map(key).collect();
        let rays = u32::from(config.rays);
        let mut visited = Vec::new();
        for _ in 0..4 {
            state.budget_left = 3 * rays;
            visited.extend(state.select(&due, rays));
        }
        let expected: Vec<OccluderKey> = (0..12).map(|n| key(n % 10)).collect();
        assert_eq!(visited, expected);
        // A budget under one source casts nothing and keeps the cursor.
        state.budget_left = rays - 1;
        assert!(state.select(&due, rays).is_empty());
    }

    #[test]
    fn due_waits_for_the_interval() {
        let config = config();
        let mut state = OcclusionState::default();
        let keys = [key(1), key(2)];
        assert_eq!(state.due(&config, FrameIndex(5), &keys), keys);
        state.record(&config, key(1), FrameIndex(5), Measure::CLEAR);
        assert_eq!(state.due(&config, FrameIndex(6), &keys), [key(2)]);
        assert_eq!(state.due(&config, FrameIndex(7), &keys), keys);
    }
}
