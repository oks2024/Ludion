# Ludion Editor — mockup product spec (shared by all design directions)

Every mockup depicts the SAME product state; only the design language differs. Faithfulness to this
spec is the first acceptance criterion. This is a static, screenshot-quality mock of a desktop
(Tauri) app window — not a website, no marketing copy anywhere.

## Product state being depicted

The **Level Designer view** of the Ludion editor, attached to a live session of the game **vigil**,
editing scene `district-plaza.yaml`, in **edit mode**, scene **dirty** (unsaved changes), with the
entity **house-a-03 selected** (selection must be visibly consistent in hierarchy + viewport +
inspector).

## Window layout (fixed, curated — top to bottom)

1. **Title strip** — app identity "LUDION" + "vigil — district-plaza.yaml" + decorative window
   controls (min/max/close).
2. **View switcher** (the defining chrome of the product): tabs
   `Level Designer` (active) · `Game Designer` · `Lighter` · `Art Director`.
   - Lighter + Art Director are disabled (subtle "awaiting engine surface" affordance — tooltip
     text or muted sublabel).
   - Game Designer carries a small activity dot: its background Claude instance is working.
3. **Session strip** (may share a row with the switcher): Edit/Play mode toggle (Edit active) ·
   Save with dirty indicator (on) · Undo / Redo · connection chip
   `ws://127.0.0.1:9411 · schema a3f2c91 · connected`.
4. **Main row**: left column (~280px) · center viewport (dominant) · right column (~320px).
5. **Terminal drawer** (~280px tall, full width, collapsible affordance visible).
6. **Status bar**: `edit mode · unsaved changes · frame 0.97 ms · 60 fps · 47 entities · Y-up · grid 1m`.

## Left column — two stacked panels

**HIERARCHY** (tree, fold carets, selected row = house-a-03):
```
district-plaza
├ floor (24×24)
├ fountain
├ house-a-01
├ house-a-02
├ house-a-03        ← selected
├ house-a-04
├ house-b-01
├ house-b-02
├ wall-north (run ×12)
├ wall-east (run ×10)
├ wall-west (run ×10)
├ gate-to-market
├ player-spawn
├ quest-merchant
└ quest-shrine
```

**KIT LIBRARY** (grid of part tiles, small abstract visual thumb + name):
house-a, house-b, wall-straight, wall-corner, gate, well, fountain, market-stall, tree, lantern,
crate, floor-slab.

## Center — viewport

A stylized impression of an HD-2D plaza scene built in CSS/SVG (abstract-stylized is correct;
screenshot realism is not expected and not wanted): ground grid plane, fountain at center, houses
as volumes, the south gate. The selected house-a-03 has a selection outline + a translate gizmo
(three axis arrows — X/Y/Z; restyle colors to the direction but keep them distinguishable).

Overlays: top-left `district-plaza.yaml · edit` · top-right `0.97 ms · 60 fps` · bottom
`cam 14.2, 9.0, −6.5 · yaw 38°`.

## Right column — INSPECTOR (house-a-03)

Editable-looking fields:
- **Transform** — position `6.0 / 0.0 / −4.5` · rotation `0° / 90° / 0°` · scale `1`.
  The Y-position field carries a small hint affordance: "floor top sits at Y=0".
- **Mesh** — `voxel/house-a.yaml` · size `8×9×7`.
- **Collider** — `static AABB` · half-extents `4.0 / 4.5 / 3.5`.
- **Material** — palette `plaza-warm` · slot `3`.

## Terminal drawer — THE SIGNATURE AREA

Per-view Claude Code instances in tabs. This is the product's differentiator; give it design
attention equal to the viewport.

- Tab set (belonging to this view): tab `level-designer · 1` ACTIVE, status chip **working**;
  tab `level-designer · 2`, status **idle**; a `+` new-terminal affordance.
- Each tab header ALSO shows two live indicators: a **context meter** (`68k / 200k`, bar or gauge)
  and a **cache countdown** (`cache 4:12`).
- Active terminal transcript (monospace, product-real):
  - persona banner: `◆ persona: level-designer · ludion-mcp @ ws://127.0.0.1:9411 · cwd games/vigil`
  - user: `> extend the south alley with six more houses along the west wall — keep the kite lane clear`
  - working lines:
    `✱ reading context file … selection house-a-03 · camera over south gate`
    `→ entity.spawn house-b ×6 along wall-west … done`
    `→ component.set transform … aligned, floor rule applied`
    `✔ 6 houses placed · kite lane 3.5 m clear · scene dirty — save when ready`
  - an input line with a cursor.

## Copy rules

Sentence case, plain verbs, no filler, no lorem ipsum, no marketing. Every string in the mock
comes from this spec's world. An action keeps one name everywhere (Save ↔ dirty ↔ "save when
ready").

## Technical constraints

- ONE self-contained `.html` file, inline CSS. **No external requests of any kind** (no CDN fonts,
  no images) — system font stacks only. This is a Windows-first machine: Bahnschrift, Segoe UI
  Variable / Segoe UI, Sitka, Cambria, Cascadia Code, Consolas are available.
- Optional tiny inline JS (tab switching, drawer collapse); a fully static mock is acceptable.
- Target 1440–1920 px wide desktop; must not break at 1280. Vertical fit: design for ~900px
  height; the window should feel like an app, `overflow` handled deliberately.
- Visible keyboard focus styles; `prefers-reduced-motion` respected if anything animates.
- Watch CSS specificity — no selectors canceling each other's spacing.
