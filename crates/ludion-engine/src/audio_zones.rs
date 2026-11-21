//! Audio zones: the `AudioZone` volumes around the listener resolved into ambient beds, per-bus
//! zone gains and one reverb, at every frame boundary. `ludion docs audio-zones` is the guide.
//!
//! The listener sits in zero, one or two relevant zones: the highest-priority zone `a` that holds it
//! (weight `w_a`, rising across its blend band) and the next one `b` beneath, which fills what `a`
//! leaves. Everything is derived from the listener point and the world, so a world with no zone
//! sends nothing and the state is host-side bookkeeping: never hashed, snapshotted or replayed.

use ludion_components::{AudioZone, AudioZoneShape};
use ludion_core::audio::{
    AUDIO_ZONE_BEDS_MAX, AudioCategory, AudioMessage, Bus, LOW_PASS_OPEN_HZ, LoopId, ReverbParams,
    VoiceSpatial,
};
use ludion_core::collections::StableMap;
use ludion_core::ids::EntityId;
use ludion_core::math;
use ludion_ecs::World;

use crate::audio::{RESYNC_GAIN_RATIO, SentLoop};

/// What the sink has been told about zones: the beds it plays, the zone layer of each bus and the
/// reverb. Reset with the audio queue (the `StopAllLoops` it sends covers the sink).
pub(crate) struct ZoneState {
    beds: StableMap<(EntityId, u8), SentLoop>,
    /// The zone gain sent per bus, indexed by `bus as usize`.
    gains: [f32; Bus::ALL.len()],
    reverb: ReverbParams,
}

impl Default for ZoneState {
    fn default() -> ZoneState {
        ZoneState {
            beds: StableMap::new(),
            gains: [1.0; Bus::ALL.len()],
            reverb: ReverbParams::DRY,
        }
    }
}

/// How far inside `shape` (in the pose `origin`/`rotation`, scale ignored) `point` is: the distance
/// to the nearest face, negative outside. A degenerate shape is never inside.
pub(crate) fn depth(
    shape: &AudioZoneShape,
    origin: [f32; 3],
    rotation: [f32; 4],
    point: [f32; 3],
) -> f32 {
    let delta = [
        point[0] - origin[0],
        point[1] - origin[1],
        point[2] - origin[2],
    ];
    match shape {
        AudioZoneShape::Sphere { radius } => {
            if !(radius.is_finite() && *radius > 0.0) {
                return f32::NEG_INFINITY;
            }
            let length =
                math::sqrt(delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]);
            radius - length
        }
        AudioZoneShape::Box { half_extents } => {
            if !half_extents.iter().all(|h| h.is_finite() && *h > 0.0) {
                return f32::NEG_INFINITY;
            }
            let conjugate = [-rotation[0], -rotation[1], -rotation[2], rotation[3]];
            let local = math::quat_rotate(conjugate, delta);
            (0..3)
                .map(|i| half_extents[i] - local[i].abs())
                .fold(f32::INFINITY, f32::min)
        }
    }
}

/// The weight of a zone given the listener's `depth` inside it: `0` outside, `1` with a hard edge,
/// else the fraction of the `blend_m` band crossed.
pub(crate) fn weight(depth: f32, blend_m: f32) -> f32 {
    if depth.is_nan() || depth <= 0.0 {
        return 0.0;
    }
    if !(blend_m.is_finite() && blend_m > 0.0) {
        return 1.0;
    }
    (depth / blend_m).clamp(0.0, 1.0)
}

/// The effective weight of each zone from `(id, priority, weight)` rows: the zones holding the
/// listener are ranked by priority (descending) then id (ascending); the first keeps its weight,
/// the second takes what it leaves, every other zone gets nothing. Returns `(id, e)` rows with
/// `e > 0`, best first.
pub(crate) fn effective(rows: &[(EntityId, i32, f32)]) -> Vec<(EntityId, f32)> {
    let mut ranked: Vec<_> = rows.iter().filter(|row| row.2 > 0.0).collect();
    ranked.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    let mut out = Vec::new();
    if let Some(a) = ranked.first() {
        out.push((a.0, a.2));
        if let Some(b) = ranked.get(1) {
            let e = (1.0 - a.2) * b.2;
            if e > 0.0 {
                out.push((b.0, e));
            }
        }
    }
    out
}

/// The zone gain on one bus from `(e, gain)` rows: each zone pulls the bus from unity toward its
/// gain by its effective weight; a bad or absent gain is unity.
pub(crate) fn blend_gain(parts: &[(f32, Option<f32>)]) -> f32 {
    parts.iter().fold(1.0, |acc, (e, gain)| {
        let gain = gain.filter(|g| g.is_finite() && *g >= 0.0).unwrap_or(1.0);
        acc * (1.0 + (gain - 1.0) * e)
    })
}

/// The reverb from `(e, params)` rows of the zones that carry one: `wet` sums scaled by each weight,
/// the rest are the weighted mean. No row is dry.
pub(crate) fn blend_reverb(parts: &[(f32, ReverbParams)]) -> ReverbParams {
    let total: f32 = parts.iter().map(|(e, _)| e).sum();
    if total.is_nan() || total <= 0.0 {
        return ReverbParams::DRY;
    }
    let mean = |field: fn(&ReverbParams) -> f32| {
        parts.iter().map(|(e, p)| e * field(p)).sum::<f32>() / total
    };
    ReverbParams {
        decay_s: mean(|p| p.decay_s),
        pre_delay_ms: mean(|p| p.pre_delay_ms),
        damping: mean(|p| p.damping),
        size: mean(|p| p.size),
        wet: parts.iter().map(|(e, p)| e * p.wet).sum(),
    }
    .clamped()
}

fn bed_gain(gain: f32) -> f32 {
    if gain.is_finite() { gain.max(0.0) } else { 1.0 }
}

/// Resolves every `AudioZone` against `listener` and returns the messages that bring the sink to
/// the resolved state, in order: zone gains, reverb, bed stops, bed syncs. Runtime never reports:
/// a bad zone is sanitized (weight 0, beds past [`AUDIO_ZONE_BEDS_MAX`] ignored, params clamped).
pub(crate) fn resolve(
    world: &mut World,
    listener: [f32; 3],
    state: &mut ZoneState,
) -> Vec<AudioMessage> {
    let ids: Vec<EntityId> = world
        .query_ids::<(&AudioZone,)>()
        .map(|(id, _)| id)
        .collect();
    let world = &*world;
    let mut zones: Vec<(EntityId, &AudioZone)> = Vec::new();
    let mut rows = Vec::new();
    for id in ids {
        let Ok(Some(zone)) = world.get::<AudioZone>(id) else {
            continue;
        };
        let Some(pose) = crate::hierarchy::world_transform(world, id) else {
            continue;
        };
        let d = depth(
            &zone.shape,
            pose.trs.translation,
            pose.trs.rotation,
            listener,
        );
        zones.push((id, zone));
        rows.push((id, zone.priority, weight(d, zone.blend_m)));
    }
    let live = effective(&rows);
    let zone_of = |id: EntityId| zones.iter().find(|(z, _)| *z == id).map(|(_, zone)| *zone);

    let mut out = Vec::new();
    for bus in Bus::ALL {
        if bus == Bus::Master {
            continue;
        }
        let parts: Vec<_> = live
            .iter()
            .filter_map(|(id, e)| zone_of(*id).map(|z| (*e, z.bus_gains.gain(bus))))
            .collect();
        let gain = blend_gain(&parts);
        let sent = state.gains[bus as usize];
        if (gain - sent).abs() > sent * RESYNC_GAIN_RATIO || (gain == 1.0 && sent != 1.0) {
            state.gains[bus as usize] = gain;
            out.push(AudioMessage::ZoneGain {
                bus,
                gain,
                fade_frames: 0,
            });
        }
    }

    let parts: Vec<_> = live
        .iter()
        .filter_map(|(id, e)| {
            zone_of(*id)
                .and_then(|z| z.reverb.as_ref())
                .map(|reverb| (*e, reverb.params().clamped()))
        })
        .collect();
    let reverb = blend_reverb(&parts);
    if !reverb.near(&state.reverb) || (reverb == ReverbParams::DRY && state.reverb != reverb) {
        state.reverb = reverb;
        out.push(AudioMessage::ReverbSet {
            params: reverb,
            fade_frames: 0,
        });
    }

    let mut syncs = Vec::new();
    let mut audible = Vec::new();
    for (id, zone) in &zones {
        let Some((_, e)) = live.iter().find(|(z, _)| z == id) else {
            continue;
        };
        for (i, bed) in zone.beds.iter().take(AUDIO_ZONE_BEDS_MAX).enumerate() {
            let key = (*id, i as u8);
            audible.push(key);
            let gain = bed_gain(bed.gain) * e;
            let current = state.beds.get(&key).is_some_and(|sent| {
                sent.is_current(
                    &bed.sound,
                    gain,
                    0.0,
                    LOW_PASS_OPEN_HZ,
                    AudioCategory::Ambient,
                )
            });
            if current {
                continue;
            }
            state.beds.insert(
                key,
                SentLoop {
                    sound: bed.sound.clone(),
                    gain,
                    pan: 0.0,
                    category: AudioCategory::Ambient,
                    low_pass_hz: LOW_PASS_OPEN_HZ,
                },
            );
            syncs.push(AudioMessage::LoopSync {
                id: LoopId::ZoneBed(*id, i as u8),
                sound: bed.sound.clone(),
                gain,
                pan: 0.0,
                category: AudioCategory::Ambient,
                spatial: VoiceSpatial::default(),
            });
        }
    }
    let mut stops: Vec<(EntityId, u8)> = state
        .beds
        .keys()
        .filter(|key| !audible.contains(key))
        .copied()
        .collect();
    stops.sort_unstable();
    for key in stops {
        state.beds.remove(&key);
        out.push(AudioMessage::LoopStop {
            id: LoopId::ZoneBed(key.0, key.1),
        });
    }
    out.extend(syncs);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn id(index: u32) -> EntityId {
        EntityId::new(index, 0)
    }

    #[test]
    fn sphere_depth_is_the_distance_inside_the_surface() {
        let shape = AudioZoneShape::Sphere { radius: 5.0 };
        let d = |p| depth(&shape, [0.0; 3], IDENTITY, p);
        assert_eq!(d([0.0; 3]), 5.0);
        assert_eq!(d([3.0, 0.0, 4.0]), 0.0);
        assert!(d([6.0, 0.0, 0.0]) < 0.0);
    }

    #[test]
    fn box_depth_follows_the_rotation() {
        let shape = AudioZoneShape::Box {
            half_extents: [4.0, 1.0, 2.0],
        };
        assert_eq!(depth(&shape, [0.0; 3], IDENTITY, [3.0, 0.0, 0.0]), 1.0);
        // A quarter turn about Y swaps the box's X and Z reach.
        let quarter = [
            0.0,
            math::sin(std::f32::consts::FRAC_PI_4),
            0.0,
            math::cos(std::f32::consts::FRAC_PI_4),
        ];
        let d = depth(&shape, [0.0; 3], quarter, [0.0, 0.0, 3.0]);
        assert!((d - 1.0).abs() < 1e-4, "{d}");
        let outside = depth(&shape, [10.0, 0.0, 0.0], IDENTITY, [0.0; 3]);
        assert!(outside < 0.0);
    }

    #[test]
    fn a_degenerate_shape_is_never_inside() {
        let bad = AudioZoneShape::Sphere { radius: f32::NAN };
        assert_eq!(weight(depth(&bad, [0.0; 3], IDENTITY, [0.0; 3]), 1.0), 0.0);
        let flat = AudioZoneShape::Box {
            half_extents: [1.0, 0.0, 1.0],
        };
        assert_eq!(weight(depth(&flat, [0.0; 3], IDENTITY, [0.0; 3]), 1.0), 0.0);
    }

    #[test]
    fn weight_is_zero_at_the_edge_and_one_deep() {
        assert_eq!(weight(0.0, 3.0), 0.0);
        assert_eq!(weight(-1.0, 3.0), 0.0);
        assert_eq!(weight(1.5, 3.0), 0.5);
        assert_eq!(weight(3.0, 3.0), 1.0);
        assert_eq!(weight(30.0, 3.0), 1.0);
        // A hard edge, or a bad band, is all or nothing.
        assert_eq!(weight(0.01, 0.0), 1.0);
        assert_eq!(weight(0.01, f32::NAN), 1.0);
    }

    #[test]
    fn two_zones_compose_by_priority_and_what_the_first_leaves() {
        let rows = [(id(1), 0, 1.0), (id(2), 10, 0.25), (id(3), 5, 0.0)];
        assert_eq!(effective(&rows), vec![(id(2), 0.25), (id(1), 0.75)]);
        // A full-weight winner masks everything beneath.
        let rows = [(id(1), 0, 1.0), (id(2), 10, 1.0)];
        assert_eq!(effective(&rows), vec![(id(2), 1.0)]);
        // Equal priority: the lowest id wins.
        let rows = [(id(4), 3, 1.0), (id(2), 3, 1.0)];
        assert_eq!(effective(&rows), vec![(id(2), 1.0)]);
        assert!(effective(&[]).is_empty());
    }

    #[test]
    fn gains_pull_from_unity_by_weight() {
        assert_eq!(blend_gain(&[]), 1.0);
        assert_eq!(blend_gain(&[(1.0, Some(0.5))]), 0.5);
        assert_eq!(blend_gain(&[(0.5, Some(0.5))]), 0.75);
        assert_eq!(blend_gain(&[(1.0, None)]), 1.0);
        assert_eq!(blend_gain(&[(1.0, Some(f32::NAN))]), 1.0);
        assert_eq!(blend_gain(&[(0.5, Some(0.5)), (0.5, Some(0.0))]), 0.375);
    }

    #[test]
    fn reverb_blends_by_weight_and_no_zone_is_dry() {
        assert_eq!(blend_reverb(&[]), ReverbParams::DRY);
        let hall = ReverbParams {
            decay_s: 2.0,
            pre_delay_ms: 20.0,
            damping: 0.4,
            size: 0.6,
            wet: 0.4,
        };
        let room = ReverbParams {
            decay_s: 1.0,
            pre_delay_ms: 0.0,
            damping: 0.6,
            size: 0.2,
            wet: 0.2,
        };
        assert_eq!(blend_reverb(&[(1.0, hall)]), hall);
        let mixed = blend_reverb(&[(0.5, hall), (0.5, room)]);
        assert!((mixed.decay_s - 1.5).abs() < 1e-6);
        assert!((mixed.wet - 0.3).abs() < 1e-6);
        // A half-weighted lone zone scales wet but keeps its character.
        let half = blend_reverb(&[(0.5, hall)]);
        assert!((half.wet - 0.2).abs() < 1e-6);
        assert!((half.decay_s - 2.0).abs() < 1e-6);
    }
}
