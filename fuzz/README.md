# Ludion fuzz targets

Coverage-guided fuzzing of the engine's parsers, quarantined from the workspace's stable pin
(`Plan/phase-9-hardening.md` decision 3 / Wave 3). This crate lives **outside** the workspace
(`members = ["crates/*", "games/*"]` excludes a root `fuzz/`; the empty `[workspace]` table in
`Cargo.toml` detaches it), with its own nightly toolchain in `rust-toolchain.toml`
(the channel is pinned there for reproducibility, the only copy). Fuzz targets ship nothing, produce no
goldens/hashes, and run only in a scheduled bounded CI job, so the two toolchains never meet.

## The eleven targets

Each is a thin `fuzz_targets/*.rs` wrapper over a shared body in `src/lib.rs` (`ludion_fuzz::*`),
so the coverage-guided path and the fallback driver run identical code.

| target                  | entry point fuzzed                                   | property |
|-------------------------|------------------------------------------------------|----------|
| `scene_parse`           | `ludion_scene::parse` (strict-subset YAML)           | never panics on arbitrary input |
| `protocol_envelope`     | `ludion_protocol::Request::from_json` (envelope + `Command` decode) | never panics; `Ok`/`L3xxx` only |
| `frame_header`          | `ludion_protocol::frame::FrameHeader::decode` (36-byte LFR1) | never panics; `Ok`/`L3004` only |
| `replay_reader`         | `ludion_protocol::replay::ReplayReader` (`new`/`records`/`seek`, LRP1) | never panics; probes in-the-clear `len` + zstd decompression bombs |
| `canonical_idempotence` | `ludion_scene::emit` ∘ `parse` (with a test registry of `f32`/`f64` components) | `emit(parse(emit(s))) == emit(s)`; canonical output always re-parses and loses no value (an `f32` field reads back as the same `f32`, an `f64` field bit for bit) |
| `kind_roundtrip`        | every `ludion_scene::KINDS` checker, plus each kind's `check_*`/`emit_*` pair | never panics; canonical output re-checks and re-emits byte-identical; a `KINDS` row with no fuzz arm fails on the first input |
| `png_decode`            | `ludion_assets::decode_png`                          | never panics; an `Ok` image is `width × height` RGBA8 |
| `wav_decode`            | `ludion_assets::decode_wav`                          | never panics |
| `restore_snapshot`      | `Command::EngineRestore` on a fresh `Engine` (project root nonexistent) | never panics; a restored state snapshots again |
| `gltf_import`           | `ludion_import::import` (`ExternalFiles::Refused`)   | never panics; touches no file |
| `font_bake`             | `ludion_font::bake::bake` (an SDF and a bitmap source over the bytes) | never panics; `Err` is fine |

Seed corpora are under `corpus/<target>/` (real scene files, valid envelope JSON, hand-built valid
LFR1 headers, the committed `games/breakout/run.lrp`, one document per checked kind, small PNG/WAV/glTF
files, the committed Silkscreen face and its truncations, two real engine snapshots). Minimized crash reproducers, when found, are
committed under `findings/<target>/`.

## Running (coverage-guided — libFuzzer)

```
rustup toolchain install <channel from fuzz/rust-toolchain.toml> --component rust-src
cargo install cargo-fuzz
# rustup selects the pinned nightly by itself from inside fuzz/:
cd fuzz && cargo fuzz run scene_parse -- -max_total_time=300
```

The scheduled CI job (`ci/scripts/fuzz.{ps1,sh}`, run nightly by the private TeamCity configs) runs every target `cargo fuzz list` prints with a time budget.

Every push compiles this crate with a stable `cargo check --manifest-path fuzz/Cargo.toml --all-targets --locked`, run from the repository root so the root's stable pin applies (`scripts/check.mjs` `fuzz-check`, the `fuzz targets` step in `ci.yml`); never `cd fuzz` for it.

### Windows note (why the fallback driver exists)

On `x86_64-pc-windows-msvc`, libFuzzer needs the AddressSanitizer runtime, and the runtime must
match rustc's bundled LLVM. A rustc whose LLVM is newer than the installed MSVC
`clang_rt.asan_dynamic-x86_64.dll` (for example LLVM 22.x against MSVC 14.37's clang ~16) is an ABI
mismatch that aborts at process start with `STATUS_DLL_INIT_FAILED`. `--sanitizer none` is not an
escape: on COFF the SanitizerCoverage section symbols (`__start___sancov_pcs` / `__stop___sancov_pcs`) are provided
by the sanitizer runtime, so the link fails without one. Coverage-guided fuzzing therefore runs on
the Linux/macOS CI agent (matching runtime) or a Windows agent with an LLVM-22 `clang_rt` on `PATH`.

## Running (fallback — deterministic mutation driver)

So the targets **execute** on any host regardless of the sanitizer situation, `src/bin/mutation_driver.rs`
runs every `ludion_fuzz::TARGETS` body over seeded byte-mutations of the committed corpus,
catching panics and writing reproducers to `findings/<target>/`. It has **no coverage feedback** —
it is an execution/smoke proof, not a replacement for coverage-guided fuzzing.

```
cargo build --release --bin mutation_driver
./target/release/mutation_driver 5000000   # iterations per target (default 2_000_000)
```

## When a finding appears

Minimize it (`cargo fuzz tmin <target> <artifact>`), commit the minimized input under
`findings/<target>/`, and — per the plan — land it as a plain `#[test]` regression in the *owning
crate* so the pinned stable CI runs the regression without needing nightly.
