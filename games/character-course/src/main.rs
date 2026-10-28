#![forbid(unsafe_code)]

//! `character-course` — the game binary: describe the engine, hand it to `ludion::run`.

use std::process::ExitCode;

fn main() -> ExitCode {
    ludion::run(character_course::game())
}
