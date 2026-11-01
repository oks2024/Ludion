//! A skinned character's clip bindings: loads the gallery mannequin's `kind: character` document
//! and prints each bound clip's name, duration, loop flag, effective speed and events.
//!
//! The character names a skinned mesh and binds short clip names (`run`, `idle`) to
//! `kind: animation` files. Each binding carries a playback `speed` and, for an imported clip that
//! has no events of its own, an `events` overlay — [`CharacterData::clip`] returns the effective
//! values `Animation.clip` resolves through. An entity draws a character with
//! `Mesh.shape: { character: "characters/mannequin.character.yaml" }`.
//!
//! Run it headless (no window, no GPU, deterministic):
//!
//! ```text
//! cargo run -p ludion-engine --example character_clips
//! ```

use std::path::Path;
use std::process::ExitCode;

use ludion_engine::load_character;

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
    let character = match load_character(
        ludion_assets::AssetRoot::content(&root),
        "characters/mannequin.character.yaml",
    ) {
        Ok(character) => character,
        Err(diagnostics) => {
            for diagnostic in diagnostics {
                eprintln!("{diagnostic}");
            }
            return ExitCode::FAILURE;
        }
    };

    println!("mesh {}", character.mesh);
    for clip in &character.clips {
        let events: Vec<String> = clip
            .events
            .iter()
            .map(|event| format!("{}@{}", event.name, event.t))
            .collect();
        println!(
            "clip {:<5} duration {}s loop {} speed {} events [{}]",
            clip.name,
            clip.clip.duration,
            clip.clip.looped,
            clip.speed,
            events.join(", ")
        );
    }
    ExitCode::SUCCESS
}
