//! The audio a playing sequence started: its voices and its duck, so a skip or cancel can stop them
//! and every end can release the duck.

use std::collections::BTreeMap;

use ludion_core::audio::{AudioEvent, VoiceId};
use ludion_core::ids::EntityId;
use ludion_ecs::World;

use super::SequencePlayer;
use super::docs::{DuckKey, MusicCrossfadeKey};
use crate::AudioQueue;

/// Ticks a skipped or cancelled beat fades its voices out over (the dialogue `cut_fade_ticks` default).
const CUT_FADE_TICKS: u32 = 6;

/// What one player has on air. Never hashed or snapshotted; reset with every world build, so a restore
/// forgets the ids (the voices play out) and the rebuild's blanket stop drops the duck.
#[derive(Debug, Default)]
pub(crate) struct SequenceAudio {
    pub(crate) by_player: BTreeMap<EntityId, PlayerAudio>,
}

/// One player's voices in push order and the release of its live duck.
#[derive(Debug, Default)]
pub(crate) struct PlayerAudio {
    voices: Vec<VoiceId>,
    duck_release: Option<u32>,
}

/// The one duck key a player owns, so a later duck key replaces it.
fn duck_key(pid: EntityId) -> String {
    format!("sequence:{pid}")
}

/// Starts a voice, attached to `entity` when given, and remembers its id.
pub(crate) fn play(
    world: &mut World,
    pid: EntityId,
    sound: &str,
    gain: f32,
    entity: Option<EntityId>,
    range: f32,
) {
    let Some(queue) = world.resource_mut::<AudioQueue>() else {
        return;
    };
    let event = AudioEvent::new(sound, gain);
    let id = match entity {
        Some(entity) => queue.push_on(entity, event, range),
        None => queue.push_voice(event),
    };
    if let Some(audio) = world.resource_mut::<SequenceAudio>() {
        audio.by_player.entry(pid).or_default().voices.push(id);
    }
}

/// Crossfades the music. Never undone.
pub(crate) fn crossfade(world: &mut World, key: &MusicCrossfadeKey) {
    if let Some(queue) = world.resource_mut::<AudioQueue>() {
        let stems: Vec<(&str, f32)> = key.stems.iter().map(|(s, g)| (s.as_str(), *g)).collect();
        queue.music_crossfade(&key.id, &stems, key.ticks);
    }
}

/// Starts the player's duck, replacing a live one, and remembers its release.
pub(crate) fn duck(world: &mut World, pid: EntityId, key: &DuckKey) {
    if world.resource::<SequenceAudio>().is_none() {
        return;
    }
    let Some(queue) = world.resource_mut::<AudioQueue>() else {
        return;
    };
    queue.duck_start(duck_key(pid), &key.buses, key.depth_db, key.attack);
    if let Some(audio) = world.resource_mut::<SequenceAudio>() {
        audio.by_player.entry(pid).or_default().duck_release = Some(key.release);
    }
}

/// Ends the player's audio: stops its voices when `stop_voices` and releases its duck.
pub(crate) fn end(world: &mut World, pid: EntityId, stop_voices: bool) {
    let Some(entry) = world
        .resource_mut::<SequenceAudio>()
        .and_then(|audio| audio.by_player.remove(&pid))
    else {
        return;
    };
    let Some(queue) = world.resource_mut::<AudioQueue>() else {
        return;
    };
    if stop_voices {
        for id in entry.voices {
            queue.voice_stop(id, CUT_FADE_TICKS);
        }
    }
    if let Some(release) = entry.duck_release {
        queue.duck_end(duck_key(pid), release);
    }
}

/// Drops the entries of players the game despawned.
pub(crate) fn prune(world: &mut World) {
    let Some(audio) = world.resource::<SequenceAudio>() else {
        return;
    };
    let gone: Vec<EntityId> = audio
        .by_player
        .keys()
        .copied()
        .filter(|pid| !matches!(world.get::<SequencePlayer>(*pid), Ok(Some(_))))
        .collect();
    if let Some(audio) = world.resource_mut::<SequenceAudio>() {
        for pid in gone {
            audio.by_player.remove(&pid);
        }
    }
}
