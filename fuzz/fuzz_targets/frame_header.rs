#![no_main]
//! Target 3 — 36-byte LFR1 frame-header decoder (`FrameHeader::decode`). Body in
//! `ludion_fuzz::frame_header`; property: any byte slice yields `Ok`/`L3004`, never a panic.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ludion_fuzz::frame_header(data));
