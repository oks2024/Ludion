#![no_main]
//! Target 6 — every `ludion_scene::KINDS` document checker. Body in
//! `ludion_fuzz::kind_roundtrip`; property: no panic, and each kind's canonical emit re-checks and is idempotent.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ludion_fuzz::kind_roundtrip(data));
