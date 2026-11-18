//! The sim-side audio seam: the [`AudioQueue`] resource game systems push into, and the
//! frame-stamped log the engine drains it into.
//!
//! Audio in Ludion is *events as plain data*, never sound synthesis on the sim thread. A game system
//! pushes an [`AudioEvent`] (a WAV path + gain), or a [loop](AudioQueue::loop_start) request, into
//! the [`AudioQueue`] world resource; the engine drains that queue at each frame boundary (see
//! [`Engine::advance_live`](crate::Engine)) into two sinks:
//!
//! - an in-engine [frame-stamped log](AudioLogEntry) — queryable in-process, the text-first
//!   verification surface (which sound played, at which frame), with no audio goldens; and
//! - an optional [`crossbeam_channel`] sender a host wires to the playback sink — the audio analog of
//!   the sim→render frame handoff, strictly one-directional (playback never feeds back into the sim).
//!
//! Because the events are a pure function of the simulation and nothing about playback is observable
//! from a tick, a replayed session regenerates the exact same stream, and the `WorldHash` is
//! unaffected (the queue is a resource, excluded from the hash).
//!
//! Music is a few more messages ([`AudioQueue::music_play`], [`AudioQueue::music_set`],
//! [`AudioQueue::music_stem_gain`], [`AudioQueue::music_stop`]); the sink streams the Ogg Vorbis
//! track or stems and times its fades.
//!
//! A loop's *playback* state lives only in the sink — the engine holds no table of what is sounding,
//! and a world rebuild sends [`AudioMessage::StopAllLoops`] rather than diffing against one.

pub use ludion_core::audio::{
    AudioCategory, AudioEvent, AudioMessage, Bus, CategoryGains, LoopId, MixLayer,
};
use ludion_core::time::FrameIndex;
use ludion_ecs::World;

/// An ECS resource: the per-tick queue of [`AudioMessage`]s a game system pushes into.
///
/// The engine installs one into every world it builds, so a game system can always
/// `world.resource_mut::<AudioQueue>()` it. The engine drains it once per tick at the frame boundary;
/// between ticks it holds the events emitted by the frame that just ran. It is a plain resource,
/// excluded from the `WorldHash` and from snapshots.
#[derive(Default, Debug)]
pub struct AudioQueue {
    pending: Vec<Queued>,
    /// Sounds queued with a NaN or infinite gain since the last drain, played at gain 0 (`L5005`).
    non_finite: Vec<String>,
}

/// One pending queue entry: a message ready to send, or a positional one-shot the engine resolves
/// against the listener at the frame boundary.
#[derive(Debug)]
pub(crate) enum Queued {
    Message(AudioMessage),
    At {
        event: AudioEvent,
        position: [f32; 3],
        range: f32,
    },
}

impl AudioQueue {
    /// Queues an [`AudioEvent`] to play once. Drained at the end of the current tick.
    pub fn push(&mut self, mut event: AudioEvent) {
        event.gain = self.finite_gain(&event.sound, event.gain);
        self.pending
            .push(Queued::Message(AudioMessage::OneShot(event)));
    }

    /// Convenience: queues a sound by path and gain (`1.0` is unattenuated), played once, on the
    /// [`AudioCategory::Sfx`] bus. An ambient one-shot goes through [`push`](Self::push).
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.play("sounds/bounce.wav", 1.0);
    /// ```
    pub fn play(&mut self, sound: impl Into<String>, gain: f32) {
        self.push(AudioEvent::new(sound, gain));
    }

    /// Queues an [`AudioEvent`] to play once from `position` in the world. At the frame boundary its
    /// gain is resolved against the listener (the camera) with the [`AudioEmitter`](crate::AudioEmitter)
    /// falloff — distance per the game's [`AudioDistance`], `range` a hard cutoff. Out of range, below
    /// audibility, or with no camera, nothing is sent or logged (and it is not retried); otherwise it
    /// is an ordinary one-shot at the attenuated gain, and under [`AudioPan::Stereo`] carries the
    /// pan of its side of the listener (the event's own `pan` is overwritten).
    /// [`play_at`](Self::play_at) is the `Sfx` convenience; this is the way to place an ambient
    /// one-shot.
    pub fn push_at(&mut self, mut event: AudioEvent, position: [f32; 3], range: f32) {
        event.gain = self.finite_gain(&event.sound, event.gain);
        self.pending.push(Queued::At {
            event,
            position,
            range,
        });
    }

    /// Convenience: queues a sound by path and gain (`1.0` is unattenuated) to play once from
    /// `position`, on the [`AudioCategory::Sfx`] bus. The gain is resolved at the frame boundary
    /// against the listener (the camera) with the [`AudioEmitter`](crate::AudioEmitter) falloff —
    /// distance per the game's [`AudioDistance`], `range` a hard cutoff. Out of range, below
    /// audibility, or with no camera, nothing is sent or logged; once resolved it is an ordinary
    /// one-shot at the attenuated gain, panned under [`AudioPan::Stereo`]. An ambient one-shot goes
    /// through [`push_at`](Self::push_at).
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.play_at("sounds/impact.wav", 1.0, [4.0, 0.0, 2.0], 12.0);
    /// ```
    pub fn play_at(&mut self, sound: impl Into<String>, gain: f32, position: [f32; 3], range: f32) {
        self.push_at(AudioEvent::new(sound, gain), position, range);
    }

    /// Starts `sound` looping at `gain` until stopped — a global bed identified by its own path, so
    /// two systems looping one path share one voice.
    ///
    /// Idempotent: calling it again with the same sound updates the running loop's gain in place
    /// rather than restarting it (no click), which makes "recompute the gain and resend it every
    /// tick" the natural calling convention. A loop is [`AudioCategory::Ambient`] — the manual
    /// primitive exists for beds.
    ///
    /// There is no liveness diff behind this: a loop plays until [`loop_stop`](Self::loop_stop) or
    /// until a world rebuild (a `scene.load`, a district switch, a restore) sends the blanket stop,
    /// so a forgotten `loop_stop` sounds forever within one scene.
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.loop_start("sounds/wind.wav", 0.4);
    /// ```
    pub fn loop_start(&mut self, sound: impl Into<String>, gain: f32) {
        let sound = sound.into();
        let gain = self.finite_gain(&sound, gain);
        self.pending.push(Queued::Message(AudioMessage::LoopSync {
            id: LoopId::Sound(sound.clone()),
            sound,
            gain,
            pan: 0.0,
            category: AudioCategory::Ambient,
        }));
    }

    /// Updates a running loop's gain: the same idempotent message [`loop_start`](Self::loop_start)
    /// sends, spelled the way an update site reads. Starts the loop if it is not already playing.
    pub fn loop_set_gain(&mut self, sound: impl Into<String>, gain: f32) {
        self.loop_start(sound, gain);
    }

    /// Stops the loop playing `sound`, if any; the sink fades it out over about 10 ms.
    pub fn loop_stop(&mut self, sound: impl Into<String>) {
        self.pending.push(Queued::Message(AudioMessage::LoopStop {
            id: LoopId::Sound(sound.into()),
        }));
    }

    /// Streams the Ogg Vorbis `track` (a content-root-relative path) as the music, looping
    /// gaplessly, fading in over `fade_frames` sim ticks (`0` starts at full `gain`).
    ///
    /// Music is one voice: this plays as a one-stem set. Playing the track already sounding (and
    /// not stopping) only updates its gain, so resending it every tick is harmless; any other track
    /// or [set](Self::music_set) hard-cuts the current one — no crossfade — and fades the new one
    /// in. It plays until [`music_stop`](Self::music_stop) or a
    /// world rebuild's blanket stop. The file streams from disk in the sink, never decoded up front;
    /// a missing or non-Vorbis file warns `L5003` and plays nothing.
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.music_play("music/theme.ogg", 0.8, 60);
    /// ```
    pub fn music_play(&mut self, track: impl Into<String>, gain: f32, fade_frames: u32) {
        self.push_music(track.into(), gain, true, fade_frames);
    }

    /// Like [`music_play`](Self::music_play), but the track plays once and the music goes idle at
    /// its end instead of looping.
    pub fn music_play_once(&mut self, track: impl Into<String>, gain: f32, fade_frames: u32) {
        self.push_music(track.into(), gain, false, fade_frames);
    }

    /// Fades the music out over `fade_frames` sim ticks (`0` stops it now), then idles. A no-op
    /// when nothing is playing.
    pub fn music_stop(&mut self, fade_frames: u32) {
        self.pending
            .push(Queued::Message(AudioMessage::MusicStop { fade_frames }));
    }

    /// Streams a **set** of Ogg Vorbis stems (content-root-relative paths with their starting
    /// gains) on one shared playhead, looping together gaplessly, fading in over `fade_frames` sim
    /// ticks. Stems must share a sample rate and length; one that does not is left out with `L5006`,
    /// as is any stem past 8.
    ///
    /// The same `id` while it plays (and not stopping) is a no-op, so resending it every tick is
    /// harmless — the initial stem gains apply only when the set starts. Move a stem afterwards with
    /// [`music_stem_gain`](Self::music_stem_gain). Any other set or [track](Self::music_play)
    /// hard-cuts the current music — no crossfade.
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.music_set(
    ///     "dungeon",
    ///     &[("music/base.ogg", 1.0), ("music/perc.ogg", 0.0)],
    ///     60,
    /// );
    /// ```
    pub fn music_set(&mut self, id: impl Into<String>, stems: &[(&str, f32)], fade_frames: u32) {
        let stems = stems
            .iter()
            .map(|&(stem, gain)| (stem.to_string(), self.finite_gain(stem, gain)))
            .collect();
        self.pending.push(Queued::Message(AudioMessage::MusicSet {
            id: id.into(),
            stems,
            fade_frames,
        }));
    }

    /// Slews one stem of the playing set to `gain` over `fade_frames` sim ticks (`0` still slews
    /// about 10 ms, so it never clicks). A stem not in the set is ignored, and resending the same
    /// gain every tick is free.
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.music_stem_gain("music/perc.ogg", 1.0, 120);
    /// ```
    pub fn music_stem_gain(&mut self, stem: impl Into<String>, gain: f32, fade_frames: u32) {
        let stem = stem.into();
        let gain = self.finite_gain(&stem, gain);
        self.pending
            .push(Queued::Message(AudioMessage::MusicStemGain {
                stem,
                gain,
                fade_frames,
            }));
    }

    /// Slews one layer of `bus`'s runtime gain to `gain` over `fade_frames` sim ticks (`0` still
    /// slews about 10 ms, so it never clicks). The effective gain is `authored x user x game x
    /// pause`, and the master bus multiplies every other bus. A world rebuild resets the `Game`
    /// layer and keeps the `User` layer.
    ///
    /// ```
    /// # use ludion_engine::audio::{AudioQueue, Bus, MixLayer};
    /// let mut queue = AudioQueue::default();
    /// // A settings slider: music at 40%.
    /// queue.bus_gain(Bus::Music, MixLayer::User, 0.4, 0);
    /// ```
    pub fn bus_gain(&mut self, bus: Bus, layer: MixLayer, gain: f32, fade_frames: u32) {
        let gain = self.finite_gain(&format!("bus {bus:?}"), gain);
        self.pending.push(Queued::Message(AudioMessage::BusGain {
            bus,
            layer,
            gain,
            fade_frames,
        }));
    }

    /// Holds `buses` over `fade_frames` sim ticks: each voice on them keeps its cursor (loops and
    /// music included) until [`bus_resume`](Self::bus_resume). An empty slice queues nothing.
    ///
    /// ```
    /// # use ludion_engine::audio::{AudioQueue, Bus};
    /// let mut queue = AudioQueue::default();
    /// // A pause menu: hold the world's sound, keep the interface alive.
    /// queue.bus_pause(&[Bus::Sfx, Bus::Ambient, Bus::Music], 6);
    /// ```
    pub fn bus_pause(&mut self, buses: &[Bus], fade_frames: u32) {
        self.push_pause(buses, true, fade_frames);
    }

    /// Releases `buses` over `fade_frames` sim ticks; held voices continue from their cursors. An
    /// empty slice queues nothing.
    ///
    /// ```
    /// # use ludion_engine::audio::{AudioQueue, Bus};
    /// let mut queue = AudioQueue::default();
    /// queue.bus_resume(&[Bus::Sfx, Bus::Ambient, Bus::Music], 6);
    /// ```
    pub fn bus_resume(&mut self, buses: &[Bus], fade_frames: u32) {
        self.push_pause(buses, false, fade_frames);
    }

    fn push_pause(&mut self, buses: &[Bus], paused: bool, fade_frames: u32) {
        if buses.is_empty() {
            return;
        }
        self.pending.push(Queued::Message(AudioMessage::BusPause {
            buses: buses.to_vec(),
            paused,
            fade_frames,
        }));
    }

    fn push_music(&mut self, track: String, gain: f32, looping: bool, fade_frames: u32) {
        let gain = self.finite_gain(&track, gain);
        self.pending.push(Queued::Message(AudioMessage::MusicPlay {
            track,
            gain,
            looping,
            fade_frames,
        }));
    }

    /// `gain`, or `0.0` after recording `sound` when it is NaN or infinite. A zero keeps the message
    /// meaningful: a dropped sync would leave a running loop at its last gain and a new loop
    /// undeclared, while at `0.0` the loop stays declared and the next valid gain slews back. A
    /// negative finite gain is left alone: it does not poison a mix.
    fn finite_gain(&mut self, sound: &str, gain: f32) -> f32 {
        if gain.is_finite() {
            return gain;
        }
        self.non_finite.push(sound.to_string());
        0.0
    }

    /// Whether no messages are currently queued.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Removes and returns every queued entry, leaving the queue empty (the engine's drain path).
    pub(crate) fn take(&mut self) -> Vec<Queued> {
        std::mem::take(&mut self.pending)
    }

    /// Removes and returns the sounds queued with a non-finite gain since the last call (the
    /// engine's drain path reports each once).
    pub(crate) fn take_non_finite(&mut self) -> Vec<String> {
        std::mem::take(&mut self.non_finite)
    }
}

/// Queues a plain one-shot `sound` at `gain` on `world`'s [`AudioQueue`]: the one call for an
/// effect that needs no position and no loop. A positional or looping sound stays on
/// [`AudioQueue`] directly.
///
/// A no-op on a world with no queue. An engine installs the queue on every world it builds, so only
/// a bare `World` in a test can lack one, and a missing sound is not worth a sim panic.
///
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::{AudioQueue, play_sound};
///
/// let mut world = World::new();
/// world.insert_resource(AudioQueue::default());
/// play_sound(&mut world, "sounds/blip.wav", 0.5);
/// // A bare world with no queue is a quiet no-op.
/// play_sound(&mut World::new(), "sounds/blip.wav", 0.5);
/// ```
pub fn play_sound(world: &mut World, sound: impl Into<String>, gain: f32) {
    if let Some(queue) = world.resource_mut::<AudioQueue>() {
        queue.play(sound, gain);
    }
}

/// The gain below which a positional emitter is treated as silent: it syncs nothing, and stops if it
/// was sounding. Small enough to be inaudible, large enough that an emitter exactly at its `range`
/// (falloff `0.0`) never sends a stream of zero-gain syncs.
pub(crate) const AUDIBILITY_EPSILON: f32 = 1e-3;

/// How far an audible emitter's gain may drift from the last one sent before it is sent again: 1% of
/// the sent gain, about 0.09 dB, far under what a listener hears. The sink slews each gain change
/// over a few milliseconds, so the step is smooth.
pub(crate) const RESYNC_GAIN_RATIO: f32 = 0.01;

/// How far an audible emitter's pan may drift from the last one sent before it is sent again: 1% of
/// the stereo field, far under what a listener hears; the sink slews it over a few milliseconds.
pub(crate) const RESYNC_PAN_STEP: f32 = 0.02;

/// Within this distance of the listener a positional sound's pan scales down toward centre, so a
/// sound passing through the listener never jumps from one side to the other.
pub(crate) const PAN_INNER_RADIUS: f32 = 0.5;

/// How a positional sound's distance to the listener is measured: the game authors it once on the
/// builder (`EngineBuilder::audio_distance`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioDistance {
    /// Ground-plane distance (`x`, `z`): height above the listener does not attenuate. The default.
    #[default]
    Planar,
    /// Full three-dimensional distance: a sound overhead is as far as one beside the listener.
    Spatial,
}

/// Whether positional sounds are placed across the stereo field: the game authors it once on the
/// builder (`EngineBuilder::audio_pan`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioPan {
    /// Every positional sound is centred (`pan == 0.0`): the stream of a game that never opts in.
    /// The default.
    #[default]
    Centred,
    /// A positional sound carries a pan from the listener's right and horizontal plane.
    Stereo,
}

/// The last [`AudioMessage::LoopSync`] sent for an emitter in earshot: the filter that keeps a
/// static emitter from resending itself every tick.
pub(crate) struct SentLoop {
    pub(crate) sound: String,
    pub(crate) gain: f32,
    pub(crate) pan: f32,
    pub(crate) category: AudioCategory,
}

impl SentLoop {
    /// Whether a sync of `sound` at `gain` and `pan` on `category` would say nothing new: the same
    /// sound and bus, a gain within [`RESYNC_GAIN_RATIO`] and a pan within [`RESYNC_PAN_STEP`] of
    /// the ones sent.
    pub(crate) fn is_current(
        &self,
        sound: &str,
        gain: f32,
        pan: f32,
        category: AudioCategory,
    ) -> bool {
        self.sound == sound
            && self.category == category
            && (gain - self.gain).abs() <= self.gain * RESYNC_GAIN_RATIO
            && (pan - self.pan).abs() <= RESYNC_PAN_STEP
    }
}

/// The gain an [`AudioEmitter`](crate::AudioEmitter) sounds at from `listener`: its authored `gain`
/// attenuated by `clamp(1 − distance/range, 0, 1)` — the [`PointLight`](crate::PointLight) falloff.
/// Under [`AudioDistance::Planar`] (the default) distance is measured on the ground plane (`x`, `z`)
/// only, so height above the ground does not attenuate; under [`AudioDistance::Spatial`] it is the
/// full three-dimensional distance.
///
/// `range` is a hard cutoff: an emitter exactly at `range` is exactly `0.0`, and one beyond it stays
/// `0.0`. A non-positive `range` is rejected at authoring time (`L1136`); should one reach here from
/// code — every emitter placed by `games/vigil` is inserted from Rust, past the deserialize-time
/// check — a non-positive or non-finite `range` is silent rather than the falloff formula's `1.0 −
/// distance/range` saturating to full gain at every distance (a negative `range` makes that quotient
/// negative, and `clamp` cannot rescue a fraction that never crosses zero).
pub(crate) fn emitter_gain(
    gain: f32,
    range: f32,
    listener: [f32; 3],
    emitter: [f32; 3],
    distance: AudioDistance,
) -> f32 {
    if !range.is_finite() || range <= 0.0 {
        return 0.0;
    }
    let dx = emitter[0] - listener[0];
    let dz = emitter[2] - listener[2];
    let squared = match distance {
        AudioDistance::Planar => dx * dx + dz * dz,
        AudioDistance::Spatial => {
            let dy = emitter[1] - listener[1];
            dx * dx + dy * dy + dz * dz
        }
    };
    gain * (1.0 - ludion_core::math::sqrt(squared) / range).clamp(0.0, 1.0)
}

/// The gain a sound placed at `at` reaches `listener` with, or `None` when it is inaudible there —
/// the one audibility rule the AudioEmitter sync and play_at share.
pub(crate) fn audible_gain(
    gain: f32,
    range: f32,
    listener: [f32; 3],
    at: [f32; 3],
    distance: AudioDistance,
) -> Option<f32> {
    let gain = emitter_gain(gain, range, listener, at, distance);
    (gain.is_finite() && gain > AUDIBILITY_EPSILON).then_some(gain)
}

/// The listener's pose for stereo placement: where the ear is, and which way is right and up.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AudioListener {
    pub(crate) position: [f32; 3],
    pub(crate) right: [f32; 3],
    pub(crate) up: [f32; 3],
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Where a sound at `at` sits across the listener's stereo field, `-1.0` (left) to `1.0` (right).
///
/// The offset is projected onto the listener's horizontal plane (perpendicular to `up`), so height
/// never moves a sound sideways: dead ahead, behind and straight overhead are exactly `0.0`. Inside
/// [`PAN_INNER_RADIUS`] the side is scaled down linearly by `len / PAN_INNER_RADIUS`, so a sound on
/// top of the listener is centred and the pan never jumps as it passes through them.
pub(crate) fn stereo_pan(listener: &AudioListener, at: [f32; 3]) -> f32 {
    let d = [
        at[0] - listener.position[0],
        at[1] - listener.position[1],
        at[2] - listener.position[2],
    ];
    let up = dot(d, listener.up);
    let p = [
        d[0] - listener.up[0] * up,
        d[1] - listener.up[1] * up,
        d[2] - listener.up[2] * up,
    ];
    let len = ludion_core::math::sqrt(dot(p, p));
    if len == 0.0 {
        return 0.0;
    }
    (dot(p, listener.right) / len * (len / PAN_INNER_RADIUS).min(1.0)).clamp(-1.0, 1.0)
}

/// The gain and pan a sound placed at `at` reaches `listener` with, or `None` when it is inaudible
/// there: [`audible_gain`] plus, under [`AudioPan::Stereo`], the [`stereo_pan`]. A
/// [`Centred`](AudioPan::Centred) game gets a literal `0.0` with no pan computed.
pub(crate) fn audible_voice(
    gain: f32,
    range: f32,
    listener: &AudioListener,
    at: [f32; 3],
    distance: AudioDistance,
    pan: AudioPan,
) -> Option<(f32, f32)> {
    let gain = audible_gain(gain, range, listener.position, at, distance)?;
    let pan = match pan {
        AudioPan::Centred => 0.0,
        AudioPan::Stereo => stereo_pan(listener, at),
    };
    Some((gain, pan))
}

/// One drained [`AudioMessage`], stamped with the frame during which it was emitted.
///
/// The engine's audio log is a `Vec` of these, in emission order — the exact, replayable stream a
/// test asserts on. The frame stamp is the emitting frame for a game system's message, and the
/// current frame for the blanket stop a world rebuild sends.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioLogEntry {
    /// The frame during which this message was emitted.
    pub frame: FrameIndex,
    /// What was asked for: a one-shot, a loop sync or stop, or the blanket stop of a world rebuild.
    pub message: AudioMessage,
}

impl AudioLogEntry {
    /// The sound this entry names, if it names one — a one-shot, a loop sync or a music play (its
    /// track). A stop names a [`LoopId`], not a path, and the blanket and music stops name nothing.
    pub fn sound(&self) -> Option<&str> {
        match &self.message {
            AudioMessage::OneShot(event) => Some(&event.sound),
            AudioMessage::LoopSync { sound, .. } => Some(sound),
            AudioMessage::MusicPlay { track, .. } => Some(track),
            AudioMessage::MusicSet { id, .. } => Some(id),
            AudioMessage::MusicStemGain { stem, .. } => Some(stem),
            AudioMessage::LoopStop { .. }
            | AudioMessage::StopAllLoops
            | AudioMessage::MusicStop { .. }
            | AudioMessage::BusGain { .. }
            | AudioMessage::BusPause { .. } => None,
        }
    }

    /// The linear gain this entry carries, if it carries one (a one-shot, a loop sync or a music play).
    pub fn gain(&self) -> Option<f32> {
        match &self.message {
            AudioMessage::OneShot(event) => Some(event.gain),
            AudioMessage::LoopSync { gain, .. }
            | AudioMessage::MusicPlay { gain, .. }
            | AudioMessage::MusicStemGain { gain, .. }
            | AudioMessage::BusGain { gain, .. } => Some(*gain),
            AudioMessage::LoopStop { .. }
            | AudioMessage::StopAllLoops
            | AudioMessage::BusPause { .. }
            | AudioMessage::MusicSet { .. }
            | AudioMessage::MusicStop { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gain every queued message carries, in order.
    fn queued_gains(queue: &mut AudioQueue) -> Vec<f32> {
        queue
            .take()
            .into_iter()
            .map(|item| match item {
                Queued::Message(AudioMessage::OneShot(event)) => event.gain,
                Queued::Message(AudioMessage::LoopSync { gain, .. }) => gain,
                Queued::Message(AudioMessage::MusicPlay { gain, .. }) => gain,
                Queued::At { event, .. } => event.gain,
                other => panic!("no gain: {other:?}"),
            })
            .collect()
    }

    /// A NaN or infinite gain is queued as 0 by every verb and recorded; a finite one is untouched.
    #[test]
    fn a_non_finite_gain_is_queued_as_zero_and_recorded() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut queue = AudioQueue::default();
            queue.play("a.wav", bad);
            queue.play_at("b.wav", bad, [0.0; 3], 5.0);
            queue.loop_start("c.wav", bad);
            queue.loop_set_gain("d.wav", bad);
            queue.music_play("e.ogg", bad, 0);
            queue.music_play_once("f.ogg", bad, 0);
            assert_eq!(queue.take_non_finite().len(), 6);
            assert_eq!(queued_gains(&mut queue), [0.0; 6]);
        }
        let mut queue = AudioQueue::default();
        queue.play("a.wav", 0.25);
        queue.loop_start("c.wav", -0.5);
        queue.music_play("e.ogg", 7.0, 0);
        assert_eq!(queued_gains(&mut queue), [0.25, -0.5, 7.0]);
        assert!(queue.take_non_finite().is_empty());
    }

    /// A non-finite bus gain is queued as 0 and recorded under the bus's name; a finite one passes.
    #[test]
    fn a_non_finite_bus_gain_is_queued_as_zero_and_recorded() {
        let mut queue = AudioQueue::default();
        queue.bus_gain(Bus::Music, MixLayer::User, f32::NAN, 3);
        queue.bus_gain(Bus::Sfx, MixLayer::Game, 0.5, 0);
        assert_eq!(queue.take_non_finite(), ["bus Music"]);
        let messages = queue.take();
        let [
            Queued::Message(AudioMessage::BusGain { gain: first, .. }),
            Queued::Message(AudioMessage::BusGain { gain: second, .. }),
        ] = messages.as_slice()
        else {
            panic!("two bus gains, got {messages:?}");
        };
        assert_eq!((*first, *second), (0.0, 0.5));
    }

    /// Pausing or resuming no buses is not a message.
    #[test]
    fn an_empty_pause_queues_nothing() {
        let mut queue = AudioQueue::default();
        queue.bus_pause(&[], 4);
        queue.bus_resume(&[], 4);
        assert!(queue.is_empty());
        queue.bus_pause(&[Bus::Sfx], 4);
        assert!(!queue.is_empty());
    }

    /// The set verbs guard every stem gain the same way.
    #[test]
    fn a_non_finite_stem_gain_is_queued_as_zero_and_recorded() {
        let mut queue = AudioQueue::default();
        queue.music_set("s", &[("a.ogg", f32::NAN), ("b.ogg", 0.5)], 0);
        queue.music_stem_gain("b.ogg", f32::INFINITY, 0);
        assert_eq!(queue.take_non_finite(), ["a.ogg", "b.ogg"]);
        let messages = queue.take();
        let [
            Queued::Message(AudioMessage::MusicSet { stems, .. }),
            Queued::Message(AudioMessage::MusicStemGain { gain, .. }),
        ] = messages.as_slice()
        else {
            panic!("two music messages, got {messages:?}");
        };
        assert_eq!(
            stems,
            &[("a.ogg".to_string(), 0.0), ("b.ogg".to_string(), 0.5)]
        );
        assert_eq!(*gain, 0.0);
        let entry = |message| AudioLogEntry {
            frame: FrameIndex(0),
            message,
        };
        let set = entry(AudioMessage::MusicSet {
            id: "s".into(),
            stems: vec![],
            fade_frames: 0,
        });
        assert_eq!((set.sound(), set.gain()), (Some("s"), None));
        let stem = entry(AudioMessage::MusicStemGain {
            stem: "b.ogg".into(),
            gain: 0.25,
            fade_frames: 0,
        });
        assert_eq!((stem.sound(), stem.gain()), (Some("b.ogg"), Some(0.25)));
    }

    /// `play` is the sound-effect verb: what it queues rides the `Sfx` bus, whatever the sink's
    /// per-category master gains are.
    #[test]
    fn play_queues_a_one_shot_on_the_sfx_bus() {
        let mut queue = AudioQueue::default();
        queue.play("shot.wav", 0.5);
        let messages = queue.take();
        let [Queued::Message(AudioMessage::OneShot(event))] = messages.as_slice() else {
            panic!("play queues exactly one one-shot, got {messages:?}");
        };
        assert_eq!(event.category, AudioCategory::Sfx);
    }

    #[test]
    fn a_resync_needs_a_gain_move_past_one_percent_or_a_new_sound_or_bus() {
        let sent = SentLoop {
            sound: "bell.wav".to_string(),
            gain: 0.6,
            pan: 0.0,
            category: AudioCategory::Ambient,
        };
        assert!(sent.is_current("bell.wav", 0.605, 0.0, AudioCategory::Ambient));
        assert!(!sent.is_current("bell.wav", 0.61, 0.0, AudioCategory::Ambient));
        assert!(!sent.is_current("gong.wav", 0.6, 0.0, AudioCategory::Ambient));
        assert!(!sent.is_current("bell.wav", 0.6, 0.0, AudioCategory::Sfx));
    }

    #[test]
    fn a_resync_needs_a_pan_move_past_the_step() {
        let sent = SentLoop {
            sound: "bell.wav".to_string(),
            gain: 0.6,
            pan: 0.3,
            category: AudioCategory::Ambient,
        };
        assert!(sent.is_current(
            "bell.wav",
            0.6,
            0.3 + RESYNC_PAN_STEP / 2.0,
            AudioCategory::Ambient
        ));
        assert!(sent.is_current(
            "bell.wav",
            0.6,
            0.3 - RESYNC_PAN_STEP / 2.0,
            AudioCategory::Ambient
        ));
        assert!(!sent.is_current("bell.wav", 0.6, 0.33, AudioCategory::Ambient));
        assert!(!sent.is_current("bell.wav", 0.6, 0.0, AudioCategory::Ambient));
    }

    /// The falloff at the four distances that pin its shape: full gain on top of the emitter, half at
    /// half `range`, exactly zero at `range`, and still zero beyond it.
    #[test]
    fn emitter_gain_falls_off_linearly_to_zero_at_range() {
        let at = |x: f32| emitter_gain(0.8, 10.0, [0.0; 3], [x, 0.0, 0.0], AudioDistance::Planar);
        assert_eq!(at(0.0), 0.8);
        assert_eq!(at(5.0), 0.4);
        assert_eq!(at(10.0), 0.0);
        assert_eq!(at(25.0), 0.0);
    }

    /// A non-positive or non-finite `range` is silent at every distance, including on top of the
    /// emitter — never the falloff formula's clamp saturating to full gain (the historical bug: a
    /// negative range never stopped sounding). `L1136` catches this at deserialize time; this is the
    /// code-inserted path (`games/vigil` builds every `AudioEmitter` in Rust) reaching the same guard.
    #[test]
    fn emitter_gain_is_silent_for_a_non_positive_or_non_finite_range() {
        let at_listener =
            |range: f32| emitter_gain(1.0, range, [0.0; 3], [0.0; 3], AudioDistance::Planar);
        assert_eq!(at_listener(-1.0), 0.0);
        assert_eq!(at_listener(0.0), 0.0);
        assert_eq!(at_listener(f32::NAN), 0.0);
        assert_eq!(at_listener(f32::INFINITY), 0.0);
        assert_eq!(at_listener(f32::NEG_INFINITY), 0.0);
    }

    /// The shared rule: audible inside `range`, `None` at `range` and for a non-positive `range`.
    #[test]
    fn audible_gain_is_none_when_inaudible() {
        let at = |x: f32, range: f32| {
            audible_gain(1.0, range, [0.0; 3], [x, 0.0, 0.0], AudioDistance::Planar)
        };
        assert_eq!(at(4.0, 10.0), Some(0.6));
        assert_eq!(at(10.0, 10.0), None);
        assert_eq!(at(0.0, 0.0), None);
        assert_eq!(at(0.0, -1.0), None);
    }

    /// Planar distance is measured on the ground plane only: height does not attenuate. Spatial
    /// distance counts it.
    #[test]
    fn planar_distance_ignores_height_and_spatial_counts_it() {
        let gain = |at: [f32; 3], distance| emitter_gain(1.0, 10.0, [0.0; 3], at, distance);
        assert_eq!(
            gain([0.0, 40.0, 5.0], AudioDistance::Planar),
            gain([0.0, 0.0, 5.0], AudioDistance::Planar),
        );
        assert_eq!(gain([0.0, 5.0, 0.0], AudioDistance::Planar), 1.0);
        assert_eq!(gain([0.0, 5.0, 0.0], AudioDistance::Spatial), 0.5);
        assert_eq!(gain([0.0, 40.0, 5.0], AudioDistance::Spatial), 0.0);
    }

    /// A listener at the origin looking down `-Z`: right is `+X`, up is `+Y`.
    fn facing_north() -> AudioListener {
        AudioListener {
            position: [0.0; 3],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        }
    }

    #[test]
    fn a_sound_to_the_right_pans_right_and_to_the_left_pans_left() {
        let listener = facing_north();
        assert!(stereo_pan(&listener, [4.0, 0.0, -1.0]) > 0.9);
        assert!(stereo_pan(&listener, [-4.0, 0.0, -1.0]) < -0.9);
        let turned = AudioListener {
            right: [-1.0, 0.0, 0.0],
            ..listener
        };
        assert!(
            stereo_pan(&turned, [4.0, 0.0, -1.0]) < -0.9,
            "a half turn flips the sign"
        );
    }

    #[test]
    fn dead_ahead_behind_and_overhead_are_exactly_centre() {
        let listener = facing_north();
        for at in [
            [0.0, 0.0, -5.0],
            [0.0, 0.0, 5.0],
            [0.0, 7.0, 0.0],
            [0.0, 7.0, -3.0],
        ] {
            assert_eq!(stereo_pan(&listener, at), 0.0, "{at:?}");
        }
    }

    #[test]
    fn the_inner_radius_scales_the_pan_toward_centre() {
        let listener = facing_north();
        assert_eq!(stereo_pan(&listener, [0.0; 3]), 0.0);
        assert_eq!(
            stereo_pan(&listener, [PAN_INNER_RADIUS * 0.5, 0.0, 0.0]),
            0.5
        );
        assert_eq!(stereo_pan(&listener, [PAN_INNER_RADIUS, 0.0, 0.0]), 1.0);
        assert_eq!(
            stereo_pan(&listener, [-PAN_INNER_RADIUS * 3.0, 0.0, 0.0]),
            -1.0
        );
    }

    #[test]
    fn a_centred_game_resolves_a_literal_zero_pan() {
        let listener = facing_north();
        let at = [4.0, 0.0, 0.0];
        let centred = audible_voice(
            1.0,
            10.0,
            &listener,
            at,
            AudioDistance::Planar,
            AudioPan::Centred,
        );
        let stereo = audible_voice(
            1.0,
            10.0,
            &listener,
            at,
            AudioDistance::Planar,
            AudioPan::Stereo,
        );
        assert_eq!(
            centred.map(|(_, pan)| pan.to_bits()),
            Some(0.0f32.to_bits())
        );
        assert_eq!(stereo.map(|(_, pan)| pan), Some(1.0));
        assert_eq!(centred.map(|(gain, _)| gain), stereo.map(|(gain, _)| gain));
        let far = [40.0, 0.0, 0.0];
        assert_eq!(
            audible_voice(
                1.0,
                10.0,
                &listener,
                far,
                AudioDistance::Planar,
                AudioPan::Stereo
            ),
            None
        );
    }
}
