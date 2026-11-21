# Plan: Dialogue — string tables, a line queue, subtitles

> **Waves 1 and 2a mechanized — text and voice ship; subtitles' built-in widget (W2b), localization's render hosts (W3b-2) and fallback chains (W3c) are laid out below. W3a (localization, text side) and W3b-1 (per-locale faces, check-scene keys) are mechanized.**
> Driven by Neon Shroud request 0024 (dialogue, subtitles, localization). Game-facing doc: `ludion docs dialogue`.

## Design decisions

1. **Opt-in, not builtin.** `.dialogue(config)` registers both kinds with `doc_kind_engine_owned`, one `validate_docs`
   pass, the two components through the builder's own `components` closure, and installs the config. Nothing is
   builtin, so `component_schemas.json`, the protocol schema, goldens, replays and every existing `WorldHash` are untouched.
2. **State is entities, not a resource** (resources reset on restore/checkpoint, `docs/checkpoints.md`). One
   `DialogueLine` entity per said line (the handle is the entity, `LineHandle(EntityId)`); one lazily spawned
   `DialogueControl` entity (blocked channels, suspended, next sequence number; the locale left it for `LocaleState`, d27). Spawned on the first API call,
   so an opted-in game that never speaks is byte-identical. Both are hashed and snapshotted for free. This deviates
   from the request's `Dialogue` resource: same API as free functions over `&mut World`.
3. **API calls only mark state; `dialogue_update` emits every event.** `say` spawns a `Queued` line; `interrupt` sets
   `cancel`; the system turns that into `Ended` the same tick it runs.
4. **Order** is `(priority desc, order, seq)`, `order` being the line's monotonic `seq` or its sequence head's.
5. **Slots:** `radio` one global, `ui` one global, `world` one per speaker entity. Channel table, filters and buses are W2.
6. **Interrupt rule** of the head vs the slot's current line: `none` never; `lower` iff current priority < head's;
   `any` always. A cut line ends `Interrupted` at once (the audio fade is W2).
7. **Sequences** (`say_sequence`): later lines are `Pending` until their predecessor ends; `Completed` promotes the next
   to `Queued` the same tick (no gap, no hold release); any other end drops the rest (`Dropped`). `holds_gameplay` of
   the head is copied to every line.
8. **Holds.** While any `Playing` line holds, `dialogue_update` calls `GameClock::hold()` each tick; it runs on the sim
   clock (`ctx.frame`), so lines advance through their own hold. During a hold a queued line may start only if
   `critical` or itself holding. Physics is not frozen. A line started on frame F with duration D holds on F..F+D-1,
   so `game_advanced` is false on F+1..=F+D (measured in `tests/dialogue.rs`).
9. **Blocked channel:** a playing line on it ends `Blocked`; a queued `when_blocked: drop` line ends `Blocked`; `wait`
   stays queued (and still ages against `max_wait_ticks`).
10. **Suspend** freezes progress (no elapsed, no aging, no starts); cancels still apply; a holding line keeps holding.
11. **Duration** (ticks), fixed at start: `duration_ticks` if authored, else `max(min_s, base_s + per_char_s * chars)`
    seconds over the formatted active-locale text, converted with `ceil(x - x * 1e-6)` (a private copy of the
    `ludion-ai` rule). Defaults `base_s 1.0, per_char_s 0.06, min_s 1.5`: **accepted by the owner 2026-11-19**.
12. **Timing:** a line started at frame F has `elapsed` 0 on F and ends `Completed` on frame F+D; its subtitle is
    present on F..F+D-1 with `end_frame = F + D` (exclusive). Cues fire on the tick `elapsed == at` (`at: 0` on the
    start tick); a suspended line fires none.
13. **Strings:** lookup active locale, then default locale (`L5132` once per locale/key), then the key itself (`L5133`
    once per key). `{name}` placeholders, `{{`/`}}` escape literal braces; a missing arg leaves `{name}` and warns
    `L5134` once. No plurals or gender. Text re-resolves every tick, so `set_locale` changes live subtitles; a started
    line keeps its duration.
14. **Locale** lives on `LocaleState` (d27). `kind: locales` replaced `DialogueConfig.default_locale`, which is removed (d26).
15. **No `voice:` field in W1**: an unknown field is `L1120`, never silently ignored; W2a added it additively.
16. **Voice path.** `voice:` on a line, `{locale}` substituted with the active locale; if that file does not read, the
    default locale's (`L5137` warning once per path); a path without `{locale}` is used as is.
17. **Voiced duration** = `clip_ticks + tail_ticks` (line `tail_ticks`, else `DialogueConfig::tail_ticks`, default **12**,
    the request's proposal), computed at start from the resolved file. An authored `duration_ticks` still wins. An
    unreadable voice falls back to the reading-time rule (`L5138` once per path).
18. **Channel audio** is `DialogueChannels` (`.dialogue_channels(...)`): per channel `ChannelAudio { category, filter, gain,
    range, cut_fade_ticks }`; defaults radio `Voice` + `band_pass(300, 3400)` (owner-accepted), world `Voice` unfiltered
    `range 20.0`, ui `Ui` unfiltered; `cut_fade_ticks 6`; `suspend_buses [Voice]`, `suspend_fade_ticks 6`.
19. **Radio and ui voices are non-positional stoppable voices** (`AudioQueue::push_voice`); world voices are `push_on` the
    speaker entity with the channel range.
20. **A playing line that ends any way but `Completed`** stops its voice with the channel's `cut_fade_ticks` (a `Completed`
    line's voice has already ended within its tail).
21. **Suspend** sends `bus_pause(suspend_buses)` once on the transition, resume `bus_resume`; only while a voiced line is on
    air, so a text-only world's audio log is unchanged. The ui channel's `Ui` bus is not paused by default.
22. **Voice ids live in an unhashed resource** reset on rebuild; a restored mid-line resumes its voice (38).
23. **Ducking is the game's `DuckRule { when: Voice, .. }`** (0023); dialogue adds nothing.
24. **Opt-in `EngineBuilder::localization()`.** It registers `kind: strings` (moved from `.dialogue()`) and `kind: locales`,
    both `doc_kind_engine_owned`, plus the validators, the `LocaleState` component and the `localize_text` system. It is
    idempotent (a builder flag), and `.dialogue()` calls it. The system is pushed like `.system(...)` at the point of the
    first call, so it runs in the game's Sim order there.
25. **`kind: locales`** (`locales.yaml`, at most one doc, else `L1722`): `default: <tag>` plus `locales: { <tag>: { name } }`.
    `default` must be listed (`L1722`). When the doc exists, every `kind: strings` locale must be listed (`L1723`). Without
    the doc the default is `en` and any locale tag is accepted. This relaxes the request's "every strings doc must be
    `en`", which would have broken W1 content.
26. **`DialogueConfig::default_locale` is removed** (BREAK; no external user). The default locale comes from `kind: locales`
    everywhere (`localization::default_locale(world)`), which also fixes W1's build-time-config gap. `L1710` keeps only the
    reading constants.
27. **The active locale is a `LocaleState { locale: Option<String> }` component** on one lazily spawned entity, mirroring
    `DialogueControl`. Hashed and snapshotted. `localization::set_locale` and `localization::locale` replace
    `dialogue::set_locale` and `dialogue::locale` (removed), and `DialogueControl.locale` goes too. `scene.save` skips the
    entity (`flatten_world`): the locale is a player setting, so a scene saved under `fr` equals one saved under `en`.
28. **`UiText`/`Text` gain `key: Option<String>`, `args: BTreeMap<String, String>` and a runtime-only `resolved`.**
    `key` is authorable (skip-if-none) and `args` skip-if-empty; both are sim state, hashed and snapshotted, so unkeyed content
    is byte-identical. `resolved` is `#[serde(skip)]` and `#[schemars(skip)]`: never written, hashed or snapshotted, a pure
    function of key, args, locale and docs. **`text` is never written by the engine**; it stays the authored fallback.
    `shown()` returns `resolved` else `text`, and layout, wrap, measure, draw and hit tests read it. `localize_text` fills
    `resolved` every tick in `EntityId` order (only when it differs): active locale, else default (`L5132`), else the key
    (`L5133`). One resolve pass also runs at the end of every rebuild or restore, once the docs are committed, so the first
    extract shows it. Clearing `key` clears `resolved`. Why not a writer that omits `text` when `key` is set: serde cannot skip
    one field conditionally on another without a hand-written `Serialize`, and it would put locale-resolved text into
    snapshots and the editor's write path. A derived runtime field keeps text the source of truth (Pillar 4).
29. **A keyed text on an engine without `.localization()`** draws its authored `text`; `L5139` warns once per engine, from the
    frame-boundary report. A `set_locale` to a tag with no strings table warns `L5140` once per tag and still applies.
30. **Out of W3a:** keys on `UiList`/`UiRadial` (trigger: a menu that needs them); `check-scene` key checks and render-host
    locales (W3b); text-size and language settings (request 0022 W3).
31. **`fonts.yaml` `locales:` overrides.** An optional `locales: {tag: {name: path}}` section replaces a base `fonts:` name for
    one locale. Each name must be one `fonts:` defines, each path follows the base path rules (`L1253`, pointer
    `/locales/<tag>/<name>`). Resolution for `n = name or default`: the locale's override of `n`, the base `n`, the locale's
    override of `default`, the base `default`, the embedded face; a face that failed to load counts as absent at each stage.
    `emit` writes the section only when non-empty, so existing tables stay byte-identical. Every override path loads into the
    library, so hot reload's `holds()` sees it.
32. **`FontLibrary::face(name, locale)`** (a break, matching the request) is the one lookup; `table_face` and `paths` take the same
    pair. `localization::active_locale(&World)` reads the active locale on a read-only world (extract and hit tests hold
    `&World`); `locale(&mut World)` delegates to it. Extract and hit tests resolve the locale only when
    `has_locale_overrides()`, so a world without overrides pays nothing and resolves as before. `placed_font` takes the
    locale, so the atlas paths agree with the layout face.
33. **check-scene keys.** A scene with a keyed `UiText`/`Text` loads the content root's strings docs into a scratch world
    (`install_content_docs`, a `World` installer the render hosts reuse). A key the default locale lacks is `L1715` (error).
    Each locale with a strings table is checked: its string, else the default's, with `{placeholders}` removed
    (`literals`), against `table_face(font, Some(locale))`, as `L1256` (warning) at `.../key` naming the locale.
34. **Unkeyed text** keeps its single `L1256` against the base face; a keyed component's authored `text` is not checked.
35. **Bare render hosts** load string tables when a scene has keyed text or a locale is asked for;
    `render_scene_with_registry(.., locale)` (a break) and `ludion render --locale` (W3b-2).
36. **`VoiceStart` carries `offset_ticks`** (0 = from the start). The sink starts the cursor that many ticks in, clamped to
    the clip. `AudioQueue::push_voice_from(event, offset_ticks)` and `push_on_from(entity, event, range, offset_ticks)`;
    `push_voice`/`push_on` are the 0 case. The tracked end is `frame + clip - offset`. An offset at or past a known clip
    emits nothing (the id is still returned; a stop is ignored).
37. **Pause-aware tracking.** The engine records the buses held by every `BusPause` it emits (a held master holds every
    bus). A tracked voice on a held bus has its end pushed one tick per held tick, so a stop after the resume still reaches
    it, and it does not keep a `DuckRule` live. The record resets with the rebuild's blanket stop. The end may run up to
    one pause fade long, because the sink's cursor moves through the fade; a stop to a voice that already ended is ignored
    by the sink.
38. **Restore resumes voices.** `DialogueLine.voice_file` records the file resolved at start, so the voice keeps the locale
    it started in. On the first `dialogue_update` after a build or restore, every playing line with a `voice_file` and no
    tracked voice restarts its voice at `offset = elapsed` (after this tick's increment), if the offset is under the clip.
    While suspended, the suspend buses are paused before that restart. Checkpoints take the same path.
39. **Fallback syntax (W3c).** A `fonts:` value is a path or a mapping `{path, fallback}` whose `fallback` is a block list of
    names (the strict-subset YAML has no non-empty flow sequences). `FontsDocument.fallbacks` keeps `(name, [names])` sorted
    by name; `fonts` stays `(name, path)`, and `emit` writes the mapping only for entries with a fallback. `L1253` at
    `/fonts/<name>/fallback[/<i>]` for an undefined name, the entry itself, a cycle, an empty list, or a flattened chain over
    8 faces. `locales:` overrides stay plain paths.
40. **Chain flattening per locale.** At load, every base name with a fallback gets a chain for the base table and for each
    `locales:` tag. Each name resolves through `(locale, name)` then the base `name` (decision 31), a face that failed to load
    is skipped, and the order is depth-first through each fallback's own fallbacks, deduped by path. A per-locale primary
    keeps the same fallbacks; a locale with no `locales:` entry uses the base chain. `FontLibrary::face` returns the chained
    face, and `chain_paths(name, locale)` lists the `(doc, image)` paths index-aligned with `face.chain()`.
41. **The chain rides inside `FontFace`**, so no measurement API changes: `FontFace::with_fallbacks`, `fallbacks`, `chain`,
    `face_for`. A fallback glyph's advance, and its kerning, convert into the primary's units with integer rounding
    `(v * L_primary + L_k / 2).div_euclid(L_k)` (exact when the units agree). Not an lcm: Silkscreen's 1250 and Inter's 2478
    have an lcm near 1.5 million, which overflows `LaidGlyph.advance_units`. Widths, extent and wrap stay in primary units.
    Kerning applies only between two glyphs of one face. Each `LaidGlyph` and `GlyphQuad` carries a `face` index. A fallback
    quad uses its face's unit (`size / L_k`) and atlas on the primary's baseline; a primary quad keeps its exact float
    expressions, so a one-face text is bit-identical. A char no face has draws the primary's notdef.
42. **One draw per face run.** Extract splits each text's quads by `face` into one `UiInstance::Text` (UiText, each UiList line,
    UiRadial) or one `TextRun` (world `Text`) per face, primary first, with that face's atlas and its own `resolve_outline`.
    A text on one face is exactly one run. The renderer is unchanged.
43. **`L1256` over the chain.** `check-scene` counts a char missing only when no chain face has it. `validate_localization_docs`
    (now given the content root) also checks every string of each locale, with placeholders removed, against
    `table_face(None, locale)`'s chain, one warning per (locale, key) at `strings/<doc path>/strings/<key>`; it is skipped
    without a font table `default`.


Validation codes: `L1719` bad channel table or `dialogue_channels` without `dialogue` (build); `L1720` a line's voice does not
read for the default locale (error); `L1721` it does not read for another locale with a `kind: strings` doc (warning).
Runtime: `L5137` fell back, `L5138` unreadable.
W3a adds `L1722` bad locales doc, `L1723` unlisted strings locale, `L5139` keyed text without the opt-in (warning), `L5140`
locale without a strings table (warning).

## Waves

- **W1 (shipped):** `kind: strings`, `kind: dialogue`, the queue, `DialogueEvents`, `Subtitles`, `L1710`-`L1718`,
  `L5130`-`L5136`, the example and `ludion docs dialogue`.
- **W2a voice (shipped):** `voice:` with default-locale fallback, duration from the WAV length plus `tail_ticks`, radio
  band-pass via `VoiceFilter`, world lines via `push_on`, `AudioQueue::push_voice`, ducking through the voice-bus
  `DuckRule`, suspend to `bus_pause`, `.dialogue_channels(...)`, `L1719`-`L1721`, `L5137`-`L5138`.
- **W2b, cut in two.** Part 1 (audio): the voice start offset, pause-aware tracking and the restore resume (36-38).
  Part 2 (subtitles): the built-in `SubtitleView` component and its golden in a new gallery scene (subtitles on by default,
  per the owner ruling); `subtitle_range` against `AudioListener` (else the lowest-id camera).
- **W3a localization, text side (shipped):** `kind: locales`, the `LocaleState` locale, keyed `UiText`/`Text` with `args`
  and `resolved`, `localization::*`, `L1722`, `L1723`, `L5139`, `L5140`, `ludion docs localization`. Engine-only, no golden.
- **W3b-1 localization, font side (mechanized):** the `locales:` section (`ludion-scene` parser), `FontLibrary::face(name, locale)`,
  `active_locale`, every engine face site passing the locale, `install_content_docs`, `check-scene` `L1715` and per-locale
  `L1256`.
- **W3b-2 localization, render hosts:** strings loaded in bare hosts, `render_scene_with_registry(.., locale)`,
  `ludion render --locale`, and the `localization` gallery scene with `en` and `fr` goldens.
- **W3c fallback chains (39-43):**
  - A `fonts:` value may be `{ path, fallback: [name, ..] }` (a block list).
  - `ludion-font` lays text out over a face chain in the primary's units, with each fallback advance rounded in (not an lcm),
    so a one-face chain is bit-identical. Kerning applies only within one face, and each glyph carries its face index.
  - Quads are grouped per face, and extract emits one `UiInstance::Text` or `TextRun` per face run, with an outline per face.
  - Hit tests and measurement walk the chain, since the chain rides inside `FontFace`.
  - `L1256` walks the chain, plus whole-table coverage: every string of a locale against that locale's `default` chain.
  - The `font-fallback` gallery scene and golden (`pixel-latin`: Silkscreen over Inter).

## Out of scope (with triggers)

- Branching and conditions: never (Pillar 6; the game's Rust decides what to say).
- Choices: `UiList`. Lip sync: a portrait that needs more than `speaking()`.
- Plurals, gender, RTL, CJK: the `Plan/font-assets.md` triggers.

## Owner rulings (2026-11-19)

- Accepted. The reading-time constants (decision 11).
- Taken (W3a). `kind: locales` relaxes the request's "every strings doc is `en`" (d25), and `DialogueConfig.default_locale` is
  removed (d26, a break with no external user).
- Accepted, scheduled (W3). Localization pulled forward from `Plan/font-assets.md` Deferred into W3 (request amendment 1), and fallback chains
  (amendment 2, W3). `Plan/font-assets.md` is not edited here; this is the record.
- Accepted. Subtitles on by default (W2); band-pass values (W2).
- Accepted. The API shape (decision 2): free functions over `&mut World` (`dialogue::say`, `set_locale`, …) with state on hashed
  `DialogueLine`/`DialogueControl` entities, instead of the request's `Dialogue` and `Locale` resources, because
  resources reset on restore/checkpoint.

## Status

- W1 mechanized: `crates/ludion-engine/src/dialogue/`, `tests/dialogue.rs`, `crates/ludion/examples/dialogue.rs`.
  The validator takes the default locale from the builder's config (the build-time world has no config resource yet).
- W1 API follow-up: `say(world, id, speaker)` takes the speaker; `DialogueEvent::Started` carries `speaker` and `channel`.
- W2a mechanized: `dialogue/voice.rs` (`DialogueChannels`, the WAV-length cache, locale resolution),
  `AudioQueue::push_voice` (a global tracked voice; `AttachedVoice.entity` is now optional), root-aware doc validators
  (`DocValidator` takes `&Path`), `tests/dialogue_voice.rs`, the example's voiced lines. Owed: a by-ear check of the radio
  band-pass on a real device.
- W3a mechanized: `crates/ludion-engine/src/localization/` (strings moved from `dialogue/`, `LocalesDoc`, `LocaleState`,
  `localize_text`), `UiText`/`Text` `key`/`args`/`resolved` + `shown()`, `tests/localization.rs`, `docs/localization.md`, the
  example's keyed HUD label. The component schema snapshot is re-blessed; no protocol schema, golden or replay moved.
- W3b-1 mechanized: `locales:` in `ludion-scene/src/fonts.rs`, `FontLibrary::face(name, locale)` and `has_locale_overrides`,
  `localization::active_locale`, `install_content_docs`/`string_in`/`literals`, `scene_font_diagnostics` keyed checks,
  `tests/localization.rs`, `crates/ludion-cli/tests/check_scene_fonts.rs`, `docs/fonts.md`, `docs/localization.md`. External
  break: `face`/`table_face`/`paths` take a locale (an external game's HUD test: `fonts.face(Some("hud"), None)`). No protocol
  schema, component schema, golden or replay moved.
- W3b-2 mechanized: bare render hosts load the content root's string tables and resolve keyed text (`extract_scene_file` takes
  `locale`; scenes without a key skip it), `ludion render --locale`, `GoldenTest::with_locale`, the gallery `localization`
  scene (`locales.yaml`, `strings/`, `fonts.yaml` `locales.fr`), `docs/localization.md`. New Windows goldens blessed on the
  RTX 3090: `localization` (en, discovered) and `localization-fr` (hand-written test). External break:
  `render_scene_with_registry` gains a trailing `locale: Option<&str>` (serious-isaac's `examples/probe_look.rs` passes `None`).
  Owed: the macOS twins of both goldens. No protocol schema, component schema, replay or existing golden moved.
- W2b part 1 mechanized: `VoiceStart.offset_ticks` (sink cursor), `AudioQueue::push_voice_from`/`push_on_from`, pause-aware
  attached-voice tracking (`held_buses`), `DialogueLine.voice_file` and the first-update voice resume in `dialogue/queue.rs`,
  `tests/audio.rs`, `tests/dialogue_voice.rs`, the `ludion-audio` offset test, `docs/dialogue.md`, `docs/audio-space.md`. No
  protocol schema, component schema, golden or replay moved; only a world with a voiced line on air hashes differently.
