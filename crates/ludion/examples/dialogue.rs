//! Dialogue: a held radio call, a bark that waits for it, a two-speaker world exchange, a jammed
//! radio window, voiced lines whose clip sets their length, and a switch to French - the copyable
//! pattern for `ludion docs dialogue`.
//!
//! A game opts in with [`EngineBuilder::dialogue`], schedules [`dialogue_update`] after the systems
//! that `say` a line, and reads [`DialogueEvents`] and [`Subtitles`]. Everything above the `main`
//! divider is copyable into a real game. Run it headless (writes the string tables, the line bank and
//! two voice clips per language to a temp content root, prints each event with its frame, then the
//! voice starts and ducks the engine sent):
//!
//! ```text
//! cargo run -p ludion --example dialogue
//! ```

use ludion::dialogue::{self, Channel, DialogueControl, DialogueEvent, DialogueLine};
use ludion::localization;
use ludion::prelude::*;
use std::path::PathBuf;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// Says the scripted lines at their frames. A real game says lines from its own systems.
fn script(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64();
    if frame == 0 {
        let (a, b) = (world.spawn(), world.spawn());
        // A held call of two lines; the bark below is not critical, so it waits out the hold.
        let _ = dialogue::say_sequence(world, &["call_1", "call_2"], &[None, None]);
        let _ = dialogue::say(world, "bark", None);
        let _ = dialogue::say_with(world, "guard_hello", Some(a), &[("name", "Neon")]);
        let _ = dialogue::say(world, "guard_reply", Some(b));
    }
    match frame {
        // The radio is jammed for a while: a new bark waits until it clears.
        260 => dialogue::block_channel(world, Channel::Radio, true),
        290 => dialogue::block_channel(world, Channel::Radio, false),
        270 => {
            let _ = dialogue::say(world, "bark", None);
        }
        // Switch language mid-run; a live subtitle re-resolves.
        310 => localization::set_locale(world, "fr"),
        // Said in French: the French clip is longer, so the line is too.
        330 => {
            let _ = dialogue::say(world, "call_1", None);
        }
        _ => {}
    }
}

fn game(root: PathBuf) -> EngineBuilder {
    Engine::builder()
        .project_root(root)
        .dialogue(DialogueConfig::default())
        // Music and ambience dip 9 dB while any `Voice`-bus line sounds.
        .audio_duck(DuckRule::default())
        .system("script", script)
        .system("dialogue_update", dialogue_update)
        .query::<(&DialogueLine,)>()
        .query::<(&DialogueControl,)>()
}

const STRINGS_EN: &str = "format: 1
kind: strings
locale: en
strings:
  speaker.operator: Operator
  speaker.guard: Guard
  call.1: Operator to Neon, do you copy?
  call.2: Move to the roof, now.
  bark: Heads up.
  guard.hello: Halt, {name}!
  guard.reply: Let them through.
  hud.label: Comms online
";

const STRINGS_FR: &str = "format: 1
kind: strings
locale: fr
strings:
  speaker.operator: Opératrice
  bark: Attention.
  guard.hello: Halte, {name} !
  hud.label: Communications actives
";

const BANK: &str = "format: 1
kind: dialogue
speakers:
  operator:
    name: speaker.operator
    channel: radio
  guard_a:
    name: speaker.guard
    channel: world
  guard_b:
    name: speaker.guard
    channel: world
lines:
  bark:
    speaker: operator
    text: bark
    priority: low
    max_wait_ticks: 300
  call_1:
    speaker: operator
    text: call.1
    priority: high
    holds_gameplay: true
    voice: voice/{locale}/call_1.wav
  call_2:
    speaker: operator
    text: call.2
    priority: high
    holds_gameplay: true
    voice: voice/{locale}/call_2.wav
    cues:
      ping: 20
  guard_hello:
    speaker: guard_a
    text: guard.hello
    voice: voice/{locale}/guard_hello.wav
  guard_reply:
    speaker: guard_b
    text: guard.reply
";

/// A mono 16-bit PCM WAV of `seconds` of silence at 44.1 kHz: a stand-in for a recorded line.
fn silence(seconds: f32) -> Vec<u8> {
    let frames = (seconds * 44_100.0) as usize;
    let data = vec![0u8; frames * 2];
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&44_100u32.to_le_bytes());
    wav.extend_from_slice(&88_200u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    wav
}

/// A keyed HUD label: it shows `hud.label` in the active locale; `text` is only the fallback.
const HUD_SCENE: &str = "format: 1
seed: 1
entities:
  - name: hud_label
    components:
      UiText:
        text: Comms
        key: hud.label
";

/// The keyed label as the engine resolved it this tick.
fn hud_label(engine: &mut Engine) -> String {
    engine
        .world_mut()
        .query::<(&UiText,)>()
        .next()
        .map(|(label,)| label.shown().to_owned())
        .unwrap_or_default()
}

// --- Headless demo `main` ------------------------------------------------------------------------

fn write(root: &std::path::Path, name: &str, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap_or(root))?;
    std::fs::write(path, bytes)
}

fn main() -> ExitCode {
    let root = std::env::temp_dir().join(format!("ludion-dialogue-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let written = write(&root, "strings/en.yaml", STRINGS_EN)
        .and_then(|()| write(&root, "strings/fr.yaml", STRINGS_FR))
        .and_then(|()| write(&root, "dialogue/main.yaml", BANK))
        // English clips; the French call is longer than the English one.
        .and_then(|()| write(&root, "voice/en/call_1.wav", silence(1.5)))
        .and_then(|()| write(&root, "voice/en/call_2.wav", silence(1.0)))
        .and_then(|()| write(&root, "voice/en/guard_hello.wav", silence(0.5)))
        .and_then(|()| write(&root, "voice/fr/call_1.wav", silence(2.0)))
        .and_then(|()| write(&root, "voice/fr/call_2.wav", silence(1.2)))
        .and_then(|()| write(&root, "voice/fr/guard_hello.wav", silence(0.6)));
    if let Err(error) = written {
        eprintln!("could not write the demo docs: {error}");
        return ExitCode::FAILURE;
    }
    let mut engine = match game(root.clone()).build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(HUD_SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    let mut printed = 0;
    for _ in 0..560 {
        if let Err(diagnostic) = engine.execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        })) {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
        let frame = engine.frame().as_u64();
        if frame == 300 || frame == 560 {
            println!("frame {frame:>3}  hud     {}", hud_label(&mut engine));
        }
        let world = engine.world();
        let Some(events) = world.resource::<DialogueEvents>() else {
            return ExitCode::FAILURE;
        };
        for event in events.iter() {
            printed += 1;
            match event {
                DialogueEvent::Started {
                    line,
                    speaker,
                    channel,
                    ..
                } => {
                    let text = world
                        .resource::<Subtitles>()
                        .and_then(|subtitles| subtitles.iter().find(|s| &s.line == line))
                        .map(|s| format!("{}: {}", s.speaker, s.text))
                        .unwrap_or_default();
                    println!("frame {frame:>3}  started {line:<11} {channel:?} {speaker:?} {text}");
                }
                DialogueEvent::Ended { line, reason, .. } => {
                    println!("frame {frame:>3}  ended   {line:<11} {reason:?}");
                }
                DialogueEvent::Cue { line, name, .. } => {
                    println!("frame {frame:>3}  cue     {line:<11} {name}");
                }
            }
        }
    }
    for entry in engine.audio_log() {
        let message = format!("{:?}", entry.message);
        if let Some(sound) = entry.sound().filter(|_| message.starts_with("VoiceStart")) {
            println!("frame {:>3}  voice   {sound}", entry.frame.as_u64());
        } else if message.starts_with("Duck") || message.starts_with("VoiceStop") {
            println!("frame {:>3}  audio   {message}", entry.frame.as_u64());
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    if printed == 0 {
        eprintln!("no dialogue events fired");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
