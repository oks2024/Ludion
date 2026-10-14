# Plan: Console Strategy

> Status: **evaluated, not scheduled.** No console work exists or should exist yet. This plan records why "Rust can't run on consoles" is the wrong framing, what a port of a Ludion game actually costs per platform, which seams the engine already has in the right place, and the two structural gaps that would bite. The decision to port is made per-title from this written evaluation, not under pressure.

## Trigger criterion

Start actual console-backend work only on a **signed platform/devkit agreement or a porting-partner contract for a specific title**. Not before. Two smaller items have their own earlier triggers (see "Gaps"); everything else in force today is zero-cost seam discipline.

## The premise, corrected

Rust on consoles is *unofficial*, not impossible — the distinction is the whole strategy.

- rustc is LLVM-based and every current console CPU is x86-64 (PS5, Xbox Series) or ARM64 (Switch, Switch 2). Code generation is a solved problem; rustc already carries tier-3 console targets (`aarch64-nintendo-switch-freestanding`, plus 3DS/PSP homebrew targets), and commercial studios have done private PlayStation Rust ports under NDA (rust-gamedev wg issue #90 documents studios holding devkits and NDAs for this).
- What is actually missing per platform: the NDA'd SDK integration (a custom target spec, a std port or partial-std, linker/startup glue), a graphics backend (wgpu has no public NVN or AGC backend), and certification plumbing. All of it is finite engineering that porting houses do routinely for indie titles.
- Consoles ban JIT and restrict dynamic code loading. Ludion's pillars — Rust is the only Turing-complete layer, no scripting VM, no dylib hot reload, AOT everything — are a **console advantage**, not a liability. Engines dragging a C#/Lua VM have a harder cert story than we do.

The honest hard parts are graphics backends and NDA'd toolchain glue. Neither is "Rust."

## Per-console difficulty (easiest → hardest)

1. **Xbox Series.** Windows-based OS, MSVC-family toolchain (Rust's best-supported proprietary target), D3D12 is wgpu's primary backend. The work is GDK integration + cert, not compiler or renderer work. Closest to "already runs."
2. **Switch / Switch 2.** Officially supports Vulkan, so wgpu-Vulkan is a plausible first bring-up; an NVN executor backend only if a title's performance demands it. ARM64 target precedent exists in rustc tier 3. The HD-2D content profile fits the hardware class well.
3. **PS5.** FreeBSD-based OS, Clang SDK. Needs a private target spec + std port (precedented under NDA, but bespoke) and — the largest cost — an AGC frame-graph executor plus a WGSL→PSSL shader path that does not exist publicly. This is the one platform where the shader corpus is a real rewrite item.

## What the engine already does right (the port-effort map)

A future port starts from these seams; keep them true.

- **Frame graph.** Declarations (`crates/ludion-render-graph/src/graph.rs`, `resource.rs`, `schedule.rs`, `dump.rs`) are wgpu-free; wgpu is confined to `execute.rs`/`readback.rs`/`timings.rs` plus three trivial `to_wgpu` converters in `resource.rs`. A console backend replaces the executor and those converters; the graph, scheduler, and `ludion-renderer`'s pass declarations survive. This is exactly the documented ash exit plan — a console is just another exit.
- **Audio.** `cpal` lives in exactly one crate (`ludion-audio`); the sim produces plain `AudioEvent`s and never touches a device. A console port swaps one crate.
- **Sim.** No wall clock in sim (the one contained site is `run_live` in `crates/ludion-engine/src/live.rs`, and it only decides *when* to tick), no rayon, pure-Rust libm, plain `std::thread` + crossbeam. Ports as-is. A console is a new pinned (target, toolchain) determinism tier: WorldHash sequences and goldens get re-blessed per platform, which the tiered-determinism honesty already accommodates.
- **Dev-only hazards vanish on console.** The `notify` watcher, process spawning (`ludion-cli/src/dev.rs`), winit (`ludion-window`), and the Tauri editor are all off the shipped-game runtime path. The editor never ports — it is an out-of-process peer client; on console the wire protocol is dev-network-only or absent.

## Gaps (the two things that would actually bite)

- **Gap A — the facade bakes a server into shipped games.** `ludion::run` (`crates/ludion/src/lib.rs`) always boots `ludion-server`: a 2-worker tokio runtime and a TCP WebSocket listener on `127.0.0.1:9411`, in every game built on the facade. Console TRCs restrict or forbid ambient listeners; it is also wrong for a shipped PC game. The engine↔server bridge is already crossbeam channels, so the fix is a server-less run path in the facade (drive `run_live` directly; feature-gate `ludion-server` out of release game builds). **Trigger: the first game shipped to real players outside the dev loop — even PC-only.**
- **Gap B — no single content-VFS seam.** `ludion-engine` does direct `std::fs` in `asset_cache.rs`, `render.rs`, `extract.rs`, `commands/scene.rs`, and `profile.rs` rather than funneling through `ludion-assets`. Consoles mount content through proprietary APIs (title storage, read-only content mounts); today there is no one seam to reimplement. The fix is a content-provider seam in `ludion-assets` that the engine's file reads flow through. **Trigger: the first packaging/distribution phase — a PC installer needs the same content-mount answer a console does.**

Both are recorded here, not implemented now — same rule as the ash seam: exit plan, not abstraction.

## The strategy: ship on PC, port per-title, keep the seams honest

- **Console ports are per-title business events, not engine milestones.** The engine's only job today is to not foreclose the port. No console code, no `#[cfg(console)]`, no speculative backends.
- **First console title goes porting-partner-first.** Partners hold the devkits, NDAs, and cert experience; this is the precedented route for indie Rust titles and converts the problem into money + a contract instead of an engine detour. The self-serve alternative, if we'd rather own it: Xbox first, where the toolchain and wgpu backend already substantially exist.
- **Platform order when it happens: Xbox → Switch 2 → PS5**, per the difficulty ranking above. Each platform lands as a new frame-graph executor + audio-sink crate + pinned determinism target, behind the same seams.

## Do early (cheap now, expensive later)

- Keep the seam discipline that is already convention: wgpu types stay out of frame-graph declarations, `cpal` stays in one crate, no new platform-coupled dependency enters the game runtime path (`ludion` facade and below) without noting it here.
- When Gap A/B triggers fire, do them as small waves — both also improve the PC shipping story, so neither is console-speculative work.

## What NOT to do

- No preemptive graphics-backend traits — the architecture doc's rule stands: the frame graph's declarative interface *is* the seam; backends are replaced, not abstracted over.
- No C++ rewrite hedge, no second engine for consoles.
- No speculative `no_std` effort — real console ports run a (ported) std, and the homebrew freestanding targets are not the shipping route.
- No devkit acquisition before a title justifies it.
