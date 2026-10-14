# Cooked assets — GPU-compressed textures and binary meshes, per platform

> **Status:** Waves 0–1 shipped 2026-10-14; Wave 2's `ludion sync` cook stage shipped, vigil's sync
> and budget cut (2b) and Waves 3–4 open. Instrument: `Plan/scene-load-speed.md`'s `load` report.
>
> **Wave 1 exit (2026-10-14, reference box, RTX 3090).** The plaza (`scenes/district-plaza.yaml`),
> `--profile 30`, one cold run discarded, medians of three warm runs, `source` → `cooked`
> (store from `ludion sync --project games/vigil`: 495 maps reused, 0 cooked, 0 failed, 507 MB):
> `texture_ms` 319.5 → 135.2, `mesh_ms` 426.5 → 416.6, `gpu_upload_ms` 239.0 → 294.9, `total_ms`
> 2497.8 → 2474.6. Counters: texture decoded 423 → 0, cooked 0 → 423, zero L1271 fallbacks; the 423
> cooked reads are 457 MB against 280 MB of PNG bytes. VRAM: the store's level bytes are 0.2500 of
> the RGBA8 mip chain (507.3 MB of 2029 MB over all 495 maps; all BC7: 184 sRGB, 311 linear). SSIM, the plaza at frame 0, 1920×1080, source vs cooked: global 0.9989
> (≥ 0.99), MAE 0.13; one tile (r1c4, a near-black stretch with a small spiky foliage prop) scores
> 0.224 under the 0.95 tile floor, and source vs source is 1.0000, so the difference is the cooked
> maps; the side-by-side shows no visible difference. PNGs and the tile crop:
> `target/tmp/cooked-exit/plaza-{source,cooked}.png`, `tile-r1c4.png`.
> Exit bullets: **texture decode ≈ 0 — met** (decode count 0; `texture_ms` −58 %, the remainder is
> reading and uploading the KTX2); **VRAM ≤ ¼ — met** (0.2500, no margin); **perceptual look —
> met** (the gate is the global SSIM; a single near-black tile is SSIM noise on low variance and the
> side-by-side is identical by eye). `total_ms` barely moves: `gpu_upload_ms` rose 56 ms despite a
> quarter of the bytes, cause unmeasured (likely per-level `write_texture` of 495 full chains), a
> follow-up before 2b's budget cut.

## Why

`Plan/scene-load-speed.md` measured the reference game's mesh plaza district. It loads 235 PNG
material maps (144 at 1024², 84 at 512²), which decode to 694 MB of RGBA8 and take 1.05 s to decode
on one thread. With GPU-generated mips that is about 0.9 GB of VRAM for one district's mesh maps.
The meshes are 23 MB of YAML (0.83 s to parse) for 83k vertices. Parallel prefetch hides the decode
cost on a 16-core desktop. It does nothing for:

- **VRAM and upload volume.** RGBA8 is 4 bytes per texel, BC7 and ASTC 4×4 are 1 byte. The iOS
  target's A15 (medium quality, a shared memory budget) cannot hold an RGBA8 district.
- **Weak CPUs.** A phone has about 6 cores, not 32. PNG inflate and YAML float parsing are the
  wrong work to do at load time on a device.
- **Mip generation and upload at load.** Precomputed mips arrive with the texture.

Engine gap map item: "PNG decoded to RGBA8 only; no BC/ASTC/KTX2 compression"; texture compression
"before iOS device work" (`Plan/engine-gap-map.md`).

## Decisions

1. **Cooking is a pipeline verb's job, never a load-time job.** The encoders are linked by
   `ludion-cli` only (a new `ludion-cook` crate, depended on by the CLI). No game binary, iOS build or
   `ludion-engine` links an encoder. Games only read cooked files.
2. **The sources stay the source of truth.** The PNG maps and `.mesh.yaml` documents remain what
   import writes, what agents read, and what every hot-reload, diff and drift gate keys on. A cooked
   file is a cache of a source. Deleting every cooked file changes load time, never correctness.
3. **Cooked files are content-addressed.** Each cooked file is keyed by the xxh3 of its source
   bytes, plus the cook settings and a cooker version constant, in a project-level store:
   `<project>/.ludion/cooked/<target>/<key>.<ext>`. `.ludion/` is already the ignored per-project
   scratch dir in both ignore files. Because the key is the source hash:
   - a re-import that writes byte-identical PNGs reuses every cooked map;
   - staleness is impossible — a changed source has a different key, so it misses;
   - the engine needs no sync record to find a cooked file. It hashes the source bytes it already
     reads (xxh3: 154 MB of plaza PNGs in under 0.1 s) and looks the key up.

   **The cooker version is a constant, not the binary's hash.** The importer keys on its binary's
   hash (`importer_fingerprint`), so every CLI rebuild re-imports. Re-encoding the whole store on
   every rebuild would cost minutes, so the cook keys on an explicit version bumped when the encoder
   or its settings change. A test pins it to the encoder dependency's version, so it cannot go stale
   silently.
4. **Targets are named, never `cfg`'d.** Two cook targets:
   - `desktop`: BC7 for every map — colour maps sRGB, metallic-roughness/normal/occlusion linear.
     Windows and Apple-silicon macOS always expose BC (wgpu `TEXTURE_COMPRESSION_BC`).
   - `ios`: ASTC 4×4 LDR. iPhones before Apple9-family GPUs do not support BC, the A15 included.

   No shader change: normal maps stay three-channel BC7/ASTC, not BC5. The runtime requests the
   compression feature only when the adapter offers it, and picks the target by adapter features,
   never by OS.
5. **The container is KTX2**, a standard container external tools can inspect. The full mip chain
   is precomputed at cook time with the same filter as `mipgen.rs`. The engine reads it through the
   pure-Rust `ktx2` parser (Apache-2.0). The writer is a small function in `ludion-cook`.
6. **Pixels change only by an explicit setting, never by whether a cache file exists.** This needs a
   new render-only quality axis, `textures: source | cooked`, with no `WorldHash` or replay effect.
   - **Verification paths default to `source`:** `ludion render`, `ludion test`, goldens,
     `--render-out`, `--replay-frames`. Every committed image stays a function of the scene and its
     sources on any machine, cooked or not.
   - **Interactive hosts default to `cooked`:** the game window, `serve`, the editor.
   - **Exported builds always run `cooked`.**

   Under `cooked`, a map with no cooked file falls back to the PNG path and reports one warning
   diagnostic per load (new `L####`, "texture not cooked — run `ludion sync`"), never an error.
   The default split was confirmed by the user on 2026-10-14.
7. **Scope: mesh material maps only.** Sprites, atlases, fonts and UI textures stay PNG/RGBA8. They
   are pixel art or SDF, sampled nearest or read exactly, where block compression is a visible
   defect and the volumes are small.
8. **Binary meshes are conditional (Wave 4).** They get built only if, after
   `Plan/scene-load-speed.md` Wave 2, the `load` report shows mesh parsing still matters on the
   pinned M4 or an iOS device. The format is target-independent:
   `.ludion/cooked/mesh/<key>.bin`, a versioned little-endian serialization of the parsed
   `MeshDocument` (not `MeshData`, so `mesh_asset::decode` stays the one decode). A version or magic
   mismatch is a miss, never an error.
9. **Encoders: `intel_tex_2` for BC7, `ctt-astcenc` for ASTC 4×4** (Wave 0 result). Presets
   `opaque_basic` and `Medium`; the cooker version constant pins both.

## `ludion sync` changes

`sync` already owns the derived import trees. It gains a **cook stage that runs after the import
stage**, over the PNG maps in every `[[import]]` output.

- **Manifest.** A project-level `[cook]` table in `ludion.toml`, every field optional:
  - `targets` — default `["desktop"]`.
  - `textures` / `meshes` — booleans, default `true` / `false` until Wave 4.

  Per `[[import]]`: an optional `max_texture_size`, the long-edge cap applied when cooking
  (downscale before encode; the source PNG is untouched). This is the content knob for kits whose
  1024² maps are wasted at the game camera's distance. It changes cooked pixels only, which
  decision 6 already makes explicit. Unknown keys are refused with the manifest's existing
  diagnostics.
- **Incremental by key.** For each map, compute the key (decision 3). An existing store file means
  no work. Encoding runs in parallel (`ludion_core::par`, `--threads` honoured; the outputs are
  byte-identical at any thread count — a test).
- **Reported.** The human output gains one summary line (`cooked N, reused M, failed K`). `--json`
  gains `cooked`, `cook_reused`, `cook_failed` (each failure with its diagnostic) and `store_bytes`.
  The fields are additive, so the editor's sync consumer keeps working unchanged. An encode failure
  fails that map only and makes the run exit 1, like a failed import entry. The imported tree is
  still swapped in, since the cook is an accelerator, not a precondition.
- **Garbage collection.** After the cook, store entries no current import output references are
  deleted. sync is the store's only writer, and GC runs under the same lock as the import.
- **Crash safety.** Every store file is written with `ludion_core::atomic::write_atomic`. A
  half-written file never exists under its key, so a killed sync leaves only misses.
- **Locking.** The cook runs inside the existing `.ludion-sync.lock` hold. A second sync waits as
  today.
- **New flags:**
  - `--target <name>` (repeatable) cooks targets beyond the manifest's — what `ludion export`
    calls.
  - `--no-cook` skips the stage, for an import-only debugging run.

  Neither changes what an import writes.
- **Docs.** The `ludion-cli` devdoc and the `sync` `--help` text, the manifest topic in `docs/`, and
  the `asset-pipeline` playbook pointer.

## Waves

### Wave 0 — encoder spike (no product code)

Pick the encoders by measurement on the plaza's 235 maps, then record the choice and the numbers in
this plan. Criteria:

- licence inside `deny.toml`'s allow list;
- builds from a clean checkout on Windows x86_64 and macOS aarch64, with no system tools;
- run-to-run byte-identical output on one machine;
- encode time;
- SSIM against the source on a textured golden scene.

Candidates: BC7 — `intel_tex_2` (MIT/Apache, prebuilt ISPC kernels; verify the aarch64-darwin
kernel). ASTC — `astcenc-rs` (ARM's encoder via `cc`; Unlicense bindings over Apache-2.0).
Alternative — Basis Universal UASTC (`basis-universal`, MIT/Apache, C++ via `cc`), transcoded to
BC7/ASTC at load. Basis means one cooked file per map instead of one per target, but puts a C++
transcoder into every game binary, iOS included, against decision 1. Choose it only if the
per-target encoders fail the criteria.

**Wave 0 result (2026-10-14, Windows x86_64, 5950X 32 threads, scratch crate outside the workspace).**
Corpus: the plaza's 235 maps (81 sRGB colour/emissive, 154 linear metallic-roughness/normal; all
opaque), each encoded with a full 2×2 box mip chain (sRGB averaged in linear light). SSIM is the
`ludion-testkit` method (11×11 binomial window, Rec. 709 luma) on level 0 after decode, plus the
worst of the R/G/B channels, decoded by `texture2ddecoder`, independent of both encoders.

| Target | Crate | Licence | Preset | Encode 1T | 32T wall | Output |
|---|---|---|---|---|---|---|
| desktop BC7 | `intel_tex_2` 0.5.0 (checked-in ISPC kernels) | MIT/Apache-2.0 | `opaque_basic` | 267 s | 19.6 s | 231 MB |
| ios ASTC 4×4 | `ctt-astcenc` 0.6.0 (astcenc via `cc`, run-time SSE2/SSE4.1/AVX2/NEON dispatch) | (MIT OR Apache-2.0 OR Zlib) AND Apache-2.0 | `Medium`, `LdrSrgb` for colour maps, `Ldr` for linear | 168 s | 9.7 s | 231 MB |

- **Pixels.** 231 MB is exactly ¼ of the RGBA8 chain. BC7: sRGB luma SSIM mean 0.9988 (min 0.9973),
  linear 0.9977 (min 0.9846). ASTC: 0.9990 (0.9970) and 0.9977 (0.9839). The worst per-channel SSIM
  is a normal map in both (BC7 0.954, ASTC 0.947); the worst five are mostly the same maps for
  both encoders. Presets on a 24-map sample (1T): BC7 `fast` takes 2.5× less time but drops linear
  min-channel SSIM from 0.989 to 0.946; `slow` gains 0.001 for 4× the time. ASTC `fast` scores
  0.982 against medium's 0.990; `thorough` 0.991 for 2× the time. Both choices sit on the knee.
- **Determinism.** Both are byte-identical across two runs and across 1, 8 and 32 threads (each map
  is encoded on one thread; parallelism is across maps). Cross-CPU byte identity is not measured
  (ISPC and astcenc pick an ISA at run time). A store is written and read on the machine that
  cooks it, so the key does not depend on it.
- **Build.** Both build from clean on Windows with only the MSVC toolchain: no ISPC, no libclang,
  no system tools. `intel_tex_2` ships `aarch64-apple-darwin` and `aarch64-apple-ios` BC7
  libraries, so the darwin kernel exists. `ctt-astcenc`'s build script has a NEON variant for
  aarch64. Neither was built on a Mac in this spike.
- **Rejected.** `astcenc-rs` 0.2.0 needs libclang (bindgen) at build time, which fails "no system
  tools". `intel_tex_2`'s own ASTC entry point is `unimplemented!()` in 0.5.0. Basis Universal was
  not needed.

### Wave 1 — runtime: read and upload cooked textures

- `ludion-assets`: KTX2 read into a compressed image type (blocks per mip, format, source key).
- `ludion-render-graph`: request `TEXTURE_COMPRESSION_BC` / `_ASTC` when the adapter offers them.
- `ludion-renderer`: upload a compressed image with its mips (no `mipgen`). The texture cache key
  includes the format.
- `ludion-engine`: the quality axis (decision 6), the store lookup in the mesh-map load path
  (`mesh_asset::load_textures` / `AssetCache`), the fallback warning, and the `load` report
  counters split `cooked` / `source`.
- Tests: cooked fixtures made by the encoder as a dev-dependency in a scratch dir. Cover a cooked
  map, a missing cooked map (fallback plus warning), a corrupt KTX2 (fallback plus warning, never a
  crash), the axis off by default in render/goldens (every existing golden byte-identical), and an
  adapter without the feature (fallback).
- **Exit:** the plaza at `textures: cooked` (cooked by hand with the spike tool) loads with texture
  decode ≈ 0 in the `load` report, mesh-map VRAM ≤ ¼ of today, and a perceptual look within the
  textured-PBR tolerance, judged on a side-by-side render.

### Wave 2 — `ludion sync` cooks the desktop target

Everything in the section above, for `desktop`.

- **Tests:** a first sync cooks N. A second sync cooks 0 and reuses N. A source edit re-cooks
  exactly that map, and GC removes its old key. A killed sync leaves no partial store file. Output
  is identical at 1/2/8 threads. A bad `[cook]` key is refused. `--no-cook` skips the stage, and
  `--json` carries the shape above.
- **Exit:** the reference game syncs its town kits and a desktop host opens the plaza at the default
  `cooked` with zero fallback warnings. Load time and VRAM are recorded in Status, and the budgets
  are re-calibrated.

### Wave 3 — `ludion export --target ios|ios-sim` ships ASTC

- export runs `sync --target ios` (it already syncs first), copies the store's `ios` entries the
  bundle's import trees reference into the bundle's `.ludion/cooked/ios/`, and runs the bundle at
  `cooked`.
- The bundle still carries the PNGs. Dropping them needs a source-less identity and the
  content-provider seam (`Plan/console-strategy.md` Gap B), so it is a separate decision. Record
  bundle sizes with and without cooked files here.
- **Exit:** the simulator runs the game from cooked ASTC with zero fallback warnings. Device memory
  and load time ride `Plan/ios-target.md` M3 (owed, on the user's word).

### Wave 4 — binary mesh files (conditional, decision 8)

- `sync` writes `.ludion/cooked/mesh/<key>.bin` when `[cook] meshes = true`. The engine's mesh load
  tries the store before the YAML parse.
- **Trigger:** the M4 or iOS `load` report after scene-load-speed Wave 2 shows mesh parsing above
  10 % of a district's load. Otherwise this wave closes as not needed.

## Traps

- **Goldens.** A cooked map is a different image. Decision 6 keeps every verification path on
  `source`. A golden that moves in Waves 1–3 is a defect, not a re-bless.
- **sRGB.** BC7/ASTC have `…Srgb` formats. A colour map cooked as linear, or decoded twice, is the
  same bug `upload_mesh_texture`'s `srgb` flag already guards. Test both roles.
- **Mip filter parity.** Cooked mips are computed on the CPU and the source path's on the GPU, so
  small differences are expected. That is why the axis exists. Do not try to make them bit-equal.
- **Block-aligned sizes.** A level 0 that is not a multiple of 4 is *resampled* up to the next multiple
  of 4 at cook time (padding would shrink the content inside UV 0..1); lower levels that are not whole
  blocks (down to 2×2, 1×1) are padded, one block minimum.
- **Store sharing.** One store serves every `[[import]]` table. GC must take the union of all
  tables' references, or one table's sync deletes another's cooked files.
- **Shared checkout.** Encoding and GPU tests are heavy. Serialize lanes with other instances.

## Not in this plan

- Compressing sprites, atlases, fonts or UI textures (decision 7).
- Source-less bundles (Wave 3 note).
- Streaming or async loading.
- A cook for non-import mesh maps (hand-authored or `mesh apply` trees). The content-addressed
  store makes that a scope change, not a redesign — take it when a project needs it.
