# Plan: Dialogue — string tables, a line queue, subtitles

> **Wave 1 mechanized — text-only dialogue ships; voice (W2) and localization (W3) are laid out below.**
> Driven by Neon Shroud request 0024 (dialogue, subtitles, localization). Game-facing doc: `ludion docs dialogue`.

## Design decisions

1. **Opt-in, not builtin.** `.dialogue(config)` registers both kinds with `doc_kind_engine_owned`, one `validate_docs`
   pass, the two components through the builder's own `components` closure, and installs the config. Nothing is
   builtin, so `component_schemas.json`, the protocol schema, goldens, replays and every existing `WorldHash` are untouched.
2. **State is entities, not a resource** (resources reset on restore/checkpoint, `docs/checkpoints.md`). One
   `DialogueLine` entity per said line (the handle is the entity, `LineHandle(EntityId)`); one lazily spawned
   `DialogueControl` entity (locale, blocked channels, suspended, next sequence number). Spawned on the first API call,
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
    `ludion-ai` rule). Defaults `base_s 1.0, per_char_s 0.06, min_s 1.5`: **Proposed, owner to confirm.**
12. **Timing:** a line started at frame F has `elapsed` 0 on F and ends `Completed` on frame F+D; its subtitle is
    present on F..F+D-1 with `end_frame = F + D` (exclusive). Cues fire on the tick `elapsed == at` (`at: 0` on the
    start tick); a suspended line fires none.
13. **Strings:** lookup active locale, then default locale (`L5132` once per locale/key), then the key itself (`L5133`
    once per key). `{name}` placeholders, `{{`/`}}` escape literal braces; a missing arg leaves `{name}` and warns
    `L5134` once. No plurals or gender. Text re-resolves every tick, so `set_locale` changes live subtitles; a started
    line keeps its duration.
14. **Locale** lives on `DialogueControl` (hashed). `kind: locales` is W3: `DialogueConfig.default_locale` stands in.
15. **No `voice:` field in W1**: an unknown field is `L1120`, never silently ignored; W2 adds it additively.

## Waves

- **W1 (shipped):** `kind: strings`, `kind: dialogue`, the queue, `DialogueEvents`, `Subtitles`, `L1710`-`L1718`,
  `L5130`-`L5136`, the example and `ludion docs dialogue`.
- **W2 voice:** `voice: voice/{locale}/x.wav` with default-locale fallback, duration from 0023's sim-side WAV header
  read plus `tail_ticks`, radio band-pass via `VoiceFilter`, world lines via `play_on`, ducking through the voice-bus
  `DuckRule`, suspend to `bus_pause`, `subtitle_range` against `AudioListener`, a mid-line restore offset or
  `Interrupted`, the built-in `SubtitleView` and its golden, `.dialogue_channels(...)`.
- **W3 localization:** `kind: locales` singleton, `UiText`/`Text` `key` + `set_text_args`, per-locale `fonts.yaml`
  overrides, fallback chains, `L1256` over string tables, an `fr` gallery scene.

## Out of scope (with triggers)

- Branching and conditions: never (Pillar 6; the game's Rust decides what to say).
- Choices: `UiList`. Lip sync: a portrait that needs more than `speaking()`.
- Plurals, gender, RTL, CJK: the `Plan/font-assets.md` triggers.

## Proposed — owner ruling owed

- The reading-time constants (decision 11).
- Localization pulled forward from `Plan/font-assets.md` Deferred into W3 (request amendment 1), and fallback chains
  (amendment 2, W3). `Plan/font-assets.md` is not edited here; this is the record.
- Subtitles on by default (W2); band-pass values (W2).
- The API shape (decision 2): free functions over `&mut World` (`dialogue::say`, `set_locale`, …) with state on hashed
  `DialogueLine`/`DialogueControl` entities, instead of the request's `Dialogue` and `Locale` resources, because
  resources reset on restore/checkpoint.

## Status

- W1 mechanized: `crates/ludion-engine/src/dialogue/`, `tests/dialogue.rs`, `crates/ludion/examples/dialogue.rs`.
  The validator takes the default locale from the builder's config (the build-time world has no config resource yet).
- W1 API follow-up: `say(world, id, speaker)` takes the speaker; `DialogueEvent::Started` carries `speaker` and `channel`.
