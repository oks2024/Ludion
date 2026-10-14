#![forbid(unsafe_code)]

//! `breakout` — the game binary. A Ludion game's `main` is this small: describe the engine, hand
//! it to `ludion::run` (which parses `--scene`/`--addr`/`--record`/`--paused`/`--window`, hosts the
//! scene over the wire protocol, and shuts down cleanly on Ctrl-C or window close).

use std::process::ExitCode;

fn main() -> ExitCode {
    ludion::run(breakout::game())
}
