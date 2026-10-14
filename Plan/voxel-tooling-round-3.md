# Voxel tooling — round 3 (answering the house-c field feedback)

> Status: **SHIPPED** — waves A–E landed (Lore revs 483, 484, 487, 489, 490). This file is the
> arc record.
>
> Trigger: `Plan/voxel-tooling-feedback.md` — the field report from authoring `house-c` (a
> 16-voxel/unit, 2.75M-cell building) end to end in the voxel-artist editor role. Exploration
> confirmed every checkable claim in it. Its headline: the *making* side of the toolchain is
> mature; the *looking* side (value calibration, honest framing, in-context comparison) is where
> sessions burn the North Star metric. Round 2's recorded `--sweep`-into-palette trigger fired
> (second independent author + the playbook hardening the workaround into standing guidance), so
> this round was built, not deferred.
>
> Companion contract: `Plan/voxel-artist-playbook.md` deletes each `[GAP n]` workaround section as
> its gap closes here. Cross-bearing: the small-local-model idea noted in that playbook — every gap
> closed lowers the capability floor that experiment would need.

## Shipped

- **Wave A (rev 483) — generator kit, encoding, docs reconciliation.** `grid.mjs` gains `wedge`
  (the building-side lathe; reproduces house-c's roof cells exactly at real span) and `repeat`
  (arithmetic march for quoin/corbel/tread rhythms); `mirrorX` returns
  `{changed, overwritten, glyphs}` and warns when the discarded half held differing non-empty
  glyphs (the speckle-before-mirror trap, JS twin of `L1134`) — and immediately caught the
  committed altar generator discarding 296 deliberate cells (owner's content fix, tracked
  in the private voxel-engine notebook). `emit({out})` writes the intermediate itself
  (`out: process.argv[2]` is the idiom — one script serves the drift gate's stdin spawn and a
  Windows author) and `from-json` warns **L1135** on UTF-8 double-encoding signatures, closing the
  PowerShell-pipe hazard that once committed a mojibake em-dash. `docs/voxel-ops` teaches the file
  loop and names `scripts/gen-houses.mjs` as the one legacy hand-emitter exception (house-a/b,
  outside the drift gate, read for the models never the method).
- **Wave B (rev 484) — the calibration loop and honest framing.** `render --set`/`--sweep` accept
  `--model`, scoped to the palette (`/palette/<glyph>/color/<0..3>` and
  `emissive|metallic|roughness`; everything else is `L4008` with the domain spelled out);
  `ludion_scene::apply_palette_overrides` is the typed applier; delivery stages a temp copy
  preserving the file name (extractor re-reads from disk; diagnostics name the original path); a
  model sweep takes exactly one view. `--true-size` sizes the sheet cell from the model's declared
  `scale:` so the subject renders at the pixels it earns under the gameplay camera
  (`PX_PER_WORLD_UNIT = 106/2.16`, the measured statue datum) — closing the silent
  character-calibrated-framing failure that argued for deleting good work. `voxel-craft`'s
  universal `200×180` instruction retired. Default sheets proven byte-identical (pinned
  synthesized-scene test + gallery goldens SSIM 1.0 on the 3090).

- **Wave C (rev 487) — the lineup render.** `--model` takes up to 8 comma-separated paths → ONE
  cell, models side by side under one rig at honest relative sizes (joint normalization over the
  union of scale-declared AABBs; the statue next to house-c reads 5.5× shorter, which is the point).
  The row runs along the camera's screen-right flattened onto the ground — not world X, which read
  backwards from the argument order under the game camera and put neighbours 24° off broadside so
  they occluded each other. Per-model captions are screen-projected with spacing-derived glyph
  size. Lineup mode: one view (default `game`), `--out` required, conflicts with `--set`/`--sweep`,
  composes with `--rig`/`--true-size` (framed by the tallest). Single-model path kept literally
  separate — byte identity unconditional, golden lane 51/51 SSIM 1.0.

- **Wave D (rev 489) — the text checks.** `check-scene --exposure --from game` ray-tests exposed
  emissive faces against the 58° gameplay camera through the model's own grid (Amanatides–Woo over
  `Occupancy`; camera constants duplicated as literals, `model_sheet` named source of truth) and
  reports facing/occluded per glyph — the loft-recess defect class as one text line. `--mirror-x`
  clusters asymmetric cells into regions (bbox + count + glyph set, largest first; `mirror_x_pairs`
  deleted per remove-the-first-way); house-c: 2 tight regions where 12,443 raw cells printed.
  `check-scene` prints meshed quads/triangles/vertices always-on via the new
  `ludion_engine::voxel_mesh_stats` — house-c's 1,075,310 filled cells collapse to **27,059
  quads**, the number `budget_exempt` decisions were missing. All three instant-ish on the
  2.75M-cell model. Calibration caveat: house-c's already-shallowed loft panel reads 640 facing /
  460 occluded — plausible at 58° pitch, but the metric has not yet been correlated against a
  render; the first author to use it should sanity-check the ratio against the image once.

- **Wave E (rev 490) — the docs split.** Both topics that crossed the 12 KB spoke budget this round
  split along their real seams, no load-bearing sentence cut: `voxel-authoring` (13.9 KB) sheds the
  whole render harness into new **`voxel-sheets`** (9.3 KB — views, game rig, lineup, palette sweep,
  model goldens) and drops to 5.5 KB (the loop + the `check-scene` text views); `voxel-ops`
  (13.6 KB) sheds the generation rung into new **`voxel-generation`** (6.9 KB — `grid.mjs`,
  `emit`/`from-json`, `L1133`/`L1135`, the drift-gate contract) and drops to 8.2 KB (the ladder +
  `new/apply/upscale/diff` + the cell budget). Every pointer at moved content re-aimed
  (`voxel-craft`, `voxel-format`, engine-guide's topic list — four topics → six — the voxel-artist
  persona, `scene-kind-documents`, the playbook's routing table); embedded mirror re-synced.

## Deferred, with recorded triggers (do not build)

- **`gen-houses.mjs` port to the sanctioned pipeline.** The doc amendment (wave A) stops the
  contradiction; the port re-blesses house-a/b bytes, needs a `shell` (rectangular tube) primitive
  in `grid.mjs`, and forces a seeded-RNG decision for the coordinate-hash weathering. **Trigger:**
  the next content change to house-a or house-b — port then; never edit the legacy emitter.
- **Capsule/angled limb primitive.** Carried from round 2 unchanged: only an axis-aligned
  `cylinder` exists. The round-3 evidence (house-c wanting `wedge`) was the *architectural*
  sibling, now built; the creature-side diagonal-limb wall still has one report. **Trigger:** a
  second model independently hitting it.
- **Colored/highlighted slice output.** Carried from round 2 unchanged; no column-counting
  friction in the house-c report. **Trigger:** a column-counting complaint landing after the ruler
  and `--exposure` are in an author's hands.
- **Lineup value reference + cobblestone stage ground.** The lineup ships without a calibrated
  value-reference figure, and the model stage's ground stays gallery gray (`--rig` already lifts a
  `ground` entity's color — point it at `model-stage-night.yaml` for the cobbled floor;
  `env-night.yaml` has no ground entity). **Trigger:** the first lineup sitting that still cannot
  answer a value question.

## Open items surfaced by this round (not this arc's to fix)

- altar generator: 296 cells discarded by its mirror (wave A's warning) — content fix by
  the model's owner.
- Untracked `statue-gemma.voxel.yaml` is corrupt (baked-in line-number prefixes) and fails the
  editor vitest lane for anyone verifying in the shared tree.
