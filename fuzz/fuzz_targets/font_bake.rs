#![no_main]
//! Target 11 — the font bake. Body in `ludion_fuzz::font_bake`; property: no panic on arbitrary face bytes.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ludion_fuzz::font_bake(data));
