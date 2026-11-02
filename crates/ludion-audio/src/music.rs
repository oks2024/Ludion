//! Streamed Ogg Vorbis music: the sink-thread feed that decodes a track into a bounded ring of
//! device-format chunks, and the real-time player that drains that ring.
//!
//! The track is never decoded up front — a two-minute stereo track is tens of megabytes of `f32`,
//! and decoding it on the sink thread would delay every sound effect queued behind it. The feed
//! decodes a little ahead of the callback; the callback only pulls finished chunks, applies a
//! linear gain ramp and sums, and never decodes or allocates. A chunk carries the *generation* of
//! the play that produced it, so a hard cut to another track discards whatever the old one left in
//! the ring.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use lewton::inside_ogg::OggStreamReader;
use lewton::samples::InterleavedSamples;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use super::{Envelope, STAMP_RECHECK, Stamped, VoiceCommand};

/// Interleaved device samples per chunk.
pub(crate) const MUSIC_CHUNK: usize = 1024;
/// Chunks the ring holds — about 0.7 s at 48 kHz stereo, headroom for a slow decode.
pub(crate) const MUSIC_RING_CHUNKS: usize = 64;
/// How often the sink thread tops the ring up while a track is feeding.
pub(crate) const MUSIC_REFILL: std::time::Duration = std::time::Duration::from_millis(10);

/// A run of whole device frames of one play's audio, ready to mix.
pub(crate) struct MusicChunk {
    generation: u32,
    len: usize,
    samples: [f32; MUSIC_CHUNK],
}

/// Whether `a` is a later generation than `b` (wrapping).
fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

fn unplayable(path: &Path, cause: &str) -> Diagnostic {
    Diagnostic::new(
        code::MUSIC_TRACK_UNPLAYABLE,
        format!("music track `{}` is unplayable: {cause}", path.display()),
    )
    .with_help(
        "encode it as Ogg Vorbis, e.g. `ffmpeg -i in.wav -c:a libvorbis -q:a 5 out.ogg`; \
         Opus-in-Ogg is not supported, and the path is relative to the content root",
    )
}

/// Whether `name` under `content_root` opens as Ogg Vorbis. Header-level: a mid-stream decode
/// failure shows only on the device path.
pub(crate) fn check_track(content_root: &Path, name: &str) -> Result<(), Diagnostic> {
    Track::open(content_root.join(name)).map(drop)
}

/// An open Ogg Vorbis file, read packet by packet.
struct Track {
    reader: OggStreamReader<BufReader<File>>,
    path: PathBuf,
    /// Whether a packet since the last (re)open carried samples — the guard that keeps a looping
    /// empty file from reopening forever.
    produced: bool,
    /// Frames returned since the last (re)open.
    frames: u64,
}

impl Track {
    fn open(path: PathBuf) -> Result<Track, Diagnostic> {
        let file = File::open(&path).map_err(|error| unplayable(&path, &error.to_string()))?;
        let reader = OggStreamReader::new(BufReader::new(file))
            .map_err(|error| unplayable(&path, &format!("not readable as Ogg Vorbis ({error})")))?;
        let rate = reader.ident_hdr.audio_sample_rate;
        if rate < ludion_assets::MIN_SAMPLE_RATE {
            let floor = ludion_assets::MIN_SAMPLE_RATE;
            return Err(unplayable(
                &path,
                &format!(
                    "sample rate {rate} Hz is below the {floor} Hz floor; re-encode it at {floor} Hz or above (44100 Hz is the usual choice)"
                ),
            ));
        }
        Ok(Track {
            reader,
            path,
            produced: false,
            frames: 0,
        })
    }

    fn rate(&self) -> u32 {
        self.reader.ident_hdr.audio_sample_rate
    }

    fn channels(&self) -> usize {
        usize::from(self.reader.ident_hdr.audio_channels)
    }

    /// The next packet's interleaved samples, or `None` at the end of the stream.
    fn next_packet(&mut self) -> Result<Option<Vec<f32>>, Diagnostic> {
        match self
            .reader
            .read_dec_packet_generic::<InterleavedSamples<f32>>()
        {
            Ok(Some(packet)) => {
                let mut samples = packet.samples;
                let channels = self.channels().max(1);
                // Lewton trims the end-of-stream packet itself once it knows a previous granule.
                // This clamp covers a one-page stream, where it does not. Mid-stream the granule
                // tracks the decoded position, so nothing is cut here. A start-trimmed stream loses
                // its excess from the tail of its first page, not its head.
                if let Some(end) = self.reader.get_last_absgp() {
                    let room = end.saturating_sub(self.frames) as usize;
                    samples.truncate(samples.len().min(room * channels));
                }
                self.frames += (samples.len() / channels) as u64;
                self.produced |= !samples.is_empty();
                Ok(Some(samples))
            }
            Ok(None) => Ok(None),
            Err(error) => Err(unplayable(
                &self.path,
                &format!("decode failed mid-stream ({error})"),
            )),
        }
    }

    fn reopen(&mut self) -> Result<(), Diagnostic> {
        *self = Track::open(self.path.clone())?;
        Ok(())
    }
}

/// Stateful linear resampler plus channel map, from a track's format to the device's.
///
/// Its state survives a loop reopen, so the seam does not reset the interpolation phase.
pub(crate) struct Converter {
    in_rate: u32,
    out_rate: u32,
    in_channels: usize,
    out_channels: usize,
    /// Input frames advanced per output frame.
    step: f64,
    /// Input frames not yet fully consumed; the first is the interpolation's left neighbour.
    carry: Vec<f32>,
    /// Position of the next output frame, in frames from the start of `carry`.
    pos: f64,
}

impl Converter {
    pub(crate) fn new(
        in_rate: u32,
        in_channels: usize,
        out_rate: u32,
        out_channels: usize,
    ) -> Converter {
        Converter {
            in_rate,
            out_rate,
            in_channels: in_channels.max(1),
            out_channels: out_channels.max(1),
            step: f64::from(in_rate.max(1)) / f64::from(out_rate.max(1)),
            carry: Vec::new(),
            pos: 0.0,
        }
    }

    /// Adopts a reopened track's format: a changed rate or channel count starts a fresh converter
    /// for the same device format; an unchanged one keeps its phase.
    fn retarget(&mut self, in_rate: u32, in_channels: usize) {
        if in_rate != self.in_rate || in_channels.max(1) != self.in_channels {
            *self = Converter::new(in_rate, in_channels, self.out_rate, self.out_channels);
        }
    }

    /// Appends `input` (interleaved, track format) and pushes every output frame it completes onto
    /// `out` (interleaved, device format).
    pub(crate) fn convert(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.carry.extend_from_slice(input);
        let ich = self.in_channels;
        let frames = self.carry.len() / ich;
        while self.pos + 1.0 < frames as f64 {
            let left = self.pos as usize;
            let frac = (self.pos - left as f64) as f32;
            let a = &self.carry[left * ich..(left + 1) * ich];
            let b = &self.carry[(left + 1) * ich..(left + 2) * ich];
            let lerp = |c: usize| a[c] + (b[c] - a[c]) * frac;
            let mean = (0..ich).map(lerp).sum::<f32>() / ich as f32;
            for c in 0..self.out_channels {
                out.push(if ich == 2 && self.out_channels >= 2 && c < 2 {
                    lerp(c)
                } else {
                    mean
                });
            }
            self.pos += self.step;
        }
        let consumed = (self.pos as usize).min(frames);
        self.carry.drain(..consumed * ich);
        self.pos -= consumed as f64;
    }
}

/// The track currently feeding the ring.
struct Current {
    name: String,
    track: Track,
    converter: Converter,
    looping: bool,
    /// The play's authored gain, kept so a reopened device can resume at it.
    gain: f32,
}

/// The sink-thread side: owns the decoder, keeps the ring full, and turns messages into the
/// commands the callback's [`MusicPlayer`] applies. Each method returns the command to forward, so
/// each is testable without a device.
pub(crate) struct MusicFeed {
    content_root: PathBuf,
    out_rate: u32,
    out_channels: usize,
    seconds_per_tick: f64,
    generation: u32,
    current: Option<Current>,
    /// Converted samples not yet sent: whole device frames.
    staging: Vec<f32>,
    /// Samples left to feed while a fade-out runs; at 0 the track is dropped.
    stop_after: Option<usize>,
    /// Files that failed, by the stamp they failed at: one warning per file version.
    failed: HashMap<String, Stamped>,
    /// How long a failure's stamp is trusted before it is read again.
    recheck: Duration,
    tx: Sender<MusicChunk>,
}

impl MusicFeed {
    pub(crate) fn new(
        content_root: PathBuf,
        out_rate: u32,
        out_channels: u16,
        seconds_per_tick: f64,
        tx: Sender<MusicChunk>,
    ) -> MusicFeed {
        MusicFeed {
            content_root,
            out_rate,
            out_channels: usize::from(out_channels.max(1)),
            seconds_per_tick,
            generation: 0,
            current: None,
            staging: Vec::new(),
            stop_after: None,
            failed: HashMap::new(),
            recheck: STAMP_RECHECK,
            tx,
        }
    }

    /// Sim ticks → device frames.
    fn device_frames(&self, ticks: u32) -> usize {
        (f64::from(ticks) * self.seconds_per_tick * f64::from(self.out_rate)).round() as usize
    }

    pub(crate) fn is_feeding(&self) -> bool {
        self.current.is_some() || !self.staging.is_empty()
    }

    /// Records a failure and warns unless this file version already failed. Returns whether it warned.
    fn note_failure(&mut self, name: &str, diagnostic: &Diagnostic) -> bool {
        if let Some(failed) = self.failed.get_mut(name)
            && failed.is_current(&self.content_root, name, self.recheck)
        {
            return false;
        }
        eprintln!("{diagnostic}");
        self.failed.insert(
            name.to_string(),
            Stamped::read(&self.content_root.join(name)),
        );
        true
    }

    pub(crate) fn play(
        &mut self,
        name: &str,
        gain: f32,
        looping: bool,
        fade_frames: u32,
    ) -> Option<VoiceCommand> {
        if self.stop_after.is_none()
            && let Some(current) = &mut self.current
            && current.name == name
        {
            current.looping = looping;
            current.gain = gain;
            return Some(VoiceCommand::MusicGain(gain));
        }
        if let Some(failed) = self.failed.get_mut(name)
            && failed.is_current(&self.content_root, name, self.recheck)
        {
            return None;
        }
        let track = match Track::open(self.content_root.join(name)) {
            Ok(track) => track,
            Err(diagnostic) => {
                self.note_failure(name, &diagnostic);
                return None;
            }
        };
        self.failed.remove(name);
        let converter = Converter::new(
            track.rate(),
            track.channels(),
            self.out_rate,
            self.out_channels,
        );
        self.generation = self.generation.wrapping_add(1);
        self.staging.clear();
        self.stop_after = None;
        self.current = Some(Current {
            name: name.to_string(),
            track,
            converter,
            looping,
            gain,
        });
        Some(VoiceCommand::MusicStart {
            generation: self.generation,
            gain,
            fade_in: self.device_frames(fade_frames),
        })
    }

    /// Moves the feed onto a reopened device: another format and a new ring. The decoder keeps its
    /// position, so the track resumes where it was (the converted audio staged for the dead ring is
    /// skipped). A fade-out in progress when the device died ends as a stop.
    pub(crate) fn retarget(&mut self, out_rate: u32, out_channels: u16, tx: Sender<MusicChunk>) {
        self.out_rate = out_rate;
        self.out_channels = usize::from(out_channels.max(1));
        self.tx = tx;
        self.staging.clear();
        if self.stop_after.take().is_some() {
            self.current = None;
        }
        if let Some(current) = &mut self.current {
            current.converter = Converter::new(
                current.track.rate(),
                current.track.channels(),
                out_rate,
                self.out_channels,
            );
        }
    }

    /// The command that starts the new device's player on the track still feeding, under a new
    /// generation so the chunks sent since [`MusicFeed::retarget`] are the ones it mixes. `None`
    /// when no track is playing.
    pub(crate) fn resume(&mut self, fade_in: usize) -> Option<VoiceCommand> {
        let gain = self.current.as_ref()?.gain;
        self.generation = self.generation.wrapping_add(1);
        Some(VoiceCommand::MusicStart {
            generation: self.generation,
            gain,
            fade_in,
        })
    }

    pub(crate) fn stop(&mut self, fade_frames: u32) -> VoiceCommand {
        let fade_out = self.device_frames(fade_frames);
        if fade_out == 0 {
            self.current = None;
            self.staging.clear();
            self.stop_after = None;
        } else if self.current.is_some() {
            self.stop_after = Some(fade_out * self.out_channels);
        }
        VoiceCommand::MusicStop { fade_out }
    }

    /// Decodes until `staging` holds a chunk's worth, or the track ends. Returns on a decode error
    /// or an empty looping file after dropping the track.
    fn stage(&mut self) {
        while self.staging.len() < MUSIC_CHUNK {
            let Some(current) = &mut self.current else {
                return;
            };
            match current.track.next_packet() {
                Ok(Some(packet)) => current.converter.convert(&packet, &mut self.staging),
                Ok(None) => {
                    let reopened = if current.looping && current.track.produced {
                        current.track.reopen().inspect(|()| {
                            let (rate, channels) = (current.track.rate(), current.track.channels());
                            current.converter.retarget(rate, channels);
                        })
                    } else {
                        // A once track, or a looping file with no samples at all, ends here.
                        self.current = None;
                        return;
                    };
                    if let Err(diagnostic) = reopened {
                        self.fail_current(&diagnostic);
                        return;
                    }
                }
                Err(diagnostic) => {
                    self.fail_current(&diagnostic);
                    return;
                }
            }
        }
    }

    fn fail_current(&mut self, diagnostic: &Diagnostic) {
        if let Some(current) = self.current.take() {
            self.note_failure(&current.name, diagnostic);
        }
    }

    /// Tops the ring up with chunks until it is full or the track ends.
    pub(crate) fn refill(&mut self) {
        let max = (MUSIC_CHUNK / self.out_channels) * self.out_channels;
        // The feed is the ring's only producer, so `!is_full` guarantees the send finds room.
        while self.current.is_some() || !self.staging.is_empty() {
            if self.tx.is_full() {
                return;
            }
            self.stage();
            let mut len = self.staging.len().min(max);
            len -= len % self.out_channels;
            if let Some(left) = self.stop_after.as_mut() {
                len = len.min(*left);
                *left -= len;
            }
            if len > 0 {
                let mut chunk = MusicChunk {
                    generation: self.generation,
                    len,
                    samples: [0.0; MUSIC_CHUNK],
                };
                chunk.samples[..len].copy_from_slice(&self.staging[..len]);
                self.staging.drain(..len);
                if self.tx.try_send(chunk).is_err() {
                    return;
                }
            }
            if self.stop_after == Some(0) {
                self.current = None;
                self.staging.clear();
                self.stop_after = None;
            } else if len == 0 {
                // Nothing staged and nothing fed: the track ended or failed.
                self.staging.clear();
                return;
            }
        }
    }
}

/// The real-time side: pulls chunks, ramps the gain and sums into the callback's scratch buffer.
/// Allocates nothing and never decodes.
pub(crate) struct MusicPlayer {
    rx: Receiver<MusicChunk>,
    channels: usize,
    generation: u32,
    active: bool,
    /// The play's gain, slewing over `ramp_frames` when it changes mid-play.
    gain: Envelope,
    ramp_frames: usize,
    level: f32,
    target: f32,
    step: f32,
    chunk: Option<MusicChunk>,
    pos: usize,
}

impl MusicPlayer {
    pub(crate) fn new(
        rx: Receiver<MusicChunk>,
        channels: usize,
        ramp_frames: usize,
    ) -> MusicPlayer {
        MusicPlayer {
            rx,
            channels: channels.max(1),
            generation: 0,
            active: false,
            gain: Envelope::at(1.0),
            ramp_frames,
            level: 0.0,
            target: 0.0,
            step: 0.0,
            chunk: None,
            pos: 0,
        }
    }

    pub(crate) fn apply(&mut self, command: VoiceCommand) {
        match command {
            VoiceCommand::MusicStart {
                generation,
                gain,
                fade_in,
            } => {
                self.generation = generation;
                self.active = true;
                self.gain = Envelope::at(gain);
                self.target = 1.0;
                self.level = if fade_in == 0 { 1.0 } else { 0.0 };
                self.step = if fade_in == 0 {
                    0.0
                } else {
                    1.0 / fade_in as f32
                };
                self.pos = 0;
            }
            VoiceCommand::MusicGain(gain) => self.gain.slew_to(gain, self.ramp_frames),
            VoiceCommand::MusicStop { fade_out } => {
                if fade_out == 0 || self.level <= 0.0 {
                    self.stop_now();
                } else {
                    self.target = 0.0;
                    self.step = self.level / fade_out as f32;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn stop_now(&mut self) {
        self.active = false;
        self.level = 0.0;
        self.chunk = None;
    }

    /// The chunk to read next, if one of the current generation is available. An older chunk is
    /// stale and dropped. A newer one is kept: its play's command is still on its way to this
    /// callback, and it must not be mistaken for stale.
    fn next_chunk(&mut self) -> bool {
        loop {
            if let Some(chunk) = &self.chunk {
                if chunk.generation == self.generation && self.pos < chunk.len {
                    return true;
                }
                if newer(chunk.generation, self.generation) {
                    return false;
                }
                self.chunk = None;
                self.pos = 0;
            }
            match self.rx.try_recv() {
                Ok(chunk) => {
                    self.chunk = Some(chunk);
                    self.pos = 0;
                }
                Err(_) => return false,
            }
        }
    }

    /// Adds the next `scratch.len()` samples, scaled by the ramp, the play's gain and `master` (the
    /// music bus), into `scratch`. An empty ring is silence — an underrun, never a block.
    pub(crate) fn mix_into(&mut self, scratch: &mut [f32], master: f32) {
        if !self.active {
            // Nothing sounds, but stale chunks must not pile up in the ring.
            self.drain_stale();
            return;
        }
        for frame in scratch.chunks_mut(self.channels) {
            if frame.len() < self.channels || !self.next_chunk() {
                break;
            }
            if self.level != self.target {
                let toward = self.target - self.level;
                if toward.abs() <= self.step {
                    self.level = self.target;
                } else {
                    self.level += self.step.copysign(toward);
                }
                if self.level == 0.0 && self.target == 0.0 {
                    self.stop_now();
                    break;
                }
            }
            let scale = self.level * self.gain.next() * master;
            if let Some(chunk) = &self.chunk {
                for (slot, sample) in frame
                    .iter_mut()
                    .zip(&chunk.samples[self.pos..self.pos + self.channels])
                {
                    *slot += sample * scale;
                }
            }
            self.pos += self.channels;
        }
    }

    /// Drops every queued chunk of this generation or older, keeping the first newer one.
    fn drain_stale(&mut self) {
        if let Some(chunk) = &self.chunk {
            if newer(chunk.generation, self.generation) {
                return;
            }
            self.chunk = None;
        }
        while let Ok(chunk) = self.rx.try_recv() {
            if newer(chunk.generation, self.generation) {
                self.chunk = Some(chunk);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TONE: &str = "tone-stereo-44k.ogg";

    fn testdata() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"))
    }

    fn scratch_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ludion-music-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A feed over `root` at `out_rate`/`out_channels`, 60 ticks a second, and its ring's receiver.
    fn feed(root: PathBuf, out_rate: u32, out_channels: u16) -> (MusicFeed, Receiver<MusicChunk>) {
        let (tx, rx) = crossbeam_channel::bounded(MUSIC_RING_CHUNKS);
        (
            MusicFeed::new(root, out_rate, out_channels, 1.0 / 60.0, tx),
            rx,
        )
    }

    /// Pulls every chunk the ring holds, refilling in between, until the track ends or `limit`
    /// samples have been collected.
    fn collect(feed: &mut MusicFeed, rx: &Receiver<MusicChunk>, limit: usize) -> Vec<f32> {
        let mut all = Vec::new();
        while all.len() < limit {
            feed.refill();
            let before = all.len();
            while let Ok(chunk) = rx.try_recv() {
                all.extend_from_slice(&chunk.samples[..chunk.len]);
            }
            if all.len() == before && !feed.is_feeding() {
                break;
            }
        }
        all
    }

    /// The granule position of the file's last Ogg page, read from the bytes: the track's true
    /// length in frames, independent of the decoder.
    fn last_granule(path: &Path) -> u64 {
        let bytes = std::fs::read(path).unwrap();
        let page = bytes
            .windows(4)
            .rposition(|w| w == b"OggS")
            .expect("an Ogg page");
        u64::from_le_bytes(bytes[page + 6..page + 14].try_into().unwrap())
    }

    #[test]
    fn it_decodes_a_stereo_track_at_its_native_rate() {
        let mut track = Track::open(testdata().join(TONE)).unwrap();
        assert_eq!((track.channels(), track.rate()), (2, 44100));
        let mut frames = 0;
        while let Some(packet) = track.next_packet().unwrap() {
            frames += packet.len() / 2;
        }
        assert_eq!(frames as u64, last_granule(&testdata().join(TONE)));
    }

    #[test]
    fn feeding_at_another_rate_scales_the_frame_count() {
        let (mut feed, rx) = feed(testdata(), 48000, 2);
        feed.play(TONE, 1.0, false, 0).unwrap();
        let samples = collect(&mut feed, &rx, usize::MAX);
        let expected = 22050.0 * 48000.0 / 44100.0;
        let frames = (samples.len() / 2) as f64;
        assert!(
            (frames - expected).abs() <= 2048.0,
            "{frames} vs {expected}"
        );
        assert!(!feed.is_feeding(), "a once track ends");
    }

    #[test]
    fn a_looping_feed_runs_past_the_track_without_a_gap() {
        let (mut once, rx) = feed(testdata(), 44100, 2);
        once.play(TONE, 1.0, false, 0).unwrap();
        let one_pass = collect(&mut once, &rx, usize::MAX);

        let (mut feed, rx) = feed(testdata(), 44100, 2);
        feed.play(TONE, 1.0, true, 0).unwrap();
        let looped = collect(&mut feed, &rx, one_pass.len() * 3 + 1);
        assert!(looped.len() > one_pass.len() * 3, "ran past 3x the track");
        assert!(feed.is_feeding(), "a loop does not end");
        // At the native rate the converter is an exact copy, so a gapless seam means every pass
        // repeats the first sample for sample, one pass being exactly the stream's granule length:
        // no encoder padding inserted at the wrap.
        let period = 2 * last_granule(&testdata().join(TONE)) as usize;
        assert_eq!(&looped[period..period + 2000], &looped[..2000]);
        assert_eq!(&looped[2 * period..2 * period + 2000], &looped[..2000]);
    }

    #[test]
    fn a_once_track_tail_is_fed_while_the_ring_is_full() {
        // The decoder hit the track's end with several chunks still staged and the ring full: the
        // feed must stay awake until the tail is sent, or the sink sleeps until the next message.
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut feed = MusicFeed::new(testdata(), 44100, 2, 1.0 / 60.0, tx);
        feed.play(TONE, 1.0, false, 0).unwrap();
        feed.current = None;
        feed.staging = vec![0.25; 3 * MUSIC_CHUNK];
        let mut got = 0;
        while feed.is_feeding() {
            feed.refill();
            while let Ok(chunk) = rx.try_recv() {
                got += chunk.len;
            }
        }
        assert_eq!(got, 3 * MUSIC_CHUNK, "the tail was stranded");
    }

    #[test]
    fn stereo_music_on_a_mono_device_is_the_mean_of_both_channels() {
        let mut track = Track::open(testdata().join(TONE)).unwrap();
        let mut stereo = Vec::new();
        while let Some(packet) = track.next_packet().unwrap() {
            stereo.extend(packet);
        }
        let mut converter = Converter::new(48000, 2, 48000, 1);
        let mut out = Vec::new();
        converter.convert(&[1.0, 0.0, 0.0, 1.0, 0.5, 0.5], &mut out);
        assert_eq!(out, vec![0.5, 0.5], "the mean, not the left channel");
        let (mut feed, rx) = feed(testdata(), 44100, 1);
        feed.play(TONE, 1.0, false, 0).unwrap();
        let mono = collect(&mut feed, &rx, usize::MAX);
        assert!(mono.len() > 20000);
        for (i, got) in mono.iter().enumerate() {
            let want = (stereo[2 * i] + stereo[2 * i + 1]) / 2.0;
            assert!((got - want).abs() < 1e-6, "frame {i}: {got} vs {want}");
        }
    }

    #[test]
    fn a_reopened_track_with_a_changed_format_gets_a_new_converter() {
        let mut converter = Converter::new(44100, 2, 48000, 2);
        let mut out = Vec::new();
        converter.convert(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6], &mut out);
        let (carry, pos) = (converter.carry.clone(), converter.pos);
        converter.retarget(44100, 2);
        assert_eq!(
            (&converter.carry, converter.pos),
            (&carry, pos),
            "same format keeps its phase"
        );
        converter.retarget(22050, 2);
        assert_eq!(converter.step, 22050.0 / 48000.0);
        assert!(converter.carry.is_empty() && converter.pos == 0.0);
        converter.retarget(22050, 1);
        assert_eq!(converter.in_channels, 1);
        assert_eq!(converter.out_channels, 2, "the device format is unchanged");
    }

    fn ogg_crc(page: &[u8]) -> u32 {
        let mut crc = 0u32;
        for &byte in page {
            crc ^= u32::from(byte) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04c1_1db7
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    /// The tone with its identification header declaring `rate`, the first page re-checksummed.
    fn ogg_at_rate(rate: u32) -> Vec<u8> {
        let mut bytes = std::fs::read(testdata().join(TONE)).unwrap();
        let lacing = usize::from(bytes[26]);
        let length = 27
            + lacing
            + bytes[27..27 + lacing]
                .iter()
                .map(|&n| usize::from(n))
                .sum::<usize>();
        let stored = u32::from_le_bytes(bytes[22..26].try_into().unwrap());
        bytes[22..26].fill(0);
        assert_eq!(ogg_crc(&bytes[..length]), stored, "the CRC oracle");
        let at = bytes[..length]
            .windows(7)
            .position(|window| window == b"\x01vorbis")
            .expect("the identification header");
        bytes[at + 12..at + 16].copy_from_slice(&rate.to_le_bytes());
        let crc = ogg_crc(&bytes[..length]);
        bytes[22..26].copy_from_slice(&crc.to_le_bytes());
        bytes
    }

    #[test]
    fn track_below_rate_floor_is_refused() {
        let root = scratch_root("rate-floor");
        for rate in [1, 7999] {
            std::fs::write(root.join("slow.ogg"), ogg_at_rate(rate)).unwrap();
            let error = check_track(&root, "slow.ogg").expect_err("below the floor");
            assert_eq!(error.code, code::MUSIC_TRACK_UNPLAYABLE);
            assert!(
                error
                    .message
                    .contains(&format!("{rate} Hz is below the 8000 Hz floor")),
                "{}",
                error.message
            );
        }
        std::fs::write(root.join("slow.ogg"), ogg_at_rate(1)).unwrap();
        let (mut feed, _rx) = feed(root, 44100, 2);
        assert!(feed.play("slow.ogg", 1.0, true, 0).is_none());
    }

    #[test]
    fn track_at_rate_floor_opens() {
        let root = scratch_root("rate-at-floor");
        std::fs::write(root.join("slow.ogg"), ogg_at_rate(8000)).unwrap();
        check_track(&root, "slow.ogg").unwrap();
        assert_eq!(Track::open(root.join("slow.ogg")).unwrap().rate(), 8000);
    }

    #[test]
    fn a_missing_file_and_a_renamed_wav_are_l5003() {
        let root = scratch_root("broken");
        std::fs::write(root.join("fake.ogg"), b"RIFF....WAVEfmt not ogg").unwrap();
        for name in ["absent.ogg", "fake.ogg"] {
            let error = Track::open(root.join(name)).err().expect("must fail");
            assert_eq!(error.code, code::MUSIC_TRACK_UNPLAYABLE);
            assert_eq!(error.code.as_str(), "L5003");
        }
    }

    #[test]
    fn a_broken_track_warns_once_per_file_version() {
        let root = scratch_root("rewarn");
        std::fs::write(root.join("fake.ogg"), b"not ogg").unwrap();
        let (mut feed, _rx) = feed(root.clone(), 44100, 2);
        feed.recheck = Duration::ZERO;
        assert!(feed.play("fake.ogg", 1.0, true, 0).is_none());
        assert_eq!(feed.failed.len(), 1);
        let stamp = feed.failed["fake.ogg"].stamp;
        assert!(feed.play("fake.ogg", 1.0, true, 0).is_none());
        assert_eq!(feed.failed["fake.ogg"].stamp, stamp);
        let diagnostic = unplayable(Path::new("fake.ogg"), "x");
        assert!(
            !feed.note_failure("fake.ogg", &diagnostic),
            "same version: silent"
        );
        // A rewritten file is tried again, and a good one plays.
        std::fs::copy(testdata().join(TONE), root.join("fake.ogg")).unwrap();
        assert!(feed.play("fake.ogg", 1.0, true, 0).is_some());
        assert!(feed.failed.is_empty());
    }

    #[test]
    fn a_failure_is_trusted_inside_the_recheck_window() {
        let root = scratch_root("failure-window");
        std::fs::write(root.join("fake.ogg"), b"not ogg").unwrap();
        let (mut feed, _rx) = feed(root.clone(), 44100, 2);
        assert_eq!(feed.recheck, STAMP_RECHECK);
        feed.recheck = Duration::from_secs(3600);
        assert!(feed.play("fake.ogg", 1.0, true, 0).is_none());
        std::fs::copy(testdata().join(TONE), root.join("fake.ogg")).unwrap();
        assert!(
            feed.play("fake.ogg", 1.0, true, 0).is_none(),
            "no stat inside the window"
        );
        feed.recheck = Duration::ZERO;
        assert!(feed.play("fake.ogg", 1.0, true, 0).is_some());
    }

    #[test]
    fn replaying_the_same_track_is_a_gain_update_and_another_bumps_the_generation() {
        let root = scratch_root("replay");
        std::fs::copy(testdata().join(TONE), root.join("a.ogg")).unwrap();
        std::fs::copy(testdata().join(TONE), root.join("b.ogg")).unwrap();
        let (mut feed, _rx) = feed(root, 44100, 2);
        let Some(VoiceCommand::MusicStart {
            generation: first, ..
        }) = feed.play("a.ogg", 1.0, true, 0)
        else {
            panic!("the first play starts");
        };
        assert!(matches!(
            feed.play("a.ogg", 0.5, true, 0),
            Some(VoiceCommand::MusicGain(g)) if g == 0.5
        ));
        let Some(VoiceCommand::MusicStart {
            generation: second, ..
        }) = feed.play("b.ogg", 1.0, true, 0)
        else {
            panic!("another track starts");
        };
        assert_ne!(first, second);
        // While stopping, the same track starts over rather than updating a fading voice.
        feed.stop(30);
        assert!(matches!(
            feed.play("b.ogg", 1.0, true, 0),
            Some(VoiceCommand::MusicStart { .. })
        ));
    }

    #[test]
    fn a_stop_with_a_fade_feeds_only_the_fade_then_idles() {
        let (mut feed, rx) = feed(testdata(), 44100, 2);
        feed.play(TONE, 1.0, true, 0).unwrap();
        let VoiceCommand::MusicStop { fade_out } = feed.stop(6) else {
            panic!("a stop command");
        };
        assert_eq!(fade_out, 4410); // 6 ticks at 60 Hz, 44.1 kHz
        let samples = collect(&mut feed, &rx, usize::MAX);
        assert_eq!(samples.len(), fade_out * 2);
        assert!(!feed.is_feeding());
    }

    #[test]
    fn a_retargeted_feed_resumes_with_a_new_generation_and_format() {
        let (mut feed, old_ring) = feed(testdata(), 44100, 2);
        let Some(VoiceCommand::MusicStart {
            generation: first, ..
        }) = feed.play(TONE, 0.7, true, 0)
        else {
            panic!("the play starts");
        };
        feed.refill();
        let held = old_ring.len();
        let (tx, new_ring) = crossbeam_channel::bounded(MUSIC_RING_CHUNKS);
        feed.retarget(48000, 1, tx);
        assert_eq!((feed.out_rate, feed.out_channels), (48000, 1));
        let Some(VoiceCommand::MusicStart {
            generation,
            gain,
            fade_in,
        }) = feed.resume(480)
        else {
            panic!("a track is playing, so it resumes");
        };
        assert_eq!((generation, gain, fade_in), (first + 1, 0.7, 480));
        feed.refill();
        assert_eq!(old_ring.len(), held, "the dead ring gets nothing more");
        assert_eq!(new_ring.try_recv().unwrap().generation, generation);

        // The latest play's gain is what a resume carries.
        feed.play(TONE, 0.3, true, 0);
        assert!(matches!(
            feed.resume(0),
            Some(VoiceCommand::MusicStart { gain, generation: g, .. }) if gain == 0.3 && g == generation + 1
        ));

        // A fade-out running when the device died ends as a stop.
        feed.stop(30);
        let (tx, _ring) = crossbeam_channel::bounded(MUSIC_RING_CHUNKS);
        feed.retarget(44100, 2, tx);
        assert!(feed.resume(0).is_none());
        assert!(!feed.is_feeding());
    }

    /// A player over a ring, with `chunks` of `value` (one generation) already queued.
    fn player(generation: u32, chunks: usize, value: f32) -> (MusicPlayer, Sender<MusicChunk>) {
        let (tx, rx) = crossbeam_channel::bounded(MUSIC_RING_CHUNKS);
        for _ in 0..chunks {
            tx.send(MusicChunk {
                generation,
                len: MUSIC_CHUNK,
                samples: [value; MUSIC_CHUNK],
            })
            .unwrap();
        }
        (MusicPlayer::new(rx, 2, 4), tx)
    }

    fn start(player: &mut MusicPlayer, generation: u32, gain: f32, fade_in: usize) {
        player.apply(VoiceCommand::MusicStart {
            generation,
            gain,
            fade_in,
        });
    }

    #[test]
    fn a_fade_in_is_linear_from_silence_to_the_gain() {
        let (mut player, _tx) = player(1, 2, 1.0);
        start(&mut player, 1, 0.5, 4);
        let mut out = vec![0.0; 12]; // 6 frames
        player.mix_into(&mut out, 1.0);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        // level 0.25, 0.5, 0.75, 1.0, then held; times gain 0.5.
        let want = [0.125, 0.25, 0.375, 0.5, 0.5, 0.5];
        for (got, want) in left.iter().zip(want) {
            assert!((got - want).abs() < 1e-6, "{left:?}");
        }
    }

    #[test]
    fn a_music_gain_change_slews_over_the_ramp() {
        let (mut player, _tx) = player(1, 1, 0.5);
        start(&mut player, 1, 1.0, 0);
        let left = |out: &[f32]| out.iter().step_by(2).copied().collect::<Vec<f32>>();
        let mut out = vec![0.0; 4];
        player.mix_into(&mut out, 1.0);
        assert_eq!(left(&out), [0.5, 0.5]);

        player.apply(VoiceCommand::MusicGain(0.5));
        let mut out = vec![0.0; 10];
        player.mix_into(&mut out, 1.0);
        assert_eq!(left(&out), [0.4375, 0.375, 0.3125, 0.25, 0.25]);
    }

    #[test]
    fn a_fade_out_reaches_silence_and_goes_inactive() {
        let (mut player, _tx) = player(1, 2, 1.0);
        start(&mut player, 1, 1.0, 0);
        player.apply(VoiceCommand::MusicStop { fade_out: 4 });
        let mut out = vec![0.0; 16];
        player.mix_into(&mut out, 1.0);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!(
            (left[0] - 0.75).abs() < 1e-6 && (left[2] - 0.25).abs() < 1e-6,
            "{left:?}"
        );
        assert_eq!(left[3], 0.0);
        assert!(!player.active);
    }

    #[test]
    fn stale_generations_are_discarded_but_newer_ones_wait_for_their_command() {
        let (mut player, tx) = player(1, 1, 0.9);
        start(&mut player, 2, 1.0, 0);
        let mut out = vec![0.0; 4];
        player.mix_into(&mut out, 1.0);
        assert_eq!(out, vec![0.0; 4], "the old play's chunk is dropped");
        // A chunk of a play whose command has not arrived yet is held, not dropped.
        tx.send(MusicChunk {
            generation: 3,
            len: MUSIC_CHUNK,
            samples: [0.5; MUSIC_CHUNK],
        })
        .unwrap();
        player.mix_into(&mut out, 1.0);
        assert_eq!(out, vec![0.0; 4]);
        start(&mut player, 3, 1.0, 0);
        player.mix_into(&mut out, 1.0);
        assert_eq!(out, vec![0.5; 4]);
    }

    #[test]
    fn an_empty_ring_mixes_silence_and_master_scales_the_output() {
        let (mut player, tx) = player(1, 0, 0.0);
        start(&mut player, 1, 1.0, 0);
        let mut out = vec![0.0; 4];
        player.mix_into(&mut out, 1.0);
        assert_eq!(out, vec![0.0; 4], "an underrun is silence");
        tx.send(MusicChunk {
            generation: 1,
            len: MUSIC_CHUNK,
            samples: [0.5; MUSIC_CHUNK],
        })
        .unwrap();
        player.mix_into(&mut out, 0.5);
        assert_eq!(out, vec![0.25; 4]);
    }

    #[test]
    fn converter_maps_channels_and_interpolates() {
        // Same rate, stereo -> 4 channels: the first two follow, the rest take the mean.
        let mut converter = Converter::new(48000, 2, 48000, 4);
        let mut out = Vec::new();
        converter.convert(&[1.0, 0.0, 0.0, 1.0, 0.5, 0.5], &mut out);
        assert_eq!(&out[..4], &[1.0, 0.0, 0.5, 0.5]);
        // Half rate up: a ramp 0 -> 1 mono comes out with the midpoint inserted.
        let mut converter = Converter::new(24000, 1, 48000, 1);
        let mut out = Vec::new();
        converter.convert(&[0.0, 1.0, 0.0], &mut out);
        assert_eq!(out, vec![0.0, 0.5, 1.0, 0.5]);
    }
}
