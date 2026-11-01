---
description: The desktop game window: title, size, render extent, fullscreen, cursor, and the launch player host
---

# The desktop window

**Scope:** What the game's own OS window does on desktop, and how a shipped game opens it with `ludion::launch`. The flags and the listener are `ludion docs running`; replay extents are `ludion docs wire-render`.

## The loop

```
cargo run -- --scene scenes/main.yaml --window                 # dev window beside the listener
cargo run -- --scene scenes/main.yaml --window --fullscreen    # borderless from the first frame
LUDION_FULLSCREEN=1 cargo run -- --scene scenes/main.yaml --window
```

A player build depends on the facade without the listener and calls `launch`:

```toml
ludion = { version = "0.1", default-features = false, features = ["window"] }
```

```rust,ignore
fn main() -> std::process::ExitCode {
    ludion::launch(game(), LaunchRoots { content, save })   // main thread only
}
```

## Format

| Setting | Rule |
| --- | --- |
| Title | The builder's `.title(..)`, else `ludion.toml` `name`, else `Ludion`. `run --window` shows `<title> — <scene file>`; the `launch` window shows `<title>` alone. |
| First size | Logical 1280×720, shrunk to fit the monitor, HiDPI-correct. |
| Extent | With no named extent, the render extent follows the window: height is the window's physical height, aspect is the window's but never narrower than 16:9, capped at the render-size maximum; the blit letterboxes the rest. A 200 ms debounce settles drag-resizes; a minimized window keeps its extent. |
| Named extent | `--render-size`, `LUDION_RENDER_SIZE` or an explicit `EngineBuilder::render_size` always wins. |
| Fullscreen | Borderless. Alt+Enter or F11 on Windows, Ctrl+Cmd+F on macOS, plus the green button. The chord is consumed and never reaches `InputState`. |
| Fullscreen at start | `--fullscreen` (needs `--window`) beats `LUDION_FULLSCREEN=1\|0`, which beats the builder's `.fullscreen(true)`. A malformed value is `L0037`. Nothing persists across launches. |
| Cursor | Hides after 2 s idle and returns on motion; `.cursor(CursorMode::Visible)` opts out. |

## Rules

- `--record` pins the default extent, because mouse coordinates are render-target pixels: record committed `.lrp` files at the default extent, never from a resized window.
- Title, fullscreen and cursor never reach the world, the hash, snapshots or the wire; the extent travels as an ordinary recorded `render.size`.
- `launch` has no argv, no scene file in the title, and in a `window`-only build no listener and no tokio. It must run on the main thread. Closing the window, Ctrl-C or a `Shutdown` request (`ludion docs running`) stops it gracefully.
- With the default features `launch` also binds the loopback listener. On a box with no display it fails `L2007`.

## Gotchas

- Packaging, icons and export are not provided: `launch` opens a window over directory roots.
- The 60 Hz wall-clock tick is presented without vsync and without interpolation, so cadence is uneven on 120/144 Hz panels. Display-paced present is tracked as Clockout request 0026.

## Next

- `ludion docs running` — the flags, the listener, `Shutdown`.
- `ludion docs wire-render` — render size and quality.
- `ludion docs verify-loop` — the headless test loop.
