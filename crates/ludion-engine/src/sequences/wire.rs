//! The wire side of sequences: `sequence.play`, `sequence.seek` and `sequence.skip`.
//!
//! A play stashes the engine snapshot taken right after the player spawned; a seek restores it and
//! replays `tick + 1` ticks exactly as [`preview::run`](super::preview::run) does, so a seek's world
//! equals `ludion render --sequence ... --at tick`'s on a builtin-only engine.

use std::collections::BTreeMap;
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_protocol::command::{MAX_STEP_TICKS, SequenceSeekParams};
use ludion_protocol::event::{Event, SceneChangeKind, SceneChangedData};
use ludion_protocol::response::SequenceEndKind;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

use super::{
    SequenceEnd, SequenceEvents, SequenceHandle, SequencePlayer, find_doc, not_enabled, play,
    release, skip,
};
use crate::checkpoints::StoreRow;
use crate::engine::Engine;

/// The host's one seek stash: the last `sequence.play`'s player and the engine snapshot taken
/// right after it spawned (pending, no key applied). The document is rendered once, at the play;
/// a snapshot or replay keyframe clones it.
#[derive(Clone)]
pub(crate) struct SequenceSeek {
    pub(crate) player: EntityId,
    pub(crate) snapshot: Arc<Value>,
}

impl Engine {
    fn require_sequences(&self) -> Result<(), Diagnostic> {
        if self.world().resource::<SequenceEvents>().is_none() {
            return Err(not_enabled());
        }
        Ok(())
    }

    /// `sequence.play`: spawns the player and stashes the seek's starting state. Returns the player.
    ///
    /// # Errors
    /// `L4354` without `.sequences()`, `L0002` for a dead bound entity (nothing spawns), `L4350` and
    /// `L4351` as [`play`].
    pub(crate) fn sequence_play(
        &mut self,
        sequence: &str,
        bindings: &BTreeMap<String, EntityId>,
    ) -> Result<EntityId, Diagnostic> {
        self.require_sequences()?;
        for (role, entity) in bindings {
            if !self.world().is_alive(*entity) {
                return Err(Diagnostic::new(
                    code::DEAD_ENTITY,
                    format!("role `{role}` is bound to entity {entity}, which is not alive"),
                )
                .with_path(format!("/params/bindings/{role}"))
                .with_help("bind each role to an entity the world currently holds"));
            }
        }
        let pairs: Vec<(&str, EntityId)> = bindings
            .iter()
            .map(|(role, entity)| (role.as_str(), *entity))
            .collect();
        let handle = play(self.world_mut(), sequence, &pairs)?;
        let snapshot = crate::snapshot::capture_stash(self)?;
        self.sequence_seek = Some(SequenceSeek {
            player: handle.0,
            snapshot: Arc::new(snapshot),
        });
        Ok(handle.0)
    }

    /// `sequence.skip`: marks the player to skip at the next tick's update.
    ///
    /// # Errors
    /// `L4354` without `.sequences()`; `L4357` when the player is not live or not skippable.
    pub(crate) fn sequence_skip(&mut self, player: EntityId) -> Result<(), Diagnostic> {
        self.require_sequences()?;
        let refusal = match self.world().get::<SequencePlayer>(player) {
            Ok(Some(live)) => {
                let skippable =
                    find_doc(self.world(), &live.sequence).is_none_or(|doc| doc.skippable);
                (!skippable).then(|| format!("`{}` declares `skippable: false`", live.sequence))
            }
            _ => Some(format!("entity {player} is not a live sequence player")),
        };
        if let Some(reason) = refusal {
            return Err(Diagnostic::new(
                code::SEQUENCE_SKIP_REFUSED,
                format!("cannot skip: {reason}"),
            )
            .with_path("/params/player")
            .with_help(
                "skip a player that is still playing, in a document with `skippable: true`",
            ));
        }
        let skipped = skip(self.world_mut(), SequenceHandle(player));
        debug_assert!(skipped, "a live, skippable player accepts a skip");
        Ok(())
    }

    /// `sequence.seek`: restores the stashed play, then runs `tick + 1` ticks, releasing the player
    /// before each. Replies with the player and how it ended by then, none while it still plays.
    ///
    /// # Errors
    /// `L4354` without `.sequences()`; `L4358` for a player the last play did not return;
    /// `L3057` for a `tick` that needs more than one step's worth of ticks; the restore's own
    /// diagnostics; a system panic during the ticks (the ticks that ran stay recorded).
    pub(crate) fn sequence_seek(
        &mut self,
        params: SequenceSeekParams,
    ) -> Result<ResponseBody, Diagnostic> {
        self.require_sequences()?;
        let stash = match &self.sequence_seek {
            Some(stash) if stash.player == params.player => stash.clone(),
            _ => {
                return Err(Diagnostic::new(
                    code::SEQUENCE_SEEK_NO_PLAY,
                    format!(
                        "no `sequence.play` returned player {}; the host keeps the last play only",
                        params.player
                    ),
                )
                .with_path("/params/player")
                .with_help("seek the player the last `sequence.play` returned"));
            }
        };
        if params.tick.saturating_add(1) > MAX_STEP_TICKS {
            return Err(Diagnostic::new(
                code::SIM_STEP_TOO_LONG,
                format!(
                    "sequence.seek asked for tick {}; a seek runs tick + 1 ticks and the most one \
                     step may run is {MAX_STEP_TICKS}",
                    params.tick
                ),
            )
            .with_path("/params/tick")
            .with_help("seek a smaller tick; the command runs synchronously on the sim thread"));
        }

        // The stashed document carries neither the stash nor the checkpoint store; both are host
        // session state the seek must not rewind. The scene path stays: a seek is not a scene change.
        let slots = self
            .checkpoints
            .as_mut()
            .map(|state| (std::mem::take(&mut state.slots), state.last_restore.take()));
        let scene_path = self.current_scene_path().map(str::to_string);
        let restored = self.restore_from_value(&stash.snapshot);
        self.sequence_seek = Some(stash);
        if slots.is_some() {
            self.install_checkpoints(StoreRow::Replace(slots));
        }
        restored?;
        self.set_current_scene_path(scene_path);
        self.push_event(Event::SceneChanged(SceneChangedData {
            kind: SceneChangeKind::Restore,
            path: None,
            request_id: self.current_request(),
        }));

        let handle = SequenceHandle(params.player);
        let mut ended = None;
        for _ in 0..=params.tick {
            release(self.world_mut(), handle);
            self.step_ticks(1);
            if let Some(panic) = self.take_system_panic() {
                self.note_applied_before_error(Command::SequenceSeek(params));
                return Err(panic);
            }
            if ended.is_none() {
                ended = self
                    .world()
                    .resource::<SequenceEvents>()
                    .and_then(|events| events.ended(handle));
            }
        }
        Ok(ResponseBody::Sequence {
            player: params.player,
            ended: ended.map(|end| match end {
                SequenceEnd::Completed => SequenceEndKind::Completed,
                SequenceEnd::Skipped => SequenceEndKind::Skipped,
                SequenceEnd::Cancelled => SequenceEndKind::Cancelled,
            }),
        })
    }
}
