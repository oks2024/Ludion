//! Dialogue voice (`ludion docs dialogue`, § Voice): durations from the WAV, locale fallback, the
//! per-channel audio, cut and suspend. Every test builds from `doc_text` documents on a fresh temp
//! root holding the WAVs, and reads the audio log the engine keeps.

use std::path::{Path, PathBuf};

use ludion_core::audio::{AudioCategory, AudioEvent, AudioMessage, Bus, VoiceFilter, VoiceId};
use ludion_core::codes::code;
use ludion_core::diagnostics::DiagnosticCode;
use ludion_core::ids::EntityId;
use ludion_engine::audio::AudioQueue;
use ludion_engine::dialogue::{
    self, ChannelAudio, DialogueChannels, DialogueControl, DialogueEvent, DialogueLine, EndReason,
    LineHandle,
};
use ludion_engine::localization;
use ludion_engine::{
    Diagnostics, DialogueConfig, DialogueEvents, DuckRule, Engine, EngineBuilder, dialogue_update,
};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;

/// Camera 0 (the listener), guard 1 (a world speaker).
const SCENE: &str = "\
format: 1
entities:
  - name: camera
    components:
      Transform: {}
      Camera: {}
  - name: guard
    components:
      Transform:
        translation:
          - 2.0
          - 0.0
          - 0.0
";
const GUARD: EntityId = EntityId::new(1, 0);

const STRINGS_EN: &str = "format: 1
kind: strings
locale: en
strings:
  speaker.operator: Operator
  speaker.guard: Guard
  speaker.hud: HUD
  t.line: Copy that.
";

const STRINGS_FR: &str = "format: 1
kind: strings
locale: fr
strings:
  t.line: Bien recu.
";

const SPEAKERS: &str = "format: 1
kind: dialogue
speakers:
  operator:
    name: speaker.operator
    channel: radio
  guard:
    name: speaker.guard
    channel: world
  hud:
    name: speaker.hud
    channel: ui
lines:
";

const VOICE: &str = "    voice: voice/{locale}/a.wav
";

/// `(id, speaker, extra fields at four-space indent)`; every line says `t.line`.
const LINES: &[(&str, &str, &str)] = &[
    ("radio", "operator", VOICE),
    (
        "radio_low",
        "operator",
        "    priority: low
    voice: voice/{locale}/a.wav
",
    ),
    (
        "radio_hi",
        "operator",
        "    priority: high
    interrupt: lower
    voice: voice/{locale}/a.wav
",
    ),
    (
        "radio_notail",
        "operator",
        "    tail_ticks: 0
    voice: voice/{locale}/a.wav
",
    ),
    (
        "radio_fixed",
        "operator",
        "    duration_ticks: 5
    voice: voice/{locale}/a.wav
",
    ),
    (
        "only_en",
        "operator",
        "    voice: voice/{locale}/only_en.wav
",
    ),
    ("plain", "operator", ""),
    (
        "gone",
        "operator",
        "    voice: voice/gone.wav
",
    ),
    ("world", "guard", VOICE),
    ("ui", "hud", VOICE),
];

fn bank(lines: &[(&str, &str, &str)]) -> String {
    let mut text = SPEAKERS.to_owned();
    for (id, speaker, extra) in lines {
        text.push_str(&format!(
            "  {id}:
    speaker: {speaker}
    text: t.line
{extra}"
        ));
    }
    text
}

/// A mono 16-bit PCM WAV of `frames` frames at 44100 Hz.
fn wav(frames: usize) -> Vec<u8> {
    let data: Vec<u8> = (0..frames).flat_map(|_| 1000i16.to_le_bytes()).collect();
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&44100u32.to_le_bytes());
    out.extend_from_slice(&88200u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// 22050 frames: 30 ticks at 60 Hz.
const EN_FRAMES: usize = 22_050;
/// 44100 frames: 60 ticks.
const FR_FRAMES: usize = 44_100;
const TAIL: u64 = 12;

fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-voice-dlg-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("voice/en")).unwrap();
    std::fs::create_dir_all(dir.join("voice/fr")).unwrap();
    std::fs::write(dir.join("voice/en/a.wav"), wav(EN_FRAMES)).unwrap();
    std::fs::write(dir.join("voice/fr/a.wav"), wav(FR_FRAMES)).unwrap();
    std::fs::write(dir.join("voice/en/only_en.wav"), wav(EN_FRAMES)).unwrap();
    std::fs::write(dir.join("voice/gone.wav"), wav(EN_FRAMES)).unwrap();
    dir
}

fn builder(dir: &Path) -> EngineBuilder {
    Engine::builder()
        .project_root(dir)
        .dialogue(DialogueConfig::default())
        .system("dialogue_update", dialogue_update)
        .query::<(&DialogueLine,)>()
        .query::<(&DialogueControl,)>()
        .doc_text("strings/en.yaml", STRINGS_EN)
        .doc_text("strings/fr.yaml", STRINGS_FR)
        .doc_text("dialogue/main.yaml", &bank(LINES))
}

fn boot_with(builder: EngineBuilder) -> Engine {
    let mut engine = builder.build().expect("the bank is valid");
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

fn boot(name: &str) -> Engine {
    boot_with(builder(&root(name)))
}

fn tick(engine: &mut Engine) -> Vec<DialogueEvent> {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .unwrap();
    engine
        .world()
        .resource::<DialogueEvents>()
        .unwrap()
        .iter()
        .cloned()
        .collect()
}

fn run(engine: &mut Engine, n: usize) -> Vec<Vec<DialogueEvent>> {
    (0..n).map(|_| tick(engine)).collect()
}

fn say(engine: &mut Engine, line: &str) -> LineHandle {
    let speaker = (line == "world").then_some(GUARD);
    dialogue::say(engine.world_mut(), line, speaker).unwrap()
}

/// The index of the tick `handle`'s line ended on, and why.
fn ended(per_tick: &[Vec<DialogueEvent>], handle: LineHandle) -> Option<(usize, EndReason)> {
    per_tick.iter().enumerate().find_map(|(i, events)| {
        events.iter().find_map(|event| match event {
            DialogueEvent::Ended {
                handle: h, reason, ..
            } if *h == handle => Some((i, *reason)),
            _ => None,
        })
    })
}

/// Every message the dialogue voice can produce, with its frame.
fn voice_log(engine: &Engine) -> Vec<(u64, AudioMessage)> {
    engine
        .audio_log()
        .iter()
        .filter(|entry| {
            matches!(
                entry.message,
                AudioMessage::VoiceStart { .. }
                    | AudioMessage::VoiceSync { .. }
                    | AudioMessage::VoiceStop { .. }
                    | AudioMessage::BusPause { .. }
                    | AudioMessage::Duck { .. }
                    | AudioMessage::OneShot(_)
            )
        })
        .map(|entry| (entry.frame.as_u64(), entry.message.clone()))
        .collect()
}

fn starts(engine: &Engine) -> Vec<AudioMessage> {
    voice_log(engine)
        .into_iter()
        .map(|(_, message)| message)
        .filter(|message| matches!(message, AudioMessage::VoiceStart { .. }))
        .collect()
}

fn count(engine: &Engine, pred: impl Fn(&AudioMessage) -> bool) -> usize {
    voice_log(engine)
        .iter()
        .filter(|(_, message)| pred(message))
        .count()
}

fn reported(engine: &Engine, code: DiagnosticCode) -> usize {
    engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code)
        .count()
}

// --- Durations and locales --------------------------------------------------------------------

#[test]
fn a_voiced_line_lasts_the_clip_plus_the_tail_and_follows_the_locale() {
    let mut engine = boot("duration");
    let en = say(&mut engine, "radio");
    let first = run(&mut engine, 80);
    assert_eq!(
        ended(&first, en),
        Some((
            (EN_FRAMES * 60).div_ceil(44_100) + TAIL as usize,
            EndReason::Completed
        ))
    );

    localization::set_locale(engine.world_mut(), "fr");
    let fr = say(&mut engine, "radio");
    let second = run(&mut engine, 80);
    assert_eq!(
        ended(&second, fr),
        Some((
            FR_FRAMES * 60 / 44_100 + TAIL as usize,
            EndReason::Completed
        )),
        "the French clip is longer"
    );
}

#[test]
fn an_authored_tail_and_an_authored_duration_override_the_default() {
    let mut engine = boot("overrides");
    let notail = say(&mut engine, "radio_notail");
    let ran = run(&mut engine, 40);
    assert_eq!(ended(&ran, notail), Some((30, EndReason::Completed)));
    let fixed = say(&mut engine, "radio_fixed");
    let ran = run(&mut engine, 10);
    assert_eq!(ended(&ran, fixed), Some((5, EndReason::Completed)));
}

#[test]
fn two_runs_produce_identical_traces() {
    let trace = |name: &str| {
        let mut engine = boot(name);
        localization::set_locale(engine.world_mut(), "fr");
        say(&mut engine, "radio");
        say(&mut engine, "world");
        let events = run(&mut engine, 100);
        (events, voice_log(&engine))
    };
    assert_eq!(trace("repro-a"), trace("repro-b"));
}

#[test]
fn a_missing_locale_file_falls_back_to_the_default_locale() {
    let mut engine = boot("fallback");
    localization::set_locale(engine.world_mut(), "fr");
    let line = say(&mut engine, "only_en");
    let ran = run(&mut engine, 60);
    assert_eq!(
        ended(&ran, line),
        Some((
            EN_FRAMES * 60 / 44_100 + TAIL as usize,
            EndReason::Completed
        ))
    );
    assert_eq!(reported(&engine, code::DIALOGUE_VOICE_FELL_BACK), 1);
    let AudioMessage::VoiceStart { sound, .. } = &starts(&engine)[0] else {
        unreachable!()
    };
    assert_eq!(sound, "voice/en/only_en.wav");
    say(&mut engine, "only_en");
    run(&mut engine, 60);
    assert_eq!(
        reported(&engine, code::DIALOGUE_VOICE_FELL_BACK),
        1,
        "once per path"
    );
}

#[test]
fn an_unreadable_voice_is_silent_and_lasts_the_reading_time() {
    let dir = root("unreadable");
    let mut engine = boot_with(builder(&dir));
    std::fs::remove_file(dir.join("voice/gone.wav")).unwrap();
    let gone = say(&mut engine, "gone");
    let plain = say(&mut engine, "plain");
    let ran = run(&mut engine, 300);
    let (gone_at, _) = ended(&ran, gone).unwrap();
    let (plain_at, _) = ended(&ran, plain).unwrap();
    // The two lines share a slot and a text, so the second starts when the first ends.
    assert_eq!(plain_at - gone_at, gone_at, "equal reading times");
    assert_eq!(reported(&engine, code::DIALOGUE_VOICE_UNREADABLE), 1);
    assert!(starts(&engine).is_empty(), "nothing is pushed");
}

// --- Channels ---------------------------------------------------------------------------------

#[test]
fn radio_lines_are_band_passed_voices_and_ui_lines_use_the_ui_bus() {
    let mut engine = boot("channels");
    say(&mut engine, "radio");
    run(&mut engine, 1);
    let AudioMessage::VoiceStart {
        sound,
        category,
        spatial,
        ..
    } = &starts(&engine)[0]
    else {
        panic!("a radio line starts a voice");
    };
    assert_eq!(sound, "voice/en/a.wav");
    assert_eq!(*category, AudioCategory::Voice);
    assert_eq!(spatial.filter, VoiceFilter::band_pass(300.0, 3400.0));

    say(&mut engine, "ui");
    run(&mut engine, 80);
    let AudioMessage::VoiceStart {
        category, spatial, ..
    } = &starts(&engine)[1]
    else {
        panic!("a ui line starts a voice");
    };
    assert_eq!(*category, AudioCategory::Ui);
    assert_eq!(spatial.filter, VoiceFilter::BYPASS);
}

#[test]
fn a_world_line_follows_its_speaker() {
    let mut engine = boot("world");
    say(&mut engine, "world");
    run(&mut engine, 1);
    let AudioMessage::VoiceStart { category, id, .. } = &starts(&engine)[0] else {
        panic!("a world line starts a voice");
    };
    assert_eq!(*category, AudioCategory::Voice);
    let id = *id;
    engine
        .world_mut()
        .get_mut::<ludion_engine::Transform>(GUARD)
        .unwrap()
        .unwrap()
        .translation = [15.0, 0.0, 0.0];
    run(&mut engine, 3);
    assert!(
        count(
            &engine,
            |m| matches!(m, AudioMessage::VoiceSync { id: i, .. } if *i == id)
        ) >= 1,
        "the voice is re-resolved against the speaker's new position"
    );
}

#[test]
fn a_channel_table_overrides_the_defaults() {
    let channels = DialogueChannels {
        radio: ChannelAudio {
            category: AudioCategory::Sfx,
            filter: VoiceFilter::BYPASS,
            gain: 0.5,
            ..ChannelAudio::default_for(dialogue::Channel::Radio)
        },
        ..DialogueChannels::default()
    };
    let dir = root("table");
    let mut engine = boot_with(builder(&dir).dialogue_channels(channels));
    say(&mut engine, "radio");
    run(&mut engine, 1);
    let AudioMessage::VoiceStart {
        category,
        gain,
        spatial,
        ..
    } = &starts(&engine)[0]
    else {
        panic!("a voice starts");
    };
    assert_eq!(*category, AudioCategory::Sfx);
    assert_eq!(*gain, 0.5);
    assert_eq!(spatial.filter, VoiceFilter::BYPASS);
}

// --- Cut, suspend, ducking --------------------------------------------------------------------

#[test]
fn a_cut_line_fades_its_voice_and_a_completed_one_sends_no_stop() {
    let mut engine = boot("cut");
    say(&mut engine, "radio_low");
    run(&mut engine, 5);
    say(&mut engine, "radio_hi");
    run(&mut engine, 2);
    let stops: Vec<_> = voice_log(&engine)
        .into_iter()
        .filter_map(|(frame, m)| match m {
            AudioMessage::VoiceStop { id, fade_frames } => Some((frame, id, fade_frames)),
            _ => None,
        })
        .collect();
    assert_eq!(
        stops,
        vec![(5, VoiceId(0), 6)],
        "the cut voice fades over 6 ticks"
    );
    assert_eq!(starts(&engine).len(), 2);

    run(&mut engine, 80);
    assert_eq!(
        count(&engine, |m| matches!(m, AudioMessage::VoiceStop { .. })),
        1,
        "the line that completed sent no stop"
    );
}

#[test]
fn suspend_pauses_the_voice_bus_once_and_resume_releases_it() {
    let mut engine = boot("suspend");
    say(&mut engine, "radio");
    run(&mut engine, 3);
    dialogue::suspend(engine.world_mut(), true);
    run(&mut engine, 3);
    let pauses = |engine: &Engine, held: bool| {
        count(engine, |m| {
            matches!(m, AudioMessage::BusPause { paused, buses, .. }
                if *paused == held && buses == &vec![Bus::Voice])
        })
    };
    assert_eq!(pauses(&engine, true), 1, "repeats send nothing");
    assert_eq!(pauses(&engine, false), 0);
    dialogue::suspend(engine.world_mut(), false);
    run(&mut engine, 3);
    assert_eq!(pauses(&engine, false), 1);
    assert_eq!(pauses(&engine, true), 1);
}

#[test]
fn a_voice_bus_duck_rule_dips_music_under_a_radio_line() {
    let dir = root("duck");
    let rule = DuckRule {
        when: Bus::Voice,
        ..DuckRule::default()
    };
    let mut engine = boot_with(builder(&dir).audio_duck(rule));
    say(&mut engine, "radio");
    run(&mut engine, 1);
    assert!(
        count(
            &engine,
            |m| matches!(m, AudioMessage::Duck { gain, .. } if *gain < 1.0)
        ) >= 1
    );
}

// --- push_voice -------------------------------------------------------------------------------

#[test]
fn push_voice_is_stoppable_retired_at_its_end_counted_for_ducking_and_never_synced() {
    let dir = root("push-voice");
    let rule = DuckRule {
        when: Bus::Voice,
        ..DuckRule::default()
    };
    let mut engine = Engine::builder()
        .project_root(&dir)
        .audio_duck(rule)
        .build()
        .unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    let voice = |sound: &str| AudioEvent {
        category: AudioCategory::Voice,
        ..AudioEvent::new(sound, 1.0)
    };
    fn queue(engine: &mut Engine) -> &mut AudioQueue {
        engine.world_mut().resource_mut::<AudioQueue>().unwrap()
    }
    let first = queue(&mut engine).push_voice(voice("voice/en/a.wav"));
    let second = queue(&mut engine).push_voice(voice("voice/en/a.wav"));
    engine.advance_live();
    assert_eq!(starts(&engine).len(), 2);
    assert!(
        count(
            &engine,
            |m| matches!(m, AudioMessage::Duck { gain, .. } if *gain < 1.0)
        ) >= 1
    );

    queue(&mut engine).voice_stop(first, 4);
    for _ in 0..40 {
        engine.advance_live();
    }
    // The second voice ended at 30 ticks: stopping it afterwards sends nothing.
    queue(&mut engine).voice_stop(second, 4);
    engine.advance_live();
    let stops: Vec<_> = voice_log(&engine)
        .into_iter()
        .filter_map(|(_, m)| match m {
            AudioMessage::VoiceStop { id, fade_frames } => Some((id, fade_frames)),
            _ => None,
        })
        .collect();
    assert_eq!(stops, vec![(first, 4)]);
    assert_eq!(
        count(&engine, |m| matches!(m, AudioMessage::VoiceSync { .. })),
        0
    );
}

// --- Validation -------------------------------------------------------------------------------

fn failing(builder: EngineBuilder) -> String {
    match builder.build() {
        Ok(_) => panic!("the build should have failed"),
        Err(error) => format!("{error:?}"),
    }
}

#[test]
fn a_bad_channel_table_or_one_without_dialogue_is_l1719() {
    let dir = root("l1719");
    let mut bad = DialogueChannels::default();
    bad.world.range = -1.0;
    assert!(failing(builder(&dir).dialogue_channels(bad)).contains("L1719"));
    let mut master = DialogueChannels::default();
    master.suspend_buses.push(Bus::Master);
    assert!(failing(builder(&dir).dialogue_channels(master)).contains("L1719"));
    let lonely = Engine::builder()
        .project_root(&dir)
        .dialogue_channels(DialogueChannels::default());
    assert!(failing(lonely).contains("L1719"));
}

#[test]
fn a_voice_that_does_not_read_is_l1720_and_a_missing_locale_file_is_the_warning_l1721() {
    let dir = root("l1720");
    std::fs::remove_file(dir.join("voice/en/a.wav")).unwrap();
    let message = failing(builder(&dir));
    assert!(message.contains("L1720"), "{message}");
    assert!(message.contains("lines/radio/voice"), "{message}");

    // `only_en` has no French file: a warning, the build succeeds.
    let engine = boot_with(builder(&root("l1721")));
    let warned: Vec<String> = engine
        .doc_validation_warnings()
        .iter()
        .filter(|d| d.code == code::DIALOGUE_VOICE_LOCALE_MISSING)
        .filter_map(|d| d.path.clone())
        .collect();
    assert_eq!(
        warned,
        vec!["dialogue/dialogue/main.yaml/lines/only_en/voice".to_owned()]
    );
}

#[test]
fn a_path_without_the_locale_placeholder_is_used_as_is() {
    let dir = root("no-placeholder");
    let bank = bank(&[(
        "fixed",
        "operator",
        "    voice: voice/gone.wav
",
    )]);
    let mut engine = boot_with(
        Engine::builder()
            .project_root(&dir)
            .dialogue(DialogueConfig::default())
            .system("dialogue_update", dialogue_update)
            .query::<(&DialogueLine,)>()
            .query::<(&DialogueControl,)>()
            .doc_text("strings/en.yaml", STRINGS_EN)
            .doc_text("strings/fr.yaml", STRINGS_FR)
            .doc_text("dialogue/main.yaml", &bank),
    );
    assert!(engine.doc_validation_warnings().is_empty());
    localization::set_locale(engine.world_mut(), "fr");
    let line = dialogue::say(engine.world_mut(), "fixed", None).unwrap();
    let ran = run(&mut engine, 60);
    assert_eq!(
        ended(&ran, line).map(|(_, r)| r),
        Some(EndReason::Completed)
    );
    assert_eq!(reported(&engine, code::DIALOGUE_VOICE_FELL_BACK), 0);
}

// --- Byte-identity ----------------------------------------------------------------------------

#[test]
fn a_text_only_world_is_unchanged_by_dialogue_channels() {
    let script = |name: &str, channels: Option<DialogueChannels>| {
        let dir = root(name);
        let mut b = builder(&dir);
        if let Some(channels) = channels {
            b = b.dialogue_channels(channels);
        }
        let mut engine = boot_with(b);
        say(&mut engine, "plain");
        run(&mut engine, 20);
        dialogue::suspend(engine.world_mut(), true);
        run(&mut engine, 3);
        dialogue::suspend(engine.world_mut(), false);
        run(&mut engine, 120);
        (
            engine.world_hash().unwrap().as_u64(),
            engine
                .audio_log()
                .iter()
                .map(|entry| (entry.frame.as_u64(), entry.message.clone()))
                .collect::<Vec<_>>(),
        )
    };
    let mut custom = DialogueChannels::default();
    custom.radio.gain = 0.25;
    custom.suspend_buses = vec![Bus::Voice, Bus::Ui];
    let (hash_a, log_a) = script("text-a", None);
    let (hash_b, log_b) = script("text-b", Some(custom));
    assert_eq!(hash_a, hash_b);
    assert_eq!(log_a, log_b);
    assert!(
        log_a.iter().all(|(_, m)| !matches!(
            m,
            AudioMessage::VoiceStart { .. } | AudioMessage::BusPause { .. }
        )),
        "a world that never voices a line sends nothing"
    );
}
