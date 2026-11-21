---
description: Localization: kind strings/locales, the active locale, keyed UiText/Text, fallbacks, args
---

# Localization (strings, locales, keyed text)

**Scope:** string tables (`kind: strings`), the shipped-locale list (`kind: locales`), the active locale and
`UiText`/`Text` with a `key`. Voiced and queued lines that use the same tables are `ludion docs dialogue`; faces and the
`font` field are `ludion docs fonts`; the UI components are `ludion docs components-ui`.

## The loop

1. `Engine::builder().localization()` opts in (`.dialogue(..)` implies it). It registers both doc kinds, the `LocaleState`
   component and the `localize_text` system, which runs where you called it in the system order.
2. Write the docs under the content root (below).
3. Put `key: hud.start` on a `UiText` or `Text`. Leave `text` as the authored fallback.
4. `localization::set_locale(world, "fr")` switches the language; keyed text re-resolves on the next tick.
5. For a placeholder, `localization::set_text_args(world, entity, &[("count", "12")])` after the system that knows the
   value.

## The docs

```yaml
format: 1
kind: strings
locale: en
strings:
  hud.start: Start game
  hud.ammo: "Ammo {count}"
```

One locale per doc; several docs may share a locale, and a key appears once per locale (`L1711`). `{name}` is a
placeholder; `{{` and `}}` are literal braces. A translation uses the default locale's placeholders (`L1716`, warning).
A doc has a `locale` (`L1718`).

```yaml
format: 1
kind: locales
default: en
locales:
  en:
    name: English
  fr:
    name: Francais
```

At most one `kind: locales` doc, and `default` must be one of the listed `locales` (`L1722`). With the doc, every
`kind: strings` locale must be listed (`L1723`). Without it the default is `en` and any tag is accepted. The default is
the fallback for every other locale and the one a dialogue line's keys must exist in (`L1715`).

## Keyed text

`UiText` and `Text` take `key` (a string-table key) and `args` (placeholder values; usually set through `set_text_args`).
The engine never writes `text`: it resolves into a runtime-only `resolved` field, and every consumer (layout, wrap,
hit test, bounds, draw) reads `shown()`, which is `resolved` when set, else `text`. So:

- A scene saved under `fr` is byte-identical to one saved under `en`, and `text` is what the editor shows.
- `resolved` is never serialized, hashed or snapshotted; `key`, `args` and the locale are. A restore resolves again
  before the first extract. Measurement is sim-visible: `measure_ui_text` over `shown()` is the French width in `fr`.
- Clearing `key` clears `resolved`; the authored `text` draws again.

Lookup is the active locale, then the default locale (`L5132`, once per locale and key), then the key itself (`L5133`,
once per key). A missing argument leaves `{name}` and warns `L5134` once. Text re-resolves every tick, so a strings doc
hot reload (`asset.reload`) shows on the next tick.

## The active locale

`LocaleState` is one lazily spawned entity, hashed and snapshotted, and skipped by `scene.save`: the locale is not saved into scenes. `localization::locale`
reads it (the default when unset). `set_locale` to a tag with no `kind: strings` doc still applies and warns `L5140`
once per tag.

## Gotchas

- A keyed text on an engine without `.localization()` draws its authored `text` and warns `L5139` once.
- `localization::text` and `format_text` resolve a key for game code (a toast, a log line); they use the active locale.
- Keys on `UiList` and `UiRadial` do not exist: resolve the string with `text` and write it to the item.
- Every locale draws with the face the component names. Per-locale face overrides and fallback chains do not exist, so a
  locale whose characters the face lacks draws `.notdef` glyphs (`L1256` reports them for authored text only).
- Plurals, gender, right-to-left and CJK shaping are not handled.
- Codes: `L1722` bad locales doc, `L1723` unlisted strings locale, `L5139`/`L5140` warnings.
- Run the example: `cargo run -p ludion --example dialogue`.
