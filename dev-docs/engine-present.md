---
description: The window present target and the RenderExtent resource — one more sink on a finished frame
---

# The present sink and the render extent

**Scope:** `attach_surface`/`present_attached`, `render_frame`'s two sinks, and the `RenderExtent`
world resource that mirrors the host's chosen stream size. The `PresentTarget` itself — surface
configuration, the blit pipeline, the reconfigure-and-skip rules — is
`ludion devdocs ludion-render-graph`; the out-of-process viewer that stays a wire client is
`ludion devdocs ludion-window`; the frames a readback produces are `ludion devdocs frame-streaming`;
the crate at large is `ludion devdocs ludion-engine`.

## Layout

- `crates/ludion-engine/src/render_extent.rs` — the `RenderExtent` world resource mirroring the `Engine`'s `render_size` (installed by `install_render_extent` on every world build and from every writer of the field); the read side of `render_size.rs`'s host-knob chain, and the `viewport` a game passes to `list_hit`/`camera_ray`. Never hashed, never snapshotted.
- `crates/ludion-engine/src/render_size.rs` — the host-knob half: it resolves one `(width, height)` from a machine's sources for the `Engine` field `render_extent.rs` then mirrors into the world.
- `crates/ludion-engine/src/render_view.rs` — the other host knob, `render.view`: the `Engine`'s one `Option<ViewPose>` the *stream* renders through instead of the scene's `Camera`. It validates a pose (`L3032`, the override untouched on refusal) and builds its `RenderCamera`, borrowing clip planes and an unnamed FOV from the scene camera. Never hashed, snapshotted or recorded; `render.screenshot` / `render.bounds` pick their camera per call and default to the scene's. `render.debug` is the same kind of lever for the debug-draw category filter (`Engine::set_debug_draw`), which stream and screenshot both honor.
- `crates/ludion-engine/src/engine.rs` — the seam itself: the optional `Presenter` (and the surface `Gpu` the sim-side renderer shares) beside the lazy `Renderer`, `attach_surface`, `present_attached`, `attach_present_backend` (test seam), and `render_frame(frame, readback, handover, tick)`.
- `crates/ludion-renderer/src/presenter.rs` — the presenter thread, its one-slot mailbox, `WindowBackend`, and `LUDION_PRESENT_TRACE` (any value but `0`/empty prints presents/s, distinct vs repeated frames, frame-index gaps, rebased ticks and acquire+present p50/p99 every 600 presents to stderr).

## Rules

- **A window renders the same extracted scene on its own renderer, on its own thread.** `attach_surface(instance,
  surface, w, h)` — called **before the first frame**, an existing renderer is `L2007` rather than a
  device swapped mid-session — picks the adapter that can present to that surface, spawns the
  presenter over a `Gpu::share` of it, and returns the `PresentSize` handle the window thread writes
  resizes into. `render_frame(frame, readback, handover, tick)` extracts once, then feeds the stream
  ring (sim-side render), the presenter (a latest-wins `Arc<RenderScene>` packet), or both; a
  handover-only frame creates no sim renderer or ring. The presenter is host state exactly as `renderer` and `stream_ring` are — never hashed,
  snapshotted or serialized — and a host without one takes exactly the calls it always did, which is
  what keeps goldens, screenshots and the frame stream byte-identical.
- **The presenter blends per refresh when packets carry `BlendData`** (`EngineBuilder::present_interpolation`; off
  collects none and draws once per frame). `alpha = (now − tick.at) / period`, clamped, never extrapolated. A packet
  blends only toward the directly preceding frame's data: a skipped frame index, a `rebased` stamp or missing data
  snaps to the newer scene (a skipped packet lost its `PresentCuts`). `Instant::now` stays inside `run_loop`.
- **Blended re-renders reuse the frame index**, so per-render renderer state (GI inject) advances per refresh, not
  per tick. Accepted because it is render-only; the sim-side renderer, which feeds every verification path, never blends.

## Verify

```
cargo test -p ludion-engine
cargo run -p ludion-cli -- serve --scene examples/gallery/lights.yaml --window
```

The `serve --window` run is the only check that exercises the present sink end to end: it must open a
real window, print its listening line, and still answer a peer (`ludion profile --attach --frames 30`
reports a gpu section beside submit — the readback ran beside the present).
