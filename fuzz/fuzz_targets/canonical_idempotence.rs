#![no_main]
//! Target 5 — canonical-writer idempotence (`emit` is a fixed point of `parse`). Body in
//! `ludion_fuzz::canonical_idempotence`; a mismatch asserts, which is the finding.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ludion_fuzz::canonical_idempotence(data));
