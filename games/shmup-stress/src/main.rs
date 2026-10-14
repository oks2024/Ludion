#![forbid(unsafe_code)]

//! `shmup-stress` — the stress-arena binary. Describe the engine, hand it to `ludion::run` (which
//! parses `--scene`/`--profile`/`--record`/`--addr`, hosts the scene over the wire protocol, and
//! shuts down cleanly on Ctrl-C).

use std::process::ExitCode;

fn main() -> ExitCode {
    ludion::run(shmup_stress::game())
}
