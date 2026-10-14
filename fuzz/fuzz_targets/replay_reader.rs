#![no_main]
//! Target 4 — LRP1 replay reader on corrupt input (`ReplayReader::new` + `records` + `seek`). Body
//! in `ludion_fuzz::replay_reader`. Probes both an in-the-clear-length steered slice/alloc and a
//! zstd decompression bomb (surfaced by libFuzzer's rss/malloc limit); framing property: no panic.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ludion_fuzz::replay_reader(data));
