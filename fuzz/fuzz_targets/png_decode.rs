#![no_main]
//! Target 7 — PNG decoder (`ludion_assets::decode_png`). Body in
//! `ludion_fuzz::png_decode`; property: never panics, and a decoded image is width × height RGBA8.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ludion_fuzz::png_decode(data));
