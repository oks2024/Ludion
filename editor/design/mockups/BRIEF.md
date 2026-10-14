# Ludion Editor — mockup functional brief (shared by all six directions)

Ludion is an LLM-first 3D game engine in Rust. The editor is a Tauri + React app that is a **peer
protocol client** of a running engine: every button is a wire command the CLI or an agent could also
send. The engine is deterministic (fixed timestep, WorldHash), scenes are canonical YAML text, and
the renderer streams frames to the editor viewport (HD-2D stylized look). The users are
professionals: level designers and engine developers who live in this tool for hours. Efficiency,
density and legibility beat decoration.

Every mockup depicts the SAME tool in the SAME state (below). Layout, styling, and the one
signature element differ per direction; the functional content does not.

## Application state to depict

**Session** (today a strip at the top; directions may re-home it)
- Connected to `ws://127.0.0.1:9411` — engine `ludion 0.6.0`
- Scene: `games/breakout/scenes/main.scene.yaml`
- Mode toggle: **Edit** (active) / Play. Edit mode pauses the sim.
- Run controls: Resume, Step (ticks input, value `1`), speed input `1.0×`
- WorldHash readout: `9f3a77c1e04b52d8` @ frame `1284` (refresh action)
- Stream stats: `1080p · 60 fps · 4.2 ms`
- Save: path input `games/breakout/scenes/main.scene.yaml`, Save button, **unsaved changes**
  indicator (dirty)
- Undo (enabled) / Redo (disabled), Disconnect

**Hierarchy panel** — title "Scene", 24 entities:
- `camera`
- `key_light` — warning flag (dangling parent) shown subtly
- `fill_light`
- `ball`
- `paddle`
- `walls` (group) → `wall_left`, `wall_right`, `wall_top`
- `bricks` (group) → `brick_00` … `brick_17` (show ~6 rows then a collapsed “+12 more” or scroll)
- Selected row: `brick_07`
- Footer control: reparent select ("Parent: bricks")

**Palette panel** — "Create": buttons Brick, Paddle, Ball, Point light, Sprite, Empty.
"Selection": Duplicate, Destroy (both enabled — there is a selection).

**Viewport** (center, dominant) — a mock HD-2D breakout scene drawn in inline SVG/CSS (do NOT leave
it a gray box; this sells the mockup): dark playfield, 3 rows of glowing amber/teal bricks, paddle
at the bottom, ball with a subtle light bloom, two colored point-light glows washing the field.
`brick_07` has a selection outline + a translate gizmo (red X / green Y arrows, blue Z dot).
Overlay chips: `frame 1284 · 60 fps · 4.2 ms`. In Edit mode the viewport shows it's paused.

**Gizmo toolbar** (near viewport): Translate (active) / Rotate / Scale · snap: grid `0.25`, angle
`15°` · hint text "Drag an axis to move · Esc cancels".

**Inspector panel** — title "Inspector", subject `brick_07` (`id 12·gen 3`), three components:
- `Transform` — translation `[-2.25, 3.5, 0]`, rotation `[0, 0, 0, 1]`, scale `[1.5, 0.6, 1]`
  (numeric fields, per-axis)
- `Sprite` — texture `textures/brick_amber.qoi`, tint `[1.0, 0.72, 0.31, 1.0]` with an amber color
  swatch
- `Health` — hits_remaining `2`

**Diagnostics** (non-modal banner pattern; show ONLY if the direction spec says so):
`L1101 · texture not found: textures/brick_amber.qoi` — help: "searched games/breakout/textures —
check the path or run ludion check-scene" — dismiss ×.

## Real protocol vocabulary (use these exact names where commands are shown)
`sim.pause` `sim.resume` `sim.step {ticks}` `sim.set_speed {speed}` `sim.hash` `scene.save {path}`
`scene.load` `world.query` `component.set` `entity.spawn` `entity.destroy` `entity.reparent`

## Copy rules
Sentence case everywhere. Controls say what they do ("Save scene", not "Submit"). Entity names and
paths, hashes, ids, commands are always monospace. No marketing copy anywhere — this is a tool, not
a landing page.

## Technical floor (every mockup)
- One self-contained `.html` file, plain HTML + CSS (a few lines of JS allowed for e.g. a tab
  switch; nothing needs to be functional). No frameworks, no build step.
- Desktop-first, designed at 1440–1920 wide, `height: 100vh` app shell; degrade gracefully to
  1200px. Not a scrolling page — it's an app frame.
- Fonts: Google Fonts via `<link>` are allowed (with honest system fallbacks); pick per the
  direction spec.
- Visible `:focus-visible` styles on interactive elements; `prefers-reduced-motion` respected if
  anything animates; text contrast ≥ 4.5:1 for body-size text.
- Cursor `default` on chrome; `pointer` only on things styled as clickable.
