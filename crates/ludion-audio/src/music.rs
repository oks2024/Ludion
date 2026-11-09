//! Streamed Ogg Vorbis music: the sink-thread feed that decodes a track into a bounded ring of
//! device-format chunks, and the real-time player that drains that ring.
//!
//! The track is never decoded up front — a two-minute stereo track is tens of megabytes of `f32`,
//! and decoding it on the sink thread would delay every sound effect queued behind it. The feed
//! decodes a little ahead of the callback; the callback only pulls finished chunks, applies a
//! linear gain ramp and sums, and never decodes or allocates. A chunk carries the *generation* of
//! the play that produced it, so a hard cut to another track discards whatever the old one left in
//! the ring.
//!
//! A *set* of up to [`MUSIC_STEMS_MAX`] stems plays on one playhead: each stem has its own decoder,
//! converter and staging, and a chunk carries every stem's samples for the same device frames, so the
//! stems cannot drift apart. The callback mixes them with a per-stem gain envelope.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use lewton::inside_ogg::OggStreamReader;
use lewton::samples::InterleavedSamples;
use ludion_core::audio::MUSIC_STEMS_MAX;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use super::{Envelope, STAMP_RECHECK, Stamped, VoiceCommand};

/// Interleaved device samples per chunk.
pub(crate) const MUSIC_CHUNK: usize = 1024;
/// Chunks the ring holds — about 0.7 s at 48 kHz stereo, headroom for a slow decode.
pub(crate) const MUSIC_RING_CHUNKS: usize = 64;
/// How often the sink thread tops the ring up while a track is feeding.
pub(crate) const MUSIC_REFILL: std::time::Duration = std::time::Duration::from_millis(10);

/// A run of whole device frames of one play's audio, ready to mix: `count` stems' samples for the
/// same frames. Inline so the callback never frees heap memory (about 32 KB a chunk, 2 MB a ring).
pub(crate) struct MusicChunk {
    generation: u32,
    /// Samples per stem.
    len: usize,
    count: usize,
    samples: [[f32; MUSIC_CHUNK]; MUSIC_STEMS_MAX],
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

fn mismatched(set: &str, stem: &str, why: &str) -> Diagnostic {
    Diagnostic::new(
        code::MUSIC_STEM_MISMATCHED,
        format!("music stem `{stem}` of set `{set}` is left out: {why}"),
    )
    .with_help(
        "export all stems of a set from one session at one sample rate and length, \
         and keep a set to 8 stems or fewer",
    )
}

/// A file's length in frames: the granule position of its last Ogg page, read from the bytes.
fn length_frames(path: &Path) -> Option<u64> {
    const TAIL: u64 = 128 * 1024;
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(TAIL))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let page = bytes.windows(4).rposition(|w| w == b"OggS")?;
    let granule = u64::from_le_bytes(bytes.get(page + 6..page + 14)?.try_into().ok()?);
    (granule != u64::MAX).then_some(granule)
}

/// A stem that opened and matches the set.
struct OpenedStem {
    name: String,
    track: Track,
    gain: f32,
}

/// What opening a set found: the playable stems, and why each of the others was left out.
struct OpenedSet {
    stems: Vec<OpenedStem>,
    /// `L5003`s, by stem name.
    unplayable: Vec<(String, Diagnostic)>,
    /// `L5006`s.
    mismatched: Vec<Diagnostic>,
}

/// Opens each stem of set `id` under `content_root`: the one rule [`MusicFeed::set`] and
/// [`check_set`] share. The first stem that opens sets the sample rate and length the rest must
/// match; stems past [`MUSIC_STEMS_MAX`] are not opened. `skip` names stems already known to fail.
fn open_set(
    content_root: &Path,
    id: &str,
    stems: &[(String, f32)],
    mut skip: impl FnMut(&str) -> bool,
) -> OpenedSet {
    let mut set = OpenedSet {
        stems: Vec::new(),
        unplayable: Vec::new(),
        mismatched: Vec::new(),
    };
    let mut reference: Option<(u32, u64)> = None;
    for (index, (name, gain)) in stems.iter().enumerate() {
        if index >= MUSIC_STEMS_MAX {
            let why = format!("a set plays at most {MUSIC_STEMS_MAX} stems");
            set.mismatched.push(mismatched(id, name, &why));
            continue;
        }
        if skip(name) {
            continue;
        }
        let path = content_root.join(name);
        let track = match Track::open(path.clone()) {
            Ok(track) => track,
            Err(diagnostic) => {
                set.unplayable.push((name.clone(), diagnostic));
                continue;
            }
        };
        let Some(length) = length_frames(&path) else {
            let diagnostic =
                unplayable(&path, "its length could not be read from its last Ogg page");
            set.unplayable.push((name.clone(), diagnostic));
            continue;
        };
        let format = (track.rate(), length);
        match reference {
            None => reference = Some(format),
            Some(first) if first != format => {
                let why = format!(
                    "it is {} Hz and {} frames long, but the set's first stem is {} Hz and {} frames",
                    format.0, format.1, first.0, first.1
                );
                set.mismatched.push(mismatched(id, name, &why));
                continue;
            }
            Some(_) => {}
        }
        set.stems.push(OpenedStem {
            name: name.clone(),
            track,
            gain: *gain,
        });
    }
    set
}

/// Whether set `id`'s every stem opens as Ogg Vorbis and matches the first one's rate and length:
/// the first `L5003` if any, else the first `L5006`. Header-level, like [`check_track`].
pub(crate) fn check_set(
    content_root: &Path,
    id: &str,
    stems: &[(String, f32)],
) -> Result<(), Diagnostic> {
    let set = open_set(content_root, id, stems, |_| false);
    match set.unplayable.into_iter().next() {
        Some((_, diagnostic)) => Err(diagnostic),
        None => set.mismatched.into_iter().next().map_or(Ok(()), Err),
    }
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

/// One stem of the music feeding the ring.
struct Stem {
    name: String,
    track: Track,
    converter: Converter,
    /// Converted samples not yet sent: whole device frames.
    staging: Vec<f32>,
    /// A once stem whose decoder reached its end; its staging is still to be sent.
    ended: bool,
}

/// The music currently feeding the ring: a `MusicPlay` track is a one-stem set.
struct Current {
    /// The track (for a `MusicPlay`) or the set id.
    id: String,
    /// Whether it came from a `MusicSet`, so a track and a set sharing a name stay distinct.
    is_set: bool,
    stems: Vec<Stem>,
    looping: bool,
    /// The play's authored gain, kept so a reopened device can resume at it.
    gain: f32,
    /// Each stem's latest gain, kept for the same reason.
    stem_gains: Vec<f32>,
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
        self.current.is_some()
    }

    /// A stem's decoder and converter for the device format.
    fn stem(&self, name: String, track: Track) -> Stem {
        let converter = Converter::new(
            track.rate(),
            track.channels(),
            self.out_rate,
            self.out_channels,
        );
        Stem {
            name,
            track,
            converter,
            staging: Vec::new(),
            ended: false,
        }
    }

    /// Makes `current` the music: a new generation, nothing staged, no fade-out pending. Returns
    /// the command that starts the player on it.
    fn start(&mut self, current: Current, fade_frames: u32) -> VoiceCommand {
        self.generation = self.generation.wrapping_add(1);
        self.stop_after = None;
        let (gain, stem_gains) = (current.gain, current.stem_gains.clone());
        self.current = Some(current);
        self.start_command(gain, &stem_gains, self.device_frames(fade_frames))
    }

    fn start_command(&self, gain: f32, stem_gains: &[f32], fade_in: usize) -> VoiceCommand {
        let mut gains = [0.0; MUSIC_STEMS_MAX];
        gains[..stem_gains.len()].copy_from_slice(stem_gains);
        VoiceCommand::MusicStart {
            generation: self.generation,
            gain,
            stem_gains: gains,
            count: stem_gains.len(),
            fade_in,
        }
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
            && !current.is_set
            && current.id == name
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
        let current = Current {
            id: name.to_string(),
            is_set: false,
            stems: vec![self.stem(name.to_string(), track)],
            looping,
            gain,
            stem_gains: vec![1.0],
        };
        Some(self.start(current, fade_frames))
    }

    /// Starts the set `id` of `stems` (path, starting gain), or does nothing when that set already
    /// plays and is not stopping — the initial gains apply only at a set's start. A stem that fails
    /// to open warns `L5003`; one that does not match the first playable stem's rate and length, or
    /// lies past the eighth, warns `L5006`. Both are left out, and `None` means none could play.
    pub(crate) fn set(
        &mut self,
        id: &str,
        stems: &[(String, f32)],
        fade_frames: u32,
    ) -> Option<VoiceCommand> {
        if self.stop_after.is_none()
            && self
                .current
                .as_ref()
                .is_some_and(|current| current.is_set && current.id == id)
        {
            return None;
        }
        let (root, recheck) = (self.content_root.clone(), self.recheck);
        let failed = &mut self.failed;
        let opened = open_set(&root, id, stems, |name| {
            failed
                .get_mut(name)
                .is_some_and(|stamped| stamped.is_current(&root, name, recheck))
        });
        for (name, diagnostic) in &opened.unplayable {
            self.note_failure(name, diagnostic);
        }
        for diagnostic in &opened.mismatched {
            eprintln!("{diagnostic}");
        }
        if opened.stems.is_empty() {
            return None;
        }
        let mut stem_gains = Vec::new();
        let mut playing = Vec::new();
        for stem in opened.stems {
            self.failed.remove(&stem.name);
            stem_gains.push(stem.gain);
            playing.push(self.stem(stem.name, stem.track));
        }
        let current = Current {
            id: id.to_string(),
            is_set: true,
            stems: playing,
            looping: true,
            gain: 1.0,
            stem_gains,
        };
        Some(self.start(current, fade_frames))
    }

    /// Slews stem `name` of the playing set to `gain`. `None` when it is not in the set.
    pub(crate) fn stem_gain(
        &mut self,
        name: &str,
        gain: f32,
        fade_frames: u32,
    ) -> Option<VoiceCommand> {
        let frames = self.device_frames(fade_frames);
        let current = self.current.as_mut()?;
        let index = current.stems.iter().position(|stem| stem.name == name)?;
        current.stem_gains[index] = gain;
        Some(VoiceCommand::MusicStemGain {
            index,
            gain,
            frames,
        })
    }

    /// Moves the feed onto a reopened device: another format and a new ring. The decoder keeps its
    /// position, so the track resumes where it was (the converted audio staged for the dead ring is
    /// skipped). A fade-out in progress when the device died ends as a stop.
    pub(crate) fn retarget(&mut self, out_rate: u32, out_channels: u16, tx: Sender<MusicChunk>) {
        self.out_rate = out_rate;
        self.out_channels = usize::from(out_channels.max(1));
        self.tx = tx;
        if self.stop_after.take().is_some() {
            self.current = None;
        }
        if let Some(current) = &mut self.current {
            for stem in &mut current.stems {
                stem.staging.clear();
                stem.converter = Converter::new(
                    stem.track.rate(),
                    stem.track.channels(),
                    out_rate,
                    self.out_channels,
                );
            }
        }
    }

    /// The command that starts the new device's player on the track still feeding, under a new
    /// generation so the chunks sent since [`MusicFeed::retarget`] are the ones it mixes. `None`
    /// when no track is playing.
    pub(crate) fn resume(&mut self, fade_in: usize) -> Option<VoiceCommand> {
        let current = self.current.as_ref()?;
        let (gain, stem_gains) = (current.gain, current.stem_gains.clone());
        self.generation = self.generation.wrapping_add(1);
        Some(self.start_command(gain, &stem_gains, fade_in))
    }

    pub(crate) fn stop(&mut self, fade_frames: u32) -> VoiceCommand {
        let fade_out = self.device_frames(fade_frames);
        if fade_out == 0 {
            self.current = None;
            self.stop_after = None;
        } else if self.current.is_some() {
            self.stop_after = Some(fade_out * self.out_channels);
        }
        VoiceCommand::MusicStop { fade_out }
    }

    /// Decodes each stem until its staging holds a chunk's worth, or it ends. A looping stem
    /// reopens at its end; a once stem is marked ended. A decode error or an empty looping file
    /// drops the whole set.
    fn stage(&mut self) {
        let Some(current) = &mut self.current else {
            return;
        };
        let looping = current.looping;
        let mut failure = None;
        'stems: for stem in &mut current.stems {
            while stem.staging.len() < MUSIC_CHUNK && !stem.ended {
                match stem.track.next_packet() {
                    Ok(Some(packet)) => stem.converter.convert(&packet, &mut stem.staging),
                    Ok(None) if looping && stem.track.produced => {
                        let reopened = stem.track.reopen().inspect(|()| {
                            let (rate, channels) = (stem.track.rate(), stem.track.channels());
                            stem.converter.retarget(rate, channels);
                        });
                        if let Err(diagnostic) = reopened {
                            failure = Some((stem.name.clone(), diagnostic));
                            break 'stems;
                        }
                    }
                    // A once stem, or a looping file with no samples at all, ends here.
                    Ok(None) => stem.ended = true,
                    Err(diagnostic) => {
                        failure = Some((stem.name.clone(), diagnostic));
                        break 'stems;
                    }
                }
            }
        }
        if let Some((name, diagnostic)) = failure {
            self.current = None;
            self.note_failure(&name, &diagnostic);
        }
    }

    /// Tops the ring up with chunks until it is full or the music ends.
    pub(crate) fn refill(&mut self) {
        let max = (MUSIC_CHUNK / self.out_channels) * self.out_channels;
        // The feed is the ring's only producer, so `!is_full` guarantees the send finds room.
        while self.current.is_some() {
            if self.tx.is_full() {
                return;
            }
            self.stage();
            let Some(current) = &mut self.current else {
                return;
            };
            // Every stem contributes the same frames, so the shortest staging sets the chunk.
            let shortest = current.stems.iter().map(|s| s.staging.len()).min();
            let mut len = shortest.unwrap_or(0).min(max);
            len -= len % self.out_channels;
            if let Some(left) = self.stop_after.as_mut() {
                len = len.min(*left);
                *left -= len;
            }
            if len > 0 {
                let mut chunk = MusicChunk {
                    generation: self.generation,
                    len,
                    count: current.stems.len(),
                    samples: [[0.0; MUSIC_CHUNK]; MUSIC_STEMS_MAX],
                };
                for (slot, stem) in chunk.samples.iter_mut().zip(&mut current.stems) {
                    slot[..len].copy_from_slice(&stem.staging[..len]);
                    stem.staging.drain(..len);
                }
                if self.tx.try_send(chunk).is_err() {
                    return;
                }
            }
            // Nothing left to send: the fade-out is spent, or a stem ended with nothing staged.
            if len == 0 || self.stop_after == Some(0) {
                self.current = None;
                self.stop_after = None;
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
    /// Each stem's gain; the first `count` are live.
    stems: [Envelope; MUSIC_STEMS_MAX],
    count: usize,
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
            stems: [Envelope::at(1.0); MUSIC_STEMS_MAX],
            count: 0,
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
                stem_gains,
                count,
                fade_in,
            } => {
                self.generation = generation;
                self.active = true;
                self.gain = Envelope::at(gain);
                self.stems = stem_gains.map(Envelope::at);
                self.count = count.min(MUSIC_STEMS_MAX);
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
            VoiceCommand::MusicStemGain {
                index,
                gain,
                frames,
            } => {
                if let Some(stem) = self.stems.get_mut(index) {
                    stem.slew_to(gain, frames.max(self.ramp_frames));
                }
            }
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
            let mut stem_gain = [0.0; MUSIC_STEMS_MAX];
            for (gain, stem) in stem_gain.iter_mut().zip(&mut self.stems[..self.count]) {
                *gain = stem.next();
            }
            if let Some(chunk) = &self.chunk {
                let live = chunk.count.min(self.count);
                for (c, slot) in frame.iter_mut().enumerate() {
                    let sum: f32 = chunk.samples[..live]
                        .iter()
                        .zip(&stem_gain)
                        .map(|(stem, gain)| stem[self.pos + c] * gain)
                        .sum();
                    *slot += sum * scale;
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
                all.extend_from_slice(&chunk.samples[0][..chunk.len]);
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
        length_frames(path).expect("an Ogg page")
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
        let (mut whole, rx) = feed(testdata(), 44100, 2);
        whole.play(TONE, 1.0, false, 0).unwrap();
        let expected = collect(&mut whole, &rx, usize::MAX).len();

        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut feed = MusicFeed::new(testdata(), 44100, 2, 1.0 / 60.0, tx);
        feed.play(TONE, 1.0, false, 0).unwrap();
        let mut got = 0;
        while feed.is_feeding() {
            feed.refill();
            while let Ok(chunk) = rx.try_recv() {
                got += chunk.len;
            }
        }
        assert_eq!(got, expected, "the tail was stranded");
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
            ..
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

    /// A one-stem chunk of `value`.
    fn chunk(generation: u32, value: f32) -> MusicChunk {
        MusicChunk {
            generation,
            len: MUSIC_CHUNK,
            count: 1,
            samples: [[value; MUSIC_CHUNK]; MUSIC_STEMS_MAX],
        }
    }

    /// A player over a ring, with `chunks` of `value` (one generation) already queued.
    fn player(generation: u32, chunks: usize, value: f32) -> (MusicPlayer, Sender<MusicChunk>) {
        let (tx, rx) = crossbeam_channel::bounded(MUSIC_RING_CHUNKS);
        for _ in 0..chunks {
            tx.send(chunk(generation, value)).unwrap();
        }
        (MusicPlayer::new(rx, 2, 4), tx)
    }

    fn start(player: &mut MusicPlayer, generation: u32, gain: f32, fade_in: usize) {
        player.apply(VoiceCommand::MusicStart {
            generation,
            gain,
            stem_gains: [1.0; MUSIC_STEMS_MAX],
            count: 1,
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
        tx.send(chunk(3, 0.5)).unwrap();
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
        tx.send(chunk(1, 0.5)).unwrap();
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

    /// The tone with its last Ogg page's granule lowered by `by` frames, the page re-checksummed.
    fn ogg_shorter(by: u64) -> Vec<u8> {
        let mut bytes = std::fs::read(testdata().join(TONE)).unwrap();
        let page = bytes
            .windows(4)
            .rposition(|w| w == b"OggS")
            .expect("an Ogg page");
        let lacing = usize::from(bytes[page + 26]);
        let length = 27
            + lacing
            + bytes[page + 27..page + 27 + lacing]
                .iter()
                .map(|&n| usize::from(n))
                .sum::<usize>();
        let granule = u64::from_le_bytes(bytes[page + 6..page + 14].try_into().unwrap());
        bytes[page + 6..page + 14].copy_from_slice(&(granule - by).to_le_bytes());
        bytes[page + 22..page + 26].fill(0);
        let crc = ogg_crc(&bytes[page..page + length]);
        bytes[page + 22..page + 26].copy_from_slice(&crc.to_le_bytes());
        bytes
    }

    /// Renders `blocks` blocks of 256 stereo frames through `player`, topping the ring up from
    /// `feed` and running `before` ahead of each block.
    fn render(
        feed: &mut MusicFeed,
        player: &mut MusicPlayer,
        blocks: usize,
        mut before: impl FnMut(usize, &mut MusicFeed, &mut MusicPlayer),
    ) -> Vec<f32> {
        let mut out = Vec::new();
        for block in 0..blocks {
            before(block, feed, player);
            feed.refill();
            let mut scratch = vec![0.0; 512];
            player.mix_into(&mut scratch, 1.0);
            out.extend(scratch);
        }
        out
    }

    fn set_of(names: &[&str]) -> Vec<(String, f32)> {
        names.iter().map(|name| (name.to_string(), 1.0)).collect()
    }

    #[test]
    fn two_stems_stay_sample_locked_across_loops_and_gain_moves() {
        let root = scratch_root("stems-locked");
        for name in ["a.ogg", "b.ogg"] {
            std::fs::copy(testdata().join(TONE), root.join(name)).unwrap();
        }
        // One loop period (the stream's granule length), taken from a looping pass: a once pass
        // drops its last frame to the converter's lookahead.
        let tone = {
            let (mut looped, rx) = feed(testdata(), 44100, 2);
            looped.play(TONE, 1.0, true, 0).unwrap();
            let period = 2 * last_granule(&testdata().join(TONE)) as usize;
            let mut pass = collect(&mut looped, &rx, period + 1);
            pass.truncate(period);
            pass
        };
        let (mut feed, rx) = feed(root, 44100, 2);
        let mut player = MusicPlayer::new(rx, 2, 4);
        let stems = [("a.ogg".to_string(), 1.0), ("b.ogg".to_string(), 0.0)];
        player.apply(feed.set("pair", &stems, 0).unwrap());

        // (block, gain of b, fade in ticks): the last two overlap, retargeting a running slew.
        let moves = [(5, 1.0, 1), (60, 0.25, 0), (120, 0.8, 2), (123, 0.3, 1)];
        let blocks = tone.len() * 3 / 512 + 10;
        let mut sent = Vec::new();
        let out = render(&mut feed, &mut player, blocks, |block, feed, player| {
            for &(at, gain, ticks) in &moves {
                if at == block {
                    let command = feed.stem_gain("b.ogg", gain, ticks).unwrap();
                    let VoiceCommand::MusicStemGain { frames, .. } = command else {
                        panic!("a stem gain command");
                    };
                    sent.push((block, gain, frames.max(4)));
                    player.apply(command);
                }
            }
        });

        let mut b = Envelope::at(0.0);
        for block in 0..blocks {
            for &(at, gain, frames) in &sent {
                if at == block {
                    b.slew_to(gain, frames);
                }
            }
            for frame in 0..256 {
                let level = 1.0 + b.next();
                for c in 0..2 {
                    let i = (block * 256 + frame) * 2 + c;
                    let want = level * tone[i % tone.len()];
                    assert!(
                        (out[i] - want).abs() < 1e-6,
                        "sample {i} (loop {}): {} vs {want}",
                        i / tone.len(),
                        out[i]
                    );
                }
            }
        }
        assert!(blocks * 512 > tone.len() * 3, "ran past three loops");
    }

    #[test]
    fn two_stems_resampled_to_the_device_sum_like_one_stem() {
        let root = scratch_root("stems-resampled");
        for name in ["a.ogg", "b.ogg"] {
            std::fs::copy(testdata().join(TONE), root.join(name)).unwrap();
        }
        let (mut feed, rx) = feed(root, 48000, 2);
        let mut player = MusicPlayer::new(rx, 2, 4);
        let stems = [("a.ogg".to_string(), 1.0), ("b.ogg".to_string(), 0.5)];
        player.apply(feed.set("pair", &stems, 0).unwrap());
        let both = render(&mut feed, &mut player, 300, |_, _, _| {});

        let (mut one, rx) = feed_at(48000);
        let mut single = MusicPlayer::new(rx, 2, 4);
        single.apply(one.play(TONE, 1.5, true, 0).unwrap());
        let reference = render(&mut one, &mut single, 300, |_, _, _| {});
        for (i, (got, want)) in both.iter().zip(&reference).enumerate() {
            assert!((got - want).abs() < 1e-6, "sample {i}: {got} vs {want}");
        }
        assert!(reference.iter().any(|&s| s.abs() > 0.1), "not silence");
    }

    /// A feed over the test data at `out_rate`, stereo.
    fn feed_at(out_rate: u32) -> (MusicFeed, Receiver<MusicChunk>) {
        feed(testdata(), out_rate, 2)
    }

    #[test]
    fn a_stem_of_another_rate_or_length_or_past_the_eighth_is_left_out_with_l5006() {
        let root = scratch_root("stems-mismatch");
        std::fs::copy(testdata().join(TONE), root.join("a.ogg")).unwrap();
        std::fs::copy(testdata().join(TONE), root.join("c.ogg")).unwrap();
        std::fs::write(root.join("slow.ogg"), ogg_at_rate(22050)).unwrap();
        std::fs::write(root.join("short.ogg"), ogg_shorter(100)).unwrap();
        assert_eq!(
            length_frames(&root.join("short.ogg")),
            Some(last_granule(&root.join("a.ogg")) - 100)
        );

        let stems = set_of(&["a.ogg", "slow.ogg", "short.ogg", "c.ogg"]);
        let opened = open_set(&root, "mix", &stems, |_| false);
        let names: Vec<_> = opened.stems.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a.ogg", "c.ogg"]);
        assert!(opened.unplayable.is_empty());
        assert_eq!(opened.mismatched.len(), 2);
        assert!(
            opened
                .mismatched
                .iter()
                .all(|d| d.code == code::MUSIC_STEM_MISMATCHED && d.code.as_str() == "L5006")
        );
        assert_eq!(
            check_set(&root, "mix", &stems).unwrap_err().code,
            code::MUSIC_STEM_MISMATCHED
        );

        // The set still starts with the stems that match.
        let (mut mix_feed, _rx) = feed(root.clone(), 44100, 2);
        let Some(VoiceCommand::MusicStart { count, .. }) = mix_feed.set("mix", &stems, 0) else {
            panic!("the matching stems play");
        };
        assert_eq!(count, 2);

        // Nine stems: the ninth is dropped.
        for n in 0..9 {
            std::fs::copy(testdata().join(TONE), root.join(format!("s{n}.ogg"))).unwrap();
        }
        let nine: Vec<String> = (0..9).map(|n| format!("s{n}.ogg")).collect();
        let nine = set_of(&nine.iter().map(String::as_str).collect::<Vec<_>>());
        let opened = open_set(&root, "nine", &nine, |_| false);
        assert_eq!(opened.stems.len(), MUSIC_STEMS_MAX);
        assert_eq!(opened.mismatched.len(), 1);
        assert_eq!(opened.mismatched[0].code, code::MUSIC_STEM_MISMATCHED);
        assert!(opened.mismatched[0].message.contains("s8.ogg"));
        let (mut nine_feed, _rx) = feed(root.clone(), 44100, 2);
        let Some(VoiceCommand::MusicStart { count, .. }) = nine_feed.set("nine", &nine, 0) else {
            panic!("eight stems play");
        };
        assert_eq!(count, MUSIC_STEMS_MAX);
    }

    #[test]
    fn an_unopenable_stem_is_l5003_and_the_rest_play() {
        let root = scratch_root("stems-missing");
        std::fs::copy(testdata().join(TONE), root.join("b.ogg")).unwrap();
        let stems = set_of(&["absent.ogg", "b.ogg"]);
        assert_eq!(
            check_set(&root, "gap", &stems).unwrap_err().code,
            code::MUSIC_TRACK_UNPLAYABLE
        );
        let (mut feed, _rx) = feed(root, 44100, 2);
        let Some(VoiceCommand::MusicStart {
            count, stem_gains, ..
        }) = feed.set("gap", &stems, 0)
        else {
            panic!("the openable stem plays");
        };
        assert_eq!((count, stem_gains[0]), (1, 1.0));
        assert!(feed.set("none", &set_of(&["absent.ogg"]), 0).is_none());
    }

    #[test]
    fn a_set_resend_is_a_no_op_and_a_stem_gain_names_a_stem_of_the_playing_set() {
        let root = scratch_root("stems-idempotent");
        for name in ["a.ogg", "b.ogg", "c.ogg"] {
            std::fs::copy(testdata().join(TONE), root.join(name)).unwrap();
        }
        let (mut feed, _rx) = feed(root, 44100, 2);
        let pair = [("a.ogg".to_string(), 1.0), ("b.ogg".to_string(), 0.0)];
        let Some(VoiceCommand::MusicStart {
            generation: first, ..
        }) = feed.set("pair", &pair, 0)
        else {
            panic!("the first set starts");
        };
        assert!(feed.set("pair", &pair, 0).is_none(), "same id: a no-op");
        assert!(feed.stem_gain("c.ogg", 1.0, 0).is_none(), "not in the set");
        assert!(matches!(
            feed.stem_gain("b.ogg", 0.7, 30),
            Some(VoiceCommand::MusicStemGain { index: 1, gain, frames: 22050 }) if gain == 0.7
        ));
        // A resume carries the moved gain, not the initial one.
        let Some(VoiceCommand::MusicStart {
            generation,
            stem_gains,
            count,
            ..
        }) = feed.resume(0)
        else {
            panic!("a set is playing");
        };
        assert_eq!((generation, count), (first + 1, 2));
        assert_eq!(&stem_gains[..2], &[1.0, 0.7]);

        // Another set, or a track, hard-cuts under a new generation.
        let Some(VoiceCommand::MusicStart {
            generation: second, ..
        }) = feed.set("other", &pair, 0)
        else {
            panic!("another set starts");
        };
        assert_eq!(second, generation + 1);
        assert!(matches!(
            feed.play("a.ogg", 1.0, true, 0),
            Some(VoiceCommand::MusicStart { .. })
        ));
        assert!(
            feed.stem_gain("b.ogg", 1.0, 0).is_none(),
            "a track has one stem"
        );
        // While stopping, the same id starts over.
        feed.set("pair", &pair, 0).unwrap();
        feed.stop(30);
        assert!(matches!(
            feed.set("pair", &pair, 0),
            Some(VoiceCommand::MusicStart { .. })
        ));
    }
}
