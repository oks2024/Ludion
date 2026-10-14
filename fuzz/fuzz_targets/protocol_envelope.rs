#![no_main]
//! Target 2 — wire request envelope + `Command` JSON decoder (`Request::from_json`). Body in
//! `ludion_fuzz::protocol_envelope`; property: arbitrary input yields `Ok`/`L3xxx`, never a panic.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ludion_fuzz::protocol_envelope(data));
