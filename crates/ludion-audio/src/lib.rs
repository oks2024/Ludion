#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-audio` — the playback sink: a thread over `cpal` that turns the engine's
//! [`AudioMessage`]s into sound.
//!
//! This crate is deliberately downstream of everything determinism-bearing. The engine emits audio
//! messages as plain data on a one-directional channel (the audio analog of the sim→render handoff);
//! this sink receives them, resolves each `sound` as a content-root-relative WAV, decodes and caches
//! its PCM, and mixes active sounds into the output device with a small gain-and-sum mixer. Playback
//! never feeds back into the sim, so the wall-clock jitter of an audio device is unobservable from a
//! tick — audio sits outside every determinism gate.
//!
//! **Loop playback state lives here and nowhere else.** A one-shot is a voice that retires; a loop is
//! a voice held in a table keyed by [`LoopId`], wrapping at the end of its buffer until the engine
//! stops it. The engine keeps no mirror of that table — it declares, the sink remembers.
//!
//! **No device is not a failure.** On a headless or device-less machine (CI, a container) the sink
//! reports [`L5001`](ludion_core::codes::code::AUDIO_DEVICE_UNAVAILABLE) once as a warning and plays
//! *silent*: it still routes every event, so nothing upstream ever blocks and the run stays green.
//! It keeps retrying the open with backoff, so a device that appears later is picked up, with the
//! declared loops and the music, and it warns of broken references as playback would
//! (`L1101` / `L1104` / `L5003`). A device lost mid-run
//! ([`L5002`](ludion_core::codes::code::AUDIO_STREAM_FAILED)) is reopened the same way.
//!
//! **Music streams.** An Ogg Vorbis track (`AudioMessage::MusicPlay`) is decoded on the sink thread
//! into a bounded ring of device-format chunks that the callback drains — never decoded up front,
//! never decoded or allocated in the callback. Fades and the gapless loop seam are computed here;
//! the music bus has its own master in [`CategoryGains`]. An `AudioMessage::MusicSet` plays up to
//! eight stems on one shared playhead, each with its own gain.
//!
//! A host owns the wiring: create the channel, hand the engine the sender
//! (`Engine::set_audio_sender`), and hand this the receiver.
//!
//! ```no_run
//! use std::path::PathBuf;
//! use ludion_audio::{AudioSink, CategoryGains, VoiceCaps};
//!
//! let (tx, rx) = crossbeam_channel::unbounded();
//! // engine.set_audio_sender(tx);  // the engine emits AudioMessages on `tx`
//! let gains = CategoryGains { sfx: 1.0, ambient: 0.6, music: 0.8 }; // the game's authored mixdown
//! let seconds_per_tick = 1.0 / 60.0; // `engine.seconds_per_tick()`: times the music fades
//! let caps = VoiceCaps::default(); // 4 per sound, 64 in all; a game overrides it in `ludion.toml`
//! let _sink = AudioSink::spawn(rx, PathBuf::from("games/hello-paddle/scenes"), gains, caps, seconds_per_tick);
//! // Dropping `_sink` detaches it: the session ends by itself once the engine's sender is gone.
//! ```

mod music;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError};
use ludion_assets::{AudioPcm, load_wav};
use ludion_core::audio::{
    AudioCategory, AudioEvent, AudioMessage, Bus, LoopId, MUSIC_STEMS_MAX, MixLayer,
};
// The mixdown the sink applies. It lives in the shared core because the game authors it on the
// engine builder and this crate consumes it; re-exported so a host names one type, not two.
pub use ludion_core::audio::{CategoryGains, VoiceCaps};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use music::{Converter, MUSIC_REFILL, MUSIC_RING_CHUNKS, MusicChunk, MusicFeed, MusicPlayer};

/// The summed level the mixdown's soft knee starts at: below it the mix is passed through untouched.
const LIMITER_KNEE: f32 = 0.8;

/// The length of a per-voice level change, in milliseconds, so no voice jumps mid-waveform.
///
/// Over it: a stop or a steal fades out; a loop's gain change slews and its bus change crossfades;
/// a music gain change slews; the music fades out on a blanket stop; a loop or the music the sink
/// restarts on its own (after a reopen, or a waiting loop that starts) fades in.
///
/// Not over it: a voice the game starts begins at its first sample at its gain; music fades
/// authored in ticks run in ticks, where `0` cuts, the hard cut to another track included; and the
/// storage bound in [`Mixer`] cuts the lowest fading voice early.
const RAMP_MS: usize = 10;

/// Device frames the callback mixes at a time, so its scratch buffer is reserved once and a larger
/// device buffer is mixed in several passes.
const MIX_BLOCK_FRAMES: usize = 1024;

/// How many voices may sound at once; past a cap the oldest live one-shot gives way.
#[derive(Clone, Copy, Debug)]
struct VoiceLimits {
    /// Most live one-shots of one sound, or `None` for no per-sound cap.
    per_sound: Option<usize>,
    /// Most live voices of all sounds, loops included. At least 1.
    total: usize,
}

/// The limits for a game's `caps`: the oldest one-shot gives way when either cap is full. The
/// defaults are [`VoiceCaps::default`]; the game's `ludion.toml` `[audio]` table overrides them.
fn voice_limits(caps: VoiceCaps) -> VoiceLimits {
    VoiceLimits {
        per_sound: Some(caps.per_sound),
        total: caps.total,
    }
}

/// A running audio sink. Its session runs on a pooled playback thread and ends on its own when the
/// engine's [`AudioMessage`] sender is dropped (the channel disconnects). Dropping the sink detaches
/// it and never waits ([`wait_timeout`](Self::wait_timeout) is the blocking end), so a host's teardown does not stand
/// on the audio device opening and closing.
///
/// **Playback threads never exit.** cpal's WASAPI backend caches its device enumerator process-wide,
/// inside the COM apartment of the first thread that asked for a device; a thread exit uninitializes
/// that apartment and leaves the cache dangling, so the next host's device lookup is an access
/// violation. A finished session therefore returns its thread to an idle pool for the next sink.
pub struct AudioSink {
    done: Receiver<()>,
}

/// One sink's work, handed to a pooled playback thread.
struct Session {
    body: Box<dyn FnOnce() + Send>,
    /// Dropped when the session ends, which is what [`AudioSink::wait_timeout`] waits on.
    done: Sender<()>,
}

/// Idle playback threads, each waiting on its own session channel.
type Pool = Mutex<Vec<Sender<Session>>>;

/// The process's pool.
static IDLE: Pool = Mutex::new(Vec::new());

fn idle_threads(pool: &'static Pool) -> MutexGuard<'static, Vec<Sender<Session>>> {
    pool.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AudioSink {
    /// Starts a sink session on an idle playback thread, or a new one. It opens the default output
    /// device (or, absent one, plays silent and keeps retrying), then routes `events` for the life
    /// of the channel, resolving each sound under `content_root` and mixing it down through `gains` — the game's
    /// authored bus balance, which a host reads off the engine ([`CategoryGains::default`] leaves the
    /// authored gains untouched) and bounding its voices by `caps` ([`VoiceCaps::default`] is the
    /// engine's policy). `seconds_per_tick` is the sim's tick length, which times music
    /// fades (given in ticks).
    ///
    /// Never fails: a device-less environment yields a silent, retrying sink, not an error.
    pub fn spawn(
        events: Receiver<AudioMessage>,
        content_root: PathBuf,
        gains: CategoryGains,
        caps: VoiceCaps,
        seconds_per_tick: f64,
    ) -> AudioSink {
        AudioSink::start_on(
            &IDLE,
            Box::new(move || {
                run(
                    CpalOutput { device: None },
                    events,
                    content_root,
                    gains,
                    caps,
                    seconds_per_tick,
                );
            }),
        )
    }

    /// Runs `body` as one session on an idle thread of `pool`, or a new one.
    fn start_on(pool: &'static Pool, body: Box<dyn FnOnce() + Send>) -> AudioSink {
        let (done_tx, done) = crossbeam_channel::bounded(0);
        let mut session = Session {
            body,
            done: done_tx,
        };
        loop {
            let Some(thread) = idle_threads(pool).pop() else {
                spawn_playback_thread(pool, session);
                break;
            };
            match thread.send(session) {
                Ok(()) => break,
                // Unreachable while threads never exit; retried rather than trusted.
                Err(returned) => session = returned.into_inner(),
            }
        }
        AudioSink { done }
    }

    /// Waits at most `timeout` for the session to end and its thread to go idle; `true` once it has.
    /// A host waits before it lets the process exit, so no thread is still opening the device then.
    pub fn wait_timeout(self, timeout: Duration) -> bool {
        // Disconnects once the session's `done` sender drops; no message is ever sent.
        !matches!(
            self.done.recv_timeout(timeout),
            Err(RecvTimeoutError::Timeout)
        )
    }
}

fn spawn_playback_thread(pool: &'static Pool, first: Session) {
    let (sessions_tx, sessions) = crossbeam_channel::unbounded::<Session>();
    std::thread::Builder::new()
        .name("ludion-audio".to_string())
        .spawn(move || {
            let mut next = Some(first);
            while let Some(session) = next.take().or_else(|| sessions.recv().ok()) {
                (session.body)();
                // Idle before the sink hears the end, so a restart after `wait_timeout` reuses this thread (one started
                // before the old session ends takes a new pooled thread).
                idle_threads(pool).push(sessions_tx.clone());
                drop(session.done);
            }
        })
        .expect("spawning the audio thread");
}

/// A voice's linear gain, stepped once per device frame: a countdown of `left` equal steps from
/// `level` to `target`, which lands on the target exactly (a float step could underflow to nothing
/// and never arrive).
#[derive(Clone, Copy, Debug)]
struct Envelope {
    level: f32,
    target: f32,
    step: f32,
    left: usize,
    /// Fading out for good: the voice retires once the countdown ends.
    retiring: bool,
}

impl Envelope {
    /// Settled at `gain`.
    fn at(gain: f32) -> Envelope {
        Envelope {
            level: gain,
            target: gain,
            step: 0.0,
            left: 0,
            retiring: false,
        }
    }

    /// Silent now, rising to `target` over `frames` device frames; settled at `target` for `0`.
    fn rising(target: f32, frames: usize) -> Envelope {
        if frames == 0 {
            return Envelope::at(target);
        }
        let mut envelope = Envelope::at(0.0);
        envelope.slew_to(target, frames);
        envelope
    }

    /// Heads for `target` over `frames` device frames, from wherever the level is now.
    fn slew_to(&mut self, target: f32, frames: usize) {
        if target == self.target {
            return;
        }
        self.target = target;
        self.step = (target - self.level) / frames as f32;
        self.left = frames.max(1);
    }

    /// Settles at `target` now for `0` frames, otherwise slews to it.
    fn steer(&mut self, target: f32, frames: usize) {
        if frames == 0 {
            *self = Envelope::at(target);
        } else {
            self.slew_to(target, frames);
        }
    }

    fn fade_out(&mut self, frames: usize) {
        self.retiring = true;
        self.slew_to(0.0, frames);
    }

    /// The level for the next device frame.
    fn next(&mut self) -> f32 {
        if self.left > 0 {
            self.left -= 1;
            self.level = if self.left == 0 {
                self.target
            } else {
                self.level + self.step
            };
        }
        self.level
    }

    fn is_finished(&self) -> bool {
        self.retiring && self.left == 0
    }
}

/// `|pan|` at which a stereo clip has fully narrowed to its channel mean.
const PAN_FULL_MONO: f32 = 0.02;

/// A voice's stereo position: an envelope stepped once per device frame, and the constant-power
/// channel gains cached for the level they were computed at, so the trig runs only when it moves.
#[derive(Clone, Copy, Debug)]
struct Panner {
    envelope: Envelope,
    gains: [f32; 2],
    /// The level `gains` belongs to; NaN until the first computation.
    gains_level: f32,
}

impl Panner {
    /// Settled at `pan` (`-1.0` hard left, `0.0` centre, `1.0` hard right).
    fn at(pan: f32) -> Panner {
        Panner {
            envelope: Envelope::at(pan),
            gains: [1.0, 1.0],
            gains_level: f32::NAN,
        }
    }

    #[cfg(test)]
    fn centred() -> Panner {
        Panner::at(0.0)
    }

    /// The left and right gains for `level`: a constant-power law scaled to unity at the centre, so
    /// hard left is `√2` on one channel and silence on the other.
    fn gains_at(&mut self, level: f32) -> [f32; 2] {
        if self.gains_level != level {
            let (sin, cos) =
                ludion_core::math::sin_cos((level + 1.0) * std::f32::consts::FRAC_PI_4);
            self.gains = [
                std::f32::consts::SQRT_2 * cos.max(0.0),
                std::f32::consts::SQRT_2 * sin,
            ];
            self.gains_level = level;
        }
        self.gains
    }
}

/// One playing sound: a shared, device-format sample buffer, a read cursor, and a gain envelope.
struct Voice {
    /// Interleaved samples at the device's rate and channel count (gain not yet applied; once
    /// applied it scales every channel alike).
    samples: Arc<Vec<f32>>,
    /// The next sample index to emit.
    pos: usize,
    /// The voice's gain envelope, stepped once per frame while mixing, before the bus master.
    envelope: Envelope,
    /// The voice's stereo position, stepped once per frame on a two-channel device only.
    pan: Panner,
    /// Which mixdown bus supplies the master gain multiplied in while mixing.
    category: AudioCategory,
    /// Whether the cursor wraps at the end of the buffer instead of the voice retiring there.
    looping: bool,
}

impl Voice {
    /// Which sound this voice plays, as the address of its buffer. Ids are compared only between
    /// voices alive at the same time, and two live `Arc`s share an address only if they share the
    /// buffer. Every voice of one file version clones the cache's one `Arc` (in [`resolve`]), so
    /// equal ids mean the same file version; a rewritten or re-decoded file is another sound.
    fn sound_id(&self) -> usize {
        Arc::as_ptr(&self.samples) as usize
    }

    /// Adds this voice's next `scratch.len()` samples (scaled by its envelope and by `master`, its
    /// bus's gain for each frame of the block) into `scratch`, advancing its cursor, and reports whether it is still playing. The
    /// envelope steps once per frame of `channels` samples. A looping voice wraps at the end of its
    /// buffer and plays until a fade-out ends; a one-shot stops at the end and retires. An empty
    /// buffer never plays — the guard that keeps a looping voice from spinning on nothing.
    ///
    /// On a two-channel device a frame whose pan level is off centre takes the constant-power
    /// path; every other frame (centre, mono, more than two channels) adds the sample unpanned, so a
    /// centred voice is bit-identical to a voice with no pan at all.
    fn advance_into(&mut self, scratch: &mut [f32], channels: usize, master: &[f32]) -> bool {
        if self.samples.is_empty() {
            return false;
        }
        for (index, frame) in scratch.chunks_mut(channels.max(1)).enumerate() {
            let level = self.envelope.next();
            if self.envelope.is_finished() {
                return false;
            }
            let gain = level * master[index];
            let pan = if channels == 2 {
                self.pan.envelope.next()
            } else {
                0.0
            };
            if pan != 0.0
                && let [left, right] = frame
            {
                let (Some(a), Some(b)) = (self.next_sample(), self.next_sample()) else {
                    return false;
                };
                let gains = self.pan.gains_at(pan);
                let narrow = (pan.abs() / PAN_FULL_MONO).min(1.0);
                let mid = (a + b) * 0.5;
                *left += (a + (mid - a) * narrow) * gain * gains[0];
                *right += (b + (mid - b) * narrow) * gain * gains[1];
                continue;
            }
            for slot in frame {
                let Some(sample) = self.next_sample() else {
                    return false;
                };
                *slot += sample * gain;
            }
        }
        self.looping || self.pos < self.samples.len()
    }

    /// The next sample, wrapping a looping voice at the end of its buffer; `None` when a one-shot
    /// has run out.
    fn next_sample(&mut self) -> Option<f32> {
        if self.pos >= self.samples.len() {
            if !self.looping {
                return None;
            }
            self.pos = 0;
        }
        let sample = self.samples[self.pos];
        self.pos += 1;
        Some(sample)
    }
}

/// A loop's identity on the callback side: `Copy`, so the callback never frees a `String`. The sink
/// thread interns a [`LoopId::Sound`] name to a number ([`Router::key`]); the table of names is
/// bounded by the game's distinct loop paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum LoopKey {
    Entity(EntityId),
    Sound(u32),
}

/// What the sink thread hands the real-time callback. Decoding and resampling have already happened
/// on the sink thread, so the callback only mixes and edits its loop table.
enum VoiceCommand {
    /// Start a voice that plays once.
    OneShot(Voice),
    /// Start, or update the gain of, the loop under this id (see [`Mixer::sync_loop`]).
    LoopSync { id: LoopKey, voice: Voice },
    /// Fade out and drop the loop under this id, if any.
    LoopStop(LoopKey),
    /// Fade out every loop and the music.
    StopAllLoops,
    /// Begin a music play: chunks of this generation are the ones to mix.
    MusicStart {
        generation: u32,
        gain: f32,
        /// Each stem's starting gain; the first `count` are live.
        stem_gains: [f32; MUSIC_STEMS_MAX],
        count: usize,
        /// Device frames of linear fade-in (`0` starts at full level).
        fade_in: usize,
    },
    /// Re-gain the music already playing: the gain slews to the new one over the ramp.
    MusicGain(f32),
    /// Slew one stem of the playing set to a new gain over `frames` device frames.
    MusicStemGain {
        index: usize,
        gain: f32,
        frames: usize,
    },
    /// Fade the music out over this many device frames (`0` stops it now).
    MusicStop { fade_out: usize },
    /// Slew one layer of a bus's runtime gain (`bus` indexes [`Bus::ALL`]) over `frames` device
    /// frames; `0` settles now.
    BusGain {
        bus: usize,
        layer: MixLayer,
        gain: f32,
        frames: usize,
    },
    /// Hold or release the buses whose bit (`1 << index`) is set in `mask`; `0` frames settles now.
    BusPause {
        mask: u8,
        paused: bool,
        frames: usize,
    },
}

/// The sink thread body: play through `output`, reopening it whenever it is lost, and route every
/// message until the channel disconnects. A device that is absent at launch is retried like a lost
/// one.
fn run<O: Output>(
    output: O,
    events: Receiver<AudioMessage>,
    content_root: PathBuf,
    gains: CategoryGains,
    caps: VoiceCaps,
    seconds_per_tick: f64,
) {
    // Dropping the playback at the end stops the device.
    let mut playback = Playback::start(
        output,
        content_root,
        gains,
        caps,
        seconds_per_tick,
        Instant::now(),
    );
    serve(&events, &mut playback);
}

/// The most commands the callback's queue holds. A full queue signals nothing: the callback drains
/// the whole queue on every render, so a burst beyond it waits in [`Router`]'s backlog (state
/// commands) or is shed (one-shots, past [`ONE_SHOT_HIGH_WATER`]). The backlog is bounded in time,
/// since it exists only until the stall verdict ([`STALL_AFTER`], or [`START_WITHIN`] before the
/// first render), and the bound caps the memory a stalled device can pin. A bounded queue's
/// receiver never frees a block, so the callback deallocates nothing here.
const COMMAND_CAPACITY: usize = 1024;

/// How long a stream that has rendered may go without rendering again before it counts as lost.
const STALL_AFTER: Duration = Duration::from_millis(500);

/// How long a stream may take to render for the first time before it counts as lost. Well past a
/// Bluetooth sink's start of about a second.
const START_WITHIN: Duration = Duration::from_secs(5);

/// How long a stream must keep rendering before the reopen wait starts over. At the longest wait,
/// so a device that cannot stay up that long keeps backing off and a flapping output costs one
/// warning and one note per wait, not per flap.
const HEALTHY_AFTER: Duration = RETRY_MAX;

/// The most queued commands at which a one-shot is still sent: half the queue. Past it a one-shot
/// could not be heard anyway (the callback admits at most `limits.total` arrivals and collapses
/// same-sound copies), and the rest of the queue stays free for the state commands (loops, music)
/// that must arrive.
const ONE_SHOT_HIGH_WATER: usize = COMMAND_CAPACITY / 2;

/// The wait before the first reopen attempt after a loss: long enough for a device that is
/// switching to settle, short enough that a blip is barely heard.
const RETRY_FIRST: Duration = Duration::from_millis(250);

/// The longest wait between reopen attempts. Retries never stop, so a device plugged back in an
/// hour later still plays.
const RETRY_MAX: Duration = Duration::from_secs(5);

/// The device's output format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeviceFormat {
    rate: u32,
    channels: u16,
}

/// The format a session that never opened a device prepares for, until a stream opens at its own.
const ASSUMED_FORMAT: DeviceFormat = DeviceFormat {
    rate: 48_000,
    channels: 2,
};

/// What a stream's callbacks tell the sink thread. The render callback bumps a counter and the
/// error callback records the first error; the sink thread only reads.
#[derive(Default)]
struct StreamHealth {
    /// Render callbacks begun.
    renders: AtomicU64,
    /// The first error cpal reported, if any.
    error: OnceLock<String>,
}

/// An output device the sink opens and starts streams on; the seam that lets reopening run under
/// test without cpal.
trait Output {
    /// What keeps a started stream playing; dropping it stops the stream.
    type Stream;

    /// Opens the current default output device.
    ///
    /// # Errors
    /// [`L5001`](code::AUDIO_DEVICE_UNAVAILABLE) when there is no usable device.
    fn open(&mut self) -> Result<DeviceFormat, Diagnostic>;

    /// Starts a stream on the opened device whose callback is `mixer`.
    ///
    /// # Errors
    /// [`L5001`](code::AUDIO_DEVICE_UNAVAILABLE) when the stream cannot be built or started.
    fn start(&mut self, mixer: Mixer) -> Result<Self::Stream, Diagnostic>;
}

/// The real device, through cpal.
struct CpalOutput {
    device: Option<(cpal::Device, cpal::SupportedStreamConfig)>,
}

impl Output for CpalOutput {
    type Stream = cpal::Stream;

    fn open(&mut self) -> Result<DeviceFormat, Diagnostic> {
        let (device, supported) = open_output()?;
        let format = DeviceFormat {
            rate: supported.sample_rate().0,
            channels: supported.channels(),
        };
        self.device = Some((device, supported));
        Ok(format)
    }

    fn start(&mut self, mixer: Mixer) -> Result<cpal::Stream, Diagnostic> {
        let (device, supported) = self
            .device
            .as_ref()
            .ok_or_else(|| device_unavailable("no output device was opened"))?;
        let config: cpal::StreamConfig = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(device, &config, mixer),
            SampleFormat::I16 => build_stream::<i16>(device, &config, mixer),
            SampleFormat::U16 => build_stream::<u16>(device, &config, mixer),
            other => {
                return Err(device_unavailable(&format!(
                    "unsupported output sample format {other:?}"
                )));
            }
        }
        .map_err(|error| {
            device_unavailable(&format!("could not build the output stream: {error}"))
        })?;
        stream.play().map_err(|error| {
            device_unavailable(&format!("could not start the output stream: {error}"))
        })?;
        Ok(stream)
    }
}

/// One started stream and what the sink thread knows of its health.
struct Live<S> {
    _stream: S,
    commands: Sender<VoiceCommand>,
    health: Arc<StreamHealth>,
    /// `health.renders` at the last wake that saw it move.
    renders: u64,
    /// When `renders` last moved (or the stream started).
    beat_at: Instant,
    /// When the stream was first seen rendering.
    rendering_since: Option<Instant>,
}

/// A freshly started stream, the format it opened at and the sender of its music ring.
struct Connected<S> {
    live: Live<S>,
    format: DeviceFormat,
    music: Sender<MusicChunk>,
}

/// Opens the device and starts a stream on it: a fresh mixer over a fresh command queue and music
/// ring.
fn connect<O: Output>(
    output: &mut O,
    gains: CategoryGains,
    caps: VoiceCaps,
    now: Instant,
) -> Result<Connected<O::Stream>, Diagnostic> {
    let format = output.open()?;
    let (commands, command_rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
    let (music_tx, music_rx) = crossbeam_channel::bounded(MUSIC_RING_CHUNKS);
    let mixer = Mixer::new(
        command_rx,
        music_rx,
        format.channels,
        format.rate,
        gains,
        voice_limits(caps),
    );
    let health = mixer.health.clone();
    let stream = output.start(mixer)?;
    let live = Live {
        _stream: stream,
        commands,
        health,
        renders: 0,
        beat_at: now,
        rendering_since: None,
    };
    Ok(Connected {
        live,
        format,
        music: music_tx,
    })
}

/// The sink thread's whole state: the output, the router, and the stream when one is playing. A
/// lost stream is dropped and the default output reopened with capped backoff; the router replays
/// the declared loops and the music into the new one.
struct Playback<O: Output> {
    output: O,
    router: Router,
    gains: CategoryGains,
    caps: VoiceCaps,
    /// `None` while the output is lost.
    live: Option<Live<O::Stream>>,
    /// When the next reopen is due, while lost.
    retry_at: Instant,
    /// The wait the next loss or failed attempt schedules.
    delay: Duration,
    /// Whether the last stream was reported as playing: a loss reports `L5002` only then, and a
    /// reopened stream's first render reports once.
    reported_playing: bool,
    /// The code the output has been down since, named by the note that reports its recovery.
    down_since: &'static str,
    /// Lines for the host to print.
    reports: Vec<String>,
}

impl<O: Output> Playback<O> {
    /// Opens the device and starts playing. When it cannot, the session starts lost: `L5001` is
    /// reported, the router still tracks every message, and the reopen path retries with backoff.
    fn start(
        mut output: O,
        content_root: PathBuf,
        gains: CategoryGains,
        caps: VoiceCaps,
        seconds_per_tick: f64,
        now: Instant,
    ) -> Self {
        match connect(&mut output, gains, caps, now) {
            Ok(Connected {
                live,
                format,
                music,
            }) => Playback {
                output,
                router: Router::new(
                    content_root,
                    format.rate,
                    format.channels,
                    seconds_per_tick,
                    caps.total,
                    music,
                ),
                gains,
                caps,
                live: Some(live),
                retry_at: now,
                delay: RETRY_FIRST,
                reported_playing: true,
                down_since: "L5002",
                reports: Vec::new(),
            },
            Err(diagnostic) => {
                // `route` never sends on this ring, and `retarget` swaps in the reopened stream's.
                let (music, _) = crossbeam_channel::bounded(MUSIC_RING_CHUNKS);
                Playback {
                    output,
                    router: Router::new(
                        content_root,
                        ASSUMED_FORMAT.rate,
                        ASSUMED_FORMAT.channels,
                        seconds_per_tick,
                        caps.total,
                        music,
                    ),
                    gains,
                    caps,
                    live: None,
                    retry_at: now + RETRY_FIRST,
                    delay: RETRY_FIRST * 2,
                    reported_playing: false,
                    down_since: "L5001",
                    reports: vec![diagnostic.to_string()],
                }
            }
        }
    }

    /// One wake of the sink thread: judge the stream's health, reopen it when lost and due, then
    /// route `message`.
    fn wake(&mut self, message: Option<AudioMessage>, now: Instant) {
        if let Some(live) = &mut self.live {
            let mut cause = None;
            if let Some(error) = live.health.error.get() {
                cause = Some(error.clone());
            } else {
                let renders = live.health.renders.load(Ordering::Relaxed);
                if renders != live.renders {
                    live.renders = renders;
                    live.beat_at = now;
                    let since = *live.rendering_since.get_or_insert(now);
                    if !self.reported_playing {
                        self.reported_playing = true;
                        self.reports.push(format!(
                            "note: the audio output reopened after {} ({} Hz, {} ch); loops and music resumed",
                            self.down_since, self.router.out_rate, self.router.out_channels
                        ));
                    }
                    if now.saturating_duration_since(since) >= HEALTHY_AFTER {
                        self.delay = RETRY_FIRST;
                    }
                } else if now.saturating_duration_since(live.beat_at) >= STALL_AFTER && renders > 0
                {
                    cause = Some(format!("no callback for {} ms", STALL_AFTER.as_millis()));
                } else if renders == 0
                    && now.saturating_duration_since(live.beat_at) >= START_WITHIN
                {
                    cause = Some(format!(
                        "no first callback within {} s",
                        START_WITHIN.as_secs()
                    ));
                }
            }
            if let Some(cause) = cause {
                self.lose(&cause, now);
            }
        }
        if self.live.is_none() && now >= self.retry_at {
            match connect(&mut self.output, self.gains, self.caps, now) {
                Err(_) => self.back_off(now),
                Ok(Connected {
                    live,
                    format,
                    music,
                }) => {
                    self.router.retarget(format, music);
                    let replayed = self.router.replay(&live.commands);
                    self.live = Some(live);
                    if !replayed {
                        self.lose("the stream's callback is gone", now);
                    }
                }
            }
        }
        if let Some(live) = &self.live {
            // One-shots above the high-water mark are shed; the room is for the state commands.
            let message = message.filter(|message| {
                !matches!(message, AudioMessage::OneShot(_))
                    || live.commands.len() < ONE_SHOT_HIGH_WATER
            });
            if !self.router.step(message, &live.commands) {
                self.lose("the stream's callback is gone", now);
            }
        } else if let Some(message) = message {
            // Lost: state messages still update the router, and any command is dropped.
            let _ = self.router.route(message);
        }
    }

    /// Drops the stream, reports `L5002` if it had been reported as playing, and schedules a reopen.
    fn lose(&mut self, cause: &str, now: Instant) {
        self.live = None;
        // The replay into the next stream covers the declared state; stale commands must not reach it.
        self.router.backlog.clear();
        if self.reported_playing {
            self.down_since = "L5002";
            self.reports.push(stream_failed(cause).to_string());
            self.reported_playing = false;
        }
        self.back_off(now);
    }

    /// Schedules the next attempt `delay` from now and doubles the delay up to [`RETRY_MAX`].
    fn back_off(&mut self, now: Instant) {
        self.retry_at = now + self.delay;
        self.delay = (self.delay * 2).min(RETRY_MAX);
    }

    /// How long the sink thread may wait for a message before it must wake anyway.
    fn wait(&self, now: Instant) -> Duration {
        if self.live.is_none() {
            self.retry_at.saturating_duration_since(now)
        } else if self.router.feed.is_feeding() || !self.router.backlog.is_empty() {
            MUSIC_REFILL
        } else {
            STALL_AFTER
        }
    }
}

/// The sink thread's state: what turns one [`AudioMessage`] into at most one [`VoiceCommand`] for
/// the callback, and keeps the music ring topped up. It holds no device or stream, so it runs under
/// test.
struct Router {
    content_root: PathBuf,
    out_rate: u32,
    out_channels: u16,
    /// Every decode attempt this session, keyed by sound path — successes and failures alike (see
    /// [`resolve`]), each stamped with the file it was made from.
    cache: HashMap<String, Decoded>,
    /// How long a cached file's stamp is trusted before it is read again.
    recheck: Duration,
    /// Loop-sound names interned to [`LoopKey::Sound`] numbers.
    loop_names: HashMap<String, u32>,
    /// The loops the engine declared and has not stopped, replayed into a reopened stream. The
    /// engine keeps no mirror; this is the one. A loop past [`Router::voice_total`] waits here
    /// (`playing` unset), since the sink thread can hold any number and the callback's table cannot;
    /// it starts, in declaration order, when a loop stops.
    declared: HashMap<LoopKey, Declared>,
    /// The most live voices the callback holds: [`VoiceCaps::total`].
    voice_total: usize,
    /// The order the next new loop key is declared in.
    next_seq: u64,
    /// The buffers the cache let go of while a voice may still hold them, so the callback never
    /// drops the last reference to one. Pruned on every step, once no voice holds a buffer.
    superseded: Vec<Arc<Vec<f32>>>,
    feed: MusicFeed,
    /// State commands waiting for room in a full queue, in order. A render makes room; only a
    /// stalled stream keeps them until the stall verdict.
    backlog: VecDeque<VoiceCommand>,
    /// How many declared loops the callback holds a voice for.
    playing: usize,
    /// Each bus's runtime state, indexed like [`Bus::ALL`], replayed into a reopened stream.
    buses: [RouterBus; BUS_COUNT],
}

/// The runtime mix the router remembers for one bus.
#[derive(Clone, Copy, PartialEq)]
struct RouterBus {
    user: f32,
    game: f32,
    paused: bool,
}

impl RouterBus {
    const DEFAULT: RouterBus = RouterBus {
        user: 1.0,
        game: 1.0,
        paused: false,
    };
}

/// A declared loop's last sync.
struct Declared {
    sound: String,
    gain: f32,
    pan: f32,
    category: AudioCategory,
    /// The order the loop's key was first declared in, which decides who starts first.
    seq: u64,
    /// The callback holds a voice under this key.
    playing: bool,
}

/// A loop voice on `samples`, ready to play from the top and rising to `gain` over `fade_in`
/// frames (`0` starts at `gain`), settled at `pan`.
fn looping_voice(
    samples: Arc<Vec<f32>>,
    gain: f32,
    pan: f32,
    category: AudioCategory,
    fade_in: usize,
) -> Voice {
    Voice {
        samples,
        pos: 0,
        envelope: Envelope::rising(gain, fade_in),
        pan: Panner::at(pan),
        category,
        looping: true,
    }
}

/// A pan the mixer can use: non-finite is centre, anything else clamps to `[-1, 1]`.
fn sane_pan(pan: f32) -> f32 {
    if pan.is_finite() {
        pan.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// The length of a per-voice level change at `rate`, in device frames.
fn ramp_frames(rate: u32) -> usize {
    (rate as usize * RAMP_MS / 1000).max(1)
}

impl Router {
    fn new(
        content_root: PathBuf,
        out_rate: u32,
        out_channels: u16,
        seconds_per_tick: f64,
        voice_total: usize,
        music: Sender<MusicChunk>,
    ) -> Router {
        let feed = MusicFeed::new(
            content_root.clone(),
            out_rate,
            out_channels,
            seconds_per_tick,
            music,
        );
        Router {
            content_root,
            out_rate,
            out_channels,
            cache: HashMap::new(),
            recheck: STAMP_RECHECK,
            loop_names: HashMap::new(),
            declared: HashMap::new(),
            voice_total,
            next_seq: 0,
            superseded: Vec::new(),
            feed,
            backlog: VecDeque::new(),
            playing: 0,
            buses: [RouterBus::DEFAULT; BUS_COUNT],
        }
    }

    /// Sends `command` behind any backlog, or queues it there when the queue is full. False only
    /// when the callback is gone.
    fn forward(&mut self, commands: &Sender<VoiceCommand>, command: VoiceCommand) -> bool {
        if !self.backlog.is_empty() {
            self.backlog.push_back(command);
            return true;
        }
        match commands.try_send(command) {
            Ok(()) => true,
            Err(TrySendError::Full(command)) => {
                self.backlog.push_back(command);
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Moves the backlog into the queue, oldest first, until it is full. False only when the
    /// callback is gone.
    fn flush_backlog(&mut self, commands: &Sender<VoiceCommand>) -> bool {
        while let Some(command) = self.backlog.pop_front() {
            match commands.try_send(command) {
                Ok(()) => {}
                Err(TrySendError::Full(command)) => {
                    self.backlog.push_front(command);
                    return true;
                }
                Err(TrySendError::Disconnected(_)) => return false,
            }
        }
        true
    }

    /// A voice for the declared loop `key`, rising over `fade_in` frames; `None` when its sound
    /// does not load.
    fn declared_voice(&mut self, key: LoopKey, fade_in: usize) -> Option<Voice> {
        let declared = &self.declared[&key];
        let samples = resolve(
            &mut self.cache,
            &mut self.superseded,
            &self.content_root,
            &declared.sound,
            self.out_rate,
            self.out_channels,
            self.recheck,
        )?;
        Some(looping_voice(
            samples,
            declared.gain,
            declared.pan,
            declared.category,
            fade_in,
        ))
    }

    /// Starts the waiting loops, oldest declaration first, while the total has room; each fades in,
    /// since the game did not ask for this start. False only when the callback is gone.
    fn start_waiting(&mut self, commands: &Sender<VoiceCommand>) -> bool {
        debug_assert_eq!(
            self.playing,
            self.declared.values().filter(|d| d.playing).count()
        );
        let mut free = self.voice_total.saturating_sub(self.playing);
        if free == 0 {
            return true;
        }
        let mut waiting: Vec<(u64, LoopKey)> = self
            .declared
            .iter()
            .filter(|(_, declared)| !declared.playing)
            .map(|(&key, declared)| (declared.seq, key))
            .collect();
        waiting.sort_unstable_by_key(|&(seq, _)| seq);
        let fade_in = ramp_frames(self.out_rate);
        for (_, id) in waiting {
            if free == 0 {
                break;
            }
            let Some(voice) = self.declared_voice(id, fade_in) else {
                continue;
            };
            if !self.forward(commands, VoiceCommand::LoopSync { id, voice }) {
                return false;
            }
            if let Some(declared) = self.declared.get_mut(&id) {
                declared.playing = true;
                self.playing += 1;
            }
            free -= 1;
        }
        true
    }

    /// The callback-side key for `id`; a sound name is interned on first sight, and its `String`
    /// drops here on the sink thread.
    fn key(&mut self, id: LoopId) -> LoopKey {
        match id {
            LoopId::Entity(entity) => LoopKey::Entity(entity),
            LoopId::Sound(name) => {
                let next = self.loop_names.len() as u32;
                LoopKey::Sound(*self.loop_names.entry(name).or_insert(next))
            }
        }
    }

    /// The command one message becomes, or `None` (unloadable sound, unplayable track).
    fn route(&mut self, message: AudioMessage) -> Option<VoiceCommand> {
        let command = match message {
            AudioMessage::OneShot(event) => {
                let samples = resolve(
                    &mut self.cache,
                    &mut self.superseded,
                    &self.content_root,
                    &event.sound,
                    self.out_rate,
                    self.out_channels,
                    self.recheck,
                )?;
                VoiceCommand::OneShot(Voice {
                    samples,
                    pos: 0,
                    envelope: Envelope::at(event.gain),
                    pan: Panner::at(sane_pan(event.pan)),
                    category: event.category,
                    looping: false,
                })
            }
            AudioMessage::LoopSync {
                id,
                sound,
                gain,
                pan,
                category,
            } => {
                let pan = sane_pan(pan);
                let key = self.key(id);
                let (seq, playing) = match self.declared.get(&key) {
                    Some(declared) => (declared.seq, declared.playing),
                    None => {
                        self.next_seq += 1;
                        (self.next_seq - 1, false)
                    }
                };
                // Declared before the resolve: a broken sound plays at a replay once it is fixed,
                // and a loop past the total waits here until a slot frees.
                self.declared.insert(
                    key,
                    Declared {
                        sound,
                        gain,
                        pan,
                        category,
                        seq,
                        playing,
                    },
                );
                if !playing && self.playing >= self.voice_total {
                    return None;
                }
                let voice = self.declared_voice(key, 0)?;
                if let Some(declared) = self.declared.get_mut(&key)
                    && !declared.playing
                {
                    declared.playing = true;
                    self.playing += 1;
                }
                VoiceCommand::LoopSync { id: key, voice }
            }
            AudioMessage::LoopStop { id } => {
                let key = self.key(id);
                if self.declared.remove(&key).is_some_and(|d| d.playing) {
                    self.playing -= 1;
                }
                VoiceCommand::LoopStop(key)
            }
            AudioMessage::StopAllLoops => {
                self.declared.clear();
                self.playing = 0;
                self.feed.stop(0);
                for bus in &mut self.buses {
                    bus.game = 1.0;
                    bus.paused = false;
                }
                VoiceCommand::StopAllLoops
            }
            AudioMessage::MusicPlay {
                track,
                gain,
                looping,
                fade_frames,
            } => self.feed.play(&track, gain, looping, fade_frames)?,
            AudioMessage::MusicSet {
                id,
                stems,
                fade_frames,
            } => self.feed.set(&id, &stems, fade_frames)?,
            AudioMessage::MusicStemGain {
                stem,
                gain,
                fade_frames,
            } => self.feed.stem_gain(&stem, gain, fade_frames)?,
            AudioMessage::MusicStop { fade_frames } => self.feed.stop(fade_frames),
            AudioMessage::BusGain {
                bus,
                layer,
                gain,
                fade_frames,
            } => {
                let gain = if gain.is_finite() { gain.max(0.0) } else { 0.0 };
                let record = &mut self.buses[bus as usize];
                match layer {
                    MixLayer::User => record.user = gain,
                    MixLayer::Game => record.game = gain,
                }
                VoiceCommand::BusGain {
                    bus: bus as usize,
                    layer,
                    gain,
                    frames: self.mix_frames(fade_frames),
                }
            }
            AudioMessage::BusPause {
                buses,
                paused,
                fade_frames,
            } => {
                let mut mask = 0u8;
                for bus in buses {
                    mask |= 1 << bus as usize;
                    self.buses[bus as usize].paused = paused;
                }
                if mask == 0 {
                    return None;
                }
                VoiceCommand::BusPause {
                    mask,
                    paused,
                    frames: self.mix_frames(fade_frames),
                }
            }
        };
        Some(command)
    }

    /// A mix change's length in device frames: the ticks asked for, never shorter than the click-free
    /// ramp.
    fn mix_frames(&self, ticks: u32) -> usize {
        self.feed
            .device_frames(ticks)
            .max(ramp_frames(self.out_rate))
    }

    /// Sends every non-default bus state, settled, so a reopened device keeps the player's volume.
    /// False only when the callback is gone.
    fn replay_buses(&mut self, commands: &Sender<VoiceCommand>) -> bool {
        let mut sent = true;
        for (index, record) in self.buses.into_iter().enumerate() {
            if record == RouterBus::DEFAULT {
                continue;
            }
            for (layer, gain) in [(MixLayer::User, record.user), (MixLayer::Game, record.game)] {
                if gain != 1.0 {
                    let command = VoiceCommand::BusGain {
                        bus: index,
                        layer,
                        gain,
                        frames: 0,
                    };
                    sent &= self.forward(commands, command);
                }
            }
            if record.paused {
                let command = VoiceCommand::BusPause {
                    mask: 1 << index,
                    paused: true,
                    frames: 0,
                };
                sent &= self.forward(commands, command);
            }
        }
        sent
    }

    /// One wake of the sink thread: forward `message`'s command, then top the music ring up. The
    /// refill comes after the send so a decode never delays a one-shot, and runs on every wake so a
    /// message that routes to nothing cannot starve the ring. A loop stop that was sent also starts
    /// the waiting loops it made room for. A command the full queue cannot take waits in the backlog,
    /// flushed first on the next wake. False only when the callback is gone: the stream is lost, and
    /// the refill is skipped.
    fn step(&mut self, message: Option<AudioMessage>, commands: &Sender<VoiceCommand>) -> bool {
        self.superseded
            .retain(|buffer| Arc::strong_count(buffer) > 1);
        if !self.flush_backlog(commands) {
            return false;
        }
        if let Some(message) = message {
            let is_stop = matches!(message, AudioMessage::LoopStop { .. });
            if let Some(command) = self.route(message) {
                if !self.forward(commands, command) {
                    return false;
                }
                if is_stop && !self.start_waiting(commands) {
                    return false;
                }
            }
        }
        self.feed.refill();
        true
    }

    /// Adopts a reopened device: another format drops the buffers decoded for the old one, and the
    /// music feed moves to the new ring.
    fn retarget(&mut self, format: DeviceFormat, music: Sender<MusicChunk>) {
        if (format.rate, format.channels) != (self.out_rate, self.out_channels) {
            self.out_rate = format.rate;
            self.out_channels = format.channels;
            let superseded = &mut self.superseded;
            self.cache.retain(|_, decoded| match decoded.buffer.take() {
                Some(buffer) => {
                    superseded.push(buffer);
                    false
                }
                None => true,
            });
        }
        self.feed.retarget(format.rate, format.channels, music);
    }

    /// Sends the declared loops (the oldest, when more than the total are declared) and the music
    /// into a freshly opened stream's queue, then fills its ring. False only when the callback is gone.
    fn replay(&mut self, commands: &Sender<VoiceCommand>) -> bool {
        for declared in self.declared.values_mut() {
            declared.playing = false;
        }
        self.playing = 0;
        let mut sent = self.replay_buses(commands);
        sent &= self.start_waiting(commands);
        if let Some(resume) = self.feed.resume(ramp_frames(self.out_rate)) {
            sent &= self.forward(commands, resume);
        }
        self.feed.refill();
        sent
    }
}

/// The sink thread's loop: wake on events, or when the playback needs to (a reopen is due, a track
/// feeds, a stall is to be judged), and hand each to `playback`, until the channel disconnects.
fn serve<O: Output>(events: &Receiver<AudioMessage>, playback: &mut Playback<O>) {
    loop {
        for report in playback.reports.drain(..) {
            eprintln!("{report}");
        }
        let message = match events.recv_timeout(playback.wait(Instant::now())) {
            Ok(message) => Some(message),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        playback.wake(message, Instant::now());
    }
}

/// A file's identity for cache purposes: its length and modification time, or `None` when its
/// metadata cannot be read at all (a missing file stamps the same way every time, which is what
/// keeps a broken reference from being re-opened once per message).
type Stamp = Option<(u64, Option<SystemTime>)>;

/// How long a file's stamp is trusted before the sink reads it again. A stamp is a `stat`, and a
/// loop or a burst of one-shots would otherwise pay one per message; a file rewritten under a
/// running host is picked up by the first message after this window.
const STAMP_RECHECK: Duration = Duration::from_millis(500);

/// A [`Stamp`] and when it was read.
struct Stamped {
    stamp: Stamp,
    read_at: Instant,
}

impl Stamped {
    fn read(path: &Path) -> Stamped {
        Stamped {
            stamp: stamp_of(path),
            read_at: Instant::now(),
        }
    }

    /// Whether the file `name` under `root` still has this stamp. Inside the `recheck` window the
    /// answer is yes without touching the filesystem (`Duration::ZERO` is always due); when due, the
    /// file is stamped again and an unchanged one restarts the window.
    fn is_current(&mut self, root: &Path, name: &str, recheck: Duration) -> bool {
        if self.read_at.elapsed() < recheck {
            return true;
        }
        let fresh = Stamped::read(&root.join(name));
        if fresh.stamp != self.stamp {
            return false;
        }
        self.read_at = fresh.read_at;
        true
    }
}

/// One decode attempt, and the file it was made from.
struct Decoded {
    /// The stamp the attempt read. A different one means the file changed underneath us.
    stamp: Stamped,
    /// The decoded buffer, or `None` for a failed decode — cached exactly like a success, so a bad
    /// `sound` costs one open per file version rather than one per message.
    buffer: Option<Arc<Vec<f32>>>,
}

/// Resolves a sound to its decoded, device-format buffer, decoding it on first use and on every
/// change of the file underneath.
///
/// `None` for an unloadable sound — a broken reference is skipped, never a failure. The cache holds
/// failures too, so the warning appears once per file version, not once per message; rewriting a
/// sound (a regenerated WAV under `ludion serve`) changes its stamp, so the first message after
/// `recheck` picks the new bytes up — a fixed file starts playing without a restart. A buffer the
/// new decode replaces goes to `superseded`, since a voice may still hold it.
fn resolve(
    cache: &mut HashMap<String, Decoded>,
    superseded: &mut Vec<Arc<Vec<f32>>>,
    content_root: &Path,
    sound: &str,
    out_rate: u32,
    out_channels: u16,
    recheck: Duration,
) -> Option<Arc<Vec<f32>>> {
    if let Some(decoded) = cache.get_mut(sound)
        && decoded.stamp.is_current(content_root, sound, recheck)
    {
        return decoded.buffer.clone();
    }
    let stamp = Stamped::read(&content_root.join(sound));
    let buffer = match load_wav(ludion_assets::AssetRoot::content(content_root), sound) {
        Ok(pcm) => Some(Arc::new(to_device_buffer(&pcm, out_rate, out_channels))),
        Err(diagnostic) => {
            eprintln!("{}", skipped(diagnostic, sound));
            None
        }
    };
    let replaced = cache.insert(
        sound.to_string(),
        Decoded {
            stamp,
            buffer: buffer.clone(),
        },
    );
    superseded.extend(replaced.and_then(|decoded| decoded.buffer));
    buffer
}

/// The warning for a sound the sink will not play: the decode diagnostic plus the consequence.
fn skipped(diagnostic: Diagnostic, sound: &str) -> Diagnostic {
    diagnostic.append_help(format!(
        "audio playback skipped `{sound}`; the run continues"
    ))
}

/// Whether the sink could play `message`: the one rule the sink and the testkit share.
///
/// A [`OneShot`](AudioMessage::OneShot) or [`LoopSync`](AudioMessage::LoopSync) must decode as a WAV
/// under `content_root` (`L1101` unreadable, `L1104` undecodable); a
/// [`MusicPlay`](AudioMessage::MusicPlay) must open as Ogg Vorbis (`L5003`, header-level: a failure
/// mid-stream shows only on a device); every stem of a [`MusicSet`](AudioMessage::MusicSet) must
/// open too (`L5003`) and match the first one's sample rate and length (`L5006`, the first
/// `L5003` winning). Every other message is `Ok`. The file is read on every call;
/// the sink's own cache memoizes by file version.
///
/// ```
/// use std::path::Path;
/// use ludion_audio::check_message;
/// use ludion_core::audio::{AudioEvent, AudioMessage};
///
/// let typo = AudioMessage::OneShot(AudioEvent::new("sounds/typo.wav", 1.0));
/// let error = check_message(Path::new("."), &typo).unwrap_err();
/// assert_eq!(error.code.as_str(), "L1101");
/// assert!(check_message(Path::new("."), &AudioMessage::StopAllLoops).is_ok());
/// ```
///
/// # Errors
/// The decode diagnostic of the named file.
pub fn check_message(content_root: &Path, message: &AudioMessage) -> Result<(), Diagnostic> {
    match message {
        AudioMessage::OneShot(AudioEvent { sound, .. }) | AudioMessage::LoopSync { sound, .. } => {
            load_wav(ludion_assets::AssetRoot::content(content_root), sound).map(drop)
        }
        AudioMessage::MusicPlay { track, .. } => music::check_track(content_root, track),
        AudioMessage::MusicSet { id, stems, .. } => music::check_set(content_root, id, stems),
        _ => Ok(()),
    }
}

/// Reads `path`'s cache stamp. Off the real-time callback and outside every determinism gate, so
/// filesystem metadata is fair game here in a way it never is sim-side.
fn stamp_of(path: &Path) -> Stamp {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()))
}

/// Opens the default output device and its default config.
///
/// # Errors
/// [`L5001`](code::AUDIO_DEVICE_UNAVAILABLE) if there is no default output device, or its default
/// configuration cannot be queried.
fn open_output() -> Result<(cpal::Device, cpal::SupportedStreamConfig), Diagnostic> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| device_unavailable("no default output device"))?;
    let supported = device
        .default_output_config()
        .map_err(|error| device_unavailable(&format!("no default output config: {error}")))?;
    Ok((device, supported))
}

/// How many mixdown buses there are: [`Bus::ALL`].
const BUS_COUNT: usize = Bus::ALL.len();

/// One bus on the callback side: the authored master and the runtime layers that multiply it.
/// `pause` is `1.0` running and `0.0` held; the bus is *held* only once it has reached `0.0`, so a
/// pause fades.
struct BusState {
    authored: f32,
    user: Envelope,
    game: Envelope,
    pause: Envelope,
    paused: bool,
}

impl BusState {
    fn new(authored: f32) -> BusState {
        BusState {
            authored,
            user: Envelope::at(1.0),
            game: Envelope::at(1.0),
            pause: Envelope::at(1.0),
            paused: false,
        }
    }

    /// Whether the bus's pause fade has finished, so its voices are skipped rather than mixed.
    fn is_held(&self) -> bool {
        self.paused && self.pause.left == 0 && self.pause.level == 0.0
    }
}

/// The real-time callback's state: the voices, the loop table and the music player. It holds no
/// device, so it runs under test.
///
/// **Live vs fading.** A *live* voice is in the loop table, or in `voices` and not retiring; only
/// live voices count against `limits`. A voice that was stopped or stolen keeps fading in `voices`
/// but has already given its slot away, so `voices` holds at most `2 × total`: when it is full the
/// fading voice with the lowest level is cut early. Loops are never stolen — a manual loop sync is
/// never re-sent, and an emitter's loop would restart from the top. The router sends no loop past
/// `total` (it holds the rest until a loop stops), so the refusal in [`Mixer::sync_loop`] is a
/// safety net.
///
/// **Real-time safety.** [`Mixer::new`] reserves every container the callback writes to, and
/// [`Mixer::render`] never grows one.
struct Mixer {
    commands: Receiver<VoiceCommand>,
    /// Each bus's authored gain and runtime layers, indexed like [`Bus::ALL`].
    buses: [BusState; BUS_COUNT],
    /// The per-frame effective gain of every bus for the block being mixed: bus `b` owns
    /// `b * MIX_BLOCK_FRAMES ..` of it.
    bus_levels: Vec<f32>,
    /// In start order — remove with `retain_mut`/`remove`, never `swap_remove`, so "oldest" is the
    /// first live voice.
    voices: Vec<Voice>,
    arriving: Vec<Voice>,
    loops: HashMap<LoopKey, Voice>,
    scratch: Vec<f32>,
    music: MusicPlayer,
    channels: usize,
    ramp_frames: usize,
    limits: VoiceLimits,
    health: Arc<StreamHealth>,
}

impl Mixer {
    fn new(
        commands: Receiver<VoiceCommand>,
        music: Receiver<MusicChunk>,
        channels: u16,
        rate: u32,
        gains: CategoryGains,
        limits: VoiceLimits,
    ) -> Mixer {
        let channels = usize::from(channels).max(1);
        Mixer {
            commands,
            buses: [
                BusState::new(1.0),
                BusState::new(gains.sfx),
                BusState::new(gains.ambient),
                BusState::new(gains.gain_for(AudioCategory::Voice)),
                BusState::new(gains.gain_for(AudioCategory::Ui)),
                BusState::new(gains.music),
            ],
            bus_levels: vec![0.0; BUS_COUNT * MIX_BLOCK_FRAMES],
            voices: Vec::with_capacity(2 * limits.total),
            arriving: Vec::with_capacity(limits.total),
            // Twice the live bound: removals leave tombstones, and a table at that capacity
            // rehashes in place instead of reallocating.
            loops: HashMap::with_capacity(2 * limits.total),
            scratch: Vec::with_capacity(MIX_BLOCK_FRAMES * channels),
            music: MusicPlayer::new(music, channels, ramp_frames(rate)),
            channels,
            ramp_frames: ramp_frames(rate),
            limits,
            health: Arc::new(StreamHealth::default()),
        }
    }

    /// Applies one command from the sink thread.
    fn apply(&mut self, command: VoiceCommand) {
        match command {
            VoiceCommand::OneShot(voice) => {
                collapse_simultaneous(&mut self.arriving, voice, self.limits.total);
            }
            VoiceCommand::LoopSync { id, voice } => self.sync_loop(id, voice),
            VoiceCommand::LoopStop(id) => {
                if let Some(voice) = self.loops.remove(&id) {
                    self.retire(voice);
                }
            }
            VoiceCommand::StopAllLoops => {
                let held = self.held_buses();
                // A voice held on a paused bus is silent anyway: it drops rather than waiting for a
                // resume that the rebuilt world never sends.
                self.voices.retain(|voice| !is_held(&held, voice));
                // The table drains in per-process order, so which fading voice survives a full
                // `voices` varies between runs.
                let room = 2 * self.limits.total;
                for (_, voice) in self.loops.drain() {
                    if !is_held(&held, &voice) {
                        retire(&mut self.voices, room, self.ramp_frames, voice);
                    }
                }
                self.music.apply(VoiceCommand::MusicStop {
                    fade_out: self.ramp_frames,
                });
                for bus in &mut self.buses {
                    bus.game.slew_to(1.0, self.ramp_frames);
                    bus.paused = false;
                    bus.pause.slew_to(1.0, self.ramp_frames);
                }
            }
            VoiceCommand::BusGain {
                bus,
                layer,
                gain,
                frames,
            } => {
                let state = &mut self.buses[bus];
                match layer {
                    MixLayer::User => state.user.steer(gain, frames),
                    MixLayer::Game => state.game.steer(gain, frames),
                }
            }
            VoiceCommand::BusPause {
                mask,
                paused,
                frames,
            } => {
                for (index, state) in self.buses.iter_mut().enumerate() {
                    if mask & (1 << index) != 0 {
                        state.paused = paused;
                        state.pause.steer(if paused { 0.0 } else { 1.0 }, frames);
                    }
                }
            }
            music_command => self.music.apply(music_command),
        }
    }

    /// Which buses hold their voices now: a bus is held when it or the master is.
    fn held_buses(&self) -> [bool; BUS_COUNT] {
        let master = self.buses[Bus::Master as usize].is_held();
        let mut held = [master; BUS_COUNT];
        for (flag, state) in held.iter_mut().zip(&self.buses) {
            *flag |= state.is_held();
        }
        held
    }

    /// Steps every bus's envelopes one device frame at a time into `bus_levels`:
    /// `authored x user x game x pause`, then each non-master bus times the master's.
    fn step_buses(&mut self, frames: usize) {
        for frame in 0..frames {
            for (index, state) in self.buses.iter_mut().enumerate() {
                self.bus_levels[index * MIX_BLOCK_FRAMES + frame] =
                    state.authored * state.user.next() * state.game.next() * state.pause.next();
            }
            let master = self.bus_levels[frame];
            for index in 1..BUS_COUNT {
                self.bus_levels[index * MIX_BLOCK_FRAMES + frame] *= master;
            }
        }
    }

    fn retire(&mut self, voice: Voice) {
        let room = 2 * self.limits.total;
        retire(&mut self.voices, room, self.ramp_frames, voice);
    }

    /// Applies a loop sync to the table: a loop already running this exact sound keeps its cursor and
    /// slews to the new gain — that is what makes resending a sync a declarative update rather than
    /// an audible restart. On another bus it crossfades: a copy of the running voice, at the same
    /// cursor, fades out on the old bus while the loop rises on the new one. A different sound under
    /// an existing id plays from the top while the old voice fades out. A new id at the total cap
    /// steals a one-shot, or is refused when every live voice is a loop; the router never sends one
    /// past the total, so the refusal is a safety net.
    fn sync_loop(&mut self, id: LoopKey, voice: Voice) {
        match self.loops.get_mut(&id) {
            Some(running) if running.sound_id() == voice.sound_id() => {
                // The target, not the level: a loop the sink restarted is still rising.
                let target = voice.envelope.target;
                let pan_target = voice.pan.envelope.target;
                if running.category == voice.category {
                    running.envelope.slew_to(target, self.ramp_frames);
                    running.pan.envelope.slew_to(pan_target, self.ramp_frames);
                    return;
                }
                let old_bus = Voice {
                    samples: running.samples.clone(),
                    pos: running.pos,
                    envelope: running.envelope,
                    pan: running.pan,
                    category: running.category,
                    looping: true,
                };
                running.category = voice.category;
                running.envelope = Envelope::rising(target, self.ramp_frames);
                running.pan.envelope.slew_to(pan_target, self.ramp_frames);
                self.retire(old_bus);
                return;
            }
            Some(_) => {}
            None => {
                if self.live() >= self.limits.total && !self.steal(None) {
                    return;
                }
            }
        }
        if let Some(displaced) = self.loops.insert(id, voice) {
            self.retire(displaced);
        }
    }

    /// Voices that hold a slot against the caps: every loop and every one-shot not fading out.
    fn live(&self) -> usize {
        self.live_one_shots(None) + self.loops.len()
    }

    fn live_one_shots(&self, sound: Option<usize>) -> usize {
        self.voices
            .iter()
            .filter(|v| !v.envelope.retiring && sound.is_none_or(|s| v.sound_id() == s))
            .count()
    }

    /// Starts an arriving one-shot, first making room under `limits` by fading out a live one-shot
    /// (or refusing the newcomer when none gives way).
    fn admit(&mut self, voice: Voice) {
        if let Some(cap) = self.limits.per_sound {
            let sound = voice.sound_id();
            if self.live_one_shots(Some(sound)) >= cap && !self.steal(Some(sound)) {
                return;
            }
        }
        if self.live() >= self.limits.total && !self.steal(None) {
            return;
        }
        let room = 2 * self.limits.total;
        make_room(&mut self.voices, room);
        if self.voices.len() < room {
            self.voices.push(voice);
        }
    }

    /// Fades out the oldest live one-shot (of `sound`, if given). False when there is no candidate.
    fn steal(&mut self, sound: Option<usize>) -> bool {
        let mut candidates = self
            .voices
            .iter_mut()
            .filter(|v| !v.envelope.retiring && sound.is_none_or(|s| v.sound_id() == s));
        let Some(victim) = candidates.next() else {
            return false;
        };
        victim.envelope.fade_out(self.ramp_frames);
        true
    }

    /// Fills `out`: the whole "~100-line gain-and-sum mixer". Applies the arriving commands
    /// (loop-table edits through [`Mixer::sync_loop`], new one-shots collapsed by
    /// [`collapse_simultaneous`] and admitted under the voice limits), then in blocks of
    /// [`MIX_BLOCK_FRAMES`] sums every voice's next samples (scaled by its envelope and its bus's
    /// per-frame level, skipping held buses) into the scratch buffer, adds the music, runs the sum through
    /// [`soft_limit`], converts to the device's sample type, and drops the voices that have finished.
    /// It allocates nothing.
    fn render<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T]) {
        self.health.renders.fetch_add(1, Ordering::Relaxed);
        while let Ok(command) = self.commands.try_recv() {
            self.apply(command);
        }
        // `take` keeps the reserved capacity: an empty `Vec` owns no allocation to lose.
        let mut arriving = std::mem::take(&mut self.arriving);
        for voice in arriving.drain(..) {
            self.admit(voice);
        }
        self.arriving = arriving;
        for block in out.chunks_mut(MIX_BLOCK_FRAMES * self.channels) {
            let frames = block.len().div_ceil(self.channels);
            let held = self.held_buses();
            self.step_buses(frames);
            mix_into(
                &mut self.voices,
                &mut self.loops,
                &mut self.scratch,
                block.len(),
                self.channels,
                &self.bus_levels,
                &held,
            );
            // A held music bus skips the player entirely: its stale-chunk drain would discard the
            // position the pause is holding.
            let music = Bus::Music as usize;
            if !held[music] {
                let start = music * MIX_BLOCK_FRAMES;
                self.music
                    .mix_into(&mut self.scratch, &self.bus_levels[start..start + frames]);
            }
            for (slot, &mixed) in block.iter_mut().zip(self.scratch.iter()) {
                *slot = T::from_sample(device_sample(mixed));
            }
        }
    }
}

/// Starts `voice` fading out in `voices`, cutting the quietest fading voice early when `voices` is
/// at `room`. A voice no room can be made for is dropped.
fn retire(voices: &mut Vec<Voice>, room: usize, ramp_frames: usize, mut voice: Voice) {
    voice.envelope.fade_out(ramp_frames);
    make_room(voices, room);
    if voices.len() < room {
        voices.push(voice);
    }
}

/// Cuts the fading voice with the lowest level when `voices` is at `room`.
fn make_room(voices: &mut Vec<Voice>, room: usize) {
    if voices.len() < room {
        return;
    }
    let lowest = voices
        .iter()
        .enumerate()
        .filter(|(_, v)| v.envelope.retiring)
        .min_by(|(_, a), (_, b)| a.envelope.level.total_cmp(&b.envelope.level))
        .map(|(index, _)| index);
    if let Some(index) = lowest {
        voices.remove(index);
    }
}

/// Builds an output stream whose callback is [`Mixer::render`].
///
/// The error callback records the first error in the mixer's [`StreamHealth`]; the sink thread
/// reads it and reopens the output.
fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut mixer: Mixer,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + FromSample<f32>,
{
    let health = mixer.health.clone();
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| mixer.render(data),
        move |error| {
            // cpal can report the same dead device on every callback; only the first is kept.
            if health.error.get().is_none() {
                let _ = health.error.set(error.to_string());
            }
        },
        None,
    )
}

/// Maps a summed sample into `[-1, 1]` through a soft knee: the identity below [`LIMITER_KNEE`],
/// and above it a saturating curve that is continuous, monotone, has slope 1 at the knee, and
/// approaches full scale without reaching it.
///
/// [`collapse_simultaneous`] bounds *identical* stacking only; this bounds the rest — the four
/// distinct sounds one staged kill fires sum past 1.0 whatever each one's authored gain is, and a
/// per-voice "keep gains under 0.8" rule cannot see a cross-voice sum. Losing a little dynamics
/// above the knee is inaudible; a hard clamp flat-tops the waveform, and that is what distortion
/// sounds like. The mixer's `[-1, 1]` clamp stays as the last line, never as the shaping.
fn soft_limit(sample: f32) -> f32 {
    let level = sample.abs();
    if level <= LIMITER_KNEE {
        return sample;
    }
    let headroom = 1.0 - LIMITER_KNEE;
    let over = (level - LIMITER_KNEE) / headroom;
    (LIMITER_KNEE + headroom * (over / (1.0 + over))).copysign(sample)
}

/// The sample the device gets for a summed `mixed`: [`soft_limit`], clamped to `[-1, 1]`. Anything
/// the curve cannot map (NaN, an infinity, a sum past `f32` range) is silent: silence is the safe
/// failure. It covers what the command queue cannot: a finite gain
/// that overflows the sum, and a non-finite [`CategoryGains`] master.
fn device_sample(mixed: f32) -> f32 {
    let limited = soft_limit(mixed).clamp(-1.0, 1.0);
    if limited.is_nan() { 0.0 } else { limited }
}

/// Adds `voice` to the voices arriving in this callback, collapsing same-sound copies and dropping
/// what no longer fits in `limit` — the sink's anti-stacking guarantee.
///
/// N copies of one sound started together are sample-aligned, so they sum to exactly N× the
/// waveform; a hit that lands on eight enemies in one tick is one hit, not one eight times as loud.
/// Enforcing it here rather than at each call site is what stops a new sound hook from getting
/// louder with the crowd, and audio has no golden test to catch it. A copy of a sound already
/// arriving raises that survivor to the loudest of their gains; the survivor keeps the first
/// arrival's category, since identity here is the sound, not the bus. The survivor also takes the
/// pan of the louder arrival (the first's on a tie), so its pan and gain always come from one copy.
/// A new sound is pushed while `arriving` holds fewer than `limit` voices (earlier arrivals win ties), so the list never grows.
///
/// Deliberately narrow: it bounds *identical* stacking only, at full fidelity. Distinct sounds still
/// sum, and gains adding past full scale are [`soft_limit`]'s job, not this one's.
fn collapse_simultaneous(arriving: &mut Vec<Voice>, voice: Voice, limit: usize) {
    let sound = voice.sound_id();
    if let Some(first) = arriving.iter_mut().find(|v| v.sound_id() == sound) {
        if voice.envelope.level > first.envelope.level {
            first.pan = voice.pan;
        }
        first.envelope = Envelope::at(first.envelope.level.max(voice.envelope.level));
    } else if arriving.len() < limit {
        arriving.push(voice);
    }
}

/// Sums every active voice into `scratch` (resized to `samples` and zeroed first) through its bus's
/// per-frame level in `levels` (bus `b` owns `b * MIX_BLOCK_FRAMES ..`), advancing each voice's
/// cursor and envelope, and drops the voices that ran out of samples or finished fading out. A voice
/// on a `held` bus is skipped, cursor and envelope kept. A loop in the table is never dropped here —
/// only a stop removes one.
///
/// The loop table's iteration order varies per process, so with three or more loops the sum can
/// differ in its last bits between runs; audio is outside every gate. The table is a `HashMap`
/// because the callback may neither allocate nor free, which a `BTreeMap` would. A sample-exact test
/// uses at most two loops.
fn mix_into(
    voices: &mut Vec<Voice>,
    loops: &mut HashMap<LoopKey, Voice>,
    scratch: &mut Vec<f32>,
    samples: usize,
    channels: usize,
    levels: &[f32],
    held: &[bool; BUS_COUNT],
) {
    let frames = samples.div_ceil(channels.max(1));
    scratch.clear();
    scratch.resize(samples, 0.0);
    voices.retain_mut(|voice| {
        if is_held(held, voice) {
            return true;
        }
        voice.advance_into(scratch, channels, bus_levels(levels, voice, frames))
    });
    for voice in loops.values_mut() {
        if !is_held(held, voice) {
            voice.advance_into(scratch, channels, bus_levels(levels, voice, frames));
        }
    }
}

/// Whether `voice`'s bus is held.
fn is_held(held: &[bool; BUS_COUNT], voice: &Voice) -> bool {
    held[Bus::from(voice.category) as usize]
}

/// The first `frames` per-frame levels of `voice`'s bus.
fn bus_levels<'a>(levels: &'a [f32], voice: &Voice, frames: usize) -> &'a [f32] {
    let start = Bus::from(voice.category) as usize * MIX_BLOCK_FRAMES;
    &levels[start..start + frames]
}

/// Converts decoded [`AudioPcm`] to an interleaved buffer at the device's rate and channel count,
/// through the same linear [`Converter`] music streams through.
///
/// A stereo clip stays stereo on a stereo device and is the mean of both channels on a mono one; a
/// mono clip fills every device channel. The clip is converted whole, with its first frame as its
/// last frame's right neighbour (periodic extension): exact across a loop's seam, and for a one-shot
/// it changes at most the final output frame. The output holds `ceil(frames × out_rate / in_rate)`
/// frames; the same rate is an exact copy.
fn to_device_buffer(pcm: &AudioPcm, out_rate: u32, out_channels: u16) -> Vec<f32> {
    if pcm.samples.is_empty() {
        return Vec::new();
    }
    let in_channels = usize::from(pcm.channels.max(1));
    let out_channels = usize::from(out_channels.max(1));
    let mut converter = Converter::new(pcm.sample_rate, in_channels, out_rate, out_channels);
    let in_frames = (pcm.samples.len() / in_channels) as u64;
    let out_frames = (in_frames * u64::from(out_rate)).div_ceil(u64::from(pcm.sample_rate.max(1)));
    let mut out = Vec::with_capacity(out_frames as usize * out_channels);
    converter.convert(&pcm.samples, &mut out);
    converter.convert(&pcm.samples[..in_channels], &mut out);
    // The converter's running position can land a rounding error short of an exact end, and emit
    // one frame too many.
    out.truncate(out_frames as usize * out_channels);
    out
}

/// Builds the [`L5001`](code::AUDIO_DEVICE_UNAVAILABLE) warning diagnostic.
fn device_unavailable(detail: &str) -> Diagnostic {
    Diagnostic::new(
        code::AUDIO_DEVICE_UNAVAILABLE,
        format!("no audio output available ({detail}); playing silent until one appears"),
    )
    .with_help("this is expected on a headless/CI machine — audio never gates a run; the sink keeps retrying, so connect an output device and no restart is needed")
}

/// Builds the [`L5002`](code::AUDIO_STREAM_FAILED) warning diagnostic.
fn stream_failed(detail: &str) -> Diagnostic {
    Diagnostic::new(
        code::AUDIO_STREAM_FAILED,
        format!("the audio output stream failed ({detail}); the sink plays silent until the default output reopens"),
    )
    .with_help(
        "the device was lost or stopped calling back (unplugged, switched, a sample-rate change, an exclusive-mode grab); \
         the sink retries the system default output with backoff (up to every 5 s), loops and music come back, no restart is needed, \
         and the simulation is unaffected",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::audio::AudioEvent;

    /// Unit bus level for every frame of a block.
    const UNITY: [f32; MIX_BLOCK_FRAMES] = [1.0; MIX_BLOCK_FRAMES];

    /// [`mix_into`] with each bus at its authored gain from `gains` for the whole block and no bus
    /// held.
    fn mix_with_gains(
        voices: &mut Vec<Voice>,
        loops: &mut HashMap<LoopKey, Voice>,
        scratch: &mut Vec<f32>,
        samples: usize,
        channels: usize,
        gains: &CategoryGains,
    ) {
        let authored = [
            1.0,
            gains.sfx,
            gains.ambient,
            gains.gain_for(AudioCategory::Voice),
            gains.gain_for(AudioCategory::Ui),
            gains.music,
        ];
        let levels: Vec<f32> = authored
            .iter()
            .flat_map(|&gain| [gain; MIX_BLOCK_FRAMES])
            .collect();
        mix_into(
            voices,
            loops,
            scratch,
            samples,
            channels,
            &levels,
            &[false; BUS_COUNT],
        );
    }

    #[test]
    fn device_unavailable_is_l5001() {
        assert_eq!(
            device_unavailable("test").code,
            code::AUDIO_DEVICE_UNAVAILABLE
        );
        assert_eq!(device_unavailable("test").code.as_str(), "L5001");
    }

    #[test]
    fn stream_failure_is_l5002() {
        assert_eq!(stream_failed("test").code, code::AUDIO_STREAM_FAILED);
        assert_eq!(stream_failed("test").code.as_str(), "L5002");
    }

    /// A one-shot voice on `samples` at `gain`, on `category`'s bus, ready to play from the top.
    fn one_shot(samples: &Arc<Vec<f32>>, gain: f32, category: AudioCategory) -> Voice {
        Voice {
            samples: samples.clone(),
            pos: 0,
            envelope: Envelope::at(gain),
            pan: Panner::centred(),
            category,
            looping: false,
        }
    }

    /// A looping voice on `samples` at `gain`, on `category`'s bus, ready to play from the top.
    fn looping(samples: &Arc<Vec<f32>>, gain: f32, category: AudioCategory) -> Voice {
        Voice {
            looping: true,
            ..one_shot(samples, gain, category)
        }
    }

    fn code_of(result: Result<(), Diagnostic>) -> Option<String> {
        result.err().map(|d| d.code.as_str().to_string())
    }

    #[test]
    fn check_message_decides_by_message_kind() {
        let root = scratch_root("check-music");
        assert_eq!(
            code_of(check_message(&root, &music_play("missing.ogg"))).as_deref(),
            Some("L5003")
        );
        std::fs::copy(tone(), root.join("tone.ogg")).unwrap();
        assert_eq!(code_of(check_message(&root, &music_play("tone.ogg"))), None);
        // The message kind decides, not the extension: an Ogg played as a one-shot is no WAV.
        let as_sound = one_shot_message("tone.ogg", 1.0, AudioCategory::Sfx);
        assert_eq!(
            code_of(check_message(&root, &as_sound)).as_deref(),
            Some("L1104")
        );
    }

    #[test]
    fn stop_messages_check_nothing() {
        let root = scratch_root("check-stops");
        let stops = [
            AudioMessage::LoopStop {
                id: LoopId::Sound("missing.wav".to_string()),
            },
            AudioMessage::StopAllLoops,
            AudioMessage::MusicStop { fade_frames: 0 },
        ];
        for message in &stops {
            assert!(check_message(&root, message).is_ok(), "{message:?}");
        }
    }

    #[test]
    fn mono_passthrough_at_same_rate_is_identity() {
        let pcm = AudioPcm {
            sample_rate: 44100,
            channels: 1,
            samples: vec![0.0, 0.5, -0.5, 1.0],
        };
        let out = to_device_buffer(&pcm, 44100, 1);
        assert_eq!(out, vec![0.0, 0.5, -0.5, 1.0]);
    }

    fn pcm(sample_rate: u32, channels: u16, samples: &[f32]) -> AudioPcm {
        AudioPcm {
            sample_rate,
            channels,
            samples: samples.to_vec(),
        }
    }

    #[test]
    fn sfx_resampling_interpolates_linearly_and_wraps_into_the_first_frame() {
        let out = to_device_buffer(&pcm(11025, 1, &[0.25, 1.0, 0.5]), 44100, 1);
        // The last three lean toward x[0] = 0.25; a zero neighbour would read 0.375, 0.25, 0.125.
        assert_eq!(
            out,
            [
                0.25, 0.4375, 0.625, 0.8125, 1.0, 0.875, 0.75, 0.625, 0.5, 0.4375, 0.375, 0.3125
            ]
        );
    }

    #[test]
    fn a_ramp_resampled_from_44k1_to_48k_is_still_a_ramp() {
        let ramp: Vec<f32> = (0..441).map(|n| n as f32 / 441.0).collect();
        let out = to_device_buffer(&pcm(44100, 1, &ramp), 48000, 1);
        assert_eq!(out.len(), 480);
        for (k, &got) in out.iter().enumerate() {
            let p = k as f64 * 44100.0 / 48000.0;
            let want = if p <= 440.0 {
                p / 441.0
            } else {
                (440.0 / 441.0) * (1.0 - (p - 440.0))
            };
            assert!(
                (f64::from(got) - want).abs() < 1e-5,
                "frame {k}: {got} vs {want}"
            );
        }
    }

    #[test]
    fn a_stereo_sound_stays_stereo_on_a_stereo_device() {
        let samples = [1.0, -1.0, 0.5, 0.25];
        assert_eq!(
            to_device_buffer(&pcm(48000, 2, &samples), 48000, 2),
            samples
        );
    }

    #[test]
    fn a_stereo_sound_is_its_mean_on_a_mono_device() {
        let out = to_device_buffer(&pcm(48000, 2, &[1.0, -1.0, 0.5, 0.25]), 48000, 1);
        assert_eq!(out, [0.0, 0.375]);
    }

    #[test]
    fn a_mono_sound_fills_every_device_channel() {
        let out = to_device_buffer(&pcm(48000, 1, &[0.5, 0.25]), 48000, 2);
        assert_eq!(out, [0.5, 0.5, 0.25, 0.25]);
    }

    #[test]
    fn resample_halving_the_rate_halves_the_frames() {
        let pcm = AudioPcm {
            sample_rate: 44100,
            channels: 1,
            samples: vec![0.0; 100],
        };
        let out = to_device_buffer(&pcm, 22050, 1);
        assert_eq!(out.len(), 50);
    }

    /// Builds `count` voices that all start now on `samples`, each at `gain` on the `Sfx` bus.
    fn simultaneous(samples: &Arc<Vec<f32>>, gain: f32, count: usize) -> Vec<Voice> {
        (0..count)
            .map(|_| one_shot(samples, gain, AudioCategory::Sfx))
            .collect()
    }

    /// The peak absolute sample the mixer would hand the device, before its `[-1, 1]` clamp.
    fn peak(mut voices: Vec<Voice>, frames: usize) -> f32 {
        let mut scratch = Vec::new();
        mix_with_gains(
            &mut voices,
            &mut HashMap::new(),
            &mut scratch,
            frames,
            1,
            &CategoryGains::default(),
        );
        scratch.iter().fold(0.0_f32, |m, s| m.max(s.abs()))
    }

    /// `voices` folded through [`collapse_simultaneous`] with no cap, in arrival order.
    fn collapsed(voices: Vec<Voice>) -> Vec<Voice> {
        let mut arriving = Vec::with_capacity(voices.len());
        for voice in voices {
            collapse_simultaneous(&mut arriving, voice, usize::MAX);
        }
        arriving
    }

    #[test]
    fn simultaneous_copies_of_one_sound_collapse_to_one_voice() {
        let buffer = Arc::new(vec![0.8, -0.8, 0.8, -0.8]);
        let arriving = collapsed(simultaneous(&buffer, 0.8, 8));
        assert_eq!(arriving.len(), 1);
        assert!((arriving[0].envelope.level - 0.8).abs() < 1e-6);
    }

    #[test]
    fn collapsing_keeps_the_loudest_gain() {
        let buffer = Arc::new(vec![1.0, 1.0]);
        let arriving = collapsed(vec![
            one_shot(&buffer, 0.2, AudioCategory::Sfx),
            one_shot(&buffer, 0.7, AudioCategory::Sfx),
            one_shot(&buffer, 0.5, AudioCategory::Sfx),
        ]);
        assert_eq!(arriving.len(), 1);
        assert!((arriving[0].envelope.level - 0.7).abs() < 1e-6);
    }

    #[test]
    fn distinct_sounds_are_never_collapsed() {
        let a = Arc::new(vec![1.0, 1.0]);
        let b = Arc::new(vec![0.5, 0.5]);
        let arriving = collapsed(vec![
            one_shot(&a, 1.0, AudioCategory::Sfx),
            one_shot(&b, 1.0, AudioCategory::Sfx),
            one_shot(&a, 1.0, AudioCategory::Sfx),
        ]);
        assert_eq!(arriving.len(), 2);
        assert!(Arc::ptr_eq(&arriving[0].samples, &a));
        assert!(Arc::ptr_eq(&arriving[1].samples, &b));
    }

    /// Survivors keep first-arrival order however the duplicates interleave — the in-place
    /// compaction writes over discards, so a run of duplicates before a new sound must not
    /// reorder or lose it.
    #[test]
    fn survivors_keep_first_arrival_order_through_interleaved_duplicates() {
        let a = Arc::new(vec![1.0]);
        let b = Arc::new(vec![0.5]);
        let c = Arc::new(vec![0.25]);
        let voice = |samples: &Arc<Vec<f32>>| one_shot(samples, 1.0, AudioCategory::Sfx);
        // a a a b c b a  ->  a b c
        let arriving = collapsed(vec![
            voice(&a),
            voice(&a),
            voice(&a),
            voice(&b),
            voice(&c),
            voice(&b),
            voice(&a),
        ]);
        let order: Vec<usize> = arriving.iter().map(Voice::sound_id).collect();
        assert_eq!(
            order,
            vec![
                Arc::as_ptr(&a) as usize,
                Arc::as_ptr(&b) as usize,
                Arc::as_ptr(&c) as usize
            ]
        );
    }

    /// The finding this guarantee exists for: one sound triggered many times on a single tick used
    /// to stack into the clamp. This fails without [`collapse_simultaneous`] (peak 6.4, hard-clipped
    /// to 1.0 = audible distortion) and passes with it.
    #[test]
    fn stacked_triggers_no_longer_drive_the_mixer_into_the_clamp() {
        let buffer = Arc::new(vec![0.8, -0.8, 0.8, -0.8]);
        let uncollapsed = peak(simultaneous(&buffer, 0.8, 8), 4);
        assert!(
            uncollapsed > 1.0,
            "the pre-guarantee behaviour must actually clip, else this test proves nothing: {uncollapsed}"
        );

        let one = peak(collapsed(simultaneous(&buffer, 0.8, 8)), 4);
        assert!(
            one <= 1.0,
            "eight simultaneous copies of one sound must not exceed full scale: {one}"
        );
        assert!((one - 0.64).abs() < 1e-6); // exactly one voice: 0.8 sample x 0.8 gain
    }

    /// A full arrival list drops a new sound but still merges a copy of one it holds.
    #[test]
    fn a_full_arrival_list_drops_a_new_sound_but_merges_a_copy() {
        let (a, b, c) = (constant(1.0, 4), constant(1.0, 4), constant(1.0, 4));
        let mut arriving = Vec::with_capacity(2);
        for voice in [
            one_shot(&a, 0.25, AudioCategory::Sfx),
            one_shot(&b, 0.5, AudioCategory::Sfx),
            one_shot(&c, 1.0, AudioCategory::Sfx),
            one_shot(&a, 0.75, AudioCategory::Sfx),
        ] {
            collapse_simultaneous(&mut arriving, voice, 2);
        }
        assert_eq!(arriving.len(), 2);
        assert!(Arc::ptr_eq(&arriving[0].samples, &a));
        assert!(Arc::ptr_eq(&arriving[1].samples, &b));
        assert_eq!(arriving[0].envelope.level, 0.75);
    }

    #[test]
    fn mixing_advances_cursors_and_retires_finished_voices() {
        let buffer = Arc::new(vec![1.0, 1.0, 1.0]);
        let mut voices = simultaneous(&buffer, 1.0, 1);
        let mut loops = HashMap::new();
        let mut scratch = Vec::new();
        mix_with_gains(
            &mut voices,
            &mut loops,
            &mut scratch,
            2,
            1,
            &CategoryGains::default(),
        );
        assert_eq!(scratch, vec![1.0, 1.0]);
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].pos, 2);
        mix_with_gains(
            &mut voices,
            &mut loops,
            &mut scratch,
            2,
            1,
            &CategoryGains::default(),
        );
        assert_eq!(scratch, vec![1.0, 0.0]); // one sample left, then silence
        assert!(voices.is_empty()); // and the voice is retired
    }

    /// The loop primitive: a voice past the end of its buffer continues from the top instead of
    /// retiring, and its cursor lands where the wrap left it.
    #[test]
    fn a_looping_voice_wraps_at_the_end_of_its_buffer() {
        let buffer = Arc::new(vec![0.5, 0.25]);
        let mut loops = HashMap::new();
        loops.insert(LoopKey::Sound(0), looping(&buffer, 1.0, AudioCategory::Sfx));
        let mut scratch = Vec::new();
        mix_with_gains(
            &mut Vec::new(),
            &mut loops,
            &mut scratch,
            5,
            1,
            &CategoryGains::default(),
        );
        assert_eq!(scratch, vec![0.5, 0.25, 0.5, 0.25, 0.5]);
        assert_eq!(loops.len(), 1, "a loop is never retired by the mixer");
        assert_eq!(loops.values().next().unwrap().pos, 1);
    }

    /// An empty buffer is the one loop that does not play — the guard against spinning on nothing.
    #[test]
    fn a_looping_voice_on_an_empty_buffer_plays_silence() {
        let mut voice = looping(&Arc::new(Vec::new()), 1.0, AudioCategory::Sfx);
        let mut scratch = vec![0.0; 4];
        assert!(!voice.advance_into(&mut scratch, 1, &UNITY));
        assert_eq!(scratch, vec![0.0; 4]);
    }

    /// A loop sync command on `id`'s sound, as the router would send it.
    fn loop_command(id: LoopKey, samples: &Arc<Vec<f32>>, gain: f32) -> VoiceCommand {
        VoiceCommand::LoopSync {
            id,
            voice: looping(samples, gain, AudioCategory::Sfx),
        }
    }

    fn one_shot_command(samples: &Arc<Vec<f32>>, gain: f32) -> VoiceCommand {
        VoiceCommand::OneShot(one_shot(samples, gain, AudioCategory::Sfx))
    }

    /// A mixer at rate 400 (a 4-frame ramp) with no device and a dummy music ring.
    fn test_mixer(channels: u16, gains: CategoryGains, limits: VoiceLimits) -> Mixer {
        let (_commands, command_rx) = crossbeam_channel::unbounded();
        let (_music_tx, music_rx) = crossbeam_channel::bounded(MUSIC_RING_CHUNKS);
        Mixer::new(command_rx, music_rx, channels, 400, gains, limits)
    }

    /// A sound of `len` samples, all `value`; every call is a distinct sound.
    fn constant(value: f32, len: usize) -> Arc<Vec<f32>> {
        Arc::new(vec![value; len])
    }

    fn default_limits() -> VoiceLimits {
        voice_limits(VoiceCaps::default())
    }

    fn limits(per_sound: Option<usize>, total: usize) -> VoiceLimits {
        VoiceLimits { per_sound, total }
    }

    /// Renders `samples` samples of `mixer`.
    fn render(mixer: &mut Mixer, samples: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; samples];
        mixer.render(&mut out);
        out
    }

    fn bed() -> LoopKey {
        LoopKey::Sound(0)
    }

    #[test]
    fn device_sample_is_finite_and_in_range_for_any_input() {
        assert_eq!(device_sample(f32::NAN), 0.0);
        assert_eq!(device_sample(f32::INFINITY), 0.0);
        assert_eq!(device_sample(f32::NEG_INFINITY), 0.0);
        assert_eq!(device_sample(f32::MAX), 0.0);
        assert_eq!(device_sample(0.5), 0.5);
        assert_eq!(device_sample(-0.25), -0.25);
        assert!(device_sample(2.0) < 1.0 && device_sample(2.0) > LIMITER_KNEE);
    }

    #[test]
    fn a_voice_gain_that_is_not_sane_never_reaches_the_device() {
        for gain in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
            let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
            mixer.apply(one_shot_command(&constant(0.5, 64), gain));
            mixer.apply(loop_command(bed(), &constant(0.5, 64), gain));
            for _ in 0..3 {
                for sample in render(&mut mixer, 16) {
                    assert!(
                        sample.is_finite() && (-1.0..=1.0).contains(&sample),
                        "gain {gain}: {sample}"
                    );
                }
            }
        }
    }

    /// A looping voice on `samples` at unity gain, settled at `pan`.
    fn panned(samples: &Arc<Vec<f32>>, pan: f32) -> Voice {
        Voice {
            pan: Panner::at(pan),
            ..looping(samples, 1.0, AudioCategory::Sfx)
        }
    }

    /// A loop sync on `bed()` whose voice is panned to `pan`.
    fn panned_command(samples: &Arc<Vec<f32>>, pan: f32) -> VoiceCommand {
        VoiceCommand::LoopSync {
            id: bed(),
            voice: panned(samples, pan),
        }
    }

    #[test]
    fn a_centred_voice_is_bit_identical_to_an_unpanned_one() {
        let samples = Arc::new(vec![0.1, 0.7, 0.3, -0.2, 0.9, 0.05, -0.4, 0.6]);
        let mut voice = panned(&samples, 0.0);
        let mut out = vec![0.0; 8];
        assert!(voice.advance_into(&mut out, 2, &UNITY));
        for (got, want) in out.iter().zip(samples.iter()) {
            assert_eq!(got.to_bits(), (want * 1.0).to_bits());
        }

        // Out to a pan and back: once the envelope lands on 0.0 the unpanned path returns.
        let mut voice = panned(&samples, 0.0);
        voice.pan.envelope.slew_to(0.5, 2);
        let mut away = vec![0.0; 4];
        voice.advance_into(&mut away, 2, &UNITY);
        assert_ne!(away[1].to_bits(), samples[1].to_bits());
        voice.pan.envelope.slew_to(0.0, 2);
        let mut back = vec![0.0; 4];
        voice.advance_into(&mut back, 2, &UNITY);
        let mut settled = vec![0.0; 8];
        voice.advance_into(&mut settled, 2, &UNITY);
        let wanted: Vec<f32> = (0..8).map(|i| samples[(8 + i) % 8]).collect();
        for (got, want) in settled.iter().zip(&wanted) {
            assert_eq!(got.to_bits(), want.to_bits());
        }
    }

    #[test]
    fn a_hard_pan_silences_the_far_channel_and_lifts_the_near_one() {
        let samples = constant(0.25, 16);
        let root2 = std::f32::consts::SQRT_2;
        let mut left = vec![0.0; 4];
        panned(&samples, -1.0).advance_into(&mut left, 2, &UNITY);
        let mut right = vec![0.0; 4];
        panned(&samples, 1.0).advance_into(&mut right, 2, &UNITY);
        for frame in left.chunks(2) {
            assert!((frame[0] - 0.25 * root2).abs() < 1e-6, "{frame:?}");
            assert_eq!(frame[1], 0.0);
        }
        for frame in right.chunks(2) {
            assert_eq!(frame[0], 0.0);
            assert!((frame[1] - 0.25 * root2).abs() < 1e-6, "{frame:?}");
        }
    }

    #[test]
    fn a_stereo_clip_at_a_hard_pan_plays_its_channel_mean() {
        let samples = Arc::new(vec![0.1, 0.3, 0.1, 0.3]);
        let mut out = vec![0.0; 4];
        panned(&samples, -1.0).advance_into(&mut out, 2, &UNITY);
        assert!((out[0] - 0.2 * std::f32::consts::SQRT_2).abs() < 1e-6);
        assert_eq!(out[1], 0.0);
        let mut centre = vec![0.0; 4];
        panned(&samples, 0.0).advance_into(&mut centre, 2, &UNITY);
        assert_eq!(
            centre,
            [0.1, 0.3, 0.1, 0.3],
            "centred, the clip stays stereo"
        );
    }

    #[test]
    fn a_pan_resync_ramps_without_a_step_or_a_cursor_jump() {
        let samples = constant(0.25, 64);
        let mut mixer = test_mixer(2, CategoryGains::default(), default_limits());
        mixer.apply(panned_command(&samples, 0.0));
        render(&mut mixer, 4);
        mixer.apply(panned_command(&samples, 1.0));
        let out = render(&mut mixer, 16);
        assert_eq!(
            mixer.loops[&bed()].pos,
            20,
            "the cursor survives the resync"
        );
        let mut previous = [0.25f32, 0.25];
        for frame in out.chunks(2) {
            for channel in 0..2 {
                assert!(
                    (frame[channel] - previous[channel]).abs() < 0.1,
                    "a step in channel {channel}: {out:?}"
                );
                previous[channel] = frame[channel];
            }
        }
        let last = &out[out.len() - 2..];
        assert_eq!(last[0], 0.0);
        assert!((last[1] - 0.25 * std::f32::consts::SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn pan_applies_only_on_a_two_channel_device() {
        let samples = constant(0.25, 24);
        for channels in [1u16, 3] {
            let mut mixer = test_mixer(channels, CategoryGains::default(), default_limits());
            mixer.apply(panned_command(&samples, -1.0));
            let out = render(&mut mixer, 6);
            assert!(
                out.iter().all(|&s| s == 0.25),
                "{channels} channels: {out:?}"
            );
        }
    }

    #[test]
    fn simultaneous_copies_keep_the_pan_of_the_louder_arrival() {
        let samples = constant(0.5, 8);
        let arrive = |first: (f32, f32), second: (f32, f32)| {
            let mut arriving = Vec::new();
            for (gain, pan) in [first, second] {
                let voice = Voice {
                    envelope: Envelope::at(gain),
                    ..panned(&samples, pan)
                };
                collapse_simultaneous(&mut arriving, voice, 8);
            }
            assert_eq!(arriving.len(), 1);
            (arriving[0].envelope.level, arriving[0].pan.envelope.level)
        };
        assert_eq!(arrive((0.5, -1.0), (1.0, 1.0)), (1.0, 1.0));
        assert_eq!(arrive((1.0, 1.0), (0.5, -1.0)), (1.0, 1.0));
        assert_eq!(
            arrive((0.75, -0.5), (0.75, 0.5)),
            (0.75, -0.5),
            "a tie keeps the first"
        );
    }

    #[test]
    fn a_pan_that_is_not_sane_never_reaches_the_device() {
        assert_eq!(sane_pan(f32::NAN), 0.0);
        assert_eq!(sane_pan(f32::INFINITY), 0.0);
        assert_eq!(sane_pan(f32::NEG_INFINITY), 0.0);
        assert_eq!(sane_pan(7.0), 1.0);
        assert_eq!(sane_pan(-7.0), -1.0);
        assert_eq!(sane_pan(0.25), 0.25);

        let root = scratch_root("sane-pan");
        std::fs::write(root.join("bed.wav"), wav_bytes(&[8192, 16384])).unwrap();
        let mut rig = rig(root);
        for pan in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let one_shot = AudioMessage::OneShot(AudioEvent {
                pan,
                ..AudioEvent::new("bed.wav", 1.0)
            });
            let Some(VoiceCommand::OneShot(voice)) = rig.router.route(one_shot) else {
                panic!("a one-shot");
            };
            assert_eq!(voice.pan.envelope.target, 0.0);
            let message = AudioMessage::LoopSync {
                id: LoopId::Sound("bed.wav".to_string()),
                sound: "bed.wav".to_string(),
                gain: 1.0,
                pan,
                category: AudioCategory::Ambient,
            };
            let Some(VoiceCommand::LoopSync { voice, .. }) = rig.router.route(message) else {
                panic!("a loop sync");
            };
            assert_eq!(voice.pan.envelope.target, 0.0);
        }
    }

    #[test]
    fn a_stereo_clip_is_half_narrowed_at_half_the_narrowing_pan() {
        let samples = Arc::new(vec![0.1, 0.3, 0.1, 0.3]);
        let mut voice = panned(&samples, PAN_FULL_MONO / 2.0);
        let mut out = vec![0.0; 2];
        voice.advance_into(&mut out, 2, &UNITY);
        let gains = voice.pan.gains_at(PAN_FULL_MONO / 2.0);
        let (a, b) = (0.1f32, 0.3f32);
        let mid = (a + b) * 0.5;
        assert!((out[0] - (a + (mid - a) * 0.5) * gains[0]).abs() < 1e-6);
        assert!((out[1] - (b + (mid - b) * 0.5) * gains[1]).abs() < 1e-6);
    }

    #[test]
    fn a_restarted_loop_keeps_its_declared_pan() {
        let root = scratch_root("declared-pan");
        std::fs::write(root.join("bed.wav"), wav_bytes(&[8192, 16384])).unwrap();
        let mut rig = rig(root);
        let message = AudioMessage::LoopSync {
            id: LoopId::Sound("bed.wav".to_string()),
            sound: "bed.wav".to_string(),
            gain: 1.0,
            pan: 0.5,
            category: AudioCategory::Ambient,
        };
        let Some(VoiceCommand::LoopSync { id, .. }) = rig.router.route(message) else {
            panic!("a loop sync");
        };
        let rebuilt = rig.router.declared_voice(id, 0).expect("the sound loads");
        assert_eq!(rebuilt.pan.envelope.target, 0.5);
        assert_eq!(rebuilt.pan.envelope.level, 0.5);
    }

    #[test]
    fn a_bus_crossfade_keeps_the_pan_target() {
        let samples = constant(0.25, 64);
        let mut mixer = test_mixer(2, CategoryGains::default(), default_limits());
        mixer.apply(VoiceCommand::LoopSync {
            id: bed(),
            voice: Voice {
                category: AudioCategory::Ambient,
                ..panned(&samples, 0.0)
            },
        });
        render(&mut mixer, 2);
        mixer.apply(panned_command(&samples, 1.0));
        assert_eq!(mixer.loops[&bed()].pan.envelope.target, 1.0);
        render(&mut mixer, 16);
        let settled = render(&mut mixer, 2);
        assert_eq!(settled[0], 0.0);
        assert!((settled[1] - 0.25 * std::f32::consts::SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn a_loop_stop_fades_out_instead_of_cutting() {
        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        mixer.apply(loop_command(bed(), &constant(0.5, 64), 1.0));
        assert_eq!(render(&mut mixer, 2), [0.5, 0.5]);
        mixer.apply(VoiceCommand::LoopStop(bed()));
        assert_eq!(render(&mut mixer, 6), [0.375, 0.25, 0.125, 0.0, 0.0, 0.0]);
        assert!(mixer.loops.is_empty() && mixer.voices.is_empty());
    }

    #[test]
    fn the_envelope_steps_once_per_frame_on_every_channel() {
        let mut mixer = test_mixer(2, CategoryGains::default(), default_limits());
        mixer.apply(loop_command(bed(), &constant(0.5, 64), 1.0));
        render(&mut mixer, 4);
        mixer.apply(VoiceCommand::LoopStop(bed()));
        assert_eq!(
            render(&mut mixer, 8),
            [0.375, 0.375, 0.25, 0.25, 0.125, 0.125, 0.0, 0.0]
        );
    }

    #[test]
    fn a_blanket_stop_fades_every_loop() {
        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        mixer.apply(loop_command(bed(), &constant(0.5, 64), 1.0));
        mixer.apply(loop_command(LoopKey::Sound(1), &constant(0.25, 64), 1.0));
        assert_eq!(render(&mut mixer, 1), [0.75]);
        mixer.apply(VoiceCommand::StopAllLoops);
        assert_eq!(render(&mut mixer, 5), [0.5625, 0.375, 0.1875, 0.0, 0.0]);
        assert!(mixer.loops.is_empty() && mixer.voices.is_empty());
    }

    /// Re-syncing a running loop on the same sound is a gain change, not a restart: the cursor does
    /// not move, so a per-tick resend never clicks, and the level slews over the ramp.
    #[test]
    fn a_loop_gain_change_slews_over_the_ramp() {
        let buffer = constant(0.5, 64);
        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        mixer.apply(loop_command(bed(), &buffer, 1.0));
        assert_eq!(render(&mut mixer, 2), [0.5, 0.5]);
        mixer.apply(loop_command(bed(), &buffer, 0.25));
        assert_eq!(mixer.loops.len(), 1);
        assert_eq!(mixer.loops[&bed()].pos, 2, "the cursor survives the sync");
        assert_eq!(
            render(&mut mixer, 5),
            [0.40625, 0.3125, 0.21875, 0.125, 0.125]
        );
    }

    /// A different sound under one id replaces the voice, playing from the top while the old one
    /// fades out.
    #[test]
    fn syncing_a_new_sound_under_one_id_restarts_from_the_top() {
        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        mixer.apply(loop_command(bed(), &constant(0.5, 64), 1.0));
        render(&mut mixer, 2);
        mixer.apply(loop_command(bed(), &constant(0.25, 64), 1.0));
        assert_eq!(mixer.loops.len(), 1);
        assert_eq!(mixer.loops[&bed()].pos, 0);
        assert_eq!(render(&mut mixer, 5), [0.625, 0.5, 0.375, 0.25, 0.25]);
    }

    /// The per-sound cap: the oldest live one-shot of the sound fades out and the newcomer starts.
    #[test]
    fn a_per_sound_cap_steals_the_oldest_with_a_fade() {
        let mut mixer = test_mixer(1, CategoryGains::default(), limits(Some(2), 8));
        let a = constant(1.0, 64);
        mixer.apply(one_shot_command(&a, 0.5));
        assert_eq!(render(&mut mixer, 1), [0.5]);
        mixer.apply(one_shot_command(&a, 0.25));
        assert_eq!(render(&mut mixer, 1), [0.75]);
        mixer.apply(one_shot_command(&a, 0.125));
        assert_eq!(render(&mut mixer, 5), [0.75, 0.625, 0.5, 0.375, 0.375]);
        assert_eq!(mixer.voices.len(), 2);
    }

    /// The shipped policy: four live one-shots of one sound, a fifth fades the first.
    #[test]
    fn the_default_caps_keep_four_one_shots_of_one_sound() {
        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        let a = constant(1.0, 64);
        for _ in 0..4 {
            mixer.apply(one_shot_command(&a, 0.125));
            render(&mut mixer, 1);
        }
        assert_eq!(mixer.voices.len(), 4);
        mixer.apply(one_shot_command(&a, 0.125));
        // The first voice steps down a quarter of its level, the other three and the newcomer hold.
        assert_eq!(render(&mut mixer, 1), [0.59375]);
        render(&mut mixer, 8);
        assert_eq!(mixer.voices.len(), 4);
    }

    #[test]
    fn the_total_cap_steals_the_oldest_across_sounds() {
        let mut mixer = test_mixer(1, CategoryGains::default(), limits(None, 2));
        mixer.apply(one_shot_command(&constant(1.0, 64), 0.5));
        assert_eq!(render(&mut mixer, 1), [0.5]);
        mixer.apply(one_shot_command(&constant(1.0, 64), 0.25));
        assert_eq!(render(&mut mixer, 1), [0.75]);
        mixer.apply(one_shot_command(&constant(1.0, 64), 0.125));
        assert_eq!(render(&mut mixer, 5), [0.75, 0.625, 0.5, 0.375, 0.375]);
    }

    #[test]
    fn loops_are_never_stolen() {
        let mut mixer = test_mixer(1, CategoryGains::default(), limits(None, 2));
        let wind = LoopKey::Sound(1);
        mixer.apply(loop_command(bed(), &constant(1.0, 64), 0.25));
        mixer.apply(one_shot_command(&constant(1.0, 64), 0.5));
        assert_eq!(render(&mut mixer, 1), [0.75]);
        // At the total: the one-shot fades, the loop stays.
        mixer.apply(one_shot_command(&constant(1.0, 64), 0.125));
        assert_eq!(render(&mut mixer, 5), [0.75, 0.625, 0.5, 0.375, 0.375]);
        // A second loop id steals the remaining one-shot.
        mixer.apply(loop_command(wind, &constant(1.0, 64), 0.125));
        render(&mut mixer, 8);
        assert_eq!(mixer.loops.len(), 2);
        assert!(mixer.voices.is_empty());
        // Two live loops fill the total: a one-shot and a third loop id are refused.
        mixer.apply(one_shot_command(&constant(1.0, 64), 0.5));
        mixer.apply(loop_command(LoopKey::Sound(2), &constant(1.0, 64), 0.5));
        assert_eq!(render(&mut mixer, 1), [0.375]);
        assert_eq!(mixer.loops.len(), 2);
        assert!(mixer.voices.is_empty());
    }

    /// A bus command on `bus`, settled now (`frames: 0`).
    fn bus_gain(bus: Bus, layer: MixLayer, gain: f32) -> VoiceCommand {
        VoiceCommand::BusGain {
            bus: bus as usize,
            layer,
            gain,
            frames: 0,
        }
    }

    fn bus_pause(bus: Bus, paused: bool, frames: usize) -> VoiceCommand {
        VoiceCommand::BusPause {
            mask: 1 << bus as usize,
            paused,
            frames,
        }
    }

    /// The layers multiply, and the master scales a loop on every bus.
    #[test]
    fn user_and_game_layers_multiply_and_the_master_scales_every_bus() {
        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        let samples = constant(1.0, 64);
        mixer.apply(VoiceCommand::LoopSync {
            id: bed(),
            voice: looping(&samples, 1.0, AudioCategory::Sfx),
        });
        mixer.apply(bus_gain(Bus::Sfx, MixLayer::User, 0.5));
        mixer.apply(bus_gain(Bus::Sfx, MixLayer::Game, 0.5));
        assert_eq!(render(&mut mixer, 2), [0.25, 0.25]);

        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        let eighth = constant(0.125, 64);
        for (index, category) in [
            AudioCategory::Sfx,
            AudioCategory::Ambient,
            AudioCategory::Voice,
            AudioCategory::Ui,
        ]
        .into_iter()
        .enumerate()
        {
            mixer.apply(VoiceCommand::LoopSync {
                id: LoopKey::Sound(index as u32),
                voice: looping(&eighth, 1.0, category),
            });
        }
        assert_eq!(render(&mut mixer, 1), [0.5]);
        mixer.apply(bus_gain(Bus::Master, MixLayer::Game, 0.5));
        assert_eq!(render(&mut mixer, 1), [0.25]);
        mixer.apply(bus_gain(Bus::Master, MixLayer::User, 0.5));
        assert_eq!(render(&mut mixer, 1), [0.125]);
    }

    /// A bus gain fade lands exactly on its target, one step per device frame.
    #[test]
    fn a_bus_gain_fade_passes_the_midpoint_and_lands_on_target() {
        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        mixer.apply(VoiceCommand::LoopSync {
            id: bed(),
            voice: looping(&constant(1.0, 64), 1.0, AudioCategory::Sfx),
        });
        mixer.apply(VoiceCommand::BusGain {
            bus: Bus::Sfx as usize,
            layer: MixLayer::User,
            gain: 0.0,
            frames: 4,
        });
        assert_eq!(render(&mut mixer, 6), [0.75, 0.5, 0.25, 0.0, 0.0, 0.0]);
    }

    /// Pause then resume continues the cursor: the output is the unpaused render with the held span
    /// removed, and the held span is silent.
    #[test]
    fn pause_then_resume_continues_the_cursor() {
        let ramp: Arc<Vec<f32>> = Arc::new((1..=8).map(|k| k as f32 * 0.0625).collect());
        let mut free = test_mixer(1, CategoryGains::default(), default_limits());
        free.apply(one_shot_command(&ramp, 1.0));
        let expected = render(&mut free, 8);

        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        mixer.apply(one_shot_command(&ramp, 1.0));
        let mut heard = render(&mut mixer, 3);
        mixer.apply(bus_pause(Bus::Sfx, true, 0));
        assert_eq!(render(&mut mixer, 2000), vec![0.0; 2000], "held span");
        assert_eq!(mixer.voices[0].pos, 3, "the cursor holds");
        mixer.apply(bus_pause(Bus::Sfx, false, 0));
        heard.extend(render(&mut mixer, 5));
        assert_eq!(heard, expected);
    }

    /// A pause fades over its frames, and a held master holds every bus.
    #[test]
    fn a_pause_fades_and_the_master_holds_every_bus() {
        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        mixer.apply(VoiceCommand::LoopSync {
            id: bed(),
            voice: looping(&constant(1.0, 64), 1.0, AudioCategory::Ui),
        });
        mixer.apply(bus_pause(Bus::Master, true, 4));
        assert_eq!(render(&mut mixer, 6), [0.75, 0.5, 0.25, 0.0, 0.0, 0.0]);
        let pos = mixer.loops[&bed()].pos;
        render(&mut mixer, 3);
        assert_eq!(mixer.loops[&bed()].pos, pos, "held under the master");
    }

    /// A paused bus holds its loops and one-shots; the others keep playing.
    #[test]
    fn a_paused_bus_holds_loops_and_one_shots_and_others_play_on() {
        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        let samples = constant(0.5, 64);
        mixer.apply(VoiceCommand::LoopSync {
            id: bed(),
            voice: looping(&samples, 1.0, AudioCategory::Sfx),
        });
        mixer.apply(VoiceCommand::OneShot(one_shot(
            &constant(0.25, 64),
            1.0,
            AudioCategory::Sfx,
        )));
        mixer.apply(VoiceCommand::LoopSync {
            id: LoopKey::Sound(1),
            voice: looping(&constant(0.125, 64), 1.0, AudioCategory::Ambient),
        });
        mixer.apply(bus_pause(Bus::Sfx, true, 0));
        assert_eq!(render(&mut mixer, 3), [0.125; 3]);
        assert_eq!(mixer.loops[&bed()].pos, 0);
        assert_eq!(mixer.voices[0].pos, 0);
        assert_eq!(mixer.loops[&LoopKey::Sound(1)].pos, 3);
    }

    /// A rebuild's blanket stop resets Game and pause, drops voices held on a paused bus, and keeps
    /// the User layer.
    #[test]
    fn stop_all_loops_resets_game_and_pause_and_keeps_user() {
        let mut mixer = test_mixer(1, CategoryGains::default(), default_limits());
        mixer.apply(VoiceCommand::LoopSync {
            id: bed(),
            voice: looping(&constant(1.0, 64), 1.0, AudioCategory::Sfx),
        });
        mixer.apply(bus_gain(Bus::Sfx, MixLayer::User, 0.5));
        mixer.apply(bus_gain(Bus::Sfx, MixLayer::Game, 0.25));
        mixer.apply(bus_pause(Bus::Sfx, true, 0));
        mixer.apply(VoiceCommand::StopAllLoops);
        assert!(
            mixer.loops.is_empty() && mixer.voices.is_empty(),
            "held voice dropped"
        );
        render(&mut mixer, 8);
        let sfx = &mixer.buses[Bus::Sfx as usize];
        assert_eq!(sfx.user.level, 0.5, "the User layer survives");
        assert_eq!(sfx.game.level, 1.0);
        assert_eq!(sfx.pause.level, 1.0);
        assert!(!sfx.paused);
        // A voice started after the rebuild sounds at User x 1.
        mixer.apply(VoiceCommand::LoopSync {
            id: bed(),
            voice: looping(&constant(1.0, 64), 1.0, AudioCategory::Sfx),
        });
        assert_eq!(render(&mut mixer, 1), [0.5]);
    }

    /// The router clamps a bus gain and never schedules a change shorter than the ramp.
    #[test]
    fn the_router_clamps_bus_gains_and_keeps_a_ramp() {
        let mut rig = rig(scratch_root("bus-clamp"));
        for (gain, expect) in [(-1.0, 0.0), (f32::NAN, 0.0), (0.5, 0.5)] {
            let command = rig.router.route(AudioMessage::BusGain {
                bus: Bus::Voice,
                layer: MixLayer::Game,
                gain,
                fade_frames: 0,
            });
            match command {
                Some(VoiceCommand::BusGain {
                    bus, gain, frames, ..
                }) => {
                    assert_eq!((bus, gain), (Bus::Voice as usize, expect));
                    assert_eq!(frames, ramp_frames(44100));
                }
                _ => panic!("a bus gain routes to its command"),
            }
        }
        let none = rig.router.route(AudioMessage::BusPause {
            buses: Vec::new(),
            paused: true,
            fade_frames: 0,
        });
        assert!(none.is_none(), "no buses, no command");
    }

    /// A reopened stream is told the player's volume and any pause, settled; the Game layer a
    /// rebuild reset is not replayed.
    #[test]
    fn a_replay_resends_non_default_bus_state_settled() {
        let mut rig = rig(scratch_root("bus-replay"));
        rig.step(AudioMessage::BusGain {
            bus: Bus::Music,
            layer: MixLayer::User,
            gain: 0.5,
            fade_frames: 6,
        });
        rig.step(AudioMessage::BusGain {
            bus: Bus::Sfx,
            layer: MixLayer::Game,
            gain: 0.25,
            fade_frames: 0,
        });
        rig.step(AudioMessage::BusPause {
            buses: vec![Bus::Ui],
            paused: true,
            fade_frames: 0,
        });
        rig.step(AudioMessage::StopAllLoops);
        rig.step(AudioMessage::BusPause {
            buses: vec![Bus::Ambient],
            paused: true,
            fade_frames: 0,
        });
        while rig.mixer.commands.try_recv().is_ok() {}
        assert!(rig.router.replay(&rig.commands));
        let replayed: Vec<VoiceCommand> = rig.mixer.commands.try_iter().collect();
        let [
            VoiceCommand::BusPause {
                mask,
                paused: true,
                frames: 0,
            },
            VoiceCommand::BusGain {
                bus,
                layer: MixLayer::User,
                gain,
                frames: 0,
            },
        ] = replayed.as_slice()
        else {
            panic!("the ambient pause and the music volume, in bus order, nothing else");
        };
        assert_eq!((*bus, *gain), (Bus::Music as usize, 0.5));
        assert_eq!(*mask, 1 << Bus::Ambient as usize);
    }

    /// A paused music bus never touches the player: the position holds across more renders than the
    /// ring holds, and the music resumes where it stopped.
    #[test]
    fn a_paused_music_bus_holds_its_position() {
        let play = |name: &str| {
            let root = scratch_root(name);
            std::fs::copy(tone(), root.join("tone.ogg")).unwrap();
            let mut rig = rig(root);
            rig.step(music_play("tone.ogg"));
            rig
        };
        let block = |rig: &mut Rig| {
            rig.router.feed.refill();
            let mut out = vec![0.0f32; 2 * MIX_BLOCK_FRAMES];
            rig.mixer.render(&mut out);
            out
        };
        let mut free = play("bus-music-free");
        let expected: Vec<f32> = (0..3).flat_map(|_| block(&mut free)).collect();
        assert!(expected.iter().any(|s| s.abs() > 1e-3), "the tone sounds");

        let mut held = play("bus-music-held");
        let mut heard = block(&mut held);
        held.mixer.apply(bus_pause(Bus::Music, true, 0));
        for _ in 0..(MUSIC_RING_CHUNKS + 8) {
            assert!(block(&mut held).iter().all(|&s| s == 0.0));
        }
        held.mixer.apply(bus_pause(Bus::Music, false, 0));
        heard.extend(block(&mut held));
        heard.extend(block(&mut held));
        assert_eq!(heard, expected);
    }

    /// The callback never allocates: a burst far over every cap leaves each container's capacity
    /// exactly as `Mixer::new` reserved it.
    #[test]
    fn a_burst_over_the_caps_never_grows_the_mixers_storage() {
        let bound = limits(Some(2), 4);
        let mut mixer = test_mixer(2, CategoryGains::default(), bound);
        let capacities = |m: &Mixer| {
            (
                m.voices.capacity(),
                m.arriving.capacity(),
                m.loops.capacity(),
                m.scratch.capacity(),
                m.bus_levels.capacity(),
            )
        };
        let before = capacities(&mixer);
        let shared: Vec<_> = (0..3).map(|_| constant(0.01, 9000)).collect();
        let beds = [constant(0.01, 5000), constant(0.01, 6000)];
        for round in 0..50usize {
            for shot in 0..10 {
                let sound = if shot % 2 == 0 {
                    shared[shot % 3].clone()
                } else {
                    constant(0.01, 4000)
                };
                mixer.apply(one_shot_command(&sound, 0.1));
            }
            for id in 0..7usize {
                let id = LoopKey::Sound(((round + id) % 7) as u32);
                mixer.apply(loop_command(id, &beds[round % 2], 0.1));
            }
            mixer.apply(VoiceCommand::LoopStop(LoopKey::Sound((round % 7) as u32)));
            mixer.apply(VoiceCommand::BusGain {
                bus: round % BUS_COUNT,
                layer: MixLayer::User,
                gain: 0.5,
                frames: round % 9,
            });
            mixer.apply(VoiceCommand::BusPause {
                mask: 1 << (round % BUS_COUNT),
                paused: round % 2 == 0,
                frames: round % 5,
            });
            if round % 5 == 0 {
                mixer.apply(VoiceCommand::StopAllLoops);
            }
            render(&mut mixer, 2 * (3 * MIX_BLOCK_FRAMES) + 6);
            assert_eq!(capacities(&mixer), before, "round {round}");
            assert!(mixer.live() <= bound.total, "round {round}");
            assert!(mixer.voices.len() <= 2 * bound.total, "round {round}");
            assert!(mixer.arriving.is_empty());
        }
    }

    /// The blanket stop a world rebuild sends: through the real mixer every loop and the music
    /// fade out over the ramp — the ring still holds the old generation's chunks to fade through —
    /// and then everything is silent.
    #[test]
    fn stop_all_loops_stops_the_music_too() {
        let root = scratch_root("stop-all");
        std::fs::write(root.join("bed.wav"), wav_bytes(&[8192, 16384])).unwrap();
        std::fs::copy(tone(), root.join("tone.ogg")).unwrap();
        let mut rig = rig(root);
        rig.step(loop_sync("bed.wav", 1.0));
        rig.step(music_play("tone.ogg"));
        let mut out = [0.0f32; 2048];
        rig.mixer.render(&mut out);
        assert!(out.iter().any(|s| s.abs() > 1e-3), "loop and music sound");
        rig.step(AudioMessage::StopAllLoops);
        let mut out = [0.0f32; 1024];
        rig.mixer.render(&mut out);
        assert!(
            out[0] > 0.0 && out[0] < 0.5,
            "it fades rather than cutting: {}",
            out[0]
        );
        // The ramp is 441 frames; the music's float ramp may end a frame after the loops'.
        assert!(out[2 * 441..].iter().all(|&s| s == 0.0), "then silence");
        assert!(rig.mixer.loops.is_empty() && rig.mixer.voices.is_empty());
        // The player went inactive, so its next callback drains the old generation's chunks.
        rig.mixer.render(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));
        assert!(rig.ring.is_empty());
    }

    /// The scene-switch shape: loop, blanket stop, loop the same bed again. The table holds exactly
    /// one voice throughout — a switch never leaks the voice it stopped.
    #[test]
    fn relooping_after_a_blanket_stop_leaves_exactly_one_voice() {
        let buffer = Arc::new(vec![1.0, 1.0]);
        let (mut mixer, _commands) = mixer(CategoryGains::default());
        mixer.apply(loop_command(bed(), &buffer, 1.0));
        mixer.render(&mut [0.0f32; 3]);
        mixer.apply(VoiceCommand::StopAllLoops);
        mixer.apply(loop_command(bed(), &buffer, 0.5));
        assert_eq!(mixer.loops.len(), 1);
        assert_eq!(
            mixer.loops[&bed()].pos,
            0,
            "the re-looped bed starts from the top"
        );
        // Past the 441-frame ramp, only the new bed remains.
        mixer.render(&mut [0.0f32; 2 * 441]);
        let mut out = [0.0f32; 2];
        mixer.render(&mut out);
        assert_eq!(out, [0.5, 0.5], "one voice, not two");
        assert!(mixer.voices.is_empty());
    }

    /// One scene of a sound effect over an ambient bed: each voice is summed through its own bus's
    /// master gain, so the two land at different attenuations. Every value here is dyadic, so the
    /// per-sample sums are exact.
    #[test]
    fn each_voice_is_summed_through_its_own_category_master() {
        let buffer = Arc::new(vec![0.5, 0.5, 0.5, 0.5]);
        let gains = CategoryGains {
            sfx: 1.0,
            ambient: 0.5,
            ..CategoryGains::default()
        };
        let mut voices = vec![one_shot(&buffer, 0.5, AudioCategory::Sfx)];
        let mut loops = HashMap::new();
        loops.insert(
            LoopKey::Sound(0),
            looping(&buffer, 0.5, AudioCategory::Ambient),
        );
        let mut scratch = Vec::new();
        mix_with_gains(&mut voices, &mut loops, &mut scratch, 3, 1, &gains);
        // 0.5 sample x 0.5 gain x 1.0 sfx master, plus 0.5 x 0.5 x 0.5 ambient master.
        assert_eq!(scratch, vec![0.375, 0.375, 0.375]);
    }

    /// The default gains are the identity: the same scene sums to the authored gains alone, so a
    /// host handing the sink `CategoryGains::default()` hears the un-attenuated mixdown.
    #[test]
    fn the_default_category_gains_leave_the_mixdown_untouched() {
        let buffer = Arc::new(vec![0.5, 0.5, 0.5, 0.5]);
        let mut voices = vec![one_shot(&buffer, 0.5, AudioCategory::Sfx)];
        let mut loops = HashMap::new();
        loops.insert(
            LoopKey::Sound(0),
            looping(&buffer, 0.5, AudioCategory::Ambient),
        );
        let mut scratch = Vec::new();
        mix_with_gains(
            &mut voices,
            &mut loops,
            &mut scratch,
            3,
            1,
            &CategoryGains::default(),
        );
        assert_eq!(scratch, vec![0.5, 0.5, 0.5]);
    }

    /// A loop's category is as declarative as its gain: re-syncing the same sound onto another bus
    /// crossfades from the old master to the new one, and the cursor stays where the last callback
    /// left it.
    #[test]
    fn a_loop_moved_to_another_bus_crossfades_without_moving_the_cursor() {
        let buffer = constant(0.5, 4);
        let gains = CategoryGains {
            sfx: 1.0,
            ambient: 0.5,
            ..CategoryGains::default()
        };
        let mut mixer = test_mixer(1, gains, default_limits());
        mixer.apply(VoiceCommand::LoopSync {
            id: bed(),
            voice: looping(&buffer, 1.0, AudioCategory::Ambient),
        });
        assert_eq!(render(&mut mixer, 2), [0.25, 0.25]);

        mixer.apply(loop_command(bed(), &buffer, 1.0));
        assert_eq!(mixer.loops[&bed()].pos, 2, "the cursor survives the sync");
        // The old bus fades out and the new one in, summed: a linear ramp from 0.25 to 0.5.
        assert_eq!(
            render(&mut mixer, 5),
            [0.3125, 0.375, 0.4375, 0.5, 0.5],
            "the sfx master takes over over the ramp"
        );
        assert!(mixer.voices.is_empty());
    }

    /// Below the knee the limiter is exactly the identity, so an ordinary scene — one sound effect
    /// over a bed — is passed through untouched, sign and all.
    #[test]
    fn a_mix_below_the_knee_passes_the_limiter_unchanged() {
        for sample in [0.0, 0.125, -0.125, 0.5, -0.5, LIMITER_KNEE, -LIMITER_KNEE] {
            assert_eq!(soft_limit(sample), sample, "{sample} is below the knee");
        }
    }

    /// The cross-voice failure `collapse_simultaneous` cannot see: four *distinct* sounds fired by
    /// one staged kill. They must land inside full scale, and — the point of a knee over a clamp —
    /// three different sums must stay three different samples instead of one flat-topped run.
    #[test]
    fn four_distinct_sounds_are_limited_without_flat_topping() {
        let buffers: Vec<Arc<Vec<f32>>> = (0..4).map(|_| Arc::new(vec![1.0, 0.9, 0.8])).collect();
        let voices: Vec<Voice> = buffers
            .iter()
            .map(|buffer| one_shot(buffer, 1.0, AudioCategory::Sfx))
            .collect();
        let mut voices = collapsed(voices); // distinct sounds: nothing to collapse
        assert_eq!(voices.len(), 4);

        let mut scratch = Vec::new();
        mix_with_gains(
            &mut voices,
            &mut HashMap::new(),
            &mut scratch,
            3,
            1,
            &CategoryGains::default(),
        );
        assert_eq!(scratch, vec![4.0, 3.6, 3.2]);
        assert!(
            scratch.iter().all(|s| s.clamp(-1.0, 1.0) == 1.0),
            "a hard clamp would flat-top all three — else this test proves nothing",
        );

        let limited: Vec<f32> = scratch.iter().map(|&s| soft_limit(s)).collect();
        assert!(
            limited.iter().all(|s| s.abs() <= 1.0),
            "the summed mix must stay inside full scale: {limited:?}",
        );
        assert!(
            limited[0] > limited[1] && limited[1] > limited[2],
            "distinct sums must stay distinct samples: {limited:?}",
        );
    }

    /// A scratch directory unique to this process + `name`, emptied first so a re-run starts clean.
    fn scratch_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ludion-audio-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A minimal mono PCM16 WAV carrying `samples` — the smallest thing `load_wav` accepts, so the
    /// cache tests exercise the real decode path rather than a stub.
    fn wav_bytes(samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // uncompressed PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&44100u32.to_le_bytes());
        out.extend_from_slice(&88200u32.to_le_bytes()); // byte rate
        out.extend_from_slice(&2u16.to_le_bytes()); // block align
        out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    /// The stereo Ogg fixture the music tests play.
    fn tone() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/tone-stereo-44k.ogg"
        ))
    }

    /// A sink session with no device: a router and a mixer wired over real channels at 44.1 kHz
    /// stereo, 60 ticks a second, default gains.
    struct Rig {
        router: Router,
        mixer: Mixer,
        commands: Sender<VoiceCommand>,
        ring: Receiver<MusicChunk>,
    }

    fn rig(root: PathBuf) -> Rig {
        let (commands, command_rx) = crossbeam_channel::unbounded();
        let (music_tx, ring) = crossbeam_channel::bounded(MUSIC_RING_CHUNKS);
        Rig {
            router: Router::new(
                root,
                44100,
                2,
                1.0 / 60.0,
                VoiceCaps::default().total,
                music_tx,
            ),
            mixer: Mixer::new(
                command_rx,
                ring.clone(),
                2,
                44100,
                CategoryGains::default(),
                default_limits(),
            ),
            commands,
            ring,
        }
    }

    impl Rig {
        /// Delivers one message to the router, as one wake of the sink thread.
        fn step(&mut self, message: AudioMessage) {
            self.router.step(Some(message), &self.commands);
        }
    }

    /// A mixer on its own, with a dummy music ring.
    fn mixer(gains: CategoryGains) -> (Mixer, Sender<VoiceCommand>) {
        let (commands, command_rx) = crossbeam_channel::unbounded();
        let (_music_tx, music_rx) = crossbeam_channel::bounded(MUSIC_RING_CHUNKS);
        (
            Mixer::new(command_rx, music_rx, 2, 44100, gains, default_limits()),
            commands,
        )
    }

    fn one_shot_message(sound: &str, gain: f32, category: AudioCategory) -> AudioMessage {
        AudioMessage::OneShot(AudioEvent {
            sound: sound.to_string(),
            gain,
            pan: 0.0,
            category,
        })
    }

    fn loop_sync(sound: &str, gain: f32) -> AudioMessage {
        AudioMessage::LoopSync {
            id: LoopId::Sound(sound.to_string()),
            sound: sound.to_string(),
            gain,
            pan: 0.0,
            category: AudioCategory::Ambient,
        }
    }

    fn music_play(track: &str) -> AudioMessage {
        AudioMessage::MusicPlay {
            track: track.to_string(),
            gain: 1.0,
            looping: true,
            fade_frames: 0,
        }
    }

    /// Message to samples: a one-shot sounds at its authored gain (16384 / 32768 x 0.5 = 0.25, each
    /// mono sample spread to both channels), runs out, and its voice retires.
    #[test]
    fn a_one_shot_message_is_heard_once_then_retires() {
        let root = scratch_root("one-shot");
        std::fs::write(root.join("hit.wav"), wav_bytes(&[16384, -16384, 8192])).unwrap();
        let mut rig = rig(root);
        rig.step(one_shot_message("hit.wav", 0.5, AudioCategory::Sfx));
        let mut out = [1.0f32; 8];
        rig.mixer.render(&mut out);
        assert_eq!(out, [0.25, 0.25, -0.25, -0.25, 0.125, 0.125, 0.0, 0.0]);
        rig.mixer.render(&mut out);
        assert_eq!(out, [0.0; 8]);
        assert!(rig.mixer.voices.is_empty());
    }

    /// A loop sync wraps at the end of its buffer until its loop stop removes it.
    #[test]
    fn a_loop_sync_loops_until_its_loop_stop() {
        let root = scratch_root("loop-sync");
        std::fs::write(root.join("bed.wav"), wav_bytes(&[8192, 16384])).unwrap();
        let mut rig = rig(root);
        rig.step(loop_sync("bed.wav", 1.0));
        let mut out = [0.0f32; 10];
        rig.mixer.render(&mut out);
        assert_eq!(
            out,
            [0.25, 0.25, 0.5, 0.5, 0.25, 0.25, 0.5, 0.5, 0.25, 0.25]
        );
        rig.step(AudioMessage::LoopStop {
            id: LoopId::Sound("bed.wav".to_string()),
        });
        let mut out = [0.0f32; 1024];
        rig.mixer.render(&mut out);
        assert!(
            out[0] > 0.0 && out[0] < 0.5,
            "it fades rather than cutting: {}",
            out[0]
        );
        assert!(out[2 * 441..].iter().all(|&s| s == 0.0), "then silence");
        assert!(rig.mixer.loops.is_empty() && rig.mixer.voices.is_empty());
    }

    #[test]
    fn an_unloadable_sound_routes_to_nothing() {
        let mut rig = rig(scratch_root("unloadable"));
        assert!(
            rig.router
                .route(one_shot_message("missing.wav", 1.0, AudioCategory::Sfx))
                .is_none()
        );
        assert!(rig.router.route(loop_sync("missing.wav", 1.0)).is_none());
    }

    /// A loop past the voice total is held by the router, not lost: it starts when a loop stops, at
    /// its latest gain, and a loop stopped while it waited never starts.
    #[test]
    fn a_loop_held_at_the_voice_cap_starts_when_a_slot_frees() {
        let root = scratch_root("loop-cap");
        std::fs::write(root.join("bed.wav"), wav_bytes(&[8192, 16384])).unwrap();
        let mut rig = rig(root);
        let entity = |index: u32| LoopId::Entity(EntityId::new(index, 0));
        for index in 0..=64 {
            rig.step(loop_message(entity(index), "bed.wav", 1.0));
        }
        render_once(&mut rig.mixer);
        let waiting = rig.router.key(entity(64));
        assert_eq!(rig.mixer.loops.len(), 64);
        assert!(!rig.mixer.loops.contains_key(&waiting));

        rig.step(loop_message(entity(64), "bed.wav", 0.5));
        let stopped_waiting = rig.router.key(entity(65));
        rig.step(loop_message(entity(65), "bed.wav", 1.0));
        rig.step(AudioMessage::LoopStop { id: entity(65) });
        rig.step(AudioMessage::LoopStop { id: entity(0) });
        render_once(&mut rig.mixer);
        let loops = &rig.mixer.loops;
        assert_eq!(loops.len(), 64);
        assert_eq!(loops[&waiting].envelope.target, 0.5);
        assert!(!loops.contains_key(&stopped_waiting));
    }

    /// The callback frees nothing: a buffer the cache let go of while a voice held it is kept by
    /// the router until that voice is gone, and dropped on the sink thread.
    #[test]
    fn the_callback_never_drops_the_last_reference_to_a_buffer() {
        let root = scratch_root("last-reference");
        let path = root.join("hit.wav");
        std::fs::write(&path, wav_bytes(&[16384])).unwrap();
        let mut rig = rig(root);
        rig.router.recheck = Duration::ZERO;
        rig.step(one_shot_message("hit.wav", 1.0, AudioCategory::Sfx));
        let weak = Arc::downgrade(rig.router.cache["hit.wav"].buffer.as_ref().unwrap());

        std::fs::write(&path, wav_bytes(&[16384, 8192])).unwrap();
        rig.step(one_shot_message("hit.wav", 1.0, AudioCategory::Sfx));
        for _ in 0..4 {
            render_once(&mut rig.mixer);
        }
        assert!(rig.mixer.voices.is_empty());
        assert!(
            weak.upgrade().is_some(),
            "the callback's drop was the last one"
        );
        rig.router.step(None, &rig.commands);
        assert!(weak.upgrade().is_none(), "the sink thread freed it");
    }

    /// A music play that fails still wakes the refill: the ring of the track already playing is
    /// topped up, not left to drain.
    #[test]
    fn a_failed_music_play_still_tops_the_ring_up() {
        let root = scratch_root("failed-music");
        std::fs::copy(tone(), root.join("tone.ogg")).unwrap();
        let mut rig = rig(root);
        rig.step(music_play("tone.ogg"));
        assert_eq!(rig.ring.len(), MUSIC_RING_CHUNKS);
        for _ in 0..10 {
            rig.ring.try_recv().unwrap();
        }
        rig.step(music_play("missing.ogg"));
        assert_eq!(rig.ring.len(), MUSIC_RING_CHUNKS);
    }

    const STEREO_44K: DeviceFormat = DeviceFormat {
        rate: 44100,
        channels: 2,
    };
    const MONO_48K: DeviceFormat = DeviceFormat {
        rate: 48000,
        channels: 1,
    };

    /// An output that answers opens from a script (the last answer repeats; `None` fails) and hands
    /// every started mixer to the test, which holds it so its queue stays connected.
    struct FakeOutput {
        opens: Vec<Option<DeviceFormat>>,
        next: usize,
        attempts: usize,
        started: Sender<Mixer>,
    }

    impl Output for FakeOutput {
        type Stream = ();

        fn open(&mut self) -> Result<DeviceFormat, Diagnostic> {
            self.attempts += 1;
            let answer = self.opens[self.next.min(self.opens.len() - 1)];
            self.next += 1;
            answer.ok_or_else(|| device_unavailable("fake: no device"))
        }

        fn start(&mut self, mixer: Mixer) -> Result<(), Diagnostic> {
            let _ = self.started.send(mixer);
            Ok(())
        }
    }

    /// A playback over a `FakeOutput`, the mixers it starts, and the clock's origin.
    fn playback(
        root: PathBuf,
        opens: Vec<Option<DeviceFormat>>,
    ) -> (Playback<FakeOutput>, Receiver<Mixer>, Instant) {
        let (started, mixers) = crossbeam_channel::unbounded();
        let output = FakeOutput {
            opens,
            next: 0,
            attempts: 0,
            started,
        };
        let t0 = Instant::now();
        let playback = Playback::start(
            output,
            root,
            CategoryGains::default(),
            VoiceCaps::default(),
            1.0 / 60.0,
            t0,
        );
        (playback, mixers, t0)
    }

    fn render_once(mixer: &mut Mixer) {
        mixer.render(&mut [0.0f32; 8]);
    }

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    /// Fails `lost`, then wakes the playback when its reopen is due, and returns the new mixer.
    fn reopen(
        playback: &mut Playback<FakeOutput>,
        mixers: &Receiver<Mixer>,
        lost: &Mixer,
        now: &mut Instant,
    ) -> Mixer {
        lost.health
            .error
            .set("fake: device lost".to_string())
            .unwrap();
        playback.wake(None, *now);
        *now += playback.wait(*now);
        playback.wake(None, *now);
        mixers.try_recv().expect("the reopen started a stream")
    }

    /// The commands queued in `mixer`, in order.
    fn queued(mixer: &Mixer) -> Vec<VoiceCommand> {
        mixer.commands.try_iter().collect()
    }

    fn l5002_count(playback: &Playback<FakeOutput>) -> usize {
        playback
            .reports
            .iter()
            .filter(|line| line.contains("L5002"))
            .count()
    }

    /// The `n`th message of an alternating sync/stop run on one loop: syncs at even `n`.
    fn sync_or_stop(n: usize) -> AudioMessage {
        let bed = LoopId::Sound("bed.wav".to_string());
        if n.is_multiple_of(2) {
            loop_message(bed, "bed.wav", 0.5)
        } else {
            AudioMessage::LoopStop { id: bed }
        }
    }

    /// A playback whose first stream has rendered once, and that stream's mixer, with `bed.wav` and
    /// `hit.wav` under its root.
    fn rendered_playback(name: &str) -> (Playback<FakeOutput>, Receiver<Mixer>, Mixer, Instant) {
        let root = scratch_root(name);
        std::fs::write(root.join("hit.wav"), wav_bytes(&[16384])).unwrap();
        std::fs::write(root.join("bed.wav"), wav_bytes(&[8192, 16384])).unwrap();
        let (mut playback, mixers, t0) = playback(root, vec![Some(STEREO_44K)]);
        let mut held = mixers.try_recv().unwrap();
        render_once(&mut held);
        playback.wake(None, t0);
        (playback, mixers, held, t0)
    }

    #[test]
    fn a_one_shot_burst_sheds_one_shots_and_keeps_the_stream() {
        let (mut playback, _mixers, held, t0) = rendered_playback("burst");
        for _ in 0..2 * COMMAND_CAPACITY {
            playback.wake(
                Some(one_shot_message("hit.wav", 1.0, AudioCategory::Sfx)),
                t0,
            );
        }
        assert_eq!(held.commands.len(), ONE_SHOT_HIGH_WATER);
        assert!(playback.live.is_some());
        assert!(playback.reports.is_empty());

        // With no further render the stall verdict still comes, once.
        playback.wake(None, t0 + STALL_AFTER);
        assert!(playback.live.is_none());
        assert_eq!(l5002_count(&playback), 1);
        assert_eq!(playback.reports.len(), 1);
        assert!(playback.reports[0].contains("no callback"));
    }

    #[test]
    fn state_commands_past_the_queue_wait_in_the_backlog_in_order() {
        let (mut playback, _mixers, held, t0) = rendered_playback("backlog");
        for n in 0..2 * COMMAND_CAPACITY {
            playback.wake(Some(sync_or_stop(n)), t0);
        }
        assert_eq!(held.commands.len(), COMMAND_CAPACITY);
        assert_eq!(playback.router.backlog.len(), COMMAND_CAPACITY);
        assert!(playback.live.is_some());
        assert!(playback.reports.is_empty());

        let mut delivered = queued(&held);
        while !playback.router.backlog.is_empty() {
            playback.wake(None, t0);
            delivered.extend(queued(&held));
        }
        assert_eq!(delivered.len(), 2 * COMMAND_CAPACITY);
        for (n, command) in delivered.iter().enumerate() {
            if n.is_multiple_of(2) {
                assert!(matches!(command, VoiceCommand::LoopSync { .. }), "{n}");
            } else {
                assert!(matches!(command, VoiceCommand::LoopStop(_)), "{n}");
            }
        }
        assert!(playback.live.is_some());
    }

    #[test]
    fn a_stall_with_a_backlog_drops_it_and_the_replay_covers_the_state() {
        let (mut playback, mixers, held, t0) = rendered_playback("backlog-stall");
        for n in 0..=2 * COMMAND_CAPACITY {
            playback.wake(Some(sync_or_stop(n)), t0);
        }
        assert!(!playback.router.backlog.is_empty());
        let mut now = t0 + STALL_AFTER;
        playback.wake(None, now);
        assert!(playback.live.is_none());
        assert!(playback.router.backlog.is_empty());
        drop(held);

        now += playback.wait(now);
        playback.wake(None, now);
        let reopened = mixers.try_recv().expect("the reopen started a stream");
        let commands = queued(&reopened);
        assert_eq!(commands.len(), 1, "only the replay: {}", commands.len());
        assert!(matches!(commands[0], VoiceCommand::LoopSync { .. }));
    }

    #[test]
    fn a_dropped_callback_is_a_lost_stream() {
        let (mut playback, _mixers, held, t0) = rendered_playback("callback-gone");
        drop(held);
        assert!(playback.live.is_some());
        playback.wake(Some(sync_or_stop(0)), t0);
        assert!(playback.live.is_none());
        assert_eq!(l5002_count(&playback), 1);
        assert!(playback.reports[0].contains("callback is gone"));
    }

    #[test]
    fn a_stream_that_opens_after_a_failed_start_but_never_renders_recovers_as_l5001() {
        let root = scratch_root("failed-start-stall");
        let (mut playback, mixers, t0) = playback(root, vec![None, Some(STEREO_44K)]);
        let mut now = t0 + playback.wait(t0);
        playback.wake(None, now);
        let _stuck = mixers.try_recv().expect("the second open started a stream");
        assert!(playback.live.is_some());

        now += START_WITHIN;
        playback.wake(None, now);
        assert!(playback.live.is_none(), "no first callback: lost");
        assert_eq!(l5002_count(&playback), 0, "L5002 was never reported");

        now += playback.wait(now);
        playback.wake(None, now);
        let mut mixer = mixers.try_recv().expect("the reopen started a stream");
        render_once(&mut mixer);
        playback.wake(None, now + ms(1));
        assert_eq!(l5002_count(&playback), 0);
        assert!(
            playback
                .reports
                .last()
                .is_some_and(|line| line.starts_with("note: the audio output reopened after L5001")),
            "{:?}",
            playback.reports
        );
    }

    #[test]
    fn a_session_that_starts_without_a_device_keeps_state_and_retries() {
        let root = scratch_root("failed-start");
        std::fs::write(root.join("bed.wav"), wav_bytes(&[8192, 16384])).unwrap();
        std::fs::copy(tone(), root.join("tone.ogg")).unwrap();
        let (mut playback, mixers, t0) = playback(root, vec![None, None, Some(STEREO_44K)]);
        assert!(playback.live.is_none());
        assert_eq!(playback.reports.len(), 1);
        assert!(playback.reports[0].contains("L5001"));
        assert_eq!(l5002_count(&playback), 0);

        let bed = LoopId::Sound("bed.wav".to_string());
        playback.wake(Some(loop_message(bed, "bed.wav", 0.5)), t0);
        playback.wake(Some(music_play("tone.ogg")), t0);
        playback.wake(
            Some(one_shot_message("missing.wav", 1.0, AudioCategory::Sfx)),
            t0,
        );
        assert_eq!(playback.output.attempts, 1, "the first retry is not due");

        let mut now = t0;
        for expected in [2, 3] {
            now += playback.wait(now);
            playback.wake(None, now);
            assert_eq!(playback.output.attempts, expected);
        }
        let mut mixer = mixers.try_recv().expect("the third open started a stream");
        let commands = queued(&mixer);
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, VoiceCommand::LoopSync { .. }))
        );
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, VoiceCommand::MusicStart { .. }))
        );

        render_once(&mut mixer);
        playback.wake(None, now + ms(1));
        assert_eq!(l5002_count(&playback), 0);
        assert!(
            playback.reports[1].starts_with("note: the audio output reopened after L5001"),
            "{:?}",
            playback.reports
        );
    }

    #[test]
    fn a_rendered_stream_that_goes_quiet_is_lost_after_the_stall_window() {
        let root = scratch_root("stall-window");
        let (mut playback, mixers, t0) = playback(root.clone(), vec![Some(STEREO_44K)]);
        let mut mixer = mixers.try_recv().unwrap();
        render_once(&mut mixer);
        playback.wake(None, t0 + ms(100));
        assert!(playback.live.is_some());
        playback.wake(None, t0 + ms(100) + STALL_AFTER - ms(1));
        assert!(playback.live.is_some(), "inside the window");
        playback.wake(None, t0 + ms(100) + STALL_AFTER);
        assert!(playback.live.is_none());
        assert_eq!(l5002_count(&playback), 1);

        // A stream that never rendered is given the start window, then lost.
        let (mut slow, _mixers, t0) = self::playback(root, vec![Some(STEREO_44K)]);
        slow.wake(None, t0 + START_WITHIN - ms(1));
        assert!(slow.live.is_some());
        assert!(slow.reports.is_empty());
        slow.wake(None, t0 + START_WITHIN);
        assert!(slow.live.is_none());
        assert_eq!(l5002_count(&slow), 1);
        assert!(slow.reports[0].contains("no first callback"));
    }

    /// A slow device that has not started yet gets the state commands but not a flood of one-shots,
    /// which would play stale anyway: past half the queue they are dropped.
    #[test]
    fn a_stream_that_has_not_started_keeps_half_its_queue_for_state() {
        let root = scratch_root("prestart");
        std::fs::write(root.join("hit.wav"), wav_bytes(&[16384])).unwrap();
        std::fs::write(root.join("bed.wav"), wav_bytes(&[8192, 16384])).unwrap();
        std::fs::copy(tone(), root.join("tone.ogg")).unwrap();
        let (mut playback, mixers, t0) = playback(root, vec![Some(STEREO_44K)]);
        let held = mixers.try_recv().unwrap();
        for _ in 0..2 * COMMAND_CAPACITY {
            playback.wake(
                Some(one_shot_message("hit.wav", 1.0, AudioCategory::Sfx)),
                t0,
            );
        }
        let bed = LoopId::Sound("bed.wav".to_string());
        playback.wake(Some(loop_message(bed, "bed.wav", 0.5)), t0);
        playback.wake(Some(music_play("tone.ogg")), t0);
        assert!(playback.live.is_some());
        assert!(playback.reports.is_empty());
        assert_eq!(held.commands.len(), COMMAND_CAPACITY / 2 + 2);
        let queue = queued(&held);
        assert!(matches!(
            queue[queue.len() - 2],
            VoiceCommand::LoopSync { .. }
        ));
        assert!(matches!(
            queue[queue.len() - 1],
            VoiceCommand::MusicStart { generation: 1, .. }
        ));
    }

    /// A device that cannot stay up keeps backing off: the first render does not reset the wait.
    #[test]
    fn a_flapping_output_backs_off_instead_of_cycling() {
        let root = scratch_root("flapping");
        let (mut playback, mixers, mut now) = playback(root, vec![Some(STEREO_44K)]);
        let mut mixer = mixers.try_recv().unwrap();
        let mut waits = Vec::new();
        for _ in 0..3 {
            render_once(&mut mixer);
            playback.wake(None, now);
            mixer
                .health
                .error
                .set("fake: device lost".to_string())
                .unwrap();
            playback.wake(None, now);
            waits.push(playback.wait(now));
            now += playback.wait(now);
            playback.wake(None, now);
            mixer = mixers.try_recv().expect("the reopen started a stream");
        }
        assert_eq!(waits, [ms(250), ms(500), ms(1000)]);

        render_once(&mut mixer);
        playback.wake(None, now);
        assert_eq!(playback.delay, ms(2000), "still backing off");
        render_once(&mut mixer);
        playback.wake(None, now + HEALTHY_AFTER);
        assert_eq!(
            playback.delay, RETRY_FIRST,
            "a stream that stayed up resets"
        );
    }

    #[test]
    fn a_lost_stream_retries_with_backoff_and_reports_each_change_once() {
        let root = scratch_root("backoff");
        std::fs::write(root.join("hit.wav"), wav_bytes(&[16384])).unwrap();
        let mut opens = vec![Some(STEREO_44K)];
        opens.extend([None; 5]);
        opens.push(Some(STEREO_44K));
        let (mut playback, mixers, mut now) = playback(root, opens);
        let first = mixers.try_recv().unwrap();
        first
            .health
            .error
            .set("fake: device lost".to_string())
            .unwrap();
        playback.wake(None, now);
        assert_eq!(playback.reports.len(), 1);
        assert_eq!(l5002_count(&playback), 1);

        for (index, wait) in [250, 500, 1000, 2000, 4000, 5000].into_iter().enumerate() {
            assert_eq!(playback.wait(now), ms(wait));
            playback.wake(None, now + ms(wait - 1));
            assert_eq!(
                playback.output.attempts,
                1 + index,
                "a wake 1 ms early waits"
            );
            now += ms(wait);
            let hit = Some(one_shot_message("hit.wav", 1.0, AudioCategory::Sfx));
            // The last attempt succeeds; its wake carries no message.
            playback.wake(if index < 5 { hit } else { None }, now);
            assert_eq!(playback.output.attempts, 2 + index);
        }
        assert_eq!(playback.reports.len(), 1, "failed retries report nothing");
        assert_eq!(
            first.commands.len(),
            0,
            "a message woken while lost reaches no mixer"
        );

        let mut second = mixers
            .try_recv()
            .expect("the sixth attempt opened a stream");
        assert!(playback.live.is_some());
        playback.wake(None, now + ms(1));
        assert_eq!(
            playback.reports.len(),
            1,
            "no note until the stream renders"
        );
        render_once(&mut second);
        playback.wake(None, now + ms(2));
        assert_eq!(playback.reports.len(), 2);
        assert!(
            playback.reports[1]
                .starts_with("note: the audio output reopened after L5002 (44100 Hz, 2 ch)")
        );
        render_once(&mut second);
        playback.wake(None, now + ms(3));
        assert_eq!(playback.reports.len(), 2);
        assert_eq!(
            playback.delay, RETRY_MAX,
            "the first render does not reset the backoff"
        );
    }

    fn loop_message(id: LoopId, sound: &str, gain: f32) -> AudioMessage {
        AudioMessage::LoopSync {
            id,
            sound: sound.to_string(),
            gain,
            pan: 0.0,
            category: AudioCategory::Ambient,
        }
    }

    #[test]
    fn a_reopened_mixer_replays_the_declared_loops_and_resumes_the_music() {
        let root = scratch_root("replay");
        for name in ["bed.wav", "hum.wav", "drone.wav"] {
            std::fs::write(root.join(name), wav_bytes(&[8192, 16384])).unwrap();
        }
        std::fs::copy(tone(), root.join("tone.ogg")).unwrap();
        let (mut playback, mixers, t0) = playback(root, vec![Some(STEREO_44K)]);
        let first = mixers.try_recv().unwrap();
        let hum = LoopId::Entity(EntityId::new(7, 0));
        let bed = LoopId::Sound("bed.wav".to_string());
        let drone = LoopId::Sound("drone.wav".to_string());
        for message in [
            loop_message(bed.clone(), "bed.wav", 0.5),
            loop_message(hum.clone(), "hum.wav", 1.0),
            AudioMessage::LoopStop { id: hum },
            loop_message(drone.clone(), "drone.wav", 1.0),
            AudioMessage::MusicPlay {
                track: "tone.ogg".to_string(),
                gain: 0.7,
                looping: true,
                fade_frames: 0,
            },
        ] {
            playback.wake(Some(message), t0);
        }
        let first_generation = queued(&first)
            .into_iter()
            .find_map(|command| match command {
                VoiceCommand::MusicStart { generation, .. } => Some(generation),
                _ => None,
            })
            .expect("the music started");

        let mut now = t0;
        let second = reopen(&mut playback, &mixers, &first, &mut now);
        let bed_key = playback.router.key(bed);
        let drone_key = playback.router.key(drone);
        let mut loops = Vec::new();
        let mut music = Vec::new();
        for command in queued(&second) {
            match command {
                VoiceCommand::LoopSync { id, voice } => {
                    assert_eq!(voice.envelope.level, 0.0, "a replayed loop fades in");
                    assert_eq!(voice.envelope.left, ramp_frames(44100));
                    loops.push((id, voice.envelope.target));
                }
                VoiceCommand::MusicStart {
                    generation, gain, ..
                } => music.push((generation, gain)),
                _ => panic!("only loops and the music are replayed"),
            }
        }
        loops.sort_by_key(|&(id, _)| format!("{id:?}"));
        let mut expected = vec![(bed_key, 0.5), (drone_key, 1.0)];
        expected.sort_by_key(|&(id, _)| format!("{id:?}"));
        assert_eq!(loops, expected, "the stopped hum is not replayed");
        assert_eq!(music, vec![(first_generation + 1, 0.7)]);

        // Once every loop and the music are stopped, there is nothing to replay.
        playback.wake(Some(AudioMessage::StopAllLoops), now);
        let third = reopen(&mut playback, &mixers, &second, &mut now);
        assert!(queued(&third).is_empty());
    }

    #[test]
    fn a_reopen_at_another_format_redecodes_for_it() {
        let root = scratch_root("format-change");
        std::fs::write(root.join("bed.wav"), wav_bytes(&[8192, 16384, -8192, 0])).unwrap();
        let (mut playback, mixers, t0) =
            playback(root.clone(), vec![Some(STEREO_44K), Some(MONO_48K)]);
        let first = mixers.try_recv().unwrap();
        playback.wake(
            Some(loop_message(
                LoopId::Sound("bed.wav".to_string()),
                "bed.wav",
                1.0,
            )),
            t0,
        );
        let old = match queued(&first).pop() {
            Some(VoiceCommand::LoopSync { voice, .. }) => voice.samples.len(),
            _ => panic!("the loop synced"),
        };
        let pcm = load_wav(ludion_assets::AssetRoot::content(&root), "bed.wav").unwrap();
        assert_eq!(old, to_device_buffer(&pcm, 44100, 2).len());

        let mut now = t0;
        let second = reopen(&mut playback, &mixers, &first, &mut now);
        let new = match queued(&second).pop() {
            Some(VoiceCommand::LoopSync { voice, .. }) => voice.samples.len(),
            _ => panic!("the loop replayed"),
        };
        assert_eq!(new, to_device_buffer(&pcm, 48000, 1).len());
        assert_ne!(new, old);
    }

    /// A format change evicts the buffers decoded for the old device, but the old stream may still
    /// hold them: the router keeps them until a wake finds them unshared.
    #[test]
    fn a_buffer_evicted_by_a_format_change_outlives_the_old_stream() {
        let root = scratch_root("evicted-buffer");
        std::fs::write(root.join("bed.wav"), wav_bytes(&[8192, 16384, -8192, 0])).unwrap();
        let (mut playback, mixers, t0) = playback(root, vec![Some(STEREO_44K), Some(MONO_48K)]);
        let first = mixers.try_recv().unwrap();
        playback.wake(
            Some(loop_message(
                LoopId::Sound("bed.wav".to_string()),
                "bed.wav",
                1.0,
            )),
            t0,
        );
        let weak = Arc::downgrade(playback.router.cache["bed.wav"].buffer.as_ref().unwrap());

        let mut now = t0;
        let _second = reopen(&mut playback, &mixers, &first, &mut now);
        drop(first);
        assert!(weak.upgrade().is_some(), "kept past the old stream");
        playback.wake(None, now);
        assert!(weak.upgrade().is_none(), "freed on the sink thread");
    }

    /// A lost stream still drains every message but queues nothing; the control run proves the same
    /// messages do produce commands while the stream lives.
    #[test]
    fn a_lost_stream_drains_every_message_and_queues_nothing() {
        let root = scratch_root("lost-stream");
        std::fs::write(root.join("hit.wav"), wav_bytes(&[16384])).unwrap();
        std::fs::copy(tone(), root.join("tone.ogg")).unwrap();
        let drive = |lost: bool| {
            let (mut playback, mixers, _) = playback(root.clone(), vec![Some(STEREO_44K), None]);
            let held = mixers.try_recv().unwrap();
            if lost {
                held.health
                    .error
                    .set("fake: device lost".to_string())
                    .unwrap();
            }
            let (events_tx, events) = crossbeam_channel::unbounded();
            for message in [
                one_shot_message("hit.wav", 1.0, AudioCategory::Sfx),
                loop_sync("hit.wav", 1.0),
                music_play("tone.ogg"),
            ] {
                events_tx.send(message).unwrap();
            }
            drop(events_tx);
            serve(&events, &mut playback);
            assert!(events.is_empty(), "every message was drained");
            held.commands.len()
        };
        assert_eq!(drive(true), 0);
        assert!(drive(false) >= 3);
    }

    /// A failed decode is cached exactly like a successful one, so a bad `sound` — or an emitter
    /// left at `sound: ""` — costs one open, not one per message for the whole run.
    #[test]
    fn a_failed_decode_is_cached_instead_of_retried() {
        let root = scratch_root("negative-cache");
        std::fs::write(root.join("broken.wav"), b"not a wav at all").unwrap();
        let mut cache = HashMap::new();

        assert!(
            resolve(
                &mut cache,
                &mut Vec::new(),
                &root,
                "broken.wav",
                44100,
                1,
                Duration::ZERO
            )
            .is_none()
        );
        assert!(
            resolve(
                &mut cache,
                &mut Vec::new(),
                &root,
                "missing.wav",
                44100,
                1,
                Duration::ZERO
            )
            .is_none()
        );
        assert_eq!(cache.len(), 2, "both failures are remembered");
        assert!(cache["broken.wav"].buffer.is_none());
        assert!(
            cache["missing.wav"].stamp.stamp.is_none(),
            "a file with no metadata stamps the same way every time, so it is not re-opened",
        );

        // The same stamp: the second ask is answered from the cache.
        let before = cache["broken.wav"].stamp.stamp;
        assert!(
            resolve(
                &mut cache,
                &mut Vec::new(),
                &root,
                "broken.wav",
                44100,
                1,
                Duration::ZERO
            )
            .is_none()
        );
        assert_eq!(cache["broken.wav"].stamp.stamp, before);
    }

    /// The cache is keyed on the file, not just its path: a sound regenerated under a running host
    /// is picked up on its next message — including a broken one that was fixed.
    #[test]
    fn a_rewritten_sound_is_decoded_again() {
        let root = scratch_root("cache-invalidation");
        let path = root.join("hit.wav");
        let mut cache = HashMap::new();

        std::fs::write(&path, b"not a wav yet").unwrap();
        assert!(
            resolve(
                &mut cache,
                &mut Vec::new(),
                &root,
                "hit.wav",
                44100,
                1,
                Duration::ZERO
            )
            .is_none()
        );

        std::fs::write(&path, wav_bytes(&[0, i16::MAX])).unwrap();
        let fixed = resolve(
            &mut cache,
            &mut Vec::new(),
            &root,
            "hit.wav",
            44100,
            1,
            Duration::ZERO,
        )
        .expect("the repaired file plays without a restart");
        assert_eq!(fixed.len(), 2);

        std::fs::write(&path, wav_bytes(&[0, i16::MAX, 0, i16::MIN])).unwrap();
        let regenerated = resolve(
            &mut cache,
            &mut Vec::new(),
            &root,
            "hit.wav",
            44100,
            1,
            Duration::ZERO,
        )
        .expect("the regenerated file decodes again");
        assert_eq!(regenerated.len(), 4, "the new bytes, not the cached ones");
        assert_eq!(cache.len(), 1, "one entry per path throughout");
    }

    /// A stamp is a `stat`: inside the recheck window a cached sound is answered without one, so a
    /// rewrite waits for the window and is picked up by the first ask after it.
    #[test]
    fn a_cached_sound_is_not_restamped_within_the_recheck_window() {
        let root = scratch_root("recheck-window");
        let path = root.join("hit.wav");
        std::fs::write(&path, wav_bytes(&[0, i16::MAX])).unwrap();
        let hour = Duration::from_secs(3600);
        let mut cache = HashMap::new();
        let first = resolve(
            &mut cache,
            &mut Vec::new(),
            &root,
            "hit.wav",
            44100,
            1,
            hour,
        )
        .unwrap();

        std::fs::write(&path, wav_bytes(&[0, i16::MAX, 0, i16::MIN])).unwrap();
        let again = resolve(
            &mut cache,
            &mut Vec::new(),
            &root,
            "hit.wav",
            44100,
            1,
            hour,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&first, &again), "inside the window: no stat");

        let due = resolve(
            &mut cache,
            &mut Vec::new(),
            &root,
            "hit.wav",
            44100,
            1,
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(due.len(), 4, "once due, the new file");
    }

    /// Every sink-side stamp cache is built with the real recheck window.
    #[test]
    fn the_sinks_caches_default_to_the_stamp_recheck_window() {
        let root = scratch_root("default-window");
        assert_eq!(rig(root).router.recheck, STAMP_RECHECK);
    }

    /// Loop names are interned on the sink thread: the callback's key is `Copy`, one number per name,
    /// shared by a name's syncs and its stop.
    #[test]
    fn a_named_loop_is_keyed_by_a_copy_id_on_the_callback_side() {
        let root = scratch_root("loop-keys");
        std::fs::write(root.join("bed.wav"), wav_bytes(&[8192, 16384])).unwrap();
        std::fs::write(root.join("wind.wav"), wav_bytes(&[8192, 16384])).unwrap();
        let mut rig = rig(root);
        let key_of = |command: Option<VoiceCommand>| match command {
            Some(VoiceCommand::LoopSync { id, .. } | VoiceCommand::LoopStop(id)) => id,
            _ => panic!("a loop command"),
        };
        let first = key_of(rig.router.route(loop_sync("bed.wav", 1.0)));
        let second = key_of(rig.router.route(loop_sync("bed.wav", 0.5)));
        let stop = key_of(rig.router.route(AudioMessage::LoopStop {
            id: LoopId::Sound("bed.wav".to_string()),
        }));
        let other = key_of(rig.router.route(loop_sync("wind.wav", 1.0)));
        assert!(matches!(first, LoopKey::Sound(_)));
        assert_eq!((first, first), (second, stop));
        assert_ne!(first, other);
    }

    /// A session over a `FakeOutput` on `pool`, and the mixers it starts.
    ///
    /// Sink-lifecycle tests never open a real device: the first open in a process costs a scan of
    /// `target/debug/deps`. Each runs on its own pool.
    fn fake_sink(
        pool: &'static Pool,
        events: Receiver<AudioMessage>,
        opens: Vec<Option<DeviceFormat>>,
    ) -> (AudioSink, Receiver<Mixer>) {
        let (started, mixers) = crossbeam_channel::unbounded();
        let output = FakeOutput {
            opens,
            next: 0,
            attempts: 0,
            started,
        };
        let sink = AudioSink::start_on(
            pool,
            Box::new(move || {
                run(
                    output,
                    events,
                    std::env::temp_dir(),
                    CategoryGains::default(),
                    VoiceCaps::default(),
                    1.0 / 60.0,
                );
            }),
        );
        (sink, mixers)
    }

    #[test]
    fn spawn_never_fails_and_ends_when_the_sender_drops() {
        static POOL: Pool = Mutex::new(Vec::new());
        // The playing path, then the retrying one.
        for opens in [vec![Some(STEREO_44K)], vec![None]] {
            let (tx, rx) = crossbeam_channel::unbounded::<AudioMessage>();
            let (sink, _mixers) = fake_sink(&POOL, rx, opens);
            drop(tx); // disconnect; the thread must notice and exit
            assert!(
                sink.wait_timeout(Duration::from_secs(10)),
                "the session ends once its sender drops"
            );
        }
    }

    #[test]
    fn a_sink_wait_times_out_while_its_sender_lives() {
        static POOL: Pool = Mutex::new(Vec::new());
        let (tx, rx) = crossbeam_channel::unbounded::<AudioMessage>();
        let (sink, _mixers) = fake_sink(&POOL, rx, vec![Some(STEREO_44K)]);
        assert!(!sink.wait_timeout(Duration::from_millis(50)));
        drop(tx); // the session then ends on its own
        let (tx, rx) = crossbeam_channel::unbounded::<AudioMessage>();
        let (sink, _mixers) = fake_sink(&POOL, rx, vec![Some(STEREO_44K)]);
        drop(tx);
        assert!(sink.wait_timeout(Duration::from_secs(10)));
    }

    #[test]
    fn dropping_a_sink_never_waits_for_its_session() {
        static POOL: Pool = Mutex::new(Vec::new());
        let (tx, rx) = crossbeam_channel::unbounded::<AudioMessage>();
        let (sink, _mixers) = fake_sink(&POOL, rx, vec![Some(STEREO_44K)]);
        let (dropped_tx, dropped) = crossbeam_channel::bounded::<()>(1);
        // `tx` stays alive, so the session cannot end: a waiting drop would hang here.
        std::thread::spawn(move || {
            drop(sink);
            let _ = dropped_tx.send(());
        });
        assert!(dropped.recv_timeout(Duration::from_secs(10)).is_ok());
        drop(tx);
    }

    #[test]
    fn a_finished_session_leaves_its_thread_idle_for_the_next_sink() {
        static POOL: Pool = Mutex::new(Vec::new());
        let (ids_tx, ids) = crossbeam_channel::unbounded::<std::thread::ThreadId>();
        for _ in 0..3 {
            let (tx, rx) = crossbeam_channel::unbounded::<AudioMessage>();
            let (started, _mixers) = crossbeam_channel::unbounded();
            let output = FakeOutput {
                opens: vec![Some(STEREO_44K)],
                next: 0,
                attempts: 0,
                started,
            };
            let ids_tx = ids_tx.clone();
            let sink = AudioSink::start_on(
                &POOL,
                Box::new(move || {
                    ids_tx.send(std::thread::current().id()).unwrap();
                    run(
                        output,
                        rx,
                        std::env::temp_dir(),
                        CategoryGains::default(),
                        VoiceCaps::default(),
                        1.0 / 60.0,
                    );
                }),
            );
            drop(tx);
            assert!(
                sink.wait_timeout(Duration::from_secs(10)),
                "the session ends once its sender drops"
            );
            assert_eq!(
                idle_threads(&POOL).len(),
                1,
                "the finished session's thread is idle"
            );
        }
        let ids: Vec<_> = ids.try_iter().collect();
        assert_eq!(ids.len(), 3);
        assert!(
            ids.iter().all(|id| *id == ids[0]),
            "one thread served all three"
        );
    }

    #[test]
    #[ignore = "opens the real audio device: cargo test -p ludion-audio --lib -- --ignored real_device"]
    fn a_real_device_session_opens_or_goes_silent_and_ends() {
        let (tx, rx) = crossbeam_channel::unbounded::<AudioMessage>();
        let sink = AudioSink::spawn(
            rx,
            std::env::temp_dir(),
            CategoryGains::default(),
            VoiceCaps::default(),
            1.0 / 60.0,
        );
        drop(tx);
        assert!(
            sink.wait_timeout(Duration::from_secs(120)),
            "the session ends once its sender drops"
        );
    }
}
