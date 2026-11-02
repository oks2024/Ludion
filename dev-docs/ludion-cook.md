---
description: The texture cook's encoders: BC7/ASTC 4x4, CPU mip chain, KTX2 writer; CLI-only, per-machine bytes
---
# ludion-cook

**Scope:** turning a decoded RGBA8 map into a BC7 (desktop) or ASTC 4×4 (iOS) KTX2 file. The key, the shared types and the KTX2 *reader* the engine uses are `ludion-assets::cooked` (`ludion devdocs ludion-assets`); the plan is `Plan/cooked-assets.md`.

## Layout
- `crates/ludion-cook/src/lib.rs` — `cook(&Image, &CookSettings) -> CompressedImage` (cap → align → chain → encode; stores `srgb_mean` of the pre-cap source pixels in `mean`), the preset and `ENCODER_VERSIONS` constants.
- `src/mips.rs` — the cap, the bilinear align-up, edge-replication padding. The box filter (`next_mip`: 2×2 on even sides, area-weighted 3-tap on odd ones; sRGB colour averages in linear light, alpha and linear maps average in bytes, rounded half up) lives in `ludion_assets::mips` beside `mask_mip_chain`. With `CookSettings.alpha_cutoff` set (a base-colour map bound by a `mask` mesh) the chain is `mask_mip_chain`; the cutoff joins the cook key only when set, so a cooked mask map is its own store entry and existing keys keep their bytes.
- `src/encode.rs` — `intel_tex_2` BC7 (`opaque_basic`, or `alpha_basic` when any level-0 alpha < 255) and `ctt-astcenc` ASTC (`Medium`; `LdrSrgb`/`Ldr` profile; one single-threaded context per `cook` call).
- `src/ktx2_write.rs` — `write_ktx2`: header, level index, DFD, one key/value entry (`LudionSrgbMean`, the image's `mean`), levels smallest first, 16-byte aligned. Returns `Vec<u8>`; the crate does no file IO.
- `tests/cook.rs`, `tests/cooker_version.rs` — round trips through `read_ktx2` and an independent decoder (`texture2ddecoder`), corruption, thread determinism, the encoder pin.

## Rules
- **Only `ludion-cli` links this crate.** An encoder in the engine's tree means every game ships a C++ ASTC encoder.
- **The CPU chain is identical everywhere; encoded bytes are per machine.** `ludion_assets::mips`, the cap, the align and the padding give the same bytes on every platform and thread count. `ctt-astcenc` builds with `ASTCENC_NO_INVARIANCE` and `-ffp-contract=fast` and dispatches by ISA; `intel_tex_2`'s ISPC kernels are per ISA, and cross-ISA identity is unmeasured. On one machine the output is run-to-run identical at any thread count (`bytes_are_identical_across_runs_and_threads`). The store is derived and per machine, so nothing may pin encoded bytes across machines. No thread-locals, no rayon (`ctt-astcenc` default features), no platform libm (the strict clippy tier bans `f32::powf`). Parallelism across maps is the caller's, through `ludion_core::par`.
- **Encoders are pinned with `=`.** Moving `intel_tex_2` or `ctt-astcenc` changes the bytes: bump `ludion_assets::cooked::COOKER_VERSION` and update `ENCODER_VERSIONS`/`ENCODER_VERSIONS_FOR`; `tests/cooker_version.rs` reads `Cargo.lock` and fails otherwise.
- **Level 0 is resampled, lower levels are padded.** wgpu needs a block-compressed level 0 that is a multiple of 4; padding it would shrink the content inside UV 0..1, so a non-aligned map is resampled up (aligned maps never are). Lower levels (6×6, 2×2, 1×1) are edge-padded for the encoder only. Order: cap → align → chain.

## Gotchas
- `ktx2` 0.5 `Reader::new` rejects a DFD that ends at EOF, which is why level data follows the DFD.
- `astcenc-rs` was rejected for its libclang build dependency.

## Verify
```
cargo nextest run -p ludion-cook -p ludion-assets
cargo test --doc -p ludion-cook
node scripts/clippy-denylist.mjs
cargo deny check licenses
```
