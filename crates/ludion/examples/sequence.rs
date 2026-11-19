//! Sequence: a radar-online beat — a camera push-in and restore, a clip, a UI flash tween, a sound, a
//! handler line and a `radar_online` event — played once to its end and once skipped, the copyable
//! pattern for `ludion docs sequences`.
//!
//! A game opts in with [`EngineBuilder::sequences`], schedules [`sequence_update`] before
//! `dialogue_update`, starts a beat with `sequences::play` and reads [`SequenceEvents`]. Everything
//! above the `main` divider is copyable into a real game. Run it headless (writes the docs to a temp
//! content root and prints each event with its frame):
//!
//! ```text
//! cargo run -p ludion --example sequence
//! ```

use ludion::prelude::*;
use ludion::sequences::{self, SequenceEvent, SequenceHandle};
use std::path::PathBuf;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

fn game(root: PathBuf) -> EngineBuilder {
    Engine::builder()
        .project_root(root)
        .dialogue(DialogueConfig::default())
        .sequences()
        .system("sequence_update", sequence_update)
        .system("dialogue_update", dialogue_update)
}

const STRINGS: &str = "format: 1
kind: strings
locale: en
strings:
  speaker.handler: Handler
  handler.online: Radar is up.
";

const BANK: &str = "format: 1
kind: dialogue
speakers:
  handler:
    name: speaker.handler
    channel: radio
lines:
  online:
    speaker: handler
    text: handler.online
    duration_ticks: 30
";

const BEAT: &str = "format: 1
kind: sequence
roles:
  - camera
  - radar
length_ticks: 60
tracks:
  camera:
    - at: 0
      role: camera
      rig:
        focus_offset:
          - 0.0
          - 1.5
          - 0.0
    - at: 60
      role: camera
      restore: true
  animation:
    - at: 10
      role: radar
      clip: spin
      fade: 0.1
  ui:
    - at: 10
      role: radar
      tracks:
        - field: opacity
          from:
            - 0.0
          to:
            - 1.0
          duration: 0.2
  audio:
    - at: 10
      play: sounds/radar_on.wav
  dialogue:
    - at: 20
      line: online
  event:
    - at: 30
      name: radar_online
";

// --- Headless demo `main` ------------------------------------------------------------------------

fn write(root: &std::path::Path, name: &str, text: &str) -> std::io::Result<()> {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap_or(root))?;
    std::fs::write(path, text)
}

/// Plays the beat, optionally skipping it at `skip_at`, printing each event; returns the camera's
/// final `focus_offset`, the radar's final clip and whether `radar_online` fired.
fn run(
    root: &std::path::Path,
    label: &str,
    skip_at: Option<usize>,
) -> Option<([f32; 3], String, bool)> {
    let mut engine = game(root.to_path_buf()).build().ok()?;
    engine
        .load_inline_scene("format: 1\nseed: 1\nentities: []\n")
        .ok()?;
    let world = engine.world_mut();
    let camera = world.spawn();
    world.insert(camera, Camera::default()).ok()?;
    world.insert(camera, CameraRig::default()).ok()?;
    let radar = world.spawn();
    world.insert(radar, Animation::default()).ok()?;
    let handle: SequenceHandle = sequences::play(
        world,
        "sequences/radar.yaml",
        &[("camera", camera), ("radar", radar)],
    )
    .ok()?;
    let mut online = false;
    for tick in 0..70 {
        if skip_at == Some(tick) {
            sequences::skip(engine.world_mut(), handle);
        }
        engine
            .execute(Command::SimStep(command::SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .ok()?;
        let events = engine.world().resource::<SequenceEvents>()?;
        for event in events.iter() {
            match event {
                SequenceEvent::KeyApplied { track, at, .. } => {
                    println!("{label} tick {tick:>2}  key {track:?} at {at}");
                }
                SequenceEvent::Fired { name, .. } => {
                    online |= name == "radar_online";
                    println!("{label} tick {tick:>2}  fired {name}");
                }
                SequenceEvent::Ended { end, .. } => {
                    println!("{label} tick {tick:>2}  ended {end:?}")
                }
            }
        }
    }
    let world = engine.world();
    let focus = world.get::<CameraRig>(camera).ok()??.rig.focus_offset;
    let clip = world.get::<Animation>(radar).ok()??.clip.clone();
    Some((focus, clip, online))
}

fn main() -> ExitCode {
    let root = std::env::temp_dir().join(format!("ludion-sequence-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let written = write(&root, "strings/en.yaml", STRINGS)
        .and_then(|()| write(&root, "dialogue/main.yaml", BANK))
        .and_then(|()| write(&root, "sequences/radar.yaml", BEAT));
    if let Err(error) = written {
        eprintln!("could not write the demo docs: {error}");
        return ExitCode::FAILURE;
    }
    let watched = run(&root, "watched", None);
    let skipped = run(&root, "skipped", Some(5));
    let _ = std::fs::remove_dir_all(&root);
    match (watched, skipped) {
        (Some(watched), Some(skipped)) if watched == skipped && watched.2 => {
            println!("both runs end in {watched:?}");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("the runs disagree or failed: {other:?}");
            ExitCode::FAILURE
        }
    }
}
