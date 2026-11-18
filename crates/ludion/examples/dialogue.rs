//! Dialogue: a held radio call, a bark that waits for it, a two-speaker world exchange, a jammed
//! radio window, and a switch to French - the copyable pattern for `ludion docs dialogue`.
//!
//! A game opts in with [`EngineBuilder::dialogue`], schedules [`dialogue_update`] after the systems
//! that `say` a line, and reads [`DialogueEvents`] and [`Subtitles`]. Everything above the `main`
//! divider is copyable into a real game. Run it headless (writes the string tables and the line bank
//! to a temp content root and prints each event with its frame):
//!
//! ```text
//! cargo run -p ludion --example dialogue
//! ```

use ludion::dialogue::{self, Channel, DialogueControl, DialogueEvent, DialogueLine};
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
        310 => dialogue::set_locale(world, "fr"),
        _ => {}
    }
}

fn game(root: PathBuf) -> EngineBuilder {
    Engine::builder()
        .project_root(root)
        .dialogue(DialogueConfig::default())
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
";

const STRINGS_FR: &str = "format: 1
kind: strings
locale: fr
strings:
  speaker.operator: Opératrice
  bark: Attention.
  guard.hello: Halte, {name} !
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
  call_2:
    speaker: operator
    text: call.2
    priority: high
    holds_gameplay: true
    cues:
      ping: 20
  guard_hello:
    speaker: guard_a
    text: guard.hello
  guard_reply:
    speaker: guard_b
    text: guard.reply
";

// --- Headless demo `main` ------------------------------------------------------------------------

fn write(root: &std::path::Path, name: &str, text: &str) -> std::io::Result<()> {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap_or(root))?;
    std::fs::write(path, text)
}

fn main() -> ExitCode {
    let root = std::env::temp_dir().join(format!("ludion-dialogue-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let written = write(&root, "strings/en.yaml", STRINGS_EN)
        .and_then(|()| write(&root, "strings/fr.yaml", STRINGS_FR))
        .and_then(|()| write(&root, "dialogue/main.yaml", BANK));
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
    if let Err(diagnostics) = engine.load_inline_scene("format: 1\nseed: 1\nentities: []\n") {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    let mut printed = 0;
    for _ in 0..400 {
        if let Err(diagnostic) = engine.execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        })) {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
        let frame = engine.frame().as_u64();
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
    let _ = std::fs::remove_dir_all(&root);
    if printed == 0 {
        eprintln!("no dialogue events fired");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
