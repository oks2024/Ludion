//! A one-liner for asserting on an engine's audio event stream.
//!
//! Audio has no goldens — its verification surface is text-first: the frame-stamped stream the
//! engine records as game systems emit sounds (see `ludion_engine::audio`). This helper flattens
//! that log to the `(frame, sound)` pairs a scripted-gameplay test compares against an expected
//! sequence, exactly as [`sim_assert`](crate::sim_assert) wraps the hash-sequence check;
//! [`audio_messages`] is the unflattened form, for a test that has to tell a loop's start from its
//! stop. [`SoundTap`] is the whole-run form: it keeps every message (the log keeps only the latest)
//! and asserts each named sound and track would decode.

use std::collections::BTreeSet;
use std::path::Path;

use crossbeam_channel::Receiver;
use ludion_engine::{AudioMessage, Engine};

/// The engine's *sounding* audio stream as `(frame, sound)` pairs, in emission order: every message
/// that names a sound — a one-shot, a loop sync (which starts a loop or updates its gain) and a
/// music play (its track).
///
/// A stop names an identity rather than a path, so stops do not appear here; use
/// [`audio_messages`] when a test needs them.
///
/// ```no_run
/// # use ludion_engine::Engine;
/// # let engine = Engine::new();
/// let stream = ludion_testkit::audio::audio_stream(&engine);
/// // assert_eq!(stream, vec![(20, "sounds/bounce.wav".to_string())]);
/// ```
pub fn audio_stream(engine: &Engine) -> Vec<(u64, String)> {
    engine
        .audio_log()
        .iter()
        .filter_map(|entry| Some((entry.frame.as_u64(), entry.sound()?.to_string())))
        .collect()
}

/// The engine's full audio stream as `(frame, message)` pairs, in emission order — the surface for
/// asserting on loops, where a sync, a stop, and the blanket stop a world rebuild sends must be told
/// apart.
///
/// ```no_run
/// # use ludion_engine::Engine;
/// # let engine = Engine::new();
/// let messages = ludion_testkit::audio::audio_messages(&engine);
/// // assert!(matches!(messages[0].1, ludion_engine::AudioMessage::StopAllLoops));
/// ```
pub fn audio_messages(engine: &Engine) -> Vec<(u64, AudioMessage)> {
    engine
        .audio_log()
        .iter()
        .map(|entry| (entry.frame.as_u64(), entry.message.clone()))
        .collect()
}

/// Every audio message an engine sends after [`install`](SoundTap::install), whole and in order.
/// `audio_log` keeps only the last entries, so a long drive would check only its tail.
///
/// ```no_run
/// # use ludion_engine::Engine;
/// # let mut engine = Engine::new();
/// let tap = ludion_testkit::audio::SoundTap::install(&mut engine);
/// // ... drive the engine ...
/// let sounds = tap.assert_sounds_decode(std::path::Path::new("games/hello-paddle/scenes"));
/// // assert!(sounds.contains("sounds/bounce.wav"));
/// ```
pub struct SoundTap {
    messages: Receiver<AudioMessage>,
}

impl SoundTap {
    /// Forwards every message `engine` sends from now on into the tap (replacing any sender).
    pub fn install(engine: &mut Engine) -> SoundTap {
        let (sender, messages) = crossbeam_channel::unbounded();
        engine.set_audio_sender(sender);
        SoundTap { messages }
    }

    /// Drains what arrived since `install` (or the last call), checks each distinct sound and track
    /// once through [`ludion_audio::check_message`] under `content_root`, and returns the distinct
    /// names checked, so a test can assert coverage.
    ///
    /// # Panics
    /// Listing every sound or track that would not play, one `name: diagnostic` per line.
    pub fn assert_sounds_decode(&self, content_root: &Path) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut failures = Vec::new();
        while let Ok(message) = self.messages.try_recv() {
            let (name, is_track) = match &message {
                AudioMessage::OneShot(event) => (event.sound.clone(), false),
                AudioMessage::LoopSync { sound, .. } => (sound.clone(), false),
                AudioMessage::MusicPlay { track, .. } => (track.clone(), true),
                _ => continue,
            };
            if !seen.insert((name.clone(), is_track)) {
                continue;
            }
            if let Err(diagnostic) = ludion_audio::check_message(content_root, &message) {
                failures.push(format!("{name}: {diagnostic}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} sound reference(s) would not play under `{}`:\n{}\n\
             help: fix the path or the file; the sink would play nothing here",
            failures.len(),
            content_root.display(),
            failures.join("\n"),
        );
        seen.into_iter().map(|(name, _)| name).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::PathBuf;

    use ludion_engine::AudioQueue;

    use super::*;

    const SCENE: &str = "format: 1\n\
entities:\n\
\x20 - components:\n\
\x20     Transform:\n\
\x20       translation:\n\
\x20         - 1.0\n\
\x20         - 2.0\n\
\x20         - 3.0\n";

    fn engine() -> Engine {
        let mut engine = Engine::new();
        engine.load_inline_scene(SCENE).expect("scene loads");
        engine
    }

    fn queue(engine: &mut Engine) -> &mut AudioQueue {
        engine
            .world_mut()
            .resource_mut::<AudioQueue>()
            .expect("AudioQueue installed")
    }

    /// A scratch directory unique to this process + `name`, holding a minimal mono PCM16 `ok.wav`.
    fn root_with_ok_wav(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-testkit-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let data: Vec<u8> = [1i16, 2, 3].iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // uncompressed PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&88200u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        std::fs::write(dir.join("ok.wav"), wav).unwrap();
        dir
    }

    fn panic_text(result: Result<BTreeSet<String>, Box<dyn std::any::Any + Send>>) -> String {
        let payload = result.expect_err("the assertion should panic");
        match payload.downcast::<String>() {
            Ok(text) => *text,
            Err(payload) => payload
                .downcast::<&str>()
                .map(|text| text.to_string())
                .expect("a string panic payload"),
        }
    }

    #[test]
    fn a_tap_checks_each_distinct_sound_once_and_names_them() {
        let root = root_with_ok_wav("distinct");
        let mut engine = engine();
        let tap = SoundTap::install(&mut engine);
        queue(&mut engine).play("ok.wav", 1.0);
        queue(&mut engine).play("ok.wav", 1.0);
        queue(&mut engine).loop_start("ok.wav", 1.0);
        engine.advance_live();
        let names = tap.assert_sounds_decode(&root);
        assert_eq!(names, BTreeSet::from(["ok.wav".to_string()]));
    }

    #[test]
    fn a_tap_names_every_sound_that_would_not_play() {
        let root = root_with_ok_wav("failures");
        let mut engine = engine();
        let tap = SoundTap::install(&mut engine);
        queue(&mut engine).play("ok.wav", 1.0);
        queue(&mut engine).play("missing.wav", 1.0);
        queue(&mut engine).music_play("missing.ogg", 1.0, 0);
        engine.advance_live();
        let text = panic_text(catch_unwind(AssertUnwindSafe(|| {
            tap.assert_sounds_decode(&root)
        })));
        for needle in ["missing.wav", "L1101", "missing.ogg", "L5003"] {
            assert!(text.contains(needle), "{needle} missing from:\n{text}");
        }
        assert!(
            !text.lines().any(|line| line.starts_with("ok.wav")),
            "{text}"
        );
    }

    /// The tap's reason to exist: the engine log keeps only its last entries, the tap keeps all.
    #[test]
    fn a_tap_sees_past_the_log_bound() {
        let root = root_with_ok_wav("bound");
        let mut engine = engine();
        let tap = SoundTap::install(&mut engine);
        queue(&mut engine).play("missing.wav", 1.0);
        for _ in 0..5000 {
            queue(&mut engine).play("ok.wav", 1.0);
        }
        engine.advance_live();
        assert!(
            engine
                .audio_log()
                .iter()
                .all(|entry| entry.sound() != Some("missing.wav")),
            "the log should have dropped the first message"
        );
        let text = panic_text(catch_unwind(AssertUnwindSafe(|| {
            tap.assert_sounds_decode(&root)
        })));
        assert!(text.contains("missing.wav"), "{text}");
    }
}
