# Plan: Font assets — TTF/OTF faces baked into a glyph atlas

> Status: **design settled 2026-10-01 (grilled, four rounds). W1 and W2 mechanized:** one
> `ludion_font::layout()` serves every text path (Text, UiText, UiList, bounds), the 8×8 face is an
> embedded `kind: font` doc + PNG, `GLYPHS` is deleted, goldens byte-identical. W2 (the bake):
> `kind: font-source` (`L1251`), the `bake` feature (outlines, kerning, own SDF/coverage rasterizers,
> `L1252`), `generate-atlas` over font sources, `check-scene` summaries, `render --font`, the
> drift gate, Inter + Silkscreen baked under `examples/gallery/fonts/`. **W3 mechanized:**
> `line_height` override (Silkscreen crisp at sizes 10, 20, 30…); the `font` field on
> `Text`/`UiText`/`UiList`, the `kind: fonts` table + `FontLibrary` (`L1253`–`L1256`), face-first
> measurement helpers, SDF + bitmap drawing in both text shaders, font reload (refused with `L1255`
> while recording), the `ui-fonts` gallery scene (macOS goldens at 960×540 and 1080), the proportional
> `list_hit` proof, the `font_text` example, the `fonts` docs topic. The eager table load and the
> embedded atlas as a dedicated upload are recorded decisions (19, 20); the `L1256` coverage warning
> covers table faces only. The Exit's "SSIM 1.0 on the RTX 3090" and the human gate are owed.
> **W4's in-repo half is mechanized:** breakout's pause menu draws and hit-tests with its own baked Inter (`scenes/fonts.yaml`, `scenes/fonts/`), `run.lrp` re-recorded (the `UiList` now serializes `font: body`), a macOS `menu.png` golden, and a proportional `list_hit` click proven through record→replay; The external game's adoption is external. `discover` skips the `kind: fonts` table so `fonts.yaml` can sit at the gallery root. Authored 2026-09-30
> from `Plan/engine-gap-map.md` "Recommended order" item 3 (font assets) and external-game request 0009
> (`0009-ttf-fonts.md` in the external game request folder, priority *painful*, filed 2026-09-24). This line
> becomes the shipping log as waves land.
>
> Fires the trigger both UI plans recorded: "UI-specific fonts / font assets — trigger: content the
> fixed glyph set can't express" (`Plan/phase-14-ui.md`, out-of-scope list) and "Proportional or
> larger fonts — trigger unchanged" (`Plan/ui-widgets.md`, deferred list). The external game's Damages Invoice
> is that content.

## Problem

Text in Ludion is one embedded 8×8 bitmap face (`crates/ludion-renderer/src/font.rs`): 47 glyphs —
space, `A–Z`, `0–9`, `. , : ! ? - + / ( )` — lowercase folds to uppercase, everything else draws
blank. There is no font file, no Unicode, no proportional advance, no kerning, and therefore no
localization, no readable prose and no menu a player would mistake for a finished game. It is the
most visible sign of an unfinished engine, and every game that ships sits on it.

What a fix must preserve: text as the source of truth (Pillar 4), bit-exact CPU state and the
golden-image tiers (Pillar 3), one way per task (Pillar 6), and — because `list_hit` and
`measure_ui_text` are called from game systems — **text measurement that is sim-side, headless and
cross-arch deterministic**.

## Consumers and their concrete needs

| Consumer | Needs | Does not need (yet) |
|---|---|---|
| **the external game** (request 0009) — the trigger | Mixed case, proportional Latin, the punctuation its prose uses (`×`, `—`, `#`, `'`, curly quotes, `…`); itemised invoice lines, a dossier paragraph (wrapped), a title + subtitle at a larger size; crisp at 1080p; exact anchoring/wrap for proportional text; game-supplied OFL/Apache font; missing file = `L####` diagnostic; missing glyph = no crash. | Localization, RTL, ligatures, outlines (not asked). |
| **The shmup** (its request folder) | Its HUD is `UiRect` + `UiText` on a 480×270 pixel-art virtual field; nothing open asks for fonts. A pixel-art game wants a **pixel face at integer scale, nearest-sampled** — an SDF vector face would look wrong on it. | Vector faces. |
| **vigil** | HUD, `UiList` menus, recaps and flavor prose (`Plan/ui-widgets.md` decision 8). Works today in uppercase. | Nothing blocking; moves on its own sitting. |
| **breakout** | Pause menu `UiList` + `list_hit` in a game system — the proof that measurement is sim-visible. **The in-repo consumer that moves to a real face (W4).** | — |
| **The platformer** (`Plan/character-controller.md`) | Menus, a title screen when it becomes a shipped slice. | — |
| **Shipping bundle** (gap map item 5) | Localization, a settings surface, accessibility-sized text. | Now — see decision 13 for the seams kept open. |

The Tauri editor is **not** a consumer of rendered text: it draws its own DOM. It meets fonts only
as a `font` string field in the generic inspector and as a pipeline verb it can spawn.

## What exists today (code facts that shape the work)

- **The face** — `font.rs`: `GLYPHS: &[(char, [u8; 8])]`, `atlas_rgba()` bakes a one-row RGBA8
  atlas at `Renderer::new` (coverage in alpha), `glyph_cell(ch)` → column. Sampled nearest.
- **Two draw paths, one layout model duplicated.** World `Text` (`build_text_draw`, render.rs;
  pen walks +X, −Y per newline, `text.wgsl`, HDR target) and screen `UiText`/`UiList`
  (`push_ui_text_verts`, render.rs; +Y down, `ui_text.wgsl`, LDR `OUTPUT_FORMAT` in the overlay
  pass). Both advance the pen by exactly one square cell per `char`.
- **Measurement is engine-side and public.** `ludion-engine/src/ui.rs`: `measure_ui_text(text,
  size)` = longest line's `chars().count() × size` by `lines × size`; `wrap_ui_text(text, size,
  wrap_width)` = greedy, `floor(wrap_width / size)` columns; `measure_ui_list`, `resolve_ui_list`,
  `list_hit` build on them. Re-exported through `ludion::math`; **breakout's `menu_input` calls
  `list_hit` from a game system** — measurement affects gameplay, so it is sim-visible.
- **The two sides cannot disagree today** only because both hard-code "one cell per char"
  (phase-14 risk "text measurement drift"). A proportional face breaks that unless layout happens
  in exactly one place.
- **Components.** `Text { text, size, color }`; `UiText { anchor, offset, text, size, color, order,
  wrap_width, visible }`; `UiList` (`size` = line height). No `font` field anywhere.
- **Hash neutrality is available.** `WorldHash` hashes each component's canonical serde
  serialization (`ludion-sim/src/hash.rs`). A `#[serde(default, skip_serializing_if =
  "Option::is_none")]` field adds no bytes at its default (the `Animation` crossfade precedent; the
  serialized `UiText.wrap_width` default is the counter-example that moved every vigil hash).
- **The atlas pipeline is the template.** `kind: atlas-source` → `ludion generate-atlas` → a
  committed `<name>.png` + `<name>.atlas.yaml` pair carrying `generated_from:`; `--check` +
  `scripts/atlas-drift.mjs` is the drift gate; `check-scene` summarizes; `render --atlas` draws a
  CPU-only contact sheet.
- **Lazy content libraries exist** (`PathLibrary`, the animation library): a sim-side resource
  loading a `kind:` doc on first use, a load failure reported through `report_diagnostic`.
- **`ludion.toml` is CLI-only.** Only `crates/ludion-cli/src/manifest.rs` parses it (for `export`);
  the runtime asset root is the scene file's own directory (`LaunchRoots`), and the gallery has no
  manifest. This is why the font table is a content doc (decision 5), not a manifest section.
- **No licence gate for dependencies.** Licences are reviewed by hand; `THIRD-PARTY-NOTICES.md` +
  `licenses/` hold credits. `size-gate.mjs` refuses any publishable file over 1 MiB not allowlisted.
- **Blast radius of text.** 22 committed scenes carry `Text`/`UiText`/`UiList` (4 gallery goldens:
  `text`, `ui-hud` (+1080), `ui-atlas-icons`, `ui-wrap-window`; vigil's HUD in every district).

## Research summary

Bevy, Godot and Unreal rasterize at runtime into per-size caches (Bevy's system-font fallback made
rendering differ across machines — issue 16354); Godot offers per-font MSDF and image fonts for
pixel art; Unity TextMeshPro bakes SDF atlases in the editor with outline/shadow as SDF material
parameters. Rust crates checked 2026-09-30: `skrifa` (+`read-fonts`; MIT/Apache, Google
fontations, pure Rust, hardened), `harfrust` (MIT, the HarfBuzz port; rustybuzz is unmaintained,
RUSTSEC-2026-0206), `swash`, `fontdue`, `ab_glyph`, `cosmic-text`/`parley` (full stacks with
system-font discovery), `fdsm` (MSDF; pulls `image` + `nalgebra`). **Spike** (Georgia, 32 px):
fontdue and ab_glyph return **no** kerning for `A`/`V` — modern faces kern in GPOS, not `kern` —
so kerning needs a shaper or a GPOS reader.

## Decisions (settled 2026-10-01)

1. **Bake at build time.** A committed TTF/OTF + a committed `kind: font-source` → a committed
   atlas PNG + `kind: font` doc. The runtime and sim read only those files: no font parser or
   rasterizer ships, the atlas is a pure function of files (never of draw history), and the drift
   gate proves the pair current. Runtime rasterization rejected (history-dependent atlas, parser on
   the sim thread); revisit only for user-typed free text.

2. **Two modes in this arc, one format.** `mode: sdf` for vector faces (single-channel distance,
   `R8Unorm`, one atlas serves 960×540 → 4K and world space; the fragment thresholds with an
   `fwidth` edge) and `mode: bitmap` for pixel faces (coverage, integer scale, nearest — today's
   path). One layout engine, one atlas pipeline, one shader per text path with a mode switch. The
   doc stays channel-count-agnostic so `mode: msdf` can be added later. GPU curve rendering
   rejected on cost.

3. **Crates and code.** New pure crate **`ludion-font`** (no wgpu, no ECS, no YAML — the
   `ludion-sprite` shape): the `kind: font` model, layout, the embedded default face. Behind a
   `bake` cargo feature enabled only by `ludion-cli`: **`skrifa`** (parse, outlines, metrics, cmap)
   and **`harfrust`** (shape every ordered charset pair to extract GPOS kerning into a sparse table).
   **Our own SDF generator** (exact distance to flattened outline segments) and **our own scanline
   coverage rasterizer** for bitmap mode (signed-area accumulation over the same flattening), so
   all bake arithmetic is basic IEEE ops + `sqrt` and byte-identical across x86 and ARM. No
   `swash`/`fontdue`/`cosmic-text`/`parley`. All dependencies MIT or MIT/Apache; dependency crates
   need no `THIRD-PARTY-NOTICES.md` entry.

4. **Formats and the verb.** `generate-atlas` accepts `kind: font-source` (no `generate-font`
   verb): one bake verb, one `--check`, `atlas-drift.mjs` learns `*.font.yaml`, the editor host can
   already spawn it.

   ```yaml
   # fonts/body.font-source.yaml — authored
   format: 1
   kind: font-source
   name: body                        # output stem; absent → the file's stem
   file: Inter-Regular.ttf           # relative to this file; committed beside it with its licence
   mode: sdf                         # sdf | bitmap
   em_px: 56                         # sdf: bake size; bitmap: the face's native pixel size
   spread_px: 6                      # sdf only
   charset:                          # a map, so new collectors are additive
     ranges: [ascii, latin-1, general-punctuation]
     chars: "×—…‘’“”"
   ```

   ```yaml
   # fonts/body.font.yaml — generated, never hand-edited
   format: 1
   kind: font
   generated_from: body.font-source.yaml
   image: body.png
   mode: sdf
   units_per_em: 2048
   ascender: 1984
   descender: -494
   line_gap: 0
   atlas: { width: 512, height: 512, em_px: 56, spread_px: 6 }
   glyphs:                            # sorted by code point; metrics in integer font units
     - { char: "A", advance: 1360, bounds: [...], rect: [x, y, w, h] }
   kerning:                           # sparse, sorted by (left, right)
     - ["A", "V", -76]
   missing: ["\u{2011}"]              # requested chars the face lacks
   ```

   Metrics are integer font units + `units_per_em`; layout scales them with basic f32 arithmetic.
   Packing is deterministic, sorted by code point. Verification surfaces: `check-scene
   body.font.yaml` → a one-line summary (mode, glyphs, kern pairs, atlas size, source); `ludion
   render --font body.font.yaml` → a CPU-only specimen sheet (charset, metric lines, kerning sample,
   pangram at three sizes). The SDF `em_px`/`spread_px` defaults are tuned in W2–W3 against the
   two-resolution golden.

5. **Components name fonts by logical name through a `kind: fonts` table.** `font: Option<String>`
   on `UiText`, `UiList` and `Text`, skip-serialized at `None` — no existing hash, scene or replay
   moves. The value is a **name**, resolved through a `fonts.yaml` (`kind: fonts`) at the asset
   root, mapping names to `*.font.yaml` paths. A reserved **`default`** entry replaces the default
   face for components with no `font`; no table or no `default` → the embedded 8×8. Why a name and
   not a path: font references are spread across every content file, so this is the one seam that
   decides whether a per-locale face swap later is a table override or a scene-wide migration
   (decision 13). Why a content doc and not `ludion.toml`: the runtime never reads the manifest,
   and assets resolve from the scene's root.

6. **`size` is the line height.** `lines × size` stays the height rule, `UiList` pitch and
   `list_hit` rows are unchanged, and the 8×8 face keeps its geometry. Em derived:
   `em = size × units_per_em / (ascender − descender + line_gap)`.

7. **Layout moves engine-side; the renderer receives positioned glyph quads.** `extract` runs the
   one `ludion-font::layout` and emits text instances as quads (atlas, pixel rect, UV rect) for
   both screen and world paths; `push_ui_text_verts`/`build_text_draw` stop knowing what a
   character is. Measurement and drawing call the same function — the phase-14 drift risk is gone
   by construction. Pixel faces sit on their grid through decision 18's metrics, not an origin
   snap in extract. A table face's atlas joins the hash-keyed texture cache; the embedded atlas
   stays a special upload (decision 20).

8. **Missing glyphs and unknown names.** A vector face draws the face's `.notdef` (tofu, always
   baked); the 8×8 face keeps drawing blank (byte-identity). The bake records requested-but-absent
   code points in `missing:`. `check-scene` warns (new `L####`) on authored `Text`/`UiText`/`UiList`
   strings with characters the named face lacks, and **errors** on a font name absent from the
   table (closest-name suggestion). At runtime an unknown name or a failed font load reports once
   through `report_diagnostic` and **falls back to the default face for both measurement and
   drawing**, so `list_hit` matches what is drawn. Runtime-formatted strings never produce per-frame
   diagnostics.

9. **Shaping: tier A only.** Advance + baked GPOS pair kerning; layout walks `char`s. Line breaking:
   greedy on space, after `-` and `—`, on hard breaks; never at U+00A0. Ligatures, complex scripts,
   mark positioning, bidi/RTL and UAX #14 are deferred (tier B: runtime harfrust, the font file on
   the sim thread, glyph-id-keyed atlases — a recorded `format` bump).

10. **Measurement in the sim.** A `FontLibrary` sim resource (the `PathLibrary` shape, but loaded
    eagerly — decision 19: the `kind: fonts` table and `kind: font` docs, failures through
    `report_diagnostic`); the sim never touches the PNG. **The signatures change** —
    `measure_ui_text`, `wrap_ui_text`, `measure_ui_list`, `resolve_ui_list` and `list_hit` take the
    resolved font; no default-face shorthands (one way). breakout updates in the same wave; external
    git-dependency games update in step. Re-baking a face used by a committed replay's scene re-runs
    that replay (content already works this way).

11. **The 8×8 face becomes data.** A committed, **authored** `kind: font` doc + PNG in
    `ludion-font` (`mode: bitmap`, no `generated_from:` — source, like a hand-drawn sprite),
    `include_bytes!`'d, written once from `font.rs`'s `GLYPHS` in W1; the arrays and every
    one-cell-per-char path are then deleted. Lowercase folding becomes an explicit alias list in
    that doc. A test asserts its PNG matches the old atlas bytes. It stays the default face, keeping
    the most legible debug face for a headless agent.

12. **Faces and licensing.** The engine ships **no default vector face**. Test faces, committed
    under `examples/gallery/fonts/` with their licence beside them, a copy in `licenses/` and a
    `THIRD-PARTY-NOTICES.md` section: **Inter** for vector mode (fallback Noto Sans), and a pixel
    face picked in W2 against the criteria — OFL-1.1, no Reserved Font Name, pixel-grid outlines,
    ASCII or wider (candidates: Silkscreen, a Press Start 2P-style face). For each, the OFL text,
    the absence of an RFN and (for Inter) GPOS kerning are verified when the file is committed.
    breakout commits its own copy of Inter. Game-supplied faces record themselves in the game's
    notices; `docs/` states the rule (a committed TTF carries its licence beside it). **Never a
    system font**: the bake reads only the file the source names.

13. **No localization work; seams kept open.** Nothing localization-specific is built (the earlier
    `charset.from:` hook is dropped — it would read a doc kind that does not exist). The user's
    requirement is that adding localization later is painless; the seams that guarantee it:
    the logical font name + `kind: fonts` table (per-locale face selection becomes a table section
    or `fonts.<locale>.yaml`, no scene edits — decision 5); `charset` is a map in a versioned
    format, so a string-table collector is an additive key; `format` versions on both docs make
    tier B's glyph-id table an explicit bump. The `kind: strings` design (a `locale`, a flat key →
    text map, formatting in Rust) belongs to the post-M7 shipping-bundle grilling.
    Amended 2026-11-19 (owner): localization (string tables, per-locale font overrides) is pulled forward into dialogue W3, see Plan/dialogue.md d13, d14.

14. **World-space `Text` is in scope** with `UiText`: the same `font` field and the SDF path in
    `text.wgsl`, one gallery golden.

15. **Effects deferred.** Outline, shadow and glow wait for the trigger "a text surface a sitting
    judges unreadable over its background"; reserved shape `outline: { width, color }` /
    `shadow: { offset, color }` on the text components, skip-serialized. Stopgap: a second `UiText`
    offset by 2 px. The trigger fired for the outline (`Plan/ui-hud.md` W5a); the shadow is W5b and the glow stays
    deferred.

16. **Editor and protocol.** Wire unchanged (components cross as opaque JSON; 38 methods, schema
    hash unmoved). The bake is `generate-atlas`, which the editor host may already spawn with
    `--json` (`Plan/comfy-generation.md` decision 15). `font` is a plain string field in the
    inspector; a picker is deferred. Re-baking during `serve`/`dev` reloads through the doc/texture
    reload path, verified in W3.

17. **Determinism and goldens.** Layout is integer metrics + basic f32 ops, cross-arch identical.
    The bake must be byte-identical on both pinned machines (owned SDF and coverage code; no
    `powf`/trig, or via `ludion_core::math`); a cross-arch mismatch surfaces as an `atlas-drift`
    failure on the M4. Image tiers unchanged: SDF text is per-GPU bytes, bit-exact on pinned GPUs,
    SSIM elsewhere. No subpixel glyph caching, so no hidden per-frame state.

18. **Pixel faces sit on their grid through a `line_height` override.** Optional on `kind:
    font-source`, in font units. The bake moves the ascender to absorb it (descender and line gap
    kept), so `size` = line height (decision 6) is unchanged and a pixel face is crisp at integer
    multiples of its source line height. Silkscreen: 1250 → sizes 10, 20, 30… Extract does not snap
    glyph origins; the metrics carry the grid instead.

19. **`FontLibrary` loads the whole table eagerly at world build.** Extract holds only `&World`, so
    a lazy library could not fill itself there; a table is a handful of small documents. Built by
    `FontLibrary::load(project_root)` beside the other libraries (and in `bare_driver`), never
    hashed or snapshotted. A face that fails to load is kept as a fault (`L1255`, once per path) and
    resolves to the default face, as does an unknown name (`L1254`, once per name).

20. **The embedded atlas stays a dedicated upload (amends decision 7).** `font: None` on a draw
    means the renderer's own embedded atlas with the nearest sampler, as `texture: None` on a UI
    rect means `self.white`. Only a table face carries a `FontDraw { texture, sdf }` into
    `RenderScene.textures`; moving the embedded atlas there would add a texture entry to every
    text scene and churn extract tests for no gain. A table face's missing atlas is an extract
    `L1101`/`L1102` at `/…/font`, never a fallback to the 8×8 atlas.

## Waves

Each wave one commit, independently landable; `node scripts/check.mjs` green at every exit.

### Wave 1 — one layout engine; the 8×8 face becomes a `kind: font` (no visible change)

- New crate `ludion-font` (layout half): the `kind: font` model and parser (in `ludion-scene`'s
  KINDS table), `layout()` with advance + kerning + wrap (space, `-`, `—`; hard-break; not U+00A0),
  integer metrics. The 8×8 face as an authored, embedded bitmap-mode doc + PNG with its alias list
  (decision 11).
- `ui.rs` measure/wrap/list functions route through `layout()` (default face; the font argument
  arrives in W3); `extract` emits positioned glyph quads; `push_ui_text_verts`/`build_text_draw`/
  `font.rs` lose their per-char logic, and `GLYPHS` is deleted.
- **Exit (mechanized):** every golden byte-identical on the reference box, every `WorldHash`
  trajectory and committed replay identical, `dump-graph` unchanged; the embedded PNG equals the
  old atlas bytes; layout unit tests (kerning applied, wrap in advance units, line height = `size`,
  empty/multi-line rules unchanged); measure == drawn extent; breakout's menu tests green.

### Wave 2 — the bake: `kind: font-source` → atlas + doc

- `ludion-font` `bake` feature (skrifa outlines, harfrust pair kerning, own SDF generator, own
  scanline coverage rasterizer); `generate-atlas` accepts `font-source`; `check-scene` summary;
  `render --font` specimen sheet; `atlas-drift.mjs` covers `*.font.yaml` (CI and TeamCity lists in
  step).
- Inter and the picked pixel face committed under `examples/gallery/fonts/` with licences,
  `licenses/` copies and `THIRD-PARTY-NOTICES.md` sections (decision 12).
- **Exit:** bake twice → byte-identical pair; `--check` detects drift in either file; kerning
  present for Inter's `A`/`V`; `missing:` lists an absent char; malformed TTF → a diagnostic, never
  a panic (a small corrupt-font corpus test); `ludion explain` for the new codes. **Owed:** the Windows
  box re-runs `atlas-drift` and the Inter PNG canary (cross-arch bake identity; the M4 made the bakes).

### Wave 3 — the `font` field, the `kind: fonts` table, SDF rendering, the coverage check

- `font` on `UiText`/`UiList`/`Text` (skip-serialized); `kind: fonts` with the reserved `default`;
  `FontLibrary`; the unknown-name diagnostic + default-face fallback; the SDF fragment path in both
  text shaders; `check-scene` coverage warning and unknown-name error; the measurement signature
  change with breakout's call sites updated (still on the 8×8 face).
- Gallery: `ui-fonts.yaml` (Inter: invoice-style lines, a wrapped paragraph, a title, kerned pairs,
  a tofu glyph; the pixel face at integer scale) blessed at 960×540 and 1920×1080; world-space
  `Text` with Inter.
- Example `crates/ludion/examples/font_text.rs`; docs topic `fonts` (authoring, bake loop, charset,
  the `fonts.yaml` table and `default`, the licensing rule, `size` = line height) via the
  `ludion-docs` skill; `screen-ui` and `components-ui` updated.
- **Exit:** new goldens green at SSIM 1.0 on the RTX 3090; every pre-existing golden and hash still
  identical; two-run `WorldHash` identical with fonts; headless `list_hit` on a proportional list
  matches the drawn rows; hot reload of a re-baked face; wire 38 methods, schema hash unmoved.
  macOS goldens queued.
- **Human gate (owed):** read the gallery scene at 960×540, 1080p and 4K — is the text crisp, is
  kerning visible, does the pixel face stay pixel-sharp.

### Wave 4 — consumers

- breakout's pause menu moves to Inter (its own copy, a `fonts.yaml`): a deliberate bless of its
  goldens and re-record of its replays; proves a proportional `list_hit` through input and replay.
- The external game adopts it (its git dependency); its owner's verdict on the Damages Invoice is the external
  exit, and external-game request 0009 moves to its `done` folder with a Shipped section. vigil waits for its own
  sitting.

## Risks

- **SDF quality at the golden's 960×540** (12 px text) — thin strokes wash out. Mitigation: tune
  `em_px`/`spread_px`; bitmap mode for tiny text; MSDF as the recorded upgrade. The two-resolution
  golden exposes it at bless time.
- **Cross-arch bake identity** — one differing byte fails `atlas-drift` on the M4. Owned code with
  basic IEEE ops is the mitigation; the gate is the detector.
- **Wave 1 refactors every text path under a byte-identity gate** — a different expression order
  for quad corners changes golden bytes. Port the arithmetic expression-for-expression; the gate is
  non-negotiable, not a bless.
- **API break for games** (decision 10) — small today, larger every month it waits.
- **Scope creep toward a text stack** — every deferral below has a written trigger; a wave adding
  one is a flagged decision.
- **Font licensing mistakes** on a public mirror — decision 12's criteria, the licence-beside-TTF
  rule, review at commit.
- **Content churn from re-bakes** — a changed charset moves glyph rects; packing sorted by code
  point keeps the diff reviewable, and the drift gate proves it current.

## Deferred / non-goals (triggers recorded)

- Runtime rasterization — trigger: user-typed free text needing arbitrary glyphs.
- Runtime shaping, ligatures, bidi/RTL, complex scripts (tier B) — trigger: a committed locale in a
  complex script.
- UAX #14 line breaking, CJK atlases, multi-page atlases — trigger: a CJK locale.
- Fallback font chains, emoji / colour glyphs — trigger: a needed glyph only another face has.
- MSDF — trigger: a verdict that SDF corners look soft at a real consumer's size.
- Outline / shadow / glow — trigger: text unreadable over its background at a sitting.
- PNG glyph-sheet sources for pixel faces — trigger: an artist's pixel font that exists only as a
  sheet.
- Rich text (inline colour, bold spans, inline icons) — trigger: content whose meaning is lost
  without it.
- Localization (string tables, locale switch, per-locale font tables, plurals) — the post-M7
  shipping grilling, on decision 13's seams.
- A shipped default vector face for `ludion new` / templates — its own small decision once Inter
  has been seen on screen.
- Variable-font axes, hinting at bake — trigger: a face that needs them.
- An editor font picker — trigger: a designer authoring fonts in the editor.
- GPOS feature narrowing for kerning extraction (`bake/kerning.rs` takes x_advance − hmtx after all
  default GPOS features; `curs`/mark positioning would record spurious pairs) — trigger: the first
  script face or combining-mark charset.
