# Plan: map layout view — a top-down walkability picture of any map

> Status: **planned, not started (2026-10-16; lessons recorded 2026-10-18; still not started).** Waits on its trigger: the reference game first runs a
> game-side layout script over several maps, and this plan is revised from what that teaches before any
> wave is built (user, 2026-10-16). Nothing here is decided beyond the shape; the open questions are
> listed for that revision.

## Why

A map's layout — where the player can walk, what blocks them, what they can see but never reach — is
the first thing to get right and the most expensive to fix once a map is dressed. The reference game's
playtests found two layout defects that only showed in a human walk: ground visible past barricades that
no street reached, and a barricade whose way round nobody found. `ludion map-check` already computes the
answer (its ASCII grid marks walkable `.`, blocked `#` and unreachable `?` cells), but as text an agent
reads and a human does not. North Star: a layout an agent can render, check and correct in a loop, and a
human can judge at a glance before any art exists.

## What exists (2026-10-16)

- `ludion map-check <map>` (`crates/ludion-cli/src/map_check.rs`, `ludion docs map-check`): generates a
  map source's entities, probes a cell grid with the kit's walker radius, flood-fills reachability from
  an anchor, lints markers (`L1028`/`L1029`/`L1031`), seals (`L1040`), and with `--traverse` times every
  gate pair. Output is text only; `--at x,z` answers one point.
- The editor shows a map only as the rendered stage; it has no top-down occupancy view.
- The reference game draws its whole-city plan with a game-side script (HTML, SVG), which is the model
  for the per-map script this plan waits on.

## Lessons from the reference game's layout script (2026-10-18)

The game-side script has now drawn 21 layouts. What it taught:

**Built game-side:**
- A PNG beside each page's HTML. Every drafting agent had been screenshotting the HTML with a headless
  browser to read its own drawing; a raster written by the script itself removed that step.
- A `drop` (void) block kind: ground that blocks like a wall but draws black, for a wall-top's edge or a
  cliff.
- The dead-end warning prints the marker to paste (`{ name, at, dead_end_ok: true }` at the stub's tip),
  and says the place must stand past the junction.
- A one-gate map is judged gate to farthest reachable point, wherever the spawn stands.
- The city plan's landmark and POI points are checked against the layout as warnings, and the seam-pairing
  verdict is printed in the summary.

**Wanted, next in the game script:** a cross-layer hatch check and named, banded routes. Only the longest
crossing was judged, so a tier route, two routes between the same gates and the end of a one-gate map
could not be banded.

**Wanted, not built (shape vocabulary):** a court with doors (one court took about 60 rectangles), a
repeat or grid shape (21 near-identical rectangles), a circle or ring, a `walk:` ribbon (a wall-top), an
"everything else is water" ground and a bridge shape. Drawing walkable ground by its complement is the
cost.

**Takeaway for this plan:** authoring ground positively (walk shapes) beats subtracting blocks, and a
raster picture an agent can read matters more than SVG polish.

## Shape (to be revised)

1. **Data out of `map-check`.** A `--json` (or `--grid-out <file>`) mode: the grid (origin, cell size,
   per-cell class: walkable, blocked, unreachable, outside), markers with kind and gate links, the
   traversal table, every lint with its JSON Pointer. The text report stays the default.
2. **A picture.** An `--svg <file>` mode, or a render of the JSON by the client; which one is an open
   question. Unreachable walkable ground is the loud colour.
3. **The editor view.** A top-down layout panel fed by the CLI subprocess with `--json`, the pipeline-verb
   route of the parity gate (no new protocol method): pick a map source, see the grid over the stage's
   outline, click a cell for the `--at` answer.

## Open questions for the revision

- Does a pre-build layout belong in the engine at all (a layout document kind, blocks and streets before
  any kit), or does the engine only picture built maps while each game keeps its own layout format?
- Cell grid or exact collider outlines? The 4 m grid misses gaps between barricade pieces that a walker
  cannot pass; a finer or collider-exact probe may be needed for the picture to be trusted.
- Which checks generalise (sealed visible ground, dead ends, gate-pair time bands) and which are a
  game's own rules?
- SVG from the CLI, or JSON only with the picture drawn by each client?

## Trigger

The reference game has drawn and iterated layouts for several maps with its own script, and its
friction list says what the engine should own. Then this plan is rewritten into waves.
