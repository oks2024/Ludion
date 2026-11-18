//! The host-side `perception` debug category: what perception last decided, drawn at extraction.
//!
//! Read-only. It reads [`PerceptionResults`] (the last completed tick), the perception components and
//! the [`StimulusRecord`]s, never runs a sense, and is never hashed. Geometry comes from the current
//! transforms and verdicts from the results, so a staggered perceiver's rays show its carried verdict
//! against the current points.

use ludion_components::{Perceiver, Sense, SightTarget, StimulusRecord};
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_renderer::DebugSegment;

use crate::debug_draw::{PERCEPTION, pen_lines};
use crate::perception::{PerceptionResults, pose, target_points};

const UP: [f32; 3] = [0.0, 1.0, 0.0];
pub(crate) const SEEN: [f32; 4] = [0.1, 0.9, 0.2, 1.0];
const IN_ZONE: [f32; 4] = [1.0, 0.7, 0.1, 1.0];
const IDLE: [f32; 4] = [0.5, 0.5, 0.5, 1.0];
pub(crate) const BLOCKED: [f32; 4] = [1.0, 0.15, 0.15, 1.0];
const NOISE: [f32; 4] = [1.0, 0.85, 0.1, 1.0];
const SIGHT: [f32; 4] = [0.0, 0.85, 0.85, 1.0];
const HEARING: [f32; 4] = [1.0, 0.9, 0.1, 1.0];
const CUSTOM: [f32; 4] = [0.9, 0.2, 0.9, 1.0];
const DIM: f32 = 0.4;
const BLOCKER_CROSS: f32 = 0.25;
const MEMORY_CROSS: f32 = 0.15;

/// Which `perception` parts [`debug_lines`] draws.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PerceptionDebugParts {
    pub(crate) zones: bool,
    pub(crate) rays: bool,
    pub(crate) noises: bool,
    pub(crate) memory: bool,
}

fn enabled_perceivers(world: &World) -> Vec<(EntityId, Perceiver)> {
    let mut perceivers: Vec<(EntityId, Perceiver)> = world
        .entities()
        .filter_map(|id| Some((id, world.get::<Perceiver>(id).ok().flatten()?.clone())))
        .filter(|(_, p)| p.enabled)
        .collect();
    perceivers.sort_by_key(|(id, _)| *id);
    perceivers
}

fn eye_of(world: &World, id: EntityId, local: [f32; 3]) -> Option<[f32; 3]> {
    Some(pose(world, id)?.with_eye(local).origin)
}

fn dim(color: [f32; 4]) -> [f32; 4] {
    [color[0] * DIM, color[1] * DIM, color[2] * DIM, color[3]]
}

/// The `perception` lines for the enabled `parts`: zones, rays, noises, then memory.
pub(crate) fn debug_lines(world: &World, parts: PerceptionDebugParts) -> Vec<DebugSegment> {
    let Some(results) = world.resource::<PerceptionResults>() else {
        return Vec::new();
    };
    let perceivers = enabled_perceivers(world);
    let mut lines = Vec::new();
    if parts.zones {
        lines.extend(zone_lines(world, results, &perceivers));
    }
    if parts.rays {
        lines.extend(ray_lines(world, results, &perceivers));
    }
    if parts.noises {
        lines.extend(noise_lines(world, results, &perceivers));
    }
    if parts.memory {
        lines.extend(memory_lines(world));
    }
    lines
}

fn zone_lines(
    world: &World,
    results: &PerceptionResults,
    perceivers: &[(EntityId, Perceiver)],
) -> Vec<DebugSegment> {
    pen_lines(PERCEPTION, |pen| {
        for (id, perceiver) in perceivers {
            let Some(eye) = eye_of(world, *id, perceiver.eye) else {
                continue;
            };
            let samples = results.sight(*id);
            for (index, zone) in perceiver.sight.iter().enumerate() {
                let Some(facing) = u8::try_from(index)
                    .ok()
                    .and_then(|zone_index| results.zone_facing(*id, zone_index))
                else {
                    continue;
                };
                let in_zone = |s: &&crate::perception::SightSample| s.zone as usize == index;
                let color = if samples.iter().filter(in_zone).any(|s| s.visible) {
                    SEEN
                } else if samples.iter().filter(in_zone).any(|s| s.in_zone) {
                    IN_ZONE
                } else {
                    IDLE
                };
                if zone.half_angle_deg <= 90.0 && zone.vertical_half_angle_deg <= 90.0 {
                    pen.frustum_wedge(
                        eye,
                        facing,
                        UP,
                        zone.half_angle_deg,
                        zone.vertical_half_angle_deg,
                        zone.range,
                        color,
                    );
                } else {
                    pen.wedge(eye, UP, facing, zone.half_angle_deg, zone.range, color);
                }
            }
        }
    })
}

fn ray_lines(
    world: &World,
    results: &PerceptionResults,
    perceivers: &[(EntityId, Perceiver)],
) -> Vec<DebugSegment> {
    pen_lines(PERCEPTION, |pen| {
        for (id, perceiver) in perceivers {
            let Some(eye) = eye_of(world, *id, perceiver.eye) else {
                continue;
            };
            for sample in results.sight(*id).iter().filter(|s| s.in_zone) {
                let Some(target) = world.get::<SightTarget>(sample.target).ok().flatten() else {
                    continue;
                };
                let Some(body) = target_points(world, sample.target, target) else {
                    continue;
                };
                for (index, point) in body.points.iter().enumerate() {
                    let seen = index < 8 && (sample.points_visible >> index) & 1 == 1;
                    pen.line(eye, *point, if seen { SEEN } else { BLOCKED });
                }
                if let Some(blocker) = sample.blocker
                    && let Some(at) = pose(world, blocker)
                {
                    pen.cross(at.origin, BLOCKER_CROSS, BLOCKED);
                }
            }
        }
    })
}

fn noise_lines(
    world: &World,
    results: &PerceptionResults,
    perceivers: &[(EntityId, Perceiver)],
) -> Vec<DebugSegment> {
    pen_lines(PERCEPTION, |pen| {
        for noise in results.noises() {
            pen.circle(noise.at, UP, noise.effective_radius, NOISE);
        }
        for heard in results.heard_all() {
            let Some((_, listener)) = perceivers.iter().find(|(id, _)| *id == heard.listener)
            else {
                continue;
            };
            let ear = listener.hearing.as_ref().map_or(listener.eye, |h| h.ear);
            if let Some(ear) = eye_of(world, heard.listener, ear) {
                pen.line(heard.at, ear, NOISE);
            }
        }
    })
}

fn memory_lines(world: &World) -> Vec<DebugSegment> {
    let mut records: Vec<(EntityId, StimulusRecord)> = world
        .entities()
        .filter_map(|id| Some((id, world.get::<StimulusRecord>(id).ok().flatten()?.clone())))
        .collect();
    records.sort_by_key(|(id, _)| *id);
    pen_lines(PERCEPTION, |pen| {
        for (_, record) in &records {
            let base = match record.sense {
                Sense::Sight => SIGHT,
                Sense::Hearing => HEARING,
                Sense::Custom(_) => CUSTOM,
            };
            let color = if record.sensed_now { base } else { dim(base) };
            pen.cross(record.last_position, MEMORY_CROSS, color);
        }
    })
}

/// The `perception` names there is something to draw for, sorted: each sub-filter whose part has
/// content, then `perception` when any does.
pub(crate) fn debug_present(world: &World) -> Vec<&'static str> {
    use crate::debug_draw::{
        PERCEPTION_MEMORY, PERCEPTION_NOISES, PERCEPTION_RAYS, PERCEPTION_ZONES,
    };
    let Some(results) = world.resource::<PerceptionResults>() else {
        return Vec::new();
    };
    let mut present = Vec::new();
    let perceivers = enabled_perceivers(world);
    if perceivers.iter().any(|(_, p)| !p.sight.is_empty()) {
        present.push(PERCEPTION_ZONES);
    }
    if results.samples().iter().any(|s| s.in_zone) {
        present.push(PERCEPTION_RAYS);
    }
    if !results.noises().is_empty() {
        present.push(PERCEPTION_NOISES);
    }
    if world.entities().any(|id| world.has::<StimulusRecord>(id)) {
        present.push(PERCEPTION_MEMORY);
    }
    if !present.is_empty() {
        present.push(PERCEPTION);
    }
    present
}
