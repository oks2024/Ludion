#![no_main]
//! Target 9 — `engine.restore` (`Command::EngineRestore`). Body in
//! `ludion_fuzz::restore_snapshot`; property: never panics on a hostile snapshot.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ludion_fuzz::restore_snapshot(data));
