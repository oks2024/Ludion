//! [`sequence_update`]: the one system that plays, skips, cancels and ends sequences.
//!
//! Per player, in ascending [`EntityId`] (a later player's write to the same component wins):
//!
//! 1. a vanished document or a cancel ends it `Cancelled`;
//! 2. a pending player stamps its start from its clock;
//! 3. a skip applies the end forms and ends it `Skipped` (waits do not stop it);
//! 4. a pending wait either clears (the line ended, or `release` was called) and the playhead
//!    resumes from the wait's `at`, or the player stores itself and applies nothing;
//! 5. otherwise the playhead is `now - started - waited`, stopped at the earliest due wait key, and
//!    every key with `applied_through < at <= playhead` applies, by track (camera, animation, ui,
//!    audio, dialogue, event), then `at`, then document order; a wait key parks the playhead;
//! 6. the tick the playhead reaches `length_ticks` with no wait pending it ends `Completed`;
//! 7. a player that stays live with `holds_gameplay` makes the system call `GameClock::hold` once.

use std::collections::BTreeMap;

use ludion_camera::{CameraRig, snap_rig};
use ludion_components::ui::{UiClock, UiTween, UiTweenRepeat};
use ludion_components::{Animation, crossfade};
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::time::GameClock;
use ludion_ecs::{SystemCtx, World};

use super::docs::{CameraKey, DialogueWait, EventWait, OnSkip, SequenceClock, SequenceDoc};
use super::events::{SequenceEnd, SequenceEvent, SequenceEvents, SequenceTrack};
use super::{
    PendingWait, PlayerState, SequenceBinding, SequenceHandle, SequencePlayer, WaitKind, find_doc,
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

    fn animation(&mut self, role: &str, clip: &str, fade: f32) {
        let Some(index) = self.resolve(role) else {
            return;
        };
        let now = self.ctx.game_frame;
        let entity = self.roles[index].entity;
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

    /// The skip: each role's last camera, animation and ui key not yet applied takes its end form,
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
