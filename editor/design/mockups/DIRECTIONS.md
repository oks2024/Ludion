# Ludion Editor — six design directions

Each direction is a complete visual identity for the same tool (see BRIEF.md for the shared
functional content and state). One signature element each; everything around it stays disciplined.

---

## 1 · Bathyscaphe — `1-bathyscaphe.html`

**Thesis.** A ludion is a Cartesian diver; the editor is the submersible's console. The viewport is
the porthole; everything around it is a calm, precise instrument in the dark, lit like sodium lamps
on brass gauges. Deep, quiet, absolutely legible.

**Palette.**
- `--abyss #0B1220` app background
- `--hull #121B2E` panels
- `--bulkhead #24334E` borders/dividers
- `--lamp #E8A33D` accent (sodium amber): selection, active states, gauge needles
- `--text #B9C5DA` body, `--bright #EAF0F9` headings/values
- `--ok #46C08D`, `--warn #E06A5A`

**Type.** Display/UI: Space Grotesk (500/600). Data & numbers: IBM Plex Mono — every number in the
app is tabular mono. Panel titles small caps-feel via letterspaced uppercase 11px.

**Layout.** Classic three-column tool frame (hierarchy+palette | viewport | inspector), but the top
strip is an **instrument rail**: left = scene identity, right = a cluster of three flat gauges
rendered as compact instrument cards — FRAME (odometer-style digits), WORLDHASH (hex in mono with a
small deterministic 4×4 identicon glyph derived from the hash), STREAM (fps/ms). Amber underline
ticks like gauge graduations under each card.

**Signature.** The determinism instrument cluster: WorldHash as a first-class cockpit gauge with
its identicon — the "same hash = same world" promise made glanceable. Everything else stays flat
and quiet; glow is reserved for the scene inside the porthole.

**Details.** 1px borders, 4px radii. Viewport framed by a slightly inset bezel (2–3px darker rim,
not a skeuomorphic pipe). Edit mode shown as an amber "PAUSED · EDIT" tag on the rail. No
diagnostic banner in this one.

---

## 2 · Wire — `2-wire.html`

**Thesis.** The parity pillar made visible: every editor action IS a protocol command, so the UI
shows the wire. Ink-on-paper light interface — a lab notebook for the protocol — with a command
dock streaming the conversation between client and engine.

**Palette.**
- `--paper #F4F6F8` background, `--card #FFFFFF` panels
- `--ink #1B2432` text/headings, `--faded #5C6678` secondary
- `--wire #2454E6` accent (electric ink blue): selection, sent commands, links
- `--reply #0E8A62` responses/ok, `--err #C2443B`
- borders `#D8DDE6`

**Type.** UI: Inter (450/600). Mono: JetBrains Mono — the wire dock, all ids, paths, hashes. The
dock is 100% mono, 12px.

**Layout.** Three columns as usual, but the bottom 160px is the **wire dock**, full width: a
two-lane feed. Each row: direction glyph (`→` sent in wire-blue, `←` reply in green), mono method
chip (`component.set`), compact JSON params (one line, truncated), frame stamp right-aligned.
Show 5 rows telling a true story: `sim.pause` → ok (entering edit), `world.query` → 24 entities,
`component.set {entity: 12·3, transform.translation…}` → ok @1284, `sim.hash` → `9f3a77c1e04b52d8`,
and a pending `scene.save` row. A thin "Ctrl+K — type a command" affordance sits in the dock
header: the command palette is a first-class way to drive the tool.

**Signature.** The wire dock. It's what makes this editor honest — a human watching it learns the
same API the agent uses.

**Details.** Cards with 6px radii and very soft shadow (barely-there, not material-design). Blue
left-edge rail on the selected hierarchy row. Session strip minimal since run control also appears
as commands in the dock. No diagnostic banner.

---

## 3 · Graphite — `3-graphite.html`

**Thesis.** The refined studio dark — chrome so quiet it disappears next to the viewport — with one
idea on top: the interface physically changes temperature with mode. Play mode is cool; Edit mode
is warm. You always know which world you're in without reading a label.

**Palette.**
- `--bg #17181B`, `--panel #1E2024`, `--raise #26292E` (inputs/hover), borders `#2E3138`
- `--text #C7CAD1`, `--bright #EFF0F4`
- Edit accent `--warm #F0B44C` (amber) — THIS mockup is in edit mode, so warm is live everywhere:
  2px viewport frame, active toggle, selection, gizmo toolbar, dirty dot
- Play accent `--cool #4CB8E8` — appears only on the inactive "Play" side of the mode switch and in
  a small legend, hinting at the flip
- `--err #E5655C`

**Type.** UI: Inter Tight at 12–13px, 500 weight headings — dense and neutral. Mono: Fira Code for
data. Type does not perform here; spacing and alignment do.

**Layout.** Tightest of the six: 12px paddings, 24px row heights, panels separated by borders only
(no gaps, no shadows). Top strip splits into three zones — identity left, a prominent segmented
**Play / Edit switch** dead center (the room's thermostat), run/save controls right. Inspector rows
align labels/values on a strict 2-column grid.

**Signature.** Mode-tinted chrome: the amber 2px frame around the viewport plus every accent in the
app agreeing on the current mode's temperature. Include a subtle "sim paused" wash — the viewport
overlay chip reads `paused @ 1284` in amber.

**Details.** 3px radii. Scrollbars styled thin. Shows the L1101 diagnostic banner (amber-bordered,
non-modal, above the viewport) to prove errors fit the system quietly.

---

## 4 · Drafting table — `4-drafting.html`

**Thesis.** The editor as a technical drawing in progress. Light, cool, exact — CAD heritage
(drafting film, title blocks, dimension callouts), not newspaper heritage. For designers who think
in coordinates and snap increments.

**Palette.**
- `--film #E9ECF0` background (cool gray, NOT cream), `--sheet #F7F9FB` panels
- `--line #C3CAD5` rules, `--ink #29303D` text, `--dim #6A7386` secondary
- `--draft #2B5FD9` accent (drafting blue): selection, active tool, dimension lines
- `--red #C4372B` reserved strictly for destructive/dirty (Destroy button outline, unsaved marker)
- Viewport playfield stays dark — the one dark rectangle on the sheet, like a rendered figure
  pasted onto drafting film.

**Type.** UI: Archivo (500/600); micro-labels uppercase letterspaced 10px like drawing annotations.
Mono: IBM Plex Mono for all coordinates.

**Layout.** Panels are **modules on a sheet**: 8px gaps of film visible between sheets, 2px radii,
1px `--line` borders. Each panel header is a mini title block — a two-cell row: NAME | a datum
(Scene: `24 entities` · Inspector: `brick_07` · Viewport: `1080p · 60fps`). The session strip is
the drawing's main title block, right cell carrying scene path + `rev: unsaved` in red.

**Signature.** Dimension-line callouts: the snap controls render as real dimension annotations —
`|← 0.25 →|` grid snap and a 15° arc glyph — and the selected brick in the viewport carries a blue
dimension line showing its translation (`-2.25, 3.5`) like a measured drawing. Gizmo toolbar styled
as a compass/tool tray.

**Details.** No shadows anywhere. Checkbox/inputs square. Undo/redo as ← → glyph buttons in the
title block. No diagnostic banner.

---

## 5 · Canon — `5-canon.html`

**Thesis.** "Text is the single source of truth" taken literally: the canonical YAML of the
selected entity is a first-class pane, and every field edit is shown as the minimal diff the save
will write. Warm, literate, IDE-adjacent dark — a writing room, not a cockpit.

**Palette.**
- `--room #1C1A17` background, `--desk #252220` panels, borders `#3A362F`
- `--text #D8D3C8` body, `--bright #F2EEE4` headings
- `--copper #C77B4A` accent: selection, active mode, focused field
- Diff colors used ONLY in the canon pane gutter: `--added #7FB069`, `--removed #D9695F`
- `--err #D9695F` shared with removed (they mean the same thing: something's wrong/leaving)

**Type.** UI: Source Sans 3 (400/600). Mono: Source Code Pro — the canon pane is a real code view
with line numbers. Slightly more generous line-height than the other directions; this one reads.

**Layout.** Left column: hierarchy + palette as usual. Center: viewport (taller half) — below it,
full-center-width, the **canon pane**: `brick_07` as canonical YAML (~12 lines: entity id, name,
Transform/Sprite/Health in canonical field order), line numbers, a diff gutter where translation.x
shows old value removed / new value added (the gizmo drag that made the scene dirty). Right:
inspector. The save control lives on the canon pane's header: `scene.save` button + `+1 −1 lines`
pending-diff badge.

**Signature.** The canon pane with its live minimal-diff gutter — proof of the canonical-writer
pillar: you can always see exactly what git will see.

**Details.** 6px radii, soft borders. The inspector field that changed (translation) carries a
copper left-edge tick linking it visually to the diff below. No diagnostic banner.

---

## 6 · Modeline — `6-modeline.html`

**Thesis.** The keyboard is the primary input. Tiling-WM / vim heritage: every panel is a numbered
tile, a modeline runs the full width of the bottom, and the current mode is an inverted block you
can read from across the room. Density maxed; zero ornament; one industrial accent.

**Palette.** Monochrome battleship grays — deliberately NOT near-black:
- `--steel #26282B` background, `--plate #2E3134` panels, `--seam #3A3E43` borders
- `--text #C5C8CC`, `--bright #EDEEF0`
- `--signal #E8642C` single accent (industrial orange): mode block, selection, active gizmo tool,
  keycap hints
- `--ok #8BC34A` used only in the modeline connection dot

**Type.** JetBrains Mono for EVERYTHING — UI labels included (this is the direction's voice).
11–12px, uppercase panel tags, tabular numbers everywhere.

**Layout.** True tiling: panels butt against each other with 1px seams, zero radii, zero gaps.
Each tile's header is a tag strip: `[1] SCENE` `[2] VIEWPORT` `[3] INSPECTOR` `[4] CREATE` — the
bracketed digit is a keycap (focus jumps by Alt+digit). The session strip is gone: its content
lives in the **modeline** — a full-width bottom bar: inverted orange `EDIT` block · scene path ·
`dirty +2` · `frame 1284` · `hash 9f3a77c1` · `60fps 4.2ms` · `ws://127.0.0.1:9411 ●`. Above the
modeline, a one-line **key hint bar**: `g translate · r rotate · s scale · ctrl+z undo · ctrl+s
save · space resume · . step`.

**Signature.** The modeline + keycap tiles: the whole session strip collapsed into one dense,
always-true status line, vim-style.

**Details.** Selected hierarchy row = orange text on `--seam`, no rounded pill. Buttons look like
keycaps: 1px border, flat. Inputs borderless with underline. Shows the L1101 diagnostic as a
modeline-red segment variant? No — as a one-line strip directly above the key hints, mono, `L1101`
prefix, dismissible with `x`.
