#![no_main]
//! Target 8 — WAV decoder (`ludion_assets::decode_wav`). Body in
//! `ludion_fuzz::wav_decode`; property: never panics on arbitrary bytes.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ludion_fuzz::wav_decode(data));
