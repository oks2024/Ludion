#![no_main]
//! Target 1 — strict-subset YAML scene parser (`ludion_scene::parse`). Body in
//! `ludion_fuzz::scene_parse`; property: never panics on arbitrary input.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ludion_fuzz::scene_parse(data));
