# Plan: iOS target — vigil on the iPhone as the first non-PC export

> Status: **D1–D5, M1, M2 COMPLETE (2026-09-24); D4's pad sitting (a full run on a pad, no keyboard) is
> owed.** Reviewed 2026-09-23 on the Mac (§Review): decisions 5–8, 10 and 11 amended — a native UIKit
> host over the facade's embedding core with winit off iOS, a safe-rect screen with a 16:9 floor,
> touch controls drawn natively from a Rust layout — and a device-free **D5** now precedes the Mac
> tail. D5 and M1–M5 start on the user's word. Evaluated and phased in a grill session 2026-09-02.
> Companion: `Plan/console-strategy.md` — this plan is the "first packaging phase" that fires its
> Gap A and Gap B triggers, and it honours that plan's seam rules (seams, never abstractions).

## Why — and why it is not a console port

- **Purpose.** A credibility artifact: "we can export to other platforms", for a possible work
  presentation around March 2027 and, more generally, for the "Rust for a game engine?" challenge.
  The demo is the game on the phone. An agent-driving-the-phone stage demo was considered and
  declined (2026-09-02); the wire listener survives only as the agent's *verification* tool.
- **Not speculative.** `Plan/console-strategy.md` defers Gap A (the facade always boots a server)
  and Gap B (no content-root seam) to "the first game shipped outside the dev loop / the first
  packaging phase". An iOS build is that phase, so D1–D2 are the small general waves that plan
  already prescribes — done as PC work, justified by the PC shipping story.
- **Not a new backend.** wgpu's Metal path is the one the M4 port already runs deterministically
  (the private macOS queue). `aarch64-apple-ios` and `aarch64-apple-ios-sim` are rustup tier-2
  targets available under the pinned 1.94.0 toolchain. Rust's `std` is complete on iOS.

## Survey (2026-09-02) — what stands in the way, ranked

1. **No direct-present path.** The engine renders offscreen only (`crates/ludion-render-graph/src/execute.rs`
   requests no compatible surface) and reads the frame back (`readback.rs`); `ludion-window` is a
   WebSocket *client* that decodes streamed frames and blits them. Presenting from inside the process
   does not exist. On a tile GPU the streamed round trip (GPU → CPU → encode → decode → GPU) is a
   bandwidth tax, and `pollster::block_on` readbacks cannot sit on the UIKit main thread.
2. **The facade is bin-shaped.** `ludion::run` (`crates/ludion/src/lib.rs`) parses argv, installs a
   Ctrl-C handler, and `host_scene` (`crates/ludion/src/host.rs`) always binds the server on
   `127.0.0.1:9411`, even with `--window` (the window is a client of it). iOS has no argv and no
   meaningful CWD.
3. **Desktop-only crates sit on the game path unconditionally.** gilrs (no iOS backend) via
   `ludion-window`, `notify` (FSEvents is macOS-only) via `ludion-assets`; both reach `vigil` through
   the facade. Whether they even compile for the target is unverified until M1.
4. **Roots assume a writable checkout.** Project root = the scene file's parent; vigil's default scene
   is CWD-relative (`games/vigil/src/lib.rs`, `default_scene("scenes/camp.yaml")`); the profile writes
   to `save/profile.yaml` under the content root (`crates/ludion-engine/src/player_profile.rs`) — read-only
   inside a bundle. `EngineBuilder::project_root` / `profile_path` exist; no host sets them from outside.
5. **No touch input**, by design (`Plan/phase-13-input.md`: trigger "a touch platform"). Vigil's
   movement is keyboard-only 8-way (`games/vigil/src/movement.rs`, `move_input`); aim already reads the
   right stick and fire the right trigger (`common.rs`); interact / confirm / back / restart / aim-mode
   are keys.
6. **No build infrastructure.** No `.cargo/config.toml`, no iOS lane in ci.yml or TeamCity; the Tauri
   editor has no `gen/apple` — irrelevant, the editor never ports.

Encouraging: zero platform cfgs in `crates/*/src` and `games/`; device creation asks only for what the
adapter offers (`TIMESTAMP_QUERY` when present, `adapter.limits()`); `Rgba16Float` / `Rgba8UnormSrgb` /
`Depth32Float` are iOS-safe; cpal's CoreAudio backend covers iOS and a device-less sink is silent, never
fatal; `ludion_core::par` is plain `std::thread`; arm64 WorldHash identity is already proven.

## Review (2026-09-23) — what D1–D4 left open, measured on the M4

1. **The shipped shape can neither show a frame nor take a touch.** Without `wire`, `launch` runs
   `host_headless` over an intake whose one sender it holds itself (`crates/ludion/src/lib.rs`); the
   in-process present (`crates/ludion/src/present.rs`) sits under `window`, which implies `wire` and
   carries gilrs and tungstenite. The old decision 6 — `ludion_app_main` → `launch` — would have
   booted a headless game behind a black screen.
2. **winit cannot be the iOS host.** 0.30 builds its `UIWindow` with no scene; an app built with the
   iOS 27 SDK must adopt the UIScene life cycle or it does not launch (Apple TN3187; winit's fix exists
   only as a fork PR). Its view-controller settings — home-indicator auto-hide, the edge-gesture
   deferral a twin-stick's thumbs need — live in `winit::platform::ios`, reachable only through
   `cfg(target_os = "ios")`.
3. **iOS presents Fifo only.** wgpu-hal's Metal adapter offers `Immediate` only where
   `displaySyncEnabled` exists (macOS), so D3's `AutoNoVsync` resolves to Fifo and the sim thread's
   `get_current_texture` can wait in `nextDrawable` (three drawables at
   `desired_maximum_frame_latency: 2`). "The fixed step never blocks on the display" holds on the
   desktop only.
4. **The HUD at device aspects** (`vigil --render-out`, the plaza, 120 frames): at 2.16:1 the corner
   anchors land in the corners — under the notch in a full-bleed frame; at the iPad's 1.43:1 the
   top-right quest line runs into the top-left stack. The iPad does not ride free.
5. **Numbers from the M4** (release build): peak memory footprint 1.19 GB (plaza, `--profile 300`)
   against a jetsam ceiling reported near 2 GB on the 4 GB iPhone 13; boot to first frame 0.34 s
   (camp); `--replay-verify run.lrp` 7.8 s on one core — roughly 13 s on the A15 by single-core ratio,
   too close to the ~20 s launch watchdog to run before the UI is up.
6. **The game crate forces the desktop tree.** vigil takes `ludion` with default features, so every
   vigil build links winit, gilrs and tokio whatever an export asks; its bin calls `ludion::run`, which
   needs `wire`. `mobile-check.mjs` lints the facade alone.
7. **The content root is not the bundle.** vigil's crate directory holds `goldens/` (8 MB), `tests/`,
   `src/`, `generation/`; what the game reads is `scenes/` (16 MB) — the default scene's directory,
   the engine's project root.
8. **Retired: 32-bit float filtering.** The voxel bake's `Rgba32Float` / `Rg32Float` are only ever
   `textureLoad`ed (`gi_inject.rs`, `gi_bake.rs`), and with no `FLOAT32_FILTERABLE` requested wgpu
   validates filterability on every backend alike — the desktop passing is the proof.
9. **The Mac is ready:** Xcode 26.6, an iOS 26.5 simulator runtime, `devicectl`; the two rustup
   targets are not yet installed.

## Decisions (settled 2026-09-02; 5–8, 10 and 11 amended 2026-09-23)

1. **Scope: tier (c).** Vigil fully playable on a recent iPhone; an M-series iPad rides along as a
   universal app under decision 7's 16:9 floor. iOS only — Android is neither built nor shaped for. A developer-owned device under
   free personal-team signing, whose certificate expires after 7 days and needs Developer Mode on the
   phone. TestFlight and the paid program only if the build is ever handed to others. Rejected
   tiers: a gallery scene (proves nothing the M4 port did not), plaza-only (would not support the
   "we can export" sentence).
2. **Determinism tier: non-pinned.** `ludion_testkit::platform_dir()` yields L2205 and goldens skip.
   The claim is WorldHash identity of the committed vigil replay on the simulator (mechanized lane)
   and on the device (over WiFi). No `goldens/ios/`, no on-device golden runner.
3. **Seams, general — never iOS-shaped.** (a) `ludion::launch(builder, LaunchRoots { content, save })`
   — server-less, argv-free; `ludion::run` becomes the argv shell over it. (b) The wire server behind a
   facade feature: on in debug/dev builds — the iOS debug build listens on the LAN so the Mac attaches —
   compiled out of release builds. (c) `ludion-window` (winit + gilrs) and the `notify` watcher behind
   features that the mobile feature set turns off. (d) Gap B minimal: one content root + one save root
   threaded through the existing `EngineBuilder` setters; **no VFS trait** — an iOS bundle is a real
   read-only directory. **No `cfg(target_os = "ios")` enters `crates/` or `games/`.**
4. **In-process presentation.** The offscreen target stays the single source of truth — goldens,
   screenshots and the frame stream are untouched — and the host copies it into the window surface once
   per frame. `run --window` switches to it. `ludion-window` remains the attach viewer for
   `dev --window`, where the window must outlive the child process restarting: two roles, not two ways.
5. **Packaging: `ludion export --target ios|ios-sim --out <dir> [--run]`** (amended 2026-09-23).
   Emits a self-contained folder: a **committed Xcode template** with placeholders substituted (no
   xcodegen, no cargo-mobile2 — one less tool per Mac, and the agent edits the template like any file;
   an Xcode project rather than a hand-linked `.app` because a free personal team's provisioning
   profile is issued only through Xcode's automatic signing, `xcodebuild -allowProvisioningUpdates`),
   the game built as a static library by `cargo rustc --lib --crate-type staticlib --target …` (the
   manifest never names `staticlib`, so no desktop build or test of the game archives its whole tree),
   and the **default scene's directory** — the engine's project root, under which every asset path
   resolves — copied as bundle resources, plus the replay under test in a debug export; never the
   crate directory (§Review 7). `--run` drives `xcodebuild` plus `devicectl` / `simctl`
   install-and-launch so the agent puts a build on the device without a human. First consumer of
   `ludion.toml` (`name`, `default_scene`, a new `[ios] bundle_id` — stable once used: a free team
   registers at most ten app IDs a week). Without Xcode the verb refuses with an L-code, never a cfg.
   The `ludion new` template ships the same shape.
6. **Entry: a native UIKit host over the facade's embedding core** (amended 2026-09-23 — was a
   five-line `main.m` calling one `ludion_app_main` into `launch`, which hosts headless; §Review 1–2).
   The template's Objective-C owns the app and decides nothing: `UIApplicationMain` with the UIScene
   life cycle; a view controller stating landscape, home-indicator auto-hide, the edge-gesture
   deferral and the safe-area insets; a view whose `+layerClass` is `CAMetalLayer`, so wgpu presents
   into it directly. Touches, size and inset changes and the life-cycle transitions cross a small C
   ABI. The Rust side is the facade's **embedding core** — start (builder, `LaunchRoots`, a raw
   surface target, a size), inject, resize, suspend, resume, stop; `raw-window-handle` names UIKit,
   AppKit and Win32 alike, so no cfg. Only the surface is made on the main thread, as UIKit requires;
   start returns at once, and the scene load, the device and the renderer build on the sim thread, so
   the main thread never nears the launch watchdog. The game contributes one `#[unsafe(no_mangle)]
   extern "C"` start function — `game()` is the one thing the facade cannot know — and the forwarders
   are the facade's. One core, three shells: the native host, `present.rs` as its winit client (so
   every desktop `--window` run exercises the phone's path), and `launch` as the core with no surface.
   **No winit on iOS.**
   **`ludion::ffi` is the one port-shaped module without a cargo feature.** It compiles on every
   desktop build, despite the rule that ports land behind features, because its UIKit part is
   plain-data handle types (about 5 lines) and nothing links UIKit on desktop. A feature would cost a
   forwarding feature and a cfg on `ludion_game_start` in every game, break `ludion export` on
   projects without it, and change `mobile-check`'s coverage of the `start` paths.
7. **Screen** (amended 2026-09-23). Landscape only. Render 720 px high at the **safe area's aspect,
   never narrower than 16:9**, and the present fits the frame into the safe rect: the notch edges go
   black, a tablet gets bars top and bottom. The one engine change is the present geometry taking a
   host rect — host state beside `PresentSize`, the same geometry the cursor mapping reads; the UI and
   everything hashed stay as they are (`RenderExtent` is never hashed, and touch reaches the sim as pad
   events, never pixels). The floor is measured, not taste (§Review 4). Rejected: full-bleed with a
   host inset on the UI canvas (one more engine feature to reclaim the 11 % of width under the notch
   and its mirror — the upgrade if the sitting finds the bars ugly), letterboxed 16:9 on the phone
   (wastes a fifth of a small screen), native resolution (the A15 will not hold 60 fps with the full
   post stack).
8. **Touch = gamepad events.** The touch source emits only the existing gamepad `InputEvent`s, so the
   sim, the protocol schema hash and every recorded replay never learn the word touch. Left virtual
   stick moves; right virtual stick aims, and deflection past the deadzone synthesizes the right
   trigger (fire) — the mobile twin-stick convention. On-screen buttons are controller buttons
   (interact, confirm, back, restart, level-up pick, aim-mode); a left-stick flick taps the D-pad, so
   a thumb works the menus (`UiNav` reads no stick by design — the latch is the mapper's). Menus stay
   controller-driven; no UI hit-testing (a whole engine feature the game does not otherwise need).
   **Who does what** (amended 2026-09-23 — the plan named no drawer): the mapper is Rust in the
   facade, platform-neutral and unit-tested on the desktop, reading a **game-owned layout** — text in
   the content: stick zones and button rects in safe-area units, each button's label; the native host
   draws the controls as non-interactive views at the rects the mapper hands it and never hit-tests.
   Labels are the HUD's pad vocabulary (`A`, `B`, `START`, …): under touch `InputDevice` latches the
   pad and the hints name those letters, so the D4 sitting's vocabulary call binds the buttons too.
9. **Prerequisite: vigil controller-complete on PC (D4).** Left-stick movement and a button for every
   key action, a full run playable on a pad with no keyboard. A PC feature in its own right.
10. **Lifecycle** (amended 2026-09-23). On background the core's `suspend` sends `sim.pause` through
    the intake and returns once it is answered: a paused engine advances no frame index, so
    `plan_egress` presents nothing and no GPU work is submitted while suspended. On foreground the
    engine stays paused behind a native "tap to resume" view until a touch, which the mapper swallows,
    so no one is dropped into a live fight. All host-level — never hashed, no game code; the game's own
    pause is out of the host's reach (`Start` toggles). An app killed while suspended loses the run in
    progress; the profile writes at run end, so meta progress is safe. Snapshot-resume over
    `engine.snapshot`/`engine.restore` is a bounded follow-up, not v1.
11. **The agent's verify loop (the north star on a phone)** (amended 2026-09-23). The debug build's
    argv reaches Rust untouched — `std::env::args` reads the process argv on Apple platforms, and
    `simctl launch` / `devicectl device process launch` pass arguments — so the argv shell's
    short-circuit modes (`--replay-verify` first) run against the bundle roots, on the sim thread after
    the UI is up; never in `main` ahead of `UIApplicationMain` (§Review 5). The verdict is read from the
    console stream (`simctl launch --console-pty`, `devicectl … --console`), never from an exit code
    the launch tools may not carry back. Simulator lane: build, launch, the replay verdict, an attach
    over loopback — a simulator app is a Mac process, so it binds the Mac's own `9411` and contends with
    any desktop host there — and `simctl io screenshot`, the one image that shows the native host, the
    safe rect and the touch views (`render.screenshot` returns the offscreen frame, as everywhere).
    Device: the same verdict, and the debug-build WiFi listener for screenshots and `profile.capture`.
    From M1 on, `mobile-check.mjs` also checks the game's lib in the shipped shape for
    `aarch64-apple-ios` wherever that target is installed, so the target never rots.

## Phases

### Desktop phases — device-free, Windows-doable; D1–D4 folded into the vigil arc as slots opened

- **D1 — launch API, roots, server feature** (2–3 days). `ludion::launch` + `LaunchRoots`; the wire
  feature; profile under the save root; `ludion::run` unchanged in behaviour. Exit: vigil hosted
  headless with no listener; every committed replay byte-identical (host-side only — no hashed state
  moves); `check.mjs` green.
- **D2 — feature gates** (half a day). Exit: the facade builds with the mobile feature set and no
  winit / gilrs / notify / tokio in the tree; the default build unchanged.
- **D3 — in-process present** (3–4 days). The surface is created before the device and shares its
  adapter (`Gpu::request` gains an optional compatible surface); the engine thread presents (a wgpu
  surface is `Send + Sync`), the winit event loop stays on the main thread. Exit: `vigil --window`
  opens its own window with zero encode; goldens, stream and screenshot byte-identical; `dev --window`
  still on the viewer; the latency arc's frames-ahead gate unaffected.
- **D4 — vigil controller-complete** (2 days + a sitting). Exit: a full run on a pad on PC with no
  keyboard; replays untouched (verify — the input map is game code reading `InputState`).
- **D5 — the embedding core and the touch mapper** (3–4 days; on the user's word — device-free, so
  either PC). The embedding core and its C ABI (decision 6); `present.rs` re-expressed as its winit
  client and `launch` as the core with no surface; the argv shell taking its roots as data, so a
  bundle runs the short-circuit modes (decision 11); `suspend` / `resume` over `sim.pause` /
  `sim.resume` (decision 10); the present geometry fitting the frame into a host rect (decision 7);
  the touch mapper, the layout format and vigil's committed layout (decision 8). vigil takes `ludion`
  without default features and forwards `window` / `wire` (default on), its bin gets
  `required-features = ["wire"]`, and `mobile-check` gains vigil's lib in the shipped shape
  (§Review 6). Exit: `vigil --window` runs through the core with zero wire / schema / hash / replay
  movement; the mapper's unit tests (both sticks, the deadzone, trigger synthesis, the flick latch,
  multi-touch, a finger sliding off a button); an in-process smoke drives the C ABI with no surface —
  start, a touch, suspend, resume, stop — and lands on the hash the same pad events reach through
  `input.inject`; `check.mjs` green.

### Mac phases — on the user's word, after D5

- **M1 — cross-compile** (1 day). `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`;
  vigil's lib for both targets, in the shipped shape and with `wire`; fix what fails — cpal's
  CoreAudio path and wgpu's Metal path are the ones to watch, gilrs / notify / winit are out of the
  tree by construction; the `mobile-check` step.
- **M2 — export verb, template, simulator lane** (2–3 days). The verb (decision 5) and the template's
  native host (decision 6): the UIScene manifest, and the Info.plist keys for landscape, a hidden
  status bar and `NSLocalNetworkUsageDescription` (M3's listener may prompt for local-network access).
  Exit: `ludion export --target ios-sim --run` boots vigil in the simulator; the replay verdict line
  matches the committed hash; the agent attaches over loopback; `simctl io screenshot` shows the frame
  inside the safe rect with the touch views drawn.
- **M3 — device bring-up** (2–3 days). Human prerequisites, once — they become owed when M3
  starts: an Apple ID signed into Xcode (the free personal team), Developer Mode on the phone, the
  developer certificate trusted in its settings, the phone paired for network deployment, auto-lock
  off while the agent drives it (`devicectl` launches only on an unlocked device). Then signing, the
  first frame on the iPhone 13, the screen policy, the WiFi listener (the LAN bind, decision 3b's open
  call), the replay verdict identical on device, one `profile.capture` for the numbers, and the three
  device-only checks: present pacing under Fifo, the A15's limits, the memory footprint (§Risks).
  The A15's limits include `engine_limits`' `max_sampled_textures_per_shader_stage = 20` that lit blended meshes
  need (`Plan/transparent-depth.md` W2b). It is verified on the M4's Metal, but the simulator exposes the Mac's GPU, so the
  phone's first lit blended frame (`glass-reflections`) is its check.
  M3 also resolves the parked opaque/screen-door mesh pipeline split (a discard-free `fs_opaque` beside
  `fs_main`, so TBDR hidden-surface removal stays on for opaque geometry; a patch against rev 959 in
  `Plan-Private/artifacts/`, folded in by the user 2026-10-10): resolve its SSIM drift on the M4, then
  an iPhone Instruments capture.
- **M4 — touch feel, lifecycle, audio session on the phone** (1–2 days). The layout tuned under real
  thumbs (sizes, the stick deadzone); a background / foreground round trip submits nothing while
  suspended and resumes on a tap; the default audio session plays, respects the silent switch and
  survives an interruption — code only where one fails.
- **M5 — the phone sitting** (human gate). The D4 pad sitting first: its vocabulary call labels the
  touch buttons. Then a full run on the phone in the **shipped shape** — no listener; what is
  presented is what is played: the tier (c) exit. If two thumbs prove too much on a 6-inch screen,
  vigil's `Auto` aim mode is the sitting's lever. Presentation readiness rides this gate: a re-export
  inside the free certificate's seven days, the mirroring path rehearsed (AirPlay or a Lightning AV
  adapter), and an on-phone screen recording of a full run as the fallback.

## Risks to meet first

- **Present pacing under Fifo** (§Review 3). The sim thread may wait on a drawable, and a 60 Hz fixed
  step beating against a 60 Hz display can stall or double a tick. M3 measures it
  (`LUDION_TICK_TRACE`); if the step stalls, the fix is a D3 revision — present off the sim thread,
  which hands over the rendered frame — never a workaround in the host.
- **The A15's limits.** The device takes `adapter.limits()` wholesale, so a silent reliance on an
  M4-sized limit (a buffer size, a 3D-texture extent, workgroup storage) validates on every Mac and
  fails first on the phone — the simulator exposes the Mac's GPU, and the M2 iPad would not catch it
  either. M3's first frame is the check.
- **Memory.** 1.19 GB peak on the M4 against a ceiling reported near 2 GB (§Review 5); 720p shrinks
  the targets, a long run on the 3× city grows the caches, and a jetsam kill leaves no diagnostic. M3
  measures it on the phone.
- **The launch watchdog** is met by construction: the core's start returns at once, and the loading
  and the replay check run on the sim thread (decisions 6, 11).
- **GPU timestamps** are unavailable on Apple / tile GPUs; the profiler degrades to its null shape
  (`Plan/engine-profiler.md`). Device frame numbers come from wall time and `profile.capture`'s sim side.
- **Metal adapter enumeration resolves the main bundle** (the readdir-stall plan in the private `Plan-Private/` folder);
  keep `target/` and the checkout out of the exported folder.
- **The A15 under the full post stack** (shadows, GI, clustered forward, the post chain): 720p at
  device aspect is the bet; `--quality` presets are the fallback lever.
- **Thread count.** `par` auto = min(available, 8) → 6 on the iPhone 13; hash invariance holds by
  construction, performance may prefer the two P-cores — measure, never presume.

## What NOT to do

No Android; no `cfg(target_os = "ios")` in Rust; no winit on iOS; no decision in the Objective-C host
(it forwards touches, sizes and transitions, and draws what the mapper hands it); no VFS trait; no
tappable UI; no touch `InputEvent` variants; no pinned iOS goldens; no third-party mobile build tool;
no TestFlight until a build is handed out; no snapshot-resume in v1; no agent-on-stage demo.

## Start trigger

D1–D4: any vigil slot — each shipped as a PC improvement with its own justification. D5 and M1–M5:
the user opens the iOS window — D5 needs no device and runs on either PC, M1–M5 need the Mac; horizon,
the presentation around March 2027.

## Wave log

- **D1 — launch API, roots, wire feature (2026-09-04, COMPLETE).** Engine: `EngineBuilder::save_root`
  + `Engine::save_root()` — the directory the profile loads from and saves to, default the project
  root, surviving `load_scene_file`; a new setter rather than an absolute `profile_path`, so the file
  name stays the game's and the root the host's (the builder assigns the field after construction —
  `Engine::from_parts` sits at clippy's seven-argument limit). Facade: `ludion::launch(builder,
  LaunchRoots { content, save })` beside `run`; the `wire` cargo feature (default on) gates
  `ludion-server` and the whole argv shell (`run`, `host_scene`, `HostOptions`, the four short-circuit
  modes); one `prepare_game_engine` shared by both shells; `host_headless` runs `run_live` on the sim
  thread over a caller-owned intake and signals-then-joins so an armed recording flushes; with `wire`
  on, `launch` hosts through `host_scene` at loopback `9411` — no address knob, the LAN bind is M3's
  call with the device in hand. Tests: the facade unit test drives a hashed `sim.step` through the
  intake and replays the flushed recording to the live hash (the first version slept 200 ms and
  compared sizes — this laptop's scheduler gave it 0–2 ticks, so it was flaky by construction);
  `crates/ludion/tests/save_root.rs`; the engine builder tests; `games/vigil/tests/launch.rs` (the
  real game with content = its crate dir and a temp save root). Docs: `dev-docs/ludion.md`,
  `dev-docs/crate-graph.md`, `docs/project-anatomy.md`, `docs/player-profiles.md`. Exit: the
  no-listener build is `cargo check -p ludion --no-default-features` (verified by hand; the CI step
  lands with D2's feature set); nothing hashed moved, the vigil and breakout committed replays stand;
  verified through crate-scoped lanes only — particles W1 was mid-flight in the same checkout, the
  workspace `check.mjs` run rides the next quiet slot. Left to later phases: how `--replay-verify`
  reaches a bundle (the argv modes are wire-gated with `run`) is M2's call.
- **D2 — feature gates (2026-09-04, COMPLETE).** Facade: a default `window` feature (implies
  `wire`) carries `ludion-window` — winit, gilrs, and `ludion-client`'s tungstenite with it — and
  `clap` moved under `wire` beside `ludion-server`. `HostOptions.window` stays a plain bool in every
  shape; a windowless build refuses `--window` before binding with `L0018` — a runtime refusal
  rather than a cfg'd-out field, so the struct stays additive across features and the flag needs no
  per-feature clap attributes. Watcher: `notify` (+ `crossbeam-channel`) sit behind `ludion-assets`'
  **opt-in** `watch` feature, enabled only by `ludion-server` (`watch_assets`) and `ludion-cli`
  (`dev`); opt-in rather than default-on because nine crates and every game depend on `ludion-assets`
  and cargo's unification would re-enable a default through any one of them. Gate:
  `scripts/mobile-check.mjs` — clippy `-D warnings` on `ludion` + `ludion-assets` with every feature
  off (the shipped shape), then on `ludion` with `wire` alone (the debug-build shape the agent
  attaches to), then a `cargo tree` assertion that the feature-less facade names none of
  winit/gilrs/notify/tokio/`ludion-window`/`ludion-server`/`ludion-client` — wired as the `mobile`
  step of `check.mjs` (default and `--rust-only`), `ci.yml`, and `ci/teamcity/provision.mjs`; the
  workspace clippy runs `--all-features`, so this is the only gate that compiles a feature-off
  branch. No `cfg(target_os)` anywhere; the default build links exactly what it did (the tree
  assertion was dry-run on all three shapes: feature-less keeps only `ctrlc` + `cpal` of the host
  crates, `wire` alone re-admits clap/server/notify/tokio and nothing windowed). Docs:
  `dev-docs/{ludion,ludion-assets,ludion-window,ludion-server,ludion-cli,crate-graph}.md`,
  `docs/project-anatomy.md` (the feature names a game author needs for a device build). Exit: the
  mobile gate green on the laptop (both shapes + the tree); the workspace `check.mjs` run owed since
  D1 done in the same slot — every gate green except `test`, where nextest fail-fasted on this
  laptop's gallery provenance reds after 8 tests, so the suite was re-run `--no-fail-fast` to
  completion (2726/2736: the seven known laptop reds — five `*_at_exactly_one` goldens, renderer
  halation, `import exit_gate`, all same-machine-vs-3090-golden — plus three load-induced ones,
  two `profile_attach` `L3009` reply timeouts and the `ludion dev` hot-reload test's 180 s
  first-listening deadline under a saturated box, all green serially once quiet at 3.6 s and 49 s —
  the last one exercising the gated watcher end to end), doctests and the four CLI smokes green by
  hand, `ludion explain L0018` prints. Left to later phases: `cargo test -p ludion
  --no-default-features` cannot build the examples or the argv-shell tests (they need `wire`) — the
  gate lints the lib, which is what a device links; `required-features` on nine examples was judged
  noise until a second consumer.
- **D3 — in-process present (2026-09-05, COMPLETE).** Render graph: `Gpu::request_for_surface(instance,
  surface)` shares one private `request_from` body with `request()` (same adapter order, same device
  request; the `Adapter` is now kept on `Gpu` for surface capabilities); `present.rs` — `PresentTarget`
  (a `Surface<'static>` plus a fullscreen-triangle blit pipeline built for the sRGB view of the
  surface's format, `AutoNoVsync` so the sim thread never blocks on the display, `Lost`/`Outdated`
  reconfigure-and-skip, `Timeout`/`Occluded` skip, `Validation` → `L2007`) and `PresentSize` (an atomic
  size handle the window thread writes on resize, plus the last-presented source size so the cursor
  maps through the very geometry the blit drew); `FrameSinks { readback, present }` + `Gpu::render_frame`
  record the passes exactly once and fan out — `render_streamed` is now the `readback: true, present:
  None` wrapper, so every golden, screenshot and stream path submits byte-for-byte what it did;
  `TEXTURE_BINDING` is granted at the physical-allocation site for flat colour targets (the dump
  serializes *declared* usages — the committed snapshot is unmoved). `ludion_core::letterbox` moved out
  of `ludion-window` so the blit and the cursor mapping share one geometry. Renderer:
  `Renderer::from_gpu`, `render_frame`. Engine: `Engine::attach_surface(instance, surface, w, h) ->
  PresentSize` — request-for-surface → present target → renderer through one `adopt_renderer` (quality
  has one owner), refusing with `L2007` if a renderer already exists, so it runs before `Server::bind`;
  `present_attached`; `render_frame(frame, readback, present)` replaces `render_streamed_rgba` with the
  same extract / ring / timing / perf / GI bookkeeping for every rendered frame; `discard_stream_ring`
  keeps the ring when a window is attached (its physical cache *is* the present path's textures) and
  drops only in-flight readbacks (`StreamReadback::discard_pending`). Live loop: the frame decision is
  the pure `plan_egress` — `stream_wanted` (a subscriber or an armed profiler) gates readback exactly
  as before; a present target presents each frame index at most once (`EgressState.last_presented`)
  and never waits on a wire client's slot; a subscriber arriving after a present-only frame reads that
  frame back (today's `frame_dirty` re-render class); `a_windowless_host_plans_exactly_what_it_always_did`
  pins the pre-presentation rule over all sixteen flag combinations. Facade: `crates/ludion/src/present.rs`
  (the `window` feature now carries winit + wgpu + `ludion-window`'s public keymap/mouse/gamepad builders
  and `spawn_gamepad_thread`): winit on the main thread; the first `resumed` creates the window
  (1280×720), its surface, `attach_surface`, then `Server::bind`, then `announce` / `watch_project`
  (factored out of `host_scene` so both shells print one header); Ctrl-C and a wire `engine.shutdown`
  feed one stop that a waiter turns into an event-loop exit; keyboard/mouse/gamepad reach
  `Server::command_sink()` as ordinary `input.inject`, so a windowed recording is indistinguishable
  from any client's; teardown joins the workers, then `server.shutdown()` so an armed recording
  flushes. `host_scene`'s window branch sits before `bind` (after `--paused`). `ludion serve --window`
  follows for free (it hosts through `host_scene`); `ludion dev --window` keeps the `ludion-window`
  viewer — the window that must outlive its restarting child stays a client: two roles, not two ways.
  No `cfg(target_os)`; zero wire/schema/hash/replay movement. Docs: `dev-docs/{ludion, ludion-window,
  ludion-render-graph, ludion-engine, ludion-renderer, ludion-core, crate-graph, frame-streaming,
  ludion-sim}.md`, `docs/running.md` + `docs/project-anatomy.md` (+ mirrors), the windowed-mode
  paragraph of `engine-architecture.md`, the `L2007` explain text. Exit: `ludion serve --scene
  examples/gallery/lights.yaml --window` and `vigil --scene games/vigil/scenes/camp.yaml --window` each
  opened a real window on the laptop, printed the listening line, and served `ludion profile --attach
  --frames 30` (a full `gpu` section beside `submit` — the readback ran beside the present, the fan-out
  under test). Verified on the laptop: `check.mjs --rust-only` clippy / voxel-kit / shader-, map-,
  voxel-drift / tauri lanes green; `nextest --no-fail-fast` 2742/2751 where the nine reds are the
  seven known laptop reds (five `*_at_exactly_one` goldens, `import exit_gate`, renderer halation) plus
  the two load-sensitive CLI tests (`profile_attach`, the `dev` hot-reload deadline), both green
  serially; doctests, the five CLI smokes, `explain L2007`, `mobile-check`, and vigil + breakout
  `--replay-verify` green. Left to later: the over-budget dev-docs (`ludion-render-graph` 17.8 KB,
  `ludion-engine` 18.8 KB, `ludion-renderer` 14.2 KB) want a split; a present-time `L2007` (a
  `Validation` acquire) rides the egress log latch — the window host, not the sim thread, ends a
  session; there is no windowed golden (no headless surface exists — the smoke is the proof).
- **D4 — vigil controller-complete (2026-09-05, COMPLETE; the pad sitting owed).** Game code only:
  the input map reads `InputState` — no engine change, no new `InputEvent`, zero wire/schema/hash
  movement, every committed replay untouched (they carry key events only, and the key paths are
  bit-identical: a held key outranks the stick, never summed, and `move_player`'s diagonal factor moved
  into `move_input` as a commuted multiply). Movement: `move_input` now returns a unit-or-zero wish —
  arrows/WASD/D-pad 8-way (a diagonal pre-scaled by `INV_SQRT2`), else the left stick past
  `STICK_DEADZONE` (one tunable for both sticks now) normalised to its exact direction (`stick_wish`),
  speed fixed at `PLAYER_SPEED` past the deadzone: the balance was tuned there and the `walk` clip has
  one speed, so analog walking would slide; a resting stick is exactly no wish, which is what a gate
  hold requires. Buttons (position-named; Xbox letters in the HUD): South = interact / hold at a gate
  (`E`, through `interact_pressed`/`interact_held` in `common.rs`) and the menus' pick (already
  `UiNav`'s); East = back (resume, leave a shop); `Start` = pause / resume, and continue out of the
  summary; `Select` = restart (`R`); North = `AimMode::toggled` (`1`/`2` stay absolute); left shoulder
  = the `O` occlusion cycle; D-pad up/down = the menu cursor as `Up`/`Down` (`UiNav` reads no stick, by
  the engine's design — no repeat latch). Deliberately **not** South on the summary: the interact/pick
  button a player is mashing at the moment of death must never skip the screen, so the summary leaves
  on `Start` — the same by-phase split `run_phase_input` already made for `Enter`; `run_phase_input`
  never reads South. Fire stays the trigger — a deflected right stick aims and never fires by itself;
  decision 8's deflection-synthesises-the-trigger is the touch host's job (M4), so the game reads one
  thing. HUD hints name the device touched last — `E - KEEPER` / `HOLD E - …` / `ESC LEAVE` /
  `ESC RESUME` / `R RESTART` on a keyboard, `A - KEEPER` / `B LEAVE` / `START RESUME` / `SELECT
  RESTART` / `START RESTART` on a pad — through `InputDevice`, a world **resource** latched by
  `track_input_device` (first in the schedule: a pad button, a deflected stick or a pulled trigger
  names the pad, a game key or the mouse button the keyboard, nothing keeps the last). A resource
  and not a component because every prompt string is hashed `UiText` that the committed keyboard
  replays pin at their keyframes — the first paired-text attempt (`E/A - KEEPER`) diverged `run.lrp`
  at frame 10200 (`L3024`); the latch never leaves `Keyboard` in a keyboard replay, so those replays
  paint byte-for-byte what they recorded, untouched. Tests — the pad twins of the keyboard tests:
  `movement` (stick walk = key walk, deadzone stillness, fixed speed on any deflection, stick-up = −Z,
  D-pad = arrows, key wins over stick), `run` (Start/B/Select; the summary refusing A and B), `combat`
  (Y toggles), `quests` (A completes a quest and opens a shop, B leaves, the list says `B LEAVE`),
  `hud` (A holds a gate; a resting stick keeps the hold, a deflection drops it; the hints follow the
  device — prompt, pause overlay, and back on a key press), `animation` (the stick walks the clip and
  pins its start frame), `spectacle` (LB cycles); the `Driver` gained
  `tap_button`/`hold_button`/`release_button`/`set_left_stick`. Docs: vigil's crate docs carry the whole
  map, `games/vigil/CLAUDE.md`. Verified on the laptop: the thirteen vigil test binaries the change
  touches green (`-j 1`); every committed vigil replay untouched and green through its own instrument
  — `run.lrp` by `vigil --replay-verify` and `tests/replay.rs`, `stress.lrp` by `tests/replay.rs`, the
  three segments by `tests/segments.rs` (the bare binary resolves a segment's scene against the wrong
  content root and reports "divergences" — not a verdict); `check.mjs --rust-only` green on
  fmt / voxel-kit / clippy / the three drift gates / the three Tauri lanes, its test step red only on
  the known gallery provenance goldens, and the six gates it skips behind that step run by hand
  (mobile-check, doctests, `explain` / `docs` / `devdocs` / `check-docs` smokes) all green; `nextest
  --no-fail-fast` 2749/2767 where the 18 reds are the seven known laptop reds (five `*_at_exactly_one`
  goldens, `import exit_gate`, renderer halation), the three phase4 fps gates the laptop cannot
  measure, and eight GPU tests that rendered blank or access-violated under nextest's parallel GPU
  load (the four vigil plaza goldens, the two-process model sheet, the gallery diff report, the bloom
  sweep) — every one green run alone on the GPU, the plaza goldens at SSIM 1.0000, which also proves
  the hint latch paints byte-for-byte what the committed golden holds. Exit: the pad sitting — a
  full run on a pad with no keyboard — is **owed, not claimed**; the HUD's Xbox-letter vocabulary
  (a PlayStation ✕ reads "A") is that sitting's call, as is the occlusion key's shoulder placement.
- **D5 — the embedding core and the touch mapper (2026-09-24, COMPLETE).** Facade: `ludion::embed::Embedded`
  is the one core — `launch`, `run`, `serve`'s `host_scene` and the in-process `--window` present all
  host through it, so `Server::bind` is called only in `embed.rs`; start returns at once and loads the
  scene, attaches a caller-made wgpu surface and builds the renderer on its own thread; inject, touch,
  resize (with a safe rect), suspend / resume over `sim.pause` / `sim.resume`, stop with a flushed
  recording. `ludion::ffi` is the C ABI a native host drives (start params with the surface, size and
  safe rect; pad, touch, resize, suspend, resume, stop; the touch controls' rects and labels to draw);
  `unsafe` lives only there and in a game's one exported `ludion_game_start`. `run_live` presents with
  egress `None`. The present fits the frame into the host's safe rect (`letterbox_in`), the cursor
  mapping reads the same geometry, and `safe_render_extent` derives 720 high, never narrower than 16:9,
  below an explicit or environment extent. The argv short-circuit modes take the launch roots; argv
  handed to an embedded start runs the named mode on the engine thread, foreign argv warns and hosts.
  Touch: `ludion::touch` maps a `kind: touch-layout` document (`touch.yaml` beside the hosted scene;
  malformed is `L0021`) onto the existing gamepad events only — two virtual sticks, the right one
  synthesising the trigger past its deadzone, buttons, a left-stick flick as a same-tick D-pad tap
  (vigil's held D-pad would override the stick), per-touch ownership; after a suspend the first touch
  resumes and is swallowed. vigil takes `ludion` without default features, its bin requires `wire`,
  its lib exports `ludion_game_start`, `scenes/touch.yaml` is its untuned layout (M4 tunes it), and
  `mobile-check` lints its lib in both shapes. New codes `L0019`–`L0021`. Tests: a C-ABI smoke with no
  surface — start, pad and touch input, suspend, resume, stop — lands on the hash the same pad events
  reach through `input.inject`; the mapper's unit tests. No wire, schema-hash or replay movement. A
  macOS trap: a window stop keeps the event loop running until the engine thread ends, since the Metal
  surface is released on the main thread. The window-close click is unverified by hand.
- **M1 — cross-compile (2026-09-24, COMPLETE).** vigil's lib builds as a static library for
  `aarch64-apple-ios` and `aarch64-apple-ios-sim`, in the shipped shape and with `wire`, with no Rust
  or manifest change; the shipped iOS tree carries no winit, gilrs, notify or tokio. `mobile-check`
  lints vigil's lib for `aarch64-apple-ios` wherever the target and the iPhoneOS SDK exist
  (coreaudio-sys runs bindgen against the Xcode SDK, so an iOS build needs Xcode) and skips cleanly
  elsewhere. The link list is in `dev-docs/ludion.md`.
- **M2 — export verb, template, simulator lane (2026-09-24, COMPLETE).** `ludion export --target
  ios|ios-sim --out <dir> [--debug] [--replay <lrp>] [--run] [-- <args>]` (`docs/export.md`): reads
  `ludion.toml` (`[ios] bundle_id`, vigil's `dev.ludion.vigil`; `L0023`), probes Xcode and the rustup
  target first (`L0022`), builds the staticlib, copies the default scene's directory as `content/`,
  substitutes the committed template in `crates/ludion-cli/templates/ios/` — a hand-written Xcode
  project, the Info.plist keys, `ludion.h` (a test holds it to `ffi.rs`), and the Objective-C host:
  UIScene, a `CAMetalLayer` view, touches to `ludion_touch`, the controls drawn from
  `ludion_touch_control*`, size and inset changes to `ludion_resize`, background to `ludion_suspend`
  and a "tap to resume" view, a latched failed start, and a process exit once `ludion_ended` reports
  the engine gone. Under `wire` the C ABI's start binds loopback `9411` like `launch`. `--run` drives
  the simulator (build, pick and boot, install, `simctl launch --console-pty` with the trailing
  argv); device `--run` is refused until M3. `ludion new` ships the same shape. Exit, live on the
  iPhone 17 Pro simulator (iOS 26.5): `--replay-verify run.lrp` printed a clean keyframe check of all
  43 keyframes; `ludion profile --attach` over loopback reported; the rotated `simctl` screenshots show
  the frame inside the safe rect with both sticks and the labelled buttons, and the resume view. Owed
  to M4: vigil's untuned `touch.yaml` overlaps the HUD (SELECT over the echoes line, B over a torch),
  and touch itself is exercised only by the tests — `simctl` cannot inject touches.
