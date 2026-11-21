---
description: Dialogue lines, string tables and subtitles: kind strings/dialogue, a line queue, events
---

# Dialogue (strings, a line queue, subtitles)

**Scope:** `kind: dialogue` (speakers and lines), the
deterministic line queue, voice (a WAV per line, played through the channel's audio) and the `Subtitles` your UI draws.
Strings, locales and keyed `UiText` are `ludion docs localization`. A drawn subtitle widget is not here. Holding gameplay time is `ludion docs game-clock`; the UI is `ludion docs screen-ui`. Playing lines on a timeline is `ludion docs sequences`.

## The loop

1. `Engine::builder().dialogue(DialogueConfig::default())`: opts in, and calls `.localization()` (`ludion docs
   localization`: the default locale comes from `kind: locales`, else `en`). `base_s`, `per_char_s`, `min_s` are the reading-time rule; `tail_ticks` (12) is how long a voiced line outlasts its clip. `build` refuses a
   bad config with `L1710`.
2. Write the docs under the content root (below).
3. Schedule `.system("dialogue_update", dialogue_update)` **after the systems that call `say`**, and declare its queries:
   `.query::<(&DialogueLine,)>().query::<(&DialogueControl,)>()`.
4. Talk with the free functions in `ludion::dialogue` over `&mut World`; read `DialogueEvents` and `Subtitles` after
   the system.

## The docs

```yaml
format: 1
kind: strings
locale: en
strings:
  speaker.operator: Operator
  radio.intro: "Operator to Neon, do you copy?"
  radio.hit: "Lost {count} drones."
```

The `kind: strings` doc and its rules (`L1711`, `L1716`, `L1718`) are in `ludion docs localization`.

```yaml
format: 1
kind: dialogue
speakers:
  operator:
    name: speaker.operator
    channel: radio
lines:
  intro:
    speaker: operator
    text: radio.intro
    priority: high
    holds_gameplay: true
    cues:
      static: 30
```

| Line field | Meaning | Default |
| --- | --- | --- |
| `speaker`, `text` | a speaker id; a string key | required |
| `priority` | `low`, `normal`, `high`, `critical` | `normal` |
| `interrupt` | cut the slot's line: `none`, `lower` (strictly lower), `any` | `none` |
| `holds_gameplay` | hold the `GameClock` while playing | `false` |
| `when_blocked` | `wait` or `drop` on a blocked channel | `wait` |
| `max_wait_ticks` | drop after waiting that many ticks | unbounded |
| `duration_ticks` | authored length, at least 1 | the reading rule |
| `voice` | a WAV under the content root; `{locale}` is the active locale | silent |
| `tail_ticks` | ticks the line outlasts its voice | the config's |
| `cues` | name to tick after the start | none |

A speaker has `name` (a string key), `channel` (`radio`, `world`, `ui`), optional `portrait` (a string for your UI)
and `color` (RGBA). Ids are unique across all dialogue docs (`L1712`, `L1713`); a line names a defined speaker
(`L1714`); every `text` and speaker `name` key exists in the default locale (`L1715`); a translation uses the
`duration_ticks` is not 0 and a cue is before the end (`L1717`); a `voice` reads for the default locale (`L1720`) and, with `{locale}`, for every other
locale that has a strings doc (`L1721`, warning). An unknown field is `L1120`.

## The API

`say(world, id, speaker)`, `say_with(world, id, speaker, &[("name", "value")])`, `say_sequence(world, &ids, &speakers)`,
`interrupt(world, handle)`, `interrupt_speaker(world, entity)`, `block_channel(world, channel, bool)`,
`suspend(world, bool)`, `speaking(world, speaker_id)`, `holding`, `line_state`. The locale is
`ludion::localization::set_locale`. Errors are `L5130` (unknown line), `L5131` (a `world` line with no
live speaker), `L5135` (bad sequence), `L5136` (not opted in).

**Gotcha:** the API only marks state; `dialogue_update` emits every event. A line said after the system in a tick
starts on the next tick.

## Slots, order, interruption

Slots are one `radio`, one `ui`, and one `world` slot per speaker entity. The slot's queued lines start by `(priority
descending, then enqueue order)`. A head line cuts the playing one per `interrupt`; the cut line ends `Interrupted`.
A sequence starts each line the tick the last one completes; any other end of a line drops the rest.

A blocked channel ends its playing line `Blocked`, ends a queued `drop` line `Blocked`, and keeps a `wait` line
queued. `suspend` freezes elapsed time, waiting and starts; cancels and a holding line's hold still apply. A dead speaker
entity ends its playing line `Interrupted` and its queued ones `Dropped`.

## Time and holds

A line started on frame F lasting D ticks is shown on frames F to F+D-1 and ends `Completed` on F+D
(`Subtitle::end_frame` is F+D, exclusive). With no `duration_ticks` it lasts `max(min_s, base_s + per_char_s * chars)`
seconds of the formatted active-locale text (defaults 1.0, 0.06, 1.5), fixed at the start. A suspended line's
`end_frame` is not extended.

A holding line calls `GameClock::hold()` on F..F+D-1, so `ctx.game_advanced()` is false on F+1 through F+D. The
dialogue itself runs on the sim clock, so it advances through its own hold. While a hold is on, a queued line starts
only if it is `critical` or holds itself. Physics is not frozen by it.

## Voice

A line with `voice: voice/{locale}/intro.wav` plays that WAV when it starts and lasts the clip plus a tail:
`ceil(frames * tick_hz / sample_rate) + tail_ticks`, with `tail_ticks` the line's, else the config's (12). An authored
`duration_ticks` still wins. `{locale}` becomes the active locale at the start; a file that does not read falls back
to the default locale's (`L5137`, once per path), and one that reads nowhere leaves the line silent for its reading
time (`L5138`, once). A path without `{locale}` is used as is. Lengths are read once per path.

`.dialogue_channels(DialogueChannels { .. })` sets how each channel sounds (`ChannelAudio { category, filter, gain,
range, cut_fade_ticks }`; `DialogueChannels::default()` is the table below). `build` refuses a bad table, or one
without `.dialogue`, with `L1719`.

| Channel | Bus | Filter | How it plays |
| --- | --- | --- | --- |
| `radio` | `Voice` | band-pass 300 Hz to 3.4 kHz | a global voice |
| `world` | `Voice` | none | from the speaker entity, range 20 m, following it (`push_on`) |
| `ui` | `Ui` | none | a global voice |

- **A line that ends any way but `Completed` fades its voice** over the channel's `cut_fade_ticks` (6).
- **`suspend(world, true)` pauses `suspend_buses`** (default `[Voice]`, faded over 6 ticks) once while a voiced line is
  on air; `suspend(world, false)` resumes them. A pause menu's own `Ui` sounds stay live.
- **Ducking is yours:** `.audio_duck(DuckRule { when: Bus::Voice, .. })` dips music under every voiced line
  (`ludion docs audio-mix`); dialogue adds nothing.
- **A restored mid-line resumes its voice.** The line remembers the file it started with (so a locale change since does
  not swap it), and the first update after a restore or checkpoint restarts that voice at the line's elapsed tick; a line
  already inside its tail restarts nothing. If the game is suspended, the suspend buses are paused first.

## Events and subtitles

`DialogueEvents` (rewritten each tick): `Started { handle, line, speaker, channel }`, `Ended { reason }` (`Completed`, `Interrupted`, `Blocked`,
`Dropped`) and `Cue { name }`, plus `started(handle)` and `ended(handle)`. Order within a tick: ends by enqueue order, starts by
slot, cues by enqueue order. `Subtitles` lists what is on air: `handle`, `line`, `speaker_id`, `speaker` (the display
name), `color`, `text` (resolved and placeholder-filled), `channel`, `portrait`, `start_frame`, `end_frame`. Drawing it is
the game's job today (`UiText`).

## Strings

Lookup, fallbacks and `{name}` placeholders are `ludion docs localization`. A line's text re-resolves every tick, so
`localization::set_locale` changes a live subtitle without changing its length.

## Gotchas

- State is entities (`DialogueLine`, `DialogueControl`; the locale is `LocaleState`), so it rides snapshots, checkpoints and the `WorldHash`; the
  events, subtitles and string index are derived resources. A game that never speaks hashes as if it had not opted in.
- A district switch drops lines unless the game carries them.
- Codes: `L1719` bad channel table, `L1720` voice missing for the default locale, `L1721` (warning) for another,
  `L5137` (warning) fell back, `L5138` (warning) unreadable.
- Run the example: `cargo run -p ludion --example dialogue`.
