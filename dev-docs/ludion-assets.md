---
description: CPU-side asset IO (PNG decode) plus the opt-in debounced file watcher; never sees a wgpu type
---
# ludion-assets

**Scope:** CPU-only asset data: reading files to bytes, XXH3-64 content hashing, and decoding PNGs to packed RGBA8. This is the minimal loader the renderer needs — `ludion-engine`'s extract step calls it to resolve scene texture references, and `ludion-renderer` consumes the decoded pixels via `RenderScene::textures`. GPU upload is `ludion-renderer`'s job; the frame graph is `ludion-render-graph`. The debounced filesystem watcher the hot-reload hosts use lives here too, behind the opt-in `watch` feature.

## Layout
- `crates/ludion-assets/src/lib.rs` — the loader: `AssetBytes`, `Image`, `content_hash`, `load_bytes`, `load_png`, `decode_png`, `Image::premultiply`/`premultiplied`, plus unit tests.
- `crates/ludion-assets/src/shelf.rs` — `shelf::place`, the one shelf-placement loop both atlas packers (sprite, font) call. `MAX_PNG_SIDE` (8192, `pub`) is the decode limit every PNG-writing packer checks its extent against.
- `crates/ludion-assets/src/cooked.rs` — the runtime half of the texture cook: `cook_key`/`cooked_file_name`, the shared `CookTarget`/`ColorSpace`/`CookSettings`/`CompressedImage` types, `read_ktx2`. The encoders are `ludion-cook` (`ludion devdocs ludion-cook`); the runtime lookup lives in `ludion-engine/src/cooked_maps.rs`.
- `crates/ludion-assets/src/mips.rs` — the CPU mip chain both the renderer's `textures: source` path and `ludion-cook` share: `next_mip` (a 2×2 box on even sides, an area-weighted 3-tap on odd ones) and `mask_mip_chain` (coverage-preserving alpha for a base-colour map bound by a `mask` mesh; `CookSettings.alpha_cutoff` keys the cooked twin).
- `crates/ludion-assets/src/watch.rs` — `watch`/`WatchHandle`: `notify`'s OS watcher on a background thread, coalescing a burst of edits into one batch per quiet period (`L1103` when the root cannot be watched). Compiled only with the `watch` feature.
- `crates/ludion-assets/Cargo.toml` — dependencies: `ludion-core` (diagnostics), `png`, `xxhash-rust`, `ktx2` (reader only); behind the opt-in `watch` feature, `notify` + `crossbeam-channel`. No wgpu, no serde.
- `crates/ludion-engine/src/extract.rs` — the main caller: `extract::load_texture` (through a fresh `AssetCache`) resolves a scene's sprite, tile and UI-rect textures with `ludion_assets::load_png(project_root, rel).map(Image::premultiplied)`. `crates/ludion-engine/src/asset_cache.rs` is the second, doing the same on a cache miss for the hosted per-frame path.
- `examples/gallery/assets/` — `brick.png`, `checker.png`: the textures the gallery scenes reference.

## Rules
- **GPU-free by boundary rule** (`engine-architecture.md` §2): this crate deals in bytes and plain RGBA pixels, never a `wgpu` type. The renderer owns GPU upload and keys its texture identity on the content hash computed here. A wgpu type here would drag wgpu into the sim-side dependency tree and wreck compile times for content work.
- **Paths are `(root, relative)` pairs.** Every load takes a project root the caller supplies (for the CLI: the directory containing the scene file) and joins the relative path. The crate never invents a search path.
- **Decoded pixels reach the renderer premultiplied.** `load_png` returns straight alpha; the two engine seams convert with `Image::premultiplied` before the pixels enter `RenderScene::textures`, so every texture the sprite and UI-rect passes sample carries colour already weighted by its own coverage and both blend `src + dst * (1 - src.a)`. The weighting happens in linear light — pixels upload as `Rgba8UnormSrgb`, so multiplying the stored byte would darken a half-covered texel by its own gamma — and an opaque texel is returned byte-for-byte unchanged, which is why converting a fully opaque PNG moves no golden. Text is outside the convention: a glyph atlas is coverage the text rasterizer builds, never read through here.
- **Decoded pixels are always RGBA8** — tightly packed, top-to-bottom, `width * height * 4` bytes — so the renderer has exactly one texture format to upload.
- **Hashes are content hashes of source file bytes** (XXH3-64), not decoded pixels: a pure function of the bytes, identical on every platform and run.
- Errors are `L####` diagnostics: `L1101` (`ASSET_FILE_IO`) for unreadable files, `L1102` (`ASSET_PNG_DECODE`) for bytes that are not a decodable PNG. Both carry the relative path and the resolved absolute path in the message.
- **The watcher is opt-in (`watch` feature), off by default.** `notify` is a desktop OS-watcher dependency a device build must not carry, so only the two hot-reloading hosts enable it — `ludion-server` (`watch_assets` → `asset.reload`) and `ludion-cli` (`dev`). The engine, the renderer and every game depend on this crate with it off; `scripts/mobile-check.mjs` asserts the feature-less facade tree has no `notify`. Deciding *what* to reload stays the caller's; atlas packing is never a load-time job (`ludion generate-atlas` bakes offline, `ludion devdocs sprite-pipeline`).

- **The runtime half of the cook lives here, the encoders never do.** The engine computes the key and reads KTX2 without linking an encoder. The key hashes the *source* hash plus `COOKER_VERSION` and every setting; changing the key record orphans every cooked store (the pinned-value test fails first). `read_ktx2` requires the `LudionSrgbMean` key/value entry (`CompressedImage::mean`, the GI voxelizer's albedo input) and refuses a file without it; a layout change bumps `COOKER_VERSION`. It accepts exactly the format its target and colour space imply and rejects anything inconsistent; it hands `ktx2::Reader::new` only files whose format field and DFD region byte-equal what `dfd_region` generates, because ktx2 0.5 panics on a corrupt DFD (a texel dimension of 255 overflows) and `catch_unwind` would not help under `panic=abort`.

## How it works
`load_bytes(root, relative)` reads the file with `std::fs::read` and pairs the bytes with `content_hash(&bytes)` in an `AssetBytes`. `load_png` builds on it: it hands the bytes to `decode_png(bytes, label)`, which drives the `png` crate with `Transformations::EXPAND | STRIP_16` — palette expands to RGB, sub-8-bit grayscale widens to 8-bit, 16-bit channels narrow to 8-bit. The remaining color types (Grayscale, GrayscaleAlpha, Rgb, Rgba) are folded into RGBA8 by the `expand` helper (gray replicated across RGB, missing alpha set to 255). The result is an `Image { width, height, rgba, hash }` where `hash` is the source-file hash from `load_bytes`.

`decode_png` is public and takes in-memory bytes precisely so it can be unit-tested (and reused) without touching disk; `load_png` is the disk-backed convenience over it.

Downstream, `ludion-engine::extract` loads each texture a scene references relative to the scene's directory, premultiplies it, and places the decoded `Image` data into `RenderScene::textures` as a `TextureImage` (same field shape: width/height/rgba/hash) — an `Arc` shared with the `AssetCache`, so a cache hit is a refcount bump, not a copy. The renderer deduplicates and caches GPU textures on `hash` — which is the *source file* hash, so a sprite and a `UiRect` naming one path share a single entry, and that shared entry is why the convention cannot differ between the two passes.

## Verify
```
cargo test -p ludion-assets     # (cooked: key pin, level maths; round trips are in ludion-cook)
cargo test -p ludion-assets     # hash stability, RGBA round-trip, L1101/L1102 error paths
cargo test -p ludion-assets --features watch   # + the watcher: a written file is reported, a missing root is L1103
cargo run -p ludion-cli -- render --scene examples/gallery/sprites.yaml --out target/sprites.png
                                # exercises load_png end to end (brick.png / checker.png)
cargo run -p ludion-cli -- explain L1101
cargo run -p ludion-cli -- explain L1102
```

## Gotchas
- `ludion-testkit` deliberately does **not** use this crate to decode golden PNGs — it has its own small local decoder over the same `png` crate (`crates/ludion-testkit/src/image.rs`), to keep testkit's dependency edges narrow. If you change decoding behavior here, the two decoders are independent; goldens are unaffected.
- `Image.hash` distinguishes *files*, not *pixels*: two visually identical PNGs with different compression settings hash differently and will be uploaded as two GPU textures.
- Interlaced or exotic PNGs that survive EXPAND/STRIP_16 with an unexpected color type fall through to the `unsupported PNG color type` L1102 arm — the fix is to re-export the image as a standard 8-bit PNG, not to widen the decoder.
- `resolve` is a plain `root.join(relative)`: absolute paths in `relative` will override `root` (std `PathBuf::join` semantics), and no path canonicalization or escape checking happens here — callers own path policy.
