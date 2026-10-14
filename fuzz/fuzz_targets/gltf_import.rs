#![no_main]
//! Target 10 — glTF/GLB importer (`ludion_import::import`). Body in
//! `ludion_fuzz::gltf_import`; property: never panics on arbitrary bytes.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| ludion_fuzz::gltf_import(data));
