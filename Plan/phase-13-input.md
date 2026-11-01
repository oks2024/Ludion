# Plan: Phase 13 — Input (mouse + gamepad) (E2)

> **CLOSED — shipped 2026-07-16; see Wave 2's "Status (2026-07-16, landed): the bit-exact encoding
> holds through the replay path on the pinned machine", and CLAUDE.md §Current state, where the G2
> twin-stick verdict this wave feeds is recorded as settled.**
> Status: **authored 2026-07-15, ready to start.** This is the wave-level plan for wave **E2** of the
> first-game arc; its brief is `Plan/engine-first-game.md` §"E2 — Input wave: mouse + gamepad" plus
> the cross-cutting rules there, scheduled by a private roadmap (rule 5: author the
> numbered wave plan with current-code facts before the wave starts). The consumer is game phase
> **G2 — Genre Verdict** (a private plan, §G2 / roadmap M2): the twin-stick-vs-auto experiment,
> which is inexpressible today because `InputState` is keyboard-only (`crates/ludion-sim/src/input.rs`).
> Settled: both devices land in **one** wave — same plumbing. The thesis: mouse (position, buttons,
> wheel) and gamepad (sticks, triggers, buttons) become deterministic, frame-stamped `InputState` data
> recorded in replays exactly like keys, plus a screen→ground-plane unproject helper for aim — with
> **capture out-of-sim** (the sim only ever sees recorded values) and **no wire change**. The E2
> brief's "Settled scope" items are decided; its "Proposed design" items are confirmed or amended
> below with current-code evidence (amendments are called out inline and summarized in the handoff).

## Current-code facts that shape the work

- **`InputState` is a `World` resource, not a registered component — nothing about it touches the
  registry, schema, scenes, or the inspector.** It holds three `StableSet<Key>` (held / pressed /
  released) and exposes `is_held` / `was_pressed` / `was_released` with **level vs one-frame-edge**
  semantics (`crates/ludion-sim/src/input.rs:258`–`309`). `begin_tick` clears the two edge sets and
  keeps `held` (`input.rs:285`); `apply` inserts/removes with idempotent no-repeat edges
  (`input.rs:295`). This is the exact API mouse/gamepad edges must mirror. Because it is a resource,
  the E1 "registry-borne components come for free" machinery **does not apply** — and neither does its
  cost: no `builtin_registry` entry, no JSON Schema, no `check-scene` surface, no inspector widget, no
  world-hash index. E2 adds **no builtin component** (contrast E1's `Collider`).
- **The driver already owns the tick-boundary input contract.** `SimDriver::tick` calls
  `input.begin_tick()` then applies every scripted event addressed to this frame in script order
  (`crates/ludion-sim/src/driver.rs:293`–`303`); `inject_input(frame, event: InputEvent)` is the
  live/protocol counterpart (`driver.rs:219`), and a fresh `InputState::default()` is inserted on
  `new`/reset (`driver.rs:100,123`). New `InputEvent` variants flow through `begin_tick`/`apply`
  unchanged in shape — the only edits are inside those two methods and the new state sub-structs.
- **`InputEvent` is an externally-tagged serde enum (`#[serde(tag = "kind", rename_all =
  "snake_case")]`), and `Key` serializes as its variant name** (`input.rs:30,133`). New variants are
  purely additive: existing `key_down`/`key_up` shapes are untouched, tag order is irrelevant for a
  tagged enum, and old scripts keep parsing. `InputScript` canonicalizes to stable (frame, push-order)
  form on every construction path (`input.rs:181`,`218`), so the new events inherit deterministic
  ordering with zero work.
- **The wire never names `InputEvent`: `input.inject` carries `event: serde_json::Value`.**
  `InputInjectParams { frame, event: Value }` (`crates/ludion-protocol/src/command.rs:355`–`363`); the
  engine handler `serde_json::from_value::<InputEvent>(params.event)` and maps a parse failure to
  **`L3005`** (`crates/ludion-engine/src/commands/sim.rs:63`–`70`). Consequence: growing `InputEvent`
  **cannot move `schema_hash` and cannot change the wire surface** (the event payload is opaque
  `Value` in the protocol schema) — exactly the E1 decision-9 situation. Malformed inject events
  already have a code (`L3005`), so **E2 needs no new L-code** (last taken is `L1116`,
  `crates/ludion-core/src/codes.rs:244`; `L3005` at `codes.rs:343`).
- **The replay stores an input event as an opaque `Value`, and the container version stays put.**
  `ReplayRecord::Input { frame, event: Value }` (`crates/ludion-protocol/src/replay.rs:121`–`128`,
  decoded at `replay.rs:410`); `write_input` serializes any `Value` (`replay.rs:204`).
  `REPLAY_MAGIC = b"LRP1"` and `REPLAY_FORMAT_VERSION = 1` (`replay.rs:43,46`); a reader **rejects any
  other version** (`replay.rs:284`). Adding `InputEvent` variants changes neither the framing nor the
  version, and `key_down`/`key_up` bodies are byte-identical, so **existing `.lrp` files (e.g.
  `games/*/run.lrp`) keep decoding with no re-record** — the compatibility comes for free from the
  `Value`-bodied record and the unchanged key encoding.
- **`InputState` is NOT part of a snapshot — so growing it needs no drift/migration story.**
  `engine.snapshot` serializes only `RunState`, `DeterministicRng`, and `Speed` (per E1's audit,
  `crates/ludion-engine/src/snapshot.rs:43,56`); the snapshot module doc states restored/seeked runs
  are driven by **re-injected inputs**, not preloaded input state (`snapshot.rs:12`–`13`). Adding
  `mouse`/`gamepad` fields to `InputState` cannot bloat a snapshot or need a `Restored{report}` — the
  transient edge sets are always rebuilt at the next tick. This answers the roadmap's "anything that
  grows with new fields needs the drift story checked": checked, nothing to do.
- **`serde_json` deserializes `f32` through `f64` — so a JSON *number* does not guarantee a bit-exact
  f32 round-trip.** serde_json's `Deserializer::deserialize_f32` forwards to `deserialize_f64`, which
  parses the decimal to the nearest `f64`; serde's `f32` visitor then casts `v as f32`. Decimal → nearest
  `f64` → nearest `f32` is a **double rounding** that can differ in the last bit from decimal → nearest
  `f32`. This is precisely the roadmap's recorded E2 risk ("no lossy text encoding in LRP1's input
  script … test exact f32 bit patterns"). Analog axes/positions therefore cannot ride the natural JSON
  number encoding if replay must be bit-exact (decision 3).
- **`ludion_core::math` gives deterministic transcendentals; `glam` is re-exported for the (already
  deterministic) linear algebra.** `sin`/`cos`/`tan`/`atan2`/`sqrt` route through pure-Rust `libm`
  (`crates/ludion-core/src/math.rs:34`–`64`), and the module doc is explicit: **"Always call these
  instead of the `f32` inherent methods on the sim path"** — because the platform C `libm` differs on
  Windows vs macOS (`math.rs:5`–`11`). Plain `+ - * /` and `glam` matrix/vector ops are deterministic
  as-is (Rust does not auto-contract to FMA, `math.rs:1`–`4`). The unproject helper must therefore do
  its trig (perspective `tan`, Euler `sin`/`cos`) via `ludion_core::math` and its matrix work via
  `glam` — never `glam`'s trig-bearing constructors (below).
- **The camera-projection math exists twice, and neither copy is reusable as code sim-side.** The Rust
  side is `crates/ludion-engine/src/extract.rs`: `euler_quat` uses `Quat::from_euler`
  (`extract.rs:460`) and `build_camera` uses `Mat4::perspective_rh` / `Mat4::orthographic_rh`
  (`extract.rs:514`–`528`) — all **`glam` constructors that call `std` `f32` trig** (non-deterministic
  across OS), all `pub(crate)`, producing a GPU-oriented `RenderCamera`. The TS side is
  `editor/app/src/editor/gizmo/camera.ts` (`buildViewProj:196`, `screenToRay:259`, `worldToScreen:226`,
  `unproject:240`), a faithful reimplementation that "mirrors `extract.rs` … and `bounds.rs`". So E2's
  helper is a **fresh deterministic implementation reusing the same formulas** (math-level reuse), not a
  call into either — `extract.rs` is glam-std-trig, private, and GPU-side; `camera.ts` is TypeScript.
- **Capture lives in `ludion-window`, which is out-of-sim, winit/wgpu-based, and forwards input as
  `input.inject` over the wire — with no dependency on `ludion-sim`.** The window is a peer protocol
  client (`crates/ludion-window/src/lib.rs:1`–`13`); `window_event` maps `WindowEvent::KeyboardInput`
  and the input thread sends `Command::InputInject { event: json!({...}) }` (`lib.rs:316`,`388`–`412`).
  The key mapping (`keymap.rs`) **mirrors `ludion_sim::Key` "without a dependency on that crate"**
  (`keymap.rs:3`), building the wire event's `"key"`/`"kind"` strings by hand. Mouse/gamepad mapping
  follows the same pattern: hand-built event `Value`s, no `ludion-sim` edge. `ludion-window` already
  owns the letterbox transform (`letterbox` module, `lib.rs:60`) — the seam that maps window pixels to
  render-target pixels for mouse position.
- **No gamepad dependency exists anywhere.** `gilrs` appears in **no** `Cargo.toml` and **not** in
  `Cargo.lock` (repo-wide grep empty); it is named only in the planning docs. `winit` is present, in
  `ludion-window` only. E2 must therefore propose the crate and its placement (decision 6).
- **The facade re-exports the input types but has no `math` surface yet.** `ludion::prelude`
  re-exports `InputEvent`, `InputState`, `Key` from `ludion_sim` (`crates/ludion/src/prelude.rs:36`);
  `ludion` has `pub mod prelude` and no `math` module (`crates/ludion/src/lib.rs:74`). E2 adds a
  `ludion::math` module (the brief's "facade's math surface") for the unproject helpers.
- **The engine guide's "Reading input" is one short section** (`docs/engine-guide.md:655`–`668`),
  keyboard-only, listing the `Key` variants games use. E2 extends it with mouse/gamepad reading and the
  unproject helper — the guide is how THE-test agents consume the feature.
- **Gallery goldens are static renders with no game systems, and input is behaviorally invisible in a
  still frame** — the same tension E1 hit (`Plan/phase-12-spatial.md` decision 8: `Collider` is
  render-inert, so its "gallery" golden proves layout, and behavior is proven by tests). Input has no
  visible surface at all in a static frame, so the "cursor-aimed turret" proof is a **runnable example +
  a `WorldHash` behavioral test**, not a blessed static golden (decision 9).

## Design decisions (settled by this plan)

1. **`InputState` grows a `mouse: MouseState` and a `gamepad: GamepadState`; the existing key API is
   untouched.** The key sets and `is_held`/`was_pressed`/`was_released(Key)` stay exactly where they
   are (no breaking change — games and the guide already call them), and the two new devices get their
   own namespaced sub-structs with the *same* edge vocabulary:
   ```rust
   MouseState {
       // level (persists across ticks):
       position: [f32; 2],              // render-target pixels, origin top-left, +Y down
       // edges (StableSet, cleared each begin_tick), same rules as keys:
       held: StableSet<MouseButton>, pressed: StableSet<MouseButton>, released: StableSet<MouseButton>,
       // per-frame accumulator (cleared each begin_tick, like an edge):
       wheel: [f32; 2],
   }
   GamepadState {
       // level analog (persist), recorded post-capture; capture-side clamps to range:
       left_stick: [f32; 2], right_stick: [f32; 2],   // each axis in [-1, 1]
       left_trigger: f32, right_trigger: f32,          // in [0, 1]
       // edges, same rules as keys:
       held: StableSet<GamepadButton>, pressed: StableSet<GamepadButton>, released: StableSet<GamepadButton>,
   }
   ```
   Accessors mirror the key API: `input.mouse().position()`, `input.mouse().is_held(MouseButton::Left)`,
   `input.mouse().was_pressed(..)`, `input.mouse().wheel()`; `input.gamepad().left_stick()`,
   `input.gamepad().left_trigger()`, `input.gamepad().is_held(GamepadButton::South)`, etc.
   `MouseButton` = `Left | Right | Middle` (snake_case serde); `GamepadButton` = the SDL/standard
   face/shoulder/dpad/stick/start/select set; both `#[derive(… PartialOrd, Ord …)]` so they key a
   `StableSet` deterministically. `begin_tick` additionally clears both button edge sets and zeroes
   `mouse.wheel`; `apply` gains match arms for the new variants (decision 2). **Determinism by
   construction:** the sim only ever sees values that a script/replay/`input.inject` recorded — the
   analog values are captured out-of-sim and injected as data, so a replay reproduces them independent
   of OS/driver jitter (the brief's core claim, now resting on decision 3's exact encoding).
   *Rejected:* nesting the key sets under a `keyboard:` field for symmetry (a breaking change to a
   shipped API with no consumer demand — compat wins; a future non-breaking cleanup can add
   `input.keyboard()` as an alias if ever wanted); a `StableMap<Axis, f32>` for gamepad axes (a fixed
   named struct is more regular and avoids a map keyed by an axis enum for a closed, tiny set).

2. **`InputEvent` gains the matching externally-tagged variants; discrete transitions are down/up
   events (like keys), continuous values are set-events.** Appended to the enum
   (`#[serde(tag = "kind", rename_all = "snake_case")]`):
   ```rust
   MouseMove   { position: [f32; 2] }         // "mouse_move"
   MouseDown   { button: MouseButton }        // "mouse_down"
   MouseUp     { button: MouseButton }        // "mouse_up"
   MouseWheel  { delta: [f32; 2] }            // "mouse_wheel"  (accumulates into wheel this frame)
   GamepadAxis { axis: GamepadAxis, value: f32 }  // "gamepad_axis"  (sets one level axis)
   GamepadButtonDown { button: GamepadButton }    // "gamepad_button_down"
   GamepadButtonUp   { button: GamepadButton }    // "gamepad_button_up"
   ```
   `GamepadAxis` (the field enum) = `LeftStickX | LeftStickY | RightStickX | RightStickY | LeftTrigger
   | RightTrigger`. One generic `gamepad_axis` event (axis + value) is chosen over six per-axis events —
   regular and extensible. `apply` sets `mouse.position` / accumulates `mouse.wheel` / sets the named
   gamepad axis / inserts-or-removes button edges with the same no-repeat idempotence keys use.
   *Rejected:* a single `mouse_state`/`gamepad_state` snapshot event carrying the whole device each
   frame (fatter replays, and it discards the clean edge model the key API already proves); per-axis
   variants (needless surface).

3. **Analog f32 fields encode as their IEEE-754 bit pattern (`u32` via `f32::to_bits`/`from_bits`) in
   the serde form — bit-exact by construction, the recorded E2 risk closed.** Every f32 in the new
   events and state that crosses the wire/replay boundary — `MouseMove.position`, `MouseWheel.delta`,
   `GamepadAxis.value` — serializes through a `#[serde(with = "f32_bits")]` helper (a `u32` field),
   **not** as a JSON number. Rationale, evidenced above: serde_json parses a JSON number to `f64` and
   casts to `f32`, and that decimal→f64→f32 double-rounding is not guaranteed to reproduce the original
   f32 bits — determinism must be **by construction, not by the parser's luck** (the P0 determinism
   pillar; the roadmap's "no lossy text encoding" risk). The bit form is lossless for every finite f32
   including subnormals, with zero dependence on any float parser's rounding. The cost — analog payloads
   are not hand-readable in a replay (`{"kind":"gamepad_axis","axis":"left_stick_x","value_bits":1056964608}`)
   — is acceptable: analog input is machine-recorded, never hand-authored, and the readable parts (frame,
   `kind`, `axis`/`button` names, key events) are unchanged. Keys stay fully symbolic and hand-writable.
   *Rejected:* the natural JSON number (double-rounding through serde_json's `f64` path — evidence in
   the facts; would make bit-exactness a property of the parser, not the format). *Rejected as a close
   second:* a ryu-shortest **decimal in a JSON string** decoded with `str::parse::<f32>()` (also exact —
   Rust's `from_str` is correctly-rounded and inverts ryu-shortest — and readable; but it introduces a
   quoted-number convention and a parse dependency the `to_bits` form needs neither of, for a channel
   that does not need readability). If the user prefers readable replays over the bit form, this is the
   one alternative to reach for — it is the only other encoding that is bit-exact.

4. **Old-replay compatibility is automatic: `REPLAY_FORMAT_VERSION` stays `1`, no re-records.** The
   input event is an opaque `Value` in both the wire command and the replay record (facts above), and
   the `key_down`/`key_up` encodings are byte-identical, so every committed `.lrp` (breakout, sokoban,
   hello-paddle, `games/vigil/run.lrp`) decodes unchanged. The format version is **not** bumped —
   nothing in the container framing changes; only the set of `InputEvent` variants a body may hold
   grows, which the version does not describe. A test decodes each committed `.lrp` and asserts its
   `WorldHash` trace is unchanged post-E2 (regression guard; the events in those files are key-only).
   *Rejected:* bumping `REPLAY_FORMAT_VERSION` (there is no framing change; a bump would needlessly
   reject every existing file — the version guards framing, not the `Value` schema).

5. **Capture lives out-of-sim in `ludion-window`; headless degrades to a default `InputState`,
   cleanly.** The window's event loop maps mouse from winit (`WindowEvent::CursorMoved` → `mouse_move`
   with the position **mapped from window pixels to render-target pixels via the existing `letterbox`
   transform**; `MouseInput` → `mouse_down`/`mouse_up`; `MouseWheel` → `mouse_wheel`) and polls the
   gamepad backend (decision 6) each loop iteration, mapping its events to `gamepad_*`. All of them go
   through the **same `input.inject` path** the input thread already runs — the `WireKey` channel
   generalizes to a pre-built event `Value` (or a small mapped enum), with the button/axis name strings
   built by hand, no `ludion-sim` dependency (mirroring `keymap.rs`). **Headless** (a `ludion serve`
   with no window attached, i.e. every agent/CI run): nothing captures, nothing is injected, and the
   sim holds `InputState::default()` — mouse at `[0,0]`, no buttons, zero axes. Game systems read a
   quiescent input and simply see no action; this is the correct, already-true degrade for keyboard and
   extends to the new devices with no special case.
   *Rejected:* capturing in the sim / a sim-side OS backend (violates the determinism denylist and the
   out-of-sim capture pillar); injecting raw window pixels and unprojecting the letterbox sim-side
   (the window owns the letterbox rect; mapping there keeps `mouse.position` in the render-target space
   the unproject helper documents).

6. **Gamepad backend: `gilrs`, added to `ludion-window` only.** `gilrs` is the de-facto cross-platform
   Rust gamepad crate (XInput/DInput on Windows, IOKit on macOS, evdev on Linux), MIT/Apache-2.0,
   already in the ecosystem `ludion-window` inhabits. It lands in `crates/ludion-window/Cargo.toml` and
   nowhere else — it is an OS-facing capture dependency and **must never reach a sim-side crate** (it
   would trip the determinism clippy denylist and drag an OS input API toward the sim). It is polled in
   the winit loop (`Gilrs::next_event`), or on a short timer, translating its normalized axis floats and
   button events into `gamepad_*` `input.inject` events. On a machine with no gamepad, `gilrs`
   instantiates and simply yields nothing — the headless/default-state path (decision 5) covers it.
   *Rejected:* a raw XInput/IOKit shim (reinvents `gilrs` across two OSes); putting `gilrs` behind a
   sim-side abstraction (there is no sim-side gamepad — the sim sees injected data only).

7. **Unproject: two pure, deterministic helpers in a new `ludion::math` module.** Signatures
   (amending the brief's `camera_ray(camera_transform, projection, screen_pos, viewport)` to plain data
   — see below):
   ```rust
   pub enum Projection { Perspective { fov_y_deg: f32 }, Orthographic { height: f32 } }
   pub struct Ray { pub origin: [f32; 3], pub dir: [f32; 3] }

   /// A world-space ray through a render-target pixel, or None on a degenerate projection.
   pub fn camera_ray(
       cam_translation: [f32; 3],
       cam_rotation_deg: [f32; 3],   // Euler XYZ degrees, matching Transform.rotation
       projection: Projection,
       near: f32, far: f32,
       screen_pos: [f32; 2],         // render-target pixels, origin top-left, +Y down
       viewport: (u32, u32),
   ) -> Option<Ray>;

   /// The (x, z) point where `ray` crosses the y = 0 ground plane, or None if parallel / pointing away.
   pub fn ray_ground_plane(ray: &Ray) -> Option<[f32; 2]>;
   ```
   Home: `crates/ludion/src/math.rs`, re-exported from `ludion::math` (which also re-exports
   `ludion_core::math`) and named in `prelude`. Implementation mirrors `camera.ts`/`extract.rs`
   formulas — build the view·projection matrix, invert it, unproject the pixel at NDC depth 0 (near,
   origin) and 1 (far, direction) — but does its **trig via `ludion_core::math`** (perspective's
   cotangent from `math::tan`, the Euler quaternion from `math::sin`/`cos`) and its **matrix work via
   `glam`** (deterministic `+ - * /`; `Mat4::inverse` is fine, `Mat4::perspective_rh`/`Quat::from_euler`
   are **not** — they call `std` trig). It is thus deterministic and cross-platform bit-stable (a
   two-run bit-identity test asserts it), and matches the renderer's glam-std-trig camera to within a
   sub-pixel, documented tolerance — **sim determinism takes precedence over last-bit agreement with the
   render matrix** (the discrepancy is far below one pixel and irrelevant to gameplay aim). `ray_ground_plane`
   is a ray-plane intersection (division only, no transcendental). Reuse is **math-level (formula), not
   code-level** — evidenced by `extract.rs` being private glam-std-trig GPU code and `camera.ts` being
   TypeScript.
   *Amendment (recorded):* the brief's `camera_ray(camera_transform, projection, …)` becomes plain
   `cam_translation` + `cam_rotation_deg` + explicit `near`/`far`. Evidence: `Transform`/`Camera` are
   builtin components in `ludion-engine`; a facade math helper naming them would couple the math surface
   to the engine's component types and obscure the deterministic core, and the ray genuinely needs
   `near`/`far` to unproject at both depths. A one-line convenience that destructures the components can
   live in the facade example, not in the pinned helper signature.
   *Rejected:* using `extract.rs::build_camera` (private, glam-std-trig, non-deterministic across OS,
   GPU-oriented); a fov-and-basis direct ray without the matrix inverse (works but diverges from the
   two shipped implementations the helper is validated against — mirroring `camera.ts` keeps one
   provable contract).

8. **Additive-only: no wire change, no component, no snapshot growth, no new L-code — confirmed, flagged
   loudly.** `input.inject`'s `event` is `Value`, so the wire surface stays **26 methods** and
   `schema_hash` is unmoved (decision-9 of E1, same mechanism); `InputState` is a resource, so there is
   **no registry/schema/inspector/scene** surface to degrade; snapshots do not embed `InputState`, so
   there is **no drift/migration** work; malformed inject events already map to **`L3005`**, so **no new
   L-code** is registered. E2's only cross-crate additions are the `ludion::math` module and the
   `ludion-window` `gilrs` dependency. **If any wave discovers a wire change is needed, that is a
   flagged decision, not a drive-by — none is foreseen.** Editor input forwarding (the Tauri viewer
   sending mouse/pad) is **explicitly deferred**, trigger: designers playtesting through the editor
   (E2 brief non-goal); when it fires it ships as `input.inject` from the editor, the same command the
   window uses — no new endpoint (the parity pillar holds).

9. **The E2 exit "gallery-adjacent smoke" is a runnable cursor-aimed-turret example plus a `WorldHash`
   behavioral test — not a blessed static golden — an amendment forced by an invisible feature.** Input
   has no visible surface in a static frame and gallery scenes run no systems (fact above; E1
   decision-8 precedent). So the feature's proof is: a runnable facade **example** (`examples/`, per the
   "every public API ships a small runnable example" convention) that reads `input.mouse().position()`,
   runs `camera_ray`/`ray_ground_plane`, and points a turret at the ground-plane aim (plus a gamepad
   right-stick path, the twin-stick shape G2 needs); and a **behavioral test** driving a scripted mouse
   + pad `InputScript` through that system and asserting the turret's resolved `Transform` via a
   two-run `WorldHash` trace. If a still image is wanted, an optional gallery scene renders a turret at
   a fixed authored aim (layout, not behavior) — but the behavioral guarantee is the test, stated
   plainly in the plan, the example, and the guide so no one reads a static frame as input coverage.
   *Rejected:* forcing a blessed static golden as *the* proof (a still frame cannot show that aim
   tracks the cursor — it would be theater); skipping the example (breaks the runnable-example
   convention and leaves G2 without a copyable twin-stick pattern).

## Waves

Ordering rationale: land the data model first (Wave 1) → **prove the risky encoding before anything
depends on it** (Wave 2, the recorded f32/replay risk) → the deterministic aim helper (Wave 3) → the
out-of-sim capture (Wave 4) → facade surface + example + docs + degrade-nowhere + exit (Wave 5). Each
wave is independently landable, one commit.

### Wave 1 — `InputState`/`InputEvent` growth (mouse + gamepad state and events)
- `MouseState`/`GamepadState`, `MouseButton`/`GamepadButton`/`GamepadAxis` enums, and the new
  `InputEvent` variants per decisions 1–2, in `crates/ludion-sim/src/input.rs`; `begin_tick`/`apply`
  extended; sub-struct accessors added; the key API byte-for-byte unchanged.
- The `f32_bits` serde helper (decision 3) applied to every analog field.
- **Exit gate (mechanized):** unit tests for each new edge (`mouse_down`/`up`, `gamepad_button_*`
  press/no-repeat/release exactly like the existing key tests) and each level/accumulator
  (`mouse_move` sets position; `mouse_wheel` accumulates within a tick and clears on `begin_tick`;
  `gamepad_axis` sets and persists); `InputEvent` serde round-trips as the documented tagged JSON with
  the analog fields in `*_bits` form; `cargo clippy -D warnings` clean under `ludion-sim`'s determinism
  denylist; `node scripts/check.mjs` green.

### Wave 2 — the encoding proof (BEFORE capture/replay depend on it) — bit-exact f32 + old-replay compat
- A proptest asserting `from_wire(to_wire(x)).to_bits() == x.to_bits()` over a wide f32 sample (both
  signs, subnormals, `MIN_POSITIVE`, values near powers of two, plus a large random draw) for every
  analog field, through the full `InputEvent` → `serde_json::Value` → `ReplayWriter::write_input` →
  `ReplayReader` → `InputEvent` path (the real replay round-trip, not just the enum's serde).
- The old-replay regression: decode each committed `.lrp` (`games/breakout`, `games/sokoban`,
  `games/hello-paddle`, `games/vigil/run.lrp`) and assert its `WorldHash` trace is unchanged post-E2
  (decision 4); `REPLAY_FORMAT_VERSION` still `1`.
- A two-run `WorldHash`-trace determinism test: an `InputScript` carrying mouse-move + wheel +
  gamepad-axis events with awkward exact f32 bit patterns drives a small system that writes the axis
  into a hashed `Transform`; both runs match, and a re-serialized-then-reloaded script matches too.
- **Exit gate (mechanized):** all of the above green; the plan's status note records that the bit-exact
  encoding holds through the replay path on the pinned machine. **This is the roadmap's named E2 risk,
  scheduled as its own wave so the encoding is proven before Wave 4 records real captures.**

> **Status (2026-07-16, landed):** the bit-exact encoding holds through the replay path on the pinned
> machine. `crates/ludion-sim/tests/replay_encoding.rs` (new dev-dependency on `ludion-protocol`, the
> direction the crate-boundary rule allows) round-trips `MouseMove`/`MouseWheel`/`GamepadAxis` through
> the real `InputEvent → Value → ReplayWriter::write_input → ReplayReader → InputEvent` path for a
> boundary set (both signs, subnormals, `MIN_POSITIVE`, `MAX`/`MIN`/`EPSILON`, `NAN`/`INFINITY`, the
> 2^24 mantissa boundary) plus a 256-case `proptest` over arbitrary `u32` bit patterns — all bit-exact.
> `crates/ludion-sim/tests/determinism.rs` gained a two-run `WorldHash`-trace test (`Aim`, a
> test-local component fed by `record_aim`) driving awkward bit patterns through mouse-move/wheel/
> gamepad-axis events, plus a serialize-then-reload variant — both identical across runs.
> `crates/ludion-protocol/tests/old_replay_regression.rs` decodes every committed `.lrp`
> (`games/breakout/run.lrp`, `games/vigil/run.lrp` — `sokoban`/`hello-paddle` commit none, proving
> replay in-process instead) and asserts format version 1 and intact records. The WorldHash-trace-
> unchanged half is each game's own suite: `cargo test -p breakout|sokoban|hello-paddle|vigil` all
> green (verified in a clean worktree at HEAD, since `ludion-window`'s Wave 4 work-in-progress in the
> shared tree transiently broke the `ludion` facade's build) — each game's `tests/replay.rs` (or
> `tests/gameplay.rs` for `hello-paddle`) already asserts recorded/replayed `WorldHash` trace equality,
> and for `breakout`/`vigil` additionally that the committed `run.lrp` matches. `cargo fmt` and
> `cargo clippy -p ludion-sim -p ludion-protocol --all-targets -- -D warnings` are clean.

### Wave 3 — the unproject helpers (`camera_ray` + `ray_ground_plane`), deterministic
- `crates/ludion/src/math.rs` with `Projection`/`Ray` and the two functions per decision 7; trig via
  `ludion_core::math`, matrices via `glam`; `ludion::math` re-exports `ludion_core::math` too.
- **Exit gate (mechanized):** unit tests against **known camera poses** — the G0 tilted follow camera
  (defaults pitch 58°, distance 30, fov 50°, per the roadmap status board) and a plain top-down orthographic one:
  a center-screen ray hits the ground at the expected look-through point; symmetric screen offsets give
  symmetric ground offsets; a camera looking parallel to / away from the ground → `ray_ground_plane`
  returns `None`; a degenerate viewport/projection → `camera_ray` returns `None`. A **round-trip parity
  test**: project a known ground point to a pixel with a `worldToScreen`-equivalent (mirroring
  `bounds.rs`), then `ray_ground_plane(camera_ray(pixel))` recovers it within a documented tolerance.
  A **double-run bit-identity** test (same inputs → same output f32 bits). `node scripts/check.mjs` green.

### Wave 4 — capture in `ludion-window` (mouse + gamepad, out-of-sim)
- Extend `window_event` to map `CursorMoved`/`MouseInput`/`MouseWheel`, mapping cursor position through
  the existing `letterbox` transform into render-target pixels; add the `gilrs` poll in the loop
  (decision 6); generalize the input thread's channel from `WireKey` to a mapped event `Value`; all
  routed through the one `input.inject` path. `gilrs` added to `ludion-window/Cargo.toml` only.
- **Exit gate (mechanized):** unit tests for the pure mapping functions — a winit mouse button/scroll →
  the expected `mouse_*` event `Value`; a window-pixel-to-render-target mapping over a known letterbox
  rect; a `gilrs` axis/button sample → the expected `gamepad_*` `Value` (the mappers are pure and
  testable without a window or a physical pad, matching `keymap.rs`'s testability). An assertion that a
  headless engine (no window) leaves `InputState` at default after ticks (the clean-degrade proof).
  `node scripts/check.mjs` green (note: a live end-to-end mouse/pad-through-a-window check is the M2
  human gate, not a mechanized test — hardware-bound, like the windowed-viewer criteria).
- **Held-input release (Clockout 0025):** on focus loss both hosts release every forwarded key and
  mouse button (`KeyMapper`/`MouseButtons::release_held`); a primary-pad unplug zeroes its non-zero
  axes, releases its buttons and promotes the next connected pad (`gamepad::on_disconnect`). All are
  ordinary recorded `input.inject` events. A focus/connection signal to games stays out of scope;
  trigger: a game that needs pause-on-focus-loss or pause-on-unplug.

### Wave 5 — facade surface, example, docs, degrade-nowhere, exit
- `ludion::prelude` re-exports the new input types (`MouseState`/`GamepadState`/`MouseButton`/
  `GamepadButton`/`GamepadAxis`) and `ludion::math` (`camera_ray`/`ray_ground_plane`/`Projection`/`Ray`);
  the prelude doc list extended.
- The cursor-aimed-turret **example** + its `WorldHash` behavioral test per decision 9 (mouse-aim and
  gamepad-right-stick-aim, the twin-stick shape G2 consumes).
- Engine-guide **"Reading input"** section extended (mouse position/buttons/wheel, gamepad
  sticks/triggers/buttons with the same level-vs-edge framing, and a copyable `camera_ray` →
  `ray_ground_plane` aim recipe); the `ludion-sim` agent-docs topic updated (via the `ludion-docs`
  skill) with the mouse/gamepad model and the bit-exact-encoding contract; a short `ludion` /
  `ludion-window` note on where capture lives.
- **Exit gate (roadmap "ready"):** **wire surface still 26 methods + protocol snapshot byte-identical +
  `schema_hash` unchanged** (existing tests prove it by construction — decision 8); **all pre-E2
  goldens byte-identical** (input touches no renderer); the turret example runs and its behavioral test
  is green; docs topic + guide section shipped; `node scripts/check.mjs` green. **Handoff to G2:** the
  twin-stick vs auto experiment builds on `InputState.mouse`/`.gamepad` + the unproject helpers; the
  pre-registered tiebreak (hybrid: manual aim, auto fire — roadmap board) is expressible on this
  surface. **No human gate is owned by E2 itself** (the input model is verified by tests/hashes); the
  milestone's human gate is G2's genre verdict, which needs a person to feel both control schemes on
  real hardware.

## Explicitly out of scope (triggers recorded)

- **Input remapping / bindings config** — games hold their bindings as `const Action`s over `InputState`'s reads (`ludion docs input`). Trigger: a
  second game wanting remapping (E2 brief non-goal).
- **Touch input** — no target platform needs it in this arc. Trigger: a touch platform (E2 brief).
- **Multiple gamepads** — player 1 only; the capture layer forwards the first connected pad. Trigger: a
  couch/local-multiplayer game (E2 brief non-goal).
- **Editor input forwarding** (the Tauri viewer sending mouse/pad) — deferred; trigger: designers
  playtesting through the editor. Ships as `input.inject` from the editor when it fires (decision 8).
- **Sub-pixel/high-frequency mouse or raw-input paths, gamepad rumble/haptics, gyro** — trigger: a game
  that measurably needs them; none foreseen for G2.
- **A wire command for input beyond `input.inject`** — the one command already carries every event
  (opaque `Value`); trigger: none foreseen (decision 8).

## Risks

- **f32 axis encoding (the roadmap's named E2 risk).** Bit-exact replay of analog values fails if the
  encoding double-rounds. Contained by making the encoding a by-construction bit pattern (decision 3,
  evidenced against serde_json's `f32`-via-`f64` path) and proving it in its own wave (Wave 2) with an
  exhaustive-ish round-trip proptest through the real replay path and a `WorldHash`-trace test —
  "prove it before anything depends on it," scheduled, not hoped.
- **`gilrs` determinism / cross-OS ordering.** `gilrs` event values and ordering differ across OSes,
  drivers, and runs — but it runs **only during capture, out-of-sim**; the sim sees injected data, and
  replay never touches `gilrs`. So `gilrs` variance is a capture-UX concern, never a determinism one.
  The mitigation is architectural (decision 5/6: the sim's only input source is recorded events) and is
  asserted by the headless-default-state test (Wave 4) and the replay determinism test (Wave 2).
- **Headless CI has no gamepad and no window.** Every mechanized gate must pass with neither. Contained
  by keeping the sim-side model (Waves 1–3) and the capture *mappers* (Wave 4) pure and testable
  without hardware — the mappers take a `gilrs`/winit event value and return an event `Value` — and by
  making the live device pass the M2 human gate, not a CI test (the same line the windowed-viewer
  criteria already draw).
- **Unproject vs the render matrix.** The deterministic (libm-trig) helper differs in the last bits
  from the renderer's glam-std-trig camera. Accepted and documented: sim determinism outranks last-bit
  agreement, the gap is sub-pixel, and the parity test asserts agreement within a stated tolerance
  (decision 7). A future move of the renderer's camera onto `ludion_core::math` would close the gap but
  is not E2's job.
- **Sequencing vs open gates.** E2 shares only the engine guide and the `ludion-sim` docs topic with the
  still-open Phase 8/9/10/11 and M0 human gates; it adds no L-code, no component, no wire method, and no
  golden churn, so it is safe to run with those gates open. Per roadmap rule 2, E2 is the one engine
  wave in flight during M1 and must be **ready before G2 starts** the genre experiment.
