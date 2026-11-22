//! [`sequence_update`]: the one system that plays, skips, cancels and ends sequences.
//!
//! First, every `SequenceBound` whose player is no longer a live `holds_gameplay` player is released
//! (or re-pointed to another holding player), shifting the entity's gameplay stamps back by the lead.
//!
//! Then, per player, in ascending [`EntityId`] (a later player's write to the same component wins):
//!
//! 1. a vanished document or a cancel ends it `Cancelled`;
//! 2. a pending player stamps its start from its clock, and a `holds_gameplay` one binds its role
//!    entities and their `Parent` descendants to their own clock;
//! 3. a skip applies the end forms and ends it `Skipped` (waits do not stop it);
//! 4. a pending wait either clears (the line ended, or `release` was called) and the playhead
//!    resumes from the wait's `at`, or the player stores itself and applies nothing;
//! 5. otherwise the playhead is `now - started - waited`, stopped at the earliest due wait key, and
//!    every key with `applied_through < at <= playhead` applies, by track (camera, transform,
//!    animation, ui, audio, dialogue, event), then `at`, then document order; a wait key parks the
//!    playhead; after the transform keys, every live transform span is written at the playhead;
//! 6. the tick the playhead reaches `length_ticks` with no wait pending it ends `Completed`;
//! 7. a player that stays live with `holds_gameplay` makes the system call `GameClock::hold` once.

use std::collections::BTreeMap;

use ludion_camera::{CameraRig, snap_rig};
use ludion_components::ui::{UiClock, UiTween, UiTweenRepeat};
use ludion_components::{Animation, Transform, crossfade};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::math::{
    Trs, euler_xyz_degrees_to_quat, quat_mul, quat_rotate, quat_to_euler_xyz_degrees,
};
use ludion_core::time::GameClock;
use ludion_ecs::{SystemCtx, World};

use super::bound::{bind, entity_clock, release_orphans};
use super::docs::{
    CameraKey, DialogueWait, EventWait, OnSkip, SequenceClock, SequenceDoc, TransformKey,
    TransformTo,
};
use super::events::{SequenceEnd, SequenceEvent, SequenceEvents, SequenceTrack};
use super::{
    PendingWait, PlayerState, SequenceBinding, SequenceHandle, SequencePlayer, SpanMotion,
    TransformSpan, WaitKind, find_doc,
};
use crate::diagnostics::report;
use crate::dialogue::{DialogueLine, LineHandle};

/// One bound role as the system sees it.
struct Role {
    binding: EntityId,
    name: String,
    entity: EntityId,
    saved_rig: Option<ludion_camera::RigConfig>,
}

/// Plays every [`SequencePlayer`] one tick and rewrites [`SequenceEvents`].
///
/// Schedule it before `dialogue_update` (a dialogue key starts the same tick) and before any system
/// that reads [`SequenceEvents`]. A no-op for a game that did not opt in.
///
/// ```
/// use ludion_engine::{Engine, SequenceEvents, sequence_update};
/// use ludion_protocol::{Command, command};
///
/// let mut engine = Engine::builder()
///     .sequences()
///     .system("sequence_update", sequence_update)
///     .build()
///     .unwrap();
/// engine
///     .execute(Command::SimStep(command::SimStepParams { ticks: 1, hash: false }))
///     .unwrap();
/// assert_eq!(engine.world().resource::<SequenceEvents>().unwrap().iter().count(), 0);
/// ```
pub fn sequence_update(world: &mut World, ctx: &SystemCtx) {
    if world.resource::<SequenceEvents>().is_none() {
        return;
    }
    release_orphans(world, ctx);
    let mut players: Vec<(EntityId, SequencePlayer)> = world
        .query_ids::<(&SequencePlayer,)>()
        .map(|(id, (player,))| (id, player.clone()))
        .collect();
    players.sort_by_key(|(id, _)| *id);
    let mut bindings: BTreeMap<EntityId, Vec<Role>> = BTreeMap::new();
    if !players.is_empty() {
        let mut found: Vec<(EntityId, SequenceBinding)> = world
            .query_ids::<(&SequenceBinding,)>()
            .map(|(id, (binding,))| (id, binding.clone()))
            .collect();
        found.sort_by_key(|(id, _)| *id);
        for (id, binding) in found {
            bindings.entry(binding.player).or_default().push(Role {
                binding: id,
                name: binding.role,
                entity: binding.entity,
                saved_rig: binding.saved_rig,
            });
        }
    }
    let mut events = Vec::new();
    let mut hold = false;
    for (id, player) in players {
        let roles = bindings.remove(&id).unwrap_or_default();
        hold |= Beat {
            world: &mut *world,
            ctx,
            pid: id,
            player,
            roles,
            events: &mut events,
        }
        .step();
    }
    if let Some(out) = world.resource_mut::<SequenceEvents>() {
        out.events = events;
    }
    if hold && let Some(clock) = world.resource_mut::<GameClock>() {
        clock.hold();
    }
}

struct Beat<'a> {
    world: &'a mut World,
    ctx: &'a SystemCtx,
    pid: EntityId,
    player: SequencePlayer,
    roles: Vec<Role>,
    events: &'a mut Vec<SequenceEvent>,
}

impl Beat<'_> {
    fn handle(&self) -> SequenceHandle {
        SequenceHandle(self.pid)
    }

    /// Plays the player one tick. `true` when it stays live with `holds_gameplay`, so the caller
    /// holds the game clock.
    fn step(mut self) -> bool {
        let Some(doc) = find_doc(self.world, &self.player.sequence) else {
            self.degrade(
                "document",
                format!(
                    "sequence `{}` was unloaded while playing; it is cancelled",
                    self.player.sequence
                ),
            );
            self.finish(SequenceEnd::Cancelled);
            return false;
        };
        if self.player.cancel {
            self.finish(SequenceEnd::Cancelled);
            return false;
        }
        let now = match self.player.clock {
            SequenceClock::Game => self.ctx.game_frame,
            SequenceClock::Sim => self.ctx.frame,
        };
        if self.player.state == PlayerState::Pending {
            self.player.state = PlayerState::Playing;
            self.player.started = now;
            if doc.holds_gameplay {
                let entities: Vec<EntityId> = self.roles.iter().map(|r| r.entity).collect();
                bind(self.world, self.pid, &entities, self.ctx);
            }
        }
        if self.player.skip {
            self.skip_to_end(&doc);
            self.finish(SequenceEnd::Skipped);
            return false;
        }
        let elapsed = now.as_u64().saturating_sub(self.player.started.as_u64());
        let playhead = if let Some(wait) = self.player.wait.clone() {
            let cleared = match wait.kind {
                WaitKind::LineEnd { seq } => !line_alive(self.world, seq),
                WaitKind::Release => std::mem::take(&mut self.player.released),
            };
            if !cleared {
                self.store();
                return doc.holds_gameplay;
            }
            self.player.waited = elapsed.saturating_sub(u64::from(wait.at));
            self.player.wait = None;
            wait.at
        } else {
            let natural =
                u32::try_from(elapsed.saturating_sub(self.player.waited)).unwrap_or(u32::MAX);
            // A playhead stops at the earliest wait key it would pass.
            let earliest = doc
                .tracks
                .dialogue
                .iter()
                .filter(|k| k.wait.is_some())
                .map(|k| k.at)
                .chain(
                    doc.tracks
                        .event
                        .iter()
                        .filter(|k| k.wait.is_some())
                        .map(|k| k.at),
                )
                .filter(|&at| at <= natural && self.player.applied_through.is_none_or(|t| at > t))
                .min();
            earliest.unwrap_or(natural)
        };
        let applied = self.player.applied_through;
        let due = |at: u32| at <= playhead && applied.is_none_or(|through| at > through);
        for key in doc.tracks.camera.iter().filter(|k| due(k.at)) {
            self.camera(key);
            self.applied(SequenceTrack::Camera, key.at);
        }
        self.transforms(&doc, &due, playhead);
        for key in doc.tracks.animation.iter().filter(|k| due(k.at)) {
            self.animation(&key.role, &key.clip, key.fade);
            self.applied(SequenceTrack::Animation, key.at);
        }
        for key in doc.tracks.ui.iter().filter(|k| due(k.at)) {
            self.ui(&key.role, &key.tracks, false);
            self.applied(SequenceTrack::Ui, key.at);
        }
        for key in doc.tracks.audio.iter().filter(|k| due(k.at)) {
            self.audio(&key.play, key.gain, key.role.as_deref(), key.range);
            self.applied(SequenceTrack::Audio, key.at);
        }
        for key in doc.tracks.dialogue.iter().filter(|k| due(k.at)) {
            let seq = self.dialogue(&key.line, key.speaker.as_deref());
            if let (Some(DialogueWait::LineEnd), Some(seq)) = (key.wait, seq) {
                self.park(key.at, WaitKind::LineEnd { seq });
            }
            self.applied(SequenceTrack::Dialogue, key.at);
        }
        for key in doc.tracks.event.iter().filter(|k| due(k.at)) {
            self.applied(SequenceTrack::Event, key.at);
            self.fire(&key.name);
            if key.wait == Some(EventWait::Release) && !std::mem::take(&mut self.player.released) {
                self.park(key.at, WaitKind::Release);
            }
        }
        self.player.applied_through = Some(playhead);
        if self.player.wait.is_none() && playhead >= doc.length_ticks {
            self.finish(SequenceEnd::Completed);
            return false;
        }
        self.store();
        doc.holds_gameplay
    }

    /// Writes the player's state back to its entity.
    fn store(self) {
        if let Ok(Some(stored)) = self.world.get_mut::<SequencePlayer>(self.pid) {
            *stored = self.player;
        }
    }

    /// Parks the playhead at `at` until `kind` clears; one wait at a time.
    fn park(&mut self, at: u32, kind: WaitKind) {
        if self.player.wait.is_none() {
            self.player.wait = Some(PendingWait { at, kind });
        }
    }

    fn applied(&mut self, track: SequenceTrack, at: u32) {
        let handle = self.handle();
        self.events
            .push(SequenceEvent::KeyApplied { handle, track, at });
    }

    fn fire(&mut self, name: &str) {
        let handle = self.handle();
        self.events.push(SequenceEvent::Fired {
            handle,
            name: name.to_owned(),
        });
    }

    /// Ends the sequence: interrupts a skipped or cancelled one's lines, emits `Ended` and despawns
    /// the player and its bindings.
    fn finish(self, end: SequenceEnd) {
        if end != SequenceEnd::Completed {
            interrupt_said(self.world, &self.player.said);
        }
        self.events.push(SequenceEvent::Ended {
            handle: SequenceHandle(self.pid),
            end,
        });
        for role in &self.roles {
            let _ = self.world.despawn(role.binding);
        }
        let _ = self.world.despawn(self.pid);
    }

    fn degrade(&mut self, reason: &str, message: String) {
        report(
            self.world,
            code::SEQUENCE_KEY_DEGRADED,
            format!("seq#{}:{reason}", self.pid),
            message,
        );
    }

    /// The index of `role`'s binding when its entity is alive; otherwise `L4352` once and `None`.
    fn resolve(&mut self, role: &str) -> Option<usize> {
        let found = self
            .roles
            .iter()
            .position(|r| r.name == role && self.world.is_alive(r.entity));
        if found.is_none() {
            report(
                self.world,
                code::SEQUENCE_ROLE_MISSING,
                format!("seq#{}:{role}", self.pid),
                format!(
                    "role `{role}` of sequence `{}` is unbound or its entity is gone; its keys are inert",
                    self.player.sequence
                ),
            );
        }
        found
    }

    fn camera(&mut self, key: &CameraKey) {
        let Some(index) = self.resolve(&key.role) else {
            return;
        };
        let entity = self.roles[index].entity;
        let saved = self.roles[index].saved_rig.clone();
        let Ok(Some(rig)) = self.world.get_mut::<CameraRig>(entity) else {
            self.degrade(
                &format!("camera:{}", key.role),
                format!(
                    "role `{}` has no `CameraRig`; its camera keys are skipped",
                    key.role
                ),
            );
            return;
        };
        let mut now_saved = None;
        if let Some(config) = &key.rig {
            if saved.is_none() {
                now_saved = Some(rig.rig.clone());
            }
            rig.rig = config.clone();
        } else if let Some(saved) = saved {
            rig.rig = saved;
        }
        if key.snap {
            snap_rig(rig);
        }
        if let Some(rig) = now_saved {
            self.roles[index].saved_rig = Some(rig.clone());
            if let Ok(Some(binding)) = self
                .world
                .get_mut::<SequenceBinding>(self.roles[index].binding)
            {
                binding.saved_rig = Some(rig);
            }
        }
    }

    /// Applies the due transform keys, then writes every live span at `playhead` and drops the ones
    /// that have ended. A key replaces its role's live span, which first writes its value now, so
    /// the new span starts from the exact current pose.
    fn transforms(&mut self, doc: &SequenceDoc, due: &dyn Fn(u32) -> bool, playhead: u32) {
        for key in doc.tracks.transform.iter().filter(|k| due(k.at)) {
            if let Some(n) = self.player.spans.iter().position(|s| s.role == key.role) {
                let old = self.player.spans.remove(n);
                self.write_span_at(&old, playhead);
            }
            if let Some(span) = self.start_span(key) {
                self.player.spans.push(span);
            }
            self.applied(SequenceTrack::Transform, key.at);
        }
        for span in std::mem::take(&mut self.player.spans) {
            self.write_span_at(&span, playhead);
            if playhead < span.at.saturating_add(span.over) {
                self.player.spans.push(span);
            }
        }
    }

    /// Writes `span` at the playhead's progress through it.
    fn write_span_at(&mut self, span: &TransformSpan, playhead: u32) {
        #[allow(clippy::cast_precision_loss)]
        let u = if span.over == 0 {
            1.0
        } else {
            (playhead.saturating_sub(span.at) as f32 / span.over as f32).min(1.0)
        };
        self.write_span(span, u);
    }

    /// Writes `span` at progress `u` to its role's entity, degrading the key when it cannot.
    fn write_span(&mut self, span: &TransformSpan, u: f32) {
        let Some(index) = self.resolve(&span.role) else {
            return;
        };
        let entity = self.roles[index].entity;
        if let Err(message) = write_span(self.world, entity, span, u) {
            self.degrade(&format!("transform:{}", span.role), message);
        }
    }

    /// The span `key` starts: the role's current world pose and the target, fixed now. `None` (the
    /// key is inert) when the role, its `Transform`, the anchor or the path is unusable.
    fn start_span(&mut self, key: &TransformKey) -> Option<TransformSpan> {
        let index = self.resolve(&key.role)?;
        let entity = self.roles[index].entity;
        let Some(pose) = crate::hierarchy::world_transform(self.world, entity) else {
            self.degrade(
                &format!("transform:{}", key.role),
                format!(
                    "role `{}` has no `Transform`; its transform keys are skipped",
                    key.role
                ),
            );
            return None;
        };
        let motion = match (&key.path, &key.to) {
            (Some(path), _) => {
                if let Err(message) = path_sampler(self.world, path) {
                    self.degrade(&format!("transform:{}", key.role), message);
                    return None;
                }
                SpanMotion::Path {
                    path: path.clone(),
                    orient: key.orient,
                }
            }
            (None, Some(to)) => {
                let (translation, rotation) = self.target(&key.role, to)?;
                SpanMotion::To {
                    from: pose.trs.translation,
                    from_rotation: pose.trs.rotation,
                    to: translation,
                    to_rotation: rotation,
                }
            }
            (None, None) => return None,
        };
        Some(TransformSpan {
            role: key.role.clone(),
            at: key.at,
            over: key.over,
            ease: key.ease,
            motion,
        })
    }

    /// The world pose `to` names, from the anchor's world pose now.
    fn target(&mut self, role: &str, to: &TransformTo) -> Option<([f32; 3], Option<[f32; 4]>)> {
        let yaw = to
            .yaw_deg
            .map(|yaw| euler_xyz_degrees_to_quat([0.0, yaw, 0.0]));
        let Some(anchor) = &to.anchor else {
            return Some((to.offset, yaw));
        };
        let index = self.resolve(anchor)?;
        let Some(pose) = crate::hierarchy::world_transform(self.world, self.roles[index].entity)
        else {
            self.degrade(
                &format!("transform:{role}"),
                format!("anchor `{anchor}` has no `Transform`; the key of `{role}` is skipped"),
            );
            return None;
        };
        let rotated = quat_rotate(pose.trs.rotation, to.offset);
        let translation = [
            pose.trs.translation[0] + rotated[0],
            pose.trs.translation[1] + rotated[1],
            pose.trs.translation[2] + rotated[2],
        ];
        Some((translation, yaw.map(|yaw| quat_mul(pose.trs.rotation, yaw))))
    }

    fn animation(&mut self, role: &str, clip: &str, fade: f32) {
        let Some(index) = self.resolve(role) else {
            return;
        };
        let entity = self.roles[index].entity;
        let now = entity_clock(self.world, entity, self.ctx).frame;
        let Ok(Some(animation)) = self.world.get_mut::<Animation>(entity) else {
            self.degrade(
                &format!("animation:{role}"),
                format!("role `{role}` has no `Animation`; its animation keys are skipped"),
            );
            return;
        };
        crossfade(animation, clip, fade, now);
    }

    /// Writes the key's tracks as the role's `UiTween`, each `start` an offset from now on its own
    /// clock. `end_form` makes every `once` track jump to its end.
    fn ui(&mut self, role: &str, tracks: &[ludion_components::ui::UiTweenTrack], end_form: bool) {
        let Some(index) = self.resolve(role) else {
            return;
        };
        let entity = self.roles[index].entity;
        let mut written = tracks.to_vec();
        for track in &mut written {
            let frame = match track.clock {
                UiClock::Real => self.ctx.frame,
                UiClock::Game => self.ctx.game_frame,
            };
            if end_form {
                if track.repeat == UiTweenRepeat::Once {
                    track.duration = 0.0;
                }
                track.start = frame.as_u64();
            } else {
                track.start += frame.as_u64();
            }
        }
        let tween = UiTween { tracks: written };
        if let Ok(Some(existing)) = self.world.get_mut::<UiTween>(entity) {
            *existing = tween;
        } else {
            let _ = self.world.insert(entity, tween);
        }
    }

    fn audio(&mut self, sound: &str, gain: f32, role: Option<&str>, range: f32) {
        let position = match role {
            None => None,
            Some(role) => {
                let Some(index) = self.resolve(role) else {
                    return;
                };
                crate::hierarchy::world_transform(self.world, self.roles[index].entity)
                    .map(|pose| pose.trs.translation)
            }
        };
        let Some(queue) = self.world.resource_mut::<crate::AudioQueue>() else {
            return;
        };
        match position {
            Some(at) => queue.play_at(sound, gain, at, range),
            None => queue.play(sound, gain),
        }
    }

    /// Says the line; the `seq` of the spoken line, or `None` when it was refused.
    fn dialogue(&mut self, line: &str, speaker: Option<&str>) -> Option<u64> {
        let entity = match speaker {
            None => None,
            Some(role) => self.resolve(role).map(|index| self.roles[index].entity),
        };
        match crate::dialogue::say(self.world, line, entity) {
            Ok(handle) => {
                let seq = self
                    .world
                    .get::<DialogueLine>(handle.0)
                    .ok()
                    .flatten()
                    .map(|said| said.seq);
                if let Some(seq) = seq {
                    self.player.said.push(seq);
                }
                seq
            }
            Err(refused) => {
                self.degrade(
                    &format!("dialogue:{line}"),
                    format!("dialogue line `{line}` was refused: {}", refused.message),
                );
                None
            }
        }
    }

    /// The skip: each role's last camera, transform, animation and ui key not yet applied takes its end form,
    /// every camera role touched snaps, and unfired `fire` events fire in order.
    fn skip_to_end(&mut self, doc: &SequenceDoc) {
        let applied = self.player.applied_through;
        let pending = |at: u32| applied.is_none_or(|through| at > through);
        let last = |roles: Vec<&str>| -> Vec<usize> {
            let mut last: BTreeMap<&str, usize> = BTreeMap::new();
            for (n, role) in roles.into_iter().enumerate() {
                last.insert(role, n);
            }
            let mut indices: Vec<usize> = last.into_values().collect();
            indices.sort_unstable();
            indices
        };
        let mut snapped: Vec<String> = doc
            .tracks
            .camera
            .iter()
            .filter(|k| !pending(k.at))
            .map(|k| k.role.clone())
            .collect();
        let camera = &doc.tracks.camera;
        for n in last(camera.iter().map(|k| k.role.as_str()).collect()) {
            let key = &camera[n];
            if pending(key.at) {
                let key = CameraKey {
                    snap: false,
                    ..key.clone()
                };
                self.camera(&key);
                snapped.push(key.role);
                self.applied(SequenceTrack::Camera, camera[n].at);
            }
        }
        for span in std::mem::take(&mut self.player.spans) {
            self.write_span(&span, 1.0);
        }
        let transform = &doc.tracks.transform;
        for n in last(transform.iter().map(|k| k.role.as_str()).collect()) {
            let key = &transform[n];
            if pending(key.at) {
                if let Some(span) = self.start_span(key) {
                    self.write_span(&span, 1.0);
                }
                self.applied(SequenceTrack::Transform, key.at);
            }
        }
        let animation = &doc.tracks.animation;
        for n in last(animation.iter().map(|k| k.role.as_str()).collect()) {
            let key = &animation[n];
            if pending(key.at) {
                self.animation(&key.role, &key.clip, 0.0);
                self.applied(SequenceTrack::Animation, key.at);
            }
        }
        let ui = &doc.tracks.ui;
        for n in last(ui.iter().map(|k| k.role.as_str()).collect()) {
            let key = &ui[n];
            if pending(key.at) {
                self.ui(&key.role, &key.tracks, true);
                self.applied(SequenceTrack::Ui, key.at);
            }
        }
        snapped.sort();
        snapped.dedup();
        for role in snapped {
            if let Some(index) = self.resolve(&role)
                && let Ok(Some(rig)) = self.world.get_mut::<CameraRig>(self.roles[index].entity)
            {
                snap_rig(rig);
            }
        }
        for key in doc.tracks.event.iter().filter(|k| pending(k.at)) {
            if key.on_skip == OnSkip::Fire {
                self.applied(SequenceTrack::Event, key.at);
                self.fire(&key.name);
            }
        }
    }
}

/// The shared sampler of the path `path`, or the loader's first message.
fn path_sampler(
    world: &mut World,
    path: &str,
) -> Result<std::sync::Arc<ludion_scene::PathSampler>, String> {
    let library = world
        .resource_mut::<crate::path_follow::PathLibrary>()
        .ok_or_else(|| "no path library is installed".to_owned())?;
    library.sampler(path).map_err(|found| {
        let reason = found.first().map_or("", |d| d.message.as_str());
        format!("path `{path}` did not load ({reason}); the transform key is skipped")
    })
}

/// Writes `span` at progress `u` (`1` is exactly its end) to `entity`; the error feeds `degrade`.
fn write_span(
    world: &mut World,
    entity: EntityId,
    span: &TransformSpan,
    u: f32,
) -> Result<(), String> {
    let eased = span.ease.apply(u);
    let no_transform = || {
        format!(
            "role `{}` has no `Transform`; its transform keys are skipped",
            span.role
        )
    };
    match &span.motion {
        SpanMotion::To {
            from,
            from_rotation,
            to,
            to_rotation,
        } => {
            let translation = if u >= 1.0 {
                *to
            } else {
                std::array::from_fn(|i| from[i] + (to[i] - from[i]) * eased)
            };
            let rotation = to_rotation.map(|end| {
                if u >= 1.0 {
                    return end;
                }
                let widen = |q: [f32; 4]| q.map(f64::from);
                let mixed = ludion_scene::slerp(
                    widen(*from_rotation),
                    widen(end),
                    f64::from(eased.clamp(0.0, 1.0)),
                );
                #[allow(clippy::cast_possible_truncation)]
                mixed.map(|c| c as f32)
            });
            write_world_pose(world, entity, translation, rotation).ok_or_else(no_transform)
        }
        SpanMotion::Path { path, orient } => {
            let sampler = path_sampler(world, path)?;
            let s = if u >= 1.0 {
                sampler.length()
            } else {
                f64::from(eased) * sampler.length()
            };
            #[allow(clippy::cast_possible_truncation)]
            let point = sampler.point_at(s).map(|c| c as f32);
            let facing = crate::path_follow::orient(&sampler, *orient, s, 1.0);
            let Ok(Some(transform)) = world.get_mut::<Transform>(entity) else {
                return Err(no_transform());
            };
            transform.translation = point;
            if let Some(rotation) = facing {
                transform.rotation = rotation;
            }
            Ok(())
        }
    }
}

/// Places `entity` at a world `translation` (and `rotation`, when given) by writing its local
/// `Transform` under its parent; scale is never written. `None` when it has no `Transform`.
fn write_world_pose(
    world: &mut World,
    entity: EntityId,
    translation: [f32; 3],
    rotation: Option<[f32; 4]>,
) -> Option<()> {
    let own = crate::hierarchy::world_transform(world, entity)?.trs;
    let parent = crate::hierarchy::parent_of(world, entity)
        .and_then(|parent| crate::hierarchy::chain_pose(world, parent.0))
        .map_or(Trs::IDENTITY, |pose| pose.trs);
    let local = parent.localize(&Trs {
        translation,
        rotation: rotation.unwrap_or(own.rotation),
        scale: own.scale,
    });
    let transform = world.get_mut::<Transform>(entity).ok().flatten()?;
    transform.translation = local.translation;
    if rotation.is_some() {
        transform.rotation = quat_to_euler_xyz_degrees(local.rotation);
    }
    Some(())
}

/// Whether a dialogue line with `seq` still exists (lines despawn when they end).
fn line_alive(world: &mut World, seq: u64) -> bool {
    world
        .query_ids::<(&DialogueLine,)>()
        .any(|(_, (line,))| line.seq == seq)
}

/// Interrupts the dialogue lines a sequence said, found by `seq`.
fn interrupt_said(world: &mut World, said: &[u64]) {
    if said.is_empty() {
        return;
    }
    let lines: Vec<(u64, EntityId)> = world
        .query_ids::<(&DialogueLine,)>()
        .filter(|(_, (line,))| said.contains(&line.seq))
        .map(|(id, (line,))| (line.seq, id))
        .collect();
    let mut lines = lines;
    lines.sort_unstable();
    for (_, id) in lines {
        crate::dialogue::interrupt(world, LineHandle(id));
    }
}
